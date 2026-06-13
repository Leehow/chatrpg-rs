use anyhow::{anyhow, Result};
use rand::Rng;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, HashSet};
use trpg_agent::{contract_block, looks_like_new_action_or_abandon, make_pending_check, parse_roll_text, GmAgent, ParsedRollText};
use trpg_ability::{AbilityService, AbilityTurnInput, AbilityTurnResult};
use trpg_semantics::{SemanticIntentService, SemanticRuleBindingService};
use trpg_combat::{CombatAgent, ConflictTurnInput, ConflictTurnResult};
use trpg_contest::ContestService;
use trpg_director::{ActionableSituationDirector, DirectorInput};
use trpg_interaction::InteractionLifecycleKernel;
use trpg_material::{MaterializationService, MaterializationTurnInput, MaterializationTurnResult};
use trpg_mechanics::{insert_roll_plan, make_roll_plan_from_check, RefereeCombatService, FollowupCheck};
use trpg_object::{ObjectService, ObjectTurnInput, ObjectTurnResult};
use trpg_orchestrator::{TurnOrchestrator, TurnOrchestratorInput, TurnOrchestrationResult};
use trpg_params::RuntimeParameterService;
use trpg_db::Db;
use trpg_llm::{system, user};
use trpg_model::*;
use trpg_referee::PlayerValueRefereeService;
use trpg_search::SearchService;
use trpg_time::WorldTimeService;
use uuid::Uuid;

mod chargen;
pub use chargen::{generate_starter_character, materialize_actor_params, CreatedCharacter};

pub mod npc_synth;

#[derive(Clone)]
pub struct RuntimeEngine {
    pub db: Db,
    pub search: Option<SearchService>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoRollExecution {
    pub primary: CheckResultRecord,
    pub followups: Vec<CheckResultRecord>,
    pub roll_policy: String,
}

impl RuntimeEngine {
    pub fn new(db: Db) -> Self {
        warn_if_core_mechanics_disabled();
        Self { db, search: None }
    }

    pub fn with_search(mut self, search: SearchService) -> Self {
        self.search = Some(search);
        self
    }

    /// Runtime rule retrieval over the PARSED rules (tantivy multi-domain). The
    /// agentic GM calls this via the `retrieve` tool to fetch a specific rule
    /// (surprise, stealth, a maneuver) instead of inventing one. Returns top-k
    /// rule snippets with page refs, or an explicit "no rule found" marker.
    pub async fn retrieve_rules(&self, ruleset_id: &str, query: &str, k: usize) -> String {
        let Some(search) = &self.search else {
            return "[retrieve unavailable: no search service]".to_string();
        };
        let mut scopes = BTreeMap::new();
        scopes.insert("ruleset_id".to_string(), ruleset_id.to_string());
        let req = SearchRequest {
            query: query.to_string(),
            mode: SearchMode::Auto,
            domains: vec!["rules".into(), "source".into(), "parsed".into(), "modules".into(), "rulings".into(), "learned".into()],
            scopes,
            limit: k as u32,
            explain: false,
            viewer: VisibilityProfile::gm(),
            rewrite_query: true,
            intent: Some("gm_runtime_retrieve".into()),
            ..Default::default()
        };
        match search.search_async(&req).await {
            Ok(resp) if !resp.hits.is_empty() => resp
                .hits
                .iter()
                .take(k)
                .map(|h| {
                    let pg = h.source_refs.first().and_then(|r| r.page).map(|p| format!("p{p} ")).unwrap_or_default();
                    let title: String = h.title.chars().take(60).collect();
                    let snip: String = h.snippet.chars().take(400).collect();
                    format!("- [{pg}{title}] {snip}")
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Ok(_) => format!("[no source-backed rule found for query: {query}]"),
            Err(err) => format!("[retrieve error: {err}]"),
        }
    }

    pub async fn start_session(&self, ruleset_id: &str, module_id: Option<&str>) -> Result<String> {
        if env_bool_runtime("TRPG_CHARACTER_ONBOARDING_REQUIRED", true) {
            let mut missing = Vec::new();
            if self.db.load_rule_kernel(ruleset_id).await?.is_none() {
                missing.push("RuleKernel");
            }
            let character_pack = self.db.load_character_onboarding_pack(ruleset_id).await?;
            match character_pack.as_ref() {
                Some(pack) => {
                    if pack.sheet_template.fields.is_empty() { missing.push("CharacterSheetTemplate.fields"); }
                    if pack.creation_flows.iter().all(|flow| flow.steps.is_empty()) { missing.push("CharacterCreationFlow.steps"); }
                    if pack.runtime_bindings.is_empty() { missing.push("CharacterRuntimeBinding"); }
                    if pack.starter_character_pack.pregens.is_empty()
                        && pack.starter_character_pack.archetypes.is_empty()
                        && pack.starter_character_pack.creation_shortcuts.is_empty() {
                        missing.push("StarterCharacterPack");
                    }
                }
                None => missing.push("CharacterOnboardingPack"),
            }
            if env_bool_runtime("TRPG_PLAYABILITY_GATE_BLOCKING", false) {
                if let Some(module_id) = module_id {
                    if !self.db.has_module_first_session_packet(module_id).await.unwrap_or(false) {
                        missing.push("ModuleFirstSessionPacket");
                    }
                }
            }
            if !missing.is_empty() {
                return Err(anyhow!(
                    "ruleset {ruleset_id} is not mechanically startable yet; missing {}. Run parse-all, then `trpg rules playability --ruleset {ruleset_id}{}` and repair the reported gaps before starting play.",
                    missing.join(", "),
                    module_id.map(|m| format!(" --module {m}")).unwrap_or_default()
                ));
            }
        }

        let session_id = format!("session_{}", Uuid::new_v4().simple());
        self.db.create_session(&session_id, ruleset_id, module_id).await?;
        let _ = WorldTimeService::new(self.db.clone()).ensure_session_time(&session_id, Some(&session_id)).await;
        let _ = self.db.ensure_interaction_generation(&session_id).await;
        if let Some(mid) = module_id {
            // fail-closed：任何失败仅 warn，不阻断开局；但必须可见——曾有空图谱静默
            // 跳过激活，导致整局 current_scene_id=NULL 而无任何线索。
            match self.db.load_module_graph(mid).await {
                Ok(Some(graph)) => match module_entry_scene_id(&graph) {
                    Some(entry) => { let _ = self.db.set_session_scene(&session_id, &entry).await; }
                    None => tracing::warn!(module_id = mid, "module graph has no scenes; entry scene not activated — re-run parse-all with the module reader enabled"),
                },
                Ok(None) => tracing::warn!(module_id = mid, "no module graph found; entry scene not activated — run parse-all for this module first"),
                Err(err) => tracing::warn!(error = %err, module_id = mid, "load_module_graph failed; entry scene not activated"),
            }
        }
        Ok(session_id)
    }

    pub async fn current_world_time(&self, session_id: &str) -> Result<WorldTimeState> {
        WorldTimeService::new(self.db.clone()).current(session_id).await
    }

    pub async fn advance_world_time(&self, request: TimeAdvanceRequest) -> Result<TimeAdvanceResult> {
        WorldTimeService::new(self.db.clone()).advance(request).await
    }

    pub async fn record_world_event(&self, session_id: &str, turn_id: Option<&str>, frame_id: Option<&str>, kind: WorldEventKind, event_json: serde_json::Value, visibility: Visibility) -> Result<WorldEvent> {
        WorldTimeService::new(self.db.clone()).record_event(session_id, turn_id, frame_id, kind, event_json, visibility).await
    }

    pub async fn prepare_turn_context(
        &self,
        request: &ContextRequest,
        state: &RuntimeState,
        current_input: Option<&str>,
        recent_transcript: Option<&str>,
    ) -> Result<CompiledContext> {
        // 每回合单点把持久化的 current_scene_id 载入运行态（P4 投影）：state 缺场景且有模组时填入。
        let mut state_owned = state.clone();
        if resolve_turn_scene_id(state_owned.scene_id.as_deref(), state_owned.module_id.as_deref(), None).is_none()
            && state_owned.module_id.is_some()
        {
            let loaded = self.db.load_session_scene(&request.session_id).await.ok().flatten();
            state_owned.scene_id = resolve_turn_scene_id(state_owned.scene_id.as_deref(), state_owned.module_id.as_deref(), loaded);
        }
        let state = &state_owned;
        let _ = InteractionLifecycleKernel::new(self.db.clone()).reconcile_session(&request.session_id).await;
        // Load the project bundle FOR THIS RULESET (several rulesets may share a
        // DB; the latest-parsed one is not necessarily the one being played).
        let project = match self.db.load_project_bundle_for_ruleset(&request.ruleset_id).await? {
            Some(p) => p,
            None => self.db.load_latest_project_bundle().await?.ok_or_else(|| anyhow!("no parsed project bundle found; run parse-all first"))?,
        };
        let mut bundle_ids = Vec::new();
        for ruleset in &project.rulesets {
            if ruleset.ruleset_id == request.ruleset_id {
                bundle_ids.push(ruleset.bundle_id.clone());
            }
        }
        if let Some(module_id) = request.module_id.as_ref().or(state.module_id.as_ref()) {
            for module in &project.modules {
                if &module.module_id == module_id {
                    bundle_ids.push(module.bundle_id.clone());
                }
            }
        }
        if bundle_ids.is_empty() {
            return Err(anyhow!("no bundle found for ruleset={} module={:?}", request.ruleset_id, request.module_id));
        }

        let mut blocks = self.db.list_context_blocks_for_bundles(&bundle_ids).await?;
        let material_refs: Vec<String> = state.pending_material_refs.iter()
            .chain(state.active_material_refs.iter())
            .cloned()
            .collect();
        let mut exact = self.db.find_material_blocks(&bundle_ids, &material_refs).await?;
        for b in &mut exact {
            if b.cache_zone == CacheZone::NeverPrompt {
                b.cache_zone = CacheZone::DynamicTail;
            }
            b.load_reason = Some("pending_or_active_material_ref".to_string());
        }
        blocks.extend(exact);

        let _ = self.db.deactivate_runtime_turn_blocks(&request.session_id, &request.turn_id).await;
        match self.db.list_runtime_context_blocks(&request.session_id, &request.turn_id, state.scene_id.as_deref()).await {
            Ok(mut runtime_loaded_blocks) => blocks.append(&mut runtime_loaded_blocks),
            Err(err) => tracing::warn!(error = %err, "runtime-loaded search blocks failed; continuing"),
        }

        if let Some(input) = current_input {
            match self.auto_search_blocks_for_turn(request, state, input).await {
                Ok(mut search_blocks) => blocks.append(&mut search_blocks),
                Err(err) => tracing::warn!(error = %err, "auto rule/module search failed; continuing without search blocks"),
            }
        }

        match self.memory_blocks_for_turn(request, state, current_input).await {
            Ok(mut memory_blocks) => blocks.append(&mut memory_blocks),
            Err(err) => tracing::warn!(error = %err, "memory retrieval failed; continuing without memory blocks"),
        }
        match self.state_frame_blocks_for_turn(request).await {
            Ok(mut frame_blocks) => blocks.append(&mut frame_blocks),
            Err(err) => tracing::warn!(error = %err, "working state frame retrieval failed; continuing without frame blocks"),
        }
        match self.module_scene_blocks_for_turn(request, state, &project).await {
            Ok(mut scene_blocks) => blocks.append(&mut scene_blocks),
            Err(err) => tracing::warn!(error = %err, "module_scene_blocks_for_turn failed; continuing without current-scene projection"),
        }
        match self.world_time_blocks_for_turn(request).await {
            Ok(mut time_blocks) => blocks.append(&mut time_blocks),
            Err(err) => tracing::warn!(error = %err, "world time retrieval failed; continuing without world time blocks"),
        }
        match self.object_blocks_for_turn(request).await {
            Ok(mut object_blocks) => blocks.append(&mut object_blocks),
            Err(err) => tracing::warn!(error = %err, "object graph projection failed; continuing without object blocks"),
        }
        match self.actor_parameter_blocks_for_turn(request, current_input).await {
            Ok(mut actor_blocks) => blocks.append(&mut actor_blocks),
            Err(err) => tracing::warn!(error = %err, "actor parameter projection failed; continuing without actor parameter blocks"),
        }
        match self.ability_blocks_for_turn(request).await {
            Ok(mut ability_blocks) => blocks.append(&mut ability_blocks),
            Err(err) => tracing::warn!(error = %err, "ability graph projection failed; continuing without ability blocks"),
        }
        match self.rule_binding_blocks_for_turn(request).await {
            Ok(mut binding_blocks) => blocks.append(&mut binding_blocks),
            Err(err) => tracing::warn!(error = %err, "rule binding projection failed; continuing without binding blocks"),
        }
        match self.materialization_blocks_for_turn(request).await {
            Ok(mut materialization_blocks) => blocks.append(&mut materialization_blocks),
            Err(err) => tracing::warn!(error = %err, "materialization projection failed; continuing without materialization blocks"),
        }
        match self.player_value_referee_blocks_for_turn(request).await {
            Ok(mut referee_blocks) => blocks.append(&mut referee_blocks),
            Err(err) => tracing::warn!(error = %err, "player value referee projection failed; continuing without referee blocks"),
        }
        match self.referee_combat_blocks_for_turn(request).await {
            Ok(mut mech_blocks) => blocks.append(&mut mech_blocks),
            Err(err) => tracing::warn!(error = %err, "referee combat ledger projection failed; continuing without mechanical ledger blocks"),
        }
        match self.contest_blocks_for_turn(request).await {
            Ok(mut contest_blocks) => blocks.append(&mut contest_blocks),
            Err(err) => tracing::warn!(error = %err, "contest/opposition projection failed; continuing without contest blocks"),
        }
        match self.learned_packet_blocks_for_turn(request, state).await {
            Ok(mut learned_blocks) => blocks.append(&mut learned_blocks),
            Err(err) => tracing::warn!(error = %err, "learned packet retrieval failed; continuing without learned packets"),
        }
        match self.rule_steward_prefix_blocks_for_turn(request).await {
            Ok(mut steward_blocks) => blocks.append(&mut steward_blocks),
            Err(err) => tracing::warn!(error = %err, "rule steward BP1 projection failed; continuing without active kernel blocks"),
        }

        blocks.push(if state.agent_loop_protocol { engine_protocol_block_agent_loop() } else { engine_protocol_block() });
        blocks.push(world_state_block(state));
        if let Some(transcript) = recent_transcript {
            blocks.push(dynamic_text_block(
                "runtime.recent_transcript",
                BlockKind::RecentTranscript,
                "Recent Transcript",
                transcript,
                vec!["recent_transcript"],
            ));
        }
        if let Some(input) = current_input {
            blocks.push(dynamic_text_block(
                "runtime.current_input",
                BlockKind::CurrentInput,
                "Current Player Input",
                input,
                vec!["current_input"],
            ));
        }

        dedupe_blocks(&mut blocks);
        let visible: Vec<ContextBlock> = blocks.into_iter()
            .filter_map(|b| project_visibility(b, &request.viewer))
            .collect();
        let planned = plan_blocks(visible, state, request);
        let compiled = ContextBuilder::default().build(planned, request)?;
        for block in compiled.prefix_blocks.iter().chain(compiled.pinned_blocks.iter()).chain(compiled.dynamic_blocks.iter()) {
            let _ = self.db.record_load_event(Some(&request.session_id), Some(&request.turn_id), block, block.load_reason.as_deref().unwrap_or("planner")).await;
        }
        if let Ok(world_time) = self.current_world_time(&request.session_id).await {
            let watermark = ContextWatermark {
                session_id: request.session_id.clone(),
                last_compiled_world_tick: world_time.world_tick,
                last_compiled_event_seq: world_time.event_seq,
                compiled_context_hash: Some(compiled.cache_key.clone()),
                updated_at: chrono::Utc::now(),
            };
            let _ = self.db.upsert_context_watermark(&watermark).await;
        }
        Ok(compiled)
    }



    /// Rust-owned GM Agent planning step. The policy/advice content is loaded
    /// from JSON advice layers and is not mixed into the LLM system prompt.
    pub async fn plan_agent_turn(
        &self,
        request: &ContextRequest,
        state: &RuntimeState,
        user_input: &str,
    ) -> Result<AgentTurnPlan> {
        let agent = GmAgent::from_env_or_default();
        let actor_id = request.viewer.actor_id.as_deref().or(Some("pc.current"));
        let plan = agent.plan_turn(trpg_agent::AgentTurnInput {
            session_id: &request.session_id,
            turn_id: &request.turn_id,
            ruleset_id: &request.ruleset_id,
            module_id: request.module_id.as_deref().or(state.module_id.as_deref()),
            actor_id,
            user_input,
        });
        Ok(plan)
    }

    pub async fn persist_agent_plan(&self, plan: &AgentTurnPlan) -> Result<()> {
        self.db.insert_agent_turn(plan, "planned").await?;
        let _ = self.db.upsert_runtime_context_block(&plan.session_id, &agent_plan_block(plan)).await;
        if let Some(check) = &plan.check {
            self.db.insert_check_contract(check, "created").await?;
            if matches!(plan.kind, TurnPlanKind::AskPlayerRoll) {
                self.db.cancel_open_pending_checks_for_session(&plan.session_id, PendingCheckStatus::Superseded).await.ok();
                let pending = make_pending_check(check);
                self.db.insert_pending_check(&pending).await?;
                let gate = InteractionGate::from_pending_check(&pending);
                self.db.insert_interaction_gate(&gate).await?;
                if let Some(frame_id) = pending.owner_frame_id.as_deref() {
                    let kernel = InteractionLifecycleKernel::new(self.db.clone());
                    kernel.attach_gate_to_active_frame(&plan.session_id, &gate.gate_id, Some(frame_id)).await.ok();
                    kernel.attach_pending_check_to_frame(&plan.session_id, &pending.check_id, frame_id, Some(&gate.gate_id)).await.ok();
                }
            }
            if let Some(block) = contract_block(plan) {
                let _ = self.db.upsert_runtime_context_block(&plan.session_id, &block).await;
            }
        }
        Ok(())
    }

    /// Single post-resolution funnel for EVERY check-resolution path. Runs the
    /// universal referee combat hook (resource tracks, HP, effect/damage
    /// follow-ups) AND generic object rule effects (e.g. a firearm spending a
    /// round on attack). Centralizing here is why object effects can't miss the
    /// auto-roll combat route the way the old per-path hooks did.
    async fn post_check_resolution(&self, contract: &CheckContract, result: &mut CheckResultRecord) -> Result<Option<FollowupCheck>> {
        let followup = RefereeCombatService::new(self.db.clone()).after_check_resolved(contract, result).await?;
        let _ = ObjectService::new(self.db.clone()).apply_object_rule_effects(&contract.session_id, contract).await;
        Ok(followup)
    }

    pub async fn try_resolve_pending_check(
        &self,
        session_id: &str,
        turn_id: &str,
        user_input: &str,
    ) -> Result<Option<CheckResultRecord>> {
        let Some(pending) = self.db.get_open_pending_check(session_id).await? else { return Ok(None); };
        if !roll_input_available(user_input) {
            return Ok(None);
        }
        let roll = self.resolve_roll_input(session_id, turn_id, Some(&pending.check_id), &pending.contract, user_input).await?;
        let outcome = self.resolve_outcome_with_opposition(&pending.contract, &roll).await?;
        let result = CheckResultRecord {
            check_id: pending.check_id.clone(),
            roll,
            outcome,
            committed_patches: vec![],
            created_at: chrono::Utc::now(),
        };
        let mut result = result;
        if let Ok(Some(object_result)) = ObjectService::new(self.db.clone()).apply_for_check_result(&result).await {
            result.committed_patches.push(StatePatch::ObjectPatch { patch_id: format!("object_result_{}", object_result.interaction_id), object_id: object_result.applied_patches.first().and_then(|p| match p { ObjectPatch::TransferObject { object_id, .. } | ObjectPatch::SetObjectLocation { object_id, .. } | ObjectPatch::SetObjectVisibility { object_id, .. } | ObjectPatch::ModifyQuantity { object_id, .. } | ObjectPatch::DamageObject { object_id, .. } | ObjectPatch::DestroyObject { object_id, .. } | ObjectPatch::SetMechanicalState { object_id, .. } | ObjectPatch::TransformObject { object_id, .. } => Some(object_id.clone()), ObjectPatch::CreateObjectInstance { object, .. } => Some(object.object_id.clone()), ObjectPatch::AddObjectEdge { edge, .. } => Some(edge.from_object_id.clone()), ObjectPatch::RemoveObjectEdge { edge_id, .. } => Some(edge_id.clone()) }), patch_json: serde_json::to_value(&object_result).unwrap_or_else(|_| json!({})), reason: "object_interaction_result".into() });
        }
        let _ = self.post_check_resolution(&pending.contract, &mut result).await;
        self.db.insert_check_result(&result).await?;
        self.db.update_pending_check_status(&pending.check_id, PendingCheckStatus::Resolved).await?;
        self.close_resolved_check_gate(session_id, &pending.check_id).await;
        Ok(Some(result))
    }

    /// Handle the currently open interaction gate, if any. This is stronger than
    /// the v0.9 `try_resolve_pending_check`: it resolves natural-language roll
    /// replies, explicitly re-prompts ambiguous replies, and closes abandoned
    /// gates so stale rolls cannot resolve old checks later.
    pub async fn handle_open_interaction_gate(
        &self,
        session_id: &str,
        turn_id: &str,
        user_input: &str,
    ) -> Result<GateHandlingResult> {
        let lifecycle = InteractionLifecycleKernel::new(self.db.clone());
        let _ = lifecycle.reconcile_session(session_id).await;
        let open_gate = self.db.get_open_interaction_gate(session_id).await?;
        if let Some(gate) = open_gate.as_ref() {
            if !roll_input_available(user_input)
                && lifecycle.player_input_supersedes_gate(gate, user_input)
            {
                let _ = lifecycle.supersede_gate_for_terminal_intent(gate, "superseded_by_terminal_intent", user_input).await;
                if let Some(check_id) = match &gate.expected_input { ExpectedInput::RollResult { check_id, .. } => Some(check_id.clone()), _ => None } {
                    let _ = self.db.supersede_pending_check(&check_id, "superseded_by_terminal_intent", None).await;
                }
                return Ok(GateHandlingResult::GateSuperseded { gate_id: gate.gate_id.clone(), reason: "superseded_by_terminal_intent".into(), prompt_public: "[system]上一个交互窗口已关闭，继续处理你的新动作。[/system]".into() });
            }
        }
        let Some(pending) = self.db.get_open_pending_check(session_id).await? else {
            // v1.12.2: a `/roll` or natural roll result must bind to the latest
            // unresolved mechanical check before any advisory direction gate can
            // consume it. This turns `/roll` from an orphan RNG call into a
            // resolution of the active attack/effect/check whenever possible.
            if roll_input_available(user_input) {
                if let Some(contract) = self.db.get_latest_unresolved_check_contract(session_id).await? {
                    let result = self.resolve_check_with_input(session_id, turn_id, &contract, user_input).await?;
                    if let Some(next_pending) = self.db.get_open_pending_check(session_id).await? {
                        if next_pending.check_id != contract.check_id && is_effect_or_damage_contract(&next_pending.contract) {
                            let prompt_public = next_pending.prompt_public.clone();
                            return Ok(GateHandlingResult::PendingFollowupCheckCreated {
                                result,
                                pending: Box::new(next_pending),
                                prompt_public,
                                reason: "awaiting_effect_roll".into(),
                            });
                        }
                    }
                    return Ok(GateHandlingResult::PendingCheckResolved { result });
                }
            }
            if let Some(gate) = open_gate.clone() {
                if lifecycle.player_input_supersedes_gate(&gate, user_input) {
                    let _ = lifecycle.supersede_gate_for_terminal_intent(&gate, SupersededReason::TerminalIntent.as_str(), user_input).await;
                    return Ok(GateHandlingResult::GateSuperseded { gate_id: gate.gate_id.clone(), reason: SupersededReason::TerminalIntent.as_str().into(), prompt_public: "[system]上一个交互窗口已关闭，继续处理你的新动作。[/system]".into() });
                }
                return self.handle_choice_or_nonroll_gate(gate, user_input).await;
            }
            return Ok(GateHandlingResult::None);
        };
        let gate = open_gate.unwrap_or_else(|| InteractionGate::from_pending_check(&pending));
        if lifecycle.player_input_supersedes_gate(&gate, user_input) {
            self.db.update_pending_check_status(&pending.check_id, PendingCheckStatus::Superseded).await.ok();
            let _ = lifecycle.supersede_gate_for_terminal_intent(&gate, SupersededReason::TerminalIntent.as_str(), user_input).await;
            return Ok(GateHandlingResult::GateSuperseded { gate_id: gate.gate_id.clone(), reason: SupersededReason::TerminalIntent.as_str().into(), prompt_public: "[system]上一个检定/反应窗口已关闭，继续处理你的新动作。[/system]".into() });
        }

        if roll_input_available(user_input) {
            let roll = self.resolve_roll_input(session_id, turn_id, Some(&pending.check_id), &pending.contract, user_input).await?;
            let outcome = self.resolve_outcome_with_opposition(&pending.contract, &roll).await?;
            let result = CheckResultRecord {
                check_id: pending.check_id.clone(),
                roll,
                outcome,
                committed_patches: vec![],
                created_at: chrono::Utc::now(),
            };
            let mut result = result;
            if let Ok(Some(object_result)) = ObjectService::new(self.db.clone()).apply_for_check_result(&result).await {
                result.committed_patches.push(StatePatch::ObjectPatch { patch_id: format!("object_result_{}", object_result.interaction_id), object_id: object_result.applied_patches.first().and_then(|p| match p { ObjectPatch::TransferObject { object_id, .. } | ObjectPatch::SetObjectLocation { object_id, .. } | ObjectPatch::SetObjectVisibility { object_id, .. } | ObjectPatch::ModifyQuantity { object_id, .. } | ObjectPatch::DamageObject { object_id, .. } | ObjectPatch::DestroyObject { object_id, .. } | ObjectPatch::SetMechanicalState { object_id, .. } | ObjectPatch::TransformObject { object_id, .. } => Some(object_id.clone()), ObjectPatch::CreateObjectInstance { object, .. } => Some(object.object_id.clone()), ObjectPatch::AddObjectEdge { edge, .. } => Some(edge.from_object_id.clone()), ObjectPatch::RemoveObjectEdge { edge_id, .. } => Some(edge_id.clone()) }), patch_json: serde_json::to_value(&object_result).unwrap_or_else(|_| json!({})), reason: "object_interaction_result".into() });
            }
            if let Ok(Some(followup)) = self.post_check_resolution(&pending.contract, &mut result).await {
                self.db.insert_check_result(&result).await?;
                self.db.update_pending_check_status(&pending.check_id, PendingCheckStatus::Resolved).await?;
                self.db.update_interaction_gate_status(&gate.gate_id, GateStatus::Resolved, json!({"check_id": pending.check_id, "result": result.clone(), "followup": "damage_roll"})).await.ok();
                return Ok(GateHandlingResult::PendingFollowupCheckCreated { result, pending: Box::new(followup.pending), prompt_public: followup.prompt_public, reason: followup.reason });
            }
            self.db.insert_check_result(&result).await?;
            self.db.update_pending_check_status(&pending.check_id, PendingCheckStatus::Resolved).await?;
            self.db.update_interaction_gate_status(&gate.gate_id, GateStatus::Resolved, json!({"check_id": pending.check_id, "result": result.clone()})).await.ok();
            return Ok(GateHandlingResult::PendingCheckResolved { result });
        }

        let should_reprompt = looks_like_gate_help_or_question(user_input)
            || (matches!(gate.on_unparseable, GateFallbackPolicy::Reprompt | GateFallbackPolicy::RequireExplicitChoice)
                && !looks_like_new_action_or_abandon(user_input));
        if should_reprompt {
            let prompt = format!(
                "[system]上一项检定仍在等待投骰授权。请只回复 `roll`，系统会调用骰子工具；如果要取消，请直接描述新的角色行动。[/system]\n\n{}",
                gate.prompt_public,
            );
            self.db.update_interaction_gate_status(&gate.gate_id, GateStatus::Open, json!({"last_unparseable_input": user_input, "reason": "unparseable_roll_reply"})).await.ok();
            return Ok(GateHandlingResult::GateReprompt { gate_id: gate.gate_id, check_id: pending.check_id, reason: "unparseable_roll_reply".into(), prompt_public: prompt });
        }

        self.db.update_pending_check_status(&pending.check_id, PendingCheckStatus::AbandonedByNewAction).await?;
        self.db.update_interaction_gate_status(&gate.gate_id, GateStatus::AbandonedByNewAction, json!({"reason": "player_started_new_action", "new_input": user_input})).await.ok();
        let prompt = "[system]上一个未完成检定已关闭，继续处理你的新动作。[/system]".to_string();
        Ok(GateHandlingResult::GateAbandoned { gate_id: gate.gate_id, check_id: pending.check_id, reason: "player_started_new_action".into(), prompt_public: prompt })
    }



    pub async fn orchestrate_turn(
        &self,
        request: &ContextRequest,
        _state: &RuntimeState,
        user_input: &str,
    ) -> Result<TurnOrchestrationResult> {
        if !turn_orchestrator_enabled() {
            // When disabled, mimic the pre-v1.7 path: no gate-first override and generic agent can still run.
            let orchestrator = TurnOrchestrator::new(self.db.clone());
            return orchestrator.reduce_turn(TurnOrchestratorInput {
                session_id: &request.session_id,
                turn_id: &request.turn_id,
                ruleset_id: &request.ruleset_id,
                module_id: request.module_id.as_deref(),
                user_input,
            }).await;
        }
        TurnOrchestrator::new(self.db.clone()).reduce_turn(TurnOrchestratorInput {
            session_id: &request.session_id,
            turn_id: &request.turn_id,
            ruleset_id: &request.ruleset_id,
            module_id: request.module_id.as_deref(),
            user_input,
        }).await
    }


    pub async fn forced_technical_assessment_plan(&self, request: &ContextRequest, user_input: &str) -> AgentTurnPlan {
        // Thin wrapper: load the ruleset's core die from the parsed kernel, then
        // delegate to the pure (DB-less, unit-tested) builder. No hardcoded die.
        let kernel = self.db.load_rule_kernel(&request.ruleset_id).await.ok().flatten();
        let dice = kernel.as_ref().and_then(|k| {
            k.dice_core.get("dice").and_then(|v| v.as_str())
                .or_else(|| k.check_model.get("dice").and_then(|v| v.as_str()))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        });
        build_forced_tech_plan(request, user_input, dice.as_deref())
    }

    /// Plan for the standalone-check route. When the semantic layer named a
    /// specific check parameter (`named_label`), build an explicit named check
    /// bound to it; otherwise fall back to the generic forced technical
    /// assessment. Same thin-wrapper shape as `forced_technical_assessment_plan`:
    /// load the ruleset's core die, then delegate to a pure, unit-tested builder.
    pub async fn named_or_forced_assessment_plan(&self, request: &ContextRequest, user_input: &str, named_label: Option<&str>) -> AgentTurnPlan {
        let kernel = self.db.load_rule_kernel(&request.ruleset_id).await.ok().flatten();
        let dice = kernel.as_ref().and_then(|k| {
            k.dice_core.get("dice").and_then(|v| v.as_str())
                .or_else(|| k.check_model.get("dice").and_then(|v| v.as_str()))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        });
        match named_label.map(str::trim).filter(|s| !s.is_empty()) {
            Some(label) => build_named_check_plan(request, user_input, label, dice.as_deref()),
            None => build_forced_tech_plan(request, user_input, dice.as_deref()),
        }
    }

    pub async fn try_handle_ability_turn(
        &self,
        request: &ContextRequest,
        state: &RuntimeState,
        user_input: &str,
    ) -> Result<AbilityTurnResult> {
        if !ability_kernel_enabled() {
            return Ok(AbilityTurnResult::not_handled());
        }
        let world_tick = self.current_world_time(&request.session_id).await.map(|t| t.world_tick).unwrap_or_default();
        let frame_id = self.db.list_active_state_frames(&request.session_id, 1).await.ok().and_then(|frames| frames.into_iter().next().map(|f| f.frame_id));
        let actor_id = request.viewer.actor_id.as_deref().or(Some("pc.current"));
        AbilityService::new(self.db.clone(), self.search.clone()).handle_turn(AbilityTurnInput {
            session_id: &request.session_id,
            turn_id: &request.turn_id,
            ruleset_id: &request.ruleset_id,
            module_id: request.module_id.as_deref().or(state.module_id.as_deref()),
            actor_id,
            frame_id: frame_id.as_deref(),
            user_input,
            world_tick,
        }).await
    }


    pub async fn referee_combat_blocks_for_turn(&self, request: &ContextRequest) -> Result<Vec<ContextBlock>> {
        let world_tick = self.current_world_time(&request.session_id).await.map(|t| t.world_tick).unwrap_or_default();
        let block = RefereeCombatService::new(self.db.clone()).mechanical_ledger_context_block(&request.session_id, &request.ruleset_id, world_tick).await?;
        Ok(vec![block])
    }

    pub async fn contest_blocks_for_turn(&self, request: &ContextRequest) -> Result<Vec<ContextBlock>> {
        let world_tick = self.current_world_time(&request.session_id).await.map(|t| t.world_tick).unwrap_or_default();
        let block = ContestService::new(self.db.clone()).contest_context_block(&request.session_id, world_tick).await?;
        Ok(vec![block])
    }

    pub async fn verify_player_supplied_values(&self, request: &ContextRequest, user_input: &str) -> Result<Option<PlayerValueRefereeResult>> {
        let result = PlayerValueRefereeService::new(self.db.clone())
            .inspect_turn(&request.session_id, &request.turn_id, &request.ruleset_id, request.module_id.as_deref(), user_input)
            .await?;
        if result.handled {
            if let Some(ctx) = &result.narration_context {
                let mut block = ContextBlock::new(
                    format!("player_value_referee.{}", request.turn_id),
                    BlockKind::PlayerValueVerification,
                    "Player-Supplied Value Referee",
                    BlockContent::Text(ctx.clone()),
                    Visibility::GmOnly,
                    Stability::TurnDynamic,
                    CacheZone::DynamicTail,
                    Scope { scope_type: ScopeType::Turn, scope_id: request.turn_id.clone() },
                    142,
                );
                block.tags = vec!["player_value_referee".into(), "rules_first".into(), "table_override_policy".into()];
                block.expires_at_turn = Some(request.turn_id.clone());
                block.load_reason = Some("player_supplied_value_verification".into());
                let _ = self.db.upsert_runtime_context_block(&request.session_id, &block).await;
            }
            Ok(Some(result))
        } else {
            Ok(None)
        }
    }

    pub async fn try_materialize_turn(
        &self,
        request: &ContextRequest,
        state: &RuntimeState,
        user_input: &str,
    ) -> Result<MaterializationTurnResult> {
        if !real_materialization_enabled() {
            return Ok(MaterializationTurnResult::not_handled());
        }
        let world_tick = self.current_world_time(&request.session_id).await.map(|t| t.world_tick).unwrap_or_default();
        let frame_id = self.db.list_active_state_frames(&request.session_id, 1).await.ok().and_then(|frames| frames.into_iter().next().map(|f| f.frame_id));
        let actor_id = request.viewer.actor_id.as_deref().or(Some("pc.current"));
        MaterializationService::from_env(self.db.clone(), self.search.clone()).materialize_turn(MaterializationTurnInput {
            session_id: &request.session_id,
            turn_id: &request.turn_id,
            ruleset_id: &request.ruleset_id,
            module_id: request.module_id.as_deref().or(state.module_id.as_deref()),
            frame_id: frame_id.as_deref(),
            actor_id,
            user_input,
            world_tick,
        }).await
    }

    pub async fn try_handle_object_turn(
        &self,
        request: &ContextRequest,
        state: &RuntimeState,
        user_input: &str,
    ) -> Result<ObjectTurnResult> {
        if !object_kernel_enabled() {
            return Ok(ObjectTurnResult::not_handled());
        }
        let frame_id = self.db.list_active_state_frames(&request.session_id, 1).await.ok().and_then(|frames| frames.into_iter().next().map(|f| f.frame_id));
        let actor_id = request.viewer.actor_id.as_deref().or(Some("pc.current"));
        ObjectService::new(self.db.clone()).handle_turn(ObjectTurnInput {
            session_id: &request.session_id,
            turn_id: &request.turn_id,
            ruleset_id: &request.ruleset_id,
            module_id: request.module_id.as_deref().or(state.module_id.as_deref()),
            actor_id,
            frame_id: frame_id.as_deref(),
            user_input,
        }).await
    }

    pub async fn try_handle_conflict_turn(
        &self,
        request: &ContextRequest,
        state: &RuntimeState,
        user_input: &str,
        orchestration: Option<&TurnOrchestrationResult>,
    ) -> Result<ConflictTurnResult> {
        if !conflict_agent_v10_enabled() {
            return Ok(ConflictTurnResult::not_handled());
        }
        let agent = CombatAgent::from_env_or_default(self.db.clone());
        let actor_id = request.viewer.actor_id.as_deref().or(Some("pc.current"));
        let semantic_hint = orchestration.and_then(Self::conflict_hint_from_orchestration);
        agent.handle_turn(ConflictTurnInput {
            session_id: &request.session_id,
            turn_id: &request.turn_id,
            ruleset_id: &request.ruleset_id,
            module_id: request.module_id.as_deref().or(state.module_id.as_deref()),
            actor_id,
            user_input,
            semantic_hint: semantic_hint.as_ref(),
        }).await
    }




fn conflict_hint_from_orchestration(result: &TurnOrchestrationResult) -> Option<ConflictIntent> {
    let action = result.intent.action_kind;
    let frame_relevant = matches!(action,
        SituationActionKind::Attack
            | SituationActionKind::UnderAttack
            | SituationActionKind::EnemyInitiatedConflict
            | SituationActionKind::SceneEntersConflict
            | SituationActionKind::Defend
            | SituationActionKind::Dodge
            | SituationActionKind::Counterattack
            | SituationActionKind::TakeCover
            | SituationActionKind::Hack
            | SituationActionKind::DisableDevice
            | SituationActionKind::InvestigateDuringConflict
            | SituationActionKind::UseItem
            | SituationActionKind::Rescue
            | SituationActionKind::Intimidate
            | SituationActionKind::CastOrUsePower);
    let terminal = matches!(result.intent.frame_relation,
        FrameRelation::ExitAttempt
            | FrameRelation::DeescalationAttempt
            | FrameRelation::Surrender
            | FrameRelation::Flee
            | FrameRelation::HideToDisengage);
    if !frame_relevant && !terminal {
        return None;
    }
    let mut evidence = result.intent.evidence_terms.clone();
    evidence.push(format!("turn_route:{}", result.route.route_kind.as_str()));
    Some(ConflictIntent {
        intent_id: format!("intent_hint_{}", Uuid::new_v4().simple()),
        language: Some("semantic_orchestrator".into()),
        relation_to_active_frame: result.intent.frame_relation,
        action_kind: action,
        escalation_level: if matches!(action, SituationActionKind::Attack | SituationActionKind::UnderAttack | SituationActionKind::EnemyInitiatedConflict | SituationActionKind::SceneEntersConflict) { EscalationLevel::High } else { EscalationLevel::Medium },
        target_refs: vec![result.intent.target_kind.clone()],
        desired_outcome: Some(match action {
            SituationActionKind::Attack | SituationActionKind::Counterattack => "resolve the declared attack or sustained pressure",
            SituationActionKind::UnderAttack | SituationActionKind::EnemyInitiatedConflict | SituationActionKind::SceneEntersConflict => "respond to incoming danger",
            SituationActionKind::Flee | SituationActionKind::LeaveScene => "exit the active frame",
            SituationActionKind::Negotiate | SituationActionKind::Surrender | SituationActionKind::EndConflict => "de-escalate or close the conflict",
            _ => "continue the active frame action",
        }.into()),
        confidence: result.intent.confidence,
        evidence_terms: evidence,
        classifier: "turn_orchestrator_semantic_hint_v1_13_3".into(),
    })
}
    async fn handle_choice_or_nonroll_gate(&self, gate: InteractionGate, user_input: &str) -> Result<GateHandlingResult> {
        match &gate.expected_input {
            ExpectedInput::Choice { option_ids: _ } => {
                let lower = user_input.to_lowercase();
                if let Some(option) = infer_semantic_choice_option(&gate, user_input).or_else(|| gate.allowed_options.iter().find(|opt| {
                    lower.contains(&opt.option_id.to_lowercase()) || (!opt.label.is_empty() && lower.contains(&opt.label.to_lowercase()))
                })) {
                    let resolution = json!({"option_id": option.option_id, "label": option.label, "input": user_input, "resolver":"semantic_choice_gate_v1_3"});
                    self.db.update_interaction_gate_status(&gate.gate_id, GateStatus::Resolved, resolution.clone()).await.ok();
                    return Ok(GateHandlingResult::GateChoiceResolved { gate_id: gate.gate_id.clone(), option_id: option.option_id.clone(), option_label: option.label.clone(), resolution_json: resolution });
                }
                let reprompt_count = gate.resolution_json.as_ref().and_then(|v| v.get("reprompt_count")).and_then(|v| v.as_u64()).unwrap_or(0) + 1;
                if reprompt_count >= 2 {
                    if let Some(option) = default_choice_after_reprompt(&gate) {
                        let resolution = json!({"option_id": option.option_id, "label": option.label, "input": user_input, "resolver":"auto_default_after_reprompt", "reprompt_count": reprompt_count});
                        self.db.update_interaction_gate_status(&gate.gate_id, GateStatus::Resolved, resolution.clone()).await.ok();
                        return Ok(GateHandlingResult::GateChoiceResolved { gate_id: gate.gate_id.clone(), option_id: option.option_id.clone(), option_label: option.label.clone(), resolution_json: resolution });
                    }
                }
                let prompt = format!("当前必须先处理一个交互窗口：{}
可选项：{}
如果你想继续当前压制/攻击，可以直接说“继续”或“继续开火”；如果要换方向，请说出目标。", gate.prompt_public, gate.allowed_options.iter().map(|o| format!("{} ({})", o.label, o.option_id)).collect::<Vec<_>>().join("；"));
                self.db.update_interaction_gate_status(&gate.gate_id, GateStatus::Open, json!({"last_unparseable_input": user_input, "reason":"unparseable_choice_reply", "reprompt_count": reprompt_count})).await.ok();
                return Ok(GateHandlingResult::GateChoiceReprompt { gate_id: gate.gate_id, reason: "unparseable_choice_reply".into(), prompt_public: prompt });
            }
            ExpectedInput::Confirmation { yes_option, no_option } => {
                let lower = user_input.to_lowercase();
                let selected = if ["yes", "y", "确认", "继续", "同意"].iter().any(|t| lower.contains(t)) { Some(yes_option.clone()) }
                    else if ["no", "n", "取消", "不", "算了"].iter().any(|t| lower.contains(t)) { Some(no_option.clone()) }
                    else { None };
                if let Some(option_id) = selected {
                    let resolution = json!({"option_id": option_id, "input": user_input});
                    self.db.update_interaction_gate_status(&gate.gate_id, GateStatus::Resolved, resolution.clone()).await.ok();
                    return Ok(GateHandlingResult::GateChoiceResolved { gate_id: gate.gate_id.clone(), option_id, option_label: "confirmation".into(), resolution_json: resolution });
                }
                let prompt = format!("请先确认：{}", gate.prompt_public);
                return Ok(GateHandlingResult::GateChoiceReprompt { gate_id: gate.gate_id, reason: "unparseable_confirmation".into(), prompt_public: prompt });
            }
            _ => Ok(GateHandlingResult::None),
        }
    }


    pub async fn prepare_actionable_situation(
        &self,
        request: &ContextRequest,
        state: &RuntimeState,
        compiled: &CompiledContext,
        user_input: &str,
        conflict: Option<&ConflictTurnResult>,
    ) -> Result<DirectorTurnResult> {
        let director = ActionableSituationDirector::from_env_or_default();
        let result = director.prepare(DirectorInput { request, state, compiled, user_input, conflict });
        if let Some(brief) = &result.brief {
            self.db.insert_actionable_situation_brief(brief).await.ok();
            let _ = self.db.upsert_runtime_context_block(&request.session_id, &actionable_situation_block(brief, &request.turn_id)).await;
        }
        if let Some(board) = &result.clue_board {
            self.db.upsert_player_facing_clue_board(board).await.ok();
            let _ = self.db.upsert_runtime_context_block(&request.session_id, &clue_board_block(board, &request.turn_id)).await;
        }
        if let Some(consequence) = &result.consequence {
            self.db.insert_consequence_contract(consequence).await.ok();
        }
        for tick in &result.clock_ticks {
            self.db.insert_clock_tick(&request.session_id, &request.turn_id, tick).await.ok();
        }
        for spotlight in &result.spotlight {
            self.db.upsert_spotlight_state(&request.session_id, spotlight).await.ok();
        }
        Ok(result)
    }

    /// Close the interaction gate that belongs to an already-resolved check.
    ///
    /// Auto-resolve paths bypass `handle_open_interaction_gate`, so without this
    /// a stale `player_roll_required` gate can remain open and hijack later turns.
    async fn close_resolved_check_gate(&self, session_id: &str, check_id: &str) {
        self.db.update_pending_check_status(check_id, PendingCheckStatus::Resolved).await.ok();

        // The canonical gate for a PendingCheck is `gate_{check_id}`.  Close it
        // directly first so auto-resolve cannot leave a stale roll gate behind even
        // if another advisory/direction gate is newer and would be returned by
        // `get_open_interaction_gate`.
        let canonical_gate_id = format!("gate_{}", check_id);
        let _ = self.db.update_interaction_gate_status(
            &canonical_gate_id,
            GateStatus::Resolved,
            json!({"check_id": check_id, "auto_resolved_gate_close": true, "canonical_gate_close": true}),
        ).await;

        // Also close the current open gate if it is bound to the same check. This
        // covers non-canonical gate ids and keeps old sessions self-healing.
        if let Ok(Some(gate)) = self.db.get_open_interaction_gate(session_id).await {
            let matches_check = matches!(
                &gate.expected_input,
                ExpectedInput::RollResult { check_id: cid, .. } if cid == check_id
            );
            if matches_check {
                let _ = self.db.update_interaction_gate_status(
                    &gate.gate_id,
                    GateStatus::Resolved,
                    json!({"check_id": check_id, "auto_resolved_gate_close": true}),
                ).await;
            }
        }
    }

    fn blocked_missing_source_check_result(&self, session_id: &str, turn_id: &str, contract: &CheckContract, reason: impl Into<String>) -> CheckResultRecord {
        let reason = reason.into();
        let now = chrono::Utc::now();
        CheckResultRecord {
            check_id: contract.check_id.clone(),
            roll: DiceRollRecord {
                roll_id: format!("roll_blocked_{}", Uuid::new_v4().simple()),
                session_id: session_id.to_string(),
                turn_id: turn_id.to_string(),
                check_id: Some(contract.check_id.clone()),
                roller_kind: contract.initiator.actor_kind,
                roller_id: Some(contract.initiator.actor_id.clone()),
                visibility: RollVisibility::NoRoll,
                expression: "blocked_missing_source_backed_parameters".into(),
                result: json!({
                    "blocked": true,
                    "reason": reason,
                    "check_label": contract.check_label,
                    "required_action": "hydrate actor/target/weapon/defense/damage facts from semantic source units before rolling",
                    "source_refs_present": !contract.source_refs.is_empty() || !contract.learned_packet_ids.is_empty(),
                }),
                seed_commitment: "no_roll_missing_source_backed_parameters".into(),
                revealed_at: Some(now),
                created_at: now,
            },
            outcome: json!({
                "check_id": contract.check_id,
                "check_label": contract.check_label,
                "blocked": true,
                "success": null,
                "target": null,
                "degree": null,
                "reason": "missing_source_backed_parameters",
                "narration_policy": "do not abort the turn; narrate that the mechanical resolution is paused until source-backed facets are hydrated; do not invent DV, HP, SP, damage, or skill totals",
            }),
            committed_patches: vec![],
            created_at: now,
        }
    }

    pub async fn resolve_check_with_input(
        &self,
        session_id: &str,
        turn_id: &str,
        contract: &CheckContract,
        input: &str,
    ) -> Result<CheckResultRecord> {
        let normalized = normalize_contract_for_system_roll(contract);
        if Self::contract_missing_source_backed_parameters(&normalized) {
            let blocked = self.blocked_missing_source_check_result(session_id, turn_id, &normalized, "source-backed mechanical parameters are missing");
            self.db.insert_check_result(&blocked).await.ok();
            self.close_resolved_check_gate(session_id, &contract.check_id).await;
            return Ok(blocked);
        }
        let roll = self.resolve_roll_input(session_id, turn_id, Some(&normalized.check_id), &normalized, input).await?;
        let outcome = self.resolve_outcome_with_opposition(&normalized, &roll).await?;
        let mut result = CheckResultRecord { check_id: normalized.check_id.clone(), roll, outcome, committed_patches: vec![], created_at: chrono::Utc::now() };
        let _ = self.post_check_resolution(&normalized, &mut result).await;
        self.db.insert_check_result(&result).await?;
        self.close_resolved_check_gate(session_id, &contract.check_id).await;
        Ok(result)
    }

    /// Central data-driven default: a check that is NOT already source-backed
    /// (i.e. built by a standalone skill/investigation planner with hardcoded
    /// dice/DV) inherits the ruleset's parsed kernel core mechanic — the dice,
    /// the typed resolution model (replacing a provisional static target), and a
    /// kernel source citation. Source-backed checks (combat/object/ability cite
    /// kernel refs) early-return untouched. Rulesets whose kernel lacks a typed
    /// dice/compare (e.g. an old pre-reader kernel) are also left as-is.
    async fn apply_kernel_defaults_if_unsourced(&self, c: &mut CheckContract) {
        if !c.source_refs.is_empty() || !c.learned_packet_ids.is_empty() { return; }
        let kernel = match self.db.load_rule_kernel(&c.ruleset_id).await { Ok(Some(k)) => k, _ => return };
        if let Some(d) = kernel.dice_core.get("dice").and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
            c.dice_expression = d;
        }
        let provisional_target = matches!(&c.target, CheckTargetModel::UnknownUntilLookup)
            || matches!(&c.target, CheckTargetModel::StaticNumber { label, .. } if {
                let l = label.to_ascii_lowercase();
                l.contains("suggested") || l.contains("provisional") || l.contains("until") || l.contains("default")
            });
        if provisional_target {
            // count_faces/meet_or_beat give a concrete CheckTargetModel; roll_under
            // (and not-yet-typed kernels) give None → clear to UnknownUntilLookup so
            // the contest kernel resolves it (e.g. PercentileRollUnder for d100), not
            // a stale roll-high static target.
            c.target = target_model_from_dice_core(&kernel.dice_core).unwrap_or(CheckTargetModel::UnknownUntilLookup);
        }
        if !kernel.source_refs.is_empty() {
            c.source_refs = kernel.source_refs.clone();
            c.ruling_status = RulingStatus::SourceBacked;
        }
    }

    pub async fn execute_agent_roll(
        &self,
        session_id: &str,
        turn_id: &str,
        contract: &CheckContract,
    ) -> Result<CheckResultRecord> {
        let mut normalized = normalize_contract_for_system_roll(contract);
        self.apply_kernel_defaults_if_unsourced(&mut normalized).await;
        if Self::contract_missing_source_backed_parameters(&normalized) {
            let blocked = self.blocked_missing_source_check_result(session_id, turn_id, &normalized, "source-backed mechanical parameters are missing");
            self.db.insert_check_result(&blocked).await.ok();
            self.close_resolved_check_gate(session_id, &contract.check_id).await;
            return Ok(blocked);
        }
        let roll = self.resolve_roll_input(session_id, turn_id, Some(&normalized.check_id), &normalized, &normalized.dice_expression).await?;
        let outcome = self.resolve_outcome_with_opposition(&normalized, &roll).await?;
        let mut result = CheckResultRecord { check_id: normalized.check_id.clone(), roll, outcome, committed_patches: vec![], created_at: chrono::Utc::now() };
        let _ = self.post_check_resolution(&normalized, &mut result).await;
        self.db.insert_check_result(&result).await?;
        self.close_resolved_check_gate(session_id, &contract.check_id).await;
        Ok(result)
    }

    /// Execute a check with the product-mode dice policy and immediately consume
    /// any generated damage/effect follow-up checks that are also system-rollable.
    ///
    /// This is the Rust boundary that prevents LLM narration from turning a
    /// CheckContract into "please roll and tell me the result" prose. CLI/API
    /// handlers can emit the returned bundle as `[roll]...[/roll]` context and
    /// then fall through to the shared narrator.
    pub async fn execute_system_roll_bundle(
        &self,
        session_id: &str,
        turn_id: &str,
        contract: &CheckContract,
    ) -> Result<AutoRollExecution> {
        let mut normalized = normalize_contract_for_system_roll(contract);
        self.apply_kernel_defaults_if_unsourced(&mut normalized).await;
        if Self::contract_missing_source_backed_parameters(&normalized) {
            let blocked = self.blocked_missing_source_check_result(session_id, turn_id, &normalized, "source-backed mechanical parameters are missing");
            self.db.insert_check_result(&blocked).await.ok();
            self.close_resolved_check_gate(session_id, &contract.check_id).await;
            return Ok(AutoRollExecution { primary: blocked, followups: vec![], roll_policy: "blocked_missing_source".into() });
        }
        let primary = self.execute_agent_roll(session_id, turn_id, &normalized).await?;
        let mut followups = Vec::new();

        // Attack resolution may create an effect/damage PendingCheck. Under
        // product mode this is another tool call, not a second player prompt.
        if let Some(pending) = self.db.get_open_pending_check(session_id).await? {
            if pending.check_id != normalized.check_id && is_effect_or_damage_contract(&pending.contract) {
                let effect_contract = normalize_contract_for_system_roll(&pending.contract);
                if Self::contract_missing_source_backed_parameters(&effect_contract) {
                    let blocked = self.blocked_missing_source_check_result(session_id, turn_id, &effect_contract, "source-backed effect/damage parameters are missing");
                    self.db.insert_check_result(&blocked).await.ok();
                    let _ = self.db.update_pending_check_status(&pending.check_id, PendingCheckStatus::Resolved).await;
                    followups.push(blocked);
                    return Ok(AutoRollExecution { primary, followups, roll_policy: "blocked_missing_source".into() });
                }
                let effect_result = self.execute_agent_roll(session_id, turn_id, &effect_contract).await?;
                let _ = self.db.update_pending_check_status(&pending.check_id, PendingCheckStatus::Resolved).await;
                followups.push(effect_result);
            }
        }

        Ok(AutoRollExecution {
            primary,
            followups,
            roll_policy: "system_rolls_visible".into(),
        })
    }


fn contract_missing_source_backed_parameters(contract: &CheckContract) -> bool {
    if !Self::fail_on_missing_source_backed_parameters() && !Self::graceful_degrade_missing_source_backed_parameters() {
        return false;
    }
    let intent = contract.intent_kind.to_ascii_lowercase();
    let label = contract.check_label.to_ascii_lowercase();
    let action = contract.action_summary.to_ascii_lowercase();
    let parameter_sensitive = ["attack", "counterattack", "defend", "dodge", "damage", "hack", "disable", "shoot", "fire", "开火", "攻击", "射击", "黑入", "切断"]
        .iter()
        .any(|needle| intent.contains(needle) || label.contains(needle) || action.contains(needle));
    if !parameter_sensitive {
        return false;
    }
    let missing_target = matches!(&contract.target, CheckTargetModel::UnknownUntilLookup);
    let missing_opposition = matches!(&contract.opposition, OppositionModel::NoMechanicalOpposition);
    let no_sources = contract.source_refs.is_empty() && contract.learned_packet_ids.is_empty();
    let bare_die = ["1d10", "d20", "1d20", "2d6", "d100", "1d100", "6d4"].contains(&contract.dice_expression.trim());
    no_sources && (missing_target || missing_opposition || bare_die)
}

fn graceful_degrade_missing_source_backed_parameters() -> bool {
    std::env::var("TRPG_GRACEFUL_DEGRADE_MISSING_SOURCE_PARAMS")
        .ok()
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(true)
}

fn fail_on_missing_source_backed_parameters() -> bool {
    std::env::var("TRPG_FAIL_ON_MISSING_SOURCE_BACKED_PARAMS")
        .ok()
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(true)
}

    async fn resolve_roll_input(
        &self,
        session_id: &str,
        turn_id: &str,
        check_id: Option<&str>,
        contract: &CheckContract,
        input: &str,
    ) -> Result<DiceRollRecord> {
        let parsed = parse_roll_text(input).unwrap_or_else(|| {
            let trimmed = input.trim();
            if wants_system_roll(input) || trimmed.eq_ignore_ascii_case("/roll") || trimmed.eq_ignore_ascii_case("roll") {
                ParsedRollText::DiceExpression(contract.dice_expression.clone())
            } else {
                ParsedRollText::DiceExpression(trimmed.strip_prefix("/roll ").unwrap_or(trimmed).trim().to_string())
            }
        });
        let parsed = match parsed {
            ParsedRollText::ReportedTotal(_) | ParsedRollText::ReportedDieAndComponents { .. }
                if !player_reported_roll_totals_allowed() =>
            {
                return Err(anyhow!("player-reported roll totals are disabled; reply `roll` to let the system dice tool roll for this check"));
            }
            ParsedRollText::DiceExpression(expr) if player_supplied_roll_expressions_allowed() => ParsedRollText::DiceExpression(expr),
            ParsedRollText::DiceExpression(_) => ParsedRollText::DiceExpression(contract.dice_expression.clone()),
            other => other,
        };
        let expression_for_record = parsed.expression_for_record();
        let result_json = match parsed {
            ParsedRollText::ReportedTotal(total) => json!({"mode":"reported_total", "total": total}),
            ParsedRollText::ReportedDieAndComponents { die, components, total } => json!({"mode":"reported_components", "die": die, "components": components, "total": total}),
            ParsedRollText::DiceExpression(expr) => {
                let rolled = roll_dice(&expr)?;
                json!({"mode":"rolled", "expression": rolled.expression, "rolls": rolled.rolls, "modifier": rolled.modifier, "total": rolled.total})
            }
        };
        let seed_material = format!("{}:{}:{}:{}", session_id, turn_id, check_id.unwrap_or("none"), serde_json::to_string(&result_json)?);
        let record = DiceRollRecord {
            roll_id: format!("roll_{}", Uuid::new_v4().simple()),
            session_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            check_id: check_id.map(str::to_string),
            roller_kind: contract.initiator.actor_kind,
            roller_id: Some(contract.initiator.actor_id.clone()),
            visibility: contract.roll_visibility,
            expression: expression_for_record.clone(),
            result: result_json,
            seed_commitment: sha256_hex(seed_material),
            revealed_at: if contract.disclosure.show_roll_to_player { Some(chrono::Utc::now()) } else { None },
            created_at: chrono::Utc::now(),
        };
        let mut roll_plan = make_roll_plan_from_check(session_id, turn_id, check_id, contract, &expression_for_record, &record.result);
        roll_plan.roll_plan_id = format!("rollplan_{}", Uuid::new_v4().simple());
        insert_roll_plan(&self.db, &roll_plan).await.ok();
        self.db.insert_dice_roll(&record).await?;
        self.db.insert_agent_tool_call(&AgentToolCallRecord {
            tool_call_id: format!("tool_{}", Uuid::new_v4().simple()),
            session_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            tool_name: "roll_dice".into(),
            visibility: if contract.roll_visibility == RollVisibility::PrivateGmRoll { Visibility::GmOnly } else { Visibility::PlayerVisible },
            input_json: json!({"expression": expression_for_record, "check_id": check_id, "roll_visibility": contract.roll_visibility}),
            output_json: Some(record.result.clone()),
            status: "done".into(),
            error: None,
            created_at: chrono::Utc::now(),
        }).await.ok();
        Ok(record)
    }

    /// 对抗时预掷防御骰(私有 GM 掷,落库供复算)随 contract 传入 contest;否则 None。
    /// contest 保持零 RNG。defender_expression 取 kernel 核心掷式(dice_core.dice),fallback 攻击式。
    async fn resolve_outcome_with_opposition(&self, contract: &CheckContract, roll: &DiceRollRecord) -> Result<serde_json::Value> {
        let svc = ContestService::new(self.db.clone());
        if !contract_is_opposed(contract) {
            return svc.resolve_outcome(contract, roll, None).await;
        }
        let def_expr = self.db.load_rule_kernel(&contract.ruleset_id).await.ok().flatten()
            .and_then(|k| k.dice_core.get("dice").and_then(|v| v.as_str()).map(str::to_string))
            .unwrap_or_else(|| contract.dice_expression.clone());
        let rolled = roll_dice(&def_expr)?;
        let seed_material = format!("{}:{}:defender:{}", contract.session_id, contract.turn_id, contract.check_id);
        let def_record = DiceRollRecord {
            roll_id: format!("roll_{}", Uuid::new_v4().simple()),
            session_id: contract.session_id.clone(),
            turn_id: contract.turn_id.clone(),
            check_id: Some(contract.check_id.clone()),
            roller_kind: ActorKind::Npc,
            roller_id: contract.target_actor.as_ref().map(|a| a.actor_id.clone()),
            visibility: RollVisibility::PrivateGmRoll,
            expression: def_expr.clone(),
            result: json!({"mode":"rolled","expression":rolled.expression,"rolls":rolled.rolls,"modifier":rolled.modifier,"total":rolled.total}),
            seed_commitment: sha256_hex(seed_material),
            revealed_at: None,
            created_at: chrono::Utc::now(),
        };
        self.db.insert_dice_roll(&def_record).await.ok(); // 落库供复算(失败不阻断)
        svc.resolve_outcome(contract, roll, Some(&def_record)).await
    }

    /// Audit a completed turn and write learning-side artifacts.
    ///
    /// By default this writes `rulings_log` + `learning_candidates` only.
    /// Set `TRPG_LEARNING_AUTO_PROMOTE=true` to promote high-evidence candidates into `learned_packets`.
    pub async fn audit_learning_for_turn(
        &self,
        session_id: &str,
        ruleset_id: &str,
        module_id: Option<&str>,
        turn_id: &str,
        user_input: &str,
        assistant_output: &str,
    ) -> Result<LearningAuditResult> {
        let audit_run_id = format!("learning_audit_{}", Uuid::new_v4().simple());
        let mut result = LearningAuditResult {
            audit_run_id: audit_run_id.clone(),
            session_id: Some(session_id.to_string()),
            turn_id: Some(turn_id.to_string()),
            ..Default::default()
        };
        if !learning_audit_enabled() {
            result.warnings.push("learning_audit_disabled".into());
            return Ok(result);
        }

        let demand_id = format!("auto_turn_{turn_id}");
        let input_json = json!({
            "session_id": session_id,
            "ruleset_id": ruleset_id,
            "module_id": module_id,
            "turn_id": turn_id,
            "demand_id": demand_id,
        });
        let lookups = self.db.list_lookup_events_for_demand(Some(session_id), &demand_id, 8).await.unwrap_or_default();
        if lookups.is_empty() {
            result.warnings.push("no_lookup_events_for_turn".into());
        }
        let mut mechanical = detect_mechanical_signal(user_input, assistant_output);
        let check_contracts = self.db.list_check_contracts_for_turn(session_id, turn_id).await.unwrap_or_default();
        let effect_contracts = self.db.list_effect_contracts_for_turn(session_id, turn_id).await.unwrap_or_default();
        if !effect_contracts.is_empty() && !mechanical.mentioned_terms.iter().any(|t| t == "effect_contract") {
            mechanical.has_signal = true;
            mechanical.mentioned_terms.push("effect_contract".into());
        }
        if let Some((kind, value)) = check_contracts.iter().find_map(check_target_signal) {
            mechanical.has_signal = true;
            mechanical.has_specific_target = true;
            mechanical.target_kind = Some(kind);
            mechanical.target_value = Some(value);
            if !mechanical.mentioned_terms.iter().any(|t| t == "check_contract") {
                mechanical.mentioned_terms.push("check_contract".into());
            }
        }
        let source_refs = extract_source_refs_from_lookup_events(&lookups);
        let hit_titles = top_search_hit_titles(&lookups, 4);
        let has_source_evidence = !source_refs.is_empty() || lookups.iter().any(|e| e.result_status == "source_backed" || e.result_status == "hit");
        if !mechanical.has_signal && !has_source_evidence {
            result.warnings.push("no_mechanical_or_source_signal".into());
            let _ = self.db.insert_learning_audit_run(&audit_run_id, Some(session_id), Some(turn_id), Some(ruleset_id), module_id, "done", input_json, serde_json::to_value(&result)?, None).await;
            return Ok(result);
        }

        let ruling_status = if has_source_evidence { RulingStatus::SourceBacked } else { RulingStatus::Provisional };
        let confidence = if has_source_evidence && mechanical.has_specific_target { RulingConfidence::Medium } else { RulingConfidence::Low };
        let ruling = RulingLogEntry {
            ruling_id: format!("ruling_{}", Uuid::new_v4().simple()),
            session_id: Some(session_id.to_string()),
            ruleset_id: Some(ruleset_id.to_string()),
            module_id: module_id.map(str::to_string),
            demand_id: Some(demand_id.clone()),
            ruling_text: make_ruling_summary(user_input, assistant_output, &mechanical, &hit_titles),
            source_refs: source_refs.clone(),
            status: ruling_status,
            confidence,
            provisional: !has_source_evidence,
            superseded_by: None,
            created_at: chrono::Utc::now(),
        };
        self.db.insert_ruling_log(&ruling).await?;
        result.rulings.push(ruling);

        if has_source_evidence {
            let now = chrono::Utc::now();
            let mut risk_flags = Vec::new();
            if source_refs.is_empty() { risk_flags.push("no_source_refs_in_hit".to_string()); }
            if !mechanical.has_specific_target { risk_flags.push("no_specific_difficulty_or_target".to_string()); }
            if !has_source_evidence { risk_flags.push("provisional_ruling".to_string()); }
            let evidence_score = evidence_score(&mechanical, &source_refs, &lookups, &risk_flags);
            if evidence_score < 0.50 { risk_flags.push("low_evidence_score".to_string()); }
            let packet_type = infer_packet_type(user_input, assistant_output);
            let packet_key = safe_packet_key(&packet_type, user_input, &mechanical);
            let candidate = LearningAuditCandidate {
                candidate_id: format!("candidate_{}", Uuid::new_v4().simple()),
                session_id: Some(session_id.to_string()),
                turn_id: Some(turn_id.to_string()),
                ruleset_id: ruleset_id.to_string(),
                module_id: module_id.map(str::to_string),
                demand_id: Some(demand_id),
                packet_type,
                packet_key,
                title: learning_candidate_title(user_input, &mechanical),
                summary: make_learning_candidate_summary(user_input, assistant_output, &mechanical, &hit_titles),
                packet_json: json!({
                    "user_input_excerpt": user_input.chars().take(400).collect::<String>(),
                    "assistant_output_excerpt": assistant_output.chars().take(900).collect::<String>(),
                    "detected_mechanics": mechanical,
                    "check_contracts": check_contracts,
                    "effect_contracts": effect_contracts,
                    "source_hit_titles": hit_titles,
                    "audit_run_id": audit_run_id,
                    "promotion_policy": "pending_review_by_default"
                }),
                source_refs,
                evidence_score,
                risk_flags,
                verifier_status: LearningCandidateStatus::PendingReview,
                verifier_notes: Some("Auto-generated by learning_audit; approve only after source/ruling review.".into()),
                created_at: now,
                updated_at: now,
            };
            self.db.insert_learning_candidate(&candidate).await?;
            if learning_auto_promote_enabled() && candidate.evidence_score >= 0.75 && !candidate.risk_flags.iter().any(|f| f == "low_evidence_score" || f == "provisional_ruling") {
                let packet = candidate.to_learned_packet(LearningStage::UsedOnce);
                self.db.upsert_learned_packet(&packet).await?;
                self.db.update_learning_candidate_status(&candidate.candidate_id, LearningCandidateStatus::AutoPromoted, Some("Auto-promoted by TRPG_LEARNING_AUTO_PROMOTE=true")).await?;
                result.promoted_packets.push(packet);
            }
            result.candidates.push(candidate);
        }

        let _ = self.db.insert_learning_audit_run(&audit_run_id, Some(session_id), Some(turn_id), Some(ruleset_id), module_id, "done", input_json, serde_json::to_value(&result)?, None).await;
        Ok(result)
    }

    async fn auto_search_blocks_for_turn(
        &self,
        request: &ContextRequest,
        state: &RuntimeState,
        current_input: &str,
    ) -> Result<Vec<ContextBlock>> {
        let Some(search) = &self.search else { return Ok(vec![]); };
        if !runtime_auto_search_enabled() { return Ok(vec![]); }
        let input = current_input.trim();
        if input.is_empty() || !looks_rule_or_module_sensitive(input) { return Ok(vec![]); }

        // Avoid hard-filtering by ruleset/module/session/scene here. Search sources are
        // dynamic and may project scope differently (source Markdown, JSONL artifacts,
        // DB rows, learned packets). Ranking/visibility happens in search; cache-zone
        // decisions happen after hits are returned.
        let scopes = BTreeMap::new();

        let limit = std::env::var("TRPG_RUNTIME_AUTO_SEARCH_LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(5);
        let domains = std::env::var("TRPG_RUNTIME_AUTO_SEARCH_DOMAINS")
            .ok()
            .map(|v| v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect::<Vec<_>>())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| vec!["learned".into(), "rules".into(), "modules".into(), "rulings".into(), "source".into(), "parsed".into()]);

        let search_request = SearchRequest {
            query: input.to_string(),
            mode: SearchMode::Auto,
            domains,
            kinds: vec![],
            tags: vec![],
            scopes,
            filters: Default::default(),
            rewrite_query: true,
            intent: Some("runtime_auto_rule_module_lookup".into()),
            limit,
            explain: true,
            viewer: request.viewer.clone(),
        };

        let response = search.search_async(&search_request).await?;
        let status = if response.hits.is_empty() { "no_hits" } else { "source_backed" };
        let lookup_event = LookupEvent {
            event_id: format!("lookup_{}", Uuid::new_v4().simple()),
            session_id: Some(request.session_id.clone()),
            ruleset_id: Some(request.ruleset_id.clone()),
            module_id: request.module_id.clone().or_else(|| state.module_id.clone()),
            demand_id: Some(format!("auto_turn_{}", request.turn_id)),
            query_text: input.to_string(),
            search_terms: vec![input.to_string()],
            source_hits: serde_json::to_value(&response.hits)?,
            result_status: status.to_string(),
            created_at: chrono::Utc::now(),
        };
        let _ = self.db.insert_lookup_event(&lookup_event).await;

        let mut blocks = Vec::new();
        let mut pinned_count = 0usize;
        let max_scene_pins = std::env::var("TRPG_RUNTIME_AUTO_SEARCH_MAX_SCENE_PINS").ok().and_then(|v| v.parse().ok()).unwrap_or(1usize);
        for hit in response.hits.into_iter().take(limit as usize) {
            let should_pin = pinned_count < max_scene_pins && should_pin_search_hit_to_scene(&hit, state);
            let load_req = SearchLoadRequest {
                hit,
                session_id: Some(request.session_id.clone()),
                turn_id: Some(request.turn_id.clone()),
                scene_id: state.scene_id.clone(),
                ruleset_id: Some(request.ruleset_id.clone()),
                module_id: request.module_id.clone().or_else(|| state.module_id.clone()),
                demand_id: Some(format!("auto_turn_{}", request.turn_id)),
                query_text: Some(input.to_string()),
                cache_zone: if should_pin { CacheZone::PinnedMiddle } else { CacheZone::DynamicTail },
                ttl: if should_pin { "scene".into() } else { "turn".into() },
                load_reason: if should_pin { "auto_scene_rule_packet".into() } else { "auto_turn_lookup".into() },
                persist: true,
            };
            let block = search.load_hit_to_context_block(&load_req).unwrap_or_else(|_| load_req.to_context_block());
            let _ = self.db.upsert_runtime_context_block(&request.session_id, &block).await;
            if should_pin { pinned_count += 1; }
            blocks.push(block);
        }
        Ok(blocks)
    }

    async fn memory_blocks_for_turn(
        &self,
        request: &ContextRequest,
        state: &RuntimeState,
        current_input: Option<&str>,
    ) -> Result<Vec<ContextBlock>> {
        let mut blocks = Vec::new();
        for snapshot in self.db.list_memory_snapshots(&request.session_id, 3).await? {
            if project_visibility(memory_snapshot_block(&snapshot), &request.viewer).is_some() {
                blocks.push(memory_snapshot_block(&snapshot));
            }
        }
        let query_text = current_input.unwrap_or_default().trim().to_string();
        let memory_query = MemoryQuery {
            session_id: request.session_id.clone(),
            text: query_text,
            ruleset_id: Some(request.ruleset_id.clone()),
            module_id: request.module_id.clone().or_else(|| state.module_id.clone()),
            scene_id: state.scene_id.clone(),
            location_id: state.location_id.clone(),
            actor_ids: state.active_npc_ids.clone(),
            tags: vec![],
            limit: std::env::var("TRPG_MEMORY_RETRIEVAL_LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(8),
            viewer: request.viewer.clone(),
        };
        let retrieved = self.db.retrieve_memory(&memory_query).await?;
        if !retrieved.facts.is_empty() || !retrieved.events.is_empty() {
            blocks.push(retrieved_memory_block(&request.session_id, &request.turn_id, &retrieved));
        }
        Ok(blocks)
    }

    async fn learned_packet_blocks_for_turn(
        &self,
        request: &ContextRequest,
        state: &RuntimeState,
    ) -> Result<Vec<ContextBlock>> {
        let limit = std::env::var("TRPG_LEARNED_PACKET_LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(8);
        let packets = self.db.list_learned_packets(&request.ruleset_id, request.module_id.as_deref().or(state.module_id.as_deref()), limit).await?;
        Ok(packets.into_iter().map(learned_packet_block).collect())
    }

    async fn rule_steward_prefix_blocks_for_turn(&self, request: &ContextRequest) -> Result<Vec<ContextBlock>> {
        let enabled = std::env::var("TRPG_RULE_STEWARD_ENABLE_V116")
            .map(|v| !matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no"))
            .unwrap_or(true);
        if !enabled {
            return Ok(Vec::new());
        }
        let mut blocks = Vec::new();
        if let Some(kernel) = self.db.load_rule_kernel(&request.ruleset_id).await? {
            // B1: the kernel rides BP1 stripped of mechanics_catalog (a CoC-sized
            // catalog inline would blow validate_compiled_budget); the catalog is
            // re-projected below as a compact index block instead.
            let mut block = ContextBlock::new(
                format!("rule_steward.active_kernel.{}", request.ruleset_id),
                BlockKind::RuleStewardKernel,
                "Active Rule Steward BP1 Kernel",
                BlockContent::Json(serde_json::to_value(&kernel_bp1_view(&kernel))?),
                Visibility::GmOnly,
                Stability::RarelyChanged,
                CacheZone::Prefix,
                Scope::ruleset(&request.ruleset_id),
                118,
            );
            block.tags = vec!["rule_steward".into(), "bp1".into(), "active_rule_kernel".into(), "source_backed".into()];
            block.source_refs = kernel.source_refs.clone();
            block.load_reason = Some("active_rule_kernel".into());
            blocks.push(block);
            // B1: compact mechanics-catalog index (one line `id | name | when_to_use`
            // per entry, data-driven tiering past the limit) + passive-modifier lines
            // for the viewer, all in ONE prefix block right under the kernel block.
            // Empty catalog -> no block (older kernels change nothing, fail-closed).
            if !kernel.mechanics_catalog.is_empty() {
                let limit = std::env::var("TRPG_MECHANICS_INDEX_BP1_LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(96);
                let mut text = catalog_index_text(&kernel.mechanics_catalog, limit);
                let viewer_actor_id = request.viewer.actor_id.as_deref().unwrap_or("pc.current");
                let pm_lines = self.passive_lines_for_viewer(&request.session_id, viewer_actor_id, &kernel.mechanics_catalog).await;
                if !pm_lines.is_empty() {
                    text.push('\n');
                    text.push_str(&pm_lines.join("\n"));
                }
                let mut index_block = ContextBlock::new(
                    format!("rule_steward.mechanics_index.{}", request.ruleset_id),
                    BlockKind::MechanicsCatalogIndex,
                    "Mechanics Catalog Index",
                    BlockContent::Text(text),
                    Visibility::GmOnly,
                    Stability::RarelyChanged,
                    CacheZone::Prefix,
                    Scope::ruleset(&request.ruleset_id),
                    116,
                );
                index_block.tags = vec!["rule_steward".into(), "bp1".into(), "mechanics_catalog_index".into(), "source_backed".into()];
                index_block.load_reason = Some("mechanics_catalog_index".into());
                blocks.push(index_block);
            }
        }
        if let Some(pack) = self.db.load_character_onboarding_pack(&request.ruleset_id).await? {
            let mut block = ContextBlock::new(
                format!("rule_steward.character_onboarding.{}", request.ruleset_id),
                BlockKind::CharacterOnboardingPack,
                "Active Character Onboarding Pack",
                BlockContent::Json(serde_json::to_value(&pack)?),
                Visibility::GmOnly,
                Stability::RarelyChanged,
                CacheZone::Prefix,
                Scope::ruleset(&request.ruleset_id),
                104,
            );
            block.tags = vec!["rule_steward".into(), "character_onboarding".into(), "character_creation".into(), "playability".into()];
            block.source_refs = pack.source_refs.clone();
            block.load_reason = Some("active_character_onboarding_pack".into());
            blocks.push(block);
        }
        Ok(blocks)
    }

    /// B1 PM-line assembly: load the viewer's card (same RuntimeParameterService
    /// usage as `refresh_actor_live_derived`) and render one passive-projection
    /// line per catalog entry that has one. Actor missing / db failure / no PM
    /// entries -> empty Vec — BP1 assembly must never fail on PM projection.
    async fn passive_lines_for_viewer(&self, session_id: &str, viewer_actor_id: &str, entries: &[MechanicEntry]) -> Vec<String> {
        let service = RuntimeParameterService::new(self.db.clone());
        let Ok(Some(params)) = service.load_actor_parameters(session_id, viewer_actor_id).await else {
            return Vec::new();
        };
        entries.iter().filter_map(|e| passive_projection_line(e, &params.sheet_json)).collect()
    }

    async fn state_frame_blocks_for_turn(&self, request: &ContextRequest) -> Result<Vec<ContextBlock>> {
        let limit = std::env::var("TRPG_STATE_FRAME_ACTIVE_LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(4);
        let frames = self.db.list_active_state_frames(&request.session_id, limit).await?;
        Ok(frames.into_iter().map(|frame| frame.to_context_block(&request.turn_id)).collect())
    }

    /// §Phase4 当前场景 deep 内容投影：取该模组当前 ModuleGraph 的当前场景，调
    /// `scene_node_to_blocks` 投影成 SceneStatic/DynamicTail 块。当前场景 = 显式
    /// `state.scene_id` 优先，缺失/不匹配则回退模组入口场景（首个 DeepExtracted）——
    /// 因引擎暂未用 scene_id 追踪模组场景导航，此回退让"进模组即见首场景"成立。
    /// fail-closed：缺 module_id、模组不属本 project、取不到 graph、或无任何
    /// DeepExtracted 场景 → 返回空 Vec(不 panic、不编造)。
    async fn module_scene_blocks_for_turn(
        &self,
        request: &ContextRequest,
        state: &RuntimeState,
        project: &ProjectBundle,
    ) -> Result<Vec<ContextBlock>> {
        let Some(module_id) = request.module_id.as_deref().or(state.module_id.as_deref()) else {
            return Ok(Vec::new());
        };
        // The project bundle carries a parse-time snapshot of module_graph that
        // P5 continue-extraction never refreshes; reading scenes from it would
        // make continued deep-extraction invisible at runtime. The single source
        // of truth for scenes is the module bundle row (kept current by both
        // parse_module and the P5 continue job). Confirm the module belongs to
        // this project, then load the CURRENT graph from the module bundle. This
        // costs one extra DB load per turn — accepted: correctness over the prior
        // dup-load optimization (which only avoided reloading the same project
        // bundle). fail-closed: unknown module / no graph -> empty Vec, no panic.
        if !project.modules.iter().any(|m| m.module_id == module_id) {
            return Ok(Vec::new());
        }
        let Some(graph) = self.db.load_module_graph(module_id).await? else {
            return Ok(Vec::new());
        };
        // 当前场景：显式 state.scene_id 优先；缺失/不匹配 → 回退到模组入口场景（首个
        // DeepExtracted，即 Pass B 深抽的入口）。引擎暂未用 scene_id 追踪模组场景导航
        // （场景切换是后续工作），此回退让"进模组即看到首场景"成立。fail-closed：
        // 无 DeepExtracted 场景 → 下面 find 返回 None → 空 Vec。
        let node = state
            .scene_id
            .as_deref()
            .and_then(|sid| graph.scenes.iter().find(|s| s.node_id == sid))
            .or_else(|| {
                graph
                    .scenes
                    .iter()
                    .find(|s| s.extraction_status == SceneExtractionStatus::DeepExtracted)
            });
        let Some(node) = node else {
            return Ok(Vec::new());
        };
        let blocks = scene_node_to_blocks(module_id, node, &graph.npcs, &graph.scenes);
        if !blocks.is_empty() {
            tracing::info!(
                target: "module_scene",
                module_id,
                scene = %node.title,
                explicit_scene = state.scene_id.is_some(),
                "projected current module scene into turn context"
            );
        }
        Ok(blocks)
    }

    /// §10.1 LIVE linkage: reload the actor's params, re-derive `recompute=live`
    /// values from CURRENT base stats, and persist if anything changed. Idempotent;
    /// call at the start of every turn so derived values stay current even on
    /// blocked/early-returning turns. No-op when there is no stored chargen_spec.
    pub async fn refresh_actor_live_derived(&self, session_id: &str, actor_id: &str) -> Result<bool> {
        let service = RuntimeParameterService::new(self.db.clone());
        if let Some(mut p) = service.load_actor_parameters(session_id, actor_id).await? {
            if chargen::recompute_live_derived(&mut p.sheet_json) {
                chargen::refresh_mechanical_profile(&mut p.mechanical_profile, &p.sheet_json);
                service.upsert_actor_parameters(&p).await?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Ensure an NPC's parameter is on its card: if absent, run the hybrid ladder
    /// (T1 source > T2 archetype > T3 persona-judge) and write the result. Provisional
    /// (T3) values bypass the strict materialization gate by writing straight to
    /// sheet_json. Returns the value, or None when the gate is off / no LLM / no card.
    pub async fn ensure_npc_parameter(&self, session_id: &str, ruleset_id: &str, npc: &npc_synth::NpcPersona,
        bucket: &str, param: &str, check_context: &str) -> anyhow::Result<Option<serde_json::Value>> {
        if !npc_synth::persona_synthesis_enabled() { return Ok(None); }
        let service = trpg_params::RuntimeParameterService::new(self.db.clone());
        // §Phase3 bug-fix: load_actor_parameters返回None时（NPC行尚未在runtime_actor_parameters中）
        // 使用ensure_actor_parameters按需创建行，再继续现搓。否则opposed prepass调用此方法时
        // 由于npc_athena等真实NPC id尚未存在而提前返回Ok(None)、无法合成防御参数。
        let world_tick = self.current_world_time(session_id).await.map(|t| t.world_tick).unwrap_or_default();
        let mut p = match service.load_actor_parameters(session_id, &npc.actor_id).await? {
            Some(p) => p,
            None => service.ensure_actor_parameters(session_id, ruleset_id, &npc.actor_id, trpg_model::ActorKind::Npc, world_tick).await?,
        };
        // per-parameter cache: already on the card?
        if let Some(v) = p.sheet_json.pointer(&format!("/{bucket}/{param}")).cloned() { return Ok(Some(v)); }
        // T1: a source value may already be on the card (e.g. under /source/<param>).
        let source_value = p.sheet_json.pointer(&format!("/source/{param}")).cloned();
        // T2: no archetype-stat source index yet (RecommendedArchetype has fit_tags, not stat params) -> empty -> falls to T3.
        let archetypes: Vec<(String, serde_json::Value)> = Vec::new();
        let llm = match trpg_llm::LlmConfig::from_env().ok().and_then(|cfg| trpg_llm::OpenAiCompatibleClient::new(cfg).ok()) {
            Some(c) => c, None => return Ok(None),
        };
        let synth = npc_synth::resolve_param_tiered(source_value, &archetypes, &llm, npc, param, check_context, ruleset_id).await?;
        npc_synth::write_synthesized_param(&mut p.sheet_json, bucket, param, &synth);
        // B1: mechanical_profile 是 sheet 的物化视图;contest 读它,故写完立即重投影,
        // 否则现搓值停在 sheet_json、结算读不到。refresh 不动 npc_param_provenance 数据。
        chargen::refresh_mechanical_profile(&mut p.mechanical_profile, &p.sheet_json);
        service.upsert_actor_parameters(&p).await?;
        Ok(Some(synth.value))
    }

    /// §Task5 Pre-resolution: resolve THIS turn's check NPC into an NpcPersona by
    /// re-personaing the single Phase-1 `npc.opposition` slot for the CURRENT scene's
    /// NPC. Loads the module graph (single source of truth: the module bundle row,
    /// mirroring `module_scene_blocks_for_turn`), picks the active scene (explicit
    /// `state.scene_id` else first DeepExtracted), takes the FIRST `referenced_npc_ids`
    /// entry, and resolves it against `graph.npcs` (name + body|summary, mirroring
    /// `scene_node_to_blocks`). fail-closed: no module / no scene / no NPC → None.
    /// (Per-NPC actor ids are Phase 2; here we reuse `npc.opposition`.)
    pub async fn current_check_npc_persona(
        &self,
        request: &ContextRequest,
        state: &RuntimeState,
    ) -> Option<npc_synth::NpcPersona> {
        let module_id = request.module_id.as_deref().or(state.module_id.as_deref())?;
        let graph = self.db.load_module_graph(module_id).await.ok().flatten()?;
        // Active scene: explicit state.scene_id else first DeepExtracted (mirror
        // module_scene_blocks_for_turn). fail-closed: none -> None.
        let node = state
            .scene_id
            .as_deref()
            .and_then(|sid| graph.scenes.iter().find(|s| s.node_id == sid))
            .or_else(|| {
                graph
                    .scenes
                    .iter()
                    .find(|s| s.extraction_status == SceneExtractionStatus::DeepExtracted)
            })?;
        // FIRST referenced NPC of the scene, resolved against graph.npcs by id
        // (mirror scene_node_to_blocks: name + body|summary).
        let npc_id = node.referenced_npc_ids.first()?;
        let v = graph
            .npcs
            .iter()
            .find(|v| v.get("id").and_then(|x| x.as_str()) == Some(npc_id.as_str()))?;
        let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let prose = v
            .get("body")
            .or_else(|| v.get("summary"))
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        if name.trim().is_empty() && prose.trim().is_empty() {
            return None;
        }
        Some(npc_synth::NpcPersona { actor_id: "npc.opposition".to_string(), name, prose })
    }

    /// §Phase3 §4.1 对抗预 pass 物料：当前场景在场的全部 NPC 作为 persona 列表，
    /// 用其在 ModuleGraph 中的**真实 id**（非 `npc.opposition` 占位符，治 spec §6③
    /// actor-id 对齐坑）。mirrors `current_check_npc_persona` 的场景选择（explicit
    /// scene_id else first DeepExtracted）与 graph.npcs 解析（name + body|summary）。
    /// fail-closed：无模组 / 无场景 / 无在场 NPC → 空 Vec。
    pub async fn scene_npc_personas(
        &self,
        request: &ContextRequest,
        state: &RuntimeState,
    ) -> Vec<npc_synth::NpcPersona> {
        let Some(module_id) = request.module_id.as_deref().or(state.module_id.as_deref()) else { return Vec::new() };
        let Some(graph) = self.db.load_module_graph(module_id).await.ok().flatten() else { return Vec::new() };
        let node = state
            .scene_id
            .as_deref()
            .and_then(|sid| graph.scenes.iter().find(|s| s.node_id == sid))
            .or_else(|| graph.scenes.iter().find(|s| s.extraction_status == SceneExtractionStatus::DeepExtracted));
        let Some(node) = node else { return Vec::new() };
        node.referenced_npc_ids
            .iter()
            .filter_map(|npc_id| {
                let v = graph.npcs.iter().find(|v| v.get("id").and_then(|x| x.as_str()) == Some(npc_id.as_str()))?;
                let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let prose = v.get("body").or_else(|| v.get("summary")).and_then(|x| x.as_str()).unwrap_or("").to_string();
                if name.trim().is_empty() && prose.trim().is_empty() { return None; }
                // 真实 graph id 作 actor_id（per-NPC 卡，治占位符串台）。
                Some(npc_synth::NpcPersona { actor_id: npc_id.clone(), name, prose })
            })
            .collect()
    }

    /// §Task5/7 Map the typed semantic action kind to the NPC parameter the contest
    /// will need from the opposition. Loads the rule kernel to get compare model:
    /// meet_or_beat → defense DV; roll_under → dodge skill.
    /// Returns `(bucket, param)` or None for kinds that need no NPC param.
    pub async fn check_param_need(&self, ruleset_id: &str, action_kind: &SituationActionKind) -> Option<(String, String)> {
        let compare = self.db.load_rule_kernel(ruleset_id).await.ok().flatten()
            .and_then(|k| k.dice_core.get("compare").and_then(|v| v.as_str()).map(str::to_string))
            .unwrap_or_default();
        map_check_param_need(action_kind, &compare)
    }

    /// §Phase3 §4.1 攻击→防御键映射（对抗预 pass 专用）：预 pass 已语义判定本回合
    /// 是攻击对手，故 action_kind 恒 Attack，只需按 kernel.compare 取该规则集的防御
    /// 键（meet_or_beat→stats.defense / roll_under→skills.dodge）。复用同一份
    /// `map_check_param_need` 数据映射，零 per-ruleset 硬编码。无 kernel / 无映射 → None。
    pub async fn attack_defense_param(&self, ruleset_id: &str) -> Option<(String, String)> {
        self.check_param_need(ruleset_id, &SituationActionKind::Attack).await
    }

    /// §Task5 Fire-and-forget wrapper: synthesize+write the NPC param BEFORE contest
    /// resolution. Ignores the Option result and never propagates errors up the turn
    /// (logged, not returned) so a failed synthesis can never block the turn.
    pub async fn prepare_npc_for_check(
        &self,
        session_id: &str,
        ruleset_id: &str,
        npc: &npc_synth::NpcPersona,
        bucket: &str,
        param: &str,
        check_context: &str,
    ) -> Result<()> {
        match self
            .ensure_npc_parameter(session_id, ruleset_id, npc, bucket, param, check_context)
            .await
        {
            Ok(_) => {}
            Err(e) => tracing::warn!(
                target: "npc_synth",
                error = %e,
                actor_id = %npc.actor_id,
                bucket,
                param,
                "prepare_npc_for_check: synthesis failed (turn continues)"
            ),
        }
        Ok(())
    }

    /// Growth/advancement entry point (CLI command + GM-agent tool both call this).
    /// Loads the actor's params, mutates a track or base value in `sheet_json`, then
    /// re-derives (live linkage) and persists. A0: engine moves the value only —
    /// the GM/player decides the rule-specific amount. Returns Ok(false) if no params.
    pub async fn apply_track_change(
        &self, session_id: &str, actor_id: &str,
        bucket: &str, id: &str, op: &str, amount: f64,
        text: Option<&str>, kind: Option<&str>, category: Option<&str>,
    ) -> Result<bool> {
        let service = RuntimeParameterService::new(self.db.clone());
        let Some(mut p) = service.load_actor_parameters(session_id, actor_id).await? else { return Ok(false); };
        chargen::apply_track_change_to_sheet(&mut p.sheet_json, bucket, id, op, amount, text, kind, category)
            .map_err(|e| anyhow!("apply_track_change: {e}"))?;
        chargen::recompute_live_derived(&mut p.sheet_json);
        chargen::refresh_mechanical_profile(&mut p.mechanical_profile, &p.sheet_json);
        service.upsert_actor_parameters(&p).await?;
        Ok(true)
    }

    async fn actor_parameter_blocks_for_turn(&self, request: &ContextRequest, current_input: Option<&str>) -> Result<Vec<ContextBlock>> {
        let world_tick = self.current_world_time(&request.session_id).await.map(|t| t.world_tick).unwrap_or_default();
        let service = RuntimeParameterService::new(self.db.clone());
        let actor_id = request.viewer.actor_id.as_deref().unwrap_or("pc.current");
        let _ = service.ensure_actor_parameters(&request.session_id, &request.ruleset_id, actor_id, ActorKind::PlayerCharacter, world_tick).await;
        // §10.1 LIVE linkage also runs here (context build for resolving turns);
        // refresh_actor_live_derived() additionally runs at turn START so even
        // early-returning (blocked) turns keep derived values current.
        let _ = self.refresh_actor_live_derived(&request.session_id, actor_id).await;
        let active_frame_exists = self.db.list_active_state_frames(&request.session_id, 1).await.map(|v| !v.is_empty()).unwrap_or(false);
        if active_frame_exists || current_input.map(mentions_runtime_npc).unwrap_or(false) {
            let _ = service.ensure_actor_parameters(&request.session_id, &request.ruleset_id, "npc.opposition", ActorKind::Npc, world_tick).await;
        }
        let block = service.actor_parameters_context_block(&request.session_id, world_tick).await?;
        Ok(vec![block])
    }

    async fn ability_blocks_for_turn(&self, request: &ContextRequest) -> Result<Vec<ContextBlock>> {
        let world_tick = self.current_world_time(&request.session_id).await.map(|t| t.world_tick).unwrap_or_default();
        let block = AbilityService::new(self.db.clone(), self.search.clone()).ability_context_block(&request.session_id, world_tick).await?;
        Ok(vec![block])
    }

    async fn rule_binding_blocks_for_turn(&self, request: &ContextRequest) -> Result<Vec<ContextBlock>> {
        let world_tick = self.current_world_time(&request.session_id).await.map(|t| t.world_tick).unwrap_or_default();
        let block = SemanticRuleBindingService::from_env(self.db.clone(), self.search.clone()).rule_binding_context_block(&request.session_id, world_tick).await?;
        Ok(vec![block])
    }


    async fn player_value_referee_blocks_for_turn(&self, request: &ContextRequest) -> Result<Vec<ContextBlock>> {
        let verifications = self.db.list_recent_player_value_verifications(&request.session_id, 8).await?;
        if verifications.is_empty() { return Ok(vec![]); }
        let mut block = ContextBlock::new(
            format!("player_value_referee.recent.{}", request.session_id),
            BlockKind::PlayerValueVerification,
            "Recent Player-Supplied Value Verifications",
            BlockContent::Json(json!({
                "policy": "rules_first_not_rules_lawyer",
                "guidance": "Player-supplied mechanical numbers are claims. Verify against rules/tables or mark provisional/table override before using. Do not ask players to supply weapon damage, DV/DC, or remaining HP if the system can verify or track it.",
                "verifications": verifications,
            })),
            Visibility::GmOnly,
            Stability::TurnDynamic,
            CacheZone::DynamicTail,
            Scope { scope_type: ScopeType::Session, scope_id: request.session_id.clone() },
            118,
        );
        block.tags = vec!["player_value_referee".into(), "rules_first".into(), "table_override_policy".into()];
        block.expires_at_turn = Some(request.turn_id.clone());
        block.load_reason = Some("recent_player_value_verifications".into());
        Ok(vec![block])
    }

    async fn materialization_blocks_for_turn(&self, request: &ContextRequest) -> Result<Vec<ContextBlock>> {
        let world_tick = self.current_world_time(&request.session_id).await.map(|t| t.world_tick).unwrap_or_default();
        let block = MaterializationService::from_env(self.db.clone(), self.search.clone()).materialization_context_block(&request.session_id, world_tick).await?;
        Ok(vec![block])
    }

    async fn object_blocks_for_turn(&self, request: &ContextRequest) -> Result<Vec<ContextBlock>> {
        let world_tick = self.current_world_time(&request.session_id).await.map(|t| t.world_tick).unwrap_or_default();
        let block = ObjectService::new(self.db.clone()).object_context_block(&request.session_id, None, request.viewer.actor_id.as_deref().unwrap_or("pc.current"), world_tick).await?;
        Ok(vec![block])
    }

    async fn world_time_blocks_for_turn(&self, request: &ContextRequest) -> Result<Vec<ContextBlock>> {
        let service = WorldTimeService::new(self.db.clone());
        let state = service.ensure_session_time(&request.session_id, Some(&request.session_id)).await?;
        let watermark = self.db.get_context_watermark(&request.session_id).await?.unwrap_or_default();
        let since_tick = watermark.last_compiled_world_tick;
        let since_seq = watermark.last_compiled_event_seq;
        let events = self.db.list_world_events_since(&request.session_id, since_tick, since_seq, std::env::var("TRPG_WORLD_TIME_CONTEXT_EVENT_LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(24)).await.unwrap_or_default();
        let mut blocks = vec![world_time_block(&state, &request.turn_id)];
        if !events.is_empty() {
            blocks.push(world_events_since_block(&request.session_id, &request.turn_id, since_tick, since_seq, &events));
        }
        Ok(blocks)
    }

    pub fn to_prompt_bands(compiled: &CompiledContext, history: Vec<ChatMessage>, user_message: impl Into<String>) -> PromptBands {
        PromptBands {
            system_prefix: compiled.prefix_text.clone(),
            semi_stable_context: compiled.pinned_text.clone(),
            dynamic_tail: compiled.dynamic_text.clone(),
            history,
            user_message: user_message.into(),
        }
    }

    pub async fn character_creation_messages(
        &self,
        ruleset_id: &str,
        module_id: Option<&str>,
        user_preferences: &str,
    ) -> Result<Vec<ChatMessage>> {
        let character_pack = self.db.load_character_onboarding_pack(ruleset_id).await?;
        let template = match character_pack.as_ref() {
            Some(pack) => pack.sheet_template.clone(),
            None => self.db.load_character_template(ruleset_id).await?
                .ok_or_else(|| anyhow!("no character template or CharacterOnboardingPack found for ruleset {ruleset_id}; run parse-all first"))?,
        };
        let template_json = serde_json::to_string_pretty(&template)?;
        let character_pack_json = match &character_pack {
            Some(pack) => serde_json::to_string_pretty(pack)?,
            None => "null".to_string(),
        };
        let module_note = match module_id {
            Some(id) => format!("Current module: {id}. Fit the character into the module without revealing GM-only secrets."),
            None => "No module selected.".to_string(),
        };
        let pack_note = if character_pack.is_some() {
            "CharacterOnboardingPack is available. Use its creation_flows, derived_formula_pack, starter_character_pack, runtime_bindings, and validation_profile as the authority for legal character creation."
        } else {
            "CharacterOnboardingPack is missing. Stay within the CharacterTemplate only and mark mechanics that need Rule Steward lookup instead of inventing formulas or resource values."
        };
        Ok(vec![
            system("You create TRPG characters interactively and safely. Use the supplied CharacterOnboardingPack and CharacterTemplate. You may propose creative identity details, but all mechanical fields, derived values, resources, equipment, and abilities must be source-backed by the pack/template or marked missing. Output readable prose first, then a fenced JSON block named character_draft. Do not reveal GM-only module secrets."),
            user(format!("CharacterTemplate:\n```json\n{template_json}\n```\n\nCharacterOnboardingPack:\n```json\n{character_pack_json}\n```\n\n{pack_note}\n\n{module_note}\n\nUser preferences / partial choices:\n{user_preferences}\n\nCreate a character draft. Ask only essential follow-up questions if required; otherwise produce a usable draft. Include a compact validation_notes section listing unresolved source-backed mechanics, if any.")),
        ])
    }

    pub async fn play_turn_messages(
        &self,
        compiled: &CompiledContext,
        user_input: &str,
    ) -> Result<Vec<ChatMessage>> {
        Ok(vec![
            system(compiled.prefix_text.clone()),
            system(player_facing_output_contract()),
            user(format!("[gm]\n[BP2: Pinned Context]\n{}\n[/gm]", compiled.pinned_text)),
            user(format!("[gm]\n[BP3: Dynamic Context]\n{}\n[/gm]\n\n[Player Input]\n{}", compiled.dynamic_text, user_input)),
        ])
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedContext {
    pub prefix_blocks: Vec<ContextBlock>,
    pub pinned_blocks: Vec<ContextBlock>,
    pub dynamic_blocks: Vec<ContextBlock>,
}

#[derive(Debug, Clone, Default)]
pub struct ContextBuilder;

impl ContextBuilder {
    pub fn build(&self, planned: PlannedContext, request: &ContextRequest) -> Result<CompiledContext> {
        let mut prefix = planned.prefix_blocks;
        let mut pinned = planned.pinned_blocks;
        let mut dynamic = planned.dynamic_blocks;
        sort_blocks(&mut prefix);
        sort_blocks(&mut pinned);
        sort_blocks(&mut dynamic);
        trim_budget(&mut prefix, request.token_budget.prefix_max);
        trim_budget(&mut pinned, request.token_budget.pinned_max);
        trim_budget(&mut dynamic, request.token_budget.dynamic_max);

        let prefix_text = render_blocks(&prefix);
        let pinned_text = render_blocks(&pinned);
        let dynamic_text = render_blocks(&dynamic);
        let prefix_hash = sha256_hex(&prefix_text);
        let pinned_hash = sha256_hex(&pinned_text);
        let dynamic_hash = sha256_hex(&dynamic_text);
        let visibility_signature = stable_json_hash(&request.viewer);
        let cache_key = format!("{}:{}:{}", &visibility_signature[..20.min(visibility_signature.len())], &prefix_hash[..20.min(prefix_hash.len())], &pinned_hash[..20.min(pinned_hash.len())]);
        let token_estimate = prefix.iter().chain(pinned.iter()).chain(dynamic.iter()).map(|b| b.token_estimate.unwrap_or(0)).sum();
        let block_version_ids = prefix.iter().chain(pinned.iter()).chain(dynamic.iter()).map(|b| format!("{}@{}", b.block_id, b.version)).collect();
        Ok(CompiledContext { prefix_blocks: prefix, pinned_blocks: pinned, dynamic_blocks: dynamic, prefix_text, pinned_text, dynamic_text, prefix_hash, pinned_hash, dynamic_hash, visibility_signature, cache_key, token_estimate, block_version_ids })
    }
}

pub fn plan_blocks(blocks: Vec<ContextBlock>, state: &RuntimeState, request: &ContextRequest) -> PlannedContext {
    let mut prefix = Vec::new();
    let mut pinned = Vec::new();
    let mut dynamic = Vec::new();
    for mut block in blocks {
        if block.cache_zone == CacheZone::NeverPrompt {
            continue;
        }
        if !scope_matches(&block.scope, state, request) && !block.tags.iter().any(|t| t == "resident") {
            continue;
        }
        if block.load_reason.is_none() {
            block.load_reason = Some("planner_scope_match".to_string());
        }
        match block.cache_zone {
            CacheZone::Prefix => prefix.push(block),
            CacheZone::PinnedMiddle => pinned.push(block),
            CacheZone::DynamicTail => dynamic.push(block),
            CacheZone::NeverPrompt => {}
        }
    }
    PlannedContext { prefix_blocks: prefix, pinned_blocks: pinned, dynamic_blocks: dynamic }
}

fn mentions_runtime_npc(input: &str) -> bool { let lower = input.to_lowercase(); ["npc", "scav", "guard", "守卫", "敌", "无人机", "警察", "帮派", "对方", "他", "她", "drone", "enemy", "opposition"].iter().any(|t| lower.contains(t)) }

fn scope_matches(scope: &Scope, state: &RuntimeState, request: &ContextRequest) -> bool {
    match scope.scope_type {
        ScopeType::Global => true,
        ScopeType::Ruleset => scope.scope_id == request.ruleset_id || scope.scope_id == state.ruleset_id,
        ScopeType::Module => request.module_id.as_deref() == Some(scope.scope_id.as_str()) || state.module_id.as_deref() == Some(scope.scope_id.as_str()),
        ScopeType::Chapter => state.chapter_id.as_deref() == Some(scope.scope_id.as_str()),
        ScopeType::Mission => state.mission_id.as_deref() == Some(scope.scope_id.as_str()),
        ScopeType::Scene => state.scene_id.as_deref() == Some(scope.scope_id.as_str()),
        ScopeType::Location => state.location_id.as_deref() == Some(scope.scope_id.as_str()),
        ScopeType::Npc => state.active_npc_ids.iter().any(|id| id == &scope.scope_id),
        ScopeType::Session => scope.scope_id == request.session_id,
        ScopeType::Turn => scope.scope_id == request.turn_id,
        ScopeType::Material => state.pending_material_refs.iter().chain(state.active_material_refs.iter()).any(|id| id == &scope.scope_id),
        ScopeType::Campaign | ScopeType::Character | ScopeType::Object => true,
    }
}


fn player_facing_output_contract() -> String {
    "Player-facing output contract:\n\
- The response is visible to the player. Use GM-only context only to adjudicate; do not reveal GM-only names, hidden identities, motives, future scenes, or unrevealed module facts.\n\
- If a lookup block says a name/entity is redacted or undiscovered, do not guess or reveal the redacted term. Describe it by player-known features instead.\n\
- Only name entities, NPCs, factions, locations, and secrets that the player has learned in-fiction or stated themselves.\n\
- The player describes fictional action only. Do not ask the player to roll dice, report totals, supply weapon damage, target numbers, remaining HP/SP/AC/SAN/Chaos, or rules-table values. Rust tools own checks, dice, damage/effects, and state writes.\n\
- If a Rust tool result is provided in [roll]...[/roll], the roll/effect already happened. Narrate its fictional consequence; do not re-request the roll or invent additional mechanical changes.\n\
- Keep out-of-fiction adjudication out of plain prose. If a short meta/status note is unavoidable, wrap it in [system]...[/system]. If a visible dice summary is necessary, wrap it in [roll]...[/roll]. Never place GM-only secrets or internal scratch text in player-visible output; [gm]...[/gm] is reserved for internal context only.\n\
- For uncertain technical, analytical, stealth, combat, social-pressure, hacking, investigation, or risky actions: resolve through provided Rust mechanical context if present; otherwise describe the fiction and stakes without exposing formulas/DV/DC/stat math.\n\
- Do not end every turn with a numbered menu. Vary the cadence: sometimes offer concise options, sometimes ask one focused question, sometimes end on pure fiction and wait for the player."
        .to_string()
}

fn project_visibility(block: ContextBlock, viewer: &VisibilityProfile) -> Option<ContextBlock> {
    match (&viewer.viewer_kind, &block.visibility) {
        (ViewerKind::Gm, _) if viewer.can_see_gm_only => Some(block),
        (ViewerKind::System, _) => Some(block),
        (_, Visibility::Public) | (_, Visibility::PlayerVisible) => Some(block),
        (ViewerKind::Npc, Visibility::NpcPrivate) => Some(block),
        _ => None,
    }
}

fn sort_blocks(blocks: &mut Vec<ContextBlock>) {
    blocks.sort_by(|a, b| {
        b.priority.cmp(&a.priority)
            .then_with(|| scope_rank(&b.scope).cmp(&scope_rank(&a.scope)))
            .then_with(|| a.block_id.cmp(&b.block_id))
            .then_with(|| a.version.cmp(&b.version))
    });
}

fn scope_rank(scope: &Scope) -> i32 {
    match scope.scope_type {
        ScopeType::Turn => 100,
        ScopeType::Scene => 90,
        ScopeType::Location | ScopeType::Npc => 80,
        ScopeType::Mission | ScopeType::Chapter => 70,
        ScopeType::Module => 60,
        ScopeType::Ruleset => 50,
        ScopeType::Global => 10,
        _ => 20,
    }
}

fn trim_budget(blocks: &mut Vec<ContextBlock>, max_tokens: u32) {
    let mut total = 0;
    blocks.retain(|b| {
        let est = b.token_estimate.unwrap_or(0);
        if total + est <= max_tokens {
            total += est;
            true
        } else {
            false
        }
    });
}

fn render_blocks(blocks: &[ContextBlock]) -> String {
    let mut out = String::new();
    for block in blocks {
        let title = redact_gm_only_prompt_text(&block.title, block.visibility);
        let content = redact_gm_only_prompt_text(&block.content.render_text(), block.visibility);
        out.push_str(&format!("

---
block_id: {}
kind: {}
visibility: {}
cache_zone: {}
priority: {}
---
# {}

{}
",
            block.block_id,
            block.kind.as_str(),
            block.visibility.as_str(),
            block.cache_zone.as_str(),
            block.priority,
            title,
            content
        ));
    }
    out.trim().to_string()
}

fn redact_gm_only_prompt_text(input: &str, visibility: Visibility) -> String {
    let redact = std::env::var("TRPG_REDACT_GM_ONLY_PROMPT_TERMS")
        .map(|v| !matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no"))
        .unwrap_or(true);
    if !redact || visibility != Visibility::GmOnly {
        return input.to_string();
    }
    let terms = std::env::var("TRPG_SECRET_TERM_OVERRIDES")
        .unwrap_or_else(|_| "Athena,Shelob".to_string());
    let mut out = input.to_string();
    for term in terms.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        out = out.replace(term, "[undiscovered entity]");
    }
    out
}

fn dedupe_blocks(blocks: &mut Vec<ContextBlock>) {
    let mut seen = HashSet::new();
    blocks.retain(|b| seen.insert((b.block_id.clone(), b.version)));
}


#[derive(Debug, Clone, Serialize, Deserialize)]
struct MechanicalSignal {
    has_signal: bool,
    has_specific_target: bool,
    target_kind: Option<String>,
    target_value: Option<i32>,
    mentioned_terms: Vec<String>,
}

fn detect_mechanical_signal(user_input: &str, assistant_output: &str) -> MechanicalSignal {
    let text = format!("{}\n{}", user_input, assistant_output);
    let mut mentioned_terms = Vec::new();
    let lower = text.to_lowercase();
    for term in ["dv", "dc", "tn", "检定", "difficulty", "check", "roll", "1d10", "d10", "d100", "2d6", "stealth", "basic tech", "combat", "attack", "damage"] {
        if lower.contains(term) { mentioned_terms.push(term.to_string()); }
    }
    let re = Regex::new(r"(?i)\b(DV|DC|TN)\s*[:=]?\s*(\d{1,3})\b").ok();
    let mut target_kind = None;
    let mut target_value = None;
    if let Some(re) = re {
        if let Some(caps) = re.captures(&text) {
            target_kind = caps.get(1).map(|m| m.as_str().to_ascii_uppercase());
            target_value = caps.get(2).and_then(|m| m.as_str().parse::<i32>().ok());
        }
    }
    if target_value.is_none() {
        if let Ok(re) = Regex::new(r#"(?i)"target"\s*:\s*(\d{1,3})"#) {
            if let Some(caps) = re.captures(&text) {
                target_kind = Some("TARGET".into());
                target_value = caps.get(1).and_then(|m| m.as_str().parse::<i32>().ok());
            }
        }
    }
    MechanicalSignal {
        has_signal: !mentioned_terms.is_empty() || target_kind.is_some(),
        has_specific_target: target_kind.is_some() && target_value.is_some(),
        target_kind,
        target_value,
        mentioned_terms,
    }
}

fn check_target_signal(contract: &CheckContract) -> Option<(String, i32)> {
    match &contract.target {
        CheckTargetModel::StaticNumber { value, label } => {
            let upper = label.to_ascii_uppercase();
            let kind = if upper.contains("DV") { "DV" } else if upper.contains("DC") { "DC" } else if upper.contains("TN") { "TN" } else { "TARGET" };
            Some((kind.to_string(), *value))
        }
        CheckTargetModel::SuccessCount { threshold } => Some(("SUCCESSES".into(), *threshold)),
        _ => None,
    }
}

fn extract_source_refs_from_lookup_events(events: &[LookupEvent]) -> Vec<SourceRef> {
    let mut refs = Vec::new();
    for event in events {
        if let Some(hits) = event.source_hits.as_array() {
            for hit in hits.iter().take(5) {
                if let Some(arr) = hit.get("source_refs").and_then(|v| v.as_array()) {
                    for src in arr.iter().take(3) {
                        if let Ok(source_ref) = serde_json::from_value::<SourceRef>(src.clone()) {
                            refs.push(source_ref);
                        }
                    }
                }
            }
        }
    }
    refs.sort_by(|a, b| format!("{}:{:?}:{:?}", a.source_id, a.page, a.anchor_id).cmp(&format!("{}:{:?}:{:?}", b.source_id, b.page, b.anchor_id)));
    refs.dedup_by(|a, b| a.source_id == b.source_id && a.page == b.page && a.anchor_id == b.anchor_id);
    refs
}

fn top_search_hit_titles(events: &[LookupEvent], limit: usize) -> Vec<String> {
    let mut titles = Vec::new();
    for event in events {
        if let Some(hits) = event.source_hits.as_array() {
            for hit in hits {
                if let Some(title) = hit.get("title").and_then(|v| v.as_str()) {
                    if !title.trim().is_empty() { titles.push(title.trim().to_string()); }
                }
                if titles.len() >= limit { return titles; }
            }
        }
    }
    titles
}

fn evidence_score(signal: &MechanicalSignal, refs: &[SourceRef], events: &[LookupEvent], risk_flags: &[String]) -> f32 {
    let mut score: f32 = 0.20;
    if !events.is_empty() { score += 0.20; }
    if !refs.is_empty() { score += 0.25; }
    if signal.has_signal { score += 0.15; }
    if signal.has_specific_target { score += 0.15; }
    if events.iter().any(|e| e.result_status == "source_backed" || e.result_status == "hit") { score += 0.10; }
    score -= (risk_flags.len() as f32) * 0.08;
    score.clamp(0.0, 1.0)
}

fn infer_packet_type(user_input: &str, assistant_output: &str) -> String {
    let lower = format!("{}\n{}", user_input, assistant_output).to_lowercase();
    if lower.contains("combat") || lower.contains("attack") || lower.contains("damage") || lower.contains("战斗") || lower.contains("攻击") { "combat_ruling".into() }
    else if lower.contains("netrun") || lower.contains("hack") || lower.contains("黑") { "netrunning_or_hacking".into() }
    else if lower.contains("stealth") || lower.contains("潜行") { "skill_check_stealth".into() }
    else if lower.contains("tech") || lower.contains("修") || lower.contains("线缆") || lower.contains("无人机") { "skill_check_tech".into() }
    else { "table_ruling".into() }
}

fn safe_packet_key(packet_type: &str, user_input: &str, signal: &MechanicalSignal) -> String {
    let mut parts = Vec::new();
    parts.push(packet_type.trim_matches('_').to_ascii_lowercase());
    if let (Some(kind), Some(value)) = (&signal.target_kind, signal.target_value) {
        parts.push(format!("{}{}", kind.to_ascii_lowercase(), value));
    }
    for word in user_input.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).take(8) {
        let cleaned: String = word.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        if !cleaned.is_empty() { parts.push(cleaned); }
    }
    let joined = parts.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("_");
    let key: String = joined.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(96).collect();
    if key.is_empty() { "table_ruling".into() } else { key.trim_matches('_').to_string() }
}

fn learning_candidate_title(user_input: &str, signal: &MechanicalSignal) -> String {
    let mut title = user_input.chars().take(80).collect::<String>();
    if let (Some(kind), Some(value)) = (&signal.target_kind, signal.target_value) {
        title = format!("{} {} — {}", kind, value, title);
    }
    title
}

/// §Phase4 当前场景投影：把一个 deep-extracted 场景节点的 read_aloud/gm_notes
/// + 在场 NPC(name/summary)语义并入一个 SceneStatic/DynamicTail/scope=Scene 的块，
/// 并打上 `expires_at_scene = node_id`(为持久化路径预留；本内存路径实际失效靠
/// 调用方的 scope_matches + 每回合重投影，不依赖该字段做持久化过滤)。
/// SkeletonOnly 场景返回空 Vec——降级交给 materializer，本函数不产块(fail-closed)。
/// 选模组入口场景 node_id：spine.entry_node_id（须在 scenes 中）→ 首个 DeepExtracted → scenes[0]。空→None。
/// §Task5 Pure mapping: typed semantic action kind → the `(bucket, param)` the
/// contest will need from the opposition NPC. None for kinds needing no NPC param.
/// Semantic over keyword (philosophy §2): match on the typed enum, not strings.
fn map_check_param_need(action_kind: &SituationActionKind, compare: &str) -> Option<(String, String)> {
    use SituationActionKind::*;
    // KNOWN-GAP(本期范围外,combat-attack 对抗 = spec §9 外):当玩家动作是被动
    // Defend/Dodge/TakeCover 时,"对手该测的键"语义上应是 NPC 的攻击技能(Fighting/
    // Firearms)而非 NPC 的 dodge——方向反。本期 live 走玩家主动 Hide/潜行(stealth_family
    // → perception,方向正确),Defend/Dodge 走 combat 路径本期不盖章对抗(故此 gap 潜伏不发作)。
    // 跟进:把被动防御动作单独映射到 NPC 攻击技能,或暂移出 attack_family。
    let attack_family = matches!(action_kind,
        Attack | UnderAttack | Counterattack | EnemyInitiatedConflict | SceneEntersConflict
        | Defend | Dodge | TakeCover | CastOrUsePower);
    // 潜行/盗窃/对抗社交 + 冲突中调查(NPC 在场时的调查被其警觉对抗)→ 对手用察觉对抗。
    let stealth_family = matches!(action_kind,
        Hide | Hack | DisableDevice | Intimidate | Negotiate | InvestigateDuringConflict);
    if attack_family {
        return Some(if compare == "meet_or_beat" {
            ("stats".to_string(), "defense".to_string())
        } else {
            ("skills".to_string(), "dodge".to_string())
        });
    }
    if stealth_family {
        return Some(("skills".to_string(), "perception".to_string()));
    }
    None
}

/// 对抗契约判定:必须同时有 target_actor 与 opponent_tested_parameter。
pub fn contract_is_opposed(c: &trpg_model::CheckContract) -> bool {
    c.target_actor.is_some() && c.opponent_tested_parameter.is_some()
}

/// 盖章:把对抗所需的 target_actor + 防御方 tested key 写进契约(B2 通道)。
/// 攻击方 tested key 走 contract.tested_parameter(named 路径已设)。纯函数,易测。
pub fn stamp_opposed_check(
    check: &mut trpg_model::CheckContract,
    npc: &npc_synth::NpcPersona,
    _bucket: &str,
    param: &str,
) {
    check.target_actor = Some(trpg_model::ActorRef {
        actor_id: npc.actor_id.clone(),
        actor_kind: trpg_model::ActorKind::Npc,
        display_name: Some(npc.name.clone()),
    });
    check.opponent_tested_parameter = Some(trpg_model::TestedParameter {
        domain: None,
        key: param.to_string(),
        label: param.to_string(),
    });
}

pub fn module_entry_scene_id(graph: &trpg_model::ModuleGraph) -> Option<String> {
    use trpg_model::SceneExtractionStatus;
    if let Some(id) = graph.spine.get("entry_node_id").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()) {
        if graph.scenes.iter().any(|s| s.node_id == id) {
            return Some(id.to_string());
        }
    }
    if let Some(s) = graph.scenes.iter().find(|s| s.extraction_status == SceneExtractionStatus::DeepExtracted) {
        return Some(s.node_id.clone());
    }
    graph.scenes.first().map(|s| s.node_id.clone())
}

/// 决定本回合生效的 scene_id：已有非空则保留；否则仅当有 module 时用载入值。
fn resolve_turn_scene_id(state_scene: Option<&str>, module_id: Option<&str>, loaded: Option<String>) -> Option<String> {
    match state_scene {
        Some(s) if !s.trim().is_empty() => Some(s.to_string()),
        _ => if module_id.is_some() { loaded } else { None },
    }
}

/// SkeletonOnly 降级块内容（N2）：投影骨架元数据 + 禁编造指令，不含正文。
fn skeleton_fallback_block(module_id: &str, n: &ScenarioNode, scenes: &[ScenarioNode]) -> ContextBlock {
    let mut body = format!("【场景骨架·未深抽】status=SkeletonOnly\n标题：{}\n", n.title);
    if !n.summary.trim().is_empty() {
        body.push_str(&format!("摘要：{}\n", n.summary));
    }
    match (n.page_start, n.page_end) {
        (Some(s), Some(e)) => body.push_str(&format!("页码：{s}–{e}\n")),
        (Some(s), None)    => body.push_str(&format!("页码：{s}\n")),
        _                  => {}
    }
    // 已知实体 id（骨架阶段 LLM 已识别的引用，供 GM 检索用）
    let mut refs: Vec<String> = Vec::new();
    refs.extend(n.referenced_npc_ids.iter().map(|id| format!("npc:{id}")));
    refs.extend(n.referenced_clue_ids.iter().map(|id| format!("clue:{id}")));
    refs.extend(n.referenced_location_ids.iter().map(|id| format!("loc:{id}")));
    refs.extend(n.referenced_encounter_ids.iter().map(|id| format!("enc:{id}")));
    if !refs.is_empty() {
        body.push_str(&format!("已知实体：{}\n", refs.join(", ")));
    }
    // 出口（与 DeepExtracted 路径相同的 fail-closed 投影）
    let exits: Vec<String> = n.links.iter().filter_map(|l| {
        let title = scenes.iter().find(|s| s.node_id == l.to_node_id).map(|s| s.title.as_str()).unwrap_or("");
        if title.trim().is_empty() { None } else { Some(format!("- {title} → {}", l.reason)) }
    }).collect();
    if !exits.is_empty() {
        body.push_str(&format!("【已知出口】\n{}\n", exits.join("\n")));
    }
    // GM 防编造指令
    body.push_str("\n⚠️ 此场景尚未深抽：叙述具体内容前先检索源文档；不要编造 read_aloud/数值/人物细节。\n");
    let mut block = ContextBlock::new(
        format!("module.{module_id}.scene.{}.skeleton", n.node_id),
        BlockKind::SceneStatic,
        format!("{}（骨架）", n.title),
        BlockContent::Text(body),
        Visibility::GmOnly,
        Stability::SceneStable,
        CacheZone::DynamicTail,
        Scope { scope_type: ScopeType::Scene, scope_id: n.node_id.clone() },
        20, // 低 token：骨架块只有元数据，不占满 context
    );
    block.expires_at_scene = Some(n.node_id.clone());
    block.load_reason = Some("skeleton_fallback".into());
    block.tags = vec!["module_scene".into(), "scene_skeleton".into(), "skeleton_only".into()];
    block
}

/// N3: DeepExtracted 当前场景正文块的缓存区，受 `TRPG_SCENE_DEEP_BLOCK_CACHE_ZONE` 控制。
/// 默认 `dynamic_tail`=当前行为（零变化）；设 `pinned_middle` 时正文（read_aloud/gm_notes/
/// fixed exits）落 PinnedMiddle + expires_at_scene，使切场景才变 pinned_hash、普通输入
/// 只变 dynamic_hash（更优缓存）。无法识别的值 fail-closed 回退 DynamicTail（不编造）。
/// 注意：仅作用于 DeepExtracted 正文块；SkeletonOnly 降级块（N2，临时降级）恒 DynamicTail。
fn scene_deep_block_cache_zone() -> CacheZone {
    match std::env::var("TRPG_SCENE_DEEP_BLOCK_CACHE_ZONE") {
        Ok(v) if v.trim().eq_ignore_ascii_case("pinned_middle") => CacheZone::PinnedMiddle,
        _ => CacheZone::DynamicTail,
    }
}

fn scene_node_to_blocks(
    module_id: &str,
    n: &ScenarioNode,
    npcs: &[serde_json::Value],
    scenes: &[ScenarioNode],
) -> Vec<ContextBlock> {
    if n.extraction_status != SceneExtractionStatus::DeepExtracted {
        // N2: SkeletonOnly（及其他非 DeepExtracted 状态）产出轻量降级块，
        // 防 GM 完全失去"我在哪个场景"。fail-closed：不含正文、不编造。
        return vec![skeleton_fallback_block(module_id, n, scenes)];
    }
    let mut body = String::new();
    if let Some(ra) = &n.read_aloud {
        if !ra.trim().is_empty() {
            body.push_str("【可念】\n");
            body.push_str(ra);
            body.push('\n');
        }
    }
    if let Some(g) = &n.gm_notes {
        if !g.trim().is_empty() {
            body.push_str("\n【GM】\n");
            body.push_str(g);
            body.push('\n');
        }
    }
    for id in &n.referenced_npc_ids {
        if let Some(v) = npcs
            .iter()
            .find(|v| v.get("id").and_then(|x| x.as_str()) == Some(id.as_str()))
        {
            let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("");
            // Deep-extracted entities (reader DEEP_SYS) carry prose in `body`;
            // shallow/index entities use `summary`. Prefer body, fall back.
            let sum = v.get("body").or_else(|| v.get("summary"))
                .and_then(|x| x.as_str()).unwrap_or("");
            body.push_str(&format!("\n[NPC] {name}: {sum}"));
        }
    }
    // §Phase4 出口投影：把本场景 links 解析成『目标场景标题 → 通往理由』，让 GM
    // 知道当前场景可去哪里(场景导航的语义素材)。目标 node_id 在 scenes 中查不到、
    // 或目标无标题 → 跳过该出口(fail-closed，不投空标题、不编造)。
    let exits: Vec<String> = n
        .links
        .iter()
        .filter_map(|l| {
            let title = scenes
                .iter()
                .find(|s| s.node_id == l.to_node_id)
                .map(|s| s.title.as_str())
                .unwrap_or("");
            if title.trim().is_empty() {
                None
            } else {
                Some(format!("- {} → {}", title, l.reason))
            }
        })
        .collect();
    if !exits.is_empty() {
        body.push_str("\n【出口】\n");
        body.push_str(&exits.join("\n"));
        body.push('\n');
    }
    let mut block = ContextBlock::new(
        format!("module.{module_id}.scene.{}", n.node_id),
        BlockKind::SceneStatic,
        n.title.clone(),
        BlockContent::Text(body),
        Visibility::GmOnly,
        Stability::SceneStable,
        // N3: 默认 DynamicTail（零变化）；TRPG_SCENE_DEEP_BLOCK_CACHE_ZONE=pinned_middle
        // 时落 PinnedMiddle，配合下方 expires_at_scene 让切场景才变 pinned_hash。
        scene_deep_block_cache_zone(),
        Scope { scope_type: ScopeType::Scene, scope_id: n.node_id.clone() },
        60,
    );
    block.expires_at_scene = Some(n.node_id.clone());
    block.load_reason = Some("current_scene_deep_projection".into());
    block.tags = vec!["module_scene".into(), "scene_static".into(), "deep_extracted".into()];
    let mut blocks = vec![block];
    // C3：当前场景机制意图索引（BP2）。渲染纯函数在 trpg-model（scene_intents_text，
    // 每条一行 id|description|tested_parameter|difficulty 摘要，不含 effect_policy 全文
    // ——结算才用，C4 按 intent_id 从图谱取）。空 intents → None → 不出块（旧模组零变化）。
    if let Some(text) = trpg_model::scene_intents_text(&n.scene_mechanics) {
        let mut b = ContextBlock::new(
            format!("module.{module_id}.scene.{}.mechanics", n.node_id),
            BlockKind::SceneStatic, // 复用既有 kind：不动 trpg-db 的 block_kind 词表
            format!("{} —— 场景机制意图", n.title),
            BlockContent::Text(text),
            Visibility::GmOnly,
            Stability::SceneStable,
            CacheZone::PinnedMiddle, // 骨架契约：pinned_hash 场景内稳定，场景切换换块
            Scope { scope_type: ScopeType::Scene, scope_id: n.node_id.clone() },
            58, // 略低于正文块（60）
        );
        b.expires_at_scene = Some(n.node_id.clone());
        b.load_reason = Some("current_scene_mechanic_intents".into());
        b.tags = vec!["module_scene".into(), "scene_mechanics".into()];
        blocks.push(b);
    }
    blocks
}

fn make_ruling_summary(user_input: &str, assistant_output: &str, signal: &MechanicalSignal, hit_titles: &[String]) -> String {
    let target = match (&signal.target_kind, signal.target_value) {
        (Some(k), Some(v)) => format!(" Detected target: {k}{v}."),
        _ => String::new(),
    };
    let evidence = if hit_titles.is_empty() { String::new() } else { format!(" Evidence hits: {}.", hit_titles.join("; ")) };
    format!(
        "Player action: {}\nRuling excerpt: {}{}{}",
        user_input.chars().take(360).collect::<String>(),
        assistant_output.chars().take(700).collect::<String>(),
        target,
        evidence
    )
}

fn make_learning_candidate_summary(user_input: &str, assistant_output: &str, signal: &MechanicalSignal, hit_titles: &[String]) -> String {
    let mut summary = String::new();
    summary.push_str("# Learned Packet Candidate\n\n");
    summary.push_str("Review-gated candidate generated from an actual table turn. Do not treat as stable knowledge until approved.\n\n");
    summary.push_str("## Player Action\n");
    summary.push_str(&user_input.chars().take(500).collect::<String>());
    summary.push_str("\n\n## GM Ruling Excerpt\n");
    summary.push_str(&assistant_output.chars().take(900).collect::<String>());
    if let (Some(kind), Some(value)) = (&signal.target_kind, signal.target_value) {
        summary.push_str(&format!("\n\n## Detected Target\n{} {}\n", kind, value));
    }
    if !hit_titles.is_empty() {
        summary.push_str("\n## Source Hits\n");
        for title in hit_titles { summary.push_str(&format!("- {}\n", title)); }
    }
    summary
}

fn learning_audit_enabled() -> bool {
    std::env::var("TRPG_LEARNING_AUDIT").map(|v| v != "0" && v.to_lowercase() != "false").unwrap_or(true)
}

fn learning_auto_promote_enabled() -> bool {
    std::env::var("TRPG_LEARNING_AUTO_PROMOTE").map(|v| v == "1" || v.to_lowercase() == "true").unwrap_or(false)
}


fn infer_semantic_choice_option<'a>(gate: &'a InteractionGate, input: &str) -> Option<&'a ActionOption> {
    let lower = input.to_lowercase();
    let is_direction_gate = matches!(gate.gate_kind, GateKind::ChooseActionMode)
        || gate.advice_refs.iter().any(|r| r.contains("direction_gate") || r.contains("stalemate"))
        || gate.bound_action_summary.contains("direction")
        || gate.prompt_public.contains("改变局面")
        || gate.prompt_public.contains("局势");
    if !is_direction_gate {
        let deescalate_or_end = contains_any_choice(&lower, &["停火", "战斗结束", "结束战斗", "和解", "别打", "谈判", "投降", "ceasefire", "end combat", "stand down", "negotiate", "surrender"]);
        if deescalate_or_end {
            return gate.allowed_options.iter()
                .find(|opt| opt.option_id.contains("take") || opt.option_id.contains("decline") || opt.option_id.contains("dodge"))
                .or_else(|| gate.allowed_options.first());
        }
        return None;
    }
    let want_continue = lower.starts_with("/roll")
        || contains_any_choice(&lower, &["继续", "继续攻击", "继续开火", "持续压制", "猛打", "正面对抗", "keep firing", "continue", "press", "attack", "shoot", "fire"]);
    let want_escape = contains_any_choice(&lower, &["撤", "逃", "脱离", "离开", "撤退", "逃跑", "withdraw", "retreat", "escape", "disengage"]);
    let want_talk = contains_any_choice(&lower, &["谈", "和解", "投降", "停火", "威慑", "negotiate", "truce", "surrender", "deescalate", "talk"]);
    let want_objective = contains_any_choice(&lower, &["目标", "电缆", "源头", "服务器", "推进", "处理", "objective", "cable", "server", "source", "push"]);
    let want_change = contains_any_choice(&lower, &["换", "改变", "绕", "战术", "change", "different", "flank", "new tactic"]);
    let wanted_ids: &[&str] = if want_continue { &["continue_conflict", "continue_pressure"] }
        else if want_escape { &["escape_or_disengage", "withdraw_or_chase", "leave_node"] }
        else if want_talk { &["deescalate", "new_leverage", "pressure", "accept_or_leave"] }
        else if want_objective { &["push_objective", "change_method", "deep_search_cost"] }
        else if want_change { &["change_tactic", "change_method"] }
        else { &[] };
    for id in wanted_ids {
        if let Some(option) = gate.allowed_options.iter().find(|opt| opt.option_id == *id) { return Some(option); }
    }
    // v1.10.1 safety valve: a direction gate must never wedge the player when
    // they clearly mean to continue pressure/attack. If custom profiles renamed
    // the option id, resolve to the first direction option instead of reprompting.
    if want_continue || want_escape || want_talk || want_objective || want_change { return gate.allowed_options.first(); }
    None
}

fn default_choice_after_reprompt(gate: &InteractionGate) -> Option<&ActionOption> {
    gate.allowed_options.iter().find(|opt| opt.is_default)
        .or_else(|| gate.allowed_options.iter().find(|opt| opt.option_id == "continue_conflict" || opt.option_id == "continue_pressure"))
        .or_else(|| gate.allowed_options.first())
}

fn contains_any_choice(text: &str, terms: &[&str]) -> bool { terms.iter().any(|term| text.contains(term)) }

fn turn_orchestrator_enabled() -> bool {
    std::env::var("TRPG_TURN_ORCHESTRATOR_ENABLE_V17").map(|v| v != "0" && v.to_ascii_lowercase() != "false").unwrap_or(true)
}

fn ability_kernel_enabled() -> bool {
    std::env::var("TRPG_ABILITY_KERNEL_ENABLE_V19").map(|v| v != "0" && v.to_ascii_lowercase() != "false").unwrap_or(true)
}

fn object_kernel_enabled() -> bool {
    std::env::var("TRPG_OBJECT_KERNEL_ENABLE_V16").map(|v| v != "0" && v.to_ascii_lowercase() != "false").unwrap_or(true)
}

fn conflict_agent_v10_enabled() -> bool {
    std::env::var("TRPG_SITUATION_ORCHESTRATOR_ENABLE_V11")
        .or_else(|_| std::env::var("TRPG_CONFLICT_AGENT_ENABLE_V10"))
        .map(|v| v != "0" && v.to_ascii_lowercase() != "false")
        .unwrap_or(true)
}

fn runtime_auto_search_enabled() -> bool {
    !std::env::var("TRPG_RUNTIME_AUTO_SEARCH").map(|v| v == "false" || v == "0").unwrap_or(false)
}

fn looks_rule_or_module_sensitive(input: &str) -> bool {
    let lowered = input.to_lowercase();
    let keywords = [
        "check", "roll", "rule", "dc", "dv", "skill", "attack", "combat", "damage", "heal",
        "netrun", "hack", "athena", "drone", "cable", "scene", "npc", "clue", "where", "how",
        "检定", "判定", "规则", "技能", "攻击", "战斗", "伤害", "治疗", "黑客", "无人机", "线缆", "线索", "调查", "地点", "怎么", "能不能",
    ];
    lowered.split_whitespace().count() > 8 || keywords.iter().any(|kw| lowered.contains(kw))
}

fn looks_like_gate_help_or_question(input: &str) -> bool {
    let lower = input.to_lowercase();
    let terms = ["怎么投", "投什么", "骰什么", "怎么算", "不会", "不懂", "?", "？", "how", "what do i roll", "what should i roll"];
    terms.iter().any(|term| lower.contains(term))
}

fn should_pin_search_hit_to_scene(hit: &SearchHit, state: &RuntimeState) -> bool {
    if state.scene_id.is_none() { return false; }
    if std::env::var("TRPG_RUNTIME_AUTOPIN_SCENE_RULES").map(|v| v == "false" || v == "0").unwrap_or(false) {
        return false;
    }
    let domain = hit.domain.as_str();
    let kind = hit.logical_kind.to_lowercase();
    let stage = hit.metadata.get("learning_stage").and_then(|v| v.as_str()).unwrap_or_default();
    let is_stable_learned = domain == "learned" && matches!(stage, "stable" | "memorized" | "used_once");
    let current_scene_hit = state.scene_id.as_ref().map(|scene_id| {
        hit.scopes.get("scene_id") == Some(scene_id) || hit.scopes.get("scope_id") == Some(scene_id)
    }).unwrap_or(false);
    is_stable_learned
        || current_scene_hit
        || domain == "modules"
        || kind.contains("rule")
        || kind.contains("procedure")
        || kind.contains("combat")
        || kind.contains("check")
        || kind.contains("scene")
        || kind.contains("encounter")
}


/// Pure builder for the forced technical-assessment plan (DB-less, unit-tested).
/// `kernel_dice` is the ruleset's core die from the parsed kernel; None => no
/// forced check (narration-only), never a hardcoded fallback die. Target stays
/// UnknownUntilLookup + NoMechanicalOpposition so the Contest/Opposition Kernel
/// resolves it per-ruleset (CoC 1d100 roll-under vs the actor's real skill;
/// Cyberpunk its real DV) — never a fabricated 1d10 / DV14 / TECH check.
fn build_forced_tech_plan(request: &ContextRequest, user_input: &str, kernel_dice: Option<&str>) -> AgentTurnPlan {
    let mut plan = AgentTurnPlan::new(&request.session_id, &request.turn_id, &request.ruleset_id, request.module_id.clone());
    let auto_roll = system_rolls_visible_policy();
    plan.kind = if auto_roll { TurnPlanKind::GmRollThenNarrate } else { TurnPlanKind::AskPlayerRoll };
    plan.input_summary = user_input.chars().take(320).collect();
    plan.reasoning_summary = if auto_roll {
        "v1.10.1 deterministic route invariant: technical/object risk assessment requires a mechanical check before narration; product mode owns the dice and auto-resolves it.".into()
    } else {
        "v1.10.1 deterministic route invariant: technical/object risk assessment requires a player-triggered check before narration; do not accept player-reported totals.".into()
    };
    plan.policy_layers_used = vec!["semantic_route_stability.v1_10_1".into(), "dice_visibility.v1".into()];
    plan.advice_refs = vec!["forced_technical_risk_assessment.v1_10_1".into()];
    let dice = match kernel_dice.map(str::trim).filter(|s| !s.is_empty()) {
        Some(d) => d.to_string(),
        None => return plan,
    };
    let actor_id = request.viewer.actor_id.clone().unwrap_or_else(|| "pc.current".into());
    let check = CheckContract {
        check_id: format!("check_{}", Uuid::new_v4().simple()),
        session_id: request.session_id.clone(),
        turn_id: request.turn_id.clone(),
        ruleset_id: request.ruleset_id.clone(),
        module_id: request.module_id.clone(),
        initiator: ActorRef { actor_id, actor_kind: ActorKind::PlayerCharacter, display_name: Some("current PC".into()) },
        target_actor: None,
        opposition: OppositionModel::NoMechanicalOpposition,
        action_summary: user_input.chars().take(500).collect(),
        intent_kind: "technical_analysis_or_intervention".into(),
        check_label: "technical analysis or intervention check".into(),
        dice_expression: dice,
        modifiers: vec![],
        target: CheckTargetModel::UnknownUntilLookup,
        tested_parameter: None,
        opponent_tested_parameter: None,
        actor_snapshot_ids: vec![],
        source_refs: vec![],
        learned_packet_ids: vec![],
        roll_visibility: if auto_roll { RollVisibility::PublicGmRoll } else { RollVisibility::PlayerRollRequired },
        roll_authority: if auto_roll { RollAuthority::System } else { RollAuthority::Player },
        disclosure: RollDisclosurePolicy::for_visibility(if auto_roll { RollVisibility::PublicGmRoll } else { RollVisibility::PlayerRollRequired }),
        stakes: CheckStakes {
            before_roll_public: "This is more than surface observation: success can reveal whether the object can be handled safely; failure may cost time, expose you, or trigger a complication.".into(),
            success_public: "You identify a safe technical approach or a concrete exploit path.".into(),
            failure_public: "You cannot confirm a safe path before pressure increases, or you misread a hidden risk.".into(),
            critical_public: None,
            fumble_public: None,
            success_patches_allowed: vec![],
            failure_patches_allowed: vec![],
            irreversible: false,
        },
        confidence: RulingConfidence::Medium,
        ruling_status: RulingStatus::Provisional,
        advice_refs: vec!["forced_technical_risk_assessment.v1_10_1".into()],
        expires_at_turn: Some(request.turn_id.clone()),
    };
    plan.check = Some(check);
    plan
}

/// Pick the named-check parameter from materialization target (kind, label)
/// pairs: prefer the ActorProfile demand (the tested value, e.g. "Sanity") over
/// the CheckTarget demand (e.g. "Sanity roll"). The label is used verbatim — the
/// LLM already named the parameter; the resolver canonicalizes it against the
/// kernel by meaning, so no hardcoded suffix list. Pure, unit-tested directly.
fn pick_named_parameter(items: &[(MaterialTargetKind, &str)]) -> Option<String> {
    let pick = |k: MaterialTargetKind| items.iter()
        .find(|(tk, _)| *tk == k)
        .map(|(_, l)| l.trim().to_string())
        .filter(|s| !s.is_empty());
    pick(MaterialTargetKind::ActorProfile).or_else(|| pick(MaterialTargetKind::CheckTarget))
}

/// Robustness fallback for the named-check route: the orchestrator's classifier
/// is one of several independent (non-deterministic) `classify_turn` calls and
/// occasionally fails to name the tested parameter. The materialization pass —
/// computed earlier in the same turn — usually still does, via an ActorProfile /
/// CheckTarget demand. Reading it makes the route fire reliably without a 5th
/// classifier call. Returns None when no parameter/check was materialized.
pub fn materialization_named_parameter(result: &MaterializationTurnResult) -> Option<String> {
    let items: Vec<(MaterialTargetKind, &str)> = result.demands.iter()
        .map(|d| (d.target_kind, d.target_label.as_str())).collect();
    pick_named_parameter(&items)
}

/// Pure builder for an EXPLICIT named check (e.g. "make a Sanity roll", "roll
/// Spot Hidden"). `named_label` is the parameter the semantic layer named; it is
/// carried as the contract's `tested_parameter` so the Contest/Opposition Kernel
/// reads the actor's REAL value for that parameter (canonicalized at resolve
/// time against the kernel's resource_tracks + the actor's stat/skill keys) — and
/// so the matching resource track's on_outcome fires deliberately. `kernel_dice`
/// is the ruleset's core die; None => narration-only (never a fabricated die).
fn build_named_check_plan(request: &ContextRequest, user_input: &str, named_label: &str, kernel_dice: Option<&str>) -> AgentTurnPlan {
    let mut plan = AgentTurnPlan::new(&request.session_id, &request.turn_id, &request.ruleset_id, request.module_id.clone());
    let auto_roll = system_rolls_visible_policy();
    plan.kind = if auto_roll { TurnPlanKind::GmRollThenNarrate } else { TurnPlanKind::AskPlayerRoll };
    plan.input_summary = user_input.chars().take(320).collect();
    plan.reasoning_summary = format!("player explicitly invoked a named rules check ({named_label}); bind it to that parameter and resolve against the actor's real value before narration.");
    plan.policy_layers_used = vec!["explicit_named_check.v1".into(), "dice_visibility.v1".into()];
    plan.advice_refs = vec!["explicit_named_check.v1".into()];
    let dice = match kernel_dice.map(str::trim).filter(|s| !s.is_empty()) {
        Some(d) => d.to_string(),
        None => return plan,
    };
    let actor_id = request.viewer.actor_id.clone().unwrap_or_else(|| "pc.current".into());
    let check = CheckContract {
        check_id: format!("check_{}", Uuid::new_v4().simple()),
        session_id: request.session_id.clone(),
        turn_id: request.turn_id.clone(),
        ruleset_id: request.ruleset_id.clone(),
        module_id: request.module_id.clone(),
        initiator: ActorRef { actor_id, actor_kind: ActorKind::PlayerCharacter, display_name: Some("current PC".into()) },
        target_actor: None,
        opposition: OppositionModel::NoMechanicalOpposition,
        action_summary: user_input.chars().take(500).collect(),
        intent_kind: "named_parameter_check".into(),
        check_label: format!("{named_label} check"),
        dice_expression: dice,
        modifiers: vec![],
        target: CheckTargetModel::UnknownUntilLookup,
        // The named parameter IS the tested parameter; the resolver canonicalizes
        // `key` to the kernel track / actor skill / stat and reads the real value.
        tested_parameter: Some(TestedParameter { domain: None, key: named_label.to_string(), label: named_label.to_string() }),
        opponent_tested_parameter: None,
        actor_snapshot_ids: vec![],
        source_refs: vec![],
        learned_packet_ids: vec![],
        roll_visibility: if auto_roll { RollVisibility::PublicGmRoll } else { RollVisibility::PlayerRollRequired },
        roll_authority: if auto_roll { RollAuthority::System } else { RollAuthority::Player },
        disclosure: RollDisclosurePolicy::for_visibility(if auto_roll { RollVisibility::PublicGmRoll } else { RollVisibility::PlayerRollRequired }),
        stakes: CheckStakes {
            before_roll_public: format!("A {named_label} check is called for; its outcome carries the consequences the rules attach to this check."),
            success_public: format!("You hold steady: the {named_label} check succeeds."),
            failure_public: format!("The {named_label} check fails, and its rules-defined consequence follows."),
            critical_public: None,
            fumble_public: None,
            success_patches_allowed: vec![],
            failure_patches_allowed: vec![],
            irreversible: false,
        },
        confidence: RulingConfidence::Medium,
        ruling_status: RulingStatus::Provisional,
        advice_refs: vec!["explicit_named_check.v1".into()],
        expires_at_turn: Some(request.turn_id.clone()),
    };
    plan.check = Some(check);
    plan
}

fn agent_plan_block(plan: &AgentTurnPlan) -> ContextBlock {
    let mut block = ContextBlock::new(
        format!("agent.turn_plan.{}", plan.plan_id),
        BlockKind::AgentPlan,
        "Rust GM Agent Turn Plan",
        BlockContent::Json(json!(plan)),
        Visibility::GmOnly,
        Stability::TurnDynamic,
        CacheZone::DynamicTail,
        Scope { scope_type: ScopeType::Turn, scope_id: plan.turn_id.clone() },
        135,
    );
    block.tags = vec!["agent".into(), "turn_plan".into(), format!("plan_kind:{:?}", plan.kind)];
    block.expires_at_turn = Some(plan.turn_id.clone());
    block.load_reason = Some("agent_turn_plan".into());
    block
}

fn learned_packet_block(packet: LearnedPacket) -> ContextBlock {
    let mut block = ContextBlock::new(
        format!("learned_packet.{}", packet.packet_id),
        BlockKind::LearnedPacket,
        packet.title.clone(),
        BlockContent::Json(json!({
            "packet_type": packet.packet_type.clone(),
            "packet_key": packet.packet_key.clone(),
            "summary": packet.summary.clone(),
            "packet": packet.packet_json.clone(),
            "ruling_status": "learned",
            "learning_stage": packet.learning_stage.as_str(),
            "use_count": packet.use_count,
        })),
        packet.visibility,
        Stability::SceneStable,
        packet.cache_zone,
        Scope::ruleset(&packet.ruleset_id),
        match packet.learning_stage {
            LearningStage::Memorized => 110,
            LearningStage::Stable => 95,
            LearningStage::UsedOnce => 80,
            _ => 60,
        },
    );
    block.tags = vec!["learned_packet".into(), packet.packet_type.clone(), packet.packet_key.clone(), packet.learning_stage.as_str().into()];
    block.source_refs = packet.source_refs;
    block.load_reason = Some("learned_packet_relevant_context".into());
    block
}

fn memory_snapshot_block(snapshot: &MemorySnapshot) -> ContextBlock {
    let mut block = ContextBlock::new(
        format!("{}.v{}", snapshot.snapshot_id, snapshot.version),
        BlockKind::MemorySnapshot,
        &snapshot.title,
        BlockContent::Markdown(snapshot.summary_markdown.clone()),
        snapshot.visibility,
        Stability::SceneStable,
        CacheZone::PinnedMiddle,
        snapshot.scope.clone(),
        75,
    );
    block.version = snapshot.version;
    block.content_hash = snapshot.content_hash.clone();
    block.token_estimate = snapshot.token_estimate;
    block.tags = vec!["memory".into(), "snapshot".into(), "cache_stable".into()];
    block.load_reason = Some("memory_snapshot_pinned".into());
    block
}

fn retrieved_memory_block(session_id: &str, turn_id: &str, result: &MemoryRetrievalResult) -> ContextBlock {
    let mut content = String::from("# Retrieved GM Memory\n\nThese memories were retrieved for this turn only. Treat them as recall aids, not as newly committed state.\n");
    if !result.facts.is_empty() {
        content.push_str("\n## Durable facts\n");
        for fact in &result.facts {
            content.push_str(&format!("- {}\n", fact.summary));
        }
    }
    if !result.events.is_empty() {
        content.push_str("\n## Relevant previous events\n");
        for event in &result.events {
            content.push_str(&format!("- {}\n", event.summary));
        }
    }
    let mut block = ContextBlock::new(
        format!("memory.retrieved.{session_id}.{turn_id}"),
        BlockKind::RetrievedMemory,
        "Retrieved GM Memory",
        BlockContent::Markdown(content),
        Visibility::GmOnly,
        Stability::TurnDynamic,
        CacheZone::DynamicTail,
        Scope { scope_type: ScopeType::Turn, scope_id: turn_id.to_string() },
        105,
    );
    block.tags = vec!["memory".into(), "retrieved".into(), "dynamic".into()];
    block.expires_at_turn = Some(turn_id.to_string());
    block.load_reason = Some("memory_retrieval_current_turn".into());
    block
}


fn actionable_situation_block(brief: &ActionableSituationBrief, turn_id: &str) -> ContextBlock {
    let mut block = ContextBlock::new(
        format!("director.brief.{}", brief.brief_id),
        BlockKind::ActionableSituationBrief,
        "Actionable Situation Brief",
        BlockContent::Json(serde_json::to_value(brief).unwrap_or_else(|_| serde_json::Value::Null)),
        Visibility::PlayerVisible,
        Stability::TurnDynamic,
        CacheZone::DynamicTail,
        Scope { scope_type: ScopeType::Turn, scope_id: turn_id.to_string() },
        110,
    );
    block.tags = vec!["director".into(), "actionable_situation".into(), brief.guidance.level.as_str().into()];
    block.load_reason = Some("actionable_situation_director".into());
    block.expires_at_turn = Some(turn_id.to_string());
    block
}

fn clue_board_block(board: &PlayerFacingClueBoard, turn_id: &str) -> ContextBlock {
    let mut block = ContextBlock::new(
        format!("director.clue_board.{}", board.board_id),
        BlockKind::ClueBoard,
        "Player Facing Clue Board",
        BlockContent::Json(serde_json::to_value(board).unwrap_or_else(|_| serde_json::Value::Null)),
        Visibility::PlayerVisible,
        Stability::SceneStable,
        CacheZone::PinnedMiddle,
        Scope { scope_type: ScopeType::Session, scope_id: board.session_id.clone() },
        95,
    );
    block.tags = vec!["director".into(), "clue_board".into(), "player_facing".into()];
    block.load_reason = Some("actionable_situation_director".into());
    block.expires_at_turn = Some(turn_id.to_string());
    block
}


fn world_time_block(state: &WorldTimeState, turn_id: &str) -> ContextBlock {
    let mut block = ContextBlock::new(
        format!("world_time.{}", state.session_id),
        BlockKind::WorldTime,
        "World Time Spine",
        BlockContent::Json(serde_json::to_value(state).unwrap_or_else(|_| serde_json::Value::Null)),
        Visibility::PlayerVisible,
        Stability::TurnDynamic,
        CacheZone::DynamicTail,
        Scope { scope_type: ScopeType::Session, scope_id: state.session_id.clone() },
        60,
    );
    block.tags = vec!["world_time".into(), state.time_scale.as_str().into(), "dynamic".into()];
    block.expires_at_turn = Some(turn_id.to_string());
    block.load_reason = Some("world_time_spine".into());
    block
}

fn world_events_since_block(session_id: &str, turn_id: &str, since_tick: i64, since_event_seq: i64, events: &[WorldEvent]) -> ContextBlock {
    let mut block = ContextBlock::new(
        format!("world_events_since.{session_id}.{turn_id}"),
        BlockKind::WorldEvent,
        "World Events Since Last Context Watermark",
        BlockContent::Json(json!({"since_tick": since_tick, "since_event_seq": since_event_seq, "events": events})),
        Visibility::GmOnly,
        Stability::TurnDynamic,
        CacheZone::DynamicTail,
        Scope { scope_type: ScopeType::Turn, scope_id: turn_id.to_string() },
        105,
    );
    block.tags = vec!["world_time".into(), "world_events".into(), "incremental".into()];
    block.expires_at_turn = Some(turn_id.to_string());
    block.load_reason = Some("world_time_watermark_incremental_context".into());
    block
}

fn engine_protocol_block() -> ContextBlock {
    let content = "Runtime protocol: use BP1 as resident rules, BP2 as current playable unit, BP3 as per-turn state. LLM may narrate and propose state changes, but deterministic procedures and validators commit mechanical state. Never leak GM-only content into player-visible output; internal context is wrapped as [gm], player-safe system notes as [system], and committed dice/tool results as [roll]. World time is authoritative: LLM must not advance time without a Rust time_advance event/tool. Search hits and learned packets must enter prompt only as ContextBlocks with TTL and visibility. If an action could materially reveal information, avoid danger, bypass an obstacle, change state, spend resources, deal damage, avoid damage, or gain a tactical advantage, use the Rust check/roll/effect context when present; otherwise state the fictional uncertainty and stakes without asking players for dice totals. Player-supplied mechanical numbers are claims, not automatic truth: verify them against rules or relevant object/ability tables; if unsupported or out-of-band, warn and suggest a rules-consistent value; if the table insists, record a table override with a balance warning.";
    let mut block = ContextBlock::new(
        "engine.protocol.runtime".to_string(),
        BlockKind::EngineProtocol,
        "Runtime Protocol",
        BlockContent::Markdown(content.to_string()),
        Visibility::SystemOnly,
        Stability::Immutable,
        CacheZone::Prefix,
        Scope::global(),
        200,
    );
    block.tags = vec!["engine".into(), "resident".into()];
    block
}

fn engine_protocol_block_agent_loop() -> ContextBlock {
    let content = "Runtime protocol (GM agent loop): use BP1 as resident rules, BP2 as current playable unit, BP3 as per-turn state. You own the decision of whether, when, and how to roll: call `roll_check` for system-executed checks (public or secret), call `request_player_roll` to pause the turn and let the player roll personally when the table should feel the die, or narrate without mechanics when fiction is enough. Tools are the only write path to mechanical state: dice, checks, damage/effects, resources, tracks, scene navigation, world time, and memory all commit through tool calls; plain prose changes nothing. Never leak GM-only content into player-visible output; internal context is wrapped as [gm], player-safe system notes as [system], and committed dice/tool results as [roll]. World time is authoritative: use `advance_time`, never narrate time forward mechanically. Player-supplied mechanical numbers are claims, not automatic truth: verify them against rules or relevant object/ability tables via `retrieve_rules`; if unsupported or out-of-band, warn and suggest a rules-consistent value; if the table insists, record a table override with a balance warning.";
    let mut block = ContextBlock::new(
        "engine.protocol.agent_loop".to_string(),
        BlockKind::EngineProtocol,
        "Runtime Protocol (Agent Loop)",
        BlockContent::Markdown(content.to_string()),
        Visibility::SystemOnly,
        Stability::Immutable,
        CacheZone::Prefix,
        Scope::global(),
        200,
    );
    block.tags = vec!["engine".into(), "resident".into(), "agent_loop".into()];
    block
}

fn world_state_block(state: &RuntimeState) -> ContextBlock {
    let mut block = ContextBlock::new(
        "runtime.world_state".to_string(),
        BlockKind::WorldState,
        "Projected World State",
        BlockContent::Json(json!(state)),
        Visibility::GmOnly,
        Stability::TurnDynamic,
        CacheZone::DynamicTail,
        Scope { scope_type: ScopeType::Turn, scope_id: "current".to_string() },
        100,
    );
    block.tags = vec!["world_state".into(), "dynamic".into()];
    block
}

fn dynamic_text_block(block_id: &str, kind: BlockKind, title: &str, text: &str, tags: Vec<&str>) -> ContextBlock {
    let mut block = ContextBlock::new(
        block_id.to_string(),
        kind,
        title,
        BlockContent::Markdown(text.to_string()),
        Visibility::GmOnly,
        Stability::TurnDynamic,
        CacheZone::DynamicTail,
        Scope { scope_type: ScopeType::Turn, scope_id: "current".to_string() },
        120,
    );
    block.tags = tags.into_iter().map(str::to_string).collect();
    block
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiceRoll {
    pub expression: String,
    pub rolls: Vec<i32>,
    pub modifier: i32,
    pub total: i32,
}

/// Pluggable dice backend. The default implementation below is pseudo-random;
/// a future frontend can register a 3D dice service by implementing the same
/// small interface and preserving the returned `DiceRoll` contract.
pub trait DiceRollerPlugin: Send + Sync {
    fn plugin_id(&self) -> &'static str;
    fn roll(&self, expression: &str) -> Result<DiceRoll>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PseudoRandomDiceRoller;

impl DiceRollerPlugin for PseudoRandomDiceRoller {
    fn plugin_id(&self) -> &'static str { "pseudo_random_v1" }

    fn roll(&self, expression: &str) -> Result<DiceRoll> {
        let re = Regex::new(r"(?i)^\s*(\d*)d(\d+)([+-]\d+)?\s*$").unwrap();
        let caps = re.captures(expression).ok_or_else(|| anyhow!("unsupported dice expression: {expression}"))?;
        let n = caps.get(1).map(|m| m.as_str()).unwrap_or("1");
        let n: i32 = if n.is_empty() { 1 } else { n.parse()? };
        let sides: i32 = caps.get(2).unwrap().as_str().parse()?;
        let modifier: i32 = caps.get(3).map(|m| m.as_str().parse()).transpose()?.unwrap_or(0);
        if n <= 0 || n > 100 || sides <= 1 || sides > 1000 { return Err(anyhow!("dice expression outside safety bounds: {expression}")); }
        let mut rng = rand::thread_rng();
        let rolls: Vec<i32> = (0..n).map(|_| rng.gen_range(1..=sides)).collect();
        let total = rolls.iter().sum::<i32>() + modifier;
        Ok(DiceRoll { expression: expression.to_string(), rolls, modifier, total })
    }
}

pub fn roll_dice_with_plugin(plugin: &dyn DiceRollerPlugin, expression: &str) -> Result<DiceRoll> {
    plugin.roll(expression)
}

pub fn roll_dice(expression: &str) -> Result<DiceRoll> {
    PseudoRandomDiceRoller.roll(expression)
}

pub fn validate_character_template_sheet(template: &CharacterTemplate, sheet: &serde_json::Value) -> ValidationReport {
    let mut report = ValidationReport { status: "ok".to_string(), ..Default::default() };
    let obj = match sheet.as_object() {
        Some(o) => o,
        None => {
            report.status = "error".to_string();
            report.errors.push(ValidationMessage { code: "sheet_not_object".into(), message: "Character sheet must be a JSON object.".into(), target: None });
            return report;
        }
    };
    for field in &template.fields {
        if field.required && !obj.contains_key(&field.field_id) {
            report.status = "error".to_string();
            report.errors.push(ValidationMessage { code: "missing_required_field".into(), message: format!("Missing required field: {}", field.field_id), target: Some(field.field_id.clone()) });
        }
    }
    report
}


fn is_effect_or_damage_contract(contract: &CheckContract) -> bool {
    let text = format!("{} {}", contract.intent_kind, contract.check_label).to_ascii_lowercase();
    text.contains("damage") || text.contains("effect") || text.contains("伤害") || text.contains("san") || text.contains("chaos") || text.contains("harm")
}

fn roll_input_available(input: &str) -> bool {
    match parse_roll_text(input) {
        Some(ParsedRollText::DiceExpression(_)) => true,
        Some(ParsedRollText::ReportedTotal(_) | ParsedRollText::ReportedDieAndComponents { .. }) => player_reported_roll_totals_allowed(),
        None => wants_system_roll(input),
    }
}

/// 裸 "roll"/"系统投" 类「让系统代掷」回复的同源判定原语（trpg-gm 头部 gate
/// 守卫复用此函数，与 resolve_roll_input 的回退语义保持单一事实源）。
pub fn wants_system_roll(input: &str) -> bool {
    let lower = input.trim().to_ascii_lowercase();
    if lower == "/roll" || lower == "roll" { return true; }
    let terms = ["你来投", "你帮我投", "系统投", "系统掷", "代投", "帮我掷", "gm roll", "roll for me", "you roll", "system roll", "auto roll"];
    terms.iter().any(|term| lower.contains(term))
}

// 桌面骰权政策谓词的单一事实源在 trpg_model::table_dice_policy;此处再导出
// 以保持 trpg_runtime::system_rolls_visible_policy() 既有公共路径(trpg-gm 在用)。
pub use trpg_model::system_rolls_visible_policy;

pub fn normalize_contract_for_system_roll(contract: &CheckContract) -> CheckContract {
    if !system_rolls_visible_policy() {
        return contract.clone();
    }
    let mut out = contract.clone();
    if out.roll_visibility == RollVisibility::PlayerRollRequired {
        out.roll_visibility = RollVisibility::PublicGmRoll;
        out.roll_authority = RollAuthority::System;
        out.disclosure = RollDisclosurePolicy::for_visibility(RollVisibility::PublicGmRoll);
    }
    out
}

fn player_reported_roll_totals_allowed() -> bool {
    std::env::var("TRPG_PLAYER_REPORTED_ROLL_TOTALS")
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on" | "allow" | "allowed"))
        .unwrap_or(false)
}

fn player_supplied_roll_expressions_allowed() -> bool {
    std::env::var("TRPG_PLAYER_SUPPLIED_ROLL_EXPRESSIONS")
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on" | "allow" | "allowed"))
        .unwrap_or(false)
}

fn env_bool_runtime(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(default)
}

/// Fail-LOUD config guard (fail-closed philosophy). The mechanical spine — check →
/// roll → resolve → effect → writeback — is gated behind these `*_ENABLE_V1xx`
/// flags; if any is OFF the engine silently degrades to narration-only and every
/// check is left "未绑定", which reads as a broken engine. Warn ONCE per process so
/// a trimmed `.env` never again masquerades as a code regression. No behavior
/// change — purely diagnostic, data-driven over the env.
fn warn_if_core_mechanics_disabled() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        const CORE: &[&str] = &[
            "TRPG_TURN_ORCHESTRATOR_ENABLE_V17",
            "TRPG_REAL_MATERIALIZATION_ENABLE_V110",
            "TRPG_CONTEST_KERNEL_ENABLE_V111",
            "TRPG_UNIFIED_ROLL_EFFECT_EXECUTOR_ENABLE_V1121",
            "TRPG_AUTO_RESOLVE_SYSTEM_EFFECT_ROLLS",
        ];
        let off: Vec<&str> = CORE.iter().copied().filter(|k| !env_bool_runtime(k, false)).collect();
        if !off.is_empty() {
            tracing::warn!(
                disabled = ?off,
                "core mechanical kernels are OFF -> checks/rolls will NOT resolve (narration-only mode). \
                 Source the full env (.env.example carries all TRPG_*_ENABLE_V1xx flags) before a real session."
            );
        }
    });
}

fn real_materialization_enabled() -> bool { std::env::var("TRPG_REAL_MATERIALIZATION_ENABLE_V110").ok().map(|v| matches!(v.to_ascii_lowercase().as_str(), "1"|"true"|"yes"|"on")).unwrap_or(true) }

#[cfg(test)]
mod forced_tech_plan_tests {
    use super::*;

    fn req() -> ContextRequest {
        ContextRequest {
            ruleset_id: "call_of_cthulhu_7e".into(),
            module_id: None,
            session_id: "s".into(),
            turn_id: "t".into(),
            viewer: VisibilityProfile::player("p", "pc.current"),
            token_budget: TokenBudget::default(),
        }
    }

    #[test]
    fn forced_tech_uses_kernel_dice_generic_target_no_hardcode() {
        for d in ["1d100", "1d10", "2d6"] {
            let plan = build_forced_tech_plan(&req(), "I examine the strange device", Some(d));
            let check = plan.check.expect("check built when kernel has dice");
            assert_eq!(check.dice_expression, d, "dice must come from the kernel, not a hardcoded 1d10");
            assert!(matches!(check.target, CheckTargetModel::UnknownUntilLookup), "target must stay generic (contest kernel resolves), never StaticNumber(14)");
            assert!(matches!(check.opposition, OppositionModel::NoMechanicalOpposition), "opposition must be generic, never StaticDc(14)");
            let label = check.check_label.to_lowercase();
            assert!(!label.contains("hacking") && !label.contains("repair") && !label.contains("dv"), "no leftover Cyberpunk TECH/hacking/DV label: {}", check.check_label);
        }
    }

    #[test]
    fn forced_tech_no_kernel_dice_is_narration_only() {
        let plan = build_forced_tech_plan(&req(), "I examine the device", None);
        assert!(plan.check.is_none(), "no kernel dice => no fabricated check (narration-only fail-closed)");
    }
}

#[cfg(test)]
mod module_scene_proj_tests {
    use super::*;
    use trpg_model::{BlockKind, CacheZone, ScenarioNode, SceneExtractionStatus};

    #[test]
    fn deep_scene_projects_scenestatic_dynamictail() {
        // N3: 此测试断言默认（env 未设）DynamicTail，须持锁+unset 隔离并行的 env 写测试。
        let _g = DeepZoneEnvGuard::unset();
        let mut n = ScenarioNode::default();
        n.node_id = "loc1".into();
        n.title = "加油站".into();
        n.read_aloud = Some("你们看到一个褪色的广告牌……".into());
        n.gm_notes = Some("老板拉斯藏着钥匙。".into());
        n.extraction_status = SceneExtractionStatus::DeepExtracted;
        n.referenced_npc_ids = vec!["npc1".into()];
        let npcs = vec![serde_json::json!({"id":"npc1","name":"拉斯","summary":"老板"})];
        let blocks = scene_node_to_blocks("mod1", &n, &npcs, &[]);
        assert!(!blocks.is_empty());
        let sb = &blocks[0];
        assert_eq!(sb.kind, BlockKind::SceneStatic);
        assert_eq!(sb.cache_zone, CacheZone::DynamicTail);
        assert_eq!(sb.scope.scope_type, trpg_model::ScopeType::Scene);
        assert_eq!(sb.scope.scope_id, "loc1");
        assert_eq!(sb.expires_at_scene.as_deref(), Some("loc1"));
        let text = sb.content.render_text();
        assert!(text.contains("拉斯"), "在场 NPC 应被并入: {text}");
        assert!(text.contains("褪色的广告牌"), "read_aloud 应并入: {text}");
        assert!(text.contains("钥匙"), "gm_notes 应并入: {text}");
    }

    #[test]
    fn skeleton_scene_projects_nothing() {
        // 旧行为：SkeletonOnly → 空（此测试会在 N2 改动后失败，改动前确认红）
        // N2 改动后此测试预期改为：返回非空降级块。暂保留原名方便 diff 对比。
        let mut n = ScenarioNode::default();
        n.node_id = "loc2".into();
        n.extraction_status = SceneExtractionStatus::SkeletonOnly;
        // After N2: must return a SceneSkeleton fallback block, NOT empty.
        // This assertion will be RED until the implementation is done.
        let blocks = scene_node_to_blocks("mod1", &n, &[], &[]);
        assert!(!blocks.is_empty(), "N2: SkeletonOnly 场景应产出降级块（非空）");
    }

    /// N2: SkeletonOnly 降级块完整验收——不含编造内容，含骨架元数据与指令。
    #[test]
    fn skeleton_scene_fallback_block_has_no_readout_has_instruction() {
        use trpg_model::{BlockKind, CacheZone, ScenarioLink, LinkType};
        let mut n = ScenarioNode::default();
        n.node_id = "sc02".into();
        n.title = "镇中心广场".into();
        n.summary = "玩家抵达镇中心，这里是全镇的心脏。".into();
        n.page_start = Some(12);
        n.page_end = Some(14);
        n.referenced_npc_ids = vec!["npc_mayor".into()];
        n.referenced_clue_ids = vec!["clue_letter".into()];
        // 有出口链接
        n.links = vec![ScenarioLink {
            to_node_id: "sc03".into(),
            reason: "通往邮局".into(),
            clue_id: None,
            link_type: LinkType::Spatial,
            source_anchor: None,
        }];
        n.extraction_status = SceneExtractionStatus::SkeletonOnly;
        // 相邻场景用于解析出口标题
        let mut sc03 = ScenarioNode::default();
        sc03.node_id = "sc03".into();
        sc03.title = "邮局".into();
        let scenes = vec![n.clone(), sc03];

        let blocks = scene_node_to_blocks("mod_test", &n, &[], &scenes);
        assert!(!blocks.is_empty(), "SkeletonOnly 应产降级块");
        let b = &blocks[0];
        // 复用 SceneStatic kind，放 DynamicTail
        assert_eq!(b.kind, BlockKind::SceneStatic, "应复用 SceneStatic kind");
        assert_eq!(b.cache_zone, CacheZone::DynamicTail, "应放 DynamicTail");
        assert_eq!(b.scope.scope_type, trpg_model::ScopeType::Scene);
        assert_eq!(b.scope.scope_id, "sc02");
        assert_eq!(b.expires_at_scene.as_deref(), Some("sc02"));

        let text = b.content.render_text();
        // 含 summary / page 范围 / 出口 / 实体 id / 指令
        assert!(text.contains("镇中心广场"), "应含 title: {text}");
        assert!(text.contains("心脏"), "应含 summary: {text}");
        assert!(text.contains("12"), "应含 page_start: {text}");
        assert!(text.contains("14"), "应含 page_end: {text}");
        assert!(text.contains("npc_mayor"), "应含 referenced_npc_ids: {text}");
        assert!(text.contains("clue_letter"), "应含 referenced_clue_ids: {text}");
        assert!(text.contains("邮局"), "应含出口目标标题: {text}");
        assert!(text.contains("SkeletonOnly"), "应含 status=SkeletonOnly: {text}");
        // 不含实际 read_aloud 内容——指令里提到 "read_aloud" 作关键词是允许的，
        // 但不应有 【可念】 标题（DeepExtracted 路径才产这个标题）。
        assert!(!text.contains("【可念】"), "不应含可念正文段落: {text}");
        // 含 GM 指令防编造
        assert!(text.contains("先检索") || text.contains("检索"), "应含检索指令: {text}");
        assert!(text.contains("编造"), "应含禁止编造提示: {text}");
    }

    /// N2: DeepExtracted 路径行为不变（现有测试守护，新增专项冒烟）。
    #[test]
    fn deep_extracted_path_unchanged_after_n2() {
        let mut n = ScenarioNode::default();
        n.node_id = "loc1".into();
        n.title = "加油站".into();
        n.read_aloud = Some("你们看到一个褪色的广告牌……".into());
        n.gm_notes = Some("老板拉斯藏着钥匙。".into());
        n.extraction_status = SceneExtractionStatus::DeepExtracted;
        n.referenced_npc_ids = vec!["npc1".into()];
        let npcs = vec![serde_json::json!({"id":"npc1","name":"拉斯","summary":"老板"})];
        let blocks = scene_node_to_blocks("mod1", &n, &npcs, &[]);
        assert!(!blocks.is_empty());
        let text = blocks[0].content.render_text();
        assert!(text.contains("褪色的广告牌"), "DeepExtracted read_aloud 应存在: {text}");
        assert!(text.contains("拉斯"), "DeepExtracted NPC 应存在: {text}");
        // 不含 SkeletonOnly 降级指令
        assert!(!text.contains("先检索"), "DeepExtracted 不应含降级指令: {text}");
        assert!(!text.contains("SkeletonOnly"), "DeepExtracted 不应含 status 标记: {text}");
    }

    #[test]
    fn deep_scene_block_includes_exit_titles() {
        use trpg_model::{LinkType, ScenarioLink, ScenarioNode, SceneExtractionStatus};
        let mut entry = ScenarioNode::default();
        entry.node_id = "loc1".into();
        entry.title = "加油站".into();
        entry.read_aloud = Some("你们停车。".into());
        entry.extraction_status = SceneExtractionStatus::DeepExtracted;
        entry.links = vec![ScenarioLink {
            to_node_id: "loc2".into(),
            reason: "主路通往".into(),
            clue_id: None,
            link_type: LinkType::Spatial,
            source_anchor: None,
        }];
        let mut town = ScenarioNode::default();
        town.node_id = "loc2".into();
        town.title = "镇中心".into();
        let scenes = vec![entry.clone(), town];
        let blocks = scene_node_to_blocks("m1", &entry, &[], &scenes);
        let body = match &blocks[0].content {
            trpg_model::BlockContent::Text(t) => t.clone(),
            _ => String::new(),
        };
        assert!(body.contains("镇中心"), "出口应含目标场景标题");
    }

    // ===== C3: 当前场景 intents 投影（BP2）=====

    fn mech_intent(id: &str) -> trpg_model::SceneMechanicIntent {
        trpg_model::SceneMechanicIntent {
            intent_id: id.into(),
            description: "玩家试图强行剪断缆线".into(),
            tested_parameter: "brawling".into(),
            difficulty: Some(serde_json::json!({"kind":"dv","value":13})),
            // 非空 effect_policy：若实现误把全文投影，下方 on_success 断言会抓住。
            effect_policy: trpg_model::EffectPolicy {
                on_success: vec![trpg_model::EffectPatchIntent::CreateFact {
                    target: "scene.lawmen".into(),
                    fact: serde_json::json!({"cable":"cut"}),
                }],
                on_failure: vec![],
            },
            source_anchor: "p.12 原文锚点".into(),
        }
    }

    fn deep_scene_with_intents(node_id: &str) -> ScenarioNode {
        let mut n = ScenarioNode::default();
        n.node_id = node_id.into();
        n.title = "执法者驾到".into();
        n.read_aloud = Some("警笛由远而近。".into());
        n.extraction_status = SceneExtractionStatus::DeepExtracted;
        n.scene_mechanics = vec![mech_intent("homecoming.lawmen.cut_cable_force")];
        n
    }

    #[test]
    fn scene_with_mechanics_projects_intents_block() {
        let n = deep_scene_with_intents("loc1");
        let blocks = scene_node_to_blocks("mod1", &n, &[], &[]);
        assert_eq!(blocks.len(), 2, "正文块 + intents 块");
        let b = &blocks[1];
        assert!(b.block_id.ends_with(".mechanics"), "block_id 应以 .mechanics 结尾: {}", b.block_id);
        assert_eq!(b.cache_zone, CacheZone::PinnedMiddle);
        assert_eq!(b.stability, trpg_model::Stability::SceneStable);
        assert_eq!(b.expires_at_scene.as_deref(), Some("loc1"));
        let text = b.content.render_text();
        assert!(text.contains("homecoming.lawmen.cut_cable_force"), "内容应含 intent_id: {text}");
        assert!(!text.contains("effect_policy"), "effect_policy 全文不投影: {text}");
        assert!(!text.contains("on_success"), "on_success 全文不投影: {text}");
    }

    #[test]
    fn scene_without_mechanics_projects_single_block_unchanged() {
        let mut n = deep_scene_with_intents("loc1");
        n.scene_mechanics = Vec::new(); // 旧模组：无 intents
        let blocks = scene_node_to_blocks("mod1", &n, &[], &[]);
        assert_eq!(blocks.len(), 1, "空 intents → 仍单块（旧模组零变化=fail-closed）");
        let text = blocks[0].content.render_text();
        assert!(!text.contains("机制意图"), "首块内容不得混入机制意图: {text}");
    }

    #[test]
    fn intents_block_bytes_stable_across_calls() {
        let n = deep_scene_with_intents("loc1");
        let first = scene_node_to_blocks("mod1", &n, &[], &[]);
        let second = scene_node_to_blocks("mod1", &n, &[], &[]);
        let a = serde_json::to_vec(&first[1]).expect("intents 块可序列化");
        let b = serde_json::to_vec(&second[1]).expect("intents 块可序列化");
        assert_eq!(a, b, "同节点两次投影的 intents 块字节必须一致（pinned_hash 场景内稳定的函数级前提）");
    }

    // ===== N3: SceneStatic 缓存区可配 PinnedMiddle =====

    // env 是进程级全局：所有读/写 TRPG_SCENE_DEEP_BLOCK_CACHE_ZONE 的测试串行执行，
    // 且每个写测试用 guard 在退出时恢复原值，避免污染默认行为断言（并行跑）。
    static N3_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct DeepZoneEnvGuard {
        prev: Option<String>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }
    impl DeepZoneEnvGuard {
        /// 持锁 + 把 env 设为 value，drop 时恢复原值（None→remove）。
        fn set(value: &str) -> Self {
            let lock = N3_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var("TRPG_SCENE_DEEP_BLOCK_CACHE_ZONE").ok();
            std::env::set_var("TRPG_SCENE_DEEP_BLOCK_CACHE_ZONE", value);
            Self { prev, _lock: lock }
        }
        /// 持锁 + 把 env 清空（模拟"未设"默认），drop 时恢复原值。
        fn unset() -> Self {
            let lock = N3_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var("TRPG_SCENE_DEEP_BLOCK_CACHE_ZONE").ok();
            std::env::remove_var("TRPG_SCENE_DEEP_BLOCK_CACHE_ZONE");
            Self { prev, _lock: lock }
        }
    }
    impl Drop for DeepZoneEnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var("TRPG_SCENE_DEEP_BLOCK_CACHE_ZONE", v),
                None => std::env::remove_var("TRPG_SCENE_DEEP_BLOCK_CACHE_ZONE"),
            }
        }
    }

    fn deep_scene_node(node_id: &str) -> ScenarioNode {
        let mut n = ScenarioNode::default();
        n.node_id = node_id.into();
        n.title = "加油站".into();
        n.read_aloud = Some("你们看到一个褪色的广告牌……".into());
        n.gm_notes = Some("老板拉斯藏着钥匙。".into());
        n.extraction_status = SceneExtractionStatus::DeepExtracted;
        n
    }

    /// N3 默认=零变化：env 未设时 DeepExtracted 正文块必须仍落 DynamicTail，
    /// 且块字节与"显式无 env"投影完全一致（字节回归，证明默认路径未被改动）。
    #[test]
    fn deep_block_default_zone_is_dynamic_tail_byte_regression() {
        let _g = DeepZoneEnvGuard::unset();
        let n = deep_scene_node("loc1");
        let blocks = scene_node_to_blocks("mod1", &n, &[], &[]);
        assert_eq!(
            blocks[0].cache_zone,
            CacheZone::DynamicTail,
            "默认（env 未设）DeepExtracted 正文块必须落 DynamicTail=零行为变化"
        );
        // expires_at_scene 保持不变
        assert_eq!(blocks[0].expires_at_scene.as_deref(), Some("loc1"));
    }

    /// N3：TRPG_SCENE_DEEP_BLOCK_CACHE_ZONE=pinned_middle → DeepExtracted 正文块落
    /// PinnedMiddle + 保留 expires_at_scene（切场景失效语义）。
    #[test]
    fn deep_block_pinned_middle_when_configured() {
        let _g = DeepZoneEnvGuard::set("pinned_middle");
        let n = deep_scene_node("loc1");
        let blocks = scene_node_to_blocks("mod1", &n, &[], &[]);
        let sb = &blocks[0];
        assert_eq!(sb.cache_zone, CacheZone::PinnedMiddle, "pinned_middle 配置下正文块应进 PinnedMiddle");
        assert_eq!(sb.expires_at_scene.as_deref(), Some("loc1"), "仍带 expires_at_scene（切场景才失效）");
        assert_eq!(sb.kind, BlockKind::SceneStatic);
        // 正文内容不变（仅缓存区变）
        let text = sb.content.render_text();
        assert!(text.contains("褪色的广告牌"));
        assert!(text.contains("钥匙"));
    }

    /// N3：大小写/前后空白容错走 pinned_middle；无法识别值 fail-closed 回退 DynamicTail。
    #[test]
    fn deep_block_zone_parsing_is_tolerant_and_fail_closed() {
        {
            let _g = DeepZoneEnvGuard::set("  Pinned_Middle  ");
            let n = deep_scene_node("loc1");
            let blocks = scene_node_to_blocks("mod1", &n, &[], &[]);
            assert_eq!(blocks[0].cache_zone, CacheZone::PinnedMiddle, "大小写+空白容错");
        }
        {
            let _g = DeepZoneEnvGuard::set("garbage_value");
            let n = deep_scene_node("loc1");
            let blocks = scene_node_to_blocks("mod1", &n, &[], &[]);
            assert_eq!(blocks[0].cache_zone, CacheZone::DynamicTail, "无法识别值 fail-closed 回退 DynamicTail");
        }
    }

    /// N3 不变量：SkeletonOnly 降级块（N2，临时降级）即便开 pinned_middle 也恒 DynamicTail。
    #[test]
    fn skeleton_fallback_stays_dynamic_tail_even_when_pinned_middle() {
        let _g = DeepZoneEnvGuard::set("pinned_middle");
        let mut n = ScenarioNode::default();
        n.node_id = "sk1".into();
        n.title = "未深抽场景".into();
        n.extraction_status = SceneExtractionStatus::SkeletonOnly;
        let blocks = scene_node_to_blocks("mod1", &n, &[], &[]);
        assert_eq!(
            blocks[0].cache_zone,
            CacheZone::DynamicTail,
            "SkeletonOnly 降级块是临时降级，不该进 pinned（恒 DynamicTail）"
        );
    }

    /// N3 缓存收益验证：pinned_middle 下，经 ContextBuilder.build 编译——
    /// 切场景 → pinned_hash 变；只改 DynamicTail 输入块 → pinned_hash 不变（dynamic_hash 变）。
    #[test]
    fn pinned_middle_scene_switch_changes_pinned_hash_input_does_not() {
        let _g = DeepZoneEnvGuard::set("pinned_middle");
        let req = ContextRequest {
            ruleset_id: "call_of_cthulhu_7e".into(),
            module_id: Some("mod1".into()),
            session_id: "s".into(),
            turn_id: "t".into(),
            viewer: VisibilityProfile::player("p", "pc.current"),
            token_budget: TokenBudget::default(),
        };
        let builder = ContextBuilder;

        // 一个 DynamicTail 的"玩家输入"块（模拟每回合变化的尾部）。
        let input_block = |text: &str| {
            ContextBlock::new(
                "turn.player_input",
                BlockKind::SceneStatic,
                "玩家输入",
                BlockContent::Text(text.to_string()),
                Visibility::GmOnly,
                Stability::TurnDynamic,
                CacheZone::DynamicTail,
                Scope { scope_type: ScopeType::Global, scope_id: "*".into() },
                10,
            )
        };

        let scene_a = deep_scene_node("loc1");
        let mut scene_b = deep_scene_node("loc2");
        scene_b.title = "镇中心".into();
        scene_b.read_aloud = Some("广场上人来人往。".into());

        let pinned_a = scene_node_to_blocks("mod1", &scene_a, &[], &[]);
        let pinned_b = scene_node_to_blocks("mod1", &scene_b, &[], &[]);
        // 确认正文块确实进了 PinnedMiddle（前提成立）
        assert_eq!(pinned_a[0].cache_zone, CacheZone::PinnedMiddle);
        assert_eq!(pinned_b[0].cache_zone, CacheZone::PinnedMiddle);

        let build = |scene_blocks: &[ContextBlock], input: ContextBlock| {
            let planned = PlannedContext {
                prefix_blocks: vec![],
                pinned_blocks: scene_blocks.to_vec(),
                dynamic_blocks: vec![input],
            };
            builder.build(planned, &req).expect("compile ok")
        };

        let c1 = build(&pinned_a, input_block("我环顾四周"));
        let c2 = build(&pinned_a, input_block("我走向门口")); // 同场景，仅输入变
        let c3 = build(&pinned_b, input_block("我环顾四周")); // 切场景，输入回到原值

        // 1) 只改 DynamicTail 输入：pinned_hash 不变，dynamic_hash 变
        assert_eq!(c1.pinned_hash, c2.pinned_hash, "普通输入只动 dynamic_hash，pinned_hash 应稳定");
        assert_ne!(c1.dynamic_hash, c2.dynamic_hash, "输入变 → dynamic_hash 应变");
        // 2) 切场景：pinned_hash 应变（场景正文进了 pinned 区）
        assert_ne!(c1.pinned_hash, c3.pinned_hash, "切场景 → pinned_hash 应变");
    }

    /// N3 对照：默认 DynamicTail 下，切场景反而是 dynamic_hash 变、pinned_hash 不变
    /// （场景正文在 dynamic 区）——这正是规划里要改善的现状，作对照锚点保留。
    #[test]
    fn default_dynamic_tail_scene_switch_only_changes_dynamic_hash() {
        let _g = DeepZoneEnvGuard::unset();
        let req = ContextRequest {
            ruleset_id: "call_of_cthulhu_7e".into(),
            module_id: Some("mod1".into()),
            session_id: "s".into(),
            turn_id: "t".into(),
            viewer: VisibilityProfile::player("p", "pc.current"),
            token_budget: TokenBudget::default(),
        };
        let builder = ContextBuilder;
        let scene_a = deep_scene_node("loc1");
        let mut scene_b = deep_scene_node("loc2");
        scene_b.read_aloud = Some("广场上人来人往。".into());
        let a = scene_node_to_blocks("mod1", &scene_a, &[], &[]);
        let b = scene_node_to_blocks("mod1", &scene_b, &[], &[]);
        assert_eq!(a[0].cache_zone, CacheZone::DynamicTail, "默认正文在 dynamic 区");

        let build = |blk: &[ContextBlock]| {
            let planned = PlannedContext { prefix_blocks: vec![], pinned_blocks: vec![], dynamic_blocks: blk.to_vec() };
            builder.build(planned, &req).expect("compile ok")
        };
        let ca = build(&a);
        let cb = build(&b);
        assert_eq!(ca.pinned_hash, cb.pinned_hash, "默认下切场景 pinned_hash 不变（正文不在 pinned 区）");
        assert_ne!(ca.dynamic_hash, cb.dynamic_hash, "默认下切场景动 dynamic_hash");
    }

    #[test]
    fn module_entry_scene_id_prefers_spine_then_deep_then_first() {
        use trpg_model::{ModuleGraph, ScenarioNode, SceneExtractionStatus};
        let mk = |id: &str, st: SceneExtractionStatus| {
            let mut n = ScenarioNode::default();
            n.node_id = id.into();
            n.extraction_status = st;
            n
        };
        let mut g = ModuleGraph::default();
        g.scenes = vec![
            mk("preface", SceneExtractionStatus::SkeletonOnly),
            mk("prologue", SceneExtractionStatus::DeepExtracted),
        ];
        g.spine = serde_json::json!({"entry_node_id": "prologue"});
        assert_eq!(module_entry_scene_id(&g).as_deref(), Some("prologue"), "spine.entry_node_id 优先");
        g.spine = serde_json::json!({});
        assert_eq!(module_entry_scene_id(&g).as_deref(), Some("prologue"), "无 spine → 首个 deep");
        g.scenes[1].extraction_status = SceneExtractionStatus::SkeletonOnly;
        assert_eq!(module_entry_scene_id(&g).as_deref(), Some("preface"), "无 deep → scenes[0]");
        assert_eq!(module_entry_scene_id(&ModuleGraph::default()), None, "空 → None");
    }

    #[test]
    fn resolve_turn_scene_id_loads_when_absent_with_module() {
        assert_eq!(resolve_turn_scene_id(Some("loc1"), Some("m"), Some("loaded".into())).as_deref(), Some("loc1"), "已有 scene → 不覆盖");
        assert_eq!(resolve_turn_scene_id(None, Some("m"), Some("loaded".into())).as_deref(), Some("loaded"), "缺+有模组 → 载入");
        assert_eq!(resolve_turn_scene_id(None, None, Some("loaded".into())), None, "无模组 → 不载");
        assert_eq!(resolve_turn_scene_id(Some(""), Some("m"), Some("loaded".into())).as_deref(), Some("loaded"), "空串视为缺");
    }

    #[test]
    fn map_check_param_need_selects_by_compare() {
        use trpg_model::SituationActionKind::*;
        // meet_or_beat:攻击族 → 被动 defense DV。
        assert_eq!(map_check_param_need(&Attack, "meet_or_beat"), Some(("stats".into(),"defense".into())));
        // roll_under:攻击族 → 对抗技能 dodge。
        assert_eq!(map_check_param_need(&Attack, "roll_under"), Some(("skills".into(),"dodge".into())));
        // 潜行/盗窃/对抗社交/冲突中调查 → perception(两种 compare 一致)。
        for ak in [Hide, Hack, Intimidate, InvestigateDuringConflict] {
            assert_eq!(map_check_param_need(&ak, "roll_under"), Some(("skills".into(),"perception".into())));
        }
        // 不需 NPC 参数 → None。
        for ak in [AskQuestion, Move, LeaveScene, Unknown] {
            assert_eq!(map_check_param_need(&ak, "roll_under"), None);
            assert_eq!(map_check_param_need(&ak, "meet_or_beat"), None);
        }
    }
}

#[cfg(test)]
mod named_check_plan_tests {
    use super::build_named_check_plan;
    use trpg_model::*;

    fn request() -> ContextRequest {
        ContextRequest {
            ruleset_id: "call_of_cthulhu_7e".into(),
            module_id: None,
            session_id: "sess".into(),
            turn_id: "turn_named".into(),
            viewer: VisibilityProfile::player("player_test", "pc.current"),
            token_budget: TokenBudget::default(),
        }
    }

    // An explicit Sanity roll binds tested_parameter to the named parameter so the
    // contest resolver reads the actor's REAL SAN (not a guessed stat) and the
    // sanity track's on_outcome fires deliberately.
    #[test]
    fn binds_tested_parameter_to_named_label() {
        let plan = build_named_check_plan(&request(), "make a Sanity roll", "Sanity", Some("1d100"));
        let check = plan.check.expect("named check must produce a contract");
        let tp = check.tested_parameter.expect("tested_parameter must be bound");
        assert_eq!(tp.key, "Sanity");
        assert!(check.check_label.to_ascii_lowercase().contains("sanity"), "label: {}", check.check_label);
        assert_eq!(check.dice_expression, "1d100");
    }

    // No kernel core die → narration-only (fail-closed, never a fabricated die),
    // mirroring the forced-tech builder.
    #[test]
    fn no_kernel_dice_is_narration_only() {
        let plan = build_named_check_plan(&request(), "make a Sanity roll", "Sanity", None);
        assert!(plan.check.is_none());
    }
}

#[cfg(test)]
mod materialization_fallback_tests {
    use super::pick_named_parameter;
    use trpg_model::MaterialTargetKind::*;

    // Robustness fallback: when the orchestrator's classifier missed the named
    // parameter, the materialization demands usually still carry it. Prefer the
    // ActorProfile demand (the tested value) over the CheckTarget demand.
    #[test]
    fn prefers_actor_profile_over_check_target() {
        let items = [(CheckTarget, "Sanity roll"), (ActorProfile, "Sanity")];
        assert_eq!(pick_named_parameter(&items).as_deref(), Some("Sanity"));
    }

    // Cross-language: the materialization label is used verbatim (no keyword
    // list); the resolver matches it to the kernel track by meaning.
    #[test]
    fn falls_back_to_check_target_verbatim() {
        let items = [(CheckTarget, "理智检定")];
        assert_eq!(pick_named_parameter(&items).as_deref(), Some("理智检定"));
    }

    // No parameter/check demand → no fallback (stays a generic assessment).
    #[test]
    fn unrelated_demands_yield_none() {
        let items = [(ObjectDefinition, "Lever"), (AbilityDefinition, "Fireball")];
        assert_eq!(pick_named_parameter(&items), None);
    }
}

#[cfg(test)]
mod b1_projection_tests {
    use serde_json::json;

    #[test]
    fn synthesized_param_projects_into_mechanical_profile() {
        use crate::npc_synth::{SynthesizedParam, write_synthesized_param};
        let mut sheet = json!({"stats":{}, "skills":{}});
        let p = SynthesizedParam {
            value: json!(45),
            status: "provisional".into(),
            provenance: json!({"tier":"persona_judge","parameter":"perception"}),
        };
        write_synthesized_param(&mut sheet, "skills", "perception", &p);
        let mut mech = json!({"ruleset":"call_of_cthulhu_7e"});
        crate::chargen::refresh_mechanical_profile(&mut mech, &sheet);
        // contest 读 mechanical_profile.skills.perception → 必须是 45(B1 修复核心断言)。
        assert_eq!(mech.pointer("/skills/perception"), Some(&json!(45)));
        // provenance 仍可访问(被归进 fields 但数据保留)。
        assert_eq!(mech.pointer("/fields/npc_param_provenance/perception/tier"), Some(&json!("persona_judge")));
    }
}

#[cfg(test)]
mod stamp_tests {
    use super::*;
    use trpg_model::*;

    #[test]
    fn stamp_opposed_check_sets_target_and_opponent_param() {
        let mut check: CheckContract = serde_json::from_value(serde_json::json!({
            "check_id":"c","session_id":"s","turn_id":"t",
            "ruleset_id":"call_of_cthulhu_7e","module_id":null,
            "initiator":{"actor_id":"pc.current","actor_kind":"player_character","display_name":null},
            "target_actor":null,
            "opposition":{"kind":"no_mechanical_opposition"},
            "action_summary":"潜行","intent_kind":"hide","check_label":"潜行 check",
            "dice_expression":"1d100","modifiers":[],
            "target":{"kind":"unknown_until_lookup"},
            "tested_parameter":{"domain":null,"key":"Stealth","label":"Stealth"},
            "opponent_tested_parameter":null,
            "actor_snapshot_ids":[],"source_refs":[],"learned_packet_ids":[],
            "roll_visibility":"public_gm_roll","roll_authority":"system",
            "disclosure":{
                "show_roll_to_player":true,"show_formula_to_player":true,
                "show_dc_to_player":true,"show_success_failure_to_player":true,
                "reveal_after_scene":false,"reveal_after_session":false
            },
            "stakes":{
                "before_roll_public":"","success_public":"","failure_public":"",
                "critical_public":null,"fumble_public":null,
                "success_patches_allowed":[],"failure_patches_allowed":[],"irreversible":false
            },
            "confidence":"medium","ruling_status":"source_backed",
            "advice_refs":[],"expires_at_turn":null
        })).unwrap();
        let npc = npc_synth::NpcPersona {
            actor_id: "npc.opposition".into(),
            name: "拉斯".into(),
            prose: "".into(),
        };
        stamp_opposed_check(&mut check, &npc, "skills", "perception");
        assert_eq!(
            check.target_actor.as_ref().map(|a| a.actor_id.as_str()),
            Some("npc.opposition")
        );
        assert_eq!(
            check.opponent_tested_parameter.as_ref().map(|t| t.key.as_str()),
            Some("perception")
        );
    }
}

#[cfg(test)]
mod opposed_tests {
    use super::*;
    use trpg_model::*;

    #[test]
    fn contract_is_opposed_only_with_target_and_opponent_param() {
        let mut c: CheckContract = serde_json::from_value(serde_json::json!({
            "check_id":"c","session_id":"s","turn_id":"t","ruleset_id":"r","module_id":null,
            "initiator":{"actor_id":"pc.current","actor_kind":"player_character","display_name":null},
            "target_actor":null,"opposition":{"kind":"no_mechanical_opposition"},
            "action_summary":"","intent_kind":"","check_label":"","dice_expression":"1d100","modifiers":[],
            "target":{"kind":"unknown_until_lookup"},"tested_parameter":null,"opponent_tested_parameter":null,
            "actor_snapshot_ids":[],"source_refs":[],"learned_packet_ids":[],
            "roll_visibility":"public_gm_roll","roll_authority":"system",
            "disclosure":{"show_roll_to_player":true,"show_formula_to_player":true,"show_dc_to_player":true,"show_success_failure_to_player":true,"reveal_after_scene":false,"reveal_after_session":false},
            "stakes":{"before_roll_public":"","success_public":"","failure_public":"","critical_public":null,
                "fumble_public":null,"success_patches_allowed":[],"failure_patches_allowed":[],"irreversible":false},
            "confidence":"medium","ruling_status":"provisional","advice_refs":[],"expires_at_turn":null
        })).unwrap();
        assert!(!contract_is_opposed(&c));
        c.target_actor = Some(ActorRef { actor_id: "npc.opposition".into(), actor_kind: ActorKind::Npc, display_name: None });
        assert!(!contract_is_opposed(&c), "只有 target_actor 还不够");
        c.opponent_tested_parameter = Some(TestedParameter { domain: None, key: "perception".into(), label: "perception".into() });
        assert!(contract_is_opposed(&c));
    }
}

#[cfg(test)]
mod agent_loop_protocol_tests {
    use super::*;

    // （spec §6 实锤：BP1 的 engine protocol 必须出 agent-loop 版文案；
    // 旧块禁令与 roll_check/request_player_roll 双工具直接矛盾）
    #[test]
    fn agent_loop_protocol_block_swaps_dice_prohibition_for_tool_semantics() {
        let legacy = engine_protocol_block();
        let agent = engine_protocol_block_agent_loop();
        let legacy_text = legacy.content.render_text();
        let agent_text = agent.content.render_text();
        // 旧禁令段不得进 agent system prompt（run_gm_turn 强制 agent_loop_protocol=true）。
        assert!(legacy_text.contains("without asking players for dice totals"));
        assert!(!agent_text.contains("without asking players for dice totals"));
        // agent 变体显式声明双工具掷骰语义。
        assert!(agent_text.contains("roll_check"));
        assert!(agent_text.contains("request_player_roll"));
        // 缓存稳定：两变体 block_id 不同（prefix 段按 ruleset/路径固定，互不污染）。
        assert_ne!(legacy.block_id, agent.block_id);
    }
}

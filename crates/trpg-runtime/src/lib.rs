use anyhow::{anyhow, Result};
use rand::{Rng, SeedableRng};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, HashSet};
use trpg_ability::{AbilityService, AbilityTurnInput, AbilityTurnResult};
use trpg_agent::{looks_like_new_action_or_abandon, parse_roll_text, ParsedRollText};
use trpg_combat::{CombatAgent, ConflictTurnInput, ConflictTurnResult};
use trpg_contest::ContestService;
use trpg_db::Db;
use trpg_director::{ActionableSituationDirector, DirectorInput};
use trpg_interaction::InteractionLifecycleKernel;
use trpg_llm::{system, user};
use trpg_material::{MaterializationService, MaterializationTurnInput, MaterializationTurnResult};
use trpg_mechanics::{
    insert_roll_plan, make_roll_plan_from_check, FollowupCheck, RefereeCombatService,
};
use trpg_model::*;
use trpg_object::{ObjectService, ObjectTurnInput, ObjectTurnResult};
use trpg_orchestrator::{TurnOrchestrationResult, TurnOrchestrator, TurnOrchestratorInput};
use trpg_params::RuntimeParameterService;
use trpg_referee::PlayerValueRefereeService;
use trpg_search::SearchService;
use trpg_semantics::SemanticRuleBindingService;
use trpg_time::WorldTimeService;
use uuid::Uuid;

mod chargen;
pub use chargen::{
    apply_chargen_formulas, generate_starter_character, materialize_actor_params, CreatedCharacter,
};

pub mod entry_gate;
pub use entry_gate::{actor_params_playable, evaluate_entry_gate, EntryGate, EntryGateBlock};

pub mod binding;
pub use binding::{
    facets_from_kernel, facets_from_need_traces, resolve_binding, shadow_bind,
    shadow_bind_with_kernel, CapabilityRegistry,
};

pub mod binding_exec;
pub use binding_exec::{
    binding_takeover_enabled, check_binding_plan, execute_check_with_binding,
    plan_authorizes_check_exec,
};

pub mod knowledge_projection;
pub use knowledge_projection::{
    npc_action_projection_from_mind, npc_speech_projection_from_mind, project_for_gm_adjudication,
    project_for_npc_action, project_for_npc_speech, project_for_player_narration,
    GmAdjudicationProjection, NpcActionProjection, NpcOwnedProjection, NpcSpeechProjection,
    PlayerNarrationProjection,
};

mod need_resolvers;

mod scene_need_resolver;
use scene_need_resolver::SceneNeedResolver;

pub mod material_need_resolver;

pub mod parameter_need_resolver;

pub mod entity_need_resolver;
pub use entity_need_resolver::{encode_entity_hint, EntityNeedResolver};

pub mod npc_profile;
pub mod npc_synth;
pub use npc_profile::persona_from_profile;
pub mod npc_relationship;
pub use npc_relationship::{apply_npc_relationship_delta, next_relationship};
pub mod npc_mind;
pub use npc_mind::{assemble_npc_mind_view, load_npc_mind_view};
pub mod npc_behavior;
pub use npc_behavior::{
    derive_npc_behavior_plan, load_active_npc_guidance, load_npc_behavior_plan,
    viewer_behavior_context,
};
pub mod director_brief;
pub mod knowledge_leak_verifier;
pub mod scene_plan_emit;
pub mod story_events;
pub mod story_observer;
pub mod story_write;
pub mod memory_guard;
pub mod world;
/// Adventure IR runtime ProgressionEngine + AdvancementFrontier (P1-3, flag
/// `TRPG_PROGRESSION_ENGINE`, default OFF). Additive; consumer wiring deferred.
pub mod progression;
pub use director_brief::{
    apply_story_proposals, build_director_block, build_director_plan_post_adjudication,
    commit_story_writes, prepare_director_brief, prepare_director_plan_post_adjudication,
};
pub use knowledge_leak_verifier::{
    behavior_finding_to_verifier_finding, to_verifier_finding, to_verifier_findings,
    verify_npc_asserted_facts, verify_npc_behavior_consistency, verify_npc_disclosure,
    verify_player_narration_leak,
};
pub use story_write::{
    apply_thread_opened, merge_rejections, rejection_proposal, story_write_loop_enabled,
};
pub use scene_plan_emit::{
    build_scene_plan_emission, emit_scene_plan_on_change, scene_forbidden_reveals,
};

pub mod verifier_private_view;
pub use verifier_private_view::{build_verifier_private_view, VerifierPrivateView};

mod scene_projection;
pub use scene_projection::{contract_is_opposed, module_entry_scene_id, stamp_opposed_check};
use scene_projection::{map_check_param_need, resolve_turn_scene_id};

mod truthgraph;
pub use truthgraph::{
    context_surfaced_events_for_scene, player_exposed_events_for_scene_narration,
};

pub mod check_outcome_facts;
pub mod relationship_extraction;
pub use relationship_extraction::{
    build_relationship_messages, parse_relationship_triples, relationship_facts_from_inputs,
    relationship_gate_should_run, resolve_entity_refs, text_has_social_signal, EntityRef,
};

pub mod memory_proposal;
pub use memory_proposal::{
    proposals_from_json, relationship_facts_to_proposals, review_and_commit_proposals,
    try_proposals_from_json, CommitContext, CommitOutcome, CommitReport, CommitStatus,
};

mod spoiler_guard;

mod npc_activation;
use npc_activation::apply_npc_activation;

pub mod clue_affordance;
pub use clue_affordance::{clue_reveal_candidates, ClueRevealCandidate, ResolvedCheck};

pub mod clue_projection;
pub use clue_projection::{
    clue_projection_enabled, project_clues_onto_scenes, ClueProjectionReport, CLUE_PROJECTION_ENV,
};

pub mod clue_surface;
pub use clue_surface::{
    module_clue_surface_text, render_clue_surface, surface_player_facing_clues, SurfacedClue,
};

pub mod met_engaged;
pub use met_engaged::{
    derive_met_engaged_gate, director_leverage_npc_ids, player_engaged_met_events,
    restrict_unmet_npc_guidance, MetEngagedGate,
};

mod npc_profile_materialize;
mod npc_testimony;

pub mod scene_establishing;
pub use scene_establishing::collect_scene_establishing;
pub mod character_context;
pub use character_context::collect_character_context;

mod context_blocks;

mod spotlight_roster;
use context_blocks::{
    actionable_situation_block, clue_board_block, continuity_anchor_block, continuity_anchor_tail,
    dynamic_text_block, engine_protocol_block, engine_protocol_block_agent_loop,
    gm_continuity_anchor_enabled, gm_opening_convergence_enabled, memory_snapshot_block,
    opening_convergence_block, retrieved_memory_block,
    world_events_since_block, world_state_block, world_time_block,
};

pub mod event_fold;
pub mod scene_navigation;
pub use scene_navigation::{
    build_nav_prompt, extract_module_scenes, prefetch_frontier, scene_navigator,
    validate_transition,
};

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

/// R5 turn 高水位守卫：回合入口处等上一回合的 critical 组落账（pp_lifecycle >= critical_done）。
/// fail-closed——从不硬拒玩家：无上一回合 / 已达 critical / heavy 未完（critical_done 非 complete）
/// 均立即放行；仅当上一回合仍 < critical_done 时 bounded 轮询，超时 warn 放行。
pub async fn await_prev_turn_critical(
    db: &Db,
    session_id: &str,
    timeout_ms: u64,
    interval_ms: u64,
) {
    let reached = |phase: &Option<String>| -> bool {
        match phase {
            None => true, // 无上一回合 → 放行
            Some(p) => pp_lifecycle_rank(p) >= pp_lifecycle_rank(PP_CRITICAL_DONE),
        }
    };
    // 首查：常见路径（critical 已落账 / 无上一回合）零等待。
    match db.load_last_turn_pp_lifecycle(session_id).await {
        Ok(ref phase) if reached(phase) => return,
        Err(err) => {
            tracing::warn!(error = %err, session_id, "high-water guard load failed; proceeding (fail-closed)");
            return;
        }
        _ => {}
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    let step = std::time::Duration::from_millis(interval_ms);
    loop {
        if tokio::time::Instant::now() >= deadline {
            tracing::warn!(session_id, timeout_ms, "high-water guard timed out waiting for prev-turn critical; proceeding (fail-closed, possibly slightly stale)");
            return;
        }
        tokio::time::sleep(step).await;
        match db.load_last_turn_pp_lifecycle(session_id).await {
            Ok(ref phase) if reached(phase) => return,
            Err(err) => {
                tracing::warn!(error = %err, session_id, "high-water guard re-poll failed; proceeding (fail-closed)");
                return;
            }
            _ => {}
        }
    }
}

/// P1-4：合并一次 Need 取数结果——先把来源 trace（need_kind / source_refs /
/// block_count / reason）记进 `trace`，再把 blocks 注入 `blocks`。source_refs 此前
/// 在 `blocks.extend(outcome.blocks)` 处被默默丢弃；这里在 extend 消费 outcome 之前
/// 先抓取 block_count，故 outcome 按值传入、顺序正确。行为与旧 extend 等价，仅多记 trace。
fn merge_need_outcome(
    blocks: &mut Vec<ContextBlock>,
    trace: &mut Vec<NeedResolutionTrace>,
    outcome: trpg_need::NeedOutcome,
    need_kind: &str,
    reason: &str,
) {
    trace.push(NeedResolutionTrace {
        need_kind: need_kind.into(),
        source_refs: outcome.source_refs,
        reason: reason.into(),
        block_count: outcome.blocks.len(),
    });
    blocks.extend(outcome.blocks);
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
            domains: vec![
                "rules".into(),
                "source".into(),
                "parsed".into(),
                "modules".into(),
                "rulings".into(),
                "learned".into(),
            ],
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
                    let pg = h
                        .source_refs
                        .first()
                        .and_then(|r| r.page)
                        .map(|p| format!("p{p} "))
                        .unwrap_or_default();
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
                    if pack.sheet_template.fields.is_empty() {
                        missing.push("CharacterSheetTemplate.fields");
                    }
                    if pack.creation_flows.iter().all(|flow| flow.steps.is_empty()) {
                        missing.push("CharacterCreationFlow.steps");
                    }
                    if pack.runtime_bindings.is_empty() {
                        missing.push("CharacterRuntimeBinding");
                    }
                    if pack.starter_character_pack.pregens.is_empty()
                        && pack.starter_character_pack.archetypes.is_empty()
                        && pack.starter_character_pack.creation_shortcuts.is_empty()
                    {
                        missing.push("StarterCharacterPack");
                    }
                }
                None => missing.push("CharacterOnboardingPack"),
            }
            if env_bool_runtime("TRPG_PLAYABILITY_GATE_BLOCKING", false) {
                if let Some(module_id) = module_id {
                    if !self
                        .db
                        .has_module_first_session_packet(module_id)
                        .await
                        .unwrap_or(false)
                    {
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
        self.ensure_session_initialized(&session_id, ruleset_id, module_id)
            .await?;
        Ok(session_id)
    }

    pub(crate) async fn ensure_session_initialized(
        &self,
        session_id: &str,
        ruleset_id: &str,
        module_id: Option<&str>,
    ) -> Result<()> {
        self.db
            .create_session(session_id, ruleset_id, module_id)
            .await?;
        let _ = WorldTimeService::new(self.db.clone())
            .ensure_session_time(session_id, Some(session_id))
            .await;
        let _ = self.db.ensure_interaction_generation(session_id).await;
        if let Some(mid) = module_id {
            // fail-closed：任何失败仅 warn，不阻断开局；但必须可见——曾有空图谱静默
            // 跳过激活，导致整局 current_scene_id=NULL 而无任何线索。
            match self.db.load_module_graph(mid).await {
                Ok(Some(graph)) => match module_entry_scene_id(&graph) {
                    Some(entry) => {
                        let _ = self.db.set_session_scene(&session_id, &entry).await;
                        // L-R durable seed — RELOCATED here (A2b-U / Q4 FLAG-TRAP fix). It used to
                        // live only inside `opening_scene_delivery` (CLI-only) so the API/engine
                        // product path never seeded durable memory. Running it at session-init
                        // (every transport — start_session / CLI / API / create-character) makes
                        // memory_facts>=1 hold from turn 0 on EVERY path. source-backed (only the
                        // module's own established opening encounter), idempotent (stable mf_open_*
                        // id), flag `TRPG_OPENING_DURABLE_SEED` default ON / OFF == no write
                        // (byte-equal). fail-soft: any upsert miss only warns.
                        if relationship_extraction::opening_durable_seed_enabled() {
                            let npc_ids: Vec<String> = graph
                                .scenes
                                .iter()
                                .find(|s| s.node_id == entry)
                                .map(|n| n.referenced_npc_ids.clone())
                                .unwrap_or_default();
                            let facts = relationship_extraction::opening_seed_facts(
                                session_id,
                                Some(entry.as_str()),
                                &npc_ids,
                            );
                            for f in &facts {
                                if let Err(err) = self.db.upsert_memory_fact(f).await {
                                    tracing::warn!(error = %err, "opening durable seed upsert failed (fail-soft)");
                                }
                            }
                        }
                    }
                    None => tracing::warn!(module_id = mid, "module graph has no scenes; entry scene not activated — re-run parse-all with the module reader enabled"),
                },
                Ok(None) => tracing::warn!(module_id = mid, "no module graph found; entry scene not activated — run parse-all for this module first"),
                Err(err) => tracing::warn!(error = %err, module_id = mid, "load_module_graph failed; entry scene not activated"),
            }
        }
        Ok(())
    }

    pub async fn current_world_time(&self, session_id: &str) -> Result<WorldTimeState> {
        WorldTimeService::new(self.db.clone())
            .current(session_id)
            .await
    }

    pub async fn advance_world_time(
        &self,
        request: TimeAdvanceRequest,
    ) -> Result<TimeAdvanceResult> {
        WorldTimeService::new(self.db.clone())
            .advance(request)
            .await
    }

    pub async fn record_world_event(
        &self,
        session_id: &str,
        turn_id: Option<&str>,
        frame_id: Option<&str>,
        kind: WorldEventKind,
        event_json: serde_json::Value,
        visibility: Visibility,
    ) -> Result<WorldEvent> {
        WorldTimeService::new(self.db.clone())
            .record_event(session_id, turn_id, frame_id, kind, event_json, visibility)
            .await
    }

    /// MAT.M2: lazily upsert a thin source-backed `npc_profile` for each active NPC
    /// that lacks a durable profile, so `build_npc_behavior_guidance` treats it as a
    /// live responder. Source-present fields only (no fabrication). fail-soft: any
    /// load/build/upsert miss is skipped (warn), never aborts the turn. Caller gates
    /// on `Enforce` (this method assumes it is only called when active).
    async fn materialize_active_npc_profiles(
        &self,
        session_id: &str,
        active_npc_ids: &[String],
        modules: &[trpg_model::ModuleBundle],
        module_id: Option<&str>,
    ) {
        let Some(module_id) = module_id else { return };
        let Some(module) = modules.iter().find(|m| m.module_id == module_id) else {
            return;
        };
        for npc_id in active_npc_ids {
            // M2/M8 idempotency: a fresh NPC gets a new profile; a PRE-M8 thin profile
            // (no persona_description) is UPGRADED in place to carry the source body prose
            // so existing sessions can finally speak. A profile that already carries persona
            // prose is left untouched (no double-write). fail-soft on any DB miss.
            let existing = match self.db.load_npc_profile(session_id, npc_id).await {
                Ok(opt) => opt,
                Err(err) => {
                    tracing::warn!(error = %err, npc_id = %npc_id, "M2: profile load failed; skipping");
                    continue;
                }
            };
            if existing
                .as_ref()
                .is_some_and(|p| p.persona_description.is_some())
            {
                continue; // already thickened — idempotent no-op
            }
            let Some(entry) =
                npc_profile_materialize::find_module_npc(&module.module_graph.npcs, npc_id)
            else {
                continue;
            };
            let Some(fresh) = npc_profile_materialize::thin_profile_from_module_npc(npc_id, entry)
            else {
                continue;
            };
            // Existing thin profile with no source body to add → nothing changed; skip the
            // redundant write (keep DB-write count minimal, byte-stable when no upgrade).
            if existing.is_some() && fresh.persona_description.is_none() {
                continue;
            }
            if let Err(err) = self.db.upsert_npc_profile(session_id, &fresh).await {
                tracing::warn!(error = %err, npc_id = %npc_id, "M2: thin profile upsert failed (fail-soft)");
            }
        }
    }

    /// L-D best-effort:取开场/当前场景标题作为开场收敛锚的 grounding hint。任何缺失(无模组/
    /// 图谱加载失败/找不到节点)⇒ None(锚仍泛指"本开场场景")。纯读 module graph 快照,不新增
    /// DB 写;零 ruleset 名分支。
    async fn opening_scene_title(
        &self,
        module_id: Option<&str>,
        scene_id: Option<&str>,
    ) -> Option<String> {
        let mid = module_id?;
        let graph = self.db.load_module_graph(mid).await.ok().flatten()?;
        let sid = scene_id
            .map(str::to_string)
            .or_else(|| module_entry_scene_id(&graph))?;
        graph
            .scenes
            .iter()
            .find(|s| s.node_id == sid)
            .map(|s| s.title.clone())
    }

    /// L-P Q3 OPENING-SCENE DELIVERY (flag `TRPG_OPENING_SCENE_DELIVERY`, default ON).
    ///
    /// Before the player's FIRST action, surface the module ENTRY scene's player-safe
    /// establishing narration (spoiler-redacted `read_aloud`; anthology/spine modules →
    /// prep-packet fallback). This makes the player — real `trpg play` shell OR the eval
    /// harness sim — enter from a GM-set opening instead of a vacuum. Q3 root cause: a
    /// vacuum opening lets the player assert an off-module landing point (a self-invented
    /// "home/apartment") that the engine can never reclaim ⇒ location split/confusion =
    /// HARD amnesia. Surfacing the authored entry establishing first removes that root.
    ///
    /// This is NOT a committed turn: it mutates no state and consumes no player action —
    /// it is pure scene-setting read BEFORE turn 1. Reuses the SAME source-backed material
    /// (`collect_scene_establishing`) the split Narrator already weaves on a normal turn.
    ///
    /// Returns `Ok(None)` — i.e. no pre-turn opening, byte-equal baseline — when: the flag
    /// is OFF (`0/false/off/no`), no module is bound, the session already has ≥1 committed
    /// turn (a resumed/in-progress session never re-delivers), or there is no establishing
    /// material. fail-soft: any DB/load error degrades to `Ok(None)` (vacuum start, never a
    /// hard error at the session door).
    pub async fn opening_scene_delivery(
        &self,
        session_id: &str,
        ruleset_id: &str,
        module_id: Option<&str>,
    ) -> Result<Option<String>> {
        if !opening_scene_delivery_enabled() {
            return Ok(None);
        }
        let Some(mid) = module_id else {
            return Ok(None);
        };
        // Only at the true start (0 committed turns); resume never re-surfaces the opening.
        if self
            .db
            .count_session_turns(session_id)
            .await
            .unwrap_or(0)
            > 0
        {
            return Ok(None);
        }
        // Entry scene was activated at session init (ensure_session_initialized); self-heal
        // path in prepare_turn_context covers stale sessions, but for a fresh session this is
        // already set. Fall back to module graph entry if the session row has no scene yet.
        let mut scene_id = self.db.load_session_scene(session_id).await.ok().flatten();
        if scene_id.as_deref().map(str::trim).unwrap_or("").is_empty() {
            if let Ok(Some(graph)) = self.db.load_module_graph(mid).await {
                scene_id = module_entry_scene_id(&graph);
            }
        }
        let project = match self.db.load_project_bundle_for_ruleset(ruleset_id).await {
            Ok(Some(p)) => Some(p),
            _ => self.db.load_latest_project_bundle().await.ok().flatten(),
        };
        let Some(project) = project else {
            return Ok(None);
        };
        // Same source-backed collector the per-turn path uses (NON-secret read_aloud,
        // spoiler-redacted); anthology/spine modules with no scene node fall back to the
        // prep-packet's player-facing establishing fields.
        let mut parts = scene_establishing::collect_scene_establishing(
            &project.modules,
            Some(mid),
            scene_id.as_deref(),
        );
        if parts.is_empty() {
            if let Ok(Some(csp)) = self.db.load_module_prep_packet_session(mid).await {
                parts = scene_establishing::prep_packet_establishing(&csp);
            }
        }
        let text = parts.join("\n\n");
        let text = text.trim();
        if text.is_empty() {
            return Ok(None);
        }
        // A2b-U (Q4): the L-R durable seed is now written at session-init
        // (`ensure_session_initialized`) on EVERY transport, not here — so the API/engine
        // product path also seeds durable memory. This method now only DELIVERS the opening
        // narration (CLI pre-turn) and records that ACTUAL delivery, so the L-G first-entry
        // gate can suppress turn-1 read_aloud re-delivery from a real fact (not an inferred
        // `flag_on && module_bound` that wrongly suppressed the un-delivered API path).
        // fail-soft: a marker write miss only warns (gate then surfaces the opening — safe).
        if let Err(err) = self.db.mark_opening_delivered(session_id).await {
            tracing::warn!(error = %err, "mark_opening_delivered failed (fail-soft)");
        }
        Ok(Some(text.to_string()))
    }

    pub async fn prepare_turn_context(
        &self,
        request: &ContextRequest,
        state: &RuntimeState,
        current_input: Option<&str>,
        recent_transcript: Option<&str>,
    ) -> Result<CompiledContext> {
        // R5 turn 高水位守卫：等上一回合 critical 组落账，消除连发回合读到陈旧
        // current_scene_id/记忆的竞态。fail-closed——超时/无上一回合/已达均放行，不卡玩家。
        await_prev_turn_critical(&self.db, &request.session_id, 2000, 100).await;
        // 每回合单点把持久化的 current_scene_id 载入运行态（P4 投影）：state 缺场景且有模组时填入。
        let mut state_owned = state.clone();
        if resolve_turn_scene_id(
            state_owned.scene_id.as_deref(),
            state_owned.module_id.as_deref(),
            None,
        )
        .is_none()
            && state_owned.module_id.is_some()
        {
            let mut loaded = self
                .db
                .load_session_scene(&request.session_id)
                .await
                .ok()
                .flatten();
            // 自愈：会话从未激活入口场景（旧会话 / 入口激活在 session-init 失败）时，
            // 每回合单点从 module graph 重派 entry scene 并持久化，避免整局 NULL 场景导致
            // director/narrator 无锚点而空叙事。fail-soft：任何失败仅 warn，不阻断回合。
            if loaded.as_deref().map(str::trim).unwrap_or("").is_empty() {
                if let Some(mid) = state_owned.module_id.as_deref() {
                    if let Ok(Some(graph)) = self.db.load_module_graph(mid).await {
                        if let Some(entry) = module_entry_scene_id(&graph) {
                            let _ = self.db.set_session_scene(&request.session_id, &entry).await;
                            loaded = Some(entry);
                        }
                    }
                }
            }
            state_owned.scene_id = resolve_turn_scene_id(
                state_owned.scene_id.as_deref(),
                state_owned.module_id.as_deref(),
                loaded,
            );
        }
        let _ = InteractionLifecycleKernel::new(self.db.clone())
            .reconcile_session(&request.session_id)
            .await;
        // Load the project bundle FOR THIS RULESET (several rulesets may share a
        // DB; the latest-parsed one is not necessarily the one being played).
        let project =
            match self
                .db
                .load_project_bundle_for_ruleset(&request.ruleset_id)
                .await?
            {
                Some(p) => p,
                None => self.db.load_latest_project_bundle().await?.ok_or_else(|| {
                    anyhow!("no parsed project bundle found; run parse-all first")
                })?,
            };
        // MAT.M1 axis-1: per-turn NPC activation derivation from scene.referenced_npc_ids
        // (DP-2 hybrid). Gated by MaterializationAffordanceMode: Off/Shadow = strict no-op
        // (caller-supplied active_npc_ids preserved unchanged, s17 correction); Enforce =
        // derive-when-empty from the current scene (stable-dedup, fail-closed). Must run
        // BEFORE `let state = &state_owned;` so world_state_block sees the derived set.
        let mat_module_id = request
            .module_id
            .as_deref()
            .or(state_owned.module_id.as_deref())
            .map(str::to_string);
        // Q-MODULE DP-C: module-bound-aware default (NOT a global env flip). When the env is
        // unset, a module-bound session defaults to Enforce (normal module play surfaces module
        // content); a no-module session stays Off (byte-equal baseline). An explicit env value
        // (incl. `off`) still wins in both directions.
        let mat_mode = MaterializationAffordanceMode::from_env_for_session(mat_module_id.is_some());
        apply_npc_activation(
            &mut state_owned.active_npc_ids,
            &project.modules,
            mat_module_id.as_deref(),
            state_owned.scene_id.as_deref(),
            mat_mode,
        );
        // MAT.M2 axis-1: ensure each newly-active NPC has a durable, thin, source-backed
        // npc_profile so build_npc_behavior_guidance (turn_loop) does not skip it. Enforce
        // only; OFF/Shadow == baseline no-op (no DB writes).
        if mat_mode.is_enforce() {
            self.materialize_active_npc_profiles(
                &request.session_id,
                &state_owned.active_npc_ids,
                &project.modules,
                mat_module_id.as_deref(),
            )
            .await;
        }
        let state = &state_owned;
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
            return Err(anyhow!(
                "no bundle found for ruleset={} module={:?}",
                request.ruleset_id,
                request.module_id
            ));
        }

        let mut blocks = self.db.list_context_blocks_for_bundles(&bundle_ids).await?;
        // P1-4：本回合 Need 取数的来源 trace（rule / scene / parameter / material），
        // 随返回的 CompiledContext 一同传出（见 merge_need_outcome）。
        let mut need_trace: Vec<NeedResolutionTrace> = Vec::new();
        let material_refs: Vec<String> = state
            .pending_material_refs
            .iter()
            .chain(state.active_material_refs.iter())
            .cloned()
            .collect();
        let mut exact = self
            .db
            .find_material_blocks(&bundle_ids, &material_refs)
            .await?;
        for b in &mut exact {
            if b.cache_zone == CacheZone::NeverPrompt {
                b.cache_zone = CacheZone::DynamicTail;
            }
            b.load_reason = Some("pending_or_active_material_ref".to_string());
        }
        blocks.extend(exact);

        let _ = self
            .db
            .deactivate_runtime_turn_blocks(&request.session_id, &request.turn_id)
            .await;
        match self
            .db
            .list_runtime_context_blocks(
                &request.session_id,
                &request.turn_id,
                state.scene_id.as_deref(),
            )
            .await
        {
            Ok(mut runtime_loaded_blocks) => blocks.append(&mut runtime_loaded_blocks),
            Err(err) => {
                tracing::warn!(error = %err, "runtime-loaded search blocks failed; continuing")
            }
        }

        if let Some(input) = current_input {
            // R2: query-driven RULE retrieval is acquired ONLY through the Need bus
            // (RuleNeedResolver → assist, which subsumes auto_search + learned-packet
            // matching and adds source_refs grounding).
            match self
                .rule_need_blocks_for_turn(request, state, input, &mut need_trace)
                .await
            {
                Ok(mut rule_blocks) => blocks.append(&mut rule_blocks),
                Err(err) => {
                    tracing::warn!(error = %err, "rule need bus failed; continuing without rule blocks")
                }
            }
        }

        match self
            .memory_blocks_for_turn(request, state, current_input)
            .await
        {
            Ok(mut memory_blocks) => blocks.append(&mut memory_blocks),
            Err(err) => {
                tracing::warn!(error = %err, "memory retrieval failed; continuing without memory blocks")
            }
        }
        match self.state_frame_blocks_for_turn(request).await {
            Ok(mut frame_blocks) => blocks.append(&mut frame_blocks),
            Err(err) => {
                tracing::warn!(error = %err, "working state frame retrieval failed; continuing without frame blocks")
            }
        }
        // R2: current-scene deep projection is acquired ONLY through the Need bus
        // (SceneNeedResolver).
        {
            let project_module_ids: Vec<String> = project
                .modules
                .iter()
                .map(|m| m.module_id.clone())
                .collect();
            // L-G 失忆锚:开场定场文(read_aloud)只在玩家**首次在当前会话有回合**时投放。flag ON
            // (默认)且本会话已有≥1回合 ⇒ 视为开场已交付 ⇒ resolver 跳过 read_aloud 正文复投,GM 续写
            // 既成局面(NPC/出口/GM注记仍每回合在)。flag OFF / DB 失败 / 零回合 ⇒ false ⇒ 投开场=字节
            // 等价基线。注:首入近似为"会话首回合";场景切换后新场景开场的再投留待 L-C(场景真推进)后细化。
            //
            // A2b-U (Q4 FLAG-TRAP fix): turn-1 (count==0) 抑制现在键于**真实投递事实**
            // (`opening_delivered`，仅 CLI 开场钩子投出非空念白后写),不再从
            // `opening_scene_delivery_enabled() && module_bound` 推断。旧推断在 API/引擎路径
            // (无任何预投递)误判"已交付"⇒静默吞掉 turn-1 开场=回归 below baseline。改后:CLI 预投
            // ⇒ marker=true ⇒ 抑制复投(无双开场);API 路径无预投 ⇒ marker=false ⇒ turn-1 浮现开场。
            // marker 仅在 count==0 时查(count>0 短路),OFF/无投递 ⇒ false ⇒ 退回 count>0 字节等价。
            let read_aloud_already_delivered =
                if scene_projection::scene_read_aloud_first_entry_only_enabled() {
                    let turns = self
                        .db
                        .count_session_turns(&request.session_id)
                        .await
                        .unwrap_or(0);
                    turns > 0
                        || self
                            .db
                            .opening_delivered(&request.session_id)
                            .await
                            .unwrap_or(false)
                } else {
                    false
                };
            let scene_need = trpg_need::Need::Scene(trpg_need::SceneNeed {
                scopes: trpg_need::NeedScopes {
                    ruleset_id: request.ruleset_id.clone(),
                    module_id: request
                        .module_id
                        .clone()
                        .or_else(|| state.module_id.clone()),
                    session_id: request.session_id.clone(),
                    turn_id: request.turn_id.clone(),
                    scene_id: state.scene_id.clone(),
                },
                project_module_ids,
                read_aloud_already_delivered,
            });
            let mut scene_bus = trpg_need::NeedBus::new();
            scene_bus.register(Box::new(SceneNeedResolver {
                db: self.db.clone(),
            }));
            scene_bus.emit(scene_need);
            let outcomes = scene_bus.resolve_all().await;
            for outcome in outcomes {
                merge_need_outcome(
                    &mut blocks,
                    &mut need_trace,
                    outcome,
                    "scene",
                    "turn context assembly",
                );
            }
        }
        match self.world_time_blocks_for_turn(request).await {
            Ok(mut time_blocks) => blocks.append(&mut time_blocks),
            Err(err) => {
                tracing::warn!(error = %err, "world time retrieval failed; continuing without world time blocks")
            }
        }
        match self.object_blocks_for_turn(request).await {
            Ok(mut object_blocks) => blocks.append(&mut object_blocks),
            Err(err) => {
                tracing::warn!(error = %err, "object graph projection failed; continuing without object blocks")
            }
        }
        // R2: actor-parameter blocks are acquired ONLY through the Need bus
        // (ParameterNeedResolver).
        {
            use crate::parameter_need_resolver::ParameterNeedResolver;
            use trpg_need::{Need, NeedResolver, NeedScopes, ParameterNeed};
            let scopes = NeedScopes {
                ruleset_id: request.ruleset_id.clone(),
                module_id: request.module_id.clone(),
                session_id: request.session_id.clone(),
                turn_id: request.turn_id.clone(),
                scene_id: state.scene_id.clone(),
            };
            let resolver = ParameterNeedResolver::new(self.db.clone());
            let need = Need::Parameter(ParameterNeed {
                scopes,
                actor_id: request.viewer.actor_id.clone(),
                current_input: current_input.map(str::to_string),
            });
            match resolver.resolve(&need).await {
                Ok(outcome) => merge_need_outcome(
                    &mut blocks,
                    &mut need_trace,
                    outcome,
                    "parameter",
                    "turn context assembly",
                ),
                Err(err) => tracing::warn!(
                    error = %err,
                    "ParameterNeedResolver failed; continuing without actor parameter blocks"
                ),
            }
        }
        match self.ability_blocks_for_turn(request).await {
            Ok(mut ability_blocks) => blocks.append(&mut ability_blocks),
            Err(err) => {
                tracing::warn!(error = %err, "ability graph projection failed; continuing without ability blocks")
            }
        }
        match self.rule_binding_blocks_for_turn(request).await {
            Ok(mut binding_blocks) => blocks.append(&mut binding_blocks),
            Err(err) => {
                tracing::warn!(error = %err, "rule binding projection failed; continuing without binding blocks")
            }
        }
        // R2: materialization blocks are acquired ONLY through the Need bus
        // (MaterialNeedResolver).
        {
            use crate::material_need_resolver::MaterialNeedResolver;
            use trpg_need::{MaterialNeed, Need, NeedResolver, NeedScopes};
            let scopes = NeedScopes {
                ruleset_id: request.ruleset_id.clone(),
                module_id: request.module_id.clone(),
                session_id: request.session_id.clone(),
                turn_id: request.turn_id.clone(),
                scene_id: state.scene_id.clone(),
            };
            let resolver = MaterialNeedResolver::new(self.db.clone());
            let need = Need::Material(MaterialNeed {
                scopes,
                user_input: None,
            });
            match resolver.resolve(&need).await {
                Ok(outcome) => merge_need_outcome(
                    &mut blocks,
                    &mut need_trace,
                    outcome,
                    "material",
                    "turn context assembly",
                ),
                Err(err) => tracing::warn!(
                    error = %err,
                    "MaterialNeedResolver failed; continuing without materialization blocks"
                ),
            }
        }
        match self.player_value_referee_blocks_for_turn(request).await {
            Ok(mut referee_blocks) => blocks.append(&mut referee_blocks),
            Err(err) => {
                tracing::warn!(error = %err, "player value referee projection failed; continuing without referee blocks")
            }
        }
        match self.referee_combat_blocks_for_turn(request).await {
            Ok(mut mech_blocks) => blocks.append(&mut mech_blocks),
            Err(err) => {
                tracing::warn!(error = %err, "referee combat ledger projection failed; continuing without mechanical ledger blocks")
            }
        }
        match self.contest_blocks_for_turn(request).await {
            Ok(mut contest_blocks) => blocks.append(&mut contest_blocks),
            Err(err) => {
                tracing::warn!(error = %err, "contest/opposition projection failed; continuing without contest blocks")
            }
        }
        // R2: learned-packet matching is performed inside the steward's `assist`
        // (RuleNeedResolver), so there is no standalone learned-packet projection.
        // BP1 active kernel projection is NOT a query-driven retrieval (it's always
        // present, non-query); R2 leaves it untouched.
        match self.rule_steward_prefix_blocks_for_turn(request).await {
            Ok(mut steward_blocks) => blocks.append(&mut steward_blocks),
            Err(err) => {
                tracing::warn!(error = %err, "rule steward BP1 projection failed; continuing without active kernel blocks")
            }
        }

        blocks.push(if state.agent_loop_protocol {
            engine_protocol_block_agent_loop()
        } else {
            engine_protocol_block()
        });
        blocks.push(world_state_block(state));
        if let Some(transcript) = recent_transcript {
            blocks.push(dynamic_text_block(
                "runtime.recent_transcript",
                BlockKind::RecentTranscript,
                "Recent Transcript",
                transcript,
                vec!["recent_transcript"],
            ));
        } else if gm_continuity_anchor_enabled() {
            // L-E 失忆锚:调用方未传 transcript（CLI/eval 路 = None）时,服务端从 `turns`
            // 表回载最近回合的 Player/GM 散文,注入 GmOnly 连续性锚——直击"GM 每回合重述
            // 开场定场文、把玩家挪回入口"的失忆病灶(J1 AMNESIA→0)。flag OFF ⇒ 此分支不跑
            // ⇒ 字节等价基线;DB 失败/无回合 ⇒ fail-soft 不注入(不阻断回合)。
            if let Ok(Some(history)) = self.db.load_recent_transcript(&request.session_id, 2).await {
                let tail = continuity_anchor_tail(&history, 1500);
                if !tail.is_empty() {
                    blocks.push(continuity_anchor_block(&tail));
                }
            }
        }
        // L-D 开场收敛锚:本局**开场回合**(尚无已落库回合 ⇒ 无连续性锚可投)注入,把玩家在真空里
        // 凭 objective 自创的"归乡/回家/前往某处"声明**收敛到模组开场场景所在地**,防 GM 另起一个
        // 模组之外的独立"家"地点、造成 turn1 念白把玩家一分为二(既在仓库又在家)= 硬位置失忆根
        // (Q2 DB+逐字 transcript 实证)。continuity anchor 的对称物(锚治 turn>1,本指令治 turn1)。
        // flag OFF / 非开场回合 / DB 失败 ⇒ 不注入 ⇒ 字节等价基线;零 ruleset 名分支(场景标题取自
        // module graph,无硬编码)。
        if gm_opening_convergence_enabled()
            && self
                .db
                .count_session_turns(&request.session_id)
                .await
                .unwrap_or(1)
                == 0
        {
            let scene_hint = self
                .opening_scene_title(
                    request.module_id.as_deref().or(state.module_id.as_deref()),
                    state.scene_id.as_deref(),
                )
                .await;
            blocks.push(opening_convergence_block(scene_hint.as_deref()));
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

        // MAT.M9a axis-2: authored clue SURFACE. Under Enforce, surface the module's
        // player-facing prep-packet clues (gm_only tiers withheld at source) as a GmOnly,
        // source-backed context block so the GM has authored material to reveal WHEN players
        // earn it via a successful investigative check (mirrors M8 folding NPC body into
        // persona). Off/Shadow == baseline (no block) → byte-equal. Pure read of the
        // already-loaded project.modules snapshot (no new DB path).
        if mat_mode.is_enforce() {
            if let Some(mid) = mat_module_id.as_deref() {
                // Read the prep-packet `current_session_packet` from its own table (the
                // project-bundle snapshot strips it to {mode}). fail-soft: any error → no block.
                if let Ok(Some(csp)) = self.db.load_module_prep_packet_session(mid).await {
                    let clues = clue_surface::surface_player_facing_clues(&csp);
                    if !clues.is_empty() {
                        blocks.push(dynamic_text_block(
                            "runtime.mat.clue_surface",
                            BlockKind::Clue,
                            "Authored Discoverable Clues",
                            &clue_surface::render_clue_surface(&clues),
                            vec!["materialization", "clue_surface", "gm_only"],
                        ));
                    }
                }
            }
        }

        // MAT.M9b / DP-A: authored NPC & scene knowledge SURFACE. The architect (Q3 DP-A)
        // widened the 4b admissible synthesis source from the empty structured
        // facts_can_reveal field to the module's GENUINELY SOURCE-PRESENT prose — the current
        // scene's read_aloud/gm_notes + each active NPC's body (spoiler-redacted). Under
        // Enforce, surface it as a GmOnly, source-backed steering block so present NPCs have
        // concrete authored material to TESTIFY from when the player engages + earns it via a
        // successful check (closing the M8 residual: narrator stopped at "willing to say more"
        // without emitting substantive testimony — the case facts live in scene/body prose).
        // Source-anchored (verbatim module prose, no invention); player-invisible (GmOnly);
        // disclosure still flows the existing reveal + presentation gate. Off/Shadow == baseline
        // (no block) → byte-equal. Pure read of the already-loaded project.modules snapshot.
        if mat_mode.is_enforce() {
            if let Some(text) = npc_testimony::module_testimony_surface_text(
                &project.modules,
                mat_module_id.as_deref(),
                state.scene_id.as_deref(),
                &state.active_npc_ids,
            ) {
                blocks.push(dynamic_text_block(
                    "runtime.mat.npc_testimony",
                    BlockKind::NpcStatic,
                    "Authored NPC & Scene Knowledge",
                    &text,
                    vec!["materialization", "npc_testimony", "gm_only"],
                ));
            }
        }

        dedupe_blocks(&mut blocks);
        let visible: Vec<ContextBlock> = blocks
            .into_iter()
            .filter_map(|b| project_visibility(b, &request.viewer))
            .collect();
        let planned = plan_blocks(visible, state, request);
        let mut compiled = ContextBuilder::default().build(planned, request)?;
        // P1-4：把本回合 Need 取数 trace 挂到返回的 CompiledContext（source_refs 不再丢弃）。
        compiled.need_trace = need_trace;
        // MAT.M7 (D1)：把本回合**派生后**的 active NPC 集（apply_npc_activation 已折进
        // state_owned）一同传出。gm 回合循环读 ctx.compiled.active_npc_ids 出对白/反应消费
        // 集，修「派生集进不了对白消费者 → NPC 对白=0」缺陷。Off/Shadow 下 apply_npc_activation
        // 无操作 ⇒ 此值 == 调用方传入集 ⇒ 字节等价基线。
        compiled.active_npc_ids = state.active_npc_ids.clone();
        // Q-MODULE DP-A'/DP-B': carry the current scene's player-deliverable establishing
        // material (NON-secret read_aloud, spoiler-redacted) to the split Narrator so module-
        // bound openings have real module texture (named location/atmosphere) instead of generic
        // prose. Enforce only; Off/Shadow ⇒ left empty ⇒ split Narrator injects nothing ⇒
        // byte-equal baseline. Secrets/clues are untouched (they stay on the A2 reward gate).
        if mat_mode.is_enforce() {
            compiled.scene_establishing = scene_establishing::collect_scene_establishing(
                &project.modules,
                mat_module_id.as_deref(),
                state.scene_id.as_deref(),
            );
            // A1 (G-1): anthology/spine 类模组的 `graph.scenes` 为空、当前是 synthetic
            // `spine:` 入口 ⇒ 图谱无场景节点 ⇒ 上面收集恒空 ⇒ Triangle 等开场退化成泛化叙事。
            // 回退:从 prep packet 的 `current_session_packet` 抽**非秘密**进场 establishing 素材,
            // 让 anthology 开场也有具名模组质感。fail-soft:任何错误/无 packet ⇒ 维持空。
            if compiled.scene_establishing.is_empty() {
                if let Some(mid) = mat_module_id.as_deref() {
                    if let Ok(Some(csp)) = self.db.load_module_prep_packet_session(mid).await {
                        compiled.scene_establishing =
                            scene_establishing::prep_packet_establishing(&csp);
                    }
                }
            }
            // OA2 (G-3): carry the PC's player-safe competency profile to the split Narrator so
            // narration can reference the PC's real abilities/traits (CoC 45+ skills/8 attrs,
            // Cyber 19/10 are in DB but never reach the Narrator). The sheet is the player's OWN
            // character ⇒ rendering it adds no leak surface. Enforce only; Off/Shadow ⇒ empty ⇒
            // injects nothing ⇒ byte-equal. fail-soft: any error / no sheet ⇒ empty.
            let pc_actor_id = request.viewer.actor_id.as_deref().unwrap_or("pc.current");
            if let Ok(Some(params)) = trpg_params::RuntimeParameterService::new(self.db.clone())
                .load_actor_parameters(&request.session_id, pc_actor_id)
                .await
            {
                // fail-closed (codex ④): only a PlayerCharacter's own sheet may render into the
                // player-visible carrier. `load_actor_parameters` keys solely by (session, actor)
                // so a mis-supplied NPC actor_id would otherwise leak that actor's numeric buckets.
                if params.actor_kind == trpg_model::ActorKind::PlayerCharacter {
                    compiled.character_context = character_context::collect_character_context(
                        &params.sheet_json,
                        params.display_name.as_deref(),
                    );
                }
            }
        }
        for block in compiled
            .prefix_blocks
            .iter()
            .chain(compiled.pinned_blocks.iter())
            .chain(compiled.dynamic_blocks.iter())
        {
            let _ = self
                .db
                .record_load_event(
                    Some(&request.session_id),
                    Some(&request.turn_id),
                    block,
                    block.load_reason.as_deref().unwrap_or("planner"),
                )
                .await;
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
        // 反剧透 TruthGraph 起步切片（优化2 #5）——观测层 write-through：当前场景引用的
        // clue/NPC 进了本回合 GM/runtime context，即记 ContextSurfaced（幂等 per-session）。
        // P0c：这是隐藏内部装载（≠ 玩家暴露），不计入玩家暴露投影、不触发关系抽取。fail-soft、
        // 加在主 context 构建完之后（非关键路径），任何失败仅 warn、绝不影响返回的 compiled。
        self.record_context_surfaced_entities(request, state).await;
        Ok(compiled)
    }

    /// 反剧透 TruthGraph T1 观测写穿（fail-soft，零行为变更）：把**当前场景**引用的
    /// 线索/NPC 记成幂等 `ContextSurfaced` 事件——"实体进了 GM/context"，**不**代表玩家
    /// 已见（玩家暴露用 `PlayerExposed`/遗留 `EntitySurfaced`）。无模组 / 无场景 / 图谱加载
    /// 失败 → warn 跳过，绝不 error 回合。scene 引用经纯函数
    /// [`context_surfaced_events_for_scene`] 映射后逐条 append（on-conflict-do-nothing 保
    /// per-session 只记一次）。
    async fn record_context_surfaced_entities(
        &self,
        request: &ContextRequest,
        state: &RuntimeState,
    ) {
        let Some(module_id) = request.module_id.as_deref().or(state.module_id.as_deref()) else {
            return; // 无模组：纯规则书会话，无场景实体可记。
        };
        let Some(scene_id) = state.scene_id.as_deref() else {
            tracing::warn!(
                module_id,
                "truthgraph: no current scene_id; skipping ContextSurfaced"
            );
            return;
        };
        let graph = match self.db.load_module_graph(module_id).await {
            Ok(Some(g)) => g,
            Ok(None) => {
                tracing::warn!(
                    module_id,
                    "truthgraph: no module graph; skipping ContextSurfaced"
                );
                return;
            }
            Err(err) => {
                tracing::warn!(error = %err, module_id, "truthgraph: load_module_graph failed; skipping ContextSurfaced");
                return;
            }
        };
        let Some(node) = graph.scenes.iter().find(|s| s.node_id == scene_id) else {
            tracing::warn!(
                module_id,
                scene_id,
                "truthgraph: current scene not in graph; skipping ContextSurfaced"
            );
            return;
        };
        let events = context_surfaced_events_for_scene(
            &request.session_id,
            &request.turn_id,
            scene_id,
            node,
        );
        for ev in &events {
            if let Err(err) = self.db.append_domain_event(ev).await {
                tracing::warn!(error = %err, event_id = %ev.event_id, "truthgraph: append ContextSurfaced failed (fail-soft)");
            }
        }
    }

    /// MAT.M7 (D2)：玩家本回合**主动接触/对抗**某 active NPC ⇒ 为它写穿一条 `PlayerExposed`
    /// （fail-soft）。这是「玩家**选择**接触在场 NPC 即是会面之举」的 met 信号——解 M6 的
    /// met/engaged 死锁（一个戏剧价值在「自报身份」的 NPC 永远进不了已会面，因 M4 闸禁其
    /// 自报）。事件由纯函数 [`met_engaged::player_engaged_met_events`] 构造（含 Enforce 门 +
    /// 「目标须在 active 集」收口 + 与念白暴露同口径的幂等 event_id）。
    ///
    /// 信号必须是引擎已**确定性解析**的接触目标（接线点取 opposed_binding.persona.actor_id，
    /// 对抗预 pass 已按 id 命中场景 NPC、fail-closed），绝非 LLM 语义猜测。
    /// Off/Shadow ⇒ 纯函数返回空 ⇒ 零写入 ⇒ 字节级基线。返回成功写入条数（便于观测/测试）。
    pub async fn record_player_engaged_npc(
        &self,
        request: &ContextRequest,
        active_npc_ids: &[String],
        engaged_npc_id: &str,
        mode: MaterializationAffordanceMode,
    ) -> usize {
        let events = met_engaged::player_engaged_met_events(
            &request.session_id,
            &request.turn_id,
            active_npc_ids,
            engaged_npc_id,
            mode,
        );
        let mut written = 0usize;
        for ev in &events {
            match self.db.append_domain_event(ev).await {
                Ok(()) => written += 1,
                Err(err) => {
                    tracing::warn!(error = %err, event_id = %ev.event_id, "MAT.M7: append player-engaged PlayerExposed failed (fail-soft)")
                }
            }
        }
        written
    }

    /// 反剧透 TruthGraph 玩家暴露写穿（fail-soft）：如果当前场景引用的 NPC/线索名字
    /// 实际出现在玩家可见叙事里，记 `PlayerExposed`。这只表示玩家见过/听见过该实体，
    /// 不表示玩家知道其隐藏事实（不会写 `KnowledgeEdge`）。
    pub async fn record_player_exposed_entities_for_narration(
        &self,
        request: &ContextRequest,
        state: &RuntimeState,
        fallback_scene_id: Option<&str>,
        narration: &str,
    ) -> usize {
        if narration.trim().is_empty() {
            return 0;
        }
        let Some(module_id) = request.module_id.as_deref().or(state.module_id.as_deref()) else {
            return 0;
        };
        let Some(scene_id) = state.scene_id.as_deref().or(fallback_scene_id) else {
            return 0;
        };
        let graph = match self.db.load_module_graph(module_id).await {
            Ok(Some(g)) => g,
            Ok(None) => return 0,
            Err(err) => {
                tracing::warn!(error = %err, module_id, "truthgraph: load_module_graph failed; skipping PlayerExposed");
                return 0;
            }
        };
        let Some(node) = graph.scenes.iter().find(|s| s.node_id == scene_id) else {
            tracing::warn!(
                module_id,
                scene_id,
                "truthgraph: current scene not in graph; skipping PlayerExposed"
            );
            return 0;
        };
        let events = player_exposed_events_for_scene_narration(
            &request.session_id,
            &request.turn_id,
            scene_id,
            node,
            &graph,
            narration,
        );
        let mut written = 0usize;
        for ev in &events {
            match self.db.append_domain_event(ev).await {
                Ok(()) => written += 1,
                Err(err) => {
                    tracing::warn!(error = %err, event_id = %ev.event_id, "truthgraph: append PlayerExposed failed (fail-soft)")
                }
            }
        }
        written
    }

    /// 子项目2 知识图谱写穿（fail-soft，env 门控 `TRPG_RELATIONSHIP_EXTRACTION` 默认开）：
    /// 在回合后处理（heavy 尾段）里，对**本局已 surface 的实体**之间，依本回合念白语义
    /// 抽取关系三元组（subject-predicate-object）并 upsert 进 `memory_facts`，供后续回合
    /// 经 `retrieve_memory` 召回。建在 [`crate::truthgraph`] 的 `EntitySurfaced` 之上（不
    /// 重扫原文找新实体），复用调用方既有 `llm` 句柄（不另 from_env）。任何缺失（特性关 /
    /// 无模组 / 念白空 / 已 surface 实体不足 2 / **本回合没 surface 新实体** / 图谱缺 /
    /// LLM 出错）→ 跳过，绝不影响回合。成本闸：实体集没变化的回合直接跳过、不烧 LLM
    /// （否则只会重得同样三元组，靠稳定 fact_id upsert 幂等但白花一次调用）。
    /// 返回成功写入的三元组条数（便于可观测 / 测试）。
    pub async fn extract_relationship_facts(
        &self,
        llm: &dyn trpg_llm::LlmClient,
        session_id: &str,
        turn_id: &str,
        module_id: Option<&str>,
        narration: &str,
        active_npc_ids: &[String],
        player_input: &str,
        scene_id: Option<&str>,
    ) -> usize {
        if !relationship_extraction::relationship_extraction_enabled()
            || narration.trim().is_empty()
        {
            return 0;
        }
        let Some(module_id) = module_id else { return 0 };
        let surfaced = self
            .db
            .list_surfaced_entities(session_id)
            .await
            .unwrap_or_default();
        // L-H(PC↔NPC)端点喂入修复:Enforce 模式下调用方 `state.active_npc_ids` 恒空(每回合派生集
        // 落在 GM compiled ctx 而非 RuntimeState)。当 PC↔NPC flag 开且调用方集为空时,退回当前场景的
        // `referenced_npc_ids`——与 PlayerExposed / npc_activation 同一 source-backed 派生——使单 NPC
        // 在场场景也能成 (pc, predicate, npc) 端点对。flag OFF 或调用方集非空 ⇒ 不派生 = 基线字节等价。
        let derived_active: Vec<String> = if relationship_extraction::pc_npc_relationship_enabled()
            && active_npc_ids.is_empty()
        {
            match self.db.load_module_graph(module_id).await {
                Ok(Some(g)) => scene_id
                    .and_then(|sid| g.scenes.iter().find(|s| s.node_id == sid))
                    .map(|n| n.referenced_npc_ids.clone())
                    .unwrap_or_default(),
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        };
        let active_npc_ids: &[String] = if derived_active.is_empty() {
            active_npc_ids
        } else {
            &derived_active
        };
        // L-H(PC↔NPC):当玩家已暴露实体不足 2,但本回合有在场 active NPC 且 flag 开,允许用
        // 「PC + 在场 NPC」做端点(下方增广),故此处不早退。OFF / 无 active NPC ⇒ 条件退化为
        // `surfaced.len() < 2` = 历史基线(字节等价)。
        let lh_pc_npc =
            relationship_extraction::pc_npc_relationship_enabled() && !active_npc_ids.is_empty();
        if surfaced.len() < 2 && !lh_pc_npc {
            return 0; // 关系至少需要两个已知端点。
        }
        // TC-D3-04 社交闸：在「本回合 surface 了新实体」之外，再放行「有 active NPC 在场 +
        // 玩家输入/念白含明确社交信号」的回合（威胁/讨价/帮助/欺骗…会改关系但不 surface 新实体）。
        // 「新实体」= EntitySurfaced 行 turn_id==本回合（append 时冻结、on-conflict 不改）。
        // fail-open：新实体判定查询出错时退回旧行为（当作有新实体照抽），宁可偶尔多花一次也不漏。
        let new_this_turn = match self
            .db
            .has_entity_surfaced_in_turn(session_id, turn_id)
            .await
        {
            Ok(b) => b,
            Err(err) => {
                tracing::warn!(error = %err, session_id, turn_id, "relationship: new-surface probe failed; proceeding (fail-open)");
                true
            }
        };
        let should_run = relationship_extraction::relationship_gate_should_run(
            new_this_turn,
            active_npc_ids.len(),
            player_input,
            narration,
        );
        if !should_run {
            return 0; // 无新实体且无 active-NPC 社交信号 → 跳过图谱加载与 LLM 调用。
        }
        let graph = match self.db.load_module_graph(module_id).await {
            Ok(Some(g)) => g,
            Ok(None) => return 0,
            Err(err) => {
                tracing::warn!(error = %err, module_id, "relationship: load_module_graph failed (fail-soft)");
                return 0;
            }
        };
        let mut entities = relationship_extraction::resolve_entity_refs(&surfaced, &graph);
        // L-H(PC↔NPC)兜底:NPC↔NPC 端点不足 2(典型:入口隐名单 NPC 场景,玩家暴露集 <2)时,
        // 若有在场 active NPC + flag 开,改用「在场 NPC + 合成 PC 端点」抽 (pc, predicate, npc)
        // 关系——这是玩家与所遇 NPC 的最基本记忆,不依赖场景推进/不依赖玩家上 spine。memory_fact
        // 为 GmOnly(不直出玩家,隐名 NPC 不剧透)。flag OFF / 无在场 NPC ⇒ 此块跳过 = 基线字节等价。
        if entities.len() < 2 && relationship_extraction::pc_npc_relationship_enabled() {
            let active_refs =
                relationship_extraction::resolve_active_npc_refs(active_npc_ids, &graph);
            if !active_refs.is_empty() {
                let mut augmented = active_refs;
                augmented.push(relationship_extraction::pc_entity_ref());
                entities = augmented; // PC + ≥1 在场 NPC ⇒ ≥2 端点
            }
        }
        if entities.len() < 2 {
            return 0;
        }
        let facts = relationship_extraction::relationship_facts_from_inputs(
            llm,
            session_id,
            turn_id,
            &entities,
            narration,
            relationship_extraction::relationship_min_confidence(),
            should_run, // 成本闸：闸关则函数内 fail-closed 返回空（与上面早退一致）。
        )
        .await;
        // TC-D3-03 proposal contract: the extractor only *proposes*. The heavy postprocess
        // path no longer writes `memory_facts` directly — it converts each extracted triple
        // into a validated `MemoryExtractionProposal` and hands the batch to the single
        // runtime-owned review/commit entry point. `review_and_commit_proposals` re-runs the
        // model gate + the session-authority guard before any write, then routes each
        // surviving fact through the same `memory_facts` upsert (as a `LegacyFact`) the old
        // loop used — so live behavior (rows in `memory_facts`, later retrievable) is
        // unchanged, but the only durable path is now the proposal commit pipeline.
        let proposals = memory_proposal::relationship_facts_to_proposals(&facts);
        let report = match memory_proposal::review_and_commit_proposals(
            &self.db,
            &memory_proposal::CommitContext {
                session_id,
                turn_id,
            },
            &proposals,
        )
        .await
        {
            Ok(report) => report,
            // Fail-soft: a proposal-commit error must never affect TurnComplete. Warn (no
            // fact prose — the error string is a DB/plumbing message) and report zero writes.
            Err(err) => {
                tracing::warn!(error = %err, session_id, turn_id, "relationship proposal commit failed (fail-soft)");
                return 0;
            }
        };
        let written = report.committed();
        if written > 0 {
            // Only identity counts are logged, never fact bodies (see CommitOutcome docs).
            tracing::info!(
                session_id,
                turn_id,
                committed = written,
                rejected = report.rejected(),
                skipped = report.skipped(),
                "relationship triples committed to memory_facts via proposal pipeline"
            );
        }
        written
    }

    /// L-W: 把本回合 kernel 已判 SUCCESS 的检定投射成 durable source-backed world_facts。
    ///
    /// 理念 §二.1/§二.3/§二.8:CheckResolved 已落账(权威),这里只把那条**已决定**事件投射成
    /// 一条 world_fact(用检定自身 `stakes.success_public`,无 LLM/无发明)。修复 J2 真根——普通
    /// 技能检定 SUCCESS 不进 committed_patches 且无 proposer 写 world_facts ⇒ 后果蒸发进念白。
    ///
    /// flag `TRPG_CHECK_OUTCOME_WORLD_FACT` 默认 ON;OFF ⇒ 无派生 = 基线字节等价。world_facts 行
    /// 的实际落盘仍由 `TRPG_KNOWLEDGE_KERNEL`(shadow/enforce)在 commit 管线门控:kernel Off ⇒
    /// 提案只落 memory_facts(基线),不写 world_facts/knowledge_edge。fail-soft:任何 DB/commit
    /// 错误只 warn、返回 0,绝不影响已发的 TurnComplete(D2)。
    pub async fn extract_check_outcome_world_facts(
        &self,
        session_id: &str,
        turn_id: &str,
    ) -> usize {
        if !check_outcome_facts::check_outcome_world_fact_enabled() {
            return 0;
        }
        let resolved = match self
            .db
            .list_resolved_check_outcomes_for_turn(session_id, turn_id)
            .await
        {
            Ok(rows) => rows,
            Err(err) => {
                tracing::warn!(error = %err, session_id, turn_id, "check-outcome world_fact: list resolved checks failed (fail-soft)");
                return 0;
            }
        };
        let proposals: Vec<_> = resolved
            .iter()
            .filter_map(|(contract, degree)| {
                check_outcome_facts::world_fact_from_check(contract, degree, turn_id)
            })
            .collect();
        if proposals.is_empty() {
            return 0;
        }
        let report = match memory_proposal::review_and_commit_proposals(
            &self.db,
            &memory_proposal::CommitContext {
                session_id,
                turn_id,
            },
            &proposals,
        )
        .await
        {
            Ok(report) => report,
            Err(err) => {
                tracing::warn!(error = %err, session_id, turn_id, "check-outcome world_fact commit failed (fail-soft)");
                return 0;
            }
        };
        let written = report.committed();
        if written > 0 {
            tracing::info!(
                session_id,
                turn_id,
                committed = written,
                "check-outcome world_facts committed via proposal pipeline"
            );
        }
        written
    }

    /// 反剧透 revealed-facts 写路径：把某条剧透事实（entity_id/node_id 作 fact_id）记为
    /// 已揭示，自此 spoiler_guard 不再裁该实体的 secret_terms。揭示**由 GM/玩家显式驱动**
    /// （引擎绝不关键词匹配 reveal_conditions 自动揭示）；幂等 per-session+fact。
    pub async fn reveal_fact(
        &self,
        session_id: &str,
        turn_id: &str,
        fact_id: &str,
        reason: Option<&str>,
    ) -> Result<()> {
        self.db
            .record_revealed_fact(session_id, turn_id, fact_id, reason)
            .await
    }

    /// Single post-resolution funnel for EVERY check-resolution path. Runs the
    /// universal referee combat hook (resource tracks, HP, effect/damage
    /// follow-ups) AND generic object rule effects (e.g. a firearm spending a
    /// round on attack). Centralizing here is why object effects can't miss the
    /// auto-roll combat route the way the old per-path hooks did.
    async fn post_check_resolution(
        &self,
        contract: &CheckContract,
        result: &mut CheckResultRecord,
    ) -> Result<Option<FollowupCheck>> {
        let followup = RefereeCombatService::new(self.db.clone())
            .after_check_resolved(contract, result)
            .await?;
        let _ = ObjectService::new(self.db.clone())
            .apply_object_rule_effects(&contract.session_id, contract)
            .await;
        Ok(followup)
    }

    pub async fn try_resolve_pending_check(
        &self,
        session_id: &str,
        turn_id: &str,
        user_input: &str,
    ) -> Result<Option<CheckResultRecord>> {
        let Some(pending) = self.db.get_open_pending_check(session_id).await? else {
            return Ok(None);
        };
        if !roll_input_available(user_input) {
            return Ok(None);
        }
        let roll = self
            .resolve_roll_input(
                session_id,
                turn_id,
                Some(&pending.check_id),
                &pending.contract,
                user_input,
            )
            .await?;
        let outcome = self
            .resolve_outcome_with_opposition(&pending.contract, &roll)
            .await?;
        let result = CheckResultRecord {
            check_id: pending.check_id.clone(),
            roll,
            outcome,
            committed_patches: vec![],
            created_at: chrono::Utc::now(),
        };
        let mut result = result;
        if let Ok(Some(object_result)) = ObjectService::new(self.db.clone())
            .apply_for_check_result(&result)
            .await
        {
            result.committed_patches.push(StatePatch::ObjectPatch {
                patch_id: format!("object_result_{}", object_result.interaction_id),
                object_id: object_result.applied_patches.first().and_then(|p| match p {
                    ObjectPatch::TransferObject { object_id, .. }
                    | ObjectPatch::SetObjectLocation { object_id, .. }
                    | ObjectPatch::SetObjectVisibility { object_id, .. }
                    | ObjectPatch::ModifyQuantity { object_id, .. }
                    | ObjectPatch::DamageObject { object_id, .. }
                    | ObjectPatch::DestroyObject { object_id, .. }
                    | ObjectPatch::SetMechanicalState { object_id, .. }
                    | ObjectPatch::TransformObject { object_id, .. } => Some(object_id.clone()),
                    ObjectPatch::CreateObjectInstance { object, .. } => {
                        Some(object.object_id.clone())
                    }
                    ObjectPatch::AddObjectEdge { edge, .. } => Some(edge.from_object_id.clone()),
                    ObjectPatch::RemoveObjectEdge { edge_id, .. } => Some(edge_id.clone()),
                }),
                patch_json: serde_json::to_value(&object_result).unwrap_or_else(|_| json!({})),
                reason: "object_interaction_result".into(),
            });
        }
        let _ = self
            .post_check_resolution(&pending.contract, &mut result)
            .await;
        self.db.insert_check_result(&result).await?;
        self.db
            .update_pending_check_status(&pending.check_id, PendingCheckStatus::Resolved)
            .await?;
        self.close_resolved_check_gate(session_id, &pending.check_id)
            .await;
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
                let _ = lifecycle
                    .supersede_gate_for_terminal_intent(
                        gate,
                        "superseded_by_terminal_intent",
                        user_input,
                    )
                    .await;
                if let Some(check_id) = match &gate.expected_input {
                    ExpectedInput::RollResult { check_id, .. } => Some(check_id.clone()),
                    _ => None,
                } {
                    let _ = self
                        .db
                        .supersede_pending_check(&check_id, "superseded_by_terminal_intent", None)
                        .await;
                }
                return Ok(GateHandlingResult::GateSuperseded {
                    gate_id: gate.gate_id.clone(),
                    reason: "superseded_by_terminal_intent".into(),
                    prompt_public: "[system]上一个交互窗口已关闭，继续处理你的新动作。[/system]"
                        .into(),
                });
            }
        }
        let Some(pending) = self.db.get_open_pending_check(session_id).await? else {
            // v1.12.2: a `/roll` or natural roll result must bind to the latest
            // unresolved mechanical check before any advisory direction gate can
            // consume it. This turns `/roll` from an orphan RNG call into a
            // resolution of the active attack/effect/check whenever possible.
            if roll_input_available(user_input) {
                if let Some(contract) = self
                    .db
                    .get_latest_unresolved_check_contract(session_id)
                    .await?
                {
                    let result = self
                        .resolve_check_with_input(session_id, turn_id, &contract, user_input)
                        .await?;
                    if let Some(next_pending) = self.db.get_open_pending_check(session_id).await? {
                        if next_pending.check_id != contract.check_id
                            && is_effect_or_damage_contract(&next_pending.contract)
                        {
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
                    let _ = lifecycle
                        .supersede_gate_for_terminal_intent(
                            &gate,
                            SupersededReason::TerminalIntent.as_str(),
                            user_input,
                        )
                        .await;
                    return Ok(GateHandlingResult::GateSuperseded {
                        gate_id: gate.gate_id.clone(),
                        reason: SupersededReason::TerminalIntent.as_str().into(),
                        prompt_public:
                            "[system]上一个交互窗口已关闭，继续处理你的新动作。[/system]".into(),
                    });
                }
                return self.handle_choice_or_nonroll_gate(gate, user_input).await;
            }
            return Ok(GateHandlingResult::None);
        };
        let gate = open_gate.unwrap_or_else(|| InteractionGate::from_pending_check(&pending));
        if lifecycle.player_input_supersedes_gate(&gate, user_input) {
            self.db
                .update_pending_check_status(&pending.check_id, PendingCheckStatus::Superseded)
                .await
                .ok();
            let _ = lifecycle
                .supersede_gate_for_terminal_intent(
                    &gate,
                    SupersededReason::TerminalIntent.as_str(),
                    user_input,
                )
                .await;
            return Ok(GateHandlingResult::GateSuperseded {
                gate_id: gate.gate_id.clone(),
                reason: SupersededReason::TerminalIntent.as_str().into(),
                prompt_public: "[system]上一个检定/反应窗口已关闭，继续处理你的新动作。[/system]"
                    .into(),
            });
        }

        if roll_input_available(user_input) {
            let roll = self
                .resolve_roll_input(
                    session_id,
                    turn_id,
                    Some(&pending.check_id),
                    &pending.contract,
                    user_input,
                )
                .await?;
            let outcome = self
                .resolve_outcome_with_opposition(&pending.contract, &roll)
                .await?;
            let result = CheckResultRecord {
                check_id: pending.check_id.clone(),
                roll,
                outcome,
                committed_patches: vec![],
                created_at: chrono::Utc::now(),
            };
            let mut result = result;
            if let Ok(Some(object_result)) = ObjectService::new(self.db.clone())
                .apply_for_check_result(&result)
                .await
            {
                result.committed_patches.push(StatePatch::ObjectPatch {
                    patch_id: format!("object_result_{}", object_result.interaction_id),
                    object_id: object_result.applied_patches.first().and_then(|p| match p {
                        ObjectPatch::TransferObject { object_id, .. }
                        | ObjectPatch::SetObjectLocation { object_id, .. }
                        | ObjectPatch::SetObjectVisibility { object_id, .. }
                        | ObjectPatch::ModifyQuantity { object_id, .. }
                        | ObjectPatch::DamageObject { object_id, .. }
                        | ObjectPatch::DestroyObject { object_id, .. }
                        | ObjectPatch::SetMechanicalState { object_id, .. }
                        | ObjectPatch::TransformObject { object_id, .. } => Some(object_id.clone()),
                        ObjectPatch::CreateObjectInstance { object, .. } => {
                            Some(object.object_id.clone())
                        }
                        ObjectPatch::AddObjectEdge { edge, .. } => {
                            Some(edge.from_object_id.clone())
                        }
                        ObjectPatch::RemoveObjectEdge { edge_id, .. } => Some(edge_id.clone()),
                    }),
                    patch_json: serde_json::to_value(&object_result).unwrap_or_else(|_| json!({})),
                    reason: "object_interaction_result".into(),
                });
            }
            if let Ok(Some(followup)) = self
                .post_check_resolution(&pending.contract, &mut result)
                .await
            {
                self.db.insert_check_result(&result).await?;
                self.db
                    .update_pending_check_status(&pending.check_id, PendingCheckStatus::Resolved)
                    .await?;
                self.db.update_interaction_gate_status(&gate.gate_id, GateStatus::Resolved, json!({"check_id": pending.check_id, "result": result.clone(), "followup": "damage_roll"})).await.ok();
                return Ok(GateHandlingResult::PendingFollowupCheckCreated {
                    result,
                    pending: Box::new(followup.pending),
                    prompt_public: followup.prompt_public,
                    reason: followup.reason,
                });
            }
            self.db.insert_check_result(&result).await?;
            self.db
                .update_pending_check_status(&pending.check_id, PendingCheckStatus::Resolved)
                .await?;
            self.db
                .update_interaction_gate_status(
                    &gate.gate_id,
                    GateStatus::Resolved,
                    json!({"check_id": pending.check_id, "result": result.clone()}),
                )
                .await
                .ok();
            return Ok(GateHandlingResult::PendingCheckResolved { result });
        }

        let should_reprompt = looks_like_gate_help_or_question(user_input)
            || (matches!(
                gate.on_unparseable,
                GateFallbackPolicy::Reprompt | GateFallbackPolicy::RequireExplicitChoice
            ) && !looks_like_new_action_or_abandon(user_input));
        if should_reprompt {
            let prompt = format!(
                "[system]上一项检定仍在等待投骰授权。请只回复 `roll`，系统会调用骰子工具；如果要取消，请直接描述新的角色行动。[/system]\n\n{}",
                gate.prompt_public,
            );
            self.db.update_interaction_gate_status(&gate.gate_id, GateStatus::Open, json!({"last_unparseable_input": user_input, "reason": "unparseable_roll_reply"})).await.ok();
            return Ok(GateHandlingResult::GateReprompt {
                gate_id: gate.gate_id,
                check_id: pending.check_id,
                reason: "unparseable_roll_reply".into(),
                prompt_public: prompt,
            });
        }

        self.db
            .update_pending_check_status(
                &pending.check_id,
                PendingCheckStatus::AbandonedByNewAction,
            )
            .await?;
        self.db
            .update_interaction_gate_status(
                &gate.gate_id,
                GateStatus::AbandonedByNewAction,
                json!({"reason": "player_started_new_action", "new_input": user_input}),
            )
            .await
            .ok();
        let prompt = "[system]上一个未完成检定已关闭，继续处理你的新动作。[/system]".to_string();
        Ok(GateHandlingResult::GateAbandoned {
            gate_id: gate.gate_id,
            check_id: pending.check_id,
            reason: "player_started_new_action".into(),
            prompt_public: prompt,
        })
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
            return orchestrator
                .reduce_turn(TurnOrchestratorInput {
                    session_id: &request.session_id,
                    turn_id: &request.turn_id,
                    ruleset_id: &request.ruleset_id,
                    module_id: request.module_id.as_deref(),
                    user_input,
                })
                .await;
        }
        TurnOrchestrator::new(self.db.clone())
            .reduce_turn(TurnOrchestratorInput {
                session_id: &request.session_id,
                turn_id: &request.turn_id,
                ruleset_id: &request.ruleset_id,
                module_id: request.module_id.as_deref(),
                user_input,
            })
            .await
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
        let world_tick = self
            .current_world_time(&request.session_id)
            .await
            .map(|t| t.world_tick)
            .unwrap_or_default();
        let frame_id = self
            .db
            .list_active_state_frames(&request.session_id, 1)
            .await
            .ok()
            .and_then(|frames| frames.into_iter().next().map(|f| f.frame_id));
        let actor_id = request.viewer.actor_id.as_deref().or(Some("pc.current"));
        AbilityService::new(self.db.clone(), self.search.clone())
            .handle_turn(AbilityTurnInput {
                session_id: &request.session_id,
                turn_id: &request.turn_id,
                ruleset_id: &request.ruleset_id,
                module_id: request.module_id.as_deref().or(state.module_id.as_deref()),
                actor_id,
                frame_id: frame_id.as_deref(),
                user_input,
                world_tick,
            })
            .await
    }

    pub async fn referee_combat_blocks_for_turn(
        &self,
        request: &ContextRequest,
    ) -> Result<Vec<ContextBlock>> {
        let world_tick = self
            .current_world_time(&request.session_id)
            .await
            .map(|t| t.world_tick)
            .unwrap_or_default();
        let block = RefereeCombatService::new(self.db.clone())
            .mechanical_ledger_context_block(&request.session_id, &request.ruleset_id, world_tick)
            .await?;
        Ok(vec![block])
    }

    pub async fn contest_blocks_for_turn(
        &self,
        request: &ContextRequest,
    ) -> Result<Vec<ContextBlock>> {
        let world_tick = self
            .current_world_time(&request.session_id)
            .await
            .map(|t| t.world_tick)
            .unwrap_or_default();
        let block = ContestService::new(self.db.clone())
            .contest_context_block(&request.session_id, world_tick)
            .await?;
        Ok(vec![block])
    }

    pub async fn verify_player_supplied_values(
        &self,
        request: &ContextRequest,
        user_input: &str,
    ) -> Result<Option<PlayerValueRefereeResult>> {
        let result = PlayerValueRefereeService::new(self.db.clone())
            .inspect_turn(
                &request.session_id,
                &request.turn_id,
                &request.ruleset_id,
                request.module_id.as_deref(),
                user_input,
            )
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
                    Scope {
                        scope_type: ScopeType::Turn,
                        scope_id: request.turn_id.clone(),
                    },
                    142,
                );
                block.tags = vec![
                    "player_value_referee".into(),
                    "rules_first".into(),
                    "table_override_policy".into(),
                ];
                block.expires_at_turn = Some(request.turn_id.clone());
                block.load_reason = Some("player_supplied_value_verification".into());
                let _ = self
                    .db
                    .upsert_runtime_context_block(&request.session_id, &block)
                    .await;
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
        let world_tick = self
            .current_world_time(&request.session_id)
            .await
            .map(|t| t.world_tick)
            .unwrap_or_default();
        let frame_id = self
            .db
            .list_active_state_frames(&request.session_id, 1)
            .await
            .ok()
            .and_then(|frames| frames.into_iter().next().map(|f| f.frame_id));
        let actor_id = request.viewer.actor_id.as_deref().or(Some("pc.current"));
        MaterializationService::from_env(self.db.clone(), self.search.clone())
            .materialize_turn(MaterializationTurnInput {
                session_id: &request.session_id,
                turn_id: &request.turn_id,
                ruleset_id: &request.ruleset_id,
                module_id: request.module_id.as_deref().or(state.module_id.as_deref()),
                frame_id: frame_id.as_deref(),
                actor_id,
                user_input,
                world_tick,
            })
            .await
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
        let frame_id = self
            .db
            .list_active_state_frames(&request.session_id, 1)
            .await
            .ok()
            .and_then(|frames| frames.into_iter().next().map(|f| f.frame_id));
        let actor_id = request.viewer.actor_id.as_deref().or(Some("pc.current"));
        ObjectService::new(self.db.clone())
            .handle_turn(ObjectTurnInput {
                session_id: &request.session_id,
                turn_id: &request.turn_id,
                ruleset_id: &request.ruleset_id,
                module_id: request.module_id.as_deref().or(state.module_id.as_deref()),
                actor_id,
                frame_id: frame_id.as_deref(),
                user_input,
            })
            .await
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
        agent
            .handle_turn(ConflictTurnInput {
                session_id: &request.session_id,
                turn_id: &request.turn_id,
                ruleset_id: &request.ruleset_id,
                module_id: request.module_id.as_deref().or(state.module_id.as_deref()),
                actor_id,
                user_input,
                semantic_hint: semantic_hint.as_ref(),
            })
            .await
    }

    fn conflict_hint_from_orchestration(
        result: &TurnOrchestrationResult,
    ) -> Option<ConflictIntent> {
        let action = result.intent.action_kind;
        let frame_relevant = matches!(
            action,
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
                | SituationActionKind::CastOrUsePower
        );
        let terminal = matches!(
            result.intent.frame_relation,
            FrameRelation::ExitAttempt
                | FrameRelation::DeescalationAttempt
                | FrameRelation::Surrender
                | FrameRelation::Flee
                | FrameRelation::HideToDisengage
        );
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
            escalation_level: if matches!(
                action,
                SituationActionKind::Attack
                    | SituationActionKind::UnderAttack
                    | SituationActionKind::EnemyInitiatedConflict
                    | SituationActionKind::SceneEntersConflict
            ) {
                EscalationLevel::High
            } else {
                EscalationLevel::Medium
            },
            target_refs: vec![result.intent.target_kind.clone()],
            desired_outcome: Some(
                match action {
                    SituationActionKind::Attack | SituationActionKind::Counterattack => {
                        "resolve the declared attack or sustained pressure"
                    }
                    SituationActionKind::UnderAttack
                    | SituationActionKind::EnemyInitiatedConflict
                    | SituationActionKind::SceneEntersConflict => "respond to incoming danger",
                    SituationActionKind::Flee | SituationActionKind::LeaveScene => {
                        "exit the active frame"
                    }
                    SituationActionKind::Negotiate
                    | SituationActionKind::Surrender
                    | SituationActionKind::EndConflict => "de-escalate or close the conflict",
                    _ => "continue the active frame action",
                }
                .into(),
            ),
            confidence: result.intent.confidence,
            evidence_terms: evidence,
            classifier: "turn_orchestrator_semantic_hint_v1_13_3".into(),
        })
    }
    async fn handle_choice_or_nonroll_gate(
        &self,
        gate: InteractionGate,
        user_input: &str,
    ) -> Result<GateHandlingResult> {
        match &gate.expected_input {
            ExpectedInput::Choice { option_ids: _ } => {
                let lower = user_input.to_lowercase();
                if let Some(option) =
                    infer_semantic_choice_option(&gate, user_input).or_else(|| {
                        gate.allowed_options.iter().find(|opt| {
                            lower.contains(&opt.option_id.to_lowercase())
                                || (!opt.label.is_empty()
                                    && lower.contains(&opt.label.to_lowercase()))
                        })
                    })
                {
                    let resolution = json!({"option_id": option.option_id, "label": option.label, "input": user_input, "resolver":"semantic_choice_gate_v1_3"});
                    self.db
                        .update_interaction_gate_status(
                            &gate.gate_id,
                            GateStatus::Resolved,
                            resolution.clone(),
                        )
                        .await
                        .ok();
                    return Ok(GateHandlingResult::GateChoiceResolved {
                        gate_id: gate.gate_id.clone(),
                        option_id: option.option_id.clone(),
                        option_label: option.label.clone(),
                        resolution_json: resolution,
                    });
                }
                let reprompt_count = gate
                    .resolution_json
                    .as_ref()
                    .and_then(|v| v.get("reprompt_count"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    + 1;
                if reprompt_count >= 2 {
                    if let Some(option) = default_choice_after_reprompt(&gate) {
                        let resolution = json!({"option_id": option.option_id, "label": option.label, "input": user_input, "resolver":"auto_default_after_reprompt", "reprompt_count": reprompt_count});
                        self.db
                            .update_interaction_gate_status(
                                &gate.gate_id,
                                GateStatus::Resolved,
                                resolution.clone(),
                            )
                            .await
                            .ok();
                        return Ok(GateHandlingResult::GateChoiceResolved {
                            gate_id: gate.gate_id.clone(),
                            option_id: option.option_id.clone(),
                            option_label: option.label.clone(),
                            resolution_json: resolution,
                        });
                    }
                }
                let prompt = format!(
                    "当前必须先处理一个交互窗口：{}
可选项：{}
如果你想继续当前压制/攻击，可以直接说“继续”或“继续开火”；如果要换方向，请说出目标。",
                    gate.prompt_public,
                    gate.allowed_options
                        .iter()
                        .map(|o| format!("{} ({})", o.label, o.option_id))
                        .collect::<Vec<_>>()
                        .join("；")
                );
                self.db.update_interaction_gate_status(&gate.gate_id, GateStatus::Open, json!({"last_unparseable_input": user_input, "reason":"unparseable_choice_reply", "reprompt_count": reprompt_count})).await.ok();
                return Ok(GateHandlingResult::GateChoiceReprompt {
                    gate_id: gate.gate_id,
                    reason: "unparseable_choice_reply".into(),
                    prompt_public: prompt,
                });
            }
            ExpectedInput::Confirmation {
                yes_option,
                no_option,
            } => {
                let lower = user_input.to_lowercase();
                let selected = if ["yes", "y", "确认", "继续", "同意"]
                    .iter()
                    .any(|t| lower.contains(t))
                {
                    Some(yes_option.clone())
                } else if ["no", "n", "取消", "不", "算了"]
                    .iter()
                    .any(|t| lower.contains(t))
                {
                    Some(no_option.clone())
                } else {
                    None
                };
                if let Some(option_id) = selected {
                    let resolution = json!({"option_id": option_id, "input": user_input});
                    self.db
                        .update_interaction_gate_status(
                            &gate.gate_id,
                            GateStatus::Resolved,
                            resolution.clone(),
                        )
                        .await
                        .ok();
                    return Ok(GateHandlingResult::GateChoiceResolved {
                        gate_id: gate.gate_id.clone(),
                        option_id,
                        option_label: "confirmation".into(),
                        resolution_json: resolution,
                    });
                }
                let prompt = format!("请先确认：{}", gate.prompt_public);
                return Ok(GateHandlingResult::GateChoiceReprompt {
                    gate_id: gate.gate_id,
                    reason: "unparseable_confirmation".into(),
                    prompt_public: prompt,
                });
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
        // P0-2 T4: load the module's director facilitation overlay as data
        // (replaces trpg-director's deleted is_homecoming hardcode). fail-soft:
        // None → generic director path.
        let module_cfg = if let Some(mid) = &request.module_id {
            self.db.load_module_config(mid).await
        } else {
            None
        };
        // Prior spotlight states are the persistence surface the tracker carries and
        // increments across turns. fail-soft: empty on the first turn or a read error.
        let prior_spotlights = self
            .db
            .load_spotlight_states(&request.session_id)
            .await
            .unwrap_or_default();
        // Real player roster: the session's player-character actor rows ARE the
        // roster (no session→PC table; solo-focused product). The acting actor
        // follows the runtime's pervasive viewer-or-`pc.current` convention so the
        // PC who acted gets credited even when the turn viewer is the GM. fail-soft:
        // empty roster (read error / before the first ensure) ≡ `&[]`, so the
        // director's viewer-derived solo fallback still runs (zero regression).
        let acting_actor_id = request.viewer.actor_id.as_deref().unwrap_or("pc.current");
        let player_actors = RuntimeParameterService::new(self.db.clone())
            .list_player_actor_parameters(&request.session_id)
            .await
            .unwrap_or_default();
        let participants =
            spotlight_roster::build_spotlight_participants(&player_actors, acting_actor_id);
        // MAT.M4 (§7-#6): present vs met/engaged gate for PROACTIVE Director leverage.
        // Under materialization Enforce, only NPCs the player has already met/engaged (their
        // id in the player-exposed set) may be pushed as proactive leverage; present-but-un-met
        // active NPCs are not (they still react via the World path). Off/Shadow ⇒ `None` ⇒ the
        // director uses the full active set exactly as before (byte-identical baseline).
        let mat_mode = MaterializationAffordanceMode::from_env();
        let leverage_ids: Option<Vec<String>> = if mat_mode.is_enforce() {
            let exposed = self
                .db
                .list_surfaced_entities(&request.session_id)
                .await
                .unwrap_or_default();
            let gate =
                met_engaged::derive_met_engaged_gate(&state.active_npc_ids, &exposed, mat_mode);
            Some(met_engaged::director_leverage_npc_ids(&gate))
        } else {
            None
        };
        let result = director.prepare(DirectorInput {
            request,
            state,
            compiled,
            user_input,
            conflict,
            module_config: module_cfg.as_ref(),
            participants: &participants,
            prior_spotlights: &prior_spotlights,
            leverage_npc_ids: leverage_ids.as_deref(),
        });
        if let Some(brief) = &result.brief {
            self.db.insert_actionable_situation_brief(brief).await.ok();
            let _ = self
                .db
                .upsert_runtime_context_block(
                    &request.session_id,
                    &actionable_situation_block(brief, &request.turn_id),
                )
                .await;
        }
        if let Some(board) = &result.clue_board {
            self.db.upsert_player_facing_clue_board(board).await.ok();
            let _ = self
                .db
                .upsert_runtime_context_block(
                    &request.session_id,
                    &clue_board_block(board, &request.turn_id),
                )
                .await;
        }
        if let Some(consequence) = &result.consequence {
            self.db.insert_consequence_contract(consequence).await.ok();
        }
        for tick in &result.clock_ticks {
            self.db
                .insert_clock_tick(&request.session_id, &request.turn_id, tick)
                .await
                .ok();
            // P6.3 (codex#3): write-through a ClockAdvanced domain event keyed on the
            // RESULTING clock state (clock_id + the applied `current` value) — so a
            // retry-advance to the same value folds to one row, while a real advance to a
            // new value lands a new row. Additive + fail-soft: the clock state write above
            // is unchanged; only the domain_events row is new (OFF==baseline mechanically).
            let ev = DomainEvent::new(
                format!(
                    "de_clock_{}_{}_{}",
                    request.session_id, tick.clock_id, tick.current
                ),
                request.session_id.clone(),
                request.turn_id.clone(),
                DomainEventKind::ClockAdvanced,
                serde_json::json!({
                    "clock_id": tick.clock_id,
                    "new_value": tick.current,
                    "delta": tick.current - tick.previous,
                }),
            );
            if let Err(err) = self.db.append_domain_event(&ev).await {
                tracing::warn!(error = %err, event_id = %ev.event_id, "append ClockAdvanced domain event failed (non-fatal)");
            }
        }
        for spotlight in &result.spotlight {
            self.db
                .upsert_spotlight_state(&request.session_id, spotlight)
                .await
                .ok();
        }
        Ok(result)
    }

    /// Close the interaction gate that belongs to an already-resolved check.
    ///
    /// Auto-resolve paths bypass `handle_open_interaction_gate`, so without this
    /// a stale `player_roll_required` gate can remain open and hijack later turns.
    async fn close_resolved_check_gate(&self, session_id: &str, check_id: &str) {
        self.db
            .update_pending_check_status(check_id, PendingCheckStatus::Resolved)
            .await
            .ok();

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
                let _ = self
                    .db
                    .update_interaction_gate_status(
                        &gate.gate_id,
                        GateStatus::Resolved,
                        json!({"check_id": check_id, "auto_resolved_gate_close": true}),
                    )
                    .await;
            }
        }
    }

    fn blocked_missing_source_check_result(
        &self,
        session_id: &str,
        turn_id: &str,
        contract: &CheckContract,
        reason: impl Into<String>,
    ) -> CheckResultRecord {
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
        let mut normalized = normalize_contract_for_system_roll(contract);
        // Symmetry with the system-roll paths (execute_agent_roll /
        // execute_system_roll_bundle): bind the ruleset kernel's data-driven
        // defaults BEFORE rolling so a player-input check and a system-rolled check
        // resolve through the same target/dice binding. The binding is a no-op when
        // the contract is already source-backed, and (post-B1) never degrades a
        // usable provisional StaticNumber to `provisional` when the kernel has no
        // replacement — so this is additive, not a regression.
        self.apply_kernel_defaults_if_unsourced(&mut normalized)
            .await;
        if Self::contract_missing_source_backed_parameters(&normalized) {
            let blocked = self.blocked_missing_source_check_result(
                session_id,
                turn_id,
                &normalized,
                "source-backed mechanical parameters are missing",
            );
            self.db.insert_check_result(&blocked).await.ok();
            self.close_resolved_check_gate(session_id, &contract.check_id)
                .await;
            return Ok(blocked);
        }
        let roll = self
            .resolve_roll_input(
                session_id,
                turn_id,
                Some(&normalized.check_id),
                &normalized,
                input,
            )
            .await?;
        let outcome = self
            .resolve_outcome_with_opposition(&normalized, &roll)
            .await?;
        let mut result = CheckResultRecord {
            check_id: normalized.check_id.clone(),
            roll,
            outcome,
            committed_patches: vec![],
            created_at: chrono::Utc::now(),
        };
        let _ = self.post_check_resolution(&normalized, &mut result).await;
        self.db.insert_check_result(&result).await?;
        self.close_resolved_check_gate(session_id, &contract.check_id)
            .await;
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
        if !c.source_refs.is_empty() || !c.learned_packet_ids.is_empty() {
            return;
        }
        let kernel = match self.db.load_rule_kernel(&c.ruleset_id).await {
            Ok(Some(k)) => k,
            _ => return,
        };
        if let Some(d) = kernel
            .dice_core
            .get("dice")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            c.dice_expression = d;
        }
        // PARAM-DRIVEN POOL (TRPG_PARAM_DRIVEN_POOL, default OFF): a count_faces
        // pool whose kernel declares `dice_core.pool_scaling_parameter` lets the
        // ACTOR's competency rating set the pool SIZE (base + per_rank*rating)
        // instead of a flat constant. Generic + data-driven (the kernel NAMES the
        // param; the engine reads it off the actor sheet) — zero ruleset_id
        // branching. OFF, or no scaling field, or no readable rating → the flat
        // expr above is untouched (byte-identical legacy behavior).
        self.scale_pool_if_param_driven(c, &kernel.dice_core).await;
        if let Some(bound) = kernel_default_target_binding(&c.target, &kernel.dice_core) {
            c.target = bound;
        }
        if !kernel.source_refs.is_empty() {
            c.source_refs = kernel.source_refs.clone();
            c.ruling_status = RulingStatus::SourceBacked;
        } else if kernel_dicecore_is_source_enabled() && kernel_dice_core_is_typed(&kernel.dice_core)
        {
            // KERNEL-DICECORE-IS-SOURCE (BUG-1, default ON kill-switch
            // `TRPG_KERNEL_DICECORE_IS_SOURCE`): when the parsed ruleset kernel
            // carries a *typed* core mechanic (a non-empty `dice` + a `compare`
            // rule) but its `source_refs` array happens to be empty (a parse
            // METADATA gap, not a "no rule exists" case), the bound dice/target
            // above ARE the source-backed ruleset rule. Cite the kernel itself so
            // the missing-source-backed guard recognizes the check as
            // ruleset-grounded instead of over-blocking an otherwise-resolvable
            // check (理念 §二: mechanics come from parsed source, never invented;
            // zero ruleset_id branching — any kernel with a typed core qualifies).
            // OFF (`0/false/off/no`) → no synth ref → byte-identical legacy.
            c.source_refs.push(SourceRef {
                source_id: format!("rule_kernel:{}:dice_core", c.ruleset_id),
                anchor_id: Some("dice_core".to_string()),
                section_path: vec!["dice_core".to_string()],
                note: Some(
                    "ruleset kernel typed core mechanic (parsed source-backed resolution rule)"
                        .to_string(),
                ),
                ..Default::default()
            });
            c.ruling_status = RulingStatus::SourceBacked;
        }
    }

    /// Param-driven pool sizing (flag-gated, additive). When `TRPG_PARAM_DRIVEN_POOL`
    /// is ON and `dice_core.pool_scaling_parameter` names a competency parameter, read
    /// the initiating actor's rating for that param and rebuild the count_faces pool
    /// expression to `base + per_rank*rating` dice (faces preserved). A competent vs
    /// incompetent agent thus rolls a DIFFERENT pool. fail-soft at every step: flag OFF,
    /// no scaling field, no actor params, or unreadable/non-numeric rating → leave the
    /// flat expr exactly as set (byte-identical legacy behavior).
    async fn scale_pool_if_param_driven(
        &self,
        c: &mut CheckContract,
        dice_core: &serde_json::Value,
    ) {
        if !env_bool_runtime("TRPG_PARAM_DRIVEN_POOL", false) {
            return;
        }
        let Some(param_name) = pool_scaling_parameter(dice_core) else {
            return;
        };
        let Some(params) = RuntimeParameterService::new(self.db.clone())
            .load_actor_parameters(&c.session_id, &c.initiator.actor_id)
            .await
            .ok()
            .flatten()
        else {
            return;
        };
        let Some(rating) = numeric_param_from_profile(&params.mechanical_profile, &param_name)
        else {
            return;
        };
        if let Some(expr) = param_driven_pool_expression(dice_core, &c.dice_expression, rating) {
            c.dice_expression = expr;
        }
    }

    /// After the kernel defaults run, bind a source-backed static target from the
    /// loaded module's `technical_option_table` when the check is still unbound.
    /// This is the GM `roll_check` counterpart of the combat path's tech-DV
    /// binding: a technical action that hits a module DV row resolves against that
    /// source-backed DV instead of stalling at `awaiting_binding`. No module / no
    /// matching row → unchanged (fail-closed; the check still blocks/awaits).
    async fn bind_source_backed_module_target(&self, c: &mut CheckContract) {
        if !matches!(c.target, CheckTargetModel::UnknownUntilLookup) {
            return;
        }
        let Some(module_id) = c.module_id.clone() else {
            return;
        };
        let Some(cfg) = self.db.load_module_config(&module_id).await else {
            return;
        };
        bind_tech_option_target(c, &cfg);
    }

    pub async fn execute_agent_roll(
        &self,
        session_id: &str,
        turn_id: &str,
        contract: &CheckContract,
    ) -> Result<CheckResultRecord> {
        let mut normalized = normalize_contract_for_system_roll(contract);
        self.apply_kernel_defaults_if_unsourced(&mut normalized)
            .await;
        self.bind_source_backed_module_target(&mut normalized).await;
        if Self::contract_missing_source_backed_parameters(&normalized) {
            let blocked = self.blocked_missing_source_check_result(
                session_id,
                turn_id,
                &normalized,
                "source-backed mechanical parameters are missing",
            );
            self.db.insert_check_result(&blocked).await.ok();
            self.close_resolved_check_gate(session_id, &contract.check_id)
                .await;
            return Ok(blocked);
        }
        let roll = self
            .resolve_roll_input(
                session_id,
                turn_id,
                Some(&normalized.check_id),
                &normalized,
                &normalized.dice_expression,
            )
            .await?;
        let outcome = self
            .resolve_outcome_with_opposition(&normalized, &roll)
            .await?;
        let mut result = CheckResultRecord {
            check_id: normalized.check_id.clone(),
            roll,
            outcome,
            committed_patches: vec![],
            created_at: chrono::Utc::now(),
        };
        let _ = self.post_check_resolution(&normalized, &mut result).await;
        self.db.insert_check_result(&result).await?;
        self.close_resolved_check_gate(session_id, &contract.check_id)
            .await;
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
        self.apply_kernel_defaults_if_unsourced(&mut normalized)
            .await;
        self.bind_source_backed_module_target(&mut normalized).await;
        if Self::contract_missing_source_backed_parameters(&normalized) {
            let blocked = self.blocked_missing_source_check_result(
                session_id,
                turn_id,
                &normalized,
                "source-backed mechanical parameters are missing",
            );
            self.db.insert_check_result(&blocked).await.ok();
            self.close_resolved_check_gate(session_id, &contract.check_id)
                .await;
            return Ok(AutoRollExecution {
                primary: blocked,
                followups: vec![],
                roll_policy: "blocked_missing_source".into(),
            });
        }
        let primary = self
            .execute_agent_roll(session_id, turn_id, &normalized)
            .await?;
        let mut followups = Vec::new();

        // Attack resolution may create an effect/damage PendingCheck. Under
        // product mode this is another tool call, not a second player prompt.
        if let Some(pending) = self.db.get_open_pending_check(session_id).await? {
            if pending.check_id != normalized.check_id
                && is_effect_or_damage_contract(&pending.contract)
            {
                let effect_contract = normalize_contract_for_system_roll(&pending.contract);
                if Self::contract_missing_source_backed_parameters(&effect_contract) {
                    let blocked = self.blocked_missing_source_check_result(
                        session_id,
                        turn_id,
                        &effect_contract,
                        "source-backed effect/damage parameters are missing",
                    );
                    self.db.insert_check_result(&blocked).await.ok();
                    let _ = self
                        .db
                        .update_pending_check_status(
                            &pending.check_id,
                            PendingCheckStatus::Resolved,
                        )
                        .await;
                    followups.push(blocked);
                    return Ok(AutoRollExecution {
                        primary,
                        followups,
                        roll_policy: "blocked_missing_source".into(),
                    });
                }
                let effect_result = self
                    .execute_agent_roll(session_id, turn_id, &effect_contract)
                    .await?;
                let _ = self
                    .db
                    .update_pending_check_status(&pending.check_id, PendingCheckStatus::Resolved)
                    .await;
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
        if !Self::fail_on_missing_source_backed_parameters()
            && !Self::graceful_degrade_missing_source_backed_parameters()
        {
            return false;
        }
        let intent = contract.intent_kind.to_ascii_lowercase();
        let label = contract.check_label.to_ascii_lowercase();
        let action = contract.action_summary.to_ascii_lowercase();
        let parameter_sensitive = [
            "attack",
            "counterattack",
            "defend",
            "dodge",
            "damage",
            "hack",
            "disable",
            "shoot",
            "fire",
            "开火",
            "攻击",
            "射击",
            "黑入",
            "切断",
        ]
        .iter()
        .any(|needle| intent.contains(needle) || label.contains(needle) || action.contains(needle));
        if !parameter_sensitive {
            return false;
        }
        let missing_target = matches!(&contract.target, CheckTargetModel::UnknownUntilLookup);
        let missing_opposition = matches!(
            &contract.opposition,
            OppositionModel::NoMechanicalOpposition
        );
        let no_sources = contract.source_refs.is_empty() && contract.learned_packet_ids.is_empty();
        let bare_die = ["1d10", "d20", "1d20", "2d6", "d100", "1d100", "6d4"]
            .contains(&contract.dice_expression.trim());
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
            if wants_system_roll(input)
                || trimmed.eq_ignore_ascii_case("/roll")
                || trimmed.eq_ignore_ascii_case("roll")
            {
                ParsedRollText::DiceExpression(contract.dice_expression.clone())
            } else {
                ParsedRollText::DiceExpression(
                    trimmed
                        .strip_prefix("/roll ")
                        .unwrap_or(trimmed)
                        .trim()
                        .to_string(),
                )
            }
        });
        let parsed = match parsed {
            ParsedRollText::ReportedTotal(_) | ParsedRollText::ReportedDieAndComponents { .. }
                if !player_reported_roll_totals_allowed() =>
            {
                return Err(anyhow!("player-reported roll totals are disabled; reply `roll` to let the system dice tool roll for this check"));
            }
            ParsedRollText::DiceExpression(expr) if player_supplied_roll_expressions_allowed() => {
                ParsedRollText::DiceExpression(expr)
            }
            ParsedRollText::DiceExpression(_) => {
                ParsedRollText::DiceExpression(contract.dice_expression.clone())
            }
            other => other,
        };
        let expression_for_record = parsed.expression_for_record();
        let result_json = match parsed {
            ParsedRollText::ReportedTotal(total) => {
                json!({"mode":"reported_total", "total": total})
            }
            ParsedRollText::ReportedDieAndComponents {
                die,
                components,
                total,
            } => {
                json!({"mode":"reported_components", "die": die, "components": components, "total": total})
            }
            ParsedRollText::DiceExpression(expr) => {
                // P6.6 同种子掷骰（flag OFF==baseline）：ON 时用 (session:turn:check:roller:expr)
                // 派生 seed **掷骰前** 计算（只含输入、EXCLUDE result_json ⇒ 重试/回放稳定），
                // 走 roll_dice_seeded；OFF 时不变——既有 thread_rng 路径（字节等价 baseline）。
                let rolled = if seeded_rolls_enabled() {
                    let seed = stable_u64(&format!(
                        "{}:{}:{}:{}:{}",
                        session_id,
                        turn_id,
                        check_id.unwrap_or("none"),
                        contract.initiator.actor_id,
                        expr
                    ));
                    roll_dice_seeded(&expr, seed)?
                } else {
                    roll_dice(&expr)?
                };
                json!({"mode":"rolled", "expression": rolled.expression, "rolls": rolled.rolls, "modifier": rolled.modifier, "total": rolled.total})
            }
        };
        let seed_material = format!(
            "{}:{}:{}:{}",
            session_id,
            turn_id,
            check_id.unwrap_or("none"),
            serde_json::to_string(&result_json)?
        );
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
            revealed_at: if contract.disclosure.show_roll_to_player {
                Some(chrono::Utc::now())
            } else {
                None
            },
            created_at: chrono::Utc::now(),
        };
        let mut roll_plan = make_roll_plan_from_check(
            session_id,
            turn_id,
            check_id,
            contract,
            &expression_for_record,
            &record.result,
        );
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
    async fn resolve_outcome_with_opposition(
        &self,
        contract: &CheckContract,
        roll: &DiceRollRecord,
    ) -> Result<serde_json::Value> {
        let svc = ContestService::new(self.db.clone());
        // 先把（如对抗则）防御方骰子算好，再把两路径汇到单点 dispatch。
        let def_record: Option<DiceRollRecord> = if contract_is_opposed(contract) {
            let def_expr = self
                .db
                .load_rule_kernel(&contract.ruleset_id)
                .await
                .ok()
                .flatten()
                .and_then(|k| {
                    k.dice_core
                        .get("dice")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| contract.dice_expression.clone());
            // P6.6 同种子（防御方 :defender: salt）：ON 时掷骰前派生 seed；OFF 不变。
            let rolled = if seeded_rolls_enabled() {
                let seed = stable_u64(&format!(
                    "{}:{}:defender:{}:{}",
                    contract.session_id, contract.turn_id, contract.check_id, def_expr
                ));
                roll_dice_seeded(&def_expr, seed)?
            } else {
                roll_dice(&def_expr)?
            };
            let seed_material = format!(
                "{}:{}:defender:{}",
                contract.session_id, contract.turn_id, contract.check_id
            );
            let rec = DiceRollRecord {
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
            self.db.insert_dice_roll(&rec).await.ok(); // 落库供复算(失败不阻断)
            Some(rec)
        } else {
            None
        };
        self.resolve_check_dispatch(&svc, contract, roll, def_record.as_ref())
            .await
    }

    /// 检定结算单点分发。binding takeover **关**（默认）→ 直调权威 `resolve_outcome`（零行为变更）。
    /// **开** → 从 kernel 派生检定 BindingPlan：Exact-可绑定则经 capability executor 执行
    /// （内部仍调 resolve_outcome → 逐字节等价）；非 Exact → fail-closed 回退原路径 + warn。
    /// 这是 BindingResolver/ExecutionTier 第一次真正驱动结算（非影子），见 [`crate::binding_exec`]。
    async fn resolve_check_dispatch(
        &self,
        svc: &ContestService,
        contract: &CheckContract,
        roll: &DiceRollRecord,
        defender_roll: Option<&DiceRollRecord>,
    ) -> Result<serde_json::Value> {
        if binding_exec::binding_takeover_enabled() {
            if let Ok(Some(kernel)) = self.db.load_rule_kernel(&contract.ruleset_id).await {
                let plan = binding_exec::check_binding_plan(&kernel);
                if binding_exec::plan_authorizes_check_exec(&plan) {
                    return binding_exec::execute_check_with_binding(
                        svc,
                        contract,
                        roll,
                        defender_roll,
                        &plan,
                    )
                    .await;
                }
                tracing::warn!(
                    ruleset = %contract.ruleset_id,
                    verdict = ?plan.verdict,
                    tier = ?plan.execution_tier,
                    "binding takeover: 检定非 Exact-可绑定，fail-closed 回退权威 resolve"
                );
            }
        }
        svc.resolve_outcome(contract, roll, defender_roll).await
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
        let lookups = self
            .db
            .list_lookup_events_for_demand(Some(session_id), &demand_id, 8)
            .await
            .unwrap_or_default();
        if lookups.is_empty() {
            result.warnings.push("no_lookup_events_for_turn".into());
        }
        let mut mechanical = detect_mechanical_signal(user_input, assistant_output);
        let check_contracts = self
            .db
            .list_check_contracts_for_turn(session_id, turn_id)
            .await
            .unwrap_or_default();
        let effect_contracts = self
            .db
            .list_effect_contracts_for_turn(session_id, turn_id)
            .await
            .unwrap_or_default();
        if !effect_contracts.is_empty()
            && !mechanical
                .mentioned_terms
                .iter()
                .any(|t| t == "effect_contract")
        {
            mechanical.has_signal = true;
            mechanical.mentioned_terms.push("effect_contract".into());
        }
        if let Some((kind, value)) = check_contracts.iter().find_map(check_target_signal) {
            mechanical.has_signal = true;
            mechanical.has_specific_target = true;
            mechanical.target_kind = Some(kind);
            mechanical.target_value = Some(value);
            if !mechanical
                .mentioned_terms
                .iter()
                .any(|t| t == "check_contract")
            {
                mechanical.mentioned_terms.push("check_contract".into());
            }
        }
        let source_refs = extract_source_refs_from_lookup_events(&lookups);
        let hit_titles = top_search_hit_titles(&lookups, 4);
        let has_source_evidence = !source_refs.is_empty()
            || lookups
                .iter()
                .any(|e| e.result_status == "source_backed" || e.result_status == "hit");
        if !mechanical.has_signal && !has_source_evidence {
            result
                .warnings
                .push("no_mechanical_or_source_signal".into());
            let _ = self
                .db
                .insert_learning_audit_run(
                    &audit_run_id,
                    Some(session_id),
                    Some(turn_id),
                    Some(ruleset_id),
                    module_id,
                    "done",
                    input_json,
                    serde_json::to_value(&result)?,
                    None,
                )
                .await;
            return Ok(result);
        }

        let ruling_status = if has_source_evidence {
            RulingStatus::SourceBacked
        } else {
            RulingStatus::Provisional
        };
        let confidence = if has_source_evidence && mechanical.has_specific_target {
            RulingConfidence::Medium
        } else {
            RulingConfidence::Low
        };
        let ruling = RulingLogEntry {
            ruling_id: format!("ruling_{}", Uuid::new_v4().simple()),
            session_id: Some(session_id.to_string()),
            ruleset_id: Some(ruleset_id.to_string()),
            module_id: module_id.map(str::to_string),
            demand_id: Some(demand_id.clone()),
            ruling_text: make_ruling_summary(
                user_input,
                assistant_output,
                &mechanical,
                &hit_titles,
            ),
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
            if source_refs.is_empty() {
                risk_flags.push("no_source_refs_in_hit".to_string());
            }
            if !mechanical.has_specific_target {
                risk_flags.push("no_specific_difficulty_or_target".to_string());
            }
            if !has_source_evidence {
                risk_flags.push("provisional_ruling".to_string());
            }
            let evidence_score = evidence_score(&mechanical, &source_refs, &lookups, &risk_flags);
            if evidence_score < 0.50 {
                risk_flags.push("low_evidence_score".to_string());
            }
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
                summary: make_learning_candidate_summary(
                    user_input,
                    assistant_output,
                    &mechanical,
                    &hit_titles,
                ),
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
                verifier_notes: Some(
                    "Auto-generated by learning_audit; approve only after source/ruling review."
                        .into(),
                ),
                created_at: now,
                updated_at: now,
            };
            self.db.insert_learning_candidate(&candidate).await?;
            if learning_auto_promote_enabled()
                && candidate.evidence_score >= 0.75
                && !candidate
                    .risk_flags
                    .iter()
                    .any(|f| f == "low_evidence_score" || f == "provisional_ruling")
            {
                let packet = candidate.to_learned_packet(LearningStage::UsedOnce);
                self.db.upsert_learned_packet(&packet).await?;
                self.db
                    .update_learning_candidate_status(
                        &candidate.candidate_id,
                        LearningCandidateStatus::AutoPromoted,
                        Some("Auto-promoted by TRPG_LEARNING_AUTO_PROMOTE=true"),
                    )
                    .await?;
                result.promoted_packets.push(packet);
            }
            result.candidates.push(candidate);
        }

        let _ = self
            .db
            .insert_learning_audit_run(
                &audit_run_id,
                Some(session_id),
                Some(turn_id),
                Some(ruleset_id),
                module_id,
                "done",
                input_json,
                serde_json::to_value(&result)?,
                None,
            )
            .await;
        Ok(result)
    }

    /// R2: the ONLY query-driven rule-retrieval path — Need bus → RuleNeedResolver →
    /// the rule steward's `assist`. Subsumes the (now-removed) legacy auto_search +
    /// learned_packet pair: `assist` already does learned-packet matching + Tantivy
    /// search + locator/rg fallback, and additionally grounds results with source_refs.
    /// Short-circuit semantics: no search service / empty input / not rule-sensitive
    /// → no blocks.
    async fn rule_need_blocks_for_turn(
        &self,
        request: &ContextRequest,
        state: &RuntimeState,
        input: &str,
        need_trace: &mut Vec<NeedResolutionTrace>,
    ) -> Result<Vec<ContextBlock>> {
        use crate::need_resolvers::{runtime_steward_data_dir, RuleNeedResolver};
        use trpg_need::{Need, NeedBus};

        // Ops escape hatch (restored from pre-R2 `auto_search_blocks_for_turn`):
        // TRPG_RUNTIME_AUTO_SEARCH=0/false → no in-turn rule retrieval at all.
        // Gate BEFORE building the bus / calling the resolver.
        if !runtime_auto_search_enabled() {
            return Ok(vec![]);
        }

        let Some(search) = &self.search else {
            return Ok(vec![]);
        };
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Ok(vec![]);
        }
        // P0-3: rule/module sensitivity is decided by GENERIC TRPG vocabulary plus
        // the CURRENT module's declared entity terms (harvested from module DATA),
        // not a keyword list with module nouns baked into the engine. None module
        // → generic baseline only.
        let module_terms = self
            .module_entity_terms(request.module_id.as_deref().or(state.module_id.as_deref()))
            .await;
        if !looks_rule_or_module_sensitive(trimmed, &module_terms) {
            return Ok(vec![]);
        }

        let mut bus = NeedBus::new();
        bus.register(Box::new(RuleNeedResolver::new(
            self.db.clone(),
            search.clone(),
            runtime_steward_data_dir(),
        )));
        bus.emit(Need::Rule(build_rule_need_for_turn(
            &request.ruleset_id,
            request.module_id.as_deref().or(state.module_id.as_deref()),
            &request.session_id,
            &request.turn_id,
            state.scene_id.as_deref(),
            trimmed,
        )));
        let outcomes = bus.resolve_all().await;
        let mut blocks = Vec::new();
        for outcome in outcomes {
            merge_need_outcome(
                &mut blocks,
                need_trace,
                outcome,
                "rule",
                "turn context assembly",
            );
        }
        Ok(blocks)
    }

    /// P0-3: harvest the CURRENT module's declared entity terms from module DATA
    /// (npc-binding / tech-option matchers + scene-alias strings). These replace
    /// the module-specific nouns (the old hardcoded `athena`/`drone`/`cable`) in
    /// the rule-sensitivity gate, so module entity recognition is data-driven
    /// rather than baked into the engine. None module / no config → empty.
    async fn module_entity_terms(&self, module_id: Option<&str>) -> Vec<String> {
        let Some(mid) = module_id else {
            return Vec::new();
        };
        let Some(cfg) = self.db.load_module_config(mid).await else {
            return Vec::new();
        };
        let mut terms = Vec::new();
        for binding in &cfg.npc_actor_bindings {
            for m in &binding.matcher {
                push_lower_term(&mut terms, m);
            }
        }
        if let Some(table) = &cfg.technical_option_table {
            for opt in table {
                for m in &opt.matcher {
                    push_lower_term(&mut terms, m);
                }
            }
        }
        for alias in &cfg.scene_entity_aliases {
            push_lower_term(&mut terms, &alias.canonical_id);
            for a in &alias.aliases {
                push_lower_term(&mut terms, a);
            }
        }
        terms
    }

    async fn memory_blocks_for_turn(
        &self,
        request: &ContextRequest,
        state: &RuntimeState,
        current_input: Option<&str>,
    ) -> Result<Vec<ContextBlock>> {
        let mut blocks = Vec::new();
        for snapshot in self
            .db
            .list_memory_snapshots(&request.session_id, 3)
            .await?
        {
            if project_visibility(memory_snapshot_block(&snapshot), &request.viewer).is_some() {
                blocks.push(memory_snapshot_block(&snapshot));
            }
        }
        let query_text = current_input.unwrap_or_default().trim().to_string();
        let memory_query = MemoryQuery {
            session_id: request.session_id.clone(),
            text: query_text,
            ruleset_id: Some(request.ruleset_id.clone()),
            module_id: request
                .module_id
                .clone()
                .or_else(|| state.module_id.clone()),
            scene_id: state.scene_id.clone(),
            location_id: state.location_id.clone(),
            actor_ids: state.active_npc_ids.clone(),
            tags: vec![],
            limit: std::env::var("TRPG_MEMORY_RETRIEVAL_LIMIT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(8),
            viewer: request.viewer.clone(),
            // M3: the per-turn memory set feeds the Adjudicator-facing context; it is allowed every
            // layer EXCEPT Story (decision #5). The guard (kill-switch, default ON) enforces this on
            // the assembled blocks below — empty layers (guard OFF) ⇒ identity ⇒ byte-equal baseline.
            layers: crate::memory_guard::adjudicator_memory_layers(),
        };
        let retrieved = self.db.retrieve_memory(&memory_query).await?;
        if !retrieved.facts.is_empty() || !retrieved.events.is_empty() {
            blocks.push(retrieved_memory_block(
                &request.session_id,
                &request.turn_id,
                &retrieved,
            ));
        }
        // M3 decision #5: fail-closed strip of any Story/Director-layer block before the Adjudicator
        // can read it. No-op on the real corpus (seam emits only Mechanical kinds) ⇒ byte-equal.
        Ok(crate::memory_guard::guard_adjudicator_memory(blocks))
    }

    async fn rule_steward_prefix_blocks_for_turn(
        &self,
        request: &ContextRequest,
    ) -> Result<Vec<ContextBlock>> {
        let enabled = std::env::var("TRPG_RULE_STEWARD_ENABLE_V116")
            .map(|v| {
                !matches!(
                    v.to_ascii_lowercase().as_str(),
                    "0" | "false" | "off" | "no"
                )
            })
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
            block.tags = vec![
                "rule_steward".into(),
                "bp1".into(),
                "active_rule_kernel".into(),
                "source_backed".into(),
            ];
            block.source_refs = kernel.source_refs.clone();
            block.load_reason = Some("active_rule_kernel".into());
            blocks.push(block);
            // B1: compact mechanics-catalog index (one line `id | name | when_to_use`
            // per entry, data-driven tiering past the limit) + passive-modifier lines
            // for the viewer, all in ONE prefix block right under the kernel block.
            // Empty catalog -> no block (older kernels change nothing, fail-closed).
            if !kernel.mechanics_catalog.is_empty() {
                let limit = std::env::var("TRPG_MECHANICS_INDEX_BP1_LIMIT")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(96);
                let mut text = catalog_index_text(&kernel.mechanics_catalog, limit);
                let viewer_actor_id = request.viewer.actor_id.as_deref().unwrap_or("pc.current");
                let pm_lines = self
                    .passive_lines_for_viewer(
                        &request.session_id,
                        viewer_actor_id,
                        &kernel.mechanics_catalog,
                    )
                    .await;
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
                index_block.tags = vec![
                    "rule_steward".into(),
                    "bp1".into(),
                    "mechanics_catalog_index".into(),
                    "source_backed".into(),
                ];
                index_block.load_reason = Some("mechanics_catalog_index".into());
                blocks.push(index_block);
            }
        }
        if let Some(pack) = self
            .db
            .load_character_onboarding_pack(&request.ruleset_id)
            .await?
        {
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
            block.tags = vec![
                "rule_steward".into(),
                "character_onboarding".into(),
                "character_creation".into(),
                "playability".into(),
            ];
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
    async fn passive_lines_for_viewer(
        &self,
        session_id: &str,
        viewer_actor_id: &str,
        entries: &[MechanicEntry],
    ) -> Vec<String> {
        let service = RuntimeParameterService::new(self.db.clone());
        let Ok(Some(params)) = service
            .load_actor_parameters(session_id, viewer_actor_id)
            .await
        else {
            return Vec::new();
        };
        entries
            .iter()
            .filter_map(|e| passive_projection_line(e, &params.sheet_json))
            .collect()
    }

    async fn state_frame_blocks_for_turn(
        &self,
        request: &ContextRequest,
    ) -> Result<Vec<ContextBlock>> {
        let limit = std::env::var("TRPG_STATE_FRAME_ACTIVE_LIMIT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4);
        let frames = self
            .db
            .list_active_state_frames(&request.session_id, limit)
            .await?;
        Ok(frames
            .into_iter()
            .map(|frame| frame.to_context_block(&request.turn_id))
            .collect())
    }

    /// §10.1 LIVE linkage: reload the actor's params, re-derive `recompute=live`
    /// values from CURRENT base stats, and persist if anything changed. Idempotent;
    /// call at the start of every turn so derived values stay current even on
    /// blocked/early-returning turns. No-op when there is no stored chargen_spec.
    pub async fn refresh_actor_live_derived(
        &self,
        session_id: &str,
        actor_id: &str,
    ) -> Result<bool> {
        chargen::refresh_actor_live_derived_db(&self.db, session_id, actor_id).await
    }

    /// Ensure an NPC's parameter is on its card: if absent, run the hybrid ladder
    /// (T1 source > T2 archetype > T3 persona-judge) and write the result. Provisional
    /// (T3) values bypass the strict materialization gate by writing straight to
    /// sheet_json. Returns the value, or None when the gate is off / no LLM / no card.
    pub async fn ensure_npc_parameter(
        &self,
        session_id: &str,
        ruleset_id: &str,
        npc: &npc_synth::NpcPersona,
        bucket: &str,
        param: &str,
        check_context: &str,
    ) -> anyhow::Result<Option<serde_json::Value>> {
        if !npc_synth::persona_synthesis_enabled() {
            return Ok(None);
        }
        let service = trpg_params::RuntimeParameterService::new(self.db.clone());
        // §Phase3 bug-fix: load_actor_parameters返回None时（NPC行尚未在runtime_actor_parameters中）
        // 使用ensure_actor_parameters按需创建行，再继续现搓。否则opposed prepass调用此方法时
        // 由于npc_athena等真实NPC id尚未存在而提前返回Ok(None)、无法合成防御参数。
        let world_tick = self
            .current_world_time(session_id)
            .await
            .map(|t| t.world_tick)
            .unwrap_or_default();
        let mut p = match service
            .load_actor_parameters(session_id, &npc.actor_id)
            .await?
        {
            Some(p) => p,
            None => {
                service
                    .ensure_actor_parameters(
                        session_id,
                        ruleset_id,
                        &npc.actor_id,
                        trpg_model::ActorKind::Npc,
                        world_tick,
                    )
                    .await?
            }
        };
        // per-parameter cache: already on the card?
        if let Some(v) = p.sheet_json.pointer(&format!("/{bucket}/{param}")).cloned() {
            return Ok(Some(v));
        }
        // T1: a source value may already be on the card (e.g. under /source/<param>).
        let source_value = p.sheet_json.pointer(&format!("/source/{param}")).cloned();
        // T2: no archetype-stat source index yet (RecommendedArchetype has fit_tags, not stat params) -> empty -> falls to T3.
        let archetypes: Vec<(String, serde_json::Value)> = Vec::new();
        let llm = match trpg_llm::LlmConfig::from_env()
            .ok()
            .and_then(|cfg| trpg_llm::OpenAiCompatibleClient::new(cfg).ok())
        {
            Some(c) => c,
            None => return Ok(None),
        };
        let synth = npc_synth::resolve_param_tiered(
            source_value,
            &archetypes,
            &llm,
            npc,
            param,
            check_context,
            ruleset_id,
        )
        .await?;
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
        let module_id = request
            .module_id
            .as_deref()
            .or(state.module_id.as_deref())?;
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
        let name = v
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let prose = entity_body_prose(v).unwrap_or("").to_string();
        if name.trim().is_empty() && prose.trim().is_empty() {
            return None;
        }
        Some(npc_synth::NpcPersona {
            actor_id: "npc.opposition".to_string(),
            name,
            prose,
        })
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
        let Some(module_id) = request.module_id.as_deref().or(state.module_id.as_deref()) else {
            return Vec::new();
        };
        let Some(graph) = self.db.load_module_graph(module_id).await.ok().flatten() else {
            return Vec::new();
        };
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
        let Some(node) = node else { return Vec::new() };
        node.referenced_npc_ids
            .iter()
            .filter_map(|npc_id| {
                let v = graph
                    .npcs
                    .iter()
                    .find(|v| v.get("id").and_then(|x| x.as_str()) == Some(npc_id.as_str()))?;
                let name = v
                    .get("name")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let prose = entity_body_prose(v).unwrap_or("").to_string();
                if name.trim().is_empty() && prose.trim().is_empty() {
                    return None;
                }
                // 真实 graph id 作 actor_id（per-NPC 卡，治占位符串台）。
                Some(npc_synth::NpcPersona {
                    actor_id: npc_id.clone(),
                    name,
                    prose,
                })
            })
            .collect()
    }

    /// §Task5/7 Map the typed semantic action kind to the NPC parameter the contest
    /// will need from the opposition. Loads the rule kernel to get compare model:
    /// meet_or_beat → defense DV; roll_under → dodge skill.
    /// Returns `(bucket, param)` or None for kinds that need no NPC param.
    pub async fn check_param_need(
        &self,
        ruleset_id: &str,
        action_kind: &SituationActionKind,
    ) -> Option<(String, String)> {
        let compare = self
            .db
            .load_rule_kernel(ruleset_id)
            .await
            .ok()
            .flatten()
            .and_then(|k| {
                k.dice_core
                    .get("compare")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        map_check_param_need(action_kind, &compare)
    }

    /// §Phase3 §4.1 攻击→防御键映射（对抗预 pass 专用）：预 pass 已语义判定本回合
    /// 是攻击对手，故 action_kind 恒 Attack，只需按 kernel.compare 取该规则集的防御
    /// 键（meet_or_beat→stats.defense / roll_under→skills.dodge）。复用同一份
    /// `map_check_param_need` 数据映射，零 per-ruleset 硬编码。无 kernel / 无映射 → None。
    pub async fn attack_defense_param(&self, ruleset_id: &str) -> Option<(String, String)> {
        self.check_param_need(ruleset_id, &SituationActionKind::Attack)
            .await
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
        &self,
        session_id: &str,
        actor_id: &str,
        bucket: &str,
        id: &str,
        op: &str,
        amount: f64,
        text: Option<&str>,
        kind: Option<&str>,
        category: Option<&str>,
    ) -> Result<bool> {
        let service = RuntimeParameterService::new(self.db.clone());
        let Some(mut p) = service.load_actor_parameters(session_id, actor_id).await? else {
            return Ok(false);
        };
        chargen::apply_track_change_to_sheet(
            &mut p.sheet_json,
            bucket,
            id,
            op,
            amount,
            text,
            kind,
            category,
        )
        .map_err(|e| anyhow!("apply_track_change: {e}"))?;
        chargen::recompute_live_derived(&mut p.sheet_json);
        chargen::refresh_mechanical_profile(&mut p.mechanical_profile, &p.sheet_json);
        service.upsert_actor_parameters(&p).await?;
        Ok(true)
    }

    async fn ability_blocks_for_turn(&self, request: &ContextRequest) -> Result<Vec<ContextBlock>> {
        let world_tick = self
            .current_world_time(&request.session_id)
            .await
            .map(|t| t.world_tick)
            .unwrap_or_default();
        let block = AbilityService::new(self.db.clone(), self.search.clone())
            .ability_context_block(&request.session_id, world_tick)
            .await?;
        Ok(vec![block])
    }

    async fn rule_binding_blocks_for_turn(
        &self,
        request: &ContextRequest,
    ) -> Result<Vec<ContextBlock>> {
        let world_tick = self
            .current_world_time(&request.session_id)
            .await
            .map(|t| t.world_tick)
            .unwrap_or_default();
        let block = SemanticRuleBindingService::from_env(self.db.clone(), self.search.clone())
            .rule_binding_context_block(&request.session_id, world_tick)
            .await?;
        Ok(vec![block])
    }

    async fn player_value_referee_blocks_for_turn(
        &self,
        request: &ContextRequest,
    ) -> Result<Vec<ContextBlock>> {
        let verifications = self
            .db
            .list_recent_player_value_verifications(&request.session_id, 8)
            .await?;
        if verifications.is_empty() {
            return Ok(vec![]);
        }
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
            Scope {
                scope_type: ScopeType::Session,
                scope_id: request.session_id.clone(),
            },
            118,
        );
        block.tags = vec![
            "player_value_referee".into(),
            "rules_first".into(),
            "table_override_policy".into(),
        ];
        block.expires_at_turn = Some(request.turn_id.clone());
        block.load_reason = Some("recent_player_value_verifications".into());
        Ok(vec![block])
    }

    async fn object_blocks_for_turn(&self, request: &ContextRequest) -> Result<Vec<ContextBlock>> {
        let world_tick = self
            .current_world_time(&request.session_id)
            .await
            .map(|t| t.world_tick)
            .unwrap_or_default();
        let block = ObjectService::new(self.db.clone())
            .object_context_block(
                &request.session_id,
                None,
                request.viewer.actor_id.as_deref().unwrap_or("pc.current"),
                world_tick,
            )
            .await?;
        Ok(vec![block])
    }

    async fn world_time_blocks_for_turn(
        &self,
        request: &ContextRequest,
    ) -> Result<Vec<ContextBlock>> {
        let service = WorldTimeService::new(self.db.clone());
        let state = service
            .ensure_session_time(&request.session_id, Some(&request.session_id))
            .await?;
        let watermark = self
            .db
            .get_context_watermark(&request.session_id)
            .await?
            .unwrap_or_default();
        let since_tick = watermark.last_compiled_world_tick;
        let since_seq = watermark.last_compiled_event_seq;
        let events = self
            .db
            .list_world_events_since(
                &request.session_id,
                since_tick,
                since_seq,
                std::env::var("TRPG_WORLD_TIME_CONTEXT_EVENT_LIMIT")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(24),
            )
            .await
            .unwrap_or_default();
        let mut blocks = vec![world_time_block(&state, &request.turn_id)];
        if !events.is_empty() {
            blocks.push(world_events_since_block(
                &request.session_id,
                &request.turn_id,
                since_tick,
                since_seq,
                &events,
            ));
        }
        Ok(blocks)
    }

    pub fn to_prompt_bands(
        compiled: &CompiledContext,
        history: Vec<ChatMessage>,
        user_message: impl Into<String>,
    ) -> PromptBands {
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
            user(format!(
                "[gm]\n[BP2: Pinned Context]\n{}\n[/gm]",
                compiled.pinned_text
            )),
            user(format!(
                "[gm]\n[BP3: Dynamic Context]\n{}\n[/gm]\n\n[Player Input]\n{}",
                compiled.dynamic_text, user_input
            )),
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
    pub fn build(
        &self,
        planned: PlannedContext,
        request: &ContextRequest,
    ) -> Result<CompiledContext> {
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
        let cache_key = format!(
            "{}:{}:{}",
            &visibility_signature[..20.min(visibility_signature.len())],
            &prefix_hash[..20.min(prefix_hash.len())],
            &pinned_hash[..20.min(pinned_hash.len())]
        );
        let token_estimate = prefix
            .iter()
            .chain(pinned.iter())
            .chain(dynamic.iter())
            .map(|b| b.token_estimate.unwrap_or(0))
            .sum();
        let block_version_ids = prefix
            .iter()
            .chain(pinned.iter())
            .chain(dynamic.iter())
            .map(|b| format!("{}@{}", b.block_id, b.version))
            .collect();
        // need_trace 由 prepare_turn_context 在 build 之后填入（ContextBuilder 不做 Need 取数）。
        Ok(CompiledContext {
            prefix_blocks: prefix,
            pinned_blocks: pinned,
            dynamic_blocks: dynamic,
            prefix_text,
            pinned_text,
            dynamic_text,
            prefix_hash,
            pinned_hash,
            dynamic_hash,
            visibility_signature,
            cache_key,
            token_estimate,
            block_version_ids,
            need_trace: Vec::new(),
            // MAT.M7 (D1): ContextBuilder 不做 NPC activation；派生集由 prepare_turn_context
            // 在 build 之后填入（见 `compiled.active_npc_ids = ...`）。此处恒空，不破坏其它
            // 调用方（plan_blocks 测试等）的字节产物。
            active_npc_ids: Vec::new(),
            // Q-MODULE: ContextBuilder 不产 establishing；由 prepare_turn_context 在 build 之后
            // 仅 Enforce 下填入（见 `compiled.scene_establishing = ...`）。此处恒空（OFF 字节等价）。
            scene_establishing: Vec::new(),
            // OA2 (G-3): ContextBuilder 不产 character_context；由 prepare_turn_context 在 build
            // 之后仅 Enforce 下填入（见 `compiled.character_context = ...`）。此处恒空（OFF 字节等价）。
            character_context: Vec::new(),
        })
    }
}

pub fn plan_blocks(
    blocks: Vec<ContextBlock>,
    state: &RuntimeState,
    request: &ContextRequest,
) -> PlannedContext {
    let mut prefix = Vec::new();
    let mut pinned = Vec::new();
    let mut dynamic = Vec::new();
    for mut block in blocks {
        if block.cache_zone == CacheZone::NeverPrompt {
            continue;
        }
        if !scope_matches(&block.scope, state, request)
            && !block.tags.iter().any(|t| t == "resident")
        {
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
    PlannedContext {
        prefix_blocks: prefix,
        pinned_blocks: pinned,
        dynamic_blocks: dynamic,
    }
}

fn scope_matches(scope: &Scope, state: &RuntimeState, request: &ContextRequest) -> bool {
    match scope.scope_type {
        ScopeType::Global => true,
        ScopeType::Ruleset => {
            scope.scope_id == request.ruleset_id || scope.scope_id == state.ruleset_id
        }
        ScopeType::Module => {
            request.module_id.as_deref() == Some(scope.scope_id.as_str())
                || state.module_id.as_deref() == Some(scope.scope_id.as_str())
        }
        ScopeType::Chapter => state.chapter_id.as_deref() == Some(scope.scope_id.as_str()),
        ScopeType::Mission => state.mission_id.as_deref() == Some(scope.scope_id.as_str()),
        ScopeType::Scene => state.scene_id.as_deref() == Some(scope.scope_id.as_str()),
        ScopeType::Location => state.location_id.as_deref() == Some(scope.scope_id.as_str()),
        ScopeType::Npc => state.active_npc_ids.iter().any(|id| id == &scope.scope_id),
        ScopeType::Session => scope.scope_id == request.session_id,
        ScopeType::Turn => scope.scope_id == request.turn_id,
        ScopeType::Material => state
            .pending_material_refs
            .iter()
            .chain(state.active_material_refs.iter())
            .any(|id| id == &scope.scope_id),
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
        b.priority
            .cmp(&a.priority)
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
        out.push_str(&format!(
            "

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
        .map(|v| {
            !matches!(
                v.to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no"
            )
        })
        .unwrap_or(true);
    if !redact || visibility != Visibility::GmOnly {
        return input.to_string();
    }
    let terms =
        std::env::var("TRPG_SECRET_TERM_OVERRIDES").unwrap_or_else(|_| "Athena,Shelob".to_string());
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

/// ContextFilter 应用：从已编译的上下文里**删除** `drop_block_ids` 命中的块，并对**真正
/// 变动过**的 band（prefix/pinned/dynamic）用既有 `render_blocks` + `sha256_hex` 重渲染
/// 文本与 hash——逐字复用首次装配（`ContextBuilder::build`）的同一渲染路径，保证字节级一致。
///
/// 未命中任何删除的 band **不触碰**（text/hash byte-stable），block_version_ids /
/// token_estimate 仅在确有删除时按现存块重算（与 build 同口径派生）。cache_key / visibility_signature
/// 复用首次装配值不变（删块不改 viewer / 前两 band 的语义签名规则——这里按"块集变化即重算
/// 受影响 band hash"的最小语义，cache_key 由调用方在需要时再行处理；本 helper 只保证三 band
/// text/hash/block_version_ids/token_estimate 与"若一开始就没有这些块"完全等价）。
///
/// **保守删**（漏删优于过删）：drop set 为空 → 完全 no-op（零拷贝、byte-stable）。该 helper
/// 是纯函数（无 DB/LLM/env 依赖，redact 行为由 `render_blocks` 内既有逻辑承担），可单测。
pub fn apply_context_filter(compiled: &mut trpg_model::CompiledContext, drop_block_ids: &[String]) {
    if drop_block_ids.is_empty() {
        return;
    }
    let drop: HashSet<&str> = drop_block_ids.iter().map(|s| s.as_str()).collect();
    let mut any_changed = false;

    let prefix_changed = retain_not_dropped(&mut compiled.prefix_blocks, &drop);
    if prefix_changed {
        compiled.prefix_text = render_blocks(&compiled.prefix_blocks);
        compiled.prefix_hash = sha256_hex(&compiled.prefix_text);
        any_changed = true;
    }
    let pinned_changed = retain_not_dropped(&mut compiled.pinned_blocks, &drop);
    if pinned_changed {
        compiled.pinned_text = render_blocks(&compiled.pinned_blocks);
        compiled.pinned_hash = sha256_hex(&compiled.pinned_text);
        any_changed = true;
    }
    let dynamic_changed = retain_not_dropped(&mut compiled.dynamic_blocks, &drop);
    if dynamic_changed {
        compiled.dynamic_text = render_blocks(&compiled.dynamic_blocks);
        compiled.dynamic_hash = sha256_hex(&compiled.dynamic_text);
        any_changed = true;
    }

    if any_changed {
        // 派生汇总按现存块重算（与 ContextBuilder::build 同口径）。
        compiled.token_estimate = compiled
            .prefix_blocks
            .iter()
            .chain(compiled.pinned_blocks.iter())
            .chain(compiled.dynamic_blocks.iter())
            .map(|b| b.token_estimate.unwrap_or(0))
            .sum();
        compiled.block_version_ids = compiled
            .prefix_blocks
            .iter()
            .chain(compiled.pinned_blocks.iter())
            .chain(compiled.dynamic_blocks.iter())
            .map(|b| format!("{}@{}", b.block_id, b.version))
            .collect();
    }
}

/// 从一个 band 删除命中 drop set 的块；返回该 band 是否真有块被删（决定是否重渲染）。
fn retain_not_dropped(blocks: &mut Vec<ContextBlock>, drop: &HashSet<&str>) -> bool {
    let before = blocks.len();
    blocks.retain(|b| !drop.contains(b.block_id.as_str()));
    blocks.len() != before
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
    for term in [
        "dv",
        "dc",
        "tn",
        "检定",
        "difficulty",
        "check",
        "roll",
        "1d10",
        "d10",
        "d100",
        "2d6",
        "stealth",
        "basic tech",
        "combat",
        "attack",
        "damage",
    ] {
        if lower.contains(term) {
            mentioned_terms.push(term.to_string());
        }
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
            let kind = if upper.contains("DV") {
                "DV"
            } else if upper.contains("DC") {
                "DC"
            } else if upper.contains("TN") {
                "TN"
            } else {
                "TARGET"
            };
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
    refs.sort_by(|a, b| {
        format!("{}:{:?}:{:?}", a.source_id, a.page, a.anchor_id)
            .cmp(&format!("{}:{:?}:{:?}", b.source_id, b.page, b.anchor_id))
    });
    refs.dedup_by(|a, b| {
        a.source_id == b.source_id && a.page == b.page && a.anchor_id == b.anchor_id
    });
    refs
}

fn top_search_hit_titles(events: &[LookupEvent], limit: usize) -> Vec<String> {
    let mut titles = Vec::new();
    for event in events {
        if let Some(hits) = event.source_hits.as_array() {
            for hit in hits {
                if let Some(title) = hit.get("title").and_then(|v| v.as_str()) {
                    if !title.trim().is_empty() {
                        titles.push(title.trim().to_string());
                    }
                }
                if titles.len() >= limit {
                    return titles;
                }
            }
        }
    }
    titles
}

fn evidence_score(
    signal: &MechanicalSignal,
    refs: &[SourceRef],
    events: &[LookupEvent],
    risk_flags: &[String],
) -> f32 {
    let mut score: f32 = 0.20;
    if !events.is_empty() {
        score += 0.20;
    }
    if !refs.is_empty() {
        score += 0.25;
    }
    if signal.has_signal {
        score += 0.15;
    }
    if signal.has_specific_target {
        score += 0.15;
    }
    if events
        .iter()
        .any(|e| e.result_status == "source_backed" || e.result_status == "hit")
    {
        score += 0.10;
    }
    score -= (risk_flags.len() as f32) * 0.08;
    score.clamp(0.0, 1.0)
}

fn infer_packet_type(user_input: &str, assistant_output: &str) -> String {
    let lower = format!("{}\n{}", user_input, assistant_output).to_lowercase();
    if lower.contains("combat")
        || lower.contains("attack")
        || lower.contains("damage")
        || lower.contains("战斗")
        || lower.contains("攻击")
    {
        "combat_ruling".into()
    } else if lower.contains("netrun") || lower.contains("hack") || lower.contains("黑") {
        "netrunning_or_hacking".into()
    } else if lower.contains("stealth") || lower.contains("潜行") {
        "skill_check_stealth".into()
    } else if lower.contains("tech")
        || lower.contains("修")
        || lower.contains("线缆")
        || lower.contains("无人机")
    {
        "skill_check_tech".into()
    } else {
        "table_ruling".into()
    }
}

fn safe_packet_key(packet_type: &str, user_input: &str, signal: &MechanicalSignal) -> String {
    let mut parts = Vec::new();
    parts.push(packet_type.trim_matches('_').to_ascii_lowercase());
    if let (Some(kind), Some(value)) = (&signal.target_kind, signal.target_value) {
        parts.push(format!("{}{}", kind.to_ascii_lowercase(), value));
    }
    for word in user_input
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(8)
    {
        let cleaned: String = word
            .to_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        if !cleaned.is_empty() {
            parts.push(cleaned);
        }
    }
    let joined = parts
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    let key: String = joined
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .take(96)
        .collect();
    if key.is_empty() {
        "table_ruling".into()
    } else {
        key.trim_matches('_').to_string()
    }
}

fn learning_candidate_title(user_input: &str, signal: &MechanicalSignal) -> String {
    let mut title = user_input.chars().take(80).collect::<String>();
    if let (Some(kind), Some(value)) = (&signal.target_kind, signal.target_value) {
        title = format!("{} {} — {}", kind, value, title);
    }
    title
}

fn make_ruling_summary(
    user_input: &str,
    assistant_output: &str,
    signal: &MechanicalSignal,
    hit_titles: &[String],
) -> String {
    let target = match (&signal.target_kind, signal.target_value) {
        (Some(k), Some(v)) => format!(" Detected target: {k}{v}."),
        _ => String::new(),
    };
    let evidence = if hit_titles.is_empty() {
        String::new()
    } else {
        format!(" Evidence hits: {}.", hit_titles.join("; "))
    };
    format!(
        "Player action: {}\nRuling excerpt: {}{}{}",
        user_input.chars().take(360).collect::<String>(),
        assistant_output.chars().take(700).collect::<String>(),
        target,
        evidence
    )
}

fn make_learning_candidate_summary(
    user_input: &str,
    assistant_output: &str,
    signal: &MechanicalSignal,
    hit_titles: &[String],
) -> String {
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
        for title in hit_titles {
            summary.push_str(&format!("- {}\n", title));
        }
    }
    summary
}

fn learning_audit_enabled() -> bool {
    std::env::var("TRPG_LEARNING_AUDIT")
        .map(|v| v != "0" && v.to_lowercase() != "false")
        .unwrap_or(true)
}

fn learning_auto_promote_enabled() -> bool {
    std::env::var("TRPG_LEARNING_AUTO_PROMOTE")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(false)
}

fn infer_semantic_choice_option<'a>(
    gate: &'a InteractionGate,
    input: &str,
) -> Option<&'a ActionOption> {
    let lower = input.to_lowercase();
    let is_direction_gate = matches!(gate.gate_kind, GateKind::ChooseActionMode)
        || gate
            .advice_refs
            .iter()
            .any(|r| r.contains("direction_gate") || r.contains("stalemate"))
        || gate.bound_action_summary.contains("direction")
        || gate.prompt_public.contains("改变局面")
        || gate.prompt_public.contains("局势");
    if !is_direction_gate {
        let deescalate_or_end = contains_any_choice(
            &lower,
            &[
                "停火",
                "战斗结束",
                "结束战斗",
                "和解",
                "别打",
                "谈判",
                "投降",
                "ceasefire",
                "end combat",
                "stand down",
                "negotiate",
                "surrender",
            ],
        );
        if deescalate_or_end {
            return gate
                .allowed_options
                .iter()
                .find(|opt| {
                    opt.option_id.contains("take")
                        || opt.option_id.contains("decline")
                        || opt.option_id.contains("dodge")
                })
                .or_else(|| gate.allowed_options.first());
        }
        return None;
    }
    let want_continue = lower.starts_with("/roll")
        || contains_any_choice(
            &lower,
            &[
                "继续",
                "继续攻击",
                "继续开火",
                "持续压制",
                "猛打",
                "正面对抗",
                "keep firing",
                "continue",
                "press",
                "attack",
                "shoot",
                "fire",
            ],
        );
    let want_escape = contains_any_choice(
        &lower,
        &[
            "撤",
            "逃",
            "脱离",
            "离开",
            "撤退",
            "逃跑",
            "withdraw",
            "retreat",
            "escape",
            "disengage",
        ],
    );
    let want_talk = contains_any_choice(
        &lower,
        &[
            "谈",
            "和解",
            "投降",
            "停火",
            "威慑",
            "negotiate",
            "truce",
            "surrender",
            "deescalate",
            "talk",
        ],
    );
    let want_objective = contains_any_choice(
        &lower,
        &[
            "目标",
            "电缆",
            "源头",
            "服务器",
            "推进",
            "处理",
            "objective",
            "cable",
            "server",
            "source",
            "push",
        ],
    );
    let want_change = contains_any_choice(
        &lower,
        &[
            "换",
            "改变",
            "绕",
            "战术",
            "change",
            "different",
            "flank",
            "new tactic",
        ],
    );
    let wanted_ids: &[&str] = if want_continue {
        &["continue_conflict", "continue_pressure"]
    } else if want_escape {
        &["escape_or_disengage", "withdraw_or_chase", "leave_node"]
    } else if want_talk {
        &["deescalate", "new_leverage", "pressure", "accept_or_leave"]
    } else if want_objective {
        &["push_objective", "change_method", "deep_search_cost"]
    } else if want_change {
        &["change_tactic", "change_method"]
    } else {
        &[]
    };
    for id in wanted_ids {
        if let Some(option) = gate.allowed_options.iter().find(|opt| opt.option_id == *id) {
            return Some(option);
        }
    }
    // v1.10.1 safety valve: a direction gate must never wedge the player when
    // they clearly mean to continue pressure/attack. If custom profiles renamed
    // the option id, resolve to the first direction option instead of reprompting.
    if want_continue || want_escape || want_talk || want_objective || want_change {
        return gate.allowed_options.first();
    }
    None
}

fn default_choice_after_reprompt(gate: &InteractionGate) -> Option<&ActionOption> {
    gate.allowed_options
        .iter()
        .find(|opt| opt.is_default)
        .or_else(|| {
            gate.allowed_options.iter().find(|opt| {
                opt.option_id == "continue_conflict" || opt.option_id == "continue_pressure"
            })
        })
        .or_else(|| gate.allowed_options.first())
}

fn contains_any_choice(text: &str, terms: &[&str]) -> bool {
    terms.iter().any(|term| text.contains(term))
}

fn turn_orchestrator_enabled() -> bool {
    std::env::var("TRPG_TURN_ORCHESTRATOR_ENABLE_V17")
        .map(|v| v != "0" && v.to_ascii_lowercase() != "false")
        .unwrap_or(true)
}

fn ability_kernel_enabled() -> bool {
    std::env::var("TRPG_ABILITY_KERNEL_ENABLE_V19")
        .map(|v| v != "0" && v.to_ascii_lowercase() != "false")
        .unwrap_or(true)
}

fn object_kernel_enabled() -> bool {
    std::env::var("TRPG_OBJECT_KERNEL_ENABLE_V16")
        .map(|v| v != "0" && v.to_ascii_lowercase() != "false")
        .unwrap_or(true)
}

fn conflict_agent_v10_enabled() -> bool {
    std::env::var("TRPG_SITUATION_ORCHESTRATOR_ENABLE_V11")
        .or_else(|_| std::env::var("TRPG_CONFLICT_AGENT_ENABLE_V10"))
        .map(|v| v != "0" && v.to_ascii_lowercase() != "false")
        .unwrap_or(true)
}

/// Deterministically map a turn's request/state/input into a `RuleNeed` the bus
/// can route. The turn pipeline (not the GM) builds this so AI cannot bypass the
/// typed retrieval path. `query` and `player_action_summary` both carry the raw
/// player input; the steward's `assist` derives its own query text from them.
pub(crate) fn build_rule_need_for_turn(
    ruleset_id: &str,
    module_id: Option<&str>,
    session_id: &str,
    turn_id: &str,
    scene_id: Option<&str>,
    input: &str,
) -> RuleNeed {
    RuleNeed {
        need_id: format!("turn_need_{}", uuid::Uuid::new_v4().simple()),
        session_id: Some(session_id.to_string()),
        turn_id: Some(turn_id.to_string()),
        ruleset_id: ruleset_id.to_string(),
        module_id: module_id.map(str::to_string),
        scene_id: scene_id.map(str::to_string),
        need_kind: RuleNeedKind::GeneralRuleQuery,
        query: input.to_string(),
        player_action_summary: input.to_string(),
        visibility: Visibility::GmOnly,
        urgency: RuleUrgency::ImmediateTurn,
        allowed_outputs: vec![RuleAssistOutputKind::ContextBlock],
        ..Default::default()
    }
}

/// Ops escape hatch (preserved from the pre-R2 `auto_search_blocks_for_turn`):
/// `TRPG_RUNTIME_AUTO_SEARCH=0`/`=false` disables in-turn rule retrieval entirely.
/// Default (unset / any other value) → enabled. Matches the OLD semantics exactly.
fn runtime_auto_search_enabled() -> bool {
    !std::env::var("TRPG_RUNTIME_AUTO_SEARCH")
        .map(|v| v == "false" || v == "0")
        .unwrap_or(false)
}

/// P0-3: decide rule/module sensitivity WITHOUT hardcoded module entity nouns.
/// The engine baseline is GENERIC TRPG vocabulary (check/roll/attack/…) plus a
/// long-input heuristic. Module-specific entity nouns (proper-noun NPCs, device
/// nouns like the old hardcoded `athena`/`drone`/`cable`) now arrive as
/// `module_terms`, harvested from the CURRENT module's DATA by
/// `module_entity_terms` — never baked into this list.
fn looks_rule_or_module_sensitive(input: &str, module_terms: &[String]) -> bool {
    let lowered = input.to_lowercase();
    const GENERIC_KEYWORDS: &[&str] = &[
        "check",
        "roll",
        "rule",
        "dc",
        "dv",
        "skill",
        "attack",
        "combat",
        "damage",
        "heal",
        "netrun",
        "hack",
        "scene",
        "npc",
        "clue",
        "where",
        "how",
        "检定",
        "判定",
        "规则",
        "技能",
        "攻击",
        "战斗",
        "伤害",
        "治疗",
        "黑客",
        "线索",
        "调查",
        "地点",
        "怎么",
        "能不能",
    ];
    lowered.split_whitespace().count() > 8
        || GENERIC_KEYWORDS.iter().any(|kw| lowered.contains(kw))
        || module_terms
            .iter()
            .any(|t| !t.is_empty() && lowered.contains(t.as_str()))
}

/// Push a trimmed, lowercased, de-duplicated term into the harvest set.
fn push_lower_term(terms: &mut Vec<String>, raw: &str) {
    let t = raw.trim().to_lowercase();
    if !t.is_empty() && !terms.contains(&t) {
        terms.push(t);
    }
}

/// Param-driven dice pool (TRPG_PARAM_DRIVEN_POOL): exercises the ENGINE glue
/// `scale_pool_if_param_driven` end-to-end against a real DB. Proves the Q-2
/// acceptance: OFF → flat 6d4 (byte-identical), ON → a competent vs incompetent
/// agent rolls a DIFFERENT pool. DB-gated (skips when DATABASE_URL is unset).
///
/// NOTE (case b): the live Triangle PC sheet carries `competency` as a CATEGORICAL
/// label ("Investigator"), not a numeric rating — so this test populates a
/// SYNTHETIC numeric `competency_rank` param to prove the mechanism. The remaining
/// data-population gap (compiling a numeric competency rating into chargen) is
/// documented in the handoff.
#[cfg(test)]
mod param_driven_pool_engine_tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    // The flag is process-global env; serialize ON/OFF tests so neither sees the
    // other's env mutation.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    const RULESET: &str = "qb_param_pool_test";

    fn pool_kernel() -> RuleKernel {
        serde_json::from_value(json!({
            "kernel_id": format!("kernel_{}", uuid::Uuid::new_v4().simple()),
            "ruleset_id": RULESET,
            "version": "test",
            "dice_core": {
                "dice": "6d4",
                "compare": "count_faces",
                "target_face": 3,
                "success_threshold": 1,
                // Generic data-driven scaling contract (NO ruleset name in engine):
                "pool_scaling_parameter": "competency_rank",
                "pool_base": 4,
                "pool_per_rank": 1
            }
        }))
        .unwrap()
    }

    fn actor_params(session: &str, actor: &str, rank: i64) -> trpg_params::RuntimeActorParameters {
        let sheet = json!({ "stats": { "competency_rank": rank } });
        chargen::materialize_actor_params(session, RULESET, actor, "tmpl", actor, &sheet, 0)
    }

    fn count_faces_contract(session: &str, actor: &str) -> CheckContract {
        serde_json::from_value(json!({
            "check_id": format!("check_{}", uuid::Uuid::new_v4().simple()),
            "session_id": session, "turn_id": "t", "ruleset_id": RULESET, "module_id": null,
            "initiator": {"actor_id": actor, "actor_kind":"player_character","display_name":null},
            "target_actor": null,
            "opposition": {"kind":"no_mechanical_opposition"},
            "action_summary": "draw on the Agency", "intent_kind": "agent_selected_check",
            "check_label": "Agency pool", "dice_expression": "6d4", "modifiers": [],
            "target": {"kind":"unknown_until_lookup"},
            "tested_parameter": null, "opponent_tested_parameter": null,
            "actor_snapshot_ids": [], "source_refs": [], "learned_packet_ids": [],
            "roll_visibility": "public_gm_roll", "roll_authority": "system",
            "disclosure": {"show_roll_to_player":true,"show_formula_to_player":true,"show_dc_to_player":true,"show_success_failure_to_player":true,"reveal_after_scene":false,"reveal_after_session":false},
            "stakes": {"before_roll_public":"","success_public":"","failure_public":"","critical_public":null,"fumble_public":null,"success_patches_allowed":[],"failure_patches_allowed":[],"irreversible":false},
            "confidence": "medium", "ruling_status": "provisional", "advice_refs": [], "expires_at_turn": null
        })).unwrap()
    }

    async fn setup() -> Option<(Db, String)> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let db = Db::connect(&url).await.ok()?;
        let session = format!("session_qb_pool_{}", uuid::Uuid::new_v4().simple());
        db.upsert_rule_kernel(&pool_kernel()).await.ok()?;
        let svc = RuntimeParameterService::new(db.clone());
        // Incompetent agent (rank 1) and competent agent (rank 5).
        svc.upsert_actor_parameters(&actor_params(&session, "pc.low", 1))
            .await
            .ok()?;
        svc.upsert_actor_parameters(&actor_params(&session, "pc.high", 5))
            .await
            .ok()?;
        Some((db, session))
    }

    #[tokio::test]
    async fn off_keeps_flat_pool_byte_identical() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("TRPG_PARAM_DRIVEN_POOL"); // default OFF
        let Some((db, session)) = setup().await else {
            eprintln!("SKIP: DATABASE_URL unset");
            return;
        };
        let engine = RuntimeEngine::new(db);
        let dc = pool_kernel().dice_core;
        // Even the most competent agent stays at the flat kernel dice when OFF.
        let mut c = count_faces_contract(&session, "pc.high");
        engine.scale_pool_if_param_driven(&mut c, &dc).await;
        assert_eq!(
            c.dice_expression, "6d4",
            "OFF: pool must be byte-identical to the flat kernel dice"
        );
    }

    #[tokio::test]
    async fn on_competent_agent_rolls_larger_pool() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("TRPG_PARAM_DRIVEN_POOL", "1");
        let result = async {
            let Some((db, session)) = setup().await else {
                eprintln!("SKIP: DATABASE_URL unset");
                return None;
            };
            let engine = RuntimeEngine::new(db);
            let dc = pool_kernel().dice_core;

            let mut low = count_faces_contract(&session, "pc.low");
            engine.scale_pool_if_param_driven(&mut low, &dc).await;
            let mut high = count_faces_contract(&session, "pc.high");
            engine.scale_pool_if_param_driven(&mut high, &dc).await;
            Some((low.dice_expression, high.dice_expression))
        }
        .await;
        std::env::remove_var("TRPG_PARAM_DRIVEN_POOL");

        let Some((low, high)) = result else { return };
        // base 4 + 1*rank: rank 1 -> 5d4, rank 5 -> 9d4. Faces preserved.
        assert_eq!(low, "5d4", "incompetent agent (rank 1) -> 5d4");
        assert_eq!(high, "9d4", "competent agent (rank 5) -> 9d4");
        assert_ne!(
            low, high,
            "ON: competent vs incompetent MUST roll a different pool"
        );
    }
}

#[cfg(test)]
mod rule_sensitivity_tests {
    use super::looks_rule_or_module_sensitive;

    // Generic TRPG vocabulary triggers retrieval with NO module data — engine baseline.
    #[test]
    fn generic_keyword_is_sensitive_without_module_terms() {
        assert!(looks_rule_or_module_sensitive("I make an attack", &[]));
        assert!(looks_rule_or_module_sensitive("掷一个检定", &[]));
    }

    // A module-declared entity term drives sensitivity (data-driven recognition).
    #[test]
    fn module_term_drives_sensitivity() {
        let terms = vec!["athena".to_string(), "雅典娜".to_string()];
        assert!(looks_rule_or_module_sensitive(
            "i walk toward athena",
            &terms
        ));
        assert!(looks_rule_or_module_sensitive("走向雅典娜", &terms));
    }

    // De-hardcode proof: the engine NO LONGER recognizes the old hardcoded nouns
    // (athena/drone/cable) on its own — they only matter when the current module
    // declares them as entity terms.
    #[test]
    fn removed_module_nouns_are_not_hardcoded() {
        assert!(!looks_rule_or_module_sensitive("i walk toward athena", &[]));
        assert!(!looks_rule_or_module_sensitive("cut the cable", &[]));
        // ...but with module data they are recognized again:
        assert!(looks_rule_or_module_sensitive(
            "cut the cable",
            &["cable".to_string()]
        ));
    }

    // The long-input heuristic still applies (a wordy turn likely needs grounding).
    #[test]
    fn long_input_is_sensitive() {
        assert!(looks_rule_or_module_sensitive(
            "i quietly move along the wall toward the far door and listen",
            &[]
        ));
    }
}

/// PURE: bind a source-backed static target from the loaded module's
/// `technical_option_table` when the kernel left the check unbound. Mirrors the
/// combat path (`trpg_combat::tech_dv_from_config`, combat lib.rs): a technical
/// action whose label/summary hits a module DV row resolves against that
/// source-backed DV (a real meet-or-beat target + StaticDc opposition) instead
/// of stalling at `awaiting_binding`. Returns true iff a DV was bound.
///
/// Fail-closed: only an `UnknownUntilLookup` target is touched (never overrides a
/// real target), and a no-match action (e.g. a pure perception/observe turn with
/// no module DV row) is left unbound so it still blocks / awaits binding. No
/// invented values — the DV is read verbatim from `technical_option_table`.
pub fn bind_tech_option_target(contract: &mut CheckContract, cfg: &ModuleConfig) -> bool {
    if !matches!(contract.target, CheckTargetModel::UnknownUntilLookup) {
        return false;
    }
    let match_text =
        format!("{} {}", contract.check_label, contract.action_summary).to_ascii_lowercase();
    // Mirrors trpg_combat::policy::tech_dv_from_config (first matcher hit wins),
    // reading the same public `technical_option_table` shape; kept here because
    // that fn is private to trpg-combat.
    let Some(dv) = cfg.technical_option_table.as_ref().and_then(|table| {
        table
            .iter()
            .find(|opt| {
                opt.matcher
                    .iter()
                    .any(|k| match_text.contains(&k.to_ascii_lowercase()))
            })
            .map(|opt| opt.dv)
    }) else {
        return false;
    };
    let label = "source-backed module technical option DV".to_string();
    contract.target = CheckTargetModel::StaticNumber {
        value: dv,
        label: label.clone(),
    };
    contract.opposition = OppositionModel::StaticDc { dc: dv, label };
    contract.source_refs.push(SourceRef {
        source_id: format!("module_config:technical_option_table:dv={dv}"),
        ..Default::default()
    });
    contract.ruling_status = RulingStatus::SourceBacked;
    true
}

#[cfg(test)]
mod tech_option_binding_tests {
    use super::*;
    use serde_json::json;

    fn contract(check_label: &str, action_summary: &str) -> CheckContract {
        serde_json::from_value(json!({
            "check_id": "check_t", "session_id": "s", "turn_id": "t",
            "ruleset_id": "cyberpunk_red", "module_id": "cyberpunk_red.homecoming",
            "initiator": {"actor_id":"pc.current","actor_kind":"player_character","display_name":null},
            "target_actor": null,
            "opposition": {"kind":"no_mechanical_opposition"},
            "action_summary": action_summary, "intent_kind": "agent_selected_check",
            "check_label": check_label, "dice_expression": "1d10", "modifiers": [],
            "target": {"kind":"unknown_until_lookup"},
            "tested_parameter": {"domain":null,"key":"tech","label":"tech"},
            "opponent_tested_parameter": null,
            "actor_snapshot_ids": [], "source_refs": [], "learned_packet_ids": [],
            "roll_visibility": "public_gm_roll", "roll_authority": "system",
            "disclosure": {"show_roll_to_player":true,"show_formula_to_player":true,"show_dc_to_player":true,"show_success_failure_to_player":true,"reveal_after_scene":false,"reveal_after_session":false},
            "stakes": {"before_roll_public":"","success_public":"","failure_public":"","critical_public":null,"fumble_public":null,"success_patches_allowed":[],"failure_patches_allowed":[],"irreversible":false},
            "confidence": "medium", "ruling_status": "provisional", "advice_refs": [], "expires_at_turn": null
        })).unwrap()
    }

    // The real Homecoming technical_option_table: cut/power/cable -> DV 14, hack/server -> DV 12.
    fn homecoming_cfg() -> ModuleConfig {
        serde_json::from_value(json!({
            "technical_option_table": [
                {"matcher": ["basic tech","cut off","power","cable","线缆","切断","供电"], "dv": 14},
                {"matcher": ["hack","interface","net","server","黑入","服务器"], "dv": 12}
            ]
        })).unwrap()
    }

    // THE FIX: a source-backed technical action binds the module DV as a real
    // static target (+ StaticDc) so it resolves instead of awaiting_binding.
    #[test]
    fn technical_action_binds_source_backed_dv() {
        let cfg = homecoming_cfg();
        let mut c = contract(
            "Basic Tech check to cut power to the cable",
            "切断外露电缆的供电",
        );
        assert!(
            bind_tech_option_target(&mut c, &cfg),
            "a matching technical action must bind a DV"
        );
        match &c.target {
            CheckTargetModel::StaticNumber { value, .. } => assert_eq!(
                *value, 14,
                "DV must come verbatim from the table (cut/power/cable -> 14)"
            ),
            other => panic!("expected StaticNumber target, got {other:?}"),
        }
        match &c.opposition {
            OppositionModel::StaticDc { dc, .. } => {
                assert_eq!(*dc, 14, "opposition StaticDc mirrors the bound DV")
            }
            other => panic!("expected StaticDc opposition, got {other:?}"),
        }
        assert!(
            matches!(c.ruling_status, RulingStatus::SourceBacked),
            "a table-bound DV is source-backed"
        );
        assert!(
            c.source_refs
                .iter()
                .any(|r| r.source_id.contains("technical_option_table")),
            "must record the module source ref: {:?}",
            c.source_refs
        );
        // No longer UnknownUntilLookup -> the contest takes the resolved branch (non-null success/degree).
        assert!(!matches!(c.target, CheckTargetModel::UnknownUntilLookup));
    }

    #[test]
    fn hack_action_binds_dv_12() {
        let cfg = homecoming_cfg();
        let mut c = contract("Interface check to hack the Athena server", "黑入服务器");
        assert!(bind_tech_option_target(&mut c, &cfg));
        assert!(
            matches!(c.target, CheckTargetModel::StaticNumber { value: 12, .. }),
            "hack/server -> DV 12: {:?}",
            c.target
        );
    }

    // FAIL-CLOSED: a perception/observe action with no matching DV row stays
    // unbound (the live-gap action), so the journey still fails closed honestly.
    #[test]
    fn perception_action_with_no_source_stays_unbound() {
        let cfg = homecoming_cfg();
        let mut c = contract(
            "Perception check to spot an ambush",
            "我压低声音靠近公寓门口，先仔细观察有没有埋伏",
        );
        assert!(
            !bind_tech_option_target(&mut c, &cfg),
            "no DV row matches a pure perception action"
        );
        assert!(
            matches!(c.target, CheckTargetModel::UnknownUntilLookup),
            "target must stay unbound: {:?}",
            c.target
        );
        assert!(
            c.source_refs.is_empty(),
            "no invented source ref when nothing matched"
        );
    }

    // Never override a target that is already bound (idempotent / no double-stamp).
    #[test]
    fn already_bound_target_is_left_untouched() {
        let cfg = homecoming_cfg();
        let mut c = contract("cut the cable", "cut the cable");
        c.target = CheckTargetModel::StaticNumber {
            value: 99,
            label: "preset".into(),
        };
        assert!(
            !bind_tech_option_target(&mut c, &cfg),
            "an already-bound target must not be re-bound"
        );
        assert!(
            matches!(c.target, CheckTargetModel::StaticNumber { value: 99, .. }),
            "preset target preserved: {:?}",
            c.target
        );
    }
}

#[cfg(test)]
mod b1_kernel_default_target_binding_tests {
    //! §6 exam B1 regression (pure / deterministic, no DB): the kernel-default
    //! target-binding gate must (1) PRESERVE a usable provisional StaticNumber DV
    //! when the kernel offers no replacement (meet_or_beat with NO static
    //! target_number — Cyberpunk RED's GM-set-DV shape), and (2) bind / replace
    //! symmetrically for roll_under, pool, and static-target kernels. Because the
    //! gate is pure (`kernel_default_target_binding`) and both the player-input
    //! path (`resolve_check_with_input`) and the system-roll paths
    //! (`execute_agent_roll` / `execute_system_roll_bundle`) feed the SAME contract
    //! target through the SAME `apply_kernel_defaults_if_unsourced`, asserting the
    //! gate output here proves both paths bind identically (symmetry).
    use super::*;
    use serde_json::json;

    fn prov(value: i32) -> CheckTargetModel {
        CheckTargetModel::StaticNumber {
            value,
            label: "GM suggested provisional DV".into(),
        }
    }

    // —— B1 CORE: a meet_or_beat kernel with NO static target_number (Cyberpunk
    // RED: DV is per-situation, not table-wide) must NOT strand a usable
    // provisional StaticNumber. The gate returns None → caller keeps the DV.
    #[test]
    fn meet_or_beat_without_target_number_preserves_provisional_static() {
        let dice_core = json!({ "dice": "1d10", "compare": "meet_or_beat" });
        let binding = kernel_default_target_binding(&prov(13), &dice_core);
        assert!(
            binding.is_none(),
            "kernel offers no replacement → must KEEP the provisional DV, got {binding:?}"
        );
    }

    // —— SYMMETRY proof: an UnknownUntilLookup target under the SAME no-tnum
    // meet_or_beat kernel stays Unknown (kernel has nothing to bind), so a check
    // that arrives unknown and one that arrives with a usable provisional DV are
    // both handled without inventing a stale roll-high static target.
    #[test]
    fn meet_or_beat_without_target_number_keeps_unknown_unknown() {
        let dice_core = json!({ "dice": "1d10", "compare": "meet_or_beat" });
        let binding =
            kernel_default_target_binding(&CheckTargetModel::UnknownUntilLookup, &dice_core);
        assert!(
            matches!(binding, Some(CheckTargetModel::UnknownUntilLookup)),
            "unknown stays unknown (contest kernel resolves it), got {binding:?}"
        );
    }

    // —— roll_under kernel: the per-actor percentile model resolves an Unknown
    // target from the sheet (kernel_can_replace via compare==roll_under), so a
    // stale roll-HIGH provisional StaticNumber MUST be replaced (cleared to
    // Unknown) to avoid mis-resolving under a roll-under contest.
    #[test]
    fn roll_under_replaces_provisional_static_with_unknown() {
        let dice_core = json!({ "dice": "1d100", "compare": "roll_under" });
        let binding = kernel_default_target_binding(&prov(60), &dice_core);
        assert!(
            matches!(binding, Some(CheckTargetModel::UnknownUntilLookup)),
            "roll_under must clear a stale roll-high static DV to Unknown, got {binding:?}"
        );
    }

    // —— meet_or_beat WITH a static target_number: the kernel yields a concrete
    // StaticNumber, so a provisional DV is replaced by the kernel target.
    #[test]
    fn meet_or_beat_with_target_number_replaces_provisional() {
        let dice_core = json!({ "dice": "1d20", "compare": "meet_or_beat", "target_number": 15 });
        match kernel_default_target_binding(&prov(13), &dice_core) {
            Some(CheckTargetModel::StaticNumber { value, .. }) => assert_eq!(
                value, 15,
                "kernel's static target_number must win over a provisional DV"
            ),
            other => panic!("expected kernel StaticNumber(15), got {other:?}"),
        }
    }

    // —— A non-provisional, already-concrete sourced target is NEVER touched
    // (no "suggested/provisional/until/default" label) — proves the gate only
    // rebinds provisional / unknown targets, so already-bound checks are no-ops.
    #[test]
    fn concrete_nonprovisional_static_is_left_untouched() {
        let dice_core = json!({ "dice": "1d10", "compare": "meet_or_beat", "target_number": 15 });
        let bound = CheckTargetModel::StaticNumber {
            value: 7,
            label: "bound DV".into(),
        };
        let binding = kernel_default_target_binding(&bound, &dice_core);
        assert!(
            binding.is_none(),
            "a concrete sourced DV must be left untouched, got {binding:?}"
        );
    }

    // —— pool kernel (count_faces): yields a concrete DicePoolCount, replacing a
    // provisional static DV (another arm of kernel_can_replace).
    #[test]
    fn count_faces_replaces_provisional_with_pool_count() {
        let dice_core = json!({ "dice": "6d4", "compare": "count_faces", "compare_to": "3" });
        assert!(
            matches!(
                kernel_default_target_binding(&prov(13), &dice_core),
                Some(CheckTargetModel::DicePoolCount { .. })
            ),
            "count_faces must replace provisional DV with a dice-pool target"
        );
    }
}

fn looks_like_gate_help_or_question(input: &str) -> bool {
    let lower = input.to_lowercase();
    let terms = [
        "怎么投",
        "投什么",
        "骰什么",
        "怎么算",
        "不会",
        "不懂",
        "?",
        "？",
        "how",
        "what do i roll",
        "what should i roll",
    ];
    terms.iter().any(|term| lower.contains(term))
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
    // ARCHITECTURE-ANCHOR (层化迁移 §19-#9 同种子确定性目标): `roll` 当前**无 seed 入参**、
    // 默认实现用 thread_rng（见下方 PseudoRandomDiceRoller::roll），故同 (state, action) 两次
    // 调用不可复现。P6 将加 seed 通道实现同种子确定性——改本签名前先看
    // crates/trpg-gm/src/arch_gates_tests.rs 的 #[ignore] seedable 测试（去 ignore 即绿门）。
    fn roll(&self, expression: &str) -> Result<DiceRoll>;

    /// P6.6 同种子掷骰（设计4 §19-#9 同种子确定性）：用 `seed` 播种 `StdRng` 掷同一
    /// 表达式，**同 seed ⇒ 同 rolls 序列 ⇒ 同 total**（可复现 / 可回放）。默认实现用
    /// `StdRng::seed_from_u64`，与无 seed 的 `roll`（thread_rng）并存——OFF 路径不受影响。
    fn roll_seeded(&self, expression: &str, seed: u64) -> Result<DiceRoll> {
        let (n, sides, modifier) = parse_dice_expr(expression)?;
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
        let rolls: Vec<i32> = (0..n).map(|_| rng.gen_range(1..=sides)).collect();
        let total = rolls.iter().sum::<i32>() + modifier;
        Ok(DiceRoll {
            expression: expression.to_string(),
            rolls,
            modifier,
            total,
        })
    }
}

/// 解析 `NdM±K` 骰式为 `(n, sides, modifier)`，含安全边界（与 `roll` 同一契约）。
/// 抽出共享，使 `roll`（thread_rng）与 `roll_seeded`（StdRng）只差 RNG、不差解析。
fn parse_dice_expr(expression: &str) -> Result<(i32, i32, i32)> {
    let re = Regex::new(r"(?i)^\s*(\d*)d(\d+)([+-]\d+)?\s*$").unwrap();
    let caps = re
        .captures(expression)
        .ok_or_else(|| anyhow!("unsupported dice expression: {expression}"))?;
    let n = caps.get(1).map(|m| m.as_str()).unwrap_or("1");
    let n: i32 = if n.is_empty() { 1 } else { n.parse()? };
    let sides: i32 = caps.get(2).unwrap().as_str().parse()?;
    let modifier: i32 = caps
        .get(3)
        .map(|m| m.as_str().parse())
        .transpose()?
        .unwrap_or(0);
    if n <= 0 || n > 100 || sides <= 1 || sides > 1000 {
        return Err(anyhow!(
            "dice expression outside safety bounds: {expression}"
        ));
    }
    Ok((n, sides, modifier))
}

/// P6.6 稳定 seed：取 sha256(s) 前 8 字节按 little-endian 组 u64。纯输入派生 ⇒ 同串同
/// seed，跨进程/跨机稳定（不用 DefaultHasher，那是 per-process 随机化的）。
///
/// 复用 `trpg_model::sha256_hex`（返回 `"sha256:<64 hex>"`）解出前 16 hex（= 前 8 字节）
/// 再按 little-endian 组 u64——避免给 trpg-runtime 加 sha2 直依赖（零新 dep 边）。
pub fn stable_u64(s: &str) -> u64 {
    let hex = trpg_model::sha256_hex(s);
    // 去掉 "sha256:" 前缀，取前 16 个 hex 字符 = 摘要前 8 字节。
    let hex = hex.strip_prefix("sha256:").unwrap_or(&hex);
    let mut bytes = [0u8; 8];
    for (i, b) in bytes.iter_mut().enumerate() {
        let lo = i * 2;
        *b = u8::from_str_radix(&hex[lo..lo + 2], 16).unwrap_or(0);
    }
    u64::from_le_bytes(bytes)
}

/// P6.6 同种子掷骰自由函数（镜像 `roll_dice`，走 `PseudoRandomDiceRoller::roll_seeded`）。
pub fn roll_dice_seeded(expression: &str, seed: u64) -> Result<DiceRoll> {
    PseudoRandomDiceRoller.roll_seeded(expression, seed)
}

/// P6.6 OFF==baseline 闸门：`TRPG_SEEDED_ROLLS` 为 truthy（`1`/`true`）时启用同种子
/// 掷骰路径；默认（unset/其它）OFF ⇒ 保持既有 thread_rng 路径、与 baseline 字节一致。
fn seeded_rolls_enabled() -> bool {
    std::env::var("TRPG_SEEDED_ROLLS")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(false)
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PseudoRandomDiceRoller;

impl DiceRollerPlugin for PseudoRandomDiceRoller {
    fn plugin_id(&self) -> &'static str {
        "pseudo_random_v1"
    }

    fn roll(&self, expression: &str) -> Result<DiceRoll> {
        let (n, sides, modifier) = parse_dice_expr(expression)?;
        let mut rng = rand::thread_rng();
        let rolls: Vec<i32> = (0..n).map(|_| rng.gen_range(1..=sides)).collect();
        let total = rolls.iter().sum::<i32>() + modifier;
        Ok(DiceRoll {
            expression: expression.to_string(),
            rolls,
            modifier,
            total,
        })
    }
}

pub fn roll_dice_with_plugin(plugin: &dyn DiceRollerPlugin, expression: &str) -> Result<DiceRoll> {
    plugin.roll(expression)
}

pub fn roll_dice(expression: &str) -> Result<DiceRoll> {
    PseudoRandomDiceRoller.roll(expression)
}

pub fn validate_character_template_sheet(
    template: &CharacterTemplate,
    sheet: &serde_json::Value,
) -> ValidationReport {
    let mut report = ValidationReport {
        status: "ok".to_string(),
        ..Default::default()
    };
    let obj = match sheet.as_object() {
        Some(o) => o,
        None => {
            report.status = "error".to_string();
            report.errors.push(ValidationMessage {
                code: "sheet_not_object".into(),
                message: "Character sheet must be a JSON object.".into(),
                target: None,
            });
            return report;
        }
    };
    for field in &template.fields {
        if field.required && !obj.contains_key(&field.field_id) {
            report.status = "error".to_string();
            report.errors.push(ValidationMessage {
                code: "missing_required_field".into(),
                message: format!("Missing required field: {}", field.field_id),
                target: Some(field.field_id.clone()),
            });
        }
    }
    report
}

fn is_effect_or_damage_contract(contract: &CheckContract) -> bool {
    let text = format!("{} {}", contract.intent_kind, contract.check_label).to_ascii_lowercase();
    text.contains("damage")
        || text.contains("effect")
        || text.contains("伤害")
        || text.contains("san")
        || text.contains("chaos")
        || text.contains("harm")
}

fn roll_input_available(input: &str) -> bool {
    match parse_roll_text(input) {
        Some(ParsedRollText::DiceExpression(_)) => true,
        Some(
            ParsedRollText::ReportedTotal(_) | ParsedRollText::ReportedDieAndComponents { .. },
        ) => player_reported_roll_totals_allowed(),
        None => wants_system_roll(input),
    }
}

/// 裸 "roll"/"系统投" 类「让系统代掷」回复的同源判定原语（trpg-gm 头部 gate
/// 守卫复用此函数，与 resolve_roll_input 的回退语义保持单一事实源）。
pub fn wants_system_roll(input: &str) -> bool {
    let lower = input.trim().to_ascii_lowercase();
    if lower == "/roll" || lower == "roll" {
        return true;
    }
    let terms = [
        "你来投",
        "你帮我投",
        "系统投",
        "系统掷",
        "代投",
        "帮我掷",
        "gm roll",
        "roll for me",
        "you roll",
        "system roll",
        "auto roll",
    ];
    terms.iter().any(|term| lower.contains(term))
}

// 桌面骰权政策谓词的单一事实源在 trpg_model::table_dice_policy;此处再导出
// 以保持 trpg_runtime::system_rolls_visible_policy() 既有公共路径(trpg-gm 在用)。
pub use trpg_model::system_rolls_visible_policy;

/// B1: decide how a kernel's parsed core mechanic rebinds a check's target when
/// the check is NOT already source-backed. Returns `Some(new_target)` when the
/// target should be replaced, or `None` to keep the existing target untouched.
///
/// Pure / data-driven (keys only on the kernel's typed `compare`/`target_number`
/// and the current target's provisional label — never on a ruleset_id/module_id
/// name; constitution §二-⑪). Extracted so the gate is deterministically unit
/// testable without a live DB.
///
/// The kernel can replace the target only when it (a) yields a concrete typed
/// target model from dice_core (count_faces pool / meet_or_beat WITH a static
/// target_number), or (b) is roll_under — whose per-actor percentile model
/// resolves an UnknownUntilLookup target directly from the actor sheet (no static
/// number needed). For a meet_or_beat kernel that declares NO static
/// target_number (e.g. GM-set-DV systems like Cyberpunk RED where the DV is
/// per-situation, not table-wide), the kernel offers NOTHING, so clearing a
/// usable provisional StaticNumber would only strand the check at `provisional`.
/// In that case keep the existing target.
fn kernel_default_target_binding(
    current: &CheckTargetModel,
    dice_core: &serde_json::Value,
) -> Option<CheckTargetModel> {
    let already_unknown = matches!(current, CheckTargetModel::UnknownUntilLookup);
    let provisional_static = matches!(current, CheckTargetModel::StaticNumber { label, .. } if {
        let l = label.to_ascii_lowercase();
        l.contains("suggested") || l.contains("provisional") || l.contains("until") || l.contains("default")
    });
    let kernel_target = target_model_from_dice_core(dice_core);
    let compare = dice_core.get("compare").and_then(|v| v.as_str());
    let kernel_can_replace = kernel_target.is_some() || compare == Some("roll_under");
    if already_unknown {
        // Unknown carries no usable target to preserve: adopt whatever the kernel
        // offers (Some → concrete model; None → stay Unknown for the contest
        // kernel / percentile path). Unchanged from prior behavior.
        Some(kernel_target.unwrap_or(CheckTargetModel::UnknownUntilLookup))
    } else if provisional_static && kernel_can_replace {
        // A stale roll-high StaticNumber would mis-resolve under a roll_under /
        // pool kernel — replace it with the kernel-derived model.
        Some(kernel_target.unwrap_or(CheckTargetModel::UnknownUntilLookup))
    } else {
        // Keep the usable provisional StaticNumber (kernel has no replacement),
        // or keep any already-concrete sourced target.
        None
    }
}

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
        .map(|v| {
            matches!(
                v.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on" | "allow" | "allowed"
            )
        })
        .unwrap_or(false)
}

fn player_supplied_roll_expressions_allowed() -> bool {
    std::env::var("TRPG_PLAYER_SUPPLIED_ROLL_EXPRESSIONS")
        .map(|v| {
            matches!(
                v.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on" | "allow" | "allowed"
            )
        })
        .unwrap_or(false)
}

/// Read a NUMERIC rating for a named parameter off an actor's materialized
/// `mechanical_profile`. Searches the standard buckets (stats, skills, fields,
/// resources, derived_values) plus the profile top level, case-insensitively.
/// Accepts a JSON number or a numeric string (e.g. "2"). Returns None when the
/// key is absent or the value is non-numeric (e.g. a categorical competency
/// label like "Investigator") — the pool then stays flat (fail-soft). Generic:
/// the caller supplies the key NAME from the kernel; no hardcoded param names.
fn numeric_param_from_profile(profile: &serde_json::Value, key: &str) -> Option<i64> {
    fn as_num(v: &serde_json::Value) -> Option<i64> {
        v.as_i64()
            .or_else(|| v.as_f64().map(|f| f as i64))
            .or_else(|| v.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
    }
    fn lookup(obj: &serde_json::Value, key: &str) -> Option<i64> {
        let m = obj.as_object()?;
        m.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .and_then(|(_, v)| as_num(v))
    }
    for bucket in ["stats", "skills", "fields", "resources", "derived_values"] {
        if let Some(b) = profile.get(bucket) {
            if let Some(v) = lookup(b, key) {
                return Some(v);
            }
        }
    }
    lookup(profile, key)
}

fn env_bool_runtime(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(default)
}

/// BUG-1 kill-switch (`TRPG_KERNEL_DICECORE_IS_SOURCE`, default ON). When ON, a
/// contract bound to the parsed ruleset kernel's *typed* core mechanic is treated
/// as source-backed even if `kernel.source_refs` is empty. Set to `0/false/off/no`
/// to restore byte-identical legacy behavior (no synthetic kernel source_ref).
fn kernel_dicecore_is_source_enabled() -> bool {
    std::env::var("TRPG_KERNEL_DICECORE_IS_SOURCE")
        .ok()
        .map(|v| !matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no"))
        .unwrap_or(true)
}

/// L-P Q3 opening-scene delivery kill-switch (`TRPG_OPENING_SCENE_DELIVERY`, default ON).
/// OFF (`0/false/off/no`) ⇒ no pre-turn opening surfaced AND the L-G read_aloud first-entry
/// gate reverts to its original turn-count condition ⇒ byte-equal baseline. Pure decision in
/// `opening_delivery_flag_on` (env-free, unit-tested) to avoid env-race in tests.
fn opening_delivery_flag_on(raw: Option<&str>) -> bool {
    !matches!(
        raw.map(|v| v.to_ascii_lowercase()).as_deref(),
        Some("0") | Some("false") | Some("off") | Some("no")
    )
}

fn opening_scene_delivery_enabled() -> bool {
    opening_delivery_flag_on(std::env::var("TRPG_OPENING_SCENE_DELIVERY").ok().as_deref())
}

/// A kernel `dice_core` is *typed* (a real resolution rule, not an empty stub)
/// when it declares both a non-empty `dice` expression AND a non-empty `compare`
/// rule. Generic over rulesets: no ruleset_id / module_id branching.
fn kernel_dice_core_is_typed(dice_core: &serde_json::Value) -> bool {
    let non_empty_str = |key: &str| {
        dice_core
            .get(key)
            .and_then(|v| v.as_str())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
    };
    non_empty_str("dice") && non_empty_str("compare")
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

fn real_materialization_enabled() -> bool {
    std::env::var("TRPG_REAL_MATERIALIZATION_ENABLE_V110")
        .ok()
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(true)
}

// NOTE: module_scene_proj_tests moved to scene_projection.rs

#[cfg(test)]
mod opening_delivery_flag_tests {
    use super::opening_delivery_flag_on;

    #[test]
    fn unset_defaults_on() {
        assert!(opening_delivery_flag_on(None), "未设 ⇒ 默认 ON");
    }

    #[test]
    fn explicit_off_values_disable() {
        for off in ["0", "false", "off", "no", "FALSE", "Off", "NO"] {
            assert!(
                !opening_delivery_flag_on(Some(off)),
                "{off} ⇒ OFF (byte-equal baseline)"
            );
        }
    }

    #[test]
    fn non_off_values_enable() {
        for on in ["1", "true", "on", "yes", "whatever"] {
            assert!(opening_delivery_flag_on(Some(on)), "{on} ⇒ ON (非关值即开)");
        }
    }
}

#[cfg(test)]
mod module_scene_proj_tests_REMOVED_SEE_SCENE_PROJECTION_PLACEHOLDER {
    // Tests moved to scene_projection.rs; this empty placeholder avoids a stale reference.
    // The real tests live in crate::scene_projection.
}

#[cfg(test)]
mod build_rule_need_tests {
    use super::*;

    #[test]
    fn build_rule_need_carries_scopes_and_input() {
        let need = build_rule_need_for_turn(
            "coc",
            Some("blood_highway"),
            "sess1",
            "turn7",
            Some("sc02"),
            "I attack the cultist with my knife",
        );
        assert_eq!(need.ruleset_id, "coc");
        assert_eq!(need.module_id.as_deref(), Some("blood_highway"));
        assert_eq!(need.session_id.as_deref(), Some("sess1"));
        assert_eq!(need.turn_id.as_deref(), Some("turn7"));
        assert_eq!(need.scene_id.as_deref(), Some("sc02"));
        assert_eq!(need.query, "I attack the cultist with my knife");
        assert_eq!(
            need.player_action_summary,
            "I attack the cultist with my knife"
        );
        assert_eq!(need.need_kind, RuleNeedKind::GeneralRuleQuery);
        assert!(matches!(need.urgency, RuleUrgency::ImmediateTurn));
        assert!(matches!(need.visibility, Visibility::GmOnly));
        assert_eq!(
            need.allowed_outputs,
            vec![RuleAssistOutputKind::ContextBlock]
        );
        assert!(
            !need.need_id.is_empty(),
            "need_id must be generated, not empty"
        );
    }

    #[test]
    fn build_rule_need_optional_scopes_default_to_none() {
        let need = build_rule_need_for_turn("orc", None, "s", "t", None, "look around");
        assert!(need.module_id.is_none());
        assert!(need.scene_id.is_none());
    }
}

#[cfg(test)]
mod kernel_dicecore_source_tests {
    use super::*;
    use serde_json::json;

    static KDS_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct EnvGuard {
        prev: Option<String>,
    }
    impl EnvGuard {
        fn set(v: Option<&str>) -> Self {
            let prev = std::env::var("TRPG_KERNEL_DICECORE_IS_SOURCE").ok();
            match v {
                Some(val) => std::env::set_var("TRPG_KERNEL_DICECORE_IS_SOURCE", val),
                None => std::env::remove_var("TRPG_KERNEL_DICECORE_IS_SOURCE"),
            }
            Self { prev }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var("TRPG_KERNEL_DICECORE_IS_SOURCE", v),
                None => std::env::remove_var("TRPG_KERNEL_DICECORE_IS_SOURCE"),
            }
        }
    }

    #[test]
    fn typed_requires_both_dice_and_compare() {
        // cyberpunk_red-shaped kernel: real typed core.
        assert!(kernel_dice_core_is_typed(
            &json!({"dice":"1d10","compare":"meet_or_beat"})
        ));
        // metadata gaps that must NOT qualify as a typed rule.
        assert!(!kernel_dice_core_is_typed(&json!({"dice":"1d10"})));
        assert!(!kernel_dice_core_is_typed(&json!({"compare":"meet_or_beat"})));
        assert!(!kernel_dice_core_is_typed(&json!({"dice":"","compare":"x"})));
        assert!(!kernel_dice_core_is_typed(&json!({})));
        assert!(!kernel_dice_core_is_typed(&serde_json::Value::Null));
    }

    #[test]
    fn enabled_defaults_on_and_off_switch_is_byte_equal_intent() {
        let _lk = KDS_ENV_LOCK.lock().unwrap();
        // default (env absent) → ON.
        let _g = EnvGuard::set(None);
        assert!(kernel_dicecore_is_source_enabled());
        // explicit truthy/empty still ON.
        let _g = EnvGuard::set(Some("1"));
        assert!(kernel_dicecore_is_source_enabled());
        // OFF tokens → legacy byte-equal path (no synth ref).
        for off in ["0", "false", "off", "no", "OFF", "False"] {
            let _g = EnvGuard::set(Some(off));
            assert!(
                !kernel_dicecore_is_source_enabled(),
                "{off} must disable the synth-ref path"
            );
        }
    }
}

#[cfg(test)]
mod runtime_auto_search_gate_tests {
    use super::*;

    // TRPG_RUNTIME_AUTO_SEARCH is process-global env; serialize all read/write
    // tests on this lock and restore the prior value on drop so the default-on
    // assertion is not polluted when tests run in parallel.
    static AUTO_SEARCH_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct AutoSearchEnvGuard {
        prev: Option<String>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }
    impl AutoSearchEnvGuard {
        fn set(value: &str) -> Self {
            let lock = AUTO_SEARCH_ENV_LOCK
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var("TRPG_RUNTIME_AUTO_SEARCH").ok();
            std::env::set_var("TRPG_RUNTIME_AUTO_SEARCH", value);
            Self { prev, _lock: lock }
        }
        fn unset() -> Self {
            let lock = AUTO_SEARCH_ENV_LOCK
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var("TRPG_RUNTIME_AUTO_SEARCH").ok();
            std::env::remove_var("TRPG_RUNTIME_AUTO_SEARCH");
            Self { prev, _lock: lock }
        }
    }
    impl Drop for AutoSearchEnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var("TRPG_RUNTIME_AUTO_SEARCH", v),
                None => std::env::remove_var("TRPG_RUNTIME_AUTO_SEARCH"),
            }
        }
    }

    /// `runtime_auto_search_enabled()` is the sole predicate gating
    /// `rule_need_blocks_for_turn` at its top: when it returns false the method
    /// short-circuits to `Ok(vec![])` BEFORE building the bus / calling the
    /// resolver. Constructing a `RuntimeEngine` here would need a live Postgres
    /// (`Db` wraps a real `PgPool`, no mock), so we assert the gate decision via
    /// this predicate — `false` is exactly equivalent to "returns empty".
    #[test]
    fn auto_search_disabled_when_zero() {
        let _g = AutoSearchEnvGuard::set("0");
        assert!(
            !runtime_auto_search_enabled(),
            "=0 must disable in-turn rule retrieval (empty blocks)"
        );
    }

    #[test]
    fn auto_search_disabled_when_false() {
        let _g = AutoSearchEnvGuard::set("false");
        assert!(
            !runtime_auto_search_enabled(),
            "=false must disable in-turn rule retrieval (empty blocks)"
        );
    }

    #[test]
    fn auto_search_enabled_by_default_when_unset() {
        let _g = AutoSearchEnvGuard::unset();
        assert!(
            runtime_auto_search_enabled(),
            "unset must keep default-ON behavior unchanged"
        );
    }

    #[test]
    fn auto_search_enabled_for_other_values() {
        let _g = AutoSearchEnvGuard::set("1");
        assert!(
            runtime_auto_search_enabled(),
            "any value other than 0/false stays enabled (matches old semantics)"
        );
    }
}

#[cfg(test)]
mod merge_need_outcome_tests {
    use super::*;
    use context_blocks::dynamic_text_block;

    fn sample_source_ref(id: &str) -> SourceRef {
        SourceRef {
            source_id: id.to_string(),
            page: Some(7),
            anchor_id: Some(format!("{id}-anchor")),
            section_path: vec!["Chapter".to_string()],
            char_start: Some(0),
            char_end: Some(10),
            text_hash: Some("sha256:x".to_string()),
            note: None,
        }
    }

    /// P1-4：helper 必须 (a) 把 outcome.blocks 全部 extend 进 blocks，且 (b) 记一条
    /// trace，携带 need_kind / outcome.source_refs / block_count（=注入块数）。
    #[test]
    fn merge_need_outcome_records_trace_and_extends_blocks() {
        let mut blocks: Vec<ContextBlock> = vec![dynamic_text_block(
            "pre.existing",
            BlockKind::RuleEntityLocator,
            "pre",
            "already here",
            vec!["pre"],
        )];
        let mut trace: Vec<NeedResolutionTrace> = Vec::new();

        let outcome = trpg_need::NeedOutcome {
            blocks: vec![
                dynamic_text_block("nb.1", BlockKind::RuleEntityLocator, "b1", "one", vec!["t"]),
                dynamic_text_block("nb.2", BlockKind::RuleEntityLocator, "b2", "two", vec!["t"]),
            ],
            source_refs: vec![sample_source_ref("r1"), sample_source_ref("r2")],
        };

        merge_need_outcome(
            &mut blocks,
            &mut trace,
            outcome,
            "rule",
            "turn context assembly",
        );

        // blocks extended by exactly 2 (pre-existing 1 → 3).
        assert_eq!(blocks.len(), 3);
        // exactly one trace entry with the expected shape.
        assert_eq!(trace.len(), 1);
        let entry = &trace[0];
        assert_eq!(entry.need_kind, "rule");
        assert_eq!(entry.reason, "turn context assembly");
        assert_eq!(entry.block_count, 2);
        assert_eq!(entry.source_refs.len(), 2);
        assert_eq!(entry.source_refs[0].source_id, "r1");
        assert_eq!(entry.source_refs[1].source_id, "r2");
    }
}

#[cfg(test)]
mod apply_context_filter_tests {
    use super::*;

    fn block(id: &str, title: &str, body: &str, zone: CacheZone) -> ContextBlock {
        ContextBlock::new(
            id,
            BlockKind::GmOnboarding,
            title,
            BlockContent::Text(body.to_string()),
            Visibility::GmOnly,
            Stability::SceneStable,
            zone,
            Scope::global(),
            0,
        )
    }

    fn request() -> ContextRequest {
        ContextRequest {
            ruleset_id: "rs".to_string(),
            module_id: None,
            session_id: "s".to_string(),
            turn_id: "t".to_string(),
            viewer: VisibilityProfile::gm(),
            token_budget: TokenBudget::default(),
        }
    }

    /// 经 runtime 真路径装配 CompiledContext，删一个 dynamic band 的块：该块消失，dynamic
    /// text/hash 与"一开始就没有这个块"的重渲染**逐字**一致，prefix band 完全 byte-stable。
    #[test]
    fn apply_context_filter_drops_block_and_rerenders() {
        let req = request();
        let planned = PlannedContext {
            prefix_blocks: vec![block("p.keep", "Prefix", "prefix body", CacheZone::Prefix)],
            pinned_blocks: vec![],
            dynamic_blocks: vec![
                block("d.keep", "Keep", "keep body", CacheZone::DynamicTail),
                block("d.drop", "Drop", "drop body", CacheZone::DynamicTail),
            ],
        };
        let mut compiled = ContextBuilder.build(planned, &req).expect("build compiled");

        // 期望基线：若一开始 dynamic 只含 d.keep，build 出的 text/hash 即应等于过滤后的值。
        let baseline_planned = PlannedContext {
            prefix_blocks: vec![block("p.keep", "Prefix", "prefix body", CacheZone::Prefix)],
            pinned_blocks: vec![],
            dynamic_blocks: vec![block("d.keep", "Keep", "keep body", CacheZone::DynamicTail)],
        };
        let baseline = ContextBuilder
            .build(baseline_planned, &req)
            .expect("build baseline");

        let prefix_text_before = compiled.prefix_text.clone();
        let prefix_hash_before = compiled.prefix_hash.clone();

        apply_context_filter(&mut compiled, &["d.drop".to_string()]);

        // 被删块不在了；保留块仍在。
        let ids: Vec<&str> = compiled
            .dynamic_blocks
            .iter()
            .map(|b| b.block_id.as_str())
            .collect();
        assert_eq!(ids, vec!["d.keep"]);
        // dynamic text/hash 与基线（从未含 d.drop 的装配）逐字一致——证明走的是同一渲染路径。
        assert_eq!(compiled.dynamic_text, baseline.dynamic_text);
        assert_eq!(compiled.dynamic_hash, baseline.dynamic_hash);
        // 未受影响的 prefix band：text/hash byte-stable（未触碰）。
        assert_eq!(compiled.prefix_text, prefix_text_before);
        assert_eq!(compiled.prefix_hash, prefix_hash_before);
        assert_eq!(compiled.prefix_text, baseline.prefix_text);
        // block_version_ids 不再含被删块。
        assert!(!compiled
            .block_version_ids
            .iter()
            .any(|v| v.starts_with("d.drop@")));
        assert!(compiled
            .block_version_ids
            .iter()
            .any(|v| v.starts_with("d.keep@")));
    }

    /// 空 drop set → 完全 no-op（所有 band text/hash byte-stable）。
    #[test]
    fn apply_context_filter_empty_drop_is_noop() {
        let req = request();
        let planned = PlannedContext {
            prefix_blocks: vec![block("p.1", "P", "p", CacheZone::Prefix)],
            pinned_blocks: vec![block("m.1", "M", "m", CacheZone::PinnedMiddle)],
            dynamic_blocks: vec![block("d.1", "D", "d", CacheZone::DynamicTail)],
        };
        let mut compiled = ContextBuilder.build(planned, &req).expect("build compiled");
        let before = compiled.clone();

        apply_context_filter(&mut compiled, &[]);

        assert_eq!(compiled.prefix_text, before.prefix_text);
        assert_eq!(compiled.prefix_hash, before.prefix_hash);
        assert_eq!(compiled.pinned_text, before.pinned_text);
        assert_eq!(compiled.pinned_hash, before.pinned_hash);
        assert_eq!(compiled.dynamic_text, before.dynamic_text);
        assert_eq!(compiled.dynamic_hash, before.dynamic_hash);
        assert_eq!(compiled.block_version_ids, before.block_version_ids);
    }

    /// drop set 命中不存在的 id → 无 band 变动 → byte-stable（漏删优于过删的最小情形）。
    #[test]
    fn apply_context_filter_unknown_id_is_noop() {
        let req = request();
        let planned = PlannedContext {
            prefix_blocks: vec![block("p.1", "P", "p", CacheZone::Prefix)],
            pinned_blocks: vec![],
            dynamic_blocks: vec![block("d.1", "D", "d", CacheZone::DynamicTail)],
        };
        let mut compiled = ContextBuilder.build(planned, &req).expect("build compiled");
        let before = compiled.clone();

        apply_context_filter(&mut compiled, &["nope.not_here".to_string()]);

        assert_eq!(compiled.dynamic_text, before.dynamic_text);
        assert_eq!(compiled.dynamic_hash, before.dynamic_hash);
        assert_eq!(compiled.prefix_text, before.prefix_text);
        assert_eq!(compiled.prefix_hash, before.prefix_hash);
    }
}

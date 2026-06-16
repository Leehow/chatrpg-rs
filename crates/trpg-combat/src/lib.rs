use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use trpg_db::Db;
use trpg_interaction::InteractionLifecycleKernel;
use trpg_model::*;
use trpg_params::RuntimeParameterService;
use trpg_time::WorldTimeService;
use uuid::Uuid;

mod formula;
use formula::{compile_formula, resolve_attack_dv};

mod policy;
use policy::{
    actor_for_combat_input, check_label_for, combat_mode_from_policy,
    investigate_opens_frame_relation, low_confidence_frame_start_ok, overlay_combat_profile,
    qualify_bare_dice, tech_dv_from_config,
};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RulesetCombatProfile {
    pub profile_id: String,
    pub ruleset_id: String,
    #[serde(default)]
    pub applies_to_modes: Vec<String>,
    #[serde(default)]
    pub default_mode: String,
    #[serde(default)]
    pub action_economy: Value,
    #[serde(default)]
    pub initiative: Value,
    #[serde(default)]
    pub reaction_windows: Vec<ReactionAdvice>,
    #[serde(default)]
    pub attack_resolution: Vec<Value>,
    #[serde(default)]
    pub damage_resolution: Vec<Value>,
    #[serde(default)]
    pub hidden_roll_policy: Value,
    #[serde(default)]
    pub player_roll_policy: Value,
    #[serde(default)]
    pub frame_exit_policy: Value,
    #[serde(default)]
    pub stalemate_policy: Value,
    #[serde(default)]
    pub npc_drive_policy: Value,
    #[serde(default)]
    pub frame_retention_policy: RetentionPolicy,
    #[serde(default)]
    pub frame_compaction_policy: CompactionPolicy,
    #[serde(default)]
    pub search_recipes: Vec<Value>,
    #[serde(default)]
    pub source_refs: Vec<SourceRef>,
    /// P0-2: when true, a Low-confidence intent may still start a frame
    /// (replaces `should_start_frame`'s `ruleset_id.contains("triangle")` gate).
    /// Default false == every non-triangle ruleset's old behavior.
    #[serde(default)]
    pub low_confidence_frame_start: bool,
    /// P0-2: when true, an `investigate` declaration (score ≥ 0.42) opens a
    /// frame (replaces `classify_situation_intent`'s triangle branch). Default
    /// false == old non-triangle behavior.
    #[serde(default)]
    pub investigate_opens_frame: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReactionAdvice {
    pub advice_id: String,
    #[serde(default)]
    pub trigger_keywords: Vec<String>,
    #[serde(default)]
    pub gate_kind: GateKind,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub prompt_public: String,
    #[serde(default)]
    pub options: Vec<ActionOption>,
    #[serde(default)]
    pub default_if_unanswered: Option<String>,
    #[serde(default)]
    pub on_unparseable: GateFallbackPolicy,
    #[serde(default)]
    pub on_new_action: GateFallbackPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CombatProfilePack {
    pub profiles: Vec<RulesetCombatProfile>,
}

impl CombatProfilePack {
    pub fn load_dir(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        // P0-2: the binary-embedded advice profiles are the BASELINE (so a clean
        // checkout with no advice dir reproduces the migrated values), and the
        // advice DIR overrides them when present. On this machine the dir has all
        // 5 → dir wins → behavior unchanged.
        let embedded = embedded_profiles();
        if !path.exists() {
            return Ok(Self::from_embedded_or_default(embedded));
        }
        let mut dir_profiles = Vec::new();
        for entry in fs::read_dir(path).with_context(|| format!("failed to read ruleset advice dir {}", path.display()))? {
            let entry = entry?;
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) != Some("json") { continue; }
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or_default();
            if !name.contains("combat") && !name.contains("conflict") && !name.contains("situation") {
                continue;
            }
            let text = fs::read_to_string(&p).with_context(|| format!("failed to read combat profile {}", p.display()))?;
            match serde_json::from_str::<RulesetCombatProfile>(&text) {
                Ok(profile) if !profile.profile_id.is_empty() => dir_profiles.push(profile),
                Ok(_) => tracing::warn!(path = %p.display(), "combat profile missing profile_id; skipped"),
                Err(err) => tracing::warn!(path = %p.display(), error = %err, "failed to parse combat profile; skipped"),
            }
        }
        if dir_profiles.is_empty() {
            return Ok(Self::from_embedded_or_default(embedded));
        }
        // Merge: embedded baseline, dir wins by (ruleset_id, profile_id).
        let mut profiles = embedded;
        for d in dir_profiles {
            match profiles.iter_mut().find(|e| e.ruleset_id == d.ruleset_id && e.profile_id == d.profile_id) {
                Some(slot) => *slot = d,
                None => profiles.push(d),
            }
        }
        Ok(Self { profiles })
    }

    /// Embedded baseline when no dir profile is available; falls through to the
    /// Rust `default_profiles()` only if the embedded set is also empty.
    fn from_embedded_or_default(embedded: Vec<RulesetCombatProfile>) -> Self {
        if embedded.is_empty() { default_profiles() } else { Self { profiles: embedded } }
    }

    pub fn resolve(&self, ruleset_id: &str, mode_hint: Option<CombatMode>) -> RulesetCombatProfile {
        let target_mode = mode_hint.map(|m| m.as_str().to_string());
        self.profiles.iter()
            .find(|p| p.ruleset_id == ruleset_id && target_mode.as_ref().map(|m| p.applies_to_modes.iter().any(|x| x == m)).unwrap_or(true))
            .cloned()
            .or_else(|| self.profiles.iter().find(|p| p.ruleset_id == ruleset_id).cloned())
            .or_else(|| self.profiles.iter().find(|p| p.ruleset_id == "generic").cloned())
            .unwrap_or_else(default_generic_profile)
    }
}

#[derive(Clone)]
pub struct CombatAgent {
    pub db: Db,
    pub profiles: CombatProfilePack,
}

impl CombatAgent {
    pub fn from_env_or_default(db: Db) -> Self {
        let dir = std::env::var("TRPG_RULESET_ADVICE_DIR").unwrap_or_else(|_| "./data/ruleset_advice".to_string());
        let profiles = CombatProfilePack::load_dir(&dir).unwrap_or_else(|err| {
            tracing::warn!(error = %err, "failed to load combat profiles; using defaults");
            default_profiles()
        });
        Self { db, profiles }
    }

    pub async fn handle_turn(&self, input: ConflictTurnInput<'_>) -> Result<ConflictTurnResult> {
        // P0-2: resolve the ruleset's frame-gate flags from data (no ruleset
        // branch). A mode-agnostic profile lookup gives investigate_opens_frame /
        // low_confidence_frame_start; the mode-specific profile is resolved once
        // the mode is known. The kernel (data policy for combat mode) is loaded
        // once and threaded to the classifier/frame gate.
        let base_profile = self.profiles.resolve(input.ruleset_id, None);
        let kernel = self.db.load_rule_kernel(input.ruleset_id).await.ok().flatten();
        if let Some(frame) = self.active_situation_frame(input.session_id).await? {
            let intent = input.semantic_hint.clone()
                .map(|hint| normalize_semantic_hint_for_frame(hint.clone(), true))
                .unwrap_or_else(|| classify_situation_intent(input, Some(&frame), base_profile.investigate_opens_frame));
            return self.handle_active_frame(input, frame, intent).await;
        }

        let intent = input.semantic_hint.clone()
            .map(|hint| normalize_semantic_hint_for_frame(hint.clone(), false))
            .unwrap_or_else(|| classify_situation_intent(input, None, base_profile.investigate_opens_frame));
        if !should_start_frame(base_profile.low_confidence_frame_start, &intent) {
            return Ok(ConflictTurnResult::not_handled());
        }

        let mode = combat_mode_from_policy(kernel.as_ref().and_then(|k| k.combat_mode_policy.as_ref()), &intent);
        let mut profile = self.profiles.resolve(input.ruleset_id, Some(mode));
        // P0-2: when no per-ruleset advice profile matched (resolve fell back to
        // "generic") but the kernel carries a combat_profile, overlay it. This is
        // additive — it only fires where there was previously no ruleset profile,
        // so curated advice files (the live path) are untouched.
        if profile.ruleset_id == "generic" {
            if let Some(kp) = kernel.as_ref().and_then(|k| k.combat_profile.as_ref()) {
                profile = overlay_combat_profile(profile, Some(kp));
            }
        }
        let mut frame = self.create_frame(input, &profile, mode, Some(intent.clone())).await?;
        let event = self.write_frame_event(&frame, input.turn_id, "combat_frame_created", json!({
            "profile_id": profile.profile_id.clone(),
            "mode": mode.as_str(),
            "trigger_input": input.user_input,
            "semantic_intent": intent.clone(),
        })).await?;
        update_combat_working_state_event(&mut frame, &event.event_id);
        self.db.upsert_state_frame(&frame).await?;

        let mut result = ConflictTurnResult::handled();
        result.intent = Some(intent.clone());
        result.profile = Some(profile.clone());
        result.events.push(event);
        result.phases.push("situation_intent_classified".into());
        result.phases.push("combat_frame_created".into());
        result.phases.push("combat_phase_changed".into());

        // Enemy-initiated/scene-framed combat starts with a required reaction window.
        // This makes the threat explicit and gives terminal intents on the following
        // turn a real gate to supersede, while player-initiated attacks still proceed
        // as ordinary frame starts.
        if should_immediately_open_reaction_gate(&intent) {
            if let Some(advice) = profile.reaction_windows.first().cloned() {
                let gate = make_reaction_gate(input, &frame, &advice);
                self.db.insert_interaction_gate(&gate).await.ok();
                InteractionLifecycleKernel::new(self.db.clone()).attach_gate_to_active_frame(input.session_id, &gate.gate_id, Some(&frame.frame_id)).await.ok();
                let gate_event = self.write_frame_event(&frame, input.turn_id, "reaction_window_opened", json!({
                    "gate_id": gate.gate_id.clone(),
                    "required": gate.required,
                    "options": gate.allowed_options.clone(),
                    "advice_id": advice.advice_id,
                    "semantic_intent": intent.clone(),
                    "opened_immediately_on_frame_start": true
                })).await?;
                frame.active_gate_ids.push(gate.gate_id.clone());
                update_combat_working_state_phase(&mut frame, CombatPhase::AwaitingRequiredReaction);
                update_combat_working_state_event(&mut frame, &gate_event.event_id);
                self.db.upsert_state_frame(&frame).await?;
                result.gate = Some(gate);
                result.events.push(gate_event);
                result.phases.push("reaction_window_opened".into());
                result.done_reason = Some("awaiting_required_reaction".into());
            }
        }

        // v1.13.1: player-initiated frame-opening actions also commit their
        // action contract in the same turn.  Previously a clear opener such as
        // "用重型手枪开火" could create a frame but leave no CheckContract to bind
        // `/roll` or auto-roll to, so the entire dice/contest/effect chain stayed
        // idle. Enemy-initiated starts still stop at the required reaction gate.
        if result.gate.is_none() && should_create_action_contract(&intent) {
            let check = self.make_combat_check_contract(input, &frame, &intent).await;
            let effect = make_effect_contract(&check, &frame, &intent);
            self.db.insert_check_contract(&check, "created").await.ok();
            self.db.insert_effect_contract(&effect, input.session_id, input.turn_id).await.ok();
            let action_event = self.write_frame_event(&frame, input.turn_id, "combat_action_declared", json!({
                "input": input.user_input,
                "semantic_intent": intent.clone(),
                "check_id": check.check_id.clone(),
                "effect_id": effect.effect_id.clone(),
                "declared_on_frame_start": true
            })).await?;
            update_combat_working_state_event(&mut frame, &action_event.event_id);
            update_combat_working_state_intent_and_progress(&mut frame, &intent, true);
            update_npc_drive_after_turn(&mut frame, &intent, true);
            self.db.upsert_state_frame(&frame).await?;
            result.events.push(action_event);
            result.check = Some(check);
            result.effect = Some(effect);
            result.phases.push("check_contract_created".into());
            result.phases.push("effect_contract_created".into());
            result.narration_context = Some("A semantic situation frame started and the player's frame-opening action was committed as a CheckContract plus EffectContract in the same turn. Use the contracts as mechanics context; do not ask the player to recreate the attack.".into());
        } else if result.narration_context.is_none() {
            result.narration_context = Some("A semantic situation frame has started. Track only working state in BP3. Enemy-initiated conflict opens a required reaction window immediately; ordinary object/attack intents cannot bypass that reaction.".into());
        }
        result.frame = Some(frame);
        Ok(result)
    }

    async fn make_combat_check_contract(&self, input: ConflictTurnInput<'_>, frame: &StateFrame, intent: &ConflictIntent) -> CheckContract {
        // P0-2: load the kernel once (its check_label_policy + dice_qualification
        // are data homes for the old contains("cyberpunk") branches) and the
        // module config once (npc bindings + tech-DV table).
        let kernel = self.db.load_rule_kernel(input.ruleset_id).await.ok().flatten();
        let module_cfg = match input.module_id {
            Some(mid) => self.db.load_module_config(mid).await,
            None => None,
        };
        let mut check = make_combat_check_contract(
            input,
            frame,
            intent,
            kernel.as_ref().and_then(|k| k.check_label_policy.as_ref()),
            module_cfg.as_ref(),
        );
        // Data-driven from the parsed kernel: core dice + source citation (the
        // target is already UnknownUntilLookup, so the contest kernel reads the
        // typed success model). The formula compiler may still override the dice
        // and bind a source-backed DV afterward.
        let attack_override = matches!(intent.action_kind, SituationActionKind::Attack | SituationActionKind::Counterattack)
            && std::env::var("TRPG_COMBAT_DEFAULT_ATTACK_EXPR").map(|e| !e.trim().is_empty()).unwrap_or(false);
        if let Some(kernel) = kernel.as_ref() {
            if !attack_override {
                if let Some(dice) = kernel.dice_core.get("dice").and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
                    check.dice_expression = dice;
                }
            }
            if check.source_refs.is_empty() && !kernel.source_refs.is_empty() {
                check.source_refs = kernel.source_refs.clone();
                check.ruling_status = RulingStatus::SourceBacked;
            }
        } else if !attack_override {
            check.dice_expression = "1d20".into();
        }
        self.hydrate_combat_check_contract_from_rule_steward(input, intent, &mut check, kernel.as_ref(), module_cfg.as_ref()).await;
        check
    }

    async fn hydrate_combat_check_contract_from_rule_steward(&self, input: ConflictTurnInput<'_>, intent: &ConflictIntent, check: &mut CheckContract, kernel: Option<&RuleKernel>, module_cfg: Option<&ModuleConfig>) {
        let pack = match self.db.load_character_onboarding_pack(input.ruleset_id).await {
            Ok(Some(pack)) => pack,
            _ => return,
        };
        let formulas = &pack.derived_formula_pack.formulas;
        let is_attack = matches!(intent.action_kind, SituationActionKind::Attack | SituationActionKind::Counterattack);
        let is_defense = matches!(intent.action_kind, SituationActionKind::Defend | SituationActionKind::Dodge | SituationActionKind::UnderAttack | SituationActionKind::EnemyInitiatedConflict | SituationActionKind::SceneEntersConflict);
        let is_tech = matches!(intent.action_kind, SituationActionKind::Hack | SituationActionKind::DisableDevice);
        let wanted: &[&str] = if is_attack {
            &["attack", "ranged", "melee", "check.total", "damage", "armor", "sp"]
        } else if is_defense {
            &["defense", "evasion", "dodge", "armor"]
        } else if is_tech {
            &["tech", "interface", "hack", "dv", "check.total"]
        } else {
            &["check.total", "skill", "resolution"]
        };
        let matching_formula_ids = formulas.iter()
            .filter(|f| {
                let hay = format!("{} {} {}", f.field_id, f.formula, f.notes.clone().unwrap_or_default()).to_ascii_lowercase();
                wanted.iter().any(|term| hay.contains(term))
            })
            .map(|f| f.field_id.clone())
            .collect::<Vec<_>>();

        if !matching_formula_ids.is_empty() || !pack.derived_formula_pack.source_refs.is_empty() || !pack.source_refs.is_empty() {
            if check.source_refs.is_empty() {
                check.source_refs = if !pack.derived_formula_pack.source_refs.is_empty() {
                    pack.derived_formula_pack.source_refs.clone()
                } else {
                    pack.source_refs.clone()
                };
            }
            check.learned_packet_ids.push(pack.derived_formula_pack.pack_id.clone());
            check.advice_refs.push("rule_steward.derived_formula_pack.bound_to_combat_check.v1_16_2".into());
            check.advice_refs.extend(matching_formula_ids.iter().map(|id| format!("formula:{id}")));
            // Step-2: compile a real source-backed dice expression from the
            // formula pack + the acting actor's stats/skills, instead of the
            // old hard-coded "1d10+0". roll_dice only accepts one numeric
            // modifier, so compile_formula sums REF+skill into one constant.
            let mut compiled_expr: Option<String> = None;
            if is_attack || is_defense {
                let preferred: &[&str] = if is_attack {
                    &["ranged_attack", "melee_attack", "attack_roll", "attack"]
                } else {
                    &["evasion", "dodge", "defense"]
                };
                let chosen = preferred.iter().find_map(|p| {
                    formulas.iter().find(|f| f.field_id.to_ascii_lowercase().contains(p))
                });
                let actor_id = input.actor_id.unwrap_or("pc.current");
                let actor = RuntimeParameterService::new(self.db.clone())
                    .load_actor_parameters(input.session_id, actor_id)
                    .await
                    .ok()
                    .flatten();
                if let (Some(f), Some(actor)) = (chosen, actor.as_ref()) {
                    // N1 tier guard: provisional_seed formulas are first-play
                    // placeholders — skip exact dice binding to avoid fabricating
                    // actor-specific modifiers. Log as context-only advice instead.
                    if f.is_provisional_seed() {
                        check.advice_refs.push(format!(
                            "formula.provisional_seed_context_only:{}",
                            f.field_id
                        ));
                        check.advice_refs.push(
                            "formula.exact_binding_skipped:source_backed_formula_required".into(),
                        );
                    } else {
                        let stats = actor.mechanical_profile.get("stats").cloned().unwrap_or_else(|| json!({}));
                        let skills = actor.mechanical_profile.get("skills").cloned().unwrap_or_else(|| json!({}));
                        if let Some(c) = compile_formula(&f.field_id, &f.formula, &stats, &skills) {
                            check.dice_expression = c.expression.clone();
                            check.modifiers = c.modifiers.clone();
                            check.advice_refs.push(format!("formula.compiled:{}={}", f.field_id, c.expression));
                            for u in &c.unresolved {
                                check.advice_refs.push(format!("formula.unbound_modifier:{u}"));
                            }
                            compiled_expr = Some(c.expression);
                        }
                    }
                }
                // Defender DV binding (step-2 symmetric half): set a source-backed
                // StaticDc so the contest resolves hit/miss as total >= DV instead
                // of leaving it narrator-decided. Range-aware via the pack's
                // ranged_attack_dv.<bracket> entries; "close" is the narrative
                // default until a range hint is plumbed.
                if is_attack && matches!(check.target, CheckTargetModel::UnknownUntilLookup) {
                    if let Some((dv, label)) = resolve_attack_dv(formulas, "close") {
                        check.target = CheckTargetModel::StaticNumber { value: dv, label: label.clone() };
                        check.opposition = OppositionModel::StaticDc { dc: dv, label };
                        check.advice_refs.push(format!("formula.dv_bound:ranged_attack_dv.close={dv}"));
                    }
                }
            }
            // P0-2: bare-dice degrade is now data-driven. The old branch was
            // cyberpunk-only + "1d10"-only; equivalence is preserved by gating on
            // the kernel actually carrying a dice_qualification (only the
            // cyberpunk override sets it → other rulesets never reach here), with
            // the template ("{dice}+0") supplying the +0 instead of a literal.
            if compiled_expr.is_none()
                && is_attack
                && check.dice_expression.trim() == "1d10"
            {
                if let Some(dq) = kernel.and_then(|k| k.dice_qualification.as_ref()) {
                    if let Some(q) = qualify_bare_dice(Some(dq), check.dice_expression.trim()) {
                        check.dice_expression = q;
                    }
                }
            }
            check.ruling_status = if !check.source_refs.is_empty() { RulingStatus::SourceBacked } else { RulingStatus::Provisional };
            if matches!(check.target, CheckTargetModel::UnknownUntilLookup) && is_tech {
                if let Some(dv) = tech_dv_from_config(module_cfg, input.user_input) {
                    check.target = CheckTargetModel::StaticNumber { value: dv, label: "source-backed module technical option DV".into() };
                    check.opposition = OppositionModel::StaticDc { dc: dv, label: "source-backed module technical option DV".into() };
                }
            }
        }
    }

    async fn active_situation_frame(&self, session_id: &str) -> Result<Option<StateFrame>> {
        let frames = self.db.list_active_state_frames(session_id, 8).await?;
        Ok(frames.into_iter().find(|f| matches!(
            f.frame_kind,
            FrameKind::Combat
                | FrameKind::Chase
                | FrameKind::Netrun
                | FrameKind::SocialConflict
                | FrameKind::Negotiation
                | FrameKind::InvestigationNode
                | FrameKind::Infiltration
                | FrameKind::HazardSequence
                | FrameKind::SideQuest
                | FrameKind::AnomalyEncounter
                | FrameKind::HorrorEncounter
        )))
    }

    async fn handle_active_frame(&self, input: ConflictTurnInput<'_>, mut frame: StateFrame, intent: ConflictIntent) -> Result<ConflictTurnResult> {
        let intent_event = self.write_frame_event(&frame, input.turn_id, "situation_intent_classified", json!({
            "input": input.user_input,
            "intent": intent,
        })).await?;
        update_combat_working_state_event(&mut frame, &intent_event.event_id);
        update_combat_working_state_intent_and_progress(&mut frame, &intent, false);

        if is_exit_relation(intent.relation_to_active_frame) {
            return self.handle_exit_intent(input, frame, intent, intent_event).await;
        }

        if matches!(intent.relation_to_active_frame, FrameRelation::HideToDisengage | FrameRelation::PauseAndObserve) {
            return self.handle_pause_or_direction_gate(input, frame, intent, intent_event).await;
        }

        // Direction/stalemate gates are advisory. They must not preempt a
        // clear in-frame mechanical action such as attack, disarm, damage,
        // or effect follow-up. Open them only when there is no actionable
        // contract to create. This prevents repeated attacks / damage inputs
        // from being swallowed by the anti-loop gate.
        if should_open_stalemate_gate(&frame) && !should_create_action_contract(&intent) {
            return self.open_stalemate_gate(input, frame, intent, intent_event, "no_decisive_progress").await;
        }

        if should_open_reaction_gate(&intent) {
            let profile = self.profiles.resolve(input.ruleset_id, infer_mode_from_frame(&frame));
            if let Some(advice) = profile.reaction_windows.first().cloned() {
                let gate = make_reaction_gate(input, &frame, &advice);
                self.db.insert_interaction_gate(&gate).await.ok();
                InteractionLifecycleKernel::new(self.db.clone()).attach_gate_to_active_frame(input.session_id, &gate.gate_id, Some(&frame.frame_id)).await.ok();
                let event = self.write_frame_event(&frame, input.turn_id, "reaction_window_opened", json!({
                    "gate_id": gate.gate_id.clone(),
                    "required": gate.required,
                    "options": gate.allowed_options.clone(),
                    "advice_id": advice.advice_id,
                    "semantic_intent": intent.clone(),
                })).await?;
                frame.active_gate_ids.push(gate.gate_id.clone());
                update_combat_working_state_phase(&mut frame, CombatPhase::AwaitingRequiredReaction);
                update_combat_working_state_event(&mut frame, &event.event_id);
                self.db.upsert_state_frame(&frame).await?;
                let mut result = ConflictTurnResult::handled();
                result.intent = Some(intent);
                result.frame = Some(frame);
                result.gate = Some(gate);
                result.events.push(intent_event);
                result.events.push(event);
                result.phases.push("situation_intent_classified".into());
                result.phases.push("reaction_window_opened".into());
                result.done_reason = Some("awaiting_required_reaction".into());
                return Ok(result);
            }
        }

        if should_create_action_contract(&intent) {
            let check = self.make_combat_check_contract(input, &frame, &intent).await;
            let effect = make_effect_contract(&check, &frame, &intent);
            self.db.insert_check_contract(&check, "created").await.ok();
            self.db.insert_effect_contract(&effect, input.session_id, input.turn_id).await.ok();
            let event = self.write_frame_event(&frame, input.turn_id, "combat_action_declared", json!({
                "input": input.user_input,
                "semantic_intent": intent.clone(),
                "check_id": check.check_id.clone(),
                "effect_id": effect.effect_id.clone(),
            })).await?;
            update_combat_working_state_event(&mut frame, &event.event_id);
            update_combat_working_state_intent_and_progress(&mut frame, &intent, true);
            update_npc_drive_after_turn(&mut frame, &intent, true);
            let novelty = apply_novelty_director(&mut frame, &intent, input.turn_id, true);
            let novelty_event = if novelty.force_tactic_shift || novelty.fresh_change.is_some() {
                Some(self.write_frame_event(&frame, input.turn_id, "npc_tactic_shift", json!({"novelty": novelty.clone(), "semantic_intent": intent.clone()})).await?)
            } else { None };
            if let Some(event) = &novelty_event { update_combat_working_state_event(&mut frame, &event.event_id); }
            self.db.upsert_state_frame(&frame).await?;
            let mut result = ConflictTurnResult::handled();
            result.intent = Some(intent);
            result.frame = Some(frame);
            result.events.push(intent_event);
            result.events.push(event);
            if let Some(novelty_event) = novelty_event { result.events.push(novelty_event); }
            result.novelty = Some(novelty);
            result.check = Some(check);
            result.effect = Some(effect);
            result.phases.push("situation_intent_classified".into());
            result.phases.push("check_contract_created".into());
            result.phases.push("effect_contract_created".into());
            result.narration_context = Some("The situation action was framed as a CheckContract plus EffectContract. Use the contracts as mechanics context; do not invent unvalidated HP/resource patches.".into());
            return Ok(result);
        }

        if matches!(intent.relation_to_active_frame, FrameRelation::InvalidOrAmbiguous | FrameRelation::Clarification | FrameRelation::OutsideFrameAction) {
            return self.open_stalemate_gate(input, frame, intent, intent_event, "ambiguous_or_outside_active_frame").await;
        }

        let event = self.write_frame_event(&frame, input.turn_id, "situation_frame_observation", json!({
            "input": input.user_input,
            "semantic_intent": intent.clone(),
        })).await?;
        update_combat_working_state_event(&mut frame, &event.event_id);
        update_npc_drive_after_turn(&mut frame, &intent, false);
        let novelty = apply_novelty_director(&mut frame, &intent, input.turn_id, false);
        let novelty_event = if novelty.force_tactic_shift || novelty.fresh_change.is_some() {
            Some(self.write_frame_event(&frame, input.turn_id, "npc_tactic_shift", json!({"novelty": novelty.clone(), "semantic_intent": intent.clone()})).await?)
        } else { None };
        if let Some(event) = &novelty_event { update_combat_working_state_event(&mut frame, &event.event_id); }
        self.db.upsert_state_frame(&frame).await?;
        let mut result = ConflictTurnResult::handled();
        result.intent = Some(intent);
        result.frame = Some(frame);
        result.events.push(intent_event);
        result.events.push(event);
        if let Some(novelty_event) = novelty_event { result.events.push(novelty_event); }
        result.novelty = Some(novelty);
        result.phases.push("situation_intent_classified".into());
        result.narration_context = Some("Continue within the active situation frame, but avoid repeating the same NPC tactic. If no decisive change occurs, open a direction gate rather than looping.".into());
        Ok(result)
    }

    async fn handle_exit_intent(&self, input: ConflictTurnInput<'_>, mut frame: StateFrame, intent: ConflictIntent, intent_event: FrameEvent) -> Result<ConflictTurnResult> {
        let exit = make_exit_contract(input, &frame, &intent);
        let event = self.write_frame_event(&frame, input.turn_id, "exit_contract_created", json!({
            "exit_contract": exit.clone(),
            "semantic_intent": intent.clone(),
        })).await?;
        update_combat_working_state_event(&mut frame, &event.event_id);
        update_combat_working_state_outcome(&mut frame, exit.success_outcome);
        update_npc_drive_after_turn(&mut frame, &intent, true);

        let closes_frame = matches!(
            exit.success_outcome,
            FrameOutcome::PlayerEscaped
                | FrameOutcome::EnemyEscaped
                | FrameOutcome::NegotiatedTruce
                | FrameOutcome::SurrenderAccepted
                | FrameOutcome::ObjectiveCompleted
                | FrameOutcome::Closed
                | FrameOutcome::Abandoned
        );

        let mut result = ConflictTurnResult::handled();
        result.intent = Some(intent.clone());
        result.exit_contract = Some(exit.clone());
        result.events.push(intent_event);
        result.events.push(event);
        result.phases.push("situation_intent_classified".into());
        result.phases.push("exit_contract_created".into());

        if closes_frame {
            frame.status = match exit.success_outcome {
                FrameOutcome::Abandoned => FrameStatus::Abandoned,
                _ => FrameStatus::Completed,
            };
            frame.updated_at = Utc::now();
            self.db.upsert_state_frame(&frame).await?;
            let compaction = self.compact_frame(&frame, &format!("Frame closed by semantic exit contract: {}", exit.success_outcome.as_str())).await?;
            InteractionLifecycleKernel::new(self.db.clone()).close_frame_cascade(&frame.frame_id, &frame.session_id, SupersededReason::ExitContract, exit.success_outcome.as_str()).await.ok();
            result.compaction = Some(compaction);
            result.phases.push("combat_frame_compacted".into());
            result.phases.push("frame_closed".into());
            result.narration_context = Some("The active situation frame has ended or transitioned. Narrate aftermath using durable consequences only; do not continue the old combat loop.".into());
        } else {
            update_combat_working_state_phase(&mut frame, CombatPhase::ExitAttempt);
            self.db.upsert_state_frame(&frame).await?;
            result.narration_context = Some("The player is trying to exit or transform the situation. Resolve the exit contract; do not treat it as a generic attack/action loop.".into());
        }
        result.frame = Some(frame);
        Ok(result)
    }

    async fn handle_pause_or_direction_gate(&self, input: ConflictTurnInput<'_>, mut frame: StateFrame, intent: ConflictIntent, intent_event: FrameEvent) -> Result<ConflictTurnResult> {
        let exit = make_exit_contract(input, &frame, &intent);
        let exit_event = self.write_frame_event(&frame, input.turn_id, "exit_contract_created", json!({
            "exit_contract": exit.clone(),
            "semantic_intent": intent.clone(),
        })).await?;
        update_combat_working_state_event(&mut frame, &exit_event.event_id);
        update_combat_working_state_outcome(&mut frame, FrameOutcome::StalemateNeedsDirection);
        update_combat_working_state_phase(&mut frame, CombatPhase::DirectionGate);
        update_npc_drive_after_turn(&mut frame, &intent, false);

        let stalemate = make_stalemate_contract(&frame, "player_paused_or_hid_without_resolving_frame");
        let gate = make_direction_gate(input, &frame, &stalemate, "局势停住了。你是在继续躲藏观察、尝试脱离、谈判，还是重新采取行动？");
        self.db.insert_interaction_gate(&gate).await.ok();
        InteractionLifecycleKernel::new(self.db.clone()).attach_gate_to_active_frame(input.session_id, &gate.gate_id, Some(&frame.frame_id)).await.ok();
        let gate_event = self.write_frame_event(&frame, input.turn_id, "conflict_direction_gate_opened", json!({
            "gate_id": gate.gate_id.clone(),
            "stalemate_contract": stalemate.clone(),
            "semantic_intent": intent.clone(),
        })).await?;
        frame.active_gate_ids.push(gate.gate_id.clone());
        update_combat_working_state_event(&mut frame, &gate_event.event_id);
        self.db.upsert_state_frame(&frame).await?;

        let mut result = ConflictTurnResult::handled();
        result.intent = Some(intent);
        result.exit_contract = Some(exit);
        result.stalemate_contract = Some(stalemate);
        result.frame = Some(frame);
        result.gate = Some(gate);
        result.events.push(intent_event);
        result.events.push(exit_event);
        result.events.push(gate_event);
        result.phases.push("situation_intent_classified".into());
        result.phases.push("exit_contract_created".into());
        result.phases.push("conflict_direction_gate_opened".into());
        result.done_reason = Some("awaiting_situation_direction".into());
        Ok(result)
    }

    async fn open_stalemate_gate(&self, input: ConflictTurnInput<'_>, mut frame: StateFrame, intent: ConflictIntent, intent_event: FrameEvent, reason: &str) -> Result<ConflictTurnResult> {
        let stalemate = make_stalemate_contract(&frame, reason);
        let gate = make_direction_gate(input, &frame, &stalemate, "局势没有产生明确推进。请选择改变局面的方向，避免循环。 ");
        self.db.insert_interaction_gate(&gate).await.ok();
        InteractionLifecycleKernel::new(self.db.clone()).attach_gate_to_active_frame(input.session_id, &gate.gate_id, Some(&frame.frame_id)).await.ok();
        let event = self.write_frame_event(&frame, input.turn_id, "conflict_direction_gate_opened", json!({
            "gate_id": gate.gate_id.clone(),
            "stalemate_contract": stalemate.clone(),
            "semantic_intent": intent.clone(),
            "reason": reason,
        })).await?;
        frame.active_gate_ids.push(gate.gate_id.clone());
        update_combat_working_state_phase(&mut frame, CombatPhase::DirectionGate);
        update_combat_working_state_outcome(&mut frame, FrameOutcome::StalemateNeedsDirection);
        update_combat_working_state_event(&mut frame, &event.event_id);
        update_npc_drive_after_turn(&mut frame, &intent, false);
        self.db.upsert_state_frame(&frame).await?;

        let mut result = ConflictTurnResult::handled();
        result.intent = Some(intent);
        result.stalemate_contract = Some(stalemate);
        result.frame = Some(frame);
        result.gate = Some(gate);
        result.events.push(intent_event);
        result.events.push(event);
        result.phases.push("situation_intent_classified".into());
        result.phases.push("conflict_direction_gate_opened".into());
        result.done_reason = Some("awaiting_situation_direction".into());
        Ok(result)
    }

    #[allow(clippy::too_many_lines)]
    async fn create_frame(&self, input: ConflictTurnInput<'_>, profile: &RulesetCombatProfile, mode: CombatMode, intent: Option<ConflictIntent>) -> Result<StateFrame> {
        let frame_id = format!("frame_{}", Uuid::new_v4().simple());
        let combat_id = format!("combat_{}", Uuid::new_v4().simple());
        let now = Utc::now();
        let actor_id = input.actor_id.unwrap_or("pc.current").to_string();
        let world_tick = WorldTimeService::new(self.db.clone()).ensure_session_time(input.session_id, Some(input.session_id)).await.map(|t| t.world_tick).unwrap_or_default();
        let params = RuntimeParameterService::new(self.db.clone());
        let pc_params = params.ensure_actor_parameters(input.session_id, input.ruleset_id, &actor_id, ActorKind::PlayerCharacter, world_tick).await.ok();
        let npc_actor_id = "npc.opposition".to_string();
        let npc_params = params.ensure_actor_parameters(input.session_id, input.ruleset_id, &npc_actor_id, ActorKind::Npc, world_tick).await.ok();
        // Participant HP from the single source of truth (live current in
        // generic_parameter_states via the kernel HP track). No gps row yet
        // (combat just started) -> load_resource_current falls back to the
        // character-derived seed (full HP). Fail-closed None when no HP track.
        let kernel = self.db.load_rule_kernel(input.ruleset_id).await.ok().flatten();
        let hp_id = kernel.as_ref().and_then(|k| trpg_model::hp_resource_track_id(&k.resource_tracks));
        let pc_hp = match (kernel.as_ref(), hp_id.as_ref()) { (Some(k), Some(id)) => self.db.load_resource_current(input.session_id, &actor_id, id, k).await, _ => None };
        let npc_hp = match (kernel.as_ref(), hp_id.as_ref()) { (Some(k), Some(id)) => self.db.load_resource_current(input.session_id, &npc_actor_id, id, k).await, _ => None };
        let working = CombatWorkingState {
            combat_id,
            combat_mode: mode,
            round: 1,
            phase: CombatPhase::EstablishingScene,
            active_actor_id: Some(actor_id.clone()),
            initiative_order: vec![
                InitiativeSlot { actor_id: actor_id.clone(), display_name: Some("Current PC".into()), initiative_total: None, acted_this_round: false, metadata: json!({"source":"runtime_parameter_hydrator_v1_8"}) },
                InitiativeSlot { actor_id: npc_actor_id.clone(), display_name: Some("Opposition".into()), initiative_total: None, acted_this_round: false, metadata: json!({"source":"runtime_parameter_hydrator_v1_8"}) },
            ],
            participants: vec![
                CombatParticipantState { actor_id: actor_id.clone(), actor_kind: ActorKind::PlayerCharacter, display_name: Some("Current PC".into()), side_id: "pcs".into(), hp_current: pc_hp, hp_max: pc_hp, visible_to_players: true, combat_status: CombatantStatus::Active, morale: Some(100), patience: Some(100), resources: pc_params.as_ref().map(|p| p.status_json.clone()).unwrap_or_else(|| json!({"source_policy":"unresolved_no_synthetic_default"})), ..Default::default() },
                CombatParticipantState { actor_id: npc_actor_id.clone(), actor_kind: ActorKind::Npc, display_name: Some("Opposition".into()), side_id: "opposition".into(), hp_current: npc_hp, hp_max: npc_hp, visible_to_players: false, combat_status: CombatantStatus::Active, morale: Some(55), patience: Some(45), resources: npc_params.as_ref().map(|p| p.status_json.clone()).unwrap_or_else(|| json!({"source_policy":"unresolved_no_synthetic_default"})), ..Default::default() },
            ],
            sides: vec![
                CombatSide { side_id: "pcs".into(), label: "Player characters".into(), actor_ids: vec![actor_id.clone()], disposition: "allied".into() },
                CombatSide { side_id: "opposition".into(), label: "Opposition".into(), actor_ids: vec![npc_actor_id.clone()], disposition: "hostile_or_uncertain".into() },
            ],
            zones: vec![CombatZone { zone_id: "zone.scene".into(), label: "Current scene".into(), summary: "The active situation location. Refine with module/search hits as needed.".into(), tags: vec!["scene".into()], metadata: json!({}) }],
            range_model: RangeModel { model_id: "range.theater".into(), kind: "theater_of_mind".into(), notes: "Use ruleset profile/search hits for exact distances when needed.".into() },
            action_economy: ActionEconomyState { profile_id: profile.profile_id.clone(), actor_slots: Default::default(), notes: profile.action_economy.to_string() },
            reaction_windows: vec![],
            active_effects: vec![],
            hazards: vec![],
            objectives: default_objectives_for_mode(mode),
            rule_packet_ids: vec![profile.profile_id.clone()],
            encounter_packet_ids: vec![],
            last_events: vec![],
            progress_tracker: FrameProgressTracker { turns_elapsed: 0, turns_since_decisive_change: 0, rounds_since_decisive_change: 0, ..Default::default() },
            npc_drive_states: vec![default_npc_drive_for_mode(mode)],
            npc_tactic_memory: vec![],
            novelty_state: NoveltyState::default(),
            tactic_palettes: vec![default_tactic_palette_for_mode(mode)],
            current_outcome: Some(FrameOutcome::Continue),
            last_intent: intent,
        };
        let frame = StateFrame {
            frame_id,
            frame_kind: frame_kind_for_mode(mode),
            session_id: input.session_id.to_string(),
            ruleset_id: input.ruleset_id.to_string(),
            module_id: input.module_id.map(str::to_string),
            parent_frame_id: None,
            scope: Scope { scope_type: ScopeType::Session, scope_id: input.session_id.to_string() },
            status: FrameStatus::Active,
            title: format!("{} situation frame", input.ruleset_id),
            objective: "Track transient situation state and compact durable consequences after resolution.".into(),
            static_refs: vec![profile.profile_id.clone()],
            working_state: serde_json::to_value(working)?,
            active_gate_ids: vec![],
            local_clocks: vec![],
            local_facts: vec![],
            local_modifiers: vec![],
            event_count: 0,
            last_event_ids: vec![],
            retention_policy: profile.frame_retention_policy.clone(),
            compaction_policy: profile.frame_compaction_policy.clone(),
            created_at: now,
            updated_at: now,
        };
        self.db.upsert_state_frame(&frame).await?;
        InteractionLifecycleKernel::new(self.db.clone()).open_context_for_frame(&frame, InteractionContextKind::Frame).await.ok();
        Ok(frame)
    }

    async fn write_frame_event(&self, frame: &StateFrame, turn_id: &str, kind: &str, event_json: Value) -> Result<FrameEvent> {
        let event = FrameEvent { event_id: format!("frame_event_{}", Uuid::new_v4().simple()), frame_id: frame.frame_id.clone(), session_id: frame.session_id.clone(), turn_id: Some(turn_id.to_string()), event_kind: kind.to_string(), event_json, visibility: Visibility::GmOnly, created_at: Utc::now() };
        self.db.insert_frame_event(&event).await?;
        Ok(event)
    }

    pub async fn compact_frame(&self, frame: &StateFrame, reason: &str) -> Result<FrameCompaction> {
        let events = self.db.list_frame_events(&frame.frame_id, 80).await.unwrap_or_default();
        let mut summary = format!("# Frame Compaction: {}\n\nReason: {}\n\n", frame.title, reason);
        summary.push_str("## Preserved durable consequences\n");
        if frame.local_facts.is_empty() {
            summary.push_str("- No durable facts were explicitly promoted by the frame. Review frame events before archiving if needed.\n");
        } else {
            for fact in &frame.local_facts { summary.push_str(&format!("- {}\n", fact.summary)); }
        }
        if let Ok(working) = serde_json::from_value::<CombatWorkingState>(frame.working_state.clone()) {
            if let Some(outcome) = working.current_outcome {
                summary.push_str(&format!("- Final outcome: {}\n", outcome.as_str()));
            }
            summary.push_str(&format!("- Transient turns elapsed: {}\n", working.progress_tracker.turns_elapsed));
        }
        summary.push_str("\n## Archived transient events\n");
        for event in events.iter().take(12) { summary.push_str(&format!("- {}\n", event.event_kind)); }
        let compaction = FrameCompaction { compaction_id: format!("frame_compaction_{}", Uuid::new_v4().simple()), frame_id: frame.frame_id.clone(), session_id: frame.session_id.clone(), summary_markdown: summary, persistent_world_patches: vec![], promoted_fact_ids: frame.local_facts.iter().map(|f| f.fact_id.clone()).collect(), archived_event_ids: events.iter().map(|e| e.event_id.clone()).collect(), created_at: Utc::now() };
        self.db.insert_frame_compaction(&compaction).await?;
        Ok(compaction)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ConflictTurnInput<'a> {
    pub session_id: &'a str,
    pub turn_id: &'a str,
    pub ruleset_id: &'a str,
    pub module_id: Option<&'a str>,
    pub actor_id: Option<&'a str>,
    pub user_input: &'a str,
    /// Semantic route hint from TurnOrchestrator.  The combat agent treats this
    /// as structured evidence from the LLM/Rust reducer so non-keyword and
    /// multilingual combat declarations do not have to pass a local phrase list.
    pub semantic_hint: Option<&'a ConflictIntent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConflictTurnResult {
    pub handled: bool,
    pub phases: Vec<String>,
    pub frame: Option<StateFrame>,
    pub profile: Option<RulesetCombatProfile>,
    pub events: Vec<FrameEvent>,
    pub gate: Option<InteractionGate>,
    pub check: Option<CheckContract>,
    pub effect: Option<EffectContract>,
    pub compaction: Option<FrameCompaction>,
    pub intent: Option<ConflictIntent>,
    pub exit_contract: Option<ExitContract>,
    pub stalemate_contract: Option<StalemateContract>,
    #[serde(default)]
    pub novelty: Option<NoveltyDecision>,
    pub done_reason: Option<String>,
    pub narration_context: Option<String>,
}

impl ConflictTurnResult {
    pub fn handled() -> Self { Self { handled: true, ..Default::default() } }
    pub fn not_handled() -> Self { Self::default() }
}


fn normalize_semantic_hint_for_frame(mut hint: ConflictIntent, active: bool) -> ConflictIntent {
    let frame_action = matches!(hint.action_kind,
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
    if frame_action && !matches!(hint.relation_to_active_frame,
        FrameRelation::ExitAttempt
            | FrameRelation::DeescalationAttempt
            | FrameRelation::Surrender
            | FrameRelation::Flee
            | FrameRelation::HideToDisengage
            | FrameRelation::GateResponse) {
        hint.relation_to_active_frame = FrameRelation::InsideFrameAction;
    }
    if active && frame_action {
        hint.relation_to_active_frame = FrameRelation::InsideFrameAction;
    }
    if hint.confidence == RulingConfidence::Low && frame_action {
        hint.confidence = RulingConfidence::Medium;
    }
    if !hint.evidence_terms.iter().any(|e| e == "turn_orchestrator_semantic_hint") {
        hint.evidence_terms.push("turn_orchestrator_semantic_hint".into());
    }
    hint
}

fn classify_situation_intent(input: ConflictTurnInput<'_>, active_frame: Option<&StateFrame>, investigate_opens_frame: bool) -> ConflictIntent {
    let text = input.user_input.trim();
    let active = active_frame.is_some();
    let attack = semantic_cluster_score(text, ATTACK_EXAMPLES);
    let defend = semantic_cluster_score(text, DEFENSE_EXAMPLES);
    let under_attack = semantic_cluster_score(text, UNDER_ATTACK_EXAMPLES);
    let scene_conflict = semantic_cluster_score(text, SCENE_CONFLICT_EXAMPLES);
    let flee = semantic_cluster_score(text, FLEE_EXAMPLES);
    let hide = semantic_cluster_score(text, HIDE_DISENGAGE_EXAMPLES);
    let deescalate = semantic_cluster_score(text, DEESCALATE_EXAMPLES);
    let surrender = semantic_cluster_score(text, SURRENDER_EXAMPLES);
    let observe = semantic_cluster_score(text, OBSERVE_EXAMPLES);
    let hack = semantic_cluster_score(text, HACK_EXAMPLES);
    let investigate = semantic_cluster_score(text, INVESTIGATE_EXAMPLES);
    let anomaly = semantic_cluster_score(text, ANOMALY_EXAMPLES);
    let lexical_attack = lexical_any(text, &["拔枪", "还击", "开枪", "朝", "打它", "射击", "攻击", "反击", "shoot", "fire", "attack", "return fire"]);
    let lexical_under_attack = lexical_any(text, &["朝我开火", "对我开火", "向我开火", "攻击我", "堵住我", "under fire", "opens fire", "shoots at me", "attacks me"]);
    let lexical_scene_conflict = lexical_any(text, &["进入战斗", "战斗开始", "冲突爆发", "枪战", "scene enters combat", "combat starts", "fight begins"]);
    let attack = if lexical_attack { attack.max(0.74) } else { attack };
    let under_attack = if lexical_under_attack { under_attack.max(0.74) } else { under_attack };
    let scene_conflict = if lexical_scene_conflict { scene_conflict.max(0.74) } else { scene_conflict };

    let mut scored = vec![
        (SituationActionKind::Attack, attack),
        (SituationActionKind::UnderAttack, under_attack),
        (SituationActionKind::EnemyInitiatedConflict, under_attack.max(scene_conflict)),
        (SituationActionKind::SceneEntersConflict, scene_conflict),
        (SituationActionKind::Defend, defend),
        (SituationActionKind::Flee, flee),
        (SituationActionKind::Hide, hide),
        (SituationActionKind::Negotiate, deescalate),
        (SituationActionKind::Surrender, surrender),
        (SituationActionKind::WaitOrHoldAction, observe),
        (SituationActionKind::Hack, hack),
        (SituationActionKind::InvestigateDuringConflict, investigate),
        (SituationActionKind::DisableDevice, anomaly),
    ];
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let (action, score) = scored.first().copied().unwrap_or((SituationActionKind::Unknown, 0.0));

    let relation = if active {
        if surrender >= 0.48 { FrameRelation::Surrender }
        else if deescalate >= 0.46 { FrameRelation::DeescalationAttempt }
        else if flee >= 0.46 { FrameRelation::Flee }
        else if hide >= 0.46 { FrameRelation::HideToDisengage }
        else if observe >= 0.58 { FrameRelation::PauseAndObserve }
        else if under_attack >= 0.40 || scene_conflict >= 0.40 { FrameRelation::GateResponse }
        else if attack >= 0.42 || defend >= 0.45 || hack >= 0.48 || investigate >= 0.50 || anomaly >= 0.50 { FrameRelation::InsideFrameAction }
        else { FrameRelation::InvalidOrAmbiguous }
    } else {
        if attack >= 0.42 || under_attack >= 0.40 || scene_conflict >= 0.40 || hack >= 0.56 || anomaly >= 0.50 || investigate_opens_frame_relation(investigate_opens_frame, investigate) { FrameRelation::InsideFrameAction }
        else { FrameRelation::OutsideFrameAction }
    };

    let score = score.max(under_attack).max(scene_conflict);
    let confidence = if score >= 0.70 { RulingConfidence::High } else if score >= 0.40 { RulingConfidence::Medium } else { RulingConfidence::Low };
    let mut evidence = Vec::new();
    if attack >= 0.42 { evidence.push(format!("attack:{attack:.2}")); }
    if under_attack >= 0.40 { evidence.push(format!("under_attack:{under_attack:.2}")); }
    if scene_conflict >= 0.40 { evidence.push(format!("scene_conflict:{scene_conflict:.2}")); }
    if flee >= 0.42 { evidence.push(format!("flee:{flee:.2}")); }
    if hide >= 0.42 { evidence.push(format!("hide_disengage:{hide:.2}")); }
    if deescalate >= 0.42 { evidence.push(format!("deescalate:{deescalate:.2}")); }
    if surrender >= 0.42 { evidence.push(format!("surrender:{surrender:.2}")); }
    if hack >= 0.42 { evidence.push(format!("hack:{hack:.2}")); }
    if investigate >= 0.42 { evidence.push(format!("investigate:{investigate:.2}")); }
    if anomaly >= 0.42 { evidence.push(format!("anomaly:{anomaly:.2}")); }

    ConflictIntent {
        intent_id: format!("intent_{}", Uuid::new_v4().simple()),
        language: detect_language_hint(text),
        relation_to_active_frame: relation,
        action_kind: action,
        escalation_level: if attack > 0.6 || under_attack > 0.6 || scene_conflict > 0.6 { EscalationLevel::High } else if attack > 0.42 || under_attack > 0.40 || scene_conflict > 0.40 || hack > 0.55 { EscalationLevel::Medium } else { EscalationLevel::Low },
        target_refs: vec![],
        desired_outcome: infer_desired_outcome(action, relation).map(str::to_string),
        confidence,
        evidence_terms: evidence,
        classifier: "rust_semantic_paraphrase_router_v1_3".into(),
    }
}

fn should_start_frame(low_confidence_frame_start: bool, intent: &ConflictIntent) -> bool {
    matches!(intent.relation_to_active_frame, FrameRelation::InsideFrameAction | FrameRelation::GateResponse)
        && matches!(intent.action_kind,
            SituationActionKind::Attack
                | SituationActionKind::UnderAttack
                | SituationActionKind::EnemyInitiatedConflict
                | SituationActionKind::SceneEntersConflict
                | SituationActionKind::Defend
                | SituationActionKind::Counterattack
                | SituationActionKind::Hack
                | SituationActionKind::DisableDevice
                | SituationActionKind::InvestigateDuringConflict
        )
        && low_confidence_frame_start_ok(low_confidence_frame_start, intent.confidence)
}

fn should_open_reaction_gate(intent: &ConflictIntent) -> bool {
    matches!(intent.action_kind, SituationActionKind::UnderAttack | SituationActionKind::EnemyInitiatedConflict | SituationActionKind::SceneEntersConflict | SituationActionKind::Defend | SituationActionKind::Dodge | SituationActionKind::Counterattack)
}

fn should_immediately_open_reaction_gate(intent: &ConflictIntent) -> bool {
    matches!(intent.action_kind, SituationActionKind::UnderAttack | SituationActionKind::EnemyInitiatedConflict | SituationActionKind::SceneEntersConflict | SituationActionKind::Defend)
}

fn should_create_action_contract(intent: &ConflictIntent) -> bool {
    matches!(intent.relation_to_active_frame, FrameRelation::InsideFrameAction)
        && matches!(intent.action_kind, SituationActionKind::Attack | SituationActionKind::Defend | SituationActionKind::Dodge | SituationActionKind::Counterattack | SituationActionKind::Hack | SituationActionKind::DisableDevice | SituationActionKind::InvestigateDuringConflict | SituationActionKind::UseItem | SituationActionKind::Rescue | SituationActionKind::Intimidate)
}

fn is_exit_relation(relation: FrameRelation) -> bool {
    matches!(relation, FrameRelation::ExitAttempt | FrameRelation::DeescalationAttempt | FrameRelation::Surrender | FrameRelation::Flee)
}

fn infer_desired_outcome(action: SituationActionKind, relation: FrameRelation) -> Option<&'static str> {
    match relation {
        FrameRelation::Flee => Some("escape the area or transition to chase"),
        FrameRelation::HideToDisengage => Some("break line of sight and stop direct engagement"),
        FrameRelation::DeescalationAttempt => Some("turn conflict into negotiation or ceasefire"),
        FrameRelation::Surrender => Some("end violence through surrender"),
        _ => match action {
            SituationActionKind::Attack => Some("harm, suppress, or drive off opposition"),
            SituationActionKind::UnderAttack | SituationActionKind::EnemyInitiatedConflict | SituationActionKind::SceneEntersConflict => Some("survive and respond to the immediate threat"),
            SituationActionKind::Hack => Some("gain control or disable a technical threat"),
            SituationActionKind::InvestigateDuringConflict => Some("gain actionable information under pressure"),
            _ => None,
        },
    }
}

fn make_reaction_gate(input: ConflictTurnInput<'_>, frame: &StateFrame, advice: &ReactionAdvice) -> InteractionGate {
    let now = Utc::now();
    InteractionGate {
        gate_id: format!("gate_reaction_{}", Uuid::new_v4().simple()),
        session_id: input.session_id.to_string(),
        turn_id: input.turn_id.to_string(),
        gate_kind: advice.gate_kind,
        status: GateStatus::Open,
        prompt_public: if advice.prompt_public.is_empty() { "你必须先处理当前反应窗口。".into() } else { advice.prompt_public.clone() },
        prompt_gm: Some("Required/optional reaction window created from ruleset situation profile; do not allow unrelated action until resolved unless profile permits it.".into()),
        required: advice.required,
        allowed_options: advice.options.clone(),
        expected_input: ExpectedInput::Choice { option_ids: advice.options.iter().map(|o| o.option_id.clone()).collect() },
        on_unparseable: advice.on_unparseable,
        on_new_action: advice.on_new_action,
        on_timeout: if advice.required { GateFallbackPolicy::RequireExplicitChoice } else { GateFallbackPolicy::ResolveAsNoReaction },
        bound_action_summary: format!("reaction window in frame {}", frame.frame_id),
        source_refs: vec![],
        advice_refs: vec![advice.advice_id.clone()],
        resolution_json: None,
        expires_at_turn: Some(input.turn_id.to_string()),
        expires_at_time: None,
        interaction_context_id: Some(format!("ctx_{}", frame.frame_id)),
        owner_frame_id: Some(frame.frame_id.clone()),
        generation: 0,
        superseded_reason: None,
        closed_at_tick: None,
        created_at: now,
        updated_at: now,
    }
}

fn make_direction_gate(input: ConflictTurnInput<'_>, frame: &StateFrame, stalemate: &StalemateContract, prompt_prefix: &str) -> InteractionGate {
    let now = Utc::now();
    InteractionGate {
        gate_id: format!("gate_direction_{}", Uuid::new_v4().simple()),
        session_id: input.session_id.to_string(),
        turn_id: input.turn_id.to_string(),
        gate_kind: GateKind::ChooseActionMode,
        status: GateStatus::Open,
        prompt_public: format!("{}\n\n{}", prompt_prefix.trim(), stalemate.direction_options.iter().map(|o| format!("- {} ({})：{}", o.label, o.option_id, o.meaning)).collect::<Vec<_>>().join("\n")),
        prompt_gm: Some("Situation direction gate opened by semantic frame evaluator. This prevents narrative/combat loops by forcing a meaningful direction change.".into()),
        required: true,
        allowed_options: stalemate.direction_options.clone(),
        expected_input: ExpectedInput::Choice { option_ids: stalemate.direction_options.iter().map(|o| o.option_id.clone()).collect() },
        on_unparseable: GateFallbackPolicy::Reprompt,
        on_new_action: GateFallbackPolicy::RequireExplicitChoice,
        on_timeout: GateFallbackPolicy::RequireExplicitChoice,
        bound_action_summary: format!("direction gate for frame {}", frame.frame_id),
        source_refs: vec![],
        advice_refs: vec!["semantic_situation_orchestrator.v1_3.direction_gate".into()],
        resolution_json: None,
        expires_at_turn: Some(input.turn_id.to_string()),
        expires_at_time: None,
        interaction_context_id: Some(format!("ctx_{}", frame.frame_id)),
        owner_frame_id: Some(frame.frame_id.clone()),
        generation: 0,
        superseded_reason: None,
        closed_at_tick: None,
        created_at: now,
        updated_at: now,
    }
}


fn combat_roll_visibility_from_policy() -> (RollVisibility, RollAuthority) {
    if system_rolls_visible_policy() {
        (RollVisibility::PublicGmRoll, RollAuthority::System)
    } else {
        (RollVisibility::PlayerRollRequired, RollAuthority::Player)
    }
}



// P0-2: `inferred_homecoming_tech_dv` (14/12) and `target_actor_for_combat_input`
// (npc.scav_boss/npc.athena_drone literals) are gone — both now read module data
// via `tech_dv_from_config` / `actor_for_combat_input` in policy.rs, sourced from
// `ModuleConfig` (data/modules/{id}.module_config.json).

fn make_combat_check_contract(
    input: ConflictTurnInput<'_>,
    frame: &StateFrame,
    intent: &ConflictIntent,
    label_policy: Option<&CheckLabelPolicy>,
    module_cfg: Option<&ModuleConfig>,
) -> CheckContract {
    let check_id = format!("check_{}", Uuid::new_v4().simple());
    let (roll_visibility, roll_authority) = combat_roll_visibility_from_policy();
    let target_actor = actor_for_combat_input(module_cfg, input.user_input, intent);
    let defender_actor = target_actor.clone().unwrap_or_else(|| ActorRef { actor_id: "npc.opposition".into(), actor_kind: ActorKind::Npc, display_name: Some("opposition".into()) });
    let label = check_label_for(label_policy, intent.action_kind);
    CheckContract {
        check_id,
        session_id: input.session_id.to_string(),
        turn_id: input.turn_id.to_string(),
        ruleset_id: input.ruleset_id.to_string(),
        module_id: input.module_id.map(str::to_string),
        initiator: ActorRef { actor_id: input.actor_id.unwrap_or("pc.current").into(), actor_kind: ActorKind::PlayerCharacter, display_name: Some("current actor".into()) },
        target_actor: if matches!(intent.action_kind, SituationActionKind::Attack | SituationActionKind::Counterattack | SituationActionKind::UnderAttack | SituationActionKind::EnemyInitiatedConflict | SituationActionKind::SceneEntersConflict) { target_actor.clone() } else { None },
        opposition: combat_opposition_for_intent(&intent.action_kind, defender_actor),
        action_summary: input.user_input.chars().take(500).collect(),
        intent_kind: intent.action_kind.as_str().into(),
        check_label: label.into(),
        dice_expression: "1d20".into(), // placeholder; overridden by combat_core_dice (kernel) in the async builder
        modifiers: vec![],
        target: combat_target_for_intent(&intent.action_kind),
        tested_parameter: None,
        actor_snapshot_ids: vec![frame.frame_id.clone()],
        source_refs: vec![],
        learned_packet_ids: vec![],
        roll_visibility,
        roll_authority,
        disclosure: RollDisclosurePolicy::for_visibility(roll_visibility),
        stakes: CheckStakes { before_roll_public: "This action has uncertainty and meaningful consequences inside the active situation frame. If source-backed actor/target parameters are missing, the runtime must hydrate them before rolling.".into(), success_public: "The action changes the situation in your favor and may create an EffectContract.".into(), failure_public: "The situation stays dangerous, worsens, or costs time/resources.".into(), critical_public: None, fumble_public: None, success_patches_allowed: vec![], failure_patches_allowed: vec![], irreversible: false },
        confidence: intent.confidence,
        ruling_status: RulingStatus::Provisional,
        advice_refs: vec!["semantic_situation_orchestrator.v1_3.action_contract".into(), "source_backed_parameters_required.v1_15_4".into()],
        expires_at_turn: Some(input.turn_id.to_string()),
        opponent_tested_parameter: None,
    }
}


// P0-2: `apply_source_backed_formula_pack_to_check` (and its only helper
// `merge_source_refs`) were removed. The function was dead code (no caller) and
// carried five ruleset-name bare-dice branches (cyberpunk/dnd/sword_world/coc/
// brp/triangle); the live degrade path is `qualify_bare_dice` (kernel.
// dice_qualification) in `hydrate_combat_check_contract_from_rule_steward`.

fn combat_target_for_intent(action_kind: &SituationActionKind) -> CheckTargetModel {
    if matches!(action_kind, SituationActionKind::Attack | SituationActionKind::Counterattack) {
        if let Some(value) = env_i32("TRPG_COMBAT_DEFAULT_ATTACK_DV") {
            CheckTargetModel::StaticNumber { value, label: "table-configured attack DV override".into() }
        } else {
            CheckTargetModel::UnknownUntilLookup
        }
    } else {
        CheckTargetModel::UnknownUntilLookup
    }
}

fn combat_opposition_for_intent(action_kind: &SituationActionKind, defender_actor: ActorRef) -> OppositionModel {
    if matches!(action_kind, SituationActionKind::Attack | SituationActionKind::Counterattack) {
        if let Some(score) = env_i32("TRPG_CONTEST_DEFAULT_ATTACK_DV") {
            OppositionModel::PassiveDefense { defender: defender_actor, passive_score_label: "table-configured attack defense/DV override".into(), passive_score: score }
        } else {
            OppositionModel::NoMechanicalOpposition
        }
    } else if matches!(action_kind, SituationActionKind::UnderAttack | SituationActionKind::EnemyInitiatedConflict | SituationActionKind::SceneEntersConflict | SituationActionKind::Defend | SituationActionKind::Dodge) {
        if let Some(dc) = env_i32("TRPG_CONTEST_DEFAULT_DEFENSE_DV") {
            OppositionModel::StaticDc { dc, label: "table-configured defense/reaction DV override".into() }
        } else {
            OppositionModel::NoMechanicalOpposition
        }
    } else {
        OppositionModel::NoMechanicalOpposition
    }
}

fn env_i32(key: &str) -> Option<i32> { std::env::var(key).ok().and_then(|v| v.parse().ok()) }

fn make_effect_contract(check: &CheckContract, _frame: &StateFrame, intent: &ConflictIntent) -> EffectContract {
    let kind = match intent.action_kind {
        SituationActionKind::Attack | SituationActionKind::Counterattack => EffectKind::Damage,
        SituationActionKind::UnderAttack | SituationActionKind::EnemyInitiatedConflict | SituationActionKind::SceneEntersConflict | SituationActionKind::Defend | SituationActionKind::Dodge | SituationActionKind::TakeCover | SituationActionKind::Hide => EffectKind::PositionChange,
        SituationActionKind::Hack | SituationActionKind::DisableDevice => EffectKind::NarrativeConsequence,
        SituationActionKind::Negotiate | SituationActionKind::Intimidate => EffectKind::MoraleChange,
        _ => EffectKind::NarrativeConsequence,
    };
    EffectContract {
        effect_id: format!("effect_{}", Uuid::new_v4().simple()),
        source_event_id: None,
        effect_kind: kind,
        target_actor_ids: check.target_actor.as_ref().map(|a| vec![a.actor_id.clone()]).unwrap_or_default(),
        source_refs: check.source_refs.clone(),
        learned_packet_ids: check.learned_packet_ids.clone(),
        deterministic_parts: vec![ResolvedEffectPart { label: "pending_resolution".into(), value_json: json!({"check_id": check.check_id.clone(), "action_kind": intent.action_kind.as_str()}), source_refs: vec![] }],
        pending_rolls: vec![EffectRollRequest { label: check.check_label.clone(), dice_expression: check.dice_expression.clone(), roll_visibility: check.roll_visibility, target_actor_id: check.target_actor.as_ref().map(|a| a.actor_id.clone()) }],
        proposed_patches: vec![],
        visibility: Visibility::GmOnly,
        confidence: intent.confidence,
        metadata: json!({"created_by":"semantic_situation_orchestrator_v1_3", "check_id": check.check_id.clone(), "intent_id": intent.intent_id.clone()}),
    }
}

fn make_exit_contract(input: ConflictTurnInput<'_>, frame: &StateFrame, intent: &ConflictIntent) -> ExitContract {
    let (exit_kind, success, response) = match intent.relation_to_active_frame {
        FrameRelation::Surrender => (ExitKind::Surrender, FrameOutcome::SurrenderAccepted, OpponentResponsePolicy::MayRefuse),
        FrameRelation::DeescalationAttempt => (ExitKind::Negotiate, FrameOutcome::NegotiatedTruce, OpponentResponsePolicy::MayRefuse),
        FrameRelation::Flee | FrameRelation::ExitAttempt => (ExitKind::FleeArea, FrameOutcome::PlayerEscaped, OpponentResponsePolicy::MayPursue),
        FrameRelation::HideToDisengage => (ExitKind::HideAndDisengage, FrameOutcome::Paused, OpponentResponsePolicy::MayPursue),
        FrameRelation::PauseAndObserve => (ExitKind::TakeCoverAndPause, FrameOutcome::Paused, OpponentResponsePolicy::NoResponseNeeded),
        _ => (ExitKind::LeaveScene, FrameOutcome::Closed, OpponentResponsePolicy::NoResponseNeeded),
    };
    ExitContract {
        exit_id: format!("exit_{}", Uuid::new_v4().simple()),
        frame_id: frame.frame_id.clone(),
        exit_kind,
        actor_id: input.actor_id.unwrap_or("pc.current").into(),
        target_side: None,
        method_summary: input.user_input.chars().take(500).collect(),
        requires_check: false,
        check_contract: None,
        opponent_response: response,
        success_outcome: success,
        failure_outcome: FrameOutcome::Continue,
        source_refs: vec![],
        advice_refs: vec!["semantic_situation_orchestrator.v1_3.exit_contract".into()],
        metadata: json!({"intent": intent.clone(), "policy":"semantic_exit_first_no_keyword_lock"}),
    }
}

fn make_stalemate_contract(frame: &StateFrame, reason: &str) -> StalemateContract {
    let tracker = serde_json::from_value::<CombatWorkingState>(frame.working_state.clone()).ok().map(|w| w.progress_tracker).unwrap_or_default();
    StalemateContract {
        stalemate_id: format!("stalemate_{}", Uuid::new_v4().simple()),
        frame_id: frame.frame_id.clone(),
        reason: reason.into(),
        turns_since_decisive_change: tracker.turns_since_decisive_change,
        repeated_action_count: tracker.repeated_action_count,
        direction_options: direction_options_for_frame(frame),
        recommended_gate_kind: GateKind::ChooseActionMode,
        metadata: json!({"created_by":"semantic_situation_orchestrator_v1_3"}),
    }
}

fn direction_options_for_frame(frame: &StateFrame) -> Vec<ActionOption> {
    match frame.frame_kind {
        FrameKind::SocialConflict | FrameKind::Negotiation => vec![
            action_option("new_leverage", "提出新筹码", "提供新信息、让步、承诺或资源，改变谈判局面。"),
            action_option("pressure", "施压 / 威慑", "承担升级风险，迫使对方改变立场。"),
            action_option("accept_or_leave", "接受条件或离开", "结束本轮拉扯，进入后果结算。"),
        ],
        FrameKind::InvestigationNode => vec![
            action_option("change_method", "换一种调查方法", "换技能、工具、对象或角度，不再重复同一动作。"),
            action_option("deep_search_cost", "花费时间/资源深查", "承担时间、风险或资源成本，换取更深层信息。"),
            action_option("leave_node", "离开这个节点", "接受当前信息，转向其他地点/NPC/线索。"),
        ],
        _ => vec![
            action_option("continue_conflict", "继续正面对抗", "接受继续承压，明确本轮目标。"),
            action_option("escape_or_disengage", "撤退 / 脱离", "尝试退出当前冲突，可能转为追逐或关闭 frame。"),
            action_option("deescalate", "谈判 / 威慑 / 投降", "把冲突转成社交解决或停火。"),
            action_option("push_objective", "冒险推进目标", "不以击败对手为目标，直接处理任务目标。"),
            action_option("change_tactic", "改变战术", "换位置、目标、资源或能力，避免重复行动。"),
        ],
    }
}

fn action_option(id: &str, label: &str, meaning: &str) -> ActionOption {
    ActionOption { option_id: id.into(), label: label.into(), meaning: meaning.into(), is_default: false, consequences: json!({}) }
}

fn default_objectives_for_mode(mode: CombatMode) -> Vec<CombatObjective> {
    let summary = match mode {
        CombatMode::AnomalyEncounter => "Resolve the anomaly encounter: capture, contain, escape, or survive with mission consequences.",
        CombatMode::HorrorEncounter => "Survive the horror encounter and preserve clue / SAN consequences.",
        CombatMode::Netrun => "Resolve the netrun objective without confusing NET state with physical combat state.",
        CombatMode::Chase => "Resolve pursuit, escape, capture, or transition back to scene play.",
        _ => "Resolve the immediate conflict objective; defeating opposition is only one possible outcome.",
    };
    vec![CombatObjective { objective_id: "objective.resolve_situation".into(), summary: summary.into(), owner_side_id: None, status: "active".into() }]
}

fn default_npc_drive_for_mode(mode: CombatMode) -> NpcDriveState {
    let (morale, patience, aggression) = match mode {
        CombatMode::HorrorEncounter | CombatMode::AnomalyEncounter => (80, 35, 70),
        CombatMode::SocialConflict => (45, 45, 20),
        _ => (55, 40, 55),
    };
    NpcDriveState { npc_id: "npc.opposition".into(), current_goal: "press the current scene objective".into(), current_tactic: "initial_pressure".into(), patience, morale, aggression, risk_tolerance: 45, self_interest: 60, ..Default::default() }
}

fn update_combat_working_state_event(frame: &mut StateFrame, event_id: &str) {
    if let Ok(mut working) = serde_json::from_value::<CombatWorkingState>(frame.working_state.clone()) {
        working.last_events.push(event_id.to_string());
        if working.last_events.len() > 12 { working.last_events.remove(0); }
        frame.working_state = serde_json::to_value(working).unwrap_or_else(|_| frame.working_state.clone());
    }
    frame.event_count += 1;
    frame.last_event_ids.push(event_id.to_string());
    if frame.last_event_ids.len() > 12 { frame.last_event_ids.remove(0); }
    frame.updated_at = Utc::now();
}

fn update_combat_working_state_phase(frame: &mut StateFrame, phase: CombatPhase) {
    if let Ok(mut working) = serde_json::from_value::<CombatWorkingState>(frame.working_state.clone()) {
        working.phase = phase;
        frame.working_state = serde_json::to_value(working).unwrap_or_else(|_| frame.working_state.clone());
    }
    frame.updated_at = Utc::now();
}

fn update_combat_working_state_outcome(frame: &mut StateFrame, outcome: FrameOutcome) {
    if let Ok(mut working) = serde_json::from_value::<CombatWorkingState>(frame.working_state.clone()) {
        working.current_outcome = Some(outcome);
        working.phase = match outcome {
            FrameOutcome::StalemateNeedsDirection => CombatPhase::DirectionGate,
            FrameOutcome::Paused => CombatPhase::Stalemate,
            FrameOutcome::PlayerEscaped | FrameOutcome::NegotiatedTruce | FrameOutcome::SurrenderAccepted | FrameOutcome::Closed | FrameOutcome::Abandoned => CombatPhase::CombatEndPendingCompaction,
            _ => CombatPhase::EvaluatingFrame,
        };
        frame.working_state = serde_json::to_value(working).unwrap_or_else(|_| frame.working_state.clone());
    }
    frame.updated_at = Utc::now();
}

fn update_combat_working_state_intent_and_progress(frame: &mut StateFrame, intent: &ConflictIntent, decisive: bool) {
    if let Ok(mut working) = serde_json::from_value::<CombatWorkingState>(frame.working_state.clone()) {
        let last = working.progress_tracker.last_action_kind;
        working.progress_tracker.turns_elapsed += 1;
        if decisive {
            working.progress_tracker.turns_since_decisive_change = 0;
            working.progress_tracker.objective_progress_delta += 1;
        } else {
            working.progress_tracker.turns_since_decisive_change += 1;
        }
        if last == Some(intent.action_kind) {
            working.progress_tracker.repeated_action_count += 1;
        } else {
            working.progress_tracker.repeated_action_count = 0;
        }
        working.progress_tracker.last_action_kind = Some(intent.action_kind);
        working.last_intent = Some(intent.clone());
        frame.working_state = serde_json::to_value(working).unwrap_or_else(|_| frame.working_state.clone());
    }
    frame.updated_at = Utc::now();
}

fn update_npc_drive_after_turn(frame: &mut StateFrame, intent: &ConflictIntent, decisive: bool) {
    if let Ok(mut working) = serde_json::from_value::<CombatWorkingState>(frame.working_state.clone()) {
        for drive in &mut working.npc_drive_states {
            if decisive {
                drive.patience = (drive.patience + 4).min(100);
                drive.repeated_strategy_count = 0;
            } else {
                drive.patience = (drive.patience - 10).max(0);
                drive.repeated_strategy_count += 1;
            }
            if matches!(intent.relation_to_active_frame, FrameRelation::DeescalationAttempt | FrameRelation::Surrender) {
                drive.aggression = (drive.aggression - 12).max(0);
            }
            if matches!(intent.action_kind, SituationActionKind::Attack | SituationActionKind::Intimidate) {
                drive.morale = (drive.morale - 5).max(0);
                drive.aggression = (drive.aggression + 5).min(100);
            }
        }
        frame.working_state = serde_json::to_value(working).unwrap_or_else(|_| frame.working_state.clone());
    }
}

fn should_open_stalemate_gate(frame: &StateFrame) -> bool {
    serde_json::from_value::<CombatWorkingState>(frame.working_state.clone()).ok().map(|working| {
        working.progress_tracker.turns_since_decisive_change >= 3
            || working.progress_tracker.repeated_action_count >= 2
            || working.npc_drive_states.iter().any(|d| d.patience <= 0 || d.repeated_strategy_count >= 3)
    }).unwrap_or(false)
}


fn apply_novelty_director(frame: &mut StateFrame, intent: &ConflictIntent, turn_id: &str, decisive: bool) -> NoveltyDecision {
    let mut decision = NoveltyDecision {
        decision_id: format!("novelty_{}", Uuid::new_v4().simple()),
        force_tactic_shift: false,
        selected_tactic_id: None,
        previous_tactic_id: None,
        novelty_score: 1.0,
        reason: "no_repeat_validator_passed".into(),
        fresh_change: None,
        beat: None,
    };
    let Ok(mut working) = serde_json::from_value::<CombatWorkingState>(frame.working_state.clone()) else { return decision; };
    let actor_id = working.npc_drive_states.first().map(|d| d.npc_id.clone()).unwrap_or_else(|| "npc.opposition".into());
    let previous = working.npc_drive_states.first().map(|d| d.current_tactic.clone()).unwrap_or_else(|| "initial_pressure".into());
    let repeated_action = working.progress_tracker.repeated_action_count >= 1;
    let repeated_npc = working.npc_drive_states.first().map(|d| d.repeated_strategy_count >= 1).unwrap_or(false);
    let force_shift = repeated_action || repeated_npc || matches!(intent.action_kind, SituationActionKind::Attack | SituationActionKind::UnderAttack | SituationActionKind::EnemyInitiatedConflict | SituationActionKind::SceneEntersConflict);

    let selected = if force_shift { select_next_tactic(&working, &actor_id, &previous, intent) } else { previous.clone() };
    if selected != previous || force_shift {
        decision.force_tactic_shift = true;
        decision.previous_tactic_id = Some(previous.clone());
        decision.selected_tactic_id = Some(selected.clone());
        decision.novelty_score = if selected == previous { 0.35 } else { 0.82 };
        decision.reason = if repeated_action { "player_repeated_action_world_must_change".into() } else if repeated_npc { "npc_tactic_repetition_prevented".into() } else { "enemy_or_scene_pressure_requires_fresh_response".into() };
        let change = FreshChange {
            change_id: format!("fresh_{}", Uuid::new_v4().simple()),
            change_type: if repeated_action { FreshChangeType::ContinuedPressureWithCost } else { FreshChangeType::NpcTacticShift },
            summary: fresh_change_summary(&selected, intent, decisive),
            caused_by: intent.action_kind.as_str().into(),
            source_refs: vec![],
            created_at: Utc::now(),
        };
        decision.fresh_change = Some(change.clone());
        let beat = BeatSignature {
            beat_id: format!("beat_{}", Uuid::new_v4().simple()),
            actor_id: actor_id.clone(),
            tactic_id: selected.clone(),
            target_id: Some("pc.current".into()),
            approach_vector: intent.action_kind.as_str().into(),
            consequence_type: change.change_type.as_str().into(),
            scene_element_used: Some(scene_element_for_tactic(&selected).into()),
            emotional_tone: Some("pressure_shift".into()),
            turn_id: Some(turn_id.into()),
        };
        decision.beat = Some(beat.clone());
        for drive in &mut working.npc_drive_states {
            if drive.npc_id == actor_id {
                drive.current_tactic = selected.clone();
                drive.repeated_strategy_count = if selected == previous { drive.repeated_strategy_count.saturating_add(1) } else { 0 };
            }
        }
        let mut found = false;
        for mem in &mut working.npc_tactic_memory {
            if mem.npc_id == actor_id && mem.tactic_id == selected {
                mem.used_count_in_frame = mem.used_count_in_frame.saturating_add(1);
                mem.consecutive_count = if selected == previous { mem.consecutive_count.saturating_add(1) } else { 1 };
                mem.last_result = Some(decision.reason.clone());
                mem.last_turn_id = Some(turn_id.into());
                found = true;
            }
        }
        if !found {
            working.npc_tactic_memory.push(NpcTacticMemory { npc_id: actor_id.clone(), tactic_id: selected.clone(), used_count_in_frame: 1, consecutive_count: 1, last_result: Some(decision.reason.clone()), last_turn_id: Some(turn_id.into()) });
        }
        working.novelty_state.recent_beats.push(beat);
        if working.novelty_state.recent_beats.len() > 8 { working.novelty_state.recent_beats.remove(0); }
        working.novelty_state.last_fresh_change = Some(change);
        working.novelty_state.forced_tactic_shift_count = working.novelty_state.forced_tactic_shift_count.saturating_add(1);
    }
    frame.working_state = serde_json::to_value(working).unwrap_or_else(|_| frame.working_state.clone());
    decision
}

fn select_next_tactic(working: &CombatWorkingState, actor_id: &str, previous: &str, intent: &ConflictIntent) -> String {
    let palette = working.tactic_palettes.iter().find(|p| p.actor_id == actor_id).or_else(|| working.tactic_palettes.first());
    let Some(palette) = palette else { return fallback_tactic_for_intent(intent, previous); };
    let recent: BTreeSet<String> = working.novelty_state.recent_beats.iter().rev().take(3).map(|b| b.tactic_id.clone()).collect();
    let mut candidates: Vec<&NpcTactic> = palette.tactics.iter().filter(|t| t.tactic_id != previous && !recent.contains(&t.tactic_id)).collect();
    if candidates.is_empty() { candidates = palette.tactics.iter().filter(|t| t.tactic_id != previous).collect(); }
    if candidates.is_empty() { return fallback_tactic_for_intent(intent, previous); }
    let preferred_tag = match intent.action_kind {
        SituationActionKind::Attack | SituationActionKind::Counterattack => "position",
        SituationActionKind::Hide | SituationActionKind::Flee => "pursuit",
        SituationActionKind::Negotiate | SituationActionKind::Surrender | SituationActionKind::Intimidate => "social_pressure",
        SituationActionKind::Hack | SituationActionKind::DisableDevice => "objective",
        _ => "pressure",
    };
    candidates.iter().find(|t| t.novelty_tags.iter().any(|tag| tag == preferred_tag)).copied().unwrap_or(candidates[0]).tactic_id.clone()
}

fn fallback_tactic_for_intent(intent: &ConflictIntent, previous: &str) -> String {
    let fallback = match intent.action_kind {
        SituationActionKind::Attack | SituationActionKind::Counterattack => "shift_position_and_pressure_objective",
        SituationActionKind::Hide | SituationActionKind::Flee => "pursue_or_cut_off_escape",
        SituationActionKind::Negotiate | SituationActionKind::Surrender => "demand_terms_or_stand_down",
        SituationActionKind::Hack | SituationActionKind::DisableDevice => "protect_control_source",
        _ => "advance_scene_pressure",
    };
    if fallback == previous { "change_target_or_clock".into() } else { fallback.into() }
}

fn fresh_change_summary(tactic_id: &str, intent: &ConflictIntent, decisive: bool) -> String {
    let pressure = if decisive { "玩家的动作产生了效果，但局势不会原地重复" } else { "局势没有明显推进，因此世界主动变化" };
    match tactic_id {
        "sweep_fire_to_cover" => format!("{pressure}：敌方不再原地互射，而是扫向掩体边缘，迫使位置选择变得重要。"),
        "threaten_downed_npc" => format!("{pressure}：敌方转向倒地 NPC 或弱势目标，把战斗从互射变成救援压力。"),
        "overheat_or_spark" => format!("{pressure}：设备开始过热或冒火花，现场新增环境危险。"),
        "retreat_to_anchor" => format!("{pressure}：敌方缩向关键锚点/控制源，暴露新的目标但拉开距离。"),
        "call_backup_or_alarm" => format!("{pressure}：敌方改变策略，尝试呼叫支援或推进警报 clock。"),
        "demand_terms_or_stand_down" => format!("{pressure}：对方不继续重复攻击，而是提出条件或试图改变冲突形式。"),
        _ => format!("{pressure}：NPC tactic shifts to `{}` in response to `{}`。", tactic_id, intent.action_kind.as_str()),
    }
}

fn scene_element_for_tactic(tactic_id: &str) -> &'static str {
    match tactic_id {
        "sweep_fire_to_cover" => "cover_edge",
        "threaten_downed_npc" => "vulnerable_npc",
        "overheat_or_spark" => "environmental_hazard",
        "retreat_to_anchor" => "objective_anchor",
        "call_backup_or_alarm" => "reinforcement_clock",
        _ => "scene_pressure",
    }
}

fn default_tactic_palette_for_mode(mode: CombatMode) -> TacticPalette {
    let archetype = match mode { CombatMode::AnomalyEncounter => "anomaly", CombatMode::HorrorEncounter => "horror", CombatMode::SocialConflict => "social_opponent", _ => "combat_opposition" };
    let tactics = match mode {
        CombatMode::AnomalyEncounter => vec![
            tactic("distort_local_rule", "扭曲局部规则", &["pressure", "weird"], 1),
            tactic("consume_scene_object", "吞噬/改写一个场景物", &["new_affordance", "hazard"], 1),
            tactic("force_agent_choice", "迫使特工选择代价", &["social_pressure", "objective"], 1),
        ],
        CombatMode::SocialConflict => vec![
            tactic("demand_terms_or_stand_down", "要求条件或停手", &["social_pressure"], 1),
            tactic("shift_leverage", "改变筹码", &["new_pressure"], 1),
            tactic("walk_away_or_deadline", "离开或设置最后期限", &["clock", "pressure"], 1),
        ],
        _ => vec![
            tactic("sweep_fire_to_cover", "扫射掩体边缘", &["position", "pressure"], 1),
            tactic("threaten_downed_npc", "转向倒地 NPC 制造压力", &["hostage_pressure", "clock"], 1),
            tactic("overheat_or_spark", "设备过热，环境危险升级", &["environment", "hazard"], 1),
            tactic("retreat_to_anchor", "缩回关键锚点保护控制源", &["objective", "position"], 1),
            tactic("call_backup_or_alarm", "呼叫支援或触发警报", &["clock", "pressure"], 1),
        ],
    };
    TacticPalette { actor_id: "npc.opposition".into(), archetype: archetype.into(), max_repeat_same_tactic: 1, tactics }
}

fn tactic(id: &str, label: &str, tags: &[&str], cooldown: u32) -> NpcTactic {
    NpcTactic { tactic_id: id.into(), label: label.into(), use_when: vec![], avoid_when: vec![], creates: vec![], novelty_tags: tags.iter().map(|s| s.to_string()).collect(), cooldown_after_use: cooldown }
}

fn frame_kind_for_mode(mode: CombatMode) -> FrameKind {
    match mode {
        CombatMode::Chase => FrameKind::Chase,
        CombatMode::Netrun => FrameKind::Netrun,
        CombatMode::AnomalyEncounter => FrameKind::AnomalyEncounter,
        CombatMode::HorrorEncounter => FrameKind::HorrorEncounter,
        CombatMode::SocialConflict => FrameKind::SocialConflict,
        CombatMode::HazardSequence => FrameKind::HazardSequence,
        _ => FrameKind::Combat,
    }
}

fn infer_mode_from_frame(frame: &StateFrame) -> Option<CombatMode> {
    serde_json::from_value::<CombatWorkingState>(frame.working_state.clone()).ok().map(|w| w.combat_mode)
}

// P0-2: `infer_combat_mode_from_intent`(ruleset_id.contains branches) is gone —
// the intent→CombatMode mapping now lives in data via `combat_mode_from_policy`
// (kernel.combat_mode_policy) in policy.rs.

fn lexical_any(input: &str, terms: &[&str]) -> bool { let lower=input.to_lowercase(); terms.iter().any(|t| lower.contains(&t.to_lowercase())) }

fn semantic_cluster_score(input: &str, examples: &[&str]) -> f32 {
    let norm = normalize_for_similarity(input);
    if norm.is_empty() { return 0.0; }
    let input_bigrams = char_bigrams(&norm);
    let mut best: f32 = 0.0;
    for example in examples {
        let ex = normalize_for_similarity(example);
        if ex.is_empty() { continue; }
        if norm.contains(&ex) || ex.contains(&norm) {
            best = best.max(0.96);
            continue;
        }
        let ex_bigrams = char_bigrams(&ex);
        let inter = input_bigrams.intersection(&ex_bigrams).count() as f32;
        let union = input_bigrams.union(&ex_bigrams).count() as f32;
        if union > 0.0 { best = best.max(inter / union); }
        let common_chars = norm.chars().filter(|c| ex.contains(*c)).count() as f32;
        let denom = norm.chars().count().max(ex.chars().count()) as f32;
        if denom > 0.0 { best = best.max(common_chars / denom * 0.72); }
    }
    best
}

fn normalize_for_similarity(input: &str) -> String {
    input.chars()
        .flat_map(|c| c.to_lowercase())
        .filter(|c| !c.is_whitespace() && !c.is_ascii_punctuation())
        .collect()
}

fn char_bigrams(input: &str) -> BTreeSet<String> {
    let chars: Vec<char> = input.chars().collect();
    let mut out = BTreeSet::new();
    if chars.len() <= 1 {
        if !input.is_empty() { out.insert(input.to_string()); }
        return out;
    }
    for i in 0..chars.len() - 1 {
        out.insert(format!("{}{}", chars[i], chars[i + 1]));
    }
    out
}

fn detect_language_hint(input: &str) -> Option<String> {
    if input.chars().any(|c| ('\u{3040}'..='\u{30ff}').contains(&c)) { Some("ja".into()) }
    else if input.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)) { Some("zh".into()) }
    else if input.chars().any(|c| c.is_ascii_alphabetic()) { Some("en".into()) }
    else { None }
}

const ATTACK_EXAMPLES: &[&str] = &[
    "拔枪还击", "拔出手枪回击", "朝它开火", "开枪反击", "射击无人机", "冲上去攻击", "砍它", "打倒敌人",
    "draw and return fire", "i shoot back", "open fire", "attack it", "strike the enemy", "fire at the drone",
    "銃を抜いて撃ち返す", "撃ち返す", "攻撃する", "発砲する",
];
const DEFENSE_EXAMPLES: &[&str] = &[
    "闪避", "躲避", "防御", "找掩体", "缩到掩体后", "避开攻击", "dodge", "defend", "take cover", "evade", "avoid the shot", "身を隠す", "回避する",
];
const UNDER_ATTACK_EXAMPLES: &[&str] = &[
    "无人机开始朝我开火", "敌人朝我开火", "对方先动手", "我被攻击", "枪口转向我", "敌人扑向我", "场面进入战斗", "under fire", "they open fire", "enemy attacks me", "the drone shoots at me", "scene enters combat", "敵が発砲", "撃たれた", "攻撃される",
];
const SCENE_CONFLICT_EXAMPLES: &[&str] = &[
    "场面进入战斗", "战斗开始", "冲突爆发", "敌方发起攻击", "现场变成枪战", "combat starts", "the fight begins", "the scene turns into combat", "hostilities break out", "戦闘開始", "戦闘になる",
];
const FLEE_EXAMPLES: &[&str] = &[
    "撤退离开", "转身撤退", "离开这片区域", "逃走", "脱离战斗", "先撤", "撤离现场", "不打了我走", "retreat", "withdraw", "flee", "leave the area", "run away", "disengage", "逃げる", "撤退する", "離脱する",
];
const HIDE_DISENGAGE_EXAMPLES: &[&str] = &[
    "躲到集装箱后面先不打了", "躲起来观察", "藏起来脱离", "先躲着", "hide and disengage", "hide behind cover", "break line of sight", "take cover and stop fighting", "隠れて様子を見る", "身を隠して離脱",
];
const DEESCALATE_EXAMPLES: &[&str] = &[
    "举手和解", "别打了谈谈", "停火谈判", "让对方冷静", "放低枪口", "谈判", "和解", "ceasefire", "parley", "negotiate", "talk this out", "stand down and talk", "話し合う", "停戦", "交渉する",
];
const SURRENDER_EXAMPLES: &[&str] = &[
    "投降", "放下武器", "举手投降", "我认输", "surrender", "drop my weapon", "give up", "降伏する", "武器を捨てる",
];
const OBSERVE_EXAMPLES: &[&str] = &[
    "先观察", "看看情况", "等一下", "按兵不动", "等待机会", "observe", "wait", "hold position", "watch for an opening", "様子を見る", "待つ",
];
const HACK_EXAMPLES: &[&str] = &[
    "黑入", "破解", "入侵系统", "接入网络", "黑客", "控制节点", "hack", "netrun", "interface with the system", "breach the device", "ネットラン", "ハックする",
];
const INVESTIGATE_EXAMPLES: &[&str] = &[
    "调查", "搜索", "检查", "判断", "分析", "找线索", "investigate", "search", "inspect", "analyze", "look for clues", "調査する", "確認する",
];
const ANOMALY_EXAMPLES: &[&str] = &[
    "异常", "收容异常", "捕获异常", "异常能力", "anomaly", "capture the anomaly", "containment", "chaos effect", "アノマリー",
];

// P0-2: the Rust fallback carries ONLY the neutral generic profile. The five
// per-ruleset profiles (cyberpunk/dnd/coc/triangle/sword_world) were duplicates
// of the curated `data/ruleset_advice/*.json` profiles (which already win at
// runtime via CombatProfilePack::load_dir → resolve); they are no longer
// hardcoded in Rust. When the advice dir is absent, an unmatched ruleset now
// resolves to `generic` instead of a hardcoded ruleset profile.
fn default_profiles() -> CombatProfilePack {
    CombatProfilePack { profiles: vec![default_generic_profile()] }
}

/// P0-2: the binary-embedded advice profiles (clean-checkout baseline). Parses
/// the 5 git-tracked `embedded_config/ruleset_advice/` copies (byte-identical to
/// data/); a profile that fails to parse or lacks a profile_id is skipped, just
/// like `load_dir` does for on-disk files.
fn embedded_profiles() -> Vec<RulesetCombatProfile> {
    const EMBEDDED: &[&str] = &[
        include_str!("../embedded_config/ruleset_advice/coc7e.conflict.v1.json"),
        include_str!("../embedded_config/ruleset_advice/cyberpunk_red.combat.v1.json"),
        include_str!("../embedded_config/ruleset_advice/dnd5e.combat.v1.json"),
        include_str!("../embedded_config/ruleset_advice/sword_world_2_5.combat.v1.json"),
        include_str!("../embedded_config/ruleset_advice/triangle_agency.conflict.v1.json"),
    ];
    EMBEDDED.iter()
        .filter_map(|t| serde_json::from_str::<RulesetCombatProfile>(t).ok())
        .filter(|p| !p.profile_id.is_empty())
        .collect()
}

fn default_generic_profile() -> RulesetCombatProfile {
    RulesetCombatProfile { profile_id: "generic.situation.v1_3".into(), ruleset_id: "generic".into(), applies_to_modes: vec!["theater_of_mind".into(), "tactical_combat".into(), "social_conflict".into()], default_mode: "theater_of_mind".into(), action_economy: json!({"policy":"fiction_first"}), initiative: json!({"policy":"fiction_first"}), reaction_windows: vec![generic_required_defense()], frame_exit_policy: json!({"state_exits":["objective_completed","side_escaped","negotiated_truce","surrender_accepted"],"stalemate_after_non_decisive_turns":3}), stalemate_policy: json!({"max_repeated_action_count":2,"open_direction_gate":true}), npc_drive_policy: json!({"default_patience":40,"default_morale":55,"max_repeat_same_tactic":2}), frame_retention_policy: RetentionPolicy { keep_event_log: true, compact_on_completion: true, keep_last_events: 12, promote_facts_with_importance_at_least: 70 }, frame_compaction_policy: CompactionPolicy { summary_target: "preserve consequences, costs, relationships, clues, open hooks; discard turn-by-turn minutiae".into(), preserve_world_patches: true, preserve_npc_impacts: true, preserve_player_costs: true, preserve_open_hooks: true }, ..Default::default() }
}


fn generic_required_defense() -> ReactionAdvice {
    ReactionAdvice { advice_id: "generic.required_defense_choice.v1_3".into(), gate_kind: GateKind::RequiredReactionChoice, required: true, prompt_public: "你正受到可见攻击。选择反应：闪避/防御、反击，或承受。".into(), options: vec![ActionOption { option_id: "defend_or_dodge".into(), label: "闪避 / 防御".into(), meaning: "尝试避免或降低攻击影响。".into(), is_default: false, consequences: json!({}) }, ActionOption { option_id: "counter_or_fight_back".into(), label: "反击".into(), meaning: "用攻击或对抗动作回应，可能改变结算。".into(), is_default: false, consequences: json!({}) }, ActionOption { option_id: "take_the_hit".into(), label: "站着挨打".into(), meaning: "不消耗反应或不防御，直接进入伤害/后果。".into(), is_default: false, consequences: json!({}) }], default_if_unanswered: None, on_unparseable: GateFallbackPolicy::Reprompt, on_new_action: GateFallbackPolicy::RequireExplicitChoice, trigger_keywords: vec![] }
}



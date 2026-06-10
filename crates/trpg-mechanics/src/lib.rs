pub mod direct_effect;

use anyhow::Result;
use chrono::Utc;
use serde_json::{json, Value};
use sqlx::Row;
use trpg_db::Db;
use trpg_model::*;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct FollowupCheck {
    pub pending: PendingCheck,
    pub prompt_public: String,
    pub reason: String,
    pub attack: Option<AttackResolutionContract>,
}

#[derive(Debug, Clone, Default)]
struct EffectApplicationOutcome {
    effect: EffectResolutionPacket,
    damage: Option<DamagePacket>,
    patches: Vec<StatePatch>,
}

#[derive(Debug, Clone)]
struct FacetDecision {
    target_kind: EffectTargetKind,
    target_id: String,
    parameter_path: String,
    operation: ParameterOperation,
    source_facet_binding_ids: Vec<String>,
    source_refs: Vec<SourceRef>,
    status: FacetExecutionStatus,
    provisional_reason: Option<String>,
}

impl FacetDecision {
    fn hp(target_id: String, reason: impl Into<String>) -> Self {
        Self {
            target_kind: EffectTargetKind::Actor,
            target_id,
            parameter_path: "hp.current".into(),
            operation: ParameterOperation::Subtract,
            source_facet_binding_ids: vec![],
            source_refs: vec![],
            status: FacetExecutionStatus::AppliedProvisional,
            provisional_reason: Some(reason.into()),
        }
    }
}

#[derive(Clone)]
pub struct RefereeCombatService { pub db: Db }


impl RefereeCombatService {
    pub fn new(db: Db) -> Self { Self { db } }

    /// Apply the rule kernel's `resource_tracks[*].on_outcome` rules to a
    /// resolved check's outcome JSON: map an outcome field (success_count,
    /// pool_miss_count, success, total, ...) to a delta on a scene/actor track,
    /// then fire edge-triggered threshold consequences. Entirely config-driven
    /// from the parsed kernel — no per-ruleset Rust, no keyword matching.
    async fn apply_outcome_resource_tracks(&self, contract: &CheckContract, result: &mut CheckResultRecord) {
        let kernel = match self.db.load_rule_kernel(&contract.ruleset_id).await {
            Ok(Some(k)) => k,
            _ => return,
        };
        // Seed/cap each track's current value from the actor's CHARACTER-DERIVED
        // resource (sheet_json.resources matched by kernel track id), falling back
        // to the kernel static initial/max. Loaded once and reused per track.
        let seeds = self.actor_resource_seeds(&contract.session_id, &contract.initiator.actor_id, &kernel).await;
        // Contract-constant: the text every track's `check_match` is scanned
        // against, including the deliberate tested_parameter binding (D2).
        let hay = check_match_hay(&contract.intent_kind, &contract.check_label, &contract.action_summary, contract.tested_parameter.as_ref());
        for track in &kernel.resource_tracks {
            let id = match track.get("id").and_then(|v| v.as_str()).or_else(|| track.get("name").and_then(|v| v.as_str())) {
                Some(s) if !s.trim().is_empty() => s.trim().to_string(),
                _ => continue,
            };
            // Whether this check deliberately rolled THIS track's parameter —
            // scopes the rule's default loss die (D3 fallback) to real rolls.
            let track_is_tested = track_matches_tested_parameter(contract.tested_parameter.as_ref(), track);
            let rules = match track.get("on_outcome").and_then(|v| v.as_array()) {
                Some(a) if !a.is_empty() => a,
                _ => continue,
            };
            let is_actor = track.get("owner_kind").and_then(|v| v.as_str()) == Some("actor");
            let target_kind = if is_actor { EffectTargetKind::Actor } else { EffectTargetKind::Scene };
            let target_id = if is_actor { contract.initiator.actor_id.clone() } else { "scene.current".to_string() };
            let path = if is_actor { format!("resources.{}.current", id) } else { format!("tracks.{}.current", id) };
            let max = track.get("max").and_then(|v| v.as_i64()).map(|n| n as i32);
            let initial = track.get("initial").and_then(|v| v.as_i64()).map(|n| n as i32);
            let success = result.outcome.get("success").and_then(|v| v.as_bool());
            for rule in rules {
                // Trigger gate: a rule fires on success, on failure, or always.
                match rule.get("trigger").and_then(|v| v.as_str()).unwrap_or("always") {
                    "on_success" if success != Some(true) => continue,
                    "on_failure" if success != Some(false) => continue,
                    // Gate on a graded success tier (e.g. CoC "extra effect on
                    // extreme"): `tier:"<id>"` and/or `min_rank:<n>` vs the
                    // success_tier the contest resolver emitted. Data-driven.
                    "on_tier" => {
                        let want_tier = rule.get("tier").and_then(|v| v.as_str());
                        let got_tier = result.outcome.get("success_tier").and_then(|v| v.as_str());
                        let tier_ok = want_tier.map(|w| Some(w) == got_tier).unwrap_or(true);
                        let rank_ok = match rule.get("min_rank").and_then(|v| v.as_i64()) {
                            Some(min) => result.outcome.get("success_tier_rank").and_then(|v| v.as_i64()).map(|r| r >= min).unwrap_or(false),
                            None => true,
                        };
                        if !(tier_ok && rank_ok) { continue; }
                    }
                    _ => {}
                }
                // Scope gate (config-driven, not hardcoded): if the rule declares
                // `check_match`, it only fires when the check's intent/label contains
                // it. This scopes e.g. "Sanity loss" to sanity checks, while an
                // unscoped rule (Triangle Chaos) fires on every roll as intended.
                if let Some(needle) = rule.get("check_match").and_then(|v| v.as_str()) {
                    if !needle.to_ascii_lowercase().split('|').any(|n| !n.trim().is_empty() && hay.contains(n.trim())) { continue; }
                }
                let op = match rule.get("op").and_then(|v| v.as_str()) {
                    Some("subtract") => ParameterOperation::Subtract,
                    Some("set") => ParameterOperation::Set,
                    _ => ParameterOperation::Add,
                };
                // Amount: dice expr ("1d6"), "=value" (the `when` field), "=<field>"
                // (read outcome.<field>, e.g. =sanity_loss), or a fixed int; with a
                // `default_amount` die fallback when the field is absent and this is
                // the tested parameter. Legacy `delta` is honored unchanged.
                let amount: i32 = match resolve_track_amount(
                    rule.get("amount").and_then(|v| v.as_str()),
                    rule.get("delta").and_then(|v| v.as_str()),
                    rule.get("when").and_then(|v| v.as_str()),
                    rule.get("default_amount").and_then(|v| v.as_str()),
                    track_is_tested,
                    &result.outcome,
                ) { Some(x) => x, None => continue };
                if amount == 0 && matches!(op, ParameterOperation::Add | ParameterOperation::Subtract) { continue; }
                // Seed `before` when no row exists yet: prefer the actor's
                // character-DERIVED current (so e.g. CoC Sanity starts at the
                // character's SAN, not the kernel static initial), then the kernel
                // `initial`.
                let before = if is_actor {
                    self.db.load_resource_current(&contract.session_id, &target_id, &id, &kernel).await
                        .or_else(|| seeds.get(&id).and_then(|s| s.0)).or(initial).unwrap_or(0)
                } else {
                    self.load_generic_parameter_value(&contract.session_id, target_kind, &target_id, &path).await
                        .ok().flatten().and_then(|v| v.as_i64()).map(|n| n as i32)
                        .or(seeds.get(&id).and_then(|s| s.0)).or(initial).unwrap_or(0)
                };
                let mut after = apply_i32_operation(before, amount, op);
                // Cap by the actor's character-DERIVED max first, then the kernel
                // static max.
                let cap = seeds.get(&id).and_then(|s| s.1).or(max);
                if let Some(m) = cap { after = after.min(m); }
                if is_actor {
                    self.db.write_resource_current(&contract.session_id, &target_id, &id, after, cap, &contract.source_refs, Visibility::GmOnly, 0).await.ok();
                } else {
                    self.upsert_generic_parameter_state(&contract.session_id, target_kind, &target_id, &path, json!(after), Visibility::GmOnly, &contract.source_refs, None, 0).await.ok();
                }
                result.committed_patches.push(StatePatch::CreateFact {
                    target: target_id.clone(),
                    fact: json!({"resource_track": id, "parameter_path": path, "before": before, "after": after, "delta_applied": after - before, "amount_rolled": amount}),
                    reason: "resource_delta_applied".into(),
                });
                if let Some(ths) = track.get("thresholds").and_then(|v| v.as_array()) {
                    for th in ths {
                        let cons = th.get("consequence").and_then(|v| v.as_str()).unwrap_or("threshold reached");
                        // (a) cumulative crossing: value reaches `at` (edge-triggered).
                        if let Some(at) = th.get("at").and_then(|v| v.as_i64()).map(|a| a as i32) {
                            let below = th.get("direction").and_then(|v| v.as_str()) == Some("at_or_below");
                            let crossed = if below { after <= at && before > at } else { after >= at && before < at };
                            if crossed {
                                result.committed_patches.push(StatePatch::CreateFact { target: target_id.clone(),
                                    fact: json!({"resource_track": id, "kind": "cumulative", "threshold_at": at, "value": after, "consequence": cons}),
                                    reason: "resource_threshold_consequence".into() });
                            }
                        }
                        // (b) single-application magnitude (e.g. CoC: lose >=5 in one roll -> temporary insanity).
                        if let Some(n) = th.get("loss_in_one_go").and_then(|v| v.as_i64()).map(|x| x as i32) {
                            let lost = before - after;
                            if matches!(op, ParameterOperation::Subtract) && lost >= n {
                                result.committed_patches.push(StatePatch::CreateFact { target: target_id.clone(),
                                    fact: json!({"resource_track": id, "kind": "loss_in_one_go", "lost": lost, "value": after, "consequence": cons}),
                                    reason: "resource_threshold_consequence".into() });
                            }
                        }
                    }
                }
            }
        }
    }

    pub async fn after_check_resolved(&self, contract: &CheckContract, result: &mut CheckResultRecord) -> Result<Option<FollowupCheck>> {
        // Data-driven resource-track accrual from the kernel's on_outcome rules
        // (e.g. Triangle pool_miss_count -> +N Chaos, threshold -> consequence).
        // Pure config: no ruleset/keyword branching. No-op for kernels without
        // on_outcome rules (e.g. cyberpunk), so existing paths are unaffected.
        self.apply_outcome_resource_tracks(contract, result).await;
        if self.is_effect_roll_check(contract) {
            let outcome = self.apply_effect_roll(contract, result).await?;
            result.committed_patches.extend(outcome.patches);
            result.committed_patches.push(StatePatch::CreateFact {
                target: outcome.effect.target_refs.first().map(|t| t.target_id.clone()).unwrap_or_else(|| "unknown_target".into()),
                fact: json!({
                    "effect_resolution_id": outcome.effect.effect_resolution_id,
                    "impact_count": outcome.effect.impacts.len(),
                    "damage_packet_id": outcome.damage.as_ref().map(|d| d.damage_packet_id.clone())
                }),
                reason: "effect_resolution_applied".into(),
            });
            return Ok(None);
        }

        if self.is_attack_check(contract) {
            let success = result.outcome.get("success").and_then(|v| v.as_bool()).unwrap_or_else(|| {
                match (result.outcome.get("total").and_then(|v| v.as_i64()), result.outcome.get("target").and_then(|v| v.as_i64())) {
                    (Some(total), Some(target)) => total >= target,
                    _ => false,
                }
            });
            let mut attack = self.record_attack(contract, result, success).await?;
            result.committed_patches.push(StatePatch::CreateFact {
                target: contract.initiator.actor_id.clone(),
                fact: json!({"attack_id": attack.attack_id, "hit_result": attack.hit_result, "target_actor_id": attack.target_actor_id}),
                reason: "attack_resolution_recorded".into(),
            });
            if success {
                let Some(damage_expr) = self.damage_expression_for_attack(contract).await else {
                    result.committed_patches.push(StatePatch::CreateFact {
                        target: attack.target_actor_id.clone().unwrap_or_else(|| "unknown_target".into()),
                        fact: json!({"attack_id": attack.attack_id, "blocked_missing_damage_expression": true, "source_policy": "source_backed_only_no_synthetic_defaults"}),
                        reason: "damage_expression_missing_source_backed_binding".into(),
                    });
                    return Ok(None);
                };
                let damage_check = self.make_effect_roll_check(contract, &attack, &damage_expr);
                let pending = PendingCheck {
                    check_id: damage_check.check_id.clone(),
                    session_id: damage_check.session_id.clone(),
                    expected_input_kind: "roll_result".into(),
                    prompt_public: "[system]效果骰已准备好。若本桌启用玩家确认投骰，请只回复 `roll`；系统会调用骰子工具并根据武器/能力/规则绑定应用到 HP、SAN、Chaos、Harm、条件或其他参数。[/system]".into(),
                    contract: damage_check.clone(),
                    expires_at: None,
                    status: PendingCheckStatus::Open,
                    interaction_context_id: None,
                    owner_frame_id: attack.frame_id.clone(),
                    gate_id: None,
                    generation: 0,
                    superseded_reason: None,
                    closed_at_tick: None,
                    created_at: Utc::now(),
                };
                self.db.insert_check_contract(&damage_check, "created").await.ok();
                self.db.insert_pending_check(&pending).await.ok();
                self.db.insert_interaction_gate(&InteractionGate::from_pending_check(&pending)).await.ok();
                let damage_roll_visibility = damage_check.roll_visibility;
                attack.damage_roll_request = Some(EffectRollRequest { label: "effect roll".into(), dice_expression: damage_expr.clone(), roll_visibility: damage_roll_visibility, target_actor_id: attack.target_actor_id.clone() });
                attack.damage_profile_id = Some(format!("effect_profile_{}", damage_check.check_id));
                attack.damage_packet_id = None;
                self.insert_attack_resolution(&attack).await.ok();
                let prompt = "[system]命中后的效果骰已准备好。若本桌启用玩家确认投骰，请只回复 `roll`；不要自行给出点数、伤害或目标剩余值。[/system]".to_string();
                return Ok(Some(FollowupCheck { pending, prompt_public: prompt, reason: "awaiting_effect_roll".into(), attack: Some(attack) }));
            }
        }
        Ok(None)
    }

    pub async fn mechanical_ledger_context_block(&self, session_id: &str, world_tick: i64) -> Result<ContextBlock> {
        let rows = sqlx::query("select actor_id, actor_kind, hp_current, hp_max, armor_current, wound_state, morale, resources_json, conditions_json, provisional_reason, world_tick from actor_mechanical_states where session_id = $1 order by updated_at desc limit 16")
            .bind(session_id).fetch_all(&self.db.pool).await?;
        let mut actors: Vec<Value> = rows.into_iter().map(|r| json!({
            "actor_id": r.get::<String,_>("actor_id"),
            "actor_kind": r.get::<String,_>("actor_kind"),
            "hp_current": r.get::<Option<i32>,_>("hp_current"),
            "hp_max": r.get::<Option<i32>,_>("hp_max"),
            "armor_current": r.get::<Option<i32>,_>("armor_current"),
            "wound_state": r.get::<Option<String>,_>("wound_state"),
            "morale": r.get::<Option<i32>,_>("morale"),
            "resources": r.get::<Value,_>("resources_json"),
            "conditions": r.get::<Value,_>("conditions_json"),
            "provisional_reason": r.get::<Option<String>,_>("provisional_reason"),
            "world_tick": r.get::<Option<i64>,_>("world_tick"),
        })).collect();
        // ams.hp_current is no longer the source of truth (resource current values
        // live in generic_parameter_states). Override each actor row's hp_current
        // with the live SSOT value via the actor's kernel HP track, so the GM sees
        // current HP. Kernels cached by ruleset to avoid reloading per actor.
        let mut kernel_cache: std::collections::HashMap<String, Option<RuleKernel>> = std::collections::HashMap::new();
        for a in actors.iter_mut() {
            let Some(actor_id) = a.get("actor_id").and_then(|v| v.as_str()).map(str::to_string) else { continue };
            let Some(rs) = self.actor_ruleset_id(session_id, &actor_id).await else { continue };
            let kernel = match kernel_cache.get(&rs) {
                Some(k) => k.clone(),
                None => { let k = self.db.load_rule_kernel(&rs).await.ok().flatten(); kernel_cache.insert(rs.clone(), k.clone()); k }
            };
            let Some(k) = kernel else { continue };
            let Some(hp_id) = trpg_model::hp_resource_track_id(&k.resource_tracks) else { continue };
            if let Some(live) = self.db.load_resource_current(session_id, &actor_id, &hp_id, &k).await {
                if let Some(obj) = a.as_object_mut() {
                    obj.insert("hp_current".into(), serde_json::json!(live));
                    obj.insert("hp_source".into(), serde_json::json!("generic_parameter_states_live"));
                }
            }
        }
        let effect_rows = sqlx::query("select effect_resolution_id, impacts_json, visibility, provisional_reason, world_tick from effect_resolution_packets where session_id = $1 order by created_at desc limit 12")
            .bind(session_id).fetch_all(&self.db.pool).await.unwrap_or_default();
        let effects: Vec<Value> = effect_rows.into_iter().map(|r| json!({
            "effect_resolution_id": r.get::<String,_>("effect_resolution_id"),
            "impacts": r.get::<Value,_>("impacts_json"),
            "visibility": r.get::<String,_>("visibility"),
            "provisional_reason": r.get::<Option<String>,_>("provisional_reason"),
            "world_tick": r.get::<Option<i64>,_>("world_tick"),
        })).collect();
        let generic_rows = sqlx::query("select target_kind, target_id, parameter_path, value_json, visibility, provisional_reason, world_tick from generic_parameter_states where session_id = $1 order by updated_at desc limit 20")
            .bind(session_id).fetch_all(&self.db.pool).await.unwrap_or_default();
        let generic_states: Vec<Value> = generic_rows.into_iter().map(|r| json!({
            "target_kind": r.get::<String,_>("target_kind"),
            "target_id": r.get::<String,_>("target_id"),
            "parameter_path": r.get::<String,_>("parameter_path"),
            "value": r.get::<Value,_>("value_json"),
            "visibility": r.get::<String,_>("visibility"),
            "provisional_reason": r.get::<Option<String>,_>("provisional_reason"),
            "world_tick": r.get::<Option<i64>,_>("world_tick"),
        })).collect();
        let facet_rows = sqlx::query("select execution_kind, target_kind, target_id, parameter_path, operation, status, output_json, provisional_reason, world_tick from parameter_facet_execution_runs where session_id = $1 order by created_at desc limit 16")
            .bind(session_id).fetch_all(&self.db.pool).await.unwrap_or_default();
        let facet_executions: Vec<Value> = facet_rows.into_iter().map(|r| json!({
            "execution_kind": r.get::<String,_>("execution_kind"),
            "target_kind": r.get::<String,_>("target_kind"),
            "target_id": r.get::<String,_>("target_id"),
            "parameter_path": r.get::<String,_>("parameter_path"),
            "operation": r.get::<String,_>("operation"),
            "status": r.get::<String,_>("status"),
            "output": r.get::<Value,_>("output_json"),
            "provisional_reason": r.get::<Option<String>,_>("provisional_reason"),
            "world_tick": r.get::<Option<i64>,_>("world_tick"),
        })).collect();
        let mut block = ContextBlock::new(
            format!("mechanical_ledger.recent.{}", session_id),
            BlockKind::MechanicalLedger,
            "Mechanical Ledger: HP / Resources / Effect Impacts / Combat State",
            BlockContent::Json(json!({
                "world_tick": world_tick,
                "policy": "The GM owns HP, SAN, Chaos, Harm, damage, target numbers, weapon/ability effects, and NPC state. Do not ask players for remaining HP/SAN/Chaos/Harm or rules-table values; verify claims against rules/tables or record table overrides.",
                "actor_mechanical_states": actors,
                "recent_effect_resolution_packets": effects,
                "generic_parameter_states": generic_states,
                "recent_parameter_facet_executions": facet_executions,
            })),
            Visibility::GmOnly,
            Stability::TurnDynamic,
            CacheZone::DynamicTail,
            Scope { scope_type: ScopeType::Session, scope_id: session_id.to_string() },
            220,
        );
        block.tags = vec!["mechanical_ledger".into(), "effect_resolution".into(), "parameter_impacts".into(), "parameter_facet_executor".into()];
        block.load_reason = Some("parameter_facet_executor".into());
        Ok(block)
    }

    async fn apply_effect_roll(&self, contract: &CheckContract, result: &CheckResultRecord) -> Result<EffectApplicationOutcome> {
        self.apply_effect_roll_with_decision(contract, result, None).await
    }

    async fn apply_effect_roll_with_decision(&self, contract: &CheckContract, result: &CheckResultRecord, decision_override: Option<FacetDecision>) -> Result<EffectApplicationOutcome> {
        let total = result.roll.result.get("total").and_then(|v| v.as_i64()).unwrap_or_default() as i32;
        let target_actor = contract.target_actor.as_ref().map(|a| a.actor_id.clone())
            .or_else(|| contract.actor_snapshot_ids.iter().find(|id| id.starts_with("npc.") || id.contains("opposition") || id.starts_with("pc.")).cloned())
            .unwrap_or_else(|| "npc.opposition".into());
        let source_actor = contract.metadata_value("source_actor_id").or_else(|| Some(contract.initiator.actor_id.clone()));
        let decision = match decision_override {
            Some(decision) => decision,
            None => self.resolve_facet_decision(contract, &target_actor).await?,
        };
        let effect_id = format!("effectres_{}", Uuid::new_v4().simple());
        let mut patches = Vec::new();
        let mut damage_packet = None;
        let mut impacts = Vec::new();
        let amount = total.max(0);

        match decision.target_kind {
            EffectTargetKind::Actor => {
                let actor_kind = if decision.target_id.starts_with("pc.") { ActorKind::PlayerCharacter } else { ActorKind::Npc };
                let state = self.ensure_actor_state(&contract.session_id, contract.module_id.as_deref(), &decision.target_id, actor_kind, None, contract.world_tick_hint()).await?;
                if decision.parameter_path == "hp.current" {
                    // HP current now lives in the single source of truth (generic_parameter_states
                    // via the kernel HP track), not actor_mechanical_states.hp_current. Armor
                    // (combat metadata) is still read from the ams row.
                    let kernel = self.db.load_rule_kernel(&contract.ruleset_id).await.ok().flatten();
                    let hp_id = kernel.as_ref().and_then(|k| trpg_model::hp_resource_track_id(&k.resource_tracks));
                    let from_hp = match (kernel.as_ref(), hp_id.as_ref()) {
                        (Some(k), Some(id)) => self.db.load_resource_current(&contract.session_id, &decision.target_id, id, k).await,
                        _ => state.hp_current.or(state.hp_max),
                    };
                    if let (Some(from_hp), Some(k), Some(id)) = (from_hp, kernel.as_ref(), hp_id.as_ref()) {
                        let (to_hp, sp_applied) = trpg_model::apply_armor_damage(from_hp, amount, state.armor_current, decision.operation);
                        let delta = to_hp - from_hp;
                        let cap = self.db.resource_cap(&contract.session_id, &decision.target_id, id, k).await;
                        self.db.write_resource_current(&contract.session_id, &decision.target_id, id, to_hp, cap, &merge_source_refs(&contract.source_refs, &decision.source_refs), Visibility::GmOnly, contract.world_tick_hint()).await?;
                        if to_hp <= 0 && actor_kind != ActorKind::PlayerCharacter {
                            self.close_active_frame_for_defeated_target(&contract.session_id, &decision.target_id, contract.world_tick_hint()).await.ok();
                        }
                        let impact = ParameterImpact {
                            impact_id: format!("impact_{}", Uuid::new_v4().simple()),
                            target_kind: EffectTargetKind::Actor,
                            target_id: decision.target_id.clone(),
                            parameter_path: "hp.current".into(),
                            operation: decision.operation,
                            value: json!(amount),
                            before: Some(json!(from_hp)),
                            after: Some(json!(to_hp)),
                            validator_status: decision.status.as_str().into(),
                            visibility: Visibility::GmOnly,
                            source_refs: merge_source_refs(&contract.source_refs, &decision.source_refs),
                            provisional_reason: decision.provisional_reason.clone(),
                        };
                        patches.push(StatePatch::ActorHpDelta { actor_id: decision.target_id.clone(), from: Some(from_hp), delta, to: Some(to_hp), reason: "parameter_facet_executor_hp_delta".into() });
                        impacts.push(impact);
                        let packet = DamagePacket {
                            damage_packet_id: format!("damage_{}", Uuid::new_v4().simple()),
                            effect_resolution_id: Some(effect_id.clone()),
                            session_id: contract.session_id.clone(),
                            turn_id: contract.turn_id.clone(),
                            frame_id: None,
                            source_actor_id: source_actor.clone(),
                            target_actor_id: decision.target_id.clone(),
                            source_object_id: contract.metadata_value("source_object_id"),
                            source_ability_id: contract.metadata_value("source_ability_id"),
                            contest_id: result.outcome.get("contest_id").and_then(|v| v.as_str()).map(str::to_string),
                            damage_expression: Some(contract.dice_expression.clone()),
                            rolled_total: total,
                            damage_type: contract.metadata_value("damage_type"),
                            armor_interaction: json!({"sp_applied": sp_applied, "rolled": amount, "after_armor": (amount - sp_applied).max(0), "policy": if state.armor_current.is_some() { "sp_subtracted_from_materialized_armor_v1162f" } else { "no_armor_materialized_full_damage_v1162f" }}),
                            final_hp_delta: delta,
                            from_hp: Some(from_hp),
                            to_hp: Some(to_hp),
                            source_refs: merge_source_refs(&contract.source_refs, &decision.source_refs),
                            provisional_reason: decision.provisional_reason.clone(),
                            world_tick: contract.world_tick_hint(),
                            created_at: Utc::now(),
                        };
                        self.insert_damage_packet(&packet).await?;
                        damage_packet = Some(packet);
                    } else {
                        let impact = ParameterImpact {
                            impact_id: format!("impact_{}", Uuid::new_v4().simple()),
                            target_kind: EffectTargetKind::Actor,
                            target_id: decision.target_id.clone(),
                            parameter_path: "hp.current".into(),
                            operation: decision.operation,
                            value: json!(amount),
                            before: None,
                            after: None,
                            validator_status: "blocked_missing_source_backed_hp".into(),
                            visibility: Visibility::GmOnly,
                            source_refs: merge_source_refs(&contract.source_refs, &decision.source_refs),
                            provisional_reason: Some("Damage roll recorded, but HP was not decremented because the target has no source-backed HP facet/stat block.".into()),
                        };
                        patches.push(StatePatch::CreateFact { target: decision.target_id.clone(), fact: json!({"blocked_hp_delta": true, "rolled_total": total, "parameter_path": "hp.current", "reason": "missing_source_backed_hp"}), reason: "parameter_facet_executor_blocked_missing_hp".into() });
                        impacts.push(impact);
                    }
                } else if matches!(decision.operation, ParameterOperation::AddCondition | ParameterOperation::RemoveCondition) || decision.parameter_path.starts_with("conditions") {
                    let mut conditions = state.conditions.clone();
                    let before = json!(conditions);
                    if matches!(decision.operation, ParameterOperation::RemoveCondition) {
                        conditions.retain(|c| c.get("path").and_then(|v| v.as_str()) != Some(decision.parameter_path.as_str()));
                    } else {
                        conditions.push(json!({
                            "path": decision.parameter_path,
                            "source": "parameter_facet_executor",
                            "severity": amount,
                            "provisional_reason": decision.provisional_reason.clone()
                        }));
                    }
                    self.update_actor_conditions(&contract.session_id, &decision.target_id, &conditions, contract.world_tick_hint()).await?;
                    let impact = ParameterImpact {
                        impact_id: format!("impact_{}", Uuid::new_v4().simple()),
                        target_kind: EffectTargetKind::Actor,
                        target_id: decision.target_id.clone(),
                        parameter_path: decision.parameter_path.clone(),
                        operation: decision.operation,
                        value: json!(amount),
                        before: Some(before),
                        after: Some(json!(conditions)),
                        validator_status: decision.status.as_str().into(),
                        visibility: Visibility::GmOnly,
                        source_refs: merge_source_refs(&contract.source_refs, &decision.source_refs),
                        provisional_reason: decision.provisional_reason.clone(),
                    };
                    patches.push(StatePatch::ModifyTrack { target: format!("{}.{}", decision.target_id, decision.parameter_path), amount, reason: "parameter_facet_executor_condition_delta".into() });
                    impacts.push(impact);
                } else {
                    // Non-HP actor resources also live in the SSOT, keyed by a resolved kernel track id.
                    let kernel = self.db.load_rule_kernel(&contract.ruleset_id).await.ok().flatten();
                    let track_id = kernel.as_ref().and_then(|k| trpg_model::resolve_resource_track_id(&decision.parameter_path, k));
                    if let (Some(k), Some(id)) = (kernel.as_ref(), track_id.as_ref()) {
                        let before = self.db.load_resource_current(&contract.session_id, &decision.target_id, id, k).await.unwrap_or(0);
                        let after = apply_i32_operation(before, amount, decision.operation);
                        let cap = self.db.resource_cap(&contract.session_id, &decision.target_id, id, k).await;
                        self.db.write_resource_current(&contract.session_id, &decision.target_id, id, after, cap, &merge_source_refs(&contract.source_refs, &decision.source_refs), Visibility::GmOnly, contract.world_tick_hint()).await?;
                        let impact = ParameterImpact {
                            impact_id: format!("impact_{}", Uuid::new_v4().simple()),
                            target_kind: EffectTargetKind::Actor,
                            target_id: decision.target_id.clone(),
                            parameter_path: decision.parameter_path.clone(),
                            operation: decision.operation,
                            value: json!(amount),
                            before: Some(json!(before)),
                            after: Some(json!(after)),
                            validator_status: decision.status.as_str().into(),
                            visibility: Visibility::GmOnly,
                            source_refs: merge_source_refs(&contract.source_refs, &decision.source_refs),
                            provisional_reason: decision.provisional_reason.clone(),
                        };
                        patches.push(StatePatch::ModifyTrack { target: format!("{}.{}", decision.target_id, decision.parameter_path), amount: after - before, reason: "parameter_facet_executor_resource_delta".into() });
                        impacts.push(impact);
                    } else {
                        patches.push(StatePatch::CreateFact { target: decision.target_id.clone(), fact: json!({"blocked_resource_delta": true, "parameter_path": decision.parameter_path, "reason": "unresolved_resource_track_id"}), reason: "parameter_facet_executor_blocked_unresolved_resource".into() });
                    }
                }
            }
            _ => {
                let before = self.load_generic_parameter_value(&contract.session_id, decision.target_kind, &decision.target_id, &decision.parameter_path).await?.and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let after = apply_i32_operation(before, amount, decision.operation);
                self.upsert_generic_parameter_state(&contract.session_id, decision.target_kind, &decision.target_id, &decision.parameter_path, json!(after), Visibility::GmOnly, &decision.source_refs, decision.provisional_reason.as_deref(), contract.world_tick_hint()).await?;
                let impact = ParameterImpact {
                    impact_id: format!("impact_{}", Uuid::new_v4().simple()),
                    target_kind: decision.target_kind,
                    target_id: decision.target_id.clone(),
                    parameter_path: decision.parameter_path.clone(),
                    operation: decision.operation,
                    value: json!(amount),
                    before: Some(json!(before)),
                    after: Some(json!(after)),
                    validator_status: decision.status.as_str().into(),
                    visibility: Visibility::GmOnly,
                    source_refs: merge_source_refs(&contract.source_refs, &decision.source_refs),
                    provisional_reason: decision.provisional_reason.clone(),
                };
                patches.push(StatePatch::ModifyTrack { target: format!("{}:{}:{}", decision.target_kind.as_str(), decision.target_id, decision.parameter_path), amount: after - before, reason: "parameter_facet_executor_generic_delta".into() });
                impacts.push(impact);
            }
        }

        let effect = EffectResolutionPacket {
            effect_resolution_id: effect_id,
            session_id: contract.session_id.clone(),
            turn_id: contract.turn_id.clone(),
            frame_id: None,
            source_actor_id: source_actor,
            source_object_id: contract.metadata_value("source_object_id"),
            source_ability_id: contract.metadata_value("source_ability_id"),
            source_event_id: None,
            target_refs: vec![TargetRef { target_kind: decision.target_kind, target_id: decision.target_id.clone(), label: Some("effect target".into()) }],
            roll_records: vec![result.roll.clone()],
            rule_binding_ids: vec![],
            parameter_facet_ids: decision.source_facet_binding_ids.clone(),
            impacts,
            visibility: Visibility::GmOnly,
            source_refs: merge_source_refs(&contract.source_refs, &decision.source_refs),
            provisional_reason: decision.provisional_reason.clone(),
            world_tick: contract.world_tick_hint(),
            created_at: Utc::now(),
        };
        self.insert_effect_resolution_packet(&effect).await?;
        for impact in &effect.impacts { self.insert_parameter_impact(&effect, impact).await?; }
        self.insert_facet_execution_run(contract, &decision, &effect, total).await.ok();
        Ok(EffectApplicationOutcome { effect, damage: damage_packet, patches })
    }

    async fn record_attack(&self, contract: &CheckContract, result: &CheckResultRecord, success: bool) -> Result<AttackResolutionContract> {
        let target = contract.target_actor.as_ref().map(|a| a.actor_id.clone()).unwrap_or_else(|| "npc.opposition".into());
        let hit_result = if success { "hit" } else { "miss" }.to_string();
        let attack = AttackResolutionContract {
            attack_id: format!("attack_{}", Uuid::new_v4().simple()),
            session_id: contract.session_id.clone(),
            turn_id: contract.turn_id.clone(),
            frame_id: None,
            source_actor_id: contract.initiator.actor_id.clone(),
            target_actor_id: Some(target.clone()),
            target_object_id: None,
            source_object_id: contract.metadata_value("source_object_id"),
            source_ability_id: contract.metadata_value("source_ability_id"),
            range_band: contract.metadata_value("range_band").or_else(|| Some("scene_range_provisional".into())),
            cover_state: contract.metadata_value("cover_state"),
            attack_check_id: Some(contract.check_id.clone()),
            contest_id: result.outcome.get("contest_id").and_then(|v| v.as_str()).map(str::to_string),
            hit_result,
            damage_profile_id: None,
            damage_roll_request: None,
            damage_packet_id: None,
            resulting_patches: result.committed_patches.clone(),
            source_refs: contract.source_refs.clone(),
            provisional_reason: Some("Attack resolution used current CheckContract; exact range/DV procedure should be upgraded by contest/source-pack bindings.".into()),
            world_tick: contract.world_tick_hint(),
            created_at: Utc::now(),
        };
        self.ensure_actor_state(&contract.session_id, contract.module_id.as_deref(), &target, ActorKind::Npc, None, attack.world_tick).await.ok();
        self.insert_attack_resolution(&attack).await?;
        Ok(attack)
    }

    fn make_effect_roll_check(&self, attack_contract: &CheckContract, attack: &AttackResolutionContract, dice_expression: &str) -> CheckContract {
        let mut check = attack_contract.clone();
        check.check_id = format!("check_effect_{}", Uuid::new_v4().simple());
        check.intent_kind = "effect_roll".into();
        check.check_label = "effect roll".into();
        check.dice_expression = dice_expression.into();
        if effect_roll_policy_system_visible() {
            check.roll_visibility = RollVisibility::PublicGmRoll;
            check.roll_authority = RollAuthority::System;
            check.disclosure = RollDisclosurePolicy::for_visibility(RollVisibility::PublicGmRoll);
        } else {
            check.roll_visibility = RollVisibility::PlayerRollRequired;
            check.roll_authority = RollAuthority::Player;
            check.disclosure = RollDisclosurePolicy::for_visibility(RollVisibility::PlayerRollRequired);
        }
        check.target = CheckTargetModel::UnknownUntilLookup;
        check.opposition = OppositionModel::NoMechanicalOpposition;
        check.target_actor = attack.target_actor_id.as_ref().map(|id| ActorRef { actor_id: id.clone(), actor_kind: ActorKind::Npc, display_name: Some("target".into()) });
        check.actor_snapshot_ids = vec![attack.target_actor_id.clone().unwrap_or_else(|| "npc.opposition".into())];
        check.stakes.before_roll_public = "Effect roll for the successful action. The GM will apply it to the correct tracked parameter (HP/SAN/Chaos/Harm/condition/object state) from rules and bound facets; do not ask the player for remaining values.".into();
        check.stakes.success_public = "The effect is applied to the target's mechanical ledger.".into();
        check.stakes.failure_public = "Effect roll could not be applied; the GM must clarify source/target rather than lose state.".into();
        check.advice_refs.push("unified_roll_effect_executor.effect_followup.v1_12_1".into());
        check
    }

    /// Read an actor's HP from runtime_actor_parameters.status_json (set by the
    /// created character / generic kernel seed) so the combat HP table can be
    /// seeded with a source-backed value. Returns hp_max (fallback hp_current).
    async fn actor_hp_from_params(&self, session_id: &str, actor_id: &str) -> Option<i32> {
        let row = sqlx::query("select coalesce(status_json->>'hp_max', status_json->>'hp_current') as hp from runtime_actor_parameters where session_id=$1 and actor_id=$2 limit 1")
            .bind(session_id).bind(actor_id).fetch_optional(&self.db.pool).await.ok().flatten()?;
        row.get::<Option<String>,_>("hp").and_then(|s| s.parse::<i32>().ok()).filter(|h| *h > 0)
    }

    /// The actor's ruleset_id from runtime_actor_parameters (so resource seeds can
    /// load the right kernel even when the caller didn't thread a ruleset in).
    async fn actor_ruleset_id(&self, session_id: &str, actor_id: &str) -> Option<String> {
        sqlx::query_scalar::<_, String>("select ruleset_id from runtime_actor_parameters where session_id=$1 and actor_id=$2 limit 1")
            .bind(session_id).bind(actor_id).fetch_optional(&self.db.pool).await.ok().flatten()
            .filter(|s| !s.trim().is_empty())
    }

    /// For each kernel resource_track, the actor's seed (current, max) derived from
    /// the character sheet (`runtime_actor_parameters.sheet_json.resources.{id}`,
    /// matched by id case-insensitively). current = max = the derived value (the
    /// derived MAX is also the starting current). Falls back to the track's static
    /// kernel `initial`/`max` when the char has no matching derived resource
    /// (fail-closed: never fabricates). Keyed by the kernel track id.
    async fn actor_resource_seeds(&self, session_id: &str, actor_id: &str, kernel: &RuleKernel) -> std::collections::HashMap<String, (Option<i32>, Option<i32>)> {
        self.db.resource_seeds(session_id, actor_id, kernel).await
    }

    async fn ensure_actor_state(&self, session_id: &str, frame_id: Option<&str>, actor_id: &str, actor_kind: ActorKind, default_hp: Option<i32>, world_tick: i64) -> Result<ActorMechanicalState> {
        let existing = sqlx::query("select state_id, session_id, frame_id, actor_id, actor_kind, hp_current, hp_max, armor_current, wound_state, morale, resources_json, conditions_json, source_refs, provisional_reason, world_tick, updated_at from actor_mechanical_states where session_id = $1 and actor_id = $2")
            .bind(session_id).bind(actor_id).fetch_optional(&self.db.pool).await?;
        if let Some(r) = existing { return Ok(row_to_actor_state(r)); }
        // Load the actor's kernel (via runtime_actor_parameters.ruleset_id, since
        // this fn isn't threaded a ruleset) so resources/HP seed from the
        // character's DERIVED resources matched by kernel track id — never a
        // hardcoded per-ruleset blob.
        let kernel = match self.actor_ruleset_id(session_id, actor_id).await {
            Some(rs) => self.db.load_rule_kernel(&rs).await.ok().flatten(),
            None => None,
        };
        let seeds = match &kernel {
            Some(k) => self.actor_resource_seeds(session_id, actor_id, k).await,
            None => std::collections::HashMap::new(),
        };
        // Resource CURRENT values now live in the SSOT (generic_parameter_states), not
        // actor_mechanical_states.resources_json. The ams row keeps only combat metadata
        // (armor/morale/conditions/frame); seed resources_json as empty.
        let resources = serde_json::json!({});
        // Connect the two HP stores: if no explicit HP was given, derive it from
        // the kernel HP track's character-derived current (semantic, not a
        // hardcoded field name), then fall back to runtime_actor_parameters
        // (created-character / kernel-seeded HP) so combat damage has a
        // source-backed value to decrement instead of blocking on
        // "missing_source_backed_hp".
        let hp_seed = kernel.as_ref()
            .and_then(|k| trpg_model::hp_resource_track_id(&k.resource_tracks))
            .and_then(|hp_id| seeds.get(&hp_id).and_then(|s| s.0));
        let default_hp = default_hp.or(hp_seed).or(self.actor_hp_from_params(session_id, actor_id).await);
        let state = ActorMechanicalState {
            state_id: format!("actor_mech_{}", Uuid::new_v4().simple()),
            session_id: session_id.into(),
            frame_id: frame_id.map(str::to_string),
            actor_id: actor_id.into(),
            actor_kind,
            hp_current: default_hp,
            hp_max: default_hp,
            armor_current: None,
            wound_state: Some(if default_hp.is_some() { "unhurt" } else { "unknown" }.into()),
            morale: Some(if actor_kind == ActorKind::PlayerCharacter { 100 } else { 55 }),
            resources,
            conditions: vec![],
            source_refs: vec![],
            provisional_reason: Some(if default_hp.is_some() { "v1.12.1 provisional mechanical state; replace with module/rules source pack when available.".into() } else { "Unbound mechanical state; HP/resource values require source-backed materialization before damage can be decremented.".into() }),
            world_tick,
            updated_at: Utc::now(),
        };
        self.upsert_actor_state(&state).await?;
        Ok(state)
    }

    async fn upsert_actor_state(&self, state: &ActorMechanicalState) -> Result<()> {
        sqlx::query(r#"
            insert into actor_mechanical_states
              (id, state_id, session_id, frame_id, actor_id, actor_kind, hp_current, hp_max, armor_current, wound_state, morale, resources_json, conditions_json, source_refs, provisional_reason, world_tick, updated_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)
            on conflict (session_id, actor_id) do update set
              frame_id = coalesce(excluded.frame_id, actor_mechanical_states.frame_id),
              hp_current = excluded.hp_current,
              hp_max = excluded.hp_max,
              armor_current = excluded.armor_current,
              wound_state = excluded.wound_state,
              morale = excluded.morale,
              resources_json = excluded.resources_json,
              conditions_json = excluded.conditions_json,
              source_refs = excluded.source_refs,
              provisional_reason = excluded.provisional_reason,
              world_tick = excluded.world_tick,
              updated_at = excluded.updated_at
            "#)
            .bind(Uuid::new_v4()).bind(&state.state_id).bind(&state.session_id).bind(&state.frame_id).bind(&state.actor_id).bind(state.actor_kind.as_str())
            .bind(state.hp_current).bind(state.hp_max).bind(state.armor_current).bind(&state.wound_state).bind(state.morale)
            .bind(&state.resources).bind(serde_json::to_value(&state.conditions)?).bind(serde_json::to_value(&state.source_refs)?).bind(&state.provisional_reason).bind(state.world_tick).bind(state.updated_at)
            .execute(&self.db.pool).await?;
        Ok(())
    }

    async fn close_active_frame_for_defeated_target(&self, session_id: &str, actor_id: &str, world_tick: i64) -> Result<()> {
        let Some(mut frame) = self.db.list_active_state_frames(session_id, 1).await?.into_iter().next() else {
            return Ok(());
        };
        frame.status = FrameStatus::Completed;
        frame.updated_at = Utc::now();
        if let Some(obj) = frame.working_state.as_object_mut() {
            obj.insert("auto_closed_by".into(), json!("parameter_facet_executor"));
            obj.insert("frame_outcome".into(), json!("opposition_defeated"));
            obj.insert("defeated_actor_id".into(), json!(actor_id));
            obj.insert("world_tick".into(), json!(world_tick));
        } else {
            frame.working_state = json!({
                "auto_closed_by": "parameter_facet_executor",
                "frame_outcome": "opposition_defeated",
                "defeated_actor_id": actor_id,
                "world_tick": world_tick
            });
        }
        frame.local_facts.push(FrameFact {
            fact_id: format!("fact_frame_closed_{}", Uuid::new_v4().simple()),
            summary: format!("{} was defeated; active frame auto-closed.", actor_id),
            visibility: Visibility::GmOnly,
            importance: 70,
            tags: vec!["frame_exit".into(), "opposition_defeated".into()],
        });
        self.db.upsert_state_frame(&frame).await?;
        Ok(())
    }

    async fn update_actor_conditions(&self, session_id: &str, actor_id: &str, conditions: &[Value], world_tick: i64) -> Result<()> {
        sqlx::query("update actor_mechanical_states set conditions_json = $3, world_tick = $4, updated_at = now() where session_id = $1 and actor_id = $2")
            .bind(session_id).bind(actor_id).bind(serde_json::to_value(conditions)?).bind(world_tick).execute(&self.db.pool).await?;
        Ok(())
    }

    async fn insert_attack_resolution(&self, attack: &AttackResolutionContract) -> Result<()> {
        sqlx::query(r#"
            insert into attack_resolution_contracts
              (id, attack_id, session_id, turn_id, frame_id, source_actor_id, target_actor_id, target_object_id, source_object_id, source_ability_id, check_id, contest_id, hit_result, damage_check_id, attack_json, world_tick, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)
            on conflict (attack_id) do update set attack_json = excluded.attack_json, damage_check_id = excluded.damage_check_id, hit_result = excluded.hit_result, contest_id = excluded.contest_id
            "#)
            .bind(Uuid::new_v4()).bind(&attack.attack_id).bind(&attack.session_id).bind(&attack.turn_id).bind(&attack.frame_id)
            .bind(&attack.source_actor_id).bind(&attack.target_actor_id).bind(&attack.target_object_id).bind(&attack.source_object_id).bind(&attack.source_ability_id)
            .bind(&attack.attack_check_id).bind(&attack.contest_id).bind(&attack.hit_result).bind(&attack.damage_profile_id).bind(serde_json::to_value(attack)?).bind(attack.world_tick).bind(attack.created_at)
            .execute(&self.db.pool).await?;
        Ok(())
    }

    async fn insert_damage_packet(&self, packet: &DamagePacket) -> Result<()> {
        sqlx::query(r#"
            insert into damage_packets
              (id, damage_packet_id, effect_resolution_id, session_id, turn_id, frame_id, source_actor_id, target_actor_id, source_object_id, source_ability_id, contest_id, damage_expression, rolled_total, damage_type, armor_interaction_json, final_hp_delta, from_hp, to_hp, source_refs, provisional_reason, damage_json, world_tick, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23)
            "#)
            .bind(Uuid::new_v4()).bind(&packet.damage_packet_id).bind(&packet.effect_resolution_id).bind(&packet.session_id).bind(&packet.turn_id).bind(&packet.frame_id)
            .bind(&packet.source_actor_id).bind(&packet.target_actor_id).bind(&packet.source_object_id).bind(&packet.source_ability_id).bind(&packet.contest_id)
            .bind(&packet.damage_expression).bind(packet.rolled_total).bind(&packet.damage_type).bind(&packet.armor_interaction).bind(packet.final_hp_delta).bind(packet.from_hp).bind(packet.to_hp)
            .bind(serde_json::to_value(&packet.source_refs)?).bind(&packet.provisional_reason).bind(serde_json::to_value(packet)?).bind(packet.world_tick).bind(packet.created_at)
            .execute(&self.db.pool).await?;
        Ok(())
    }

    async fn insert_effect_resolution_packet(&self, packet: &EffectResolutionPacket) -> Result<()> {
        sqlx::query(r#"
            insert into effect_resolution_packets
              (id, effect_resolution_id, session_id, turn_id, frame_id, source_actor_id, source_object_id, source_ability_id, source_event_id, target_refs, roll_records, rule_binding_ids, parameter_facet_ids, impacts_json, visibility, source_refs, provisional_reason, packet_json, world_tick, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20)
            on conflict (effect_resolution_id) do update set packet_json = excluded.packet_json, impacts_json = excluded.impacts_json
            "#)
            .bind(Uuid::new_v4()).bind(&packet.effect_resolution_id).bind(&packet.session_id).bind(&packet.turn_id).bind(&packet.frame_id)
            .bind(&packet.source_actor_id).bind(&packet.source_object_id).bind(&packet.source_ability_id).bind(&packet.source_event_id)
            .bind(serde_json::to_value(&packet.target_refs)?).bind(serde_json::to_value(&packet.roll_records)?).bind(serde_json::to_value(&packet.rule_binding_ids)?).bind(serde_json::to_value(&packet.parameter_facet_ids)?).bind(serde_json::to_value(&packet.impacts)?).bind(packet.visibility.as_str()).bind(serde_json::to_value(&packet.source_refs)?).bind(&packet.provisional_reason).bind(serde_json::to_value(packet)?).bind(packet.world_tick).bind(packet.created_at)
            .execute(&self.db.pool).await?;
        Ok(())
    }

    async fn insert_parameter_impact(&self, packet: &EffectResolutionPacket, impact: &ParameterImpact) -> Result<()> {
        sqlx::query(r#"
            insert into parameter_impacts
              (id, impact_id, effect_resolution_id, session_id, turn_id, target_kind, target_id, parameter_path, operation, value_json, before_json, after_json, validator_status, visibility, source_refs, provisional_reason)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)
            on conflict (impact_id) do nothing
            "#)
            .bind(Uuid::new_v4()).bind(&impact.impact_id).bind(&packet.effect_resolution_id).bind(&packet.session_id).bind(&packet.turn_id)
            .bind(impact.target_kind.as_str()).bind(&impact.target_id).bind(&impact.parameter_path).bind(impact.operation.as_str()).bind(&impact.value).bind(&impact.before).bind(&impact.after).bind(&impact.validator_status).bind(impact.visibility.as_str()).bind(serde_json::to_value(&impact.source_refs)?).bind(&impact.provisional_reason)
            .execute(&self.db.pool).await?;
        Ok(())
    }

    fn is_attack_check(&self, contract: &CheckContract) -> bool {
        let k = contract.intent_kind.to_ascii_lowercase();
        let label = contract.check_label.to_ascii_lowercase();
        matches!(k.as_str(), "attack" | "counterattack") || label.contains("attack") || label.contains("combat") || label.contains("fire") || label.contains("开火") || label.contains("攻击")
    }
    fn is_effect_roll_check(&self, contract: &CheckContract) -> bool { matches!(contract.intent_kind.as_str(), "damage_roll" | "effect_roll" | "sanity_roll" | "chaos_roll" | "harm_roll" | "resource_loss") || contract.check_label.to_ascii_lowercase().contains("damage") || contract.check_label.contains("伤害") || contract.check_label.to_ascii_lowercase().contains("effect") }

    async fn damage_expression_for_attack(&self, contract: &CheckContract) -> Option<String> {
        if let Some(expr) = contract.metadata_value("damage_expression") { return Some(expr); }
        // Source-backed weapon damage from the formula pack (step-2): a
        // DerivedValue with field_id starting "weapon_damage" whose `formula` is
        // a plain dice expression (e.g. "3d6" for a Heavy Pistol). Pack-driven
        // so no synthetic constant is fabricated when the weapon is unbound.
        if let Ok(Some(pack)) = self.db.load_character_onboarding_pack(&contract.ruleset_id).await {
            if let Some(f) = pack.derived_formula_pack.formulas.iter()
                .find(|f| f.field_id.to_ascii_lowercase().starts_with("weapon_damage"))
            {
                let dice = f.formula.trim();
                if is_plain_dice(dice) { return Some(dice.to_string()); }
            }
        }
        if contract.ruleset_id.contains("cyberpunk") { return std::env::var("TRPG_COMBAT_DEFAULT_CYBERPUNK_DAMAGE").ok(); }
        std::env::var("TRPG_COMBAT_DEFAULT_DAMAGE_EXPR").ok()
    }

    async fn resolve_facet_decision(&self, contract: &CheckContract, default_actor_id: &str) -> Result<FacetDecision> {
        if let Some(decision) = self.lookup_bound_facet_decision(contract, default_actor_id).await? {
            return Ok(decision);
        }
        Ok(self.ruleset_fallback_facet_decision(contract, default_actor_id).await)
    }

    async fn lookup_bound_facet_decision(&self, contract: &CheckContract, default_actor_id: &str) -> Result<Option<FacetDecision>> {
        let rows = sqlx::query(r#"
            select facet_binding_id, target_kind, target_id, facet_kind, facet_json, source_refs, verification_status
            from parameter_facet_bindings
            where session_id = $1
              and facet_kind in ('effect_damage_binding','effect_resource_binding','effect_condition_binding','ability_effect_facet','object_damage_facet','actor_resource_track','visibility_binding')
            order by created_at desc
            limit 24
        "#).bind(&contract.session_id).fetch_all(&self.db.pool).await.unwrap_or_default();
        for r in rows {
            let facet_json: Value = r.get("facet_json");
            let path = facet_json.get("target_parameter")
                .or_else(|| facet_json.get("parameter_path"))
                .or_else(|| facet_json.get("effect_target_path"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let Some(parameter_path) = path else { continue; };
            let op = facet_json.get("operation").and_then(|v| v.as_str()).and_then(parse_parameter_operation).unwrap_or_else(|| default_operation_for_path(&parameter_path, &contract.ruleset_id));
            let target_kind = facet_json.get("target_kind").and_then(|v| v.as_str()).and_then(parse_effect_target_kind)
                .unwrap_or_else(|| default_target_kind_for_path(&parameter_path, &contract.ruleset_id));
            let target_id = facet_json.get("target_id").and_then(|v| v.as_str()).map(str::to_string).unwrap_or_else(|| match target_kind {
                EffectTargetKind::Actor => default_actor_id.to_string(),
                EffectTargetKind::Scene => "scene.current".into(),
                EffectTargetKind::Anomaly => "anomaly.current".into(),
                EffectTargetKind::Clock => "clock.current".into(),
                EffectTargetKind::Campaign => "campaign.current".into(),
                EffectTargetKind::Object => contract.metadata_value("target_object_id").unwrap_or_else(|| "object.current".into()),
                EffectTargetKind::Relationship => "relationship.current".into(),
                EffectTargetKind::World => "world.current".into(),
            });
            let source_refs: Vec<SourceRef> = serde_json::from_value(r.get("source_refs")).unwrap_or_default();
            let verification_status: String = r.get("verification_status");
            let status = if verification_status.contains("verified") { FacetExecutionStatus::Applied } else { FacetExecutionStatus::AppliedProvisional };
            return Ok(Some(FacetDecision {
                target_kind,
                target_id,
                parameter_path,
                operation: op,
                source_facet_binding_ids: vec![r.get::<String,_>("facet_binding_id")],
                source_refs,
                status,
                provisional_reason: Some("Applied parameter facet binding selected from parameter_facet_bindings.".into()),
            }));
        }
        Ok(None)
    }

    async fn ruleset_fallback_facet_decision(&self, _contract: &CheckContract, default_actor_id: &str) -> FacetDecision {
        // De-hardcoded: NO per-ruleset / keyword routing (chaos/sanity/harm/anomaly
        // were game-specific guesses). Resource-track effects now flow data-driven
        // via the kernel's resource_tracks.on_outcome rules (apply_outcome_resource_tracks),
        // and source-bound facets route through lookup_bound_facet_decision. When
        // neither applies, the only honest structural default for an unbound
        // physical effect is hp.current; richer effects require a bound facet.
        FacetDecision::hp(default_actor_id.into(), "No bound effect facet and no kernel resource rule matched; defaulted this physical effect to hp.current. Bind a source-backed facet or add a kernel resource_tracks rule to route it precisely.")
    }

    async fn load_generic_parameter_value(&self, session_id: &str, target_kind: EffectTargetKind, target_id: &str, parameter_path: &str) -> Result<Option<Value>> {
        let row = sqlx::query("select value_json from generic_parameter_states where session_id = $1 and target_kind = $2 and target_id = $3 and parameter_path = $4")
            .bind(session_id).bind(target_kind.as_str()).bind(target_id).bind(parameter_path).fetch_optional(&self.db.pool).await?;
        Ok(row.map(|r| r.get::<Value,_>("value_json")))
    }

    async fn upsert_generic_parameter_state(&self, session_id: &str, target_kind: EffectTargetKind, target_id: &str, parameter_path: &str, value: Value, visibility: Visibility, source_refs: &[SourceRef], provisional_reason: Option<&str>, world_tick: i64) -> Result<()> {
        sqlx::query(r#"
            insert into generic_parameter_states
              (id, state_id, session_id, target_kind, target_id, parameter_path, value_json, visibility, source_refs, provisional_reason, world_tick, updated_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,now())
            on conflict (session_id, target_kind, target_id, parameter_path) do update set
              value_json = excluded.value_json,
              visibility = excluded.visibility,
              source_refs = excluded.source_refs,
              provisional_reason = excluded.provisional_reason,
              world_tick = excluded.world_tick,
              updated_at = now()
        "#)
            .bind(Uuid::new_v4()).bind(format!("generic_state_{}", Uuid::new_v4().simple())).bind(session_id).bind(target_kind.as_str()).bind(target_id).bind(parameter_path)
            .bind(value).bind(visibility.as_str()).bind(serde_json::to_value(source_refs)?).bind(provisional_reason).bind(world_tick)
            .execute(&self.db.pool).await?;
        Ok(())
    }

    async fn insert_facet_execution_run(&self, contract: &CheckContract, decision: &FacetDecision, effect: &EffectResolutionPacket, roll_total: i32) -> Result<()> {
        let exec = ParameterFacetExecution {
            execution_id: format!("facetexec_{}", Uuid::new_v4().simple()),
            session_id: contract.session_id.clone(),
            turn_id: Some(contract.turn_id.clone()),
            frame_id: None,
            execution_kind: FacetExecutionKind::EffectImpact,
            source_facet_binding_ids: decision.source_facet_binding_ids.clone(),
            ruleset_id: contract.ruleset_id.clone(),
            target_kind: decision.target_kind,
            target_id: decision.target_id.clone(),
            parameter_path: decision.parameter_path.clone(),
            operation: decision.operation,
            input_json: json!({"roll_total": roll_total, "check_id": contract.check_id, "effect_resolution_id": effect.effect_resolution_id}),
            output_json: json!({"impact_count": effect.impacts.len(), "impacts": effect.impacts}),
            status: decision.status,
            visibility: Visibility::GmOnly,
            source_refs: merge_source_refs(&contract.source_refs, &decision.source_refs),
            provisional_reason: decision.provisional_reason.clone(),
            world_tick: Some(contract.world_tick_hint()),
            created_at: Utc::now(),
        };
        sqlx::query(r#"
            insert into parameter_facet_execution_runs
              (id, execution_id, session_id, turn_id, frame_id, execution_kind, source_facet_binding_ids, ruleset_id, target_kind, target_id, parameter_path, operation, input_json, output_json, status, visibility, source_refs, provisional_reason, world_tick, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20)
            on conflict (execution_id) do nothing
        "#)
            .bind(Uuid::new_v4()).bind(&exec.execution_id).bind(&exec.session_id).bind(&exec.turn_id).bind(&exec.frame_id).bind(exec.execution_kind.as_str()).bind(serde_json::to_value(&exec.source_facet_binding_ids)?).bind(&exec.ruleset_id).bind(exec.target_kind.as_str()).bind(&exec.target_id).bind(&exec.parameter_path).bind(exec.operation.as_str()).bind(&exec.input_json).bind(&exec.output_json).bind(exec.status.as_str()).bind(exec.visibility.as_str()).bind(serde_json::to_value(&exec.source_refs)?).bind(&exec.provisional_reason).bind(exec.world_tick).bind(exec.created_at)
            .execute(&self.db.pool).await?;
        Ok(())
    }

}

fn effect_roll_policy_system_visible() -> bool {
    let v = std::env::var("TRPG_AGENT_TABLE_DICE_POLICY").unwrap_or_else(|_| "system_rolls_visible".into());
    matches!(v.to_ascii_lowercase().as_str(), "system_rolls_visible" | "system" | "gm_rolls_visible" | "auto" | "auto_visible")
}

pub fn make_roll_plan_from_check(session_id: &str, turn_id: &str, check_id: Option<&str>, contract: &CheckContract, expression: &str, result_json: &Value) -> RollPlan {
    let lower = format!("{} {}", contract.intent_kind, contract.check_label).to_ascii_lowercase();
    let roll_kind = if lower.contains("damage") { RollKind::Damage }
        else if lower.contains("effect") { RollKind::EffectRoll }
        else if lower.contains("san") { RollKind::Sanity }
        else if lower.contains("chaos") { RollKind::Chaos }
        else if lower.contains("harm") { RollKind::Harm }
        else if lower.contains("save") || lower.contains("豁免") { RollKind::SavingThrow }
        else if lower.contains("percent") || lower.contains("d100") { RollKind::PercentileCheck }
        else if lower.contains("attack") || lower.contains("combat") || lower.contains("fire") || lower.contains("开火") || lower.contains("攻击") { RollKind::Attack }
        else { RollKind::SkillCheck };
    RollPlan {
        roll_plan_id: format!("rollplan_{}", Uuid::new_v4().simple()),
        session_id: session_id.into(),
        turn_id: turn_id.into(),
        frame_id: None,
        roll_kind,
        actor_id: Some(contract.initiator.actor_id.clone()),
        target_refs: contract.target_actor.as_ref().map(|a| vec![TargetRef { target_kind: EffectTargetKind::Actor, target_id: a.actor_id.clone(), label: a.display_name.clone() }]).unwrap_or_default(),
        dice_expression: expression.into(),
        parameters_used: json!({"check_id": check_id, "contract_expression": contract.dice_expression, "roll_result": result_json}),
        target_model: serde_json::to_value(&contract.target).unwrap_or_else(|_| json!({})),
        authority: contract.roll_authority,
        visibility: contract.roll_visibility,
        display_policy: json!({"show_formula": contract.disclosure.show_formula_to_player, "show_roll": contract.disclosure.show_roll_to_player, "show_dc": contract.disclosure.show_dc_to_player}),
        expected_effects: vec![contract.intent_kind.clone(), contract.check_label.clone()],
        source_refs: contract.source_refs.clone(),
        rule_binding_ids: vec![],
        parameter_facet_ids: vec![],
        world_tick: contract.world_tick_hint(),
        created_at: Utc::now(),
    }
}

pub async fn insert_roll_plan(db: &Db, plan: &RollPlan) -> Result<()> {
    sqlx::query(r#"
        insert into roll_plans
          (id, roll_plan_id, session_id, turn_id, frame_id, roll_kind, actor_id, target_refs, dice_expression, parameters_used, target_model, authority, visibility, display_policy, expected_effects, source_refs, rule_binding_ids, parameter_facet_ids, world_tick, created_at)
        values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20)
        on conflict (roll_plan_id) do nothing
        "#)
        .bind(Uuid::new_v4()).bind(&plan.roll_plan_id).bind(&plan.session_id).bind(&plan.turn_id).bind(&plan.frame_id).bind(plan.roll_kind.as_str()).bind(&plan.actor_id).bind(serde_json::to_value(&plan.target_refs)?).bind(&plan.dice_expression).bind(&plan.parameters_used).bind(&plan.target_model).bind(match plan.authority { RollAuthority::Player => "player", RollAuthority::GmAgent => "gm_agent", RollAuthority::System => "system" }).bind(plan.visibility.as_str()).bind(&plan.display_policy).bind(serde_json::to_value(&plan.expected_effects)?).bind(serde_json::to_value(&plan.source_refs)?).bind(serde_json::to_value(&plan.rule_binding_ids)?).bind(serde_json::to_value(&plan.parameter_facet_ids)?).bind(plan.world_tick).bind(plan.created_at)
        .execute(&db.pool).await?;
    Ok(())
}


/// True for a plain dice expression roll_dice can evaluate (NdM, NdM+K, NdM-K)
/// with NO named tokens. Used to accept source-backed weapon damage like "3d6"
/// and reject formula strings like "max(0, weapon_damage - SP)".
/// Roll a plain dice amount expression (NdM[+/-K]) and return the summed total.
/// Used by data-driven effect amounts (e.g. CoC Sanity loss "1d6"). Leaf-crate
/// roller to avoid a trpg-runtime dependency cycle.
fn roll_amount_dice(expr: &str) -> i32 {
    use rand::Rng;
    let s = expr.trim().to_ascii_lowercase().replace(' ', "");
    let dpos = match s.find('d') { Some(p) => p, None => return s.parse().unwrap_or(0) };
    let (dice_part, modifier) = match s[dpos + 1..].find(|c| c == '+' || c == '-') {
        Some(rel) => { let split = dpos + 1 + rel; (&s[..split], s[split..].parse::<i32>().unwrap_or(0)) }
        None => (&s[..], 0),
    };
    let mut it = dice_part.splitn(2, 'd');
    let n = { let a = it.next().unwrap_or("1"); if a.is_empty() { 1 } else { a.parse::<i32>().unwrap_or(1) } }.clamp(1, 100);
    let m = it.next().unwrap_or("6").parse::<i32>().unwrap_or(6).clamp(1, 1000);
    let mut rng = rand::thread_rng();
    (0..n).map(|_| rng.gen_range(1..=m)).sum::<i32>() + modifier
}

fn is_plain_dice(s: &str) -> bool {
    let s = s.trim().to_ascii_lowercase();
    if !s.contains('d') { return false; }
    let core = s.split(['+', '-']).next().unwrap_or("");
    let mut it = core.splitn(2, 'd');
    let a = it.next().unwrap_or("");
    let b = it.next().unwrap_or("");
    let dice_ok = (a.is_empty() || a.chars().all(|c| c.is_ascii_digit()))
        && !b.is_empty()
        && b.chars().all(|c| c.is_ascii_digit());
    let only_dice_chars = s
        .chars()
        .all(|c| c.is_ascii_digit() || c == 'd' || c == '+' || c == '-' || c.is_whitespace());
    dice_ok && only_dice_chars
}

/// The text a resource-track `check_match` is scanned against. Includes the
/// check's DELIBERATE engine-set binding (`tested_parameter`) alongside its
/// intent/label and the player prose, so a check that was explicitly bound to a
/// track (e.g. a Sanity roll → tested_parameter=Sanity) reliably fires that
/// track's on_outcome even when the prose itself never names the parameter.
fn check_match_hay(intent_kind: &str, check_label: &str, action_summary: &str, tested: Option<&TestedParameter>) -> String {
    let tp = tested.map(|t| format!("{} {}", t.key, t.label)).unwrap_or_default();
    format!("{} {} {} {}", intent_kind, check_label, tp, action_summary).to_ascii_lowercase()
}

/// Whether `tested` (the check's deliberate parameter binding) names THIS track,
/// matched against the track's id/name and its `on_outcome[*].check_match`
/// aliases (multilingual, already authored). Used to scope a track's default
/// loss die to a check that actually rolled this parameter — never to an
/// unrelated check whose prose merely mentions the keyword.
fn track_matches_tested_parameter(tested: Option<&TestedParameter>, track: &Value) -> bool {
    let Some(tp) = tested else { return false };
    let needles: Vec<String> = [tp.key.as_str(), tp.label.as_str()]
        .iter().map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()).collect();
    if needles.is_empty() { return false; }
    let mut aliases: Vec<String> = Vec::new();
    for k in ["id", "name"] {
        if let Some(s) = track.get(k).and_then(|v| v.as_str()) {
            let s = s.trim().to_ascii_lowercase();
            if !s.is_empty() { aliases.push(s); }
        }
    }
    if let Some(arr) = track.get("on_outcome").and_then(|v| v.as_array()) {
        for r in arr {
            if let Some(cm) = r.get("check_match").and_then(|v| v.as_str()) {
                aliases.extend(cm.split('|').map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()));
            }
        }
    }
    needles.iter().any(|n| aliases.iter().any(|a| n == a || n.contains(a.as_str()) || a.contains(n.as_str())))
}

/// Resolve a resource-track on_outcome amount, fully data-driven:
///   - `amount` may be a dice expr (`1d6`), `=value` (the `when` outcome field),
///     `=<field>` (read `outcome.<field>` directly, e.g. `=sanity_loss`), or a
///     fixed int;
///   - legacy `delta` is honored unchanged;
///   - when `amount`'s referenced outcome field is ABSENT, fall back to the
///     rule's configurable `default_amount` die — but ONLY when this track is the
///     check's deliberately-tested parameter (`track_is_tested`), so a bare
///     "make a Sanity roll" loses the default SAN die while an unrelated check
///     never drains an untested resource.
/// Returns None when nothing resolves (caller skips the rule).
fn resolve_track_amount(
    amount: Option<&str>,
    delta: Option<&str>,
    when: Option<&str>,
    default_amount: Option<&str>,
    track_is_tested: bool,
    outcome: &Value,
) -> Option<i32> {
    let field_amount = |w: &str| -> Option<i32> {
        match outcome.get(w) {
            Some(v) if v.is_boolean() => Some(if v.as_bool().unwrap_or(false) { 1 } else { 0 }),
            Some(v) => v.as_i64().map(|x| x as i32),
            None => None,
        }
    };
    let read_expr = |expr: &str| -> Option<i32> {
        let e = expr.trim();
        if e == "=value" {
            when.and_then(&field_amount)
        } else if let Some(field) = e.strip_prefix('=') {
            field_amount(field)
        } else if is_plain_dice(e) {
            Some(roll_amount_dice(e))
        } else {
            e.parse::<i32>().ok()
        }
    };
    if let Some(a) = amount {
        match read_expr(a) {
            Some(x) => Some(x),
            None if track_is_tested => default_amount.and_then(|d| read_expr(d)),
            None => None,
        }
    } else if let Some(d) = delta {
        let dt = d.trim();
        if dt == "=value" {
            when.and_then(&field_amount)
        } else {
            Some(dt.parse::<i32>().unwrap_or_else(|_| when.and_then(&field_amount).unwrap_or(0)))
        }
    } else {
        when.and_then(&field_amount)
    }
}

fn parse_parameter_operation(s: &str) -> Option<ParameterOperation> {
    match s.to_ascii_lowercase().as_str() {
        "add" | "increase" | "increment" => Some(ParameterOperation::Add),
        "subtract" | "decrease" | "damage" | "loss" | "delta_negative" => Some(ParameterOperation::Subtract),
        "set" | "assign" => Some(ParameterOperation::Set),
        "add_condition" | "condition" => Some(ParameterOperation::AddCondition),
        "remove_condition" => Some(ParameterOperation::RemoveCondition),
        "advance_clock" | "tick" => Some(ParameterOperation::AdvanceClock),
        "mark_revealed" | "reveal" => Some(ParameterOperation::MarkRevealed),
        _ => None,
    }
}

fn parse_effect_target_kind(s: &str) -> Option<EffectTargetKind> {
    match s.to_ascii_lowercase().as_str() {
        "actor" | "character" | "npc" | "pc" => Some(EffectTargetKind::Actor),
        "object" | "item" | "device" => Some(EffectTargetKind::Object),
        "scene" | "location" => Some(EffectTargetKind::Scene),
        "clock" | "timer" => Some(EffectTargetKind::Clock),
        "relationship" | "npc_relation" => Some(EffectTargetKind::Relationship),
        "anomaly" => Some(EffectTargetKind::Anomaly),
        "campaign" | "mission" => Some(EffectTargetKind::Campaign),
        "world" => Some(EffectTargetKind::World),
        _ => None,
    }
}

fn default_target_kind_for_path(path: &str, _ruleset_id: &str) -> EffectTargetKind {
    // Structural inference from the parameter path prefix only — no per-ruleset branching.
    let p = path.to_ascii_lowercase();
    if p.starts_with("tracks.") || p.starts_with("scene.") { return EffectTargetKind::Scene; }
    if p.starts_with("anomaly") { return EffectTargetKind::Anomaly; }
    if p.starts_with("object.") { return EffectTargetKind::Object; }
    if p.starts_with("clock.") { return EffectTargetKind::Clock; }
    EffectTargetKind::Actor
}

fn default_operation_for_path(path: &str, _ruleset_id: &str) -> ParameterOperation {
    // Structural inference from the parameter path only — no per-ruleset branching.
    let p = path.to_ascii_lowercase();
    if p.starts_with("tracks.") || p.contains("clock") { return ParameterOperation::Add; }
    if p.contains("condition") || p.ends_with("major_wound") { return ParameterOperation::AddCondition; }
    if p.contains("state") || p.contains("control") { return ParameterOperation::Set; }
    ParameterOperation::Subtract
}

fn apply_i32_operation(before: i32, amount: i32, op: ParameterOperation) -> i32 {
    match op {
        ParameterOperation::Add | ParameterOperation::AdvanceClock => before.saturating_add(amount),
        ParameterOperation::Subtract => (before - amount).max(0),
        ParameterOperation::Set => amount.max(0),
        ParameterOperation::AddCondition | ParameterOperation::RemoveCondition | ParameterOperation::MarkRevealed => before,
    }
}

fn merge_source_refs(a: &[SourceRef], b: &[SourceRef]) -> Vec<SourceRef> {
    let mut out = a.to_vec();
    out.extend_from_slice(b);
    out
}

trait CheckContractExt {
    fn metadata_value(&self, key: &str) -> Option<String>;
    fn world_tick_hint(&self) -> i64;
}
impl CheckContractExt for CheckContract {
    fn metadata_value(&self, _key: &str) -> Option<String> { None }
    fn world_tick_hint(&self) -> i64 { 0 }
}

fn row_to_actor_state(r: sqlx::postgres::PgRow) -> ActorMechanicalState {
    let kind_str: String = r.get("actor_kind");
    let actor_kind = match kind_str.as_str() { "player_character" => ActorKind::PlayerCharacter, "environment" => ActorKind::Environment, "hazard" => ActorKind::Hazard, "system" => ActorKind::System, _ => ActorKind::Npc };
    ActorMechanicalState {
        state_id: r.get("state_id"), session_id: r.get("session_id"), frame_id: r.get("frame_id"), actor_id: r.get("actor_id"), actor_kind,
        hp_current: r.get("hp_current"), hp_max: r.get("hp_max"), armor_current: r.get("armor_current"), wound_state: r.get("wound_state"), morale: r.get("morale"),
        resources: r.get("resources_json"), conditions: serde_json::from_value(r.get("conditions_json")).unwrap_or_default(), source_refs: serde_json::from_value(r.get("source_refs")).unwrap_or_default(),
        provisional_reason: r.get("provisional_reason"), world_tick: r.get::<Option<i64>,_>("world_tick").unwrap_or_default(), updated_at: r.get("updated_at"),
    }
}


#[cfg(test)]
mod on_outcome_match_amount_tests {
    use super::{check_match_hay, resolve_track_amount, track_matches_tested_parameter};
    use serde_json::json;
    use trpg_model::TestedParameter;

    fn tp(key: &str) -> TestedParameter { TestedParameter { domain: None, key: key.into(), label: key.into() } }

    // D2: the on_outcome 干草堆必须含 tested_parameter (key+label) —— 这样
    // 刻意绑定到 sanity track 的 check 即使念白里没"sanity"也能被 check_match 命中。
    #[test]
    fn hay_includes_tested_parameter() {
        let hay = check_match_hay("named_parameter_check", "Sanity check", "/roll", Some(&tp("Sanity")));
        assert!(hay.contains("sanity"), "tested_parameter 应进入干草堆: {hay}");
    }

    #[test]
    fn hay_lowercased_and_joins_all_fields() {
        let hay = check_match_hay("INTENT", "Label", "Prose Here", None);
        assert_eq!(hay, "intent label  prose here");
    }

    // D3: tested_parameter 指向本 track → 是被刻意检定的参数。
    #[test]
    fn track_is_tested_when_parameter_matches_track_id() {
        let track = json!({"id":"sanity","name":"Sanity","on_outcome":[{"check_match":"sanity|san roll"}]});
        assert!(track_matches_tested_parameter(Some(&tp("Sanity")), &track));
    }

    #[test]
    fn track_is_tested_via_check_match_alias() {
        let track = json!({"id":"sanity","name":"Sanity","on_outcome":[{"check_match":"sanity|理智"}]});
        assert!(track_matches_tested_parameter(Some(&tp("理智")), &track));
    }

    #[test]
    fn unrelated_parameter_does_not_match_track() {
        let track = json!({"id":"sanity","name":"Sanity","on_outcome":[{"check_match":"sanity|san roll"}]});
        assert!(!track_matches_tested_parameter(Some(&tp("Spot Hidden")), &track));
    }

    #[test]
    fn no_tested_parameter_means_not_tested() {
        let track = json!({"id":"sanity","name":"Sanity"});
        assert!(!track_matches_tested_parameter(None, &track));
    }

    // D3: =<field> 读 outcome.<field>。
    #[test]
    fn equals_field_reads_named_outcome_field() {
        let outcome = json!({"sanity_loss": 4, "success": false});
        assert_eq!(resolve_track_amount(Some("=sanity_loss"), None, None, Some("1d6"), true, &outcome), Some(4));
    }

    // D3 核心: 字段缺失 + 该 track 是被测参数 → 回退到可配置默认骰。
    #[test]
    fn missing_field_falls_back_to_default_die_when_tested() {
        let outcome = json!({"success": false});
        let v = resolve_track_amount(Some("=sanity_loss"), None, None, Some("1d6"), true, &outcome);
        let v = v.expect("被测参数缺字段应回退默认骰");
        assert!((1..=6).contains(&v), "1d6 应在 1..=6: {v}");
    }

    // D3 护栏: 字段缺失但本 track 不是被测参数 → 不落 delta (防止念白里偶现关键词误扣)。
    #[test]
    fn missing_field_no_fallback_when_not_tested() {
        let outcome = json!({"success": false});
        assert_eq!(resolve_track_amount(Some("=sanity_loss"), None, None, Some("1d6"), false, &outcome), None);
    }

    // =value 仍读 `when` 指定的 outcome 字段 (向后兼容)。
    #[test]
    fn equals_value_reads_when_field() {
        let outcome = json!({"total": 3});
        assert_eq!(resolve_track_amount(Some("=value"), None, Some("total"), None, true, &outcome), Some(3));
    }

    // 普通骰 / 定值 / 裸 when 字段 三条旧路径不变。
    #[test]
    fn plain_dice_and_fixed_int_and_bare_when_field() {
        let outcome = json!({"success_count": 5});
        assert_eq!(resolve_track_amount(Some("1d1"), None, None, None, false, &outcome), Some(1));
        assert_eq!(resolve_track_amount(Some("2"), None, None, None, false, &outcome), Some(2));
        assert_eq!(resolve_track_amount(None, None, Some("success_count"), None, false, &outcome), Some(5));
    }

    // legacy delta 字段保持原语义。
    #[test]
    fn legacy_delta_field_preserved() {
        let outcome = json!({"success_count": 2});
        assert_eq!(resolve_track_amount(None, Some("=value"), Some("success_count"), None, false, &outcome), Some(2));
        assert_eq!(resolve_track_amount(None, Some("3"), None, None, false, &outcome), Some(3));
    }
}

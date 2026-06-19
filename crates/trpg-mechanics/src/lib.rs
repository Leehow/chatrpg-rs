pub mod direct_effect;
pub mod ledger_semantics;
pub mod watcher;
#[cfg(test)]
mod watcher_tests;
pub use ledger_semantics::*;

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
pub struct RefereeCombatService {
    pub db: Db,
}

impl RefereeCombatService {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Apply the rule kernel's `resource_tracks[*].on_outcome` rules to a
    /// resolved check's outcome JSON: map an outcome field (success_count,
    /// pool_miss_count, success, total, ...) to a delta on a scene/actor track,
    /// then fire edge-triggered threshold consequences. Entirely config-driven
    /// from the parsed kernel — no per-ruleset Rust, no keyword matching.
    async fn apply_outcome_resource_tracks(
        &self,
        contract: &CheckContract,
        result: &mut CheckResultRecord,
    ) {
        let kernel = match self.db.load_rule_kernel(&contract.ruleset_id).await {
            Ok(Some(k)) => k,
            _ => return,
        };
        // Seed/cap each track's current value from the actor's CHARACTER-DERIVED
        // resource (sheet_json.resources matched by kernel track id), falling back
        // to the kernel static initial/max. Loaded once and reused per track.
        let seeds = self
            .actor_resource_seeds(&contract.session_id, &contract.initiator.actor_id, &kernel)
            .await;
        // Contract-constant: the text every track's `check_match` is scanned
        // against, including the deliberate tested_parameter binding (D2).
        let hay = check_match_hay(
            &contract.intent_kind,
            &contract.check_label,
            &contract.action_summary,
            contract.tested_parameter.as_ref(),
        );
        for track in &kernel.resource_tracks {
            let id = match track
                .get("id")
                .and_then(|v| v.as_str())
                .or_else(|| track.get("name").and_then(|v| v.as_str()))
            {
                Some(s) if !s.trim().is_empty() => s.trim().to_string(),
                _ => continue,
            };
            // Whether this check deliberately rolled THIS track's parameter —
            // scopes the rule's default loss die (D3 fallback) to real rolls.
            let track_is_tested =
                track_matches_tested_parameter(contract.tested_parameter.as_ref(), track);
            let rules = match track.get("on_outcome").and_then(|v| v.as_array()) {
                Some(a) if !a.is_empty() => a,
                _ => continue,
            };
            let is_actor = track.get("owner_kind").and_then(|v| v.as_str()) == Some("actor");
            let target_kind = if is_actor {
                EffectTargetKind::Actor
            } else {
                EffectTargetKind::Scene
            };
            let target_id = if is_actor {
                contract.initiator.actor_id.clone()
            } else {
                "scene.current".to_string()
            };
            let path = if is_actor {
                format!("resources.{}.current", id)
            } else {
                format!("tracks.{}.current", id)
            };
            let max = track.get("max").and_then(|v| v.as_i64()).map(|n| n as i32);
            let initial = track
                .get("initial")
                .and_then(|v| v.as_i64())
                .map(|n| n as i32);
            for rule in rules {
                // Trigger gate (A5, three layers — see trigger_fires): a rule
                // fires on success/failure/always/on_tier (legacy strings,
                // unchanged), or on a structured outcome band
                // {kind:"band", band_id:"fumble"}; unrecognized triggers skip
                // the rule (fail-closed), a missing key stays "always".
                if !trigger_fires(rule, &result.outcome) {
                    continue;
                }
                // Scope gate (config-driven, not hardcoded): if the rule declares
                // `check_match`, it only fires when the check's intent/label contains
                // it. This scopes e.g. "Sanity loss" to sanity checks, while an
                // unscoped rule (Triangle Chaos) fires on every roll as intended.
                if let Some(needle) = rule.get("check_match").and_then(|v| v.as_str()) {
                    if !needle
                        .to_ascii_lowercase()
                        .split('|')
                        .any(|n| !n.trim().is_empty() && hay.contains(n.trim()))
                    {
                        continue;
                    }
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
                ) {
                    Some(x) => x,
                    None => continue,
                };
                if amount == 0
                    && matches!(op, ParameterOperation::Add | ParameterOperation::Subtract)
                {
                    continue;
                }
                // Seed `before` when no row exists yet: prefer the actor's
                // character-DERIVED current (so e.g. CoC Sanity starts at the
                // character's SAN, not the kernel static initial), then the kernel
                // `initial`.
                let before = if is_actor {
                    self.db
                        .load_resource_current(&contract.session_id, &target_id, &id, &kernel)
                        .await
                        .or_else(|| seeds.get(&id).and_then(|s| s.0))
                        .or(initial)
                        .unwrap_or(0)
                } else {
                    self.load_generic_parameter_value(
                        &contract.session_id,
                        target_kind,
                        &target_id,
                        &path,
                    )
                    .await
                    .ok()
                    .flatten()
                    .and_then(|v| v.as_i64())
                    .map(|n| n as i32)
                    .or(seeds.get(&id).and_then(|s| s.0))
                    .or(initial)
                    .unwrap_or(0)
                };
                let mut after = apply_i32_operation(before, amount, op);
                // Cap by the actor's character-DERIVED max first, then the kernel
                // static max.
                let cap = seeds.get(&id).and_then(|s| s.1).or(max);
                if let Some(m) = cap {
                    after = after.min(m);
                }
                if is_actor {
                    self.db
                        .write_resource_current(
                            &contract.session_id,
                            &target_id,
                            &id,
                            after,
                            cap,
                            &contract.source_refs,
                            Visibility::GmOnly,
                            0,
                        )
                        .await
                        .ok();
                } else {
                    self.upsert_generic_parameter_state(
                        &contract.session_id,
                        target_kind,
                        &target_id,
                        &path,
                        json!(after),
                        Visibility::GmOnly,
                        &contract.source_refs,
                        None,
                        0,
                    )
                    .await
                    .ok();
                }
                result.committed_patches.push(StatePatch::CreateFact {
                    target: target_id.clone(),
                    fact: json!({"resource_track": id, "parameter_path": path, "before": before, "after": after, "delta_applied": after - before, "amount_rolled": amount}),
                    reason: "resource_delta_applied".into(),
                });
                // 护栏 §3.5.1：规则经 check_match regex（字符串路径）命中、且契约
                // 未带 "mechanic:" 结构化引用 → 账本打可观测回退标注，绝不静默。
                if let Some(p) = check_match_fallback_fact(
                    rule,
                    &contract.advice_refs,
                    &target_id,
                    &id,
                    &contract.check_label,
                ) {
                    result.committed_patches.push(p);
                }
                // B4 watcher：阈值判定抽成纯函数单点（行为零变化，金样测试钉死）；
                // CreateFact 从 crossing 重建 + 新增 MechanicDue 落库（债务必清通路）。
                // owner_kind 取 track 字段原值（开放枚举，party/world 不丢），无则按
                // is_actor 二分回填。
                let owner_kind = track
                    .get("owner_kind")
                    .and_then(|v| v.as_str())
                    .unwrap_or(if is_actor { "actor" } else { "scene" });
                let crossings =
                    watcher::detect_crossings(track, owner_kind, &target_id, before, after, op);
                for c in &crossings {
                    result.committed_patches.push(StatePatch::CreateFact {
                        target: target_id.clone(),
                        fact: watcher::crossing_fact(c),
                        reason: "resource_threshold_consequence".into(),
                    });
                }
                if !crossings.is_empty() {
                    let _ = self
                        .detect_threshold_dues(contract, result, &kernel, &crossings)
                        .await;
                }
            }
        }
    }

    pub async fn after_check_resolved(
        &self,
        contract: &CheckContract,
        result: &mut CheckResultRecord,
    ) -> Result<Option<FollowupCheck>> {
        // Data-driven resource-track accrual from the kernel's on_outcome rules
        // (e.g. Triangle pool_miss_count -> +N Chaos, threshold -> consequence).
        // Pure config: no ruleset/keyword branching. No-op for kernels without
        // on_outcome rules (e.g. cyberpunk), so existing paths are unaffected.
        self.apply_outcome_resource_tracks(contract, result).await;
        if self.is_effect_roll_check(contract) {
            let outcome = self.apply_effect_roll(contract, result).await?;
            result.committed_patches.extend(outcome.patches);
            result.committed_patches.push(StatePatch::CreateFact {
                target: outcome
                    .effect
                    .target_refs
                    .first()
                    .map(|t| t.target_id.clone())
                    .unwrap_or_else(|| "unknown_target".into()),
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
            let success = result
                .outcome
                .get("success")
                .and_then(|v| v.as_bool())
                .unwrap_or_else(|| {
                    match (
                        result.outcome.get("total").and_then(|v| v.as_i64()),
                        result.outcome.get("target").and_then(|v| v.as_i64()),
                    ) {
                        (Some(total), Some(target)) => total >= target,
                        _ => false,
                    }
                });
            // No distinct target → record_attack returns None: skip attack
            // resolution entirely (no row, no follow-up) rather than fabricate
            // a target or persist a self-attack.
            let Some(mut attack) = self.record_attack(contract, result, success).await? else {
                return Ok(None);
            };
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
                self.db
                    .insert_check_contract(&damage_check, "created")
                    .await
                    .ok();
                self.db.insert_pending_check(&pending).await.ok();
                self.db
                    .insert_interaction_gate(&InteractionGate::from_pending_check(&pending))
                    .await
                    .ok();
                let damage_roll_visibility = damage_check.roll_visibility;
                attack.damage_roll_request = Some(EffectRollRequest {
                    label: "effect roll".into(),
                    dice_expression: damage_expr.clone(),
                    roll_visibility: damage_roll_visibility,
                    target_actor_id: attack.target_actor_id.clone(),
                });
                attack.damage_profile_id =
                    Some(format!("effect_profile_{}", damage_check.check_id));
                attack.damage_packet_id = None;
                self.insert_attack_resolution(&attack).await.ok();
                let prompt = "[system]命中后的效果骰已准备好。若本桌启用玩家确认投骰，请只回复 `roll`；不要自行给出点数、伤害或目标剩余值。[/system]".to_string();
                return Ok(Some(FollowupCheck {
                    pending,
                    prompt_public: prompt,
                    reason: "awaiting_effect_roll".into(),
                    attack: Some(attack),
                }));
            }
        }
        Ok(None)
    }

    /// B3 signature: `ruleset_id` is the session's ruleset — scene/party/world
    /// rows have no actor to resolve a ruleset from, and the semantic lines
    /// need a kernel. Loaded once and reused; the per-actor kernel cache below
    /// stays (multi-ruleset actor sessions remain correct), the new parameter
    /// only serves non-actor rows and actors missing a ruleset mapping.
    pub async fn mechanical_ledger_context_block(
        &self,
        session_id: &str,
        ruleset_id: &str,
        world_tick: i64,
    ) -> Result<ContextBlock> {
        let session_kernel = self.db.load_rule_kernel(ruleset_id).await.ok().flatten();
        let rows = sqlx::query("select actor_id, actor_kind, hp_current, hp_max, armor_current, wound_state, morale, resources_json, conditions_json, provisional_reason, world_tick from actor_mechanical_states where session_id = $1 order by updated_at desc limit 16")
            .bind(session_id).fetch_all(&self.db.pool).await?;
        let mut actors: Vec<Value> = rows
            .into_iter()
            .map(|r| {
                json!({
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
                })
            })
            .collect();
        // ams.hp_current/resources_json are no longer the source of truth
        // (resource current values live in generic_parameter_states, with sheet
        // seeds as the read fallback). Project every actor-owned kernel resource
        // into the GM ledger context so HP/SAN/Luck/etc. are visible from one
        // path. Kernels are cached by ruleset to avoid reloading per actor.
        let mut kernel_cache: std::collections::HashMap<String, Option<RuleKernel>> =
            std::collections::HashMap::new();
        kernel_cache.insert(ruleset_id.to_string(), session_kernel.clone());
        for a in actors.iter_mut() {
            let Some(actor_id) = a
                .get("actor_id")
                .and_then(|v| v.as_str())
                .map(str::to_string)
            else {
                continue;
            };
            // Actors without a ruleset mapping fall back to the session ruleset (B3).
            let rs = self
                .actor_ruleset_id(session_id, &actor_id)
                .await
                .unwrap_or_else(|| ruleset_id.to_string());
            let kernel = match kernel_cache.get(&rs) {
                Some(k) => k.clone(),
                None => {
                    let k = self.db.load_rule_kernel(&rs).await.ok().flatten();
                    kernel_cache.insert(rs.clone(), k.clone());
                    k
                }
            };
            let Some(k) = kernel else { continue };
            let hp_id = trpg_model::hp_resource_track_id(&k.resource_tracks);
            let mut resolved_resources = serde_json::Map::new();
            for track in &k.resource_tracks {
                let Some(track_id) = track
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                else {
                    continue;
                };
                if track
                    .get("owner_kind")
                    .and_then(|v| v.as_str())
                    .map(|owner| owner != "actor")
                    .unwrap_or(false)
                {
                    continue;
                }
                let Some(live) = self
                    .db
                    .load_resource_current(session_id, &actor_id, track_id, &k)
                    .await
                else {
                    continue;
                };
                let cap = self
                    .db
                    .resource_cap(session_id, &actor_id, track_id, &k)
                    .await;
                let mut resource = serde_json::Map::new();
                resource.insert("current".into(), serde_json::json!(live));
                if let Some(max) = cap {
                    resource.insert("max".into(), serde_json::json!(max));
                }
                resource.insert(
                    "path".into(),
                    serde_json::json!(format!("resources.{track_id}.current")),
                );
                resource.insert(
                    "source".into(),
                    serde_json::json!("generic_parameter_states_live_or_sheet_seed"),
                );
                if let Some(name) = track.get("name").and_then(|v| v.as_str()) {
                    resource.insert("name".into(), serde_json::json!(name));
                }
                if let Some(kind) = track.get("kind").and_then(|v| v.as_str()) {
                    resource.insert("kind".into(), serde_json::json!(kind));
                }
                if let Some(derived_from) = track.get("derived_from").and_then(|v| v.as_str()) {
                    resource.insert("derived_from".into(), serde_json::json!(derived_from));
                }
                if let Some(zero_means) = track.get("zero_means").and_then(|v| v.as_str()) {
                    resource.insert("zero_means".into(), serde_json::json!(zero_means));
                }
                if let Some(line) =
                    semantic_for_parameter_path(&format!("resources.{track_id}.current"), live, &k)
                {
                    resource.insert("semantic".into(), serde_json::json!(line));
                }
                resolved_resources.insert(track_id.to_string(), Value::Object(resource));
            }
            if let Some(obj) = a.as_object_mut() {
                obj.insert(
                    "resolved_resources".into(),
                    Value::Object(resolved_resources),
                );
            }
            if let Some(hp_id) = hp_id {
                let Some(live) = self
                    .db
                    .load_resource_current(session_id, &actor_id, &hp_id, &k)
                    .await
                else {
                    continue;
                };
                if let Some(obj) = a.as_object_mut() {
                    obj.insert("hp_current".into(), serde_json::json!(live));
                    obj.insert(
                        "hp_source".into(),
                        serde_json::json!("generic_parameter_states_live_or_sheet_seed"),
                    );
                    // B3: the live HP number also gets its kernel-track semantic
                    // line (threshold zone / zero_means) — fail-closed absent.
                    if let Some(line) = semantic_for_parameter_path("hp", live, &k) {
                        obj.insert("semantic".into(), serde_json::json!(line));
                    }
                }
            }
        }
        let effect_rows = sqlx::query("select effect_resolution_id, impacts_json, visibility, provisional_reason, world_tick from effect_resolution_packets where session_id = $1 order by created_at desc limit 12")
            .bind(session_id).fetch_all(&self.db.pool).await.unwrap_or_default();
        let effects: Vec<Value> = effect_rows
            .into_iter()
            .map(|r| {
                json!({
                    "effect_resolution_id": r.get::<String,_>("effect_resolution_id"),
                    "impacts": r.get::<Value,_>("impacts_json"),
                    "visibility": r.get::<String,_>("visibility"),
                    "provisional_reason": r.get::<Option<String>,_>("provisional_reason"),
                    "world_tick": r.get::<Option<i64>,_>("world_tick"),
                })
            })
            .collect();
        let generic_rows = sqlx::query("select target_kind, target_id, parameter_path, value_json, visibility, provisional_reason, world_tick from generic_parameter_states where session_id = $1 order by updated_at desc limit 20")
            .bind(session_id).fetch_all(&self.db.pool).await.unwrap_or_default();
        let mut generic_states: Vec<Value> = generic_rows
            .into_iter()
            .map(|r| {
                json!({
                    "target_kind": r.get::<String,_>("target_kind"),
                    "target_id": r.get::<String,_>("target_id"),
                    "parameter_path": r.get::<String,_>("parameter_path"),
                    "value": r.get::<Value,_>("value_json"),
                    "visibility": r.get::<String,_>("visibility"),
                    "provisional_reason": r.get::<Option<String>,_>("provisional_reason"),
                    "world_tick": r.get::<Option<i64>,_>("world_tick"),
                })
            })
            .collect();
        // B3: every generic row (actor AND scene/party/world — same read path)
        // gets a "semantic" line from the kernel track it resolves to.
        if let Some(k) = session_kernel.as_ref() {
            attach_generic_state_semantics(&mut generic_states, k);
        }
        let facet_rows = sqlx::query("select execution_kind, target_kind, target_id, parameter_path, operation, status, output_json, provisional_reason, world_tick from parameter_facet_execution_runs where session_id = $1 order by created_at desc limit 16")
            .bind(session_id).fetch_all(&self.db.pool).await.unwrap_or_default();
        let facet_executions: Vec<Value> = facet_rows
            .into_iter()
            .map(|r| {
                json!({
                    "execution_kind": r.get::<String,_>("execution_kind"),
                    "target_kind": r.get::<String,_>("target_kind"),
                    "target_id": r.get::<String,_>("target_id"),
                    "parameter_path": r.get::<String,_>("parameter_path"),
                    "operation": r.get::<String,_>("operation"),
                    "status": r.get::<String,_>("status"),
                    "output": r.get::<Value,_>("output_json"),
                    "provisional_reason": r.get::<Option<String>,_>("provisional_reason"),
                    "world_tick": r.get::<Option<i64>,_>("world_tick"),
                })
            })
            .collect();
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
            Scope {
                scope_type: ScopeType::Session,
                scope_id: session_id.to_string(),
            },
            220,
        );
        block.tags = vec![
            "mechanical_ledger".into(),
            "effect_resolution".into(),
            "parameter_impacts".into(),
            "parameter_facet_executor".into(),
        ];
        block.load_reason = Some("parameter_facet_executor".into());
        Ok(block)
    }

    async fn apply_effect_roll(
        &self,
        contract: &CheckContract,
        result: &CheckResultRecord,
    ) -> Result<EffectApplicationOutcome> {
        self.apply_effect_roll_with_decision(contract, result, None)
            .await
    }

    async fn apply_effect_roll_with_decision(
        &self,
        contract: &CheckContract,
        result: &CheckResultRecord,
        decision_override: Option<FacetDecision>,
    ) -> Result<EffectApplicationOutcome> {
        let total = result
            .roll
            .result
            .get("total")
            .and_then(|v| v.as_i64())
            .unwrap_or_default() as i32;
        let target_actor = contract
            .target_actor
            .as_ref()
            .map(|a| a.actor_id.clone())
            .or_else(|| {
                contract
                    .actor_snapshot_ids
                    .iter()
                    .find(|id| {
                        id.starts_with("npc.") || id.contains("opposition") || id.starts_with("pc.")
                    })
                    .cloned()
            })
            .unwrap_or_else(|| "npc.opposition".into());
        let source_actor = contract
            .metadata_value("source_actor_id")
            .or_else(|| Some(contract.initiator.actor_id.clone()));
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
                let actor_kind = if decision.target_id.starts_with("pc.") {
                    ActorKind::PlayerCharacter
                } else {
                    ActorKind::Npc
                };
                let state = self
                    .ensure_actor_state(
                        &contract.session_id,
                        contract.module_id.as_deref(),
                        &decision.target_id,
                        actor_kind,
                        None,
                        Some(&contract.ruleset_id),
                        contract.world_tick_hint(),
                    )
                    .await?;
                if decision.parameter_path == "hp.current" {
                    // HP current now lives in the single source of truth (generic_parameter_states
                    // via the kernel HP track), not actor_mechanical_states.hp_current. Armor
                    // (combat metadata) is still read from the ams row.
                    let kernel = self
                        .db
                        .load_rule_kernel(&contract.ruleset_id)
                        .await
                        .ok()
                        .flatten();
                    let hp_id = kernel
                        .as_ref()
                        .and_then(|k| trpg_model::hp_resource_track_id(&k.resource_tracks));
                    let from_hp = match (kernel.as_ref(), hp_id.as_ref()) {
                        (Some(k), Some(id)) => {
                            self.db
                                .load_resource_current(
                                    &contract.session_id,
                                    &decision.target_id,
                                    id,
                                    k,
                                )
                                .await
                        }
                        _ => state.hp_current.or(state.hp_max),
                    };
                    if let (Some(from_hp), Some(k), Some(id)) =
                        (from_hp, kernel.as_ref(), hp_id.as_ref())
                    {
                        let (to_hp, sp_applied) = trpg_model::apply_armor_damage(
                            from_hp,
                            amount,
                            state.armor_current,
                            decision.operation,
                        );
                        let delta = to_hp - from_hp;
                        let cap = self
                            .db
                            .resource_cap(&contract.session_id, &decision.target_id, id, k)
                            .await;
                        self.db
                            .write_resource_current(
                                &contract.session_id,
                                &decision.target_id,
                                id,
                                to_hp,
                                cap,
                                &merge_source_refs(&contract.source_refs, &decision.source_refs),
                                Visibility::GmOnly,
                                contract.world_tick_hint(),
                            )
                            .await?;
                        if to_hp <= 0 && actor_kind != ActorKind::PlayerCharacter {
                            self.close_active_frame_for_defeated_target(
                                &contract.session_id,
                                &decision.target_id,
                                contract.world_tick_hint(),
                            )
                            .await
                            .ok();
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
                            source_refs: merge_source_refs(
                                &contract.source_refs,
                                &decision.source_refs,
                            ),
                            provisional_reason: decision.provisional_reason.clone(),
                        };
                        patches.push(StatePatch::ActorHpDelta {
                            actor_id: decision.target_id.clone(),
                            from: Some(from_hp),
                            delta,
                            to: Some(to_hp),
                            reason: "parameter_facet_executor_hp_delta".into(),
                        });
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
                            contest_id: result
                                .outcome
                                .get("contest_id")
                                .and_then(|v| v.as_str())
                                .map(str::to_string),
                            damage_expression: Some(contract.dice_expression.clone()),
                            rolled_total: total,
                            damage_type: contract.metadata_value("damage_type"),
                            armor_interaction: json!({"sp_applied": sp_applied, "rolled": amount, "after_armor": (amount - sp_applied).max(0), "policy": if state.armor_current.is_some() { "sp_subtracted_from_materialized_armor_v1162f" } else { "no_armor_materialized_full_damage_v1162f" }}),
                            final_hp_delta: delta,
                            from_hp: Some(from_hp),
                            to_hp: Some(to_hp),
                            source_refs: merge_source_refs(
                                &contract.source_refs,
                                &decision.source_refs,
                            ),
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
                } else if matches!(
                    decision.operation,
                    ParameterOperation::AddCondition | ParameterOperation::RemoveCondition
                ) || decision.parameter_path.starts_with("conditions")
                {
                    let mut conditions = state.conditions.clone();
                    let before = json!(conditions);
                    if matches!(decision.operation, ParameterOperation::RemoveCondition) {
                        conditions.retain(|c| {
                            c.get("path").and_then(|v| v.as_str())
                                != Some(decision.parameter_path.as_str())
                        });
                    } else {
                        conditions.push(json!({
                            "path": decision.parameter_path,
                            "source": "parameter_facet_executor",
                            "severity": amount,
                            "provisional_reason": decision.provisional_reason.clone()
                        }));
                    }
                    self.update_actor_conditions(
                        &contract.session_id,
                        &decision.target_id,
                        &conditions,
                        contract.world_tick_hint(),
                    )
                    .await?;
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
                        source_refs: merge_source_refs(
                            &contract.source_refs,
                            &decision.source_refs,
                        ),
                        provisional_reason: decision.provisional_reason.clone(),
                    };
                    patches.push(StatePatch::ModifyTrack {
                        target: format!("{}.{}", decision.target_id, decision.parameter_path),
                        amount,
                        reason: "parameter_facet_executor_condition_delta".into(),
                    });
                    impacts.push(impact);
                } else {
                    // Non-HP actor resources also live in the SSOT, keyed by a resolved kernel track id.
                    let kernel = self
                        .db
                        .load_rule_kernel(&contract.ruleset_id)
                        .await
                        .ok()
                        .flatten();
                    let track_id = kernel.as_ref().and_then(|k| {
                        trpg_model::resolve_resource_track_id(&decision.parameter_path, k)
                    });
                    if let (Some(k), Some(id)) = (kernel.as_ref(), track_id.as_ref()) {
                        let before = self
                            .db
                            .load_resource_current(&contract.session_id, &decision.target_id, id, k)
                            .await
                            .unwrap_or(0);
                        let after = apply_i32_operation(before, amount, decision.operation);
                        let cap = self
                            .db
                            .resource_cap(&contract.session_id, &decision.target_id, id, k)
                            .await;
                        self.db
                            .write_resource_current(
                                &contract.session_id,
                                &decision.target_id,
                                id,
                                after,
                                cap,
                                &merge_source_refs(&contract.source_refs, &decision.source_refs),
                                Visibility::GmOnly,
                                contract.world_tick_hint(),
                            )
                            .await?;
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
                            source_refs: merge_source_refs(
                                &contract.source_refs,
                                &decision.source_refs,
                            ),
                            provisional_reason: decision.provisional_reason.clone(),
                        };
                        patches.push(StatePatch::ModifyTrack {
                            target: format!("{}.{}", decision.target_id, decision.parameter_path),
                            amount: after - before,
                            reason: "parameter_facet_executor_resource_delta".into(),
                        });
                        impacts.push(impact);
                    } else {
                        patches.push(StatePatch::CreateFact { target: decision.target_id.clone(), fact: json!({"blocked_resource_delta": true, "parameter_path": decision.parameter_path, "reason": "unresolved_resource_track_id"}), reason: "parameter_facet_executor_blocked_unresolved_resource".into() });
                    }
                }
            }
            _ => {
                let before = self
                    .load_generic_parameter_value(
                        &contract.session_id,
                        decision.target_kind,
                        &decision.target_id,
                        &decision.parameter_path,
                    )
                    .await?
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0) as i32;
                let after = apply_i32_operation(before, amount, decision.operation);
                self.upsert_generic_parameter_state(
                    &contract.session_id,
                    decision.target_kind,
                    &decision.target_id,
                    &decision.parameter_path,
                    json!(after),
                    Visibility::GmOnly,
                    &decision.source_refs,
                    decision.provisional_reason.as_deref(),
                    contract.world_tick_hint(),
                )
                .await?;
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
                patches.push(StatePatch::ModifyTrack {
                    target: format!(
                        "{}:{}:{}",
                        decision.target_kind.as_str(),
                        decision.target_id,
                        decision.parameter_path
                    ),
                    amount: after - before,
                    reason: "parameter_facet_executor_generic_delta".into(),
                });
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
            target_refs: vec![TargetRef {
                target_kind: decision.target_kind,
                target_id: decision.target_id.clone(),
                label: Some("effect target".into()),
            }],
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
        for impact in &effect.impacts {
            self.insert_parameter_impact(&effect, impact).await?;
        }
        self.insert_facet_execution_run(contract, &decision, &effect, total)
            .await
            .ok();
        Ok(EffectApplicationOutcome {
            effect,
            damage: damage_packet,
            patches,
        })
    }

    async fn record_attack(
        &self,
        contract: &CheckContract,
        result: &CheckResultRecord,
        success: bool,
    ) -> Result<Option<AttackResolutionContract>> {
        // Fail closed: no explicit, distinct target → never fabricate one and
        // never persist a self-attack row.
        let Some(target) = resolved_attack_target(contract) else {
            return Ok(None);
        };
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
        self.ensure_actor_state(
            &contract.session_id,
            contract.module_id.as_deref(),
            &target,
            ActorKind::Npc,
            provisional_opposition_hp_seed(&target),
            Some(&contract.ruleset_id),
            attack.world_tick,
        )
        .await
        .ok();
        self.insert_attack_resolution(&attack).await?;
        Ok(Some(attack))
    }

    fn make_effect_roll_check(
        &self,
        attack_contract: &CheckContract,
        attack: &AttackResolutionContract,
        dice_expression: &str,
    ) -> CheckContract {
        let mut check = attack_contract.clone();
        check.check_id = format!("check_effect_{}", Uuid::new_v4().simple());
        check.intent_kind = "effect_roll".into();
        check.check_label = "effect roll".into();
        check.dice_expression = dice_expression.into();
        if system_rolls_visible_policy() {
            check.roll_visibility = RollVisibility::PublicGmRoll;
            check.roll_authority = RollAuthority::System;
            check.disclosure = RollDisclosurePolicy::for_visibility(RollVisibility::PublicGmRoll);
        } else {
            check.roll_visibility = RollVisibility::PlayerRollRequired;
            check.roll_authority = RollAuthority::Player;
            check.disclosure =
                RollDisclosurePolicy::for_visibility(RollVisibility::PlayerRollRequired);
        }
        check.target = CheckTargetModel::UnknownUntilLookup;
        check.opposition = OppositionModel::NoMechanicalOpposition;
        check.target_actor = attack.target_actor_id.as_ref().map(|id| ActorRef {
            actor_id: id.clone(),
            actor_kind: ActorKind::Npc,
            display_name: Some("target".into()),
        });
        check.actor_snapshot_ids = vec![attack
            .target_actor_id
            .clone()
            .unwrap_or_else(|| "npc.opposition".into())];
        check.stakes.before_roll_public = "Effect roll for the successful action. The GM will apply it to the correct tracked parameter (HP/SAN/Chaos/Harm/condition/object state) from rules and bound facets; do not ask the player for remaining values.".into();
        check.stakes.success_public =
            "The effect is applied to the target's mechanical ledger.".into();
        check.stakes.failure_public = "Effect roll could not be applied; the GM must clarify source/target rather than lose state.".into();
        check
            .advice_refs
            .push("unified_roll_effect_executor.effect_followup.v1_12_1".into());
        check
    }

    /// Read an actor's HP from runtime_actor_parameters.status_json (set by the
    /// created character / generic kernel seed) so the combat HP table can be
    /// seeded with a source-backed value. Returns hp_max (fallback hp_current).
    async fn actor_hp_from_params(&self, session_id: &str, actor_id: &str) -> Option<i32> {
        let row = sqlx::query("select coalesce(status_json->>'hp_max', status_json->>'hp_current') as hp from runtime_actor_parameters where session_id=$1 and actor_id=$2 limit 1")
            .bind(session_id).bind(actor_id).fetch_optional(&self.db.pool).await.ok().flatten()?;
        row.get::<Option<String>, _>("hp")
            .and_then(|s| s.parse::<i32>().ok())
            .filter(|h| *h > 0)
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
    async fn actor_resource_seeds(
        &self,
        session_id: &str,
        actor_id: &str,
        kernel: &RuleKernel,
    ) -> std::collections::HashMap<String, (Option<i32>, Option<i32>)> {
        self.db.resource_seeds(session_id, actor_id, kernel).await
    }

    async fn ensure_actor_state(
        &self,
        session_id: &str,
        frame_id: Option<&str>,
        actor_id: &str,
        actor_kind: ActorKind,
        default_hp: Option<i32>,
        ruleset_id: Option<&str>,
        world_tick: i64,
    ) -> Result<ActorMechanicalState> {
        let existing = sqlx::query("select state_id, session_id, frame_id, actor_id, actor_kind, hp_current, hp_max, armor_current, wound_state, morale, resources_json, conditions_json, source_refs, provisional_reason, world_tick, updated_at from actor_mechanical_states where session_id = $1 and actor_id = $2")
            .bind(session_id).bind(actor_id).fetch_optional(&self.db.pool).await?;
        if let Some(r) = existing {
            return Ok(row_to_actor_state(r));
        }
        // Load the actor's kernel (via runtime_actor_parameters.ruleset_id, since
        // this fn isn't threaded a ruleset) so resources/HP seed from the
        // character's DERIVED resources matched by kernel track id — never a
        // hardcoded per-ruleset blob.
        let kernel = match self
            .actor_ruleset_id(session_id, actor_id)
            .await
            .or_else(|| ruleset_id.map(str::to_string))
        {
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
        let hp_seed = kernel
            .as_ref()
            .and_then(|k| trpg_model::hp_resource_track_id(&k.resource_tracks))
            .and_then(|hp_id| seeds.get(&hp_id).and_then(|s| s.0));
        let default_hp = default_hp
            .or(hp_seed)
            .or(self.actor_hp_from_params(session_id, actor_id).await);
        let state = ActorMechanicalState {
            state_id: format!("actor_mech_{}", Uuid::new_v4().simple()),
            session_id: session_id.into(),
            frame_id: frame_id.map(str::to_string),
            actor_id: actor_id.into(),
            actor_kind,
            hp_current: default_hp,
            hp_max: default_hp,
            armor_current: None,
            wound_state: Some(
                if default_hp.is_some() {
                    "unhurt"
                } else {
                    "unknown"
                }
                .into(),
            ),
            morale: Some(if actor_kind == ActorKind::PlayerCharacter {
                100
            } else {
                55
            }),
            resources,
            conditions: vec![],
            source_refs: vec![],
            provisional_reason: Some(if default_hp.is_some() {
                "v1.12.1 provisional mechanical state; replace with module/rules source pack when available.".into()
            } else {
                "Unbound mechanical state; HP/resource values require source-backed materialization before damage can be decremented.".into()
            }),
            world_tick,
            updated_at: Utc::now(),
        };
        self.upsert_actor_state(&state).await?;
        if let (Some(hp), Some(k)) = (state.hp_current, kernel.as_ref()) {
            if let Some(hp_id) = trpg_model::hp_resource_track_id(&k.resource_tracks) {
                let cap = state.hp_max.or(Some(hp));
                let _ = self
                    .db
                    .write_resource_current(
                        session_id,
                        actor_id,
                        &hp_id,
                        hp,
                        cap,
                        &state.source_refs,
                        Visibility::GmOnly,
                        world_tick,
                    )
                    .await;
            }
        }
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

    async fn close_active_frame_for_defeated_target(
        &self,
        session_id: &str,
        actor_id: &str,
        world_tick: i64,
    ) -> Result<()> {
        let Some(mut frame) = self
            .db
            .list_active_state_frames(session_id, 1)
            .await?
            .into_iter()
            .next()
        else {
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

    async fn update_actor_conditions(
        &self,
        session_id: &str,
        actor_id: &str,
        conditions: &[Value],
        world_tick: i64,
    ) -> Result<()> {
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

    async fn insert_parameter_impact(
        &self,
        packet: &EffectResolutionPacket,
        impact: &ParameterImpact,
    ) -> Result<()> {
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
        is_attack_like_check(contract)
    }
    fn is_effect_roll_check(&self, contract: &CheckContract) -> bool {
        matches!(
            contract.intent_kind.as_str(),
            "damage_roll"
                | "effect_roll"
                | "sanity_roll"
                | "chaos_roll"
                | "harm_roll"
                | "resource_loss"
        ) || contract.check_label.to_ascii_lowercase().contains("damage")
            || contract.check_label.contains("伤害")
            || contract.check_label.to_ascii_lowercase().contains("effect")
    }

    async fn damage_expression_for_attack(&self, contract: &CheckContract) -> Option<String> {
        if let Some(expr) = contract.metadata_value("damage_expression") {
            return Some(expr);
        }
        if let Some(expr) = self.damage_expression_from_held_weapon(contract).await {
            return Some(expr);
        }
        // Source-backed weapon damage from the formula pack (step-2): a
        // DerivedValue with field_id starting "weapon_damage" whose `formula` is
        // a plain dice expression (e.g. "3d6" for a Heavy Pistol). Pack-driven
        // so no synthetic constant is fabricated when the weapon is unbound.
        if let Ok(Some(pack)) = self
            .db
            .load_character_onboarding_pack(&contract.ruleset_id)
            .await
        {
            if let Some(f) = pack
                .derived_formula_pack
                .formulas
                .iter()
                .find(|f| f.field_id.to_ascii_lowercase().starts_with("weapon_damage"))
            {
                let dice = f.formula.trim();
                if is_plain_dice(dice) {
                    return Some(dice.to_string());
                }
            }
        }
        // Per-ruleset debug/override hook, derived from the ruleset id (no hardcoded
        // ruleset name): e.g. ruleset "cyberpunk_red" -> TRPG_COMBAT_DEFAULT_CYBERPUNK_RED_DAMAGE.
        // Falls back to the generic var. Both are unset in normal operation (-> None).
        let key = format!(
            "TRPG_COMBAT_DEFAULT_{}_DAMAGE",
            contract
                .ruleset_id
                .to_ascii_uppercase()
                .replace(|c: char| !c.is_ascii_alphanumeric(), "_")
        );
        if let Ok(v) = std::env::var(&key) {
            return Some(v);
        }
        std::env::var("TRPG_COMBAT_DEFAULT_DAMAGE_EXPR").ok()
    }

    async fn damage_expression_from_held_weapon(&self, contract: &CheckContract) -> Option<String> {
        let row = sqlx::query(
            r#"
            select mechanical_state
            from object_instances
            where session_id = $1
              and active = true
              and 'weapon' = any(tags)
              and $2 = any(tags)
            order by updated_at desc
            limit 1
            "#,
        )
        .bind(&contract.session_id)
        .bind(format!("held_by:{}", contract.initiator.actor_id))
        .fetch_optional(&self.db.pool)
        .await
        .ok()
        .flatten()?;
        let state = row.get::<Value, _>("mechanical_state");
        let expr = state
            .get("damage_expression")
            .or_else(|| state.get("damage"))
            .and_then(|v| v.as_str())?
            .trim();
        if is_plain_dice(expr) {
            Some(expr.to_string())
        } else {
            None
        }
    }

    async fn resolve_facet_decision(
        &self,
        contract: &CheckContract,
        default_actor_id: &str,
    ) -> Result<FacetDecision> {
        if let Some(decision) = self
            .lookup_bound_facet_decision(contract, default_actor_id)
            .await?
        {
            return Ok(decision);
        }
        Ok(self
            .ruleset_fallback_facet_decision(contract, default_actor_id)
            .await)
    }

    async fn lookup_bound_facet_decision(
        &self,
        contract: &CheckContract,
        default_actor_id: &str,
    ) -> Result<Option<FacetDecision>> {
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
            let path = facet_json
                .get("target_parameter")
                .or_else(|| facet_json.get("parameter_path"))
                .or_else(|| facet_json.get("effect_target_path"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let Some(parameter_path) = path else {
                continue;
            };
            let op = facet_json
                .get("operation")
                .and_then(|v| v.as_str())
                .and_then(parse_parameter_operation)
                .unwrap_or_else(|| {
                    default_operation_for_path(&parameter_path, &contract.ruleset_id)
                });
            let target_kind = facet_json
                .get("target_kind")
                .and_then(|v| v.as_str())
                .and_then(parse_effect_target_kind)
                .unwrap_or_else(|| {
                    default_target_kind_for_path(&parameter_path, &contract.ruleset_id)
                });
            let target_id = facet_json
                .get("target_id")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| match target_kind {
                    EffectTargetKind::Actor => default_actor_id.to_string(),
                    EffectTargetKind::Scene => "scene.current".into(),
                    EffectTargetKind::Anomaly => "anomaly.current".into(),
                    EffectTargetKind::Clock => "clock.current".into(),
                    EffectTargetKind::Campaign => "campaign.current".into(),
                    EffectTargetKind::Object => contract
                        .metadata_value("target_object_id")
                        .unwrap_or_else(|| "object.current".into()),
                    EffectTargetKind::Relationship => "relationship.current".into(),
                    EffectTargetKind::World => "world.current".into(),
                });
            let source_refs: Vec<SourceRef> =
                serde_json::from_value(r.get("source_refs")).unwrap_or_default();
            let verification_status: String = r.get("verification_status");
            let status = if verification_status.contains("verified") {
                FacetExecutionStatus::Applied
            } else {
                FacetExecutionStatus::AppliedProvisional
            };
            return Ok(Some(FacetDecision {
                target_kind,
                target_id,
                parameter_path,
                operation: op,
                source_facet_binding_ids: vec![r.get::<String, _>("facet_binding_id")],
                source_refs,
                status,
                provisional_reason: Some(
                    "Applied parameter facet binding selected from parameter_facet_bindings."
                        .into(),
                ),
            }));
        }
        Ok(None)
    }

    async fn ruleset_fallback_facet_decision(
        &self,
        _contract: &CheckContract,
        default_actor_id: &str,
    ) -> FacetDecision {
        // De-hardcoded: NO per-ruleset / keyword routing (chaos/sanity/harm/anomaly
        // were game-specific guesses). Resource-track effects now flow data-driven
        // via the kernel's resource_tracks.on_outcome rules (apply_outcome_resource_tracks),
        // and source-bound facets route through lookup_bound_facet_decision. When
        // neither applies, the only honest structural default for an unbound
        // physical effect is hp.current; richer effects require a bound facet.
        FacetDecision::hp(default_actor_id.into(), "No bound effect facet and no kernel resource rule matched; defaulted this physical effect to hp.current. Bind a source-backed facet or add a kernel resource_tracks rule to route it precisely.")
    }

    async fn load_generic_parameter_value(
        &self,
        session_id: &str,
        target_kind: EffectTargetKind,
        target_id: &str,
        parameter_path: &str,
    ) -> Result<Option<Value>> {
        let row = sqlx::query("select value_json from generic_parameter_states where session_id = $1 and target_kind = $2 and target_id = $3 and parameter_path = $4")
            .bind(session_id).bind(target_kind.as_str()).bind(target_id).bind(parameter_path).fetch_optional(&self.db.pool).await?;
        Ok(row.map(|r| r.get::<Value, _>("value_json")))
    }

    async fn upsert_generic_parameter_state(
        &self,
        session_id: &str,
        target_kind: EffectTargetKind,
        target_id: &str,
        parameter_path: &str,
        value: Value,
        visibility: Visibility,
        source_refs: &[SourceRef],
        provisional_reason: Option<&str>,
        world_tick: i64,
    ) -> Result<()> {
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

    async fn insert_facet_execution_run(
        &self,
        contract: &CheckContract,
        decision: &FacetDecision,
        effect: &EffectResolutionPacket,
        roll_total: i32,
    ) -> Result<()> {
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

pub fn make_roll_plan_from_check(
    session_id: &str,
    turn_id: &str,
    check_id: Option<&str>,
    contract: &CheckContract,
    expression: &str,
    result_json: &Value,
) -> RollPlan {
    let lower = format!("{} {}", contract.intent_kind, contract.check_label).to_ascii_lowercase();
    let roll_kind = if lower.contains("damage") {
        RollKind::Damage
    } else if lower.contains("effect") {
        RollKind::EffectRoll
    } else if lower.contains("san") {
        RollKind::Sanity
    } else if lower.contains("chaos") {
        RollKind::Chaos
    } else if lower.contains("harm") {
        RollKind::Harm
    } else if lower.contains("save") || lower.contains("豁免") {
        RollKind::SavingThrow
    } else if lower.contains("percent") || lower.contains("d100") {
        RollKind::PercentileCheck
    } else if is_attack_like_check(contract) {
        RollKind::Attack
    } else {
        RollKind::SkillCheck
    };
    RollPlan {
        roll_plan_id: format!("rollplan_{}", Uuid::new_v4().simple()),
        session_id: session_id.into(),
        turn_id: turn_id.into(),
        frame_id: None,
        roll_kind,
        actor_id: Some(contract.initiator.actor_id.clone()),
        target_refs: contract
            .target_actor
            .as_ref()
            .map(|a| {
                vec![TargetRef {
                    target_kind: EffectTargetKind::Actor,
                    target_id: a.actor_id.clone(),
                    label: a.display_name.clone(),
                }]
            })
            .unwrap_or_default(),
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

/// Resolve the actor id that an attack-resolution row should target.
///
/// Returns `None` when the check has no explicit target actor, or when the
/// target is the same actor as the source. A missing target must never be
/// defaulted to a fabricated opponent id, and a self-attack (source == target)
/// must never be persisted — both were live-journey pollution sources where a
/// status/aftermath check seeded a spurious `attack_resolution_contracts` row.
fn resolved_attack_target(contract: &CheckContract) -> Option<String> {
    let target = contract.target_actor.as_ref()?.actor_id.trim().to_string();
    if target.is_empty() || target == contract.initiator.actor_id {
        return None;
    }
    Some(target)
}

fn is_attack_like_check(contract: &CheckContract) -> bool {
    let intent = contract.intent_kind.to_ascii_lowercase();
    let text = attack_classification_text(contract);
    if is_effect_or_damage_like_check(contract)
        || contains_any(
            &text,
            &[
                "initiative",
                "combat_order",
                "turn_order",
                "order",
                "reload",
                "reloading",
                "dodge",
                "stealth",
                "先手",
                "顺序",
                "行动顺序",
                "战斗顺序",
                "装弹",
                "换弹",
                "闪避",
                "潜行",
            ],
        )
        // Aftermath/status/recovery mechanics are CONSEQUENCES of an attack, not
        // attacks. They must be excluded before the inclusion scan below, because
        // labels like "Zero HP state after gunshot" contain "shot" as a substring
        // and would otherwise seed a spurious attack_resolution_contracts row.
        || contains_any(
            &text,
            &[
                "gunshot",
                "major wound",
                "minor wound",
                "wound state",
                "zero hp",
                "zero hit",
                "dying",
                "conscious",
                "consciousness",
                "recovery",
                "aftermath",
                "aftereffect",
                "status query",
                "重伤",
                "昏迷",
                "濒死",
                "失去意识",
            ],
        )
    {
        return false;
    }
    matches!(intent.as_str(), "attack" | "counterattack")
        || contains_any(
            &text,
            &[
                "attack",
                "counterattack",
                "firearms",
                "handgun",
                "pistol",
                "m1911",
                "shoot",
                "shot",
                "开火",
                "攻击",
                "射击",
                "开枪",
                "手枪",
            ],
        )
}

fn attack_classification_text(contract: &CheckContract) -> String {
    let tested = contract
        .tested_parameter
        .as_ref()
        .map(|p| format!("{} {}", p.key, p.label))
        .unwrap_or_default();
    format!(
        "{} {} {} {}",
        contract.intent_kind, contract.check_label, contract.action_summary, tested
    )
    .to_ascii_lowercase()
}

fn is_effect_or_damage_like_check(contract: &CheckContract) -> bool {
    let text = format!("{} {}", contract.intent_kind, contract.check_label).to_ascii_lowercase();
    text.contains("damage")
        || text.contains("effect")
        || text.contains("伤害")
        || text.contains("sanity")
        || text.contains("chaos")
        || text.contains("harm")
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
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
    let dpos = match s.find('d') {
        Some(p) => p,
        None => return s.parse().unwrap_or(0),
    };
    let (dice_part, modifier) = match s[dpos + 1..].find(|c| c == '+' || c == '-') {
        Some(rel) => {
            let split = dpos + 1 + rel;
            (&s[..split], s[split..].parse::<i32>().unwrap_or(0))
        }
        None => (&s[..], 0),
    };
    let mut it = dice_part.splitn(2, 'd');
    let n = {
        let a = it.next().unwrap_or("1");
        if a.is_empty() {
            1
        } else {
            a.parse::<i32>().unwrap_or(1)
        }
    }
    .clamp(1, 100);
    let m = it
        .next()
        .unwrap_or("6")
        .parse::<i32>()
        .unwrap_or(6)
        .clamp(1, 1000);
    let mut rng = rand::thread_rng();
    (0..n).map(|_| rng.gen_range(1..=m)).sum::<i32>() + modifier
}

fn is_plain_dice(s: &str) -> bool {
    let s = s.trim().to_ascii_lowercase();
    if !s.contains('d') {
        return false;
    }
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

/// Deterministic maximum of a plain dice amount expression (no randomness):
/// "1d10"→10, "2d6"→12, "2d6+3"→15, "2d6-1"→11, "d8"→8; a bare integer ("7")
/// is a legal degenerate form (→7). Anything that is not a plain NdM±K dice
/// expression (named tokens, formula strings) → None. Shares the NdM±K parsing
/// rules of `roll_amount_dice` (incl. the 1..=100 / 1..=1000 clamps) but stays
/// an independent small function — ten copied lines over a forced DRY knot.
fn dice_max(expr: &str) -> Option<i32> {
    let s = expr.trim().to_ascii_lowercase().replace(' ', "");
    if s.is_empty() {
        return None;
    }
    let dpos = match s.find('d') {
        Some(p) => p,
        None => return s.parse().ok(),
    };
    let (dice_part, modifier) = match s[dpos + 1..].find(|c| c == '+' || c == '-') {
        Some(rel) => {
            let split = dpos + 1 + rel;
            (&s[..split], s[split..].parse::<i32>().ok()?)
        }
        None => (&s[..], 0),
    };
    let mut it = dice_part.splitn(2, 'd');
    let a = it.next().unwrap_or("1");
    let n = if a.is_empty() {
        1
    } else {
        a.parse::<i32>().ok()?
    }
    .clamp(1, 100);
    let m = it.next()?.parse::<i32>().ok()?.clamp(1, 1000);
    Some(n * m + modifier)
}

/// Structured trigger form (pure): `{kind:"band", band_id:"fumble"}` →
/// Some("fumble"). Exact shape only — `kind` MUST be "band", extra keys are
/// tolerated, a missing `band_id` → None. Strings and any other shape → None
/// (string triggers never reach this function — the call site takes the
/// legacy `as_str()` word-table branch first, byte-for-byte unchanged).
fn band_trigger_id(trigger: &Value) -> Option<String> {
    let obj = trigger.as_object()?;
    if obj.get("kind").and_then(|v| v.as_str()) != Some("band") {
        return None;
    }
    obj.get("band_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// on_outcome trigger gate, three layers (A5):
///   1. missing `trigger` key → fire (legacy "always" default — absence is NOT
///      "unrecognized"; old kernels carry many bare rules that rely on this);
///   2. string trigger → the legacy word table, behavior unchanged
///      (always/on_success/on_failure/on_tier incl. tier/min_rank reconcile);
///   3. non-string trigger → `band_trigger_id`: a band form is reconciled
///      case-insensitively against `outcome.success_tier` — the SAME key the
///      contest resolver writes and the on_tier branch already reads (verified
///      2026-06-10 against live check_results.outcome_json rows: band ids
///      "regular"/"hard"/"extreme" live under `success_tier`; failed checks may
///      omit the key → no match → skip). Anything unrecognized (other objects,
///      arrays, numbers) SKIPS the rule — never falls back to "always":
///      firing on an unparsed trigger would amplify the error (fail-closed).
fn trigger_fires(rule: &Value, outcome: &Value) -> bool {
    let Some(trigger) = rule.get("trigger") else {
        return true;
    };
    if let Some(s) = trigger.as_str() {
        let success = outcome.get("success").and_then(|v| v.as_bool());
        return match s {
            "on_success" => success == Some(true),
            "on_failure" => success == Some(false),
            // Gate on a graded success tier (e.g. CoC "extra effect on
            // extreme"): `tier:"<id>"` and/or `min_rank:<n>` vs the
            // success_tier the contest resolver emitted. Data-driven.
            "on_tier" => {
                let want_tier = rule.get("tier").and_then(|v| v.as_str());
                let got_tier = outcome.get("success_tier").and_then(|v| v.as_str());
                let tier_ok = want_tier.map(|w| Some(w) == got_tier).unwrap_or(true);
                let rank_ok = match rule.get("min_rank").and_then(|v| v.as_i64()) {
                    Some(min) => outcome
                        .get("success_tier_rank")
                        .and_then(|v| v.as_i64())
                        .map(|r| r >= min)
                        .unwrap_or(false),
                    None => true,
                };
                tier_ok && rank_ok
            }
            _ => true,
        };
    }
    match band_trigger_id(trigger) {
        Some(band_id) => outcome
            .get("success_tier")
            .and_then(|v| v.as_str())
            .map(|t| t.eq_ignore_ascii_case(&band_id))
            .unwrap_or(false),
        None => false,
    }
}

/// 护栏 §3.5.1（C7 债项落地，spec 终审 important #2）：on_outcome 规则经
/// `check_match` regex（字符串路径）命中触发、且契约 advice_refs 无 "mechanic:"
/// 前缀的结构化引用（即未走结构化绑定）时，产出账本可观测回退标注
/// （CreateFact，与 resource_delta_applied 同通道）——字符串回退绝不静默。
/// 结构化路径不标注：① 规则 trigger 是 A5 结构化 band 形态（band_trigger_id）；
/// ② 契约带 "mechanic:" 引用；③ 规则没有 check_match（未走字符串扫描）。
/// 调用点位于 check_match 门之后 ⇒ 走到这里即 regex 已命中。
fn check_match_fallback_fact(
    rule: &Value,
    advice_refs: &[String],
    target_id: &str,
    track_id: &str,
    check_label: &str,
) -> Option<StatePatch> {
    rule.get("check_match").and_then(|v| v.as_str())?;
    if rule
        .get("trigger")
        .is_some_and(|t| band_trigger_id(t).is_some())
    {
        return None;
    }
    if advice_refs.iter().any(|r| r.starts_with("mechanic:")) {
        return None;
    }
    Some(StatePatch::CreateFact {
        target: target_id.to_string(),
        fact: json!(format!(
            "fallback:check_match regex matched track '{track_id}' for check '{check_label}'"
        )),
        reason: "check_match_fallback_observed".into(),
    })
}

/// The text a resource-track `check_match` is scanned against. Includes the
/// check's DELIBERATE engine-set binding (`tested_parameter`) alongside its
/// intent/label and the player prose, so a check that was explicitly bound to a
/// track (e.g. a Sanity roll → tested_parameter=Sanity) reliably fires that
/// track's on_outcome even when the prose itself never names the parameter.
fn check_match_hay(
    intent_kind: &str,
    check_label: &str,
    action_summary: &str,
    tested: Option<&TestedParameter>,
) -> String {
    let tp = tested
        .map(|t| format!("{} {}", t.key, t.label))
        .unwrap_or_default();
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
        .iter()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    if needles.is_empty() {
        return false;
    }
    let mut aliases: Vec<String> = Vec::new();
    for k in ["id", "name"] {
        if let Some(s) = track.get(k).and_then(|v| v.as_str()) {
            let s = s.trim().to_ascii_lowercase();
            if !s.is_empty() {
                aliases.push(s);
            }
        }
    }
    if let Some(arr) = track.get("on_outcome").and_then(|v| v.as_array()) {
        for r in arr {
            if let Some(cm) = r.get("check_match").and_then(|v| v.as_str()) {
                aliases.extend(
                    cm.split('|')
                        .map(|s| s.trim().to_ascii_lowercase())
                        .filter(|s| !s.is_empty()),
                );
            }
        }
    }
    needles.iter().any(|n| {
        aliases
            .iter()
            .any(|a| n == a || n.contains(a.as_str()) || a.contains(n.as_str()))
    })
}

/// Resolve a resource-track on_outcome amount, fully data-driven:
///   - `amount` may be a dice expr (`1d6`), `=value` (the `when` outcome field),
///     `=<field>` (read `outcome.<field>` directly, e.g. `=sanity_loss`),
///     `max_of:<dice>` (deterministic dice maximum, e.g. `max_of:1d10`→10), or
///     a fixed int;
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
        } else if let Some(rest) = e.strip_prefix("max_of:") {
            // Deterministic dice maximum (e.g. fumble loses the MAX of the
            // die). Unparseable argument → None: same fallback semantics as a
            // missing outcome field (default_amount when tested, else skip).
            dice_max(rest.trim())
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
            Some(
                dt.parse::<i32>()
                    .unwrap_or_else(|_| when.and_then(&field_amount).unwrap_or(0)),
            )
        }
    } else {
        when.and_then(&field_amount)
    }
}

fn parse_parameter_operation(s: &str) -> Option<ParameterOperation> {
    match s.to_ascii_lowercase().as_str() {
        "add" | "increase" | "increment" => Some(ParameterOperation::Add),
        "subtract" | "decrease" | "damage" | "loss" | "delta_negative" => {
            Some(ParameterOperation::Subtract)
        }
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
    if p.starts_with("tracks.") || p.starts_with("scene.") {
        return EffectTargetKind::Scene;
    }
    if p.starts_with("anomaly") {
        return EffectTargetKind::Anomaly;
    }
    if p.starts_with("object.") {
        return EffectTargetKind::Object;
    }
    if p.starts_with("clock.") {
        return EffectTargetKind::Clock;
    }
    EffectTargetKind::Actor
}

fn default_operation_for_path(path: &str, _ruleset_id: &str) -> ParameterOperation {
    // Structural inference from the parameter path only — no per-ruleset branching.
    let p = path.to_ascii_lowercase();
    if p.starts_with("tracks.") || p.contains("clock") {
        return ParameterOperation::Add;
    }
    if p.contains("condition") || p.ends_with("major_wound") {
        return ParameterOperation::AddCondition;
    }
    if p.contains("state") || p.contains("control") {
        return ParameterOperation::Set;
    }
    ParameterOperation::Subtract
}

fn apply_i32_operation(before: i32, amount: i32, op: ParameterOperation) -> i32 {
    match op {
        ParameterOperation::Add | ParameterOperation::AdvanceClock => before.saturating_add(amount),
        ParameterOperation::Subtract => (before - amount).max(0),
        ParameterOperation::Set => amount.max(0),
        ParameterOperation::AddCondition
        | ParameterOperation::RemoveCondition
        | ParameterOperation::MarkRevealed => before,
    }
}

fn provisional_opposition_hp_seed(actor_id: &str) -> Option<i32> {
    if actor_id != "npc.opposition" {
        return None;
    }
    if std::env::var("TRPG_RUNTIME_PARAM_ALLOW_SYNTHETIC_NPC_SEEDS")
        .map(|v| {
            matches!(
                v.to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        })
        .unwrap_or(false)
    {
        return None;
    }
    std::env::var("TRPG_PROVISIONAL_OPPOSITION_HP")
        .ok()
        .and_then(|v| v.trim().parse::<i32>().ok())
        .filter(|hp| *hp > 0)
        .or(Some(10))
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
    fn metadata_value(&self, _key: &str) -> Option<String> {
        None
    }
    fn world_tick_hint(&self) -> i64 {
        0
    }
}

#[cfg(test)]
mod combat_check_classification_tests {
    use super::*;
    use serde_json::json;
    use sqlx::postgres::PgPoolOptions;

    fn dummy_service() -> RefereeCombatService {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://chatrpg:chatrpg@localhost:1/chatrpg")
            .expect("lazy pool");
        RefereeCombatService::new(Db { pool })
    }

    fn contract(intent: &str, label: &str, summary: &str, key: &str) -> CheckContract {
        CheckContract {
            check_id: "check_test".into(),
            session_id: "session_test".into(),
            turn_id: "turn_test".into(),
            ruleset_id: "call_of_cthulhu_7e".into(),
            module_id: None,
            initiator: ActorRef {
                actor_id: "pc.current".into(),
                actor_kind: ActorKind::PlayerCharacter,
                display_name: Some("Evelyn Wu".into()),
            },
            target_actor: Some(ActorRef {
                actor_id: "npc.opposition".into(),
                actor_kind: ActorKind::Npc,
                display_name: Some("持刀人影".into()),
            }),
            opposition: OppositionModel::NoMechanicalOpposition,
            action_summary: summary.into(),
            intent_kind: intent.into(),
            check_label: label.into(),
            dice_expression: "1d100".into(),
            modifiers: vec![],
            target: CheckTargetModel::StaticNumber {
                value: 90,
                label: "Firearms (Handgun)".into(),
            },
            tested_parameter: Some(TestedParameter {
                domain: Some(ParamDomain::Skill),
                key: key.into(),
                label: key.into(),
            }),
            opponent_tested_parameter: None,
            actor_snapshot_ids: vec![],
            source_refs: vec![],
            learned_packet_ids: vec![],
            roll_visibility: RollVisibility::PublicGmRoll,
            roll_authority: RollAuthority::System,
            disclosure: RollDisclosurePolicy::for_visibility(RollVisibility::PublicGmRoll),
            stakes: CheckStakes::default(),
            confidence: RulingConfidence::Medium,
            ruling_status: RulingStatus::SourceBacked,
            advice_refs: vec![],
            expires_at_turn: None,
        }
    }

    #[tokio::test]
    async fn firearms_m1911_shooting_is_classified_as_attack() {
        let c = contract(
            "agent_selected_check",
            "Firearms (Handgun) / Colt M1911 射击",
            "玩家用 Colt M1911 .45 automatic 开枪射击持刀人影",
            "Firearms (Handgun)",
        );
        assert!(
            dummy_service().is_attack_check(&c),
            "source-backed firearm shooting must create an attack follow-up"
        );

        let plan = make_roll_plan_from_check(
            &c.session_id,
            &c.turn_id,
            Some(&c.check_id),
            &c,
            "1d100",
            &json!({"total": 21}),
        );
        assert_eq!(plan.roll_kind, RollKind::Attack);
    }

    #[tokio::test]
    async fn initiative_with_weapon_context_is_not_attack() {
        let c = contract(
            "combat_order",
            "Combat initiative / DEX order",
            "玩家准备拔出 Colt M1911 开枪，但本检定只决定行动顺序",
            "DEX",
        );
        assert!(
            !dummy_service().is_attack_check(&c),
            "initiative/order checks must not spend ammo or create damage follow-ups"
        );

        let plan = make_roll_plan_from_check(
            &c.session_id,
            &c.turn_id,
            Some(&c.check_id),
            &c,
            "1d100",
            &json!({"total": 85}),
        );
        assert_ne!(plan.roll_kind, RollKind::Attack);
    }

    #[tokio::test]
    async fn effect_damage_roll_is_not_reclassified_as_attack() {
        let c = contract(
            "effect_roll",
            "M1911 damage / 伤害 1d10+2",
            "命中后的伤害骰",
            "damage",
        );
        assert!(
            !dummy_service().is_attack_check(&c),
            "damage/effect follow-ups must not recursively create attacks"
        );
    }

    // Aftermath/status mechanics whose labels mention a gun ("after gunshot")
    // must not be reclassified as attacks via the "shot" substring; otherwise a
    // status query seeds a spurious attack_resolution_contracts row.
    #[tokio::test]
    async fn zero_hp_state_after_gunshot_is_not_attack() {
        let c = contract(
            "agent_selected_check",
            "Zero HP state after gunshot",
            "持刀人影中枪倒地后的状态判定",
            "hit_points",
        );
        assert!(
            !dummy_service().is_attack_check(&c),
            "zero-HP/aftermath status checks must not create attack follow-ups"
        );
    }

    // Major-wound consciousness rolls and other status/recovery/dying labels are
    // consequences of an attack, never attacks themselves.
    #[tokio::test]
    async fn status_and_aftermath_labels_are_not_attacks() {
        for (label, summary, key) in [
            (
                "Major wound: remain conscious after damage",
                "重伤后是否保持清醒的判定",
                "con",
            ),
            ("Dying and consciousness", "濒死与意识判定", "con"),
            ("Unconscious recovery check", "昏迷恢复判定", "con"),
            ("Status query", "查询当前状态", "hit_points"),
            // Brief-required labels: each embeds an attack substring ("shot" /
            // "gunshot") yet is a status/aftermath consequence. They prove the
            // status guard independent of the effect/damage guard — none of
            // these contain "damage"/"effect".
            (
                "Major wound consciousness after the shot",
                "中枪后重伤是否保持清醒",
                "con",
            ),
            ("Recover from gunshot wound", "从枪伤中恢复", "hit_points"),
            ("Dying after the shot", "中枪后濒死判定", "con"),
            (
                "Status query: aftereffect of the gunshot wound",
                "查询中枪后遗症状态",
                "hit_points",
            ),
        ] {
            let c = contract("agent_selected_check", label, summary, key);
            assert!(
                !dummy_service().is_attack_check(&c),
                "status/aftermath label must not be an attack: {label}"
            );
        }
    }

    // record_attack must never fabricate or default a target. A check that is
    // classified attack-like but carries an explicit, distinct target actor
    // resolves to that target and is persisted.
    #[test]
    fn attack_with_distinct_target_resolves_target() {
        let c = contract(
            "attack",
            "Firearms (Handgun) / Colt M1911 射击",
            "玩家用 Colt M1911 开枪射击持刀人影",
            "Firearms (Handgun)",
        );
        assert_eq!(
            resolved_attack_target(&c),
            Some("npc.opposition".to_string()),
            "a distinct explicit target must be preserved"
        );
    }

    // No explicit target → no fabricated "npc.opposition" default → no attack
    // resolution row persisted.
    #[test]
    fn attack_with_missing_target_is_not_persisted() {
        let mut c = contract("attack", "Firearms shot", "开枪但没有锁定目标", "Firearms");
        c.target_actor = None;
        assert_eq!(
            resolved_attack_target(&c),
            None,
            "missing target must not be defaulted to a fabricated opponent id"
        );
    }

    // Source == target → self-attack → never persisted (the live regression was
    // npc.opposition attacking itself from a status label).
    #[test]
    fn self_attack_is_not_persisted() {
        let mut c = contract("attack", "Firearms shot", "对自身的伪攻击", "Firearms");
        c.initiator.actor_id = "npc.opposition".into();
        assert_eq!(
            c.initiator.actor_id,
            c.target_actor.as_ref().unwrap().actor_id,
            "test precondition: source equals target"
        );
        assert_eq!(
            resolved_attack_target(&c),
            None,
            "a self-attack (source == target) must not be persisted"
        );
    }
}

fn row_to_actor_state(r: sqlx::postgres::PgRow) -> ActorMechanicalState {
    let kind_str: String = r.get("actor_kind");
    let actor_kind = match kind_str.as_str() {
        "player_character" => ActorKind::PlayerCharacter,
        "environment" => ActorKind::Environment,
        "hazard" => ActorKind::Hazard,
        "system" => ActorKind::System,
        _ => ActorKind::Npc,
    };
    ActorMechanicalState {
        state_id: r.get("state_id"),
        session_id: r.get("session_id"),
        frame_id: r.get("frame_id"),
        actor_id: r.get("actor_id"),
        actor_kind,
        hp_current: r.get("hp_current"),
        hp_max: r.get("hp_max"),
        armor_current: r.get("armor_current"),
        wound_state: r.get("wound_state"),
        morale: r.get("morale"),
        resources: r.get("resources_json"),
        conditions: serde_json::from_value(r.get("conditions_json")).unwrap_or_default(),
        source_refs: serde_json::from_value(r.get("source_refs")).unwrap_or_default(),
        provisional_reason: r.get("provisional_reason"),
        world_tick: r.get::<Option<i64>, _>("world_tick").unwrap_or_default(),
        updated_at: r.get("updated_at"),
    }
}

#[cfg(test)]
mod on_outcome_match_amount_tests {
    use super::{
        check_match_fallback_fact, check_match_hay, dice_max, resolve_track_amount,
        track_matches_tested_parameter, trigger_fires,
    };
    use serde_json::json;
    use trpg_model::{StatePatch, TestedParameter};

    fn tp(key: &str) -> TestedParameter {
        TestedParameter {
            domain: None,
            key: key.into(),
            label: key.into(),
        }
    }

    // D2: the on_outcome 干草堆必须含 tested_parameter (key+label) —— 这样
    // 刻意绑定到 sanity track 的 check 即使念白里没"sanity"也能被 check_match 命中。
    #[test]
    fn hay_includes_tested_parameter() {
        let hay = check_match_hay(
            "named_parameter_check",
            "Sanity check",
            "/roll",
            Some(&tp("Sanity")),
        );
        assert!(
            hay.contains("sanity"),
            "tested_parameter 应进入干草堆: {hay}"
        );
    }

    #[test]
    fn hay_lowercased_and_joins_all_fields() {
        let hay = check_match_hay("INTENT", "Label", "Prose Here", None);
        assert_eq!(hay, "intent label  prose here");
    }

    // D3: tested_parameter 指向本 track → 是被刻意检定的参数。
    #[test]
    fn track_is_tested_when_parameter_matches_track_id() {
        let track =
            json!({"id":"sanity","name":"Sanity","on_outcome":[{"check_match":"sanity|san roll"}]});
        assert!(track_matches_tested_parameter(Some(&tp("Sanity")), &track));
    }

    #[test]
    fn track_is_tested_via_check_match_alias() {
        let track =
            json!({"id":"sanity","name":"Sanity","on_outcome":[{"check_match":"sanity|理智"}]});
        assert!(track_matches_tested_parameter(Some(&tp("理智")), &track));
    }

    #[test]
    fn unrelated_parameter_does_not_match_track() {
        let track =
            json!({"id":"sanity","name":"Sanity","on_outcome":[{"check_match":"sanity|san roll"}]});
        assert!(!track_matches_tested_parameter(
            Some(&tp("Spot Hidden")),
            &track
        ));
    }

    #[test]
    fn no_tested_parameter_means_not_tested() {
        let track = json!({"id":"sanity","name":"Sanity"});
        assert!(!track_matches_tested_parameter(None, &track));
    }

    // 护栏 §3.5.1：check_match regex 命中且契约无 "mechanic:" 结构化引用 →
    // 结算产物出可观测回退标注 fact（reason=check_match_fallback_observed）。
    #[test]
    fn check_match_regex_hit_emits_fallback_fact() {
        let rule = json!({"check_match": "sanity|理智", "trigger": "on_failure", "amount": "1d6"});
        let patch = check_match_fallback_fact(&rule, &[], "pc.current", "sanity", "Sanity check")
            .expect("regex 命中应产出回退标注");
        let StatePatch::CreateFact {
            target,
            fact,
            reason,
        } = patch
        else {
            panic!("应为 CreateFact")
        };
        assert_eq!(target, "pc.current");
        assert_eq!(reason, "check_match_fallback_observed");
        let s = fact.as_str().unwrap_or_default();
        assert!(
            s.starts_with("fallback:check_match"),
            "fact 前缀应为 fallback:check_match: {s}"
        );
        assert!(
            s.contains("'sanity'") && s.contains("'Sanity check'"),
            "fact 应含 track id 与 check label: {s}"
        );
    }

    // 结构化路径三分支均不标注：A5 band 触发 / mechanic: 引用 / 无 check_match。
    #[test]
    fn structured_paths_emit_no_fallback_fact() {
        let band_rule =
            json!({"check_match": "sanity", "trigger": {"kind": "band", "band_id": "fumble"}});
        assert!(
            check_match_fallback_fact(&band_rule, &[], "pc.current", "sanity", "SAN").is_none(),
            "A5 结构化 band 触发不打标注"
        );
        let rule = json!({"check_match": "sanity", "trigger": "on_failure"});
        assert!(
            check_match_fallback_fact(
                &rule,
                &["mechanic:sanity_loss".to_string()],
                "pc.current",
                "sanity",
                "SAN"
            )
            .is_none(),
            "mechanic: 结构化引用不打标注"
        );
        let no_cm = json!({"trigger": "on_failure"});
        assert!(
            check_match_fallback_fact(&no_cm, &[], "pc.current", "sanity", "SAN").is_none(),
            "无 check_match 未走字符串扫描不打标注"
        );
    }

    // D3: =<field> 读 outcome.<field>。
    #[test]
    fn equals_field_reads_named_outcome_field() {
        let outcome = json!({"sanity_loss": 4, "success": false});
        assert_eq!(
            resolve_track_amount(
                Some("=sanity_loss"),
                None,
                None,
                Some("1d6"),
                true,
                &outcome
            ),
            Some(4)
        );
    }

    // D3 核心: 字段缺失 + 该 track 是被测参数 → 回退到可配置默认骰。
    #[test]
    fn missing_field_falls_back_to_default_die_when_tested() {
        let outcome = json!({"success": false});
        let v = resolve_track_amount(
            Some("=sanity_loss"),
            None,
            None,
            Some("1d6"),
            true,
            &outcome,
        );
        let v = v.expect("被测参数缺字段应回退默认骰");
        assert!((1..=6).contains(&v), "1d6 应在 1..=6: {v}");
    }

    // D3 护栏: 字段缺失但本 track 不是被测参数 → 不落 delta (防止念白里偶现关键词误扣)。
    #[test]
    fn missing_field_no_fallback_when_not_tested() {
        let outcome = json!({"success": false});
        assert_eq!(
            resolve_track_amount(
                Some("=sanity_loss"),
                None,
                None,
                Some("1d6"),
                false,
                &outcome
            ),
            None
        );
    }

    // =value 仍读 `when` 指定的 outcome 字段 (向后兼容)。
    #[test]
    fn equals_value_reads_when_field() {
        let outcome = json!({"total": 3});
        assert_eq!(
            resolve_track_amount(Some("=value"), None, Some("total"), None, true, &outcome),
            Some(3)
        );
    }

    // 普通骰 / 定值 / 裸 when 字段 三条旧路径不变。
    #[test]
    fn plain_dice_and_fixed_int_and_bare_when_field() {
        let outcome = json!({"success_count": 5});
        assert_eq!(
            resolve_track_amount(Some("1d1"), None, None, None, false, &outcome),
            Some(1)
        );
        assert_eq!(
            resolve_track_amount(Some("2"), None, None, None, false, &outcome),
            Some(2)
        );
        assert_eq!(
            resolve_track_amount(None, None, Some("success_count"), None, false, &outcome),
            Some(5)
        );
    }

    // legacy delta 字段保持原语义。
    #[test]
    fn legacy_delta_field_preserved() {
        let outcome = json!({"success_count": 2});
        assert_eq!(
            resolve_track_amount(
                None,
                Some("=value"),
                Some("success_count"),
                None,
                false,
                &outcome
            ),
            Some(2)
        );
        assert_eq!(
            resolve_track_amount(None, Some("3"), None, None, false, &outcome),
            Some(3)
        );
    }

    // ===== A5: on_outcome 结构化 band 触发 + amount max_of =====

    // 回归锚：既有字符串词表（always/on_success/on_failure/on_tier）行为零变化。
    #[test]
    fn string_triggers_behave_unchanged() {
        let ok = json!({"success": true, "success_tier": "hard", "success_tier_rank": 2});
        let fail = json!({"success": false});
        // on_success 正反两断言：success=true 触发 / false 不触发。
        assert!(trigger_fires(&json!({"trigger": "on_success"}), &ok));
        assert!(!trigger_fires(&json!({"trigger": "on_success"}), &fail));
        assert!(trigger_fires(&json!({"trigger": "on_failure"}), &fail));
        assert!(!trigger_fires(&json!({"trigger": "on_failure"}), &ok));
        assert!(trigger_fires(&json!({"trigger": "always"}), &fail));
        // on_tier 的 tier/min_rank 对账原样。
        assert!(trigger_fires(
            &json!({"trigger": "on_tier", "tier": "hard"}),
            &ok
        ));
        assert!(!trigger_fires(
            &json!({"trigger": "on_tier", "tier": "extreme"}),
            &ok
        ));
        assert!(trigger_fires(
            &json!({"trigger": "on_tier", "min_rank": 2}),
            &ok
        ));
        assert!(!trigger_fires(
            &json!({"trigger": "on_tier", "min_rank": 3}),
            &ok
        ));
    }

    // 老 kernel 裸 rule 行为锁死：无 trigger 键 → 照常触发（缺失 ≠ 认不出）。
    #[test]
    fn missing_trigger_defaults_to_always() {
        assert!(trigger_fires(
            &json!({"op": "subtract"}),
            &json!({"success": false})
        ));
    }

    // 结构化 band 触发：与结算器写的 outcome.success_tier 同词汇表对账。
    #[test]
    fn band_trigger_matches_outcome_band() {
        let rule = json!({"trigger": {"kind": "band", "band_id": "fumble"}});
        assert!(
            trigger_fires(&rule, &json!({"success_tier": "fumble"})),
            "band 命中应触发(资源被改)"
        );
        assert!(
            !trigger_fires(&rule, &json!({"success_tier": "regular"})),
            "band 不匹配不触发"
        );
    }

    #[test]
    fn band_trigger_case_insensitive() {
        let rule = json!({"trigger": {"kind": "band", "band_id": "Fumble"}});
        assert!(trigger_fires(&rule, &json!({"success_tier": "fumble"})));
    }

    // 验收 12② 后半：认不出的 trigger → 该 rule 跳过（绝不回落 always），
    // 同 track 其余 rule 照常、不 panic。
    #[test]
    fn unrecognized_trigger_object_skips_rule() {
        let outcome = json!({"success": true, "success_tier": "regular"});
        assert!(!trigger_fires(
            &json!({"trigger": {"kind": "phase_of_moon"}}),
            &outcome
        ));
        assert!(!trigger_fires(&json!({"trigger": 42}), &outcome));
        // 同 track 其余（合法 trigger 的）rule 照常触发。
        assert!(trigger_fires(&json!({"trigger": "always"}), &outcome));
    }

    // 骰式最大值：确定性、与 roll_amount_dice 同一 NdM±K 解析规则。
    #[test]
    fn dice_max_forms() {
        assert_eq!(dice_max("1d10"), Some(10));
        assert_eq!(dice_max("2d6"), Some(12));
        assert_eq!(dice_max("2d6+3"), Some(15));
        assert_eq!(dice_max("2d6-1"), Some(11));
        assert_eq!(dice_max("d8"), Some(8));
        assert_eq!(dice_max("7"), Some(7));
        assert_eq!(dice_max("max(0,x)"), None);
        assert_eq!(dice_max(""), None);
    }

    // fumble 掉最大值由 prose 变机械事实的最小证据：确定性断言，无随机容差。
    #[test]
    fn amount_max_of_resolves_max() {
        let outcome = json!({"success": false});
        assert_eq!(
            resolve_track_amount(Some("max_of:1d10"), None, None, None, false, &outcome),
            Some(10)
        );
    }

    // 认不出的 max_of 实参沿既有 None 语义：被测参数走 default_amount 兜底，否则跳过。
    #[test]
    fn amount_max_of_garbage_falls_back() {
        let outcome = json!({"success": false});
        let v = resolve_track_amount(
            Some("max_of:garbage"),
            None,
            None,
            Some("1d6"),
            true,
            &outcome,
        );
        let v = v.expect("track_is_tested=true 应走 default_amount 兜底");
        assert!((1..=6).contains(&v), "1d6 应在 1..=6: {v}");
        assert_eq!(
            resolve_track_amount(
                Some("max_of:garbage"),
                None,
                None,
                Some("1d6"),
                false,
                &outcome
            ),
            None
        );
    }
}

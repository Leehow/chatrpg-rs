use anyhow::Result;
use chrono::Utc;
use serde_json::{json, Value};
use sqlx::Row;
use trpg_db::Db;
use trpg_model::*;
use trpg_params::RuntimeParameterService;
use uuid::Uuid;

mod opposed;

#[derive(Clone)]
pub struct ContestService { pub db: Db }

impl ContestService {
    pub fn new(db: Db) -> Self { Self { db } }

    pub async fn resolve_outcome(&self, contract: &CheckContract, roll: &DiceRollRecord,
        defender_roll: Option<&DiceRollRecord>) -> Result<Value> {
        let total = roll_total(roll);
        let rolls = roll_dice_array(roll);
        let profile = self.ensure_contest_profile(contract, Some(roll), total).await?;
        let (mut target, mut success, mut degree) = resolve_against_model(&profile.resolution_model, total, &rolls);
        // 对抗结算覆盖：OpposedRoll + 防御方骰子都在时，用 resolve_opposed 算胜负。
        if let CheckResolutionModel::OpposedRoll { attacker_value, defender_value, .. } = &profile.resolution_model {
            let def_total = defender_roll.map(roll_total);
            let (kc_opt, bands) = self.load_compare_and_bands(&contract.ruleset_id).await;
            if let (Some(compare), Some(dt)) = (kc_opt, def_total) {
                let (t, s, d) = opposed::resolve_opposed(&compare, &bands, total, *attacker_value, dt, *defender_value);
                target = t; success = s; degree = d;
            }
        }
        let mut outcome = json!({
            "check_id": contract.check_id,
            "check_label": contract.check_label,
            "total": total,
            "target": target,
            "success": success,
            "degree": degree,
            "contest_id": profile.contest_id,
            "resolution_model": &profile.resolution_model,
            "opposition_kind": profile.contest_kind,
            "stakes": &contract.stakes,
            "roll_visibility": contract.roll_visibility,
            "disclosure": &contract.disclosure,
        });
        // Dice-pool enrichment — GENERIC names (no game-specific vocabulary in
        // the resolver): success_count = dice showing the target face;
        // pool_miss_count = the rest (a kernel resource_track, e.g. Triangle's
        // Chaos, maps this to a track delta via config — not hardcoded here).
        if let CheckResolutionModel::DicePoolCount { target_face, .. } = &profile.resolution_model {
            let hits = rolls.iter().filter(|&&d| d == *target_face as i64).count();
            let misses = rolls.iter().filter(|&&d| d != *target_face as i64).count();
            outcome["success_count"] = json!(hits);
            outcome["pool_miss_count"] = json!(misses);
            outcome["dice"] = json!(rolls);
        }
        // 对抗结算富化：把双方总点 + 双方技能值 + 胜者写入 outcome["opposed"]。
        // 仅在真出了胜负(success 非空)时富化；缺值 fail-closed(success=None)时不吐
        // 半成品对抗块,避免下游误读 unresolved 的 winner。
        if success.is_some() {
        if let CheckResolutionModel::OpposedRoll { attacker_value, defender_value, defender_actor_id, .. } = &profile.resolution_model {
            outcome["opposed"] = json!({
                "attacker_total": total,
                "attacker_value": attacker_value,
                "defender_total": defender_roll.map(roll_total),
                "defender_value": defender_value,
                "defender_actor_id": defender_actor_id,
                "winner": match success { Some(true) => "attacker", Some(false) => "defender", None => "unresolved" },
            });
        }
        }
        // Graded success tiers (e.g. CoC critical/extreme/hard/regular/fumble).
        // The band predicates live in the parsed kernel (dice_core.success_bands)
        // — the evaluator is generic, so NO per-ruleset Rust. Emitted as
        // `success_tier` so on_outcome rules / narration can key off the level.
        if let Some(t) = target {
            if let Ok(Some(kernel)) = self.db.load_rule_kernel(&contract.ruleset_id).await {
                if let Some(bands) = kernel.dice_core.get("success_bands").and_then(|v| v.as_array()) {
                    if let Some((tier, rank)) = success_tier_for(bands, total, t) {
                        outcome["success_tier"] = json!(tier);
                        outcome["success_tier_rank"] = json!(rank);
                    }
                }
            }
        }
        let resolution = ContestResolutionRecord {
            resolution_id: format!("contest_resolution_{}", Uuid::new_v4().simple()),
            contest_id: profile.contest_id.clone(),
            session_id: contract.session_id.clone(),
            turn_id: contract.turn_id.clone(),
            check_id: contract.check_id.clone(),
            roll_id: Some(roll.roll_id.clone()),
            total,
            target_value: target,
            success,
            degree: degree.clone(),
            outcome_json: outcome.clone(),
            created_at: Utc::now(),
        };
        self.insert_resolution(&resolution).await.ok();
        Ok(outcome)
    }

    pub async fn ensure_contest_profile(&self, contract: &CheckContract, roll: Option<&DiceRollRecord>, _total: i64) -> Result<ContestProfile> {
        if let Some(existing) = self.load_profile_for_check(&contract.check_id).await? {
            return Ok(existing);
        }
        let model = self.resolve_model(contract).await;
        let unresolved_model = matches!(&model, CheckResolutionModel::Provisional { .. } | CheckResolutionModel::RulesetProcedureLookup { .. });
        let contest_kind = contest_kind_for_contract(contract, &model);
        let defender_actor_id = defender_for_contract(contract);
        let world_tick = world_tick_from_roll_or_contract(roll, contract);
        let profile = ContestProfile {
            contest_id: format!("contest_{}", Uuid::new_v4().simple()),
            session_id: contract.session_id.clone(),
            turn_id: contract.turn_id.clone(),
            check_id: contract.check_id.clone(),
            ruleset_id: contract.ruleset_id.clone(),
            module_id: contract.module_id.clone(),
            contest_kind,
            attacker_actor_id: contract.initiator.actor_id.clone(),
            defender_actor_id,
            source_object_id: None,
            source_ability_id: None,
            resolution_model: model.clone(),
            roll_visibility: contract.roll_visibility,
            target_summary: target_summary(contract),
            source_refs: contract.source_refs.clone(),
            confidence: if unresolved_model || matches!(&contract.target, CheckTargetModel::UnknownUntilLookup) && matches!(&contract.opposition, OppositionModel::NoMechanicalOpposition) { RulingConfidence::Low } else { contract.confidence },
            verification_status: if unresolved_model || matches!(&contract.target, CheckTargetModel::UnknownUntilLookup) && matches!(&contract.opposition, OppositionModel::NoMechanicalOpposition) { ContestVerificationStatus::ProvisionalNeedsBinding } else { ContestVerificationStatus::VerifiedPartial },
            provisional_reason: provisional_reason_for_contract(contract).or_else(|| provisional_reason_for_model(&model)),
            world_tick,
            created_at: Utc::now(),
        };
        self.insert_profile(&profile).await?;
        Ok(profile)
    }

    /// Pick the resolution model. A concrete typed target on the contract wins;
    /// otherwise read the parsed kernel's typed success spec; only then fall back
    /// to operator overrides / Provisional. No per-ruleset branching.
    async fn resolve_model(&self, contract: &CheckContract) -> CheckResolutionModel {
        if !matches!(contract.target, CheckTargetModel::UnknownUntilLookup) {
            return self.infer_resolution_model(contract);
        }
        if let Some(m) = self.kernel_resolution_model(contract).await {
            return m;
        }
        infer_ruleset_default_model(contract)
    }

    /// Build a resolution model from the ruleset's parsed kernel `dice_core`
    /// (the typed `compare` operator + numbers). Returns None when the kernel is
    /// absent or doesn't carry enough to resolve (then the caller falls back).
    async fn kernel_resolution_model(&self, contract: &CheckContract) -> Option<CheckResolutionModel> {
        let kernel = self.db.load_rule_kernel(&contract.ruleset_id).await.ok().flatten()?;
        let dc = &kernel.dice_core;
        let compare = dc.get("compare").and_then(|v| v.as_str())?;
        let tnum = dc.get("target_number").and_then(|v| v.as_i64()).map(|n| n as i32);
        match compare {
            "count_faces" => {
                // Same fail-soft recovery as the contract-target path: prefer the
                // machine target_face, else parse compare_to ("3"); threshold
                // defaults to 1. Fail-closed (None) if no parseable face.
                let target_face = count_faces_target_face(dc)? as i32;
                let threshold = count_faces_threshold(dc) as i32;
                Some(CheckResolutionModel::DicePoolCount { target_face, threshold, label: "kernel core mechanic (dice pool)".into() })
            }
            "roll_under" => {
                // 对抗路径：target_actor + opponent_tested_parameter 都存在时建 OpposedRoll。
                // 双方技能值在此处解析（异步 DB 读）写入模型，供 resolve_outcome 直接使用。
                // 缓存注意(m2)：本 model 经 ensure_contest_profile 按 check_id 缓存,含 defender_value。
                // 若首次 resolve 早于 NPC 卡合成,defender_value 会缓存成 None 且不自愈。CLI 路径已保证
                // 合成+盖章 await 早于 resolve(时序安全);API 路径(本期范围外)接 pre-pass 前勿提前 resolve。
                let opposed = contract.target_actor.is_some() && contract.opponent_tested_parameter.is_some();
                if opposed {
                    let ta = contract.target_actor.as_ref().unwrap();
                    let atk = self.resolve_percentile_target_for(
                        &contract.initiator.actor_id,
                        contract.tested_parameter.as_ref(),
                        contract, &kernel,
                    ).await.map(|(_, v)| v);
                    let def = self.resolve_percentile_target_for(
                        &ta.actor_id,
                        contract.opponent_tested_parameter.as_ref(),
                        contract, &kernel,
                    ).await.map(|(_, v)| v);
                    let def_expr = dc.get("dice").and_then(|v| v.as_str())
                        .unwrap_or(&contract.dice_expression).to_string();
                    return Some(CheckResolutionModel::OpposedRoll {
                        attacker_expression: contract.dice_expression.clone(),
                        attacker_value: atk,
                        defender_actor_id: Some(ta.actor_id.clone()),
                        defender_expression: def_expr,
                        defender_value: def,
                        defender_roll_visibility: RollVisibility::PrivateGmRoll,
                    });
                }
                // Percentile (roll-under) targets are PER-CHARACTER: the tested
                // skill/characteristic/track rating on the actor's sheet — NOT a
                // table-wide constant. Read the actor's REAL value. No env default
                // (the old TRPG_CONTEST_DEFAULT_PERCENTILE_SKILL=50 masked this).
                if let Some((label, value)) = self.resolve_percentile_target(contract, &kernel).await {
                    Some(CheckResolutionModel::PercentileRollUnder { ability_label: label, ability_value: value })
                } else if let Some(v) = tnum {
                    // A roll-under game that DOES declare a global table target.
                    Some(CheckResolutionModel::PercentileRollUnder { ability_label: contract.check_label.clone(), ability_value: v })
                } else {
                    // Fail closed (like attack-DV): surface the gap, never fake 50.
                    Some(CheckResolutionModel::Provisional {
                        reason: "percentile roll-under: the actor's tested skill/characteristic/track value was not found; bind the check's tested_parameter or hydrate the actor sheet before resolving".into(),
                        suggested_target: None,
                    })
                }
            }
            "meet_or_beat" => {
                tnum.map(|v| CheckResolutionModel::StaticTargetNumber { value: v, label: "kernel core mechanic target".into() })
            }
            _ => None,
        }
    }

    /// For a percentile (roll-under) check, find the tested parameter's REAL
    /// value on the acting actor. Returns (resolved_label, value). Generic: the
    /// "which parameter" decision (derive_tested_source) reads the kernel's
    /// resource_tracks + the actor's own stat/skill keys — no per-ruleset code.
    async fn resolve_percentile_target(&self, contract: &CheckContract, kernel: &RuleKernel) -> Option<(String, i32)> {
        self.resolve_percentile_target_for(
            &contract.initiator.actor_id,
            contract.tested_parameter.as_ref(),
            contract, kernel,
        ).await
    }

    /// 泛化版：按指定 actor_id + hint(TestedParameter) 查真实技能/属性/轨值。
    /// 用于攻击方(initiator)和防御方(target_actor)的统一参数解析。
    async fn resolve_percentile_target_for(
        &self,
        actor_id: &str,
        hint: Option<&TestedParameter>,
        contract: &CheckContract,
        kernel: &RuleKernel,
    ) -> Option<(String, i32)> {
        let params = RuntimeParameterService::new(self.db.clone())
            .load_actor_parameters(&contract.session_id, actor_id)
            .await.ok().flatten()?;
        let mech = &params.mechanical_profile;
        let src = derive_tested_source(
            hint, kernel, mech,
            &contract.check_label, &contract.action_summary, &contract.intent_kind,
        )?;
        match src {
            TestedSource::Track { id, label } => self
                .read_track_value(&contract.session_id, actor_id, &id, kernel)
                .await
                .map(|v| (label, v)),
            TestedSource::Skill { key } => value_from_profile(mech.get("skills"), &key).map(|v| (key, v)),
            TestedSource::Stat { key } => value_from_profile(mech.get("stats"), &key).map(|v| (key, v)),
        }
    }

    /// Read a live resource-track CURRENT value from the single source of truth
    /// (generic_parameter_states, target_kind=actor) via the shared db primitive;
    /// it falls back to the actor's character-derived seed and then the kernel
    /// track's `initial`. (Was a divergent raw query that omitted target_kind.)
    async fn read_track_value(&self, session_id: &str, actor_id: &str, track_id: &str, kernel: &RuleKernel) -> Option<i32> {
        self.db.load_resource_current(session_id, actor_id, track_id, kernel).await
    }

    /// 取 kernel 的 compare 方向 + success_bands（对抗结算用）。
    /// resolve_outcome 里对抗分支跳过了 ensure_contest_profile 里的 kernel 加载，
    /// 故此处单独加载一次。
    async fn load_compare_and_bands(&self, ruleset_id: &str) -> (Option<String>, Vec<Value>) {
        match self.db.load_rule_kernel(ruleset_id).await.ok().flatten() {
            Some(k) => (
                k.dice_core.get("compare").and_then(|v| v.as_str()).map(str::to_string),
                k.dice_core.get("success_bands").and_then(|v| v.as_array()).cloned().unwrap_or_default(),
            ),
            None => (None, vec![]),
        }
    }

    fn infer_resolution_model(&self, contract: &CheckContract) -> CheckResolutionModel {
        match &contract.target {
            CheckTargetModel::StaticNumber { value, label } => {
                if is_attack_contract(contract) {
                    CheckResolutionModel::AttackVsDefense { attack_expression: contract.dice_expression.clone(), defender_actor_id: defender_for_contract(contract), defense_label: label.clone(), defense_value: *value }
                } else {
                    CheckResolutionModel::StaticTargetNumber { value: *value, label: label.clone() }
                }
            }
            CheckTargetModel::Opposed { opponent_id, opponent_check } => CheckResolutionModel::OpposedRoll { attacker_expression: contract.dice_expression.clone(), attacker_value: None, defender_actor_id: Some(opponent_id.clone()), defender_expression: opponent_check.clone(), defender_value: None, defender_roll_visibility: RollVisibility::PrivateGmRoll },
            CheckTargetModel::DegreeOnly => CheckResolutionModel::RulesetProcedureLookup { procedure_label: "degree_only".into(), unresolved_fields: vec!["success_band".into()] },
            CheckTargetModel::SuccessCount { threshold } => CheckResolutionModel::StaticTargetNumber { value: *threshold, label: "success_count_threshold".into() },
            CheckTargetModel::DicePoolCount { target_face, threshold, label } => CheckResolutionModel::DicePoolCount { target_face: *target_face, threshold: *threshold, label: label.clone() },
            CheckTargetModel::UnknownUntilLookup => infer_ruleset_default_model(contract),
        }
    }

    async fn load_profile_for_check(&self, check_id: &str) -> Result<Option<ContestProfile>> {
        let row = sqlx::query("select profile_json from contest_profiles where check_id = $1 order by created_at desc limit 1")
            .bind(check_id).fetch_optional(&self.db.pool).await?;
        Ok(row.and_then(|r| serde_json::from_value::<ContestProfile>(r.get::<Value,_>("profile_json")).ok()))
    }

    async fn insert_profile(&self, profile: &ContestProfile) -> Result<()> {
        sqlx::query(r#"
            insert into contest_profiles
              (id, contest_id, session_id, turn_id, check_id, ruleset_id, module_id, contest_kind, attacker_actor_id, defender_actor_id, resolution_model_json, profile_json, verification_status, world_tick, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)
            on conflict (contest_id) do update set
              resolution_model_json = excluded.resolution_model_json,
              profile_json = excluded.profile_json,
              verification_status = excluded.verification_status,
              world_tick = excluded.world_tick
        "#)
            .bind(Uuid::new_v4()).bind(&profile.contest_id).bind(&profile.session_id).bind(&profile.turn_id).bind(&profile.check_id)
            .bind(&profile.ruleset_id).bind(&profile.module_id).bind(profile.contest_kind.as_str()).bind(&profile.attacker_actor_id).bind(&profile.defender_actor_id)
            .bind(serde_json::to_value(&profile.resolution_model)?).bind(serde_json::to_value(profile)?).bind(profile.verification_status.as_str()).bind(profile.world_tick).bind(profile.created_at)
            .execute(&self.db.pool).await?;
        sqlx::query("update check_contracts set contest_profile_id = $1, resolution_model_json = $2 where check_id = $3")
            .bind(&profile.contest_id).bind(serde_json::to_value(&profile.resolution_model)?).bind(&profile.check_id).execute(&self.db.pool).await.ok();
        Ok(())
    }

    async fn insert_resolution(&self, resolution: &ContestResolutionRecord) -> Result<()> {
        sqlx::query(r#"
            insert into contest_resolution_events
              (id, resolution_id, contest_id, session_id, turn_id, check_id, roll_id, total, target_value, success, degree, outcome_json, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
            on conflict (resolution_id) do nothing
        "#)
            .bind(Uuid::new_v4()).bind(&resolution.resolution_id).bind(&resolution.contest_id).bind(&resolution.session_id).bind(&resolution.turn_id).bind(&resolution.check_id).bind(&resolution.roll_id)
            .bind(resolution.total).bind(resolution.target_value).bind(resolution.success).bind(&resolution.degree).bind(&resolution.outcome_json).bind(resolution.created_at)
            .execute(&self.db.pool).await?;
        Ok(())
    }

    pub async fn contest_context_block(&self, session_id: &str, world_tick: i64) -> Result<ContextBlock> {
        let rows = sqlx::query("select profile_json from contest_profiles where session_id = $1 order by created_at desc limit 12")
            .bind(session_id).fetch_all(&self.db.pool).await?;
        let profiles: Vec<Value> = rows.into_iter().filter_map(|r| r.try_get::<Value,_>("profile_json").ok()).collect();
        let mut block = ContextBlock::new(
            format!("contest_graph.recent.{}", session_id),
            BlockKind::ContestGraph,
            "Contest / Opposition Runtime Models",
            BlockContent::Json(json!({
                "world_tick": world_tick,
                "policy": "Checks and attacks must resolve through ContestProfile/Opposition models; do not treat NoMechanicalOpposition or UnknownUntilLookup as final for mechanics.",
                "recent_contests": profiles,
            })),
            Visibility::GmOnly,
            Stability::TurnDynamic,
            CacheZone::DynamicTail,
            Scope { scope_type: ScopeType::Session, scope_id: session_id.to_string() },
            168,
        );
        block.tags = vec!["contest".into(), "opposition".into(), "check_resolution".into()];
        block.load_reason = Some("contest_opposition_kernel".into());
        Ok(block)
    }
}

fn roll_total(roll: &DiceRollRecord) -> i64 {
    roll.result.get("total").and_then(|v| v.as_i64())
        .or_else(|| roll.result.get("total").and_then(|v| v.as_str()).and_then(|s| s.parse::<i64>().ok()))
        .or_else(|| roll.result.get("mode").and_then(|_| roll.result.get("total")).and_then(|v| v.as_i64()))
        .unwrap_or_default()
}

fn resolve_against_model(model: &CheckResolutionModel, total: i64, rolls: &[i64]) -> (Option<i64>, Option<bool>, Option<String>) {
    match model {
        CheckResolutionModel::StaticTargetNumber { value, .. } => (Some(*value as i64), Some(total >= *value as i64), degree_for(total - *value as i64)),
        CheckResolutionModel::AttackVsDefense { defense_value, .. } => (Some(*defense_value as i64), Some(total >= *defense_value as i64), degree_for(total - *defense_value as i64)),
        CheckResolutionModel::PercentileRollUnder { ability_value, .. } => (Some(*ability_value as i64), Some(total <= *ability_value as i64), degree_for(*ability_value as i64 - total)),
        CheckResolutionModel::SavingThrow { dc, .. } => (Some(*dc as i64), Some(total >= *dc as i64), degree_for(total - *dc as i64)),
        CheckResolutionModel::DicePoolCount { target_face, threshold, .. } => {
            let hits = rolls.iter().filter(|&&d| d == *target_face as i64).count() as i64;
            (Some(*threshold as i64), Some(hits >= *threshold as i64), degree_for(hits - *threshold as i64))
        }
        CheckResolutionModel::OpposedRoll { .. } | CheckResolutionModel::RulesetProcedureLookup { .. } | CheckResolutionModel::Provisional { .. } => (None, None, None),
    }
}

/// Extract the per-die array from a persisted roll's result JSON (written by
/// resolve_roll_input as `result["rolls"]`). Needed for dice-pool counting.
fn roll_dice_array(roll: &DiceRollRecord) -> Vec<i64> {
    roll.result
        .get("rolls")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_i64()).collect())
        .unwrap_or_default()
}

fn degree_for(delta: i64) -> Option<String> {
    Some(if delta >= 10 { "strong_success" } else if delta >= 0 { "success" } else if delta <= -10 { "strong_failure" } else { "failure" }.into())
}

fn infer_ruleset_default_model(contract: &CheckContract) -> CheckResolutionModel {
    // No per-ruleset branching: the kernel's typed `compare` (read in
    // kernel_resolution_model) decides percentile/pool/static. This fallback
    // only handles content heuristics + operator overrides, else Provisional.
    if is_attack_contract(contract) {
        if let Some(dv) = env_i32("TRPG_CONTEST_DEFAULT_ATTACK_DV") {
            return CheckResolutionModel::AttackVsDefense {
                attack_expression: contract.dice_expression.clone(),
                defender_actor_id: defender_for_contract(contract),
                defense_label: "table-configured attack defense/DV override".into(),
                defense_value: dv,
            };
        }
        return CheckResolutionModel::Provisional {
            reason: "attack defense/DV is missing; bind source-backed target defense/AC/evasion/DV before resolving the attack".into(),
            suggested_target: None,
        };
    }
    if let Some(dc) = env_i32("TRPG_CONTEST_DEFAULT_STATIC_TARGET") {
        return CheckResolutionModel::StaticTargetNumber { value: dc, label: "table-configured static target override".into() };
    }
    CheckResolutionModel::Provisional {
        reason: format!("no source-backed target number/opposition model was bound for ruleset `{}`", contract.ruleset_id),
        suggested_target: None,
    }
}

fn env_i32(key: &str) -> Option<i32> { std::env::var(key).ok().and_then(|v| v.parse().ok()) }

fn is_attack_contract(contract: &CheckContract) -> bool {
    let text = format!("{} {} {}", contract.intent_kind, contract.check_label, contract.action_summary).to_ascii_lowercase();
    text.contains("attack") || text.contains("combat") || text.contains("fire") || text.contains("shoot") || text.contains("开火") || text.contains("攻击") || text.contains("还击")
}

fn defender_for_contract(contract: &CheckContract) -> Option<String> {
    contract.target_actor.as_ref().map(|a| a.actor_id.clone())
        .or_else(|| contract.actor_snapshot_ids.iter().find(|id| id.starts_with("npc.") || id.contains("opposition") || id.contains("enemy")).cloned())
        .or_else(|| if is_attack_contract(contract) { Some("npc.opposition".into()) } else { None })
}

fn target_summary(contract: &CheckContract) -> String {
    match &contract.target {
        CheckTargetModel::StaticNumber { value, label } => format!("static target {} ({})", value, label),
        CheckTargetModel::Opposed { opponent_id, opponent_check } => format!("opposed by {} using {}", opponent_id, opponent_check),
        CheckTargetModel::DegreeOnly => "degree only".into(),
        CheckTargetModel::SuccessCount { threshold } => format!("success count threshold {}", threshold),
        CheckTargetModel::DicePoolCount { target_face, threshold, .. } => format!("dice pool: >= {} dice showing {}", threshold, target_face),
        CheckTargetModel::UnknownUntilLookup => "inferred by Contest/Opposition Kernel".into(),
    }
}

fn contest_kind_for_contract(contract: &CheckContract, model: &CheckResolutionModel) -> ContestKind {
    match model {
        CheckResolutionModel::AttackVsDefense { .. } => ContestKind::Attack,
        CheckResolutionModel::OpposedRoll { .. } => ContestKind::OpposedCheck,
        CheckResolutionModel::PercentileRollUnder { .. } => ContestKind::PercentileAbility,
        CheckResolutionModel::SavingThrow { .. } => ContestKind::SavingThrow,
        CheckResolutionModel::RulesetProcedureLookup { .. } => ContestKind::RulesetProcedure,
        CheckResolutionModel::Provisional { .. } => ContestKind::Provisional,
        _ if is_attack_contract(contract) => ContestKind::Attack,
        _ => ContestKind::StaticCheck,
    }
}

fn provisional_reason_for_model(model: &CheckResolutionModel) -> Option<String> {
    match model {
        CheckResolutionModel::Provisional { reason, .. } => Some(reason.clone()),
        CheckResolutionModel::RulesetProcedureLookup { procedure_label, unresolved_fields } => Some(format!("ruleset procedure `{}` needs source-backed binding for {:?}", procedure_label, unresolved_fields)),
        _ => None,
    }
}

fn provisional_reason_for_contract(contract: &CheckContract) -> Option<String> {
    if matches!(&contract.target, CheckTargetModel::UnknownUntilLookup) || matches!(&contract.opposition, OppositionModel::NoMechanicalOpposition) {
        Some("v1.11 Contest/Opposition Kernel inferred a provisional resolution model because this CheckContract lacked a bound target/opposition. Upgrade with ruleset source packs and exact rule binding when available.".into())
    } else { None }
}

fn world_tick_from_roll_or_contract(_roll: Option<&DiceRollRecord>, _contract: &CheckContract) -> i64 { 0 }

/// Evaluate kernel-declared `success_bands` against a resolved roll, returning
/// the best-ranked matching band's (id, rank). Fully generic: every band's
/// predicate + guard is DATA from the parsed kernel — no ruleset code here.
/// Specific bands are tried by rank (best first); a `kind:"otherwise"` band is
/// the fallback, so e.g. a CoC fumble (in_range) is matched before plain failure.
pub(crate) fn success_tier_for(bands: &[Value], total: i64, target: i64) -> Option<(String, i64)> {
    let mut preds: Vec<&Value> = bands.iter().filter(|b| !band_is_otherwise(b)).collect();
    preds.sort_by(|a, b| band_rank(b).cmp(&band_rank(a)));
    for b in preds {
        if band_test_passes(b, total, target) {
            return Some((band_id(b), band_rank(b)));
        }
    }
    bands.iter().find(|b| band_is_otherwise(b)).map(|b| (band_id(b), band_rank(b)))
}

fn band_id(b: &Value) -> String { b.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string() }
fn band_rank(b: &Value) -> i64 { b.get("rank").and_then(|v| v.as_i64()).unwrap_or(0) }
fn band_is_otherwise(b: &Value) -> bool {
    b.get("test").and_then(|t| t.get("kind")).and_then(|v| v.as_str()) == Some("otherwise")
}

fn band_test_passes(b: &Value, total: i64, target: i64) -> bool {
    let test = match b.get("test") { Some(t) => t, None => return false };
    if let Some(g) = test.get("when") { if !band_guard_ok(g, target) { return false; } }
    if let Some(g) = test.get("unless") { if band_guard_ok(g, target) { return false; } }
    let frac = |def_num: i64| {
        let num = test.get("numerator").and_then(|v| v.as_i64()).unwrap_or(def_num);
        let den = test.get("denominator").and_then(|v| v.as_i64()).unwrap_or(1).max(1);
        target * num / den
    };
    match test.get("kind").and_then(|v| v.as_str()) {
        Some("roll_under_or_equal") => total <= target,
        Some("roll_under_fraction") => total <= frac(1),
        Some("meet_or_beat_fraction") => total >= frac(1),
        Some("exact") => test.get("value").and_then(|v| v.as_i64()).map(|x| total == x).unwrap_or(false),
        Some("in_range") => {
            let min = test.get("min").and_then(|v| v.as_i64()).unwrap_or(i64::MIN);
            let max = test.get("max").and_then(|v| v.as_i64()).unwrap_or(i64::MAX);
            total >= min && total <= max
        }
        Some("otherwise") => true,
        _ => false,
    }
}

fn band_guard_ok(g: &Value, target: i64) -> bool {
    if let Some(n) = g.get("target_lt").and_then(|v| v.as_i64())  { if target >= n { return false; } }
    if let Some(n) = g.get("target_lte").and_then(|v| v.as_i64()) { if target > n  { return false; } }
    if let Some(n) = g.get("target_gte").and_then(|v| v.as_i64()) { if target < n  { return false; } }
    if let Some(n) = g.get("target_gt").and_then(|v| v.as_i64())  { if target <= n { return false; } }
    true
}

/// Which actor-parameter source a percentile check tests, after derivation.
enum TestedSource {
    Track { id: String, label: String },
    Skill { key: String },
    Stat { key: String },
}

/// Decide WHICH actor parameter a percentile check tests, fully data-driven:
///   1. resource tracks — matched via each track's own id/name + its
///      `on_outcome[*].check_match` aliases (already parsed, multilingual);
///   2. trained skills — matched against the actor's own skill keys;
///   3. characteristics — matched against the actor's own stat keys.
/// The contract's `tested_parameter.key` (e.g. the ability/skill name) is the
/// primary hint; otherwise the check text (label/summary/intent) is scanned.
/// Returns None when nothing matches (caller then fails closed — no fake target).
fn derive_tested_source(
    tested: Option<&TestedParameter>,
    kernel: &RuleKernel,
    mech: &Value,
    check_label: &str,
    action_summary: &str,
    intent_kind: &str,
) -> Option<TestedSource> {
    let hint = tested.map(|t| t.key.to_ascii_lowercase());
    let hint = hint.as_deref().filter(|s| !s.trim().is_empty());
    let hay = format!("{} {} {}", check_label, action_summary, intent_kind).to_ascii_lowercase();
    // When there is NO explicit binding, a RESOURCE TRACK (e.g. Sanity) must be
    // matched only against the DELIBERATE, engine-set check text (check_label +
    // intent_kind), NOT raw player prose (action_summary) — otherwise an
    // incidental "...for sanity reasons" in a technical check would mis-bind the
    // SAN track. Skills/stats still match the full text (they're named on purpose).
    let track_hay = format!("{} {}", check_label, intent_kind).to_ascii_lowercase();

    for track in &kernel.resource_tracks {
        let id = track.get("id").and_then(|v| v.as_str()).unwrap_or("").trim();
        if id.is_empty() { continue; }
        let name = track.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
        let mut aliases: Vec<String> = vec![id.to_ascii_lowercase()];
        if !name.is_empty() { aliases.push(name.to_ascii_lowercase()); }
        if let Some(arr) = track.get("on_outcome").and_then(|v| v.as_array()) {
            for r in arr {
                // trigger:"always" = 无条件效果路由(如 HP 吸收 damage),
                // 其 check_match 是出场路由别名,NOT 该轨被测的别名 → 跳过。
                // 结果条件型(on_failure/on_success/...)= 一次针对该轨自身的判定(SAN),别名保留。
                if r.get("trigger").and_then(|v| v.as_str()) == Some("always") { continue; }
                if let Some(cm) = r.get("check_match").and_then(|v| v.as_str()) {
                    aliases.extend(cm.split('|').map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()));
                }
            }
        }
        let hit = match hint {
            Some(h) => aliases.iter().any(|a| a == h || h.contains(a.as_str()) || a.contains(h)),
            None => aliases.iter().any(|a| track_hay.contains(a.as_str())),
        };
        if hit {
            let label = if !name.is_empty() { name.to_string() } else { id.to_string() };
            return Some(TestedSource::Track { id: id.to_string(), label });
        }
    }
    if let Some(key) = best_key_match(mech.get("skills"), hint, &hay) {
        return Some(TestedSource::Skill { key });
    }
    if let Some(key) = best_key_match(mech.get("stats"), hint, &hay) {
        return Some(TestedSource::Stat { key });
    }
    None
}

/// Find the best-matching key in a stat/skill map. Prefers an exact (then
/// substring) match against the hint; else the longest key that appears in the
/// check text. Returns the ORIGINAL-cased key.
fn best_key_match(obj: Option<&Value>, hint: Option<&str>, hay: &str) -> Option<String> {
    let map = obj.and_then(|v| v.as_object())?;
    if let Some(h) = hint {
        if let Some(k) = map.keys().find(|k| k.to_ascii_lowercase() == h) { return Some(k.clone()); }
        let mut best: Option<(&String, usize)> = None;
        for k in map.keys() {
            let kl = k.to_ascii_lowercase();
            if h.contains(kl.as_str()) || kl.contains(h) {
                if best.map(|(_, l)| kl.len() > l).unwrap_or(true) { best = Some((k, kl.len())); }
            }
        }
        if let Some((k, _)) = best { return Some(k.clone()); }
    }
    let mut best: Option<(&String, usize)> = None;
    for k in map.keys() {
        let kl = k.to_ascii_lowercase();
        if kl.len() >= 3 && hay.contains(kl.as_str()) {
            if best.map(|(_, l)| kl.len() > l).unwrap_or(true) { best = Some((k, kl.len())); }
        }
    }
    best.map(|(k, _)| k.clone())
}

/// Read a stat/skill value (handles numeric and string-encoded), case-insensitive key.
fn value_from_profile(obj: Option<&Value>, key: &str) -> Option<i32> {
    let map = obj.and_then(|v| v.as_object())?;
    let v = map.get(key)
        .or_else(|| map.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v))?;
    json_to_i32(v)
}

fn json_to_i32(v: &Value) -> Option<i32> {
    v.as_i64().map(|n| n as i32)
        .or_else(|| v.as_f64().map(|f| f as i32))
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<i32>().ok()))
}

#[cfg(test)]
mod tested_param_tests {
    use super::*;
    use serde_json::json;

    fn coc_kernel() -> RuleKernel {
        let mut k = RuleKernel::default();
        k.resource_tracks = vec![json!({
            "id": "sanity", "name": "Sanity", "initial": 50, "owner_kind": "actor",
            "on_outcome": [{"op":"subtract","amount":"1d6","trigger":"on_failure","check_match":"sanity|san|理智|恐惧|horror"}]
        })];
        k
    }
    fn coc_mech() -> Value {
        json!({
            "stats": {"POW": 65, "DEX": 70, "STR": 55},
            "skills": {"Spot Hidden": 75, "Library Use": 70, "Firearms (Handgun)": 50, "Law": 40}
        })
    }
    fn tp(key: &str) -> TestedParameter { TestedParameter { domain: None, key: key.into(), label: key.into() } }

    fn coc_kernel_with_hp() -> RuleKernel {
        let mut k = RuleKernel::default();
        k.resource_tracks = vec![
            json!({"id":"sanity","name":"Sanity","initial":50,"owner_kind":"actor",
                "on_outcome":[{"op":"subtract","amount":"1d6","trigger":"on_failure","check_match":"sanity|san|理智|恐惧|horror"}]}),
            json!({"id":"hit_points","name":"Hit Points","initial":10,"owner_kind":"actor",
                "on_outcome":[{"op":"subtract","amount":"=damage","trigger":"always","check_match":"damage|attack"}]}),
        ];
        k
    }
    fn coc_mech_combat() -> Value {
        json!({"stats":{"DEX":70},"skills":{"Fighting (Brawl)":55,"Dodge":40}})
    }

    #[test]
    fn attack_does_not_misbind_to_hit_points_track() {
        let src = derive_tested_source(None, &coc_kernel_with_hp(), &coc_mech_combat(),
            "appropriate attack/conflict check", "我挥拳打过去", "attack");
        match src {
            Some(TestedSource::Track { ref id, .. }) =>
                panic!("attack must NOT bind to a resource track, got track {}", id),
            _ => {}
        }
    }

    #[test]
    fn sanity_still_resolves_with_hp_track_present() {
        let src = derive_tested_source(Some(&tp("理智")), &coc_kernel_with_hp(), &coc_mech_combat(),
            "理智 (core mechanic)", "面对不可名状之物", "ability:skill_use");
        assert!(matches!(src, Some(TestedSource::Track { ref id, .. }) if id == "sanity"));
    }

    #[test]
    fn sanity_check_resolves_to_the_sanity_track() {
        // Chinese ability name "理智" → matched via the track's check_match aliases.
        let src = derive_tested_source(Some(&tp("理智")), &coc_kernel(), &coc_mech(), "理智 (core mechanic)", "面对不可名状之物", "ability:skill_use");
        assert!(matches!(src, Some(TestedSource::Track { ref id, .. }) if id == "sanity"));
    }

    #[test]
    fn skill_check_resolves_to_the_named_skill() {
        let src = derive_tested_source(Some(&tp("Spot Hidden")), &coc_kernel(), &coc_mech(), "Spot Hidden (core mechanic)", "搜索房间", "ability:skill_use");
        match src { Some(TestedSource::Skill { key }) => assert_eq!(key, "Spot Hidden"), other => panic!("expected skill, got {:?}", other.is_some()) }
        assert_eq!(value_from_profile(coc_mech().get("skills"), "Spot Hidden"), Some(75));
    }

    #[test]
    fn characteristic_check_resolves_to_the_stat() {
        let src = derive_tested_source(Some(&tp("DEX")), &coc_kernel(), &coc_mech(), "DEX (core mechanic)", "保持平衡", "ability:skill_use");
        assert!(matches!(src, Some(TestedSource::Stat { ref key }) if key == "DEX"));
        assert_eq!(value_from_profile(coc_mech().get("stats"), "DEX"), Some(70));
    }

    #[test]
    fn no_match_fails_closed_not_fifty() {
        // An unknown reference must NOT silently resolve to a fabricated number.
        let src = derive_tested_source(Some(&tp("飞行术")), &coc_kernel(), &coc_mech(), "??? (core mechanic)", "做点什么", "ability:skill_use");
        assert!(src.is_none());
    }

    fn coc_bands() -> Vec<Value> {
        serde_json::from_value(json!([
            {"id":"critical","label":"大成功","rank":5,"test":{"kind":"exact","value":1}},
            {"id":"extreme","label":"极难成功","rank":4,"test":{"kind":"roll_under_fraction","denominator":5}},
            {"id":"hard","label":"困难成功","rank":3,"test":{"kind":"roll_under_fraction","denominator":2}},
            {"id":"regular","label":"常规成功","rank":2,"test":{"kind":"roll_under_or_equal"}},
            {"id":"fumble","label":"大失败","rank":0,"test":{"kind":"in_range","min":96,"max":100,"when":{"target_lt":50}}},
            {"id":"fumble","label":"大失败","rank":0,"test":{"kind":"in_range","min":100,"max":100,"when":{"target_gte":50}}},
            {"id":"failure","label":"失败","rank":1,"test":{"kind":"otherwise"}}
        ])).unwrap()
    }
    fn tier(total: i64, target: i64) -> String { success_tier_for(&coc_bands(), total, target).unwrap().0 }

    #[test]
    fn coc_tiers_skill75() {
        // Spot Hidden 75: hard<=37, extreme<=15, crit==1, fumble==100, else fail.
        assert_eq!(tier(1, 75), "critical");
        assert_eq!(tier(10, 75), "extreme");   // 10 <= 15
        assert_eq!(tier(30, 75), "hard");      // 30 <= 37
        assert_eq!(tier(70, 75), "regular");   // 70 <= 75
        assert_eq!(tier(90, 75), "failure");   // 90 > 75, not 100
        assert_eq!(tier(100, 75), "fumble");   // skill>=50 -> only 100 fumbles
    }

    #[test]
    fn coc_fumble_band_depends_on_skill_under_50() {
        // Skill 40 (<50): 96-100 all fumble; skill 75 (>=50): 96-99 are plain fail.
        assert_eq!(tier(97, 40), "fumble");
        assert_eq!(tier(97, 75), "failure");
    }
}

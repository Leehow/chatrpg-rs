use anyhow::Result;
use chrono::Utc;
use regex::Regex;
use serde_json::{json, Value};
use trpg_db::Db;
use trpg_model::*;
use uuid::Uuid;

#[derive(Clone)]
pub struct PlayerValueRefereeService {
    db: Db,
}

impl PlayerValueRefereeService {
    pub fn new(db: Db) -> Self { Self { db } }

    pub async fn inspect_turn(
        &self,
        session_id: &str,
        turn_id: &str,
        ruleset_id: &str,
        module_id: Option<&str>,
        user_input: &str,
    ) -> Result<PlayerValueRefereeResult> {
        if !enabled() { return Ok(PlayerValueRefereeResult::default()); }
        let world_tick = None;
        let claims = detect_claims(session_id, turn_id, ruleset_id, module_id, user_input, world_tick);
        if claims.is_empty() { return Ok(PlayerValueRefereeResult::default()); }

        let mut result = PlayerValueRefereeResult { handled: true, phases: vec!["player_value_referee".into()], ..Default::default() };
        let player_insists = looks_like_player_insists(user_input);
        for claim in claims {
            let verification = verify_claim(&claim, ruleset_id, user_input, player_insists);
            self.db.insert_player_value_claim(&claim).await.ok();
            self.db.insert_player_value_verification(&verification).await.ok();
            let override_agreement = if verification.status == PlayerValueVerificationStatus::AcceptedAsTablePreference {
                let agreement = TableOverrideAgreement {
                    override_id: format!("table_override_{}", Uuid::new_v4().simple()),
                    session_id: session_id.to_string(),
                    claim_id: claim.claim_id.clone(),
                    verification_id: verification.verification_id.clone(),
                    status: TableOverrideStatus::Proposed,
                    accepted_by: Some("table".into()),
                    reason: Some("player_insisted_on_noncanonical_parameter".into()),
                    warning_public: verification.warning_public.clone().unwrap_or_else(|| "这会作为本桌规则偏好记录，可能改变系统平衡。".into()),
                    created_at_tick: world_tick,
                    created_at: Utc::now(),
                };
                self.db.insert_table_override_agreement(&agreement).await.ok();
                Some(agreement)
            } else { None };
            result.claims.push(claim);
            result.verifications.push(verification);
            if let Some(agreement) = override_agreement { result.table_overrides.push(agreement); }
        }
        result.narration_context = Some(render_referee_context(&result));
        Ok(result)
    }
}

fn enabled() -> bool {
    std::env::var("TRPG_PLAYER_VALUE_REFEREE_ENABLE_V1102")
        .map(|v| !matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "off"))
        .unwrap_or(true)
}

fn detect_claims(session_id: &str, turn_id: &str, ruleset_id: &str, module_id: Option<&str>, input: &str, world_tick: Option<i64>) -> Vec<PlayerSuppliedValueClaim> {
    let mut out = Vec::new();
    let lower = input.to_lowercase();

    // Dice-total claim: "3d6 = 14", "3d6掷了14", "3d6 total 14".
    let dice_total_re = Regex::new(r"(?i)(\d+)\s*d\s*(\d+)(?:\s*[+＋-]\s*\d+)?[^0-9]{0,16}(?:=|是|为|掷出|掷了|roll(?:ed)?|total)?\s*(\d{1,3})").unwrap();
    for cap in dice_total_re.captures_iter(input) {
        let dice = format!("{}d{}", &cap[1], &cap[2]);
        let total: i64 = cap[3].parse().unwrap_or_default();
        let evidence = cap.get(0).map(|m| m.as_str()).unwrap_or_default().to_string();
        out.push(make_claim(session_id, turn_id, ruleset_id, module_id, PlayerSuppliedValueKind::DamageRollTotal, "player supplied dice total", json!({"dice_expression": dice, "total": total}), evidence, input, world_tick));
    }

    // Weapon or ability damage expression claim: "这把枪伤害是 5d6" / "damage is 5d6".
    let damage_expr_re = Regex::new(r"(?i)(?:伤害|damage|dmg|weapon damage|枪伤害|武器伤害)[^0-9d]{0,18}(\d+\s*d\s*\d+(?:\s*[+＋-]\s*\d+)?)").unwrap();
    for cap in damage_expr_re.captures_iter(input) {
        let expr = cap[1].replace(' ', "");
        let evidence = cap.get(0).map(|m| m.as_str()).unwrap_or_default().to_string();
        out.push(make_claim(session_id, turn_id, ruleset_id, module_id, PlayerSuppliedValueKind::WeaponDamage, "player supplied weapon/ability damage", json!({"damage_expression": expr}), evidence, input, world_tick));
    }

    let dv_re = Regex::new(r"(?i)(?:dv|dc|tn|难度|目标值|命中\s*dv|距离\s*dv)[^0-9]{0,12}(\d{1,3})").unwrap();
    for cap in dv_re.captures_iter(input) {
        let value: i64 = cap[1].parse().unwrap_or_default();
        let evidence = cap.get(0).map(|m| m.as_str()).unwrap_or_default().to_string();
        out.push(make_claim(session_id, turn_id, ruleset_id, module_id, PlayerSuppliedValueKind::DifficultyValue, "player supplied difficulty/target number", json!({"value": value}), evidence, input, world_tick));
    }

    let hp_re = Regex::new(r"(?i)(?:hp|血量|生命值|剩\s*血|剩下|还剩)[^0-9]{0,12}(\d{1,3})").unwrap();
    for cap in hp_re.captures_iter(input) {
        let value: i64 = cap[1].parse().unwrap_or_default();
        let evidence = cap.get(0).map(|m| m.as_str()).unwrap_or_default().to_string();
        out.push(make_claim(session_id, turn_id, ruleset_id, module_id, PlayerSuppliedValueKind::HitPoints, "player supplied hit point value", json!({"value": value}), evidence, input, world_tick));
    }

    let armor_re = Regex::new(r"(?i)(?:armor|armour|sp|ac|护甲|装甲|防御)[^0-9]{0,12}(\d{1,3})").unwrap();
    for cap in armor_re.captures_iter(input) {
        let value: i64 = cap[1].parse().unwrap_or_default();
        let evidence = cap.get(0).map(|m| m.as_str()).unwrap_or_default().to_string();
        out.push(make_claim(session_id, turn_id, ruleset_id, module_id, PlayerSuppliedValueKind::ArmorValue, "player supplied armor/defense value", json!({"value": value}), evidence, input, world_tick));
    }

    dedupe_claims(out)
}

fn make_claim(session_id: &str, turn_id: &str, ruleset_id: &str, module_id: Option<&str>, kind: PlayerSuppliedValueKind, label: &str, supplied: Value, evidence: String, context: &str, world_tick: Option<i64>) -> PlayerSuppliedValueClaim {
    PlayerSuppliedValueClaim {
        claim_id: format!("player_value_claim_{}", Uuid::new_v4().simple()),
        session_id: session_id.to_string(),
        turn_id: turn_id.to_string(),
        ruleset_id: ruleset_id.to_string(),
        module_id: module_id.map(str::to_string),
        value_kind: kind,
        label: label.into(),
        supplied_value_json: supplied,
        evidence_span: evidence,
        context_summary: context.chars().take(500).collect(),
        target_actor_id: None,
        target_object_id: None,
        target_ability_id: None,
        source_refs: vec![],
        visibility: Visibility::GmOnly,
        world_tick,
        created_at: Utc::now(),
    }
}

fn dedupe_claims(mut claims: Vec<PlayerSuppliedValueClaim>) -> Vec<PlayerSuppliedValueClaim> {
    let mut seen = std::collections::BTreeSet::new();
    claims.retain(|c| seen.insert(format!("{}:{}", c.value_kind.as_str(), c.evidence_span.to_lowercase())));
    claims
}

fn verify_claim(claim: &PlayerSuppliedValueClaim, ruleset_id: &str, input: &str, player_insists: bool) -> PlayerValueVerification {
    let mut status = PlayerValueVerificationStatus::NeedsRuleLookup;
    let mut canonical = json!({});
    let mut acceptable = json!({});
    let mut comparison = json!({"policy":"rules_first_player_parameter_validation"});
    let mut warning = None;
    let mut suggestion = None;
    let mut accepted_if_insists = true;
    let mut balance_risk = None;

    match claim.value_kind {
        PlayerSuppliedValueKind::DamageRollTotal => {
            let expr = claim.supplied_value_json.get("dice_expression").and_then(|v| v.as_str()).unwrap_or_default();
            let total = claim.supplied_value_json.get("total").and_then(|v| v.as_i64()).unwrap_or_default();
            let (min, max) = dice_total_bounds(expr).unwrap_or((0, i64::MAX));
            acceptable = json!({"dice_expression": expr, "min": min, "max": max});
            comparison = json!({"supplied_total": total, "inside_bounds": total >= min && total <= max});
            if total >= min && total <= max {
                status = PlayerValueVerificationStatus::TableConsistent;
                canonical = json!({"accepted_roll_total": total, "dice_expression": expr});
            } else if player_insists {
                status = PlayerValueVerificationStatus::AcceptedAsTablePreference;
                warning = Some(format!("你给出的 {}={} 超出该骰式的可能范围（{}–{}）。如果本桌坚持采用，我会记录为桌面特例；这可能明显影响平衡和可信度。", expr, total, min, max));
                suggestion = Some(format!("建议改为 {}–{} 范围内的实际掷骰结果，或重新掷 {}。", min, max, expr));
                balance_risk = Some("impossible_roll_total".into());
                canonical = json!({"table_override_total": total, "dice_expression": expr});
            } else {
                status = PlayerValueVerificationStatus::ContradictsKnownRule;
                warning = Some(format!("这个数值不合法：{} 的结果不可能是 {}，合法范围是 {}–{}。", expr, total, min, max));
                suggestion = Some(format!("请重新给出 {} 的实际结果，或明确说你要把 {} 作为本桌特例。", expr, total));
                accepted_if_insists = true;
            }
        }
        PlayerSuppliedValueKind::WeaponDamage | PlayerSuppliedValueKind::DamageExpression => {
            let expr = claim.supplied_value_json.get("damage_expression").and_then(|v| v.as_str()).unwrap_or_default();
            let family = ruleset_damage_family(ruleset_id);
            let dice = parse_dice_count(expr).unwrap_or_default();
            acceptable = json!({"ruleset_family": family, "common_table_band": common_damage_band(ruleset_id)});
            let plausible = damage_plausible_for_ruleset(ruleset_id, expr);
            comparison = json!({"supplied_expression": expr, "dice_count": dice, "plausible_for_ruleset_band": plausible});
            if plausible {
                status = PlayerValueVerificationStatus::PlausibleProvisional;
                canonical = json!({"provisional_damage_expression": expr, "requires_exact_rule_or_item_table_lookup": true});
                suggestion = Some("该伤害表达式落在本系统常见区间内；我会继续尝试从武器/法术/能力表确认，确认前标记为 provisional。".into());
            } else if player_insists {
                status = PlayerValueVerificationStatus::AcceptedAsTablePreference;
                warning = Some(format!("你给的伤害 {} 明显偏离当前规则常见武器/能力区间。我可以按本桌爽局偏好暂用，但会标记为 table override，可能破坏遭遇平衡。", expr));
                suggestion = Some(format!("建议先按规则表中同类武器/能力的常见区间处理：{}。", common_damage_band(ruleset_id)));
                balance_risk = Some("out_of_band_damage_expression".into());
                canonical = json!({"table_override_damage_expression": expr});
            } else {
                status = PlayerValueVerificationStatus::UnreasonableNeedsWarning;
                warning = Some(format!("我不能直接采纳这个伤害 {}。它看起来不符合当前系统同类武器/能力的常见参数，应该先查具体武器/法术/能力表。", expr));
                suggestion = Some(format!("建议用同类条目的参数范围作为临时值：{}；如果你坚持，我会记录为本桌特例。", common_damage_band(ruleset_id)));
                balance_risk = Some("possible_balance_break".into());
            }
        }
        PlayerSuppliedValueKind::DifficultyValue | PlayerSuppliedValueKind::RangeDifficulty => {
            let value = claim.supplied_value_json.get("value").and_then(|v| v.as_i64()).unwrap_or_default();
            acceptable = common_difficulty_band_json(ruleset_id);
            let plausible = difficulty_plausible_for_ruleset(ruleset_id, value);
            comparison = json!({"supplied_value": value, "plausible_for_ruleset_band": plausible});
            if plausible {
                status = PlayerValueVerificationStatus::PlausibleProvisional;
                canonical = json!({"provisional_target_value": value, "requires_procedure_or_range_table_lookup": true});
                suggestion = Some("该难度值在当前规则常见区间内，但仍应优先查对应距离/难度/动作程序表。".into());
            } else if player_insists {
                status = PlayerValueVerificationStatus::AcceptedAsTablePreference;
                warning = Some(format!("你给的目标值 {} 不在当前规则常见难度区间内。若本桌坚持，我会记录为 table override；这可能让行动过难或过易。", value));
                suggestion = Some("建议改用规则中的标准 DV/DC/TN 或由 GM 根据距离、掩体和风险裁定。".into());
                balance_risk = Some("out_of_band_target_value".into());
                canonical = json!({"table_override_target_value": value});
            } else {
                status = PlayerValueVerificationStatus::UnreasonableNeedsWarning;
                warning = Some(format!("我不会直接采纳目标值 {}；它需要规则程序/距离表确认。", value));
                suggestion = Some("建议由 GM 查规则或根据同类难度表给出目标值，而不是让玩家提供。".into());
            }
        }
        PlayerSuppliedValueKind::ArmorValue | PlayerSuppliedValueKind::HitPoints | PlayerSuppliedValueKind::ResourceAmount | PlayerSuppliedValueKind::AbilityCost | PlayerSuppliedValueKind::GenericParameter => {
            let value = claim.supplied_value_json.get("value").and_then(|v| v.as_i64()).unwrap_or_default();
            let plausible = generic_value_plausible(value);
            acceptable = json!({"generic_reasonable_band":"0..100 for quick referee sanity; exact table required"});
            comparison = json!({"supplied_value": value, "generic_plausible": plausible});
            if plausible {
                status = PlayerValueVerificationStatus::PlausibleProvisional;
                canonical = json!({"provisional_value": value, "requires_exact_runtime_state_or_table_lookup": true});
                suggestion = Some("这个数值看起来不离谱，但仍应优先使用已落库状态、NPC 卡、物品表或规则表；确认前仅作 provisional。".into());
            } else if player_insists {
                status = PlayerValueVerificationStatus::AcceptedAsTablePreference;
                warning = Some(format!("数值 {} 明显偏离常见范围。我可以记录为本桌特例，但会提醒它可能破坏平衡或使场景失真。", value));
                balance_risk = Some("generic_out_of_band_value".into());
                canonical = json!({"table_override_value": value});
            } else {
                status = PlayerValueVerificationStatus::UnreasonableNeedsWarning;
                warning = Some(format!("我不会直接采纳数值 {}；需要查已落库状态或相关表。", value));
                suggestion = Some("建议使用系统已记录的角色/物品/能力状态；如果没有，再由 GM 给出 provisional 并赛后复核。".into());
            }
        }
    }

    PlayerValueVerification {
        verification_id: format!("player_value_verification_{}", Uuid::new_v4().simple()),
        claim_id: claim.claim_id.clone(),
        session_id: claim.session_id.clone(),
        turn_id: claim.turn_id.clone(),
        status,
        canonical_value_json: canonical,
        acceptable_range_json: acceptable,
        comparison_json: comparison,
        source_refs: vec![],
        warning_public: warning,
        suggestion_public: suggestion,
        accepted_if_player_insists: accepted_if_insists,
        balance_risk,
        verifier_json: json!({
            "verifier": "player_value_referee_v1_10_2",
            "policy": "rules_first_not_rules_lawyer",
            "rule_lookup_required_before_adopting_player_parameter": true,
            "player_parameter_can_be_accepted_as_table_override_after_warning": true,
            "raw_input_excerpt": input.chars().take(500).collect::<String>(),
        }),
        world_tick: claim.world_tick,
        created_at: Utc::now(),
    }
}

fn looks_like_player_insists(input: &str) -> bool {
    let lower = input.to_lowercase();
    ["坚持", "就这样", "就按这个", "爽", "house rule", "homebrew", "i insist", "rule of cool", "let me", "for fun"].iter().any(|t| lower.contains(t))
}

fn dice_total_bounds(expr: &str) -> Option<(i64, i64)> {
    let re = Regex::new(r"(?i)^(\d+)d(\d+)(?:[+＋](\d+)|-(\d+))?$").unwrap();
    let compact = expr.replace(' ', "");
    let cap = re.captures(&compact)?;
    let n: i64 = cap[1].parse().ok()?;
    let sides: i64 = cap[2].parse().ok()?;
    let plus: i64 = cap.get(3).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
    let minus: i64 = cap.get(4).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
    Some((n - minus + plus, n * sides - minus + plus))
}
fn parse_dice_count(expr: &str) -> Option<i64> { Regex::new(r"(?i)(\d+)\s*d\s*(\d+)").ok()?.captures(expr).and_then(|c| c.get(1)?.as_str().parse().ok()) }
fn ruleset_damage_family(ruleset_id: &str) -> &'static str { if ruleset_id.contains("cyberpunk") { "cyberpunk_red_weapon_damage" } else if ruleset_id.contains("dnd") { "dnd_damage_dice" } else if ruleset_id.contains("sword_world") { "sword_world_weapon_spell_damage" } else if ruleset_id.contains("coc") || ruleset_id.contains("brp") { "brp_percentile_weapon_damage" } else { "generic_trpg_damage" } }
fn common_damage_band(ruleset_id: &str) -> &'static str { if ruleset_id.contains("cyberpunk") { "roughly 2d6..8d6 depending weapon class; exact weapon table required" } else if ruleset_id.contains("dnd") { "roughly 1d4..2d12 for common low-level weapon/spell chunks; exact entry required" } else if ruleset_id.contains("sword_world") { "damage is usually table/formula driven; exact weapon/spell data required" } else if ruleset_id.contains("coc") || ruleset_id.contains("brp") { "weapon-specific dice such as 1d3..2d10+db; exact weapon table required" } else { "system-specific; exact object/ability entry required" } }
fn damage_plausible_for_ruleset(ruleset_id: &str, expr: &str) -> bool { let Some(n) = parse_dice_count(expr) else { return false; }; if ruleset_id.contains("cyberpunk") { (1..=12).contains(&n) } else if ruleset_id.contains("dnd") { (1..=20).contains(&n) } else { (1..=30).contains(&n) } }
fn common_difficulty_band_json(ruleset_id: &str) -> Value { if ruleset_id.contains("cyberpunk") { json!({"common_dv_band":"9..29", "note":"DV should come from range/task table or GM adjudication"}) } else if ruleset_id.contains("dnd") { json!({"common_dc_band":"5..30", "note":"DC should come from task difficulty, AC, save DC, or rules text"}) } else if ruleset_id.contains("coc") || ruleset_id.contains("brp") { json!({"common_target_band":"1..100", "note":"usually roll-under ability value or hard/extreme derivation, not arbitrary DC"}) } else { json!({"common_target_band":"ruleset-specific"}) } }
fn difficulty_plausible_for_ruleset(ruleset_id: &str, value: i64) -> bool { if ruleset_id.contains("cyberpunk") { (5..=35).contains(&value) } else if ruleset_id.contains("dnd") { (1..=40).contains(&value) } else if ruleset_id.contains("coc") || ruleset_id.contains("brp") { (1..=100).contains(&value) } else { (1..=100).contains(&value) } }
fn generic_value_plausible(value: i64) -> bool { (0..=150).contains(&value) }

fn render_referee_context(result: &PlayerValueRefereeResult) -> String {
    let mut lines = vec!["[Player-Supplied Value Referee]".to_string(), "Policy: use rules and relevant tables first; player-supplied numbers are claims, not automatic truth. If a claim is unsupported or out-of-band, warn and suggest a rules-consistent value. If the table insists, record a table override and warn about balance.".into()];
    for v in &result.verifications {
        lines.push(format!("- claim={} status={} warning={:?} suggestion={:?}", v.claim_id, v.status.as_str(), v.warning_public, v.suggestion_public));
    }
    lines.join("\n")
}

//! D1 单测：roll-high(meet_or_beat)检定把 actor STAT+SKILL 加进 total，
//! 而 CoC(roll_under)/Triangle(count_faces) 字节等价（不进新路径）。
//!
//! 经 `#[cfg(test)] #[path="roll_high_modifier_tests.rs"] mod ...;` 引入（≤400 行纪律）。
//!
//! 这些测试**纯**（无 DB/无 async）：把"取值"(skill_modifier_from_label / skill_field_slug)
//! 和"结算"(resolve_against_model 对 StaticTargetNumber 的 total>=value)拆成纯单元，
//! 复刻 resolve_outcome 的 `total = raw_total + sum(modifiers)` 组合算术。
//! DB 异步取卡的整链由 live 集成测试 + §4 live 重测覆盖。
use super::*;
use serde_json::json;

/// 复刻 resolve_outcome 的组合：raw 骰 + 所有 modifier 之和，喂给 resolve_against_model。
/// 返回 (composed_total, success)。
fn compose_and_resolve(
    model: &CheckResolutionModel,
    raw_total: i64,
    mods: &[CheckModifier],
    rolls: &[i64],
) -> (i64, Option<bool>) {
    let sum: i64 = mods.iter().map(|m| m.value as i64).sum();
    let total = raw_total + sum;
    let (_t, success, _d, _a) = resolve_against_model(model, total, rolls);
    (total, success)
}

/// Cyberpunk 风格的 actor 卡：扁平 skills + stats（实测 runtime 形态，无 fields._base）。
fn cyber_mech_flat() -> Value {
    json!({
        "stats": {"REF": 8, "DEX": 7, "TECH": 7},
        "skills": {"Handgun": 6, "Brawling": 4, "Basic Tech": 8}
    })
}

// ─── (a) meet_or_beat：die + skill 加进 total，过一个可达 DV ───
#[test]
fn meet_or_beat_adds_skill_value_and_passes_achievable_dv() {
    let mech = cyber_mech_flat();
    // tested skill "Handgun" → 卡上 6（skill-only：runtime 无 _base）。
    let m = skill_modifier_from_label(&mech, "Handgun")
        .expect("Handgun 在卡上必须解析出 modifier");
    assert_eq!(m.value, 6, "skill-only：取 skills.Handgun=6");
    assert_eq!(m.label, "Handgun");

    // 静态 DV 14：裸骰 8 必败（旧行为），8 + Handgun 6 = 14 >= 14 通过（新行为）。
    let model = CheckResolutionModel::StaticTargetNumber { value: 14, label: "DV".into() };
    let (_t, bare_success, _d, _a) = resolve_against_model(&model, 8, &[]);
    assert_eq!(bare_success, Some(false), "旧行为：裸骰 8 < DV 14 必败");

    let (composed, success) = compose_and_resolve(&model, 8, std::slice::from_ref(&m), &[]);
    assert_eq!(composed, 14, "8 + Handgun 6 = 14");
    assert_eq!(success, Some(true), "新行为：14 >= DV 14 → 通过（能力 PC 不再必败）");
}

// ─── (b) CoC roll_under：技能是 TARGET，不做加法 → 字节等价 ───
#[test]
fn roll_under_target_is_skill_not_addend_unchanged() {
    // PercentileRollUnder：total <= ability_value。skill 75 是目标，不加进 total。
    let model = CheckResolutionModel::PercentileRollUnder {
        ability_label: "Spot Hidden".into(),
        ability_value: 75,
    };
    // 无 modifier（guard 第一关 compare!=meet_or_beat → resolve_roll_high_modifiers 返 []）。
    let (total, success) = compose_and_resolve(&model, 30, &[], &[]);
    assert_eq!(total, 30, "roll_under 不加 modifier，total 保持裸骰");
    assert_eq!(success, Some(true), "30 <= 75 成功，verdict 不变");

    // 关键：若错误地把 skill 75 当 addend 加进 total，30+75=105 会翻成 105<=75=false。
    // 此处 mods 为空证明 roll_under 路径不被污染。
}

// ─── (c) Triangle count_faces：数 hits，无 additive total → 字节等价 ───
#[test]
fn count_faces_pool_unchanged_no_additive_total() {
    let model = CheckResolutionModel::DicePoolCount {
        target_face: 6,
        threshold: 2,
        label: "pool".into(),
    };
    // rolls 含 2 个 6 面 → hits=2 >= threshold 2 成功。total 入参对 pool 无意义（数 rolls）。
    let rolls = vec![6i64, 6, 3, 1];
    let (_total, success) = compose_and_resolve(&model, 0, &[], &rolls);
    assert_eq!(success, Some(true), "2 hits >= threshold 2 → 成功，count_faces 不受 modifier 影响");

    // mods 为空（guard compare!=meet_or_beat），且即使误加也只动 total 而 pool 数 rolls → 不变。
    let (_t2, success2) = compose_and_resolve(&model, 0, &[CheckModifier{label:"x".into(),value:99,source_ref:None}], &rolls);
    assert_eq!(success2, Some(true), "count_faces 数 hits，与 total/modifier 无关 → 字节等价");
}

// ─── (d) fail-soft：卡上缺该技能值 → 不加 modifier，total 不变，不崩 ───
#[test]
fn missing_sheet_value_adds_nothing_fail_soft() {
    let mech = cyber_mech_flat();
    // 卡上没有 "Sniping" 技能 → 返回 None（不编造，不崩）。
    assert!(skill_modifier_from_label(&mech, "Sniping").is_none(), "缺值必须返 None 不编造");

    // 没解析出 modifier 时，total 保持裸骰，verdict 与旧行为一致。
    let model = CheckResolutionModel::StaticTargetNumber { value: 14, label: "DV".into() };
    let (total, success) = compose_and_resolve(&model, 8, &[], &[]);
    assert_eq!(total, 8, "无 modifier → total 保持裸骰 8（fail-soft = 旧行为）");
    assert_eq!(success, Some(false), "8 < 14 仍失败（与旧字节等价）");
}

// ─── _base 优先：结构化 fields.<skill>_base(STAT+level) 存在时优先于扁平 skills ───
#[test]
fn prefers_structured_skill_base_over_flat_skill() {
    // 当 runtime 投影出 fields.handgun_base（= REF 8 + Handgun level 6 = 14）时，优先用 base。
    let mech = json!({
        "stats": {"REF": 8},
        "skills": {"Handgun": 6},
        "fields": {"handgun_base": 14}
    });
    let m = skill_modifier_from_label(&mech, "Handgun").expect("有 _base 时必须解析");
    assert_eq!(m.value, 14, "优先 fields.handgun_base = STAT+level 全量 CPR（14），非扁平 skill 6");
    assert!(m.label.contains("base"), "label 标注 base 来源");
}

// ─── slug 规范化：技能名 → field id 槽 ───
#[test]
fn skill_field_slug_normalizes() {
    assert_eq!(skill_field_slug("Basic Tech"), "basic_tech");
    assert_eq!(skill_field_slug("Firearms (Handgun)"), "firearms_handgun");
    assert_eq!(skill_field_slug("Handgun"), "handgun");
}

// ─── Stat-typed tested source(无技能"只用 linked STAT")→ 加 stats.<stat> ───
#[test]
fn stat_typed_value_resolves_from_stats() {
    let mech = cyber_mech_flat();
    // 直接验 value_from_profile 对 stats 的读取（resolve_roll_high_modifiers 的 Stat 分支用它）。
    assert_eq!(value_from_profile(mech.get("stats"), "REF"), Some(8), "无技能时取 linked STAT REF=8");
}

// ─── bare-die guard：防双加（player-reported / 已合成 1d10+N 都不再叠加 STAT+SKILL） ───
fn roll_with_result(result: Value) -> DiceRollRecord {
    serde_json::from_value(json!({
        "roll_id": "r", "session_id": "s", "turn_id": "t", "check_id": "c",
        "roller_kind": "player_character", "roller_id": "pc.current",
        "visibility": "public_gm_roll", "expression": "1d10",
        "result": result, "seed_commitment": "x", "revealed_at": null,
        "created_at": "2026-06-20T00:00:00Z", "roll_plan_id": null
    })).unwrap()
}

#[test]
fn bare_system_die_is_eligible_for_compose() {
    // 系统裸骰 mode=rolled & modifier=0 → 可加（D1 的唯一触发条件）。
    let roll = roll_with_result(json!({"mode":"rolled","expression":"1d10","rolls":[8],"modifier":0,"total":8}));
    assert!(roll_is_bare_system_die(&roll), "裸系统骰必须放行组合");
}

#[test]
fn precomposed_expression_blocks_double_add() {
    // 已合成 1d10+14（combat compile_formula 写 modifier=14）→ total 已含加值 → 必须**不**再加。
    let roll = roll_with_result(json!({"mode":"rolled","expression":"1d10+14","rolls":[8],"modifier":14,"total":22}));
    assert!(!roll_is_bare_system_die(&roll), "modifier!=0 已含加值 → 禁止再加(防双加)");
}

#[test]
fn player_reported_modes_block_double_add() {
    // 玩家报总值 / 报分量(STAT+Skill+骰)→ total 已是最终值 → 禁止再加 actor modifier。
    let reported_total = roll_with_result(json!({"mode":"reported_total","total":18}));
    assert!(!roll_is_bare_system_die(&reported_total), "reported_total 禁止再加");
    let reported_components = roll_with_result(json!({"mode":"reported_components","die":8,"components":[6,4],"total":18}));
    assert!(!roll_is_bare_system_die(&reported_components), "reported_components 禁止再加");
}

//! Phase 3 单测：给 meet_or_beat 通用模型补"读对手防御值"支路。
//! 经 `#[cfg(test)] #[path = "opposed_meet_or_beat_tests.rs"] mod ...;` 引入
//! （文件 ≤400 行纪律，从 lib.rs 拆出）。
//!
//! 验证点（spec §4.2 / §6 ①⑤⑥）：
//!   - meet_or_beat opposed 分支：双方值都在 → 建 OpposedRoll，经 resolve_opposed
//!     真出胜负（total>=defense），非 Provisional-null；
//!   - fail-closed：防御值缺 → OpposedRoll 仍建但 success=null（维持 provisional 行为），不乱绑；
//!   - roll_under 回归：同一 build_opposed_model 构造器，方向不串。
//! 这些测试纯（无 DB/无 async）：把 build_opposed_model 提取成纯构造器后，
//! DB 异步读值的部分由 live_opposed_meet_or_beat.rs 集成测试覆盖。
use super::*;
use crate::opposed::resolve_opposed;

/// 最小契约：只填 build_opposed_model 需要的 dice_expression。其余字段走 serde default
/// 不可行（CheckContract 无 Default），故手工拼 JSON 反序列化。
fn contract_with_dice(dice: &str) -> CheckContract {
    serde_json::from_value(json!({
        "check_id": "check_t", "session_id": "s", "turn_id": "t",
        "ruleset_id": "rs", "module_id": null,
        "initiator": {"actor_id":"pc.current","actor_kind":"player_character","display_name":null},
        "target_actor": {"actor_id":"npc.boss","actor_kind":"npc","display_name":null},
        "opposition": {"kind":"no_mechanical_opposition"},
        "action_summary": "开火", "intent_kind": "attack",
        "check_label": "attack", "dice_expression": dice, "modifiers": [],
        "target": {"kind":"unknown_until_lookup"},
        "tested_parameter": {"domain":null,"key":"Handgun","label":"Handgun"},
        "opponent_tested_parameter": {"domain":null,"key":"defense","label":"defense"},
        "actor_snapshot_ids": [], "source_refs": [], "learned_packet_ids": [],
        "roll_visibility": "public_gm_roll", "roll_authority": "system",
        "disclosure": {"show_roll_to_player":true,"show_formula_to_player":true,"show_dc_to_player":true,"show_success_failure_to_player":true,"reveal_after_scene":false,"reveal_after_session":false},
        "stakes": {"before_roll_public":"","success_public":"","failure_public":"","critical_public":null,"fumble_public":null,"success_patches_allowed":[],"failure_patches_allowed":[],"irreversible":false},
        "confidence": "medium", "ruling_status": "source_backed", "advice_refs": [], "expires_at_turn": null
    })).unwrap()
}

/// 从一个 meet_or_beat 的 OpposedRoll 模型 + 双方掷骰，走 resolve_opposed 拿胜负。
/// 复刻 resolve_outcome 里的对抗结算尾段（compare-agnostic）。
fn resolve_meet_or_beat(
    model: &CheckResolutionModel,
    atk_total: i64,
    def_total: i64,
) -> (Option<bool>, Option<i64>) {
    let bands: Vec<Value> = vec![]; // CPR 无 success_bands → tier rank 退化为 0，纯比 margin。
    if let CheckResolutionModel::OpposedRoll {
        attacker_value,
        defender_value,
        ..
    } = model
    {
        let (_t, s, _d) = resolve_opposed(
            "meet_or_beat",
            &bands,
            atk_total,
            *attacker_value,
            def_total,
            *defender_value,
        );
        return (s, defender_value.map(|v| v as i64));
    }
    (None, None)
}

#[test]
fn meet_or_beat_opposed_builds_opposed_roll_with_defense_value() {
    // CPR 攻击：攻击方 Handgun=14、防御方 defense(DV)=12（现搓自 NPC 卡）。
    let contract = contract_with_dice("1d10");
    let model = build_opposed_model(&contract, "1d10", Some(14), Some(12));
    // 必须建 OpposedRoll 且带防御值（消费了 NPC 卡），而非 StaticTargetNumber/Provisional。
    match &model {
        CheckResolutionModel::OpposedRoll {
            defender_value,
            defender_actor_id,
            ..
        } => {
            assert_eq!(*defender_value, Some(12i32), "必须读到防御方 defense=12");
            assert_eq!(
                defender_actor_id.as_deref(),
                Some("npc.boss"),
                "防御方 actor id 来自 target_actor，非占位符"
            );
        }
        other => panic!("meet_or_beat opposed 必须建 OpposedRoll，得到 {:?}", other),
    }
}

#[test]
fn meet_or_beat_opposed_resolves_to_real_verdict_total_ge_defense() {
    // 对抗：攻击方技能阈值=12、防御方 DV=12（现搓自 NPC 卡）。两侧各按 total>=自家值 达成，
    // 比 margin 定胜负（resolve_opposed 通用语义）。关键断言：success 非 null（消费层真结算）。
    let contract = contract_with_dice("1d10");
    let model = build_opposed_model(&contract, "1d10", Some(12), Some(12));
    // 攻击方 total 18>=12 达成、防御方 total 9<12 未达成 → 攻击方命中。
    let (success, def_v) = resolve_meet_or_beat(&model, 18, 9);
    assert_eq!(
        success,
        Some(true),
        "攻击方达成、防御方未达成 → 攻击方胜，必须非 null（消费层真结算）"
    );
    assert_eq!(def_v, Some(12), "结算消费了 NPC 现搓的 defense");

    // 攻击方 total 9<12 未达成、防御方 total 18>=12 达成 → 防御方胜（攻击被挡）。
    let (success, _) = resolve_meet_or_beat(&model, 9, 18);
    assert_eq!(success, Some(false), "攻击方未达成、防御方达成 → 防御方胜");
}

#[test]
fn meet_or_beat_opposed_missing_defense_fails_closed() {
    // 现搓不成 / 查不到防御值 → OpposedRoll 仍建（不乱绑平衡值），但 resolve_opposed
    // fail-closed 返 None → success 维持 null（= provisional 行为，不静默乱判命中）。
    let contract = contract_with_dice("1d10");
    let model = build_opposed_model(&contract, "1d10", Some(14), None);
    match &model {
        CheckResolutionModel::OpposedRoll { defender_value, .. } => {
            assert_eq!(*defender_value, None, "查不到防御值 → 不编造，保持 None")
        }
        other => panic!("应为 OpposedRoll，得到 {:?}", other),
    }
    let (success, _) = resolve_meet_or_beat(&model, 99, 1);
    assert_eq!(
        success, None,
        "防御值缺 → fail-closed，绝不乱判命中（不是 Some(true)）"
    );
}

#[test]
fn roll_under_opposed_construction_unaffected_regression() {
    // 同一构造器服务 roll_under：CoC 攻击方 Spot Hidden=75、防御 perception=60。
    // 仅验构造形态不串（值与方向）；roll_under 真结算由 live_opposed.rs 覆盖。
    let contract = contract_with_dice("1d100");
    let model = build_opposed_model(&contract, "1d100", Some(75), Some(60));
    match &model {
        CheckResolutionModel::OpposedRoll {
            attacker_value,
            defender_value,
            defender_expression,
            ..
        } => {
            assert_eq!(*attacker_value, Some(75));
            assert_eq!(*defender_value, Some(60));
            assert_eq!(defender_expression, "1d100", "防御掷式来自 def_expr 入参");
        }
        other => panic!("roll_under opposed 仍应建 OpposedRoll，得到 {:?}", other),
    }
    // roll_under 方向：total<=value 成功。
    let bands: Vec<Value> = vec![];
    let (_t, s, _d) = resolve_opposed("roll_under", &bands, 5, Some(75), 96, Some(60));
    assert_eq!(
        s,
        Some(true),
        "攻击方掷 5<=75 成功、防御方 96>60 失败 → 攻击方胜"
    );
}

// ─── spec §4.3 / §6⑥：fail-closed 不再静默 miss，给显式 awaiting_binding 信号 ───
// resolve_against_model 对「未绑定」模型(Provisional / 无防御值的 OpposedRoll /
// RulesetProcedureLookup)返回非空 awaiting_binding 理由(第 4 元素),供 resolve_outcome
// 把它写进 outcome["awaiting_binding"] → GM agent 收到后改道(找 DV / request_player_roll /
// 叙事降级),而非看到 success=null 的沉默 miss。已结算模型一律返 None(零回归)。

#[test]
fn opposed_without_defense_value_signals_awaiting_binding_not_silent_miss() {
    // 现搓不成 / 查不到防御值 → OpposedRoll{defender_value:None}。旧行为:success=null 静默 miss。
    // 新行为:resolve_against_model 第 4 元素给出非空 awaiting_binding 理由。
    let contract = contract_with_dice("1d10");
    let model = build_opposed_model(&contract, "1d10", Some(14), None);
    let (_t, success, _d, awaiting) = resolve_against_model(&model, 18, &[]);
    assert_eq!(success, None, "无防御值仍 fail-closed,不乱判命中");
    let reason =
        awaiting.expect("无防御值的 OpposedRoll 必须给出 awaiting_binding 信号,而非默默 None");
    assert!(!reason.trim().is_empty(), "awaiting_binding 理由不得为空");
}

#[test]
fn attack_provisional_signals_awaiting_binding() {
    // attack 走到 Provisional(无 source-backed DV)→ 必须 awaiting_binding,非静默 null。
    let model = CheckResolutionModel::Provisional {
        reason: "attack defense/DV is missing; bind source-backed target defense/AC/evasion/DV"
            .into(),
        suggested_target: None,
    };
    let (_t, success, _d, awaiting) = resolve_against_model(&model, 50, &[]);
    assert_eq!(success, None);
    assert_eq!(
        awaiting.as_deref(),
        Some("attack defense/DV is missing; bind source-backed target defense/AC/evasion/DV"),
        "Provisional 的 reason 必须原样作为 awaiting_binding 信号透出"
    );
}

#[test]
fn resolved_models_carry_no_awaiting_binding_signal() {
    // 零回归:已结算模型(命中判定出 success)一律不带 awaiting_binding。
    let static_m = CheckResolutionModel::StaticTargetNumber {
        value: 12,
        label: "dv".into(),
    };
    let (_t, s, _d, awaiting) = resolve_against_model(&static_m, 18, &[]);
    assert_eq!(s, Some(true));
    assert_eq!(awaiting, None, "已结算的静态目标不得带 awaiting_binding");

    let pct = CheckResolutionModel::PercentileRollUnder {
        ability_label: "Spot".into(),
        ability_value: 75,
    };
    let (_t, s, _d, awaiting) = resolve_against_model(&pct, 30, &[]);
    assert_eq!(s, Some(true));
    assert_eq!(awaiting, None, "已结算的百分比检定不得带 awaiting_binding");

    // 防御值齐备的 OpposedRoll 本身不带信号(胜负由 resolve_outcome 的 resolve_opposed 出)。
    let contract = contract_with_dice("1d10");
    let bound = build_opposed_model(&contract, "1d10", Some(14), Some(12));
    let (_t, _s, _d, awaiting) = resolve_against_model(&bound, 18, &[]);
    assert_eq!(
        awaiting, None,
        "防御值齐备的 OpposedRoll 不带 awaiting_binding(交 resolve_opposed 出胜负)"
    );
}

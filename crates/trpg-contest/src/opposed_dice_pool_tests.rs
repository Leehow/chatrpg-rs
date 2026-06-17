//! count_faces 骰池(Triangle)单测：①target_face=null 的 fail-closed 收口；
//! ②骰池对抗模型构造 + resolve_against_model 待绑信号 + resolve_pool_opposed 真胜负。
//! 经 `#[cfg(test)] #[path = "opposed_dice_pool_tests.rs"] mod ...;` 引入（文件 ≤400 行纪律）。
//! 纯测试（无 DB/无 async）；整链 in-game 由 live_pool_opposed.rs 集成测试覆盖。
use super::*;
use crate::opposed::resolve_pool_opposed;

// ─── Part 1：count_faces target_face=null 不再静默落回通用兜底，给清晰 fail-closed ───

#[test]
fn count_faces_null_face_unparseable_fails_closed_with_clear_reason() {
    // target_face=null 且 compare_to 无可解析整数面 → 必须给 count_faces 专属 Provisional
    // （清晰 reason），而非 None 静默落回 infer_ruleset_default_model 的通用兜底（破契约）。
    let dc = json!({"compare":"count_faces","target_face":null,"compare_to":"the highest die"});
    match count_faces_model(&dc) {
        CheckResolutionModel::Provisional { reason, .. } => {
            let r = reason.to_ascii_lowercase();
            assert!(r.contains("count_faces") || r.contains("target_face") || r.contains("face"),
                "fail-closed reason 必须点名 count_faces/target_face，便于 GM 改道，实际={reason}");
        }
        other => panic!("无可解析面 → 必须 fail-closed 成 Provisional，得到 {:?}", other),
    }
}

#[test]
fn count_faces_recovers_face_from_compare_to() {
    // target_face=null 但 compare_to 带整数面 "3" → 还原成 DicePoolCount{face=3,threshold=1}。
    let dc = json!({"compare":"count_faces","target_face":null,"compare_to":"3","success_threshold":null});
    match count_faces_model(&dc) {
        CheckResolutionModel::DicePoolCount { target_face, threshold, .. } =>
            assert_eq!((target_face, threshold), (3, 1), "从 compare_to 还原面，阈值默认 1"),
        other => panic!("应还原 DicePoolCount，得到 {:?}", other),
    }
}

#[test]
fn count_faces_prefers_explicit_machine_fields() {
    let dc = json!({"compare":"count_faces","compare_to":"3","target_face":5,"success_threshold":2});
    match count_faces_model(&dc) {
        CheckResolutionModel::DicePoolCount { target_face, threshold, .. } =>
            assert_eq!((target_face, threshold), (5, 2), "machine 字段优先于 compare_to"),
        other => panic!("应建 DicePoolCount，得到 {:?}", other),
    }
}

// ─── Part 2：count_faces 对抗模型构造 + 结算 ───

fn opposed_pool_contract(dice: &str) -> CheckContract {
    serde_json::from_value(json!({
        "check_id": "check_pool", "session_id": "s", "turn_id": "t",
        "ruleset_id": "triangle_agency", "module_id": null,
        "initiator": {"actor_id":"pc.current","actor_kind":"player_character","display_name":null},
        "target_actor": {"actor_id":"npc.rival","actor_kind":"npc","display_name":null},
        "opposition": {"kind":"no_mechanical_opposition"},
        "action_summary": "对抗 Rival", "intent_kind": "opposed_action",
        "check_label": "opposed pool", "dice_expression": dice, "modifiers": [],
        "target": {"kind":"unknown_until_lookup"},
        "tested_parameter": {"domain":null,"key":"Competence","label":"Competence"},
        "opponent_tested_parameter": {"domain":null,"key":"Competence","label":"Competence"},
        "actor_snapshot_ids": [], "source_refs": [], "learned_packet_ids": [],
        "roll_visibility": "public_gm_roll", "roll_authority": "system",
        "disclosure": {"show_roll_to_player":true,"show_formula_to_player":true,"show_dc_to_player":true,"show_success_failure_to_player":true,"reveal_after_scene":false,"reveal_after_session":false},
        "stakes": {"before_roll_public":"","success_public":"","failure_public":"","critical_public":null,"fumble_public":null,"success_patches_allowed":[],"failure_patches_allowed":[],"irreversible":false},
        "confidence": "medium", "ruling_status": "source_backed", "advice_refs": [], "expires_at_turn": null
    })).unwrap()
}

#[test]
fn pool_opposed_builds_dice_pool_opposed_model() {
    // 对抗 count_faces 契约 → 必须建 DicePoolOpposed（不读技能值，hits 决胜），带防御方掷式与 actor id。
    let contract = opposed_pool_contract("6d4");
    let model = build_pool_opposed_model(&contract, 3, 1, "6d4");
    match &model {
        CheckResolutionModel::DicePoolOpposed { target_face, threshold, defender_actor_id, defender_expression, .. } => {
            assert_eq!((*target_face, *threshold), (3, 1));
            assert_eq!(defender_actor_id.as_deref(), Some("npc.rival"), "防御方 id 来自 target_actor");
            assert_eq!(defender_expression, "6d4", "防御掷式=入参池掷式");
        }
        other => panic!("count_faces 对抗必须建 DicePoolOpposed，得到 {:?}", other),
    }
}

#[test]
fn pool_opposed_without_defender_roll_signals_awaiting_binding() {
    // 防御方还没掷池（resolve_against_model 见不到防御骰）→ fail-closed 给显式 awaiting_binding，
    // 而非默默 success=null 静默 miss。防御骰齐备时由 resolve_outcome 的 resolve_pool_opposed 覆盖。
    let contract = opposed_pool_contract("6d4");
    let model = build_pool_opposed_model(&contract, 3, 1, "6d4");
    let (_t, success, _d, awaiting) = resolve_against_model(&model, 0, &[]);
    assert_eq!(success, None, "无防御骰仍 fail-closed，不乱判胜负");
    let reason = awaiting.expect("DicePoolOpposed 无防御骰必须给 awaiting_binding 信号");
    assert!(!reason.trim().is_empty(), "awaiting_binding 理由不得为空");
}

#[test]
fn pool_opposed_resolves_to_real_verdict_via_hit_counts() {
    // 复刻 resolve_outcome 的骰池对抗尾段：攻击池 3 hits、防御池 1 hit → 攻击方胜（非 null，真结算）。
    let contract = opposed_pool_contract("6d4");
    let model = build_pool_opposed_model(&contract, 3, 1, "6d4");
    if let CheckResolutionModel::DicePoolOpposed { target_face, threshold, .. } = &model {
        let (_t, s, d) = resolve_pool_opposed(*target_face, *threshold, &[3, 3, 1, 2, 4, 3], &[3, 1, 2, 2, 4, 1]);
        assert_eq!(s, Some(true), "攻击 3 hits > 防御 1 hit → 攻击方胜（消费层真结算，非 null）");
        assert_eq!(d.as_deref(), Some("attacker_wins"));
    } else {
        panic!("应为 DicePoolOpposed");
    }
}

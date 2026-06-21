//! V2-P2 — field-level Response Contract (蓝图 §四). Each player action is
//! decomposed into the specific information FIELDS it requests; the evaluator
//! checks, per field, whether the GM reply addressed it. RESPONSE_INTENT_MISMATCH
//! is upgraded from a binary turn flag to "which requested field went unanswered".
//!
//! These tests pin: (1) multi-field extraction, (2) field-level evidence on the
//! mismatch, (3) answered fields are NOT flagged (metric-driven, not hardcoded),
//! (4) pure physical actions carry no contract, and (5) the milestone-1 fixtures
//! still produce RESPONSE_INTENT_MISMATCH with field-level evidence.

use trpg_eval::{contract_for, contracts, parse_transcript, IntentField, Turn};

fn turn(index: u32, action: &str, gm: &str) -> Turn {
    Turn {
        index,
        player_action: action.into(),
        gm_raw: gm.into(),
        ..Default::default()
    }
}

fn fields_of(c: &trpg_eval::ResponseContract) -> Vec<IntentField> {
    c.requests.iter().map(|r| r.field).collect()
}

#[test]
fn contract_extracts_multiple_intent_fields() {
    // One interrogation action that asks for TWO distinct information fields.
    let c = contract_for(&turn(
        49,
        "我逼问头目：是谁出钱要拿下这个街区？",
        "他咬紧牙关，什么都没说，你没有任何回应。",
    ));
    let fields = fields_of(&c);
    assert!(fields.contains(&IntentField::Extract), "逼问 → Extract: {fields:?}");
    assert!(fields.contains(&IntentField::Identity), "是谁 → Identity: {fields:?}");
    assert_eq!(fields.len(), 2, "exactly two fields requested: {fields:?}");
}

#[test]
fn unanswered_field_is_flagged_answered_field_is_not() {
    // No-info reply → both requested fields unanswered.
    let stonewalled = contract_for(&turn(
        804,
        "我隔着窗缝数清楚对方人数和武器，评估这场冲突的胜算。",
        "你眯起眼，可什么都没能真正抓住，局面没有变化。",
    ));
    let unanswered: Vec<_> = stonewalled.unanswered().map(|r| r.field).collect();
    assert!(unanswered.contains(&IntentField::Count), "Count unanswered: {unanswered:?}");
    assert!(unanswered.contains(&IntentField::Assess), "Assess unanswered: {unanswered:?}");

    // Concrete reply (no no-info marker) → same fields, but answered.
    let delivered = contract_for(&turn(
        804,
        "我隔着窗缝数清楚对方人数和武器，评估这场冲突的胜算。",
        "你数清了：六个人，四支冲锋枪两把霰弹枪；正面硬拼胜算不到三成。",
    ));
    assert_eq!(
        delivered.unanswered().count(),
        0,
        "a concrete answer leaves no unanswered field — metric-driven, not hardcoded"
    );
    assert_eq!(delivered.requests.len(), 2, "fields still extracted regardless of answer");
}

#[test]
fn pure_physical_action_has_no_contract() {
    let c = contract_for(&turn(
        7,
        "我换上新弹匣，调整呼吸，准备下一轮射击。",
        "你利落地拍上弹匣，吐出一口气。",
    ));
    assert!(c.requests.is_empty(), "no information requested → empty contract: {:?}", c.requests);
}

#[test]
fn milestone1_fixtures_keep_field_level_mismatch() {
    // cyber: the t49 interrogation must surface a multi-field mismatch.
    let md = std::fs::read_to_string(format!(
        "{}/fixtures/bad/cyber_repetition.md",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let t = parse_transcript(&md);
    let cs = contracts(&t);
    let t49 = cs.iter().find(|c| c.turn == 49).expect("turn 49 has a contract");
    assert!(
        t49.unanswered().count() >= 1,
        "cyber t49 interrogation left a requested field unanswered: {:?}",
        t49.requests
    );

    // coc: at least one turn must carry an unanswered request (drives the verdict's
    // RESPONSE_INTENT_MISMATCH — asserted structurally in negative_golden.rs).
    let md = std::fs::read_to_string(format!(
        "{}/fixtures/bad/coc_semantic_shell.md",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let t = parse_transcript(&md);
    let any_unanswered = contracts(&t).iter().any(|c| c.unanswered().count() >= 1);
    assert!(any_unanswered, "coc fixture has at least one unanswered information request");
}

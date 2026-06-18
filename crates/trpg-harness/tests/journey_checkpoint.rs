//! Provider-free coverage for the fail-closed check/dice classifier and the
//! human-input / roll-request guards (TC-JRNY-01).

use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use trpg_harness::{
    build_check_evidence, classify_check_checkpoint, contest_row_awaiting_binding,
    contest_row_is_resolved, human_player_input_findings, player_visible_effect_present,
    player_visible_roll_request_findings, when_satisfied, CheckCheckpoint, CheckEvidence,
    CheckpointState,
};

fn check_resolution_spec() -> CheckCheckpoint {
    CheckCheckpoint {
        require_dice: true,
        require_check_resolution: true,
        ..Default::default()
    }
}

/// The live-gap contest row: dice rolled, but target/success/degree null and
/// `awaiting_binding` present.
fn live_gap_contest_row() -> Value {
    json!({
        "target_value": null,
        "success": null,
        "degree": null,
        "total": 10,
        "outcome_json": {
            "awaiting_binding": "no source-backed target number/opposition model was bound for ruleset `cyberpunk_red`",
            "target": null,
            "success": null,
            "degree": null
        }
    })
}

fn resolved_contest_row() -> Value {
    json!({
        "target_value": 14,
        "success": true,
        "degree": 1,
        "total": 18,
        "outcome_json": {"target": 14, "success": true, "degree": 1}
    })
}

#[test]
fn no_trigger_is_not_triggered() {
    let ev = CheckEvidence {
        natural_player_action: true,
        ..Default::default()
    };
    assert_eq!(
        classify_check_checkpoint(&check_resolution_spec(), &ev),
        CheckpointState::NotTriggered
    );
}

#[test]
fn missing_natural_action_is_invalid_setup() {
    let ev = CheckEvidence {
        roll_plan_present: true,
        dice_present: true,
        ..Default::default()
    };
    assert_eq!(
        classify_check_checkpoint(&check_resolution_spec(), &ev),
        CheckpointState::InvalidSetup
    );
}

#[test]
fn roll_plus_dice_without_resolution_is_triggered_no_mechanism() {
    let count_delta = BTreeMap::from([
        ("roll_plans".to_string(), 1),
        ("dice_rolls".to_string(), 1),
        ("contest_resolution_events".to_string(), 1),
    ]);
    let row = live_gap_contest_row();
    let ev = build_check_evidence(true, &count_delta, Some(&row), true);
    assert!(ev.dice_present && ev.roll_plan_present);
    assert!(
        ev.awaiting_binding,
        "live-gap row must flag awaiting_binding"
    );
    assert!(!ev.resolved_check_present);
    assert_eq!(
        classify_check_checkpoint(&check_resolution_spec(), &ev),
        CheckpointState::TriggeredNoMechanism
    );
}

#[test]
fn require_dice_without_dice_is_triggered_no_mechanism() {
    let ev = CheckEvidence {
        natural_player_action: true,
        roll_plan_present: true,
        ..Default::default()
    };
    let spec = CheckCheckpoint {
        require_dice: true,
        ..Default::default()
    };
    assert_eq!(
        classify_check_checkpoint(&spec, &ev),
        CheckpointState::TriggeredNoMechanism
    );
}

#[test]
fn resolved_check_without_visible_effect_is_mechanism_no_user_effect() {
    let count_delta = BTreeMap::from([
        ("roll_plans".to_string(), 1),
        ("dice_rolls".to_string(), 1),
        ("contest_resolution_events".to_string(), 1),
    ]);
    let row = resolved_contest_row();
    let ev = build_check_evidence(true, &count_delta, Some(&row), false);
    assert!(ev.resolved_check_present);
    let spec = CheckCheckpoint {
        require_dice: true,
        require_check_resolution: true,
        require_player_visible_effect: true,
        ..Default::default()
    };
    assert_eq!(
        classify_check_checkpoint(&spec, &ev),
        CheckpointState::MechanismNoUserEffect
    );
}

#[test]
fn full_trigger_dice_resolution_and_effect_is_pass() {
    let count_delta = BTreeMap::from([
        ("roll_plans".to_string(), 1),
        ("dice_rolls".to_string(), 1),
        ("contest_resolution_events".to_string(), 1),
    ]);
    let row = resolved_contest_row();
    let ev = build_check_evidence(true, &count_delta, Some(&row), true);
    let spec = CheckCheckpoint {
        require_dice: true,
        require_check_resolution: true,
        require_player_visible_effect: true,
        ..Default::default()
    };
    assert_eq!(classify_check_checkpoint(&spec, &ev), CheckpointState::Pass);
}

#[test]
fn contest_row_resolution_helpers() {
    assert!(!contest_row_is_resolved(&live_gap_contest_row()));
    assert!(contest_row_awaiting_binding(&live_gap_contest_row()));
    assert!(contest_row_is_resolved(&resolved_contest_row()));
    assert!(!contest_row_awaiting_binding(&resolved_contest_row()));
}

#[test]
fn visible_effect_substring_and_fallback() {
    let effects = vec!["你发现".to_string()];
    assert!(player_visible_effect_present(
        &effects,
        "你发现了埋伏的痕迹。"
    ));
    assert!(!player_visible_effect_present(&effects, "门口很安静。"));
    // Empty list falls back to non-empty output.
    assert!(player_visible_effect_present(&[], "some narration"));
    assert!(!player_visible_effect_present(&[], "   "));
}

#[test]
fn when_gate_membership() {
    let mut flags = BTreeSet::new();
    assert!(when_satisfied(None, &flags));
    assert!(when_satisfied(Some(""), &flags));
    assert!(!when_satisfied(Some("check_resolved"), &flags));
    flags.insert("check_resolved".to_string());
    assert!(when_satisfied(Some("check_resolved"), &flags));
}

#[test]
fn checkpoint_state_serializes_screaming_snake() {
    assert_eq!(
        serde_json::to_value(CheckpointState::TriggeredNoMechanism).unwrap(),
        json!("TRIGGERED_NO_MECHANISM")
    );
    assert_eq!(
        serde_json::to_value(CheckpointState::MechanismNoUserEffect).unwrap(),
        json!("MECHANISM_NO_USER_EFFECT")
    );
    assert_eq!(
        serde_json::to_value(CheckpointState::NotTriggered).unwrap(),
        json!("NOT_TRIGGERED")
    );
}

#[test]
fn internal_event_names_are_rejected() {
    for probe in [
        "emit PlayerLearnedFact for the player",
        "create a pending_check on the door",
        "I expect CheckResolved to fire",
        "insert a row into dice_rolls",
    ] {
        let findings = human_player_input_findings(probe, false);
        assert!(
            !findings.is_empty(),
            "expected non-human findings for {probe:?}"
        );
    }
}

#[test]
fn json_and_sql_prose_is_rejected() {
    let findings = human_player_input_findings(r#"{"action":"roll","dc":15}"#, false);
    assert!(findings.iter().any(|f| f.contains("JSON/code-like")));
    let sql = human_player_input_findings("select * from sessions", false);
    assert!(sql
        .iter()
        .any(|f| f.contains("sql") || f.contains("database")));
}

#[test]
fn manual_dice_language_is_rejected_unless_allowed() {
    let blocked = human_player_input_findings("我掷出了 1d10，总共 10", false);
    assert!(!blocked.is_empty());
    // The legacy/manual escape hatch relaxes only the dice-result patterns.
    let allowed = human_player_input_findings("我掷出了一个结果", true);
    assert!(
        allowed.is_empty(),
        "manual-roll-allowed mode should not flag plain narration: {allowed:?}"
    );
}

#[test]
fn natural_player_action_passes() {
    let findings = human_player_input_findings("我压低声音靠近公寓门口，先观察有没有埋伏。", false);
    assert!(
        findings.is_empty(),
        "clean human action flagged: {findings:?}"
    );
}

#[test]
fn roll_request_in_visible_output_is_flagged() {
    let body = "门口很安静。请掷一个感知检定，告诉我点数。";
    let findings = player_visible_roll_request_findings(body);
    assert!(!findings.is_empty());
    // The same request wrapped in an allowed system tag is exempt.
    let tagged = "门口很安静。[system]请掷一个感知检定[/system]";
    assert!(player_visible_roll_request_findings(tagged).is_empty());
}

//! Provider-free coverage for the accepted-live-run cassette writer and the
//! deterministic/replay executor verification (TC-JRNY-02). Proves the writer →
//! consumer round-trip with no provider.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use trpg_harness::{
    build_cassette, classify_check_checkpoint, fixture_turn_evidence, verify_fixture_plan,
    CheckCheckpoint, CheckpointState, ExecutionMode, Fixture, FixtureTurn, ReplayMismatch,
    ScenarioIdentity, DIAGNOSTIC_FIXTURE_PROVENANCE, EVIDENCE_SCHEMA_VERSION,
};

fn identity() -> ScenarioIdentity {
    ScenarioIdentity::new("JRNY-CYBER-TECH-PASS", "1")
}

fn resolved_row() -> Value {
    json!({
        "target_value": 14, "success": true, "degree": 1, "total": 18,
        "outcome_json": {"target": 14, "success": true, "degree": 1}
    })
}

fn provisional_row() -> Value {
    json!({
        "target_value": null, "success": null, "degree": null, "total": 10,
        "outcome_json": {"awaiting_binding": "no source-backed target number for `cyberpunk_red`"}
    })
}

fn pass_turn() -> FixtureTurn {
    FixtureTurn {
        user_input: "我拆开墙边的接线盒，动手切断公寓的供电电缆。".to_string(),
        count_delta: BTreeMap::from([
            ("roll_plans".to_string(), 1),
            ("dice_rolls".to_string(), 1),
            ("contest_resolution_events".to_string(), 1),
        ]),
        newest_contest_row: Some(resolved_row()),
        player_visible_body: "你成功切断了供电电缆，公寓陷入黑暗。".to_string(),
        knowledge: None,
        npc_social: None,
        memory: None,
        flight_recorder: None,
    }
}

#[test]
fn accepted_cassette_from_successful_live_run_is_replay_consumable() {
    let build = build_cassette(true, &identity(), vec![pass_turn()]);
    assert!(build.accepted);
    assert_eq!(build.fixture.recorded_mode, "live");
    assert_eq!(build.fixture.identity, identity());

    // Round-trips through JSON exactly as a hand-authored fixture would.
    let json_text = serde_json::to_string(&build.fixture).unwrap();
    let reparsed: Fixture = serde_json::from_str(&json_text).unwrap();
    assert_eq!(reparsed.identity.scenario_id, "JRNY-CYBER-TECH-PASS");
    assert_eq!(reparsed.identity.scenario_version, "1");
    assert_eq!(
        reparsed.identity.evidence_schema_version,
        EVIDENCE_SCHEMA_VERSION
    );
    assert_eq!(reparsed.turns[0].user_input, pass_turn().user_input);
    assert_eq!(reparsed.turns[0].count_delta, pass_turn().count_delta);
    assert_eq!(
        reparsed.turns[0].newest_contest_row,
        pass_turn().newest_contest_row
    );
    assert_eq!(
        reparsed.turns[0].player_visible_body,
        pass_turn().player_visible_body
    );

    // The replay executor accepts it: matching identity + live provenance +
    // matching inputs ⇒ no mismatches, and the turn classifies PASS.
    let inputs = vec![pass_turn().user_input];
    assert!(
        verify_fixture_plan(&build.fixture, &identity(), &inputs, ExecutionMode::Replay).is_empty()
    );
    let spec = CheckCheckpoint {
        require_dice: true,
        require_check_resolution: true,
        require_player_visible_effect: true,
        ..Default::default()
    };
    let ev = fixture_turn_evidence(
        &build.fixture.turns[0],
        &["切断".to_string(), "成功".to_string()],
    );
    assert_eq!(classify_check_checkpoint(&spec, &ev), CheckpointState::Pass);
}

#[test]
fn failed_live_run_does_not_produce_accepted_cassette() {
    let build = build_cassette(false, &identity(), vec![pass_turn()]);
    assert!(
        !build.accepted,
        "failed run must not be an accepted cassette"
    );
    assert_eq!(build.fixture.recorded_mode, DIAGNOSTIC_FIXTURE_PROVENANCE);

    // Replay must reject the diagnostic artifact on provenance grounds — no
    // live fallback, no false accept.
    let inputs = vec![pass_turn().user_input];
    let mismatches =
        verify_fixture_plan(&build.fixture, &identity(), &inputs, ExecutionMode::Replay);
    assert!(mismatches
        .iter()
        .any(|m| matches!(m, ReplayMismatch::Provenance { .. })));
}

#[test]
fn mode_parse_and_provenance() {
    assert_eq!(ExecutionMode::parse("Live"), Some(ExecutionMode::Live));
    assert_eq!(
        ExecutionMode::parse("det"),
        Some(ExecutionMode::Deterministic)
    );
    assert_eq!(ExecutionMode::parse("replay"), Some(ExecutionMode::Replay));
    assert_eq!(ExecutionMode::parse("nope"), None);
    assert!(ExecutionMode::Live.allows_live_provider());
    assert!(!ExecutionMode::Replay.allows_live_provider());
    assert!(!ExecutionMode::Deterministic.allows_live_provider());
    assert!(ExecutionMode::Replay.is_fixture_driven());
    assert_eq!(
        ExecutionMode::Replay.required_fixture_provenance(),
        Some("live")
    );
    assert_eq!(
        ExecutionMode::Deterministic.required_fixture_provenance(),
        Some("deterministic")
    );
}

#[test]
fn evidence_header_carries_identity_and_mode() {
    let h = identity().evidence_header(ExecutionMode::Deterministic);
    assert_eq!(h["scenario_id"], json!("JRNY-CYBER-TECH-PASS"));
    assert_eq!(h["scenario_version"], json!("1"));
    assert_eq!(h["mode"], json!("deterministic"));
    assert_eq!(h["evidence_schema_version"], json!(EVIDENCE_SCHEMA_VERSION));
}

#[test]
fn perception_fixture_turn_is_fail_closed() {
    let turn = FixtureTurn {
        user_input: "我压低声音靠近公寓门口，先仔细观察有没有埋伏的迹象。".to_string(),
        count_delta: BTreeMap::from([
            ("roll_plans".to_string(), 1),
            ("dice_rolls".to_string(), 1),
            ("contest_resolution_events".to_string(), 1),
        ]),
        newest_contest_row: Some(provisional_row()),
        player_visible_body: "你屏息观察，门后毫无动静。".to_string(),
        knowledge: None,
        npc_social: None,
        memory: None,
        flight_recorder: None,
    };
    let ev = fixture_turn_evidence(&turn, &["发现".to_string(), "没有动静".to_string()]);
    assert!(ev.awaiting_binding && !ev.resolved_check_present);
    let spec = CheckCheckpoint {
        require_dice: true,
        require_check_resolution: true,
        ..Default::default()
    };
    assert_eq!(
        classify_check_checkpoint(&spec, &ev),
        CheckpointState::TriggeredNoMechanism
    );
}

#[test]
fn matching_fixture_plan_has_no_mismatch() {
    let fixture = Fixture {
        identity: identity(),
        recorded_mode: "live".to_string(),
        turns: vec![pass_turn()],
    };
    let inputs = vec![pass_turn().user_input];
    assert!(verify_fixture_plan(&fixture, &identity(), &inputs, ExecutionMode::Replay).is_empty());
}

#[test]
fn replay_missing_turn_fails_without_live_fallback() {
    let fixture = Fixture {
        identity: identity(),
        recorded_mode: "live".to_string(),
        turns: vec![],
    };
    let inputs = vec![pass_turn().user_input];
    let mismatches = verify_fixture_plan(&fixture, &identity(), &inputs, ExecutionMode::Replay);
    assert!(matches!(
        mismatches.as_slice(),
        [ReplayMismatch::MissingTurn { index: 0, .. }]
    ));
}

#[test]
fn replay_rejects_identity_and_provenance_mismatch() {
    // A deterministic-provenance fixture cannot satisfy a replay run, and a
    // different scenario id is rejected.
    let fixture = Fixture {
        identity: ScenarioIdentity::new("OTHER-SCENARIO", "1"),
        recorded_mode: "deterministic".to_string(),
        turns: vec![pass_turn()],
    };
    let inputs = vec![pass_turn().user_input];
    let mismatches = verify_fixture_plan(&fixture, &identity(), &inputs, ExecutionMode::Replay);
    assert!(mismatches
        .iter()
        .any(|m| matches!(m, ReplayMismatch::Identity { .. })));
    assert!(mismatches
        .iter()
        .any(|m| matches!(m, ReplayMismatch::Provenance { .. })));
}

#[test]
fn unexpected_extra_recorded_turn_is_flagged() {
    let fixture = Fixture {
        identity: identity(),
        recorded_mode: "deterministic".to_string(),
        turns: vec![pass_turn(), pass_turn()],
    };
    let inputs = vec![pass_turn().user_input];
    let mismatches =
        verify_fixture_plan(&fixture, &identity(), &inputs, ExecutionMode::Deterministic);
    assert!(matches!(
        mismatches.as_slice(),
        [ReplayMismatch::UnexpectedTurn { index: 1, .. }]
    ));
}

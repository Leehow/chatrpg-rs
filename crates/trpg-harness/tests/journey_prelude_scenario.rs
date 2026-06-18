//! End-to-end, provider-free terminal classification of the
//! `journey-prelude-character-session` scenario from the artifacts under
//! `harness/scenarios/prelude/`. No CLI, LLM, or database is touched: the
//! prelude readiness, the persisted verdict, the replay-cassette verification,
//! and the check classification all run from disk fixtures.

use std::path::PathBuf;

use serde_json::Value;
use trpg_harness::{
    classify_check_checkpoint, evaluate_persisted_verification, fixture_turn_evidence,
    parse_character_creation_jsonl, verify_fixture_plan, CheckCheckpoint, CheckpointState,
    ExecutionMode, Fixture, PersistedVerdict, PersistedVerification, ScenarioIdentity,
};

fn prelude_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR = <workspace>/crates/trpg-harness
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../harness/scenarios/prelude")
        .canonicalize()
        .expect("prelude scenario directory must exist")
}

fn read(name: &str) -> String {
    std::fs::read_to_string(prelude_dir().join(name))
        .unwrap_or_else(|e| panic!("failed to read {name}: {e}"))
}

#[test]
fn journey_prelude_character_session_reaches_terminal_pass() {
    let contract: Value = serde_json::from_str(&read("journey-prelude-character-session.json"))
        .expect("scenario contract must be valid JSON");

    // --- Prelude readiness (TC-JRNY-00) ---
    let setup = &contract["setup"]["character"];
    let require_persisted = setup["require_persisted"].as_bool().unwrap_or(false);
    let require_binding = setup["require_session_binding"].as_bool().unwrap_or(false);
    assert!(
        require_persisted && require_binding,
        "prelude contract is strict"
    );

    let readiness = parse_character_creation_jsonl(
        &read("character-creation.jsonl"),
        require_persisted,
        require_binding,
    );
    assert!(readiness.ok, "prelude not ready: {:?}", readiness.failures);
    let character_id = readiness.character_id.clone().expect("character_id");
    let session_id = readiness.session_id.clone().expect("session_id");
    let actor_id = readiness.actor_id.clone().expect("actor_id");

    // --- Persisted/session-binding verdict (TC-JRNY-00) ---
    // Provider-free: simulate a clean DB read (all rows present) for the bound
    // identity. The same function returns Blocked without a DB and Invalid when a
    // required row is missing, so a live run fails closed.
    let mut persisted = PersistedVerification {
        required: require_persisted,
        require_session_binding: require_binding,
        db_available: true,
        db_checked: true,
        character_id: Some(character_id),
        session_id: Some(session_id),
        actor_id: Some(actor_id),
        character_row_present: true,
        session_row_present: true,
        actor_params_present: true,
        ..Default::default()
    };
    assert_eq!(
        evaluate_persisted_verification(&mut persisted),
        PersistedVerdict::Ok,
        "persisted verdict failures: {:?}",
        persisted.failures
    );

    // --- Replay cassette verification (TC-JRNY-02) ---
    let identity = ScenarioIdentity::new(
        contract["scenario_id"].as_str().unwrap(),
        contract["scenario_version"].as_str().unwrap(),
    );
    let turn_inputs: Vec<String> = contract["turns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["user_input"].as_str().unwrap().to_string())
        .collect();

    let fixture: Fixture =
        serde_json::from_str(&read("journey-prelude-character-session.replay.json"))
            .expect("replay cassette must deserialize");
    let mismatches = verify_fixture_plan(&fixture, &identity, &turn_inputs, ExecutionMode::Replay);
    assert!(
        mismatches.is_empty(),
        "replay cassette must drive the scenario with no live fallback: {:?}",
        mismatches.iter().map(|m| m.message()).collect::<Vec<_>>()
    );

    // --- Check classification (TC-JRNY-01) ---
    let check: CheckCheckpoint =
        serde_json::from_value(contract["turns"][0]["check"].clone()).expect("check spec");
    let evidence = fixture_turn_evidence(&fixture.turns[0], &check.player_visible_effect_any);
    assert_eq!(
        classify_check_checkpoint(&check, &evidence),
        CheckpointState::Pass,
        "the recorded resolved-contest turn must classify PASS"
    );
}

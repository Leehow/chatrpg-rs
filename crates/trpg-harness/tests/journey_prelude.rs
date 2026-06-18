//! Provider-free coverage for the journey-prelude character/session readiness
//! verdicts (TC-JRNY-00). No process, LLM, or database is touched.

use trpg_harness::{
    evaluate_persisted_verification, parse_character_creation_jsonl, PersistedVerdict,
    PersistedVerification,
};

fn valid_stream() -> String {
    [
        r#"{"event":"phase","phase":"start","data":{"kind":"character_create_auto"}}"#,
        r#"{"event":"phase","phase":"character_created","data":{"character_id":"character_abc","name":"Jin","status":"ready","session_id":"sess_1","actor_id":"pc.current","validation":{"status":"ok","errors":[],"warnings":[],"info":[]},"sheet":{"name":"Jin","stats":{"STR":55}}}}"#,
        r#"{"event":"phase","phase":"bound","data":{"session_id":"sess_1","actor_id":"pc.current"}}"#,
        r#"{"event":"phase","phase":"done","data":{}}"#,
    ]
    .join("\n")
}

#[test]
fn valid_character_stream_is_ready() {
    let r = parse_character_creation_jsonl(&valid_stream(), true, true);
    assert!(r.ok, "expected ready, failures: {:?}", r.failures);
    assert_eq!(r.character_id.as_deref(), Some("character_abc"));
    assert_eq!(r.session_id.as_deref(), Some("sess_1"));
    assert_eq!(r.actor_id.as_deref(), Some("pc.current"));
    assert!(r.saw_character_created && r.saw_bound);
    assert!(r.failures.is_empty());
}

#[test]
fn missing_bound_phase_is_not_ready() {
    let stream = [
        r#"{"event":"phase","phase":"character_created","data":{"character_id":"character_abc","name":"Jin","status":"ready","session_id":"sess_1","actor_id":"pc.current","validation":{"status":"ok","errors":[]},"sheet":{"name":"Jin"}}}"#,
    ]
    .join("\n");
    let r = parse_character_creation_jsonl(&stream, true, true);
    assert!(!r.ok);
    assert!(r.failures.iter().any(|f| f.contains("bound")));
}

#[test]
fn empty_ids_and_validation_errors_are_not_ready() {
    let stream = [
        r#"{"event":"phase","phase":"character_created","data":{"character_id":"","name":"Jin","status":"draft_needs_rules_source","session_id":"","actor_id":"","validation":{"status":"error","errors":[{"code":"missing","message":"no rules"}]},"sheet":{}}}"#,
        r#"{"event":"phase","phase":"bound","data":{"session_id":"","actor_id":""}}"#,
    ]
    .join("\n");
    let r = parse_character_creation_jsonl(&stream, true, true);
    assert!(!r.ok);
    assert!(r.character_id.is_none(), "empty string id must be None");
    assert!(r.session_id.is_none());
    assert!(r.failures.iter().any(|f| f.contains("character_id")));
    assert!(r.failures.iter().any(|f| f.contains("sheet")));
    assert!(r.failures.iter().any(|f| f.contains("validation")));
}

#[test]
fn persisted_not_required_is_ok() {
    let mut v = PersistedVerification {
        required: false,
        ..Default::default()
    };
    assert_eq!(
        evaluate_persisted_verification(&mut v),
        PersistedVerdict::Ok
    );
    assert!(v.failures.is_empty());
}

#[test]
fn persisted_required_without_db_is_blocked() {
    let mut v = PersistedVerification {
        required: true,
        db_available: false,
        ..Default::default()
    };
    assert_eq!(
        evaluate_persisted_verification(&mut v),
        PersistedVerdict::Blocked
    );
    assert!(v.failures.iter().any(|f| f.contains("no database")));
}

#[test]
fn persisted_required_with_query_error_is_blocked() {
    let mut v = PersistedVerification {
        required: true,
        db_available: true,
        db_checked: true,
        query_errors: vec!["characters: relation does not exist".to_string()],
        ..Default::default()
    };
    assert_eq!(
        evaluate_persisted_verification(&mut v),
        PersistedVerdict::Blocked
    );
    assert!(v.failures.iter().any(|f| f.contains("could not verify")));
}

#[test]
fn persisted_required_with_missing_rows_is_invalid() {
    let mut v = PersistedVerification {
        required: true,
        require_session_binding: true,
        db_available: true,
        db_checked: true,
        character_id: Some("character_abc".into()),
        session_id: Some("sess_1".into()),
        actor_id: Some("pc.current".into()),
        character_row_present: false,
        session_row_present: true,
        actor_params_present: false,
        ..Default::default()
    };
    assert_eq!(
        evaluate_persisted_verification(&mut v),
        PersistedVerdict::Invalid
    );
    assert!(v.failures.iter().any(|f| f.contains("characters row")));
    assert!(v
        .failures
        .iter()
        .any(|f| f.contains("runtime_actor_parameters row")));
    assert!(
        !v.failures.iter().any(|f| f.contains("sessions row")),
        "present session row must not be reported missing"
    );
}

#[test]
fn persisted_required_all_rows_present_is_ok() {
    let mut v = PersistedVerification {
        required: true,
        require_session_binding: true,
        db_available: true,
        db_checked: true,
        character_id: Some("character_abc".into()),
        session_id: Some("sess_1".into()),
        actor_id: Some("pc.current".into()),
        character_row_present: true,
        session_row_present: true,
        actor_params_present: true,
        ..Default::default()
    };
    assert_eq!(
        evaluate_persisted_verification(&mut v),
        PersistedVerdict::Ok
    );
    assert!(v.failures.is_empty());
}

#[test]
fn persisted_without_session_binding_ignores_session_rows() {
    let mut v = PersistedVerification {
        required: true,
        require_session_binding: false,
        db_available: true,
        db_checked: true,
        character_id: Some("character_abc".into()),
        character_row_present: true,
        session_row_present: false,
        actor_params_present: false,
        ..Default::default()
    };
    assert_eq!(
        evaluate_persisted_verification(&mut v),
        PersistedVerdict::Ok
    );
    assert!(v.failures.is_empty());
}

#[test]
fn error_event_is_captured() {
    let stream = [
        r#"{"event":"phase","phase":"start","data":{}}"#,
        r#"{"event":"error","message":"llm backend unreachable"}"#,
    ]
    .join("\n");
    let r = parse_character_creation_jsonl(&stream, true, true);
    assert!(!r.ok);
    assert!(r.saw_error);
    assert!(r.failures.iter().any(|f| f.contains("error event")));
}

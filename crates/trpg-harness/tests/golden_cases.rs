//! Golden harness case coverage (TC-P2-03).
//!
//! These tests validate that the three representative golden cases
//! (Cyberpunk RED Homecoming, Triangle Agency, CoC-style investigation) parse
//! into the shared `HarnessCase` schema, and that the shared assertion evaluator
//! catches a spoiler leak and a missing required event.
//!
//! Everything here is provider-free: streams are synthetic JSONL strings fed
//! through the exact same `parse_jsonl_events` + `evaluate_assertions` code the
//! `trpg-harness` binary uses, so no `trpg` process, network, or LLM is needed.

use std::path::PathBuf;
use trpg_harness::{evaluate_assertions, parse_jsonl_events, HarnessCase};

/// Repo-root `harness/cases` directory, resolved relative to this crate.
fn cases_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../harness/cases")
}

fn load_case(file: &str) -> HarnessCase {
    let path = cases_dir().join(file);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("invalid harness case JSON {}: {e}", path.display()))
}

fn phase(name: &str) -> String {
    format!(r#"{{"event":"phase","phase":"{name}"}}"#)
}

fn phase_with_data(name: &str, data: &str) -> String {
    format!(r#"{{"event":"phase","phase":"{name}","data":{data}}}"#)
}

fn delta(text: &str) -> String {
    format!(r#"{{"event":"delta","data":"{text}"}}"#)
}

#[test]
fn golden_case_parses_homecoming_no_spoiler() {
    let case = load_case("homecoming_no_spoiler_golden.json");
    assert_eq!(case.ruleset_id, "cyberpunk_red");
    assert_eq!(case.module_id.as_deref(), Some("cyberpunk_red.homecoming"));
    assert_eq!(case.stream_format, "jsonl");
    // No-spoiler intent: at least one forbidden term, including the known leak.
    assert!(
        case.forbidden_terms.iter().any(|t| t == "Athena"),
        "homecoming golden case must guard the known spoiler term"
    );
    assert!(!case.user_input.trim().is_empty());
    assert!(case.required_events.iter().any(|e| e == "phase:done"));
}

#[test]
fn golden_case_parses_triangle_required_events() {
    let case = load_case("triangle_required_events_golden.json");
    assert_eq!(case.ruleset_id, "triangle_agency");
    assert_eq!(case.module_id.as_deref(), Some("triangle_agency.the_vault"));
    // Required-event intent: the conflict/stream event must be asserted.
    assert!(
        case.required_events
            .iter()
            .any(|e| e == "phase:conflict_agent"),
        "triangle golden case must require the conflict_agent stream event"
    );
    assert!(case
        .required_event_contains
        .iter()
        .any(|a| { a.event == "phase:conflict_agent" && a.contains == "anomaly_encounter" }));
}

#[test]
fn golden_case_parses_coc_investigation() {
    let case = load_case("coc_investigation_golden.json");
    assert_eq!(case.ruleset_id, "coc7e");
    // Investigation intent: a clue/reveal no-spoiler guard plus stream structure.
    assert!(
        !case.forbidden_terms.is_empty(),
        "coc investigation golden case must hide the solution from the player"
    );
    assert!(case
        .required_events
        .iter()
        .any(|e| e == "phase:context_compiled"));
    assert!(case.required_events.iter().any(|e| e == "phase:done"));
}

#[test]
fn harness_forbidden_terms_fail_on_leak() {
    let case = load_case("homecoming_no_spoiler_golden.json");
    let leak_term = case
        .forbidden_terms
        .first()
        .expect("golden case has a forbidden term")
        .clone();

    // A stream whose narration leaks the spoiler must fail the assertion.
    let leaked = [
        phase("context_compiled"),
        delta(&format!("阴影里有人提到了 {leak_term} 背后的计划。")),
        phase("done"),
    ]
    .join("\n");
    let parsed = parse_jsonl_events(&leaked);
    let outcome = evaluate_assertions(&case, &parsed);
    assert!(
        outcome.forbidden_hits.contains(&leak_term),
        "expected forbidden hit for {leak_term}, got {:?}",
        outcome.forbidden_hits
    );
    assert!(
        outcome.failures.iter().any(|f| f.contains(&leak_term)),
        "expected a failure mentioning {leak_term}, got {:?}",
        outcome.failures
    );

    // Control: a clean stream that satisfies required events must pass cleanly.
    let clean = [
        phase("context_compiled"),
        delta("你停在巷口，街上很安静，门口暂时没有动静。"),
        phase("done"),
    ]
    .join("\n");
    let clean_parsed = parse_jsonl_events(&clean);
    let clean_outcome = evaluate_assertions(&case, &clean_parsed);
    assert!(
        clean_outcome.forbidden_hits.is_empty(),
        "clean stream tripped forbidden terms: {:?}",
        clean_outcome.forbidden_hits
    );
    assert!(
        clean_outcome.failures.is_empty(),
        "clean stream produced unexpected failures: {:?}",
        clean_outcome.failures
    );
}

#[test]
fn harness_required_events_fail_when_missing() {
    let case = load_case("triangle_required_events_golden.json");

    // A stream missing the conflict_agent event must fail the assertion.
    let missing = [phase("context_compiled"), phase("done")].join("\n");
    let parsed = parse_jsonl_events(&missing);
    let outcome = evaluate_assertions(&case, &parsed);
    assert!(
        outcome
            .missing_events
            .iter()
            .any(|e| e == "phase:conflict_agent"),
        "expected phase:conflict_agent to be reported missing, got {:?}",
        outcome.missing_events
    );
    assert!(
        outcome
            .failures
            .iter()
            .any(|f| f.contains("phase:conflict_agent")),
        "expected a failure mentioning the missing event, got {:?}",
        outcome.failures
    );

    // Control: a complete stream satisfies both required_events and
    // required_event_contains, so there are no failures.
    let complete = [
        phase("context_compiled"),
        phase_with_data("conflict_agent", r#"{"frame":"anomaly_encounter"}"#),
        phase("done"),
    ]
    .join("\n");
    let complete_parsed = parse_jsonl_events(&complete);
    let complete_outcome = evaluate_assertions(&case, &complete_parsed);
    assert!(
        complete_outcome.missing_events.is_empty(),
        "complete stream reported missing events: {:?}",
        complete_outcome.missing_events
    );
    assert!(
        complete_outcome.failures.is_empty(),
        "complete stream produced unexpected failures: {:?}",
        complete_outcome.failures
    );
}

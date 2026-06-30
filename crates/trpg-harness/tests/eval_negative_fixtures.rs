//! Offline TRPG evaluation negative fixtures.
//!
//! These tests intentionally depend on the new `trpg_eval` crate before it
//! exists. They are the RED step for the first eval-runner MVP: bad battle
//! reports must fail with concrete, evidence-backed categories.

use std::path::PathBuf;

use trpg_eval::{evaluate_fixture, parse_markdown_fixture, FindingCategory, Verdict};

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../eval/fixtures/negative")
        .join(name)
}

#[test]
fn cyber_negative_fixture_fails_with_contract_and_debt_findings() {
    let text = std::fs::read_to_string(fixture_path("cyber_50turn.md")).unwrap();
    let fixture = parse_markdown_fixture(&text).unwrap();
    let report = evaluate_fixture(&fixture);

    assert_eq!(report.verdict, Verdict::Fail);
    assert!(report.has_category(FindingCategory::ResponseIntentMismatch));
    assert!(report.has_category(FindingCategory::PendingMechanicalDebt));
    assert!(report.has_category(FindingCategory::SemanticNoop));
    assert!(report.has_category(FindingCategory::PlayerScriptLoop));
}

#[test]
fn coc_negative_fixture_fails_success_without_information() {
    let text = std::fs::read_to_string(fixture_path("coc_10turn.md")).unwrap();
    let fixture = parse_markdown_fixture(&text).unwrap();
    let report = evaluate_fixture(&fixture);

    assert_eq!(report.verdict, Verdict::Fail);
    assert!(report.has_category(FindingCategory::SuccessWithoutInformation));
    assert!(report.has_category(FindingCategory::ResponseIntentMismatch));
}

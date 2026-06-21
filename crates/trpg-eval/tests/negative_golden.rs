//! Milestone-1 acceptance (蓝图 §十 第一阶段 + RUN_SPEC_V2 验收1):
//! the two bad battle reports are negative golden fixtures. The evaluator MUST
//! judge them FAIL and precisely list the documented root causes. A clean
//! control fixture MUST pass — proving the verdict is metric-driven, not a
//! hardcoded RED.

use std::collections::HashSet;
use trpg_eval::{evaluate, parse_transcript, RootCause, Verdict};

fn verdict_for(rel: &str) -> Verdict {
    let path = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), rel);
    let md = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let transcript = parse_transcript(&md);
    assert!(
        transcript.turns.len() >= 5,
        "{rel}: parser only found {} turns",
        transcript.turns.len()
    );
    evaluate(&transcript)
}

fn assert_has(causes: &HashSet<RootCause>, want: &[RootCause], ctx: &str) {
    for c in want {
        assert!(
            causes.contains(c),
            "{ctx}: missing root cause {c:?}; detected = {causes:?}"
        );
    }
}

#[test]
fn cyber_repetition_fixture_fails_with_all_root_causes() {
    let v = verdict_for("fixtures/bad/cyber_repetition.md");
    assert!(v.is_fail(), "cyber repetition fixture must FAIL");
    let causes = v.root_causes();
    assert_has(
        &causes,
        &[
            RootCause::PlayerActionLoop,
            RootCause::SceneReset,
            RootCause::SemanticNoop,
            RootCause::UnresolvedMechanicalDebt,
            RootCause::ResponseIntentMismatch,
        ],
        "cyber",
    );
    // every finding must carry turn evidence — no empty "looks bad" verdicts.
    for f in &v.findings {
        assert!(!f.turn_ids.is_empty(), "finding {:?} has no turn_ids", f.cause);
        assert!(!f.evidence.is_empty(), "finding {:?} has no evidence", f.cause);
    }
}

#[test]
fn coc_semantic_shell_fixture_fails_with_shell_causes() {
    let v = verdict_for("fixtures/bad/coc_semantic_shell.md");
    assert!(v.is_fail(), "coc semantic-shell fixture must FAIL");
    assert_has(
        &v.root_causes(),
        &[
            RootCause::SemanticNoop,
            RootCause::SuccessWithoutInformation,
            RootCause::ResponseIntentMismatch,
        ],
        "coc",
    );
}

#[test]
fn clean_control_fixture_passes() {
    let v = verdict_for("fixtures/good/clean_min.md");
    assert!(
        !v.is_fail(),
        "clean control fixture must PASS (proves metric-driven, not hardcoded RED); findings = {:?}",
        v.findings
    );
}

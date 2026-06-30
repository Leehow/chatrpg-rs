//! L1.2 SPINE — post-adjudication outcome overlay (PURE, commit-nothing).
//!
//! The turn-order inversion fix (design R1): the pre-adjudication Director (built at the
//! ContextAssembly phase) cannot see the committed `check_result`/effects, so its beat can only
//! GUESS the outcome. This overlay re-shapes an already-built [`DirectorPlan`] so its beat
//! FUNCTION reflects the REAL committed result — a failed check earns a *fail-forward*
//! complication (the story moves on with a new obstacle, never a dead end), a passed check earns
//! an escalation (capitalize on the win). It is the minimal, generic adjustment that makes the
//! Director *load-bearing* on the actual mechanics.
//!
//! Discipline:
//! - **Pure + additive.** Takes a plan + the read-only committed views, returns a new plan. No
//!   DB, no env, no name-branching (constitution rule 11) — the signal is the generic
//!   pass/fail disposition, never a game-specific label.
//! - **Fail-closed.** No committed pass/fail signal (empty results, or every view
//!   `Unresolved`) ⇒ the plan is returned UNCHANGED. The overlay never invents an outcome.
//! - **Preserves every guarantee.** `reveal_candidate_fact_ids` (fail-closed `gm_truth ∖
//!   player_known`), anti-railroad selections, world-candidate pool-filter — all untouched.
//!   Only `beat_kind` + `desired_change` are re-pointed to the committed outcome.

use trpg_model::{BeatKind, CheckOutcomeView, DirectorPlan, MechanicalResultView};

/// Generic `desired_change` token set by the overlay (structural, not prose — codex fold #2/#3).
pub const DESIRED_CHANGE_FAIL_FORWARD: &str = "fail_forward";
pub const DESIRED_CHANGE_CAPITALIZE_SUCCESS: &str = "capitalize_success";

/// The committed aggregate disposition over a turn's results: failure dominates (the most
/// dramatically salient + the one that must NOT dead-end), then success, else no signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommittedDisposition {
    /// At least one committed check FAILED.
    Failed,
    /// At least one committed check PASSED and none failed.
    Passed,
    /// No committed pass/fail signal (empty, or all `Unresolved`). Fail-closed ⇒ no override.
    None,
}

fn aggregate(results: &[MechanicalResultView]) -> CommittedDisposition {
    let mut any_passed = false;
    for r in results {
        match r.outcome {
            CheckOutcomeView::Failed => return CommittedDisposition::Failed,
            CheckOutcomeView::Passed => any_passed = true,
            CheckOutcomeView::Unresolved => {}
        }
    }
    if any_passed {
        CommittedDisposition::Passed
    } else {
        CommittedDisposition::None
    }
}

/// Re-shape `plan` so its beat reflects the committed `results`. Fail-closed: no committed
/// pass/fail signal ⇒ `plan` returned unchanged. Only `beat_kind` + `desired_change` move; all
/// reveal/selection/anti-railroad fields are preserved verbatim.
pub fn apply_committed_outcome(
    mut plan: DirectorPlan,
    results: &[MechanicalResultView],
) -> DirectorPlan {
    match aggregate(results) {
        // Fail-forward: the failure complicates the situation but the story keeps moving — the
        // content-gravity move (steer, don't dead-end). Reveal candidates stay intact so a
        // legitimately-surfaceable fact can still land within the complication.
        CommittedDisposition::Failed => {
            plan.beat_kind = BeatKind::Complicate;
            plan.desired_change = DESIRED_CHANGE_FAIL_FORWARD.into();
        }
        // Capitalize: a committed success raises the stakes rather than stalling.
        CommittedDisposition::Passed => {
            plan.beat_kind = BeatKind::Escalate;
            plan.desired_change = DESIRED_CHANGE_CAPITALIZE_SUCCESS.into();
        }
        // No committed signal ⇒ leave the pre-adjudication plan exactly as-is (fail-closed).
        CommittedDisposition::None => {}
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::{EffectSummary, MechanicalResultView};

    fn view(check_id: &str, outcome: CheckOutcomeView) -> MechanicalResultView {
        MechanicalResultView {
            check_id: check_id.into(),
            outcome,
            degree: None,
            roll_expression: Some("1d100".into()),
            effects: vec![EffectSummary::default()],
        }
    }

    fn base_plan() -> DirectorPlan {
        DirectorPlan {
            beat_kind: BeatKind::Respond,
            desired_change: "shift_situation".into(),
            primary_thread_id: Some("t.alpha".into()),
            reveal_candidate_fact_ids: vec!["fact.ledger".into()],
            ..Default::default()
        }
    }

    #[test]
    fn no_results_leaves_plan_unchanged_fail_closed() {
        let plan = base_plan();
        let out = apply_committed_outcome(plan.clone(), &[]);
        assert_eq!(out, plan, "no committed result ⇒ no override");
    }

    #[test]
    fn all_unresolved_leaves_plan_unchanged_fail_closed() {
        let plan = base_plan();
        let results = vec![view("c.x", CheckOutcomeView::Unresolved)];
        let out = apply_committed_outcome(plan.clone(), &results);
        assert_eq!(
            out, plan,
            "unresolved is not a committed pass/fail ⇒ no override"
        );
    }

    #[test]
    fn failed_check_yields_fail_forward_distinct_from_pre_adjudication() {
        let pre = base_plan(); // the pre-adjudication plan (no committed result seen)
        let results = vec![view("c.dodge", CheckOutcomeView::Failed)];
        let post = apply_committed_outcome(pre.clone(), &results);

        assert_eq!(post.beat_kind, BeatKind::Complicate);
        assert_eq!(post.desired_change, DESIRED_CHANGE_FAIL_FORWARD);
        // Provably different from the pre-adjudication plan on the SAME fixture.
        assert_ne!(post.beat_kind, pre.beat_kind);
        assert_ne!(post.desired_change, pre.desired_change);
        // Every guarantee preserved: reveal gating + thread selection untouched.
        assert_eq!(
            post.reveal_candidate_fact_ids,
            pre.reveal_candidate_fact_ids
        );
        assert_eq!(post.primary_thread_id, pre.primary_thread_id);
    }

    #[test]
    fn passed_check_capitalizes_with_escalate() {
        let pre = base_plan();
        let results = vec![view("c.spot", CheckOutcomeView::Passed)];
        let post = apply_committed_outcome(pre.clone(), &results);
        assert_eq!(post.beat_kind, BeatKind::Escalate);
        assert_eq!(post.desired_change, DESIRED_CHANGE_CAPITALIZE_SUCCESS);
        assert_ne!(post, pre);
    }

    #[test]
    fn failure_dominates_a_mixed_turn() {
        let pre = base_plan();
        let results = vec![
            view("c.spot", CheckOutcomeView::Passed),
            view("c.dodge", CheckOutcomeView::Failed),
        ];
        let post = apply_committed_outcome(pre, &results);
        assert_eq!(
            post.beat_kind,
            BeatKind::Complicate,
            "failure dominates a mixed turn"
        );
        assert_eq!(post.desired_change, DESIRED_CHANGE_FAIL_FORWARD);
    }
}

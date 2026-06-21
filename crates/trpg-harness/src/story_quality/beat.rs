//! L2.x — Story-quality checkpoints (pure, provider-free; no LLM, no DB).
//!
//! These mirror the mechanics-checkpoint families already in `lib.rs` (Knowledge / NpcSocial /
//! Memory / FlightRecorder): a typed spec + observed evidence + a fail-closed terminal state +
//! a pure classifier, all unit-testable without spawning the CLI, an LLM, or a database.
//!
//! Checkpoint #1 (this lane, L2.1) — **the Beat reflects the committed result**: the structural
//! guarantee of the post-adjudication Director spine (L1.2). The spine's overlay
//! (`apply_committed_outcome`) re-points the beat to the REAL committed disposition — a committed
//! FAILURE earns a fail-forward `Complicate`, a committed SUCCESS earns an escalating `Escalate`,
//! and no committed pass/fail signal leaves the beat untouched (fail-closed). This checkpoint is
//! an INDEPENDENT oracle: it re-derives the expected disposition from the committed outcomes and
//! checks the emitted beat against it — it never calls the production overlay (so it cannot be
//! tautological).
//!
//! Subsequent lanes (L2.2 rejected-thread no-railroad, L2.3 premature-reveal fail-closed,
//! L9.1 the remaining 5) extend THIS module with their own checkpoint families.

use serde::{Deserialize, Serialize};
use trpg_model::{BeatKind, CheckOutcomeView};

/// The generic `desired_change` tokens the spine overlay stamps (kept in sync with
/// `trpg-director::story::outcome` — re-encoded here so the harness is an INDEPENDENT verifier,
/// not a caller of the production code it checks).
pub const DESIRED_CHANGE_FAIL_FORWARD: &str = "fail_forward";
pub const DESIRED_CHANGE_CAPITALIZE_SUCCESS: &str = "capitalize_success";

/// Checkpoint #1 spec: assert the post-adjudication beat reflects the committed result.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoryBeatCheckpoint {
    /// When false the checkpoint is inert (no story-beat assertion configured for the turn).
    #[serde(default)]
    pub require_beat_reflects_committed: bool,
}

impl StoryBeatCheckpoint {
    pub fn is_active(&self) -> bool {
        self.require_beat_reflects_committed
    }
}

/// Observed story-beat evidence for one turn: the committed check dispositions (from the turn
/// ledger, projected as `MechanicalResultView`s) and the emitted post-adjudication beat.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoryBeatEvidence {
    /// The committed check outcomes this turn (order-preserving; failure dominance is by value,
    /// not position). Empty / all-`Unresolved` ⇒ no committed pass/fail signal.
    #[serde(default)]
    pub committed_outcomes: Vec<CheckOutcomeView>,
    /// The `beat_kind` the post-adjudication DirectorPlan emitted (the `director_spine` trace's
    /// `beat_kind` field).
    #[serde(default)]
    pub beat_kind: Option<BeatKind>,
    /// The DirectorPlan `desired_change` token emitted alongside the beat.
    #[serde(default)]
    pub desired_change: String,
}

/// The committed aggregate disposition (mirror of the overlay's `aggregate`): failure dominates,
/// then success, else no signal. Re-derived independently here so the checkpoint is an oracle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommittedDisposition {
    Failed,
    Passed,
    None,
}

pub fn aggregate_disposition(outcomes: &[CheckOutcomeView]) -> CommittedDisposition {
    let mut any_passed = false;
    for o in outcomes {
        match o {
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

/// Terminal classification of the story-beat checkpoint. Fail-closed: a beat that contradicts the
/// committed result can never reach `Pass`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StoryBeatCheckpointState {
    /// No active assertion, or no beat was emitted to check.
    InvalidSetup,
    /// No committed pass/fail this turn ⇒ the overlay leaves the beat untouched (fail-closed);
    /// there is no committed reflection to verify. Not a failure.
    NotTriggered,
    /// A committed pass/fail exists but the emitted beat does not reflect it (the inversion the
    /// spine fixes: a beat that ignores the real outcome).
    BeatIgnoresCommitted,
    /// The beat reflects the committed disposition.
    Pass,
}

impl StoryBeatCheckpointState {
    pub fn as_str(self) -> &'static str {
        match self {
            StoryBeatCheckpointState::InvalidSetup => "INVALID_SETUP",
            StoryBeatCheckpointState::NotTriggered => "NOT_TRIGGERED",
            StoryBeatCheckpointState::BeatIgnoresCommitted => "BEAT_IGNORES_COMMITTED",
            StoryBeatCheckpointState::Pass => "PASS",
        }
    }

    /// True when the checkpoint did not fully pass. `NotTriggered` is NOT failing (fail-closed
    /// no-op is legitimate), matching the mechanics families' "did not run ⇒ not green-but-not-red"
    /// convention only for the trigger gate.
    pub fn is_failing(self) -> bool {
        matches!(
            self,
            StoryBeatCheckpointState::InvalidSetup | StoryBeatCheckpointState::BeatIgnoresCommitted
        )
    }

    /// Earlier (more fundamental) failures sort first. Lower is earlier.
    pub fn chain_rank(self) -> u8 {
        match self {
            StoryBeatCheckpointState::InvalidSetup => 0,
            StoryBeatCheckpointState::BeatIgnoresCommitted => 1,
            StoryBeatCheckpointState::NotTriggered => 2,
            StoryBeatCheckpointState::Pass => 3,
        }
    }
}

/// True when `(beat_kind, desired_change)` reflect the committed `disposition` per the spine
/// contract. `None` disposition is handled by the caller (NotTriggered), never here.
fn beat_reflects(disposition: CommittedDisposition, beat: BeatKind, desired_change: &str) -> bool {
    match disposition {
        CommittedDisposition::Failed => {
            beat == BeatKind::Complicate && desired_change == DESIRED_CHANGE_FAIL_FORWARD
        }
        CommittedDisposition::Passed => {
            beat == BeatKind::Escalate && desired_change == DESIRED_CHANGE_CAPITALIZE_SUCCESS
        }
        // The caller never asks about `None` (it short-circuits to NotTriggered).
        CommittedDisposition::None => true,
    }
}

/// Classify checkpoint #1 against observed evidence. Pure, fail-closed, root-cause ordered.
pub fn classify_story_beat_checkpoint(
    spec: &StoryBeatCheckpoint,
    ev: &StoryBeatEvidence,
) -> StoryBeatCheckpointState {
    if !spec.is_active() {
        return StoryBeatCheckpointState::InvalidSetup;
    }
    let Some(beat) = ev.beat_kind else {
        // an active assertion but no emitted beat to verify against ⇒ cannot run.
        return StoryBeatCheckpointState::InvalidSetup;
    };
    match aggregate_disposition(&ev.committed_outcomes) {
        CommittedDisposition::None => StoryBeatCheckpointState::NotTriggered,
        disposition => {
            if beat_reflects(disposition, beat, &ev.desired_change) {
                StoryBeatCheckpointState::Pass
            } else {
                StoryBeatCheckpointState::BeatIgnoresCommitted
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active() -> StoryBeatCheckpoint {
        StoryBeatCheckpoint {
            require_beat_reflects_committed: true,
        }
    }

    // ── PASSING fixtures: the beat reflects the committed result ───────────────────────────
    #[test]
    fn passing_fixture_failed_check_yields_fail_forward_complicate() {
        let ev = StoryBeatEvidence {
            committed_outcomes: vec![CheckOutcomeView::Failed],
            beat_kind: Some(BeatKind::Complicate),
            desired_change: DESIRED_CHANGE_FAIL_FORWARD.into(),
        };
        assert_eq!(
            classify_story_beat_checkpoint(&active(), &ev),
            StoryBeatCheckpointState::Pass
        );
    }

    #[test]
    fn passing_fixture_passed_check_yields_capitalize_escalate() {
        let ev = StoryBeatEvidence {
            committed_outcomes: vec![CheckOutcomeView::Passed],
            beat_kind: Some(BeatKind::Escalate),
            desired_change: DESIRED_CHANGE_CAPITALIZE_SUCCESS.into(),
        };
        assert_eq!(
            classify_story_beat_checkpoint(&active(), &ev),
            StoryBeatCheckpointState::Pass
        );
    }

    #[test]
    fn failure_dominates_a_mixed_turn_passes_when_beat_is_fail_forward() {
        let ev = StoryBeatEvidence {
            committed_outcomes: vec![CheckOutcomeView::Passed, CheckOutcomeView::Failed],
            beat_kind: Some(BeatKind::Complicate),
            desired_change: DESIRED_CHANGE_FAIL_FORWARD.into(),
        };
        assert_eq!(
            classify_story_beat_checkpoint(&active(), &ev),
            StoryBeatCheckpointState::Pass
        );
    }

    // ── FAILING fixture: a beat that ignores the committed result (the inversion) ───────────
    #[test]
    fn failing_fixture_beat_ignores_committed_failure() {
        // a committed FAILURE but the beat escalated as if it had succeeded ⇒ the turn-order
        // inversion the spine exists to fix. Must classify as a failure, never Pass.
        let ev = StoryBeatEvidence {
            committed_outcomes: vec![CheckOutcomeView::Failed],
            beat_kind: Some(BeatKind::Escalate),
            desired_change: DESIRED_CHANGE_CAPITALIZE_SUCCESS.into(),
        };
        let state = classify_story_beat_checkpoint(&active(), &ev);
        assert_eq!(state, StoryBeatCheckpointState::BeatIgnoresCommitted);
        assert!(state.is_failing());
    }

    #[test]
    fn failing_fixture_right_beat_kind_wrong_desired_change_still_fails() {
        // beat_kind matches but the desired_change token drifted ⇒ still a contradiction.
        let ev = StoryBeatEvidence {
            committed_outcomes: vec![CheckOutcomeView::Passed],
            beat_kind: Some(BeatKind::Escalate),
            desired_change: "freeform".into(),
        };
        assert_eq!(
            classify_story_beat_checkpoint(&active(), &ev),
            StoryBeatCheckpointState::BeatIgnoresCommitted
        );
    }

    // ── trigger gate: no committed pass/fail ⇒ NotTriggered (fail-closed no-op, not a failure) ─
    #[test]
    fn no_committed_signal_is_not_triggered_not_a_failure() {
        let ev = StoryBeatEvidence {
            committed_outcomes: vec![CheckOutcomeView::Unresolved],
            beat_kind: Some(BeatKind::Respond),
            desired_change: "shift_situation".into(),
        };
        let state = classify_story_beat_checkpoint(&active(), &ev);
        assert_eq!(state, StoryBeatCheckpointState::NotTriggered);
        assert!(!state.is_failing());
    }

    #[test]
    fn empty_outcomes_is_not_triggered() {
        let ev = StoryBeatEvidence {
            committed_outcomes: vec![],
            beat_kind: Some(BeatKind::Respond),
            desired_change: String::new(),
        };
        assert_eq!(
            classify_story_beat_checkpoint(&active(), &ev),
            StoryBeatCheckpointState::NotTriggered
        );
    }

    // ── setup guards ───────────────────────────────────────────────────────────────────────
    #[test]
    fn inactive_spec_is_invalid_setup() {
        let ev = StoryBeatEvidence::default();
        assert_eq!(
            classify_story_beat_checkpoint(&StoryBeatCheckpoint::default(), &ev),
            StoryBeatCheckpointState::InvalidSetup
        );
    }

    #[test]
    fn active_but_no_beat_emitted_is_invalid_setup() {
        let ev = StoryBeatEvidence {
            committed_outcomes: vec![CheckOutcomeView::Failed],
            beat_kind: None,
            desired_change: String::new(),
        };
        assert_eq!(
            classify_story_beat_checkpoint(&active(), &ev),
            StoryBeatCheckpointState::InvalidSetup
        );
    }

    #[test]
    fn serde_round_trip_evidence() {
        let ev = StoryBeatEvidence {
            committed_outcomes: vec![CheckOutcomeView::Failed, CheckOutcomeView::Passed],
            beat_kind: Some(BeatKind::Complicate),
            desired_change: DESIRED_CHANGE_FAIL_FORWARD.into(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: StoryBeatEvidence = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }
}

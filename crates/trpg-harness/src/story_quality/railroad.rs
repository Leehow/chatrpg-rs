//! Checkpoint #2 (L2.2) — **rejected-thread no-railroad**.
//!
//! The Director's anti-railroad invariant (`trpg-director::story::select`, §二十四-#13): a thread
//! the player has rejected (`rejected_thread_ids`) carries a dominating `railroading_risk` penalty
//! (`REJECTED_RAILROAD_PENALTY = 1000.0`, `select.rs:59`) AND is skipped explicitly in the
//! primary/secondary pick loop (`select.rs:121`) — so it can never be re-pushed as a spotlighted
//! thread. This checkpoint is an INDEPENDENT oracle: it re-checks `rejected ∩ (primary ∪ secondary)`
//! directly against the emitted `DirectorPlan`, never calling the production scorer.
//!
//! Fail-closed: a rejected thread appearing as primary or secondary can NEVER reach `Pass`.

use serde::{Deserialize, Serialize};

/// Checkpoint #2 spec: assert no rejected thread is spotlighted.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RejectedThreadCheckpoint {
    /// When false the checkpoint is inert (no anti-railroad assertion configured for the turn).
    #[serde(default)]
    pub require_no_rejected_spotlight: bool,
}

impl RejectedThreadCheckpoint {
    pub fn is_active(&self) -> bool {
        self.require_no_rejected_spotlight
    }
}

/// Observed selection evidence for one turn: the player-rejected threads and the threads the
/// emitted `DirectorPlan` actually spotlighted (primary + secondary).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RejectedThreadEvidence {
    /// Threads the player rejected (the `rejected_thread_ids` fed to the selector).
    #[serde(default)]
    pub rejected_thread_ids: Vec<String>,
    /// The spotlighted primary thread (`DirectorPlan.primary_thread_id`), if any.
    #[serde(default)]
    pub primary_thread_id: Option<String>,
    /// The spotlighted secondary threads (`DirectorPlan.secondary_thread_ids`).
    #[serde(default)]
    pub secondary_thread_ids: Vec<String>,
}

impl RejectedThreadEvidence {
    /// All threads the plan spotlighted (primary first, then secondaries), order-preserving.
    fn spotlighted(&self) -> Vec<&str> {
        self.primary_thread_id
            .as_deref()
            .into_iter()
            .chain(self.secondary_thread_ids.iter().map(String::as_str))
            .collect()
    }

    /// Rejected threads that nonetheless got spotlighted — the railroad violation set (empty ⇒ ok).
    pub fn spotlighted_rejected_ids(&self) -> Vec<String> {
        let rejected: std::collections::HashSet<&str> =
            self.rejected_thread_ids.iter().map(String::as_str).collect();
        self.spotlighted()
            .into_iter()
            .filter(|id| rejected.contains(id))
            .map(str::to_string)
            .collect()
    }
}

/// Terminal classification of the anti-railroad checkpoint. Fail-closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RejectedThreadCheckpointState {
    /// No active assertion configured.
    InvalidSetup,
    /// No rejected threads this turn ⇒ the anti-railroad path was not exercised (not a failure).
    NotTriggered,
    /// A rejected thread was re-pushed to primary/secondary (the railroad the invariant forbids).
    RejectedThreadSpotlighted,
    /// No rejected thread is spotlighted.
    Pass,
}

impl RejectedThreadCheckpointState {
    pub fn as_str(self) -> &'static str {
        match self {
            RejectedThreadCheckpointState::InvalidSetup => "INVALID_SETUP",
            RejectedThreadCheckpointState::NotTriggered => "NOT_TRIGGERED",
            RejectedThreadCheckpointState::RejectedThreadSpotlighted => "REJECTED_THREAD_SPOTLIGHTED",
            RejectedThreadCheckpointState::Pass => "PASS",
        }
    }

    pub fn is_failing(self) -> bool {
        matches!(
            self,
            RejectedThreadCheckpointState::InvalidSetup
                | RejectedThreadCheckpointState::RejectedThreadSpotlighted
        )
    }

    /// Earlier (more fundamental) failures sort first. Lower is earlier.
    pub fn chain_rank(self) -> u8 {
        match self {
            RejectedThreadCheckpointState::InvalidSetup => 0,
            RejectedThreadCheckpointState::RejectedThreadSpotlighted => 1,
            RejectedThreadCheckpointState::NotTriggered => 2,
            RejectedThreadCheckpointState::Pass => 3,
        }
    }
}

/// Classify checkpoint #2 against observed evidence. Pure, fail-closed, root-cause ordered.
pub fn classify_rejected_thread_checkpoint(
    spec: &RejectedThreadCheckpoint,
    ev: &RejectedThreadEvidence,
) -> RejectedThreadCheckpointState {
    if !spec.is_active() {
        return RejectedThreadCheckpointState::InvalidSetup;
    }
    if ev.rejected_thread_ids.is_empty() {
        // no rejection to honor this turn ⇒ the anti-railroad invariant is vacuous (no-op).
        return RejectedThreadCheckpointState::NotTriggered;
    }
    if ev.spotlighted_rejected_ids().is_empty() {
        RejectedThreadCheckpointState::Pass
    } else {
        RejectedThreadCheckpointState::RejectedThreadSpotlighted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active() -> RejectedThreadCheckpoint {
        RejectedThreadCheckpoint {
            require_no_rejected_spotlight: true,
        }
    }

    // ── PASSING fixtures: a rejected thread is NOT re-pushed ────────────────────────────────
    #[test]
    fn passing_fixture_rejected_thread_not_spotlighted() {
        let ev = RejectedThreadEvidence {
            rejected_thread_ids: vec!["t_rejected".into()],
            primary_thread_id: Some("t_other".into()),
            secondary_thread_ids: vec!["t_third".into()],
        };
        assert_eq!(
            classify_rejected_thread_checkpoint(&active(), &ev),
            RejectedThreadCheckpointState::Pass
        );
    }

    #[test]
    fn passing_fixture_rejected_thread_with_no_selection_emitted() {
        // rejected thread present but nothing spotlighted (e.g. WorldQuery turn) ⇒ invariant holds.
        let ev = RejectedThreadEvidence {
            rejected_thread_ids: vec!["t_rejected".into()],
            primary_thread_id: None,
            secondary_thread_ids: vec![],
        };
        assert_eq!(
            classify_rejected_thread_checkpoint(&active(), &ev),
            RejectedThreadCheckpointState::Pass
        );
    }

    // ── FAILING fixtures: a rejected thread re-pushed (the railroad) ────────────────────────
    #[test]
    fn failing_fixture_rejected_thread_as_primary() {
        let ev = RejectedThreadEvidence {
            rejected_thread_ids: vec!["t_rejected".into()],
            primary_thread_id: Some("t_rejected".into()),
            secondary_thread_ids: vec![],
        };
        let state = classify_rejected_thread_checkpoint(&active(), &ev);
        assert_eq!(state, RejectedThreadCheckpointState::RejectedThreadSpotlighted);
        assert!(state.is_failing());
        assert_eq!(ev.spotlighted_rejected_ids(), vec!["t_rejected".to_string()]);
    }

    #[test]
    fn failing_fixture_rejected_thread_as_secondary() {
        let ev = RejectedThreadEvidence {
            rejected_thread_ids: vec!["t_a".into(), "t_rejected".into()],
            primary_thread_id: Some("t_clean".into()),
            secondary_thread_ids: vec!["t_rejected".into()],
        };
        assert_eq!(
            classify_rejected_thread_checkpoint(&active(), &ev),
            RejectedThreadCheckpointState::RejectedThreadSpotlighted
        );
    }

    // ── trigger gate: no rejected threads ⇒ NotTriggered (vacuous, not a failure) ────────────
    #[test]
    fn no_rejected_threads_is_not_triggered_not_a_failure() {
        let ev = RejectedThreadEvidence {
            rejected_thread_ids: vec![],
            primary_thread_id: Some("t_any".into()),
            secondary_thread_ids: vec!["t_other".into()],
        };
        let state = classify_rejected_thread_checkpoint(&active(), &ev);
        assert_eq!(state, RejectedThreadCheckpointState::NotTriggered);
        assert!(!state.is_failing());
    }

    // ── setup guard ─────────────────────────────────────────────────────────────────────────
    #[test]
    fn inactive_spec_is_invalid_setup() {
        let ev = RejectedThreadEvidence::default();
        assert_eq!(
            classify_rejected_thread_checkpoint(&RejectedThreadCheckpoint::default(), &ev),
            RejectedThreadCheckpointState::InvalidSetup
        );
    }

    #[test]
    fn serde_round_trip_evidence() {
        let ev = RejectedThreadEvidence {
            rejected_thread_ids: vec!["t_rejected".into()],
            primary_thread_id: Some("t_clean".into()),
            secondary_thread_ids: vec!["t_rejected".into(), "t_b".into()],
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: RejectedThreadEvidence = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
        // chain ordering sanity: a failure sorts before NotTriggered/Pass.
        assert!(
            RejectedThreadCheckpointState::RejectedThreadSpotlighted.chain_rank()
                < RejectedThreadCheckpointState::Pass.chain_rank()
        );
    }
}

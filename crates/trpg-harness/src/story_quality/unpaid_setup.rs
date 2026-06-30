//! Checkpoint #4 (L9.1) — **unpaid setups** (Chekhov's gun).
//!
//! A promise that has matured to Ripe (its payoff is set up and overdue) but is never paid off is
//! a dangling setup — the story promised something it never delivered. This checkpoint is an
//! INDEPENDENT oracle over the committed [`crate::story_quality`] promise observations: it re-checks
//! "a ripe, overdue promise that is neither PaidOff nor explicitly Broken" directly, never calling
//! the production Story Observer. Fail-closed: a ripe overdue unpaid promise can never `Pass`.

use serde::{Deserialize, Serialize};

/// Checkpoint #4 spec: the maturity at/above which a promise is considered "ripe" (overdue to pay).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UnpaidSetupCheckpoint {
    #[serde(default)]
    pub active: bool,
    /// Maturity threshold for "ripe". Default `0.8` (matches the Story Observer's Ripe floor).
    #[serde(default)]
    pub ripe_threshold: f32,
}

impl Default for UnpaidSetupCheckpoint {
    fn default() -> Self {
        Self {
            active: false,
            ripe_threshold: 0.8,
        }
    }
}

impl UnpaidSetupCheckpoint {
    pub fn active() -> Self {
        Self {
            active: true,
            ripe_threshold: 0.8,
        }
    }
    pub fn is_active(&self) -> bool {
        self.active
    }
}

/// One observed promise at chapter end.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PromiseObservation {
    pub promise_id: String,
    #[serde(default)]
    pub maturity: f32,
    /// The promise has been delivered (`PromiseStatus::PaidOff`).
    #[serde(default)]
    pub paid_off: bool,
    /// The promise was explicitly broken (`PromiseStatus::Broken`) — a deliberate, acceptable end.
    #[serde(default)]
    pub broken: bool,
    /// The promise is past its `earliest_payoff_turn` floor (overdue to pay off).
    #[serde(default)]
    pub overdue: bool,
}

/// All observed promises for the chapter transcript.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct UnpaidSetupEvidence {
    #[serde(default)]
    pub promises: Vec<PromiseObservation>,
}

impl UnpaidSetupEvidence {
    /// Ripe, overdue promises that were never delivered nor explicitly broken (the violation set).
    pub fn unpaid_ids(&self, ripe_threshold: f32) -> Vec<String> {
        self.promises
            .iter()
            .filter(|p| p.maturity >= ripe_threshold && p.overdue && !p.paid_off && !p.broken)
            .map(|p| p.promise_id.clone())
            .collect()
    }
}

/// Terminal classification. Fail-closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UnpaidSetupCheckpointState {
    InvalidSetup,
    /// No promises to evaluate this chapter ⇒ vacuous (not a failure).
    NotTriggered,
    /// A ripe, overdue promise was never paid off (the dangling setup).
    UnpaidSetup,
    Pass,
}

impl UnpaidSetupCheckpointState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidSetup => "INVALID_SETUP",
            Self::NotTriggered => "NOT_TRIGGERED",
            Self::UnpaidSetup => "UNPAID_SETUP",
            Self::Pass => "PASS",
        }
    }
    pub fn is_failing(self) -> bool {
        matches!(self, Self::InvalidSetup | Self::UnpaidSetup)
    }
}

/// Classify checkpoint #4. Pure, fail-closed.
pub fn classify_unpaid_setup_checkpoint(
    spec: &UnpaidSetupCheckpoint,
    ev: &UnpaidSetupEvidence,
) -> UnpaidSetupCheckpointState {
    if !spec.is_active() {
        return UnpaidSetupCheckpointState::InvalidSetup;
    }
    if ev.promises.is_empty() {
        return UnpaidSetupCheckpointState::NotTriggered;
    }
    if ev.unpaid_ids(spec.ripe_threshold).is_empty() {
        UnpaidSetupCheckpointState::Pass
    } else {
        UnpaidSetupCheckpointState::UnpaidSetup
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(id: &str, maturity: f32, paid: bool, broken: bool, overdue: bool) -> PromiseObservation {
        PromiseObservation {
            promise_id: id.into(),
            maturity,
            paid_off: paid,
            broken,
            overdue,
        }
    }

    #[test]
    fn passing_fixture_ripe_promise_paid_off() {
        let ev = UnpaidSetupEvidence {
            promises: vec![obs("pr", 1.0, true, false, true)],
        };
        assert_eq!(
            classify_unpaid_setup_checkpoint(&UnpaidSetupCheckpoint::active(), &ev),
            UnpaidSetupCheckpointState::Pass
        );
    }

    #[test]
    fn passing_fixture_immature_promise_not_yet_due() {
        // Not ripe and not overdue ⇒ not a dangling setup.
        let ev = UnpaidSetupEvidence {
            promises: vec![obs("pr", 0.4, false, false, false)],
        };
        assert_eq!(
            classify_unpaid_setup_checkpoint(&UnpaidSetupCheckpoint::active(), &ev),
            UnpaidSetupCheckpointState::Pass
        );
    }

    #[test]
    fn passing_fixture_broken_promise_is_acceptable() {
        let ev = UnpaidSetupEvidence {
            promises: vec![obs("pr", 0.9, false, true, true)],
        };
        assert_eq!(
            classify_unpaid_setup_checkpoint(&UnpaidSetupCheckpoint::active(), &ev),
            UnpaidSetupCheckpointState::Pass
        );
    }

    #[test]
    fn failing_fixture_ripe_overdue_unpaid() {
        let ev = UnpaidSetupEvidence {
            promises: vec![obs("pr_dangling", 0.95, false, false, true)],
        };
        let state = classify_unpaid_setup_checkpoint(&UnpaidSetupCheckpoint::active(), &ev);
        assert_eq!(state, UnpaidSetupCheckpointState::UnpaidSetup);
        assert!(state.is_failing());
        assert_eq!(ev.unpaid_ids(0.8), vec!["pr_dangling".to_string()]);
    }

    #[test]
    fn no_promises_is_not_triggered() {
        assert_eq!(
            classify_unpaid_setup_checkpoint(
                &UnpaidSetupCheckpoint::active(),
                &UnpaidSetupEvidence::default()
            ),
            UnpaidSetupCheckpointState::NotTriggered
        );
    }

    #[test]
    fn inactive_is_invalid_setup() {
        assert_eq!(
            classify_unpaid_setup_checkpoint(
                &UnpaidSetupCheckpoint::default(),
                &UnpaidSetupEvidence::default()
            ),
            UnpaidSetupCheckpointState::InvalidSetup
        );
    }

    #[test]
    fn serde_round_trip() {
        let ev = UnpaidSetupEvidence {
            promises: vec![obs("pr", 0.9, false, false, true)],
        };
        let back: UnpaidSetupEvidence =
            serde_json::from_str(&serde_json::to_string(&ev).unwrap()).unwrap();
        assert_eq!(ev, back);
    }
}

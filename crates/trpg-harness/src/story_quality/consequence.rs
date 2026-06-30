//! Checkpoint #7 (L9.1) — **choice → consequence**.
//!
//! A committed player choice must produce a downstream consequence (a later beat/event that
//! references it) — choices that vanish make the world feel inert. This INDEPENDENT oracle
//! re-derives "committed choice ids with no later consequence reference" directly, never calling
//! production code. Fail-closed: a dangling choice can never `Pass`.

use serde::{Deserialize, Serialize};

/// Checkpoint #7 spec.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConsequenceCheckpoint {
    #[serde(default)]
    pub require_consequences: bool,
}

impl ConsequenceCheckpoint {
    pub fn active() -> Self {
        Self {
            require_consequences: true,
        }
    }
    pub fn is_active(&self) -> bool {
        self.require_consequences
    }
}

/// Observed choice/consequence evidence for the chapter.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConsequenceEvidence {
    /// Ids of committed player choices.
    #[serde(default)]
    pub committed_choice_ids: Vec<String>,
    /// Choice ids a LATER consequence beat/event referenced (the payoff side).
    #[serde(default)]
    pub consequence_referenced_choice_ids: Vec<String>,
}

impl ConsequenceEvidence {
    /// Committed choices with no downstream consequence reference (the violation set).
    pub fn dangling_choice_ids(&self) -> Vec<String> {
        let referenced: std::collections::HashSet<&str> = self
            .consequence_referenced_choice_ids
            .iter()
            .map(String::as_str)
            .collect();
        self.committed_choice_ids
            .iter()
            .filter(|c| !referenced.contains(c.as_str()))
            .cloned()
            .collect()
    }
}

/// Terminal classification. Fail-closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConsequenceCheckpointState {
    InvalidSetup,
    /// No committed choices this chapter ⇒ vacuous.
    NotTriggered,
    /// A committed choice produced no downstream consequence.
    DanglingChoice,
    Pass,
}

impl ConsequenceCheckpointState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidSetup => "INVALID_SETUP",
            Self::NotTriggered => "NOT_TRIGGERED",
            Self::DanglingChoice => "DANGLING_CHOICE",
            Self::Pass => "PASS",
        }
    }
    pub fn is_failing(self) -> bool {
        matches!(self, Self::InvalidSetup | Self::DanglingChoice)
    }
}

/// Classify checkpoint #7. Pure, fail-closed.
pub fn classify_consequence_checkpoint(
    spec: &ConsequenceCheckpoint,
    ev: &ConsequenceEvidence,
) -> ConsequenceCheckpointState {
    if !spec.is_active() {
        return ConsequenceCheckpointState::InvalidSetup;
    }
    if ev.committed_choice_ids.is_empty() {
        return ConsequenceCheckpointState::NotTriggered;
    }
    if ev.dangling_choice_ids().is_empty() {
        ConsequenceCheckpointState::Pass
    } else {
        ConsequenceCheckpointState::DanglingChoice
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passing_fixture_every_choice_has_consequence() {
        let ev = ConsequenceEvidence {
            committed_choice_ids: vec!["c1".into(), "c2".into()],
            consequence_referenced_choice_ids: vec!["c1".into(), "c2".into(), "c0".into()],
        };
        assert_eq!(
            classify_consequence_checkpoint(&ConsequenceCheckpoint::active(), &ev),
            ConsequenceCheckpointState::Pass
        );
    }

    #[test]
    fn failing_fixture_dangling_choice() {
        let ev = ConsequenceEvidence {
            committed_choice_ids: vec!["c1".into(), "c2".into()],
            consequence_referenced_choice_ids: vec!["c1".into()],
        };
        let state = classify_consequence_checkpoint(&ConsequenceCheckpoint::active(), &ev);
        assert_eq!(state, ConsequenceCheckpointState::DanglingChoice);
        assert!(state.is_failing());
        assert_eq!(ev.dangling_choice_ids(), vec!["c2".to_string()]);
    }

    #[test]
    fn no_choices_is_not_triggered() {
        assert_eq!(
            classify_consequence_checkpoint(
                &ConsequenceCheckpoint::active(),
                &ConsequenceEvidence::default()
            ),
            ConsequenceCheckpointState::NotTriggered
        );
    }

    #[test]
    fn inactive_is_invalid_setup() {
        assert_eq!(
            classify_consequence_checkpoint(
                &ConsequenceCheckpoint::default(),
                &ConsequenceEvidence::default()
            ),
            ConsequenceCheckpointState::InvalidSetup
        );
    }

    #[test]
    fn serde_round_trip() {
        let ev = ConsequenceEvidence {
            committed_choice_ids: vec!["c1".into()],
            consequence_referenced_choice_ids: vec![],
        };
        let back: ConsequenceEvidence =
            serde_json::from_str(&serde_json::to_string(&ev).unwrap()).unwrap();
        assert_eq!(ev, back);
    }
}

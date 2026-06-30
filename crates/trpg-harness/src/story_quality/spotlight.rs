//! Checkpoint #8 (L9.1) — **ignored PC background** (spotlight debt).
//!
//! A PC whose `CharacterArcState.spotlight_debt` climbs high but who never gets focus is being
//! sidelined — their background/hooks go unused. This INDEPENDENT oracle re-derives "an arc with
//! debt above threshold that was never recently in `focus_actor_ids`" directly, never calling the
//! production scorer (the L5.2 arc-aware term is what it cross-checks). Fail-closed.

use serde::{Deserialize, Serialize};

/// Checkpoint #8 spec: the spotlight-debt above which an unfocused PC is a violation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpotlightDebtCheckpoint {
    #[serde(default)]
    pub active: bool,
    /// Debt threshold (normalized `0.0..=1.0`). Default `0.7`.
    #[serde(default)]
    pub max_debt: f32,
}

impl Default for SpotlightDebtCheckpoint {
    fn default() -> Self {
        Self {
            active: false,
            max_debt: 0.7,
        }
    }
}

impl SpotlightDebtCheckpoint {
    pub fn active() -> Self {
        Self {
            active: true,
            max_debt: 0.7,
        }
    }
    pub fn is_active(&self) -> bool {
        self.active
    }
}

/// One observed character arc at chapter end.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ArcObservation {
    pub character_id: String,
    #[serde(default)]
    pub spotlight_debt: f32,
    /// The character was in `focus_actor_ids` at least once over the assessed window.
    #[serde(default)]
    pub focused_recently: bool,
}

/// Observed arcs for the chapter.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SpotlightDebtEvidence {
    #[serde(default)]
    pub arcs: Vec<ArcObservation>,
}

impl SpotlightDebtEvidence {
    /// Arcs over the debt threshold that were never focused (the violation set).
    pub fn neglected_ids(&self, max_debt: f32) -> Vec<String> {
        self.arcs
            .iter()
            .filter(|a| a.spotlight_debt > max_debt && !a.focused_recently)
            .map(|a| a.character_id.clone())
            .collect()
    }
}

/// Terminal classification. Fail-closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SpotlightDebtCheckpointState {
    InvalidSetup,
    /// No arcs to evaluate ⇒ vacuous.
    NotTriggered,
    /// A high-debt PC was never given focus (sidelined background).
    IgnoredBackground,
    Pass,
}

impl SpotlightDebtCheckpointState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidSetup => "INVALID_SETUP",
            Self::NotTriggered => "NOT_TRIGGERED",
            Self::IgnoredBackground => "IGNORED_BACKGROUND",
            Self::Pass => "PASS",
        }
    }
    pub fn is_failing(self) -> bool {
        matches!(self, Self::InvalidSetup | Self::IgnoredBackground)
    }
}

/// Classify checkpoint #8. Pure, fail-closed.
pub fn classify_spotlight_debt_checkpoint(
    spec: &SpotlightDebtCheckpoint,
    ev: &SpotlightDebtEvidence,
) -> SpotlightDebtCheckpointState {
    if !spec.is_active() {
        return SpotlightDebtCheckpointState::InvalidSetup;
    }
    if ev.arcs.is_empty() {
        return SpotlightDebtCheckpointState::NotTriggered;
    }
    if ev.neglected_ids(spec.max_debt).is_empty() {
        SpotlightDebtCheckpointState::Pass
    } else {
        SpotlightDebtCheckpointState::IgnoredBackground
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arc(id: &str, debt: f32, focused: bool) -> ArcObservation {
        ArcObservation {
            character_id: id.into(),
            spotlight_debt: debt,
            focused_recently: focused,
        }
    }

    #[test]
    fn passing_fixture_high_debt_but_focused() {
        let ev = SpotlightDebtEvidence {
            arcs: vec![arc("pc_1", 0.9, true)],
        };
        assert_eq!(
            classify_spotlight_debt_checkpoint(&SpotlightDebtCheckpoint::active(), &ev),
            SpotlightDebtCheckpointState::Pass
        );
    }

    #[test]
    fn passing_fixture_low_debt_unfocused_ok() {
        let ev = SpotlightDebtEvidence {
            arcs: vec![arc("pc_1", 0.3, false)],
        };
        assert_eq!(
            classify_spotlight_debt_checkpoint(&SpotlightDebtCheckpoint::active(), &ev),
            SpotlightDebtCheckpointState::Pass
        );
    }

    #[test]
    fn failing_fixture_high_debt_never_focused() {
        let ev = SpotlightDebtEvidence {
            arcs: vec![arc("pc_neglected", 0.85, false), arc("pc_ok", 0.2, false)],
        };
        let state = classify_spotlight_debt_checkpoint(&SpotlightDebtCheckpoint::active(), &ev);
        assert_eq!(state, SpotlightDebtCheckpointState::IgnoredBackground);
        assert!(state.is_failing());
        assert_eq!(ev.neglected_ids(0.7), vec!["pc_neglected".to_string()]);
    }

    #[test]
    fn no_arcs_is_not_triggered() {
        assert_eq!(
            classify_spotlight_debt_checkpoint(
                &SpotlightDebtCheckpoint::active(),
                &SpotlightDebtEvidence::default()
            ),
            SpotlightDebtCheckpointState::NotTriggered
        );
    }

    #[test]
    fn inactive_is_invalid_setup() {
        assert_eq!(
            classify_spotlight_debt_checkpoint(
                &SpotlightDebtCheckpoint::default(),
                &SpotlightDebtEvidence::default()
            ),
            SpotlightDebtCheckpointState::InvalidSetup
        );
    }

    #[test]
    fn serde_round_trip() {
        let ev = SpotlightDebtEvidence {
            arcs: vec![arc("pc_1", 0.9, false)],
        };
        let back: SpotlightDebtEvidence =
            serde_json::from_str(&serde_json::to_string(&ev).unwrap()).unwrap();
        assert_eq!(ev, back);
    }
}

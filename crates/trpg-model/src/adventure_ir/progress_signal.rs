//! Adventure IR — ProgressSignal (P0-3): the unified record of *what advanced*
//! this turn. J3 used to see only scene transitions and thus missed real progress
//! (objective completed in place, revelation found, clock advanced, mission phase
//! changed). These signals are the measuring stick (设计评审 §九): J3a semantic /
//! J3b spatial·phase / J3c closure read from them.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A single progression event emitted by the ProgressionEngine. `id` is the unit/
/// objective/clock/revelation/outcome it concerns; `detail` is an optional
/// human/diagnostic note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProgressSignal {
    pub kind: ProgressSignalKind,
    pub id: String,
    #[serde(default)]
    pub detail: Option<String>,
}

/// The taxonomy of progression. Grouped by J3 dimension in [`ProgressSignalKind::j3_axis`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProgressSignalKind {
    // — J3a semantic —
    BeatActivated,
    BeatResolved,
    ObjectiveAdvanced,
    ObjectiveCompleted,
    RevelationUnlocked,
    RevelationDelivered,
    ClockAdvanced,
    AgendaStepTriggered,
    OutcomeUnlocked,
    // — J3b spatial · phase —
    MissionPhaseChanged,
    LocationChanged,
}

/// The three J3 v2 axes a signal counts toward (设计评审 §九).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum J3Axis {
    /// Objective / Revelation / Clock / Agenda / Beat / Outcome.
    Semantic,
    /// Location / Scene / MissionPhase.
    SpatialPhase,
}

impl ProgressSignalKind {
    /// Which J3 v2 axis this signal feeds. This is what lets J3 stop misjudging
    /// "completed an objective in the same location" as no-progress.
    pub fn j3_axis(self) -> J3Axis {
        use ProgressSignalKind::*;
        match self {
            MissionPhaseChanged | LocationChanged => J3Axis::SpatialPhase,
            _ => J3Axis::Semantic,
        }
    }
}

impl ProgressSignal {
    pub fn new(kind: ProgressSignalKind, id: impl Into<String>) -> Self {
        ProgressSignal {
            kind,
            id: id.into(),
            detail: None,
        }
    }
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn objective_completion_is_semantic_not_spatial() {
        // The exact J3 misjudgment this fixes: completing an objective in place is
        // semantic progress even with no LocationChanged.
        assert_eq!(
            ProgressSignalKind::ObjectiveCompleted.j3_axis(),
            J3Axis::Semantic
        );
        assert_eq!(
            ProgressSignalKind::LocationChanged.j3_axis(),
            J3Axis::SpatialPhase
        );
        assert_eq!(
            ProgressSignalKind::MissionPhaseChanged.j3_axis(),
            J3Axis::SpatialPhase
        );
        assert_eq!(
            ProgressSignalKind::RevelationUnlocked.j3_axis(),
            J3Axis::Semantic
        );
    }

    #[test]
    fn signal_roundtrips_json() {
        let s = ProgressSignal::new(ProgressSignalKind::ClockAdvanced, "clock.escape")
            .with_detail("+1");
        let j = serde_json::to_string(&s).unwrap();
        let back: ProgressSignal = serde_json::from_str(&j).unwrap();
        assert_eq!(s, back);
    }
}

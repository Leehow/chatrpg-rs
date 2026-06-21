//! Checkpoint #5 (L9.1) — **repeated beat-kind** (monotony guard).
//!
//! The Director steers pacing by varying beats; the SAME `BeatKind` emitted too many turns in a
//! row reads as a stuck record (e.g. three consecutive `Complicate`s with no relief). This is an
//! INDEPENDENT oracle over the committed beat history: it re-derives the longest run of identical
//! consecutive beat kinds, never calling the production pacing logic. Fail-closed.

use serde::{Deserialize, Serialize};

/// Checkpoint #5 spec: the maximum allowed run of identical consecutive beat kinds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepeatedBeatCheckpoint {
    #[serde(default)]
    pub active: bool,
    /// Longest acceptable consecutive run of one beat kind. Default `2` (a third repeat fails).
    #[serde(default)]
    pub max_consecutive: usize,
}

impl Default for RepeatedBeatCheckpoint {
    fn default() -> Self {
        Self { active: false, max_consecutive: 2 }
    }
}

impl RepeatedBeatCheckpoint {
    pub fn active() -> Self {
        Self { active: true, max_consecutive: 2 }
    }
    pub fn is_active(&self) -> bool {
        self.active
    }
}

/// Observed beat-kind history for the chapter (one token per committed turn, in order).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepeatedBeatEvidence {
    #[serde(default)]
    pub beat_kinds: Vec<String>,
}

impl RepeatedBeatEvidence {
    /// The longest run of identical consecutive beat kinds (0 for an empty history).
    pub fn longest_run(&self) -> usize {
        let mut best = 0usize;
        let mut cur = 0usize;
        let mut prev: Option<&str> = None;
        for k in &self.beat_kinds {
            if Some(k.as_str()) == prev {
                cur += 1;
            } else {
                cur = 1;
                prev = Some(k.as_str());
            }
            best = best.max(cur);
        }
        best
    }
}

/// Terminal classification. Fail-closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RepeatedBeatCheckpointState {
    InvalidSetup,
    /// Too few beats to assess a run ⇒ vacuous.
    NotTriggered,
    /// A beat kind repeated beyond `max_consecutive` (monotony).
    RepeatedBeat,
    Pass,
}

impl RepeatedBeatCheckpointState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidSetup => "INVALID_SETUP",
            Self::NotTriggered => "NOT_TRIGGERED",
            Self::RepeatedBeat => "REPEATED_BEAT",
            Self::Pass => "PASS",
        }
    }
    pub fn is_failing(self) -> bool {
        matches!(self, Self::InvalidSetup | Self::RepeatedBeat)
    }
}

/// Classify checkpoint #5. Pure, fail-closed.
pub fn classify_repeated_beat_checkpoint(
    spec: &RepeatedBeatCheckpoint,
    ev: &RepeatedBeatEvidence,
) -> RepeatedBeatCheckpointState {
    if !spec.is_active() || spec.max_consecutive == 0 {
        return RepeatedBeatCheckpointState::InvalidSetup;
    }
    if ev.beat_kinds.len() <= spec.max_consecutive {
        return RepeatedBeatCheckpointState::NotTriggered;
    }
    if ev.longest_run() > spec.max_consecutive {
        RepeatedBeatCheckpointState::RepeatedBeat
    } else {
        RepeatedBeatCheckpointState::Pass
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kinds: &[&str]) -> RepeatedBeatEvidence {
        RepeatedBeatEvidence {
            beat_kinds: kinds.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn passing_fixture_varied_beats() {
        let e = ev(&["complicate", "complicate", "relief", "escalate"]);
        assert_eq!(e.longest_run(), 2);
        assert_eq!(
            classify_repeated_beat_checkpoint(&RepeatedBeatCheckpoint::active(), &e),
            RepeatedBeatCheckpointState::Pass
        );
    }

    #[test]
    fn failing_fixture_three_in_a_row() {
        let e = ev(&["complicate", "complicate", "complicate", "relief"]);
        let state = classify_repeated_beat_checkpoint(&RepeatedBeatCheckpoint::active(), &e);
        assert_eq!(state, RepeatedBeatCheckpointState::RepeatedBeat);
        assert!(state.is_failing());
        assert_eq!(e.longest_run(), 3);
    }

    #[test]
    fn non_adjacent_repeats_do_not_fail() {
        // same kind appears 3x total but never 3 consecutively ⇒ ok.
        let e = ev(&["reveal", "complicate", "reveal", "complicate", "reveal"]);
        assert_eq!(e.longest_run(), 1);
        assert_eq!(
            classify_repeated_beat_checkpoint(&RepeatedBeatCheckpoint::active(), &e),
            RepeatedBeatCheckpointState::Pass
        );
    }

    #[test]
    fn too_short_history_is_not_triggered() {
        assert_eq!(
            classify_repeated_beat_checkpoint(
                &RepeatedBeatCheckpoint::active(),
                &ev(&["complicate", "complicate"])
            ),
            RepeatedBeatCheckpointState::NotTriggered
        );
    }

    #[test]
    fn inactive_is_invalid_setup() {
        assert_eq!(
            classify_repeated_beat_checkpoint(
                &RepeatedBeatCheckpoint::default(),
                &ev(&["a", "b", "c", "d"])
            ),
            RepeatedBeatCheckpointState::InvalidSetup
        );
    }

    #[test]
    fn serde_round_trip() {
        let e = ev(&["complicate", "complicate", "complicate"]);
        let back: RepeatedBeatEvidence =
            serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(e, back);
    }
}

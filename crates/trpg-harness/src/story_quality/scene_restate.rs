//! Checkpoint #6 (L9.1) — **scene-restate loop** (treading water).
//!
//! The Director should relocate/advance, not re-describe the same scene turn after turn with no
//! new substance. This INDEPENDENT oracle re-derives the longest run of consecutive turns that
//! sit in the SAME scene WITHOUT making progress (no new fact / no beat advancement), never
//! calling the production scene logic. Fail-closed.

use serde::{Deserialize, Serialize};

/// Checkpoint #6 spec: the maximum allowed run of same-scene, no-progress turns.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SceneRestateCheckpoint {
    #[serde(default)]
    pub active: bool,
    /// Longest acceptable run of no-progress turns in one scene. Default `2`.
    #[serde(default)]
    pub max_stall: usize,
}

impl Default for SceneRestateCheckpoint {
    fn default() -> Self {
        Self {
            active: false,
            max_stall: 2,
        }
    }
}

impl SceneRestateCheckpoint {
    pub fn active() -> Self {
        Self {
            active: true,
            max_stall: 2,
        }
    }
    pub fn is_active(&self) -> bool {
        self.active
    }
}

/// One observed turn: its scene id and whether the turn made narrative progress (a new fact
/// surfaced, a beat advanced, a thread moved — any non-restate substance).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SceneTurn {
    pub scene_id: String,
    #[serde(default)]
    pub made_progress: bool,
}

/// Observed per-turn scene history for the chapter, in order.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SceneRestateEvidence {
    #[serde(default)]
    pub turns: Vec<SceneTurn>,
}

impl SceneRestateEvidence {
    /// Longest run of consecutive turns in the same scene that ALL made no progress.
    pub fn longest_stall(&self) -> usize {
        let mut best = 0usize;
        let mut cur = 0usize;
        let mut prev_scene: Option<&str> = None;
        for t in &self.turns {
            let same_scene = Some(t.scene_id.as_str()) == prev_scene;
            if same_scene && !t.made_progress {
                cur += 1;
            } else if !t.made_progress {
                cur = 1; // a fresh scene that already stalls counts as a run of 1
            } else {
                cur = 0; // progress breaks the stall
            }
            prev_scene = Some(t.scene_id.as_str());
            best = best.max(cur);
        }
        best
    }
}

/// Terminal classification. Fail-closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SceneRestateCheckpointState {
    InvalidSetup,
    NotTriggered,
    /// The same scene was restated past `max_stall` turns with no progress.
    SceneRestateLoop,
    Pass,
}

impl SceneRestateCheckpointState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidSetup => "INVALID_SETUP",
            Self::NotTriggered => "NOT_TRIGGERED",
            Self::SceneRestateLoop => "SCENE_RESTATE_LOOP",
            Self::Pass => "PASS",
        }
    }
    pub fn is_failing(self) -> bool {
        matches!(self, Self::InvalidSetup | Self::SceneRestateLoop)
    }
}

/// Classify checkpoint #6. Pure, fail-closed.
pub fn classify_scene_restate_checkpoint(
    spec: &SceneRestateCheckpoint,
    ev: &SceneRestateEvidence,
) -> SceneRestateCheckpointState {
    if !spec.is_active() || spec.max_stall == 0 {
        return SceneRestateCheckpointState::InvalidSetup;
    }
    if ev.turns.len() <= spec.max_stall {
        return SceneRestateCheckpointState::NotTriggered;
    }
    if ev.longest_stall() > spec.max_stall {
        SceneRestateCheckpointState::SceneRestateLoop
    } else {
        SceneRestateCheckpointState::Pass
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(scene: &str, progress: bool) -> SceneTurn {
        SceneTurn {
            scene_id: scene.into(),
            made_progress: progress,
        }
    }

    #[test]
    fn passing_fixture_progress_breaks_stall() {
        let ev = SceneRestateEvidence {
            turns: vec![
                turn("s1", false),
                turn("s1", true), // progress resets the stall
                turn("s1", false),
                turn("s2", false),
            ],
        };
        assert_eq!(ev.longest_stall(), 1);
        assert_eq!(
            classify_scene_restate_checkpoint(&SceneRestateCheckpoint::active(), &ev),
            SceneRestateCheckpointState::Pass
        );
    }

    #[test]
    fn failing_fixture_three_no_progress_same_scene() {
        let ev = SceneRestateEvidence {
            turns: vec![
                turn("s1", false),
                turn("s1", false),
                turn("s1", false),
                turn("s2", true),
            ],
        };
        let state = classify_scene_restate_checkpoint(&SceneRestateCheckpoint::active(), &ev);
        assert_eq!(state, SceneRestateCheckpointState::SceneRestateLoop);
        assert!(state.is_failing());
        assert_eq!(ev.longest_stall(), 3);
    }

    #[test]
    fn scene_change_resets_stall_counter() {
        // alternating scenes, each no-progress, never stalls in ONE scene > max.
        let ev = SceneRestateEvidence {
            turns: vec![
                turn("s1", false),
                turn("s2", false),
                turn("s1", false),
                turn("s2", false),
            ],
        };
        assert_eq!(ev.longest_stall(), 1);
        assert_eq!(
            classify_scene_restate_checkpoint(&SceneRestateCheckpoint::active(), &ev),
            SceneRestateCheckpointState::Pass
        );
    }

    #[test]
    fn too_short_is_not_triggered() {
        assert_eq!(
            classify_scene_restate_checkpoint(
                &SceneRestateCheckpoint::active(),
                &SceneRestateEvidence {
                    turns: vec![turn("s1", false), turn("s1", false)]
                }
            ),
            SceneRestateCheckpointState::NotTriggered
        );
    }

    #[test]
    fn inactive_is_invalid_setup() {
        assert_eq!(
            classify_scene_restate_checkpoint(
                &SceneRestateCheckpoint::default(),
                &SceneRestateEvidence::default()
            ),
            SceneRestateCheckpointState::InvalidSetup
        );
    }

    #[test]
    fn serde_round_trip() {
        let ev = SceneRestateEvidence {
            turns: vec![turn("s1", false), turn("s1", true)],
        };
        let back: SceneRestateEvidence =
            serde_json::from_str(&serde_json::to_string(&ev).unwrap()).unwrap();
        assert_eq!(ev, back);
    }
}

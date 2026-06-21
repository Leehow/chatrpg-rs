//! Checkpoint #3 (L2.3) — **premature-reveal fail-closed**.
//!
//! The Director's reveal gate (`trpg-director::story::select::compute_reveal`, `select.rs:199`):
//! `reveal_candidate_fact_ids = gm_truth ∖ player_known`, BUT if EITHER input is `None` (a DB read
//! failure) the reveal collapses to EMPTY — a read failure must never over-reveal. Only ids in
//! `gm_truth` and not already in `player_known` may ever surface.
//!
//! This checkpoint is an INDEPENDENT oracle: it recomputes the permitted set and asserts the
//! emitted reveal never strays outside it. It tolerates legitimate downstream NARROWING (the
//! reactive `forbidden_reveals` regen ladder at `turn_loop.rs:2368-2455` can shrink the reveal),
//! so a revealed set that is a SUBSET of the permitted difference still passes — only an
//! OVER-reveal (an id outside `gm_truth ∖ player_known`, incl. any reveal when an input is `None`)
//! is a failure. Fail-closed.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Checkpoint #3 spec: assert no premature / out-of-bounds reveal.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RevealCheckpoint {
    /// When false the checkpoint is inert (no reveal-gating assertion configured for the turn).
    #[serde(default)]
    pub require_fail_closed_reveal: bool,
}

impl RevealCheckpoint {
    pub fn is_active(&self) -> bool {
        self.require_fail_closed_reveal
    }
}

/// Observed reveal evidence for one turn. `player_known` / `gm_truth` are `Option` so a `None`
/// signals a DB read failure (which must fail-close the reveal to empty) — exactly as the
/// production gate's inputs are typed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RevealEvidence {
    /// Facts the player already knows (`None` ⇒ read failure ⇒ permitted reveal is empty).
    #[serde(default)]
    pub player_known: Option<Vec<String>>,
    /// Facts the GM holds as true (`None` ⇒ read failure ⇒ permitted reveal is empty).
    #[serde(default)]
    pub gm_truth: Option<Vec<String>>,
    /// The reveal the Director actually emitted (`DirectorPlan.reveal_candidate_fact_ids`).
    #[serde(default)]
    pub revealed_fact_ids: Vec<String>,
}

impl RevealEvidence {
    /// The permitted reveal set `gm_truth ∖ player_known`, fail-closed: either input `None`
    /// ⇒ empty (mirrors `compute_reveal`, independently recomputed here).
    pub fn permitted_reveal(&self) -> Vec<String> {
        let (Some(known), Some(truth)) = (self.player_known.as_ref(), self.gm_truth.as_ref()) else {
            return Vec::new();
        };
        let known_set: HashSet<&str> = known.iter().map(String::as_str).collect();
        let mut out: Vec<String> = truth
            .iter()
            .filter(|id| !known_set.contains(id.as_str()))
            .cloned()
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Revealed ids that fall OUTSIDE the permitted set — the over-reveal violation (empty ⇒ ok).
    pub fn over_revealed_ids(&self) -> Vec<String> {
        let permitted_owned = self.permitted_reveal();
        let permitted_set: HashSet<&str> = permitted_owned.iter().map(String::as_str).collect();
        self.revealed_fact_ids
            .iter()
            .filter(|id| !permitted_set.contains(id.as_str()))
            .cloned()
            .collect()
    }
}

/// Terminal classification of the reveal-gating checkpoint. Fail-closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RevealCheckpointState {
    /// No active assertion configured.
    InvalidSetup,
    /// No fact was legitimately revealable (permitted set empty — incl. a `None`-input read
    /// failure) AND nothing was revealed ⇒ the fail-closed no-op was honored (not a failure).
    NotTriggered,
    /// A revealed fact fell outside `gm_truth ∖ player_known` (incl. any reveal under a `None`
    /// input) — the premature/over-reveal the gate forbids.
    PrematureReveal,
    /// A legitimate, in-bounds reveal (subset of the permitted difference).
    Pass,
}

impl RevealCheckpointState {
    pub fn as_str(self) -> &'static str {
        match self {
            RevealCheckpointState::InvalidSetup => "INVALID_SETUP",
            RevealCheckpointState::NotTriggered => "NOT_TRIGGERED",
            RevealCheckpointState::PrematureReveal => "PREMATURE_REVEAL",
            RevealCheckpointState::Pass => "PASS",
        }
    }

    pub fn is_failing(self) -> bool {
        matches!(
            self,
            RevealCheckpointState::InvalidSetup | RevealCheckpointState::PrematureReveal
        )
    }

    /// Earlier (more fundamental) failures sort first. Lower is earlier.
    pub fn chain_rank(self) -> u8 {
        match self {
            RevealCheckpointState::InvalidSetup => 0,
            RevealCheckpointState::PrematureReveal => 1,
            RevealCheckpointState::NotTriggered => 2,
            RevealCheckpointState::Pass => 3,
        }
    }
}

/// Classify checkpoint #3 against observed evidence. Pure, fail-closed, root-cause ordered.
pub fn classify_reveal_checkpoint(
    spec: &RevealCheckpoint,
    ev: &RevealEvidence,
) -> RevealCheckpointState {
    if !spec.is_active() {
        return RevealCheckpointState::InvalidSetup;
    }
    if !ev.over_revealed_ids().is_empty() {
        // any revealed id outside gm_truth∖player_known (incl. a reveal under a None input).
        return RevealCheckpointState::PrematureReveal;
    }
    if ev.revealed_fact_ids.is_empty() {
        // nothing revealed; the permitted set may be empty (None input or no novel truth) or the
        // reveal was simply withheld — either way no premature reveal occurred (fail-closed no-op).
        RevealCheckpointState::NotTriggered
    } else {
        RevealCheckpointState::Pass
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active() -> RevealCheckpoint {
        RevealCheckpoint {
            require_fail_closed_reveal: true,
        }
    }

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    // ── PASSING fixtures: in-bounds reveal ──────────────────────────────────────────────────
    #[test]
    fn passing_fixture_reveals_only_gm_truth_not_player_known() {
        // gm_truth = {f1, f2, f3}, player_known = {f1} ⇒ permitted {f2, f3}; reveal both.
        let ev = RevealEvidence {
            player_known: Some(v(&["f1"])),
            gm_truth: Some(v(&["f1", "f2", "f3"])),
            revealed_fact_ids: v(&["f2", "f3"]),
        };
        assert_eq!(
            classify_reveal_checkpoint(&active(), &ev),
            RevealCheckpointState::Pass
        );
    }

    #[test]
    fn passing_fixture_narrowed_reveal_is_a_subset_and_passes() {
        // downstream forbidden_reveals narrowing dropped f3 ⇒ subset of permitted still passes.
        let ev = RevealEvidence {
            player_known: Some(v(&["f1"])),
            gm_truth: Some(v(&["f1", "f2", "f3"])),
            revealed_fact_ids: v(&["f2"]),
        };
        assert_eq!(
            classify_reveal_checkpoint(&active(), &ev),
            RevealCheckpointState::Pass
        );
    }

    // ── FAILING fixtures: over-reveal ───────────────────────────────────────────────────────
    #[test]
    fn failing_fixture_reveals_a_player_known_fact() {
        // f1 is already player-known ⇒ revealing it is an over-reveal (∉ gm_truth∖player_known).
        let ev = RevealEvidence {
            player_known: Some(v(&["f1"])),
            gm_truth: Some(v(&["f1", "f2"])),
            revealed_fact_ids: v(&["f1", "f2"]),
        };
        let state = classify_reveal_checkpoint(&active(), &ev);
        assert_eq!(state, RevealCheckpointState::PrematureReveal);
        assert!(state.is_failing());
        assert_eq!(ev.over_revealed_ids(), v(&["f1"]));
    }

    #[test]
    fn failing_fixture_reveals_a_fact_not_in_gm_truth() {
        // f9 is not in gm_truth at all ⇒ inventing a reveal (fail-closed forbids it).
        let ev = RevealEvidence {
            player_known: Some(v(&[])),
            gm_truth: Some(v(&["f1"])),
            revealed_fact_ids: v(&["f9"]),
        };
        assert_eq!(
            classify_reveal_checkpoint(&active(), &ev),
            RevealCheckpointState::PrematureReveal
        );
    }

    #[test]
    fn failing_fixture_none_input_with_a_reveal_is_premature() {
        // the acceptance's None-input over-reveal: a read failure (gm_truth None) MUST yield empty;
        // any reveal here is premature.
        let ev = RevealEvidence {
            player_known: Some(v(&["f1"])),
            gm_truth: None,
            revealed_fact_ids: v(&["f2"]),
        };
        assert_eq!(
            classify_reveal_checkpoint(&active(), &ev),
            RevealCheckpointState::PrematureReveal
        );
    }

    // ── fail-closed no-op: None input producing EMPTY is honored (not a failure) ──────────────
    #[test]
    fn none_input_producing_empty_reveal_is_not_a_failure() {
        let ev = RevealEvidence {
            player_known: None,
            gm_truth: Some(v(&["f1", "f2"])),
            revealed_fact_ids: vec![],
        };
        let state = classify_reveal_checkpoint(&active(), &ev);
        assert_eq!(state, RevealCheckpointState::NotTriggered);
        assert!(!state.is_failing());
        assert!(ev.permitted_reveal().is_empty());
    }

    #[test]
    fn both_none_empty_reveal_is_not_triggered() {
        let ev = RevealEvidence {
            player_known: None,
            gm_truth: None,
            revealed_fact_ids: vec![],
        };
        assert_eq!(
            classify_reveal_checkpoint(&active(), &ev),
            RevealCheckpointState::NotTriggered
        );
    }

    #[test]
    fn no_novel_truth_empty_reveal_is_not_triggered() {
        // gm_truth ⊆ player_known ⇒ permitted empty; nothing revealed ⇒ vacuous no-op.
        let ev = RevealEvidence {
            player_known: Some(v(&["f1", "f2"])),
            gm_truth: Some(v(&["f1"])),
            revealed_fact_ids: vec![],
        };
        assert_eq!(
            classify_reveal_checkpoint(&active(), &ev),
            RevealCheckpointState::NotTriggered
        );
    }

    // ── setup guard + serde ─────────────────────────────────────────────────────────────────
    #[test]
    fn inactive_spec_is_invalid_setup() {
        let ev = RevealEvidence::default();
        assert_eq!(
            classify_reveal_checkpoint(&RevealCheckpoint::default(), &ev),
            RevealCheckpointState::InvalidSetup
        );
    }

    #[test]
    fn serde_round_trip_evidence() {
        let ev = RevealEvidence {
            player_known: Some(v(&["f1"])),
            gm_truth: Some(v(&["f1", "f2"])),
            revealed_fact_ids: v(&["f2"]),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: RevealEvidence = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }
}

//! L6.2 — **Narrator authors no mechanics** (the split-Narrator structural guarantee).
//!
//! When `TRPG_NARRATOR_SPLIT` is ON the GM loses prose authorship: a tool-less Narrator
//! (`run_narrator_phase`, `turn_loop.rs`) writes the player-visible fiction with an iron-rule
//! prompt ("绝不发明未列出的检定/伤害/资源/状态") and a post-stream `NarrationVerifier`
//! (`trpg-agent::gm_loop`) folds violations into the `core.no_mechanical_invention` trace. The GM
//! adjudicator (台下) owns ALL mechanics; the Narrator only describes committed facts.
//!
//! This checkpoint is an INDEPENDENT oracle: it scans the Narrator's player-visible prose for
//! mechanical AUTHORING (a dice roll / check / DC, or a numbered effect such as damage / HP /
//! resource / condition) and cross-checks it against what the turn ledger actually committed. A
//! mechanical mention with NO committed backing is invention — the split's core violation. Pure
//! prose with no mechanical mention is the ideal (NotTriggered). It re-derives the invariant
//! independently (its own scanner), so it is never tautological with the production verifier.
//!
//! Generic / no ruleset name-branch (§二-⑪): the scanner keys on dice notation + generic
//! mechanical vocabulary, never on a specific module or ruleset id.

use serde::{Deserialize, Serialize};

/// L6.2 spec: assert the split Narrator authored no unbacked mechanics this turn.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct NarratorMechanicsCheckpoint {
    /// When false the checkpoint is inert (narrator-split not engaged / nothing to assert).
    #[serde(default)]
    pub require_no_invention: bool,
}

impl NarratorMechanicsCheckpoint {
    pub fn is_active(&self) -> bool {
        self.require_no_invention
    }
}

/// Observed evidence for one split-Narrator turn: the player-visible prose plus what the ledger
/// actually committed (a resolution = a check/roll fact; an effect = damage/resource/state patch).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct NarratorMechanicsEvidence {
    /// The Narrator's player-visible output (the single transported source under split).
    #[serde(default)]
    pub narrator_text: String,
    /// Did the turn ledger commit a check/roll fact this turn?
    #[serde(default)]
    pub committed_check: bool,
    /// Did the turn ledger commit an effect (damage / resource / condition / state patch)?
    #[serde(default)]
    pub committed_effect: bool,
}

/// Dice-notation / resolution tokens — UNAMBIGUOUS authoring of a check or roll in prose.
const RESOLUTION_TOKENS: &[&str] = &[
    "d4",
    "d6",
    "d8",
    "d10",
    "d12",
    "d20",
    "d100",
    "1d",
    "2d",
    "3d",
    "roll a ",
    "rolled ",
    "you roll",
    "dice",
    "dc ",
    "saving throw",
    "skill check",
    "投骰",
    "掷骰",
    "骰子",
    "检定值",
    "难度等级",
];

/// Numbered mechanical-effect tokens — authoring of a damage / resource / state change in prose.
const EFFECT_TOKENS: &[&str] = &[
    "damage",
    "hit points",
    " hp",
    "takes 1",
    "takes 2",
    "takes 3",
    "lose ",
    "loses ",
    "heals ",
    "healed ",
    "点伤害",
    "生命值",
    "理智值",
    "san 值",
];

fn text_contains_any(haystack: &str, needles: &[&str]) -> bool {
    let lower = haystack.to_lowercase();
    needles.iter().any(|n| lower.contains(&n.to_lowercase()))
}

impl NarratorMechanicsEvidence {
    /// Does the prose author a resolution (a check / roll / DC)?
    pub fn mentions_resolution(&self) -> bool {
        text_contains_any(&self.narrator_text, RESOLUTION_TOKENS)
    }

    /// Does the prose author a numbered mechanical effect (damage / resource / state)?
    pub fn mentions_effect(&self) -> bool {
        text_contains_any(&self.narrator_text, EFFECT_TOKENS)
    }

    /// Any mechanical mention at all (a resolution OR an effect).
    pub fn mentions_any_mechanic(&self) -> bool {
        self.mentions_resolution() || self.mentions_effect()
    }

    /// The mechanical kinds the prose authored WITHOUT a committed ledger backing (invention).
    pub fn invented_kinds(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.mentions_resolution() && !self.committed_check {
            out.push("resolution");
        }
        if self.mentions_effect() && !self.committed_effect {
            out.push("effect");
        }
        out
    }
}

/// Terminal classification of the no-mechanical-invention checkpoint. Fail-closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NarratorMechanicsCheckpointState {
    /// No active assertion configured (narrator-split not engaged for the turn).
    InvalidSetup,
    /// Pure description — the Narrator authored no mechanics at all (the ideal split output).
    NotTriggered,
    /// The Narrator authored a mechanic with no committed ledger backing — invention.
    MechanicalInvention,
    /// The Narrator referenced a mechanic, and every such mention is backed by the ledger.
    Pass,
}

impl NarratorMechanicsCheckpointState {
    pub fn as_str(self) -> &'static str {
        match self {
            NarratorMechanicsCheckpointState::InvalidSetup => "INVALID_SETUP",
            NarratorMechanicsCheckpointState::NotTriggered => "NOT_TRIGGERED",
            NarratorMechanicsCheckpointState::MechanicalInvention => "MECHANICAL_INVENTION",
            NarratorMechanicsCheckpointState::Pass => "PASS",
        }
    }

    pub fn is_failing(self) -> bool {
        matches!(
            self,
            NarratorMechanicsCheckpointState::InvalidSetup
                | NarratorMechanicsCheckpointState::MechanicalInvention
        )
    }

    /// Earlier (more fundamental) failures sort first. Lower is earlier.
    pub fn chain_rank(self) -> u8 {
        match self {
            NarratorMechanicsCheckpointState::InvalidSetup => 0,
            NarratorMechanicsCheckpointState::MechanicalInvention => 1,
            NarratorMechanicsCheckpointState::NotTriggered => 2,
            NarratorMechanicsCheckpointState::Pass => 3,
        }
    }
}

/// Classify L6.2 against observed evidence. Pure, fail-closed, root-cause ordered.
pub fn classify_narrator_mechanics_checkpoint(
    spec: &NarratorMechanicsCheckpoint,
    ev: &NarratorMechanicsEvidence,
) -> NarratorMechanicsCheckpointState {
    if !spec.is_active() {
        return NarratorMechanicsCheckpointState::InvalidSetup;
    }
    if !ev.invented_kinds().is_empty() {
        return NarratorMechanicsCheckpointState::MechanicalInvention;
    }
    if !ev.mentions_any_mechanic() {
        // pure prose: the Narrator described only — the ideal split output, no invention possible.
        NarratorMechanicsCheckpointState::NotTriggered
    } else {
        // mechanics mentioned, all backed by committed ledger facts.
        NarratorMechanicsCheckpointState::Pass
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active() -> NarratorMechanicsCheckpoint {
        NarratorMechanicsCheckpoint {
            require_no_invention: true,
        }
    }

    // ── PASSING fixtures ────────────────────────────────────────────────────────────────────
    #[test]
    fn passing_fixture_pure_prose_is_not_triggered() {
        // The ideal split Narrator output: sensory description, zero mechanics.
        let ev = NarratorMechanicsEvidence {
            narrator_text: "你推开吱呀作响的木门，霉味扑面而来，烛光在墙上投下摇曳的影子。"
                .to_string(),
            committed_check: false,
            committed_effect: false,
        };
        assert_eq!(
            classify_narrator_mechanics_checkpoint(&active(), &ev),
            NarratorMechanicsCheckpointState::NotTriggered
        );
    }

    #[test]
    fn passing_fixture_resolution_backed_by_committed_check() {
        // Narrator references a roll AND the ledger committed a check ⇒ backed, not invented.
        let ev = NarratorMechanicsEvidence {
            narrator_text: "You roll the dice and the latch gives way under your steady hands."
                .to_string(),
            committed_check: true,
            committed_effect: false,
        };
        assert_eq!(
            classify_narrator_mechanics_checkpoint(&active(), &ev),
            NarratorMechanicsCheckpointState::Pass
        );
    }

    #[test]
    fn passing_fixture_effect_backed_by_committed_effect() {
        let ev = NarratorMechanicsEvidence {
            narrator_text: "利爪撕裂你的手臂，你失去 4 点生命值，鲜血浸透了袖口。".to_string(),
            committed_check: false,
            committed_effect: true,
        };
        assert_eq!(
            classify_narrator_mechanics_checkpoint(&active(), &ev),
            NarratorMechanicsCheckpointState::Pass
        );
    }

    // ── FAILING fixtures: invention ─────────────────────────────────────────────────────────
    #[test]
    fn failing_fixture_invents_a_roll_with_no_committed_check() {
        // The Narrator authored a dice roll the GM never committed ⇒ invention.
        let ev = NarratorMechanicsEvidence {
            narrator_text: "You roll a d20 and score a critical success on the lock.".to_string(),
            committed_check: false,
            committed_effect: false,
        };
        let state = classify_narrator_mechanics_checkpoint(&active(), &ev);
        assert_eq!(state, NarratorMechanicsCheckpointState::MechanicalInvention);
        assert!(state.is_failing());
        assert_eq!(ev.invented_kinds(), vec!["resolution"]);
    }

    #[test]
    fn failing_fixture_invents_an_effect_with_no_committed_effect() {
        let ev = NarratorMechanicsEvidence {
            narrator_text: "The blow lands and you take 6 damage to your hit points.".to_string(),
            committed_check: false,
            committed_effect: false,
        };
        let state = classify_narrator_mechanics_checkpoint(&active(), &ev);
        assert_eq!(state, NarratorMechanicsCheckpointState::MechanicalInvention);
        assert!(state.is_failing());
        assert_eq!(ev.invented_kinds(), vec!["effect"]);
    }

    #[test]
    fn failing_fixture_invents_both_kinds() {
        let ev = NarratorMechanicsEvidence {
            narrator_text: "You roll a d100 check; the trap deals 3 damage to your hp.".to_string(),
            committed_check: false,
            committed_effect: false,
        };
        let state = classify_narrator_mechanics_checkpoint(&active(), &ev);
        assert_eq!(state, NarratorMechanicsCheckpointState::MechanicalInvention);
        assert_eq!(ev.invented_kinds(), vec!["resolution", "effect"]);
    }

    // ── setup guard + serde ─────────────────────────────────────────────────────────────────
    #[test]
    fn inactive_spec_is_invalid_setup() {
        let ev = NarratorMechanicsEvidence::default();
        assert_eq!(
            classify_narrator_mechanics_checkpoint(&NarratorMechanicsCheckpoint::default(), &ev),
            NarratorMechanicsCheckpointState::InvalidSetup
        );
    }

    #[test]
    fn serde_round_trip_evidence() {
        let ev = NarratorMechanicsEvidence {
            narrator_text: "You roll a d20.".to_string(),
            committed_check: true,
            committed_effect: false,
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: NarratorMechanicsEvidence = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }
}

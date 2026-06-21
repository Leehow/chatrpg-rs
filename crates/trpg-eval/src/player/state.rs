//! Simulated player state + decision trajectory (蓝图 §二.1 / §二.3).
//!
//! PC state (HP, position, ammo) lives in the runtime; THIS is the *human*
//! layer — goals, plan continuity, beliefs, and the affective state
//! (confusion / frustration / boredom / trust) that lets the player refuse to
//! mechanically continue (§二.4). Kept separate from the PC prompt by design.

use super::persona::PlayerPersona;
use crate::contract::IntentField;
use serde::Serialize;

/// The catalogue of moves a player can take. Deterministically scored by
/// `deliberate`; the GM-facing text is generated from the chosen kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum ActionKind {
    /// Gather concrete evidence about the current scene.
    Investigate,
    /// Ask the GM to clarify what is possible / what was just said.
    AskClarify,
    /// Press forward with the plan (the "mechanical continue" move).
    Advance,
    /// Pull back from a threatening situation.
    Retreat,
    /// Engage / attack / force a confrontation.
    Confront,
    /// Weigh odds, positioning, and resources before acting.
    AssessRisk,
    /// Try a different approach to the same obstacle.
    UseAlternative,
    /// Drop an unproductive line entirely.
    AbandonPath,
    /// Point out a contradiction / re-ask an ignored question with reason.
    ChallengeContradiction,
}

impl ActionKind {
    pub fn id(&self) -> &'static str {
        match self {
            ActionKind::Investigate => "INVESTIGATE",
            ActionKind::AskClarify => "ASK_CLARIFY",
            ActionKind::Advance => "ADVANCE",
            ActionKind::Retreat => "RETREAT",
            ActionKind::Confront => "CONFRONT",
            ActionKind::AssessRisk => "ASSESS_RISK",
            ActionKind::UseAlternative => "USE_ALTERNATIVE",
            ActionKind::AbandonPath => "ABANDON_PATH",
            ActionKind::ChallengeContradiction => "CHALLENGE_CONTRADICTION",
        }
    }

    pub const ALL: [ActionKind; 9] = [
        ActionKind::Investigate,
        ActionKind::AskClarify,
        ActionKind::Advance,
        ActionKind::Retreat,
        ActionKind::Confront,
        ActionKind::AssessRisk,
        ActionKind::UseAlternative,
        ActionKind::AbandonPath,
        ActionKind::ChallengeContradiction,
    ];

    /// Coarse intent family — used by the §六 invariance test: a reworded but
    /// equivalent GM reply must keep the player in the same family.
    pub fn category(&self) -> &'static str {
        match self {
            ActionKind::Investigate | ActionKind::UseAlternative => "INVESTIGATE",
            ActionKind::AskClarify | ActionKind::ChallengeContradiction => "CLARIFY",
            ActionKind::Advance => "ADVANCE",
            ActionKind::Confront => "AGGRESSIVE",
            ActionKind::Retreat | ActionKind::AbandonPath => "WITHDRAW",
            ActionKind::AssessRisk => "ASSESS",
        }
    }

    /// Information fields this action expects the GM to address (§二.3
    /// expected_gm_resolution, links to the §四 Response Contract).
    pub fn expected_contract(&self) -> Vec<IntentField> {
        match self {
            ActionKind::Investigate => vec![IntentField::Perceive, IntentField::Assess],
            ActionKind::AskClarify => vec![IntentField::Inquire],
            ActionKind::Advance => vec![IntentField::Perceive],
            ActionKind::Retreat => vec![IntentField::Perceive],
            ActionKind::Confront => vec![IntentField::Extract, IntentField::Identity],
            ActionKind::AssessRisk => vec![IntentField::Count, IntentField::Assess],
            ActionKind::UseAlternative => vec![IntentField::Perceive, IntentField::Inquire],
            ActionKind::AbandonPath => vec![IntentField::Inquire],
            ActionKind::ChallengeContradiction => vec![IntentField::Inquire, IntentField::Reason],
        }
    }
}

/// One scored candidate (蓝图 §二.2 candidate_actions).
#[derive(Debug, Clone, Serialize)]
pub struct ActionCandidate {
    pub kind: ActionKind,
    pub score: f32,
    pub rationale: String,
}

/// The structured trajectory of a single decision (蓝图 §二.3). Only
/// `selected` is sent to the GM; the rest is evaluation evidence.
#[derive(Debug, Clone, Serialize)]
pub struct PlayerDecision {
    pub perceived_facts: Vec<String>,
    pub missed: Vec<String>,
    pub active_goal: String,
    pub candidates: Vec<ActionCandidate>,
    pub selected: ActionKind,
    pub evidence: Vec<String>,
    pub expected_contract: Vec<IntentField>,
    pub repeat_justification: Option<String>,
    pub confidence: f32,
    /// True when the GM left the player's question unanswered this turn — feeds
    /// the affective update in [`SimulatedPlayerState::observe`] (§二.4).
    pub gm_unresponsive: bool,
    /// Perceived risk after folding in remembered dangers (§六 sensitivity/记忆
    /// observable: rises when a known threat is on screen).
    pub alert_level: f32,
    /// A remembered danger entity was recognised this turn (§六 记忆测试).
    pub belief_alert: bool,
}

#[derive(Debug, Clone)]
pub struct SimulatedPlayerState {
    pub persona: PlayerPersona,
    pub goal: String,
    pub beliefs: Vec<String>,
    pub unresolved_questions: Vec<String>,
    pub recent_actions: Vec<ActionKind>,
    pub confusion: f32,
    pub frustration: f32,
    pub boredom: f32,
    pub trust: f32,
}

impl SimulatedPlayerState {
    pub fn new(persona: PlayerPersona, goal: impl Into<String>) -> Self {
        SimulatedPlayerState {
            persona,
            goal: goal.into(),
            beliefs: Vec::new(),
            unresolved_questions: Vec::new(),
            recent_actions: Vec::new(),
            confusion: 0.0,
            frustration: 0.0,
            boredom: 0.0,
            trust: 0.6,
        }
    }

    /// Fold a decision back into the human layer (蓝图 §二.2 情绪/信念更新).
    /// Unanswered turns accumulate frustration; informative turns relieve it
    /// and build trust. Records the action so loops can be detected.
    pub fn observe(&mut self, d: &PlayerDecision) {
        if d.gm_unresponsive {
            self.frustration = (self.frustration + 0.25).min(1.0);
            self.confusion = (self.confusion + 0.15).min(1.0);
            self.trust = (self.trust - 0.1).max(0.0);
            self.boredom = (self.boredom + 0.1).min(1.0);
        } else if !d.perceived_facts.is_empty() {
            self.frustration = (self.frustration - 0.1).max(0.0);
            self.trust = (self.trust + 0.05).min(1.0);
            for f in &d.perceived_facts {
                if !self.beliefs.iter().any(|b| b == f) {
                    self.beliefs.push(f.clone());
                }
            }
        }
        self.recent_actions.push(d.selected);
    }
}

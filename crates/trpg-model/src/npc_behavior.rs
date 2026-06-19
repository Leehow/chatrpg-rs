//! NPC Behavior Plan v1 (TC-NPC-04): a structured, deterministic action/speech
//! proposal derived from a prompt-safe [`NpcMindView`] before GM narration renders prose.
//!
//! Design contract:
//! - **Proposal, not state.** [`NpcBehaviorPlan::derive`] is a pure function of its
//!   inputs. It writes no DB rows, appends no events, mutates no relationship, and
//!   commits nothing. The GM may consume it; it is guidance, never authority.
//! - **No omniscience.** The plan only ever sees what the mind view holds. A fact the
//!   NPC does not know as TRUE can never enter [`NpcBehaviorPlan::facts_can_reveal`].
//!   Beliefs (possibly false) inform tone but are never promoted to revealable truth.
//! - **Deterministic + bounded.** Willingness/risk are integer scores clamped to
//!   `[0, 100]`, derived from the focus relationship's trust/fear/hostility via fixed
//!   formulas. Same inputs → same plan.
//! - **Conservative disclosure.** A known fact the caller's [`NpcBehaviorContext`] marks
//!   secret is withheld unless reveal willingness clears [`REVEAL_SECRET_THRESHOLD`]
//!   (the relationship/fear "explicitly allows reveal" path). Non-secret known facts are
//!   freely revealable.
//!
//! The planner is told the NPC's secrets only as opaque fact ids via the context — it is
//! never handed GM world truth.
use crate::npc_mind::NpcMindView;
use crate::npc_relationship::RelationshipStance;
use crate::world_reaction::WorldReactionCandidate;
use serde::{Deserialize, Serialize};

/// Reveal willingness (0..100) a known secret must reach before it moves from
/// `facts_will_withhold` to `facts_can_reveal`. v1 is conservative: only a strong
/// relationship or coercion-driven compliance clears it.
pub const REVEAL_SECRET_THRESHOLD: i16 = 65;

#[inline]
fn clamp_pct(v: i64) -> i16 {
    v.clamp(0, 100) as i16
}

/// Coarse emotional read used to color narration. Derived from the focus relationship's
/// dominant channel; never a state mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NpcEmotionalState {
    Hostile,
    Afraid,
    Guarded,
    Neutral,
    Warm,
}

/// Non-truth inputs the GM supplies to orient a plan. Carries NO world truth: secrets
/// are opaque fact ids, the goal is a free-form hint, and `source_event_ids` is plain
/// provenance. Defaults give a player-party-focused, no-secret, calm-default context.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NpcBehaviorContext {
    /// Relationship target this plan is oriented toward. `None` ⇒ the player party.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus_target: Option<FocusTarget>,
    /// Known fact ids the NPC treats as secret. Only ids the NPC actually knows as true
    /// are gated; unknown ids are ignored (you cannot withhold what you do not know).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_fact_ids: Vec<String>,
    /// Optional explicit current goal, overriding the persona's first goal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_goal: Option<String>,
    /// When true, fear makes COMPLIANCE plausible (coercion/intimidation): fear then
    /// RAISES reveal willingness instead of lowering it. Default false ⇒ fear clams up.
    #[serde(default)]
    pub fear_drives_compliance: bool,
    /// Provenance event ids to attach to the plan (e.g. the driving turn/evidence ids).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_event_ids: Vec<String>,
}

/// Which relationship the plan focuses on. Mirrors the compact summary's
/// `(target_kind, target_id)` so callers need not reconstruct a full target enum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusTarget {
    pub target_kind: String,
    pub target_id: String,
}

/// A structured, deterministic action/speech proposal for one NPC. Every field is
/// derived; nothing here is committed state. Willingness/risk are `0..100` scores.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NpcBehaviorPlan {
    pub npc_id: String,
    /// Summary stance toward the focus target (from the relationship; Neutral if none).
    pub stance: RelationshipStance,
    /// How much the NPC wants to engage the focus target (-100 avoid .. 100 seek).
    pub interaction_desire: i16,

    // Bounded willingness/risk scores in [0, 100].
    pub willingness_to_help: i16,
    pub willingness_to_lie: i16,
    pub willingness_to_fight: i16,
    pub willingness_to_reveal_secret: i16,
    pub risk_tolerance: i16,

    pub current_goal: Option<String>,
    pub emotional_state: NpcEmotionalState,

    /// Fact ids the NPC knows as TRUE (never beliefs).
    pub facts_knows: Vec<String>,
    /// Subset of `facts_knows` the NPC may disclose now (non-secret, or a secret unlocked
    /// by reveal willingness). Always ⊆ `facts_knows`.
    pub facts_can_reveal: Vec<String>,
    /// Known facts the NPC is holding back this turn (secrets below the reveal threshold).
    pub facts_will_withhold: Vec<String>,

    pub preferred_actions: Vec<String>,
    pub forbidden_actions: Vec<String>,
    /// Persona speech-style prompt block (carries no secret content).
    pub speech_style_prompt: String,
    /// One-line narration guidance summarizing posture.
    pub dialogue_guidance: String,
    /// Provenance event ids passed through from the context.
    pub source_event_ids: Vec<String>,
}

impl NpcBehaviorPlan {
    /// Derive a plan from a projected mind view and non-truth context. Pure: no IO, no
    /// mutation of `view`/`ctx`, deterministic, and bounded.
    pub fn derive(view: &NpcMindView, ctx: &NpcBehaviorContext) -> Self {
        let rel = focus_relationship(view, ctx);
        let (trust, fear, hostility, interaction_desire, stance) = match rel {
            Some(r) => (
                r.trust as i64,
                r.fear as i64,
                r.hostility as i64,
                r.interaction_desire,
                r.stance,
            ),
            None => (0, 0, 0, 0, RelationshipStance::Neutral),
        };

        let willingness_to_help = clamp_pct(50 + trust / 2 - hostility / 2 - fear / 4);
        let willingness_to_lie = clamp_pct(40 - trust / 2 + hostility / 3);
        let willingness_to_fight = clamp_pct(10 + hostility - trust / 3);
        // Fear lowers reveal by default; under compliance pressure it raises it instead.
        let fear_term = if ctx.fear_drives_compliance {
            fear / 3
        } else {
            -(fear / 3)
        };
        let willingness_to_reveal_secret = clamp_pct(45 + trust / 2 + fear_term - hostility / 3);
        let risk_tolerance = clamp_pct(45 + hostility / 4 + trust / 4 - fear / 2);

        let emotional_state = derive_emotion(trust, fear, hostility);

        // Disclosure: split the NPC's KNOWN-true facts into revealable vs withheld.
        let facts_knows: Vec<String> = view
            .known_fact_ids()
            .into_iter()
            .map(str::to_string)
            .collect();
        let mut facts_can_reveal = Vec::new();
        let mut facts_will_withhold = Vec::new();
        for fact in &facts_knows {
            let is_secret = ctx.secret_fact_ids.iter().any(|s| s == fact);
            if is_secret && willingness_to_reveal_secret < REVEAL_SECRET_THRESHOLD {
                facts_will_withhold.push(fact.clone());
            } else {
                facts_can_reveal.push(fact.clone());
            }
        }

        let (preferred_actions, forbidden_actions) = derive_actions(
            view,
            stance,
            willingness_to_help,
            willingness_to_reveal_secret,
            !facts_will_withhold.is_empty(),
        );

        let current_goal = ctx
            .current_goal
            .clone()
            .or_else(|| view.persona.goals.first().cloned());

        NpcBehaviorPlan {
            npc_id: view.npc_id.clone(),
            stance,
            interaction_desire,
            willingness_to_help,
            willingness_to_lie,
            willingness_to_fight,
            willingness_to_reveal_secret,
            risk_tolerance,
            current_goal,
            emotional_state,
            dialogue_guidance: dialogue_guidance(view, stance, emotional_state),
            speech_style_prompt: view.persona.speech_style.to_prompt_block(),
            facts_knows,
            facts_can_reveal,
            facts_will_withhold,
            preferred_actions,
            forbidden_actions,
            source_event_ids: ctx.source_event_ids.clone(),
        }
    }

    /// Pure projection of this derived plan into a commit-nothing
    /// [`WorldReactionCandidate`] for the World simulation layer (P4.3).
    ///
    /// No async, no IO, no DB, no mutation — same plan in ⇒ same candidate out. Scores
    /// are normalized to `0.0..=1.0`:
    /// - `urgency = interaction_desire / 100`
    /// - `feasibility = min(willingness_to_help, risk_tolerance) / 100`
    /// - `risk = risk_tolerance / 100`
    ///
    /// **Disclosure safety (§二十四-#4):** `knowledge_basis` is sourced ONLY from
    /// [`Self::facts_can_reveal`] — `facts_will_withhold` can NEVER leak into the
    /// candidate, so the World layer never grounds a reaction on a fact the NPC cannot
    /// disclose. This carries no action intent (mechanical follow-through is wired
    /// later, gated, in P4.6); a bare projection proposes posture only.
    pub fn to_reaction_candidate(&self) -> WorldReactionCandidate {
        WorldReactionCandidate {
            npc_id: self.npc_id.clone(),
            stance: enum_word(&self.stance, "neutral"),
            emotional_state: enum_word(&self.emotional_state, "neutral"),
            urgency: f64::from(self.interaction_desire) / 100.0,
            feasibility: f64::from(self.willingness_to_help.min(self.risk_tolerance)) / 100.0,
            risk: f64::from(self.risk_tolerance) / 100.0,
            action_intent: None,
            // ONLY revealable facts — withheld facts must never enter the World basis.
            knowledge_basis: self.facts_can_reveal.clone(),
            source_event_ids: self.source_event_ids.clone(),
        }
    }

    /// Render this plan as a prompt-safe guidance block for GM narration assembly.
    ///
    /// The block is **id-stable** (`[npc_behavior_guidance npc=<id>]…[/npc_behavior_guidance]`)
    /// and carries ONLY safe material: the persona safe-view speech style + dialogue
    /// guidance (no GM-only secret text — those are structurally absent from the mind
    /// view this plan was derived from), derived stance/willingness scores, action
    /// hints, and fact **ids** in the reveal/withhold sets. It never emits fact prose,
    /// so a withheld secret's id may appear as a steering hint without leaking content.
    pub fn to_guidance_block(&self) -> String {
        let stance = enum_word(&self.stance, "neutral");
        let emotion = enum_word(&self.emotional_state, "neutral");
        let mut lines: Vec<String> = Vec::new();
        lines.push(format!("[npc_behavior_guidance npc={}]", self.npc_id));
        lines.push(self.dialogue_guidance.clone());
        lines.push(format!(
            "Stance toward focus: {stance} (interaction desire {}); emotional state {emotion}.",
            self.interaction_desire
        ));
        lines.push(format!(
            "Willingness (0-100): help {}, lie {}, fight {}, reveal-secret {}; risk tolerance {}.",
            self.willingness_to_help,
            self.willingness_to_lie,
            self.willingness_to_fight,
            self.willingness_to_reveal_secret,
            self.risk_tolerance
        ));
        if let Some(goal) = self
            .current_goal
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            lines.push(format!("Current goal: {goal}"));
        }
        if !self.speech_style_prompt.trim().is_empty() {
            lines.push(self.speech_style_prompt.clone());
        }
        push_list(&mut lines, "Preferred actions", &self.preferred_actions);
        push_list(&mut lines, "Forbidden actions", &self.forbidden_actions);
        if !self.facts_can_reveal.is_empty() {
            lines.push(format!(
                "May reveal fact ids: {}",
                self.facts_can_reveal.join(", ")
            ));
        }
        if !self.facts_will_withhold.is_empty() {
            lines.push(format!(
                "Withhold fact ids: {}",
                self.facts_will_withhold.join(", ")
            ));
        }
        lines.push("[/npc_behavior_guidance]".to_string());
        lines.join("\n")
    }
}

#[path = "npc_behavior_helpers.rs"]
mod helpers;
use helpers::*;

#[cfg(test)]
#[path = "npc_behavior_tests.rs"]
mod tests;

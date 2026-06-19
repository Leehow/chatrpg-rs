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
use crate::npc_mind::{NpcMindView, RelationshipSummary};
use crate::npc_relationship::RelationshipStance;
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

/// Serialize a `serde`-tagged enum to its snake_case wire word for prompt text.
fn enum_word<T: Serialize>(value: &T, fallback: &str) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| fallback.to_string())
}

/// Append a labeled bullet list, skipping an empty list (no empty headers).
fn push_list(lines: &mut Vec<String>, label: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    lines.push(format!("{label}:"));
    for item in items {
        lines.push(format!("- {item}"));
    }
}

/// Pick the relationship summary the plan focuses on. With an explicit target, match it;
/// otherwise default to the player party. Returns `None` if no such relationship exists.
fn focus_relationship<'a>(
    view: &'a NpcMindView,
    ctx: &NpcBehaviorContext,
) -> Option<&'a RelationshipSummary> {
    match &ctx.focus_target {
        Some(t) => view
            .relationships
            .iter()
            .find(|r| r.target_kind == t.target_kind && r.target_id == t.target_id),
        None => view
            .relationships
            .iter()
            .find(|r| r.target_kind == "player_party"),
    }
}

/// Dominant-channel emotional read. Severity-ordered so hostility wins over fear, etc.
fn derive_emotion(trust: i64, fear: i64, hostility: i64) -> NpcEmotionalState {
    if hostility >= 50 {
        NpcEmotionalState::Hostile
    } else if fear >= 50 {
        NpcEmotionalState::Afraid
    } else if trust <= -20 || hostility >= 25 {
        NpcEmotionalState::Guarded
    } else if trust >= 40 {
        NpcEmotionalState::Warm
    } else {
        NpcEmotionalState::Neutral
    }
}

/// Deterministic preferred/forbidden action hints. Stance sets the baseline posture;
/// low help willingness and any withheld secret add explicit guardrails. Persona
/// behavioral boundaries and taboo topics are always folded into the forbidden list.
fn derive_actions(
    view: &NpcMindView,
    stance: RelationshipStance,
    help: i16,
    reveal: i16,
    has_withheld: bool,
) -> (Vec<String>, Vec<String>) {
    let mut preferred = Vec::new();
    let mut forbidden = Vec::new();
    match stance {
        RelationshipStance::Hostile => {
            preferred.push("stand ground and warn the party off".into());
            preferred.push("demand they leave or state their business".into());
            forbidden.push("offer help freely".into());
        }
        RelationshipStance::Wary => {
            preferred.push("keep distance and give guarded answers".into());
            preferred.push("probe the party's intentions before committing".into());
        }
        RelationshipStance::Neutral => {
            preferred.push("respond plainly and stay noncommittal".into());
        }
        RelationshipStance::Cordial | RelationshipStance::Friendly | RelationshipStance::Allied => {
            preferred.push("engage warmly and offer reasonable help".into());
            preferred.push("share information that is safe to share".into());
        }
    }
    if help < 25 {
        forbidden.push("volunteer assistance the NPC would not give".into());
    }
    if has_withheld || reveal < REVEAL_SECRET_THRESHOLD {
        forbidden.push("disclose protected secrets".into());
    }
    for boundary in &view.persona.behavioral_boundaries {
        forbidden.push(boundary.clone());
    }
    for taboo in &view.persona.speech_style.taboo_topics {
        forbidden.push(format!("raise taboo topic: {taboo}"));
    }
    (preferred, forbidden)
}

/// One compact narration-steering line. Carries persona name + derived posture only.
fn dialogue_guidance(
    view: &NpcMindView,
    stance: RelationshipStance,
    emotion: NpcEmotionalState,
) -> String {
    let stance_word = serde_json::to_value(stance)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "neutral".into());
    let emotion_word = serde_json::to_value(emotion)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "neutral".into());
    format!(
        "Voice {} as {stance_word} and {emotion_word}; stay in character and reveal only what this plan permits.",
        view.persona.name
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        KnowledgeState, NpcKnowledgeEntry, NpcMindView, NpcProfile, NpcRelationship,
        NpcRelationshipDelta, NpcRelationshipTarget,
    };

    fn profile() -> NpcProfile {
        NpcProfile {
            actor_id: "npc_lars".into(),
            name: "Lars".into(),
            goals: vec!["protect the workshop".into()],
            behavioral_boundaries: vec!["never harm a child".into()],
            ..Default::default()
        }
    }

    fn rel(trust: i16, fear: i16, hostility: i16) -> NpcRelationship {
        let mut r =
            NpcRelationship::new("s", "npc_lars", NpcRelationshipTarget::PlayerParty).unwrap();
        r.apply_delta(&NpcRelationshipDelta {
            trust,
            fear,
            hostility,
            evidence_event_ids: vec!["e".into()],
            ..Default::default()
        })
        .unwrap();
        r
    }

    fn view(rel: NpcRelationship, entries: &[NpcKnowledgeEntry]) -> NpcMindView {
        NpcMindView::build("s", "npc_lars", &profile(), &[rel], entries).unwrap()
    }

    #[test]
    fn emotion_tracks_dominant_channel() {
        assert_eq!(derive_emotion(0, 0, 70), NpcEmotionalState::Hostile);
        assert_eq!(derive_emotion(0, 70, 0), NpcEmotionalState::Afraid);
        assert_eq!(derive_emotion(60, 0, 0), NpcEmotionalState::Warm);
        assert_eq!(derive_emotion(0, 0, 0), NpcEmotionalState::Neutral);
    }

    #[test]
    fn current_goal_falls_back_to_persona_then_override() {
        let plan =
            NpcBehaviorPlan::derive(&view(rel(0, 0, 0), &[]), &NpcBehaviorContext::default());
        assert_eq!(plan.current_goal.as_deref(), Some("protect the workshop"));
        let ctx = NpcBehaviorContext {
            current_goal: Some("flee the city".into()),
            ..Default::default()
        };
        let plan = NpcBehaviorPlan::derive(&view(rel(0, 0, 0), &[]), &ctx);
        assert_eq!(plan.current_goal.as_deref(), Some("flee the city"));
    }

    #[test]
    fn persona_boundaries_become_forbidden_actions() {
        let plan =
            NpcBehaviorPlan::derive(&view(rel(0, 0, 0), &[]), &NpcBehaviorContext::default());
        assert!(plan
            .forbidden_actions
            .iter()
            .any(|a| a == "never harm a child"));
    }

    #[test]
    fn no_focus_relationship_yields_neutral_defaults() {
        // A faction-focused plan with no faction relationship stored falls back to neutral.
        let ctx = NpcBehaviorContext {
            focus_target: Some(FocusTarget {
                target_kind: "faction".into(),
                target_id: "faction_x".into(),
            }),
            ..Default::default()
        };
        let plan = NpcBehaviorPlan::derive(&view(rel(80, 0, 0), &[]), &ctx);
        assert_eq!(plan.stance, RelationshipStance::Neutral);
        assert_eq!(plan.interaction_desire, 0);
    }

    #[test]
    fn source_event_ids_pass_through_as_provenance() {
        let ctx = NpcBehaviorContext {
            source_event_ids: vec!["evt_1".into(), "evt_2".into()],
            ..Default::default()
        };
        let entries = vec![NpcKnowledgeEntry {
            fact_id: "k".into(),
            state: KnowledgeState::KnowsTrue,
        }];
        let plan = NpcBehaviorPlan::derive(&view(rel(0, 0, 0), &entries), &ctx);
        assert_eq!(plan.source_event_ids, vec!["evt_1", "evt_2"]);
    }
}

//! World Reaction proposal types (P4.2): commit-nothing typed proposals the World
//! simulation layer emits as candidate reactions for the Director to select among and
//! the Kernel to (maybe) commit. Mirrors the proposal-type discipline of
//! [`crate::WorldFactCandidate`] / [`crate::npc_behavior::NpcBehaviorPlan`]:
//!
//! - **Proposals, not state.** Nothing here carries a DB handle, appends an event,
//!   or mutates anything. These are pure data the World layer hands upward; only the
//!   System Kernel commits (设计4补充 §二-④/⑩).
//! - **World only proposes reasonable reactions** — it never forces a climax, never
//!   rolls dice, never resolves mechanics (§二-⑥). Mechanical follow-through (e.g. an
//!   NPC attack) is re-entered through Rules downstream, not decided here.
//! - **No omniscience (§二十四-#4).** A reaction's knowledge basis may only cite facts
//!   an NPC can actually disclose; facts the NPC must withhold never appear. The pure
//!   projection that builds these (see `NpcBehaviorPlan::to_reaction_candidate`)
//!   enforces this structurally.
//! - **Round-trips from partial JSON.** Containers carry `#[serde(default)]` so a
//!   partial payload deserializes with empty/zero defaults (fail-closed: an absent
//!   field never invents pressure).
use serde::{Deserialize, Serialize};

/// One NPC's candidate reaction to the current situation — a pure projection of an
/// already-derived [`crate::npc_behavior::NpcBehaviorPlan`]. Scores are normalized to
/// `0.0..=1.0`. `knowledge_basis` lists ONLY fact ids the NPC may disclose.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldReactionCandidate {
    /// The reacting NPC.
    pub npc_id: String,
    /// Summary stance toward the focus target, as the candidate's snake_case word.
    #[serde(default)]
    pub stance: String,
    /// Coarse emotional read, as the candidate's snake_case word.
    #[serde(default)]
    pub emotional_state: String,
    /// How strongly the NPC wants to act, normalized `0.0..=1.0`
    /// (= interaction_desire / 100, clamped at the projection).
    #[serde(default)]
    pub urgency: f64,
    /// How able/willing the NPC is to act helpfully, normalized `0.0..=1.0`
    /// (= min(willingness_to_help, risk_tolerance) / 100).
    #[serde(default)]
    pub feasibility: f64,
    /// The NPC's risk appetite, normalized `0.0..=1.0` (= risk_tolerance / 100).
    #[serde(default)]
    pub risk: f64,
    /// Optional concrete action intent this reaction proposes (e.g. an attack). `None`
    /// ⇒ a speech/posture-only reaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_intent: Option<NpcActionIntent>,
    /// Fact ids the NPC MAY disclose to ground this reaction. NEVER facts the NPC must
    /// withhold (§二十四-#4): World must not use facts the NPC can't reveal.
    #[serde(default)]
    pub knowledge_basis: Vec<String>,
    /// Provenance event ids carried through from the source plan.
    #[serde(default)]
    pub source_event_ids: Vec<String>,
}

/// A concrete, typed action an NPC proposes to take. Carries NO resolution — kind +
/// target + provenance only. Any mechanical settlement (dice, damage) happens later in
/// Rules, never in World (§二-⑤/⑥). Defaults to a no-op `Wait`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NpcActionIntent {
    /// The kind of action proposed. `#[serde(default)]` ⇒ unknown payloads fail-closed
    /// to a non-mechanical `Wait`.
    #[serde(default)]
    pub kind: NpcActionKind,
    /// Optional target ref (e.g. `character:pc_1`). Free-form provenance, not a handle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_ref: Option<String>,
    /// Human-readable hint describing the proposed action (no mechanical numbers).
    #[serde(default)]
    pub description: String,
}

/// Kinds of NPC action a World reaction may propose. Non-exhaustive in spirit: extend
/// additively. `Wait` is the fail-closed default (proposes no mechanical change).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NpcActionKind {
    /// No mechanical action — hold position / observe.
    #[default]
    Wait,
    /// Speak / negotiate (no mechanical settlement).
    Speak,
    /// Move toward or away (positioning hint only).
    Move,
    /// Propose a mechanical attack. Settlement is re-entered through Rules downstream
    /// (gated, P4.6) — World never resolves it here.
    Attack,
    /// Propose using or activating something (positioning/social hint).
    Use,
}

/// A proposal to advance a clock by one or more ticks. Commit-nothing: the Kernel
/// decides whether to apply. Mirrors the world-pressure [`crate::ClockTick`] surface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClockAdvanceProposal {
    pub clock_id: String,
    #[serde(default)]
    pub label: String,
    /// Number of ticks proposed (>=0). Fail-closed default 0 ⇒ proposes nothing.
    #[serde(default)]
    pub ticks: i32,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub visible_to_players: bool,
    #[serde(default)]
    pub source_event_ids: Vec<String>,
}

/// A proposal that some holder's knowledge should change (e.g. an NPC now believes X).
/// Commit-nothing; the Kernel owns the actual knowledge write. Carries fact + holder
/// ids only — no truth adjudication, no DB handle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeDeltaProposal {
    /// The fact id whose holder-knowledge would change.
    pub fact_id: String,
    /// The holder gaining/losing the knowledge (e.g. `npc:lars`). Free-form ref.
    #[serde(default)]
    pub holder_ref: String,
    /// Proposed knowledge state token (e.g. `knows_true`). Free-form; the Kernel maps
    /// it onto the canonical `KnowledgeState`. Empty ⇒ no-op.
    #[serde(default)]
    pub knowledge_state: String,
    #[serde(default)]
    pub source_event_ids: Vec<String>,
}

/// The full set of World reaction proposals for one turn. Pure aggregate; every field
/// is `#[serde(default)]` so a partial payload round-trips to an empty (fail-closed)
/// set that proposes nothing.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WorldReactionSet {
    #[serde(default)]
    pub reactions: Vec<WorldReactionCandidate>,
    #[serde(default)]
    pub clock_advances: Vec<ClockAdvanceProposal>,
    #[serde(default)]
    pub knowledge_deltas: Vec<KnowledgeDeltaProposal>,
}

impl WorldReactionSet {
    /// True when the set proposes nothing at all (every channel empty). Useful as a
    /// fail-closed sentinel: an empty set commits nothing.
    pub fn is_empty(&self) -> bool {
        self.reactions.is_empty()
            && self.clock_advances.is_empty()
            && self.knowledge_deltas.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_reaction_set_default_is_empty() {
        let set = WorldReactionSet::default();
        assert!(set.is_empty());
        assert!(set.reactions.is_empty());
        assert!(set.clock_advances.is_empty());
        assert!(set.knowledge_deltas.is_empty());
    }

    #[test]
    fn npc_action_kind_defaults_to_wait_fail_closed() {
        assert_eq!(NpcActionKind::default(), NpcActionKind::Wait);
        let intent = NpcActionIntent {
            kind: NpcActionKind::default(),
            ..NpcActionIntent {
                kind: NpcActionKind::Wait,
                target_ref: None,
                description: String::new(),
            }
        };
        assert_eq!(intent.kind, NpcActionKind::Wait);
    }

    #[test]
    fn empty_object_round_trips_to_empty_set() {
        // Partial JSON (no fields) must deserialize to a fail-closed empty set.
        let set: WorldReactionSet = serde_json::from_str("{}").unwrap();
        assert!(set.is_empty());
    }

    #[test]
    fn world_reaction_candidate_round_trips() {
        let cand = WorldReactionCandidate {
            npc_id: "npc_lars".into(),
            stance: "wary".into(),
            emotional_state: "guarded".into(),
            urgency: 0.5,
            feasibility: 0.25,
            risk: 0.45,
            action_intent: Some(NpcActionIntent {
                kind: NpcActionKind::Speak,
                target_ref: Some("player_party".into()),
                description: "warn the party off".into(),
            }),
            knowledge_basis: vec!["fact_a".into()],
            source_event_ids: vec!["evt_1".into()],
        };
        let json = serde_json::to_string(&cand).unwrap();
        let back: WorldReactionCandidate = serde_json::from_str(&json).unwrap();
        assert_eq!(cand, back);
    }

    #[test]
    fn partial_candidate_json_uses_defaults() {
        // Only npc_id supplied; every defaulted field comes back empty/zero.
        let cand: WorldReactionCandidate = serde_json::from_str(r#"{"npc_id":"npc_x"}"#).unwrap();
        assert_eq!(cand.npc_id, "npc_x");
        assert_eq!(cand.urgency, 0.0);
        assert_eq!(cand.feasibility, 0.0);
        assert_eq!(cand.risk, 0.0);
        assert!(cand.stance.is_empty());
        assert!(cand.emotional_state.is_empty());
        assert!(cand.action_intent.is_none());
        assert!(cand.knowledge_basis.is_empty());
        assert!(cand.source_event_ids.is_empty());
    }

    #[test]
    fn full_set_round_trips() {
        let set = WorldReactionSet {
            reactions: vec![WorldReactionCandidate {
                npc_id: "npc_a".into(),
                stance: "hostile".into(),
                emotional_state: "hostile".into(),
                urgency: 1.0,
                feasibility: 0.1,
                risk: 0.9,
                action_intent: None,
                knowledge_basis: vec![],
                source_event_ids: vec![],
            }],
            clock_advances: vec![ClockAdvanceProposal {
                clock_id: "clock.scene_pressure".into(),
                label: "局势压力".into(),
                ticks: 1,
                reason: "stall".into(),
                visible_to_players: true,
                source_event_ids: vec!["e".into()],
            }],
            knowledge_deltas: vec![KnowledgeDeltaProposal {
                fact_id: "f1".into(),
                holder_ref: "npc:lars".into(),
                knowledge_state: "knows_true".into(),
                source_event_ids: vec![],
            }],
        };
        let json = serde_json::to_string(&set).unwrap();
        let back: WorldReactionSet = serde_json::from_str(&json).unwrap();
        assert_eq!(set, back);
        assert!(!set.is_empty());
    }
}

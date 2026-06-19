//! Director plan types (P5.2): the commit-nothing typed plan the Director emits for one
//! turn — which World candidates it selects, which threads it spotlights, what dramatic
//! change it wants, and (when the pool is empty) what it asks the World for. Mirrors the
//! proposal-type discipline of [`crate::world_reaction`]:
//!
//! - **Plan, not state.** Nothing here carries a DB handle or commits anything; the
//!   Kernel owns any follow-through (设计4补充 §二-④).
//! - **Round-trips from partial JSON.** Containers carry `#[serde(default)]` so a partial
//!   payload deserializes to a fail-closed plan that selects nothing.
//!
//! ## Candidate identity (codex fold #1)
//! [`crate::world_reaction::WorldReactionCandidate`] carries NO `candidate_id`. The
//! Director's selection key is the composite [`WorldCandidateRef`] `{npc_id,
//! source_event_ids, action_kind}`. Because production `source_event_ids` may be empty and
//! could otherwise collide for the same NPC+action, the key is **canonicalized**:
//! `source_event_ids` are sorted + deduped and `action_intent` maps to an explicit
//! `action_kind` (with a distinct `None` value when the candidate has no action). Two
//! candidates that canonicalize to the same key compare `Eq` and hash identically, so the
//! key is a sound set member for replay-parity (§二十四-#10).
//!
//! ## ProposalMeta (codex fold #6)
//! Grepped `crates/`: no `ProposalMeta` / `MechanicalResultView` / `MechanicalResult`
//! exists anywhere. So this defines a minimal, real typed [`ProposalMeta`] here (it is the
//! single source for proposal provenance; not a parallel duplicate of any existing type).
use crate::world_reaction::{NpcActionKind, WorldReactionCandidate};
use serde::{Deserialize, Serialize};

/// Minimal provenance/identity envelope shared by typed proposals. Commit-nothing: carries
/// ids + origin only, never a handle. Defined here because no `ProposalMeta` exists upstream
/// (codex fold #6); extend additively as more proposal types adopt it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProposalMeta {
    /// Stable id for this proposal (e.g. `director_plan:turn_12`). Empty ⇒ anonymous.
    #[serde(default)]
    pub proposal_id: String,
    /// Which layer/agent emitted it (e.g. `director`). Free-form provenance token.
    #[serde(default)]
    pub origin: String,
    /// The turn this proposal belongs to, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    /// Provenance event ids this proposal derives from.
    #[serde(default)]
    pub source_event_ids: Vec<String>,
}

/// The Director's selection key for a [`WorldReactionCandidate`] — a CANONICAL composite of
/// `{npc_id, source_event_ids, action_kind}` (no `candidate_id`, since the P4 type carries
/// none). `source_event_ids` are sorted + deduped and `action_kind` is explicit (its
/// `None`-vs-`Some` distinction preserved), so equal candidates produce equal keys.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WorldCandidateRef {
    pub npc_id: String,
    /// Canonicalized provenance event ids: sorted + deduped (deterministic; empty handled
    /// deterministically as an empty vec).
    #[serde(default)]
    pub source_event_ids: Vec<String>,
    /// The candidate's proposed action kind. `None` ⇒ the candidate has no `action_intent`
    /// (a speech/posture-only reaction) — kept distinct so a no-action candidate never
    /// collides with one that proposes a concrete action.
    #[serde(default)]
    pub action_kind: Option<NpcActionKind>,
}

// `Hash` is implemented manually rather than derived: the P4 [`NpcActionKind`] (outside
// this worker's write set) does not derive `Hash`, so we hash its stable serde token. This
// stays consistent with the derived `Eq` because equal `action_kind`s serialize to equal
// tokens. The token is a pure mapping over the fieldless enum; no allocation needed beyond
// the `&'static str`.
impl std::hash::Hash for WorldCandidateRef {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.npc_id.hash(state);
        self.source_event_ids.hash(state);
        action_kind_token(self.action_kind).hash(state);
    }
}

/// Stable, allocation-free token for an optional action kind, used for hashing. `None` maps
/// to a sentinel distinct from every concrete kind so a no-action key never hash-collides
/// with an action-bearing one by token.
fn action_kind_token(kind: Option<NpcActionKind>) -> &'static str {
    match kind {
        None => "__none__",
        Some(NpcActionKind::Wait) => "wait",
        Some(NpcActionKind::Speak) => "speak",
        Some(NpcActionKind::Move) => "move",
        Some(NpcActionKind::Attack) => "attack",
        Some(NpcActionKind::Use) => "use",
    }
}

impl WorldCandidateRef {
    /// Build the canonical selection key from a candidate. Sorts + dedups
    /// `source_event_ids` and maps `action_intent` → `action_kind` with explicit `None`
    /// handling (no `action_intent` ⇒ `action_kind = None`). Pure.
    pub fn from_candidate(candidate: &WorldReactionCandidate) -> Self {
        let mut source_event_ids = candidate.source_event_ids.clone();
        source_event_ids.sort();
        source_event_ids.dedup();
        let action_kind = candidate.action_intent.as_ref().map(|intent| intent.kind);
        Self {
            npc_id: candidate.npc_id.clone(),
            source_event_ids,
            action_kind,
        }
    }
}

/// The Director's typed plan for one turn. Every field `#[serde(default)]` so a partial
/// payload round-trips to a fail-closed plan (no selections, `Respond` beat).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DirectorPlan {
    #[serde(default)]
    pub meta: ProposalMeta,
    #[serde(default)]
    pub beat_kind: crate::story::BeatKind,
    /// The World candidates this plan selects, by canonical key (§二十四-#3: only keys that
    /// were actually in the proposal pool).
    #[serde(default)]
    pub selected_world_candidates: Vec<WorldCandidateRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_thread_id: Option<String>,
    #[serde(default)]
    pub secondary_thread_ids: Vec<String>,
    #[serde(default)]
    pub focus_actor_ids: Vec<String>,
    /// Fact ids the plan would reveal. NEVER raw secret prose — ids only (codex fold #2).
    #[serde(default)]
    pub reveal_candidate_fact_ids: Vec<String>,
    #[serde(default)]
    pub callback_event_ids: Vec<String>,
    #[serde(default)]
    pub dramatic_function: String,
    #[serde(default)]
    pub desired_change: String,
    #[serde(default)]
    pub must_preserve: Vec<String>,
    #[serde(default)]
    pub must_avoid: Vec<String>,
    #[serde(default)]
    pub open_player_affordances: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spotlight_target: Option<String>,
    /// Set when the candidate pool is empty: what the Director asks the World for instead
    /// of forcing a beat (§二十四-#3 fail-closed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world_query: Option<WorldQuery>,
}

/// A query the Director sends to the World when it has no usable candidate to select — it
/// asks for a fitting actor/thread rather than inventing one. Commit-nothing.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WorldQuery {
    #[serde(default)]
    pub intent: WorldQueryIntent,
    #[serde(default)]
    pub question: String,
    /// Fact ids constraining the query (e.g. the secret an answering NPC must plausibly
    /// know). Ids only — no prose.
    #[serde(default)]
    pub constraint_fact_ids: Vec<String>,
}

/// The shape of a [`WorldQuery`]. `Generic` is the fail-closed default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorldQueryIntent {
    /// Is there an NPC who plausibly knows the constraint fact(s)?
    ExistsKnowingNpc,
    /// Is there an NPC motivated to act on the situation?
    ExistsMotivatedNpc,
    /// Is there an available thread to pivot to?
    ExistsAvailableThread,
    #[default]
    Generic,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::story::BeatKind;
    use crate::world_reaction::{NpcActionIntent, NpcActionKind};

    fn candidate(npc: &str, events: Vec<&str>, action: Option<NpcActionKind>) -> WorldReactionCandidate {
        WorldReactionCandidate {
            npc_id: npc.into(),
            stance: String::new(),
            emotional_state: String::new(),
            urgency: 0.0,
            feasibility: 0.0,
            risk: 0.0,
            action_intent: action.map(|kind| NpcActionIntent {
                kind,
                target_ref: None,
                description: String::new(),
            }),
            knowledge_basis: vec![],
            source_event_ids: events.into_iter().map(String::from).collect(),
        }
    }

    #[test]
    fn director_plan_default_round_trips() {
        let plan = DirectorPlan::default();
        assert_eq!(plan.beat_kind, BeatKind::Respond);
        assert!(plan.selected_world_candidates.is_empty());
        assert!(plan.world_query.is_none());
        let back: DirectorPlan = serde_json::from_str("{}").unwrap();
        assert_eq!(plan, back);
    }

    #[test]
    fn director_plan_full_round_trips() {
        let plan = DirectorPlan {
            meta: ProposalMeta {
                proposal_id: "director_plan:turn_12".into(),
                origin: "director".into(),
                turn_id: Some("turn_12".into()),
                source_event_ids: vec!["evt_1".into()],
            },
            beat_kind: BeatKind::Reveal,
            selected_world_candidates: vec![WorldCandidateRef {
                npc_id: "npc_a".into(),
                source_event_ids: vec!["evt_1".into()],
                action_kind: Some(NpcActionKind::Speak),
            }],
            primary_thread_id: Some("thread_1".into()),
            secondary_thread_ids: vec!["thread_2".into()],
            focus_actor_ids: vec!["npc_a".into()],
            reveal_candidate_fact_ids: vec!["fact_x".into()],
            callback_event_ids: vec!["evt_0".into()],
            dramatic_function: "raise stakes".into(),
            desired_change: "trust broken".into(),
            must_preserve: vec!["pc agency".into()],
            must_avoid: vec!["railroad".into()],
            open_player_affordances: vec!["flee".into(), "negotiate".into()],
            spotlight_target: Some("npc_a".into()),
            world_query: Some(WorldQuery {
                intent: WorldQueryIntent::ExistsKnowingNpc,
                question: "who knows the secret?".into(),
                constraint_fact_ids: vec!["fact_x".into()],
            }),
        };
        let json = serde_json::to_string(&plan).unwrap();
        let back: DirectorPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(plan, back);
    }

    #[test]
    fn world_query_intent_defaults_generic_and_snake_case() {
        assert_eq!(WorldQueryIntent::default(), WorldQueryIntent::Generic);
        assert_eq!(
            serde_json::to_string(&WorldQueryIntent::ExistsKnowingNpc).unwrap(),
            "\"exists_knowing_npc\""
        );
    }

    #[test]
    fn canonical_key_equal_for_reordered_and_duplicated_events() {
        // Same npc, same action, source_event_ids differ only by order + duplicates.
        let a = candidate("npc_a", vec!["e2", "e1", "e1"], Some(NpcActionKind::Speak));
        let b = candidate("npc_a", vec!["e1", "e2"], Some(NpcActionKind::Speak));
        let ka = WorldCandidateRef::from_candidate(&a);
        let kb = WorldCandidateRef::from_candidate(&b);
        assert_eq!(ka, kb);
        assert_eq!(ka.source_event_ids, vec!["e1".to_string(), "e2".to_string()]);
        // Usable as a set key: both map to one entry.
        let set: std::collections::HashSet<_> = vec![ka, kb].into_iter().collect();
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn canonical_key_none_action_distinct_from_some() {
        let no_action = candidate("npc_a", vec!["e1"], None);
        let with_action = candidate("npc_a", vec!["e1"], Some(NpcActionKind::Wait));
        let kn = WorldCandidateRef::from_candidate(&no_action);
        let kw = WorldCandidateRef::from_candidate(&with_action);
        assert_eq!(kn.action_kind, None);
        assert_eq!(kw.action_kind, Some(NpcActionKind::Wait));
        assert_ne!(kn, kw);
    }

    #[test]
    fn canonical_key_empty_events_deterministic() {
        let a = candidate("npc_a", vec![], Some(NpcActionKind::Speak));
        let b = candidate("npc_a", vec![], Some(NpcActionKind::Speak));
        let ka = WorldCandidateRef::from_candidate(&a);
        let kb = WorldCandidateRef::from_candidate(&b);
        // Empty source_event_ids ⇒ identical (deterministic) keys — they collide by design;
        // P5.3 is responsible for fail-closing on such ambiguous selections.
        assert_eq!(ka, kb);
        assert!(ka.source_event_ids.is_empty());
    }

    #[test]
    fn canonical_key_round_trips() {
        let key = WorldCandidateRef {
            npc_id: "npc_a".into(),
            source_event_ids: vec!["e1".into()],
            action_kind: Some(NpcActionKind::Attack),
        };
        let json = serde_json::to_string(&key).unwrap();
        let back: WorldCandidateRef = serde_json::from_str(&json).unwrap();
        assert_eq!(key, back);
    }

    #[test]
    fn proposal_meta_partial_json_defaults() {
        let meta: ProposalMeta = serde_json::from_str("{}").unwrap();
        assert!(meta.proposal_id.is_empty());
        assert!(meta.origin.is_empty());
        assert!(meta.turn_id.is_none());
        assert!(meta.source_event_ids.is_empty());
    }
}

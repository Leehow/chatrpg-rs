//! Adventure IR — Relation (P0-1): typed STATIC relations. The core semantic-
//! cleanup: "two units share an entity" is `AssociatedByEntity/RetrievalOnly`,
//! NOT `SpatialAdjacent` (sharing an NPC proves relatedness, not adjacency /
//! causality / traversability). Whether a relation constrains movement is decided
//! per-`Enforcement`, never a blanket yes/no (设计评审 §二/§八).
use crate::SourceRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The kind of static relation between two content units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    /// Document hierarchy: chapter contains mission, mission contains location.
    Contains,
    /// Physically adjacent / traversable.
    SpatialAdjacent,
    /// Authored "after X, do Y" flow.
    AuthoredNext,
    /// Completing/knowing source opens the target.
    Unlocks,
    /// Source reveals a fact/target.
    Reveals,
    /// Source schedules the target (timer/event).
    Schedules,
    /// Target is an alternative path to the source.
    AlternativeTo,
    /// Source contributes to the target's score.
    ScoresToward,
    /// The two units merely mention the same entity — retrieval/context only.
    AssociatedByEntity,
}

/// How strongly a relation constrains runtime progression (设计评审 §八 4-tier).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Enforcement {
    /// Walls, doors, roads — physically unreachable otherwise.
    HardWorld,
    /// Cannot *complete* the beat until the gate condition holds (but may act).
    AuthoredGate,
    /// Authored-recommended next beat; followed only when the player leans there.
    SoftGravity,
    /// Recall/context only; never participates in progression.
    RetrievalOnly,
}

/// Provenance of a relation — used with the fail-closed tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Authority {
    /// Backed by explicit source text.
    Authored,
    /// Derived deterministically from structure (e.g. entity co-occurrence).
    Structural,
    /// LLM-inferred with no firm anchor → retrieval only.
    Inferred,
}

/// A typed static relation with provenance and evidence (设计评审 §三.2).
/// (No `Eq`: embeds `Vec<SourceRef>`, which is only `PartialEq`.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Relation {
    pub kind: RelationKind,
    pub from: String,
    pub to: String,
    pub enforcement: Enforcement,
    pub authority: Authority,
    #[serde(default)]
    pub evidence: Vec<SourceRef>,
}

impl Relation {
    /// Build the relation an entity co-occurrence should produce: associated, not
    /// spatial; retrieval-only; structural provenance. This is the P0-1 fix that
    /// stops co-occurrence from masquerading as a transition candidate.
    pub fn from_entity_cooccurrence(from: &str, to: &str, shared: usize) -> Self {
        let mut note = SourceRef::default();
        note.note = Some(format!("共享 {shared} 个实体(实体共现,非空间相邻)"));
        Relation {
            kind: RelationKind::AssociatedByEntity,
            from: from.to_string(),
            to: to.to_string(),
            enforcement: Enforcement::RetrievalOnly,
            authority: Authority::Structural,
            evidence: vec![note],
        }
    }

    /// Whether this relation may constrain/inform progression at all. Retrieval-
    /// only relations must never gate or drive a transition.
    pub fn participates_in_progression(&self) -> bool {
        !matches!(self.enforcement, Enforcement::RetrievalOnly)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cooccurrence_is_associated_not_spatial() {
        let r = Relation::from_entity_cooccurrence("scene.a", "scene.b", 2);
        assert_eq!(r.kind, RelationKind::AssociatedByEntity);
        assert_ne!(r.kind, RelationKind::SpatialAdjacent);
        assert_eq!(r.enforcement, Enforcement::RetrievalOnly);
        assert_eq!(r.authority, Authority::Structural);
        assert!(
            !r.participates_in_progression(),
            "retrieval-only never drives"
        );
        assert!(!r.evidence.is_empty(), "carries evidence note");
    }

    #[test]
    fn authored_next_participates() {
        let r = Relation {
            kind: RelationKind::AuthoredNext,
            from: "beat.x".into(),
            to: "beat.y".into(),
            enforcement: Enforcement::SoftGravity,
            authority: Authority::Authored,
            evidence: vec![],
        };
        assert!(r.participates_in_progression());
    }

    #[test]
    fn relation_roundtrips_json() {
        let r = Relation::from_entity_cooccurrence("a", "b", 1);
        let s = serde_json::to_string(&r).unwrap();
        let back: Relation = serde_json::from_str(&s).unwrap();
        assert_eq!(r, back);
    }
}

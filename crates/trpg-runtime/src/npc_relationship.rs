//! Runtime orchestration for NPC Relationship v1 (TC-NPC-02).
//!
//! The model ([`trpg_model::npc_relationship`]) owns the bounded, evidence-gated state
//! machine. This adapter wires it to durable storage: load-or-default the relationship,
//! apply a delta through the same evidence-required path, and persist the result. The
//! pure step ([`next_relationship`]) is unit-testable without a database; the DB-bound
//! wrapper ([`apply_npc_relationship_delta`]) is the runtime store-and-update entry
//! point so an NPC's attitude toward the player no longer rides on a free-form memory
//! summary.
//!
//! No LLM ever sets final values: callers may only propose an [`NpcRelationshipDelta`],
//! which still passes through the bounded clamp and the non-empty-evidence gate.
use trpg_db::Db;
use trpg_model::{
    NpcRelationship, NpcRelationshipDelta, NpcRelationshipTarget, RelationshipError,
};

/// Pure store-update step. Given the currently persisted relationship (or `None` for a
/// first interaction) plus identity and a delta, produce the next relationship to
/// persist. When `current` is `None` a fresh neutral relationship is constructed (which
/// validates the NPC id fail-closed). The delta is then applied through the model's
/// bounded, evidence-required path. No IO — the DB wrapper supplies `current`.
pub fn next_relationship(
    current: Option<NpcRelationship>,
    session_id: &str,
    npc_id: &str,
    target: NpcRelationshipTarget,
    delta: &NpcRelationshipDelta,
) -> Result<NpcRelationship, RelationshipError> {
    let mut rel = match current {
        Some(rel) => rel,
        None => NpcRelationship::new(session_id, npc_id, target)?,
    };
    rel.apply_delta(delta)?;
    Ok(rel)
}

/// Durable store-and-update: load the existing relationship, apply `delta`, persist, and
/// return the new state. Fail-closed at every layer — the identity is validated on
/// construction and the delta is refused without evidence, so nothing is written for an
/// unstable id or an evidence-less change.
pub async fn apply_npc_relationship_delta(
    db: &Db,
    session_id: &str,
    npc_id: &str,
    target: NpcRelationshipTarget,
    delta: &NpcRelationshipDelta,
) -> anyhow::Result<NpcRelationship> {
    let current = db
        .load_npc_relationship(session_id, npc_id, target.kind_token(), target.target_id())
        .await?;
    let next = next_relationship(current, session_id, npc_id, target, delta)
        .map_err(|e| anyhow::anyhow!("apply_npc_relationship_delta: {e}"))?;
    db.upsert_npc_relationship(&next).await?;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::RelationshipStance;

    #[test]
    fn first_interaction_constructs_then_applies() {
        let next = next_relationship(
            None,
            "sess",
            "npc_lars",
            NpcRelationshipTarget::PlayerParty,
            &NpcRelationshipDelta::help(vec!["evt".into()]),
        )
        .unwrap();
        assert!(next.trust > 0 && next.debt > 0);
        assert_eq!(next.evidence_event_ids, vec!["evt".to_string()]);
    }

    #[test]
    fn subsequent_delta_accumulates_on_loaded_state() {
        let first = next_relationship(
            None,
            "sess",
            "npc_lars",
            NpcRelationshipTarget::PlayerParty,
            &NpcRelationshipDelta::help(vec!["e1".into()]),
        )
        .unwrap();
        let trust_after_help = first.trust;
        let second = next_relationship(
            Some(first),
            "sess",
            "npc_lars",
            NpcRelationshipTarget::PlayerParty,
            &NpcRelationshipDelta::threat(vec!["e2".into()]),
        )
        .unwrap();
        assert!(second.trust < trust_after_help, "threat after help lowers trust");
        assert!(second.fear > 0);
        assert_eq!(second.evidence_event_ids, vec!["e1".to_string(), "e2".to_string()]);
    }

    #[test]
    fn missing_evidence_refuses_even_on_first_interaction() {
        let err = next_relationship(
            None,
            "sess",
            "npc_lars",
            NpcRelationshipTarget::PlayerParty,
            &NpcRelationshipDelta::default(),
        )
        .unwrap_err();
        assert_eq!(err, RelationshipError::MissingEvidence);
    }

    #[test]
    fn unstable_npc_id_refused_before_any_store() {
        let err = next_relationship(
            None,
            "sess",
            "The Butler",
            NpcRelationshipTarget::PlayerParty,
            &NpcRelationshipDelta::help(vec!["e".into()]),
        )
        .unwrap_err();
        assert!(matches!(err, RelationshipError::UnstableId(_)));
    }

    #[test]
    fn help_then_more_help_can_reach_friendly_or_higher() {
        let mut rel = next_relationship(
            None,
            "s",
            "npc_a",
            NpcRelationshipTarget::PlayerParty,
            &NpcRelationshipDelta::help(vec!["e1".into()]),
        )
        .unwrap();
        for i in 0..8 {
            rel = next_relationship(
                Some(rel),
                "s",
                "npc_a",
                NpcRelationshipTarget::PlayerParty,
                &NpcRelationshipDelta {
                    affection: 12,
                    ..NpcRelationshipDelta::help(vec![format!("e_more_{i}")])
                },
            )
            .unwrap();
        }
        assert!(matches!(
            rel.stance,
            RelationshipStance::Friendly | RelationshipStance::Allied | RelationshipStance::Cordial
        ));
    }
}

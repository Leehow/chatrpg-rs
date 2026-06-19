//! Runtime orchestration for NPC Mind View v1 (TC-NPC-03).
//!
//! The model ([`trpg_model::npc_mind`]) owns the pure projection: validate identity,
//! drop GM-only secrets via the persona safe view, split knowledge vs belief, and
//! compact the relationship state. This adapter wires it to durable storage: pull the
//! NPC's own knowledge/belief edges and its stored relationships, then assemble one
//! prompt-safe [`NpcMindView`].
//!
//! Foundation only: it produces the view a later NPC behavior planner / dialogue path
//! will consume. It does NOT generate dialogue, plan behavior, or touch the turn loop.
use trpg_db::Db;
use trpg_model::{
    NpcKnowledgeEntry, NpcMindView, NpcProfile, NpcRelationship, NpcRelationshipTarget,
};

/// Pure assembly step: given an NPC's persona, its stored relationships, and its durable
/// knowledge entries, project the prompt-safe mind view. No IO — the DB wrapper supplies
/// the relationships/entries. Fail-closed on an unstable id or a mismatched profile.
pub fn assemble_npc_mind_view(
    session_id: &str,
    npc_actor_id: &str,
    profile: &NpcProfile,
    relationships: &[NpcRelationship],
    entries: &[NpcKnowledgeEntry],
) -> anyhow::Result<NpcMindView> {
    NpcMindView::build(session_id, npc_actor_id, profile, relationships, entries)
        .map_err(|e| anyhow::anyhow!("assemble_npc_mind_view: {e}"))
}

/// Durable load-and-project: read this NPC's own knowledge/belief edges and the stored
/// relationship toward each requested target, then assemble the mind view. Only the
/// NPC's own edges are read (the DB query filters by holder), so GM world truth and
/// other holders' knowledge can never enter. Relationships that aren't stored yet are
/// simply absent (first-interaction NPCs project an empty attitude list).
pub async fn load_npc_mind_view(
    db: &Db,
    session_id: &str,
    npc_actor_id: &str,
    profile: &NpcProfile,
    relationship_targets: &[NpcRelationshipTarget],
) -> anyhow::Result<NpcMindView> {
    let entries = db.list_npc_mind_facts(session_id, npc_actor_id).await?;
    let mut relationships = Vec::new();
    for target in relationship_targets {
        if let Some(rel) = db
            .load_npc_relationship(
                session_id,
                npc_actor_id,
                target.kind_token(),
                target.target_id(),
            )
            .await?
        {
            relationships.push(rel);
        }
    }
    assemble_npc_mind_view(session_id, npc_actor_id, profile, &relationships, &entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::{
        KnowledgeState, NpcFactStanding, NpcRelationshipDelta, NpcSecret, Visibility,
    };

    fn profile() -> NpcProfile {
        NpcProfile {
            actor_id: "npc_alice".into(),
            name: "Alice".into(),
            role: Some("innkeeper".into()),
            secrets: vec![NpcSecret {
                secret_id: "s".into(),
                content: "GMONLY_secret".into(),
                visibility: Visibility::GmOnly,
                source_refs: vec![],
            }],
            ..Default::default()
        }
    }

    fn entry(id: &str, state: KnowledgeState) -> NpcKnowledgeEntry {
        NpcKnowledgeEntry {
            fact_id: id.into(),
            state,
        }
    }

    #[test]
    fn assemble_projects_knowledge_belief_and_relationship() {
        let mut rel =
            NpcRelationship::new("s", "npc_alice", NpcRelationshipTarget::PlayerParty).unwrap();
        rel.apply_delta(&NpcRelationshipDelta::threat(vec!["e".into()]))
            .unwrap();
        let entries = vec![
            entry("known", KnowledgeState::KnowsTrue),
            entry("false", KnowledgeState::BelievesFalse),
        ];
        let view = assemble_npc_mind_view("s", "npc_alice", &profile(), &[rel], &entries).unwrap();
        assert_eq!(view.known_fact_ids(), vec!["known"]);
        assert_eq!(view.belief_fact_ids(), vec!["false"]);
        assert_eq!(view.relationships.len(), 1);
        assert!(view
            .facts
            .iter()
            .find(|f| f.fact_id == "false")
            .map(|f| f.standing == NpcFactStanding::Belief)
            .unwrap_or(false));
    }

    #[test]
    fn assemble_speech_context_drops_gm_secret() {
        let view = assemble_npc_mind_view("s", "npc_alice", &profile(), &[], &[]).unwrap();
        assert!(!view.speech_context().contains("GMONLY_secret"));
    }

    #[test]
    fn assemble_fails_closed_on_unstable_id() {
        assert!(assemble_npc_mind_view("s", "The Innkeeper", &profile(), &[], &[]).is_err());
    }
}

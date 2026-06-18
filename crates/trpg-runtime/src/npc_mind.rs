//! Runtime orchestration for NPC Mind View v1 (TC-NPC-03).
//!
//! The model ([`trpg_model::npc_mind`]) owns the pure projection: validate identity,
//! drop GM-only secrets via the persona safe view, split knowledge vs belief, and
//! compact the relationship state. This adapter exposes the pure assembly entry point
//! so a caller can project a prompt-safe [`NpcMindView`] from already-loaded inputs.
//!
//! Scope note (P1 slice-1): the holder identity gate's id prerequisite is closed, so the
//! DB-bound [`load_npc_mind_view`] now reads this NPC's own durable `knowledge_edges`
//! (`holder_kind='npc'`, normalized stable holder id) via
//! [`trpg_db::Db::list_npc_knowledge_entries`] plus its durable relationships via
//! [`trpg_db::Db::list_npc_relationships`], and assembles the prompt-safe view. This is a
//! **read-only** production surface: no gameplay writer decides when an NPC learns a fact
//! (edges are seeded out-of-band / by tests). The placeholder `npc.opposition` fails closed
//! at the read APIs and the DB CHECK. The pure [`assemble_npc_mind_view`] entry point is
//! preserved for callers that already hold the inputs.
use trpg_db::Db;
use trpg_model::{NpcKnowledgeEntry, NpcMindView, NpcProfile, NpcRelationship};

/// Pure assembly step: given an NPC's persona, its stored relationships, and its durable
/// knowledge entries, project the prompt-safe mind view. No IO — the caller supplies the
/// relationships/entries. Fail-closed on an unstable id or a mismatched profile.
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

/// DB-bound assembly: load this NPC's durable relationships and own `knowledge_edges`
/// (read-only) and project the prompt-safe [`NpcMindView`]. Fail-closed — the id is
/// validated by the DB read APIs and again by [`assemble_npc_mind_view`], so the
/// `npc.opposition` placeholder, empty ids, and display names never reach a query.
pub async fn load_npc_mind_view(
    db: &Db,
    session_id: &str,
    npc_actor_id: &str,
    profile: &NpcProfile,
) -> anyhow::Result<NpcMindView> {
    let relationships = db.list_npc_relationships(session_id, npc_actor_id).await?;
    let entries = db.list_npc_knowledge_entries(session_id, npc_actor_id).await?;
    assemble_npc_mind_view(session_id, npc_actor_id, profile, &relationships, &entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::{
        KnowledgeState, NpcFactStanding, NpcRelationshipDelta, NpcRelationshipTarget, NpcSecret,
        Visibility,
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
        NpcKnowledgeEntry { fact_id: id.into(), state }
    }

    #[test]
    fn assemble_projects_knowledge_belief_and_relationship() {
        let mut rel =
            NpcRelationship::new("s", "npc_alice", NpcRelationshipTarget::PlayerParty).unwrap();
        rel.apply_delta(&NpcRelationshipDelta::threat(vec!["e".into()])).unwrap();
        let entries = vec![
            entry("known", KnowledgeState::KnowsTrue),
            entry("false", KnowledgeState::BelievesFalse),
        ];
        let view =
            assemble_npc_mind_view("s", "npc_alice", &profile(), &[rel], &entries).unwrap();
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

//! Runtime orchestration for NPC Mind View v1 (TC-NPC-03).
//!
//! The model ([`trpg_model::npc_mind`]) owns the pure projection: validate identity,
//! drop GM-only secrets via the persona safe view, split knowledge vs belief, and
//! compact the relationship state. This adapter exposes the pure assembly entry point
//! so a caller can project a prompt-safe [`NpcMindView`] from already-loaded inputs.
//!
//! Scope note (P0b foundation): the DB-bound `load_npc_mind_view` from the original port
//! read this NPC's own `knowledge_edges` rows via `holder_kind='npc'`. The current
//! integration base GATES NPC-as-holder edges (see
//! `docs/superpowers/specs/2026-06-17-npc-holder-identity-gate.md`): the schema CHECK
//! forbids `npc` holders and no trustworthy writer exists yet. So the DB knowledge-read
//! path is intentionally NOT provided here — it would be an empty, misleading capability.
//! Callers supply the NPC's knowledge `entries` directly until the gate opens. The
//! relationship inputs ARE durable (see [`trpg_db::Db::load_npc_relationship`]).
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

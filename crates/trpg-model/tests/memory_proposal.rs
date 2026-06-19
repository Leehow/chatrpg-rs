//! TC-P2-01 Memory Extraction Proposals — model-layer tests (pure, no DB / no LLM).
//!
//! Covers the proposal contract: fact identity vs holder knowledge separation,
//! fail-closed identity gating, the preserved evidence gate, the no-commit guarantee, and
//! serde round-trippability.
use trpg_model::KnowledgeUpdateCandidate;
use trpg_model::{
    KnowledgeHolderKind, KnowledgeState, MemoryExtractionProposal, NpcRelationship,
    NpcRelationshipDelta, NpcRelationshipDeltaCandidate, NpcRelationshipTarget, ProposalError,
    ProposalHolder, RelationshipStance, WorldFactCandidate,
};

fn world_fact() -> WorldFactCandidate {
    WorldFactCandidate {
        fact_id: "fact_butler_killer".into(),
        subject: "npc_butler".into(),
        predicate: "is".into(),
        object: "the_killer".into(),
        summary: "The butler is the killer.".into(),
        confidence: Some(0.9),
        source_event_ids: vec!["ev_scene1".into()],
        turn_id: Some("t1".into()),
        truth_status: None,
    }
}

#[test]
fn memory_proposal_separates_world_fact_from_holder_knowledge() {
    // The truth half: a world fact carries subject/predicate/object and NO holder.
    let fact = world_fact();
    // The who-knows half: references the SAME fact_id, plus a holder + state, and carries
    // no fact body. The two are distinct proposal concepts that share only the identity.
    let know = KnowledgeUpdateCandidate {
        fact_id: fact.fact_id.clone(),
        holder: ProposalHolder::npc("npc_alice"),
        knowledge_state: KnowledgeState::Suspects,
        confidence: Some(0.5),
        source_event_ids: vec!["ev_scene1".into()],
        learned_at_turn_id: Some("t1".into()),
        reason: None,
    };
    assert_eq!(
        fact.fact_id, know.fact_id,
        "knowledge references fact identity"
    );
    assert_eq!(know.holder.holder_kind, KnowledgeHolderKind::Npc);

    let fact_proposal = MemoryExtractionProposal::WorldFact(fact)
        .validated()
        .unwrap();
    let know_proposal = MemoryExtractionProposal::KnowledgeUpdate(know)
        .validated()
        .unwrap();
    assert_eq!(fact_proposal.kind_token(), "world_fact");
    assert_eq!(know_proposal.kind_token(), "knowledge_update");
    assert_ne!(fact_proposal.kind_token(), know_proposal.kind_token());
}

#[test]
fn memory_proposal_rejects_unstable_npc_holder() {
    // A display-name NPC holder must fail closed (TC-KNOW-00 contract).
    let bad = KnowledgeUpdateCandidate {
        fact_id: "f1".into(),
        holder: ProposalHolder::npc("The Butler"),
        knowledge_state: KnowledgeState::KnowsTrue,
        confidence: None,
        source_event_ids: vec!["ev1".into()],
        learned_at_turn_id: None,
        reason: None,
    };
    assert!(matches!(
        MemoryExtractionProposal::KnowledgeUpdate(bad).validated(),
        Err(ProposalError::UnstableHolder(_))
    ));
    // A missing NPC id also fails closed — never invented.
    assert!(matches!(
        ProposalHolder {
            holder_kind: KnowledgeHolderKind::Npc,
            holder_id: None
        }
        .validated(),
        Err(ProposalError::UnstableHolder(_))
    ));
    // A stable NPC id passes and is normalized (trimmed).
    let ok = ProposalHolder::npc("  npc_alice  ").validated().unwrap();
    assert_eq!(ok.holder_id.as_deref(), Some("npc_alice"));
    assert_eq!(ok.token(), "npc:npc_alice");
    // Collective holders drop any id and stay durable runtime identities.
    assert_eq!(ProposalHolder::gm().validated().unwrap().token(), "gm");
    assert_eq!(
        ProposalHolder::player_party().validated().unwrap().token(),
        "player_party"
    );
    assert_eq!(
        ProposalHolder::system().validated().unwrap().token(),
        "system"
    );
}

#[test]
fn memory_proposal_relationship_requires_evidence() {
    // No evidence → the TC-NPC-02 hard gate refuses the proposal.
    let no_evidence = NpcRelationshipDeltaCandidate {
        session_id: "s".into(),
        npc_id: "npc_lars".into(),
        target: NpcRelationshipTarget::PlayerParty,
        delta: NpcRelationshipDelta::default(),
        reason: None,
    };
    assert!(matches!(
        MemoryExtractionProposal::NpcRelationshipDelta(no_evidence).validated(),
        Err(ProposalError::MissingEvidence)
    ));
    // With evidence it validates (but is NOT applied here).
    let with_evidence = NpcRelationshipDeltaCandidate {
        session_id: "s".into(),
        npc_id: "npc_lars".into(),
        target: NpcRelationshipTarget::PlayerParty,
        delta: NpcRelationshipDelta::help(vec!["ev1".into()]),
        reason: Some("party healed Lars".into()),
    };
    assert!(
        MemoryExtractionProposal::NpcRelationshipDelta(with_evidence)
            .validated()
            .is_ok()
    );
    // An unstable relationship holder still fails closed even with evidence.
    let unstable = NpcRelationshipDeltaCandidate {
        session_id: "s".into(),
        npc_id: "The Butler".into(),
        target: NpcRelationshipTarget::PlayerParty,
        delta: NpcRelationshipDelta::help(vec!["ev1".into()]),
        reason: None,
    };
    assert!(matches!(
        MemoryExtractionProposal::NpcRelationshipDelta(unstable).validated(),
        Err(ProposalError::UnstableHolder(_))
    ));
}

#[test]
fn memory_proposal_does_not_commit_state() {
    // A standing relationship the proposal "describes a change to".
    let rel = NpcRelationship::new("s", "npc_lars", NpcRelationshipTarget::PlayerParty).unwrap();
    let candidate = NpcRelationshipDeltaCandidate {
        session_id: "s".into(),
        npc_id: "npc_lars".into(),
        target: NpcRelationshipTarget::PlayerParty,
        delta: NpcRelationshipDelta::help(vec!["ev1".into()]),
        reason: None,
    };
    // Representing + validating the proposal must NOT mutate the relationship: applying
    // stays a separate, runtime-owned step.
    let _ = MemoryExtractionProposal::NpcRelationshipDelta(candidate)
        .validated()
        .unwrap();
    assert_eq!(rel.trust, 0, "proposal layer must not apply the delta");
    assert_eq!(rel.debt, 0);
    assert_eq!(rel.stance, RelationshipStance::Neutral);
    assert!(rel.evidence_event_ids.is_empty());
}

#[test]
fn memory_proposal_preserves_source_refs() {
    let proposal = MemoryExtractionProposal::WorldFact(WorldFactCandidate {
        source_event_ids: vec!["ev1".into(), "ev2".into()],
        ..world_fact()
    })
    .validated()
    .unwrap();
    assert_eq!(
        proposal.evidence_refs(),
        vec!["ev1".to_string(), "ev2".to_string()]
    );

    // Missing evidence fails closed.
    let bad = MemoryExtractionProposal::WorldFact(WorldFactCandidate {
        source_event_ids: vec![],
        ..world_fact()
    });
    assert!(matches!(
        bad.validated(),
        Err(ProposalError::MissingEvidence)
    ));

    // Missing fact identity fails closed.
    let bad_id = MemoryExtractionProposal::WorldFact(WorldFactCandidate {
        subject: "  ".into(),
        ..world_fact()
    });
    assert!(matches!(
        bad_id.validated(),
        Err(ProposalError::MissingFactIdentity)
    ));

    // Serde round-trips and preserves the evidence refs.
    let json = serde_json::to_string(&proposal).unwrap();
    let back: MemoryExtractionProposal = serde_json::from_str(&json).unwrap();
    assert_eq!(back.evidence_refs(), proposal.evidence_refs());
    assert_eq!(back.kind_token(), "world_fact");
}

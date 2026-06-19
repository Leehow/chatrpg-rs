//! TC-P2-01 Memory Extraction Proposals — runtime-layer tests (pure, no DB / no LLM).
//!
//! Covers bridging existing relationship triples into proposals, deterministic JSON
//! parsing (lenient + strict/fail-closed), evidence preservation, and the no-commit
//! guarantee (the helpers have no DB handle and write nothing).
use serde_json::json;
use trpg_model::{MemoryExtractionProposal, MemoryFact};
use trpg_runtime::{proposals_from_json, relationship_facts_to_proposals, try_proposals_from_json};

/// A relationship-triple `MemoryFact` shaped exactly like `parse_relationship_triples`
/// emits, built via serde so the test needs no chrono dependency.
fn rel_fact() -> MemoryFact {
    serde_json::from_value(json!({
        "fact_id": "mf_rel_abc",
        "session_id": "sess_a",
        "scope": {"scope_type": "session", "scope_id": "sess_a"},
        "visibility": "gm_only",
        "subject": "raul",
        "predicate": "wrote",
        "object": "letter",
        "summary": "Raul wrote the bloody letter.",
        "status": "active",
        "confidence": 0.92,
        "source_event_ids": ["de_surfaced_sess_a_raul", "de_surfaced_sess_a_letter"],
        "tags": ["relationship", "npc", "clue"],
        "importance": 2,
        "turn_id": "turn7",
        "created_at": "2026-06-18T00:00:00Z",
        "updated_at": "2026-06-18T00:00:00Z"
    }))
    .expect("valid MemoryFact json")
}

#[test]
fn relationship_triple_can_be_mapped_to_memory_proposal() {
    let proposals = relationship_facts_to_proposals(&[rel_fact()]);
    assert_eq!(proposals.len(), 1, "one triple → one proposal");
    let p = &proposals[0];
    assert_eq!(p.kind_token(), "memory_fact");
    match p {
        MemoryExtractionProposal::MemoryFact(c) => {
            assert_eq!(c.fact.subject, "raul");
            assert_eq!(c.fact.predicate, "wrote");
        }
        other => panic!("expected memory_fact proposal, got {}", other.kind_token()),
    }
}

#[test]
fn memory_proposal_preserves_source_refs() {
    let proposals = relationship_facts_to_proposals(&[rel_fact()]);
    assert_eq!(proposals.len(), 1);
    let refs = proposals[0].evidence_refs();
    assert!(refs.contains(&"de_surfaced_sess_a_raul".to_string()));
    assert!(refs.contains(&"de_surfaced_sess_a_letter".to_string()));
}

#[test]
fn memory_proposal_does_not_commit_state() {
    // The bridge has no Db handle and writes nothing: it is a pure, deterministic mapping.
    let facts = vec![rel_fact()];
    let first = relationship_facts_to_proposals(&facts);
    let second = relationship_facts_to_proposals(&facts);
    assert_eq!(first.len(), 1);
    assert_eq!(
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&second).unwrap(),
        "same input → identical proposals, no IO/commit side effects"
    );
}

#[test]
fn proposals_from_json_parses_and_drops_unsupported() {
    let raw = json!({"proposals": [
        {
            "proposal_kind": "world_fact",
            "fact_id": "f1",
            "subject": "a",
            "predicate": "relates_to",
            "object": "b",
            "source_event_ids": ["ev1"]
        },
        {
            "proposal_kind": "knowledge_update",
            "fact_id": "f1",
            "holder": {"holder_kind": "npc", "holder_id": "npc_alice"},
            "knowledge_state": "suspects",
            "source_event_ids": ["ev1"]
        },
        {"proposal_kind": "bogus_kind"}
    ]});
    // Lenient: the unsupported entry is dropped, the two valid ones survive.
    let ok = proposals_from_json(&raw);
    assert_eq!(ok.len(), 2);
    assert_eq!(ok[0].kind_token(), "world_fact");
    assert_eq!(ok[1].kind_token(), "knowledge_update");

    // Strict: an unsupported/malformed entry fails the whole batch with a typed error.
    assert!(try_proposals_from_json(&raw).is_err());

    // A payload with no proposals array yields empty (lenient) / typed error (strict).
    assert!(proposals_from_json(&json!({})).is_empty());
    assert!(try_proposals_from_json(&json!({})).is_err());
}

#[test]
fn proposals_from_json_fails_closed_on_unstable_holder() {
    // A knowledge update naming a display-name NPC holder is dropped (lenient) and
    // rejected (strict) — never committed, never invented.
    let raw = json!({"proposals": [{
        "proposal_kind": "knowledge_update",
        "fact_id": "f1",
        "holder": {"holder_kind": "npc", "holder_id": "The Butler"},
        "knowledge_state": "knows_true",
        "source_event_ids": ["ev1"]
    }]});
    assert!(proposals_from_json(&raw).is_empty());
    assert!(try_proposals_from_json(&raw).is_err());
}

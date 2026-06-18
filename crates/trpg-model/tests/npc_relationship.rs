//! TC-NPC-02 acceptance tests: NPC Relationship v1 — bounded channels, evidence-gated
//! deltas, and the two required scenario behaviors. These are the named acceptance
//! tests from the task card; fine-grained coverage lives inline in
//! `trpg_model::npc_relationship`.
use trpg_model::{
    NpcRelationship, NpcRelationshipDelta, NpcRelationshipTarget, RelationshipError,
    CHANNEL_SIGNED_MAX, CHANNEL_SIGNED_MIN, CHANNEL_UNIPOLAR_MAX, CHANNEL_UNIPOLAR_MIN,
};

fn party_rel() -> NpcRelationship {
    NpcRelationship::new("sess_npc02", "npc_lars", NpcRelationshipTarget::PlayerParty).unwrap()
}

/// Every applied delta clamps each channel to its declared band, no matter how large.
#[test]
fn relationship_delta_is_bounded() {
    let mut rel = party_rel();
    // Push every channel far past both bounds across two applications.
    NpcRelationshipDelta {
        trust: 10_000,
        respect: 10_000,
        affection: 10_000,
        debt: 10_000,
        fear: 10_000,
        suspicion: 10_000,
        hostility: 10_000,
        leverage: 10_000,
        talkativeness: 10_000,
        evidence_event_ids: vec!["evt_up".into()],
        ..Default::default()
    }
    .apply_to(&mut rel)
    .unwrap();
    assert_eq!(rel.trust, CHANNEL_SIGNED_MAX);
    assert_eq!(rel.affection, CHANNEL_SIGNED_MAX);
    assert_eq!(rel.debt, CHANNEL_SIGNED_MAX);
    assert_eq!(rel.fear, CHANNEL_UNIPOLAR_MAX);
    assert_eq!(rel.hostility, CHANNEL_UNIPOLAR_MAX);
    assert_eq!(rel.talkativeness, CHANNEL_UNIPOLAR_MAX);
    // Derived interaction_desire is itself bounded.
    assert!(rel.interaction_desire <= CHANNEL_SIGNED_MAX);
    assert!(rel.interaction_desire >= CHANNEL_SIGNED_MIN);

    NpcRelationshipDelta {
        trust: -10_000,
        respect: -10_000,
        affection: -10_000,
        debt: -10_000,
        fear: -10_000,
        suspicion: -10_000,
        hostility: -10_000,
        leverage: -10_000,
        talkativeness: -10_000,
        evidence_event_ids: vec!["evt_down".into()],
        ..Default::default()
    }
    .apply_to(&mut rel)
    .unwrap();
    assert_eq!(rel.trust, CHANNEL_SIGNED_MIN);
    assert_eq!(rel.debt, CHANNEL_SIGNED_MIN);
    assert_eq!(rel.fear, CHANNEL_UNIPOLAR_MIN, "unipolar floor is 0, never negative");
    assert_eq!(rel.hostility, CHANNEL_UNIPOLAR_MIN);
    assert_eq!(rel.talkativeness, CHANNEL_UNIPOLAR_MIN);
}

/// A delta with no evidence_event_ids is refused and mutates nothing — the hard gate.
#[test]
fn relationship_delta_requires_evidence_event() {
    let mut rel = party_rel();
    let before = rel.clone();
    let no_evidence = NpcRelationshipDelta {
        trust: 25,
        fear: 25,
        evidence_event_ids: vec![],
        ..Default::default()
    };
    let err = no_evidence.apply_to(&mut rel).unwrap_err();
    assert_eq!(err, RelationshipError::MissingEvidence);
    assert_eq!(rel, before, "a rejected delta must leave state untouched");

    // The same magnitudes apply once evidence is supplied.
    let with_evidence = NpcRelationshipDelta {
        evidence_event_ids: vec!["evt_1".into()],
        ..no_evidence
    };
    with_evidence.apply_to(&mut rel).unwrap();
    assert_eq!(rel.trust, 25);
    assert_eq!(rel.evidence_event_ids, vec!["evt_1".to_string()]);
}

/// Helping an NPC increases trust/respect/debt (all three move up here).
#[test]
fn help_increases_trust_or_debt() {
    let mut rel = party_rel();
    NpcRelationshipDelta::help(vec!["evt_helped".into()])
        .apply_to(&mut rel)
        .unwrap();
    assert!(rel.trust > 0, "helping should raise trust");
    assert!(rel.debt > 0, "helping should raise debt (NPC now feels indebted)");
    assert!(rel.respect > 0, "helping should raise respect");
    assert!(rel.evidence_event_ids.contains(&"evt_helped".to_string()));
}

/// Threatening an NPC raises fear/suspicion and lowers trust.
#[test]
fn threat_decreases_trust_and_increases_fear() {
    let mut rel = party_rel();
    // Start from a positive-trust baseline so the decrease is unambiguous.
    NpcRelationshipDelta {
        trust: 30,
        evidence_event_ids: vec!["evt_rapport".into()],
        ..Default::default()
    }
    .apply_to(&mut rel)
    .unwrap();
    let trust_before = rel.trust;

    NpcRelationshipDelta::threat(vec!["evt_threatened".into()])
        .apply_to(&mut rel)
        .unwrap();
    assert!(rel.fear > 0, "threatening should raise fear");
    assert!(rel.suspicion > 0, "threatening should raise suspicion");
    assert!(rel.trust < trust_before, "threatening should reduce trust");
}

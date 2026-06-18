//! NPC Mind View v1 acceptance (TC-NPC-03): a prompt-safe projected view for ONE
//! specific NPC combining only that NPC's durable knowledge/beliefs, the static
//! persona safe view (GM-only secrets structurally dropped), and a compact
//! relationship summary. Pure — no DB, no LLM.
//!
//! Required named tests (per the task card):
//!   - npc_mind_view_excludes_unknown_fact
//!   - npc_mind_view_includes_false_belief_as_belief
//!   - npc_speech_prompt_uses_npc_knowledge_not_gm_truth
//!   - npc_mind_view_includes_persona_and_relationship
use trpg_model::{
    KnowledgeState, NpcFactStanding, NpcKnowledgeEntry, NpcMindView, NpcProfile, NpcRelationship,
    NpcRelationshipDelta, NpcRelationshipTarget, NpcSecret, Visibility,
};

fn profile() -> NpcProfile {
    NpcProfile {
        actor_id: "npc_alice".into(),
        name: "Alice".into(),
        role: Some("innkeeper".into()),
        personality_traits: vec!["wary".into()],
        drives: vec!["protect the inn".into()],
        secrets: vec![NpcSecret {
            secret_id: "s_gm".into(),
            content: "GMONLY_alice_is_the_spy".into(),
            visibility: Visibility::GmOnly,
            source_refs: vec![],
        }],
        ..Default::default()
    }
}

/// One stored relationship toward the player party, nudged through the bounded,
/// evidence-gated apply path so the summary reflects real state.
fn relationship() -> NpcRelationship {
    let mut rel =
        NpcRelationship::new("sess1", "npc_alice", NpcRelationshipTarget::PlayerParty).unwrap();
    rel.apply_delta(&NpcRelationshipDelta::threat(vec!["evt_threat".into()]))
        .unwrap();
    rel
}

fn entry(fact_id: &str, state: KnowledgeState) -> NpcKnowledgeEntry {
    NpcKnowledgeEntry {
        fact_id: fact_id.into(),
        state,
    }
}

#[test]
fn npc_mind_view_excludes_unknown_fact() {
    // The NPC knows fact_known, but is "unknown" / merely "exposed to" the others.
    // Only knows_true + belief states are projected; weaker/unknown states are excluded.
    let entries = vec![
        entry("fact_known", KnowledgeState::KnowsTrue),
        entry("fact_unknown", KnowledgeState::Unknown),
        entry("fact_rumor", KnowledgeState::Exposed),
    ];
    let view = NpcMindView::build(
        "sess1",
        "npc_alice",
        &profile(),
        &[relationship()],
        &entries,
    )
    .unwrap();
    let ids: Vec<&str> = view.facts.iter().map(|f| f.fact_id.as_str()).collect();
    assert!(ids.contains(&"fact_known"), "known fact must appear");
    assert!(!ids.contains(&"fact_unknown"), "unknown fact must be excluded");
    assert!(!ids.contains(&"fact_rumor"), "weak/heard-about fact must be excluded");
}

#[test]
fn npc_mind_view_includes_false_belief_as_belief() {
    let entries = vec![
        entry("fact_true", KnowledgeState::KnowsTrue),
        entry("fact_false", KnowledgeState::BelievesFalse),
    ];
    let view =
        NpcMindView::build("sess1", "npc_alice", &profile(), &[], &entries).unwrap();

    let standing = |id: &str| {
        view.facts
            .iter()
            .find(|f| f.fact_id == id)
            .map(|f| f.standing)
    };
    assert_eq!(standing("fact_true"), Some(NpcFactStanding::Known));
    assert_eq!(
        standing("fact_false"),
        Some(NpcFactStanding::Belief),
        "a false belief must be marked as a belief, not known truth"
    );

    // The speech context must label beliefs as not-fact and never as known truth.
    let ctx = view.speech_context();
    assert!(ctx.contains("fact_false"));
    assert!(
        view.known_fact_ids().iter().all(|id| *id != "fact_false"),
        "a belief must never be reported as a known fact"
    );
}

#[test]
fn npc_speech_prompt_uses_npc_knowledge_not_gm_truth() {
    // The NPC knows alice_knows. A GM-only world truth (gm_secret_fact) the NPC does
    // NOT hold is never passed into the mind view, so the prompt built from the view
    // cannot contain it. The GM-only PROFILE secret is also structurally absent.
    let entries = vec![entry("alice_knows", KnowledgeState::KnowsTrue)];
    let view =
        NpcMindView::build("sess1", "npc_alice", &profile(), &[relationship()], &entries)
            .unwrap();
    let ctx = view.speech_context();

    assert!(ctx.contains("alice_knows"), "the NPC's own knowledge is present");
    assert!(
        !ctx.contains("gm_secret_fact"),
        "a GM world truth the NPC doesn't know must not appear in the speech prompt"
    );
    assert!(
        !ctx.contains("GMONLY_alice_is_the_spy"),
        "a GM-only profile secret must never reach the prompt-safe speech context"
    );
}

#[test]
fn npc_mind_view_includes_persona_and_relationship() {
    let entries = vec![entry("fact_a", KnowledgeState::KnowsTrue)];
    let view =
        NpcMindView::build("sess1", "npc_alice", &profile(), &[relationship()], &entries)
            .unwrap();

    // Persona (safe view) is present in compact form.
    assert_eq!(view.persona.actor_id, "npc_alice");
    assert_eq!(view.persona.name, "Alice");
    let ctx = view.speech_context();
    assert!(ctx.contains("Alice"), "persona name in context");
    assert!(ctx.contains("innkeeper"), "persona role in context");

    // Relationship is present in compact form (the threat raised fear; stance reflects it).
    assert_eq!(view.relationships.len(), 1);
    let summary = &view.relationships[0];
    assert_eq!(summary.target_kind, "player_party");
    assert!(summary.fear > 0, "threat raised fear in the compact summary");
    assert!(
        ctx.to_lowercase().contains("attitude") || ctx.to_lowercase().contains("toward"),
        "relationship appears in the speech context"
    );
}

/// DA-KNOW-03 (pure, no DB): the SAME fact id diverges across two NPC holders —
/// NPC A `knows_true` while NPC B `believes_false` — and each mind view reflects only
/// its own holder's state. A belief is never reported as known truth, and neither view
/// borrows the other's standing. This is the holder-specific divergence the DB/runtime
/// projections must preserve, proven at the model layer without storage.
#[test]
fn two_npc_mind_views_diverge_on_same_fact() {
    let shared = "fact_who_killed_the_mayor";
    let alice_profile = NpcProfile {
        actor_id: "npc_alice".into(),
        name: "Alice".into(),
        ..Default::default()
    };
    let bob_profile = NpcProfile {
        actor_id: "npc_bob".into(),
        name: "Bob".into(),
        ..Default::default()
    };

    // NPC A knows the shared fact as true.
    let alice = NpcMindView::build(
        "sess1",
        "npc_alice",
        &alice_profile,
        &[],
        &[entry(shared, KnowledgeState::KnowsTrue)],
    )
    .unwrap();
    // NPC B holds a FALSE belief about the very same fact id.
    let bob = NpcMindView::build(
        "sess1",
        "npc_bob",
        &bob_profile,
        &[],
        &[entry(shared, KnowledgeState::BelievesFalse)],
    )
    .unwrap();

    // Same fact id, divergent standing per holder.
    assert_eq!(alice.known_fact_ids(), vec![shared], "NPC A knows the fact as true");
    assert!(alice.belief_fact_ids().is_empty(), "NPC A holds no belief on it");
    assert_eq!(bob.belief_fact_ids(), vec![shared], "NPC B merely believes (falsely)");
    assert!(
        bob.known_fact_ids().is_empty(),
        "NPC B's false belief must never be reported as known truth"
    );
    // A belief is structurally a Belief standing, not Known — never world truth.
    let bob_standing = bob.facts.iter().find(|f| f.fact_id == shared).map(|f| f.standing);
    assert_eq!(bob_standing, Some(NpcFactStanding::Belief));
    // Neither view borrows the other's standing: B's speech context labels the fact as a
    // belief (not fact); A's reports it as known.
    assert!(bob.speech_context().to_lowercase().contains("belief"));
}

#[test]
fn mind_view_fails_closed_on_unstable_or_mismatched_id() {
    // Display-name / placeholder NPC id is rejected.
    assert!(NpcMindView::build("s", "The Innkeeper", &profile(), &[], &[]).is_err());
    // A profile anchored to a DIFFERENT npc must not be projected under this id.
    assert!(
        NpcMindView::build("s", "npc_bob", &profile(), &[], &[]).is_err(),
        "profile actor_id must match the requested NPC id"
    );
}

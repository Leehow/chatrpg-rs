//! P4.3 — `NpcBehaviorPlan::to_reaction_candidate` pure-projection guards.
//!
//! These assert the integer→f64/100 normalization and, critically, the §二十四-#4
//! disclosure guard: a fact the NPC must WITHHOLD can never enter the World reaction
//! candidate's `knowledge_basis`.
use trpg_model::{
    KnowledgeState, NpcBehaviorContext, NpcBehaviorPlan, NpcKnowledgeEntry, NpcMindView,
    NpcProfile, NpcRelationship, NpcRelationshipDelta, NpcRelationshipTarget, RelationshipStance,
};

fn profile() -> NpcProfile {
    NpcProfile {
        actor_id: "npc_lars".into(),
        name: "Lars".into(),
        goals: vec!["protect the workshop".into()],
        behavioral_boundaries: vec!["never harm a child".into()],
        ..Default::default()
    }
}

fn rel(trust: i16, fear: i16, hostility: i16) -> NpcRelationship {
    let mut r = NpcRelationship::new("s", "npc_lars", NpcRelationshipTarget::PlayerParty).unwrap();
    r.apply_delta(&NpcRelationshipDelta {
        trust,
        fear,
        hostility,
        evidence_event_ids: vec!["e".into()],
        ..Default::default()
    })
    .unwrap();
    r
}

fn knowledge(ids: &[&str]) -> Vec<NpcKnowledgeEntry> {
    ids.iter()
        .map(|id| NpcKnowledgeEntry {
            fact_id: (*id).to_string(),
            state: KnowledgeState::KnowsTrue,
        })
        .collect()
}

fn view(rel: NpcRelationship, entries: &[NpcKnowledgeEntry]) -> NpcMindView {
    NpcMindView::build("s", "npc_lars", &profile(), &[rel], entries).unwrap()
}

#[test]
fn reaction_candidate_normalizes_scores() {
    // Table-driven over a few derived plans: urgency/feasibility/risk = the documented
    // integer→f64/100 projections of the derived plan fields.
    for (t, f, h) in [(0, 0, 0), (80, 0, 0), (0, 80, 80), (40, 20, 60)] {
        let plan =
            NpcBehaviorPlan::derive(&view(rel(t, f, h), &[]), &NpcBehaviorContext::default());
        let cand = plan.to_reaction_candidate();
        assert_eq!(cand.npc_id, plan.npc_id);
        assert_eq!(cand.urgency, f64::from(plan.interaction_desire) / 100.0);
        assert_eq!(
            cand.feasibility,
            f64::from(plan.willingness_to_help.min(plan.risk_tolerance)) / 100.0
        );
        assert_eq!(cand.risk, f64::from(plan.risk_tolerance) / 100.0);
        assert!(cand.action_intent.is_none());
    }
}

#[test]
fn reaction_candidate_carries_stance_and_emotion_words() {
    let plan = NpcBehaviorPlan::derive(&view(rel(0, 0, 70), &[]), &NpcBehaviorContext::default());
    let cand = plan.to_reaction_candidate();
    assert_eq!(cand.stance, "hostile");
    assert_eq!(cand.emotional_state, "hostile");
    // Sanity: the derived stance really is hostile (the words mirror the enum).
    assert_eq!(plan.stance, RelationshipStance::Hostile);
}

#[test]
fn reaction_candidate_knowledge_basis_is_revealable_only() {
    // Non-secret known facts ⇒ all revealable ⇒ knowledge_basis mirrors them.
    let plan = NpcBehaviorPlan::derive(
        &view(rel(0, 0, 0), &knowledge(&["fact_open"])),
        &NpcBehaviorContext::default(),
    );
    let cand = plan.to_reaction_candidate();
    assert_eq!(cand.knowledge_basis, vec!["fact_open".to_string()]);
    assert_eq!(cand.knowledge_basis, plan.facts_can_reveal);
}

// §二十四-#4 GUARD: a withheld secret must NEVER leak into the World candidate's
// knowledge_basis. With a known secret below the reveal threshold, the fact lands in
// facts_will_withhold; the projection must source ONLY facts_can_reveal.
#[test]
fn reaction_candidate_never_leaks_withheld_facts() {
    let ctx = NpcBehaviorContext {
        // Hostile relationship + secret ⇒ reveal willingness stays below threshold.
        secret_fact_ids: vec!["secret_loc".into()],
        ..Default::default()
    };
    let plan = NpcBehaviorPlan::derive(
        &view(rel(0, 0, 80), &knowledge(&["secret_loc", "open_fact"])),
        &ctx,
    );
    // Precondition: the secret really is being withheld this turn.
    assert!(
        plan.facts_will_withhold.contains(&"secret_loc".to_string()),
        "test setup must actually withhold the secret"
    );
    let cand = plan.to_reaction_candidate();
    assert!(
        !cand.knowledge_basis.contains(&"secret_loc".to_string()),
        "withheld fact leaked into World reaction knowledge_basis (§24-#4)"
    );
    for withheld in &plan.facts_will_withhold {
        assert!(
            !cand.knowledge_basis.contains(withheld),
            "withheld fact {withheld} must not appear in knowledge_basis"
        );
    }
    // The basis equals exactly the revealable set.
    assert_eq!(cand.knowledge_basis, plan.facts_can_reveal);
}

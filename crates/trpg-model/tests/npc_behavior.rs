//! TC-NPC-04 acceptance tests for the deterministic NPC behavior planner.
//!
//! These exercise the pure model API only: derive an `NpcBehaviorPlan` from an
//! already-projected `NpcMindView` plus a non-truth `NpcBehaviorContext`. The plan is
//! a proposal/guidance object — it never mutates state, never invents truth the NPC
//! does not hold, and stays bounded + deterministic.
use trpg_model::{
    KnowledgeState, NpcBehaviorContext, NpcBehaviorPlan, NpcKnowledgeEntry, NpcMindView,
    NpcProfile, NpcRelationship, NpcRelationshipDelta, NpcRelationshipTarget,
};

fn profile() -> NpcProfile {
    NpcProfile {
        actor_id: "npc_lars".into(),
        name: "Lars".into(),
        role: Some("mechanic".into()),
        goals: vec!["keep the workshop running".into()],
        ..Default::default()
    }
}

/// A relationship toward the player party with channels set directly (bounded by the
/// model). Carries evidence so the delta applies through the normal gated path.
fn rel_with(trust: i16, fear: i16, hostility: i16) -> NpcRelationship {
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

fn view_with(rel: NpcRelationship, entries: &[NpcKnowledgeEntry]) -> NpcMindView {
    NpcMindView::build("s", "npc_lars", &profile(), &[rel], entries).unwrap()
}

fn entry(id: &str, state: KnowledgeState) -> NpcKnowledgeEntry {
    NpcKnowledgeEntry {
        fact_id: id.into(),
        state,
    }
}

#[test]
fn hostile_npc_has_lower_help_willingness() {
    let hostile = NpcBehaviorPlan::derive(&view_with(rel_with(0, 0, 80), &[]), &Default::default());
    let neutral = NpcBehaviorPlan::derive(&view_with(rel_with(0, 0, 0), &[]), &Default::default());
    assert!(
        hostile.willingness_to_help < neutral.willingness_to_help,
        "hostility must lower help willingness: hostile={} neutral={}",
        hostile.willingness_to_help,
        neutral.willingness_to_help
    );
}

#[test]
fn fearful_npc_has_lower_reveal_willingness() {
    // Default context: fear does NOT make compliance plausible, so fear clams the NPC up.
    let fearful = NpcBehaviorPlan::derive(&view_with(rel_with(0, 80, 0), &[]), &Default::default());
    let calm = NpcBehaviorPlan::derive(&view_with(rel_with(0, 0, 0), &[]), &Default::default());
    assert!(
        fearful.willingness_to_reveal_secret < calm.willingness_to_reveal_secret,
        "fear must lower reveal willingness by default: fearful={} calm={}",
        fearful.willingness_to_reveal_secret,
        calm.willingness_to_reveal_secret
    );
}

#[test]
fn fear_can_raise_reveal_when_compliance_plausible() {
    // The "unless fear makes compliance plausible" branch: under coercion, fear raises
    // reveal willingness instead of lowering it.
    let ctx = NpcBehaviorContext {
        fear_drives_compliance: true,
        ..Default::default()
    };
    let coerced = NpcBehaviorPlan::derive(&view_with(rel_with(0, 80, 0), &[]), &ctx);
    let calm = NpcBehaviorPlan::derive(&view_with(rel_with(0, 0, 0), &[]), &Default::default());
    assert!(
        coerced.willingness_to_reveal_secret > calm.willingness_to_reveal_secret,
        "fear under compliance pressure must raise reveal: coerced={} calm={}",
        coerced.willingness_to_reveal_secret,
        calm.willingness_to_reveal_secret
    );
}

#[test]
fn trusting_npc_has_higher_cooperation() {
    let trusting =
        NpcBehaviorPlan::derive(&view_with(rel_with(80, 0, 0), &[]), &Default::default());
    let neutral = NpcBehaviorPlan::derive(&view_with(rel_with(0, 0, 0), &[]), &Default::default());
    assert!(
        trusting.willingness_to_help > neutral.willingness_to_help,
        "trust must raise cooperation: trusting={} neutral={}",
        trusting.willingness_to_help,
        neutral.willingness_to_help
    );
}

#[test]
fn npc_cannot_reveal_unknown_fact() {
    // The NPC only holds a FALSE belief ("rumor") and knows nothing as true. The
    // disclosure policy even names a fact id the NPC has never encountered.
    let entries = vec![entry("rumor", KnowledgeState::BelievesFalse)];
    let ctx = NpcBehaviorContext {
        secret_fact_ids: vec!["ghost_fact".into()],
        ..Default::default()
    };
    // Max trust so willingness is high — proving the gate is knowledge, not willingness.
    let plan = NpcBehaviorPlan::derive(&view_with(rel_with(100, 0, 0), &entries), &ctx);

    assert!(
        !plan.facts_knows.iter().any(|f| f == "rumor"),
        "a belief is not known truth"
    );
    assert!(
        !plan.facts_can_reveal.iter().any(|f| f == "rumor"),
        "a mere belief must not be revealable as truth"
    );
    assert!(
        !plan.facts_can_reveal.iter().any(|f| f == "ghost_fact"),
        "a fact the NPC never knew must never be revealable"
    );
    assert!(
        plan.facts_can_reveal.is_empty(),
        "no known true facts => nothing to reveal: {:?}",
        plan.facts_can_reveal
    );
}

#[test]
fn npc_can_withhold_known_secret() {
    let entries = vec![
        entry("open_fact", KnowledgeState::KnowsTrue),
        entry("secret_fact", KnowledgeState::KnowsTrue),
    ];
    let ctx = NpcBehaviorContext {
        secret_fact_ids: vec!["secret_fact".into()],
        ..Default::default()
    };
    // Neutral relationship => low reveal willingness => the secret stays withheld.
    let plan = NpcBehaviorPlan::derive(&view_with(rel_with(0, 0, 0), &entries), &ctx);

    assert!(
        plan.facts_knows.iter().any(|f| f == "secret_fact"),
        "the NPC does know the secret"
    );
    assert!(
        plan.facts_will_withhold.iter().any(|f| f == "secret_fact"),
        "a known secret with low willingness must be withheld"
    );
    assert!(
        !plan.facts_can_reveal.iter().any(|f| f == "secret_fact"),
        "a withheld secret must not also be revealable"
    );
    assert!(
        plan.facts_can_reveal.iter().any(|f| f == "open_fact"),
        "a non-secret known fact is freely revealable"
    );
}

#[test]
fn relationship_can_unlock_a_known_secret() {
    // Conservative-but-not-absolute: strong trust raises reveal willingness past the
    // threshold, so relationship logic CAN move a known secret to revealable.
    let entries = vec![entry("secret_fact", KnowledgeState::KnowsTrue)];
    let ctx = NpcBehaviorContext {
        secret_fact_ids: vec!["secret_fact".into()],
        ..Default::default()
    };
    let plan = NpcBehaviorPlan::derive(&view_with(rel_with(100, 0, 0), &entries), &ctx);
    assert!(
        plan.facts_can_reveal.iter().any(|f| f == "secret_fact"),
        "high trust should unlock the secret: reveal_willingness={}",
        plan.willingness_to_reveal_secret
    );
    assert!(!plan.facts_will_withhold.iter().any(|f| f == "secret_fact"));
}

#[test]
fn guidance_block_is_id_stable_and_carries_safe_steering() {
    // The rendered block is wrapped in the stable id and surfaces stance, willingness,
    // and the reveal/withhold fact-id sets — the safe steering a GM narration path needs.
    let entries = vec![
        entry("open_fact", KnowledgeState::KnowsTrue),
        entry("secret_fact", KnowledgeState::KnowsTrue),
    ];
    let ctx = NpcBehaviorContext {
        secret_fact_ids: vec!["secret_fact".into()],
        ..Default::default()
    };
    let plan = NpcBehaviorPlan::derive(&view_with(rel_with(0, 0, 80), &entries), &ctx);
    let block = plan.to_guidance_block();
    assert!(block.starts_with("[npc_behavior_guidance npc=npc_lars]"));
    assert!(block.trim_end().ends_with("[/npc_behavior_guidance]"));
    assert!(block.contains("Stance toward focus: hostile"));
    assert!(block.contains("May reveal fact ids: open_fact"));
    assert!(block.contains("Withhold fact ids: secret_fact"));
}

#[test]
fn guidance_block_omits_unknown_fact_and_belief_ids() {
    // A fact the NPC never knew (named only in the secret set) and a mere belief must
    // not surface in the rendered guidance — only known-true ids can appear.
    let entries = vec![entry("rumor", KnowledgeState::BelievesFalse)];
    let ctx = NpcBehaviorContext {
        secret_fact_ids: vec!["ghost_fact".into()],
        ..Default::default()
    };
    let plan = NpcBehaviorPlan::derive(&view_with(rel_with(100, 0, 0), &entries), &ctx);
    let block = plan.to_guidance_block();
    assert!(
        !block.contains("ghost_fact"),
        "unknown fact id must not enter guidance: {block}"
    );
    assert!(
        !block.contains("rumor"),
        "a belief id must not enter guidance: {block}"
    );
    assert!(!block.contains("May reveal fact ids"));
    assert!(!block.contains("Withhold fact ids"));
}

#[test]
fn plan_is_pure_proposal_and_deterministic() {
    // The plan is guidance, not committed state: deriving must be a pure function of its
    // inputs (identical inputs => identical output) and must not mutate the source view.
    let view = view_with(
        rel_with(40, 10, 5),
        &[entry("k", KnowledgeState::KnowsTrue)],
    );
    let ctx = NpcBehaviorContext::default();
    let snapshot = view.clone();
    let a = NpcBehaviorPlan::derive(&view, &ctx);
    let b = NpcBehaviorPlan::derive(&view, &ctx);
    assert_eq!(a, b, "derivation must be deterministic");
    assert_eq!(view, snapshot, "derivation must not mutate the source view");
    assert_eq!(a.npc_id, "npc_lars");
    // Willingness/risk values stay bounded to [0, 100].
    for v in [
        a.willingness_to_help,
        a.willingness_to_lie,
        a.willingness_to_fight,
        a.willingness_to_reveal_secret,
        a.risk_tolerance,
    ] {
        assert!((0..=100).contains(&v), "value out of band: {v}");
    }
}

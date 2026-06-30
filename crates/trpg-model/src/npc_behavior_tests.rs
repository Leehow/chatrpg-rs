use super::*;
use crate::{
    KnowledgeState, NpcKnowledgeEntry, NpcMindView, NpcProfile, NpcRelationship,
    NpcRelationshipDelta, NpcRelationshipTarget,
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

fn view(rel: NpcRelationship, entries: &[NpcKnowledgeEntry]) -> NpcMindView {
    NpcMindView::build("s", "npc_lars", &profile(), &[rel], entries).unwrap()
}

#[test]
fn emotion_tracks_dominant_channel() {
    assert_eq!(derive_emotion(0, 0, 70), NpcEmotionalState::Hostile);
    assert_eq!(derive_emotion(0, 70, 0), NpcEmotionalState::Afraid);
    assert_eq!(derive_emotion(60, 0, 0), NpcEmotionalState::Warm);
    assert_eq!(derive_emotion(0, 0, 0), NpcEmotionalState::Neutral);
}

#[test]
fn current_goal_falls_back_to_persona_then_override() {
    let plan = NpcBehaviorPlan::derive(&view(rel(0, 0, 0), &[]), &NpcBehaviorContext::default());
    assert_eq!(plan.current_goal.as_deref(), Some("protect the workshop"));
    let ctx = NpcBehaviorContext {
        current_goal: Some("flee the city".into()),
        ..Default::default()
    };
    let plan = NpcBehaviorPlan::derive(&view(rel(0, 0, 0), &[]), &ctx);
    assert_eq!(plan.current_goal.as_deref(), Some("flee the city"));
}

#[test]
fn persona_boundaries_become_forbidden_actions() {
    let plan = NpcBehaviorPlan::derive(&view(rel(0, 0, 0), &[]), &NpcBehaviorContext::default());
    assert!(plan
        .forbidden_actions
        .iter()
        .any(|a| a == "never harm a child"));
}

#[test]
fn no_focus_relationship_yields_neutral_defaults() {
    // A faction-focused plan with no faction relationship stored falls back to neutral.
    let ctx = NpcBehaviorContext {
        focus_target: Some(FocusTarget {
            target_kind: "faction".into(),
            target_id: "faction_x".into(),
        }),
        ..Default::default()
    };
    let plan = NpcBehaviorPlan::derive(&view(rel(80, 0, 0), &[]), &ctx);
    assert_eq!(plan.stance, RelationshipStance::Neutral);
    assert_eq!(plan.interaction_desire, 0);
}

#[test]
fn source_event_ids_pass_through_as_provenance() {
    let ctx = NpcBehaviorContext {
        source_event_ids: vec!["evt_1".into(), "evt_2".into()],
        ..Default::default()
    };
    let entries = vec![NpcKnowledgeEntry {
        fact_id: "k".into(),
        state: KnowledgeState::KnowsTrue,
    }];
    let plan = NpcBehaviorPlan::derive(&view(rel(0, 0, 0), &entries), &ctx);
    assert_eq!(plan.source_event_ids, vec!["evt_1", "evt_2"]);
}
// ===== MAT.M8: source-grounded persona prose reaches the guidance block =====

fn profile_with_body(desc: Option<&str>) -> NpcProfile {
    NpcProfile {
        actor_id: "npc_lars".into(),
        name: "Lars".into(),
        persona_description: desc.map(str::to_string),
        ..Default::default()
    }
}

fn view_with(profile: &NpcProfile, rel: NpcRelationship) -> NpcMindView {
    NpcMindView::build("s", "npc_lars", profile, &[rel], &[]).unwrap()
}

#[test]
fn persona_description_surfaces_in_guidance_block() {
    // TDD #3: a body-carrying profile yields NON-empty persona guidance (the thing that
    // was empty before M8 — the no-invention guard kept such NPCs silent).
    let p = profile_with_body(Some("白天通常待在埃索加油站的三人之一。"));
    let plan =
        NpcBehaviorPlan::derive(&view_with(&p, rel(0, 0, 0)), &NpcBehaviorContext::default());
    assert_eq!(
        plan.persona_description.as_deref(),
        Some("白天通常待在埃索加油站的三人之一。")
    );
    let block = plan.to_guidance_block();
    assert!(
        block.contains("白天通常待在埃索加油站的三人之一。"),
        "guidance block must carry the source persona prose: {block}"
    );
}

#[test]
fn absent_persona_description_omits_line_baseline() {
    // A profile with no source body produces no persona line — byte-stable pre-M8 baseline.
    let p = profile_with_body(None);
    let plan =
        NpcBehaviorPlan::derive(&view_with(&p, rel(0, 0, 0)), &NpcBehaviorContext::default());
    assert!(plan.persona_description.is_none());
    assert!(
        !plan.to_guidance_block().contains("Persona (source)"),
        "no persona line when there is no source body"
    );
}

#[test]
fn persona_description_does_not_enter_knowledge_basis() {
    // M4 invariant: persona prose is descriptive, not a fact channel. The World
    // reaction candidate's knowledge_basis stays sourced ONLY from facts_can_reveal.
    let p = profile_with_body(Some("一些人设散文。"));
    let entries = vec![NpcKnowledgeEntry {
        fact_id: "k1".into(),
        state: KnowledgeState::KnowsTrue,
    }];
    let view = NpcMindView::build("s", "npc_lars", &p, &[rel(0, 0, 0)], &entries).unwrap();
    let plan = NpcBehaviorPlan::derive(&view, &NpcBehaviorContext::default());
    let candidate = plan.to_reaction_candidate();
    assert_eq!(candidate.knowledge_basis, plan.facts_can_reveal);
    assert!(!candidate
        .knowledge_basis
        .iter()
        .any(|b| b.contains("人设散文")));
}

// P4.3 `to_reaction_candidate` projection tests (incl. the §24-#4 withheld-fact
// leak guard) live in the sibling integration test `tests/reaction_candidate.rs`
// to keep this file lean (it predates the ≤400-line budget).

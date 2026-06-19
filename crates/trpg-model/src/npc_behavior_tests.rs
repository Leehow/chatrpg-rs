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
    let mut r =
        NpcRelationship::new("s", "npc_lars", NpcRelationshipTarget::PlayerParty).unwrap();
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
    let plan =
        NpcBehaviorPlan::derive(&view(rel(0, 0, 0), &[]), &NpcBehaviorContext::default());
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
    let plan =
        NpcBehaviorPlan::derive(&view(rel(0, 0, 0), &[]), &NpcBehaviorContext::default());
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
// P4.3 `to_reaction_candidate` projection tests (incl. the §24-#4 withheld-fact
// leak guard) live in the sibling integration test `tests/reaction_candidate.rs`
// to keep this file lean (it predates the ≤400-line budget).

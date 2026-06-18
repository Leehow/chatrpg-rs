//! Provider-free connected NPC journey validation.
//!
//! This test keeps the journey evidence in harness/scenarios/npc_connected and
//! binds the behavioral checks to the real NPC model/runtime pure APIs. It does
//! not spawn the CLI, call an LLM, or touch a database.

use std::path::PathBuf;

use serde_json::Value;
use trpg_harness::{
    human_player_input_findings, verify_fixture_plan, ExecutionMode, Fixture, ScenarioIdentity,
};
use trpg_model::{
    KnowledgeState, NpcBehaviorContext, NpcKnowledgeEntry, NpcMindView, NpcProfile,
    NpcRelationship, NpcRelationshipDelta, NpcRelationshipTarget, NpcSecret,
    RelationshipStance, SpeechStyle, Visibility,
};
use trpg_runtime::derive_npc_behavior_plan;

const GM_ONLY_SECRET_TEXT: &str = "GM_ONLY_SAFEHOUSE_COORDINATES";

fn scenario_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../harness/scenarios/npc_connected")
        .canonicalize()
        .expect("npc connected scenario directory must exist")
}

fn read(name: &str) -> String {
    std::fs::read_to_string(scenario_dir().join(name))
        .unwrap_or_else(|e| panic!("failed to read {name}: {e}"))
}

fn string_array(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("expected array")
        .iter()
        .map(|v| v.as_str().expect("expected string").to_string())
        .collect()
}

fn contains(items: &[String], needle: &str) -> bool {
    items.iter().any(|item| item == needle)
}

fn profile(npc_id: &str) -> NpcProfile {
    NpcProfile {
        actor_id: npc_id.to_string(),
        name: "Mara".into(),
        role: Some("market mechanic".into()),
        personality_traits: vec!["guarded".into(), "practical".into()],
        goals: vec!["keep the night market powered".into()],
        secrets: vec![
            NpcSecret {
                secret_id: "gm_only_safehouse".into(),
                content: GM_ONLY_SECRET_TEXT.into(),
                visibility: Visibility::GmOnly,
                source_refs: vec![],
            },
            NpcSecret {
                secret_id: "public_reputation".into(),
                content: "Mara fixes generators for the market".into(),
                visibility: Visibility::PlayerVisible,
                source_refs: vec![],
            },
        ],
        speech_style: SpeechStyle {
            directness: Some("plain".into()),
            emotionality: Some("restrained unless trust is earned".into()),
            taboo_topics: vec!["crew politics".into()],
            ..Default::default()
        },
        behavioral_boundaries: vec!["do not endanger bystanders".into()],
        ..Default::default()
    }
}

#[test]
fn journey_npc_connected_reaches_provider_free_pass() {
    let contract: Value = serde_json::from_str(&read("npc-connected-journey.json"))
        .expect("scenario contract must be valid JSON");
    let replay_value: Value = serde_json::from_str(&read("npc-connected-journey.replay.json"))
        .expect("replay cassette must be valid JSON");

    let identity = ScenarioIdentity::new(
        contract["scenario_id"].as_str().unwrap(),
        contract["scenario_version"].as_str().unwrap(),
    );
    let turn_inputs: Vec<String> = contract["turns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|turn| turn["user_input"].as_str().unwrap().to_string())
        .collect();

    let fixture: Fixture =
        serde_json::from_value(replay_value.clone()).expect("replay must deserialize");
    let mismatches = verify_fixture_plan(&fixture, &identity, &turn_inputs, ExecutionMode::Replay);
    assert!(
        mismatches.is_empty(),
        "replay cassette must match with no live fallback: {:?}",
        mismatches.iter().map(|m| m.message()).collect::<Vec<_>>()
    );

    for input in &turn_inputs {
        let findings = human_player_input_findings(input, false);
        assert!(findings.is_empty(), "unnatural input `{input}`: {findings:?}");
    }

    let npc = &replay_value["npc_journey"];
    let session_id = npc["session_id"].as_str().unwrap();
    let npc_id = npc["npc_id"].as_str().unwrap();
    let known_fact = npc["known_fact_ids"][0].as_str().unwrap();
    let hidden_fact = npc["secret_fact_ids"][0].as_str().unwrap();
    let unknown_fact = npc["unknown_fact_id"].as_str().unwrap();
    let evidence_event_ids = string_array(&npc["relationship_evidence_event_ids"]);

    let profile = profile(npc_id);
    let safe_profile_json = serde_json::to_string(&profile.safe_view()).unwrap();
    assert!(!safe_profile_json.contains(GM_ONLY_SECRET_TEXT));

    let entries = vec![
        NpcKnowledgeEntry {
            fact_id: known_fact.into(),
            state: KnowledgeState::KnowsTrue,
        },
        NpcKnowledgeEntry {
            fact_id: hidden_fact.into(),
            state: KnowledgeState::KnowsTrue,
        },
        NpcKnowledgeEntry {
            fact_id: "fact_wrong_rumor".into(),
            state: KnowledgeState::BelievesFalse,
        },
        NpcKnowledgeEntry {
            fact_id: "fact_seen_noise".into(),
            state: KnowledgeState::Exposed,
        },
    ];

    let baseline_rel =
        NpcRelationship::new(session_id, npc_id, NpcRelationshipTarget::PlayerParty).unwrap();
    let baseline_view =
        NpcMindView::build(session_id, npc_id, &profile, &[baseline_rel.clone()], &entries)
            .unwrap();
    let baseline_plan = derive_npc_behavior_plan(
        &baseline_view,
        &NpcBehaviorContext {
            secret_fact_ids: vec![hidden_fact.into(), unknown_fact.into()],
            source_event_ids: vec!["evt_meet_mara".into()],
            ..Default::default()
        },
    );

    let mut updated_rel = baseline_rel.clone();
    let delta = NpcRelationshipDelta {
        trust: 32,
        respect: 20,
        affection: 20,
        talkativeness: 10,
        last_interaction_turn_id: Some("turn_help_generator".into()),
        evidence_event_ids: evidence_event_ids.clone(),
        ..Default::default()
    };
    updated_rel.apply_delta(&delta).unwrap();

    let updated_view =
        NpcMindView::build(session_id, npc_id, &profile, &[updated_rel.clone()], &entries)
            .unwrap();
    let updated_plan = derive_npc_behavior_plan(
        &updated_view,
        &NpcBehaviorContext {
            secret_fact_ids: vec![hidden_fact.into(), unknown_fact.into()],
            source_event_ids: evidence_event_ids.clone(),
            ..Default::default()
        },
    );

    assert_eq!(baseline_plan.stance, RelationshipStance::Neutral);
    assert!(matches!(
        updated_plan.stance,
        RelationshipStance::Cordial | RelationshipStance::Friendly | RelationshipStance::Allied
    ));
    assert!(updated_rel.trust > baseline_rel.trust);
    assert!(updated_rel.interaction_desire > baseline_rel.interaction_desire);
    assert_eq!(updated_rel.evidence_event_ids, evidence_event_ids);
    assert!(updated_plan.willingness_to_help > baseline_plan.willingness_to_help);
    assert_ne!(updated_plan.dialogue_guidance, baseline_plan.dialogue_guidance);
    assert_eq!(updated_plan.source_event_ids, evidence_event_ids);

    assert!(contains(&updated_plan.facts_can_reveal, known_fact));
    assert!(contains(&updated_plan.facts_will_withhold, hidden_fact));
    assert!(!contains(&updated_plan.facts_can_reveal, hidden_fact));
    assert!(!contains(&updated_plan.facts_can_reveal, unknown_fact));
    assert!(!contains(&updated_plan.facts_will_withhold, unknown_fact));

    let guidance = updated_plan.to_guidance_block();
    assert!(!guidance.contains(GM_ONLY_SECRET_TEXT));
    assert!(!guidance.contains(unknown_fact));

    let baseline_dialogue = npc["baseline_dialogue"].as_str().unwrap();
    let later_dialogue = npc["later_dialogue"].as_str().unwrap();
    assert_ne!(baseline_dialogue, later_dialogue);
    assert!(later_dialogue.contains("愿意") || later_dialogue.contains("道谢"));
    let visible_fixture_text = format!("{baseline_dialogue}\n{later_dialogue}\n{guidance}");
    assert!(!visible_fixture_text.contains(GM_ONLY_SECRET_TEXT));

    let reloaded_rel: NpcRelationship =
        serde_json::from_str(&serde_json::to_string(&updated_rel).unwrap()).unwrap();
    assert_eq!(reloaded_rel, updated_rel);
    assert_eq!(
        string_array(&npc["reload"]["relationship_evidence_event_ids"]),
        updated_rel.evidence_event_ids
    );
    assert!(contains(&string_array(&npc["reload"]["known_fact_ids"]), known_fact));
}

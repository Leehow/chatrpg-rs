//! C1 regression guards: ScenarioNode.scene_mechanics must leave legacy
//! module-graph JSON deserializing exactly as before, and the §3 intent
//! types (SceneMechanicIntent / EffectPolicy / EffectPatchIntent) must
//! round-trip losslessly, including the fail-closed Other downgrade.
//!
//! `fixtures/scene_node_legacy.json` is a real-database dump (one scene
//! node from parsed_bundles module_graph, module call_of_cthulhu_7e.document)
//! taken before any deep-extract produced scene_mechanics — it contains no
//! `scene_mechanics` key.

use trpg_model::{EffectPatchIntent, ScenarioNode, SceneMechanicIntent};

const LEGACY_NODE: &str = include_str!("fixtures/scene_node_legacy.json");

#[test]
fn legacy_scenario_node_deserializes_with_empty_scene_mechanics() {
    let fixture: serde_json::Value =
        serde_json::from_str(LEGACY_NODE).expect("fixture is valid JSON");
    assert!(
        fixture.get("scene_mechanics").is_none(),
        "fixture must stay a legacy node (no scene_mechanics key)"
    );

    let node: ScenarioNode =
        serde_json::from_str(LEGACY_NODE).expect("legacy node JSON must deserialize unchanged");
    assert!(
        node.scene_mechanics.is_empty(),
        "missing scene_mechanics key defaults to an empty vec"
    );
}

#[test]
fn legacy_module_graph_roundtrip_is_lossless() {
    let fixture: serde_json::Value =
        serde_json::from_str(LEGACY_NODE).expect("fixture is valid JSON");
    let node: ScenarioNode = serde_json::from_str(LEGACY_NODE).expect("legacy node deserializes");
    let reserialized = serde_json::to_value(&node).expect("node reserializes");

    assert_eq!(reserialized["node_id"], fixture["node_id"]);
    assert_eq!(reserialized["title"], fixture["title"]);
    assert_eq!(
        reserialized["links"].as_array().expect("links array").len(),
        fixture["links"].as_array().expect("fixture links").len(),
        "link count survives the round-trip"
    );
    assert_eq!(
        reserialized["referenced_npc_ids"], fixture["referenced_npc_ids"],
        "referenced_npc_ids survive the round-trip"
    );
}

#[test]
fn effect_patch_intent_five_forms_roundtrip() {
    let forms = vec![
        EffectPatchIntent::SetObjectState {
            object_id: "cable_junction".into(),
            patch: serde_json::json!({"state": "cut"}),
        },
        EffectPatchIntent::ModifyTrack {
            owner_kind: "actor".into(),
            owner_id: None,
            track_id: "hit_points".into(),
            op: "subtract".into(),
            amount: 3,
        },
        EffectPatchIntent::CreateFact {
            target: "scene".into(),
            fact: serde_json::json!({"text": "the alarm is ringing"}),
        },
        EffectPatchIntent::StartCountdown {
            label: "reinforcements".into(),
            amount: 10,
            scale: "minutes".into(),
            payload: serde_json::json!({"event": "lawmen_arrive"}),
        },
        EffectPatchIntent::Other(serde_json::json!({"kind": "custom", "x": 1})),
    ];

    let round_tripped: Vec<EffectPatchIntent> = forms
        .iter()
        .map(|f| {
            let v = serde_json::to_value(f).expect("intent serializes");
            serde_json::from_value(v).expect("intent deserializes back")
        })
        .collect();

    assert!(matches!(
        &round_tripped[0],
        EffectPatchIntent::SetObjectState { object_id, patch }
            if object_id == "cable_junction" && patch["state"] == "cut"
    ));
    assert!(matches!(
        &round_tripped[1],
        EffectPatchIntent::ModifyTrack { owner_kind, owner_id, track_id, op, amount }
            if owner_kind == "actor"
                && owner_id.is_none()
                && track_id == "hit_points"
                && op == "subtract"
                && *amount == 3
    ));
    assert!(matches!(
        &round_tripped[2],
        EffectPatchIntent::CreateFact { target, fact }
            if target == "scene" && fact["text"] == "the alarm is ringing"
    ));
    assert!(matches!(
        &round_tripped[3],
        EffectPatchIntent::StartCountdown { label, amount, scale, payload }
            if label == "reinforcements"
                && *amount == 10
                && scale == "minutes"
                && payload["event"] == "lawmen_arrive"
    ));
    assert!(matches!(
        &round_tripped[4],
        EffectPatchIntent::Other(v) if v["kind"] == "custom" && v["x"] == 1
    ));
}

#[test]
fn effect_patch_intent_unknown_kind_falls_to_other_losslessly() {
    let raw = serde_json::json!({"kind": "summon_demon", "x": 1});
    let parsed: EffectPatchIntent =
        serde_json::from_value(raw.clone()).expect("unknown kind must still parse");
    match &parsed {
        EffectPatchIntent::Other(v) => {
            assert_eq!(
                v.to_string(),
                raw.to_string(),
                "downgraded payload preserves the original JSON byte-for-byte"
            );
        }
        other => panic!("expected Other downgrade, got {other:?}"),
    }
}

#[test]
fn scene_mechanic_intent_defaults() {
    let raw = serde_json::json!({
        "intent_id": "homecoming.lawmen.cut_cable_force",
        "description": "force the cable with raw muscle",
        "tested_parameter": "brawling",
        "source_anchor": "If the crew tries to force the cable..."
    });
    let intent: SceneMechanicIntent =
        serde_json::from_value(raw).expect("minimal intent deserializes");
    assert_eq!(intent.intent_id, "homecoming.lawmen.cut_cable_force");
    assert!(intent.difficulty.is_none(), "difficulty defaults to None");
    assert!(
        intent.effect_policy.on_success.is_empty(),
        "effect_policy.on_success defaults to empty"
    );
    assert!(
        intent.effect_policy.on_failure.is_empty(),
        "effect_policy.on_failure defaults to empty"
    );
}

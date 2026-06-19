//! A1 regression guards: the *only* field-level changes of this phase
//! (RuleKernel.mechanics_catalog / ScenarioNode.scene_mechanics) must leave
//! pre-existing kernel/graph JSON deserializing exactly as before.
//!
//! `fixtures/coc_kernel_pre_mechanics.json` is a real-database dump
//! (rule_kernels.content_json, active kernel) taken before any mechanics
//! compile pass ran — it contains no `mechanics_catalog` key.

use trpg_model::{RuleKernel, ScenarioNode};

#[test]
fn old_coc_kernel_json_deserializes_unchanged() {
    let raw = include_str!("fixtures/coc_kernel_pre_mechanics.json");
    let fixture: serde_json::Value = serde_json::from_str(raw).expect("fixture is valid JSON");
    assert!(
        fixture.get("mechanics_catalog").is_none(),
        "fixture must stay a pre-mechanics kernel (no mechanics_catalog key)"
    );

    let kernel: RuleKernel =
        serde_json::from_str(raw).expect("old kernel JSON must deserialize unchanged");
    assert!(
        kernel.mechanics_catalog.is_empty(),
        "missing mechanics_catalog key defaults to an empty vec"
    );

    // Key regions must survive verbatim — the new field must not disturb the
    // old layout. Compare against the fixture's own jsonb, item by item.
    let tracks_in_fixture = fixture["resource_tracks"]
        .as_array()
        .expect("fixture has resource_tracks array");
    assert_eq!(kernel.resource_tracks.len(), tracks_in_fixture.len());
    assert_eq!(
        serde_json::to_value(&kernel.resource_tracks).expect("serialize tracks"),
        fixture["resource_tracks"],
        "resource_tracks survive byte-for-byte"
    );
    assert_eq!(
        serde_json::to_value(&kernel.dice_core).expect("serialize dice_core"),
        fixture["dice_core"],
        "dice_core survives byte-for-byte"
    );
    assert_eq!(
        serde_json::to_value(&kernel.character_sheet_schema).expect("serialize sheet schema"),
        fixture["character_sheet_schema"],
        "character_sheet_schema survives byte-for-byte"
    );
    assert_eq!(
        kernel.kernel_id,
        fixture["kernel_id"].as_str().unwrap_or_default()
    );
    assert_eq!(
        kernel.ruleset_id,
        fixture["ruleset_id"].as_str().unwrap_or_default()
    );
    assert_eq!(
        kernel.version,
        fixture["version"].as_str().unwrap_or_default()
    );

    // Re-serializing the upgraded struct must still not disturb old regions.
    let v2 = serde_json::to_value(&kernel).expect("re-serialize kernel");
    assert_eq!(v2["resource_tracks"], fixture["resource_tracks"]);
    assert_eq!(v2["dice_core"], fixture["dice_core"]);

    // P0-2 Task 1: new policy fields default to None on a pre-P0-2 kernel.
    assert!(
        kernel.combat_profile.is_none(),
        "missing combat_profile → None"
    );
    assert!(
        kernel.combat_mode_policy.is_none(),
        "missing combat_mode_policy → None"
    );
    assert!(
        kernel.check_label_policy.is_none(),
        "missing check_label_policy → None"
    );
    assert!(
        kernel.referee_value_bands.is_none(),
        "missing referee_value_bands → None"
    );
    assert!(
        kernel.dice_qualification.is_none(),
        "missing dice_qualification → None"
    );
    // re-serializing a None-policy kernel must not emit the new keys (clean old layout).
    assert!(
        v2.get("combat_profile").is_none(),
        "None policy field must not serialize a key"
    );
}

#[test]
fn old_scenario_node_json_no_scene_mechanics_ok() {
    // Pre-phase-2 node shape: no scene_mechanics key (and none of the other
    // #[serde(default)] extensions either).
    let raw = serde_json::json!({
        "node_id": "sc01",
        "title": "Opening",
        "node_type": "scene",
        "summary": "The investigators arrive at the gas station.",
        "read_aloud": null,
        "gm_notes": null,
        "links": [],
        "assets": [],
        "data": {}
    });
    let node: ScenarioNode =
        serde_json::from_value(raw).expect("old ScenarioNode JSON must deserialize");
    assert!(
        node.scene_mechanics.is_empty(),
        "missing scene_mechanics key defaults to an empty vec"
    );
}

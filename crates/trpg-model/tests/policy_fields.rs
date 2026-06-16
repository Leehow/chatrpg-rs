//! P0-2 Task 1: round-trip + GENERIC_* equivalence + ModuleConfig defaults for
//! the new kernel-resident strategy types. These are pure data contracts; the
//! generic defaults must equal the engine's legacy generic branches verbatim so
//! the None → GENERIC_* fallback in Task 2-5 is a behavioral no-op.

use trpg_model::*;

#[test]
fn combat_profile_round_trips() {
    let p = CombatProfile {
        profile_id: "cyberpunk_red.firefight.v1_3".into(),
        default_mode: "firefight".into(),
        applies_to_modes: vec!["firefight".into(), "netrun".into(), "chase".into()],
        action_economy: serde_json::json!({"turn_slots":[{"slot_id":"action","count":1}]}),
        initiative: serde_json::json!({"kind":"formula","expression":"REF + 1d10"}),
        reaction_windows: vec![serde_json::json!({"advice_id":"generic.required_defense_choice.v1_3"})],
        frame_exit_policy: serde_json::json!({"allow_disengage":true}),
        stalemate_policy: serde_json::Value::Null,
        npc_drive_policy: serde_json::json!({"mook_morale":45}),
        search_recipes: vec![serde_json::json!({"query":"Friday Night Firefight"})],
    };
    let s = serde_json::to_string(&p).unwrap();
    let back: CombatProfile = serde_json::from_str(&s).unwrap();
    assert_eq!(p, back, "CombatProfile must round-trip");
}

#[test]
fn referee_bands_round_trip() {
    let b = RefereeValueBands {
        damage_family: "cyberpunk_red_weapon_damage".into(),
        damage_band: "roughly 2d6..8d6".into(),
        damage_plausible_range: (1, 12),
        difficulty_band: serde_json::json!({"common_dv_band":"9..29"}),
        difficulty_plausible_range: (5, 35),
    };
    let back: RefereeValueBands = serde_json::from_str(&serde_json::to_string(&b).unwrap()).unwrap();
    assert_eq!(b, back);
}

#[test]
fn mode_policy_and_module_config_round_trip() {
    let mp = CombatModePolicy {
        rules: vec![CombatModeRule { mode: CombatMode::HorrorEncounter, when_action_kinds: vec![], when_evidence_contains: vec![] }],
        fallback_mode: CombatMode::TheaterOfMind,
    };
    let back: CombatModePolicy = serde_json::from_str(&serde_json::to_string(&mp).unwrap()).unwrap();
    assert_eq!(mp, back);

    let mc = ModuleConfig {
        npc_actor_bindings: vec![NpcActorBinding { matcher: vec!["drone".into(), "无人机".into()], actor_id: "npc.athena_drone".into(), display_name: Some("rogue drone".into()) }],
        technical_option_table: Some(vec![TechOption { matcher: vec!["cable".into()], dv: 14 }]),
        scene_entity_aliases: vec![EntityAlias { canonical_id: "athena".into(), aliases: vec!["雅典娜".into()] }],
        module_search_profile: Some(SearchProfile { preferred_sections: vec!["Redesigned NPC cards".into()] }),
        director: Some(DirectorModuleConfig {
            scene_facts: vec![DirectorSceneFact { text: "外露电缆是可观察的交互抓手".into(), source: "module_override".into() }],
            npc_advice: vec![NpcBiasedAdvice { npc_id: "injured_lawman".into(), speaker_label: "受伤警察".into(), advice_text: "把火力压住".into(), bias_or_goal: "想活下来".into(), not_official_solution: true }],
            place_summary_fallback: Some("高压现场".into()),
            ..Default::default()
        }),
    };
    let back: ModuleConfig = serde_json::from_str(&serde_json::to_string(&mc).unwrap()).unwrap();
    assert_eq!(mc, back);
}

#[test]
fn empty_json_objects_deserialize_to_defaults() {
    // Pre-P0-2 bundle/kernel JSON: empty object → all-default structs.
    let mc: ModuleConfig = serde_json::from_str("{}").unwrap();
    assert!(mc.npc_actor_bindings.is_empty());
    assert!(mc.technical_option_table.is_none());
    let p: CombatProfile = serde_json::from_str("{}").unwrap();
    assert_eq!(p, CombatProfile::default());
}

#[test]
fn generic_defaults_match_legacy_generic_branch() {
    // GENERIC_REFEREE_BANDS must equal trpg-referee's generic branch verbatim.
    assert_eq!(GENERIC_REFEREE_BANDS.damage_family, "generic_trpg_damage");
    assert_eq!(GENERIC_REFEREE_BANDS.damage_band, "system-specific; exact object/ability entry required");
    assert_eq!(GENERIC_REFEREE_BANDS.damage_plausible_range, (1, 30));
    assert_eq!(GENERIC_REFEREE_BANDS.difficulty_band, serde_json::json!({"common_target_band":"ruleset-specific"}));
    assert_eq!(GENERIC_REFEREE_BANDS.difficulty_plausible_range, (1, 100));
    // GENERIC_COMBAT_PROFILE mirrors default_generic_profile's fiction-first.
    assert_eq!(GENERIC_COMBAT_PROFILE.action_economy, serde_json::json!({"policy":"fiction_first"}));
    assert_eq!(GENERIC_COMBAT_PROFILE.default_mode, "theater_of_mind");
    assert_eq!(GENERIC_DICE_QUALIFICATION.bare_dice_template, "{dice}+0");
    assert_eq!(GENERIC_COMBAT_MODE_POLICY.fallback_mode, CombatMode::TheaterOfMind);
}

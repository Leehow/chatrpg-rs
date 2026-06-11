//! Unit tests for the B1 BP1 catalog-projection renderers.
//! Test list is the spec: each test name comes verbatim from the plan's B1 ③.

use super::*;
use crate::{EngineHook, MechanicEntry, MechanicKind, RuleKernel};
use serde_json::json;

fn entry(id: &str, kind: MechanicKind) -> MechanicEntry {
    MechanicEntry {
        id: id.to_string(),
        name: format!("Name of {id}"),
        kind,
        when_to_use: format!("whenever {id} applies"),
        ..Default::default()
    }
}

#[test]
fn kernel_bp1_view_strips_catalog_only() {
    let kernel = RuleKernel {
        kernel_id: "kernel.test".into(),
        ruleset_id: "ruleset.test".into(),
        version: "1.0".into(),
        dice_core: json!({"die": "d100", "success_bands": [{"id": "extreme"}]}),
        resource_tracks: vec![json!({"id": "stress", "max": 99, "thresholds": [{"at": 0}]})],
        mechanics_catalog: vec![
            entry("m.alpha", MechanicKind::SkillCheck),
            entry("m.beta", MechanicKind::SubsystemProcedure),
            entry("m.gamma", MechanicKind::Reaction),
        ],
        ..Default::default()
    };
    let view = kernel_bp1_view(&kernel);
    assert!(view.mechanics_catalog.is_empty(), "view must strip mechanics_catalog");

    let mut original = serde_json::to_value(&kernel).unwrap();
    let mut projected = serde_json::to_value(&view).unwrap();
    let original = original.as_object_mut().unwrap();
    let projected = projected.as_object_mut().unwrap();
    original.remove("mechanics_catalog");
    projected.remove("mechanics_catalog");
    assert_eq!(original.len(), projected.len(), "no other key added/removed");
    for (key, value) in original.iter() {
        assert_eq!(projected.get(key), Some(value), "field `{key}` must be preserved verbatim");
    }
}

#[test]
fn catalog_index_text_is_deterministic_one_line_per_entry() {
    let entries = vec![
        entry("m.alpha", MechanicKind::SkillCheck),
        entry("m.beta", MechanicKind::SubsystemProcedure),
        entry("m.gamma", MechanicKind::Spend),
    ];
    let first = catalog_index_text(&entries, 96);
    let second = catalog_index_text(&entries, 96);
    assert_eq!(first, second, "same entries must render byte-identical");

    let lines: Vec<&str> = first.lines().collect();
    assert_eq!(lines.len(), entries.len(), "exactly one line per entry");
    for (line, e) in lines.iter().zip(&entries) {
        let segments: Vec<&str> = line.split(" | ").collect();
        assert_eq!(segments.len(), 3, "line must have `id | name | when_to_use` segments: {line}");
        assert_eq!(segments[0], e.id);
        assert_eq!(segments[1], e.name);
        assert_eq!(segments[2], e.when_to_use);
    }
}

#[test]
fn catalog_index_over_limit_keeps_procedures_hooks_and_pm() {
    let limit = 4usize;
    // limit + 4 entries total: a long tail of plain SkillChecks plus one each of
    // SubsystemProcedure / with-hooks / with-passive_projection.
    let mut entries: Vec<MechanicEntry> = (0..limit + 1)
        .map(|i| entry(&format!("m.longtail_{i}"), MechanicKind::SkillCheck))
        .collect();
    entries.push(entry("m.subsystem", MechanicKind::SubsystemProcedure));
    let mut hooked = entry("m.hooked", MechanicKind::SkillCheck);
    hooked.hooks = vec![EngineHook::SceneEnter];
    entries.push(hooked);
    let mut pm = entry("m.passive", MechanicKind::SkillCheck);
    pm.passive_projection = Some("credit {value}".into());
    entries.push(pm);
    assert_eq!(entries.len(), limit + 4);

    let text = catalog_index_text(&entries, limit);
    for i in 0..=limit {
        let id = format!("m.longtail_{i}");
        assert!(!text.contains(&id), "over-limit long-tail SkillCheck `{id}` must be omitted");
    }
    assert!(text.contains("m.subsystem"), "SubsystemProcedure entry must be kept");
    assert!(text.contains("m.hooked"), "entry with hooks must be kept");
    assert!(text.contains("m.passive"), "entry with passive_projection must be kept");
    let tail = text.lines().last().unwrap();
    assert!(tail.contains("lookup_mechanic"), "tail line must mention lookup_mechanic: {tail}");
}

#[test]
fn passive_projection_line_substitutes_value_or_skips() {
    let sheet = json!({"skills": {"credit_rating": 55}});
    let mut e = entry("m.credit", MechanicKind::SkillCheck);
    e.passive_projection = Some("信用评级 {value}".into());
    e.tested_parameter = Some("credit_rating".into());
    assert_eq!(passive_projection_line(&e, &sheet), Some("信用评级 55".to_string()));

    // Case-insensitive bucket lookup (contract: stats/skills/resources/tracks/field).
    let mut upper = e.clone();
    upper.tested_parameter = Some("Credit_Rating".into());
    assert_eq!(passive_projection_line(&upper, &sheet), Some("信用评级 55".to_string()));

    let missing = json!({"skills": {}});
    assert_eq!(passive_projection_line(&e, &missing), None, "value not found -> None");

    let mut no_template = e.clone();
    no_template.passive_projection = None;
    assert_eq!(passive_projection_line(&no_template, &sheet), None, "no passive_projection -> None");
}

// ===== B3 ①②: band semantics line (fail-closed projection of dice_core) =====

#[test]
fn band_semantics_line_renders_from_success_tier_or_band() {
    let dice_core = json!({"success_bands":[{"id":"extreme","label":"极难成功","semantics":"贯穿/卓越效果"}]});
    // outcome carries "success_tier" (mechanics on_tier accounting shape)
    let by_tier = band_semantics_line(&dice_core, &json!({"success_tier":"extreme"}))
        .expect("success_tier form must render");
    for seg in ["extreme", "极难成功", "贯穿/卓越效果"] {
        assert!(by_tier.contains(seg), "line must contain `{seg}`: {by_tier}");
    }
    // outcome carries "band" (gm_loop fixture shape) — both real-world forms
    let by_band = band_semantics_line(&dice_core, &json!({"band":"extreme"}))
        .expect("band form must render");
    for seg in ["extreme", "极难成功", "贯穿/卓越效果"] {
        assert!(by_band.contains(seg), "line must contain `{seg}`: {by_band}");
    }
    // outcome without any band field -> None
    assert_eq!(band_semantics_line(&dice_core, &json!({"total": 18})), None);
}

#[test]
fn band_without_semantics_is_none() {
    // band hit but the entry has no semantics key -> None (fail-closed half of
    // acceptance 12③: the caller falls back to the bare band id+label).
    let dice_core = json!({"success_bands":[{"id":"extreme","label":"极难成功"}]});
    assert_eq!(band_semantics_line(&dice_core, &json!({"success_tier":"extreme"})), None);
}

// ===== C3: scene_intents_text (current-scene intents index, BP2) =====

fn intent(id: &str, tested: &str, difficulty: Option<serde_json::Value>) -> crate::SceneMechanicIntent {
    crate::SceneMechanicIntent {
        intent_id: id.to_string(),
        description: format!("玩家试图 {id}"),
        tested_parameter: tested.to_string(),
        difficulty,
        effect_policy: crate::EffectPolicy::default(),
        source_anchor: "p.12 原文锚点".to_string(),
    }
}

#[test]
fn scene_intents_text_renders_one_line_per_intent() {
    let intents = vec![
        intent("homecoming.lawmen.cut_cable_force", "brawling", Some(json!({"kind":"dv","value":13}))),
        intent("homecoming.lawmen.sneak_past", "stealth", None),
    ];
    let text = scene_intents_text(&intents).expect("non-empty intents must render");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "one line per intent: {text}");
    assert!(lines[0].contains("homecoming.lawmen.cut_cable_force"), "line must carry intent_id: {}", lines[0]);
    assert!(lines[0].contains("brawling"), "line must carry tested_parameter: {}", lines[0]);
    assert!(
        lines[0].contains(r#"{"kind":"dv","value":13}"#),
        "difficulty summary must be the compact serde_json string: {}",
        lines[0]
    );
    assert!(lines[1].contains("homecoming.lawmen.sneak_past"), "line must carry intent_id: {}", lines[1]);
    assert!(lines[1].contains("stealth"), "line must carry tested_parameter: {}", lines[1]);
    assert!(lines[1].ends_with("| -"), "difficulty None must render as `-`: {}", lines[1]);
}

#[test]
fn scene_intents_text_empty_returns_none() {
    assert_eq!(scene_intents_text(&[]), None, "empty slice -> None (caller emits no block)");
}

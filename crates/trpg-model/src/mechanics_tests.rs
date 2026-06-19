//! Unit tests for the phase-2 shared mechanics types (task A1).
//! Test list is the spec: each test name comes verbatim from the plan's A1 ③.

use super::*;
use crate::{RuleKernel, SourceRef};
use serde_json::json;

fn full_entry() -> MechanicEntry {
    MechanicEntry {
        id: "coc.sanity_check".into(),
        name: "Sanity check".into(),
        kind: MechanicKind::SubsystemProcedure,
        description: "Roll against the current value of the tested track".into(),
        when_to_use: "When the character witnesses something mind-bending".into(),
        tested_parameter: Some("sanity".into()),
        procedure: vec![
            ProcedureStep::Roll {
                dice: "1d100".into(),
                vs: Some("sanity".into()),
                note: None,
            },
            ProcedureStep::Apply {
                track: "sanity".into(),
                op: "subtract".into(),
                amount: "1d6".into(),
                note: Some("on failure".into()),
            },
            ProcedureStep::TableRoll {
                table_ref: "bout_of_madness".into(),
                note: None,
            },
            ProcedureStep::Gate {
                condition: "loss_in_one_go >= 5".into(),
                note: None,
            },
        ],
        hooks: vec![
            EngineHook::SceneEnter,
            EngineHook::Calendar {
                granularity: CalendarGranularity {
                    unit: "segment".into(),
                    seconds_per_unit: None,
                    segments_per_day: Some(8),
                    label: Some("watch".into()),
                },
            },
        ],
        passive_projection: Some("信用评级 {value}：{band}".into()),
        followup_links: vec![FollowupLink {
            condition: FollowupCondition::Threshold {
                track_id: "sanity".into(),
                threshold_ref: "loss_in_one_go:5".into(),
            },
            procedure_id: "coc.temporary_insanity".into(),
        }],
        locked_until: Some("after the first bout of madness".into()),
        source_refs: vec![SourceRef {
            source_id: "rulebook_main".into(),
            page: Some(155),
            ..Default::default()
        }],
    }
}

#[test]
fn mechanics_catalog_round_trips() {
    let kernel = RuleKernel {
        kernel_id: "k1".into(),
        ruleset_id: "rs1".into(),
        version: "1".into(),
        mechanics_catalog: vec![full_entry()],
        ..Default::default()
    };
    let v1 = serde_json::to_value(&kernel).expect("serialize kernel");
    let back: RuleKernel = serde_json::from_value(v1.clone()).expect("deserialize kernel");
    let v2 = serde_json::to_value(&back).expect("re-serialize kernel");
    assert_eq!(v1, v2, "kernel round-trip must be lossless");
    assert_eq!(back.mechanics_catalog.len(), 1);
    let e = &back.mechanics_catalog[0];
    assert_eq!(e.id, "coc.sanity_check");
    assert_eq!(e.name, "Sanity check");
    assert_eq!(e.kind, MechanicKind::SubsystemProcedure);
    assert_eq!(e.tested_parameter.as_deref(), Some("sanity"));
    assert_eq!(
        e.procedure.len(),
        4,
        "all 4 structured procedure forms survive"
    );
    assert_eq!(e.hooks.len(), 2, "hooks incl. Calendar survive");
    assert_eq!(e.followup_links.len(), 1);
    assert_eq!(e.followup_links[0].procedure_id, "coc.temporary_insanity");
    assert_eq!(
        e.passive_projection.as_deref(),
        Some("信用评级 {value}：{band}")
    );
    assert_eq!(
        e.locked_until.as_deref(),
        Some("after the first bout of madness")
    );
    assert_eq!(e.source_refs.len(), 1);
    assert_eq!(e.source_refs[0].page, Some(155));
}

#[test]
fn mechanic_kind_other_fallback() {
    let k: MechanicKind =
        serde_json::from_value(json!("honor_economy")).expect("open enum accepts unknown kind");
    assert_eq!(k, MechanicKind::Other("honor_economy".into()));
    let k: MechanicKind = serde_json::from_value(json!("skill_check")).expect("known kind");
    assert_eq!(k, MechanicKind::SkillCheck);
    let e: MechanicEntry =
        serde_json::from_value(json!({"id": "x", "name": "X"})).expect("kind field omitted");
    assert_eq!(
        e.kind,
        MechanicKind::Other(String::new()),
        "missing kind takes default"
    );
}

#[test]
fn procedure_step_unknown_shape_preserved() {
    let raw = json!({"step": "weird_custom", "foo": 1});
    let step: ProcedureStep =
        serde_json::from_value(raw.clone()).expect("unknown step downgrades to Other");
    match &step {
        ProcedureStep::Other(v) => assert_eq!(v, &raw, "Other keeps the original JSON intact"),
        other => panic!("expected ProcedureStep::Other, got {other:?}"),
    }
    let back = serde_json::to_value(&step).expect("serialize downgraded step");
    assert_eq!(
        back, raw,
        "downgraded step round-trips without losing a byte"
    );
}

#[test]
fn followup_condition_other_preserved() {
    let raw = json!({"kind": "chaos_pool_full", "pool": "chaos"});
    let cond: FollowupCondition =
        serde_json::from_value(raw.clone()).expect("unknown condition downgrades to Other");
    match &cond {
        FollowupCondition::Other(v) => assert_eq!(v, &raw),
        other => panic!("expected FollowupCondition::Other, got {other:?}"),
    }
    let back = serde_json::to_value(&cond).expect("serialize downgraded condition");
    assert_eq!(
        back, raw,
        "downgraded condition round-trips without losing a byte"
    );
}

#[test]
fn engine_hook_serde_tags() {
    let h: EngineHook =
        serde_json::from_value(json!({"event": "scene_enter"})).expect("scene_enter");
    assert!(matches!(h, EngineHook::SceneEnter));
    let h: EngineHook = serde_json::from_value(
        json!({"event": "calendar", "granularity": {"unit": "segment", "segments_per_day": 8}}),
    )
    .expect("calendar with granularity");
    match h {
        EngineHook::Calendar { granularity } => {
            assert_eq!(granularity.unit, "segment");
            assert_eq!(granularity.segments_per_day, Some(8));
        }
        other => panic!("expected EngineHook::Calendar, got {other:?}"),
    }
    assert!(
        serde_json::from_value::<EngineHook>(json!({"event": "galactic_alignment"})).is_err(),
        "unknown engine event must be a hard Err — the A3 guardrail keys off this Err; \
         this enum is the single source of truth for the engine event vocabulary"
    );
}

#[test]
fn granularity_seconds_units() {
    let g = |unit: &str, spu: Option<i64>, spd: Option<u32>| CalendarGranularity {
        unit: unit.into(),
        seconds_per_unit: spu,
        segments_per_day: spd,
        label: None,
    };
    assert_eq!(granularity_seconds(&g("day", None, None)), Some(86_400));
    assert_eq!(granularity_seconds(&g("week", None, None)), Some(604_800));
    assert_eq!(
        granularity_seconds(&g("segment", None, Some(8))),
        Some(10_800),
        "Fate: one day split into 8 segments"
    );
    assert_eq!(
        granularity_seconds(&g("day", Some(3_600), None)),
        Some(3_600),
        "seconds_per_unit takes precedence over unit"
    );
    assert_eq!(
        granularity_seconds(&g("month", None, None)),
        None,
        "irregular calendar month: the data must say seconds_per_unit, never guess"
    );
}

#[test]
fn expressiveness_tier_priority() {
    let mut e = MechanicEntry {
        id: "m".into(),
        name: "M".into(),
        ..Default::default()
    };
    e.procedure = vec![ProcedureStep::Gate {
        condition: "x".into(),
        note: None,
    }];
    e.hooks = vec![EngineHook::TurnStart];
    e.passive_projection = Some("p".into());
    assert_eq!(
        expressiveness_tier(&e),
        ExpressivenessTier::Procedure,
        "procedure wins first"
    );
    e.procedure = vec![];
    assert_eq!(
        expressiveness_tier(&e),
        ExpressivenessTier::Hook,
        "then hooks"
    );
    e.hooks = vec![];
    assert_eq!(
        expressiveness_tier(&e),
        ExpressivenessTier::PassiveModifier,
        "then passive_projection alone"
    );
    e.passive_projection = None;
    assert_eq!(
        expressiveness_tier(&e),
        ExpressivenessTier::Semantic,
        "all empty -> Semantic"
    );
}

#[test]
fn track_semantic_line_renders_threshold_zone() {
    let track = json!({
        "id": "sanity",
        "thresholds": [
            {"at": 0, "direction": "at_or_below", "consequence": "永久疯狂"},
            {"loss_in_one_go": 5, "consequence": "临时疯狂"}
        ],
        "zero_means": "心智彻底崩溃"
    });
    let line = track_semantic_line(&track, 38).expect("semantic line for a track with thresholds");
    assert!(line.contains("sanity"), "line names the track: {line}");
    assert!(
        line.contains("38"),
        "line carries the current value: {line}"
    );
    assert!(
        line.contains("永久疯狂") || line.contains("临时疯狂"),
        "line carries locatable threshold semantics: {line}"
    );
    assert!(
        track_semantic_line(&json!({"id": "mana"}), 7).is_none(),
        "no thresholds and no zero_means -> None (fail-closed, caller falls back to bare number)"
    );
}

#[test]
fn scene_mechanic_intent_round_trips() {
    let intent = SceneMechanicIntent {
        intent_id: "homecoming.lawmen.cut_cable_force".into(),
        description: "Forcing the elevator cable instead of cutting it cleanly".into(),
        tested_parameter: "brawling".into(),
        difficulty: Some(json!({"kind": "dv", "value": 13})),
        effect_policy: EffectPolicy {
            on_success: vec![EffectPatchIntent::SetObjectState {
                object_id: "cable".into(),
                patch: json!({"state": "cut"}),
            }],
            on_failure: vec![
                EffectPatchIntent::ModifyTrack {
                    owner_kind: "actor".into(),
                    owner_id: None,
                    track_id: "hp".into(),
                    op: "subtract".into(),
                    amount: 2,
                },
                EffectPatchIntent::CreateFact {
                    target: "scene".into(),
                    fact: json!({"alarm": "raised"}),
                },
                EffectPatchIntent::StartCountdown {
                    label: "lawmen arrive".into(),
                    amount: 10,
                    scale: "minutes".into(),
                    payload: json!({"event": "lawmen_arrive"}),
                },
            ],
        },
        source_anchor: "Page 12 - The Lawmen".into(),
    };
    let v1 = serde_json::to_value(&intent).expect("serialize intent");
    let back: SceneMechanicIntent = serde_json::from_value(v1.clone()).expect("deserialize intent");
    let v2 = serde_json::to_value(&back).expect("re-serialize intent");
    assert_eq!(v1, v2, "intent round-trip must be lossless");
    assert_eq!(back.intent_id, "homecoming.lawmen.cut_cable_force");
    assert_eq!(back.tested_parameter, "brawling");
    assert_eq!(back.difficulty, Some(json!({"kind": "dv", "value": 13})));
    assert_eq!(back.effect_policy.on_success.len(), 1);
    assert_eq!(back.effect_policy.on_failure.len(), 3);
    assert_eq!(back.source_anchor, "Page 12 - The Lawmen");
}

#[test]
fn effect_patch_intent_other_keeps_json() {
    let raw = json!({"kind": "summon_meteor", "target": "town"});
    let p: EffectPatchIntent =
        serde_json::from_value(raw.clone()).expect("unknown intent downgrades to Other");
    match &p {
        EffectPatchIntent::Other(v) => assert_eq!(v, &raw, "Other keeps the original JSON"),
        other => panic!("expected EffectPatchIntent::Other, got {other:?}"),
    }
    assert_eq!(
        serde_json::to_value(&p).expect("serialize"),
        raw,
        "downgraded intent round-trips without losing a byte"
    );
}

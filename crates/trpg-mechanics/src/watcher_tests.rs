//! B4/B5 watcher 纯函数测试（不碰 db）。track fixture 直接抄真 CoC kernel
//! sanity 轨形状（测试清单指定，fixture 内的规则集词汇不属于逻辑硬编码）。
use crate::watcher::{
    calendar_crossed, crossing_fact, detect_crossings, due_from_crossing, hook_due_candidates,
    hook_matches_event, HookEvent,
};
use serde_json::json;
use trpg_model::{CalendarGranularity, DueSource, DueStatus, EngineHook, MechanicEntry, ParameterOperation};

fn sanity_track() -> serde_json::Value {
    json!({"id":"sanity","owner_kind":"actor","thresholds":[
        {"loss_in_one_go":5,"consequence":"may trigger temporary insanity","followup_procedure_id":"coc.temporary_insanity"},
        {"at":0,"direction":"at_or_below","consequence":"permanent insanity"}]})
}

#[test]
fn san_loss_of_6_in_one_go_produces_crossing() {
    let cs = detect_crossings(&sanity_track(), "actor", "pc.current", 38, 32, ParameterOperation::Subtract);
    assert_eq!(cs.len(), 1, "exactly one crossing expected: {cs:?}");
    assert_eq!(cs[0].kind, "loss_in_one_go");
    assert_eq!(cs[0].followup_procedure_id.as_deref(), Some("coc.temporary_insanity"));
    assert_eq!(cs[0].before, 38);
    assert_eq!(cs[0].after, 32);
}

#[test]
fn san_loss_of_4_produces_no_crossing() {
    let cs = detect_crossings(&sanity_track(), "actor", "pc.current", 38, 34, ParameterOperation::Subtract);
    assert!(cs.is_empty(), "loss of 4 must not cross loss_in_one_go:5: {cs:?}");
}

#[test]
fn hp_reaching_zero_crosses_at_or_below() {
    let track = json!({"id":"hit_points","owner_kind":"actor","thresholds":[
        {"at":0,"direction":"at_or_below","consequence":"unconscious or dying"}]});
    let cs = detect_crossings(&track, "actor", "pc.current", 3, 0, ParameterOperation::Subtract);
    assert_eq!(cs.len(), 1);
    assert_eq!(cs[0].kind, "cumulative");
    // 边沿触发：before==after==0 没有"穿越"，不重复发。
    let none = detect_crossings(&track, "actor", "pc.current", 0, 0, ParameterOperation::Subtract);
    assert!(none.is_empty(), "edge-triggered: staying at 0 must not re-cross: {none:?}");
}

#[test]
fn threshold_without_followup_still_yields_due_with_prose() {
    // sanity 轨第二条阈值（at:0）没有 followup_procedure_id ——
    // fail-closed 但不静默：due 仍产出，threshold_desc 带 prose consequence 原文。
    let cs = detect_crossings(&sanity_track(), "actor", "pc.current", 2, 0, ParameterOperation::Subtract);
    assert_eq!(cs.len(), 1, "only the at:0 cumulative threshold crosses: {cs:?}");
    assert_eq!(cs[0].followup_procedure_id, None);
    let due = due_from_crossing("s", "t", &cs[0]);
    assert_eq!(due.threshold_desc, "permanent insanity");
}

#[test]
fn due_from_crossing_evidence_has_before_after_delta() {
    let cs = detect_crossings(&sanity_track(), "actor", "pc.current", 38, 32, ParameterOperation::Subtract);
    let due = due_from_crossing("sess_x", "turn_y", &cs[0]);
    assert_eq!(due.evidence.get("before").and_then(|v| v.as_i64()), Some(38));
    assert_eq!(due.evidence.get("after").and_then(|v| v.as_i64()), Some(32));
    assert_eq!(due.evidence.get("delta").and_then(|v| v.as_i64()), Some(-6), "delta == after - before");
    assert_eq!(due.status, DueStatus::Open);
    assert_eq!(due.source, DueSource::Threshold);
    assert_eq!(due.source_track.as_deref(), Some("sanity"));
    assert_eq!(due.session_id, "sess_x");
    assert_eq!(due.turn_id, "turn_y");
    assert_eq!(due.owner_kind, "actor");
    assert_eq!(due.owner_id, "pc.current");
    assert!(due.due_id.starts_with("due_"), "due_id must be 'due_{{uuid simple}}': {}", due.due_id);
}

// ===== B5：EngineHook 事件点纯函数 =====

#[test]
fn calendar_crossing_fires_only_across_segment_boundary() {
    // Fate 一天 8 段实证：段长 86400/8 = 10800s。
    let seg = CalendarGranularity { unit: "segment".to_string(), seconds_per_unit: None, segments_per_day: Some(8), label: None };
    assert!(calendar_crossed(&seg, 5000, 12000), "5000→12000 crosses the 10800s segment boundary");
    assert!(!calendar_crossed(&seg, 1000, 9000), "1000→9000 stays inside the first segment");
    let day = CalendarGranularity { unit: "day".to_string(), seconds_per_unit: None, segments_per_day: None, label: None };
    assert!(calendar_crossed(&day, 86000, 87000), "86000→87000 crosses the day boundary at 86400");
}

#[test]
fn unknown_granularity_is_fail_closed() {
    let g = CalendarGranularity { unit: "???".to_string(), seconds_per_unit: None, segments_per_day: None, label: None };
    assert!(!calendar_crossed(&g, 0, 1_000_000), "unrecognized granularity must never fire (no guessing)");
}

#[test]
fn zero_second_granularity_never_panics() {
    // LLM 编译遍可产出 segments_per_day > 86400（如 100000）→ 86400/n 整除得 0。
    // calendar_crossed 必须把 0 当认不出（fail-closed），绝不除零 panic。
    let g = CalendarGranularity { unit: "segment".to_string(), seconds_per_unit: None, segments_per_day: Some(100_000), label: None };
    assert!(!calendar_crossed(&g, 0, 1_000_000), "segments_per_day=100000 yields 0s granularity: must be false, not panic");
}

#[test]
fn hook_matches_event_maps_variants_one_to_one() {
    assert!(hook_matches_event(&EngineHook::TurnStart, &HookEvent::TurnStart));
    assert!(!hook_matches_event(&EngineHook::SceneEnter, &HookEvent::TurnStart));
    assert!(hook_matches_event(&EngineHook::Rest, &HookEvent::Rest));
    // Calendar 不经此函数直配（强制走 calendar_crossed 分支）。
    let cal = EngineHook::Calendar { granularity: CalendarGranularity { unit: "day".to_string(), seconds_per_unit: None, segments_per_day: None, label: None } };
    assert!(!hook_matches_event(&cal, &HookEvent::TimeAdvance { from_tick: 0, to_tick: 1_000_000 }));
}

#[test]
fn turn_start_hook_entry_produces_due_with_mechanic_id() {
    let entry = MechanicEntry {
        id: "test.upkeep".to_string(),
        name: "Upkeep".to_string(),
        when_to_use: "at the start of every turn".to_string(),
        hooks: vec![EngineHook::TurnStart],
        ..Default::default()
    };
    let dues = hook_due_candidates(&[entry], &HookEvent::TurnStart, "sess_x", "turn_y");
    assert_eq!(dues.len(), 1, "exactly one candidate expected: {dues:?}");
    assert_eq!(dues[0].source, DueSource::Hook);
    assert_eq!(dues[0].mechanic_id.as_deref(), Some("test.upkeep"));
    assert_eq!(dues[0].hook_event.as_deref(), Some("turn_start"));
}

/// 重构回归金样：既有 `resource_threshold_consequence` CreateFact 的 fact JSON
/// 在同输入下逐字节不变（以重构前 lib.rs L168–191 的字面构造为金样）。
#[test]
fn apply_outcome_threshold_facts_unchanged() {
    // 金样 A：单独 loss_in_one_go（before=38→32, Subtract）。
    let cs = detect_crossings(&sanity_track(), "actor", "pc.current", 38, 32, ParameterOperation::Subtract);
    let golden_loss = json!({"resource_track": "sanity", "kind": "loss_in_one_go", "lost": 6, "value": 32, "consequence": "may trigger temporary insanity"});
    assert_eq!(
        serde_json::to_string(&crossing_fact(&cs[0])).unwrap(),
        serde_json::to_string(&golden_loss).unwrap(),
        "loss_in_one_go fact must be byte-identical to the pre-refactor literal"
    );
    // 金样 B：同次结算两条阈值齐发（before=6→0, Subtract）：
    // 旧循环按 thresholds 数组序产出 [loss_in_one_go(th1), cumulative(th2)]。
    let cs = detect_crossings(&sanity_track(), "actor", "pc.current", 6, 0, ParameterOperation::Subtract);
    assert_eq!(cs.len(), 2, "both thresholds fire: {cs:?}");
    let golden_loss2 = json!({"resource_track": "sanity", "kind": "loss_in_one_go", "lost": 6, "value": 0, "consequence": "may trigger temporary insanity"});
    let golden_cumulative = json!({"resource_track": "sanity", "kind": "cumulative", "threshold_at": 0, "value": 0, "consequence": "permanent insanity"});
    assert_eq!(serde_json::to_string(&crossing_fact(&cs[0])).unwrap(), serde_json::to_string(&golden_loss2).unwrap());
    assert_eq!(serde_json::to_string(&crossing_fact(&cs[1])).unwrap(), serde_json::to_string(&golden_cumulative).unwrap());
}

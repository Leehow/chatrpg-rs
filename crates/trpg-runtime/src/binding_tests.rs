//! binding.rs 单测（文件 ≤400 行纪律：从 binding.rs 拆出，经 #[path] 挂回为 `mod tests`）。
use super::*;
use trpg_model::{CAP_CHECK_MEET_OR_BEAT, CAP_CHECK_ROLL_UNDER};

fn a_source_ref() -> SourceRef {
    SourceRef {
        source_id: "coc_rulebook".to_string(),
        page: Some(88),
        ..Default::default()
    }
}

fn facet(candidates: Vec<&str>, sources: Vec<SourceRef>) -> AssetFacet {
    AssetFacet {
        facet_kind: "check".to_string(),
        binding_candidates: candidates.into_iter().map(|c| c.to_string()).collect(),
        confidence: 0.5,
        source_refs: sources,
    }
}

fn trace(need_kind: &str, sources: Vec<SourceRef>) -> NeedResolutionTrace {
    NeedResolutionTrace {
        need_kind: need_kind.to_string(),
        source_refs: sources,
        reason: "test".to_string(),
        block_count: 0,
    }
}

#[test]
fn verdict_exact_when_candidate_registered_and_sourced() {
    let reg = CapabilityRegistry::with_defaults();
    let facets = vec![facet(vec![CAP_CHECK_ROLL_UNDER], vec![a_source_ref()])];
    let plan = resolve_binding("rule", &facets, &reg);
    assert_eq!(plan.verdict, BindingVerdict::Exact);
    assert_eq!(plan.capability.as_deref(), Some(CAP_CHECK_ROLL_UNDER));
    assert_eq!(plan.execution_tier, ExecutionTier::ExactExecution);
    assert_eq!(plan.confidence, 0.9);
    assert!(plan.unresolved_reason.is_none());
}

#[test]
fn verdict_partial_when_registered_no_source() {
    let reg = CapabilityRegistry::with_defaults();
    let facets = vec![facet(vec![CAP_CHECK_MEET_OR_BEAT], vec![])];
    let plan = resolve_binding("rule", &facets, &reg);
    assert_eq!(plan.verdict, BindingVerdict::Partial);
    assert_eq!(plan.capability.as_deref(), Some(CAP_CHECK_MEET_OR_BEAT));
    assert_eq!(plan.execution_tier, ExecutionTier::PartialExecution);
    assert_eq!(plan.confidence, 0.6);
}

#[test]
fn verdict_guided_when_candidate_unregistered_but_sourced() {
    let reg = CapabilityRegistry::with_defaults();
    let facets = vec![facet(vec!["check.bogus_unregistered"], vec![a_source_ref()])];
    let plan = resolve_binding("rule", &facets, &reg);
    assert_eq!(plan.verdict, BindingVerdict::Guided);
    assert!(plan.capability.is_none());
    assert_eq!(plan.execution_tier, ExecutionTier::GuidedRuling);
    assert_eq!(plan.confidence, 0.4);
    assert_eq!(
        plan.unresolved_reason.as_deref(),
        Some("no registered capability for candidates")
    );
}

#[test]
fn verdict_source_only_when_only_source() {
    let reg = CapabilityRegistry::with_defaults();
    let facets = vec![facet(vec![], vec![a_source_ref()])];
    let plan = resolve_binding("rule", &facets, &reg);
    assert_eq!(plan.verdict, BindingVerdict::SourceOnly);
    assert!(plan.capability.is_none());
    assert_eq!(plan.execution_tier, ExecutionTier::SourceOnly);
    assert_eq!(plan.confidence, 0.3);
}

#[test]
fn verdict_unsupported_when_empty() {
    let reg = CapabilityRegistry::with_defaults();
    let facets = vec![facet(vec![], vec![])];
    let plan = resolve_binding("rule", &facets, &reg);
    assert_eq!(plan.verdict, BindingVerdict::Unsupported);
    assert!(plan.capability.is_none());
    assert_eq!(plan.execution_tier, ExecutionTier::SourceOnly);
    assert_eq!(plan.confidence, 0.0);
    assert_eq!(
        plan.unresolved_reason.as_deref(),
        Some("no facet candidates or source")
    );
}

#[test]
fn resolve_binding_is_deterministic() {
    let reg = CapabilityRegistry::with_defaults();
    let facets = vec![facet(vec![CAP_CHECK_ROLL_UNDER], vec![a_source_ref()])];
    let a = resolve_binding("rule", &facets, &reg);
    let b = resolve_binding("rule", &facets, &reg);
    assert_eq!(a, b);
    assert_eq!(a.binding_id, "bind:rule");
}

#[test]
fn shadow_bind_over_need_traces() {
    let reg = CapabilityRegistry::with_defaults();
    let traces = vec![
        trace("rule", vec![a_source_ref()]),
        trace("scene", vec![]),
    ];
    let plans = shadow_bind(&traces, &reg);
    assert_eq!(plans.len(), 2);

    let rule_plan = plans.iter().find(|p| p.need_kind == "rule").unwrap();
    // rule → candidates include registered check caps + has source → Exact.
    assert_eq!(rule_plan.verdict, BindingVerdict::Exact);
    assert_eq!(rule_plan.binding_id, "bind:rule");

    let scene_plan = plans.iter().find(|p| p.need_kind == "scene").unwrap();
    // scene → candidates registered (visibility/clock) but no source → Partial.
    assert_eq!(scene_plan.verdict, BindingVerdict::Partial);
    assert_eq!(scene_plan.binding_id, "bind:scene");
}

#[test]
fn registry_has_all_11() {
    let reg = CapabilityRegistry::with_defaults();
    assert_eq!(ALL_CAPABILITIES.len(), 11);
    for cap in ALL_CAPABILITIES {
        assert!(reg.has(cap), "registry should have {cap}");
    }
    assert!(!reg.has("nope"));
}

// ── kernel-facets：从已解析 RuleKernel 派生权威 facet ──────────────────────

/// 造一个最小 RuleKernel：可指定 dice_core.compare + 是否带 source_refs + 是否带 resource_tracks。
fn kernel_with(
    compare: Option<&str>,
    sourced: bool,
    resource_tracks: Vec<serde_json::Value>,
) -> trpg_model::RuleKernel {
    let dice_core = match compare {
        Some(c) => serde_json::json!({ "compare": c }),
        None => serde_json::json!({}),
    };
    trpg_model::RuleKernel {
        kernel_id: "k".into(),
        ruleset_id: "rs".into(),
        version: "1".into(),
        dice_core,
        resource_tracks,
        source_refs: if sourced { vec![a_source_ref()] } else { vec![] },
        ..Default::default()
    }
}

// roll_under → check_model facet[CAP_CHECK_ROLL_UNDER] + source → resolve = Exact / ExactExecution.
#[test]
fn facets_from_kernel_roll_under_yields_check_roll_under() {
    let reg = CapabilityRegistry::with_defaults();
    let kernel = kernel_with(Some("roll_under"), true, vec![]);
    let facets = facets_from_kernel(&kernel);
    let check = facets.iter().find(|f| f.facet_kind == "check_model").expect("check facet");
    assert_eq!(check.binding_candidates, vec![CAP_CHECK_ROLL_UNDER.to_string()]);
    assert_eq!(check.confidence, 0.9);
    assert!(!check.source_refs.is_empty(), "facet 必须透传 kernel source_refs");

    let plan = resolve_binding("ruleset_check", &facets, &reg);
    assert_eq!(plan.verdict, BindingVerdict::Exact);
    assert_eq!(plan.execution_tier, ExecutionTier::ExactExecution);
    assert_eq!(plan.capability.as_deref(), Some(CAP_CHECK_ROLL_UNDER));
}

// meet_or_beat → CAP_CHECK_MEET_OR_BEAT，有据 → Exact。
#[test]
fn facets_from_kernel_meet_or_beat_yields_check_meet_or_beat() {
    let reg = CapabilityRegistry::with_defaults();
    let kernel = kernel_with(Some("meet_or_beat"), true, vec![]);
    let facets = facets_from_kernel(&kernel);
    let plan = resolve_binding("ruleset_check", &facets, &reg);
    assert_eq!(plan.verdict, BindingVerdict::Exact);
    assert_eq!(plan.capability.as_deref(), Some(CAP_CHECK_MEET_OR_BEAT));
}

// count_faces → CAP_CHECK_COUNT_FACES，有据 → Exact。
#[test]
fn facets_from_kernel_count_faces_yields_check_count_faces() {
    let reg = CapabilityRegistry::with_defaults();
    let kernel = kernel_with(Some("count_faces"), true, vec![]);
    let facets = facets_from_kernel(&kernel);
    let plan = resolve_binding("ruleset_check", &facets, &reg);
    assert_eq!(plan.verdict, BindingVerdict::Exact);
    assert_eq!(plan.capability.as_deref(), Some(CAP_CHECK_COUNT_FACES));
}

// 空/未知 compare → 不产 check facet（fail-soft）。
#[test]
fn facets_from_kernel_empty_kernel_yields_no_check_facet() {
    let no_compare = kernel_with(None, true, vec![]);
    assert!(
        !facets_from_kernel(&no_compare).iter().any(|f| f.facet_kind == "check_model"),
        "无 compare → 无 check facet",
    );
    let bogus = kernel_with(Some("totally_unknown"), true, vec![]);
    assert!(
        !facets_from_kernel(&bogus).iter().any(|f| f.facet_kind == "check_model"),
        "未知 compare → 无 check facet",
    );
    // 完全空 kernel（无 dice_core / 无 resource）→ 空 Vec。
    let empty = kernel_with(None, false, vec![]);
    assert!(facets_from_kernel(&empty).is_empty(), "空 kernel → 空 facet（fail-soft）");
}

// resource_tracks 非空 → 一条聚合 resource facet[CAP_RESOURCE_DELTA]。
#[test]
fn facets_from_kernel_resource_tracks_yield_resource_facet() {
    let tracks = vec![serde_json::json!({"id":"hp","kind":"health"}), serde_json::json!({"id":"san"})];
    let kernel = kernel_with(Some("roll_under"), true, tracks);
    let facets = facets_from_kernel(&kernel);
    let res = facets.iter().find(|f| f.facet_kind == "resource").expect("resource facet");
    assert_eq!(res.binding_candidates, vec![CAP_RESOURCE_DELTA.to_string()]);
    assert_eq!(res.confidence, 0.8);
    assert_eq!(
        facets.iter().filter(|f| f.facet_kind == "resource").count(),
        1,
        "多条 track 仍只产一条聚合 resource facet",
    );
}

// shadow_bind_with_kernel：need-trace 计划 + kernel 派生 ruleset_check / ruleset_resource。
#[test]
fn shadow_bind_with_kernel_appends_kernel_plan() {
    let reg = CapabilityRegistry::with_defaults();
    let traces = vec![trace("scene", vec![])];
    let kernel = kernel_with(Some("roll_under"), true, vec![serde_json::json!({"id":"hp","kind":"health"})]);

    let base = shadow_bind(&traces, &reg);
    assert_eq!(base.len(), 1, "无 kernel → 仅 need-trace 计划");

    let plans = shadow_bind_with_kernel(&traces, Some(&kernel), &reg);
    assert_eq!(plans.len(), 3, "need-trace + ruleset_check + ruleset_resource");

    let check = plans.iter().find(|p| p.need_kind == "ruleset_check").expect("ruleset_check plan");
    assert_eq!(check.verdict, BindingVerdict::Exact, "kernel 有据 → Exact");
    assert_eq!(check.capability.as_deref(), Some(CAP_CHECK_ROLL_UNDER));

    let res = plans.iter().find(|p| p.need_kind == "ruleset_resource").expect("ruleset_resource plan");
    assert_eq!(res.verdict, BindingVerdict::Exact);
    assert_eq!(res.capability.as_deref(), Some(CAP_RESOURCE_DELTA));

    // back-compat：None 等价旧 shadow_bind（零行为变更佐证）。
    assert_eq!(shadow_bind_with_kernel(&traces, None, &reg), base);
}

// kernel 无可用 dice_core/resource → 不追加任何 kernel 计划（fail-soft）。
#[test]
fn shadow_bind_with_kernel_empty_kernel_appends_nothing() {
    let reg = CapabilityRegistry::with_defaults();
    let traces = vec![trace("scene", vec![])];
    let empty = kernel_with(None, false, vec![]);
    assert_eq!(
        shadow_bind_with_kernel(&traces, Some(&empty), &reg),
        shadow_bind(&traces, &reg),
        "空 kernel → 与无 kernel 等价",
    );
}

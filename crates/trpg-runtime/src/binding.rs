//! 影子 BindingResolver（asset-binding 切片 T2）。
//!
//! 纯逻辑、确定性、**无执行 / 无 LLM / 无 DB / 无随机 / 无时钟**：
//! - `CapabilityRegistry`：已注册 capability id 集合（默认 = `trpg_model::ALL_CAPABILITIES`）。
//! - `resolve_binding`：给定 need_kind + 其 facets + registry，产出**单个** `BindingPlan`
//!   （verdict / capability / execution_tier / confidence 由确定性规则推出，见 §2 / 本文档下方表）。
//! - `facets_from_need_traces`：轻量 adapter，从既有 `NeedResolutionTrace` 派生最小 `AssetFacet`
//!   （facet_kind = need_kind、binding_candidates 按 need_kind→候选 capability 的**启发式**数据映射、
//!   source_refs 透传）。
//! - `shadow_bind`：便利函数，adapter + 逐组 resolve，产出 advisory `BindingPlan` 列表
//!   （trpg-gm T3 调用入口；advisory，不改实际结算）。
//!
//! verdict → execution_tier 映射（确定性）：
//! | 条件                                            | verdict     | capability       | tier              | confidence |
//! |-------------------------------------------------|-------------|------------------|-------------------|-----------|
//! | 有候选 & 有命中 registry & 有 source            | Exact       | Some(registered) | ExactExecution    | 0.9       |
//! | 有候选 & 有命中 registry & 无 source            | Partial     | Some(registered) | PartialExecution  | 0.6       |
//! | 有候选 & 无命中 registry & 有 source            | Guided      | None             | GuidedRuling      | 0.4       |
//! | 无候选 & 有 source                              | SourceOnly  | None             | SourceOnly        | 0.3       |
//! | 其余（什么都没有）                              | Unsupported | None             | SourceOnly        | 0.0       |

use std::collections::HashSet;
use trpg_model::{
    AssetFacet, BindingPlan, BindingVerdict, ExecutionTier, NeedResolutionTrace, SourceRef,
    CAP_ACTOR_PATCH, CAP_CHECK_COUNT_FACES, CAP_CHECK_MEET_OR_BEAT, CAP_CHECK_OPPOSED,
    CAP_CHECK_ROLL_UNDER, CAP_CLOCK_TICK, CAP_RESOURCE_DELTA, CAP_TABLE_LOOKUP,
    CAP_VISIBILITY_REVEAL, ALL_CAPABILITIES,
};

/// 已注册 capability id 集合。resolver 按 `has` 判定候选是否可机械执行。
pub struct CapabilityRegistry {
    ids: HashSet<String>,
}

impl CapabilityRegistry {
    /// 注册全部 `trpg_model::ALL_CAPABILITIES`（11 个稳定 capability）。
    pub fn with_defaults() -> Self {
        let ids = ALL_CAPABILITIES.iter().map(|c| c.to_string()).collect();
        Self { ids }
    }

    /// 该 capability id 是否已注册。
    pub fn has(&self, id: &str) -> bool {
        self.ids.contains(id)
    }
}

/// 纯函数：给定 need_kind + 其 facets + registry，产出**单个** `BindingPlan`。
///
/// 确定性：`binding_id = "bind:{need_kind}"`（同输入恒同输出，绝不用随机 / uuid）。
pub fn resolve_binding(
    need_kind: &str,
    facets: &[AssetFacet],
    reg: &CapabilityRegistry,
) -> BindingPlan {
    // 跨 facets 汇总候选（保序 + 去重）。
    let mut candidates: Vec<String> = Vec::new();
    for facet in facets {
        for c in &facet.binding_candidates {
            if !candidates.contains(c) {
                candidates.push(c.clone());
            }
        }
    }

    // 跨 facets 汇总 source_refs（保序）。
    let mut source_refs: Vec<SourceRef> = Vec::new();
    for facet in facets {
        for sr in &facet.source_refs {
            source_refs.push(sr.clone());
        }
    }

    // 候选中命中 registry 的子集（保序）。
    let registered: Vec<String> = candidates
        .iter()
        .filter(|c| reg.has(c))
        .cloned()
        .collect();

    let has_source = !source_refs.is_empty();
    let has_candidates = !candidates.is_empty();
    let has_registered = !registered.is_empty();

    let (verdict, capability, execution_tier, confidence, unresolved_reason) = if has_candidates
        && has_registered
        && has_source
    {
        (
            BindingVerdict::Exact,
            Some(registered[0].clone()),
            ExecutionTier::ExactExecution,
            0.9_f32,
            None,
        )
    } else if has_candidates && has_registered && !has_source {
        (
            BindingVerdict::Partial,
            Some(registered[0].clone()),
            ExecutionTier::PartialExecution,
            0.6_f32,
            None,
        )
    } else if has_candidates && !has_registered && has_source {
        (
            BindingVerdict::Guided,
            None,
            ExecutionTier::GuidedRuling,
            0.4_f32,
            Some("no registered capability for candidates".to_string()),
        )
    } else if !has_candidates && has_source {
        (
            BindingVerdict::SourceOnly,
            None,
            ExecutionTier::SourceOnly,
            0.3_f32,
            None,
        )
    } else {
        (
            BindingVerdict::Unsupported,
            None,
            ExecutionTier::SourceOnly,
            0.0_f32,
            Some("no facet candidates or source".to_string()),
        )
    };

    BindingPlan {
        binding_id: format!("bind:{need_kind}"),
        need_kind: need_kind.to_string(),
        asset_ids: vec![], // 影子阶段尚无 asset store
        capability,
        execution_tier,
        verdict,
        confidence,
        source_refs,
        unresolved_reason,
    }
}

/// need_kind → 候选 capability 的**启发式**映射（advisory 绑定用）。
///
/// 注意：这是 advisory 阶段的启发式（按 need_kind 大类粗映射候选 capability）；
/// 当 binding 真正接管执行时会按 facet 数据细化（refined when binding takes over execution）。
fn candidates_for_need_kind(k: &str) -> Vec<String> {
    match k {
        "rule" => vec![
            CAP_CHECK_MEET_OR_BEAT.to_string(),
            CAP_CHECK_ROLL_UNDER.to_string(),
            CAP_CHECK_COUNT_FACES.to_string(),
            CAP_CHECK_OPPOSED.to_string(),
            CAP_TABLE_LOOKUP.to_string(),
        ],
        "parameter" => vec![CAP_RESOURCE_DELTA.to_string(), CAP_ACTOR_PATCH.to_string()],
        "material" => vec![CAP_ACTOR_PATCH.to_string(), CAP_TABLE_LOOKUP.to_string()],
        "scene" => vec![
            CAP_VISIBILITY_REVEAL.to_string(),
            CAP_CLOCK_TICK.to_string(),
        ],
        "entity" => vec![CAP_ACTOR_PATCH.to_string()],
        _ => vec![],
    }
}

/// 轻量 adapter：从既有 `NeedResolutionTrace` 派生最小 `AssetFacet`，按 need_kind 分组。
///
/// 每条 trace → 一个 facet（facet_kind = trace.need_kind、binding_candidates 按 need_kind 启发式映射、
/// confidence = 0.5、source_refs 透传）。返回 `(need_kind, facets)` 列表（按 need_kind 分组，保序）。
pub fn facets_from_need_traces(
    need_traces: &[NeedResolutionTrace],
) -> Vec<(String, Vec<AssetFacet>)> {
    let mut groups: Vec<(String, Vec<AssetFacet>)> = Vec::new();
    for trace in need_traces {
        let facet = AssetFacet {
            facet_kind: trace.need_kind.clone(),
            binding_candidates: candidates_for_need_kind(&trace.need_kind),
            confidence: 0.5,
            source_refs: trace.source_refs.clone(),
        };
        match groups.iter_mut().find(|(k, _)| *k == trace.need_kind) {
            Some((_, facets)) => facets.push(facet),
            None => groups.push((trace.need_kind.clone(), vec![facet])),
        }
    }
    groups
}

/// 便利函数：adapter + 逐组 resolve，产出 advisory `BindingPlan` 列表（trpg-gm T3 入口）。
pub fn shadow_bind(
    need_traces: &[NeedResolutionTrace],
    reg: &CapabilityRegistry,
) -> Vec<BindingPlan> {
    facets_from_need_traces(need_traces)
        .into_iter()
        .map(|(need_kind, facets)| resolve_binding(&need_kind, &facets, reg))
        .collect()
}

#[cfg(test)]
mod tests {
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
}

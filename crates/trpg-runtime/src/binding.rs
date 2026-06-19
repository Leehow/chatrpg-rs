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
    AssetFacet, BindingPlan, BindingVerdict, ExecutionTier, NeedResolutionTrace, RuleKernel,
    SourceRef, ALL_CAPABILITIES, CAP_ACTOR_PATCH, CAP_CHECK_COUNT_FACES, CAP_CHECK_MEET_OR_BEAT,
    CAP_CHECK_OPPOSED, CAP_CHECK_ROLL_UNDER, CAP_CLOCK_TICK, CAP_RESOURCE_DELTA, CAP_TABLE_LOOKUP,
    CAP_VISIBILITY_REVEAL,
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
    let registered: Vec<String> = candidates.iter().filter(|c| reg.has(c)).cloned().collect();

    let has_source = !source_refs.is_empty();
    let has_candidates = !candidates.is_empty();
    let has_registered = !registered.is_empty();

    let (verdict, capability, execution_tier, confidence, unresolved_reason) =
        if has_candidates && has_registered && has_source {
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
/// 仅经 need_kind 启发式（无 kernel）；back-compat 委派给 `shadow_bind_with_kernel(None)`。
pub fn shadow_bind(
    need_traces: &[NeedResolutionTrace],
    reg: &CapabilityRegistry,
) -> Vec<BindingPlan> {
    shadow_bind_with_kernel(need_traces, None, reg)
}

/// kernel.dice_core.compare 值 → 检定 capability id 的**确定性**映射（无 LLM、无规则集名）。
/// 这是通用 check-model 名（roll_under/meet_or_beat/count_faces），非规则集名 → 守卫放行。
/// 未知/空 → None（fail-soft，不造 facet）。
fn check_capability_for_compare(compare: &str) -> Option<&'static str> {
    match compare {
        "roll_under" => Some(CAP_CHECK_ROLL_UNDER),
        "meet_or_beat" => Some(CAP_CHECK_MEET_OR_BEAT),
        "count_faces" => Some(CAP_CHECK_COUNT_FACES),
        _ => None,
    }
}

/// 纯函数（确定性、无 LLM/DB/随机/时钟）：从已解析的 `RuleKernel` 派生权威 `AssetFacet`。
///
/// - **check_model facet**：读 `kernel.dice_core.compare` 字符串 → `check_capability_for_compare`
///   映射到对应检定 capability；facet_kind="check_model"、binding_candidates=[该 capability]、
///   confidence=0.9、source_refs=kernel.source_refs（**有据** → resolve_binding 出 Exact）。
///   compare 未知/缺失 → 不产 check facet（fail-soft）。
/// - **resource facet**：若 `kernel.resource_tracks` 非空 → 产一条聚合 resource facet
///   （facet_kind="resource"、binding_candidates=[CAP_RESOURCE_DELTA]、confidence=0.8、
///   source_refs=kernel.source_refs）。
/// - 无可用 dice_core/resources → 返回空 Vec（fail-soft）。
pub fn facets_from_kernel(kernel: &RuleKernel) -> Vec<AssetFacet> {
    let mut facets: Vec<AssetFacet> = Vec::new();

    let compare = kernel
        .dice_core
        .get("compare")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if let Some(cap) = check_capability_for_compare(compare) {
        facets.push(AssetFacet {
            facet_kind: "check_model".to_string(),
            binding_candidates: vec![cap.to_string()],
            confidence: 0.9,
            source_refs: kernel.source_refs.clone(),
        });
    }

    if !kernel.resource_tracks.is_empty() {
        facets.push(AssetFacet {
            facet_kind: "resource".to_string(),
            binding_candidates: vec![CAP_RESOURCE_DELTA.to_string()],
            confidence: 0.8,
            source_refs: kernel.source_refs.clone(),
        });
    }

    facets
}

/// 便利函数（trpg-gm 影子入口）：need-trace 启发式计划 **加上**（kernel 存在时）从已解析
/// kernel 派生的权威计划——`ruleset_check`（dice_core→检定 capability，有据 → Exact）与
/// `ruleset_resource`（resource_tracks → CAP_RESOURCE_DELTA）。kernel=None 则等价旧 `shadow_bind`。
///
/// 仍是 advisory / SHADOW：只充实 trace.binding_trace，**零行为变更**（不改 emit/结算/状态）。
pub fn shadow_bind_with_kernel(
    need_traces: &[NeedResolutionTrace],
    kernel: Option<&RuleKernel>,
    reg: &CapabilityRegistry,
) -> Vec<BindingPlan> {
    let mut plans: Vec<BindingPlan> = facets_from_need_traces(need_traces)
        .into_iter()
        .map(|(need_kind, facets)| resolve_binding(&need_kind, &facets, reg))
        .collect();

    if let Some(kernel) = kernel {
        let kernel_facets = facets_from_kernel(kernel);
        let check_facets: Vec<AssetFacet> = kernel_facets
            .iter()
            .filter(|f| f.facet_kind == "check_model")
            .cloned()
            .collect();
        if !check_facets.is_empty() {
            plans.push(resolve_binding("ruleset_check", &check_facets, reg));
        }
        let resource_facets: Vec<AssetFacet> = kernel_facets
            .iter()
            .filter(|f| f.facet_kind == "resource")
            .cloned()
            .collect();
        if !resource_facets.is_empty() {
            plans.push(resolve_binding("ruleset_resource", &resource_facets, reg));
        }
    }

    plans
}

#[cfg(test)]
#[path = "binding_tests.rs"]
mod tests;

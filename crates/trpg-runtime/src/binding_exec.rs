//! Binding takeover（检定竖切）：让影子 BindingResolver **第一次驱动一次结算**。
//!
//! 设计见 `docs/superpowers/specs/2026-06-17-binding-takeover-vertical-slice-design.md`。
//! 核心：在检定结算咽喉点（runtime `resolve_outcome_with_opposition`）按 `BindingPlan` 的
//! verdict/tier/capability **门控 + 分发**到检定 capability executor；executor 内部仍调权威
//! `ContestService::resolve_outcome`——**indirection 是新的、算法是旧的 → 天然等价**。
//!
//! env `TRPG_BINDING_TAKEOVER` **默认开**（竖切已逐字段等价证明，见
//! `tests/live_binding_takeover_equiv.rs`）：Exact-可绑定的检定经 binding executor 执行
//! （内部仍调权威 resolve_outcome → 等价），非 Exact → fail-closed 回退原路径 + warn。
//! 设 `0/false/off/no` 可关回纯原路径（逃生阀）。ExecutionTier 在此**第一次 load-bearing**。

use crate::binding::{facets_from_kernel, resolve_binding, CapabilityRegistry};
use trpg_contest::ContestService;
use trpg_model::{
    AssetFacet, BindingPlan, BindingVerdict, CheckContract, DiceRollRecord, ExecutionTier,
    RuleKernel, CAP_CHECK_COUNT_FACES, CAP_CHECK_MEET_OR_BEAT, CAP_CHECK_ROLL_UNDER,
};

/// 是否开启 binding takeover（检定结算经 BindingResolver 驱动）。**默认开**（已等价证明）。
pub fn binding_takeover_enabled() -> bool {
    takeover_flag_from(std::env::var("TRPG_BINDING_TAKEOVER").ok())
}

/// 纯解析：未设置 => 开（binding 接管）；仅 `0/false/off/no` => 关回纯原路径。拆出纯函数以便
/// 不改写进程级 env 即可单测（并发 set_var/getenv 在 glibc 上是数据竞争，见 lazy_object_schema_policy）。
fn takeover_flag_from(raw: Option<String>) -> bool {
    match raw {
        Some(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        ),
        None => true,
    }
}

/// 从已解析 `RuleKernel` 确定性派生**检定** BindingPlan（无 LLM/DB/随机）。
/// 复用 `facets_from_kernel`（dice_core.compare → check_model facet，有 source → Exact）。
pub fn check_binding_plan(kernel: &RuleKernel) -> BindingPlan {
    let check_facets: Vec<AssetFacet> = facets_from_kernel(kernel)
        .into_iter()
        .filter(|f| f.facet_kind == "check_model")
        .collect();
    resolve_binding(
        "ruleset_check",
        &check_facets,
        &CapabilityRegistry::with_defaults(),
    )
}

/// 该 plan 是否授权 Rust 机械执行检定：verdict=Exact + tier=ExactExecution + capability 是
/// 已知检定能力（roll_under/meet_or_beat/count_faces）。否则上游 fail-closed 回退原路径。
pub fn plan_authorizes_check_exec(plan: &BindingPlan) -> bool {
    plan.verdict == BindingVerdict::Exact
        && plan.execution_tier == ExecutionTier::ExactExecution
        && matches!(
            plan.capability.as_deref(),
            Some(CAP_CHECK_ROLL_UNDER) | Some(CAP_CHECK_MEET_OR_BEAT) | Some(CAP_CHECK_COUNT_FACES)
        )
}

/// 经 binding 执行检定结算：按 `plan.capability` 分发到对应检定 executor。
/// 每个 executor = 薄包装，内部仍调权威 `resolve_outcome`（含其 opposed/pool 覆盖逻辑）→
/// 输出与原路径**逐字节等价**。binding 在此 load-bearing：capability 选执行器、tier 已门控。
pub async fn execute_check_with_binding(
    svc: &ContestService,
    contract: &CheckContract,
    roll: &DiceRollRecord,
    defender_roll: Option<&DiceRollRecord>,
    plan: &BindingPlan,
) -> anyhow::Result<serde_json::Value> {
    match plan.capability.as_deref() {
        Some(CAP_CHECK_ROLL_UNDER) | Some(CAP_CHECK_MEET_OR_BEAT) | Some(CAP_CHECK_COUNT_FACES) => {
            tracing::info!(
                capability = plan.capability.as_deref().unwrap_or(""),
                tier = ?plan.execution_tier,
                check_id = %contract.check_id,
                "binding takeover: check resolved via capability executor"
            );
            svc.resolve_outcome(contract, roll, defender_roll).await
        }
        // 上游 plan_authorizes_check_exec 已保证只在 Exact+CAP_CHECK_* 进来；
        // 兜底 fail-safe 仍走权威 resolve（绝不静默改算法）。
        _ => svc.resolve_outcome(contract, roll, defender_roll).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::BindingPlan;

    fn plan_with(cap: Option<&str>, verdict: BindingVerdict, tier: ExecutionTier) -> BindingPlan {
        BindingPlan {
            binding_id: "bind:ruleset_check".into(),
            need_kind: "ruleset_check".into(),
            asset_ids: vec![],
            capability: cap.map(|c| c.to_string()),
            execution_tier: tier,
            verdict,
            confidence: 0.9,
            source_refs: vec![],
            unresolved_reason: None,
        }
    }

    #[test]
    fn takeover_default_on_and_disable_values() {
        assert!(
            takeover_flag_from(None),
            "未设置必须默认开（binding 接管，已等价证明）"
        );
        for off in ["0", "false", "off", "no", "OFF", " No "] {
            assert!(!takeover_flag_from(Some(off.into())), "{off} 应关回原路径");
        }
        for on in ["1", "true", "yes", "on", "whatever"] {
            assert!(takeover_flag_from(Some(on.into())), "{on} 应为开");
        }
    }

    #[test]
    fn exact_check_plan_authorizes_exec() {
        let p = plan_with(
            Some(CAP_CHECK_ROLL_UNDER),
            BindingVerdict::Exact,
            ExecutionTier::ExactExecution,
        );
        assert!(plan_authorizes_check_exec(&p));
    }

    #[test]
    fn non_exact_or_non_check_plans_fail_closed() {
        // Partial（无 source）→ 不授权（回退原路径）。
        assert!(!plan_authorizes_check_exec(&plan_with(
            Some(CAP_CHECK_ROLL_UNDER),
            BindingVerdict::Partial,
            ExecutionTier::PartialExecution
        )));
        // Guided（无命中 capability）→ 不授权。
        assert!(!plan_authorizes_check_exec(&plan_with(
            None,
            BindingVerdict::Guided,
            ExecutionTier::GuidedRuling
        )));
        // 非检定 capability（如 resource）→ 不授权（本竖切只接管检定）。
        assert!(!plan_authorizes_check_exec(&plan_with(
            Some("resource.delta"),
            BindingVerdict::Exact,
            ExecutionTier::ExactExecution
        )));
    }
}

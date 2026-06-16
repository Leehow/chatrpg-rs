//! Flight Recorder 组装 + 阶段错误分级（obs slice T2，spec §4.2/§4.4）。
//!
//! 把 execute.rs 的 run_pipeline 收尾逻辑里"组装 TurnTrace / 判定阶段错误处置"的
//! 部分抽到本模块，让 execute.rs 保持精简（≤400 行）。三件事：
//! - `phase_error_policy(PhaseId)`：阶段失败处置策略（中止 / 警告续跑 / 后台仅 warn）。
//! - `failure_kind_for(PhaseId)`：落 turns.failure_kind 的归类字符串。
//! - `build_turn_trace(..)`：从已装配的 TurnContext（need_trace + BP hash）+ 失败/警告
//!   组装一条 `TurnTrace`，供 run_pipeline 在成功/失败两路径 write-through 落库（fail-soft）。
//!
//! 理念守卫：trace 只存 hash + need 来源元数据，**不存私密 prompt 正文**（避免 explain
//! 泄底，spec §5.4）；narration_hash 是念白的稳定 hash，非念白原文。

use crate::turn_loop::TurnContext;
use crate::turn_plan::PhaseId;
use crate::execute::OwnedTurnRequest;
use std::sync::LazyLock;
use trpg_model::{PhaseErrorPolicy, TurnFailureRecord, TurnTrace};

/// 进程级影子 capability registry（advisory）。一次性建、只读、纯查询，
/// 影子绑定全程不改任何 emit/结算/状态，故可安全跨回合共享。
static SHADOW_REGISTRY: LazyLock<trpg_runtime::CapabilityRegistry> =
    LazyLock::new(trpg_runtime::CapabilityRegistry::with_defaults);

/// 确定性阶段失败（dispatch_deterministic 的 Err 载荷）：带触发阶段 + 人读信息，
/// caller 据此构造 TurnFailed 事件 + TurnFailureRecord + turns.failure_kind。
#[derive(Debug, Clone)]
pub(crate) struct PhaseFailure {
    pub phase: PhaseId,
    pub message: String,
}

/// 阶段失败处置策略分类器（spec §4.2）。
/// - ModeInference / ContextAssembly / Finalize（save_turn）→ **AbortTurn**（critical，中止回合）。
/// - VerifyAfterStream → **EmitWarningContinue**（叙事已流出，校验失败发 TurnWarning 续跑）。
/// - 其余（AuditLearning / SceneNavigate / CarryoverDebt / 深抽-frontier 等 heavy）→
///   **BackgroundWarnOnly**（D2 隔离内仅 `warn!`，不发事件不阻塞）。
pub(crate) fn phase_error_policy(phase: PhaseId) -> PhaseErrorPolicy {
    match phase {
        PhaseId::ModeInference | PhaseId::ContextAssembly | PhaseId::Finalize => {
            PhaseErrorPolicy::AbortTurn
        }
        PhaseId::VerifyAfterStream => PhaseErrorPolicy::EmitWarningContinue,
        _ => PhaseErrorPolicy::BackgroundWarnOnly,
    }
}

/// 落 turns.failure_kind 的归类字符串（spec §2 表）。
/// 成功回合不调本函数（failure_kind 保持 NULL）。
pub(crate) fn failure_kind_for(phase: PhaseId) -> &'static str {
    match phase {
        PhaseId::ModeInference => "failed_mode",
        PhaseId::ContextAssembly => "failed_context",
        PhaseId::Finalize => "failed_finalize",
        _ => "failed_other",
    }
}

/// 空串 hash → None（CompiledContext::default() 的 hash 为空串；TurnTrace 用 Option 表"无"）。
fn hash_opt(h: &str) -> Option<String> {
    if h.is_empty() { None } else { Some(h.to_string()) }
}

/// 组装一条 TurnTrace（成功/失败两路径共用）。
///
/// 从已装配的 `ctx.compiled()` 直取 need_trace（克隆）+ BP1/BP2/BP3 hash（空串映 None）；
/// `signal` 是短字符串（caller 传 `format!("{signal:?}")` 等）；`narration` 为 Some 时算
/// 稳定 hash 进 narration_hash（用 trpg_model::sha256_hex，确定性）。`failure`/`warnings`/
/// `pp_lifecycle`/`phases_run` 由 caller 按路径填。
pub(crate) fn build_turn_trace(
    req: &OwnedTurnRequest,
    ctx: &TurnContext,
    signal: String,
    phases_run: Vec<String>,
    narration: Option<&str>,
    failure: Option<TurnFailureRecord>,
    warnings: Vec<String>,
    pp_lifecycle: &str,
) -> TurnTrace {
    let compiled = ctx.compiled();
    let mut trace = TurnTrace::new(&req.request.turn_id, &req.request.session_id);
    trace.phases_run = phases_run;
    trace.need_trace = compiled.need_trace.clone();
    // 影子绑定（advisory，优化2 #3 起步）：按本回合 Need traces 产 BindingPlan 记进 Flight Recorder。
    // 纯函数、确定性、零行为变更——不改任何 emit/结算/状态，仅充实 trace。
    trace.binding_trace = trpg_runtime::shadow_bind(&compiled.need_trace, &SHADOW_REGISTRY);
    trace.bp1_hash = hash_opt(&compiled.prefix_hash);
    trace.bp2_hash = hash_opt(&compiled.pinned_hash);
    trace.bp3_hash = hash_opt(&compiled.dynamic_hash);
    trace.signal = signal;
    trace.failure = failure;
    trace.warnings = warnings;
    trace.pp_lifecycle = pp_lifecycle.to_string();
    trace.narration_hash = narration.map(trpg_model::sha256_hex);
    trace
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execute::OwnedTurnRequest;
    use crate::turn_loop::TurnContext;
    use trpg_model::{
        BindingVerdict, CompiledContext, ContextRequest, NeedResolutionTrace, RuntimeState,
        SourceRef, TokenBudget, VisibilityProfile,
    };

    fn req_with(turn_id: &str, session_id: &str) -> OwnedTurnRequest {
        OwnedTurnRequest {
            request: ContextRequest {
                ruleset_id: "rs".into(),
                module_id: None,
                session_id: session_id.into(),
                turn_id: turn_id.into(),
                viewer: VisibilityProfile::gm(),
                token_budget: TokenBudget::default(),
            },
            state: RuntimeState { ruleset_id: "rs".into(), ..Default::default() },
            user_input: "go".into(),
            history: vec![],
            recent_transcript: None,
            module_id: None,
            data_dir: std::env::temp_dir(),
        }
    }

    // 三分支：critical(Abort) / verify(WarnContinue) / heavy(BackgroundWarn)。
    #[test]
    fn phase_error_policy_three_branches() {
        for p in [PhaseId::ModeInference, PhaseId::ContextAssembly, PhaseId::Finalize] {
            assert_eq!(phase_error_policy(p), PhaseErrorPolicy::AbortTurn, "{p:?} must AbortTurn");
        }
        assert_eq!(
            phase_error_policy(PhaseId::VerifyAfterStream),
            PhaseErrorPolicy::EmitWarningContinue,
        );
        for p in [
            PhaseId::AuditLearning,
            PhaseId::SceneNavigate,
            PhaseId::CarryoverDebt,
            PhaseId::Gate,
        ] {
            assert_eq!(
                phase_error_policy(p),
                PhaseErrorPolicy::BackgroundWarnOnly,
                "{p:?} must BackgroundWarnOnly",
            );
        }
    }

    // failure_kind_for 映射：mode/context/finalize 各自归类，其余 failed_other。
    #[test]
    fn failure_kind_for_mapping() {
        assert_eq!(failure_kind_for(PhaseId::ModeInference), "failed_mode");
        assert_eq!(failure_kind_for(PhaseId::ContextAssembly), "failed_context");
        assert_eq!(failure_kind_for(PhaseId::Finalize), "failed_finalize");
        assert_eq!(failure_kind_for(PhaseId::VerifyAfterStream), "failed_other");
        assert_eq!(failure_kind_for(PhaseId::AgentLoop), "failed_other");
    }

    // build_turn_trace 从 ctx.compiled 填 BP hash（空串→None）+ need_trace + failure。
    #[test]
    fn build_turn_trace_populates_bp_need_and_failure() {
        let req = req_with("turn-7", "sess-3");
        let mut ctx = TurnContext::new();
        let need = NeedResolutionTrace {
            need_kind: "rule".into(),
            source_refs: vec![],
            reason: "Sanity roll".into(),
            block_count: 2,
        };
        ctx.set_compiled_for_test(CompiledContext {
            prefix_hash: "sha256:bp1".into(),
            pinned_hash: "sha256:bp2".into(),
            dynamic_hash: String::new(), // 空串 → None
            need_trace: vec![need.clone()],
            ..Default::default()
        });

        let failure = TurnFailureRecord {
            phase: "context_assembly".into(),
            message: "over budget".into(),
            failure_kind: "failed_context".into(),
        };
        let trace = build_turn_trace(
            &req,
            &ctx,
            "AbortTurn".into(),
            vec!["context_assembly".into()],
            Some("念白正文。"),
            Some(failure.clone()),
            vec!["a warning".into()],
            "failed",
        );

        assert_eq!(trace.turn_id, "turn-7");
        assert_eq!(trace.session_id, "sess-3");
        assert_eq!(trace.bp1_hash.as_deref(), Some("sha256:bp1"));
        assert_eq!(trace.bp2_hash.as_deref(), Some("sha256:bp2"));
        assert_eq!(trace.bp3_hash, None, "空 dynamic_hash 必须映 None");
        assert_eq!(trace.need_trace, vec![need]);
        assert_eq!(trace.failure, Some(failure));
        assert_eq!(trace.warnings, vec!["a warning".to_string()]);
        assert_eq!(trace.pp_lifecycle, "failed");
        assert_eq!(trace.signal, "AbortTurn");
        assert_eq!(trace.phases_run, vec!["context_assembly".to_string()]);
        // narration_hash 必须是稳定 hash（非原文）。
        let h = trace.narration_hash.expect("narration hash present");
        assert!(h.starts_with("sha256:"), "narration hash 必须是 sha256: 前缀: {h}");
        assert_eq!(h, trpg_model::sha256_hex("念白正文。"), "确定性 hash");
    }

    // 影子绑定（T3）：build_turn_trace 从同一份 need_trace 产 binding_trace（advisory）。
    // need_kind="rule" + 带 source_ref → 候选命中 registry + 有来源 → verdict=Exact。
    #[test]
    fn build_turn_trace_populates_binding_trace_from_need_trace() {
        let req = req_with("turn-9", "sess-9");
        let mut ctx = TurnContext::new();
        let need = NeedResolutionTrace {
            need_kind: "rule".into(),
            source_refs: vec![SourceRef {
                source_id: "coc_rulebook".into(),
                page: Some(88),
                ..Default::default()
            }],
            reason: "Sanity roll".into(),
            block_count: 1,
        };
        ctx.set_compiled_for_test(CompiledContext {
            need_trace: vec![need],
            ..Default::default()
        });

        let trace = build_turn_trace(
            &req,
            &ctx,
            "TurnComplete".into(),
            vec!["context_assembly".into()],
            None,
            None,
            vec![],
            "complete",
        );

        assert_eq!(trace.binding_trace.len(), 1, "rule need 应产一条 BindingPlan");
        let plan = &trace.binding_trace[0];
        assert_eq!(plan.need_kind, "rule");
        assert_eq!(plan.verdict, BindingVerdict::Exact, "候选命中 registry + 有来源 → Exact");
        assert!(plan.capability.is_some(), "Exact 必带命中的 capability");
    }

    // 失败路径（空 need_trace）：shadow_bind 产空 → binding_trace 空（零行为变更佐证）。
    #[test]
    fn build_turn_trace_empty_need_trace_yields_empty_binding_trace() {
        let req = req_with("turn-0", "sess-0");
        let ctx = TurnContext::new(); // 默认 CompiledContext，need_trace 为空
        let trace = build_turn_trace(
            &req,
            &ctx,
            "AbortTurn".into(),
            vec![],
            None,
            None,
            vec![],
            "failed",
        );
        assert!(trace.binding_trace.is_empty(), "空 need_trace → binding_trace 必空");
    }
}

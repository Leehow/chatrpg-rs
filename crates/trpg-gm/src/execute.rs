//! 统一回合执行器 facade（R1 §4.2）：CLI/API 唯一回合入口。
//! 按声明式 CANONICAL_TURN_PLAN 解释回合管线，产 TurnEvent 流。
//! transport（CLI 前台 drain / API 后台 spawn drain）决定尾部前后台，
//! 执行器本身不分前后台——只按 plan 顺序解释、经 mpsc 发事件。
use crate::turn_event::TurnEvent;
use crate::turn_loop::{GmLoop, GmTurnInput, TurnContext};
use crate::turn_plan::{PhaseId, PhaseKind, TurnPhasePlan};
use tokio_stream::wrappers::ReceiverStream;

/// owned 版回合请求（spawn 进 tokio 任务需 'static / owned —— 借用版
/// GmTurnInput<'a> 无法跨 spawn 边界）。装配方从各 transport 的借用上下文
/// clone 出 owned 副本传入。
pub struct OwnedTurnRequest {
    pub request: trpg_model::ContextRequest,
    pub state: trpg_model::RuntimeState,
    pub user_input: String,
    pub history: Vec<trpg_model::ChatMessage>,
    pub recent_transcript: Option<String>,
    pub module_id: Option<String>,
    pub data_dir: std::path::PathBuf,
}

/// agent_loop phase 的终态信号——决定尾部 phase 取舍（早返 vs 全尾部）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentSignal {
    Narration,
    AwaitingPlayerRoll,
}

/// 纯选择器：plan + agent 终态 + 条件位 → 实际执行的 phase 序列。
/// 无 DB / 无 IO ⇒ 确定性可单测。解释器逐项消费它。
/// 规则（契约「AwaitingPlayerRoll 早返语义」+「conditional phase」）：
/// - head（Deterministic）+ AgentLoop 永远保留；
/// - AwaitingPlayerRoll：尾部只保留 VerifyAfterStream + Finalize，
///   丢弃 SceneNavigate / AuditLearning / CarryoverDebt；
/// - Narration：尾部全保留，再按条件位过滤 conditional phase；
/// - SceneNavigate 仅 module_present；CarryoverDebt 仅 has_pending_obligations。
pub(crate) fn select_phases(
    plan: &'static [TurnPhasePlan],
    signal: AgentSignal,
    module_present: bool,
    has_pending_obligations: bool,
) -> Vec<TurnPhasePlan> {
    plan.iter()
        .copied()
        .filter(|p| match p.kind {
            PhaseKind::Deterministic => true, // 头部确定性步
            PhaseKind::AgentLoop => true,     // body 必跑
            PhaseKind::Postprocess => match signal {
                AgentSignal::AwaitingPlayerRoll => matches!(
                    p.id,
                    PhaseId::VerifyAfterStream | PhaseId::Finalize
                ),
                AgentSignal::Narration => match p.id {
                    PhaseId::SceneNavigate => module_present,
                    PhaseId::CarryoverDebt => has_pending_obligations,
                    _ => true,
                },
            },
        })
        .collect()
}

/// 统一回合执行器：建 mpsc channel → spawn 任务跑 pipeline → event 经 tx
/// 出 → 返回 ReceiverStream<TurnEvent>。transport 决定尾部前台(CLI 同步
/// drain)/后台(API spawn drain)。GmLoop 按 owned req 'static 跨 spawn。
pub fn execute_turn(
    mut gm: GmLoop,
    req: OwnedTurnRequest,
    plan: &'static [TurnPhasePlan],
) -> ReceiverStream<TurnEvent> {
    // 容量 128 对齐 trpg-api play_turn_sse channel——逐 token Delta 高频，
    // 满了背压而非丢，保证真流式不缓冲不丢失。
    let (tx, rx) = tokio::sync::mpsc::channel::<TurnEvent>(128);
    tokio::spawn(async move {
        run_pipeline(&mut gm, req, plan, &tx).await;
    });
    ReceiverStream::new(rx)
}

/// 解释器主体：按 plan 顺序解释。Deterministic 头部逐 phase 调 gm.phase_*（经
/// 本地 TurnContext 累积）；AgentLoop 调 run_agent_loop（产 Delta /
/// AwaitingPlayerRoll 经 tx 实时发）；据 agent 终态 select_phases 选尾部 phase。
/// 任何尾部 phase 失败由 gm.phase_* 内部 warn 吞错（D2 错误后置勘误不中断）。
async fn run_pipeline(
    gm: &mut GmLoop,
    req: OwnedTurnRequest,
    plan: &'static [TurnPhasePlan],
    tx: &tokio::sync::mpsc::Sender<TurnEvent>,
) {
    let input = GmTurnInput {
        request: &req.request,
        state: &req.state,
        user_input: &req.user_input,
        history: &req.history,
        recent_transcript: req.recent_transcript.as_deref(),
    };
    let mut ctx = TurnContext::new();

    // —— 1. 确定性头部：按 plan 顺序跑所有 Deterministic phase ——
    // mode_inference / context_assembly fail-closed 返 Err 终止回合（与
    // run_gm_turn 一致）：直接收尾发 TurnComplete（Narration 空文本）后早返。
    for phase in plan.iter().filter(|p| p.kind == PhaseKind::Deterministic) {
        if !dispatch_deterministic(gm, &mut ctx, &input, phase.id).await {
            let _ = tx.send(TurnEvent::TurnComplete { outcome: gm.take_outcome(&mut ctx) }).await;
            return;
        }
    }

    // —— 2. AgentLoop body：产 Delta / AwaitingPlayerRoll 经 tx，返回终态信号 ——
    let signal = gm.run_agent_loop(&mut ctx, &input, tx).await;

    // —— 3. 尾部：据 agent 终态 + 运行期条件位选 phase ——
    let module_present = req.module_id.is_some();
    let has_pending = gm.has_pending_obligations();
    for phase in select_phases(plan, signal, module_present, has_pending) {
        if phase.kind != PhaseKind::Postprocess { continue; }
        if phase.id == PhaseId::VerifyAfterStream {
            let _ = tx.send(TurnEvent::PostprocessScheduled).await;
        }
        dispatch_postprocess(gm, &mut ctx, &input, phase.id, signal, tx).await;
    }

    // —— 4. 收尾事件：把 agent 终态折成 TurnComplete ——
    let outcome = gm.take_outcome(&mut ctx);
    let _ = tx.send(TurnEvent::TurnComplete { outcome }).await;
}

/// 确定性头部 phase 分发。返回 false ⇒ fail-closed 终止（mode/context Err）。
async fn dispatch_deterministic(
    gm: &mut GmLoop,
    ctx: &mut TurnContext,
    input: &GmTurnInput<'_>,
    id: PhaseId,
) -> bool {
    match id {
        PhaseId::RecordPlayerAction => gm.phase_record_player_action(ctx, input).await,
        PhaseId::RefreshLiveDerived => gm.phase_refresh_live_derived(ctx, input).await,
        PhaseId::Reconcile => gm.phase_reconcile(ctx, input).await,
        PhaseId::Gate => gm.phase_gate(ctx, input).await,
        PhaseId::StimulusPass => gm.phase_stimulus_pass(ctx, input).await,
        PhaseId::OpposedPrepass => gm.phase_opposed_prepass(ctx, input).await,
        PhaseId::ModeInference => {
            if let Err(err) = gm.phase_mode_inference(ctx, input).await {
                tracing::warn!(error = %err, "mode_inference failed; aborting turn");
                return false;
            }
        }
        PhaseId::DebtLoad => gm.phase_debt_load(ctx, input).await,
        PhaseId::ContextAssembly => {
            if let Err(err) = gm.phase_context_assembly(ctx, input).await {
                tracing::warn!(error = %err, "context_assembly failed; aborting turn");
                return false;
            }
        }
        // AgentLoop / Postprocess 不经此路径（解释器分流）；fail-closed no-op。
        _ => {}
    }
    true
}

/// 后置尾部 phase 分发。AuditLearning 是 no-op（learning audit 已在 finalize_turn
/// 内跑，避免双跑审计）；SceneNavigate 发 SceneTransition（scene_navigator 现
/// 返 () 不回传切换详情，恒 None ⇒ 暂不发该事件）。
async fn dispatch_postprocess(
    gm: &mut GmLoop,
    ctx: &mut TurnContext,
    input: &GmTurnInput<'_>,
    id: PhaseId,
    signal: AgentSignal,
    tx: &tokio::sync::mpsc::Sender<TurnEvent>,
) {
    match id {
        PhaseId::VerifyAfterStream => gm.phase_verify_after_stream(ctx, input).await,
        PhaseId::Finalize => {
            // awaiting 终态 finalize 状态 = "awaiting_player_roll"，正常 = "ready"。
            let status = match signal {
                AgentSignal::AwaitingPlayerRoll => "awaiting_player_roll",
                AgentSignal::Narration => "ready",
            };
            gm.phase_finalize(ctx, input, status).await;
        }
        // AuditLearning：no-op——finalize_turn 内部已跑 audit_learning_for_turn
        // （含门控与自吞错）。此处不重跑，避免双 audit（CANONICAL_TURN_PLAN 把它
        // 列为独立 phase 是为表达管线完整性；实现上由 Finalize 覆盖）。
        PhaseId::AuditLearning => {}
        PhaseId::SceneNavigate => {
            if let Some(t) = gm.phase_scene_navigate_critical(ctx, input).await {
                let to = t.to.clone();
                let _ = tx.send(TurnEvent::SceneTransition { from: t.from, to: t.to, reason: t.reason }).await;
                if let Some(module_id) = input.request.module_id.as_deref() {
                    gm.phase_scene_navigate_heavy(&to, module_id).await;
                }
            }
        }
        PhaseId::CarryoverDebt => gm.phase_carryover_debt(ctx, input).await,
        _ => {}
    }
}

#[cfg(test)]
#[path = "execute_tests.rs"]
mod tests;

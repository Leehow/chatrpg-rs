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
    gm: GmLoop,
    req: OwnedTurnRequest,
    plan: &'static [TurnPhasePlan],
) -> ReceiverStream<TurnEvent> {
    // 容量 128 对齐 trpg-api play_turn_sse channel——逐 token Delta 高频，
    // 满了背压而非丢，保证真流式不缓冲不丢失。
    let (tx, rx) = tokio::sync::mpsc::channel::<TurnEvent>(128);
    tokio::spawn(async move {
        // gm 整体 move 进 pipeline（by value）：critical 用 &mut gm，heavy 把 gm/ctx/req
        // 整体 move 进另起的后台 spawn（需 'static / owned）。
        run_pipeline(gm, req, plan, &tx).await;
    });
    ReceiverStream::new(rx)
}

/// 解释器主体：按 plan 顺序解释。Deterministic 头部逐 phase 调 gm.phase_*（经
/// 本地 TurnContext 累积）；AgentLoop 调 run_agent_loop（产 Delta /
/// AwaitingPlayerRoll 经 tx 实时发）；据 agent 终态 select_phases 选尾部 phase。
///
/// R5 尾段拆 critical/heavy：critical（VerifyAfterStream + Finalize 的 save_turn +
/// SceneNavigate 的切场景决策/set_session_scene/SceneChanged）同步在 TurnComplete 前
/// await；发 TurnComplete 后把 owned gm/ctx/req move 进**另起**的 `tokio::spawn` 跑
/// heavy（memory/audit + 到场深抽/frontier + carryover），后台、失败隔离（D2）。
/// 任何尾部 phase 失败由 gm.phase_* 内部 warn 吞错（D2 错误后置勘误不中断）。
async fn run_pipeline(
    mut gm: GmLoop,
    req: OwnedTurnRequest,
    plan: &'static [TurnPhasePlan],
    tx: &tokio::sync::mpsc::Sender<TurnEvent>,
) {
    let mut ctx = TurnContext::new();
    let signal;
    let selected;
    let mut scene_commit: Option<crate::turn_loop::SceneTransitionInfo> = None;
    // input 借用 req；heavy 需 own req，故 head/critical 全放进块内，块末 drop(input)
    // 再 move req 进 heavy spawn。
    {
        let input = GmTurnInput {
            request: &req.request,
            state: &req.state,
            user_input: &req.user_input,
            history: &req.history,
            recent_transcript: req.recent_transcript.as_deref(),
        };

        // —— 1. 确定性头部：按 plan 顺序跑所有 Deterministic phase ——
        // mode_inference / context_assembly fail-closed 返 Err 终止回合（与
        // run_gm_turn 一致）：直接收尾发 TurnComplete（Narration 空文本）后早返。
        for phase in plan.iter().filter(|p| p.kind == PhaseKind::Deterministic) {
            if !dispatch_deterministic(&mut gm, &mut ctx, &input, phase.id).await {
                let _ = tx.send(TurnEvent::TurnComplete { outcome: gm.take_outcome(&mut ctx) }).await;
                return;
            }
        }

        // —— 2. AgentLoop body：产 Delta / AwaitingPlayerRoll 经 tx，返回终态信号 ——
        signal = gm.run_agent_loop(&mut ctx, &input, tx).await;

        // —— 3a. CRITICAL 尾段（同步，TurnComplete 前 await）——
        // 与 R1 同 phase 集合（select_phases），但只跑 critical 半边：VerifyAfterStream
        // （勘误判定要在 finalize 前注入 ledger 真相）→ Finalize 的 save_turn →
        // SceneNavigate 的切场景决策 + set_session_scene + SceneChanged 事件。
        // AuditLearning（finalize 的 memory/audit 半边）/ 深抽 / carryover → heavy。
        let module_present = req.module_id.is_some();
        let has_pending = gm.has_pending_obligations();
        selected = select_phases(plan, signal, module_present, has_pending);
        for phase in &selected {
            if phase.kind != PhaseKind::Postprocess { continue; }
            match phase.id {
                PhaseId::VerifyAfterStream => {
                    let _ = tx.send(TurnEvent::PostprocessScheduled).await;
                    gm.phase_verify_after_stream(&mut ctx, &input).await;
                }
                PhaseId::Finalize => {
                    // awaiting 终态 finalize 状态 = "awaiting_player_roll"，正常 = "ready"。
                    let status = match signal {
                        AgentSignal::AwaitingPlayerRoll => "awaiting_player_roll",
                        AgentSignal::Narration => "ready",
                    };
                    gm.phase_finalize(&mut ctx, &input, status).await; // R5：只 save_turn
                }
                PhaseId::SceneNavigate => {
                    if let Some(t) = gm.phase_scene_navigate_critical(&ctx, &input).await {
                        let _ = tx.send(TurnEvent::SceneTransition { from: t.from.clone(), to: t.to.clone(), reason: t.reason.clone() }).await;
                        scene_commit = Some(t);
                    }
                }
                // AuditLearning / CarryoverDebt → heavy（spawn_heavy 内跑）。
                _ => {}
            }
        }

        // —— 4. TurnComplete（critical 已落账，可继续下一回合）——
        let outcome = gm.take_outcome(&mut ctx);
        let _ = tx.send(TurnEvent::TurnComplete { outcome }).await;
        // input 在此块结束时释放对 req 的借用，下面 move req 进 heavy spawn。
    }

    // —— 3b. HEAVY 尾段（TurnComplete 后另起 spawn，gm/ctx/req 整体 move）——
    spawn_heavy(gm, ctx, req, selected, scene_commit);
}

/// HEAVY 尾段：TurnComplete 后台跑——audit/memory 写、到场深抽+frontier、carryover。
/// owned gm/ctx/req（spec：heavy 需 owned 句柄，沿用 R1 spawn 模式；errata/obligations
/// 随 gm 自带、跨回合连续性不破）。失败隔离：整个任务包在独立 `tokio::spawn` 里，
/// panic/Err 只杀该任务，绝不影响已发的 TurnComplete（D2）；外层不 unwrap/不 panic。
fn spawn_heavy(
    gm: GmLoop,
    ctx: TurnContext,
    req: OwnedTurnRequest,
    selected: Vec<TurnPhasePlan>,
    scene_commit: Option<crate::turn_loop::SceneTransitionInfo>,
) {
    tokio::spawn(async move {
        // 测试探针（heavy-only seam）：记录 heavy 进入时序 / 注入慢或 panic。
        gm.heavy_probe_enter().await;
        let mut gm = gm;
        let mut ctx = ctx;
        let input = GmTurnInput {
            request: &req.request,
            state: &req.state,
            user_input: &req.user_input,
            history: &req.history,
            recent_transcript: req.recent_transcript.as_deref(),
        };
        // turn 摘要 memory + learning audit（原 finalize_turn 的 memory/audit 半边）。
        gm.phase_finalize_heavy_memory(&ctx, &input).await;
        // 到场深抽 + frontier（仅 critical 真切了场景时）。
        if let (Some(commit), Some(module_id)) = (&scene_commit, req.module_id.as_deref()) {
            gm.phase_scene_navigate_heavy(&commit.to, module_id).await;
        }
        // carryover 债务记忆（select_phases 已据 has_pending 决定是否在 selected 里）。
        if selected.iter().any(|p| p.id == PhaseId::CarryoverDebt && p.kind == PhaseKind::Postprocess) {
            gm.phase_carryover_debt(&mut ctx, &input).await;
            // T2 锚点：在此（heavy 末）写 pp_lifecycle=complete（本任务暂不写）。
        }
        // errata 记忆已在 critical 的 phase_verify_after_stream 内落账（save_memory_event）——
        // R1 行为：verify 在 finalize 前、属 critical。heavy 不重复 errata 写。
    });
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

// R5 Task1c：旧 dispatch_postprocess 已收编进 run_pipeline 的 critical 循环
// （VerifyAfterStream/Finalize/SceneNavigate）+ spawn_heavy（AuditLearning 折成
// heavy memory/audit、CarryoverDebt）。不再有单一 postprocess 分发器。

#[cfg(test)]
#[path = "execute_tests.rs"]
mod tests;

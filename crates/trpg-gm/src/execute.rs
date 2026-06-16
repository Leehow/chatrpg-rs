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

/// #B（GPT Pro P0-1）carryover 决策纯函数：仅在 Narration 终态 + （verify 后）有未决义务时跑。
/// AwaitingPlayerRoll 不 carryover（GM 在等掷骰，回合未结，同 R1）。调用方在 heavy（verify 之后）
/// 传 `has_pending = gm.has_pending_obligations()` 的**实时**值——绝不能用 verify 前算的旧值，
/// 否则本回合 verify 新生的 retro debt 会漏掉 carryover。phase_carryover_debt 再自门控 carryover_block。
pub(crate) fn should_run_carryover(signal: AgentSignal, has_pending: bool) -> bool {
    matches!(signal, AgentSignal::Narration) && has_pending
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
    // heavy 段所需的 assistant_output：必须在块内 take_outcome 清空 ctx **之前**快照，
    // 但在块外（spawn_heavy 调用处）使用，故在此声明、块内赋值。
    let heavy_assistant_output;
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
        // heavy 段（spawn_heavy 内）跑富版回合记忆 + learning audit，需要本回合 assistant_output；
        // 但 take_outcome 会 std::mem::take 清空 ctx.visible_text / awaiting_gate，而 heavy 在
        // take_outcome 之后才 spawn。故此处（清空前）按 critical 同口径快照 assistant_output，
        // owned 传进 spawn_heavy——否则 heavy 从已清空的 ctx 现读到空串、记忆/审计每回合静默丢失。
        heavy_assistant_output = ctx.heavy_assistant_output();
        let outcome = gm.take_outcome(&mut ctx);
        let _ = tx.send(TurnEvent::TurnComplete { outcome }).await;
        // R5 高水位锚点：critical 组已落账（save_turn + 切场景），标 pp_lifecycle=critical_done，
        // 让下一回合入口守卫可放行（heavy 仍后台跑、不阻塞）。失败仅 log，绝不影响已发的 TurnComplete。
        if let Err(e) = gm.engine.db.set_turn_pp_lifecycle(&req.request.turn_id, trpg_model::PP_CRITICAL_DONE).await {
            tracing::warn!(error = %e, turn_id = %req.request.turn_id, "set pp_lifecycle=critical_done failed (non-fatal)");
        }
        // input 在此块结束时释放对 req 的借用，下面 move req 进 heavy spawn。
    }

    // —— 3b. HEAVY 尾段（TurnComplete 后另起 spawn，gm/ctx/req 整体 move）——
    spawn_heavy(gm, ctx, req, signal, scene_commit, heavy_assistant_output);
}

/// HEAVY 尾段：TurnComplete 后台跑——audit/memory 写、到场深抽+frontier、carryover。
/// owned gm/ctx/req（spec：heavy 需 owned 句柄，沿用 R1 spawn 模式；errata/obligations
/// 随 gm 自带、跨回合连续性不破）。失败隔离：整个任务包在独立 `tokio::spawn` 里，
/// panic/Err 只杀该任务，绝不影响已发的 TurnComplete（D2）；外层不 unwrap/不 panic。
fn spawn_heavy(
    gm: GmLoop,
    ctx: TurnContext,
    req: OwnedTurnRequest,
    signal: AgentSignal,
    scene_commit: Option<crate::turn_loop::SceneTransitionInfo>,
    heavy_assistant_output: String,
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
        // assistant_output 用 take_outcome 清空 ctx **之前**于 run_pipeline 快照的 owned 值
        // （非现读 ctx——此刻 ctx.visible_text/awaiting_gate 已被 take_outcome 清空）。
        gm.phase_finalize_heavy_memory(&heavy_assistant_output, &input).await;
        // 到场深抽 + frontier（仅 critical 真切了场景时）。
        if let (Some(commit), Some(module_id)) = (&scene_commit, req.module_id.as_deref()) {
            gm.phase_scene_navigate_heavy(&commit.to, module_id).await;
        }
        // carryover 债务记忆。#B 修复（GPT Pro P0-1）：carryover 决策必须用 **verify 后** 的
        // 义务状态——verify_after_stream（critical，已在 TurnComplete 前跑）经 absorb_retro_debts
        // 可能新生本回合 retro debt；旧版用 run_pipeline 内 verify **前** 算的 has_pending 选
        // phase（selected），导致"本回合无旧债但 verify 新生 debt"时 carryover 被跳、债务不落账。
        // 此处在 heavy（verify 之后）按 signal + 实时 has_pending 重判；phase_carryover_debt
        // 再自门控 carryover_block（无块即 no-op）。AwaitingPlayerRoll 不 carryover（同 R1）。
        if should_run_carryover(signal, gm.has_pending_obligations()) {
            gm.phase_carryover_debt(&mut ctx, &input).await;
        }
        // errata 记忆已在 critical 的 phase_verify_after_stream 内落账（save_memory_event）——
        // R1 行为：verify 在 finalize 前、属 critical。heavy 不重复 errata 写。
        // R5 高水位锚点：heavy 组（memory/audit + 到场深抽/frontier + carryover）全跑完，
        // 标 pp_lifecycle=complete（无论 carryover 是否触发，都是 heavy 末态）。失败仅 log——
        // heavy 在独立 spawn、TurnComplete 早已发，此写失败不回退、不 panic（D2 失败隔离）。
        if let Err(e) = gm.engine.db.set_turn_pp_lifecycle(&req.request.turn_id, trpg_model::PP_COMPLETE).await {
            tracing::warn!(error = %e, turn_id = %req.request.turn_id, "set pp_lifecycle=complete failed (non-fatal)");
        }
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

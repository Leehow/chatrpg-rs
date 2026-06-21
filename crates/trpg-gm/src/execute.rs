//! 统一回合执行器 facade（R1 §4.2）：CLI/API 唯一回合入口。
//! 按声明式 CANONICAL_TURN_PLAN 解释回合管线，产 TurnEvent 流。
//! transport（CLI 前台 drain / API 后台 spawn drain）决定尾部前后台，
//! 执行器本身不分前后台——只按 plan 顺序解释、经 mpsc 发事件。
use crate::turn_event::TurnEvent;
use crate::turn_loop::{GmLoop, GmTurnInput, TurnContext};
use crate::turn_plan::{PhaseId, PhaseKind, TurnPhasePlan};
use crate::turn_trace::{build_turn_trace, failure_kind_for, phase_error_policy, PhaseFailure};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use trpg_model::TurnFailureRecord;

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
    /// 可选取消令牌（P1-3 follow-up）：transport 把驱动器的**同一** [`CancellationToken`]
    /// 经此穿进 pipeline。客户端在状态变更前断开 → driver `cancel.cancel()` → run_agent_loop
    /// 在 LLM 流边界 break（放弃在途生成、不再续烧 token）+ run_pipeline 短路尾段（不落半截
    /// 回合）。`None` ⇒ 不可取消（CLI/测试默认）——非取消路径逐字节零行为变更。
    pub cancel: Option<CancellationToken>,
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
                AgentSignal::AwaitingPlayerRoll => {
                    matches!(p.id, PhaseId::VerifyAfterStream | PhaseId::Finalize)
                }
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
// ARCHITECTURE-ANCHOR (层化迁移 INV-2): 这是**事实上的 Turn Orchestrator / 控制平面**
// （设计4 §4），串起 15-phase CANONICAL_TURN_PLAN。目标态控制平面概念落于此——勿投到
// trpg-orchestrator 的同名类型 `TurnOrchestrator`（后者干 gate/lifecycle）。
// 详见 docs/architecture/layered-runtime-invariants.md INV-2。
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
    // T2 Flight Recorder：本回合实际跑过的 phase id（按解释顺序累积），收尾写进 TurnTrace。
    let mut phases_run: Vec<String> = Vec::new();
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

        // —— 0. domain event write-through（优化2 #6，additive/fail-soft/零行为变更）——
        // TurnStarted：回合入口。append 失败仅 warn，绝不入控制流、绝不改任何 emit 的 TurnEvent。
        append_lifecycle_event(
            &gm,
            &req.request,
            trpg_model::DomainEventKind::TurnStarted,
            serde_json::json!({ "ruleset_id": req.request.ruleset_id, "module_id": req.module_id }),
        )
        .await;

        // —— 1. 确定性头部：按 plan 顺序跑所有 Deterministic phase ——
        // mode_inference / context_assembly fail-closed 返 Err（P1-1）：发 TurnFailed
        // （**不再**发空 TurnComplete 伪装成功）+ 落 turns.failure_kind + 写失败 TurnTrace 后早返。
        for phase in plan.iter().filter(|p| p.kind == PhaseKind::Deterministic) {
            phases_run.push(format!("{:?}", phase.id));
            if let Err(f) = dispatch_deterministic(&mut gm, &mut ctx, &input, phase.id).await {
                // 确定性头部失败的只有 mode_inference / context_assembly，按分级器均为 AbortTurn
                // （fail-closed 中止）。policy 显式查询，避免硬编码"恒中止"的假设——后续若有
                // WarnContinue 类确定性失败可在此扩展（当前确定性头无此类）。
                let _policy = phase_error_policy(f.phase); // 确定性头失败均 AbortTurn（policy 显式查询非硬编码）
                let kind = failure_kind_for(f.phase);
                let phase_label = format!("{:?}", f.phase);
                // 顺序关键：DB 写（failure_kind + 失败 TurnTrace）必须在 emit TurnFailed **之前**。
                // 否则 CLI `turn` 收到 TurnFailed 即 bail! → 进程退出 → tokio runtime 关闭，
                // 会在 emit 之后、upsert 之前杀掉本任务，失败 trace 永不落库（live e2e 实测）。
                // 先落库再发终态事件，保证无论 transport 如何 bail，可观测记录都已持久化。
                if let Err(e) = gm
                    .engine
                    .db
                    .set_turn_failure_kind(&req.request.turn_id, kind)
                    .await
                {
                    tracing::warn!(error = %e, turn_id = %req.request.turn_id, "set_turn_failure_kind failed (non-fatal)");
                }
                // 失败 TurnTrace（pp_lifecycle="failed"、failure=Some、narration=None、phases=已尝试）。
                let failure = TurnFailureRecord {
                    phase: phase_label.clone(),
                    message: f.message.clone(),
                    failure_kind: kind.to_string(),
                };
                let trace = build_turn_trace(
                    &req,
                    &ctx,
                    "aborted".to_string(),
                    phases_run.clone(),
                    None,
                    Some(failure),
                    vec![],
                    "failed",
                );
                if let Err(e) = gm.engine.db.upsert_turn_trace(&trace).await {
                    tracing::warn!(error = %e, turn_id = %req.request.turn_id, "upsert_turn_trace (failure path) failed (non-fatal)");
                }
                // domain event write-through：TurnFailed（与 TurnTrace.failure 同源）。fail-soft，
                // emit TurnFailed 之前 append（与 DB 落账同序），但不入控制流、不改后续 emit。
                append_lifecycle_event(
                    &gm,
                    &req.request,
                    trpg_model::DomainEventKind::TurnFailed,
                    serde_json::json!({ "phase": phase_label, "failure_kind": kind }),
                )
                .await;
                // DB 已落账后再发失败事件代替空 TurnComplete——客户端能区分"失败"与"成功的空白回合"。
                let _ = tx
                    .send(TurnEvent::TurnFailed {
                        phase: phase_label,
                        message: f.message,
                        recoverable: false,
                    })
                    .await;
                return;
            }
        }

        // —— 2. AgentLoop body：产 Delta / AwaitingPlayerRoll 经 tx，返回终态信号 ——
        phases_run.push(format!("{:?}", PhaseId::AgentLoop));
        // P1 TRPG_NARRATOR_SPLIT：默认 OFF。读一次 env flag。
        // OFF ⇒ stream_prose=true（逐字节现行直发行为，与基线结构等价）；
        // ON  ⇒ stream_prose=false（buffer 模式：adjudicator prose 累进 ctx.visible_text
        //        但不直发；之后跑 Narrator phase 投影 NarrationPacket 流玩家散文）。
        let narrator_split = std::env::var("TRPG_NARRATOR_SPLIT")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        // P1-3 follow-up：把取消令牌穿进 agent loop——客户端在状态变更前断开时 driver fire 它，
        // run_agent_loop 在 LLM 流边界 break、放弃在途生成（不再续烧 token）。
        signal = gm
            .run_agent_loop(&mut ctx, &input, tx, req.cancel.as_ref(), !narrator_split)
            .await;

        // P1-3 follow-up：令牌已 fire ⇒ run_agent_loop 已放弃在途生成。此处同样短路尾段——
        // 不跑 verify/finalize/save_turn、不 spawn heavy，绝不持久化半截回合（兑现 driver
        // CancelBeforeStateMutation 的「no partial persisted」契约）。客户端早已断开、无人接收，
        // 不再 emit 任何事件。fail-closed：仅令牌存在且已 cancelled 时短路；非取消路径
        // （cancel=None 或未 fire）逐字节零行为变更。
        if req.cancel.as_ref().is_some_and(|c| c.is_cancelled()) {
            tracing::info!(
                turn_id = %req.request.turn_id,
                "turn cancelled (client disconnected before state mutation); skipping tail phases — no partial persisted"
            );
            return;
        }

        // —— 2a-bis. P6.7 ResolutionCommit（逻辑/审计边界，非 phase）——
        // AgentLoop 已结束、机械状态已由循环内工具落库（codex#7）。此命名边界触发重定位后的
        // BeforeCommit advisory hook（trace-only，不阻断）。仅非取消路径进入（取消已在上方短路）。
        gm.resolution_commit_boundary(&mut ctx, &req.request).await;

        // —— 2b. P1 Narrator phase（TRPG_NARRATOR_SPLIT ON 且 Narration 终态）——
        // 仅 Narration 终态跑：AwaitingPlayerRoll（桌面骰 prompt_public）走确定性文本、
        // 已在 agent loop 内流出，不经 Narrator（ON/OFF 行为一致）。run_narrator_phase 内
        // 自带 fail-soft：Narrator 失败/空 → 回退直发 buffer 的 adjudicator prose（= OFF 基线，
        // 无新增泄漏、绝不空白）。cancel 已在上方短路；此处仅未取消路径进入。
        if narrator_split && signal == AgentSignal::Narration {
            phases_run.push("Narrator".to_string());
            gm.run_narrator_phase(&mut ctx, &input, tx, req.cancel.as_ref())
                .await;
        }

        // —— 3a. CRITICAL 尾段（同步，TurnComplete 前 await）——
        // 与 R1 同 phase 集合（select_phases），但只跑 critical 半边：VerifyAfterStream
        // （勘误判定要在 finalize 前注入 ledger 真相）→ Finalize 的 save_turn →
        // SceneNavigate 的切场景决策 + set_session_scene + SceneChanged 事件。
        // AuditLearning（finalize 的 memory/audit 半边）/ 深抽 / carryover → heavy。
        let module_present = req.module_id.is_some();
        let has_pending = gm.has_pending_obligations();
        selected = select_phases(plan, signal, module_present, has_pending);
        for phase in &selected {
            if phase.kind != PhaseKind::Postprocess {
                continue;
            }
            match phase.id {
                PhaseId::VerifyAfterStream => {
                    phases_run.push(format!("{:?}", PhaseId::VerifyAfterStream));
                    let _ = tx.send(TurnEvent::PostprocessScheduled).await;
                    // P2：buffered-narration 就位 = TRPG_NARRATOR_SPLIT ON 且 Narration 终态
                    //（与上方 Narrator phase 触发条件一致——文字经 Narrator 缓冲，gate Block 时
                    // repair ladder 真正生效；否则 gate ON 也只记 trace）。
                    let buffered_narration = narrator_split && signal == AgentSignal::Narration;
                    gm.phase_verify_after_stream(
                        &mut ctx,
                        &input,
                        tx,
                        req.cancel.as_ref(),
                        buffered_narration,
                    )
                    .await;
                    // P2 步骤7：emit 一条 gate 决策事件（OFF/ON 都 emit，不阻断；§13 PresentationCommit
                    // 的真正扣留留待 P1 收口实时流后接 Block 分支）。TurnWarning 复用 verify phase 通道。
                    if let crate::presentation_gate::PresentationGate::Block(findings) =
                        ctx.presentation_gate()
                    {
                        let kinds: Vec<String> =
                            findings.iter().map(|f| format!("{:?}", f.kind)).collect();
                        let _ = tx
                            .send(TurnEvent::TurnWarning {
                                phase: "presentation_gate".to_string(),
                                message: format!("gate=Block kinds={kinds:?}"),
                            })
                            .await;
                    }
                    // —— P6.7 PresentationCommit（逻辑边界，非 phase）——
                    // 终审 gate（repair ladder 已沉降）后、Finalize/save_turn 前：触发重定位后的
                    // BeforeNarration advisory hook（split/非split 一致），并在终审 Allow 时按 fact_id
                    // 排序提交 reveal 提名（Block ⇒ 丢弃）。提交在 repair 之后 ⇒ 先 Block 后修复成
                    // Allow 的回合会提交（codex#6）。
                    gm.presentation_commit_boundary(&mut ctx, &req.request)
                        .await;
                }
                PhaseId::Finalize => {
                    phases_run.push(format!("{:?}", PhaseId::Finalize));
                    // awaiting 终态 finalize 状态 = "awaiting_player_roll"，正常 = "ready"。
                    let status = match signal {
                        AgentSignal::AwaitingPlayerRoll => "awaiting_player_roll",
                        AgentSignal::Narration => "ready",
                    };
                    gm.phase_finalize(&mut ctx, &input, status).await; // R5：只 save_turn
                }
                PhaseId::SceneNavigate => {
                    phases_run.push(format!("{:?}", PhaseId::SceneNavigate));
                    if let Some(t) = gm.phase_scene_navigate_critical(&ctx, &input).await {
                        let _ = tx
                            .send(TurnEvent::SceneTransition {
                                from: t.from.clone(),
                                to: t.to.clone(),
                                reason: t.reason.clone(),
                            })
                            .await;
                        // domain event write-through：SceneTransitioned（fail-soft，SceneTransition emit 之后）。
                        append_lifecycle_event(
                            &gm,
                            &req.request,
                            trpg_model::DomainEventKind::SceneTransitioned,
                            serde_json::json!({ "from": t.from, "to": t.to, "reason": t.reason }),
                        )
                        .await;
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
        if let Err(e) = gm
            .engine
            .db
            .set_turn_pp_lifecycle(&req.request.turn_id, trpg_model::PP_CRITICAL_DONE)
            .await
        {
            tracing::warn!(error = %e, turn_id = %req.request.turn_id, "set pp_lifecycle=critical_done failed (non-fatal)");
        }
        // —— 5. 成功路径 TurnTrace（write-through，fail-soft）——
        // 等价铁律：不改任何已发事件——TurnTrace 纯 DB 副作用，必须在此块内（ctx.compiled 仍
        // 可读、take_outcome 已快照 heavy_assistant_output 为念白）组装并落库；写失败仅 warn，
        // 绝不回退/影响已发的 Delta…TurnComplete 序列（D2 失败隔离）。
        let trace = build_turn_trace(
            &req,
            &ctx,
            format!("{signal:?}"),
            phases_run.clone(),
            Some(&heavy_assistant_output),
            None,
            vec![],
            trpg_model::PP_CRITICAL_DONE,
        );
        if let Err(e) = gm.engine.db.upsert_turn_trace(&trace).await {
            tracing::warn!(error = %e, turn_id = %req.request.turn_id, "upsert_turn_trace (success path) failed (non-fatal)");
        }
        // domain event write-through：TurnFinalized（成功路径，TurnComplete 已发 + critical 已落账后）。
        // fail-soft，纯附加 DB 写，绝不回退/影响已发的 Delta…TurnComplete 序列（D2 失败隔离）。
        append_lifecycle_event(
            &gm,
            &req.request,
            trpg_model::DomainEventKind::TurnFinalized,
            serde_json::json!({ "signal": format!("{signal:?}") }),
        )
        .await;
        // input 在此块结束时释放对 req 的借用，下面 move req 进 heavy spawn。
    }

    // —— 3b. HEAVY 尾段（TurnComplete 后另起 spawn，gm/ctx/req 整体 move）——
    spawn_heavy(gm, ctx, req, signal, scene_commit, heavy_assistant_output);
}

/// domain event write-through 收口（优化2 #6）：构造确定性幂等 `DomainEvent` 并 append。
/// event_id = `de_{turn_id}_{kind}`（idempotent on event_id：同回合同种类二次 append 不重复）。
/// **fail-soft**：append 失败仅 warn，绝不入控制流、绝不影响回合或任何已发的 TurnEvent。
async fn append_lifecycle_event(
    gm: &GmLoop,
    request: &trpg_model::ContextRequest,
    kind: trpg_model::DomainEventKind,
    data: serde_json::Value,
) {
    let ev = trpg_model::DomainEvent::new(
        format!("de_{}_{}", request.turn_id, kind.as_str()),
        request.session_id.clone(),
        request.turn_id.clone(),
        kind,
        data,
    );
    if let Err(e) = gm.engine.db.append_domain_event(&ev).await {
        tracing::warn!(error = %e, turn_id = %request.turn_id, kind = %kind.as_str(), "append_domain_event failed (non-fatal)");
    }
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
        gm.phase_finalize_heavy_memory(&heavy_assistant_output, &input)
            .await;
        // 玩家暴露 TruthGraph：用当前/初始转入场景的 referenced 实体 + 玩家可见念白
        // 写 `PlayerExposed`。这必须在 relationship extraction 前执行，否则关系抽取会因
        // 玩家暴露集为空而跳过。
        let fallback_scene_id = req
            .state
            .scene_id
            .as_deref()
            .or(scene_commit.as_ref().map(|commit| commit.to.as_str()));
        gm.engine
            .record_player_exposed_entities_for_narration(
                &req.request,
                &req.state,
                fallback_scene_id,
                &heavy_assistant_output,
            )
            .await;
        // 子项目2 知识图谱：对本局已 surface 的实体之间，按本回合念白语义抽关系三元组
        // 写入 memory_facts（fail-soft、env 门控、复用既有 gm.llm 句柄）。后续回合经
        // retrieve_memory 召回。失败/无实体 → no-op，绝不影响已发的 TurnComplete（D2）。
        gm.engine
            .extract_relationship_facts(
                gm.llm.as_ref(),
                &req.request.session_id,
                &req.request.turn_id,
                req.module_id.as_deref(),
                &heavy_assistant_output,
                // TC-D3-04 社交闸信号：active NPC + 本回合玩家输入，让无新实体但有明确社交
                // 互动（威胁/讨价/帮助…）的回合也能抽关系；普通非社交回合仍跳过。
                &req.state.active_npc_ids,
                &req.user_input,
                // L-H：当前场景（PlayerExposed 同一 fallback）供 PC↔NPC 端点派生。
                fallback_scene_id,
            )
            .await;
        // L-W：把本回合 kernel 已判 SUCCESS 的检定投射成 durable source-backed world_facts
        // （理念 §二.1/§二.3/§二.8：已落账的 CheckResolved 投成 world_fact，非发明）。修复 J2 真根——
        // 普通技能检定 SUCCESS 不进 committed_patches、无 proposer 写 world_facts ⇒ 后果蒸发进念白。
        // flag 门控、fail-soft、OFF==字节等价；world_facts 实际落盘仍由 KNOWLEDGE_KERNEL 门控。
        gm.engine
            .extract_check_outcome_world_facts(&req.request.session_id, &req.request.turn_id)
            .await;
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
        if let Err(e) = gm
            .engine
            .db
            .set_turn_pp_lifecycle(&req.request.turn_id, trpg_model::PP_COMPLETE)
            .await
        {
            tracing::warn!(error = %e, turn_id = %req.request.turn_id, "set pp_lifecycle=complete failed (non-fatal)");
        }
    });
}

/// 确定性头部 phase 分发。`Ok(())` ⇒ 阶段成功；`Err(PhaseFailure)` ⇒ fail-closed
/// 终止（mode/context Err，带触发 PhaseId + 人读信息，caller 据此发 TurnFailed +
/// 落 turns.failure_kind，**不再**发空 TurnComplete 伪装成功）。
async fn dispatch_deterministic(
    gm: &mut GmLoop,
    ctx: &mut TurnContext,
    input: &GmTurnInput<'_>,
    id: PhaseId,
) -> Result<(), PhaseFailure> {
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
                return Err(PhaseFailure {
                    phase: PhaseId::ModeInference,
                    message: err.to_string(),
                });
            }
        }
        PhaseId::DebtLoad => gm.phase_debt_load(ctx, input).await,
        PhaseId::ContextAssembly => {
            if let Err(err) = gm.phase_context_assembly(ctx, input).await {
                tracing::warn!(error = %err, "context_assembly failed; aborting turn");
                return Err(PhaseFailure {
                    phase: PhaseId::ContextAssembly,
                    message: err.to_string(),
                });
            }
        }
        // AgentLoop / Postprocess 不经此路径（解释器分流）；fail-closed no-op。
        _ => {}
    }
    Ok(())
}

// R5 Task1c：旧 dispatch_postprocess 已收编进 run_pipeline 的 critical 循环
// （VerifyAfterStream/Finalize/SceneNavigate）+ spawn_heavy（AuditLearning 折成
// heavy memory/audit、CarryoverDebt）。不再有单一 postprocess 分发器。

#[cfg(test)]
#[path = "execute_tests.rs"]
mod tests;

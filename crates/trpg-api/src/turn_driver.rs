//! SSE 回合驱动器（P1-3）：把 `execute_turn` 的 `TurnEvent` 流翻译成 SSE `Event`，
//! 并在客户端中途断开时按 [`CancellationPolicy`] 决定「取消省 token」还是「续跑保一致」。
//!
//! ## 为什么需要它
//! `play_turn_sse` 过去 `let _ = tx.send(...)` 吞掉所有发送错误：客户端断开后
//! 没人接收，回合却照常续烧 LLM、照常落账，断开毫无语义。本驱动器：
//! - 跟踪 `state_mutated`（本回合 durable 结果是否已开始落账）与 `first_delta_sent`；
//! - 把「发送失败」**当作断开信号**（不再无差别吞掉）；并辅以
//!   `select!` 主动探测响应体关闭（`Sender::closed`），即便此刻无 Delta 在发也能发现；
//! - 断开发生在状态变更**前** → 触发 [`tokio_util::sync::CancellationToken`] 取消在途工作、
//!   不落半截回合；变更**后** → 续 drain 让 critical finalize 落地，并在该回合记
//!   `client_disconnected` 标记。
//!
//! ## 范围与诚实的边界
//! 本驱动器只活在 trpg-api（「只读 GM 类型」约束）。`execute_turn` 内部自起 detached
//! tokio 任务跑 pipeline，并不返回其 JoinHandle，且 `run_agent_loop` 对自己的 `tx.send`
//! 也 `let _ =` 吞错——**故从 trpg-api 无法硬中断已在飞的 LLM 生成**。这里的「取消」是
//! 尽力而为：停止 relay、drop 掉事件流、fire CancellationToken（语义信号，且为未来把
//! token 穿进 `execute_turn` 留好接缝）。真正的 mid-LLM 硬中断需 `execute_turn` 协作
//! （后续切片，需改 GM crate，超出本任务范围）。

use axum::response::sse::Event;
use serde::Deserialize;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::future::Future;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use trpg_db::Db;
use trpg_gm::{TurnEvent, TurnOutcome};
use trpg_model::{DomainEvent, DomainEventKind};

/// 客户端中途断开时如何处置仍在进行的回合工作。
///
/// 默认 [`CancellationPolicy::CancelBeforeStateMutation`]——既省 token 又绝不腐化状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancellationPolicy {
    /// **默认（安全且省 token）**：断开发生在本回合状态变更**前**（还在流式叙事、
    /// save_turn 未跑）→ 取消在途工作、不落任何半截回合；变更**后** → 续跑 critical
    /// finalize 保一致并记 `client_disconnected`。
    CancelBeforeStateMutation,
    /// 永不取消：一旦断开，无论是否已变更状态都续 drain 到终态让回合一致落账，并记
    /// `client_disconnected`。比默认更保守（pre-mutation 也不省 token），换「总有完整记录」。
    ContinueAfterStateMutation,
    /// 永不取消、也不记任何断开标记——等价 legacy 行为（drain 到底、吞掉断开）。
    /// 仅为向后兼容/调试保留。
    AlwaysContinue,
}

impl Default for CancellationPolicy {
    fn default() -> Self {
        CancellationPolicy::CancelBeforeStateMutation
    }
}

impl CancellationPolicy {
    /// 稳定 token——写进 `client_disconnected` 标记载荷 + 日志。
    pub fn as_str(&self) -> &'static str {
        match self {
            CancellationPolicy::CancelBeforeStateMutation => "cancel_before_state_mutation",
            CancellationPolicy::ContinueAfterStateMutation => "continue_after_state_mutation",
            CancellationPolicy::AlwaysContinue => "always_continue",
        }
    }
}

/// 发送失败哨兵：接收端已被 axum drop（客户端断开）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Disconnected;

/// SSE 发送端抽象：生产实现包 axum 的 mpsc Sender；测试用真 mpsc 或 fake 注入断开时机。
pub trait EventSink {
    /// 推一条 SSE Event。`Err(Disconnected)` ⇒ 客户端已断开。
    fn send(&mut self, ev: Event) -> impl Future<Output = Result<(), Disconnected>> + Send;
    /// 在接收端被 drop 时 resolve——供 `select!` 主动探测断开（响应体关闭信号）。
    fn closed(&self) -> impl Future<Output = ()> + Send;
}

/// 生产 SSE sink：包 `play_turn_sse` 的 `mpsc::Sender<Result<Event, Infallible>>`。
pub struct AxumSseSink {
    tx: mpsc::Sender<Result<Event, Infallible>>,
}

impl AxumSseSink {
    pub fn new(tx: mpsc::Sender<Result<Event, Infallible>>) -> Self {
        Self { tx }
    }
}

impl EventSink for AxumSseSink {
    async fn send(&mut self, ev: Event) -> Result<(), Disconnected> {
        self.tx.send(Ok(ev)).await.map_err(|_| Disconnected)
    }
    async fn closed(&self) {
        self.tx.closed().await
    }
}

/// 断开时记标记所需的上下文。
#[derive(Debug, Clone, Copy)]
pub struct DisconnectInfo {
    pub policy: CancellationPolicy,
    pub state_mutated: bool,
    pub first_delta_sent: bool,
}

/// 断开标记落库抽象（DI 接缝）：生产写 `domain_events`，测试用 recorder 断言调用。
pub trait DisconnectMarker {
    fn record(&self, turn_id: &str, info: &DisconnectInfo) -> impl Future<Output = ()> + Send;
}

/// 生产标记：把 `ClientDisconnected` 领域事件 append 进该回合（幂等 per-turn，fail-soft）。
pub struct DbDisconnectMarker {
    pub db: Db,
    pub session_id: String,
}

impl DisconnectMarker for DbDisconnectMarker {
    async fn record(&self, turn_id: &str, info: &DisconnectInfo) {
        let ev = DomainEvent::new(
            format!("de_{turn_id}_ClientDisconnected"),
            self.session_id.clone(),
            turn_id.to_string(),
            DomainEventKind::ClientDisconnected,
            json!({
                "policy": info.policy.as_str(),
                "state_mutated": info.state_mutated,
                "first_delta_sent": info.first_delta_sent,
            }),
        );
        if let Err(e) = self.db.append_domain_event(&ev).await {
            tracing::warn!(error = %e, turn_id, "append ClientDisconnected marker failed (non-fatal)");
        }
    }
}

/// 驱动器终态——`play_turn_sse` 据此了解断开是否发生、是否取消/续跑。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveOutcome {
    /// 客户端全程在线，回合正常 drain 到终态。
    Completed,
    /// 断开发生在状态变更前，按策略取消了在途工作（未记标记）。
    CancelledBeforeMutation,
    /// 断开后续 drain 到终态（critical finalize 落地）；除 `AlwaysContinue` 外记 `client_disconnected`。
    ContinuedAfterDisconnect,
}

/// 驱动一个 `TurnEvent` 流：翻译成 SSE、跟踪 `state_mutated`/`first_delta_sent`、
/// 处置客户端断开。见模块文档。
pub async fn drive_turn_stream<S, M>(
    mut stream: ReceiverStream<TurnEvent>,
    mut sink: S,
    policy: CancellationPolicy,
    cancel: CancellationToken,
    marker: &M,
    turn_id: &str,
) -> DriveOutcome
where
    S: EventSink + Send,
    M: DisconnectMarker + Sync,
{
    let mut state_mutated = false;
    let mut first_delta_sent = false;
    let mut disconnected = false;

    loop {
        // Phase A：取下一事件 OR 主动探测断开。此处只读借用 sink（`closed()`），
        // select! 结束即释放，故与下面 Phase B 的 `&mut sink` 发送不重叠。
        let next = tokio::select! {
            biased;
            _ = sink.closed(), if !disconnected => None,
            ev = stream.next() => Some(ev),
        };
        match next {
            // closed() 触发：响应体已被 axum 关闭（客户端断开）。
            None => disconnected = true,
            // 流提前结束（execute_turn 任务无终态收口）——按已 drain 完处理。
            Some(None) => break,
            Some(Some(ev)) => {
                if marks_state_mutation(&ev) {
                    state_mutated = true;
                }
                // Phase B：仍在线才发送；发送失败即断开信号（不再 `let _ =` 吞错）。
                if !disconnected
                    && send_event_translated(&mut sink, &ev, &mut first_delta_sent)
                        .await
                        .is_err()
                {
                    disconnected = true;
                }
                if !disconnected && is_terminal(&ev) {
                    return DriveOutcome::Completed;
                }
            }
        }
        if disconnected {
            return handle_disconnect(
                policy,
                state_mutated,
                first_delta_sent,
                &cancel,
                stream,
                marker,
                turn_id,
            )
            .await;
        }
    }
    DriveOutcome::Completed
}

/// 断开处置：按策略 × 是否已变更状态决定取消还是续跑 + 记标记。
async fn handle_disconnect<M: DisconnectMarker + Sync>(
    policy: CancellationPolicy,
    state_mutated: bool,
    first_delta_sent: bool,
    cancel: &CancellationToken,
    mut stream: ReceiverStream<TurnEvent>,
    marker: &M,
    turn_id: &str,
) -> DriveOutcome {
    let cancel_now =
        matches!(policy, CancellationPolicy::CancelBeforeStateMutation) && !state_mutated;
    if cancel_now {
        // 尽力而为取消：fire token + drop 事件流停止 relay。execute_turn 的 detached
        // 尾段无法从本层硬中断（见模块文档），但本回合不在 trpg-api 侧落任何标记。
        cancel.cancel();
        drop(stream);
        tracing::info!(
            turn_id,
            policy = policy.as_str(),
            "client disconnected before state mutation; cancelling in-flight turn (no partial persisted)"
        );
        return DriveOutcome::CancelledBeforeMutation;
    }
    // 续 drain 到终态：让 execute_turn 的 critical finalize 落地后再收尾。
    drain_to_terminal(&mut stream).await;
    if !matches!(policy, CancellationPolicy::AlwaysContinue) {
        marker
            .record(
                turn_id,
                &DisconnectInfo { policy, state_mutated, first_delta_sent },
            )
            .await;
        tracing::info!(
            turn_id,
            policy = policy.as_str(),
            state_mutated,
            "client disconnected after state mutation; finalized turn + recorded client_disconnected"
        );
    }
    DriveOutcome::ContinuedAfterDisconnect
}

/// 事件是否意味着本回合 durable 结果已开始/已落账（断开后续跑而非取消的依据）。
/// Delta（流式叙事 token，save_turn 未跑）/ Errata / Warning / HeavyDone 不算；
/// PostprocessScheduled（verify 起、save_turn 同步紧随，已不可由本层阻止）、
/// AwaitingPlayerRoll（pending check 已落）、SceneTransition（session scene 已落）、
/// TurnComplete（critical 已落账）、TurnFailed（failure_kind/trace 已落）均算。
fn marks_state_mutation(ev: &TurnEvent) -> bool {
    matches!(
        ev,
        TurnEvent::PostprocessScheduled
            | TurnEvent::AwaitingPlayerRoll { .. }
            | TurnEvent::SceneTransition { .. }
            | TurnEvent::TurnComplete { .. }
            | TurnEvent::TurnFailed { .. }
    )
}

fn is_terminal(ev: &TurnEvent) -> bool {
    matches!(ev, TurnEvent::TurnComplete { .. } | TurnEvent::TurnFailed { .. })
}

/// 断开后续 drain：不再发送，只把剩余事件读到终态，让 execute_turn 尾段不因背压卡住。
async fn drain_to_terminal(stream: &mut ReceiverStream<TurnEvent>) {
    while let Some(ev) = stream.next().await {
        if is_terminal(&ev) {
            break;
        }
    }
}

/// 把一个 `TurnEvent` 翻译成 SSE 并发送（与原 play_turn_sse 翻译逐字节等价）。
/// 任一发送失败立即 `Err(Disconnected)` 短路。
async fn send_event_translated<S: EventSink>(
    sink: &mut S,
    ev: &TurnEvent,
    first_delta_sent: &mut bool,
) -> Result<(), Disconnected> {
    match ev {
        TurnEvent::Delta(delta) => {
            sink.send(sse_delta(delta)).await?;
            *first_delta_sent = true;
        }
        TurnEvent::AwaitingPlayerRoll { check_id, prompt_public } => {
            sink.send(sse_delta(prompt_public)).await?;
            sink.send(sse_phase("pending_check_created", json!({ "check_id": check_id }))).await?;
            sink.send(sse_phase("done", json!({ "reason": "awaiting_player_roll" }))).await?;
        }
        TurnEvent::SceneTransition { from, to, reason } => {
            sink.send(sse_named(
                "scene_transition",
                json!({ "from": from, "to": to, "reason": reason }),
            ))
            .await?;
        }
        TurnEvent::Errata(entry) => {
            sink.send(sse_phase(
                "errata",
                serde_json::to_value(entry).unwrap_or_else(|_| json!({})),
            ))
            .await?;
        }
        TurnEvent::PostprocessScheduled => {
            sink.send(sse_phase("postprocess_scheduled", json!({}))).await?;
        }
        TurnEvent::HeavyPostprocessDone => {}
        TurnEvent::TurnFailed { phase, message, recoverable } => {
            sink.send(sse_named(
                "error",
                json!({ "phase": phase, "message": message, "recoverable": recoverable }),
            ))
            .await?;
        }
        TurnEvent::TurnWarning { phase, message } => {
            sink.send(sse_named("warning", json!({ "phase": phase, "message": message }))).await?;
        }
        TurnEvent::TurnComplete { outcome } => {
            if let TurnOutcome::Narration(_) = outcome {
                sink.send(sse_phase("done", json!({}))).await?;
            }
        }
    }
    Ok(())
}

fn sse_delta(delta: &str) -> Event {
    Event::default().event("delta").data(delta.to_string())
}

fn sse_phase(phase: &str, data: Value) -> Event {
    Event::default()
        .event("phase")
        .data(json!({ "phase": phase, "data": data }).to_string())
}

fn sse_named(name: &str, data: Value) -> Event {
    Event::default().event(name).data(data.to_string())
}

#[cfg(test)]
#[path = "turn_driver_tests.rs"]
mod tests;

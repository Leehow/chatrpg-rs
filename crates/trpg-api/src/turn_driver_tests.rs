//! P1-3 驱动器单测：用真 mpsc 事件流 + 可控断开时机的 fake sink/marker，验证
//! 「断开 × 状态是否变更 × 策略」的处置矩阵。被测对象是真生产逻辑 `drive_turn_stream`；
//! 注入的只是 IO 边界（SSE 发送端 / 标记落库）的 DI 替身。

use super::*;
use std::sync::Mutex;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use trpg_gm::{TurnEvent, TurnOutcome};

/// 可控断开时机的 SSE sink：成功发送 `alive_for` 条后，后续 send 返回 `Disconnected`。
/// `proactive_close=true` 时 `closed()` 在饱和后立即 resolve（模拟 axum drop 接收端、
/// 经 `select!` 主动探测）；为 false 时 `closed()` 永不 resolve（只能靠 send 失败发现）。
struct FakeSink {
    sent: usize,
    alive_for: usize,
    proactive_close: bool,
}

impl FakeSink {
    fn new(alive_for: usize, proactive_close: bool) -> Self {
        Self { sent: 0, alive_for, proactive_close }
    }
}

impl EventSink for FakeSink {
    async fn send(&mut self, _ev: Event) -> Result<(), Disconnected> {
        if self.sent >= self.alive_for {
            return Err(Disconnected);
        }
        self.sent += 1;
        Ok(())
    }
    async fn closed(&self) {
        if self.proactive_close && self.sent >= self.alive_for {
            // 已断开：立即 resolve（响应体关闭信号）。
        } else {
            std::future::pending::<()>().await
        }
    }
}

/// 记录 record() 调用的 fake marker。
#[derive(Default)]
struct FakeMarker {
    calls: Mutex<Vec<(String, DisconnectInfo)>>,
}

impl DisconnectMarker for FakeMarker {
    async fn record(&self, turn_id: &str, info: &DisconnectInfo) {
        self.calls.lock().unwrap().push((turn_id.to_string(), *info));
    }
}

fn stream_of(events: Vec<TurnEvent>) -> ReceiverStream<TurnEvent> {
    let (tx, rx) = mpsc::channel(64);
    for ev in events {
        tx.try_send(ev).expect("test stream capacity");
    }
    drop(tx);
    ReceiverStream::new(rx)
}

fn narration_complete() -> TurnEvent {
    TurnEvent::TurnComplete { outcome: TurnOutcome::Narration("done".into()) }
}

// ---- 验收 #1：状态变更前断开 → 取消在途工作、不落半截回合 ----
#[tokio::test]
async fn disconnect_before_state_mutation_cancels_and_persists_nothing() {
    // 流式叙事中（仅 Delta，PostprocessScheduled 之前）客户端断开。
    let stream = stream_of(vec![
        TurnEvent::Delta("a".into()),
        TurnEvent::Delta("b".into()),
        TurnEvent::PostprocessScheduled,
        narration_complete(),
    ]);
    let sink = FakeSink::new(1, true); // 第 1 条 Delta 发出后断开
    let cancel = CancellationToken::new();
    let marker = FakeMarker::default();

    let outcome = drive_turn_stream(
        stream,
        sink,
        CancellationPolicy::CancelBeforeStateMutation,
        cancel.clone(),
        &marker,
        "turn_x",
    )
    .await;

    assert_eq!(outcome, DriveOutcome::CancelledBeforeMutation);
    assert!(cancel.is_cancelled(), "应 fire 取消信号");
    assert!(marker.calls.lock().unwrap().is_empty(), "未落账的回合不记 client_disconnected");
}

// ---- 验收 #2：状态变更后断开 → 续 critical finalize + 记 client_disconnected ----
#[tokio::test]
async fn disconnect_after_state_mutation_finalizes_and_records_marker() {
    // PostprocessScheduled 之后断开（save_turn 半边已不可逆）。
    let stream = stream_of(vec![
        TurnEvent::Delta("a".into()),
        TurnEvent::PostprocessScheduled,
        narration_complete(),
    ]);
    // Delta + PostprocessScheduled 两条发出后断开（done 那条发不出去）。
    let sink = FakeSink::new(2, true);
    let cancel = CancellationToken::new();
    let marker = FakeMarker::default();

    let outcome = drive_turn_stream(
        stream,
        sink,
        CancellationPolicy::CancelBeforeStateMutation,
        cancel.clone(),
        &marker,
        "turn_y",
    )
    .await;

    assert_eq!(outcome, DriveOutcome::ContinuedAfterDisconnect);
    assert!(!cancel.is_cancelled(), "状态已变更，不取消");
    let calls = marker.calls.lock().unwrap();
    assert_eq!(calls.len(), 1, "应记一次 client_disconnected");
    assert_eq!(calls[0].0, "turn_y");
    assert!(calls[0].1.state_mutated);
    assert!(calls[0].1.first_delta_sent);
}

// ---- 客户端全程在线 → 正常 drain，无标记、无取消 ----
#[tokio::test]
async fn no_disconnect_drains_to_completion() {
    let stream = stream_of(vec![
        TurnEvent::Delta("a".into()),
        TurnEvent::PostprocessScheduled,
        narration_complete(),
    ]);
    let sink = FakeSink::new(999, true);
    let cancel = CancellationToken::new();
    let marker = FakeMarker::default();

    let outcome = drive_turn_stream(
        stream,
        sink,
        CancellationPolicy::CancelBeforeStateMutation,
        cancel.clone(),
        &marker,
        "turn_ok",
    )
    .await;

    assert_eq!(outcome, DriveOutcome::Completed);
    assert!(!cancel.is_cancelled());
    assert!(marker.calls.lock().unwrap().is_empty());
}

// ---- 仅靠 send 失败发现断开（无主动 closed 探测）：pre-mutation → 取消 ----
#[tokio::test]
async fn send_error_pre_mutation_detected_and_cancelled() {
    let stream = stream_of(vec![
        TurnEvent::Delta("a".into()),
        TurnEvent::Delta("b".into()),
        TurnEvent::PostprocessScheduled,
        narration_complete(),
    ]);
    let sink = FakeSink::new(1, false); // closed() 永不 resolve，只能靠 send 失败
    let cancel = CancellationToken::new();
    let marker = FakeMarker::default();

    let outcome = drive_turn_stream(
        stream,
        sink,
        CancellationPolicy::CancelBeforeStateMutation,
        cancel.clone(),
        &marker,
        "turn_se",
    )
    .await;

    assert_eq!(outcome, DriveOutcome::CancelledBeforeMutation);
    assert!(cancel.is_cancelled());
    assert!(marker.calls.lock().unwrap().is_empty());
}

// ---- 仅靠 send 失败发现断开：post-mutation → 续跑 + 记标记 ----
#[tokio::test]
async fn send_error_post_mutation_continues_and_marks() {
    let stream = stream_of(vec![
        TurnEvent::Delta("a".into()),
        TurnEvent::PostprocessScheduled,
        narration_complete(),
    ]);
    let sink = FakeSink::new(2, false);
    let cancel = CancellationToken::new();
    let marker = FakeMarker::default();

    let outcome = drive_turn_stream(
        stream,
        sink,
        CancellationPolicy::CancelBeforeStateMutation,
        cancel.clone(),
        &marker,
        "turn_se2",
    )
    .await;

    assert_eq!(outcome, DriveOutcome::ContinuedAfterDisconnect);
    assert!(!cancel.is_cancelled());
    assert_eq!(marker.calls.lock().unwrap().len(), 1);
}

// ---- ContinueAfterStateMutation：即便 pre-mutation 断开也续跑 + 记标记，绝不取消 ----
#[tokio::test]
async fn continue_policy_never_cancels_even_pre_mutation() {
    let stream = stream_of(vec![
        TurnEvent::Delta("a".into()),
        TurnEvent::Delta("b".into()),
        TurnEvent::PostprocessScheduled,
        narration_complete(),
    ]);
    let sink = FakeSink::new(1, true); // 第 1 条后断开（PostprocessScheduled 之前）
    let cancel = CancellationToken::new();
    let marker = FakeMarker::default();

    let outcome = drive_turn_stream(
        stream,
        sink,
        CancellationPolicy::ContinueAfterStateMutation,
        cancel.clone(),
        &marker,
        "turn_c",
    )
    .await;

    assert_eq!(outcome, DriveOutcome::ContinuedAfterDisconnect);
    assert!(!cancel.is_cancelled(), "ContinueAfterStateMutation 绝不取消");
    assert_eq!(marker.calls.lock().unwrap().len(), 1);
}

// ---- AlwaysContinue：断开后 drain 到底，不取消、也不记标记（legacy 等价） ----
#[tokio::test]
async fn always_continue_drains_without_marker() {
    let stream = stream_of(vec![
        TurnEvent::Delta("a".into()),
        TurnEvent::Delta("b".into()),
        narration_complete(),
    ]);
    let sink = FakeSink::new(1, true);
    let cancel = CancellationToken::new();
    let marker = FakeMarker::default();

    let outcome = drive_turn_stream(
        stream,
        sink,
        CancellationPolicy::AlwaysContinue,
        cancel.clone(),
        &marker,
        "turn_a",
    )
    .await;

    assert_eq!(outcome, DriveOutcome::ContinuedAfterDisconnect);
    assert!(!cancel.is_cancelled());
    assert!(marker.calls.lock().unwrap().is_empty(), "AlwaysContinue 不记标记");
}

// ---- 配置：默认策略 = 安全省 token 的那个 ----
#[test]
fn default_policy_is_cancel_before_state_mutation() {
    assert_eq!(CancellationPolicy::default(), CancellationPolicy::CancelBeforeStateMutation);
}

// ---- 配置：per-request 可经 JSON snake_case 覆盖 ----
#[test]
fn policy_deserializes_from_snake_case() {
    let p: CancellationPolicy = serde_json::from_str("\"always_continue\"").unwrap();
    assert_eq!(p, CancellationPolicy::AlwaysContinue);
    let p: CancellationPolicy =
        serde_json::from_str("\"continue_after_state_mutation\"").unwrap();
    assert_eq!(p, CancellationPolicy::ContinueAfterStateMutation);
    let p: CancellationPolicy = serde_json::from_str("\"cancel_before_state_mutation\"").unwrap();
    assert_eq!(p, CancellationPolicy::CancelBeforeStateMutation);
}

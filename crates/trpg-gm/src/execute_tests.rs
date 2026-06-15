use super::*;
use crate::turn_plan::{PhaseId, CANONICAL_TURN_PLAN};

// 正常叙事终态：跑全部 15 phase，顺序与 CANONICAL_TURN_PLAN 一致。
#[test]
fn normal_narration_runs_all_phases_in_plan_order() {
    let phases = select_phases(
        CANONICAL_TURN_PLAN,
        AgentSignal::Narration,
        /* module_present */ true,
        /* has_pending_obligations */ true,
    );
    let ids: Vec<PhaseId> = phases.iter().map(|p| p.id).collect();
    let expected: Vec<PhaseId> = CANONICAL_TURN_PLAN.iter().map(|p| p.id).collect();
    assert_eq!(ids, expected, "normal turn must run every phase in canonical order");
}

// 早返：AwaitingPlayerRoll → 跑 VerifyAfterStream + Finalize，跳过
// SceneNavigate / AuditLearning / CarryoverDebt。
#[test]
fn awaiting_player_roll_skips_tail_but_runs_finalize() {
    let phases = select_phases(
        CANONICAL_TURN_PLAN,
        AgentSignal::AwaitingPlayerRoll,
        true,  // module 存在也不该跑 scene_navigate
        true,  // 有未决义务也不该跑 carryover
    );
    let ids: Vec<PhaseId> = phases.iter().map(|p| p.id).collect();
    assert!(ids.contains(&PhaseId::VerifyAfterStream), "verify must still run on awaiting");
    assert!(ids.contains(&PhaseId::Finalize), "finalize must still run on awaiting");
    assert!(!ids.contains(&PhaseId::SceneNavigate), "scene_navigate must be skipped on awaiting");
    assert!(!ids.contains(&PhaseId::AuditLearning), "audit_learning must be skipped on awaiting");
    assert!(!ids.contains(&PhaseId::CarryoverDebt), "carryover_debt must be skipped on awaiting");
}

// 条件 phase：无 module → 跳 SceneNavigate；无未决义务 → 跳 CarryoverDebt。
#[test]
fn conditional_phases_skip_when_condition_unmet() {
    let no_module = select_phases(CANONICAL_TURN_PLAN, AgentSignal::Narration, false, true);
    let ids: Vec<PhaseId> = no_module.iter().map(|p| p.id).collect();
    assert!(!ids.contains(&PhaseId::SceneNavigate), "scene_navigate needs module_id");
    assert!(ids.contains(&PhaseId::CarryoverDebt), "carryover still runs with pending obligations");

    let no_debt = select_phases(CANONICAL_TURN_PLAN, AgentSignal::Narration, true, false);
    let ids2: Vec<PhaseId> = no_debt.iter().map(|p| p.id).collect();
    assert!(ids2.contains(&PhaseId::SceneNavigate), "scene_navigate runs with module");
    assert!(!ids2.contains(&PhaseId::CarryoverDebt), "carryover skipped without pending obligations");
}

// AgentLoop 永远在序列里且恰好一次（它是 body，非可选）。
#[test]
fn agent_loop_present_exactly_once() {
    for sig in [AgentSignal::Narration, AgentSignal::AwaitingPlayerRoll] {
        let phases = select_phases(CANONICAL_TURN_PLAN, sig, true, true);
        let n = phases.iter().filter(|p| p.id == PhaseId::AgentLoop).count();
        assert_eq!(n, 1, "agent_loop must appear exactly once for {sig:?}");
    }
}

// ============================ channel / spawn / 真流式 ============================

use crate::turn_loop::{GmLoop, HeavyProbe, LoopConfig};
use crate::tools::ToolRegistry;
use async_stream::try_stream;
use async_trait::async_trait;
use futures_core::Stream;
use futures_util::StreamExt;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use trpg_db::Db;
use trpg_llm::{LlmClient, StreamEvent, ToolChoice};
use trpg_model::{ChatMessage, CompiledContext, ContextRequest, RuntimeState, TokenBudget, VisibilityProfile};
use trpg_runtime::RuntimeEngine;

// 复刻 turn_loop_tests.rs MockLlm：恒空刺激命中、脚本化 stream。
struct MockLlm { scripts: Mutex<Vec<Vec<StreamEvent>>> }
#[async_trait]
impl LlmClient for MockLlm {
    async fn complete_text(&self, _: Vec<ChatMessage>, _: f32) -> anyhow::Result<String> { unimplemented!() }
    async fn complete_json(&self, _: Vec<ChatMessage>, _: f32) -> anyhow::Result<Value> { Ok(json!({"hits": [], "moved": false})) }
    async fn stream_chat(&self, _: Vec<ChatMessage>, _: f32) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<String>> + Send>>> { unimplemented!() }
    async fn complete_with_tools(&self, _: Vec<Value>, _: Vec<Value>) -> anyhow::Result<Value> { unimplemented!() }
    async fn stream_chat_with_tools(&self, _: Vec<Value>, _: Vec<Value>, _: ToolChoice) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<StreamEvent>> + Send>>> {
        let script = self.scripts.lock().unwrap().remove(0);
        let s = try_stream! { for ev in script { yield ev; } };
        Ok(Box::pin(s))
    }
}

// 公共装配：脚本化 MockLlm + lazy pool + 临时 gm_skill 目录 + ctx_provider 绕真 DB。
// `probe` = heavy-only 时序探针（None ⇒ 普通；Some ⇒ 慢/panic/记录 heavy 时序）。
fn exec_fixture_inner(scripts: Vec<Vec<StreamEvent>>, probe: Option<HeavyProbe>) -> (GmLoop, OwnedTurnRequest) {
    let pool = PgPoolOptions::new().connect_lazy("postgres://chatrpg:chatrpg@localhost:54347/chatrpg").expect("lazy pool");
    let engine = RuntimeEngine::new(Db { pool });
    let llm = Arc::new(MockLlm { scripts: Mutex::new(scripts) });
    let data_dir = std::env::temp_dir().join(format!("exec_test_{}_{}", std::process::id(), uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(data_dir.join("agent/gm_skill/global")).unwrap();
    std::fs::write(data_dir.join("agent/gm_skill/global/10_test.md"), "test gm skill").unwrap();
    let mut gm = GmLoop::new(engine, llm, ToolRegistry::from_tools(vec![]), LoopConfig { max_tool_rounds: 1, repeat_finding_threshold: 3 }, data_dir.clone());
    gm.ctx_provider = Some(Arc::new(|_r, _s| CompiledContext { prefix_text: "BP1".into(), pinned_text: "BP2".into(), dynamic_text: "BP3".into(), prefix_hash: "p".into(), pinned_hash: "m".into(), dynamic_hash: "d".into(), ..Default::default() }));
    gm.heavy_probe = probe;
    let req = OwnedTurnRequest {
        request: ContextRequest { ruleset_id: "rs".into(), module_id: None, session_id: "s".into(), turn_id: "t".into(), viewer: VisibilityProfile::gm(), token_budget: TokenBudget::default() },
        state: RuntimeState { ruleset_id: "rs".into(), ..Default::default() },
        user_input: "go".into(), history: vec![], recent_transcript: None, module_id: None, data_dir,
    };
    (gm, req)
}

fn exec_fixture(scripts: Vec<Vec<StreamEvent>>) -> (GmLoop, OwnedTurnRequest) {
    exec_fixture_inner(scripts, None)
}

/// heavy 段入口慢 200ms 并把 "heavy_enter" 推进 `order`（heavy-only 探针，critical
/// 不经此路径）→ 验 TurnComplete 在 heavy 工作之前到达流。
fn exec_fixture_with_order(scripts: Vec<Vec<StreamEvent>>, order: Arc<Mutex<Vec<&'static str>>>) -> (GmLoop, OwnedTurnRequest) {
    exec_fixture_inner(scripts, Some(HeavyProbe { order: Some(order), sleep_ms: 200, panic: false }))
}

/// heavy 段入口 panic（heavy-only 探针）→ 验失败隔离：panic 只杀 heavy spawn，
/// 不影响已发的 TurnComplete / critical 落账。
fn exec_fixture_panicking_heavy(scripts: Vec<Vec<StreamEvent>>) -> (GmLoop, OwnedTurnRequest) {
    exec_fixture_inner(scripts, Some(HeavyProbe { order: None, sleep_ms: 0, panic: true }))
}

// 真流式直通：ContentDelta 逐块 → TurnEvent::Delta（不合并不缓冲），
// 末尾恰好一个 TurnComplete。需要 :54347（lazy pool；finalize warn 不 panic）。
#[tokio::test]
async fn execute_turn_streams_deltas_and_completes() {
    if std::env::var("SKIP_DB_TESTS").is_ok() { return; }
    let (gm, req) = exec_fixture(vec![vec![
        StreamEvent::ContentDelta("第一块。".into()),
        StreamEvent::ContentDelta("第二块。".into()),
        StreamEvent::Done { finish_reason: Some("stop".into()) },
    ]]);
    let mut stream = execute_turn(gm, req, CANONICAL_TURN_PLAN);
    let mut deltas: Vec<String> = vec![];
    let mut completed = false;
    while let Some(ev) = stream.next().await {
        match ev {
            TurnEvent::Delta(d) => deltas.push(d),
            TurnEvent::TurnComplete { .. } => completed = true,
            _ => {}
        }
    }
    assert_eq!(deltas, vec!["第一块。".to_string(), "第二块。".to_string()], "delta 必须逐块直通不合并");
    assert!(completed, "stream must end with TurnComplete");
}

// ============================ R5 Task1c: critical/heavy 时序 + 隔离 ============================

// ① heavy 慢不挂住 TurnComplete：heavy 段入口探针 sleep 200ms 后记 "heavy_enter"；
//    drain 收到 TurnComplete 时记 "turn_complete"。断言 turn_complete 早于 heavy_enter
//    （critical 已落账、TurnComplete 已发，heavy 后台跑，绝不挂住流）。需 :54347。
#[tokio::test]
async fn turn_complete_precedes_heavy_work() {
    if std::env::var("SKIP_DB_TESTS").is_ok() { return; }
    let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let (gm, req) = exec_fixture_with_order(vec![vec![
        StreamEvent::ContentDelta("叙事。".into()),
        StreamEvent::Done { finish_reason: Some("stop".into()) },
    ]], order.clone());
    let mut stream = execute_turn(gm, req, CANONICAL_TURN_PLAN);
    while let Some(ev) = stream.next().await {
        if let TurnEvent::TurnComplete { .. } = ev { order.lock().unwrap().push("turn_complete"); }
    }
    // drain 结束（SSE/CLI 在 TurnComplete 即可结束）后给 heavy spawn 时间跑完 sleep+记录。
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let log = order.lock().unwrap().clone();
    let tc = log.iter().position(|x| *x == "turn_complete");
    let heavy = log.iter().position(|x| *x == "heavy_enter");
    assert!(tc.is_some(), "must see TurnComplete, order={log:?}");
    assert!(heavy.is_some(), "heavy spawn must run after TurnComplete, order={log:?}");
    assert!(tc < heavy, "TurnComplete 必须先于 heavy 工作（heavy 不得挂住 TurnComplete），order={log:?}");
}

// ② heavy panic 隔离：heavy 段入口探针 panic（独立 tokio::spawn）→ 只杀 heavy 任务、
//    主 drain 不被毒化 → TurnComplete 仍正常到达（critical 已落账）。需 :54347。
#[tokio::test]
async fn heavy_panic_does_not_break_turn_complete() {
    if std::env::var("SKIP_DB_TESTS").is_ok() { return; }
    let (gm, req) = exec_fixture_panicking_heavy(vec![vec![
        StreamEvent::ContentDelta("叙事。".into()),
        StreamEvent::Done { finish_reason: Some("stop".into()) },
    ]]);
    let mut stream = execute_turn(gm, req, CANONICAL_TURN_PLAN);
    let mut completed = false;
    while let Some(ev) = stream.next().await {
        if let TurnEvent::TurnComplete { .. } = ev { completed = true; }
    }
    assert!(completed, "heavy panic 后 TurnComplete 仍必须到达（critical/已发事件不受影响）");
}

// ③ 事件序：Delta… → TurnComplete 为 transport 末事件（heavy 不经 tx）。断言 Delta
//    严格早于 TurnComplete、TurnComplete 仅一个且是流的最后事件。需 :54347。
#[tokio::test]
async fn event_order_deltas_then_turn_complete_last() {
    if std::env::var("SKIP_DB_TESTS").is_ok() { return; }
    let (gm, req) = exec_fixture(vec![vec![
        StreamEvent::ContentDelta("A".into()), StreamEvent::ContentDelta("B".into()),
        StreamEvent::Done { finish_reason: Some("stop".into()) },
    ]]);
    let mut stream = execute_turn(gm, req, CANONICAL_TURN_PLAN);
    let mut kinds: Vec<&str> = vec![];
    while let Some(ev) = stream.next().await {
        kinds.push(match ev {
            TurnEvent::Delta(_) => "delta",
            TurnEvent::TurnComplete { .. } => "complete",
            TurnEvent::PostprocessScheduled => "pp_sched",
            _ => "other",
        });
    }
    let last = kinds.last().copied();
    assert_eq!(last, Some("complete"), "TurnComplete 必须是 transport 末事件（heavy 不经 tx），kinds={kinds:?}");
    let c = kinds.iter().position(|k| *k == "complete").unwrap();
    assert_eq!(kinds.iter().filter(|k| **k == "complete").count(), 1, "恰好一个 TurnComplete，kinds={kinds:?}");
    assert!(kinds[..c].iter().any(|k| *k == "delta"), "TurnComplete 前必须有 Delta，kinds={kinds:?}");
}

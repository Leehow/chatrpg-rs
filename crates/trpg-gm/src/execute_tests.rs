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

use crate::turn_loop::{GmLoop, LoopConfig};
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

fn exec_fixture(scripts: Vec<Vec<StreamEvent>>) -> (GmLoop, OwnedTurnRequest) {
    let pool = PgPoolOptions::new().connect_lazy("postgres://chatrpg:chatrpg@localhost:54347/chatrpg").expect("lazy pool");
    let engine = RuntimeEngine::new(Db { pool });
    let llm = Arc::new(MockLlm { scripts: Mutex::new(scripts) });
    let data_dir = std::env::temp_dir().join(format!("exec_test_{}_{}", std::process::id(), uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(data_dir.join("agent/gm_skill/global")).unwrap();
    std::fs::write(data_dir.join("agent/gm_skill/global/10_test.md"), "test gm skill").unwrap();
    let mut gm = GmLoop::new(engine, llm, ToolRegistry::from_tools(vec![]), LoopConfig { max_tool_rounds: 1, repeat_finding_threshold: 3 }, data_dir.clone());
    gm.ctx_provider = Some(Arc::new(|_r, _s| CompiledContext { prefix_text: "BP1".into(), pinned_text: "BP2".into(), dynamic_text: "BP3".into(), prefix_hash: "p".into(), pinned_hash: "m".into(), dynamic_hash: "d".into(), ..Default::default() }));
    let req = OwnedTurnRequest {
        request: ContextRequest { ruleset_id: "rs".into(), module_id: None, session_id: "s".into(), turn_id: "t".into(), viewer: VisibilityProfile::gm(), token_budget: TokenBudget::default() },
        state: RuntimeState { ruleset_id: "rs".into(), ..Default::default() },
        user_input: "go".into(), history: vec![], recent_transcript: None, module_id: None, data_dir,
    };
    (gm, req)
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

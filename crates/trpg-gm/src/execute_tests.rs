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
    assert_eq!(
        ids, expected,
        "normal turn must run every phase in canonical order"
    );
}

// 早返：AwaitingPlayerRoll → 跑 VerifyAfterStream + Finalize，跳过
// SceneNavigate / AuditLearning / CarryoverDebt。
#[test]
fn awaiting_player_roll_skips_tail_but_runs_finalize() {
    let phases = select_phases(
        CANONICAL_TURN_PLAN,
        AgentSignal::AwaitingPlayerRoll,
        true, // module 存在也不该跑 scene_navigate
        true, // 有未决义务也不该跑 carryover
    );
    let ids: Vec<PhaseId> = phases.iter().map(|p| p.id).collect();
    assert!(
        ids.contains(&PhaseId::VerifyAfterStream),
        "verify must still run on awaiting"
    );
    assert!(
        ids.contains(&PhaseId::Finalize),
        "finalize must still run on awaiting"
    );
    assert!(
        !ids.contains(&PhaseId::SceneNavigate),
        "scene_navigate must be skipped on awaiting"
    );
    assert!(
        !ids.contains(&PhaseId::AuditLearning),
        "audit_learning must be skipped on awaiting"
    );
    assert!(
        !ids.contains(&PhaseId::CarryoverDebt),
        "carryover_debt must be skipped on awaiting"
    );
}

// 条件 phase：无 module → 跳 SceneNavigate；无未决义务 → 跳 CarryoverDebt。
#[test]
fn conditional_phases_skip_when_condition_unmet() {
    let no_module = select_phases(CANONICAL_TURN_PLAN, AgentSignal::Narration, false, true);
    let ids: Vec<PhaseId> = no_module.iter().map(|p| p.id).collect();
    assert!(
        !ids.contains(&PhaseId::SceneNavigate),
        "scene_navigate needs module_id"
    );
    assert!(
        ids.contains(&PhaseId::CarryoverDebt),
        "carryover still runs with pending obligations"
    );

    let no_debt = select_phases(CANONICAL_TURN_PLAN, AgentSignal::Narration, true, false);
    let ids2: Vec<PhaseId> = no_debt.iter().map(|p| p.id).collect();
    assert!(
        ids2.contains(&PhaseId::SceneNavigate),
        "scene_navigate runs with module"
    );
    assert!(
        !ids2.contains(&PhaseId::CarryoverDebt),
        "carryover skipped without pending obligations"
    );
}

// #B（GPT Pro P0-1）heavy carryover gate：用 verify 后实时 has_pending 重判，非 verify 前的旧值。
// 这是修复"本回合无旧债但 verify 新生 retro debt 时 carryover 被跳"的关键——spawn_heavy 在 verify
// 之后跑、传 gm.has_pending_obligations() 的实时值给本纯函数。AwaitingPlayerRoll 恒不 carryover。
#[test]
fn should_run_carryover_uses_signal_and_live_pending() {
    assert!(
        should_run_carryover(AgentSignal::Narration, true),
        "Narration + (verify 后)有债 → 跑 carryover"
    );
    assert!(
        !should_run_carryover(AgentSignal::Narration, false),
        "Narration + 无债 → 不跑（phase 内也会自门控）"
    );
    assert!(
        !should_run_carryover(AgentSignal::AwaitingPlayerRoll, true),
        "AwaitingPlayerRoll → 恒不 carryover（同 R1）"
    );
    assert!(
        !should_run_carryover(AgentSignal::AwaitingPlayerRoll, false),
        "AwaitingPlayerRoll + 无债 → 不跑"
    );
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

use crate::tools::ToolRegistry;
use crate::turn_loop::{GmLoop, HeavyProbe, LoopConfig};
use async_stream::try_stream;
use async_trait::async_trait;
use futures_core::Stream;
use futures_util::StreamExt;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
use trpg_db::Db;
use trpg_llm::{LlmClient, StreamEvent, ToolChoice};
use trpg_model::{
    ChatMessage, CompiledContext, ContextRequest, RuntimeState, TokenBudget, VisibilityProfile,
};
use trpg_runtime::RuntimeEngine;

// 复刻 turn_loop_tests.rs MockLlm：恒空刺激命中、脚本化 stream。
struct MockLlm {
    scripts: Mutex<Vec<Vec<StreamEvent>>>,
}
#[async_trait]
impl LlmClient for MockLlm {
    async fn complete_text(&self, _: Vec<ChatMessage>, _: f32) -> anyhow::Result<String> {
        unimplemented!()
    }
    async fn complete_json(&self, _: Vec<ChatMessage>, _: f32) -> anyhow::Result<Value> {
        Ok(json!({"hits": [], "moved": false}))
    }
    async fn stream_chat(
        &self,
        _: Vec<ChatMessage>,
        _: f32,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<String>> + Send>>> {
        unimplemented!()
    }
    async fn complete_with_tools(&self, _: Vec<Value>, _: Vec<Value>) -> anyhow::Result<Value> {
        unimplemented!()
    }
    async fn stream_chat_with_tools(
        &self,
        _: Vec<Value>,
        _: Vec<Value>,
        _: ToolChoice,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<StreamEvent>> + Send>>> {
        let script = self.scripts.lock().unwrap().remove(0);
        let s = try_stream! { for ev in script { yield ev; } };
        Ok(Box::pin(s))
    }
}

// 公共装配：脚本化 MockLlm + lazy pool + 临时 gm_skill 目录 + ctx_provider 绕真 DB。
// `probe` = heavy-only 时序探针（None ⇒ 普通；Some ⇒ 慢/panic/记录 heavy 时序）。
fn exec_fixture_inner(
    scripts: Vec<Vec<StreamEvent>>,
    probe: Option<HeavyProbe>,
) -> (GmLoop, OwnedTurnRequest) {
    exec_fixture_with_llm(
        Arc::new(MockLlm {
            scripts: Mutex::new(scripts),
        }),
        probe,
    )
}

// 同 exec_fixture_inner，但接受任意 LlmClient（mid-stream 取消测试用慢速流 mock）。
fn exec_fixture_with_llm(
    llm: Arc<dyn LlmClient>,
    probe: Option<HeavyProbe>,
) -> (GmLoop, OwnedTurnRequest) {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://chatrpg:chatrpg@localhost:54347/chatrpg")
        .expect("lazy pool");
    let engine = RuntimeEngine::new(Db { pool });
    let data_dir = std::env::temp_dir().join(format!(
        "exec_test_{}_{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(data_dir.join("agent/gm_skill/global")).unwrap();
    std::fs::write(
        data_dir.join("agent/gm_skill/global/10_test.md"),
        "test gm skill",
    )
    .unwrap();
    let mut gm = GmLoop::new(
        engine,
        llm,
        ToolRegistry::from_tools(vec![]),
        LoopConfig {
            max_tool_rounds: 1,
            repeat_finding_threshold: 3,
        },
        data_dir.clone(),
    );
    gm.ctx_provider = Some(Arc::new(|_r, _s| CompiledContext {
        prefix_text: "BP1".into(),
        pinned_text: "BP2".into(),
        dynamic_text: "BP3".into(),
        prefix_hash: "p".into(),
        pinned_hash: "m".into(),
        dynamic_hash: "d".into(),
        ..Default::default()
    }));
    gm.heavy_probe = probe;
    let req = OwnedTurnRequest {
        request: ContextRequest {
            ruleset_id: "rs".into(),
            module_id: None,
            session_id: "s".into(),
            turn_id: "t".into(),
            viewer: VisibilityProfile::gm(),
            token_budget: TokenBudget::default(),
        },
        state: RuntimeState {
            ruleset_id: "rs".into(),
            ..Default::default()
        },
        user_input: "go".into(),
        history: vec![],
        recent_transcript: None,
        module_id: None,
        data_dir,
        cancel: None,
    };
    (gm, req)
}

fn exec_fixture(scripts: Vec<Vec<StreamEvent>>) -> (GmLoop, OwnedTurnRequest) {
    exec_fixture_inner(scripts, None)
}

/// heavy 段入口慢 200ms 并把 "heavy_enter" 推进 `order`（heavy-only 探针，critical
/// 不经此路径）→ 验 TurnComplete 在 heavy 工作之前到达流。
fn exec_fixture_with_order(
    scripts: Vec<Vec<StreamEvent>>,
    order: Arc<Mutex<Vec<&'static str>>>,
) -> (GmLoop, OwnedTurnRequest) {
    exec_fixture_inner(
        scripts,
        Some(HeavyProbe {
            order: Some(order),
            sleep_ms: 200,
            panic: false,
        }),
    )
}

/// heavy 段入口 panic（heavy-only 探针）→ 验失败隔离：panic 只杀 heavy spawn，
/// 不影响已发的 TurnComplete / critical 落账。
fn exec_fixture_panicking_heavy(scripts: Vec<Vec<StreamEvent>>) -> (GmLoop, OwnedTurnRequest) {
    exec_fixture_inner(
        scripts,
        Some(HeavyProbe {
            order: None,
            sleep_ms: 0,
            panic: true,
        }),
    )
}

/// T2 失败注入：ctx_provider 产一个 prefix_text 多词的 CompiledContext，配合极小
/// token_budget.prefix_max → phase_context_assembly 的 validate_compiled_budget 返 Err
/// → ContextAssembly fail-closed（AbortTurn）。验证回合发 TurnFailed 而非空 TurnComplete。
fn exec_fixture_context_fail() -> (GmLoop, OwnedTurnRequest) {
    let (mut gm, mut req) = exec_fixture_inner(vec![vec![]], None);
    // 8 个词远超 prefix_max=2 → validate_compiled_budget Err（"prefix segment exceeds ..."）。
    gm.ctx_provider = Some(Arc::new(|_r, _s| CompiledContext {
        prefix_text: "one two three four five six seven eight".into(),
        prefix_hash: "p".into(),
        pinned_hash: "m".into(),
        dynamic_hash: "d".into(),
        ..Default::default()
    }));
    req.request.token_budget = TokenBudget {
        prefix_max: 2,
        pinned_max: 2,
        dynamic_max: 2,
        total_max: 8,
    };
    (gm, req)
}

// 真流式直通：ContentDelta 逐块 → TurnEvent::Delta（不合并不缓冲），
// 末尾恰好一个 TurnComplete。需要 :54347（lazy pool；finalize warn 不 panic）。
#[tokio::test]
async fn execute_turn_streams_deltas_and_completes() {
    if std::env::var("SKIP_DB_TESTS").is_ok() {
        return;
    }
    let (gm, req) = exec_fixture(vec![vec![
        StreamEvent::ContentDelta("第一块。".into()),
        StreamEvent::ContentDelta("第二块。".into()),
        StreamEvent::Done {
            finish_reason: Some("stop".into()),
        },
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
    assert_eq!(
        deltas,
        vec!["第一块。".to_string(), "第二块。".to_string()],
        "delta 必须逐块直通不合并"
    );
    assert!(completed, "stream must end with TurnComplete");
}

// ============================ P1-3 follow-up: 取消令牌掐断在途 LLM ============================

// 取消令牌穿进 execute_turn 后真正有牙：预先 fire 令牌（模拟客户端在状态变更前断开、
// driver 已 `cancel.cancel()`）→ run_agent_loop 在 LLM 流边界 break（不再续烧 token、不消费任何
// delta）+ run_pipeline 短路尾段（不 finalize、不落半截回合）→ 整个事件流 drain 完**绝无**
// TurnComplete、**零** Delta。对照组 `execute_turn_streams_deltas_and_completes`：同款脚本不取消时
// 必有 2 个 Delta + TurnComplete。需 :54347（lazy pool；head phase fail-soft）。
#[tokio::test]
async fn cancelled_token_aborts_agent_loop_and_skips_finalize() {
    if std::env::var("SKIP_DB_TESTS").is_ok() {
        return;
    }
    let (gm, mut req) = exec_fixture(vec![vec![
        StreamEvent::ContentDelta("不该被消费。".into()),
        StreamEvent::ContentDelta("更不该被消费。".into()),
        StreamEvent::Done {
            finish_reason: Some("stop".into()),
        },
    ]]);
    let token = CancellationToken::new();
    token.cancel(); // 预先取消：state mutation 前断开 → driver fire 的就是这枚同一 token。
    req.cancel = Some(token);

    let mut stream = execute_turn(gm, req, CANONICAL_TURN_PLAN);
    let mut deltas = 0usize;
    let mut completed = false;
    while let Some(ev) = stream.next().await {
        match ev {
            TurnEvent::Delta(_) => deltas += 1,
            TurnEvent::TurnComplete { .. } => completed = true,
            _ => {}
        }
    }
    assert_eq!(
        deltas, 0,
        "取消令牌已 fire ⇒ run_agent_loop 必须在消费任何 delta 之前 break（不续烧 token）"
    );
    assert!(
        !completed,
        "取消令牌已 fire ⇒ run_pipeline 必须短路尾段、绝不发 TurnComplete（不落半截回合）"
    );
}

// 慢速流 mock：吐第一块后 sleep 3s（模拟 LLM 仍在生成下一块），给 mid-stream 取消留出
// select 竞速窗口。若取消未真正掐断在途流，第二块会在 3s 后流出 + TurnComplete（测试红）。
struct SlowStreamLlm;
#[async_trait]
impl LlmClient for SlowStreamLlm {
    async fn complete_text(&self, _: Vec<ChatMessage>, _: f32) -> anyhow::Result<String> {
        unimplemented!()
    }
    async fn complete_json(&self, _: Vec<ChatMessage>, _: f32) -> anyhow::Result<Value> {
        Ok(json!({"hits": [], "moved": false}))
    }
    async fn stream_chat(
        &self,
        _: Vec<ChatMessage>,
        _: f32,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<String>> + Send>>> {
        unimplemented!()
    }
    async fn complete_with_tools(&self, _: Vec<Value>, _: Vec<Value>) -> anyhow::Result<Value> {
        unimplemented!()
    }
    async fn stream_chat_with_tools(
        &self,
        _: Vec<Value>,
        _: Vec<Value>,
        _: ToolChoice,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<StreamEvent>> + Send>>> {
        let s = try_stream! {
            yield StreamEvent::ContentDelta("第一块。".into());
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            yield StreamEvent::ContentDelta("第二块不该流出。".into());
            yield StreamEvent::Done { finish_reason: Some("stop".into()) };
        };
        Ok(Box::pin(s))
    }
}

// mid-stream（select! 路径）取消：令牌在 LLM **正流式**时 fire——测试收到第一块后立即 cancel，
// 此刻 run_agent_loop 正 park 在 `next_stream_event` 的 select（stream.next() 在 sleep）。fire 令牌
// → 取消等待分支胜出 → 放弃在途流（第二块永不被消费）、break 'rounds → run_pipeline 短路。
// 断言：恰好收到第一块、绝无第二块、绝无 TurnComplete。这是「真正掐断在飞 LLM」的核心证据，
// 与 pre-cancelled 测试（走 top-of-round 同步检查路径）互补。需 :54347。
#[tokio::test]
async fn cancel_mid_stream_abandons_in_flight_generation() {
    if std::env::var("SKIP_DB_TESTS").is_ok() {
        return;
    }
    let (gm, mut req) = exec_fixture_with_llm(Arc::new(SlowStreamLlm), None);
    let token = CancellationToken::new();
    req.cancel = Some(token.clone());

    let mut stream = execute_turn(gm, req, CANONICAL_TURN_PLAN);
    let mut deltas: Vec<String> = vec![];
    let mut completed = false;
    while let Some(ev) = stream.next().await {
        match ev {
            TurnEvent::Delta(d) => {
                deltas.push(d);
                token.cancel(); // 收到第一块即取消：此刻 LLM 流正 park 在 select，fire 掐断在途生成。
            }
            TurnEvent::TurnComplete { .. } => completed = true,
            _ => {}
        }
    }
    assert_eq!(
        deltas,
        vec!["第一块。".to_string()],
        "mid-stream 取消后第二块绝不该再流出（在途生成被放弃）"
    );
    assert!(
        !completed,
        "mid-stream 取消后 run_pipeline 必须短路、绝不发 TurnComplete"
    );
}

// ============================ R5 Task1c: critical/heavy 时序 + 隔离 ============================

// ① heavy 慢不挂住 TurnComplete：heavy 段入口探针 sleep 200ms 后记 "heavy_enter"；
//    drain 收到 TurnComplete 时记 "turn_complete"。断言 turn_complete 早于 heavy_enter
//    （critical 已落账、TurnComplete 已发，heavy 后台跑，绝不挂住流）。需 :54347。
#[tokio::test]
async fn turn_complete_precedes_heavy_work() {
    if std::env::var("SKIP_DB_TESTS").is_ok() {
        return;
    }
    let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let (gm, req) = exec_fixture_with_order(
        vec![vec![
            StreamEvent::ContentDelta("叙事。".into()),
            StreamEvent::Done {
                finish_reason: Some("stop".into()),
            },
        ]],
        order.clone(),
    );
    let mut stream = execute_turn(gm, req, CANONICAL_TURN_PLAN);
    while let Some(ev) = stream.next().await {
        if let TurnEvent::TurnComplete { .. } = ev {
            order.lock().unwrap().push("turn_complete");
        }
    }
    // drain 结束（SSE/CLI 在 TurnComplete 即可结束）后给 heavy spawn 时间跑完 sleep+记录。
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let log = order.lock().unwrap().clone();
    let tc = log.iter().position(|x| *x == "turn_complete");
    let heavy = log.iter().position(|x| *x == "heavy_enter");
    assert!(tc.is_some(), "must see TurnComplete, order={log:?}");
    assert!(
        heavy.is_some(),
        "heavy spawn must run after TurnComplete, order={log:?}"
    );
    assert!(
        tc < heavy,
        "TurnComplete 必须先于 heavy 工作（heavy 不得挂住 TurnComplete），order={log:?}"
    );
}

// ② heavy panic 隔离：heavy 段入口探针 panic（独立 tokio::spawn）→ 只杀 heavy 任务、
//    主 drain 不被毒化 → TurnComplete 仍正常到达（critical 已落账）。需 :54347。
#[tokio::test]
async fn heavy_panic_does_not_break_turn_complete() {
    if std::env::var("SKIP_DB_TESTS").is_ok() {
        return;
    }
    let (gm, req) = exec_fixture_panicking_heavy(vec![vec![
        StreamEvent::ContentDelta("叙事。".into()),
        StreamEvent::Done {
            finish_reason: Some("stop".into()),
        },
    ]]);
    let mut stream = execute_turn(gm, req, CANONICAL_TURN_PLAN);
    let mut completed = false;
    while let Some(ev) = stream.next().await {
        if let TurnEvent::TurnComplete { .. } = ev {
            completed = true;
        }
    }
    assert!(
        completed,
        "heavy panic 后 TurnComplete 仍必须到达（critical/已发事件不受影响）"
    );
}

// ③ 事件序：Delta… → TurnComplete 为 transport 末事件（heavy 不经 tx）。断言 Delta
//    严格早于 TurnComplete、TurnComplete 仅一个且是流的最后事件。需 :54347。
#[tokio::test]
async fn event_order_deltas_then_turn_complete_last() {
    if std::env::var("SKIP_DB_TESTS").is_ok() {
        return;
    }
    let (gm, req) = exec_fixture(vec![vec![
        StreamEvent::ContentDelta("A".into()),
        StreamEvent::ContentDelta("B".into()),
        StreamEvent::Done {
            finish_reason: Some("stop".into()),
        },
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
    assert_eq!(
        last,
        Some("complete"),
        "TurnComplete 必须是 transport 末事件（heavy 不经 tx），kinds={kinds:?}"
    );
    let c = kinds.iter().position(|k| *k == "complete").unwrap();
    assert_eq!(
        kinds.iter().filter(|k| **k == "complete").count(),
        1,
        "恰好一个 TurnComplete，kinds={kinds:?}"
    );
    assert!(
        kinds[..c].iter().any(|k| *k == "delta"),
        "TurnComplete 前必须有 Delta，kinds={kinds:?}"
    );
}

// ============================ R5 fix: heavy 记忆/审计读到叙事内容 ============================

use crate::tools::AwaitingPlayerRoll;
use crate::turn_loop::TurnContext;

// 纯单测：heavy_assistant_output 的派生必须逐字复刻 R1 旧 finalize_turn 的 assistant_output
// 选择——Narration→visible_text；awaiting+空 visible_text→gate.prompt_public；awaiting+非空
// visible_text→visible_text。这是 critical save_turn 与 heavy memory/audit 共用的单一事实源。
#[test]
fn heavy_assistant_output_matches_r1_derivation() {
    // Narration 终态：取 visible_text。
    let mut ctx = TurnContext::new();
    ctx.set_agent_products_for_test("叙事正文。".into(), None);
    assert_eq!(
        ctx.heavy_assistant_output(),
        "叙事正文。",
        "Narration 终态必须取 visible_text"
    );

    // awaiting + 空 visible_text：兜底 gate.prompt_public。
    let mut ctx = TurnContext::new();
    ctx.set_agent_products_for_test(
        String::new(),
        Some(AwaitingPlayerRoll {
            check_id: "chk".into(),
            prompt_public: "请掷 Spot Hidden。".into(),
        }),
    );
    assert_eq!(
        ctx.heavy_assistant_output(),
        "请掷 Spot Hidden。",
        "awaiting+空文本必须兜底 gate.prompt_public"
    );

    // awaiting + 非空 visible_text：仍取 visible_text（gate 不覆盖已流出的叙事）。
    let mut ctx = TurnContext::new();
    ctx.set_agent_products_for_test(
        "掷骰前的叙事。".into(),
        Some(AwaitingPlayerRoll {
            check_id: "chk".into(),
            prompt_public: "请掷骰。".into(),
        }),
    );
    assert_eq!(
        ctx.heavy_assistant_output(),
        "掷骰前的叙事。",
        "awaiting+非空 visible_text 必须取 visible_text"
    );
}

// DB-gated 回归测试（本 bug 漏网的根因 = 缺这条）：跑完整一回合后，heavy 段必须真正
// 持久化富版回合记忆 MemoryEvent（mem_turn_* / tag session_memory），且其 summary +
// transcript_excerpt 必含本回合叙事内容（非空）。
//
// 修复前（buggy）：take_outcome 先清空 ctx，heavy 的 phase_finalize_heavy_memory 从空
// ctx 现读 → assistant_output="" → heavy_finalize_memory 早返 → 该事件根本不写 → 本断言
// 红（找不到含叙事的 mem_turn 事件）。修复后：assistant_output 在清空前快照传入 → 事件
// 带叙事内容落库 → 绿。需 :54347（SKIP_DB_TESTS 跳过）。
#[tokio::test]
async fn heavy_memory_event_persists_with_narration_content() {
    if std::env::var("SKIP_DB_TESTS").is_ok() {
        return;
    }
    let narration = format!("R5回归叙事_{}", uuid::Uuid::new_v4().simple());
    let session_id = format!("s_mem_{}", uuid::Uuid::new_v4().simple());
    let turn_id = format!("t_mem_{}", uuid::Uuid::new_v4().simple());

    let (gm, mut req) = exec_fixture(vec![vec![
        StreamEvent::ContentDelta(narration.clone()),
        StreamEvent::Done {
            finish_reason: Some("stop".into()),
        },
    ]]);
    req.request.session_id = session_id.clone();
    req.request.turn_id = turn_id.clone();

    // 独立 Db 句柄（gm 会被 move 进 execute_turn，事后不可用）查 memory_events。
    let probe_db = Db {
        pool: PgPoolOptions::new()
            .connect_lazy("postgres://chatrpg:chatrpg@localhost:54347/chatrpg")
            .expect("lazy pool"),
    };
    // memory_events/turns 对 sessions(session_id) 有 FK：必须先建 session 行，否则
    // critical 的 save_turn 与 heavy 的 save_memory_event 都会 FK 失败被 warn 吞 → 查不到事件
    // （会与"修复前 bug"误判同形）。建 session 后，本测试才真正只考验 #A 的 assistant_output 快照。
    probe_db
        .create_session(
            &session_id,
            &req.request.ruleset_id,
            req.request.module_id.as_deref(),
        )
        .await
        .expect("create session row (FK prereq)");

    let mut stream = execute_turn(gm, req, CANONICAL_TURN_PLAN);
    let mut completed = false;
    while let Some(ev) = stream.next().await {
        if let TurnEvent::TurnComplete { .. } = ev {
            completed = true;
        }
    }
    assert!(completed, "回合必须以 TurnComplete 收尾");

    // heavy 在 TurnComplete 后另起 spawn 跑——轮询等它把 mem_turn 事件写进库（best-effort 后台）。
    let mut found: Option<trpg_model::MemoryEvent> = None;
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let events = probe_db
            .list_memory_events(&session_id, 50)
            .await
            .expect("list_memory_events");
        if let Some(e) = events.into_iter().find(|e| {
            e.event_id.starts_with("mem_turn_")
                && e.tags.iter().any(|t| t == "session_memory")
                && e.summary.contains(&narration)
        }) {
            found = Some(e);
            break;
        }
    }

    let event = found.expect(
        "heavy 段必须持久化含本回合叙事的 mem_turn_* / session_memory MemoryEvent；\
         未找到 ⇒ take_outcome 清空 ctx 后 heavy 读到空 assistant_output 而早返（修复前的 bug）",
    );
    assert!(
        event.summary.contains(&narration),
        "记忆 summary 必含叙事内容: {}",
        event.summary
    );
    let transcript = event.transcript_excerpt.unwrap_or_default();
    assert!(
        transcript.contains(&narration),
        "记忆 transcript_excerpt 必含叙事内容: {transcript}"
    );
    assert_eq!(
        event.turn_id.as_deref(),
        Some(turn_id.as_str()),
        "记忆事件必须绑定本回合 turn_id"
    );
}

// ============================ T2: 失败不再伪装成空白成功 ============================

// headline fix（P1-1）：确定性头部阶段失败（context_assembly fail-closed）→ 事件流必须含
// TurnFailed{phase:ContextAssembly} 且**绝不**含 TurnComplete（旧版会发空白 TurnComplete 伪装
// 成功）。turns.failure_kind / turn_traces 落库走 :54347（已含 0029 schema），故 DB-gated。
#[tokio::test]
async fn context_assembly_failure_emits_turn_failed_not_complete() {
    if std::env::var("SKIP_DB_TESTS").is_ok() {
        return;
    }
    let session_id = format!("s_fail_{}", uuid::Uuid::new_v4().simple());
    let turn_id = format!("t_fail_{}", uuid::Uuid::new_v4().simple());
    let (gm, mut req) = exec_fixture_context_fail();
    req.request.session_id = session_id.clone();
    req.request.turn_id = turn_id.clone();

    // 独立 Db 句柄查 turns.failure_kind / turn_traces（gm 被 move 进 execute_turn）。
    let probe_db = Db {
        pool: PgPoolOptions::new()
            .connect_lazy("postgres://chatrpg:chatrpg@localhost:54347/chatrpg")
            .expect("lazy pool"),
    };
    // turns/turn_traces 对 sessions FK：先建 session 行，否则落库被 warn 吞、查不到。
    probe_db
        .create_session(
            &session_id,
            &req.request.ruleset_id,
            req.request.module_id.as_deref(),
        )
        .await
        .expect("create session row (FK prereq)");

    let mut stream = execute_turn(gm, req, CANONICAL_TURN_PLAN);
    let mut failed_phase: Option<String> = None;
    let mut saw_complete = false;
    while let Some(ev) = stream.next().await {
        match ev {
            TurnEvent::TurnFailed {
                phase, recoverable, ..
            } => {
                assert!(!recoverable, "fail-closed 阶段失败 recoverable 必须 false");
                failed_phase = Some(phase);
            }
            TurnEvent::TurnComplete { .. } => saw_complete = true,
            _ => {}
        }
    }
    assert_eq!(
        failed_phase.as_deref(),
        Some("ContextAssembly"),
        "必须发 TurnFailed{{phase:ContextAssembly}}"
    );
    assert!(
        !saw_complete,
        "失败回合绝不能发 TurnComplete（伪装成功）—— headline fix"
    );

    // 失败 TurnTrace 落库（轮询：upsert 在早返前 await，但 fail-soft、轮询稳健）：
    // failure 非 None、pp_lifecycle=failed、phases 含 ContextAssembly、failure_kind=failed_context。
    // 注：context_assembly 在 Finalize/save_turn 之前中止 ⇒ 此刻无 turns 行，set_turn_failure_kind
    // 的 UPDATE 命中 0 行（fail-soft，不报错）；失败归类的**权威落库**是 turn_traces.failure（PK upsert）。
    let mut loaded = None;
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if let Ok(Some(t)) = probe_db.load_turn_trace(&turn_id).await {
            loaded = Some(t);
            break;
        }
    }
    let trace = loaded.expect("失败 TurnTrace 必须落库");
    assert_eq!(trace.pp_lifecycle, "failed");
    let failure = trace.failure.expect("失败 trace 必须带 failure record");
    assert_eq!(failure.failure_kind, "failed_context");
    assert_eq!(failure.phase, "ContextAssembly");
    assert!(
        trace.phases_run.iter().any(|p| p == "ContextAssembly"),
        "phases_run 必须含已尝试的 ContextAssembly: {:?}",
        trace.phases_run
    );
    assert!(
        trace.narration_hash.is_none(),
        "失败回合 narration=None ⇒ narration_hash 必须 None"
    );
}

// ============================ T2: domain_events write-through（优化2 #6） ============================

// 成功回合 → domain_events 落 TurnStarted + TurnFinalized（fail-soft write-through，零行为变更）。
// 不改任何已发的 TurnEvent；这里只额外断言 DB 旁路落了两条生命周期事件。需 :54347（DB-gated）。
#[tokio::test]
async fn success_turn_appends_started_and_finalized_domain_events() {
    if std::env::var("SKIP_DB_TESTS").is_ok() {
        return;
    }
    let session_id = format!("s_de_ok_{}", uuid::Uuid::new_v4().simple());
    let turn_id = format!("t_de_ok_{}", uuid::Uuid::new_v4().simple());
    let (gm, mut req) = exec_fixture(vec![vec![
        StreamEvent::ContentDelta("叙事。".into()),
        StreamEvent::Done {
            finish_reason: Some("stop".into()),
        },
    ]]);
    req.request.session_id = session_id.clone();
    req.request.turn_id = turn_id.clone();

    let probe_db = Db {
        pool: PgPoolOptions::new()
            .connect_lazy("postgres://chatrpg:chatrpg@localhost:54347/chatrpg")
            .expect("lazy pool"),
    };
    // domain_events.session_id 不带 FK，但保持与其它测试同形先建 session 行（无害）。
    probe_db
        .create_session(
            &session_id,
            &req.request.ruleset_id,
            req.request.module_id.as_deref(),
        )
        .await
        .expect("create session row");

    let mut stream = execute_turn(gm, req, CANONICAL_TURN_PLAN);
    let mut completed = false;
    while let Some(ev) = stream.next().await {
        if let TurnEvent::TurnComplete { .. } = ev {
            completed = true;
        }
    }
    assert!(completed, "回合必须以 TurnComplete 收尾");

    // TurnStarted 在入口 append、TurnFinalized 在 TurnComplete 后（仍同步在主 spawn 内）append；
    // 轮询稳健（append fail-soft，但成功路径必落）。
    let mut kinds: Vec<String> = vec![];
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let evs = probe_db
            .list_domain_events_for_turn(&turn_id)
            .await
            .expect("list_domain_events_for_turn");
        kinds = evs.iter().map(|e| e.kind.as_str().to_string()).collect();
        if kinds.iter().any(|k| k == "TurnStarted") && kinds.iter().any(|k| k == "TurnFinalized") {
            break;
        }
    }
    assert!(
        kinds.iter().any(|k| k == "TurnStarted"),
        "成功回合必须 append TurnStarted domain event，得到: {kinds:?}"
    );
    assert!(
        kinds.iter().any(|k| k == "TurnFinalized"),
        "成功回合必须 append TurnFinalized domain event，得到: {kinds:?}"
    );
}

// 失败回合（context_assembly fail-closed）→ domain_events 落 TurnFailed（与 TurnTrace.failure 同源）。
// 镜像 context_assembly_failure_emits_turn_failed_not_complete，额外查 domain event。需 :54347。
#[tokio::test]
async fn failure_turn_appends_turn_failed_domain_event() {
    if std::env::var("SKIP_DB_TESTS").is_ok() {
        return;
    }
    let session_id = format!("s_de_fail_{}", uuid::Uuid::new_v4().simple());
    let turn_id = format!("t_de_fail_{}", uuid::Uuid::new_v4().simple());
    let (gm, mut req) = exec_fixture_context_fail();
    req.request.session_id = session_id.clone();
    req.request.turn_id = turn_id.clone();

    let probe_db = Db {
        pool: PgPoolOptions::new()
            .connect_lazy("postgres://chatrpg:chatrpg@localhost:54347/chatrpg")
            .expect("lazy pool"),
    };
    probe_db
        .create_session(
            &session_id,
            &req.request.ruleset_id,
            req.request.module_id.as_deref(),
        )
        .await
        .expect("create session row");

    let mut stream = execute_turn(gm, req, CANONICAL_TURN_PLAN);
    let mut saw_failed = false;
    while let Some(ev) = stream.next().await {
        if let TurnEvent::TurnFailed { .. } = ev {
            saw_failed = true;
        }
    }
    assert!(saw_failed, "失败回合必须发 TurnFailed");

    let mut found: Option<trpg_model::DomainEvent> = None;
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let evs = probe_db
            .list_domain_events_for_turn(&turn_id)
            .await
            .expect("list_domain_events_for_turn");
        if let Some(e) = evs
            .into_iter()
            .find(|e| e.kind == trpg_model::DomainEventKind::TurnFailed)
        {
            found = Some(e);
            break;
        }
    }
    let ev = found.expect("失败回合必须 append TurnFailed domain event");
    // data 携带 phase（ContextAssembly）+ failure_kind（与 TurnTrace.failure 同源）。
    assert_eq!(
        ev.data.get("phase").and_then(|v| v.as_str()),
        Some("ContextAssembly"),
        "TurnFailed data.phase 必须是 ContextAssembly: {:?}",
        ev.data
    );
}

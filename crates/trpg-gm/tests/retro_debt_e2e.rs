//! C7① e2e（spec §8 验收 11，`#[ignore]`）：追溯债务闭环。
//!
//! 跑法（cwd 在 crate 目录——cargo test -p 的既有坑）：
//! ```bash
//! cd crates/trpg-gm && DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
//!   cargo test -p trpg-gm --test retro_debt_e2e -- --ignored --nocapture
//! ```
//! 真 DB + MockLlm 脚本（spec §8.11 原文即"MockLlm 脚本"）：「叙事声称伤害但未调
//! 工具」靠真 LLM 不可复现，必须脚本可控。engine 用真 Db（Db::connect + migrate），
//! session 经 engine.start_session 真 bootstrap，绝不自造 session_id。
//! 断言只看结构面（债务存在/回填发生/债务清除/库值变化），不依赖勘误记忆措辞。

use anyhow::Result;
use async_stream::try_stream;
use async_trait::async_trait;
use futures_core::Stream;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use trpg_db::Db;
use trpg_gm::tools::effect::ApplyEffectTool;
use trpg_gm::tools::mechanic::WaiveObligationTool;
use trpg_gm::{GmLoop, GmTurnInput, LoopConfig, ToolRegistry, TurnOutcome};
use trpg_llm::{AggregatedToolCall, LlmClient, StreamEvent, ToolChoice};
use trpg_model::{ChatMessage, ContextRequest, RuleKernel, RuntimeState, TokenBudget, VisibilityProfile};
use trpg_runtime::RuntimeEngine;

/// 对标 turn_loop_tests.rs 的 MockLlm 样板；scripts 可在回合间追加
/// （debt_id 运行时才生成，回合 2 脚本必须拿到真 id 后再注入）。
struct MockLlm {
    scripts: Mutex<Vec<Vec<StreamEvent>>>,
    requests: Mutex<Vec<Vec<Value>>>,
}

impl MockLlm {
    fn push_script(&self, script: Vec<StreamEvent>) {
        self.scripts.lock().unwrap().push(script);
    }
}

#[async_trait]
impl LlmClient for MockLlm {
    async fn complete_text(&self, _: Vec<ChatMessage>, _: f32) -> Result<String> {
        unimplemented!("unused by run_gm_turn e2e")
    }
    async fn complete_json(&self, _: Vec<ChatMessage>, _: f32) -> Result<Value> {
        // 刺激预 pass 走 complete_json：恒回空命中（本测试不测该通路）。
        Ok(serde_json::json!({"hits": []}))
    }
    async fn stream_chat(&self, _: Vec<ChatMessage>, _: f32) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        unimplemented!("unused by run_gm_turn e2e")
    }
    async fn complete_with_tools(&self, _: Vec<Value>, _: Vec<Value>) -> Result<Value> {
        unimplemented!("unused by run_gm_turn e2e")
    }
    async fn stream_chat_with_tools(&self, messages: Vec<Value>, _: Vec<Value>, _: ToolChoice) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        self.requests.lock().unwrap().push(messages);
        let mut scripts = self.scripts.lock().unwrap();
        assert!(!scripts.is_empty(), "MockLlm script exhausted: the loop asked for one more round than scripted (debt not cleared?)");
        let script = scripts.remove(0);
        let s = try_stream! { for event in script { yield event; } };
        Ok(Box::pin(s))
    }
}

fn content(text: &str) -> StreamEvent {
    StreamEvent::ContentDelta(text.to_string())
}

fn done(reason: &str) -> StreamEvent {
    StreamEvent::Done { finish_reason: Some(reason.to_string()) }
}

fn tool_call(name: &str, args: Value) -> StreamEvent {
    StreamEvent::ToolCalls(vec![AggregatedToolCall { id: format!("call_{name}"), name: name.to_string(), arguments: args.to_string() }])
}

/// 真库装配：Db::connect(env DATABASE_URL) + migrate + start_session 真 bootstrap。
/// 规则集经 env 注入（默认 call_of_cthulhu_7e——测试 fixture，非逻辑分支；
/// HP 轨 id 由 kernel 数据驱动解析，零硬编码）。
async fn fixture(turn1_script: Vec<StreamEvent>) -> Result<(GmLoop, Arc<MockLlm>, String, String, RuleKernel)> {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must point at the live test DB (e.g. :54347)");
    let db = Db::connect(&url).await?;
    db.migrate().await?;
    let ruleset = std::env::var("TRPG_TEST_RULESET").unwrap_or_else(|_| "call_of_cthulhu_7e".to_string());
    let kernel = db.load_rule_kernel(&ruleset).await?.unwrap_or_else(|| panic!("no active kernel for {ruleset}; run parse-all first"));
    let engine = RuntimeEngine::new(db.clone());
    let session_id = engine.start_session(&ruleset, None).await?;
    println!("SESSION_ID={session_id}");
    let llm = Arc::new(MockLlm { scripts: Mutex::new(vec![turn1_script]), requests: Mutex::new(Vec::new()) });
    // 真 data 目录（repo data/——gm_skill fail-closed 硬依赖）。
    let data_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data");
    let tools = ToolRegistry::from_tools(vec![Box::new(ApplyEffectTool), Box::new(WaiveObligationTool)]);
    let gm = GmLoop::new(engine, llm.clone(), tools, LoopConfig::default(), data_dir);
    Ok((gm, llm, session_id, ruleset, kernel))
}

/// 测试角色 seed（对标 trpg-mechanics/tests/live_apply_effect_roll.rs 的
/// seed_actor 前例）：bootstrap session 无角色 → HP seed 缺失 → 资源 Subtract
/// 在 0 处被引擎正确钳住，伤害无从观测。给 pc.current 一张最小 sheet
/// （resources.{hp_id}=10），伤害链路本身仍全部走真实 mechanics 原语。
async fn seed_test_character(db: &Db, session_id: &str, ruleset: &str, hp_id: &str) -> Result<()> {
    sqlx::query(r#"insert into runtime_actor_parameters
        (id, actor_param_id, session_id, actor_id, actor_kind, ruleset_id, source_kind, template_id, display_name, sheet_json, mechanical_profile, status_json, visibility, created_at_tick, updated_at_tick)
        values (gen_random_uuid(), $1, $2, 'pc.current', 'player_character', $3, 'test_seed', null, 'pc.current', $4, '{}'::jsonb, '{}'::jsonb, 'gm_only', 0, 0)
        on conflict (session_id, actor_id) do update set sheet_json=excluded.sheet_json"#)
        .bind(format!("ap_retro_{session_id}"))
        .bind(session_id)
        .bind(ruleset)
        .bind(json!({"resources": {hp_id: 10}}))
        .execute(&db.pool)
        .await?;
    Ok(())
}

async fn run_turn(gm: &mut GmLoop, session_id: &str, ruleset: &str, user_input: &str) -> Result<(String, TurnOutcome)> {
    let turn_id = format!("turn_{}", uuid::Uuid::new_v4().simple());
    let request = ContextRequest {
        ruleset_id: ruleset.to_string(),
        module_id: None,
        session_id: session_id.to_string(),
        turn_id: turn_id.clone(),
        viewer: VisibilityProfile::gm(),
        token_budget: TokenBudget::default(),
    };
    let state = RuntimeState { ruleset_id: ruleset.to_string(), ..Default::default() };
    let outcome = gm
        .run_gm_turn(GmTurnInput { request: &request, state: &state, user_input, history: &[], recent_transcript: None }, &mut |_| {})
        .await?;
    Ok((turn_id, outcome))
}

/// 回合 1 公共断言：恰 1 条追溯债务（kind=debt，debt_ 前缀），finding detail 带
/// B7 子串扫描回退前缀（本回合账本为空 → referenced_ledger_ids 为空 → 回退路径，
/// 一并断言 = B7 可观测技术债半边）。返回 debt_id。
fn assert_single_retro_debt(gm: &GmLoop) -> String {
    let blocking = gm.obligations.blocking();
    assert_eq!(blocking.len(), 1, "exactly one retro debt expected after turn 1: {blocking:?}");
    assert_eq!(blocking[0].kind, "debt", "obligation kind must be debt: {blocking:?}");
    assert!(blocking[0].target_id.starts_with("debt_"), "debt id shape: {}", blocking[0].target_id);
    assert!(
        blocking[0].summary.contains("fallback:substring_scan"),
        "InventedEffect detail must carry the B7 fallback marker: {}",
        blocking[0].summary
    );
    blocking[0].target_id.clone()
}

/// 回合 2 首请求的 dynamic tail（最后一条消息）必须回填债务：debt_id + finding 摘要。
fn assert_turn2_tail_carries_debt(llm: &MockLlm, debt_id: &str) {
    let requests = llm.requests.lock().unwrap();
    let turn2_first = requests.get(1).expect("turn 2 must have issued at least one request").clone();
    let tail = turn2_first.last().and_then(|m| m.get("content")).and_then(Value::as_str).unwrap_or("");
    assert!(tail.contains("[obligations_carryover]"), "turn 2 tail missing obligations block: {tail}");
    assert!(tail.contains(debt_id), "turn 2 tail missing debt id {debt_id}: {tail}");
    assert!(tail.contains("fallback:substring_scan"), "turn 2 tail missing finding summary: {tail}");
}

/// 用例 1（验收 11 主链）：InventedEffect → 追溯债务 → 下回合补 apply_effect
/// 落账 → 债务清除进叙事轮 → 库中状态真实变化（HP path -3）。
#[tokio::test]
#[ignore]
async fn retro_debt_settled_by_apply_effect_changes_db_state() -> Result<()> {
    // 回合 1：MockLlm 不调任何工具，直接叙事机械效果（账本为空）。
    let turn1 = vec![content("子弹擦过你的肩膀，你失去了 3 点生命。"), done("stop")];
    let (mut gm, llm, session_id, ruleset, kernel) = fixture(turn1).await?;
    let hp_id = trpg_model::hp_resource_track_id(&kernel.resource_tracks)
        .unwrap_or_else(|| panic!("kernel for {ruleset} has no HP-like resource track"));
    seed_test_character(&gm.engine.db, &session_id, &ruleset, &hp_id).await?;
    let (_t1, out1) = run_turn(&mut gm, &session_id, &ruleset, "我冲过街口").await?;
    assert!(matches!(out1, TurnOutcome::Narration(_)), "turn 1 must end as narration");
    let debt_id = assert_single_retro_debt(&gm);
    // 库面 before：与 mechanics 执行器同一原语（缺省 0 与执行器 unwrap_or(0) 对齐）。
    let before = gm.engine.db.load_resource_current(&session_id, "pc.current", &hp_id, &kernel).await.unwrap_or(0);
    // 回合 2 脚本三步：step1 收到 BP3 债务回填（请求面断言）；step2 补 apply_effect
    // （HP -3，target=测试角色）；step3 正常叙事收尾。
    llm.push_script(vec![
        tool_call("apply_effect", json!({"target_actor": "pc.current", "track_id": hp_id, "op": "subtract", "amount": 3, "reason": "settle retroactive debt from last turn"})),
        done("tool_calls"),
    ]);
    llm.push_script(vec![content("你包扎好肩头，喘了口气。"), done("stop")]);
    let (_t2, out2) = run_turn(&mut gm, &session_id, &ruleset, "我处理伤口").await?;
    assert_turn2_tail_carries_debt(&llm, &debt_id);
    // 债务清除 → 进叙事轮（narrated），而非被门控回填挡住。
    match &out2 {
        TurnOutcome::Narration(text) => assert!(text.contains("包扎"), "turn 2 narration mismatch: {text}"),
        other => panic!("turn 2 must reach narration after settlement, got {other:?}"),
    }
    assert!(gm.obligations.blocking().is_empty(), "debt must be settled by the apply_effect ledger entry: {:?}", gm.obligations.blocking());
    // 库面终验（验收 11 的"库中状态真实变化"）：HP path 的 value 真实 -3。
    let path = format!("resources.{hp_id}.current");
    let after: Option<Value> = sqlx::query_scalar(
        "select value_json from generic_parameter_states where session_id=$1 and target_kind='actor' and target_id='pc.current' and parameter_path=$2",
    )
    .bind(&session_id)
    .bind(&path)
    .fetch_optional(&gm.engine.db.pool)
    .await?;
    let after_n = after.as_ref().and_then(Value::as_i64).unwrap_or_else(|| panic!("no generic_parameter_states row for {path}: {after:?}"));
    assert_eq!(after_n, i64::from(before) - 3, "HP current must drop by exactly 3 (before={before})");
    println!("SESSION_ID={session_id} HP_PATH={path} BEFORE={before} AFTER={after_n}");
    Ok(())
}

/// 用例 2（验收 11 或然分支）：waive_obligation(debt_id, reason) → blocking 清空
/// 进叙事轮 + memory_events 落 gm_waive 审计（库面复核）。
#[tokio::test]
#[ignore]
async fn retro_debt_waived_with_reason_unblocks_and_audits() -> Result<()> {
    let turn1 = vec![content("爆炸的余波让你失去了 2 点理智。"), done("stop")];
    let (mut gm, llm, session_id, ruleset, _kernel) = fixture(turn1).await?;
    let (_t1, out1) = run_turn(&mut gm, &session_id, &ruleset, "我回头看爆炸").await?;
    assert!(matches!(out1, TurnOutcome::Narration(_)), "turn 1 must end as narration");
    let debt_id = assert_single_retro_debt(&gm);
    // 回合 2：waive 带理由（spec §5.3 原文示例语义）→ 正常叙事。
    llm.push_script(vec![
        tool_call("waive_obligation", json!({"target_id": debt_id, "reason": "叙事中已收回该说法"})),
        done("tool_calls"),
    ]);
    llm.push_script(vec![content("夜色重新安静下来。"), done("stop")]);
    let (_t2, out2) = run_turn(&mut gm, &session_id, &ruleset, "我深呼吸稳住自己").await?;
    assert_turn2_tail_carries_debt(&llm, &debt_id);
    match &out2 {
        TurnOutcome::Narration(text) => assert!(text.contains("安静"), "turn 2 narration mismatch: {text}"),
        other => panic!("turn 2 must reach narration after waive, got {other:?}"),
    }
    assert!(gm.obligations.blocking().is_empty(), "waive must unblock the ledger: {:?}", gm.obligations.blocking());
    // 库面复核：gm_waive 审计事件真实落 memory_events。
    let waive_rows: i64 = sqlx::query_scalar("select count(*) from memory_events where session_id=$1 and 'gm_waive'=any(tags)")
        .bind(&session_id)
        .fetch_one(&gm.engine.db.pool)
        .await?;
    assert!(waive_rows >= 1, "expected at least one gm_waive audit row, got {waive_rows}");
    println!("SESSION_ID={session_id} WAIVE_AUDIT_ROWS={waive_rows}");
    Ok(())
}

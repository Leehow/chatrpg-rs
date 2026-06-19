//! GM agent 战斗冻结修复 e2e（`#[ignore]`，真 DB + MockLlm 脚本）：
//! request_player_roll 在桌面骰权政策双分支下的行为。
//!
//! 跑法（cwd 在 crate 目录——cargo test -p 的既有坑）：
//! ```bash
//! cd crates/trpg-gm && DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
//!   cargo test -p trpg-gm --test player_roll_policy_e2e -- --ignored --nocapture
//! ```
//! 两分支必须在同一测试函数内串行跑（TRPG_AGENT_TABLE_DICE_POLICY 是进程级
//! env，并行测试会互踩），各用独立 session 隔离库面。断言只看结构面：
//! 终态种类 / dice_rolls 行数 / 契约 roll_authority / open gate 有无。

use anyhow::Result;
use async_stream::try_stream;
use async_trait::async_trait;
use futures_core::Stream;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use trpg_db::Db;
use trpg_gm::{GmLoop, GmTurnInput, LoopConfig, ToolRegistry, TurnOutcome};
use trpg_llm::{AggregatedToolCall, LlmClient, StreamEvent, ToolChoice};
use trpg_model::{ChatMessage, ContextRequest, RuntimeState, TokenBudget, VisibilityProfile};
use trpg_runtime::RuntimeEngine;

struct MockLlm {
    scripts: Mutex<Vec<Vec<StreamEvent>>>,
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
    async fn stream_chat(
        &self,
        _: Vec<ChatMessage>,
        _: f32,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        unimplemented!("unused by run_gm_turn e2e")
    }
    async fn complete_with_tools(&self, _: Vec<Value>, _: Vec<Value>) -> Result<Value> {
        unimplemented!("unused by run_gm_turn e2e")
    }
    async fn stream_chat_with_tools(
        &self,
        _: Vec<Value>,
        _: Vec<Value>,
        _: ToolChoice,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let mut scripts = self.scripts.lock().unwrap();
        assert!(
            !scripts.is_empty(),
            "MockLlm script exhausted: the loop asked for one more round than scripted"
        );
        let script = scripts.remove(0);
        let s = try_stream! { for event in script { yield event; } };
        Ok(Box::pin(s))
    }
}

fn content(text: &str) -> StreamEvent {
    StreamEvent::ContentDelta(text.to_string())
}

fn done(reason: &str) -> StreamEvent {
    StreamEvent::Done {
        finish_reason: Some(reason.to_string()),
    }
}

fn tool_call(name: &str, args: Value) -> StreamEvent {
    StreamEvent::ToolCalls(vec![AggregatedToolCall {
        id: format!("call_{name}"),
        name: name.to_string(),
        arguments: args.to_string(),
    }])
}

/// 真库装配（对标 retro_debt_e2e::fixture）：真 ToolRegistry::standard()——
/// 被测对象正是真 RequestPlayerRollTool 的政策分支。
async fn fixture() -> Result<(GmLoop, Arc<MockLlm>, String, String)> {
    let url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must point at the live test DB (e.g. :54347)");
    let db = Db::connect(&url).await?;
    db.migrate().await?;
    let ruleset =
        std::env::var("TRPG_TEST_RULESET").unwrap_or_else(|_| "call_of_cthulhu_7e".to_string());
    db.load_rule_kernel(&ruleset)
        .await?
        .unwrap_or_else(|| panic!("no active kernel for {ruleset}; run parse-all first"));
    let engine = RuntimeEngine::new(db.clone());
    let session_id = engine.start_session(&ruleset, None).await?;
    println!("SESSION_ID={session_id}");
    let llm = Arc::new(MockLlm {
        scripts: Mutex::new(Vec::new()),
    });
    let data_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data");
    let gm = GmLoop::new(
        engine,
        llm.clone(),
        ToolRegistry::standard(),
        LoopConfig::default(),
        data_dir,
    );
    Ok((gm, llm, session_id, ruleset))
}

async fn run_turn(
    gm: &mut GmLoop,
    session_id: &str,
    ruleset: &str,
    user_input: &str,
) -> Result<TurnOutcome> {
    let turn_id = format!("turn_{}", uuid::Uuid::new_v4().simple());
    let request = ContextRequest {
        ruleset_id: ruleset.to_string(),
        module_id: None,
        session_id: session_id.to_string(),
        turn_id,
        viewer: VisibilityProfile::gm(),
        token_budget: TokenBudget::default(),
    };
    let state = RuntimeState {
        ruleset_id: ruleset.to_string(),
        ..Default::default()
    };
    gm.run_gm_turn(
        GmTurnInput {
            request: &request,
            state: &state,
            user_input,
            history: &[],
            recent_transcript: None,
        },
        &mut |_| {},
    )
    .await
}

fn player_roll_args() -> Value {
    json!({
        "check_label": "Dodge",
        "tested_parameter": "dodge",
        "stakes": {"before": "它的爪子撕向你的咽喉。", "success": "你侧身让开。", "failure": "利爪撕开你的肩膀。"}
    })
}

async fn dice_roll_count(db: &Db, session_id: &str) -> Result<i64> {
    Ok(
        sqlx::query_scalar("select count(*) from dice_rolls where session_id=$1")
            .bind(session_id)
            .fetch_one(&db.pool)
            .await?,
    )
}

/// 双分支验收：
/// A（政策关=真人摇骰桌）：request_player_roll 照旧开 gate，AwaitingPlayerRoll 终态。
/// B（政策开=system_rolls_visible，产品默认）：同一工具调用被转系统代掷当场结算
/// ——回合走到叙事终态、dice_rolls 真有行、契约 roll_authority=system、无 open gate
/// （= 战斗不再冻结在 awaiting_player_roll/零掷骰）。
#[tokio::test]
#[ignore]
async fn request_player_roll_honors_table_dice_policy_both_branches() -> Result<()> {
    // —— 分支 A：政策关，gate 行为原样保留（回归保护）——
    std::env::set_var("TRPG_AGENT_TABLE_DICE_POLICY", "player_rolls");
    let (mut gm, llm, session_a, ruleset) = fixture().await?;
    llm.push_script(vec![
        tool_call("request_player_roll", player_roll_args()),
        done("tool_calls"),
    ]);
    let out_a = run_turn(&mut gm, &session_a, &ruleset, "我朝那东西开枪").await?;
    match &out_a {
        TurnOutcome::AwaitingPlayerRoll { prompt_public, .. } => {
            assert!(
                !prompt_public.trim().is_empty(),
                "gate prompt must be non-empty"
            );
        }
        other => panic!("player_rolls policy must keep the gate terminal state, got {other:?}"),
    }
    let pending = gm.engine.db.get_open_pending_check(&session_a).await?;
    assert!(
        pending.is_some(),
        "an open pending check must exist under player_rolls policy"
    );
    assert_eq!(
        dice_roll_count(&gm.engine.db, &session_a).await?,
        0,
        "no system roll may happen under player_rolls policy"
    );

    // —— 分支 B：政策开（产品默认），转系统代掷当场结算 ——
    std::env::set_var("TRPG_AGENT_TABLE_DICE_POLICY", "system_rolls_visible");
    let (mut gm, llm, session_b, ruleset) = fixture().await?;
    llm.push_script(vec![
        tool_call("request_player_roll", player_roll_args()),
        done("tool_calls"),
    ]);
    llm.push_script(vec![
        content("枪声未落，那东西已扑到半空——你堪堪侧身让开。"),
        done("stop"),
    ]);
    let out_b = run_turn(&mut gm, &session_b, &ruleset, "我朝那东西开枪").await?;
    match &out_b {
        TurnOutcome::Narration(text) => {
            assert!(!text.trim().is_empty(), "narration must be non-empty")
        }
        other => panic!(
            "system_rolls_visible policy must settle in-turn and reach narration, got {other:?}"
        ),
    }
    assert!(
        dice_roll_count(&gm.engine.db, &session_b).await? >= 1,
        "the converted check must record at least one dice roll"
    );
    let authority: Option<String> = sqlx::query_scalar(
        "select contract_json->>'roll_authority' from check_contracts where session_id=$1 order by created_at desc limit 1",
    )
    .bind(&session_b)
    .fetch_optional(&gm.engine.db.pool)
    .await?;
    assert_eq!(
        authority.as_deref(),
        Some("system"),
        "stored contract must carry system authority after conversion"
    );
    assert!(
        gm.engine
            .db
            .get_open_pending_check(&session_b)
            .await?
            .is_none(),
        "no gate may stay open under system_rolls_visible policy"
    );
    println!("BRANCH_A_SESSION={session_a} BRANCH_B_SESSION={session_b}");
    Ok(())
}

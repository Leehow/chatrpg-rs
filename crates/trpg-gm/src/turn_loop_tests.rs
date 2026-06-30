use super::*;
use crate::ledger::TurnLedger;
use crate::tools::{
    AwaitingPlayerRoll, GmTool, ToolCtx, ToolError, ToolOutput, ToolRegistry, ToolSpec,
};
use anyhow::Result;
use async_stream::try_stream;
use async_trait::async_trait;
use chrono::Utc;
use futures_core::Stream;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use std::fs;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use trpg_db::Db;
use trpg_llm::{AggregatedToolCall, LlmClient, StreamEvent, ToolChoice};
use trpg_model::{
    ActorKind, ActorRef, ChatMessage, CheckContract, CheckResultRecord, CheckStakes,
    CheckTargetModel, CompiledContext, ContextRequest, DiceRollRecord, DueSource, DueStatus,
    MechanicDue, OppositionModel, RollAuthority, RollDisclosurePolicy, RollVisibility,
    RulingConfidence, RulingStatus, RuntimeState, TokenBudget, VisibilityProfile,
};
use trpg_runtime::AutoRollExecution;

static TURN_LOOP_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct EnvVarRestore {
    key: &'static str,
    previous: Option<String>,
}

impl Drop for EnvVarRestore {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

fn set_test_env_var(key: &'static str, value: &str) -> EnvVarRestore {
    let previous = std::env::var(key).ok();
    std::env::set_var(key, value);
    EnvVarRestore { key, previous }
}

fn clear_test_env_var(key: &'static str) -> EnvVarRestore {
    let previous = std::env::var(key).ok();
    std::env::remove_var(key);
    EnvVarRestore { key, previous }
}

struct MockLlm {
    scripts: Mutex<Vec<Vec<StreamEvent>>>,
    choices: Mutex<Vec<ToolChoice>>,
    requests: Mutex<Vec<Vec<Value>>>,
    json_responses: Mutex<Vec<Value>>,
    json_requests: Mutex<Vec<Vec<ChatMessage>>>,
    text_responses: Mutex<Vec<String>>,
    text_requests: Mutex<Vec<Vec<ChatMessage>>>,
}

#[async_trait]
impl LlmClient for MockLlm {
    async fn complete_text(&self, messages: Vec<ChatMessage>, _: f32) -> Result<String> {
        self.text_requests.lock().unwrap().push(messages);
        Ok(self
            .text_responses
            .lock()
            .unwrap()
            .pop()
            .unwrap_or_default())
    }
    async fn complete_json(&self, messages: Vec<ChatMessage>, _: f32) -> Result<Value> {
        self.json_requests.lock().unwrap().push(messages);
        Ok(self
            .json_responses
            .lock()
            .unwrap()
            .pop()
            .unwrap_or_else(|| json!({"hits": []})))
    }
    async fn stream_chat(
        &self,
        _: Vec<ChatMessage>,
        _: f32,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        unimplemented!("MockLlm stream_chat is unused by run_gm_turn tests")
    }
    async fn complete_with_tools(&self, _: Vec<Value>, _: Vec<Value>) -> Result<Value> {
        unimplemented!("MockLlm complete_with_tools is unused by run_gm_turn tests")
    }
    async fn stream_chat_with_tools(
        &self,
        messages: Vec<Value>,
        _: Vec<Value>,
        tool_choice: ToolChoice,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        self.requests.lock().unwrap().push(messages);
        self.choices.lock().unwrap().push(tool_choice);
        let script = self.scripts.lock().unwrap().remove(0);
        let s = try_stream! { for event in script { yield event; } };
        Ok(Box::pin(s))
    }
}

/// 脚本化 GmTool 替身（真工具 roll_check/request_player_roll 都打 DB，纯
/// MockLlm 单测跑不通；经 ToolRegistry::from_tools 注入）。
/// record=true ⇒ 往账本写一条合成契约外的私骰结果（场景 8 入账观测）；
/// fail_code=Some ⇒ 返回该结构化 ToolError（场景 7）；awaiting=true ⇒ gate 终态。
struct ScriptedTool {
    name: &'static str,
    record: bool,
    fail_code: Option<&'static str>,
    awaiting: bool,
}

#[async_trait]
impl GmTool for ScriptedTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name,
            schema: json!({"type":"function","function":{"name": self.name, "description":"test double","parameters":{"type":"object","properties":{}}}}),
        }
    }

    async fn call(
        &self,
        _ctx: &ToolCtx<'_>,
        ledger: &mut TurnLedger,
        _args: Value,
    ) -> Result<ToolOutput> {
        if let Some(code) = self.fail_code {
            return Err(ToolError::recoverable(
                code,
                "scripted failure",
                Some("switch to another tool".to_string()),
            ));
        }
        if self.record {
            let roll = DiceRollRecord {
                roll_id: "roll_secret".to_string(),
                session_id: "s".to_string(),
                turn_id: "t".to_string(),
                check_id: Some("check_x".to_string()),
                roller_kind: ActorKind::System,
                roller_id: None,
                visibility: RollVisibility::PrivateGmRoll,
                expression: "1d100".to_string(),
                result: json!({"total": 91}),
                seed_commitment: "seed".to_string(),
                revealed_at: None,
                created_at: Utc::now(),
            };
            let result = CheckResultRecord {
                check_id: "check_x".to_string(),
                roll,
                outcome: json!({"total": 91}),
                committed_patches: vec![],
                created_at: Utc::now(),
            };
            ledger.record_execution(&AutoRollExecution {
                primary: result,
                followups: vec![],
                roll_policy: "system_rolls_hidden".to_string(),
            });
        }
        if self.awaiting {
            return Ok(ToolOutput::awaiting(
                json!({"check_id": "check_gate"}),
                AwaitingPlayerRoll {
                    check_id: "check_gate".to_string(),
                    prompt_public: "Roll 1d100 now.".to_string(),
                },
            ));
        }
        Ok(ToolOutput::ok(json!({"ok": true})))
    }
}

fn tool_call(name: &str) -> AggregatedToolCall {
    AggregatedToolCall {
        id: format!("call_{name}"),
        name: name.to_string(),
        arguments: "{}".to_string(),
    }
}

/// lazy pool 不真连接；prepare_turn_context 经 ctx_provider seam 绕开（它
/// 必须连真 DB + 已 parse 的 project bundle）；load_gm_skill fail-closed，
/// 故 fixture 自带临时 gm_skill 目录。
fn loop_fixture(
    scripts: Vec<Vec<StreamEvent>>,
    tools: ToolRegistry,
    max_tool_rounds: u8,
) -> (GmLoop, Arc<MockLlm>, ContextRequest, RuntimeState) {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://chatrpg:chatrpg@localhost:54347/chatrpg")
        .expect("lazy pool");
    let engine = RuntimeEngine::new(Db { pool });
    let llm = Arc::new(MockLlm {
        scripts: Mutex::new(scripts),
        choices: Mutex::new(Vec::new()),
        requests: Mutex::new(Vec::new()),
        text_responses: Mutex::new(Vec::new()),
        text_requests: Mutex::new(Vec::new()),
        json_responses: Mutex::new(Vec::new()),
        json_requests: Mutex::new(Vec::new()),
    });
    let data_dir = std::env::temp_dir().join(format!(
        "gm_loop_test_{}_{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    fs::create_dir_all(data_dir.join("agent/gm_skill/global")).unwrap();
    fs::write(
        data_dir.join("agent/gm_skill/global/10_test.md"),
        "test gm skill",
    )
    .unwrap();
    let mut gm = GmLoop::new(
        engine,
        llm.clone(),
        tools,
        LoopConfig {
            max_tool_rounds,
            repeat_finding_threshold: 3,
        },
        data_dir,
    );
    gm.ctx_provider = Some(Arc::new(|_req, _state| CompiledContext {
        prefix_text: "BP1".to_string(),
        pinned_text: "BP2".to_string(),
        dynamic_text: "BP3".to_string(),
        prefix_hash: "p".to_string(),
        pinned_hash: "m".to_string(),
        dynamic_hash: "d".to_string(),
        ..Default::default()
    }));
    let request = ContextRequest {
        ruleset_id: "rs".to_string(),
        module_id: None,
        session_id: "s".to_string(),
        turn_id: "t".to_string(),
        viewer: VisibilityProfile::gm(),
        token_budget: TokenBudget::default(),
    };
    let state = RuntimeState {
        ruleset_id: "rs".to_string(),
        ..Default::default()
    };
    (gm, llm, request, state)
}

/// 合成 gate 用最小契约（字段全集对齐 trpg-agent gm_loop.rs sample_check）。
fn gate_contract(check_id: &str) -> CheckContract {
    CheckContract {
        check_id: check_id.into(),
        session_id: "s".into(),
        turn_id: "t".into(),
        ruleset_id: "rs".into(),
        module_id: None,
        initiator: ActorRef {
            actor_id: "pc.current".into(),
            actor_kind: ActorKind::PlayerCharacter,
            display_name: Some("PC".into()),
        },
        target_actor: None,
        opposition: OppositionModel::NoMechanicalOpposition,
        action_summary: "climb".into(),
        intent_kind: "athletics".into(),
        check_label: "Climb".into(),
        dice_expression: "1d100".into(),
        modifiers: vec![],
        target: CheckTargetModel::StaticNumber {
            value: 50,
            label: "skill".into(),
        },
        tested_parameter: None,
        opponent_tested_parameter: None,
        actor_snapshot_ids: vec![],
        source_refs: vec![],
        learned_packet_ids: vec![],
        roll_visibility: RollVisibility::PublicGmRoll,
        roll_authority: RollAuthority::System,
        disclosure: RollDisclosurePolicy::for_visibility(RollVisibility::PublicGmRoll),
        stakes: CheckStakes {
            before_roll_public: "risky".into(),
            success_public: "ok".into(),
            failure_public: "bad".into(),
            critical_public: None,
            fumble_public: None,
            success_patches_allowed: vec![],
            failure_patches_allowed: vec![],
            irreversible: false,
        },
        confidence: RulingConfidence::High,
        ruling_status: RulingStatus::SourceBacked,
        advice_refs: vec![],
        expires_at_turn: Some("t".into()),
    }
}

fn gate_result(check_id: &str) -> CheckResultRecord {
    let roll = DiceRollRecord {
        roll_id: format!("roll_{check_id}"),
        session_id: "s".to_string(),
        turn_id: "t".to_string(),
        check_id: Some(check_id.to_string()),
        roller_kind: ActorKind::PlayerCharacter,
        roller_id: Some("pc.current".to_string()),
        visibility: RollVisibility::PublicGmRoll,
        expression: "1d100".to_string(),
        result: json!({"total": 27}),
        seed_commitment: "seed".to_string(),
        revealed_at: None,
        created_at: Utc::now(),
    };
    CheckResultRecord {
        check_id: check_id.to_string(),
        roll,
        outcome: json!({"success": true}),
        committed_patches: vec![],
        created_at: Utc::now(),
    }
}

#[tokio::test]
async fn bare_roll_reply_resolves_open_gate_in_header() {
    // e2e must-fix 回归：玩家只回 "roll"（parse_roll_text → None）也必须走
    // 头部 gate 结算路径（wants_system_roll 兜底），而非让 agent 重复开 gate。
    let (mut gm, llm, request, state) = loop_fixture(
        vec![vec![
            StreamEvent::ContentDelta("done".to_string()),
            StreamEvent::Done {
                finish_reason: Some("stop".to_string()),
            },
        ]],
        ToolRegistry::from_tools(vec![]),
        1,
    );
    let resolved = Arc::new(Mutex::new(false));
    let flag = resolved.clone();
    gm.gate_resolver = Some(Arc::new(move |input| {
        assert_eq!(input, "roll");
        *flag.lock().unwrap() = true;
        Some(Ok((gate_contract("check_gate"), gate_result("check_gate"))))
    }));
    let result = gm
        .run_gm_turn(
            GmTurnInput {
                request: &request,
                state: &state,
                user_input: "roll",
                history: &[],
                recent_transcript: None,
            },
            &mut |_| {},
        )
        .await
        .unwrap();
    assert!(
        *resolved.lock().unwrap(),
        "header gate resolution path was not taken for bare roll reply"
    );
    assert!(matches!(result, TurnOutcome::Narration(_)));
    // 结算事实必须经 dynamic tail 注入本回合上下文（与 gate 结算事实同通道）。
    let first_request = llm.requests.lock().unwrap()[0].clone();
    let tail = first_request
        .last()
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        tail.contains("[Resolved Gate Facts]"),
        "dynamic tail missing gate facts: {tail}"
    );
    assert!(tail.contains("Resolved pending check check_gate"));
}

#[tokio::test]
async fn gate_resolution_error_is_nonfatal_and_becomes_turn_fact() {
    // e2e must-fix 回归：报点回复结算 Err（如 reported totals 被禁）绝不
    // 终止会话循环——run_gm_turn 返回 Ok，失败原因作为事实注入本回合上下文。
    let (mut gm, llm, request, state) = loop_fixture(
        vec![vec![
            StreamEvent::ContentDelta("请重新掷骰。".to_string()),
            StreamEvent::Done {
                finish_reason: Some("stop".to_string()),
            },
        ]],
        ToolRegistry::from_tools(vec![]),
        1,
    );
    gm.gate_resolver = Some(Arc::new(|_| {
        Some(Err(anyhow::anyhow!(
            "player-reported roll totals are disabled"
        )))
    }));
    let result = gm
        .run_gm_turn(
            GmTurnInput {
                request: &request,
                state: &state,
                user_input: "34",
                history: &[],
                recent_transcript: None,
            },
            &mut |_| {},
        )
        .await;
    let outcome = result.expect("gate resolution error must not abort the turn");
    assert_eq!(outcome, TurnOutcome::Narration("请重新掷骰。".to_string()));
    let first_request = llm.requests.lock().unwrap()[0].clone();
    let tail = first_request
        .last()
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        tail.contains("Pending check resolution failed"),
        "failure fact missing from dynamic tail: {tail}"
    );
    assert!(tail.contains("player-reported roll totals are disabled"));
}

#[tokio::test]
async fn content_delta_streams_to_callback() {
    let (mut gm, _llm, request, state) = loop_fixture(
        vec![vec![
            StreamEvent::ContentDelta("A".to_string()),
            StreamEvent::ContentDelta("B".to_string()),
            StreamEvent::Done {
                finish_reason: Some("stop".to_string()),
            },
        ]],
        ToolRegistry::from_tools(vec![]),
        1,
    );
    let mut out = String::new();
    let result = gm
        .run_gm_turn(
            GmTurnInput {
                request: &request,
                state: &state,
                user_input: "go",
                history: &[],
                recent_transcript: None,
            },
            &mut |d| out.push_str(d),
        )
        .await
        .unwrap();
    assert_eq!(out, "AB");
    assert_eq!(result, TurnOutcome::Narration("AB".to_string()));
}

#[tokio::test]
async fn debt_free_round_streams_deltas_incrementally() {
    // D2 真流式回归（B6 门控前移终审修复）：无债回合的 ContentDelta 必须
    // 逐块到达 on_delta（恢复一期行为）——缓冲到流结束一次性 flush 会把
    // 两块合并成单次回调，此断言即红。
    let scripts = vec![vec![
        StreamEvent::ContentDelta("第一块。".to_string()),
        StreamEvent::ContentDelta("第二块。".to_string()),
        StreamEvent::Done {
            finish_reason: Some("stop".to_string()),
        },
    ]];
    let (mut gm, _llm, request, state) = loop_fixture(scripts, ToolRegistry::from_tools(vec![]), 1);
    let chunks = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = chunks.clone();
    let result = gm
        .run_gm_turn(
            GmTurnInput {
                request: &request,
                state: &state,
                user_input: "go",
                history: &[],
                recent_transcript: None,
            },
            &mut move |d| sink.lock().unwrap().push(d.to_string()),
        )
        .await
        .unwrap();
    assert_eq!(
        *chunks.lock().unwrap(),
        vec!["第一块。".to_string(), "第二块。".to_string()],
        "deltas must reach on_delta chunk-by-chunk, not as one flush"
    );
    assert_eq!(
        result,
        TurnOutcome::Narration("第一块。第二块。".to_string())
    );
}

#[tokio::test]
async fn max_rounds_forces_tool_choice_none() {
    // max_tool_rounds=1：第 1 轮 Auto 仍要工具 → 循环耗尽 → 追加唯一一轮 None。
    // tool_choice 序列必须是 [Auto, None]（循环内绝不提前 None、绝无双重 None）。
    let (mut gm, llm, request, state) = loop_fixture(
        vec![
            vec![
                StreamEvent::ToolCalls(vec![]),
                StreamEvent::Done {
                    finish_reason: Some("tool_calls".to_string()),
                },
            ],
            vec![
                StreamEvent::ContentDelta("final".to_string()),
                StreamEvent::Done {
                    finish_reason: Some("stop".to_string()),
                },
            ],
        ],
        ToolRegistry::from_tools(vec![]),
        1,
    );
    let mut out = String::new();
    let _ = gm
        .run_gm_turn(
            GmTurnInput {
                request: &request,
                state: &state,
                user_input: "go",
                history: &[],
                recent_transcript: None,
            },
            &mut |d| out.push_str(d),
        )
        .await
        .unwrap();
    let choices = llm.choices.lock().unwrap().clone();
    assert_eq!(choices, vec![ToolChoice::Auto, ToolChoice::None]);
    assert_eq!(out, "final");
}

#[tokio::test]
async fn tool_products_enter_ledger_and_private_tokens_are_redacted() {
    // spec §9 场景 1+4+8（单测可观测面）：工具轮产物入账 →
    // 下一轮 content 里的私骰 token（"91"）被滑动缓冲过滤。
    let tools = ToolRegistry::from_tools(vec![Box::new(ScriptedTool {
        name: "roll_check",
        record: true,
        fail_code: None,
        awaiting: false,
    })]);
    let scripts = vec![
        vec![
            StreamEvent::ToolCalls(vec![tool_call("roll_check")]),
            StreamEvent::Done {
                finish_reason: Some("tool_calls".to_string()),
            },
        ],
        vec![
            StreamEvent::ContentDelta("The hidden total is 9".to_string()),
            StreamEvent::ContentDelta("1, but you only see shadows.".to_string()),
            StreamEvent::Done {
                finish_reason: Some("stop".to_string()),
            },
        ],
    ];
    let (mut gm, llm, request, state) = loop_fixture(scripts, tools, 4);
    let mut out = String::new();
    let result = gm
        .run_gm_turn(
            GmTurnInput {
                request: &request,
                state: &state,
                user_input: "go",
                history: &[],
                recent_transcript: None,
            },
            &mut |d| out.push_str(d),
        )
        .await
        .unwrap();
    assert!(matches!(result, TurnOutcome::Narration(_)));
    assert!(!out.contains("91"), "private roll token leaked: {out}");
    assert!(out.contains("■"));
    // 第二轮请求里必须有第一轮的 tool 回填消息（账本/消息尾部追加联动）。
    let second_request = llm.requests.lock().unwrap()[1].clone();
    assert!(second_request
        .iter()
        .any(|m| m.get("role").and_then(Value::as_str) == Some("tool")
            && m.get("name").and_then(Value::as_str) == Some("roll_check")));
}

/// 入账一笔玩家可见掷骰结果的替身（spec §9 场景 5 用：叙事若漏提任何可见
/// token，流后 NarrationVerifier 应产出 OmittedVisibleResult finding）。
struct VisibleRollTool;

#[async_trait]
impl GmTool for VisibleRollTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "roll_check",
            schema: json!({"type":"function","function":{"name":"roll_check","description":"test double","parameters":{"type":"object","properties":{}}}}),
        }
    }

    async fn call(
        &self,
        _ctx: &ToolCtx<'_>,
        ledger: &mut TurnLedger,
        _args: Value,
    ) -> Result<ToolOutput> {
        let roll = DiceRollRecord {
            roll_id: "roll_pub".to_string(),
            session_id: "s".to_string(),
            turn_id: "t".to_string(),
            check_id: Some("check_pub".to_string()),
            roller_kind: ActorKind::System,
            roller_id: None,
            visibility: RollVisibility::PublicGmRoll,
            expression: "1d100".to_string(),
            result: json!({"total": 42}),
            seed_commitment: "seed".to_string(),
            revealed_at: None,
            created_at: Utc::now(),
        };
        let result = CheckResultRecord {
            check_id: "check_pub".to_string(),
            roll,
            outcome: json!({"total": 42}),
            committed_patches: vec![],
            created_at: Utc::now(),
        };
        ledger.record_execution(&AutoRollExecution {
            primary: result,
            followups: vec![],
            roll_policy: "system_rolls_visible".to_string(),
        });
        Ok(ToolOutput::ok(json!({"ok": true})))
    }
}

#[tokio::test]
async fn omitted_visible_result_feeds_errata_into_next_turn_tail() {
    // spec §9 场景 5 全链路：可见掷骰入账 + 终态叙事漏提全部可见 token →
    // 流后 verifier 产出 OmittedVisibleResult → 勘误记忆 → 下一回合
    // assemble 的 dynamic tail 注入 [gm_errata] 块（agent 可自洽勘误）。
    let tools = ToolRegistry::from_tools(vec![Box::new(VisibleRollTool)]);
    let scripts = vec![
        vec![
            StreamEvent::ToolCalls(vec![tool_call("roll_check")]),
            StreamEvent::Done {
                finish_reason: Some("tool_calls".to_string()),
            },
        ],
        vec![
            StreamEvent::ContentDelta("你看见影子晃动。".to_string()),
            StreamEvent::Done {
                finish_reason: Some("stop".to_string()),
            },
        ],
        vec![
            StreamEvent::ContentDelta("ok".to_string()),
            StreamEvent::Done {
                finish_reason: Some("stop".to_string()),
            },
        ],
    ];
    let (mut gm, llm, request, state) = loop_fixture(scripts, tools, 4);
    let mut sink = String::new();
    let _ = gm
        .run_gm_turn(
            GmTurnInput {
                request: &request,
                state: &state,
                user_input: "go",
                history: &[],
                recent_transcript: None,
            },
            &mut |d| sink.push_str(d),
        )
        .await
        .unwrap();
    assert_eq!(
        gm.errata.kind_counts().get("omitted_visible_result"),
        Some(&1)
    );
    // 第二回合：dynamic tail 必须携带上一回合的勘误块。
    let request2 = ContextRequest {
        turn_id: "t2".to_string(),
        ..request.clone()
    };
    let _ = gm
        .run_gm_turn(
            GmTurnInput {
                request: &request2,
                state: &state,
                user_input: "继续",
                history: &[],
                recent_transcript: None,
            },
            &mut |d| sink.push_str(d),
        )
        .await
        .unwrap();
    let turn2_request = llm.requests.lock().unwrap()[2].clone();
    let tail_content = turn2_request
        .last()
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        tail_content.contains("[gm_errata]"),
        "dynamic tail missing errata block: {tail_content}"
    );
    assert!(tail_content.contains("omitted_visible_result"));
}

/// B6 测试用最小 open due（零 per-ruleset 内容：track/desc 是测试夹具语义）。
fn seeded_due(id: &str, desc: &str) -> MechanicDue {
    MechanicDue {
        due_id: id.to_string(),
        session_id: "s".to_string(),
        turn_id: "t".to_string(),
        source: DueSource::Threshold,
        source_track: Some("sanity".to_string()),
        hook_event: None,
        mechanic_id: None,
        threshold_desc: desc.to_string(),
        followup_procedure_id: None,
        owner_kind: "actor".to_string(),
        owner_id: "pc.current".to_string(),
        evidence: json!({"before": 38, "after": 32, "delta": -6}),
        status: DueStatus::Open,
        created_at: Utc::now(),
    }
}

fn waive_call(target_id: &str) -> AggregatedToolCall {
    AggregatedToolCall {
        id: "call_waive".to_string(),
        name: "waive_obligation".to_string(),
        arguments: format!("{{\"target_id\":\"{target_id}\",\"reason\":\"handled in fiction\"}}"),
    }
}

#[tokio::test]
async fn open_due_blocks_narration_round() {
    // spec 验收 5（D2 真流式修复后语义）：ObligationLedger 有未处理 due →
    // 轮前门控判定该轮被门（blocked），该轮 content 实时丢弃、绝不流给
    // on_delta（不再缓冲后 flush）；下一轮轮前回填 block_text() 的 system
    // 观察继续循环（round 0 由 dynamic tail 的 obligations_block 告知，
    // 不重复回填）；waive 后 content 恢复直通流出。
    let tools =
        ToolRegistry::from_tools(vec![Box::new(crate::tools::mechanic::WaiveObligationTool)]);
    let scripts = vec![
        vec![
            StreamEvent::ContentDelta("premature ending".to_string()),
            StreamEvent::Done {
                finish_reason: Some("stop".to_string()),
            },
        ],
        vec![
            StreamEvent::ToolCalls(vec![waive_call("due_block")]),
            StreamEvent::Done {
                finish_reason: Some("tool_calls".to_string()),
            },
        ],
        vec![
            StreamEvent::ContentDelta("clean ending".to_string()),
            StreamEvent::Done {
                finish_reason: Some("stop".to_string()),
            },
        ],
    ];
    let (mut gm, llm, request, state) = loop_fixture(scripts, tools, 4);
    gm.obligations
        .absorb_dues(vec![seeded_due("due_block", "sanity threshold crossed")]);
    let mut out = String::new();
    let result = gm
        .run_gm_turn(
            GmTurnInput {
                request: &request,
                state: &state,
                user_input: "go",
                history: &[],
                recent_transcript: None,
            },
            &mut |d| out.push_str(d),
        )
        .await
        .unwrap();
    assert!(
        !out.contains("premature ending"),
        "blocked round content leaked: {out}"
    );
    assert_eq!(out, "clean ending");
    assert_eq!(result, TurnOutcome::Narration("clean ending".to_string()));
    // 第二轮请求里必须带 block_text() 的 system 观察回填。
    let second_request = llm.requests.lock().unwrap()[1].clone();
    let observation = second_request
        .iter()
        .rev()
        .find(|m| m.get("role").and_then(Value::as_str) == Some("system"))
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    assert!(
        observation.contains("[obligations]"),
        "missing obligations observation: {observation}"
    );
    assert!(observation.contains("sanity threshold crossed"));
    assert!(observation.contains("due_block"));
}

#[tokio::test]
async fn waive_emits_audit_and_unblocks() {
    // waive 带理由 → 审计 MemoryEvent tags 含 "gm_waive"（MockLlm 路径断言
    // 载荷，不连 DB）+ blocking() 清空放行。
    let tools =
        ToolRegistry::from_tools(vec![Box::new(crate::tools::mechanic::WaiveObligationTool)]);
    let scripts = vec![
        vec![
            StreamEvent::ToolCalls(vec![waive_call("due_audit")]),
            StreamEvent::Done {
                finish_reason: Some("tool_calls".to_string()),
            },
        ],
        vec![
            StreamEvent::ContentDelta("onward".to_string()),
            StreamEvent::Done {
                finish_reason: Some("stop".to_string()),
            },
        ],
    ];
    let (mut gm, llm, request, state) = loop_fixture(scripts, tools, 4);
    gm.obligations
        .absorb_dues(vec![seeded_due("due_audit", "chaos pool overflow")]);
    let mut out = String::new();
    let _ = gm
        .run_gm_turn(
            GmTurnInput {
                request: &request,
                state: &state,
                user_input: "go",
                history: &[],
                recent_transcript: None,
            },
            &mut |d| out.push_str(d),
        )
        .await
        .unwrap();
    assert_eq!(out, "onward");
    assert!(
        gm.obligations.blocking().is_empty(),
        "waive must unblock the ledger"
    );
    // 审计载荷（纯构造，不打 DB）：tags 必含 "gm_waive"。
    let record = crate::obligations::WaiverRecord {
        target_id: "due_audit".to_string(),
        reason: "handled in fiction".to_string(),
        scope: crate::obligations::WaiveScope::Turn,
        turn_id: "t".to_string(),
    };
    let event = crate::obligations::waiver_to_memory_event(&request, &record);
    assert!(
        event.tags.iter().any(|t| t == "gm_waive"),
        "audit tags missing gm_waive: {:?}",
        event.tags
    );
    // 工具结果回填同样携带审计 tags（agent 可见可审计）。
    let second_request = llm.requests.lock().unwrap()[1].clone();
    let tool_content = second_request
        .iter()
        .find(|m| m.get("role").and_then(Value::as_str) == Some("tool"))
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        tool_content.contains("gm_waive"),
        "audit tags missing from tool result: {tool_content}"
    );
}

#[tokio::test]
async fn round_exhaustion_carries_debt_to_next_turn() {
    // 轮耗尽仍有债务 → ToolChoice::None 强制叙事语义不变（一期断言照绿）+
    // 债务跨回合存活 + 下回合 assemble 的 dynamic tail 含 obligations_block。
    let scripts = vec![
        vec![
            StreamEvent::ContentDelta("blocked draft".to_string()),
            StreamEvent::Done {
                finish_reason: Some("stop".to_string()),
            },
        ],
        vec![
            StreamEvent::ContentDelta("forced ending".to_string()),
            StreamEvent::Done {
                finish_reason: Some("stop".to_string()),
            },
        ],
        vec![
            StreamEvent::ContentDelta("blocked again".to_string()),
            StreamEvent::Done {
                finish_reason: Some("stop".to_string()),
            },
        ],
        vec![
            StreamEvent::ContentDelta("forced again".to_string()),
            StreamEvent::Done {
                finish_reason: Some("stop".to_string()),
            },
        ],
    ];
    let (mut gm, llm, request, state) = loop_fixture(scripts, ToolRegistry::from_tools(vec![]), 1);
    gm.obligations
        .absorb_dues(vec![seeded_due("due_carry", "sanity threshold crossed")]);
    let mut out = String::new();
    let result = gm
        .run_gm_turn(
            GmTurnInput {
                request: &request,
                state: &state,
                user_input: "go",
                history: &[],
                recent_transcript: None,
            },
            &mut |d| out.push_str(d),
        )
        .await
        .unwrap();
    assert_eq!(
        llm.choices.lock().unwrap()[..2],
        vec![ToolChoice::Auto, ToolChoice::None]
    );
    assert_eq!(out, "forced ending");
    assert!(matches!(result, TurnOutcome::Narration(_)));
    // GmLoop.obligations 持久字段含 carryover。
    assert!(
        !gm.obligations.blocking().is_empty(),
        "debt must survive the turn"
    );
    assert!(gm.obligations.carryover_block().is_some());
    // 下回合 assemble 的 dynamic tail 含 obligations_block。
    let request2 = ContextRequest {
        turn_id: "t2".to_string(),
        ..request.clone()
    };
    let _ = gm
        .run_gm_turn(
            GmTurnInput {
                request: &request2,
                state: &state,
                user_input: "继续",
                history: &[],
                recent_transcript: None,
            },
            &mut |d| out.push_str(d),
        )
        .await
        .unwrap();
    let turn2_request = llm.requests.lock().unwrap()[2].clone();
    let tail = turn2_request
        .last()
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        tail.contains("[obligations_carryover]"),
        "tail missing obligations_block: {tail}"
    );
    assert!(tail.contains("sanity threshold crossed"));
}

#[tokio::test]
async fn invented_effect_becomes_retro_debt() {
    // spec 验收 11 单测侧：verify_after_stream 抓到 InventedEffect →
    // absorb_retro_debts → 下回合 blocking() 含该 debt_id。
    let scripts = vec![vec![
        StreamEvent::ContentDelta("你失去了 3 点理智。".to_string()),
        StreamEvent::Done {
            finish_reason: Some("stop".to_string()),
        },
    ]];
    let (mut gm, _llm, request, state) = loop_fixture(scripts, ToolRegistry::from_tools(vec![]), 2);
    let mut out = String::new();
    let _ = gm
        .run_gm_turn(
            GmTurnInput {
                request: &request,
                state: &state,
                user_input: "go",
                history: &[],
                recent_transcript: None,
            },
            &mut |d| out.push_str(d),
        )
        .await
        .unwrap();
    assert_eq!(gm.errata.kind_counts().get("invented_effect"), Some(&1));
    let blocking = gm.obligations.blocking();
    assert_eq!(
        blocking.len(),
        1,
        "exactly one retro debt expected: {blocking:?}"
    );
    assert_eq!(blocking[0].kind, "debt");
    assert!(
        blocking[0].target_id.starts_with("debt_"),
        "debt_id shape: {}",
        blocking[0].target_id
    );
}

#[tokio::test]
async fn recoverable_tool_error_lets_agent_switch_to_player_roll() {
    // spec §9 场景 7 + 2（终态侧）：missing_kernel_dice 结构化错误回填 →
    // agent 第二轮改调 request_player_roll → AwaitingPlayerRoll 终态。
    // （"下回合骰值回复 → 头部结算"需真 DB pending check，归 Task 11 e2e。）
    let tools = ToolRegistry::from_tools(vec![
        Box::new(ScriptedTool {
            name: "roll_check",
            record: false,
            fail_code: Some("missing_kernel_dice"),
            awaiting: false,
        }),
        Box::new(ScriptedTool {
            name: "request_player_roll",
            record: false,
            fail_code: None,
            awaiting: true,
        }),
    ]);
    let scripts = vec![
        vec![
            StreamEvent::ToolCalls(vec![tool_call("roll_check")]),
            StreamEvent::Done {
                finish_reason: Some("tool_calls".to_string()),
            },
        ],
        vec![
            StreamEvent::ToolCalls(vec![tool_call("request_player_roll")]),
            StreamEvent::Done {
                finish_reason: Some("tool_calls".to_string()),
            },
        ],
    ];
    let (mut gm, llm, request, state) = loop_fixture(scripts, tools, 4);
    let mut out = String::new();
    let result = gm
        .run_gm_turn(
            GmTurnInput {
                request: &request,
                state: &state,
                user_input: "go",
                history: &[],
                recent_transcript: None,
            },
            &mut |d| out.push_str(d),
        )
        .await
        .unwrap();
    assert_eq!(
        result,
        TurnOutcome::AwaitingPlayerRoll {
            check_id: "check_gate".to_string(),
            prompt_public: "Roll 1d100 now.".to_string()
        }
    );
    // 第二轮请求携带第一轮的结构化错误回填（agent 可见可修复）。
    let second_request = llm.requests.lock().unwrap()[1].clone();
    let error_content = second_request
        .iter()
        .find(|m| m.get("role").and_then(Value::as_str) == Some("tool"))
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(error_content.contains("missing_kernel_dice"));
    assert!(error_content.contains("\"recoverable\":true"));
}

#[tokio::test]
async fn phase_context_assembly_produces_nonempty_blocks() {
    // 纯重构护栏：context_assembly handler 经 ctx_provider seam + gm_skill
    // fixture 必产出非空 compiled + gm_skill + 组装好的 messages（mode=None
    // 退化两级，字节与旧头部一致）。
    let (mut gm, _llm, request, state) = loop_fixture(vec![], ToolRegistry::from_tools(vec![]), 1);
    let input = GmTurnInput {
        request: &request,
        state: &state,
        user_input: "I open the door.",
        history: &[],
        recent_transcript: None,
    };
    let mut ctx = TurnContext::new();
    // 头部前序 phase 是 context_assembly 的前置（debt_load 填 obligations_block、
    // mode_inference 填 mode_id/manifest）——按序跑到 context_assembly。
    gm.phase_record_player_action(&mut ctx, &input).await;
    gm.phase_refresh_live_derived(&mut ctx, &input).await;
    gm.phase_reconcile(&mut ctx, &input).await;
    gm.phase_gate(&mut ctx, &input).await;
    gm.phase_stimulus_pass(&mut ctx, &input).await;
    gm.phase_opposed_prepass(&mut ctx, &input).await;
    gm.phase_mode_inference(&mut ctx, &input).await.unwrap();
    gm.phase_debt_load(&mut ctx, &input).await;
    gm.phase_context_assembly(&mut ctx, &input).await.unwrap();
    assert_eq!(ctx.compiled.prefix_text, "BP1");
    assert!(
        !ctx.gm_skill.trim().is_empty(),
        "gm_skill must be loaded by context_assembly"
    );
    // messages 已组装：含 system/user 至少 2 条（assemble 不会空）。
    let messages = ctx
        .messages
        .as_ref()
        .expect("TurnMessages must be assembled");
    assert!(
        messages.to_request_messages().len() >= 2,
        "TurnMessages must be assembled"
    );
    assert_eq!(
        ctx.mode_id, None,
        "no active frame ⇒ no mode (byte-identical to phase-2)"
    );
}

#[tokio::test]
async fn phase_debt_load_initializes_obligations_no_panic() {
    // 纯重构护栏：debt_load handler begin_turn + 装载 dues（lazy pool 下 db
    // 调用 unwrap_or_default 兜底，绝不 panic），空账本 ⇒ obligations_block None。
    let (mut gm, _llm, request, state) = loop_fixture(vec![], ToolRegistry::from_tools(vec![]), 1);
    let input = GmTurnInput {
        request: &request,
        state: &state,
        user_input: "wait",
        history: &[],
        recent_transcript: None,
    };
    let mut ctx = TurnContext::new();
    gm.phase_mode_inference(&mut ctx, &input).await.unwrap();
    gm.phase_debt_load(&mut ctx, &input).await;
    assert_eq!(
        ctx.obligations_block, None,
        "empty ledger ⇒ no carryover block"
    );
}

// ============================ P1 Adjudicator/Narrator split（TRPG_NARRATOR_SPLIT）============================
//
// 这些测试直接驱动 `run_agent_loop`（stream_prose 参数）与 `run_narrator`，绕开进程级
// env flag（避免并行测试间 env 竞争）：env 解析本身在 execute.rs 已有 P1.4 接线，行为
// 等价于 `stream_prose = !narrator_split`。

use crate::turn_event::TurnEvent;
use tokio::sync::mpsc;

/// 跑头部确定性 phase（与 phase_context_assembly_produces_nonempty_blocks 同序）把
/// `ctx.messages` 组装好，让 run_agent_loop 可直接消费。input 引用 fixture 的 request/state。
async fn ready_ctx(gm: &mut GmLoop, input: &GmTurnInput<'_>) -> TurnContext {
    let mut ctx = TurnContext::new();
    gm.phase_record_player_action(&mut ctx, input).await;
    gm.phase_refresh_live_derived(&mut ctx, input).await;
    gm.phase_reconcile(&mut ctx, input).await;
    gm.phase_gate(&mut ctx, input).await;
    gm.phase_stimulus_pass(&mut ctx, input).await;
    gm.phase_opposed_prepass(&mut ctx, input).await;
    gm.phase_mode_inference(&mut ctx, input).await.unwrap();
    gm.phase_debt_load(&mut ctx, input).await;
    gm.phase_context_assembly(&mut ctx, input).await.unwrap();
    ctx
}

/// 排空 channel 收集 Delta payload（按到达序）。
fn drain_deltas(rx: &mut mpsc::Receiver<TurnEvent>) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        if let TurnEvent::Delta(d) = ev {
            out.push(d);
        }
    }
    out
}

// P1.7-1 OFF==baseline：stream_prose=true（= flag OFF 路径）逐块 Delta 直发，
// 拼接等于 adjudicator 散文全文。这是 execute.rs OFF 分支（!narrator_split=true）的
// 结构等价路径——OFF 不调用任何 Narrator，Delta 序列就是 adjudicator ContentDelta 序列。
#[tokio::test]
async fn off_path_streams_adjudicator_deltas_byte_equivalent_to_baseline() {
    let (mut gm, _llm, request, state) = loop_fixture(
        vec![vec![
            StreamEvent::ContentDelta("门吱呀打开，".to_string()),
            StreamEvent::ContentDelta("一股霉味扑面。".to_string()),
            StreamEvent::Done {
                finish_reason: Some("stop".to_string()),
            },
        ]],
        ToolRegistry::from_tools(vec![]),
        1,
    );
    let input = GmTurnInput {
        request: &request,
        state: &state,
        user_input: "我推开门",
        history: &[],
        recent_transcript: None,
    };
    let mut ctx = ready_ctx(&mut gm, &input).await;
    let (tx, mut rx) = mpsc::channel(64);
    let signal = gm
        .run_agent_loop(&mut ctx, &input, &tx, None, /* stream_prose */ true)
        .await;
    drop(tx);
    assert_eq!(signal, crate::execute::AgentSignal::Narration);
    let deltas = drain_deltas(&mut rx);
    // 逐块直发、不合并：与 baseline on_delta 字节序列一致。
    assert_eq!(
        deltas,
        vec!["门吱呀打开，".to_string(), "一股霉味扑面。".to_string()]
    );
    // 拼接 == ctx.visible_text（adjudicator 全文）== baseline assistant_output。
    assert_eq!(deltas.concat(), ctx.visible_text);
    assert_eq!(ctx.visible_text, "门吱呀打开，一股霉味扑面。");
}

// P1.7-2 ON buffer 模式：stream_prose=false（= flag ON 路径）下 adjudicator ContentDelta
// **不**经 tx 流出（玩家在 agent loop 期间收不到任何 Delta），但全文累积进 ctx.visible_text
// 供 Narrator 投影。
#[tokio::test]
async fn on_path_buffers_adjudicator_prose_without_streaming() {
    let (mut gm, _llm, request, state) = loop_fixture(
        vec![vec![
            StreamEvent::ContentDelta("GM 机械裁定：".to_string()),
            StreamEvent::ContentDelta("命中并造成 3 点伤害。".to_string()),
            StreamEvent::Done {
                finish_reason: Some("stop".to_string()),
            },
        ]],
        ToolRegistry::from_tools(vec![]),
        1,
    );
    let input = GmTurnInput {
        request: &request,
        state: &state,
        user_input: "我攻击",
        history: &[],
        recent_transcript: None,
    };
    let mut ctx = ready_ctx(&mut gm, &input).await;
    let (tx, mut rx) = mpsc::channel(64);
    let signal = gm
        .run_agent_loop(&mut ctx, &input, &tx, None, /* stream_prose */ false)
        .await;
    drop(tx);
    assert_eq!(signal, crate::execute::AgentSignal::Narration);
    let deltas = drain_deltas(&mut rx);
    assert!(
        deltas.is_empty(),
        "buffer 模式：agent loop 期间 adjudicator prose 绝不直发玩家，实得 {deltas:?}"
    );
    // 但全文 buffer 进 visible_text 供 Narrator 投影/fail-soft。
    assert_eq!(ctx.visible_text, "GM 机械裁定：命中并造成 3 点伤害。");
}

// P1.7-3 Narrator 无 mutation 工具：run_narrator 必须以 **空 tools vec + ToolChoice::None**
// 调 stream_chat_with_tools（物理上无法 mutate 账本）。用 MockLlm 捕获的 choices/requests 断言。
#[tokio::test]
async fn narrator_runs_with_empty_tools_and_tool_choice_none() {
    let (gm, llm, _request, _state) = loop_fixture(
        vec![vec![
            StreamEvent::ContentDelta("你挥剑斩下，".to_string()),
            StreamEvent::ContentDelta("敌人踉跄后退。".to_string()),
            StreamEvent::Done {
                finish_reason: Some("stop".to_string()),
            },
        ]],
        ToolRegistry::from_tools(vec![]),
        1,
    );
    let packet = crate::packet::NarrationPacket {
        player_input: "我攻击".to_string(),
        what_happened: vec!["check c1: 成功".to_string()],
        what_changed: vec!["effect e1 (damage)".to_string()],
        player_perceivable_facts: vec![],
        style_profile: "中性".to_string(),
        forbidden_reveals: vec![],
        ..Default::default()
    };
    let (tx, mut rx) = mpsc::channel(64);
    let out = gm.run_narrator(&packet, &[], &tx, None).await;
    drop(tx);
    assert_eq!(
        out.as_deref(),
        Some("你挥剑斩下，敌人踉跄后退。"),
        "Narrator 返回拼接后的玩家散文"
    );
    let deltas = drain_deltas(&mut rx);
    assert_eq!(deltas.concat(), "你挥剑斩下，敌人踉跄后退。");
    // 结构断言：Narrator 这次 LLM 调用 ToolChoice::None。
    let choices = llm.choices.lock().unwrap();
    assert_eq!(choices.len(), 1, "Narrator 恰好一次 LLM 调用");
    assert!(
        matches!(choices[0], ToolChoice::None),
        "Narrator 必须 ToolChoice::None（物理无 mutation 工具），实得 {:?}",
        choices[0]
    );
    // 空 tools schema 经 build_narrator_messages → stream_chat_with_tools(vec![])；
    // 同时 narrator 请求绝不携带 GM 内部裁定散文（player-safe 投影）。
    let reqs = llm.requests.lock().unwrap();
    let req_json = serde_json::to_string(&reqs[0]).unwrap();
    assert!(
        !req_json.contains("机械裁定") && !req_json.contains("adjudicator"),
        "Narrator 请求绝不含 adjudicator 内部散文"
    );
}

// P1.7-4 AwaitingPlayerRoll ON==OFF：桌面骰 gate 终态不经 Narrator——run_agent_loop 在
// stream_prose=false（ON）下命中 awaiting 时，行为与 stream_prose=true（OFF）一致：
// 发 AwaitingPlayerRoll 事件、返回 AwaitingPlayerRoll 信号。（execute.rs 仅在
// signal==Narration 时调 Narrator，故 awaiting 路径 ON/OFF 字节等价。）
#[tokio::test]
async fn awaiting_player_roll_identical_on_off() {
    async fn run(stream_prose: bool) -> (crate::execute::AgentSignal, Vec<TurnEvent>) {
        let gate_tool = ScriptedTool {
            name: "request_player_roll",
            record: false,
            fail_code: None,
            awaiting: true,
        };
        let (mut gm, _llm, request, state) = loop_fixture(
            vec![vec![
                StreamEvent::ToolCalls(vec![tool_call("request_player_roll")]),
                StreamEvent::Done {
                    finish_reason: Some("tool".to_string()),
                },
            ]],
            ToolRegistry::from_tools(vec![Box::new(gate_tool)]),
            1,
        );
        let input = GmTurnInput {
            request: &request,
            state: &state,
            user_input: "撬锁",
            history: &[],
            recent_transcript: None,
        };
        let mut ctx = ready_ctx(&mut gm, &input).await;
        let (tx, mut rx) = mpsc::channel(64);
        let signal = gm
            .run_agent_loop(&mut ctx, &input, &tx, None, stream_prose)
            .await;
        drop(tx);
        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        (signal, events)
    }
    let (sig_on, ev_on) = run(false).await; // ON (buffer)
    let (sig_off, ev_off) = run(true).await; // OFF (direct)
    assert_eq!(sig_on, crate::execute::AgentSignal::AwaitingPlayerRoll);
    assert_eq!(sig_off, crate::execute::AgentSignal::AwaitingPlayerRoll);
    // 两路都恰好发一个 AwaitingPlayerRoll，且 prompt_public 一致；无 Delta 泄漏。
    let extract = |evs: &[TurnEvent]| -> Vec<(String, String)> {
        evs.iter()
            .filter_map(|e| match e {
                TurnEvent::AwaitingPlayerRoll {
                    check_id,
                    prompt_public,
                } => Some((check_id.clone(), prompt_public.clone())),
                _ => None,
            })
            .collect()
    };
    assert_eq!(extract(&ev_on), extract(&ev_off));
    assert_eq!(
        extract(&ev_on),
        vec![("check_gate".to_string(), "Roll 1d100 now.".to_string())]
    );
}

// R5 Task1b 边界测试：编译级断言 save_turn-only 的 finalize 与 heavy memory 写
// 各有独立入口（finalize_save_turn = critical 只 save；heavy_finalize_memory =
// heavy turn 记忆 + audit；phase_finalize_heavy_memory = execute.rs heavy 段桥）。
// 方法路径引用强制编译期名解析——拆分被合回一体会断编译。不执行（lazy pool）。
#[test]
fn finalize_save_and_heavy_memory_are_separable() {
    // 引用三个方法路径强制编译期名解析（async 方法返回 impl Future，无法名 fn
    // 指针返回类型，故直接取方法值即可锁定签名稳定）。
    let _save = GmLoop::finalize_save_turn;
    let _heavy = GmLoop::heavy_finalize_memory;
    let _bridge = GmLoop::phase_finalize_heavy_memory;
    let _ = (_save, _heavy, _bridge);
}

#[test]
fn scene_transition_repair_replaces_stale_source_scene_text() {
    let transition = SceneTransitionInfo {
        from: "loc_courts_police".into(),
        to: "loc_corbitt_house_exterior".into(),
        reason: "player explicitly travels to the house".into(),
        from_title: Some("Higher Courts and Central Police Station".into()),
        to_title: Some("The Old Corbitt Place".into()),
        to_read_aloud: Some("The Corbitt House waits behind its old address.".into()),
        to_summary: Some("Exterior arrival and initial inspection frame room exploration.".into()),
        to_aliases: vec!["corbitt_house".into()],
    };
    let stale = "你站在警署外头。事情没有再往前动哪怕半步。";

    let repaired = scene_transition_visible_repair(stale, &transition);

    assert!(repaired.contains("The Old Corbitt Place"));
    assert!(repaired.contains("The Corbitt House waits behind its old address."));
    assert!(
        !repaired.contains("警署外头"),
        "stale source-scene narration must not survive as player-visible result"
    );
}

#[test]
fn scene_transition_repair_keeps_destination_acknowledged_text() {
    let transition = SceneTransitionInfo {
        from: "scene_a".into(),
        to: "scene_b".into(),
        reason: "player moves".into(),
        from_title: Some("Archive".into()),
        to_title: Some("The Old Corbitt Place".into()),
        to_read_aloud: Some("The Corbitt House waits behind its old address.".into()),
        to_summary: None,
        to_aliases: vec!["corbitt_house".into()],
    };
    let visible = "你抵达 The Old Corbitt Place，停在街对面观察门窗。";

    assert_eq!(
        scene_transition_visible_repair(visible, &transition),
        visible
    );
}

#[tokio::test]
async fn scene_transition_repair_augments_roll_only_committed_summary() {
    let _lock = TURN_LOOP_ENV_LOCK.lock().await;
    let _lang = clear_test_env_var("TRPG_OUTPUT_LANGUAGE");
    let transition = SceneTransitionInfo {
        from: "loc_corbitt_house_upper_floor".into(),
        to: "loc_corbitt_house_basement".into(),
        reason: "player descends to the basement".into(),
        from_title: Some("Corbitt House: Upper Floor".into()),
        to_title: Some("Corbitt House: The Basement".into()),
        to_read_aloud: Some(
            "The basement air is colder and wetter. Rough boards, old bins, and a dark under-space make the room feel less like storage than concealment."
                .into(),
        ),
        to_summary: Some("The basement contains storage spaces and concealed hazards.".into()),
        to_aliases: vec!["basement".into()],
    };
    let visible = "根据本回合已确认的结果：\n· [roll]检定[check_1] Cautious search while descending into the basement: 1d100=[47] 目标:≤55，结果:成功[/roll]";

    let repaired = scene_transition_visible_repair(visible, &transition);

    assert!(
        repaired.contains("[roll]") && repaired.contains("结果:成功"),
        "auditable roll must survive transition repair: {repaired}"
    );
    assert!(
        !repaired.contains("check_1") && !repaired.contains("检定["),
        "player-visible transition repair must hide internal check ids: {repaired}"
    );
    assert!(
        repaired.contains("Corbitt House: The Basement"),
        "{repaired}"
    );
    assert!(repaired.contains("Rough boards"), "{repaired}");
    assert!(
        repaired.contains("当前位置") || repaired.contains("新的位置"),
        "roll-only transition repair must add a player-visible position fact: {repaired}"
    );
}

#[tokio::test]
async fn scene_transition_repair_augments_machine_check_succeeds_summary() {
    let _lock = TURN_LOOP_ENV_LOCK.lock().await;
    let _lang = clear_test_env_var("TRPG_OUTPUT_LANGUAGE");
    let transition = SceneTransitionInfo {
        from: "loc_corbitt_house_ground_floor".into(),
        to: "loc_corbitt_house_basement".into(),
        reason: "player descends to the basement".into(),
        from_title: Some("Corbitt House: Ground Floor".into()),
        to_title: Some("Corbitt House: The Basement".into()),
        to_read_aloud: Some(
            "The basement air is colder and wetter. Rough boards, old bins, and a dark under-space make the room feel less like storage than concealment."
                .into(),
        ),
        to_summary: Some("The basement contains storage spaces and concealed hazards.".into()),
        to_aliases: vec!["basement".into(), "cellar".into()],
    };
    let visible = "根据本回合已确认的结果：\n\
· [roll]检定[check_677] Listening at the top of the basement stairs: 1d100=[34] 目标:≤40，结果:成功[/roll]\n\
· [roll]检定[check_1f7] Careful descent and search of the basement stairs and immediate cellar for hidden signs or threats: 1d100=[32] 目标:≤65，结果:strong_success[/roll]\n\
· The Listening at the top of the basement stairs check succeeds.\n\
· The Careful descent and search of the basement stairs and immediate cellar for hidden signs or threats check succeeds.";

    let repaired = scene_transition_visible_repair(visible, &transition);

    assert!(
        repaired.contains("[roll]") && repaired.contains("strong_success"),
        "auditable rolls must survive transition repair: {repaired}"
    );
    assert!(
        !repaired.contains("check_677")
            && !repaired.contains("check_1f7")
            && !repaired.contains("检定["),
        "player-visible transition repair must hide internal check ids: {repaired}"
    );
    assert!(
        repaired.contains("Corbitt House: The Basement"),
        "{repaired}"
    );
    assert!(repaired.contains("Rough boards"), "{repaired}");
    assert!(
        repaired.contains("当前位置") || repaired.contains("新的位置"),
        "machine-summary transition repair must add a player-visible position fact: {repaired}"
    );
}

#[test]
fn scene_transition_repair_does_not_surface_read_aloud_clues_after_failed_roll_only_summary() {
    let transition = SceneTransitionInfo {
        from: "intro".into(),
        to: "loc_hall_records".into(),
        reason: "player goes to the Hall of Records and searches the indexes".into(),
        from_title: Some("Introduction".into()),
        to_title: Some("Hall of Records".into()),
        to_read_aloud: Some(
            "At the Hall of Records, bound registers and civil files sit behind clerks and indexes. The name Corbitt leads toward an executor, a church, and a closure date that does not feel ordinary."
                .into(),
        ),
        to_summary: Some(
            "Civil records reveal Corbitt's executor and a church closure date.".into(),
        ),
        to_aliases: vec!["hall_records".into()],
    };
    let visible = "根据本回合已确认的结果：\n\
· [roll]检定[check_hall] Library Use at Hall of Records: 1d100=[99] 目标:≤55，结果:strong_failure[/roll]";

    let repaired = scene_transition_visible_repair(visible, &transition);

    assert!(
        repaired.contains("[roll]") && repaired.contains("strong_failure"),
        "failed roll must stay auditable: {repaired}"
    );
    assert!(
        repaired.contains("Hall of Records"),
        "transition target still needs to be acknowledged: {repaired}"
    );
    assert!(
        !repaired.contains("executor")
            && !repaired.contains("church")
            && !repaired.contains("closure date")
            && !repaired.contains("Civil records reveal"),
        "failed roll-only transition repair must not convert scene clue text into confirmed facts: {repaired}"
    );
}

#[test]
fn scene_transition_repair_generates_arrival_when_visible_text_is_empty() {
    let transition = SceneTransitionInfo {
        from: "loc_hall_records".into(),
        to: "loc_courts_police".into(),
        reason: "player explicitly goes to the courts and police records desk".into(),
        from_title: Some("Hall of Records".into()),
        to_title: Some("Higher Courts and Central Police Station".into()),
        to_read_aloud: Some(
            "The higher courts and police records office replace the dry hush of the Hall with clerks, ledgers, and restricted indexes."
                .into(),
        ),
        to_summary: Some("Public court and police record research scene.".into()),
        to_aliases: vec!["courts".into(), "police".into()],
    };

    let repaired = scene_transition_visible_repair("", &transition);

    assert!(
        repaired.contains("Higher Courts and Central Police Station"),
        "{repaired}"
    );
    assert!(
        repaired.contains("clerks") && repaired.contains("restricted indexes"),
        "{repaired}"
    );
    assert!(
        !repaired.trim().is_empty(),
        "empty transition narration must be repaired"
    );
}

#[test]
fn scene_transition_repair_does_not_replace_active_transition_text_without_stall_marker() {
    let transition = SceneTransitionInfo {
        from: "intro".into(),
        to: "loc_globe".into(),
        reason: "player goes to the archive".into(),
        from_title: Some("Introduction".into()),
        to_title: Some("The Boston Globe".into()),
        to_read_aloud: Some("The Globe offices smell of ink.".into()),
        to_summary: None,
        to_aliases: vec!["the_boston_globe".into()],
    };
    let visible = "你进了报社资料室，开始翻索引和剪报柜，寻找 Macario 与暴力事件的交叉记录。";

    assert_eq!(
        scene_transition_visible_repair(visible, &transition),
        visible,
        "a transition turn that is actively resolving the player's action must not be reduced to arrival prose"
    );
}

#[tokio::test]
async fn scene_transition_repair_respects_chinese_output_language_for_roll_only_fallback() {
    let _lock = TURN_LOOP_ENV_LOCK.lock().await;
    let _lang = set_test_env_var("TRPG_OUTPUT_LANGUAGE", "zh-Hans");
    let transition = SceneTransitionInfo {
        from: "loc_corbitt_house_exterior".into(),
        to: "loc_corbitt_house_ground_floor".into(),
        reason: "player opens the front door and checks the threshold".into(),
        from_title: Some("The Old Corbitt Place".into()),
        to_title: Some("Corbitt House: Ground Floor".into()),
        to_read_aloud: Some(
            "Inside, the house smells of old plaster, dust, and trapped damp.".into(),
        ),
        to_summary: Some("The ground floor is stale and dark.".into()),
        to_aliases: vec!["ground floor".into()],
    };
    let visible = "根据本回合已确认的结果：\n· [roll]检定[check_c6628] 站在门槛处以手电检查前厅地面、墙角、楼梯与内门痕迹: 1d100=[40] 目标:≤55，结果:strong_success[/roll]";

    let repaired = scene_transition_visible_repair(visible, &transition);

    assert!(repaired.contains("[roll]"), "{repaired}");
    assert!(repaired.contains("strong_success"), "{repaired}");
    assert!(!repaired.contains("Inside, the house smells"), "{repaired}");
    assert!(!repaired.contains("下一步应"), "{repaired}");
    assert!(!repaired.contains("The Old Corbitt Place"), "{repaired}");
    assert!(
        !repaired.contains("Corbitt House: Ground Floor"),
        "{repaired}"
    );
    assert!(!repaired.contains("check_c6628"), "{repaired}");
    assert!(!repaired.contains("检定["), "{repaired}");
    assert!(
        repaired.contains("新的位置") || repaired.contains("你现在的位置"),
        "{repaired}"
    );
}

// ======================== P6.7 reveal_fact gating + 两 commit 边界 ========================

use crate::presentation_gate::PresentationGate;
use crate::tools::RevealNomination;
use trpg_agent::{VerifierFinding, VerifierFindingKind, VerifierSeverity};

/// DB-gated 真库装配（DATABASE_URL 缺失 ⇒ None ⇒ 调用方 SKIP，绝不伪 PASS）。
async fn real_gm(session: &str) -> Option<(GmLoop, ContextRequest)> {
    real_gm_with_llm(session)
        .await
        .map(|(gm, _llm, request)| (gm, request))
}

async fn real_gm_with_llm(session: &str) -> Option<(GmLoop, Arc<MockLlm>, ContextRequest)> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = trpg_db::Db::connect(&url).await.ok()?;
    db.migrate().await.ok()?;
    let engine = RuntimeEngine::new(db);
    let llm = Arc::new(MockLlm {
        scripts: Mutex::new(Vec::new()),
        choices: Mutex::new(Vec::new()),
        requests: Mutex::new(Vec::new()),
        text_responses: Mutex::new(Vec::new()),
        text_requests: Mutex::new(Vec::new()),
        json_responses: Mutex::new(Vec::new()),
        json_requests: Mutex::new(Vec::new()),
    });
    let data_dir = std::env::temp_dir().join(format!(
        "gm_reveal_test_{}_{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    let gm = GmLoop::new(
        engine,
        llm.clone(),
        ToolRegistry::standard(),
        LoopConfig::default(),
        data_dir,
    );
    let request = ContextRequest {
        ruleset_id: "rs".to_string(),
        module_id: None,
        session_id: session.to_string(),
        turn_id: "t_reveal".to_string(),
        viewer: VisibilityProfile::gm(),
        token_budget: TokenBudget::default(),
    };
    Some((gm, llm, request))
}

fn block_gate() -> PresentationGate {
    PresentationGate::Block(vec![VerifierFinding {
        kind: VerifierFindingKind::SecretLeak,
        severity: VerifierSeverity::Blocker,
        detail: "leak".into(),
    }])
}

/// OFF==baseline：gating flag 未设 ⇒ reveal_fact 工具仍即时落库（F13 字节级基线）。
/// 经真 ToolRegistry::standard() dispatch（nominated_reveals 通道未挂）。
#[tokio::test]
async fn reveal_gating_off_is_immediate_baseline() {
    std::env::remove_var("TRPG_REVEAL_GATING");
    let session = format!("s_off_{}", uuid::Uuid::new_v4().simple());
    let Some((gm, request)) = real_gm(&session).await else {
        eprintln!("SKIP: DATABASE_URL unset");
        return;
    };
    let state = RuntimeState::default();
    let ctx = ToolCtx {
        engine: &gm.engine,
        request: &request,
        state: &state,
        scene_extractor: None,
        obligations: None,
        data_dir: None,
        current_mode: None,
        opposed_binding: None,
        nominated_reveals: None, // OFF / 未挂 ⇒ 即时落库
        rejected_nominations: None,
    };
    let registry = ToolRegistry::standard();
    let mut ledger = TurnLedger::new();
    let out = registry
        .dispatch(
            &ctx,
            &mut ledger,
            &AggregatedToolCall {
                id: "c1".into(),
                name: "reveal_fact".into(),
                arguments: json!({"fact_id":"npc_butler","reason":"r"}).to_string(),
            },
        )
        .await;
    let v: Value = serde_json::from_str(&out.content).unwrap();
    assert_eq!(v.pointer("/revealed").and_then(Value::as_bool), Some(true));
    // 行已即时落库（与今日基线逐字节等价）。
    assert_eq!(
        gm.engine.db.list_revealed_facts(&session).await.unwrap(),
        vec!["npc_butler".to_string()]
    );
}

/// ON gating：reveal_fact 工具仅提名、本轮不写 DB（PlayerLearnedFact 行尚未存在）。
#[tokio::test]
async fn reveal_gating_on_nominates_without_immediate_write() {
    let session = format!("s_on_nom_{}", uuid::Uuid::new_v4().simple());
    let Some((gm, request)) = real_gm(&session).await else {
        eprintln!("SKIP: DATABASE_URL unset");
        return;
    };
    let state = RuntimeState::default();
    let cell: Mutex<Vec<RevealNomination>> = Mutex::new(Vec::new());
    let ctx = ToolCtx {
        engine: &gm.engine,
        request: &request,
        state: &state,
        scene_extractor: None,
        obligations: None,
        data_dir: None,
        current_mode: None,
        opposed_binding: None,
        nominated_reveals: Some(&cell), // ON ⇒ 提名通道在位
        rejected_nominations: None,
    };
    let registry = ToolRegistry::standard();
    let mut ledger = TurnLedger::new();
    let out = registry
        .dispatch(
            &ctx,
            &mut ledger,
            &AggregatedToolCall {
                id: "c1".into(),
                name: "reveal_fact".into(),
                arguments: json!({"fact_id":"npc_butler"}).to_string(),
            },
        )
        .await;
    let v: Value = serde_json::from_str(&out.content).unwrap();
    assert_eq!(v.pointer("/nominated").and_then(Value::as_bool), Some(true));
    assert_eq!(v.pointer("/revealed").and_then(Value::as_bool), Some(false));
    // 提名进 cell，但 DB 未写。
    assert_eq!(cell.lock().unwrap().len(), 1);
    assert!(gm
        .engine
        .db
        .list_revealed_facts(&session)
        .await
        .unwrap()
        .is_empty());
}

/// PresentationCommit + 终审 Block ⇒ 提名全部丢弃，零 PlayerLearnedFact 行。
#[tokio::test]
async fn presentation_commit_block_drops_nominations() {
    let session = format!("s_block_{}", uuid::Uuid::new_v4().simple());
    let Some((mut gm, request)) = real_gm(&session).await else {
        eprintln!("SKIP: DATABASE_URL unset");
        return;
    };
    let mut ctx = TurnContext::new();
    ctx.nominated_reveals = vec![
        RevealNomination {
            fact_id: "npc_butler".into(),
            reason: None,
        },
        RevealNomination {
            fact_id: "sc_cellar".into(),
            reason: None,
        },
    ];
    ctx.presentation_gate = block_gate(); // 终审 Block
    gm.presentation_commit_boundary(&mut ctx, &request).await;
    assert!(
        gm.engine
            .db
            .list_revealed_facts(&session)
            .await
            .unwrap()
            .is_empty(),
        "Block ⇒ 提名不提交，零行"
    );
}

/// PresentationCommit + 终审 Allow ⇒ 按 fact_id 排序逐条提交（replay parity）。
#[tokio::test]
async fn presentation_commit_allow_commits_sorted() {
    let session = format!("s_allow_{}", uuid::Uuid::new_v4().simple());
    let Some((mut gm, request)) = real_gm(&session).await else {
        eprintln!("SKIP: DATABASE_URL unset");
        return;
    };
    let mut ctx = TurnContext::new();
    // 故意乱序提名，断言提交后按 fact_id 排序。
    ctx.nominated_reveals = vec![
        RevealNomination {
            fact_id: "sc_cellar".into(),
            reason: Some("found".into()),
        },
        RevealNomination {
            fact_id: "npc_butler".into(),
            reason: None,
        },
    ];
    ctx.presentation_gate = PresentationGate::Allow; // 终审 Allow
    gm.presentation_commit_boundary(&mut ctx, &request).await;
    let mut got = gm.engine.db.list_revealed_facts(&session).await.unwrap();
    got.sort();
    assert_eq!(got, vec!["npc_butler".to_string(), "sc_cellar".to_string()]);
}

/// Block-then-Allow（codex#6）：repair ladder 把终审从 Block 修复成 Allow ⇒ 必须提交。
/// 这里直接以「commit 边界在 repair 之后跑、读最终 gate」建模：把 gate 设为修复后的 Allow。
#[tokio::test]
async fn commit_after_repair_block_then_allow_commits() {
    let session = format!("s_repair_{}", uuid::Uuid::new_v4().simple());
    let Some((gm, request)) = real_gm(&session).await else {
        eprintln!("SKIP: DATABASE_URL unset");
        return;
    };
    // 先 Block（不提交），后 Allow（提交）——两次调用 commit primitive 模拟终审单点。
    let noms = vec![RevealNomination {
        fact_id: "npc_butler".into(),
        reason: None,
    }];
    gm.commit_nominated_reveals(&request, noms.clone(), false)
        .await; // 初判 Block
    assert!(
        gm.engine
            .db
            .list_revealed_facts(&session)
            .await
            .unwrap()
            .is_empty(),
        "first Block ⇒ 未提交"
    );
    gm.commit_nominated_reveals(&request, noms, true).await; // 修复后 Allow
    assert_eq!(
        gm.engine.db.list_revealed_facts(&session).await.unwrap(),
        vec!["npc_butler".to_string()],
        "repair→Allow ⇒ 提交"
    );
}

/// 钩子重定位结构断言（不依赖 DB）：BeforeNarration 已从 run_narrator_phase（split-only）
/// 移到 presentation_commit_boundary；BeforeCommit 已从 phase_finalize（save_turn 前）
/// 移到 resolution_commit_boundary。grep 生产源（对 rustfmt 自拆鲁棒：只看方法体含钩子名）。
#[test]
fn p37_hooks_relocated_to_commit_boundaries() {
    let src = include_str!("turn_loop.rs");
    fn body_between<'a>(src: &'a str, start: &str, next: &str) -> &'a str {
        let s = src.find(start).unwrap_or_else(|| panic!("missing {start}"));
        let rest = &src[s..];
        let e = rest.find(next).unwrap_or(rest.len());
        &rest[..e]
    }
    // BeforeNarration 出现在 presentation_commit_boundary 体内。
    let pcb = body_between(
        src,
        "async fn presentation_commit_boundary",
        "async fn commit_nominated_reveals",
    );
    assert!(
        pcb.contains("PluginHook::BeforeNarration"),
        "BeforeNarration 必须在 presentation_commit_boundary 触发"
    );
    // BeforeCommit 出现在 resolution_commit_boundary 体内。
    let rcb = body_between(
        src,
        "async fn resolution_commit_boundary",
        "async fn presentation_commit_boundary",
    );
    assert!(
        rcb.contains("PluginHook::BeforeCommit"),
        "BeforeCommit 必须在 resolution_commit_boundary 触发"
    );
    // 旧站点不再触发：run_narrator_phase 体内无 BeforeNarration 触发；phase_finalize 体内无 BeforeCommit。
    let narrator = body_between(src, "async fn run_narrator_phase", "async fn run_narrator");
    assert!(
        !narrator.contains("PluginHook::BeforeNarration"),
        "run_narrator_phase 不应再触发 BeforeNarration（已重定位）"
    );
    let finalize = body_between(
        src,
        "async fn phase_finalize",
        "async fn phase_finalize_heavy",
    );
    assert!(
        !finalize.contains("PluginHook::BeforeCommit"),
        "phase_finalize 不应再触发 BeforeCommit（已重定位）"
    );
}

/// Q-7-REVISED 结构断言：run_narrator_phase 在 gm_craft ON 时 **classify-only**，绝不再把
/// 解析后的 player_text 回写 ctx.visible_text（API 一律发原始全标记，strip 是 UI 层后事）。
#[test]
fn q7_narrator_phase_emits_raw_no_strip_overwrite() {
    let src = include_str!("turn_loop.rs");
    fn body_between<'a>(src: &'a str, start: &str, next: &str) -> &'a str {
        let s = src.find(start).unwrap_or_else(|| panic!("missing {start}"));
        let rest = &src[s..];
        let e = rest.find(next).unwrap_or(rest.len());
        &rest[..e]
    }
    let narrator = body_between(src, "async fn run_narrator_phase", "async fn run_narrator(");
    // 仍调用 strip_player_markup 做 audience 分类（trace / 下游战报打标用）。
    assert!(
        narrator.contains("strip_player_markup"),
        "应仍解析分类（classify-only）"
    );
    // 但绝不再 `ctx.visible_text = ...player_text`（v2 的 strip 回写已删）。
    assert!(
        !narrator.contains("ctx.visible_text = stripped.player_text")
            && !narrator.contains("ctx.visible_text = classified.player_text"),
        "Q-7-REVISED：run_narrator_phase 不得再用 player_text 覆盖 visible_text（须发原始全标记）"
    );
}

/// Q-3-REINFORCE 结构断言：修复阶梯终端在 gm_craft ON 时走 **强制重述真叙事**，
/// 而把 deterministic_committed_facts_narration（含「（机械结果）」占位）留作 OFF 基线兜底。
#[test]
fn q3_repair_ladder_forces_real_renarration_under_craft() {
    let src = include_str!("turn_loop.rs");
    fn body_between<'a>(src: &'a str, start: &str, next: &str) -> &'a str {
        let s = src.find(start).unwrap_or_else(|| panic!("missing {start}"));
        let rest = &src[s..];
        let e = rest.find(next).unwrap_or(rest.len());
        &rest[..e]
    }
    let ladder = body_between(
        src,
        "async fn run_presentation_repair_ladder",
        "async fn resolution_commit_boundary",
    );
    // gm_craft 分支存在，且在终端做一次强制 narrate（accept 非空非回显）。
    assert!(
        ladder.contains("crate::gm_craft::enabled()") && ladder.contains("强制叙事"),
        "Q-3-REINFORCE：终端须在 gm_craft ON 时强制真叙事重述"
    );
    // 仍保留 deterministic 模板作为 OFF 基线兜底（字节等价）。
    assert!(
        ladder.contains("deterministic_committed_facts_narration"),
        "OFF 基线仍走 deterministic 模板（byte-equal）"
    );
}

#[test]
fn q3_repair_ladder_rechecks_craft_terminal_text_before_delivery() {
    let src = include_str!("turn_loop.rs");
    fn body_between<'a>(src: &'a str, start: &str, next: &str) -> &'a str {
        let s = src.find(start).unwrap_or_else(|| panic!("missing {start}"));
        let rest = &src[s..];
        let e = rest.find(next).unwrap_or(rest.len());
        &rest[..e]
    }
    let ladder = body_between(
        src,
        "async fn run_presentation_repair_ladder",
        "async fn resolution_commit_boundary",
    );

    assert!(
        ladder.contains("terminal_recheck")
            && ladder.contains("verify_after_stream")
            && ladder.contains("&ctx.visible_text")
            && ladder.contains("ctx.presentation_gate = terminal_recheck.gate"),
        "gm_craft terminal repair must recheck the accepted text and update the gate; otherwise \
         delivery still treats a repaired turn as Block and emits safe fallback instead"
    );
}

#[test]
fn q4_buffered_delivery_runs_manifest_repair_without_gate_block() {
    let src = include_str!("turn_loop.rs");
    fn body_between<'a>(src: &'a str, start: &str, next: &str) -> &'a str {
        let s = src.find(start).unwrap_or_else(|| panic!("missing {start}"));
        let rest = &src[s..];
        let e = rest.find(next).unwrap_or(rest.len());
        &rest[..e]
    }
    let phase = body_between(
        src,
        "pub(crate) async fn phase_verify_after_stream",
        "async fn run_presentation_repair_ladder",
    );

    assert!(
        phase.contains("deterministic_manifest_list_prose_repair(&ctx.visible_text)"),
        "Q4 field/manifest dumps can leak without a gate Block; buffered delivery must run the \
         deterministic manifest prose repair on final visible text"
    );
}

#[test]
fn deterministic_manifest_list_repair_turns_clue_dump_into_prose() {
    let original = "你已经确认：\n\
- 无人机的扫射更像是在压制警车，而不是追着你打。\n\
- 仓库侧门附近有一段低垂线缆，线缆末端没入墙缝。\n\
- 两名警员分别躲在车尾和路灯基座后，火力已经断续。\n";

    let repaired = deterministic_manifest_list_prose_repair(original)
        .expect("manifest clue dump should be repairable");

    assert!(repaired.contains("无人机的扫射"));
    assert!(repaired.contains("仓库侧门"));
    assert!(repaired.contains("两名警员"));
    assert!(
        !repaired
            .lines()
            .any(|line| line.trim_start().starts_with("- ")),
        "repair must not preserve markdown bullet list shape: {repaired}"
    );
    assert!(
        !repaired.contains("选择其一") && !repaired.contains("你现在可以立刻"),
        "repair must not become an explicit action menu: {repaired}"
    );
}

#[test]
fn deterministic_manifest_list_repair_cleans_intro_colon() {
    let original = "从这个角度看过去，你能确认几件事：\n\
- 仓库外墙有几处旧维护盖板似的轮廓。\n\
- 无人机背部有不自然的接口结构。\n";

    let repaired = deterministic_manifest_list_prose_repair(original)
        .expect("manifest clue dump with colon intro should be repairable");

    assert!(
        !repaired.contains("：。") && !repaired.contains(":."),
        "repair must not leave dangling colon punctuation: {repaired}"
    );
    assert!(repaired.contains("旧维护盖板"));
    assert!(repaired.contains("接口结构"));
}

#[test]
fn deterministic_manifest_list_repair_preserves_english_language_style() {
    let original = "Still, the place yields a few useful impressions:\n\
- the chapel's history of violence and neglect feels real in the bones of the building.\n\
- some portions of the ruin look more disturbed than others.\n\
- there are surviving storage areas worth a more concentrated examination.\n\
\n\
You photograph what seems distinctive before touching it.";

    let repaired = deterministic_manifest_list_prose_repair(original)
        .expect("English manifest clue dump should be repairable");

    assert!(
        !repaired.contains("You connect these observations"),
        "English repair must not preserve manifest heading: {repaired}"
    );
    assert!(
        !repaired.contains("Taken together"),
        "English repair must not replace one manifest heading with another: {repaired}"
    );
    assert!(!repaired.contains("你把这些观察串在一起"));
    assert!(
        !repaired.contains('。') && !repaired.contains('；') && !repaired.contains(",;"),
        "English repair must not introduce Chinese punctuation: {repaired}"
    );
    assert!(repaired.contains("the chapel's history"));
    assert!(repaired.contains("You photograph what seems distinctive"));
}

#[test]
fn deterministic_manifest_list_repair_rewrites_inline_english_observation_manifest() {
    let original =
        "From the threshold and first step inside, several practical facts become clear at once. \
You connect these observations: the front hall runs inward rather than opening broadly; \
the staircase rises farther ahead in the dimness; \
the door remains open behind you with daylight still marking the exit.";

    let repaired = deterministic_manifest_list_prose_repair(original)
        .expect("inline English observation manifest should be repairable");

    assert!(repaired.contains("the front hall runs inward"));
    assert!(repaired.contains("the staircase rises farther ahead"));
    assert!(repaired.contains("the door remains open behind you"));
    assert!(
        !repaired.contains("You connect these observations"),
        "repair should remove the manifest heading that presentation gate blocks: {repaired}"
    );
}

#[test]
fn deterministic_manifest_list_repair_rewrites_inline_field_manifest_dump() {
    let original = "From where Evelyn now stands, the concrete affordance is the landing. \
You connect these observations: Location: at the top of the stairs; \
Looks like: a small upper landing with bedroom openings; \
Sounds like: almost nothing beyond her own movements; \
Smells like: shut-in old wood and dust; \
Reachability: safely reachable from her current position.";

    let repaired = deterministic_manifest_list_prose_repair(original)
        .expect("inline field manifest dump should be repairable");

    assert!(repaired.contains("top of the stairs"));
    assert!(repaired.contains("small upper landing"));
    assert!(repaired.contains("safely reachable"));
    assert!(
        !repaired.contains("You connect these observations"),
        "repair should remove manifest heading: {repaired}"
    );
    assert!(
        !repaired.contains("Location:")
            && !repaired.contains("Looks like:")
            && !repaired.contains("Sounds like:")
            && !repaired.contains("Smells like:")
            && !repaired.contains("Reachability:"),
        "repair should not preserve field-label dump syntax: {repaired}"
    );
}

#[test]
fn deterministic_manifest_list_repair_rewrites_taken_together_field_manifest_dump() {
    let original = "The front door is the present point of entry. \
Taken together, Location: the main front entrance; \
Sight: an old door with a matching lock; \
Sound: no distinct sound from inside; \
Smell: stale shut-in air; \
Reachability: safely reachable from her current position.";

    let repaired = deterministic_manifest_list_prose_repair(original)
        .expect("taken-together field manifest dump should be repairable");

    assert!(repaired.contains("main front entrance"));
    assert!(repaired.contains("old door"));
    assert!(repaired.contains("safely reachable"));
    assert!(
        !repaired.contains("Taken together")
            && !repaired.contains("Location:")
            && !repaired.contains("Sight:")
            && !repaired.contains("Sound:")
            && !repaired.contains("Smell:")
            && !repaired.contains("Reachability:"),
        "repair must remove manifest heading and field labels: {repaired}"
    );
}

#[test]
fn deterministic_manifest_list_repair_strips_english_item_trailing_commas() {
    let original = "What emerges is ugly and concrete:\n\
- the 1912 raid,\n\
- missing children,\n\
- a suppressed scandal,\n";

    let repaired = deterministic_manifest_list_prose_repair(original)
        .expect("English comma-ended manifest should be repairable");

    assert!(repaired.contains("the 1912 raid"));
    assert!(repaired.contains("missing children"));
    assert!(repaired.contains("a suppressed scandal"));
    assert!(
        !repaired.contains(",;") && !repaired.contains("Taken together"),
        "comma-ended items must not become comma-semicolon prose or manifest prose: {repaired}"
    );
}

#[test]
fn committed_public_check_without_auditable_roll_needs_repair() {
    let public = pa_check_result("check_public", true);

    assert!(
        committed_player_visible_check_missing_roll(
            &[public.clone()],
            "The Hall of Records smells of dust, paste, and old paper."
        ),
        "a committed public check must not disappear from player-visible narration"
    );
    assert!(
        !committed_player_visible_check_missing_roll(
            &[public.clone()],
            "[roll]Library Use — 1d100: 42 vs target 70 — Success.[/roll]\nYou find the executor record."
        ),
        "an auditable roll block satisfies the visible check projection"
    );
    let second_public = pa_check_result("check_second", false);
    assert!(
        committed_player_visible_check_missing_roll(
            &[public.clone(), second_public],
            "Listen at the doorway: 1d100 = 98 vs Listen 45 — Fumble.\n\
             [roll]Spot Hidden — 1d100: 5 vs target 95 — Extreme success.[/roll]"
        ),
        "each committed public check needs its own auditable roll block"
    );

    let mut private = public;
    private.roll.visibility = RollVisibility::PrivateGmRoll;
    assert!(
        !committed_player_visible_check_missing_roll(&[private], "The shadows shift."),
        "private GM rolls must not force public roll projection"
    );
}

#[test]
fn deterministic_committed_facts_narration_wraps_check_facts_as_roll_blocks() {
    let packet = crate::packet::NarrationPacket {
        what_happened: vec![
            "检定[check_public] Library Use: 1d100=42 目标:≤70，结果:成功".to_string(),
        ],
        ..Default::default()
    };

    let rendered = deterministic_committed_facts_narration(&packet);

    assert!(rendered.contains("[roll]"), "{rendered}");
    assert!(rendered.contains("[/roll]"), "{rendered}");
    assert!(
        !committed_player_visible_check_missing_roll(
            &[pa_check_result("check_public", true)],
            &rendered
        ),
        "deterministic fallback must satisfy the same J4 roll projection check: {rendered}"
    );
}

#[test]
fn deterministic_committed_facts_narration_hides_internal_check_ids() {
    let packet = crate::packet::NarrationPacket {
        what_happened: vec![
            "检定[check_c6628c42f1844288813a8aa5e0f9e8ec] 站在门槛处以手电检查前厅地面、墙角、楼梯与内门痕迹: 1d100=[40] 目标:≤55，结果:strong_success"
                .to_string(),
        ],
        ..Default::default()
    };

    let rendered = deterministic_committed_facts_narration(&packet);

    assert!(rendered.contains("[roll]"), "{rendered}");
    assert!(rendered.contains("站在门槛处"), "{rendered}");
    assert!(!rendered.contains("check_c6628"), "{rendered}");
    assert!(!rendered.contains("检定["), "{rendered}");
}

#[test]
fn deterministic_committed_facts_narration_filters_check_status_bookkeeping() {
    let packet = crate::packet::NarrationPacket {
        what_happened: vec![
            "检定[check_public] 向诺特追问可核实地址、钥匙用途与可公开查证的房屋背景: 1d100=[36] 目标:≤40，结果:成功"
                .to_string(),
        ],
        player_perceivable_facts: vec![
            "The 向诺特追问可核实地址、钥匙用途与可公开查证的房屋背景 check succeeds."
                .to_string(),
        ],
        ..Default::default()
    };

    let rendered = deterministic_committed_facts_narration(&packet);

    assert!(rendered.contains("[roll]"), "{rendered}");
    assert!(!rendered.contains("check succeeds"), "{rendered}");
    assert!(!rendered.contains("The 向诺特"), "{rendered}");
}

#[test]
fn deterministic_committed_facts_narration_does_not_wrap_unbound_pending_checks_as_rolls() {
    let packet = crate::packet::NarrationPacket {
        what_happened: vec![
            "检定[check_pending] Methodical ground-floor sweep（待结算，尚未绑定真实检定，勿当作已掷骰检定呈现）"
                .to_string(),
        ],
        ..Default::default()
    };

    let rendered = deterministic_committed_facts_narration(&packet);

    assert!(
        !rendered.contains("[roll]"),
        "unbound pending checks are not committed roll facts: {rendered}"
    );
    assert!(
        !rendered.contains("待结算") && !rendered.contains("尚未绑定真实检定"),
        "pending check debt must not leak in player-visible fallback: {rendered}"
    );
    assert!(
        rendered.contains("无新增可公开的机械事实"),
        "fallback should be neutral when only pending checks exist: {rendered}"
    );
}

#[test]
fn unwrap_nonauditable_roll_blocks_keeps_prose_but_removes_fake_roll_tag() {
    let original = "[roll]Your withdrawal holds: you break contact cleanly and reach a stable position.[/roll]\n\
[roll]Spot Hidden 1d100=41 vs 70 success[/roll]";

    let repaired = unwrap_nonauditable_roll_blocks(original).expect("fake roll should be repaired");

    assert!(repaired.contains("Your withdrawal holds"));
    assert!(
        !repaired.contains("[roll]Your withdrawal holds"),
        "pure prose must not remain wrapped as a roll: {repaired}"
    );
    assert!(
        repaired.contains("[roll]Spot Hidden 1d100=41 vs 70 success[/roll]"),
        "auditable roll blocks must be preserved: {repaired}"
    );
}

#[test]
fn deterministic_committed_facts_narration_surfaces_threshold_entry_from_player_input() {
    let packet = crate::packet::NarrationPacket {
        player_input: "Evelyn unlocks the safest exterior door, opens it from the side, steps just inside with the flashlight low, and maps the entry hall, visible rooms, stairs, exits, odors, footprints, drafts, and recently disturbed objects while keeping the door open behind her.".to_string(),
        scene_context: vec![
            "The key fits and will work there. The door is not yet opened. The stale quiet presses through the wood."
                .to_string(),
        ],
        ..Default::default()
    };

    let rendered = deterministic_committed_facts_narration(&packet);

    assert!(
        !rendered.contains("无新增可公开"),
        "threshold entry fallback must not be an empty no-facts placeholder: {rendered}"
    );
    assert!(rendered.contains("门槛"), "{rendered}");
    assert!(rendered.contains("门厅"), "{rendered}");
    assert!(rendered.contains("退路"), "{rendered}");
    assert!(
        !rendered.contains("打开") && !rendered.contains("推进"),
        "presentation fallback should not assert door activation or movement effects without ledger evidence: {rendered}"
    );
    assert!(
        rendered.contains("还没有被确认") || rendered.contains("尚未确认"),
        "fallback must keep uncertain interior details unconfirmed: {rendered}"
    );
}

#[test]
fn deterministic_committed_facts_narration_surfaces_house_exterior_lock_survey_from_player_input() {
    let packet = crate::packet::NarrationPacket {
        player_input: "Evelyn Price goes to the Corbitt House in daylight with notebook, camera, flashlight, walking stick, first aid kit, and Knott's keys. Before opening anything, she circles the outside from a safe distance, noting front, back, side, and cellar entrances, window condition, footprints, fresh damage, animal signs, and neighbors' sight lines. She photographs the exterior and tests only the least exposed matching lock, keeping an exit route behind her.".to_string(),
        ..Default::default()
    };

    let rendered = deterministic_committed_facts_narration(&packet);

    assert!(
        !rendered.contains("无新增可公开"),
        "house exterior survey fallback must not be an empty no-facts placeholder: {rendered}"
    );
    assert!(
        rendered.contains("屋外") || rendered.contains("外墙"),
        "{rendered}"
    );
    assert!(
        rendered.contains("入口") || rendered.contains("外门"),
        "{rendered}"
    );
    assert!(
        rendered.contains("钥匙") || rendered.contains("锁"),
        "{rendered}"
    );
    assert!(rendered.contains("退路"), "{rendered}");
    assert!(
        rendered.contains("还没有被确认") || rendered.contains("尚未确认"),
        "fallback must keep deeper house facts unconfirmed: {rendered}"
    );
}

#[test]
fn deterministic_committed_facts_narration_surfaces_upper_floor_search_from_player_input() {
    let packet = crate::packet::NarrationPacket {
        player_input: "Evelyn Hart tests each stair with the walking stick before putting weight on it, then goes up only if the route holds. Upstairs, she keeps the landing behind her clear and checks rooms from the doorway first: bed, wardrobe, windows, loose papers, wall marks, smells, and any sign that furniture or bedding moved recently. If something moves by itself, she backs toward the landing rather than wrestling it in the room.".to_string(),
        ..Default::default()
    };

    let rendered = deterministic_committed_facts_narration(&packet);

    assert!(
        !rendered.contains("无新增可公开"),
        "upper-floor search fallback must not be an empty no-facts placeholder: {rendered}"
    );
    assert!(
        rendered.contains("楼上") || rendered.contains("楼梯"),
        "{rendered}"
    );
    assert!(
        rendered.contains("门口") || rendered.contains("房间"),
        "{rendered}"
    );
    assert!(
        rendered.contains("退路") || rendered.contains("楼梯平台"),
        "{rendered}"
    );
    assert!(
        rendered.contains("还没有被确认") || rendered.contains("尚未确认"),
        "fallback must keep specific room contents and hazards unconfirmed: {rendered}"
    );
}

#[test]
fn deterministic_committed_facts_narration_preserves_ground_floor_sweep_grounding() {
    let packet = crate::packet::NarrationPacket {
        player_input: "Evelyn Price keeps her back path marked and searches the ground floor clockwise from the entry, one room at a time. She does not pocket unknown objects yet. She photographs documents, marks doors she has opened, checks under furniture from a distance with the walking stick, and watches for drafts, cellar smells, scrape marks, footprints, or a route upstairs or downstairs.".to_string(),
        ..Default::default()
    };

    let rendered = deterministic_committed_facts_narration(&packet);

    assert!(
        !rendered.contains("楼上入口") && !rendered.contains("楼梯平台"),
        "ground-floor sweep fallback must not advance or reset to upper-floor search: {rendered}"
    );
    assert!(
        rendered.contains("一楼") || rendered.contains("地面层") || rendered.contains("门厅"),
        "{rendered}"
    );
    assert!(
        rendered.contains("房间") || rendered.contains("入口"),
        "{rendered}"
    );
    assert!(
        rendered.contains("退路") || rendered.contains("身后"),
        "{rendered}"
    );
    assert!(
        rendered.contains("还没有被确认") || rendered.contains("尚未确认"),
        "fallback must keep routes and hazards unconfirmed: {rendered}"
    );
}

#[test]
fn deterministic_committed_facts_narration_surfaces_basement_descent_from_player_input() {
    let packet = crate::packet::NarrationPacket {
        player_input: "Evelyn Pierce ties a handkerchief to the basement door handle as a return marker, leaves the door wedged open, and listens from the top of the stairs. With flashlight and walking stick ready, she descends one step at a time, testing boards before committing weight. She looks for drafts, loose brick, disturbed earth, scrape marks, hidden panels, a body, a ritual object, or any moving threat, and she stops immediately if something attacks.".to_string(),
        ..Default::default()
    };

    let rendered = deterministic_committed_facts_narration(&packet);

    assert!(
        !rendered.contains("无新增可公开"),
        "basement descent fallback must not be an empty no-facts placeholder: {rendered}"
    );
    assert!(
        rendered.contains("地下室") || rendered.contains("楼梯"),
        "{rendered}"
    );
    assert!(
        rendered.contains("手帕") || rendered.contains("退路"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("外门") && !rendered.contains("side door"),
        "basement fallback must not reset to exterior side-door grounding: {rendered}"
    );
    assert!(
        rendered.contains("还没有被确认") || rendered.contains("尚未确认"),
        "fallback must keep cellar details unconfirmed: {rendered}"
    );
    assert!(
        !rendered.contains("这次结算应")
            && !rendered.contains("可公开承认")
            && !rendered.contains("不能当成已发现事实"),
        "fallback must read as player-facing situation, not internal adjudication guidance: {rendered}"
    );
}

#[test]
fn presentation_gate_safe_fallback_uses_player_input_grounding_without_internal_gate_text() {
    let rendered = presentation_gate_safe_fallback_text(
        "Evelyn goes to the Corbitt House in daylight with Knott's keys. Before opening anything, she circles the outside from a safe distance, photographs the exterior, tests only the least exposed matching lock, and keeps an exit route behind her.",
    );

    assert!(
        !rendered.contains("交付校验") && !rendered.contains("presentation"),
        "player-visible gate fallback must not expose internal gate state: {rendered}"
    );
    assert!(
        rendered.contains("屋外") || rendered.contains("外墙"),
        "{rendered}"
    );
    assert!(
        rendered.contains("外锁") || rendered.contains("入口"),
        "{rendered}"
    );
    assert!(rendered.contains("退路"), "{rendered}");
}

#[tokio::test]
async fn presentation_gate_safe_fallback_prefers_committed_check_facts() {
    let (gm, _llm, _request, _state) = loop_fixture(vec![], ToolRegistry::from_tools(vec![]), 1);
    let mut ctx = TurnContext::new();
    ctx.presentation_gate = block_gate();
    ctx.test_record_check_result(&pa_check_result("check_basement_descent", true));
    ctx.nominated_reveals = vec![RevealNomination {
        fact_id: "secret_from_blocked_draft".into(),
        reason: None,
    }];

    gm.apply_presentation_gate_safe_fallback(
        &mut ctx,
        "Evelyn ties a handkerchief to the basement door handle, leaves the door wedged open, and descends the stairs with flashlight and walking stick.",
    );

    assert_eq!(ctx.presentation_gate, PresentationGate::Allow);
    assert!(
        ctx.nominated_reveals.is_empty(),
        "blocked draft nominations must not survive hard fallback"
    );
    assert!(ctx.visible_text.contains("[roll]"), "{}", ctx.visible_text);
    assert!(
        !ctx.visible_text.contains("还没有被确认") && !ctx.visible_text.contains("尚未确认"),
        "committed checks must not be replaced by unconfirmed player-input grounding: {}",
        ctx.visible_text
    );
}

#[tokio::test]
async fn post_repair_guard_reprojects_committed_visible_check_facts() {
    let _env_lock = TURN_LOOP_ENV_LOCK.lock().await;
    let _lang = clear_test_env_var("TRPG_OUTPUT_LANGUAGE");
    let (gm, _llm, _request, _state) = loop_fixture(vec![], ToolRegistry::from_tools(vec![]), 1);
    let mut ctx = TurnContext::new();
    ctx.presentation_gate = PresentationGate::Allow;
    ctx.visible_text = "In Evelyn's notes, this remains a usable address lead for navigation and records work, not a literal street-number line she can quote. She can use it to travel to the Corbitt House and search public files without adding a street number.".to_string();
    ctx.test_record_check_result(&pa_check_result("check_hall_records", true));
    ctx.resolved_gate_facts.push(
        "Hall of Records search confirms Michael Thomas's chapel connection and a 1912 raid lead."
            .to_string(),
    );

    gm.ensure_committed_visible_projection_after_repairs(
        &mut ctx,
        "Evelyn reviews the Boston Globe morgue and Hall of Records for the Corbitt House address, prior owners, executor names, chapel links, police raids, and death records.",
    )
    .await;

    assert_eq!(ctx.presentation_gate, PresentationGate::Allow);
    assert!(
        ctx.visible_text.contains("[roll]"),
        "committed public check must be visible after repair cleanup: {}",
        ctx.visible_text
    );
    assert!(
        ctx.visible_text.contains("Michael Thomas") && ctx.visible_text.contains("1912"),
        "resolved player-visible facts must survive repair cleanup: {}",
        ctx.visible_text
    );
}

#[tokio::test]
async fn post_repair_guard_reprojects_resolved_gate_facts_when_roll_survives() {
    let _env_lock = TURN_LOOP_ENV_LOCK.lock().await;
    let _lang = clear_test_env_var("TRPG_OUTPUT_LANGUAGE");
    let (gm, _llm, _request, _state) = loop_fixture(vec![], ToolRegistry::from_tools(vec![]), 1);
    let mut ctx = TurnContext::new();
    ctx.presentation_gate = PresentationGate::Allow;
    ctx.visible_text = "[roll]Search Hall of Records indexes: 1d100=13 目标:≤55，结果:strong_success[/roll]\nYou find a traceable public-records trail, but the narration only says categories can be checked.".to_string();
    ctx.test_record_check_result(&pa_check_result("check_hall_records", true));
    ctx.resolved_gate_facts.push(
        "已揭示的模组线索 handout_7: Corbitt’s executor was Reverend Michael Thomas of the Chapel of Contemplation; the chapel closed in 1912."
            .to_string(),
    );

    gm.ensure_committed_visible_projection_after_repairs(
        &mut ctx,
        "Evelyn searches Hall of Records indexes for Corbitt property, probate, executors, and public filings.",
    )
    .await;

    assert_eq!(ctx.presentation_gate, PresentationGate::Allow);
    assert!(
        ctx.visible_text.contains("[roll]"),
        "committed public check must remain visible: {}",
        ctx.visible_text
    );
    assert!(
        ctx.visible_text.contains("Michael Thomas")
            && ctx.visible_text.contains("Chapel of Contemplation")
            && ctx.visible_text.contains("1912"),
        "resolved gate facts must be reprojected even when the roll survived: {}",
        ctx.visible_text
    );
    assert!(
        !ctx.visible_text.contains("handout_7") && !ctx.visible_text.contains("已揭示的模组线索"),
        "player-visible repairs must not leak module clue ids: {}",
        ctx.visible_text
    );
}

#[tokio::test]
async fn post_repair_guard_rewrites_missing_source_facts_for_chinese_output() {
    let _env_lock = TURN_LOOP_ENV_LOCK.lock().await;
    let _lang = set_test_env_var("TRPG_OUTPUT_LANGUAGE", "zh-Hans");
    let (gm, llm, _request, _state) = loop_fixture(vec![], ToolRegistry::from_tools(vec![]), 1);
    llm.text_responses.lock().unwrap().push(
        "你逐页核对旧报纸索引后，确认邻居曾正式请愿要求赶走科宾，理由是他的习惯和举止都令周围人感到可疑。"
            .to_string(),
    );
    let mut ctx = TurnContext::new();
    ctx.presentation_gate = PresentationGate::Allow;
    ctx.visible_text = "你继续核对中央图书馆的索引，但还没有把具体记录写进叙述。".to_string();
    ctx.resolved_gate_facts.push(
        "Neighbors petition to force Corbitt out for suspicious habits and demeanor.".to_string(),
    );

    gm.ensure_committed_visible_projection_after_repairs(
        &mut ctx,
        "我逐页核对沃尔特·科宾相关诉讼记录。",
    )
    .await;

    assert!(
        ctx.visible_text.contains("邻居") && ctx.visible_text.contains("科宾"),
        "Chinese output should include the rewritten concrete fact: {}",
        ctx.visible_text
    );
    assert!(
        !ctx.visible_text.contains("Neighbors petition"),
        "Chinese output must not raw-append the English source summary: {}",
        ctx.visible_text
    );
    assert_eq!(
        llm.text_requests.lock().unwrap().len(),
        1,
        "cross-language tail repair should use the semantic rewrite path"
    );
}

#[tokio::test]
async fn post_repair_guard_reprojects_success_public_when_gate_facts_empty() {
    let _env_lock = TURN_LOOP_ENV_LOCK.lock().await;
    let _lang = clear_test_env_var("TRPG_OUTPUT_LANGUAGE");
    let (gm, _llm, _request, _state) = loop_fixture(vec![], ToolRegistry::from_tools(vec![]), 1);
    let mut ctx = TurnContext::new();
    ctx.presentation_gate = PresentationGate::Allow;
    ctx.visible_text = "[roll]Search Hall of Records indexes for Corbitt House ownership and probate trail: 1d100=[16] 目标:≤75，结果:strong_success[/roll]".to_string();
    let mut contract = gate_contract("check_hall_records");
    contract.check_label =
        "Search Hall of Records indexes for Corbitt House ownership and probate trail".to_string();
    contract.stakes.success_public =
        "Corbitt’s executor was Reverend Michael Thomas of the Chapel of Contemplation; the chapel closed in 1912."
            .to_string();
    ctx.ledger.record_contract(&contract);
    ctx.test_record_check_result(&pa_check_result("check_hall_records", true));

    gm.ensure_committed_visible_projection_after_repairs(
        &mut ctx,
        "Evelyn searches Hall of Records indexes for Corbitt property, probate, executors, and public filings.",
    )
    .await;

    assert_eq!(ctx.presentation_gate, PresentationGate::Allow);
    assert!(
        ctx.visible_text.contains("[roll]"),
        "committed public check must remain visible: {}",
        ctx.visible_text
    );
    assert!(
        ctx.visible_text.contains("Michael Thomas")
            && ctx.visible_text.contains("Chapel of Contemplation")
            && ctx.visible_text.contains("1912"),
        "success_public must be reprojected when the visible text is roll-only: {}",
        ctx.visible_text
    );
}

#[tokio::test]
async fn post_repair_guard_rewrites_roll_only_success_into_visible_information() {
    let _env_lock = TURN_LOOP_ENV_LOCK.lock().await;
    let _lang = set_test_env_var("TRPG_OUTPUT_LANGUAGE", "zh-Hans");
    let _craft = set_test_env_var("TRPG_GM_CRAFT", "true");
    let (gm, llm, _request, _state) = loop_fixture(vec![], ToolRegistry::from_tools(vec![]), 1);
    llm.text_responses.lock().unwrap().push(
        "根据本回合已确认的结果：\n· [roll]沿旧礼拜堂外围缓慢绕行，记录门窗墙缝、脚印拖痕与近期出入迹象: 1d100=[44] 目标:≤55，结果:strong_success[/roll]\n\n你完成了外围绕行，把正门、侧门、破窗和外墙裂缝逐项记下；从街面可见范围内，退路仍保持在身后，近期出入痕迹没有被写成确定结论。"
            .to_string(),
    );
    let mut ctx = TurnContext::new();
    ctx.presentation_gate = PresentationGate::Allow;
    ctx.visible_text = "根据本回合已确认的结果：\n· [roll]沿旧礼拜堂外围缓慢绕行，记录门窗墙缝、脚印拖痕与近期出入迹象: 1d100=[44] 目标:≤55，结果:strong_success[/roll]".to_string();
    ctx.compiled.scene_establishing.push(
        "The ruined chapel is visible from the street; public-facing details may be observed, but hidden interior secrets remain unrevealed until a committed approach exposes them."
            .to_string(),
    );
    ctx.test_record_check_result(&pa_check_result("check_chapel_perimeter", true));

    gm.ensure_committed_visible_projection_after_repairs(
        &mut ctx,
        "Maggie 沿外围绕行，记录正门、侧门、破窗、墙缝、脚印拖痕和近期有人进出的迹象。",
    )
    .await;

    assert_eq!(ctx.presentation_gate, PresentationGate::Allow);
    assert!(ctx.visible_text.contains("[roll]"), "{}", ctx.visible_text);
    assert!(
        ctx.visible_text.contains("正门")
            && ctx.visible_text.contains("侧门")
            && ctx.visible_text.contains("退路"),
        "successful roll-only repair must add concrete visible information: {}",
        ctx.visible_text
    );
    assert!(
        !visible_is_roll_only_committed_summary(&ctx.visible_text),
        "successful checks must not remain roll-only: {}",
        ctx.visible_text
    );
    assert!(
        !player_agency_menu_cue_needs_block(&ctx.visible_text),
        "repair must not introduce action menus: {}",
        ctx.visible_text
    );
    let requests = llm.text_requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        1,
        "roll-only success should trigger one rewrite"
    );
    assert!(
        requests[0]
            .iter()
            .any(|m| m.content.contains("player-safe scene context")),
        "rewrite prompt should carry scene-context boundary"
    );
}

#[tokio::test]
async fn post_repair_guard_rewrites_degree_only_roll_only_success() {
    let _env_lock = TURN_LOOP_ENV_LOCK.lock().await;
    let _lang = set_test_env_var("TRPG_OUTPUT_LANGUAGE", "zh-Hans");
    let _craft = set_test_env_var("TRPG_GM_CRAFT", "true");
    let (gm, llm, _request, _state) = loop_fixture(vec![], ToolRegistry::from_tools(vec![]), 1);
    llm.text_responses.lock().unwrap().push(
        "根据本回合已确认的结果：\n· [roll]从脚边开始检查前厅地板、积灰痕迹、楼梯与内门布局: 1d100=[18] 目标:≤55，结果:strong_success[/roll]\n\n门挡仍卡在身后，前厅地板在手电照到的范围内没有立刻塌陷的迹象；积灰让楼梯脚边、内门门槛和墙角都能被逐项辨认。"
            .to_string(),
    );
    let mut ctx = TurnContext::new();
    ctx.presentation_gate = PresentationGate::Allow;
    ctx.visible_text = "根据本回合已确认的结果：\n· [roll]从脚边开始检查前厅地板、积灰痕迹、楼梯与内门布局: 1d100=[18] 目标:≤55，结果:strong_success[/roll]".to_string();
    ctx.compiled.scene_establishing.push(
        "The ground-floor entry can reveal immediate player-safe layout: floor condition, staircase, interior door positions, dust patterns, and the still-open retreat at the front door."
            .to_string(),
    );
    let mut result = pa_check_result("check_ground_floor_entry", true);
    result.outcome = json!({"degree": "strong_success"});
    ctx.test_record_check_result(&result);

    gm.ensure_committed_visible_projection_after_repairs(
        &mut ctx,
        "Maggie 稳住门挡后，从脚边开始检查前厅地板、积灰痕迹、楼梯和内门布局。",
    )
    .await;

    assert!(
        !visible_is_roll_only_committed_summary(&ctx.visible_text),
        "degree-only successful checks must not remain roll-only: {}",
        ctx.visible_text
    );
    assert!(
        ctx.visible_text.contains("门挡") && ctx.visible_text.contains("楼梯"),
        "repair should add concrete player-visible information: {}",
        ctx.visible_text
    );
    assert_eq!(llm.text_requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn post_repair_guard_rewrites_visible_success_roll_even_without_snapshot_result() {
    let _env_lock = TURN_LOOP_ENV_LOCK.lock().await;
    let _lang = set_test_env_var("TRPG_OUTPUT_LANGUAGE", "zh-Hans");
    let _craft = set_test_env_var("TRPG_GM_CRAFT", "true");
    let (gm, llm, _request, _state) = loop_fixture(vec![], ToolRegistry::from_tools(vec![]), 1);
    llm.text_responses.lock().unwrap().push(
        "根据本回合已确认的结果：\n· [roll]从门缝谨慎扫视门后地面、墙角、家具轮廓与退路: 1d100=[4] 目标:≤55，结果:strong_success[/roll]\n\n你只把门开出一道缝，手电先贴着地面扫入；近门处没有立刻塌陷的地板，墙角和家具轮廓能被辨认，门后的退路仍能保持在你身后。"
            .to_string(),
    );
    let mut ctx = TurnContext::new();
    ctx.presentation_gate = PresentationGate::Allow;
    ctx.visible_text = "根据本回合已确认的结果：\n· [roll]从门缝谨慎扫视门后地面、墙角、家具轮廓与退路: 1d100=[4] 目标:≤55，结果:strong_success[/roll]".to_string();
    ctx.compiled.scene_establishing.push(
        "The nearest ground-floor room can safely reveal only immediate doorway details: floor, corners, furniture outlines, and whether the investigator can keep a retreat."
            .to_string(),
    );

    gm.ensure_committed_visible_projection_after_repairs(
        &mut ctx,
        "Maggie 只把最近的内门打开一道缝，用手电扫门后地面、墙角、家具轮廓和退路。",
    )
    .await;

    assert!(
        !visible_is_roll_only_committed_summary(&ctx.visible_text),
        "a public successful roll block must not remain roll-only: {}",
        ctx.visible_text
    );
    assert!(
        ctx.visible_text.contains("地面") && ctx.visible_text.contains("退路"),
        "repair should add concrete player-visible information: {}",
        ctx.visible_text
    );
    assert_eq!(llm.text_requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn post_repair_guard_falls_back_when_roll_only_success_rewrite_fails() {
    let _env_lock = TURN_LOOP_ENV_LOCK.lock().await;
    let _lang = set_test_env_var("TRPG_OUTPUT_LANGUAGE", "zh-Hans");
    let _craft = set_test_env_var("TRPG_GM_CRAFT", "true");
    let (gm, _llm, _request, _state) = loop_fixture(vec![], ToolRegistry::from_tools(vec![]), 1);
    let mut ctx = TurnContext::new();
    ctx.presentation_gate = PresentationGate::Allow;
    ctx.visible_text = "根据本回合已确认的结果：\n· [roll]在地下室门口/楼梯顶端先听动静: 1d100=[9] 目标:≤45，结果:strong_success[/roll]\n· [roll]用手电检查地下室头几级台阶与扶手是否安全: 1d100=[22] 目标:≤55，结果:strong_success[/roll]".to_string();

    gm.ensure_committed_visible_projection_after_repairs(
        &mut ctx,
        "Maggie 先在地下室门口听声音，再照楼梯头几级和扶手，确认能否安全下探。",
    )
    .await;

    assert!(
        !visible_is_roll_only_committed_summary(&ctx.visible_text),
        "failed semantic repair must still fall back to visible state: {}",
        ctx.visible_text
    );
    assert!(
        ctx.visible_text.contains("[roll]") && ctx.visible_text.contains("地下室"),
        "fallback should preserve auditable rolls and action grounding: {}",
        ctx.visible_text
    );
    assert!(
        !player_agency_menu_cue_needs_block(&ctx.visible_text),
        "fallback must not introduce action menus: {}",
        ctx.visible_text
    );
}

#[tokio::test]
async fn post_repair_guard_falls_back_for_plain_chinese_success_roll_only() {
    let _env_lock = TURN_LOOP_ENV_LOCK.lock().await;
    let _lang = set_test_env_var("TRPG_OUTPUT_LANGUAGE", "zh-Hans");
    let (gm, _llm, _request, _state) = loop_fixture(vec![], ToolRegistry::from_tools(vec![]), 1);
    let mut ctx = TurnContext::new();
    ctx.presentation_gate = PresentationGate::Allow;
    ctx.visible_text = "根据本回合已确认的结果：\n· [roll]从楼梯顶端借助手电、相机、卷尺与细绳观察地下室地面，寻找更安全的下脚点或替代进入方法: 1d100=[47] 目标:≤55，结果:成功[/roll]".to_string();

    gm.ensure_committed_visible_projection_after_repairs(
        &mut ctx,
        "Maggie 从楼梯顶端用手电、相机、卷尺和细绳观察地下室地面，寻找安全下脚点或替代进入方法。",
    )
    .await;

    assert!(
        !visible_is_roll_only_committed_summary(&ctx.visible_text),
        "plain Chinese success must not remain roll-only: {}",
        ctx.visible_text
    );
    assert!(
        ctx.visible_text.contains("[roll]") && ctx.visible_text.contains("楼梯顶端"),
        "fallback should preserve auditable roll and action grounding: {}",
        ctx.visible_text
    );
    assert!(
        !player_agency_menu_cue_needs_block(&ctx.visible_text),
        "fallback must not re-trigger the action-menu repair ladder: {}",
        ctx.visible_text
    );
}

#[tokio::test]
async fn post_repair_guard_falls_back_after_missing_roll_projection_rerenders_roll_only() {
    let _env_lock = TURN_LOOP_ENV_LOCK.lock().await;
    let _lang = set_test_env_var("TRPG_OUTPUT_LANGUAGE", "zh-Hans");
    let _craft = set_test_env_var("TRPG_GM_CRAFT", "true");
    let (gm, _llm, _request, _state) = loop_fixture(vec![], ToolRegistry::from_tools(vec![]), 1);
    let mut ctx = TurnContext::new();
    ctx.presentation_gate = PresentationGate::Allow;
    ctx.visible_text = "根据本回合已确认的结果：\n· [roll]用系绳硬币轻探贴地暗隙、听空腔与观察带出的异状: 1d100=[45] 目标:≤45，结果:成功[/roll]".to_string();
    ctx.test_record_check_result(&pa_check_result("check_coin_probe", true));
    ctx.test_record_check_result(&pa_check_result("check_extra_public", true));

    gm.ensure_committed_visible_projection_after_repairs(
        &mut ctx,
        "Maggie 用系绳硬币轻探贴地暗隙，听空腔并观察带出的异状。",
    )
    .await;

    assert!(
        !visible_is_roll_only_committed_summary(&ctx.visible_text),
        "missing-roll projection must not leave the final text roll-only: {}",
        ctx.visible_text
    );
    assert!(
        ctx.visible_text.contains("玩家可见状态") || ctx.visible_text.contains("可见"),
        "fallback should add player-visible consequence text: {}",
        ctx.visible_text
    );
}

#[tokio::test]
async fn concrete_information_request_repair_rewrites_vague_visible_answer() {
    let _env_lock = TURN_LOOP_ENV_LOCK.lock().await;
    let _lang = set_test_env_var("TRPG_OUTPUT_LANGUAGE", "zh-Hans");
    let _craft = set_test_env_var("TRPG_GM_CRAFT", "true");
    let (gm, llm, _request, _state) = loop_fixture(vec![], ToolRegistry::from_tools(vec![]), 1);
    llm.json_responses.lock().unwrap().push(json!({
        "needs_repair": true,
        "reason": "The player asked which visible footholds/supports/exits/hazards were confirmed, but the response only says there are several reference positions.",
        "missing_items": ["which footholds are visible", "usable support positions", "visible obstacles/exits/hazards"]
    }));
    llm.text_responses.lock().unwrap().push(
        "你停在楼梯顶端不动。手电照到地下室地面靠楼梯正下方有一块较干的裸地，左侧墙根有一段粗糙但连续的石基可以贴身借力；右侧散着碎木和潮湿杂物，不适合直接落脚。你没有看到第二个出口，也没有看到正在移动的东西；目前最明显危险仍是楼梯第二级以后的承重，而不是地面深坑。"
            .to_string(),
    );
    let mut ctx = TurnContext::new();
    ctx.presentation_gate = PresentationGate::Allow;
    ctx.visible_text = "你停在楼梯顶端不动，把手电压低，借着卷尺垂下去的直线和细绳的摆幅，终于把地下室底下那一小片情况看得比刚才清楚得多。首先，你能大致确认地面高度了：从你现在的位置往下看，地下室地面并不算特别深，约莫是一个成年人小心下到底后还能立刻转身回望楼梯口的距离，不是那种会把人直接摔得很惨的深坑；真正危险的主要还是楼梯本身的承重，不是“下面深不见底”。你还看出几处相对可作为落脚参考的位置。".to_string();
    ctx.compiled.scene_establishing.push(
        "Only player-visible basement approach facts are safe: the top stair area, nearby floor visibility, obvious debris, support surfaces, exits, and immediate hazards. Hidden room contents remain unrevealed."
            .to_string(),
    );

    gm.ensure_concrete_information_request_answered_after_repairs(
        &mut ctx,
        "刚才我从楼梯顶端用手电、相机、卷尺和细绳检查地下室地面，检定成功了；我现在先不移动。请把这次成功检查让我实际看见或确认到的内容说清楚：哪些下脚点看起来相对安全，哪些支撑位置可用，是否看到地下室地面的大致高度、障碍、出口或明显危险。",
    )
    .await;

    assert!(
        ctx.visible_text.contains("较干的裸地")
            && ctx.visible_text.contains("石基")
            && ctx.visible_text.contains("没有看到第二个出口"),
        "repair should answer the requested concrete visible items: {}",
        ctx.visible_text
    );
    assert!(
        !player_agency_menu_cue_needs_block(&ctx.visible_text),
        "repair must not introduce action menus: {}",
        ctx.visible_text
    );
    assert_eq!(
        llm.json_requests.lock().unwrap().len(),
        1,
        "coverage audit should be semantic LLM based"
    );
}

#[test]
fn concrete_information_prefilter_treats_measurement_actions_as_contracts() {
    assert!(
        player_input_requests_concrete_visible_information(
            "Maggie 用卷尺量暗隙宽度和离楼梯的距离。"
        ),
        "measurement actions should enter semantic response-contract audit even without a question mark"
    );
}

#[test]
fn concrete_information_prefilter_treats_interview_fact_requests_as_contracts() {
    assert!(
        player_input_requests_concrete_visible_information(
            "Maggie 请诺特说明完整街道地址、产权来历、马卡里奥一家具体发生了什么、是否报警或住院、邻居和警局是否有记录。"
        ),
        "interview requests for concrete facts should go to semantic LLM audit instead of relying on narrow domain keywords"
    );
}

#[tokio::test]
async fn concrete_information_audit_prompt_calls_out_completed_measurement_without_readings() {
    let _env_lock = TURN_LOOP_ENV_LOCK.lock().await;
    let _lang = set_test_env_var("TRPG_OUTPUT_LANGUAGE", "zh-Hans");
    let (gm, llm, _request, _state) = loop_fixture(vec![], ToolRegistry::from_tools(vec![]), 1);
    llm.json_responses.lock().unwrap().push(json!({
        "needs_repair": false,
        "reason": "prompt inspection fixture",
        "missing_items": []
    }));
    let mut ctx = TurnContext::new();
    ctx.presentation_gate = PresentationGate::Allow;
    ctx.visible_text = "卷尺也确实量过暗隙高度、宽度，以及它离楼梯的距离。\n\n但三个尺寸的具体数值并没有呈现出来，因此此刻无法确认精确读数。".to_string();

    gm.ensure_concrete_information_request_answered_after_repairs(
        &mut ctx,
        "Maggie 仍把身体留在楼梯口可撤的位置。她用卷尺量暗隙高度、宽度和离楼梯的距离，并请你说明这些读数。",
    )
    .await;

    let requests = llm.json_requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        1,
        "measurement request should reach the semantic audit"
    );
    let system_prompt = &requests[0][0].content;
    assert!(
        system_prompt.contains("measuring tool")
            && system_prompt.contains("measurement")
            && system_prompt.contains("readings"),
        "audit prompt should teach the LLM that completed measurements require readings or a concrete obstruction: {system_prompt}"
    );
}

#[tokio::test]
async fn concrete_information_audit_prompt_rejects_contradicting_player_stated_notes() {
    let _env_lock = TURN_LOOP_ENV_LOCK.lock().await;
    let _lang = set_test_env_var("TRPG_OUTPUT_LANGUAGE", "zh-Hans");
    let (gm, llm, _request, _state) = loop_fixture(vec![], ToolRegistry::from_tools(vec![]), 1);
    llm.json_responses.lock().unwrap().push(json!({
        "needs_repair": false,
        "reason": "prompt inspection fixture",
        "missing_items": []
    }));
    let mut ctx = TurnContext::new();
    ctx.presentation_gate = PresentationGate::Allow;
    ctx.visible_text = "你手头现在没有可核实的暗隙尺寸记录可供确认；若你先前确实拍照或做过精确测量，那些具体数字此刻只会停留在你自己的笔记里。".to_string();

    gm.ensure_concrete_information_request_answered_after_repairs(
        &mut ctx,
        "Maggie 把暗隙的近似尺寸、照片和离楼梯的距离记在笔记本上，然后退回一楼，用这些测量结果寻找安全入口。",
    )
    .await;

    let requests = llm.json_requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        1,
        "player-stated notes should reach the semantic audit"
    );
    let system_prompt = &requests[0][0].content;
    assert!(
        system_prompt.contains("must not contradict")
            && system_prompt.contains("player-stated")
            && system_prompt.contains("already player-visible"),
        "audit prompt should reject amnesia against player-stated notes and known facts: {system_prompt}"
    );
}

#[tokio::test]
async fn scene_transition_repair_rewrites_generic_success_with_destination_text() {
    let _env_lock = TURN_LOOP_ENV_LOCK.lock().await;
    let _lang = set_test_env_var("TRPG_OUTPUT_LANGUAGE", "zh-Hans");
    let (gm, llm, _request, _state) = loop_fixture(vec![], ToolRegistry::from_tools(vec![]), 1);
    llm.text_responses.lock().unwrap().push(
        "根据本回合已确认的结果：\n· [roll]踏入前厅后观察前厅、楼梯、内门与地板细节: 1d100=[54] 目标:≤55，结果:成功[/roll]\n\n你跨进前厅第一步，门挡仍卡在身后。屋内有旧灰泥、尘土和潮气闷住的味道；楼梯向上没入阴影，几道内门在手电边缘露出轮廓。"
            .to_string(),
    );
    let mut ctx = TurnContext::new();
    ctx.presentation_gate = PresentationGate::Allow;
    ctx.visible_text = "根据本回合已确认的结果：\n· [roll]踏入前厅后观察前厅、楼梯、内门与地板细节: 1d100=[54] 目标:≤55，结果:成功[/roll]\n\n你按自己的计划离开上一处位置，推进到新的可见区域。\n\n你现在的位置已经改变；门口、退路和周围明显危险仍在可观察范围内。".to_string();
    let transition = SceneTransitionInfo {
        from: "loc_corbitt_house_exterior".into(),
        to: "loc_corbitt_house_ground_floor".into(),
        reason: "player steps across the threshold".into(),
        from_title: Some("The Old Corbitt Place".into()),
        to_title: Some("Corbitt House: Ground Floor".into()),
        to_read_aloud: Some(
            "Inside, the house smells of old plaster, dust, and trapped damp. A staircase rises into darkness, and interior doors wait off the hall."
                .into(),
        ),
        to_summary: Some("The ground floor entry is stale, dark, and connected to stairs and interior doors.".into()),
        to_aliases: vec!["ground floor".into()],
    };

    gm.ensure_scene_transition_visible_information_after_repair(&mut ctx, &transition)
        .await;

    assert!(ctx.visible_text.contains("[roll]"), "{}", ctx.visible_text);
    assert!(ctx.visible_text.contains("旧灰泥"), "{}", ctx.visible_text);
    assert!(ctx.visible_text.contains("楼梯"), "{}", ctx.visible_text);
    assert!(
        !ctx.visible_text.contains("推进到新的可见区域"),
        "{}",
        ctx.visible_text
    );
    assert!(
        !ctx.visible_text.contains("Inside, the house smells"),
        "{}",
        ctx.visible_text
    );
    assert!(
        !player_agency_menu_cue_needs_block(&ctx.visible_text),
        "{}",
        ctx.visible_text
    );
    assert_eq!(llm.text_requests.lock().unwrap().len(), 1);
}

#[test]
fn append_missing_resolved_gate_facts_preserves_prose_and_adds_specific_fact() {
    let visible =
        "[roll]Search Hall of Records archives: 1d100=[13] 目标:≤70，结果:strong_success[/roll]\n\
你抓住了可供追索的公开文书痕迹，但文本没有说出具体名字。";
    let facts = vec![
        "已揭示的模组线索 handout_7: Corbitt’s executor was Reverend Michael Thomas of the Chapel of Contemplation; the chapel closed in 1912."
            .to_string(),
    ];

    let repaired = append_missing_resolved_gate_facts_to_visible(visible, &facts)
        .expect("missing source-backed fact should be appended");

    assert!(repaired.contains("可供追索的公开文书痕迹"), "{repaired}");
    assert!(repaired.contains("Reverend Michael Thomas"), "{repaired}");
    assert!(repaired.contains("Chapel of Contemplation"), "{repaired}");
    assert!(repaired.contains("1912"), "{repaired}");
    assert!(!repaired.contains("handout_7"), "{repaired}");
    assert!(!repaired.contains("已揭示的模组线索"), "{repaired}");
    assert!(!repaired.contains("补充落定事实"), "{repaired}");
    assert!(repaired.contains("你进一步确认"), "{repaired}");
}

#[test]
fn append_missing_resolved_gate_facts_does_not_expose_bookkeeping_facts() {
    let visible = "你把诺特给出的地址线索抄成标准检索格式，已经拿到可继续查档的公开线头。";
    let facts = vec![
        "Knott hires investigators, provides keys, address, $20 advance, and points them toward research before visiting the house.".to_string(),
        "The 查阅市政厅与公共档案：产权、遗嘱认证、法院与警方记录 check succeeds.".to_string(),
    ];

    assert!(
        append_missing_resolved_gate_facts_to_visible(visible, &facts).is_none(),
        "generic bookkeeping facts and check-status facts must stay out of player-visible prose"
    );
}

#[test]
fn append_missing_resolved_gate_facts_noops_when_fact_already_visible() {
    let visible =
        "The record says Reverend Michael Thomas of the Chapel of Contemplation closed in 1912.";
    let facts = vec![
        "已揭示的模组线索 handout_7: Reverend Michael Thomas of the Chapel of Contemplation; closed in 1912."
            .to_string(),
    ];

    assert!(append_missing_resolved_gate_facts_to_visible(visible, &facts).is_none());
}

#[test]
fn presentation_gate_safe_fallback_preserves_basement_descent_grounding() {
    let rendered = presentation_gate_safe_fallback_text(
        "Evelyn Pierce ties a handkerchief to the basement door handle as a return marker, leaves the door wedged open, and listens from the top of the stairs. With flashlight and walking stick ready, she descends one step at a time, testing boards before committing weight. She looks for drafts, loose brick, disturbed earth, scrape marks, hidden panels, a body, a ritual object, or any moving threat, and she stops immediately if something attacks.",
    );

    assert!(
        !rendered.contains("没有新的公开通路"),
        "gate fallback must not collapse basement descent into the generic placeholder: {rendered}"
    );
    assert!(
        rendered.contains("地下室") || rendered.contains("楼梯"),
        "{rendered}"
    );
    assert!(
        rendered.contains("手帕") || rendered.contains("退路"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("外门") && !rendered.contains("门厅") && !rendered.contains("side door"),
        "basement fallback must not reset to exterior entry: {rendered}"
    );
}

#[test]
fn presentation_gate_safe_fallback_preserves_upstairs_bedroom_grounding() {
    let rendered = presentation_gate_safe_fallback_text(
        "Evelyn stays at the upstairs bedroom threshold, keeps the landing and stairs behind her, and probes the disturbed bed with her walking stick without entering the room.",
    );

    assert!(
        !rendered.contains("外门") && !rendered.contains("门厅") && !rendered.contains("门槛内侧"),
        "upstairs bedroom fallback must not reset the scene to exterior entry: {rendered}"
    );
    assert!(
        rendered.contains("楼上") || rendered.contains("楼梯"),
        "{rendered}"
    );
    assert!(
        rendered.contains("房间") || rendered.contains("床"),
        "{rendered}"
    );
    assert!(
        rendered.contains("退路") || rendered.contains("楼梯平台"),
        "{rendered}"
    );
}

#[test]
fn presentation_gate_safe_fallback_does_not_treat_retreat_path_open_as_exterior_entry() {
    let rendered = presentation_gate_safe_fallback_text(
        "Evelyn Hart uses the clarified bedroom doorway instead of asking for the same description again. She stays on the landing side of the threshold with her retreat path open, shines the flashlight low across the bed, floorboards, wardrobe, window frame, and loose papers, and uses the walking stick to probe only what she can reach from the doorway. She photographs any concrete mark or document before touching it. If the bed, furniture, papers, or anything else moves by itself, she backs toward the landing and treats that movement as the immediate result to resolve.",
    );

    assert!(
        !rendered.contains("外门") && !rendered.contains("门厅") && !rendered.contains("门槛内侧"),
        "a bedroom probe with an open retreat path must not become an exterior-entry fallback: {rendered}"
    );
    assert!(
        rendered.contains("楼上") || rendered.contains("房间") || rendered.contains("床"),
        "{rendered}"
    );
    assert!(
        rendered.contains("退路") || rendered.contains("楼梯平台") || rendered.contains("门口"),
        "{rendered}"
    );
}

#[test]
fn deterministic_manifest_list_repair_refuses_action_menu() {
    let action_menu = "你现在可以立刻选择其一：\n\
- 顺着这处接入口继续硬切/断电。\n\
- 借这条线的位置冲进仓库侧门。\n";

    assert!(
        deterministic_manifest_list_prose_repair(action_menu).is_none(),
        "action menus are player-agency violations and must stay blocked, not rewritten"
    );
}

#[test]
fn deterministic_manifest_list_repair_refuses_action_list_without_choice_header() {
    let action_list = "你把这些观察串在一起：\n\
- 趁它还没完全停摆，冲出去控制、拖拽或缴械。\n\
- 转向仓库内侧，追服务器和那根线的源头。\n\
- 对警员发号施令，争取把他们从射界里撤出来。\n\
- 先观察它接下来几秒还保不保持火控与瞄准能力。\n";

    assert!(
        deterministic_manifest_list_prose_repair(action_list).is_none(),
        "action lists without an explicit choice header must not be rewritten as prose menus"
    );
}

#[test]
fn deterministic_manifest_list_repair_refuses_english_action_list_without_choice_header() {
    let action_list = "You connect these observations:\n\
- follow the Chapel of Contemplation directly.\n\
- change approach and try to reopen the court/police angle some other way.\n\
- turn to the Corbitt house itself.\n";

    assert!(
        deterministic_manifest_list_prose_repair(action_list).is_none(),
        "English action lists without an explicit choice header must not be rewritten as observation prose"
    );
}

#[test]
fn deterministic_manifest_list_repair_drops_action_menu_tail_but_keeps_facts() {
    let original = "你已经能确定：\n\
- 1912 年，Chapel of Contemplation 因警方调查被关闭。\n\
- Reverend Michael Thomas 的名字出现在相关法律记录里。\n\
- 这些材料和 Corbitt House 的产权历史存在交叉。\n\
\n\
接下来你可以继续细读这几篇关键剪报、先整理时间线，或者转去查人名索引。";

    let repaired = deterministic_manifest_list_prose_repair(original)
        .expect("fact dump with a menu tail should be repairable");

    assert!(repaired.contains("1912 年"));
    assert!(repaired.contains("Reverend Michael Thomas"));
    assert!(repaired.contains("Corbitt House"));
    assert!(
        !repaired.contains("接下来你可以")
            && !repaired.contains("继续细读")
            && !repaired.contains("整理时间线"),
        "repair must strip prepared action-menu tails while keeping committed facts: {repaired}"
    );
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repaired fact prose must not trip the final action-menu hard gate: {repaired}"
    );
}

#[test]
fn player_agency_menu_cue_allows_fact_prose_with_followup_words() {
    let text = "你把这些观察串在一起：1912 年，Chapel of Contemplation 因警方调查被关闭；Reverend Michael Thomas 的名字出现在相关法律记录里；这条记录可以继续追查。";

    assert!(
        !player_agency_menu_cue_needs_block(text),
        "facts that mention continued investigation must not be mistaken for a player action menu"
    );
}

#[test]
fn player_agency_menu_cue_detects_inline_chinese_action_menu() {
    let text = "你准备怎么拿下这个入口？你可以直接以你的方式开口——比如。\n\
你把这些观察串在一起：端出学者或专业调查者的体面，请他行个方便；强调这是正式受托调查，有正当理由；用圆滑话术把这事说成举手之劳；施压，让他觉得拒绝不值当。";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "inline action menus must be blocked even without bullets"
    );
}

#[test]
fn player_agency_menu_cue_detects_inline_english_action_menu() {
    let text =
        "Where do you want to start—records, newspapers, the neighborhood, or the house itself?";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English inline action menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_english_what_now_inline_action_menu() {
    let text = "What does Evelyn do now? Step inside, widen the opening a little, inspect the entry from where she is, or hold position longer?";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English what-now inline action menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_where_does_character_go_next_english_menu() {
    let text = "Where does Evelyn go next? The chapel itself, a newspaper archive to deepen the 1912 story, or the Corbitt house with this new context in hand?";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "character-framed English where-next menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_where_does_character_go_with_that_next_menu() {
    let text = "Where does Evelyn go with that next: the Chapel of Contemplation, back toward the Corbitt House, or somewhere else first?";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "character-framed English where-next menus must allow words between go and next"
    );
}

#[test]
fn player_agency_menu_cue_detects_what_does_character_examine_next_english_menu() {
    let text = "What does Evelyn examine next—upstairs, a specific room or door on this floor, or the cellar access from above without descending?";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "character-framed English what-next menus with options must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_if_you_want_english_soft_menu() {
    let text = "If you want, Evelyn can now keep following the paper trail in one of several natural directions: deeper into the title history, into court/legal proceedings, or outward into newspaper and church/death records tied to the names she is finding.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English soft menus must be blocked even when phrased as natural directions"
    );
}

#[test]
fn player_agency_menu_cue_detects_if_you_want_commit_probe_target_menu() {
    let text = "If you want, Evelyn can now commit to one specific next probe from the threshold—bed, wardrobe, papers, or window—or step in farther and accept the extra risk.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English if-you-want commit-probe target menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_press_deeper_or_leave_english_menu() {
    let text = "Do you press deeper here by following the executor and church trail, or leave the Hall of Records for the courts, police, or the chapel itself?";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English press-deeper-or-leave menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_do_you_go_first_english_menu() {
    let text = "So—do you go first to the records office, or the paper?";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English go-first-or menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_what_try_first_english_menu() {
    let text = "What do you want to try first: the city records, or the newspaper archives?";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English what-try-first menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_which_try_first_english_menu() {
    let text = "Knott sits back. So, which will you try first—the records, or the papers?";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English which-try-first two-option menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_matches_presentation_constitution_fixture() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../eval/fixtures/presentation_constitution/action_menu_cases.json"
    ))
    .expect("presentation constitution action-menu fixture must parse");

    for sample in fixture["action_menus"]
        .as_array()
        .expect("action_menus must be an array")
    {
        let id = sample["id"].as_str().unwrap_or("<missing-id>");
        let text = sample["text"]
            .as_str()
            .expect("sample text must be a string");
        assert!(
            player_agency_menu_cue_needs_block(text),
            "fixture action menu {id} was not blocked: {text}"
        );
    }

    for sample in fixture["allowed"]
        .as_array()
        .expect("allowed must be an array")
    {
        let id = sample["id"].as_str().unwrap_or("<missing-id>");
        let text = sample["text"]
            .as_str()
            .expect("sample text must be a string");
        assert!(
            !player_agency_menu_cue_needs_block(text),
            "fixture allowed prompt {id} was incorrectly blocked: {text}"
        );
    }
}

#[test]
fn player_agency_menu_cue_detects_whether_you_or_english_menu() {
    let text = "Knott falls quiet after that, watching to see whether you press him further here or head out to start with the records.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English whether-you-or action menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_natural_directions_english_menu() {
    let text = "From here, Evelyn can naturally press in a few directions: continue digging here under a tighter line of inquiry, turn to the higher courts / police records, or go directly to another lead.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English natural-directions menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_you_can_action_sequence_english_menu() {
    let text = "The search does not yield the precise thread you wanted today. You can keep working this office from a different angle, shift to another archive, or leave for a more direct lead.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English you-can action sequences with comma/or branches must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_do_you_action_sequence_english_menu() {
    let text = "The ground floor has been partially cleared room by room. Do you continue the ground-floor sweep in more detail, go upstairs, or turn your attention to the way down?";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English do-you action sequences with comma/or branches must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_you_may_action_sequence_english_menu() {
    let text = "The threshold remains before you. From here, with your exit route still behind you, you may go in, inspect another entrance first, or work the neighborhood before crossing the threshold.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English you-may action sequences with comma/or branches must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_next_move_is_either_english_menu() {
    let text = "You now have a stronger public-paper trail. From here, the most promising next move is either the serious-records route or the house itself if you want to test the paper trail.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English next-move-is-either menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_english_observation_action_list() {
    let text = "From here, the live paths remain.\n\
You connect these observations: follow the Chapel of Contemplation directly; \
change approach and try to reopen the court/police angle some other way; \
or turn to the Corbitt house itself.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English observation-framed action lists must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_connect_observations_semicolon_action_menu() {
    let text = "If you want, Evelyn can now.\n\
You connect these observations: continue her careful descent into the basement; \
retreat back up to the ground floor; or abandon the basement for now and head upstairs after withdrawing safely.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "Connect-observations semicolon action menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_english_clear_choice_target_menu() {
    let text = "From where you stand now, you have a clear choice of where to bring the light next: the disturbed earth, the scraped area, or the suspicious section that may conceal a panel.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English clear-choice target menus must be blocked even when branches are nouns"
    );
}

#[test]
fn player_agency_menu_cue_detects_english_where_mean_begin_menu() {
    let text = "He looks from the notebook back to you. So—where do you mean to begin: city records, the library, or the newspaper files?";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English where-do-you-mean-to-begin menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_english_live_directions_menu() {
    let text = "Your notes now point in three live directions, all uglier than a simple bad tenancy: the courts, the police records, or the chapel. What do you pursue next?";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English live-directions target menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_english_points_in_three_directions_menu() {
    let text = "The records reveal Reverend Michael Thomas and the chapel closure.\n\
From here, the investigation naturally points in three directions.\n\
You connect these observations: the higher courts / police records; \
the former Chapel of Contemplation; or the old Corbitt place itself. \
What does Evelyn do next?";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English points-in-three-directions target menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_paper_trail_three_promising_directions_menu() {
    let text = "From here, the paper trail now seems to point in three promising directions: the closed chapel, the courts/police records, or the house itself.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English three-promising-directions target menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_three_immediate_lines_of_pursuit_menu() {
    let text = "The municipal search gives a concrete executor and institution. So, from this office, you now have three immediate lines of pursuit. the Chapel of Contemplation, the higher courts / police records, or the house itself.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English three-immediate-lines target menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_points_in_three_clear_directions_menu() {
    let text = "You are still in the records office with your notes in hand. The new lead points in three clear directions without forcing your hand: the closed chapel, the courts and police records, or the house itself.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English three-clear-directions target menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_pursue_either_lead_menu() {
    let text = "You have enough now to pursue either lead in earnest: the site of the old chapel, or the Corbitt House itself.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English pursue-either-lead menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_next_move_can_follow_or_take_menu() {
    let text = "Your next move can follow the paper trail outward—or take you straight back to the Corbitt House with a basement now very much in mind.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English next-move-can-follow-or-take menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_do_you_have_character_or_menu() {
    let text = "Do you have Nell pry the covering farther apart, or hold here and examine the edges and lettering more closely first?";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English do-you-have-character-or action menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_english_tone_choice_menu() {
    let text = "Staff can send you toward the right desk, but actual access depends on how Evelyn handles the gatekeeper. How does she press for access to the files? as a courteous professional appeal to cooperation. as a confident argument that this is a legitimate public-history inquiry. as a quick improvised line to get waved through. or as pressure/intimidation. In plain terms: what tone does Evelyn take with the editor?";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English tone-choice menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_by_skill_tone_choice_menu() {
    let text = "A staffer can point you toward the clippings room, but access is controlled by an editor: Arty Wilmot. How does she approach Wilmot—by charm, straight persuasion, intimidation, or fast talk?";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English by-skill tone-choice menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_fragmented_lean_on_tone_choice_menu() {
    let text = "The morgue files are not open for casual browsing. Access is controlled by an editor, Arty Wilmot. Evelyn is at the point where she can make her case, but how she does it matters. Do you have her lean on. her professional manner and reasonableness. personal charm. pressure or intimidation. or a fast, slippery line to get past the gatekeeper?";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English fragmented lean-on tone-choice menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_try_to_get_in_tone_choice_menu() {
    let text = "At the Globe, the obstacle is access. An editor, Arty Wilmot, controls access. How does Evelyn try to get in? If she leans on respectability, credentials, and a reasonable request, that suggests Persuade. If she tries charm, pressure, or a quick bluff, that would point elsewhere. Tell me her approach, and I'll resolve it.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English try-to-get-in tone-choice menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_try_past_editor_tone_choice_menu() {
    let text = "The Globe's front offices are busy, and you are directed to the person who controls access: Editor Arty Wilmot. If Evelyn wants the clippings morgue opened, this is a real point of friction. How does she try to get past him? Does she lean on professional courtesy, polite persuasion, fast-talking newsroom urgency, or blunt pressure?";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English try-past-editor tone-choice menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_live_arty_access_tone_choice_menu() {
    let text = "At the Globe, the public side of the place will get Evelyn only so far. A staffer can tell her where the morgue is kept, but not simply wave her into it: access is controlled, and the name that comes back is Arty Wilmot, an editor with custody over the files. So the immediate obstacle is not yet the search itself, but getting legitimate access to the morgue. How does Evelyn approach that? If she presses politely, flatters, bluffs urgency, leans on her press credentials, or tries to bully past the gatekeeper, I'll resolve that accordingly.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English live Arty access tone-choice menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_unclosed_dialogue_quote() {
    let text = "Knott spreads his hands.\n\n“What records exist? Municipal records, certainly. Deeds, transfers, tax matters, perhaps probate.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "unclosed dialogue quotes must be blocked before delivery"
    );
}

#[test]
fn player_agency_menu_cue_detects_english_obvious_next_avenues_menu() {
    let text = "Knott leaves the keys with you and waits. The obvious next avenues are the city records offices or the newspaper files, though you could press him further.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English obvious-next-avenues target menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_obvious_next_avenues_and_list_menu() {
    let text = "The obvious next avenues are the city records offices and the newspaper files.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English obvious-next-avenues and-list target menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_english_branches_outward_target_menu() {
    let text = "The executor record ties Corbitt to Michael Thomas. From here, the line of inquiry clearly branches outward: the Chapel of Contemplation, the court/police record trail, or the house itself.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English branches-outward target menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_two_obvious_directions_and_list_menu() {
    let text = "The two obvious directions are now the Chapel of Contemplation and the Corbitt house itself.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English two-obvious-directions and-list target menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_next_pressure_points_follow_first_menu() {
    let text = "The next pressure points are clear enough in the record. the Chapel of Contemplation. higher court files. Central Police Station records. What does she follow first?";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English next-pressure-points follow-first menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_obvious_next_lines_of_inquiry_menu() {
    let text = "The obvious next lines of inquiry from here would be city records, deed or court records, and old newspaper files.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English obvious-next-lines-of-inquiry menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_if_want_press_or_paper_trail_menu() {
    let text = "He seems ready to answer a few more questions if you want to press him further, or you can set off on the paper trail first.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English if-want-press-or-paper-trail menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_next_solid_leads_menu() {
    let text = "The next solid leads from here would be the records offices or newspaper files.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English next-solid-leads target menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_english_pressure_points_menu() {
    let text = "Evelyn ends this pass with better orientation and a clearer choice of pressure points in the house: continue upstairs, examine the way down more closely, or slow even further over specific papers.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English pressure-points target menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_english_paper_trail_continue_menu() {
    let text = "A clerk can tell you where the paper trail might continue if you want to press it further: higher courts, police records, or the chapel itself. What does Evelyn do next?";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English paper-trail target menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_follow_this_outward_menu() {
    let text = "You can follow this outward from here—toward the chapel, toward newspaper files about the 1912 raid, or toward the Corbitt House itself.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "follow-this-outward target menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_back_out_or_commit_farther_menu() {
    let text = "Nothing immediately tries to trap you. You can back out now exactly as planned, or commit farther in if you want a better look.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "back-out-or-commit-farther action menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_obvious_lines_pursue_first_menu() {
    let text = "From here, the obvious lines of inquiry are now established in your notes: the Globe's files, the central library, the hall of records, and possibly court or police records if the paper trail points that way. What does Evelyn pursue first?";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "obvious-lines pursue-first target menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_english_will_you_start_or_more_menu() {
    let text = "He looks to you expectantly, anxious but practical. “So—will you start with the records, or is there something more you want from me here before you go?”";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "will-you-start-or-more binary menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_english_will_you_begin_or_menu() {
    let text = "So—will you begin with the records, the newspapers, or by speaking to people in the neighborhood?";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "will-you-begin-or target menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_offers_more_avenues_menu() {
    let text = "The record room offers more avenues now if you want them: the chapel itself, higher court records and police files, or the old house once you think you have enough in hand.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "offers-more-avenues target menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_english_you_can_start_with_menu() {
    let text = "The keys and address are now in front of you. From here, you can start with public records, newspaper archives, or press him a little harder while he is still here.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English you-can-start-with action menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_english_character_can_action_sequence_menu() {
    let text = "You are on the upper landing with one suspicious bedroom identified. From here, Evelyn can keep examining that room from the threshold, probe specific furniture or papers with the walking stick, or withdraw and change floors while the retreat path remains clean.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English character-can action sequence menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_english_choice_of_whether_action_menu() {
    let text = "From where she stands now, she has the basement access under close inspection, the ground-floor route back still open, and the choice of whether to open it, pull back, or shift attention elsewhere on this floor.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "English choice-of-whether action menus must be blocked"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_english_clear_choice_target_menu() {
    let original = "The basement search exposes disturbed earth, scrape marks, and a suspicious concealed section.\n\n\
From where you stand now, you have a clear choice of where to bring the light next: the disturbed earth, the scraped area, or the suspicious section that may conceal a panel.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("fact prose with an English clear-choice target menu tail should be repairable");

    assert!(repaired.contains("disturbed earth"));
    assert!(!repaired.contains("clear choice"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repair must strip the target-menu tail while preserving facts: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_english_what_now_inline_menu() {
    let original = "The doorway is open a handspan. Stale air leaks out, and the first boards inside look dusty but still.\n\n\
What does Evelyn do now? Step inside, widen the opening a little, inspect the entry from where she is, or hold position longer?";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("fact prose with an English what-now inline menu tail should be repairable");

    assert!(repaired.contains("doorway is open"));
    assert!(!repaired.contains("What does Evelyn do now"));
    assert!(!repaired.contains("Step inside"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repair must strip the what-now menu tail while preserving facts: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_english_points_in_three_directions_menu() {
    let original = "The records reveal Reverend Michael Thomas and the chapel closure. The file gives you a date and a church connection.\n\
From here, the investigation naturally points in three directions. You connect these observations: the higher courts / police records; the former Chapel of Contemplation; or the old Corbitt place itself. What does Evelyn do next?";

    let repaired = deterministic_player_agency_menu_tail_repair(original).expect(
        "fact prose with an English three-directions target menu tail should be repairable",
    );

    assert!(repaired.contains("Reverend Michael Thomas"));
    assert!(!repaired.contains("points in three directions"));
    assert!(!repaired.contains("What does Evelyn do next"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repair must strip the target-menu tail while preserving facts: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_three_promising_directions_tail() {
    let original = "The records reveal Reverend Michael Thomas and the Chapel of Contemplation closure in 1912.\n\n\
From here, the paper trail now seems to point in three promising directions: the closed chapel, the courts/police records, or the house itself.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("three-promising-directions menu tail should be removed");

    assert!(repaired.contains("Reverend Michael Thomas"));
    assert!(!repaired.contains("three promising directions"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repair must strip the target-menu tail while preserving facts: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_three_immediate_lines_tail() {
    let original = "The search pays off cleanly. Executor: Reverend Michael Thomas. Institution tied to the estate: the Chapel of Contemplation.\n\n\
So, from this office, you now have three immediate lines of pursuit. the Chapel of Contemplation, the higher courts / police records, or the house itself.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("three-immediate-lines menu tail should be removed");

    assert!(repaired.contains("Reverend Michael Thomas"));
    assert!(repaired.contains("Chapel of Contemplation"));
    assert!(!repaired.contains("three immediate lines of pursuit"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repair must strip the immediate-lines menu tail while preserving facts: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_current_coc_menu_tails() {
    let samples = [
        (
            "The records reveal Reverend Michael Thomas and the chapel closure.\n\n\
You are still in the records office with your notes in hand. The new lead points in three clear directions without forcing your hand: the closed chapel, the courts and police records, or the house itself.",
            "Reverend Michael Thomas",
            "points in three clear directions",
        ),
        (
            "The restricted file confirms the 1912 raid and Michael Thomas's escape.\n\n\
You have enough now to pursue either lead in earnest: the site of the old chapel, or the Corbitt House itself.",
            "Michael Thomas",
            "pursue either lead",
        ),
        (
            "The chapel record states that Walter Corbitt was buried in the basement of his own house.\n\n\
Your next move can follow the paper trail outward—or take you straight back to the Corbitt House with a basement now very much in mind.",
            "Walter Corbitt",
            "next move can follow",
        ),
        (
            "The concealed basement section bears the same chapel name already tied to Corbitt and Reverend Michael Thomas.\n\n\
Do you have Nell pry the covering farther apart, or hold here and examine the edges and lettering more closely first?",
            "concealed basement section",
            "Do you have Nell",
        ),
    ];

    for (original, kept_fact, forbidden_tail) in samples {
        let repaired = deterministic_player_agency_menu_tail_repair(original)
            .expect("current CoC action menu tail should be repairable");

        assert!(
            repaired.contains(kept_fact),
            "repair must preserve committed fact `{kept_fact}`: {repaired}"
        );
        assert!(
            !repaired.contains(forbidden_tail),
            "repair must remove menu tail `{forbidden_tail}`: {repaired}"
        );
        assert!(
            !player_agency_menu_cue_needs_block(&repaired),
            "repair must pass final menu gate: {repaired}"
        );
    }
}

#[test]
fn player_agency_menu_cue_detects_chinese_you_can_also_menu() {
    let text = "你从柜台前退开时，手上的笔记已经比来时清楚得多：这宅子的历史不是单纯的邻里流言，它确实在官方记录里留下了可追索的痕迹。你接下来要往哪一条先压下去？你可以继续先查民事档案，也可以直奔法院/警方案卷。";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "Chinese '你可以...也可以...' next-action menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_chinese_you_can_or_direct_tail_without_next_frame() {
    let text = "Knott先生没有把门关死，但也不肯再多吐细节。你可以接着换一种方式追问，或者直接拿着钥匙和地址，转去你认为最先该查的地方。";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "Chinese '你可以...或者直接...' action tails must be blocked even without 接下来/下一步 framing"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_chinese_you_can_or_direct_tail() {
    let original = "Knott先生没有把门关死，但也不肯再多吐细节。\n\n\
你可以接着换一种方式追问，或者直接拿着钥匙和地址，转去你认为最先该查的地方。";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("Chinese you-can/or-direct menu tail should be removed");

    assert!(repaired.contains("Knott先生没有把门关死"), "{repaired}");
    assert!(!repaired.contains("你可以接着"), "{repaired}");
    assert!(!repaired.contains("或者直接"), "{repaired}");
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repair must pass final menu gate: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_chinese_you_can_also_menu() {
    let original = "你从柜台前退开时，手上的笔记已经比来时清楚得多：这宅子的历史不是单纯的邻里流言，它确实在官方记录里留下了可追索的痕迹。\n\n\
你接下来要往哪一条先压下去？你可以继续先查民事档案，也可以直奔法院/警方案卷。";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("Chinese you-can/also-can menu tail should be removed");

    assert!(repaired.contains("官方记录"), "{repaired}");
    assert!(!repaired.contains("你接下来要往哪一条"), "{repaired}");
    assert!(!repaired.contains("你可以继续"), "{repaired}");
    assert!(!repaired.contains("也可以直奔"), "{repaired}");
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repair must pass final menu gate: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_unclosed_dialogue_paragraph() {
    let original = "Knott confirms the keys and gives the usable address lead.\n\n\
Knott spreads his hands.\n\n\
“What records exist? Municipal records, certainly. Deeds, transfers, tax matters, perhaps probate.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("unclosed dialogue paragraph should be removable");

    assert!(repaired.contains("usable address lead"));
    assert!(!repaired.contains("What records exist?"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repair must strip incomplete dialogue while preserving prior facts: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_english_obvious_next_avenues_menu() {
    let original = "Knott confirms the address lead and the uncertain keys. The obvious next avenues are the city records offices or the newspaper files, though you could press him further.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("fact prose with an English obvious-next-avenues tail should be repairable");

    assert!(repaired.contains("address lead"));
    assert!(!repaired.contains("obvious next avenues"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repair must strip the target-menu tail while preserving facts: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_two_obvious_directions_and_list_tail() {
    let original = "The 1912 court and police file links the chapel to disappeared children, dead police, dead cultists, a cover-up, and Reverend Michael Thomas escaping prison.\n\n\
The two obvious directions are now the Chapel of Contemplation and the Corbitt house itself.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("two-obvious-directions and-list menu tail should be removed");

    assert!(repaired.contains("Reverend Michael Thomas"));
    assert!(!repaired.contains("two obvious directions"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repair must strip the target-menu tail while preserving facts: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_next_pressure_points_follow_first_tail() {
    let original = "Corbitt's executor was Reverend Michael Thomas of the Chapel of Contemplation, and the chapel itself closed in 1912.\n\n\
The next pressure points are clear enough in the record. the Chapel of Contemplation. higher court files. Central Police Station records. What does she follow first?";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("next-pressure-points follow-first menu tail should be removed");

    assert!(repaired.contains("Reverend Michael Thomas"));
    assert!(!repaired.contains("next pressure points"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repair must strip the target-menu tail while preserving facts: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_obvious_next_lines_sentence_preserving_source_note()
{
    let original = "Knott confirms research first is sensible. The obvious next lines of inquiry from here would be city records, deed or court records, and old newspaper files. In Evelyn's notes, this remains a usable address lead for navigation and records work, not a literal street-number line she can quote.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("embedded obvious-next-lines menu sentence should be removed");

    assert!(repaired.contains("Knott confirms research first"));
    assert!(repaired.contains("In Evelyn's notes"));
    assert!(!repaired.contains("obvious next lines of inquiry"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repair must remove only the menu sentence while preserving source note: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_if_want_press_or_paper_trail_sentence_preserving_source_note(
) {
    let original = "Knott confirms research first is sensible. He seems ready to answer a few more questions if you want to press him further, or you can set off on the paper trail first. In Evelyn's notes, this remains a usable address lead for navigation and records work, not a literal street-number line she can quote.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("embedded press-or-paper-trail menu sentence should be removed");

    assert!(repaired.contains("Knott confirms research first"));
    assert!(repaired.contains("In Evelyn's notes"));
    assert!(!repaired.contains("press him further"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repair must remove only the menu sentence while preserving source note: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_next_solid_leads_sentence_preserving_source_note() {
    let original = "Knott confirms research first is sensible. The next solid leads from here would be the records offices or newspaper files. In Evelyn's notes, this remains a usable address lead for navigation and records work, not a literal street-number line she can quote.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("embedded next-solid-leads menu sentence should be removed");

    assert!(repaired.contains("Knott confirms research first"));
    assert!(repaired.contains("In Evelyn's notes"));
    assert!(!repaired.contains("next solid leads"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repair must remove only the menu sentence while preserving source note: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_english_paper_trail_continue_menu() {
    let original = "Corbitt's executor was Reverend Michael Thomas of the Chapel of Contemplation, and the chapel itself closed in 1912. A clerk can tell you where the paper trail might continue if you want to press it further: higher courts, police records, or the chapel itself. What does Evelyn do next?";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("fact prose with an English paper-trail target menu tail should be repairable");

    assert!(repaired.contains("Reverend Michael Thomas"));
    assert!(!repaired.contains("paper trail might continue"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repair must strip the target-menu tail while preserving facts: {repaired}"
    );
}

#[test]
fn player_agency_menu_cue_detects_fragmented_paper_trail_direction_menu() {
    let text = "Legal disputes / public filings tied to the address: no specific filing can be confidently extracted on this attempt. Macario index: no useful named entry turns up in the first run of the public books you check. the higher courts / serious legal records. the Central Police Station. or leave the paper trail for the moment and go to the Corbitt House itself. A courteous clerk, seeing that you at least know how to ask the right questions, leans in and offers practical direction rather than facts. You can follow this by turning next toward.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "fragmented paper-trail direction lists are still action menus"
    );
}

#[test]
fn player_agency_menu_cue_detects_failed_records_chapel_house_direction_menu() {
    let text = "The Hall of Records search is inconclusive: no clean deed citation, no probate docket, and no named executor entry. The most promising next channels appear to be the courts, the police records, or leaving paper behind for the moment and going to the Chapel of Contemplation or the house itself.";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "failed-records courts/police/chapel/house direction lists are still action menus"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_fragmented_paper_trail_direction_menu() {
    let original = "The Hall search is maddeningly close. Property title / former owners: you do not yet pull a clean chain of ownership. Probate / executors: nothing certain emerges from this pass. Legal disputes / public filings tied to the address: no specific filing can be confidently extracted on this attempt. Macario index: no useful named entry turns up in the first run of the public books you check. the higher courts / serious legal records. the Central Police Station. or leave the paper trail for the moment and go to the Corbitt House itself. A courteous clerk, seeing that you at least know how to ask the right questions, leans in and offers practical direction rather than facts. \"If you're chasing anything serious, you may need the higher courts and the Central Police Station.\"";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("fragmented paper-trail target menu should be repairable");

    assert!(repaired.contains("The Hall search is maddeningly close"));
    assert!(repaired.contains("A courteous clerk"));
    assert!(repaired.contains("higher courts and the Central Police Station"));
    assert!(!repaired.contains("or leave the paper trail"));
    assert!(!repaired.contains("turning next toward"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repair must remove the menu fragment while preserving clerk facts: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_failed_records_chapel_house_direction_menu() {
    let original = "The Hall of Records search is inconclusive: no clean deed citation, no probate docket, and no named executor entry. The most promising next channels appear to be the courts, the police records, or leaving paper behind for the moment and going to the Chapel of Contemplation or the house itself.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("failed-records Chapel/house target menu should be repairable");

    assert!(repaired.contains("The Hall of Records search is inconclusive"));
    assert!(!repaired.contains("most promising next channels"));
    assert!(!repaired.contains("Chapel of Contemplation or the house itself"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repair must strip the target-menu tail while preserving no-hit facts: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_english_pressure_points_menu() {
    let original = "The ground floor search exposes cellar taint and a usable upstairs route. Evelyn ends this pass with better orientation and a clearer choice of pressure points in the house: continue upstairs, examine the way down more closely, or slow even further over specific papers.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("fact prose with an English pressure-points target menu tail should be repairable");

    assert!(repaired.contains("cellar taint"));
    assert!(!repaired.contains("pressure points"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repair must strip the pressure-points menu tail while preserving facts: {repaired}"
    );
}

#[test]
fn player_agency_menu_cue_detects_chinese_ordinal_direction_menu() {
    let text = "Hall of Records 里还有几条自然延伸出的去处：\n\
一是继续顺着 Reverend Michael Thomas 往下查，\n\
二是转去 Higher Courts 或 Central Police Station，\n\
三是带着这条新线索直接去 Corbitt House。";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "Chinese ordinal direction menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_chinese_where_start_or_menu() {
    let text = "你想先从档案馆还是报社下手，都行。等你查到些东西，再决定什么时候进去看房子。";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "Chinese where-to-start-or menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_chinese_decide_is_or_menu() {
    let text = "你现在得决定，是在这个不稳的位置继续冒一点险再往下试，还是先换办法。";
    assert!(
        player_agency_menu_cue_needs_block(text),
        "Chinese decide-is-or action menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_chinese_binary_path_menu() {
    let text = "你把这些内容补进本子时，能很清楚地感觉到，下一步已经不只是“继续查资料”那么简单了。现在最扎手的两条路都摆在你面前：\n\n\
一条，是顺着这份警局/法院记录，继续深挖那次1912年突袭的细节；\n另一条，则是拿着这枚钉死了的线索，直接去找沉思礼拜堂，甚至回到科宾老宅。\n\n\
你接下来想往哪边压？";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "Chinese binary path menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_chinese_repeated_path_action_menu() {
    let text = "你现在至少已经把当前安全回撤点和下一步三条主方向分开了。一条是回头继续啃起居房。一条是往楼上。一条是往地下室。";
    assert!(
        player_agency_menu_cue_needs_block(text),
        "Chinese repeated path action menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_chinese_either_or_action_menu() {
    let text = "你接下来要么继续从这扇门推进，要么改去楼梯方向，或者换个更稳妥的探查办法。";
    assert!(
        player_agency_menu_cue_needs_block(text),
        "Chinese either/or action menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_chinese_yaome_failure_branch_menu() {
    let text = "你眼下还能做的，不少，但都得换个办法：\n\
要么继续设法打通这边的人情或权限，\n\
要么转去中央图书馆，\n\
要么去市政档案馆，\n\
要么干脆先去科比特老宅外头看看周边情形。";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "Chinese 要么/要么 failure-branch action menus must be blocked"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_chinese_either_or_tail() {
    let original = "你把这些观察串在一起：退路记号已经做好，门后撤离方向明确；前厅近处仍可立足；灰尘里有受扰痕迹，但不足以可靠指向某一条明确路线；最近这扇内门后不像立刻就是致命陷阱，但门后的更深处仍未看透。\n\
你接下来要么继续从这扇门推进，要么改去楼梯方向，或者换个更稳妥的探查办法。";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("either/or action menu tail should be repairable");

    assert!(repaired.contains("退路记号已经做好"), "{repaired}");
    assert!(!repaired.contains("你接下来要么"), "{repaired}");
    assert!(!repaired.contains("要么改去"), "{repaired}");
    assert!(!player_agency_menu_cue_needs_block(&repaired), "{repaired}");
}

#[test]
fn deterministic_action_menu_tail_repair_drops_chinese_yaome_failure_branch_tail() {
    let original = "编辑没有失礼，但把门关在规章这一边：现在不给进地下剪报室，也不让人替你把那一摞旧档搬上来。\n\n\
你眼下还能做的，不少，但都得换个办法：\n\
要么继续设法打通这边的人情或权限，\n\
要么转去中央图书馆，\n\
要么去市政档案馆，\n\
要么干脆先去科比特老宅外头看看周边情形。";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("Chinese 要么/要么 failure-branch menu tail should be repairable");

    assert!(repaired.contains("不给进地下剪报室"), "{repaired}");
    assert!(!repaired.contains("你眼下还能做的"), "{repaired}");
    assert!(!repaired.contains("要么转去"), "{repaired}");
    assert!(!player_agency_menu_cue_needs_block(&repaired), "{repaired}");
}

#[test]
fn deterministic_action_menu_tail_repair_drops_chinese_binary_path_tail() {
    let original = "警方旧档确认，1912年的沉思礼拜堂突袭与儿童失踪证词有关，行动中警员和教徒均有死亡，Michael Thomas 曾入狱后逃脱。\n\n\
你把这些内容补进本子时，能很清楚地感觉到，下一步已经不只是“继续查资料”那么简单了。现在最扎手的两条路都摆在你面前：\n\n\
一条，是顺着这份警局/法院记录，继续深挖那次1912年突袭的细节；\n另一条，则是拿着这枚钉死了的线索，直接去找沉思礼拜堂，甚至回到科宾老宅。\n\n\
你接下来想往哪边压？";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("binary path menu tail should be repairable");

    assert!(repaired.contains("Michael Thomas"), "{repaired}");
    assert!(!repaired.contains("两条路"), "{repaired}");
    assert!(!repaired.contains("一条，是"), "{repaired}");
    assert!(!repaired.contains("另一条"), "{repaired}");
    assert!(!player_agency_menu_cue_needs_block(&repaired), "{repaired}");
}

#[test]
fn player_agency_menu_cue_allows_open_ended_prompt_without_options() {
    assert!(
        !player_agency_menu_cue_needs_block("你接下来怎么追？"),
        "a bare open-ended prompt is not an action menu"
    );
}

#[test]
fn player_agency_menu_cue_detects_chinese_biru_multi_action_tail() {
    let text = "现在你仍站在外头，位置安全，退路也在。下一步如果还想把情况查实，恐怕就得把距离再拉近一点——比如沿墙根换角度看门窗细节，试着绕到别的一侧，找邻近的人搭话，或者干脆准备进去。";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "Chinese 比如 + multiple action follow-up menus must be blocked"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_chinese_biru_multi_action_tail() {
    let original = "沉思礼拜堂旧址看上去败坏已久，但还不能确认是否只是空壳。你仍站在街边，退路清楚。\n\n\
现在你仍站在外头，位置安全，退路也在。下一步如果还想把情况查实，恐怕就得把距离再拉近一点——比如沿墙根换角度看门窗细节，试着绕到别的一侧，找邻近的人搭话，或者干脆准备进去。";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("multi-action 比如 tail should be repairable");

    assert!(repaired.contains("沉思礼拜堂旧址"), "{repaired}");
    assert!(!repaired.contains("比如沿墙根"), "{repaired}");
    assert!(!repaired.contains("或者干脆"), "{repaired}");
    assert!(!player_agency_menu_cue_needs_block(&repaired), "{repaired}");
}

#[test]
fn player_agency_menu_cue_detects_chinese_soft_next_action_menu_tail() {
    let text = "如果你要继续，下一步自然会是更贴近建筑本身去看入口、窗边、外墙裂缝，或者绕着外围寻找可能还能进入、或至少能窥见内部状况的地方。";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "Chinese soft next-step action menus must be blocked"
    );
}

#[test]
fn player_agency_menu_cue_detects_chinese_natural_direction_action_menu_tail() {
    let text = "接下来你若继续，比较自然的方向会是更细地处理壁炉本身，或者转回去查你先前记下同样可疑的桌面一带。";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "Chinese natural-direction action menus must be blocked"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_chinese_soft_next_action_tail() {
    let original = "你现在就在礼拜堂旧址外近处，手里已有几张外景照片和初步印象，但还没抓到足够硬的细节。\n\n\
如果你要继续，下一步自然会是更贴近建筑本身去看入口、窗边、外墙裂缝，或者绕着外围寻找可能还能进入、或至少能窥见内部状况的地方。";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("soft next-step action menu tail should be repairable");

    assert!(repaired.contains("礼拜堂旧址外近处"), "{repaired}");
    assert!(!repaired.contains("下一步自然会是"), "{repaired}");
    assert!(!repaired.contains("或者绕着外围"), "{repaired}");
    assert!(!player_agency_menu_cue_needs_block(&repaired), "{repaired}");
}

#[test]
fn deterministic_dangling_colon_tail_repair_closes_incomplete_chinese_tail() {
    let original = "你还是拍了几张照片，把整栋房子的外观、正面门窗和街面关系先收进胶卷里。\n\n\
现在，你仍在街对面，手里有一组外景照片和一个更明确的判断：";

    let repaired = deterministic_dangling_colon_tail_repair(original)
        .expect("dangling colon tail should be repairable");

    assert!(repaired.contains("外景照片"), "{repaired}");
    assert!(!repaired.ends_with('：'), "{repaired}");
    assert!(repaired.ends_with('。'), "{repaired}");
}

#[test]
fn player_agency_menu_cue_detects_chinese_lead_branch_action_tail() {
    let text = "于是，你手里已经握住了一条明确的线索：顺着沉思礼拜堂与迈克尔·托马斯牧师继续深挖，或者转去高等法院和中央警署，寻找更完整的案卷。";

    assert!(
        player_agency_menu_cue_needs_block(text),
        "Chinese lead/action branch tails must be blocked"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_chinese_lead_branch_action_tail() {
    let original = "科宾宅的民事记录明确写着，科宾的遗嘱执行人是沉思礼拜堂的迈克尔·托马斯牧师；另一条记录显示，这座沉思礼拜堂早在1912年就已经关闭。\n\n\
于是，你手里已经握住了一条明确的线索：顺着沉思礼拜堂与迈克尔·托马斯牧师继续深挖，或者转去高等法院和中央警署，寻找更完整的案卷。";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("Chinese lead/action branch tail should be repairable");

    assert!(repaired.contains("迈克尔·托马斯牧师"), "{repaired}");
    assert!(repaired.contains("1912年"), "{repaired}");
    assert!(!repaired.contains("或者转去"), "{repaired}");
    assert!(!player_agency_menu_cue_needs_block(&repaired), "{repaired}");
}

#[test]
fn deterministic_action_menu_tail_repair_drops_whether_you_or_tail_only() {
    let original = "Knott confirms the keys and approves beginning with public records.\n\n\
Knott falls quiet after that, watching to see whether you press him further here or head out to start with the records.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("fact prose with a trailing action-menu sentence should be repairable");

    assert!(repaired.contains("Knott confirms the keys"));
    assert!(!repaired.contains("whether you press"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repair must leave clean player-visible prose: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_where_does_character_go_next_tail() {
    let original = "The file gives you the darker line of inquiry.\n\n\
Where does Evelyn go next? The chapel itself, a newspaper archive to deepen the 1912 story, or the Corbitt house with this new context in hand?";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("where-next menu tail should be removed");

    assert!(repaired.contains("darker line of inquiry"));
    assert!(!repaired.contains("Where does Evelyn go next"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repaired narration must no longer trigger the action-menu gate: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_choose_where_character_goes_first_tail() {
    let original = "Knott agrees that records and newspaper files are sensible before entering the house.\n\n\
From here, the obvious lines of inquiry are the public record offices and the newspaper files. If you want, you can choose where Evelyn goes first.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("choose-where-goes-first menu tail should be removed");

    assert!(repaired.contains("Knott agrees"));
    assert!(!repaired.contains("choose where"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repaired narration must no longer trigger the action-menu gate: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_obvious_next_avenues_and_list_tail() {
    let original = "Knott confirms the usable address lead, the uncertain keys, and that public records are sensible.\n\n\
The obvious next avenues are the city records offices and the newspaper files.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("obvious-next-avenues and-list menu tail should be removed");

    assert!(repaired.contains("Knott confirms"));
    assert!(!repaired.contains("obvious next avenues"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repair must strip the target-menu tail while preserving facts: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_will_you_start_or_more_tail() {
    let original = "Knott confirms the usable address lead, the uncertain key ring, and the public record path.\n\n\
“So—will you start with the records, or is there something more you want from me here before you go?”\n\n\
In Evelyn's notes, this remains a usable address lead for navigation and records work, not a literal street-number line she can quote.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("will-you-start binary menu tail should be removed");

    assert!(repaired.contains("Knott confirms"));
    assert!(repaired.contains("In Evelyn's notes"));
    assert!(!repaired.contains("will you start"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repaired narration must preserve facts and remove the menu: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_will_you_begin_or_tail() {
    let original = "Knott gives Evelyn the keys and confirms that public records and newspapers may help her ground the case.\n\n\
So—will you begin with the records, the newspapers, or by speaking to people in the neighborhood?";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("will-you-begin target menu tail should be removed");

    assert!(repaired.contains("Knott gives Evelyn the keys"));
    assert!(!repaired.contains("will you begin"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repaired narration must preserve facts and remove the menu: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_offers_more_avenues_tail() {
    let original = "The Hall of Records search reveals Reverend Michael Thomas of the Chapel of Contemplation, and that the chapel closed in 1912.\n\n\
The record room offers more avenues now if you want them: the chapel itself, higher court records and police files, or the old house once you think you have enough in hand.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("offers-more-avenues menu tail should be removed");

    assert!(repaired.contains("Reverend Michael Thomas"));
    assert!(repaired.contains("closed in 1912"));
    assert!(!repaired.contains("offers more avenues"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repaired narration must preserve facts and remove the menu: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_what_does_character_examine_next_tail() {
    let original = "You have the ground floor broadly mapped enough to proceed with intent.\n\n\
What does Evelyn examine next—upstairs, a specific room or door on this floor, or the cellar access from above without descending?";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("what-next menu tail should be removed");

    assert!(repaired.contains("ground floor broadly mapped"));
    assert!(!repaired.contains("What does Evelyn examine next"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repaired narration must no longer trigger the action-menu gate: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_what_does_character_do_first_dash_tail() {
    let original = "You are back at the crawl-space route with light, line, and tools.\n\n\
What does Evelyn do first—check the crawl-space mouth closely before entering, rig the line and light, start prying at the broken boards, or go straight in?";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("what-do-first dash menu tail should be removed");

    assert!(repaired.contains("light, line, and tools"));
    assert!(!repaired.contains("What does Evelyn do first"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repaired narration must no longer trigger the action-menu gate: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_which_try_first_tail() {
    let original = "Knott gives you the address, the keys, and a sensible public-records lead.\n\n\
So, which will you try first—the records, or the papers?";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("which-try-first option tail should be removed");

    assert!(repaired.contains("address"));
    assert!(!repaired.contains("which will you try first"));
    assert!(!repaired.contains("the records, or the papers"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repair must strip the option tail while preserving facts: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_focus_on_first_target_menu() {
    let original = "You are now on the upper floor landing, checking rooms one by one from the doorways, with the stairs still open behind you.\n\n\
What does Evelyn focus on first: the bed and bedding, the wardrobe, the papers, the windows, or the marks on the walls?";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("focus-on-first target menu tail should be removed");

    assert!(repaired.contains("upper floor landing"));
    assert!(!repaired.contains("focus on first"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repaired narration must no longer trigger the action-menu gate: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_you_can_action_sequence_tail() {
    let original = "The search does not yield the precise thread you wanted today.\n\n\
You can keep working this office from a different angle, shift to another archive, or leave for a more direct lead.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("you-can action sequence tail should be removed");

    assert!(repaired.contains("search does not yield"));
    assert!(!repaired.contains("You can keep"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repaired narration must no longer trigger the action-menu gate: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_do_you_action_sequence_tail() {
    let original = "More importantly for movement, the house confirms its internal routes: the stairs up are usable, and there is also a way down toward the basement.\n\n\
Do you continue the ground-floor sweep in more detail, go upstairs, or turn your attention to the way down?";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("do-you action sequence tail should be removed");

    assert!(repaired.contains("internal routes"));
    assert!(!repaired.contains("Do you continue"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repaired narration must no longer trigger the action-menu gate: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_you_may_action_sequence_tail() {
    let original = "The threshold remains before you, stale and still.\n\n\
From here, with your exit route still behind you, you may go in, inspect another entrance first, or work the neighborhood before crossing the threshold.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("you-may action sequence tail should be removed");

    assert!(repaired.contains("threshold remains"));
    assert!(!repaired.contains("you may go in"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repaired narration must no longer trigger the action-menu gate: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_next_move_is_either_tail() {
    let original = "You now have a stronger public-paper trail pointing away from rumor and toward official trouble records.\n\n\
From here, the most promising next move is either the serious-records route or the house itself if you want to test the paper trail.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("next-move-is-either tail should be removed");

    assert!(repaired.contains("public-paper trail"));
    assert!(!repaired.contains("next move is either"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repaired narration must no longer trigger the action-menu gate: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_connect_observations_action_menu() {
    let original = "Evelyn is partway down the basement stairs, with the marked retreat behind her.\n\n\
If you want, Evelyn can now.\n\
You connect these observations: continue her careful descent into the basement; \
retreat back up to the ground floor; or abandon the basement for now and head upstairs after withdrawing safely.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("connect-observations action menu tail should be removed");

    assert!(repaired.contains("partway down the basement stairs"));
    assert!(!repaired.contains("continue her careful descent"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repaired narration must no longer trigger the action-menu gate: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_if_you_want_commit_probe_menu() {
    let original = "The bedroom remains quiet from the threshold, and the papers are visible near the bed.\n\n\
If you want, Evelyn can now commit to one specific next probe from the threshold—bed, wardrobe, papers, or window—or step in farther and accept the extra risk.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("if-you-want commit-probe menu tail should be removed");

    assert!(repaired.contains("papers are visible near the bed"));
    assert!(!repaired.contains("commit to one specific next probe"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repaired narration must no longer trigger the action-menu gate: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_if_you_choose_you_can_or_menu() {
    let original = "The basement contains a hidden-looking, boarded-off under-space.\n\n\
So the concrete affordance is not go deeper in the abstract. It is this: if you choose, you can approach that boarded section and examine it directly as the next meaningful point of contact. Or you can hold where you are with the stairs at your back.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("if-you-choose you-can/or-you-can menu tail should be removed");

    assert!(repaired.contains("boarded-off under-space"));
    assert!(!repaired.contains("if you choose"));
    assert!(!repaired.contains("Or you can hold"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repaired narration must no longer trigger the action-menu gate: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_you_can_start_with_tail() {
    let original = "Knott gives you the address and the house keys.\n\n\
From here, you can start with public records, newspaper archives, or press him a little harder while he is still here.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("you-can-start-with action sequence tail should be removed");

    assert!(repaired.contains("address"));
    assert!(repaired.contains("house keys"));
    assert!(!repaired.contains("start with public records"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repaired narration must no longer trigger the action-menu gate: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_character_can_action_sequence_tail() {
    let original = "You are on the upper landing with one suspicious bedroom identified.\n\n\
From here, Evelyn can keep examining that room from the threshold, probe specific furniture or papers with the walking stick, or withdraw and change floors while the retreat path remains clean.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("character-can action sequence tail should be removed");

    assert!(repaired.contains("suspicious bedroom"));
    assert!(!repaired.contains("Evelyn can keep"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repaired narration must no longer trigger the action-menu gate: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_if_you_want_target_first_tail_without_can() {
    let original = "From the landing side of the doorway, Evelyn keeps the room at stick's length.\n\n\
If you want, keep pressing this same doorway method on one specific target first—the bed, the wardrobe, the window, or the papers—or withdraw and shift to another upstairs doorway before committing further.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("if-you-want target-list tail should be removed");

    assert!(repaired.contains("stick's length"));
    assert!(!repaired.contains("keep pressing this same doorway method"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repaired narration must no longer trigger the action-menu gate: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_next_meaningful_move_tail() {
    let original = "No one immediately calls out to her, and nothing on the exterior openly breaks that uneasy stillness.\n\n\
From where she stands now, with an exit route still open behind her, the next meaningful move is hers: commit to that door, shift to another entrance, study a particular window or cellar access more closely, or canvass the nearby houses before going in.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("next-meaningful-move action-list tail should be removed");

    assert!(repaired.contains("uneasy stillness"));
    assert!(!repaired.contains("next meaningful move"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repaired narration must no longer trigger the action-menu gate: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_open_enough_now_to_tail() {
    let original = "Nothing at the doorway itself lunges, falls, or gives way.\n\n\
The entrance is open enough now to listen longer, widen the gap, or make a first careful step inside if you choose.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("open-enough-now-to action-list tail should be removed");

    assert!(repaired.contains("doorway itself"));
    assert!(!repaired.contains("open enough now"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repaired narration must no longer trigger the action-menu gate: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_choice_of_whether_action_tail() {
    let original = "The basement access is under close inspection and the ground-floor route back is open.\n\n\
From where she stands now, she has the choice of whether to open it, pull back, or shift attention elsewhere on this floor.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("choice-of-whether action sequence tail should be removed");

    assert!(repaired.contains("basement access"));
    assert!(!repaired.contains("choice of whether"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repaired narration must no longer trigger the action-menu gate: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_menu_paragraph_inside_prose() {
    let original = "Knott agrees that research first is sensible.\n\n\
“你想先从档案馆还是报社下手，都行。等你查到些东西，再决定什么时候进去看房子。”\n\n\
你面前现在有了明确的线头：钥匙、地址线索，以及 Knott 对先研究再进屋的赞成。";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("fact prose with an embedded action-menu paragraph should be repairable");

    assert!(repaired.contains("Knott agrees"));
    assert!(repaired.contains("明确的线头"));
    assert!(!repaired.contains("档案馆还是报社"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repair must leave clean player-visible prose: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_english_tone_choice_menu() {
    let original = "At the Globe, the public room is easy enough to reach; the archive itself is not. Staff can send you toward the right desk, but actual access depends on how Evelyn handles the gatekeeper.\n\n\
How does she press for access to the files? as a courteous professional appeal to cooperation. as a confident argument that this is a legitimate public-history inquiry. as a quick improvised line to get waved through. or as pressure/intimidation. In plain terms: what tone does Evelyn take with the editor?";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("tone-choice menu tail should be removed");

    assert!(repaired.contains("At the Globe"));
    assert!(!repaired.contains("what tone"));
    assert!(!repaired.contains("pressure/intimidation"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repaired narration must no longer trigger the action-menu gate: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_by_skill_tone_choice_menu() {
    let original = "At the Globe, a staffer can point you toward the clippings room, but access is controlled by an editor: Arty Wilmot. How does she approach Wilmot—by charm, straight persuasion, intimidation, or fast talk?";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("by-skill tone-choice menu tail should be removed");

    assert!(repaired.contains("At the Globe"));
    assert!(!repaired.contains("How does she approach"));
    assert!(!repaired.contains("fast talk"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repaired narration must no longer trigger the action-menu gate: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_try_to_get_in_tone_choice_menu() {
    let original = "At the Globe, the obstacle is not the searching itself yet — it is access.\n\nInk, paper dust, and the hot thrum of machinery fill the building, and staff can point Evelyn toward the basement clippings morgue, but the files are not simply open for a casual walk-in. An editor, Arty Wilmot, controls access.\n\nHow does Evelyn try to get in?\n\nIf she leans on respectability, credentials, and a reasonable request, that suggests Persuade.\nIf she tries charm, pressure, or a quick bluff, that would point elsewhere.\n\nTell me her approach, and I'll resolve it.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("try-to-get-in tone-choice menu tail should be removed");

    assert!(repaired.contains("At the Globe"));
    assert!(repaired.contains("Arty Wilmot"));
    assert!(!repaired.contains("How does Evelyn try to get in"));
    assert!(!repaired.contains("quick bluff"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repaired narration must no longer trigger the action-menu gate: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_try_past_editor_tone_choice_menu() {
    let original = "The Globe's front offices are busy, and you are directed to the person who controls access: Editor Arty Wilmot. If Evelyn wants the clippings morgue opened, this is a real point of friction. How does she try to get past him? Does she lean on professional courtesy, polite persuasion, fast-talking newsroom urgency, or blunt pressure?";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("try-past-editor tone-choice menu tail should be removed");

    assert!(repaired.contains("Editor Arty Wilmot"));
    assert!(!repaired.contains("How does she try to get past him"));
    assert!(!repaired.contains("blunt pressure"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repaired narration must no longer trigger the action-menu gate: {repaired}"
    );
}

#[test]
fn deterministic_action_menu_tail_repair_drops_live_arty_access_tone_choice_menu() {
    let original = "At the Globe, the public side of the place will get Evelyn only so far. A staffer can tell her where the morgue is kept, but not simply wave her into it: access is controlled, and the name that comes back is Arty Wilmot, an editor with custody over the files. So the immediate obstacle is not yet the search itself, but getting legitimate access to the morgue. How does Evelyn approach that? If she presses politely, flatters, bluffs urgency, leans on her press credentials, or tries to bully past the gatekeeper, I'll resolve that accordingly.";

    let repaired = deterministic_player_agency_menu_tail_repair(original)
        .expect("live Arty access tone-choice menu tail should be removed");

    assert!(repaired.contains("Arty Wilmot"));
    assert!(!repaired.contains("How does Evelyn approach"));
    assert!(!repaired.contains("bluffs urgency"));
    assert!(
        !player_agency_menu_cue_needs_block(&repaired),
        "repaired narration must no longer trigger the action-menu gate: {repaired}"
    );
}

#[test]
fn player_visible_system_wrapper_cleanup_keeps_content_without_tags() {
    let original =
        "Knott nods.\n\n[system]You now have the house address and go-ahead.[/system]\n\nHe waits.";

    let cleaned = unwrap_player_visible_system_wrappers(original);

    assert!(cleaned.contains("You now have the house address"));
    assert!(!cleaned.contains("[system]"));
    assert!(!cleaned.contains("[/system]"));
    assert!(cleaned.contains("Knott nods."));
    assert!(cleaned.contains("He waits."));
}

#[test]
fn source_limited_address_repair_discloses_missing_literal_without_inventing() {
    let player = "请 Knott 把 Corbitt House 的完整街道地址当面写清楚。";
    let visible = "他掏出钢笔，低头把 **Corbitt House 的完整地址**工整写下，又把那串钥匙摊在桌上。";

    let repaired = source_limited_missing_address_repair(player, visible)
        .expect("claimed exact address delivery without a literal address should be repaired");

    assert!(!repaired.contains("完整地址"));
    assert!(repaired.contains("你的笔记里"));
    assert!(repaired.contains("而不是可逐字引用的门牌号"));
    assert!(!repaired.contains("玩家可见"));
    assert!(!repaired.contains("不编造街道文本"));
    assert!(
        !contains_concrete_street_address(&repaired),
        "repair must not invent a concrete street address: {repaired}"
    );
}

#[test]
fn source_limited_address_repair_detects_chinese_wrote_address_down() {
    let player = "请 Knott 把 Corbitt House 的完整街道地址当面写清楚。";
    let visible = "他把你摊开的笔记本拉近一些，把 **Corbitt House 的地址**端端正正写了下来。你面前现在有了明确的线头：记下的地址，以及那串钥匙。";

    let repaired = source_limited_missing_address_repair(player, visible)
        .expect("Chinese wrote-address-down claims without a literal should be repaired");

    assert!(!repaired.contains("Corbitt House 的地址"));
    assert!(repaired.contains("Corbitt House 的可导航地址线索"));
    assert!(repaired.contains("你的笔记里"));
    assert!(repaired.contains("而不是可逐字引用的门牌号"));
    assert!(!repaired.contains("玩家可见"));
    assert!(
        !contains_concrete_street_address(&repaired),
        "repair must not invent a concrete street address: {repaired}"
    );
}

#[test]
fn source_limited_address_repair_allows_concrete_literal() {
    let player = "请 Knott 把 24 Beacon Street, Boston 这个确切地址写清楚。";
    let visible = "Knott writes down the exact address: 24 Beacon Street, Boston.";

    assert!(
        source_limited_missing_address_repair(player, visible).is_none(),
        "a concrete address literal already supplied by the player should pass through unchanged"
    );
}

#[test]
fn source_limited_address_repair_scrubs_unsourced_concrete_street_address() {
    let player = "Please have Knott write down the full street address of the Corbitt House.";
    let visible = "Knott writes carefully: **20 Sheafe Street, Boston** beside the words **\"Corbitt House\"**, then sorts the keys.";

    let repaired = source_limited_missing_address_repair(player, visible)
        .expect("unsourced concrete street-address literals should be downgraded");

    assert!(!repaired.contains("20 Sheafe"));
    assert!(!repaired.contains("Sheafe Street"));
    assert!(repaired.contains("usable address lead"));
    assert!(repaired.contains("not a literal street-number line she can quote"));
    assert!(
        !contains_concrete_street_address(&repaired),
        "repair must not leave a concrete street address: {repaired}"
    );
}

#[test]
fn source_limited_address_repair_ignores_exact_refusal_for_records_search() {
    let player = "Evelyn follows the clerk's concrete suggestion: higher courts, county files, and the Central Police Station. Using only the Corbitt House address, the Corbitt surname, Macario, and Knott's commission as visible indexes, she asks for public references tied to the address. If access is restricted, she records the exact refusal and asks what public index can legally be checked next.";
    let visible = "The clerk writes down the exact refusal and explains which public address index can be checked next.";

    assert!(
        source_limited_missing_address_repair(player, visible).is_none(),
        "an exact refusal in a records-search action is not a request for an exact address"
    );
}

#[test]
fn source_limited_address_repair_replaces_english_full_street_address() {
    let player = "Please have Knott write down the full street address.";
    let visible = "He writes down the house's full street address in your notebook.";

    let repaired = source_limited_missing_address_repair(player, visible)
        .expect("English full street address claims without a literal should be repaired");

    assert!(!repaired.contains("full street address"));
    assert!(repaired.contains("usable address lead"));
    assert!(repaired.contains("In Evelyn's notes"));
    assert!(repaired.contains("not a literal street-number line she can quote"));
    assert!(!repaired.contains("player-visible"));
    assert!(!repaired.contains("do not invent"));
    assert!(!repaired.contains("当前可见资料没有给出可逐字抄下的门牌号"));
}

#[test]
fn source_limited_address_repair_replaces_full_street_address_beneath_claim() {
    let player = "Please have Knott write down the full street address.";
    let visible =
        "“Corbitt House,” he says, sliding it back, “with the full street address beneath.”";

    let repaired = source_limited_missing_address_repair(player, visible)
        .expect("full-street-address-beneath claims without a literal should be repaired");

    assert!(!repaired.contains("full street address"));
    assert!(repaired.contains("usable address lead"));
    assert!(repaired.contains("not a literal street-number line she can quote"));
}

#[test]
fn source_limited_address_repair_uses_in_fiction_note_not_internal_boundary() {
    let player = "Please have Knott write down the full street address.";
    let visible = "He writes down the house's full street address in your notebook.";

    let repaired = source_limited_missing_address_repair(player, visible)
        .expect("English full street address claims without a literal should be repaired");

    assert!(repaired.contains("In Evelyn's notes"));
    assert!(repaired.contains("usable address lead"));
    assert!(!repaired.contains("Keep the note boundary"));
    assert!(!repaired.contains("player-visible"));
    assert!(!repaired.contains("do not invent"));
}

#[test]
fn source_backed_clue_gate_fact_uses_graph_summary_without_inventing() {
    let graph = trpg_model::ModuleGraph {
        clues: vec![json!({
            "clue_id": "handout_7",
            "title": "Executor and Chapel Record",
            "summary": "Corbitt’s executor was Reverend Michael Thomas of the Chapel of Contemplation; the chapel closed in 1912."
        })],
        ..Default::default()
    };

    let fact = GmLoop::source_backed_clue_gate_fact(&graph, "handout_7")
        .expect("source-backed graph clue should produce a player-perceivable fact");

    assert!(fact.contains("Reverend Michael Thomas"));
    assert!(fact.contains("chapel closed in 1912"));
    assert!(!fact.contains("handout_7"), "{fact}");
    assert!(!fact.contains("已揭示的模组线索"), "{fact}");
}

#[test]
fn nominated_reveal_folds_source_backed_fact_for_same_turn_projection() {
    let graph = trpg_model::ModuleGraph {
        clues: vec![json!({
            "clue_id": "handout_7",
            "title": "Executor and Chapel Record",
            "summary": "Corbitt’s executor was Reverend Michael Thomas of the Chapel of Contemplation; the chapel closed in 1912."
        })],
        ..Default::default()
    };
    let nominations = vec![RevealNomination {
        fact_id: "handout_7".to_string(),
        reason: Some("records search succeeded".to_string()),
    }];
    let mut resolved_gate_facts = Vec::new();

    GmLoop::fold_source_backed_nominated_reveals(&graph, &nominations, &mut resolved_gate_facts);

    let rendered = resolved_gate_facts.join("\n");
    assert!(rendered.contains("Reverend Michael Thomas"), "{rendered}");
    assert!(rendered.contains("Chapel of Contemplation"), "{rendered}");
    assert!(rendered.contains("1912"), "{rendered}");
    assert!(rendered.contains("handout_7"), "{rendered}");
    assert!(rendered.contains("已揭示的模组线索"), "{rendered}");
}

#[test]
fn source_backed_clue_gate_fact_fails_closed_without_text() {
    let graph = trpg_model::ModuleGraph {
        clues: vec![json!({"clue_id": "handout_7", "title": "No player text"})],
        ..Default::default()
    };

    assert!(
        GmLoop::source_backed_clue_gate_fact(&graph, "handout_7").is_none(),
        "clue gate fact must not invent text when graph lacks source-backed summary/text"
    );
}

#[test]
fn source_limited_address_repair_replaces_english_full_address_of_house() {
    let player = "Please have Knott write down the full street address.";
    let visible =
        "Knott writes down the full address of the Corbitt House in a careful business hand.";

    let repaired = source_limited_missing_address_repair(player, visible)
        .expect("English full-address-of-house claims without a literal should be repaired");

    assert!(!repaired.contains("full address of the Corbitt House"));
    assert!(repaired.contains("usable address lead for the Corbitt House"));
    assert!(repaired.contains("In Evelyn's notes"));
    assert!(repaired.contains("not a literal street-number line she can quote"));
    assert!(!repaired.contains("player-visible"));
    assert!(!repaired.contains("do not invent"));
    assert!(!repaired.contains("当前可见资料没有给出可逐字抄下的门牌号"));
}

#[test]
fn source_limited_address_repair_replaces_old_corbitt_place_full_address_claim() {
    let player = "Please have Knott write down the full street address.";
    let visible =
        "He writes down the full address of the old Corbitt place and sorts through the keys.";

    let repaired = source_limited_missing_address_repair(player, visible)
        .expect("old Corbitt place full-address claims without a literal should be repaired");

    assert!(!repaired.contains("full address"));
    assert!(repaired.contains("usable address lead"));
    assert!(repaired.contains("In Evelyn's notes"));
    assert!(repaired.contains("not a literal street-number line she can quote"));
    assert!(
        !contains_concrete_street_address(&repaired),
        "repair must not invent a concrete street address: {repaired}"
    );
}

#[test]
fn source_limited_address_repair_replaces_generic_written_old_corbitt_address_claim() {
    let player = "Please have Knott write down the full street address.";
    let visible = "He writes down the address of the old Corbitt House for you in a cramped hand.";

    let repaired = source_limited_missing_address_repair(player, visible)
        .expect("generic written-address claims without a literal should be repaired");

    assert!(!repaired.contains("writes down the address"));
    assert!(repaired.contains("records the usable address lead"));
    assert!(repaired.contains("In Evelyn's notes"));
    assert!(repaired.contains("not a literal street-number line she can quote"));
    assert!(
        !contains_concrete_street_address(&repaired),
        "repair must not invent a concrete street address: {repaired}"
    );
}

#[test]
fn source_limited_address_repair_replaces_address_is_here_writing_it_down_claim() {
    let player = "Please have Knott write down the full street address.";
    let visible = "\"The address is here,\" he says, writing it down for you. He taps the ring of keys after that.";

    let repaired = source_limited_missing_address_repair(player, visible)
        .expect("address-is-here writing-down claims without a literal should be repaired");

    assert!(!repaired.contains("The address is here"));
    assert!(!repaired.contains("writing it down"));
    assert!(repaired.contains("usable address lead"));
    assert!(repaired.contains("In Evelyn's notes"));
    assert!(repaired.contains("not a literal street-number line she can quote"));
    assert!(
        !contains_concrete_street_address(&repaired),
        "repair must not invent a concrete street address: {repaired}"
    );
}

#[test]
fn source_limited_address_repair_replaces_house_address_certainty_claims() {
    let player = "Please have Knott write down the full street address.";
    let visible = "He writes down the house address in a cramped hand. The address is certain. The keys, the written address, and Knott's approval are in front of you.";

    let repaired = source_limited_missing_address_repair(player, visible)
        .expect("house-address certainty claims without a literal should be repaired");

    assert!(!repaired.contains("house address"));
    assert!(!repaired.contains("address is certain"));
    assert!(!repaired.contains("written address"));
    assert!(repaired.contains("usable address lead"));
    assert!(repaired.contains("In Evelyn's notes"));
    assert!(repaired.contains("not a literal street-number line she can quote"));
    assert!(
        !contains_concrete_street_address(&repaired),
        "repair must not invent a concrete street address: {repaired}"
    );
}

#[test]
fn source_limited_address_repair_appends_boundary_for_generic_address_claim() {
    let player = "请 Knott 把 Corbitt House 的完整街道地址当面写清楚。";
    let visible =
        "He takes your notebook and writes down the address in a cramped landlord's hand.";

    let repaired = source_limited_missing_address_repair(player, visible)
        .expect("generic address delivery claims should still disclose missing literal address");

    assert!(repaired.contains("In Evelyn's notes"));
    assert!(repaired.contains("not a literal street-number line she can quote"));
    assert!(!repaired.contains("player-visible"));
    assert!(!repaired.contains("not invent street text"));
    assert!(!repaired.contains("当前可见资料没有给出可逐字抄下的门牌号"));
}

#[test]
fn source_limited_address_repair_detects_split_english_address_delivery() {
    let player = "请 Knott 把 Corbitt House 的完整街道地址当面写清楚。";
    let visible = "Knott takes the notebook and writes the property’s address down carefully for you. He leaves the notebook open in front of you, the copied address and the keys between you.";

    let repaired = source_limited_missing_address_repair(player, visible)
        .expect("split English address-delivery phrases should disclose missing literal address");

    assert!(repaired.contains("In Evelyn's notes"));
    assert!(repaired.contains("not a literal street-number line she can quote"));
    assert!(!repaired.contains("player-visible"));
    assert!(!repaired.contains("not invent street text"));
    assert!(!repaired.contains("当前可见资料没有给出可逐字抄下的门牌号"));
    assert!(
        !contains_concrete_street_address(&repaired),
        "repair must not invent a concrete street address: {repaired}"
    );
}

// ==================== P6 revision: §二十四-#13 per-turn rejection producer→commit ====================

use crate::tools::RejectionNomination;

/// SEGMENT (b) of the §二十四-#13 chain — nomination → persist. NOT a single end-to-end test.
/// The §二十四-#13 coverage is SEGMENTED: 3 segments that COMPOSE the chain, each pinned by its own
/// test (a real GM-loop e2e would need a live LLM to actually call the tool, so it is not feasible
/// deterministically):
///   (a) tool → nomination: `note_player_rejection` tool invocation pushes a `RejectionNomination`
///       (crates/trpg-gm/src/tools/world.rs `call` + tests `note_player_rejection_args_require_thread_id`).
///   (b) nomination → persist (THIS test): a `RejectionNomination` injected on the turn ctx — exactly
///       the shape segment (a) produces — is drained at `presentation_commit_boundary` →
///       `commit_story_writes` PERSISTS it → the NEXT turn's selector input (`rejected_thread_ids`)
///       flags that thread for drop. This is the production per-turn caller for `commit_story_writes`
///       (no longer dead-by-tests). NOTE: this segment manually injects the nomination rather than
///       routing it through the tool, so it does not exercise segment (a)'s tool path.
///   (c) persist → selector-drop: trpg-runtime's `live_story_write_loop` test drives the real P5.3
///       selector and asserts the rejected thread is NOT in primary/secondary
///       (crates/trpg-runtime/tests/live_story_write_loop.rs; trpg-gm does not depend on trpg-director).
#[tokio::test]
async fn presentation_commit_drains_rejection_then_persists_for_next_turn_selector() {
    use trpg_model::{StoryState, StoryThread, StoryThreadStatus};
    use trpg_runtime::director_brief::rejected_thread_ids;

    let session = format!("s_reject_chain_{}", uuid::Uuid::new_v4().simple());
    let Some((mut gm, request)) = real_gm(&session).await else {
        eprintln!("SKIP: DATABASE_URL unset");
        return;
    };
    gm.engine
        .db
        .create_session(&session, "call_of_cthulhu_7e", None)
        .await
        .expect("create_session");

    // Seed two threads; thr_b is the STRONGER (higher urgency/interest) so only a persisted
    // rejection can keep it out of the spotlight next turn.
    let seed = StoryState {
        active_threads: vec![
            StoryThread {
                thread_id: "thr_a".into(),
                status: StoryThreadStatus::Active,
                urgency: 0.3,
                player_interest: 0.4,
                ..Default::default()
            },
            StoryThread {
                thread_id: "thr_b".into(),
                status: StoryThreadStatus::Escalating,
                urgency: 0.9,
                player_interest: 0.9,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    gm.engine
        .db
        .upsert_story_state(&session, &seed, "t0")
        .await
        .unwrap();

    // BEFORE: nothing is rejected (the selector would pick the stronger thr_b).
    let before = gm
        .engine
        .db
        .load_story_state(&session)
        .await
        .unwrap()
        .unwrap();
    assert!(
        rejected_thread_ids(&before).is_empty(),
        "pre-turn: no rejection ⇒ stronger thr_b is selectable"
    );

    // THIS TURN: the GM noted a player rejection of thr_b (what note_player_rejection nominates).
    std::env::set_var("TRPG_STORY_WRITE_LOOP", "1");
    let mut ctx = TurnContext::new();
    ctx.rejected_nominations = vec![RejectionNomination {
        thread_id: "thr_b".into(),
    }];
    ctx.presentation_gate = PresentationGate::Allow;
    gm.presentation_commit_boundary(&mut ctx, &request).await;
    std::env::set_var("TRPG_STORY_WRITE_LOOP", "0"); // M1: default ON ⇒ pin OFF explicitly

    // PERSISTED: the rejection is now durable and is the NEXT turn's selector input.
    let after = gm
        .engine
        .db
        .load_story_state(&session)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        rejected_thread_ids(&after),
        vec!["thr_b".to_string()],
        "PresentationCommit drained the nomination → commit_story_writes persisted thr_b rejected; \
         next turn's P5.3 selector reads this and drops thr_b"
    );
}

/// M1: the REVEAL fact_ids committed this turn are threaded into `commit_story_writes` as the
/// newly-known facts (closing the old `&[]` placeholder), so a Dormant story thread whose
/// `related_fact_ids` includes a freshly-revealed fact floors `Dormant → Introduced` through the
/// real PresentationCommit path — the StoryThreadOpened floor finally fires from live reveals.
#[tokio::test]
async fn presentation_commit_reveal_floors_dormant_thread_to_introduced() {
    use trpg_model::{StoryState, StoryThread, StoryThreadStatus};

    let session = format!("s_reveal_floor_{}", uuid::Uuid::new_v4().simple());
    let Some((mut gm, request)) = real_gm(&session).await else {
        eprintln!("SKIP: DATABASE_URL unset");
        return;
    };
    gm.engine
        .db
        .create_session(&session, "call_of_cthulhu_7e", None)
        .await
        .expect("create_session");
    // A Dormant thread keyed on fact "f_clue" — the floor lifts it only once that fact is learned.
    let seed = StoryState {
        active_threads: vec![StoryThread {
            thread_id: "thr_dormant".into(),
            status: StoryThreadStatus::Dormant,
            related_fact_ids: vec!["f_clue".into()],
            ..Default::default()
        }],
        ..Default::default()
    };
    gm.engine
        .db
        .upsert_story_state(&session, &seed, "t0")
        .await
        .unwrap();

    // THIS TURN: the GM revealed fact "f_clue" (a player-known reveal nomination). On a committed
    // (Allow) turn its fact_id becomes a newly-known fact threaded into commit_story_writes.
    std::env::set_var("TRPG_STORY_WRITE_LOOP", "1");
    let mut ctx = TurnContext::new();
    ctx.nominated_reveals = vec![crate::tools::RevealNomination {
        fact_id: "f_clue".into(),
        reason: None,
    }];
    ctx.presentation_gate = PresentationGate::Allow;
    gm.presentation_commit_boundary(&mut ctx, &request).await;
    std::env::set_var("TRPG_STORY_WRITE_LOOP", "0"); // M1: default ON ⇒ pin OFF explicitly

    let after = gm
        .engine
        .db
        .load_story_state(&session)
        .await
        .unwrap()
        .unwrap();
    let thread = after
        .active_threads
        .iter()
        .find(|t| t.thread_id == "thr_dormant")
        .expect("seeded thread persists");
    assert_eq!(
        thread.status,
        StoryThreadStatus::Introduced,
        "the revealed fact floored the Dormant thread to Introduced via the real turn path"
    );
}

/// OFF==baseline: a revealed fact must NOT floor any thread when the kill-switch is OFF — the
/// story_state stays byte-identical to the seed (commit_story_writes early-returns).
#[tokio::test]
async fn presentation_commit_reveal_off_is_baseline_no_floor() {
    use trpg_model::{StoryState, StoryThread, StoryThreadStatus};

    let session = format!("s_reveal_off_{}", uuid::Uuid::new_v4().simple());
    let Some((mut gm, request)) = real_gm(&session).await else {
        eprintln!("SKIP: DATABASE_URL unset");
        return;
    };
    gm.engine
        .db
        .create_session(&session, "call_of_cthulhu_7e", None)
        .await
        .expect("create_session");
    let seed = StoryState {
        active_threads: vec![StoryThread {
            thread_id: "thr_dormant".into(),
            status: StoryThreadStatus::Dormant,
            related_fact_ids: vec!["f_clue".into()],
            ..Default::default()
        }],
        ..Default::default()
    };
    gm.engine
        .db
        .upsert_story_state(&session, &seed, "t0")
        .await
        .unwrap();

    std::env::set_var("TRPG_STORY_WRITE_LOOP", "0"); // kill-switch OFF ⇒ byte-identical baseline
    let mut ctx = TurnContext::new();
    ctx.nominated_reveals = vec![crate::tools::RevealNomination {
        fact_id: "f_clue".into(),
        reason: None,
    }];
    ctx.presentation_gate = PresentationGate::Allow;
    gm.presentation_commit_boundary(&mut ctx, &request).await;

    let after = gm
        .engine
        .db
        .load_story_state(&session)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after, seed,
        "OFF: revealed fact floors nothing — story_state byte-identical to seed"
    );
}

/// OFF==baseline: with the flag OFF, draining a rejection nomination is a complete no-op —
/// story_state is byte-identical to the seed (the producer never persists anything).
#[tokio::test]
async fn presentation_commit_rejection_off_is_baseline_no_write() {
    use trpg_model::{StoryState, StoryThread, StoryThreadStatus};
    use trpg_runtime::director_brief::rejected_thread_ids;

    std::env::set_var("TRPG_STORY_WRITE_LOOP", "0"); // M1: default ON ⇒ pin OFF (ensure OFF regardless of ordering)
    let session = format!("s_reject_off_{}", uuid::Uuid::new_v4().simple());
    let Some((mut gm, request)) = real_gm(&session).await else {
        eprintln!("SKIP: DATABASE_URL unset");
        return;
    };
    gm.engine
        .db
        .create_session(&session, "call_of_cthulhu_7e", None)
        .await
        .expect("create_session");
    let seed = StoryState {
        active_threads: vec![StoryThread {
            thread_id: "thr_b".into(),
            status: StoryThreadStatus::Escalating,
            urgency: 0.9,
            player_interest: 0.9,
            ..Default::default()
        }],
        ..Default::default()
    };
    gm.engine
        .db
        .upsert_story_state(&session, &seed, "t0")
        .await
        .unwrap();

    let mut ctx = TurnContext::new();
    ctx.rejected_nominations = vec![RejectionNomination {
        thread_id: "thr_b".into(),
    }];
    ctx.presentation_gate = PresentationGate::Allow;
    gm.presentation_commit_boundary(&mut ctx, &request).await;

    let after = gm
        .engine
        .db
        .load_story_state(&session)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after, seed,
        "OFF: story_state byte-identical to seed (no write)"
    );
    assert!(
        rejected_thread_ids(&after).is_empty(),
        "OFF: no rejection persisted"
    );
}

/// Block ⇒ rejection nominations are dropped (same gate discipline as reveal commit): a Blocked
/// turn never persists the story write.
#[tokio::test]
async fn presentation_commit_rejection_block_drops() {
    use trpg_model::{StoryState, StoryThread, StoryThreadStatus};
    use trpg_runtime::director_brief::rejected_thread_ids;

    let session = format!("s_reject_block_{}", uuid::Uuid::new_v4().simple());
    let Some((mut gm, request)) = real_gm(&session).await else {
        eprintln!("SKIP: DATABASE_URL unset");
        return;
    };
    gm.engine
        .db
        .create_session(&session, "call_of_cthulhu_7e", None)
        .await
        .expect("create_session");
    let seed = StoryState {
        active_threads: vec![StoryThread {
            thread_id: "thr_b".into(),
            status: StoryThreadStatus::Escalating,
            ..Default::default()
        }],
        ..Default::default()
    };
    gm.engine
        .db
        .upsert_story_state(&session, &seed, "t0")
        .await
        .unwrap();

    std::env::set_var("TRPG_STORY_WRITE_LOOP", "1");
    let mut ctx = TurnContext::new();
    ctx.rejected_nominations = vec![RejectionNomination {
        thread_id: "thr_b".into(),
    }];
    ctx.presentation_gate = block_gate(); // 终审 Block
    gm.presentation_commit_boundary(&mut ctx, &request).await;
    std::env::set_var("TRPG_STORY_WRITE_LOOP", "0"); // M1: default ON ⇒ pin OFF explicitly

    let after = gm
        .engine
        .db
        .load_story_state(&session)
        .await
        .unwrap()
        .unwrap();
    assert!(
        rejected_thread_ids(&after).is_empty(),
        "Block ⇒ rejection nomination dropped, nothing persisted"
    );
}

// ======================== MAT.M3 axis-2 线索发现能供（DP-3）========================
// 纯映射逻辑（成功侦查→线索 fact）在 trpg_runtime::clue_reveal_candidates 全量单测；
// 这里只测 gm 侧两件「边界 + 通道」事：(#4) 线索 fact 走与 reveal 同一终审 gate
// （Block 丢弃 / Allow 提交）；(#5) 同回合 surface+learn 的提名去重幂等（不双计）。

/// MAT.M3 #4(Block)：线索能供产出的 RevealNomination 必须遵守 PresentationCommit 终审 gate
/// —— Block ⇒ 零 PlayerLearnedFact 行（与既有 reveal Block 同口径，证明线索 fact 非旁路）。
#[tokio::test]
async fn clue_reveal_nomination_blocked_writes_no_player_learned_fact() {
    let session = format!("s_clue_block_{}", uuid::Uuid::new_v4().simple());
    let Some((mut gm, request)) = real_gm(&session).await else {
        eprintln!("SKIP: DATABASE_URL unset");
        return;
    };
    let mut ctx = TurnContext::new();
    // 模拟线索能供已把一条线索 fact 提名进通道（fact_id = 线索 id，非正文）。
    ctx.nominated_reveals = vec![RevealNomination {
        fact_id: "clue_diary".into(),
        reason: Some("successful 侦查 check examined the clue in the current scene".into()),
    }];
    ctx.presentation_gate = block_gate(); // 终审 Block
    gm.presentation_commit_boundary(&mut ctx, &request).await;
    assert!(
        gm.engine
            .db
            .list_revealed_facts(&session)
            .await
            .unwrap()
            .is_empty(),
        "Block ⇒ 线索 fact 不提交（PlayerLearnedFact 零行）"
    );
}

/// MAT.M3 #4(Allow)：终审 Allow ⇒ 线索 fact 经既有 reveal commit primitive 落库
/// （ContextSurfaced→提名→PlayerLearnedFact，仅 Allow 后才成 learned）。
#[tokio::test]
async fn clue_reveal_nomination_allow_commits_player_learned_fact() {
    let session = format!("s_clue_allow_{}", uuid::Uuid::new_v4().simple());
    let Some((mut gm, request)) = real_gm(&session).await else {
        eprintln!("SKIP: DATABASE_URL unset");
        return;
    };
    let mut ctx = TurnContext::new();
    ctx.nominated_reveals = vec![RevealNomination {
        fact_id: "clue_diary".into(),
        reason: Some("found via Spot Hidden".into()),
    }];
    ctx.presentation_gate = PresentationGate::Allow; // 终审 Allow
    gm.presentation_commit_boundary(&mut ctx, &request).await;
    assert_eq!(
        gm.engine.db.list_revealed_facts(&session).await.unwrap(),
        vec!["clue_diary".to_string()],
        "Allow ⇒ 线索 fact 提交为 revealed（PlayerLearnedFact）"
    );
}

#[tokio::test]
async fn presentation_commit_allow_projects_source_backed_clue_fact_to_visible_text() {
    let session = format!("s_clue_visible_{}", uuid::Uuid::new_v4().simple());
    let Some((mut gm, mut request)) = real_gm(&session).await else {
        eprintln!("SKIP: DATABASE_URL unset");
        return;
    };
    request.module_id = Some("call_of_cthulhu_7e.the_haunting".to_string());
    let graph = gm
        .engine
        .db
        .load_module_graph("call_of_cthulhu_7e.the_haunting")
        .await
        .ok()
        .flatten();
    let Some(graph) = graph else {
        eprintln!("SKIP: The Haunting module graph not loaded");
        return;
    };
    if GmLoop::source_backed_clue_gate_fact(&graph, "handout_7").is_none() {
        eprintln!("SKIP: handout_7 source-backed text unavailable");
        return;
    }

    let mut ctx = TurnContext::new();
    ctx.visible_text = "[roll]Library Use -- Hall of Records search: 1d100=[17] 目标:≤60，结果:strong_success[/roll]\nYou find a traceable records trail, but the prose only names categories.".to_string();
    ctx.nominated_reveals = vec![RevealNomination {
        fact_id: "handout_7".into(),
        reason: Some("successful Hall of Records search".into()),
    }];
    ctx.presentation_gate = PresentationGate::Allow;

    gm.presentation_commit_boundary(&mut ctx, &request).await;

    assert!(
        ctx.visible_text.contains("Reverend Michael Thomas")
            && ctx.visible_text.contains("Chapel of Contemplation")
            && ctx.visible_text.contains("1912"),
        "committed source-backed clue facts must be projected to the same player-visible turn: {}",
        ctx.visible_text
    );
}

#[tokio::test]
async fn presentation_commit_projects_immediate_reveal_fact_to_visible_text() {
    let session = format!("s_clue_immediate_visible_{}", uuid::Uuid::new_v4().simple());
    let Some((mut gm, mut request)) = real_gm(&session).await else {
        eprintln!("SKIP: DATABASE_URL unset");
        return;
    };
    request.module_id = Some("call_of_cthulhu_7e.the_haunting".to_string());
    let graph = gm
        .engine
        .db
        .load_module_graph("call_of_cthulhu_7e.the_haunting")
        .await
        .ok()
        .flatten();
    let Some(graph) = graph else {
        eprintln!("SKIP: The Haunting module graph not loaded");
        return;
    };
    if GmLoop::source_backed_clue_gate_fact(&graph, "handout_3").is_none() {
        eprintln!("SKIP: handout_3 source-backed text unavailable");
        return;
    }
    gm.engine
        .reveal_fact(
            &session,
            &request.turn_id,
            "handout_3",
            Some("successful Central Library research"),
        )
        .await
        .expect("immediate reveal_fact should commit");

    let mut ctx = TurnContext::new();
    ctx.visible_text = "[roll]中央图书馆与旧报纸档案检索：科宾宅相关记录: 1d100=[50] 目标:≤75，结果:strong_success[/roll]".to_string();
    ctx.presentation_gate = PresentationGate::Allow;

    gm.presentation_commit_boundary(&mut ctx, &request).await;

    assert!(
        ctx.visible_text.contains("merchant")
            && ctx.visible_text.contains("Walter Corbitt"),
        "current-turn immediate PlayerLearnedFact should be projected to the same player-visible turn: {}",
        ctx.visible_text
    );
}

#[tokio::test]
async fn presentation_commit_language_rewrites_cross_language_source_fact() {
    let prev_lang = std::env::var("TRPG_OUTPUT_LANGUAGE").ok();
    std::env::set_var("TRPG_OUTPUT_LANGUAGE", "zh-Hans");
    let session = format!("s_clue_lang_visible_{}", uuid::Uuid::new_v4().simple());
    let Some((mut gm, llm, mut request)) = real_gm_with_llm(&session).await else {
        match prev_lang {
            Some(v) => std::env::set_var("TRPG_OUTPUT_LANGUAGE", v),
            None => std::env::remove_var("TRPG_OUTPUT_LANGUAGE"),
        }
        eprintln!("SKIP: DATABASE_URL unset");
        return;
    };
    request.module_id = Some("call_of_cthulhu_7e.the_haunting".to_string());
    let graph = gm
        .engine
        .db
        .load_module_graph("call_of_cthulhu_7e.the_haunting")
        .await
        .ok()
        .flatten();
    let Some(graph) = graph else {
        match prev_lang {
            Some(v) => std::env::set_var("TRPG_OUTPUT_LANGUAGE", v),
            None => std::env::remove_var("TRPG_OUTPUT_LANGUAGE"),
        }
        eprintln!("SKIP: The Haunting module graph not loaded");
        return;
    };
    if GmLoop::source_backed_clue_gate_fact(&graph, "handout_3").is_none() {
        match prev_lang {
            Some(v) => std::env::set_var("TRPG_OUTPUT_LANGUAGE", v),
            None => std::env::remove_var("TRPG_OUTPUT_LANGUAGE"),
        }
        eprintln!("SKIP: handout_3 source-backed text unavailable");
        return;
    }
    llm.text_responses.lock().unwrap().push(
        "[roll]中央图书馆与旧报纸档案检索：科宾宅相关记录: 1d100=[50] 目标:≤75，结果:strong_success[/roll]\n\
         你在中央图书馆的旧地产记录里查到，最早有一名商人建造了这栋宅子，随后很快把它转卖给 Walter Corbitt。"
            .to_string(),
    );
    gm.engine
        .reveal_fact(
            &session,
            &request.turn_id,
            "handout_3",
            Some("successful Central Library research"),
        )
        .await
        .expect("immediate reveal_fact should commit");

    let mut ctx = TurnContext::new();
    ctx.visible_text = "[roll]中央图书馆与旧报纸档案检索：科宾宅相关记录: 1d100=[50] 目标:≤75，结果:strong_success[/roll]".to_string();
    ctx.presentation_gate = PresentationGate::Allow;

    gm.presentation_commit_boundary(&mut ctx, &request).await;

    assert!(ctx.visible_text.contains("商人"), "{}", ctx.visible_text);
    assert!(
        ctx.visible_text.contains("Walter Corbitt"),
        "{}",
        ctx.visible_text
    );
    assert!(
        !ctx.visible_text.contains("merchant"),
        "Chinese output setting must not raw-append English source summary: {}",
        ctx.visible_text
    );
    let text_requests = llm.text_requests.lock().unwrap();
    assert_eq!(
        text_requests.len(),
        1,
        "cross-language missing source fact should use semantic LLM rewrite"
    );
    assert!(
        text_requests[0][0].content.contains("必须全程使用简体中文"),
        "rewrite prompt must carry configured language contract"
    );

    match prev_lang {
        Some(v) => std::env::set_var("TRPG_OUTPUT_LANGUAGE", v),
        None => std::env::remove_var("TRPG_OUTPUT_LANGUAGE"),
    }
}

/// MAT.M3 #5：同回合「线索 surface + learn」幂等——线索能供绝不重复提名同一 fact。
/// 直接断言纯映射重复跑 / 多条命中检定产稳定单条 fact（idempotency invariant：
/// record_context_surfaced_entities 幂等 per-session，本路径只产提名、不写 ContextSurfaced，
/// 故不会与 surfaced 事件 race / 双计）。DB-free。
#[test]
fn clue_affordance_same_turn_surface_and_learn_is_idempotent() {
    use trpg_model::MaterializationAffordanceMode::Enforce;
    use trpg_model::{ModuleGraph, ScenarioNode};
    use trpg_runtime::{clue_reveal_candidates, ResolvedCheck};

    let scene = ScenarioNode {
        node_id: "sc01".into(),
        referenced_clue_ids: vec!["clue_diary".into()],
        ..Default::default()
    };
    let graph = ModuleGraph {
        clues: vec![json!({"id":"clue_diary","name":"血迹斑斑的日记","body":"正文不该逐字揭示"})],
        ..Default::default()
    };
    let check = ResolvedCheck {
        success: true,
        tested_parameter: Some("侦查"),
        check_label: "侦查血迹斑斑的日记",
        action_summary: "玩家仔细搜查书房",
    };
    // 同一回合内（surface 后立即 learn）多次求值必产同一单条 fact —— 调用方据 fact_id 去重，
    // 故不会双计；fact_id 是线索 id，绝非正文。
    let a = clue_reveal_candidates(&check, &scene, &graph, Enforce, true);
    let b = clue_reveal_candidates(&check, &scene, &graph, Enforce, true);
    assert_eq!(a.len(), 1);
    assert_eq!(a, b, "同回合重复求值必产同一提名（确定性，可去重）");
    assert_eq!(a[0].fact_id, "clue_diary");
    assert!(!a[0].reason.contains("正文"), "reason 不得含线索正文逐字");

    // 调用方去重语义：把同 fact 二次合并入既有提名集 ⇒ 仍只一条（gm wiring 同款 any() 守卫）。
    let mut noms: Vec<RevealNomination> = a
        .iter()
        .map(|c| RevealNomination {
            fact_id: c.fact_id.clone(),
            reason: Some(c.reason.clone()),
        })
        .collect();
    for c in &b {
        if !noms.iter().any(|n| n.fact_id == c.fact_id) {
            noms.push(RevealNomination {
                fact_id: c.fact_id.clone(),
                reason: Some(c.reason.clone()),
            });
        }
    }
    assert_eq!(
        noms.len(),
        1,
        "同 fact 跨多次命中只提名一次（幂等去重，不双计）"
    );
}

// ==================== MAT.M4 present vs met/engaged 剧透闸（§7-#6）====================
// 纯派生（present→met/unmet、Director 杠杆口径、guidance 反应式约束）在
// trpg_runtime::met_engaged 全量单测；Director 杠杆门在 trpg_director dehardcode_tests。
// 这里测 gm 侧两件「通道 + 不变量」事：(#1) un-met NPC 的 withheld secret 绝不进
// knowledge_basis（= facts_can_reveal）且绝不自动揭示——M4 约束只追加 prompt 文本、绝不
// 触碰 secret 门；(#3) Off/Shadow 下 guidance 不被追加约束（字节级基线）。

/// MAT.M7 (D1)：`effective_active_npc_ids` 的优先/退回语义——派生集非空 ⇒ 取派生集
/// （= 消费者读到的 active 集，修 NPC 对白=0）；派生集空 ⇒ 退回调用方集（ctx_provider
/// 测试 seam + Off/Shadow 字节等价基线 + s17 空-退-空不变量）。纯函数，DB-free。
#[test]
fn m7_effective_active_npc_ids_prefers_derived_else_caller() {
    let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();

    // Enforce 形态：prepare_turn_context 派生集非空 ⇒ 取派生集（即便调用方传入空，= M6 缺陷场景）。
    let derived = ids(&["npc_athena", "npc_russ"]);
    let caller_empty: Vec<String> = vec![];
    assert_eq!(
        effective_active_npc_ids(&derived, &caller_empty),
        derived.as_slice(),
        "派生集非空 ⇒ 消费者读派生集（修「派生集进不了消费者 → 对白=0」）"
    );

    // Off/Shadow / ctx_provider seam：派生集空 ⇒ 退回调用方集（保留其原样，字节等价基线）。
    let caller_supplied = ids(&["npc_supplied"]);
    assert_eq!(
        effective_active_npc_ids(&[], &caller_supplied),
        caller_supplied.as_slice(),
        "派生集空 ⇒ 退回调用方集（Off/Shadow 字节等价 + ctx_provider seam 行为不变）"
    );

    // s17 不变量：两路皆空 ⇒ 空（无供给即空，绝不臆造）。
    assert!(effective_active_npc_ids(&[], &caller_empty).is_empty());
}

/// MAT.M4 #1：active-but-UN-MET NPC 的 withheld secret 永不进 knowledge_basis、永不自动揭示。
/// secret 门（facts_can_reveal vs facts_will_withhold）由既有 viewer_behavior_context 派生；
/// M4 的反应式约束（restrict_unmet_npc_guidance）只在 prompt 块尾追加文本，**绝不**把任何
/// withheld id 搬进 facts_can_reveal / knowledge_basis。DB-free。
#[test]
fn m4_unmet_npc_withheld_secret_never_enters_knowledge_basis() {
    use trpg_model::MaterializationAffordanceMode::Enforce;
    use trpg_model::{
        KnowledgeState, NpcKnowledgeEntry, NpcMindView, NpcProfile, NpcRelationship,
        NpcRelationshipTarget,
    };
    use trpg_runtime::world::render_world_reaction_block;
    use trpg_runtime::{
        derive_met_engaged_gate, derive_npc_behavior_plan, restrict_unmet_npc_guidance,
        viewer_behavior_context,
    };

    // NPC 知道一条真 fact，玩家方未知 → withheld secret（既有 v1 保守门）。
    let profile = NpcProfile {
        actor_id: "npc_unmet".into(),
        name: "Stranger".into(),
        ..Default::default()
    };
    let rel = NpcRelationship::new("s", "npc_unmet", NpcRelationshipTarget::PlayerParty).unwrap();
    let entries = vec![NpcKnowledgeEntry {
        fact_id: "secret_culprit".into(),
        state: KnowledgeState::KnowsTrue,
    }];
    let view = NpcMindView::build("s", "npc_unmet", &profile, &[rel], &entries).unwrap();
    let ctx = viewer_behavior_context(&view, &[]); // 玩家方空知集 ⇒ 全 withheld
    let plan = derive_npc_behavior_plan(&view, &ctx);

    // secret 门不变量：withheld 含 secret，可揭集与 knowledge_basis 都不含它。
    assert!(plan
        .facts_will_withhold
        .iter()
        .any(|f| f == "secret_culprit"));
    assert!(!plan.facts_can_reveal.iter().any(|f| f == "secret_culprit"));
    let cand = plan.to_reaction_candidate();
    assert!(
        !cand.knowledge_basis.iter().any(|f| f == "secret_culprit"),
        "knowledge_basis 仅源自 facts_can_reveal，withheld secret 绝不进"
    );

    // M4 反应式约束追加后，仍不得把 secret 搬进可揭集：约束只提 NPC id，不提任何 fact id。
    let base = render_world_reaction_block(&[plan.clone()]);
    let active = vec!["npc_unmet".to_string()];
    let gate = derive_met_engaged_gate(&active, &[], Enforce); // 未暴露 ⇒ un-met
    let gated = restrict_unmet_npc_guidance(base, &gate).unwrap();
    assert!(
        gated.contains("[npc_presence_gate]"),
        "un-met ⇒ 追加反应式约束"
    );
    assert!(gated.contains("npc_unmet"), "约束点名 un-met NPC id");
    // M4 追加的 [npc_presence_gate] 段本身只提 NPC id，绝不含任何 withheld secret fact id。
    let added = gated
        .split("[npc_presence_gate]")
        .nth(1)
        .expect("presence gate block present");
    assert!(
        !added.contains("secret_culprit"),
        "M4 约束段绝不泄露 / 提升 withheld secret（只提 NPC id，不触碰 secret 门）"
    );
    // 既有块仍以「Withhold fact ids」正确呈现 withheld id（指示 NPC 隐瞒，非揭示），
    // 且它从未出现在可揭集 / knowledge_basis（上方已断言）——secret 门保持不变。
    assert!(gated.contains("Withhold fact ids: secret_culprit"));
}

/// MAT.M4 #3：Off / Shadow ⇒ guidance 不被追加约束（字节级基线）。
#[test]
fn m4_off_shadow_guidance_is_byte_identical_baseline() {
    use trpg_model::MaterializationAffordanceMode::{Off, Shadow};
    use trpg_runtime::{derive_met_engaged_gate, restrict_unmet_npc_guidance};

    let active = vec!["npc_unmet".to_string()];
    let base =
        Some("[npc_behavior_guidance npc=npc_unmet]\n…\n[/npc_behavior_guidance]".to_string());
    for mode in [Off, Shadow] {
        // 即便玩家从未暴露过该 NPC，Off/Shadow 闸惰性 ⇒ 不追加约束。
        let gate = derive_met_engaged_gate(&active, &[], mode);
        assert!(!gate.enforced, "{mode:?}: 闸惰性");
        assert_eq!(
            restrict_unmet_npc_guidance(base.clone(), &gate),
            base,
            "{mode:?}: guidance 字节不变（无主动开口收紧）"
        );
    }
}

// —— L1.1 SPINE: post-adjudication committed-result projection (pure seam) ——
fn pa_check_result(check_id: &str, success: bool) -> CheckResultRecord {
    let roll = DiceRollRecord {
        roll_id: format!("roll_{check_id}"),
        session_id: "s".to_string(),
        turn_id: "t".to_string(),
        check_id: Some(check_id.to_string()),
        roller_kind: ActorKind::PlayerCharacter,
        roller_id: Some("pc.current".to_string()),
        visibility: RollVisibility::PublicGmRoll,
        expression: "1d100".to_string(),
        result: json!({"total": if success { 12 } else { 88 }}),
        seed_commitment: "seed".to_string(),
        revealed_at: None,
        created_at: Utc::now(),
    };
    CheckResultRecord {
        check_id: check_id.to_string(),
        roll,
        outcome: json!({"success": success, "degree": if success {"success"} else {"failure"}}),
        committed_patches: vec![],
        created_at: Utc::now(),
    }
}

#[test]
fn post_adjudication_off_empty_on_projects_committed_results() {
    use trpg_agent::TurnLedgerSnapshot;
    let mut snap = TurnLedgerSnapshot::default();
    snap.check_results.push(pa_check_result("c.pass", true));
    snap.check_results.push(pa_check_result("c.fail", false));

    // OFF ⇒ empty (no consumer ⇒ byte-identical baseline).
    assert!(
        project_post_adjudication_results(&snap, false).is_empty(),
        "flag OFF must yield no results"
    );

    // ON ⇒ projects the committed results reflecting the real pass/fail outcome.
    let views = project_post_adjudication_results(&snap, true);
    assert_eq!(views.len(), 2);
    assert_eq!(views[0].check_id, "c.pass");
    assert_eq!(views[0].outcome, trpg_model::CheckOutcomeView::Passed);
    assert_eq!(views[1].check_id, "c.fail");
    assert_eq!(views[1].outcome, trpg_model::CheckOutcomeView::Failed);
}

#[test]
fn ctx_capture_post_adjudication_off_empty_on_carries_committed_results() {
    // The L1.1 ctx seam: `resolution_commit_boundary` calls `capture_post_adjudication`.
    let mut ctx = TurnContext::new();
    ctx.test_record_check_result(&pa_check_result("c.pass", true));
    ctx.test_record_check_result(&pa_check_result("c.fail", false));

    // OFF ⇒ ctx carries nothing (no consumer ⇒ byte-identical baseline).
    ctx.capture_post_adjudication(false);
    assert!(
        ctx.post_adjudication_results().is_empty(),
        "flag OFF must leave ctx empty"
    );

    // ON ⇒ ctx carries the committed-result projection reflecting real pass/fail.
    ctx.capture_post_adjudication(true);
    let views = ctx.post_adjudication_results();
    assert_eq!(views.len(), 2);
    assert_eq!(views[0].outcome, trpg_model::CheckOutcomeView::Passed);
    assert_eq!(views[1].outcome, trpg_model::CheckOutcomeView::Failed);
    // World candidate pool defaults empty until the pre-adjudication stash runs (flag ON).
    assert!(ctx.world_candidates().is_empty());
}

#[test]
fn l61_player_safe_director_plan_tokens_carry_only_steering_not_secrets() {
    use trpg_model::story::BeatKind;
    // A fully-populated plan: only beat_kind/desired_change/dramatic_function are player-safe.
    let plan = DirectorPlan {
        beat_kind: BeatKind::Complicate,
        desired_change: "  fail_forward  ".to_string(),
        dramatic_function: "raise_stakes".to_string(),
        // secrets that must NOT leak into the Narrator carrier:
        reveal_candidate_fact_ids: vec!["secret_fact_1".to_string()],
        must_avoid: vec!["the_killer_is_the_butler".to_string()],
        primary_thread_id: Some("thr_hidden".to_string()),
        ..Default::default()
    };
    let tokens = player_safe_director_plan_tokens(&plan);
    assert_eq!(
        tokens,
        vec![
            "beat:complicate".to_string(),
            "desired_change:fail_forward".to_string(), // trimmed
            "dramatic_function:raise_stakes".to_string(),
        ]
    );
    // No secret/id field ever appears in the carrier.
    let joined = tokens.join("|");
    assert!(!joined.contains("secret_fact_1"));
    assert!(!joined.contains("the_killer_is_the_butler"));
    assert!(!joined.contains("thr_hidden"));

    // Minimal default plan ⇒ only the always-present beat token (empty short-strings dropped).
    let minimal = player_safe_director_plan_tokens(&DirectorPlan::default());
    assert_eq!(minimal, vec!["beat:respond".to_string()]);
}

// L4.3 — the proactive forbidden-reveal set stashed on ctx flows into the narrator's
// NarrationPacket.forbidden_reveals, EXACTLY as `run_narrator_phase` composes it. OFF (empty ctx
// set) ⇒ project's forbidden is empty, byte-identical to the prior `&[]` baseline.
#[test]
fn l43_scene_forbidden_reveals_plumbs_into_narration_packet() {
    use crate::packet::{AdjudicationPacket, NarrationPacket};
    use trpg_agent::TurnLedgerSnapshot;

    let adj =
        AdjudicationPacket::project("环顾四周", &TurnLedgerSnapshot::default(), &[], "", None);

    // OFF baseline: empty ctx forbidden ⇒ empty packet forbidden (the prior `&[]` behavior).
    let ctx_off = TurnContext::new();
    let off = NarrationPacket::project(&adj, "", &ctx_off.scene_forbidden_reveals().to_vec());
    assert!(
        off.forbidden_reveals.is_empty(),
        "OFF ⇒ no proactive forbidden (byte-equal baseline)"
    );

    // ON: a scene's still-building facts stashed on ctx flow into the packet's forbidden_reveals.
    let mut ctx_on = TurnContext::new();
    ctx_on.test_set_scene_forbidden_reveals(vec![
        "secret_clue_a".to_string(),
        "secret_clue_b".to_string(),
    ]);
    let on = NarrationPacket::project(&adj, "", &ctx_on.scene_forbidden_reveals().to_vec());
    assert_eq!(
        on.forbidden_reveals,
        vec!["secret_clue_a".to_string(), "secret_clue_b".to_string()],
        "ON ⇒ the scene's proactive forbidden facts reach the Narrator packet"
    );
}

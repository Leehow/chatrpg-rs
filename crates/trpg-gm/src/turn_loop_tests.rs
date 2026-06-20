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

struct MockLlm {
    scripts: Mutex<Vec<Vec<StreamEvent>>>,
    choices: Mutex<Vec<ToolChoice>>,
    requests: Mutex<Vec<Vec<Value>>>,
}

#[async_trait]
impl LlmClient for MockLlm {
    async fn complete_text(&self, _: Vec<ChatMessage>, _: f32) -> Result<String> {
        unimplemented!("MockLlm complete_text is unused by run_gm_turn tests")
    }
    // 刺激预 pass 走 complete_json：恒回空命中（这些测试不测该通路）。
    async fn complete_json(&self, _: Vec<ChatMessage>, _: f32) -> Result<Value> {
        Ok(json!({"hits": []}))
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

// ======================== P6.7 reveal_fact gating + 两 commit 边界 ========================

use crate::presentation_gate::PresentationGate;
use crate::tools::RevealNomination;
use trpg_agent::{VerifierFinding, VerifierFindingKind, VerifierSeverity};

/// DB-gated 真库装配（DATABASE_URL 缺失 ⇒ None ⇒ 调用方 SKIP，绝不伪 PASS）。
async fn real_gm(session: &str) -> Option<(GmLoop, ContextRequest)> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = trpg_db::Db::connect(&url).await.ok()?;
    db.migrate().await.ok()?;
    let engine = RuntimeEngine::new(db);
    let llm = Arc::new(MockLlm {
        scripts: Mutex::new(Vec::new()),
        choices: Mutex::new(Vec::new()),
        requests: Mutex::new(Vec::new()),
    });
    let data_dir = std::env::temp_dir().join(format!(
        "gm_reveal_test_{}_{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    let gm = GmLoop::new(
        engine,
        llm,
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
    Some((gm, request))
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
    let before = gm.engine.db.load_story_state(&session).await.unwrap().unwrap();
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
    std::env::remove_var("TRPG_STORY_WRITE_LOOP");

    // PERSISTED: the rejection is now durable and is the NEXT turn's selector input.
    let after = gm.engine.db.load_story_state(&session).await.unwrap().unwrap();
    assert_eq!(
        rejected_thread_ids(&after),
        vec!["thr_b".to_string()],
        "PresentationCommit drained the nomination → commit_story_writes persisted thr_b rejected; \
         next turn's P5.3 selector reads this and drops thr_b"
    );
}

/// OFF==baseline: with the flag OFF, draining a rejection nomination is a complete no-op —
/// story_state is byte-identical to the seed (the producer never persists anything).
#[tokio::test]
async fn presentation_commit_rejection_off_is_baseline_no_write() {
    use trpg_model::{StoryState, StoryThread, StoryThreadStatus};
    use trpg_runtime::director_brief::rejected_thread_ids;

    std::env::remove_var("TRPG_STORY_WRITE_LOOP"); // ensure OFF regardless of ordering
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

    let after = gm.engine.db.load_story_state(&session).await.unwrap().unwrap();
    assert_eq!(after, seed, "OFF: story_state byte-identical to seed (no write)");
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
    std::env::remove_var("TRPG_STORY_WRITE_LOOP");

    let after = gm.engine.db.load_story_state(&session).await.unwrap().unwrap();
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
    assert_eq!(noms.len(), 1, "同 fact 跨多次命中只提名一次（幂等去重，不双计）");
}

// ==================== MAT.M4 present vs met/engaged 剧透闸（§7-#6）====================
// 纯派生（present→met/unmet、Director 杠杆口径、guidance 反应式约束）在
// trpg_runtime::met_engaged 全量单测；Director 杠杆门在 trpg_director dehardcode_tests。
// 这里测 gm 侧两件「通道 + 不变量」事：(#1) un-met NPC 的 withheld secret 绝不进
// knowledge_basis（= facts_can_reveal）且绝不自动揭示——M4 约束只追加 prompt 文本、绝不
// 触碰 secret 门；(#3) Off/Shadow 下 guidance 不被追加约束（字节级基线）。

/// MAT.M4 #1：active-but-UN-MET NPC 的 withheld secret 永不进 knowledge_basis、永不自动揭示。
/// secret 门（facts_can_reveal vs facts_will_withhold）由既有 viewer_behavior_context 派生；
/// M4 的反应式约束（restrict_unmet_npc_guidance）只在 prompt 块尾追加文本，**绝不**把任何
/// withheld id 搬进 facts_can_reveal / knowledge_basis。DB-free。
#[test]
fn m4_unmet_npc_withheld_secret_never_enters_knowledge_basis() {
    use trpg_model::MaterializationAffordanceMode::Enforce;
    use trpg_model::{KnowledgeState, NpcKnowledgeEntry, NpcProfile, NpcRelationship,
        NpcRelationshipTarget, NpcMindView};
    use trpg_runtime::world::render_world_reaction_block;
    use trpg_runtime::{derive_met_engaged_gate, restrict_unmet_npc_guidance,
        viewer_behavior_context, derive_npc_behavior_plan};

    // NPC 知道一条真 fact，玩家方未知 → withheld secret（既有 v1 保守门）。
    let profile = NpcProfile { actor_id: "npc_unmet".into(), name: "Stranger".into(), ..Default::default() };
    let rel = NpcRelationship::new("s", "npc_unmet", NpcRelationshipTarget::PlayerParty).unwrap();
    let entries = vec![NpcKnowledgeEntry { fact_id: "secret_culprit".into(), state: KnowledgeState::KnowsTrue }];
    let view = NpcMindView::build("s", "npc_unmet", &profile, &[rel], &entries).unwrap();
    let ctx = viewer_behavior_context(&view, &[]); // 玩家方空知集 ⇒ 全 withheld
    let plan = derive_npc_behavior_plan(&view, &ctx);

    // secret 门不变量：withheld 含 secret，可揭集与 knowledge_basis 都不含它。
    assert!(plan.facts_will_withhold.iter().any(|f| f == "secret_culprit"));
    assert!(!plan.facts_can_reveal.iter().any(|f| f == "secret_culprit"));
    let cand = plan.to_reaction_candidate();
    assert!(!cand.knowledge_basis.iter().any(|f| f == "secret_culprit"),
        "knowledge_basis 仅源自 facts_can_reveal，withheld secret 绝不进");

    // M4 反应式约束追加后，仍不得把 secret 搬进可揭集：约束只提 NPC id，不提任何 fact id。
    let base = render_world_reaction_block(&[plan.clone()]);
    let active = vec!["npc_unmet".to_string()];
    let gate = derive_met_engaged_gate(&active, &[], Enforce); // 未暴露 ⇒ un-met
    let gated = restrict_unmet_npc_guidance(base, &gate).unwrap();
    assert!(gated.contains("[npc_presence_gate]"), "un-met ⇒ 追加反应式约束");
    assert!(gated.contains("npc_unmet"), "约束点名 un-met NPC id");
    // M4 追加的 [npc_presence_gate] 段本身只提 NPC id，绝不含任何 withheld secret fact id。
    let added = gated
        .split("[npc_presence_gate]")
        .nth(1)
        .expect("presence gate block present");
    assert!(!added.contains("secret_culprit"),
        "M4 约束段绝不泄露 / 提升 withheld secret（只提 NPC id，不触碰 secret 门）");
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
    let base = Some("[npc_behavior_guidance npc=npc_unmet]\n…\n[/npc_behavior_guidance]".to_string());
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

    use super::*;
    use crate::ledger::TurnLedger;
    use crate::tools::{AwaitingPlayerRoll, GmTool, ToolCtx, ToolError, ToolOutput, ToolRegistry, ToolSpec};
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
    use trpg_model::{ActorKind, ActorRef, ChatMessage, CheckContract, CheckResultRecord, CheckStakes, CheckTargetModel, CompiledContext, ContextRequest, DiceRollRecord, DueSource, DueStatus, MechanicDue, OppositionModel, RollAuthority, RollDisclosurePolicy, RollVisibility, RulingConfidence, RulingStatus, RuntimeState, TokenBudget, VisibilityProfile};
    use trpg_runtime::AutoRollExecution;

    struct MockLlm { scripts: Mutex<Vec<Vec<StreamEvent>>>, choices: Mutex<Vec<ToolChoice>>, requests: Mutex<Vec<Vec<Value>>> }

    #[async_trait]
    impl LlmClient for MockLlm {
        async fn complete_text(&self, _: Vec<ChatMessage>, _: f32) -> Result<String> { unimplemented!("MockLlm complete_text is unused by run_gm_turn tests") }
        async fn complete_json(&self, _: Vec<ChatMessage>, _: f32) -> Result<Value> { unimplemented!("MockLlm complete_json is unused by run_gm_turn tests") }
        async fn stream_chat(&self, _: Vec<ChatMessage>, _: f32) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> { unimplemented!("MockLlm stream_chat is unused by run_gm_turn tests") }
        async fn complete_with_tools(&self, _: Vec<Value>, _: Vec<Value>) -> Result<Value> { unimplemented!("MockLlm complete_with_tools is unused by run_gm_turn tests") }
        async fn stream_chat_with_tools(&self, messages: Vec<Value>, _: Vec<Value>, tool_choice: ToolChoice) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
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
    struct ScriptedTool { name: &'static str, record: bool, fail_code: Option<&'static str>, awaiting: bool }

    #[async_trait]
    impl GmTool for ScriptedTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec { name: self.name, schema: json!({"type":"function","function":{"name": self.name, "description":"test double","parameters":{"type":"object","properties":{}}}}) }
        }

        async fn call(&self, _ctx: &ToolCtx<'_>, ledger: &mut TurnLedger, _args: Value) -> Result<ToolOutput> {
            if let Some(code) = self.fail_code {
                return Err(ToolError::recoverable(code, "scripted failure", Some("switch to another tool".to_string())));
            }
            if self.record {
                let roll = DiceRollRecord { roll_id: "roll_secret".to_string(), session_id: "s".to_string(), turn_id: "t".to_string(), check_id: Some("check_x".to_string()), roller_kind: ActorKind::System, roller_id: None, visibility: RollVisibility::PrivateGmRoll, expression: "1d100".to_string(), result: json!({"total": 91}), seed_commitment: "seed".to_string(), revealed_at: None, created_at: Utc::now() };
                let result = CheckResultRecord { check_id: "check_x".to_string(), roll, outcome: json!({"total": 91}), committed_patches: vec![], created_at: Utc::now() };
                ledger.record_execution(&AutoRollExecution { primary: result, followups: vec![], roll_policy: "system_rolls_hidden".to_string() });
            }
            if self.awaiting {
                return Ok(ToolOutput::awaiting(json!({"check_id": "check_gate"}), AwaitingPlayerRoll { check_id: "check_gate".to_string(), prompt_public: "Roll 1d100 now.".to_string() }));
            }
            Ok(ToolOutput::ok(json!({"ok": true})))
        }
    }

    fn tool_call(name: &str) -> AggregatedToolCall {
        AggregatedToolCall { id: format!("call_{name}"), name: name.to_string(), arguments: "{}".to_string() }
    }

    /// lazy pool 不真连接；prepare_turn_context 经 ctx_provider seam 绕开（它
    /// 必须连真 DB + 已 parse 的 project bundle）；load_gm_skill fail-closed，
    /// 故 fixture 自带临时 gm_skill 目录。
    fn loop_fixture(scripts: Vec<Vec<StreamEvent>>, tools: ToolRegistry, max_tool_rounds: u8) -> (GmLoop, Arc<MockLlm>, ContextRequest, RuntimeState) {
        let pool = PgPoolOptions::new().connect_lazy("postgres://chatrpg:chatrpg@localhost:54347/chatrpg").expect("lazy pool");
        let engine = RuntimeEngine::new(Db { pool });
        let llm = Arc::new(MockLlm { scripts: Mutex::new(scripts), choices: Mutex::new(Vec::new()), requests: Mutex::new(Vec::new()) });
        let data_dir = std::env::temp_dir().join(format!("gm_loop_test_{}_{}", std::process::id(), uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(data_dir.join("agent/gm_skill/global")).unwrap();
        fs::write(data_dir.join("agent/gm_skill/global/10_test.md"), "test gm skill").unwrap();
        let mut gm = GmLoop::new(engine, llm.clone(), tools, LoopConfig { max_tool_rounds, repeat_finding_threshold: 3 }, data_dir);
        gm.ctx_provider = Some(Arc::new(|_req, _state| CompiledContext { prefix_text: "BP1".to_string(), pinned_text: "BP2".to_string(), dynamic_text: "BP3".to_string(), prefix_hash: "p".to_string(), pinned_hash: "m".to_string(), dynamic_hash: "d".to_string(), ..Default::default() }));
        let request = ContextRequest { ruleset_id: "rs".to_string(), module_id: None, session_id: "s".to_string(), turn_id: "t".to_string(), viewer: VisibilityProfile::gm(), token_budget: TokenBudget::default() };
        let state = RuntimeState { ruleset_id: "rs".to_string(), ..Default::default() };
        (gm, llm, request, state)
    }

    /// 合成 gate 用最小契约（字段全集对齐 trpg-agent gm_loop.rs sample_check）。
    fn gate_contract(check_id: &str) -> CheckContract {
        CheckContract {
            check_id: check_id.into(), session_id: "s".into(), turn_id: "t".into(), ruleset_id: "rs".into(), module_id: None,
            initiator: ActorRef { actor_id: "pc.current".into(), actor_kind: ActorKind::PlayerCharacter, display_name: Some("PC".into()) },
            target_actor: None, opposition: OppositionModel::NoMechanicalOpposition,
            action_summary: "climb".into(), intent_kind: "athletics".into(), check_label: "Climb".into(),
            dice_expression: "1d100".into(), modifiers: vec![],
            target: CheckTargetModel::StaticNumber { value: 50, label: "skill".into() },
            tested_parameter: None, opponent_tested_parameter: None, actor_snapshot_ids: vec![], source_refs: vec![], learned_packet_ids: vec![],
            roll_visibility: RollVisibility::PublicGmRoll, roll_authority: RollAuthority::System,
            disclosure: RollDisclosurePolicy::for_visibility(RollVisibility::PublicGmRoll),
            stakes: CheckStakes { before_roll_public: "risky".into(), success_public: "ok".into(), failure_public: "bad".into(), critical_public: None, fumble_public: None, success_patches_allowed: vec![], failure_patches_allowed: vec![], irreversible: false },
            confidence: RulingConfidence::High, ruling_status: RulingStatus::SourceBacked, advice_refs: vec![], expires_at_turn: Some("t".into()),
        }
    }

    fn gate_result(check_id: &str) -> CheckResultRecord {
        let roll = DiceRollRecord { roll_id: format!("roll_{check_id}"), session_id: "s".to_string(), turn_id: "t".to_string(), check_id: Some(check_id.to_string()), roller_kind: ActorKind::PlayerCharacter, roller_id: Some("pc.current".to_string()), visibility: RollVisibility::PublicGmRoll, expression: "1d100".to_string(), result: json!({"total": 27}), seed_commitment: "seed".to_string(), revealed_at: None, created_at: Utc::now() };
        CheckResultRecord { check_id: check_id.to_string(), roll, outcome: json!({"success": true}), committed_patches: vec![], created_at: Utc::now() }
    }

    #[tokio::test]
    async fn bare_roll_reply_resolves_open_gate_in_header() {
        // e2e must-fix 回归：玩家只回 "roll"（parse_roll_text → None）也必须走
        // 头部 gate 结算路径（wants_system_roll 兜底），而非让 agent 重复开 gate。
        let (mut gm, llm, request, state) = loop_fixture(vec![vec![StreamEvent::ContentDelta("done".to_string()), StreamEvent::Done { finish_reason: Some("stop".to_string()) }]], ToolRegistry::from_tools(vec![]), 1);
        let resolved = Arc::new(Mutex::new(false));
        let flag = resolved.clone();
        gm.gate_resolver = Some(Arc::new(move |input| {
            assert_eq!(input, "roll");
            *flag.lock().unwrap() = true;
            Some(Ok((gate_contract("check_gate"), gate_result("check_gate"))))
        }));
        let result = gm.run_gm_turn(GmTurnInput { request: &request, state: &state, user_input: "roll", history: &[], recent_transcript: None }, &mut |_| {}).await.unwrap();
        assert!(*resolved.lock().unwrap(), "header gate resolution path was not taken for bare roll reply");
        assert!(matches!(result, TurnOutcome::Narration(_)));
        // 结算事实必须经 dynamic tail 注入本回合上下文（与 gate 结算事实同通道）。
        let first_request = llm.requests.lock().unwrap()[0].clone();
        let tail = first_request.last().and_then(|m| m.get("content")).and_then(Value::as_str).unwrap_or("");
        assert!(tail.contains("[Resolved Gate Facts]"), "dynamic tail missing gate facts: {tail}");
        assert!(tail.contains("Resolved pending check check_gate"));
    }

    #[tokio::test]
    async fn gate_resolution_error_is_nonfatal_and_becomes_turn_fact() {
        // e2e must-fix 回归：报点回复结算 Err（如 reported totals 被禁）绝不
        // 终止会话循环——run_gm_turn 返回 Ok，失败原因作为事实注入本回合上下文。
        let (mut gm, llm, request, state) = loop_fixture(vec![vec![StreamEvent::ContentDelta("请重新掷骰。".to_string()), StreamEvent::Done { finish_reason: Some("stop".to_string()) }]], ToolRegistry::from_tools(vec![]), 1);
        gm.gate_resolver = Some(Arc::new(|_| Some(Err(anyhow::anyhow!("player-reported roll totals are disabled")))));
        let result = gm.run_gm_turn(GmTurnInput { request: &request, state: &state, user_input: "34", history: &[], recent_transcript: None }, &mut |_| {}).await;
        let outcome = result.expect("gate resolution error must not abort the turn");
        assert_eq!(outcome, TurnOutcome::Narration("请重新掷骰。".to_string()));
        let first_request = llm.requests.lock().unwrap()[0].clone();
        let tail = first_request.last().and_then(|m| m.get("content")).and_then(Value::as_str).unwrap_or("");
        assert!(tail.contains("Pending check resolution failed"), "failure fact missing from dynamic tail: {tail}");
        assert!(tail.contains("player-reported roll totals are disabled"));
    }

    #[tokio::test]
    async fn content_delta_streams_to_callback() {
        let (mut gm, _llm, request, state) = loop_fixture(vec![vec![StreamEvent::ContentDelta("A".to_string()), StreamEvent::ContentDelta("B".to_string()), StreamEvent::Done { finish_reason: Some("stop".to_string()) }]], ToolRegistry::from_tools(vec![]), 1);
        let mut out = String::new();
        let result = gm.run_gm_turn(GmTurnInput { request: &request, state: &state, user_input: "go", history: &[], recent_transcript: None }, &mut |d| out.push_str(d)).await.unwrap();
        assert_eq!(out, "AB");
        assert_eq!(result, TurnOutcome::Narration("AB".to_string()));
    }

    #[tokio::test]
    async fn debt_free_round_streams_deltas_incrementally() {
        // D2 真流式回归（B6 门控前移终审修复）：无债回合的 ContentDelta 必须
        // 逐块到达 on_delta（恢复一期行为）——缓冲到流结束一次性 flush 会把
        // 两块合并成单次回调，此断言即红。
        let scripts = vec![vec![StreamEvent::ContentDelta("第一块。".to_string()), StreamEvent::ContentDelta("第二块。".to_string()), StreamEvent::Done { finish_reason: Some("stop".to_string()) }]];
        let (mut gm, _llm, request, state) = loop_fixture(scripts, ToolRegistry::from_tools(vec![]), 1);
        let chunks = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = chunks.clone();
        let result = gm.run_gm_turn(GmTurnInput { request: &request, state: &state, user_input: "go", history: &[], recent_transcript: None }, &mut move |d| sink.lock().unwrap().push(d.to_string())).await.unwrap();
        assert_eq!(*chunks.lock().unwrap(), vec!["第一块。".to_string(), "第二块。".to_string()], "deltas must reach on_delta chunk-by-chunk, not as one flush");
        assert_eq!(result, TurnOutcome::Narration("第一块。第二块。".to_string()));
    }

    #[tokio::test]
    async fn max_rounds_forces_tool_choice_none() {
        // max_tool_rounds=1：第 1 轮 Auto 仍要工具 → 循环耗尽 → 追加唯一一轮 None。
        // tool_choice 序列必须是 [Auto, None]（循环内绝不提前 None、绝无双重 None）。
        let (mut gm, llm, request, state) = loop_fixture(vec![vec![StreamEvent::ToolCalls(vec![]), StreamEvent::Done { finish_reason: Some("tool_calls".to_string()) }], vec![StreamEvent::ContentDelta("final".to_string()), StreamEvent::Done { finish_reason: Some("stop".to_string()) }]], ToolRegistry::from_tools(vec![]), 1);
        let mut out = String::new();
        let _ = gm.run_gm_turn(GmTurnInput { request: &request, state: &state, user_input: "go", history: &[], recent_transcript: None }, &mut |d| out.push_str(d)).await.unwrap();
        let choices = llm.choices.lock().unwrap().clone();
        assert_eq!(choices, vec![ToolChoice::Auto, ToolChoice::None]);
        assert_eq!(out, "final");
    }

    #[tokio::test]
    async fn tool_products_enter_ledger_and_private_tokens_are_redacted() {
        // spec §9 场景 1+4+8（单测可观测面）：工具轮产物入账 →
        // 下一轮 content 里的私骰 token（"91"）被滑动缓冲过滤。
        let tools = ToolRegistry::from_tools(vec![Box::new(ScriptedTool { name: "roll_check", record: true, fail_code: None, awaiting: false })]);
        let scripts = vec![
            vec![StreamEvent::ToolCalls(vec![tool_call("roll_check")]), StreamEvent::Done { finish_reason: Some("tool_calls".to_string()) }],
            vec![StreamEvent::ContentDelta("The hidden total is 9".to_string()), StreamEvent::ContentDelta("1, but you only see shadows.".to_string()), StreamEvent::Done { finish_reason: Some("stop".to_string()) }],
        ];
        let (mut gm, llm, request, state) = loop_fixture(scripts, tools, 4);
        let mut out = String::new();
        let result = gm.run_gm_turn(GmTurnInput { request: &request, state: &state, user_input: "go", history: &[], recent_transcript: None }, &mut |d| out.push_str(d)).await.unwrap();
        assert!(matches!(result, TurnOutcome::Narration(_)));
        assert!(!out.contains("91"), "private roll token leaked: {out}");
        assert!(out.contains("■"));
        // 第二轮请求里必须有第一轮的 tool 回填消息（账本/消息尾部追加联动）。
        let second_request = llm.requests.lock().unwrap()[1].clone();
        assert!(second_request.iter().any(|m| m.get("role").and_then(Value::as_str) == Some("tool") && m.get("name").and_then(Value::as_str) == Some("roll_check")));
    }

    /// 入账一笔玩家可见掷骰结果的替身（spec §9 场景 5 用：叙事若漏提任何可见
    /// token，流后 NarrationVerifier 应产出 OmittedVisibleResult finding）。
    struct VisibleRollTool;

    #[async_trait]
    impl GmTool for VisibleRollTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec { name: "roll_check", schema: json!({"type":"function","function":{"name":"roll_check","description":"test double","parameters":{"type":"object","properties":{}}}}) }
        }

        async fn call(&self, _ctx: &ToolCtx<'_>, ledger: &mut TurnLedger, _args: Value) -> Result<ToolOutput> {
            let roll = DiceRollRecord { roll_id: "roll_pub".to_string(), session_id: "s".to_string(), turn_id: "t".to_string(), check_id: Some("check_pub".to_string()), roller_kind: ActorKind::System, roller_id: None, visibility: RollVisibility::PublicGmRoll, expression: "1d100".to_string(), result: json!({"total": 42}), seed_commitment: "seed".to_string(), revealed_at: None, created_at: Utc::now() };
            let result = CheckResultRecord { check_id: "check_pub".to_string(), roll, outcome: json!({"total": 42}), committed_patches: vec![], created_at: Utc::now() };
            ledger.record_execution(&AutoRollExecution { primary: result, followups: vec![], roll_policy: "system_rolls_visible".to_string() });
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
            vec![StreamEvent::ToolCalls(vec![tool_call("roll_check")]), StreamEvent::Done { finish_reason: Some("tool_calls".to_string()) }],
            vec![StreamEvent::ContentDelta("你看见影子晃动。".to_string()), StreamEvent::Done { finish_reason: Some("stop".to_string()) }],
            vec![StreamEvent::ContentDelta("ok".to_string()), StreamEvent::Done { finish_reason: Some("stop".to_string()) }],
        ];
        let (mut gm, llm, request, state) = loop_fixture(scripts, tools, 4);
        let mut sink = String::new();
        let _ = gm.run_gm_turn(GmTurnInput { request: &request, state: &state, user_input: "go", history: &[], recent_transcript: None }, &mut |d| sink.push_str(d)).await.unwrap();
        assert_eq!(gm.errata.kind_counts().get("omitted_visible_result"), Some(&1));
        // 第二回合：dynamic tail 必须携带上一回合的勘误块。
        let request2 = ContextRequest { turn_id: "t2".to_string(), ..request.clone() };
        let _ = gm.run_gm_turn(GmTurnInput { request: &request2, state: &state, user_input: "继续", history: &[], recent_transcript: None }, &mut |d| sink.push_str(d)).await.unwrap();
        let turn2_request = llm.requests.lock().unwrap()[2].clone();
        let tail_content = turn2_request.last().and_then(|m| m.get("content")).and_then(Value::as_str).unwrap_or("");
        assert!(tail_content.contains("[gm_errata]"), "dynamic tail missing errata block: {tail_content}");
        assert!(tail_content.contains("omitted_visible_result"));
    }

    /// B6 测试用最小 open due（零 per-ruleset 内容：track/desc 是测试夹具语义）。
    fn seeded_due(id: &str, desc: &str) -> MechanicDue {
        MechanicDue { due_id: id.to_string(), session_id: "s".to_string(), turn_id: "t".to_string(), source: DueSource::Threshold, source_track: Some("sanity".to_string()), hook_event: None, mechanic_id: None, threshold_desc: desc.to_string(), followup_procedure_id: None, owner_kind: "actor".to_string(), owner_id: "pc.current".to_string(), evidence: json!({"before": 38, "after": 32, "delta": -6}), status: DueStatus::Open, created_at: Utc::now() }
    }

    fn waive_call(target_id: &str) -> AggregatedToolCall {
        AggregatedToolCall { id: "call_waive".to_string(), name: "waive_obligation".to_string(), arguments: format!("{{\"target_id\":\"{target_id}\",\"reason\":\"handled in fiction\"}}") }
    }

    #[tokio::test]
    async fn open_due_blocks_narration_round() {
        // spec 验收 5（D2 真流式修复后语义）：ObligationLedger 有未处理 due →
        // 轮前门控判定该轮被门（blocked），该轮 content 实时丢弃、绝不流给
        // on_delta（不再缓冲后 flush）；下一轮轮前回填 block_text() 的 system
        // 观察继续循环（round 0 由 dynamic tail 的 obligations_block 告知，
        // 不重复回填）；waive 后 content 恢复直通流出。
        let tools = ToolRegistry::from_tools(vec![Box::new(crate::tools::mechanic::WaiveObligationTool)]);
        let scripts = vec![
            vec![StreamEvent::ContentDelta("premature ending".to_string()), StreamEvent::Done { finish_reason: Some("stop".to_string()) }],
            vec![StreamEvent::ToolCalls(vec![waive_call("due_block")]), StreamEvent::Done { finish_reason: Some("tool_calls".to_string()) }],
            vec![StreamEvent::ContentDelta("clean ending".to_string()), StreamEvent::Done { finish_reason: Some("stop".to_string()) }],
        ];
        let (mut gm, llm, request, state) = loop_fixture(scripts, tools, 4);
        gm.obligations.absorb_dues(vec![seeded_due("due_block", "sanity threshold crossed")]);
        let mut out = String::new();
        let result = gm.run_gm_turn(GmTurnInput { request: &request, state: &state, user_input: "go", history: &[], recent_transcript: None }, &mut |d| out.push_str(d)).await.unwrap();
        assert!(!out.contains("premature ending"), "blocked round content leaked: {out}");
        assert_eq!(out, "clean ending");
        assert_eq!(result, TurnOutcome::Narration("clean ending".to_string()));
        // 第二轮请求里必须带 block_text() 的 system 观察回填。
        let second_request = llm.requests.lock().unwrap()[1].clone();
        let observation = second_request.iter().rev().find(|m| m.get("role").and_then(Value::as_str) == Some("system")).and_then(|m| m.get("content")).and_then(Value::as_str).unwrap_or("").to_string();
        assert!(observation.contains("[obligations]"), "missing obligations observation: {observation}");
        assert!(observation.contains("sanity threshold crossed"));
        assert!(observation.contains("due_block"));
    }

    #[tokio::test]
    async fn waive_emits_audit_and_unblocks() {
        // waive 带理由 → 审计 MemoryEvent tags 含 "gm_waive"（MockLlm 路径断言
        // 载荷，不连 DB）+ blocking() 清空放行。
        let tools = ToolRegistry::from_tools(vec![Box::new(crate::tools::mechanic::WaiveObligationTool)]);
        let scripts = vec![
            vec![StreamEvent::ToolCalls(vec![waive_call("due_audit")]), StreamEvent::Done { finish_reason: Some("tool_calls".to_string()) }],
            vec![StreamEvent::ContentDelta("onward".to_string()), StreamEvent::Done { finish_reason: Some("stop".to_string()) }],
        ];
        let (mut gm, llm, request, state) = loop_fixture(scripts, tools, 4);
        gm.obligations.absorb_dues(vec![seeded_due("due_audit", "chaos pool overflow")]);
        let mut out = String::new();
        let _ = gm.run_gm_turn(GmTurnInput { request: &request, state: &state, user_input: "go", history: &[], recent_transcript: None }, &mut |d| out.push_str(d)).await.unwrap();
        assert_eq!(out, "onward");
        assert!(gm.obligations.blocking().is_empty(), "waive must unblock the ledger");
        // 审计载荷（纯构造，不打 DB）：tags 必含 "gm_waive"。
        let record = crate::obligations::WaiverRecord { target_id: "due_audit".to_string(), reason: "handled in fiction".to_string(), scope: crate::obligations::WaiveScope::Turn, turn_id: "t".to_string() };
        let event = crate::obligations::waiver_to_memory_event(&request, &record);
        assert!(event.tags.iter().any(|t| t == "gm_waive"), "audit tags missing gm_waive: {:?}", event.tags);
        // 工具结果回填同样携带审计 tags（agent 可见可审计）。
        let second_request = llm.requests.lock().unwrap()[1].clone();
        let tool_content = second_request.iter().find(|m| m.get("role").and_then(Value::as_str) == Some("tool")).and_then(|m| m.get("content")).and_then(Value::as_str).unwrap_or("");
        assert!(tool_content.contains("gm_waive"), "audit tags missing from tool result: {tool_content}");
    }

    #[tokio::test]
    async fn round_exhaustion_carries_debt_to_next_turn() {
        // 轮耗尽仍有债务 → ToolChoice::None 强制叙事语义不变（一期断言照绿）+
        // 债务跨回合存活 + 下回合 assemble 的 dynamic tail 含 obligations_block。
        let scripts = vec![
            vec![StreamEvent::ContentDelta("blocked draft".to_string()), StreamEvent::Done { finish_reason: Some("stop".to_string()) }],
            vec![StreamEvent::ContentDelta("forced ending".to_string()), StreamEvent::Done { finish_reason: Some("stop".to_string()) }],
            vec![StreamEvent::ContentDelta("blocked again".to_string()), StreamEvent::Done { finish_reason: Some("stop".to_string()) }],
            vec![StreamEvent::ContentDelta("forced again".to_string()), StreamEvent::Done { finish_reason: Some("stop".to_string()) }],
        ];
        let (mut gm, llm, request, state) = loop_fixture(scripts, ToolRegistry::from_tools(vec![]), 1);
        gm.obligations.absorb_dues(vec![seeded_due("due_carry", "sanity threshold crossed")]);
        let mut out = String::new();
        let result = gm.run_gm_turn(GmTurnInput { request: &request, state: &state, user_input: "go", history: &[], recent_transcript: None }, &mut |d| out.push_str(d)).await.unwrap();
        assert_eq!(llm.choices.lock().unwrap()[..2], vec![ToolChoice::Auto, ToolChoice::None]);
        assert_eq!(out, "forced ending");
        assert!(matches!(result, TurnOutcome::Narration(_)));
        // GmLoop.obligations 持久字段含 carryover。
        assert!(!gm.obligations.blocking().is_empty(), "debt must survive the turn");
        assert!(gm.obligations.carryover_block().is_some());
        // 下回合 assemble 的 dynamic tail 含 obligations_block。
        let request2 = ContextRequest { turn_id: "t2".to_string(), ..request.clone() };
        let _ = gm.run_gm_turn(GmTurnInput { request: &request2, state: &state, user_input: "继续", history: &[], recent_transcript: None }, &mut |d| out.push_str(d)).await.unwrap();
        let turn2_request = llm.requests.lock().unwrap()[2].clone();
        let tail = turn2_request.last().and_then(|m| m.get("content")).and_then(Value::as_str).unwrap_or("");
        assert!(tail.contains("[obligations_carryover]"), "tail missing obligations_block: {tail}");
        assert!(tail.contains("sanity threshold crossed"));
    }

    #[tokio::test]
    async fn invented_effect_becomes_retro_debt() {
        // spec 验收 11 单测侧：verify_after_stream 抓到 InventedEffect →
        // absorb_retro_debts → 下回合 blocking() 含该 debt_id。
        let scripts = vec![vec![StreamEvent::ContentDelta("你失去了 3 点理智。".to_string()), StreamEvent::Done { finish_reason: Some("stop".to_string()) }]];
        let (mut gm, _llm, request, state) = loop_fixture(scripts, ToolRegistry::from_tools(vec![]), 2);
        let mut out = String::new();
        let _ = gm.run_gm_turn(GmTurnInput { request: &request, state: &state, user_input: "go", history: &[], recent_transcript: None }, &mut |d| out.push_str(d)).await.unwrap();
        assert_eq!(gm.errata.kind_counts().get("invented_effect"), Some(&1));
        let blocking = gm.obligations.blocking();
        assert_eq!(blocking.len(), 1, "exactly one retro debt expected: {blocking:?}");
        assert_eq!(blocking[0].kind, "debt");
        assert!(blocking[0].target_id.starts_with("debt_"), "debt_id shape: {}", blocking[0].target_id);
    }

    #[tokio::test]
    async fn recoverable_tool_error_lets_agent_switch_to_player_roll() {
        // spec §9 场景 7 + 2（终态侧）：missing_kernel_dice 结构化错误回填 →
        // agent 第二轮改调 request_player_roll → AwaitingPlayerRoll 终态。
        // （"下回合骰值回复 → 头部结算"需真 DB pending check，归 Task 11 e2e。）
        let tools = ToolRegistry::from_tools(vec![
            Box::new(ScriptedTool { name: "roll_check", record: false, fail_code: Some("missing_kernel_dice"), awaiting: false }),
            Box::new(ScriptedTool { name: "request_player_roll", record: false, fail_code: None, awaiting: true }),
        ]);
        let scripts = vec![
            vec![StreamEvent::ToolCalls(vec![tool_call("roll_check")]), StreamEvent::Done { finish_reason: Some("tool_calls".to_string()) }],
            vec![StreamEvent::ToolCalls(vec![tool_call("request_player_roll")]), StreamEvent::Done { finish_reason: Some("tool_calls".to_string()) }],
        ];
        let (mut gm, llm, request, state) = loop_fixture(scripts, tools, 4);
        let mut out = String::new();
        let result = gm.run_gm_turn(GmTurnInput { request: &request, state: &state, user_input: "go", history: &[], recent_transcript: None }, &mut |d| out.push_str(d)).await.unwrap();
        assert_eq!(result, TurnOutcome::AwaitingPlayerRoll { check_id: "check_gate".to_string(), prompt_public: "Roll 1d100 now.".to_string() });
        // 第二轮请求携带第一轮的结构化错误回填（agent 可见可修复）。
        let second_request = llm.requests.lock().unwrap()[1].clone();
        let error_content = second_request.iter().find(|m| m.get("role").and_then(Value::as_str) == Some("tool")).and_then(|m| m.get("content")).and_then(Value::as_str).unwrap_or("");
        assert!(error_content.contains("missing_kernel_dice"));
        assert!(error_content.contains("\"recoverable\":true"));
    }

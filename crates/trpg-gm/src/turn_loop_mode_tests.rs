//! 批2 姿态回合级集成测试：交锋簇债务收紧（cluster 未闭合 → content 终态被
//! 债务观察拦截，复用 B6 测试模式）+ novelty 战术事实注入。需要 :54347 真库
//! 落 combat frame（mode 由 active frame 推导）；LLM 走 MockLlm 脚本。
use super::*;
use crate::tools::ToolRegistry;
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
    CheckTargetModel, CompiledContext, ContextRequest, DiceRollRecord, FrameKind, FrameStatus,
    MechanicEntry, MechanicKind, OppositionModel, RollAuthority, RollDisclosurePolicy,
    RollVisibility, RuleKernel, RulingConfidence, RulingStatus, RuntimeState, Scope, ScopeType,
    StateFrame, TokenBudget, VisibilityProfile,
};

struct MockLlm {
    scripts: Mutex<Vec<Vec<StreamEvent>>>,
    requests: Mutex<Vec<Vec<Value>>>,
}

#[async_trait]
impl LlmClient for MockLlm {
    async fn complete_text(&self, _: Vec<ChatMessage>, _: f32) -> Result<String> {
        unimplemented!()
    }
    // 刺激预 pass 走 complete_json：mock 恒回空命中（语义="本回合无刺激"），
    // 不消费 scripts——脚本只属于 stream_chat_with_tools 的工具轮。
    async fn complete_json(&self, _: Vec<ChatMessage>, _: f32) -> Result<Value> {
        Ok(json!({"hits": []}))
    }
    async fn stream_chat(
        &self,
        _: Vec<ChatMessage>,
        _: f32,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        unimplemented!()
    }
    async fn complete_with_tools(&self, _: Vec<Value>, _: Vec<Value>) -> Result<Value> {
        unimplemented!()
    }
    async fn stream_chat_with_tools(
        &self,
        messages: Vec<Value>,
        _: Vec<Value>,
        _: ToolChoice,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        self.requests.lock().unwrap().push(messages);
        let script = self.scripts.lock().unwrap().remove(0);
        let s = try_stream! { for event in script { yield event; } };
        Ok(Box::pin(s))
    }
}

/// 真库 fixture：唯一 session + 落一个 live Combat frame + data_dir 带 combat
/// mode 包（manifest tempo 收紧 + catalog_filter + global.md 标记文本）；
/// ctx_provider 绕开真投影。ruleset_id 参数化（目录联动测试需唯一 kernel）。
async fn mode_fixture(
    scripts: Vec<Vec<StreamEvent>>,
    working_state: Value,
    install_manifest: bool,
    ruleset_id: &str,
) -> (GmLoop, Arc<MockLlm>, ContextRequest, RuntimeState) {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://chatrpg:chatrpg@localhost:54347/chatrpg")
        .expect("lazy pool");
    let engine = RuntimeEngine::new(Db { pool });
    let session_id = format!("mode_loop_test_{}", uuid::Uuid::new_v4().simple());
    let now = Utc::now();
    let frame = StateFrame {
        frame_id: format!("frame_{}", uuid::Uuid::new_v4().simple()),
        frame_kind: FrameKind::Combat,
        session_id: session_id.clone(),
        ruleset_id: ruleset_id.into(),
        module_id: None,
        parent_frame_id: None,
        scope: Scope {
            scope_type: ScopeType::Session,
            scope_id: session_id.clone(),
        },
        status: FrameStatus::Active,
        title: "combat mode".into(),
        objective: "test".into(),
        static_refs: vec![],
        working_state,
        active_gate_ids: vec![],
        local_clocks: vec![],
        local_facts: vec![],
        local_modifiers: vec![],
        event_count: 0,
        last_event_ids: vec![],
        retention_policy: Default::default(),
        compaction_policy: Default::default(),
        created_at: now,
        updated_at: now,
    };
    engine
        .db
        .upsert_state_frame(&frame)
        .await
        .expect("seed combat frame (db at :54347 required)");
    let llm = Arc::new(MockLlm {
        scripts: Mutex::new(scripts),
        requests: Mutex::new(Vec::new()),
    });
    let data_dir = std::env::temp_dir().join(format!(
        "gm_mode_loop_{}_{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    fs::create_dir_all(data_dir.join("agent/gm_skill/global")).unwrap();
    fs::write(
        data_dir.join("agent/gm_skill/global/10_test.md"),
        "test gm skill",
    )
    .unwrap();
    if install_manifest {
        fs::create_dir_all(data_dir.join("agent/gm_skill/modes/combat")).unwrap();
        fs::write(
            data_dir.join("agent/gm_skill/modes/combat/manifest.json"),
            r#"{"mode_id":"combat","frame_kind":"combat","extra_tools":["open_combat_frame","close_frame"],"catalog_filter":{"kinds":["reaction"]},"tempo":{"effect_closure_per_cluster":true}}"#,
        ).unwrap();
        fs::write(
            data_dir.join("agent/gm_skill/modes/combat/global.md"),
            "[combat-mode-marker] cinematic",
        )
        .unwrap();
    }
    let mut gm = GmLoop::new(
        engine,
        llm.clone(),
        ToolRegistry::from_tools(vec![]),
        LoopConfig {
            max_tool_rounds: 4,
            repeat_finding_threshold: 3,
        },
        data_dir,
    );
    gm.ctx_provider = Some(Arc::new(|_req, _state| CompiledContext {
        prefix_text: "BP1".into(),
        pinned_text: "BP2".into(),
        dynamic_text: "BP3".into(),
        prefix_hash: "p".into(),
        pinned_hash: "m".into(),
        dynamic_hash: "d".into(),
        ..Default::default()
    }));
    let request = ContextRequest {
        ruleset_id: ruleset_id.into(),
        module_id: None,
        session_id,
        turn_id: "t".into(),
        viewer: VisibilityProfile::gm(),
        token_budget: TokenBudget::default(),
    };
    let state = RuntimeState {
        ruleset_id: ruleset_id.into(),
        ..Default::default()
    };
    (gm, llm, request, state)
}

async fn cleanup(gm: &GmLoop, session_id: &str) {
    let _ = sqlx::query("delete from memory_events where session_id = $1")
        .bind(session_id)
        .execute(&gm.engine.db.pool)
        .await;
    let _ = sqlx::query("delete from world_events where session_id = $1")
        .bind(session_id)
        .execute(&gm.engine.db.pool)
        .await;
    let _ = sqlx::query("delete from turns where session_id = $1")
        .bind(session_id)
        .execute(&gm.engine.db.pool)
        .await;
    let _ = sqlx::query("delete from state_frames where session_id = $1")
        .bind(session_id)
        .execute(&gm.engine.db.pool)
        .await;
    fs::remove_dir_all(&gm.data_dir).ok();
}

fn gate_contract(check_id: &str, session_id: &str) -> CheckContract {
    CheckContract {
        check_id: check_id.into(),
        session_id: session_id.into(),
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
        action_summary: "attack".into(),
        intent_kind: "attack".into(),
        check_label: "Fighting".into(),
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
            success_public: "hit".into(),
            failure_public: "miss".into(),
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

fn gate_result(check_id: &str, session_id: &str) -> CheckResultRecord {
    let roll = DiceRollRecord {
        roll_id: format!("roll_{check_id}"),
        session_id: session_id.into(),
        turn_id: "t".into(),
        check_id: Some(check_id.into()),
        roller_kind: ActorKind::PlayerCharacter,
        roller_id: Some("pc.current".into()),
        visibility: RollVisibility::PublicGmRoll,
        expression: "1d100".into(),
        result: json!({"total": 27}),
        seed_commitment: "seed".into(),
        revealed_at: None,
        created_at: Utc::now(),
    };
    CheckResultRecord {
        check_id: check_id.into(),
        roll,
        outcome: json!({"success": true}),
        committed_patches: vec![],
        created_at: Utc::now(),
    }
}

#[tokio::test]
async fn combat_tempo_blocks_unclosed_cluster_until_waived() {
    // B6 模式复用（spec §4.6 战斗收紧）：gate 结算入账一笔已结算检定（命中）
    // 但零效果落账 → 簇未闭合 → 纯叙事终态被拦（content 丢弃）→ 债务观察列出
    // cluster 债 → agent waive 带理由 → 放行。mode 由 db 里的 Combat frame 推导。
    let scripts = vec![
        vec![
            StreamEvent::ContentDelta("过早收尾".to_string()),
            StreamEvent::Done {
                finish_reason: Some("stop".to_string()),
            },
        ],
        vec![
            StreamEvent::ToolCalls(vec![AggregatedToolCall {
                id: "c1".into(),
                name: "waive_obligation".into(),
                arguments: "{\"target_id\":\"cluster.t\",\"reason\":\"敌人逃散，后果并入下一幕\"}"
                    .into(),
            }]),
            StreamEvent::Done {
                finish_reason: Some("tool_calls".to_string()),
            },
        ],
        vec![
            StreamEvent::ContentDelta("干净收尾".to_string()),
            StreamEvent::Done {
                finish_reason: Some("stop".to_string()),
            },
        ],
    ];
    let (mut gm, llm, request, state) =
        mode_fixture(scripts, json!({"mode_id":"combat"}), true, "rs").await;
    let sid = request.session_id.clone();
    gm.gate_resolver = Some(Arc::new(move |_| {
        Some(Ok((
            gate_contract("check_hit", &sid),
            gate_result("check_hit", &sid),
        )))
    }));
    let mut out = String::new();
    let result = gm
        .run_gm_turn(
            GmTurnInput {
                request: &request,
                state: &state,
                user_input: "roll",
                history: &[],
                recent_transcript: None,
            },
            &mut |d| out.push_str(d),
        )
        .await
        .unwrap();
    assert!(
        !out.contains("过早收尾"),
        "unclosed cluster round content must be discarded: {out}"
    );
    assert_eq!(out, "干净收尾");
    assert!(matches!(result, TurnOutcome::Narration(_)));
    // mode 提示覆盖层已生效（system 消息含 combat global.md 标记）。
    let first_request = llm.requests.lock().unwrap()[0].clone();
    let system = first_request[0]
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        system.contains("[combat-mode-marker]"),
        "combat prompt overlay missing: {system}"
    );
    // 第二轮请求带 cluster 债务观察回填（B6 同通道）。
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
        observation.contains("cluster.t"),
        "cluster debt must appear in the obligations observation: {observation}"
    );
    cleanup(&gm, &request.session_id).await;
}

#[tokio::test]
async fn novelty_facts_injected_into_dynamic_tail_only_in_mode() {
    // frame 状态含已用战术 + combat mode 激活 → dynamic tail 出现 [combat_novelty]
    // 事实块（一期 novelty 数据复用，零额外 LLM 调用）。
    let tactics = json!({"mode_id":"combat","npc_tactic_memory":[{"npc_id":"npc.ghoul","tactic_id":"ambush_from_dark","used_count_in_frame":2,"consecutive_count":2,"last_result":null,"last_turn_id":"t0"}]});
    let scripts = vec![vec![
        StreamEvent::ContentDelta("ok".to_string()),
        StreamEvent::Done {
            finish_reason: Some("stop".to_string()),
        },
    ]];
    let (mut gm, llm, request, state) = mode_fixture(scripts, tactics.clone(), true, "rs").await;
    let mut out = String::new();
    let _ = gm
        .run_gm_turn(
            GmTurnInput {
                request: &request,
                state: &state,
                user_input: "我环顾四周",
                history: &[],
                recent_transcript: None,
            },
            &mut |d| out.push_str(d),
        )
        .await
        .unwrap();
    let first_request = llm.requests.lock().unwrap()[0].clone();
    let tail = first_request
        .last()
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        tail.contains("[combat_novelty]"),
        "novelty facts missing from dynamic tail: {tail}"
    );
    assert!(tail.contains("ambush_from_dark"));
    cleanup(&gm, &request.session_id).await;

    // 同样的 frame 战术数据，但无 mode 包（mode=None）→ 绝不注入（二期一致）。
    let scripts = vec![vec![
        StreamEvent::ContentDelta("ok".to_string()),
        StreamEvent::Done {
            finish_reason: Some("stop".to_string()),
        },
    ]];
    let (mut gm, llm, request, state) = mode_fixture(scripts, tactics, false, "rs").await;
    let _ = gm
        .run_gm_turn(
            GmTurnInput {
                request: &request,
                state: &state,
                user_input: "我环顾四周",
                history: &[],
                recent_transcript: None,
            },
            &mut |d| {},
        )
        .await
        .unwrap();
    let first_request = llm.requests.lock().unwrap()[0].clone();
    let tail = first_request
        .last()
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        !tail.contains("[combat_novelty]"),
        "mode=None path must stay byte-identical to phase 2: {tail}"
    );
    cleanup(&gm, &request.session_id).await;
}

#[tokio::test]
async fn mode_catalog_subset_rides_mode_prompt_layer_and_none_path_has_no_section() {
    // 终审返工 critical-2（spec §4.5 目录联动）：mode 激活 → kernel
    // mechanics_catalog 经 manifest.catalog_filter（kinds=["reaction"]）过滤，
    // 渲染进 mode 提示层（system 消息、mode 段之后）；mode=None 即使 kernel
    // 在库也恒无该节（二期字节回归语义）。kernel 用唯一 ruleset_id 隔离共库。
    let ruleset_id = format!("rs_cat_{}", uuid::Uuid::new_v4().simple());
    let kernel = RuleKernel {
        kernel_id: format!("kernel_{}", uuid::Uuid::new_v4().simple()),
        ruleset_id: ruleset_id.clone(),
        version: "1".into(),
        mechanics_catalog: vec![
            MechanicEntry {
                id: "shield_parry".into(),
                name: "Shield Parry".into(),
                kind: MechanicKind::Reaction,
                when_to_use: "when attacked in melee".into(),
                ..Default::default()
            },
            MechanicEntry {
                id: "library_use".into(),
                name: "Library Use".into(),
                kind: MechanicKind::SkillCheck,
                when_to_use: "research between sessions".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let scripts = vec![vec![
        StreamEvent::ContentDelta("ok".to_string()),
        StreamEvent::Done {
            finish_reason: Some("stop".to_string()),
        },
    ]];
    let (mut gm, llm, request, state) =
        mode_fixture(scripts, json!({"mode_id":"combat"}), true, &ruleset_id).await;
    gm.engine
        .db
        .upsert_rule_kernel(&kernel)
        .await
        .expect("seed kernel (db at :54347 required)");
    let _ = gm
        .run_gm_turn(
            GmTurnInput {
                request: &request,
                state: &state,
                user_input: "我举盾",
                history: &[],
                recent_transcript: None,
            },
            &mut |_| {},
        )
        .await
        .unwrap();
    let first_request = llm.requests.lock().unwrap()[0].clone();
    let system = first_request[0]
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    assert!(
        system.contains("[mode 目录子集]"),
        "catalog subset section missing from mode prompt layer: {system}"
    );
    assert!(
        system.contains("shield_parry | Shield Parry | when attacked in melee"),
        "matching entry line missing: {system}"
    );
    assert!(
        !system.contains("library_use"),
        "filtered-out entry must not be injected: {system}"
    );
    // 注入位置：mode 提示段（global.md 标记）之后——同生命周期、同有因失效。
    let marker = system
        .find("[combat-mode-marker]")
        .expect("mode overlay must be present");
    let section = system
        .find("[mode 目录子集]")
        .expect("section must be present");
    assert!(
        marker < section,
        "catalog subset must ride AFTER the mode prompt overlay: {system}"
    );
    cleanup(&gm, &request.session_id).await;

    // mode=None（无 mode 包，kernel 仍在库）→ 该节恒不存在。
    let scripts = vec![vec![
        StreamEvent::ContentDelta("ok".to_string()),
        StreamEvent::Done {
            finish_reason: Some("stop".to_string()),
        },
    ]];
    let (mut gm, llm, request, state) =
        mode_fixture(scripts, json!({"mode_id":"combat"}), false, &ruleset_id).await;
    let _ = gm
        .run_gm_turn(
            GmTurnInput {
                request: &request,
                state: &state,
                user_input: "我举盾",
                history: &[],
                recent_transcript: None,
            },
            &mut |_| {},
        )
        .await
        .unwrap();
    let first_request = llm.requests.lock().unwrap()[0].clone();
    let system = first_request[0]
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        !system.contains("[mode 目录子集]"),
        "mode=None must never carry the catalog subset: {system}"
    );
    let _ = sqlx::query("delete from rule_kernels where ruleset_id = $1")
        .bind(&ruleset_id)
        .execute(&gm.engine.db.pool)
        .await;
    cleanup(&gm, &request.session_id).await;
}

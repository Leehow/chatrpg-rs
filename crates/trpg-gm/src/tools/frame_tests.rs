//! 批2 战斗 skill 单测（计划测试清单即规格）：open 建 Combat frame + 投影块、
//! close 触发压缩调用、参与者进 BP3、novelty 渲染、交锋簇债务收紧（单元侧）、
//! combat mode 包数据断言（manifest/extra_tools/提示文件四级合并）。
//! db 测试走 :54347 真库 + 唯一 session id（测试尾清理自建行）。
use super::*;
use crate::ledger::TurnLedger;
use crate::obligations::{ObligationLedger, WaiveScope};
use crate::tools::{ToolCtx, ToolError, ToolRegistry};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use trpg_db::Db;
use trpg_model::{
    ContextRequest, FrameKind, FrameStatus, RuntimeState, TokenBudget, VisibilityProfile,
};
use trpg_runtime::RuntimeEngine;

fn temp_data_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "gm_frame_test_{}_{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    fs::create_dir_all(dir.join("agent/gm_skill/modes/combat")).unwrap();
    fs::write(
        dir.join("agent/gm_skill/modes/combat/manifest.json"),
        r#"{"mode_id":"combat","frame_kind":"combat","exit_obligations":["所有交锋簇效果落账 + frame 压缩归档"],"tempo":{"effect_closure_per_cluster":true}}"#,
    )
    .unwrap();
    dir
}

fn engine_request_state() -> (RuntimeEngine, ContextRequest, RuntimeState) {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://chatrpg:chatrpg@localhost:54347/chatrpg")
        .expect("lazy pool");
    let engine = RuntimeEngine::new(Db { pool });
    let session_id = format!("frame_test_{}", uuid::Uuid::new_v4().simple());
    let request = ContextRequest {
        ruleset_id: "rs".into(),
        module_id: None,
        session_id,
        turn_id: "t".into(),
        viewer: VisibilityProfile::gm(),
        token_budget: TokenBudget::default(),
    };
    let state = RuntimeState {
        ruleset_id: "rs".into(),
        ..Default::default()
    };
    (engine, request, state)
}

async fn cleanup(engine: &RuntimeEngine, session_id: &str) {
    let _ = sqlx::query("delete from frame_compactions where session_id = $1")
        .bind(session_id)
        .execute(&engine.db.pool)
        .await;
    let _ = sqlx::query("delete from frame_events where session_id = $1")
        .bind(session_id)
        .execute(&engine.db.pool)
        .await;
    let _ = sqlx::query("delete from interaction_contexts where session_id = $1")
        .bind(session_id)
        .execute(&engine.db.pool)
        .await;
    let _ = sqlx::query("delete from state_frames where session_id = $1")
        .bind(session_id)
        .execute(&engine.db.pool)
        .await;
}

fn typed_err(result: anyhow::Result<crate::tools::ToolOutput>) -> ToolError {
    match result {
        Ok(_) => panic!("expected a typed ToolError"),
        Err(e) => e
            .downcast_ref::<ToolError>()
            .expect("typed ToolError")
            .clone(),
    }
}

#[tokio::test]
async fn open_combat_frame_validates_arguments() {
    let dir = temp_data_dir();
    let (engine, request, state) = engine_request_state();
    let mut ledger = TurnLedger::new();
    let ctx = ToolCtx {
        engine: &engine,
        request: &request,
        state: &state,
        scene_extractor: None,
        obligations: None,
        data_dir: Some(&dir),
        current_mode: None,
        opposed_binding: None,
        nominated_reveals: None,
    };
    // participants 缺失 / 空数组 / stakes 缺失 → invalid_arguments（不打 db）。
    let err = typed_err(
        OpenCombatFrameTool
            .call(&ctx, &mut ledger, json!({"stakes":"伏击"}))
            .await,
    );
    assert_eq!(err.code, "invalid_arguments");
    let err = typed_err(
        OpenCombatFrameTool
            .call(
                &ctx,
                &mut ledger,
                json!({"participants":[],"stakes":"伏击"}),
            )
            .await,
    );
    assert_eq!(err.code, "invalid_arguments");
    let err = typed_err(
        OpenCombatFrameTool
            .call(&ctx, &mut ledger, json!({"participants":["pc.current"]}))
            .await,
    );
    assert_eq!(err.code, "invalid_arguments");
    // 已在另一姿态（downtime）内开战斗 frame → 嵌套拒绝（栈深 1）。
    let ctx = ToolCtx {
        engine: &engine,
        request: &request,
        state: &state,
        scene_extractor: None,
        obligations: None,
        data_dir: Some(&dir),
        current_mode: Some("downtime"),
        opposed_binding: None,
        nominated_reveals: None,
    };
    let err = typed_err(
        OpenCombatFrameTool
            .call(
                &ctx,
                &mut ledger,
                json!({"participants":["pc.current"],"stakes":"伏击"}),
            )
            .await,
    );
    assert_eq!(err.code, "mode_nesting_unsupported");
    fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn open_combat_frame_creates_live_combat_frame_with_bp3_projection() {
    let dir = temp_data_dir();
    let (engine, request, state) = engine_request_state();
    let mut ledger = TurnLedger::new();
    let ctx = ToolCtx {
        engine: &engine,
        request: &request,
        state: &state,
        scene_extractor: None,
        obligations: None,
        data_dir: Some(&dir),
        current_mode: None,
        opposed_binding: None,
        nominated_reveals: None,
    };
    let output = OpenCombatFrameTool
        .call(
            &ctx,
            &mut ledger,
            json!({"participants":["pc.current","npc.ghoul"],"stakes":"食尸鬼从墓穴扑出"}),
        )
        .await
        .expect("open_combat_frame must succeed against live db");
    let frame_id = output
        .result
        .get("frame_id")
        .and_then(|v| v.as_str())
        .expect("frame_id in output")
        .to_string();
    // db 真有一个 live Combat frame。
    let frames = engine
        .db
        .list_active_state_frames(&request.session_id, 8)
        .await
        .unwrap();
    assert_eq!(frames.len(), 1);
    let frame = &frames[0];
    assert_eq!(frame.frame_id, frame_id);
    assert!(matches!(frame.frame_kind, FrameKind::Combat));
    assert!(matches!(frame.status, FrameStatus::Active));
    // 参与者/赌注/战术记忆进 working_state（BP3 投影载体）。
    assert_eq!(
        frame
            .working_state
            .pointer("/participants/1")
            .and_then(|v| v.as_str()),
        Some("npc.ghoul")
    );
    assert_eq!(
        frame.working_state.get("stakes").and_then(|v| v.as_str()),
        Some("食尸鬼从墓穴扑出")
    );
    assert!(
        frame.working_state.get("npc_tactic_memory").is_some(),
        "tactic memory bucket must be seeded"
    );
    // 参与者进 BP3：既有投影通道 to_context_block 的 JSON 内容携带参与者。
    let block = frame.to_context_block(&request.turn_id);
    let rendered = serde_json::to_string(&block.content).unwrap();
    assert!(
        rendered.contains("npc.ghoul"),
        "participants must reach the BP3 projection block: {rendered}"
    );
    // 该 frame 即姿态投影：current_mode 推导为 combat。
    assert_eq!(
        crate::mode::current_mode(&frames),
        Some("combat".to_string())
    );
    cleanup(&engine, &request.session_id).await;
    fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn open_combat_frame_enriches_existing_mode_frame_instead_of_duplicating() {
    let dir = temp_data_dir();
    let (engine, request, state) = engine_request_state();
    let mut ledger = TurnLedger::new();
    // 先 enter_mode 建姿态 frame（同生产顺序：enter → open_combat_frame 武装）。
    let ctx = ToolCtx {
        engine: &engine,
        request: &request,
        state: &state,
        scene_extractor: None,
        obligations: None,
        data_dir: Some(&dir),
        current_mode: None,
        opposed_binding: None,
        nominated_reveals: None,
    };
    crate::mode::EnterModeTool
        .call(
            &ctx,
            &mut ledger,
            json!({"mode":"combat","reason":"ambush"}),
        )
        .await
        .expect("enter_mode");
    let before = engine
        .db
        .list_active_state_frames(&request.session_id, 8)
        .await
        .unwrap();
    assert_eq!(before.len(), 1);
    // 同回合 open_combat_frame（头部推导仍 None）→ 武装既有 frame，绝不开第二个。
    OpenCombatFrameTool
        .call(
            &ctx,
            &mut ledger,
            json!({"participants":["pc.current","npc.cultist"],"stakes":"教徒拔刀"}),
        )
        .await
        .expect("open enriches");
    let after = engine
        .db
        .list_active_state_frames(&request.session_id, 8)
        .await
        .unwrap();
    assert_eq!(
        after.len(),
        1,
        "must enrich the existing combat frame, not duplicate"
    );
    assert_eq!(after[0].frame_id, before[0].frame_id);
    assert_eq!(
        after[0]
            .working_state
            .pointer("/participants/1")
            .and_then(|v| v.as_str()),
        Some("npc.cultist")
    );
    // enter_mode 写入的进入语义被保留（merge 不覆盖无关键）。
    assert_eq!(
        after[0]
            .working_state
            .get("entered_reason")
            .and_then(|v| v.as_str()),
        Some("ambush")
    );
    cleanup(&engine, &request.session_id).await;
    fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn close_frame_without_live_frame_is_frame_not_found() {
    let dir = temp_data_dir();
    let (engine, request, state) = engine_request_state();
    let mut ledger = TurnLedger::new();
    let ctx = ToolCtx {
        engine: &engine,
        request: &request,
        state: &state,
        scene_extractor: None,
        obligations: None,
        data_dir: Some(&dir),
        current_mode: None,
        opposed_binding: None,
        nominated_reveals: None,
    };
    let err = typed_err(
        CloseFrameTool
            .call(&ctx, &mut ledger, json!({"summary":"战斗结束"}))
            .await,
    );
    assert_eq!(err.code, "frame_not_found");
    assert!(err.recoverable);
    // summary 缺失 → invalid_arguments。
    let err = typed_err(CloseFrameTool.call(&ctx, &mut ledger, json!({})).await);
    assert_eq!(err.code, "invalid_arguments");
    fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn close_frame_compacts_closes_and_settles_exit_obligations() {
    let dir = temp_data_dir();
    let (engine, request, state) = engine_request_state();
    let mut ledger = TurnLedger::new();
    let cell = Mutex::new(ObligationLedger::default());
    {
        let mut obligations = cell.lock().unwrap();
        obligations.begin_turn("t");
        obligations.ensure_mode_exit_obligations(
            "combat",
            &["所有交锋簇效果落账 + frame 压缩归档".to_string()],
        );
        assert_eq!(obligations.exit_blocking().len(), 1);
    }
    let ctx = ToolCtx {
        engine: &engine,
        request: &request,
        state: &state,
        scene_extractor: None,
        obligations: Some(&cell),
        data_dir: Some(&dir),
        current_mode: Some("combat"),
        opposed_binding: None,
        nominated_reveals: None,
    };
    OpenCombatFrameTool
        .call(
            &ctx,
            &mut ledger,
            json!({"participants":["pc.current","npc.ghoul"],"stakes":"墓地交锋"}),
        )
        .await
        .expect("open");
    let frame_id = engine
        .db
        .list_active_state_frames(&request.session_id, 8)
        .await
        .unwrap()[0]
        .frame_id
        .clone();
    let output = CloseFrameTool
        .call(
            &ctx,
            &mut ledger,
            json!({"summary":"食尸鬼被击退，调查员带伤撤入教堂"}),
        )
        .await
        .expect("close_frame must succeed");
    assert_eq!(
        output
            .result
            .get("closed_frame_id")
            .and_then(|v| v.as_str()),
        Some(frame_id.as_str())
    );
    let compaction_id = output
        .result
        .get("compaction_id")
        .and_then(|v| v.as_str())
        .expect("compaction_id in output")
        .to_string();
    // 压缩真落库（复用一期 conflict kernel 原语 insert_frame_compaction）。
    let count: i64 = sqlx::query_scalar(
        "select count(*)::bigint from frame_compactions where compaction_id = $1 and frame_id = $2",
    )
    .bind(&compaction_id)
    .bind(&frame_id)
    .fetch_one(&engine.db.pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "compaction row must exist");
    // frame 已关：active 列表为空，下一回合头部回落叙事姿态。
    assert!(engine
        .db
        .list_active_state_frames(&request.session_id, 8)
        .await
        .unwrap()
        .is_empty());
    // close_frame 成功 = combat 退出义务的确定性闭合事件 → exit_mode 放行。
    assert!(
        cell.lock().unwrap().exit_blocking().is_empty(),
        "close_frame must settle combat mode_exit obligations"
    );
    let exit = crate::mode::ExitModeTool
        .call(&ctx, &mut ledger, json!({"reason":"战斗落幕"}))
        .await
        .expect("exit_mode passes after close_frame");
    assert_eq!(
        exit.result.get("exited_mode").and_then(|v| v.as_str()),
        Some("combat")
    );
    cleanup(&engine, &request.session_id).await;
    fs::remove_dir_all(dir).ok();
}

#[test]
fn novelty_block_renders_used_tactics_and_is_fail_closed() {
    let mk = |ws: serde_json::Value, status: FrameStatus| {
        let now = chrono::Utc::now();
        trpg_model::StateFrame {
            frame_id: "f1".into(),
            frame_kind: FrameKind::Combat,
            session_id: "s".into(),
            ruleset_id: "rs".into(),
            module_id: None,
            parent_frame_id: None,
            scope: trpg_model::Scope {
                scope_type: trpg_model::ScopeType::Session,
                scope_id: "s".into(),
            },
            status,
            title: "t".into(),
            objective: "o".into(),
            static_refs: vec![],
            working_state: ws,
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
        }
    };
    let tactics = json!({"npc_tactic_memory":[{"npc_id":"npc.ghoul","tactic_id":"ambush_from_dark","used_count_in_frame":2,"consecutive_count":2,"last_result":null,"last_turn_id":"t1"}]});
    // 有已用战术 → 提示块出现，含 npc 与战术 id。
    let block = novelty_block(&[mk(tactics.clone(), FrameStatus::Active)])
        .expect("tactic memory must render a block");
    assert!(block.contains("[combat_novelty]"), "missing tag: {block}");
    assert!(block.contains("npc.ghoul") && block.contains("ambush_from_dark"));
    assert!(block.contains("2"), "use counts must surface: {block}");
    // 空记忆 / 缺键 / 已关 frame / 形状不对 → None（fail-closed 不编造）。
    assert!(novelty_block(&[mk(json!({"npc_tactic_memory": []}), FrameStatus::Active)]).is_none());
    assert!(novelty_block(&[mk(json!({}), FrameStatus::Active)]).is_none());
    assert!(novelty_block(&[mk(tactics.clone(), FrameStatus::Completed)]).is_none());
    assert!(novelty_block(&[mk(
        json!({"npc_tactic_memory": "garbage"}),
        FrameStatus::Active
    )])
    .is_none());
}

#[test]
fn cluster_debt_opens_blocks_and_clears() {
    let mut ledger = ObligationLedger::default();
    ledger.begin_turn("t");
    // 紧节拍 + 已结算检定 + 零效果落账 → 簇未闭合债务进 blocking。
    ledger.update_cluster_closure(true, 1, 0);
    let blocking = ledger.blocking();
    assert_eq!(blocking.len(), 1, "open cluster must block: {blocking:?}");
    assert_eq!(blocking[0].kind, "cluster");
    assert_eq!(blocking[0].target_id, "cluster.t");
    assert!(ledger.block_text().unwrap().contains("apply_effect"));
    // 效果落账（effect/track/committed patches 任一）→ 簇闭合。
    ledger.update_cluster_closure(true, 1, 1);
    assert!(ledger.blocking().is_empty());
    // 节拍未收紧（mode=None / 幕间）→ 永不产生簇债务（二期行为字节级一致）。
    ledger.update_cluster_closure(false, 3, 0);
    assert!(ledger.blocking().is_empty());
    // 无检定 → 不产生（纯叙事轮不被姿态节拍误伤）。
    ledger.update_cluster_closure(true, 0, 0);
    assert!(ledger.blocking().is_empty());
    // waive 通道照常：豁免后不再 blocking（id 确定性 cluster.<turn_id>）。
    ledger.update_cluster_closure(true, 2, 0);
    ledger
        .waive("cluster.t", "玩家中途逃跑，后果延后结算", WaiveScope::Turn)
        .expect("cluster debt must be waivable");
    assert!(ledger.blocking().is_empty());
    // begin_turn 重置（簇债务是回合内节拍，跨回合由 verifier 追溯债务兜底）。
    ledger.update_cluster_closure(true, 2, 0);
    ledger.begin_turn("t2");
    assert!(ledger.blocking().is_empty());
}

#[test]
fn combat_manifest_loads_with_frame_tools_and_tight_tempo() {
    let data_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
    let manifest = crate::mode::load_mode_manifest(&data_dir, "combat")
        .expect("combat manifest must load from data/");
    assert_eq!(manifest.mode_id, "combat");
    assert_eq!(manifest.frame_kind, "combat");
    assert_eq!(
        manifest.extra_tools,
        vec!["open_combat_frame".to_string(), "close_frame".to_string()]
    );
    assert_eq!(
        manifest.tempo.effect_closure_per_cluster,
        Some(true),
        "combat tempo must tighten cluster closure"
    );
    assert!(
        !manifest.exit_obligations.is_empty(),
        "combat must declare exit obligations"
    );
    // 目录过滤器选中战斗类条目、排除幕间/无关条目（与批3 downtime 夹具对偶）。
    let filter = &manifest.catalog_filter;
    assert!(
        filter.matches(
            "combat_procedure",
            &["combat_round".to_string()],
            &["initiative".to_string()]
        ),
        "filter must select combat entries: {filter:?}"
    );
    assert!(
        filter.matches("reaction", &[], &[]),
        "reaction tier entries are combat-relevant: {filter:?}"
    );
    assert!(
        !filter.matches(
            "downtime",
            &["development_phase".to_string()],
            &["rest".to_string()]
        ),
        "filter must NOT select downtime entries: {filter:?}"
    );
    assert!(
        !filter.matches("lore", &[], &["worldbuilding".to_string()]),
        "filter must NOT select unrelated entries: {filter:?}"
    );
    // for_mode 真组装：基础 15 + 两个 frame 工具在尾部（schema 确定性）。
    let registry =
        ToolRegistry::for_mode(&data_dir, Some("combat")).expect("combat extra tools must resolve");
    let schemas = registry.schemas();
    assert_eq!(schemas.len(), 17);
    let names: Vec<&str> = schemas
        .iter()
        .filter_map(|s| s.pointer("/function/name").and_then(|v| v.as_str()))
        .collect();
    assert_eq!(names[15], "open_combat_frame");
    assert_eq!(names[16], "close_frame");
}

#[test]
fn combat_mode_prompts_merge_for_both_rulesets() {
    // 四级合并装载真实 data/：global.md（电影化准则）必入；ruleset 文件按
    // ruleset id 追加（dnd5e / call_of_cthulhu_7e 各自命中，互不串扰）。
    let data_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
    let dnd = crate::prompts::load_gm_skill_with_mode(&data_dir, "dnd5e", Some("combat")).unwrap();
    assert!(
        dnd.contains("[combat-mode]"),
        "combat global.md marker missing for dnd5e"
    );
    assert!(dnd.contains("[combat-mode:dnd5e]"), "dnd5e 战斗提示未并入");
    assert!(
        !dnd.contains("[combat-mode:call_of_cthulhu_7e]"),
        "ruleset 提示串扰"
    );
    let coc =
        crate::prompts::load_gm_skill_with_mode(&data_dir, "call_of_cthulhu_7e", Some("combat"))
            .unwrap();
    assert!(coc.contains("[combat-mode]"));
    assert!(
        coc.contains("[combat-mode:call_of_cthulhu_7e]"),
        "CoC 战斗提示未并入"
    );
    assert!(!coc.contains("[combat-mode:dnd5e]"));
    // 呈现层硬约束必须写进电影化准则（评测 grep 断言的同源词）。
    assert!(
        dnd.contains("先攻表") || dnd.contains("轮次"),
        "电影化准则必须明令不报轮次表/先攻表"
    );
}

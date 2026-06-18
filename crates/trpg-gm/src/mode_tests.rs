//! 批1 姿态框架核心单测（计划测试清单即规格）：manifest fail-closed、
//! current_mode 三态、for_mode 未知工具名、enter/exit 嵌套拒绝、
//! exit 义务拦截 + waive 放行、CatalogFilter 匹配语义。
use super::*;
use crate::obligations::{ObligationLedger, WaiveScope};
use crate::tools::{ToolCtx, ToolError, ToolRegistry};
use chrono::Utc;
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use trpg_db::Db;
use trpg_model::{
    ContextRequest, FrameKind, RuntimeState, Scope, ScopeType, TokenBudget, VisibilityProfile,
};
use trpg_runtime::RuntimeEngine;

/// 临时 data_dir 夹具：tests 自建 modes/<mode>/manifest.json。
fn temp_data_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "gm_mode_test_{}_{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    fs::create_dir_all(dir.join("agent/gm_skill/modes")).unwrap();
    dir
}

fn write_manifest(dir: &Path, mode: &str, body: &str) {
    let mode_dir = dir.join("agent/gm_skill/modes").join(mode);
    fs::create_dir_all(&mode_dir).unwrap();
    fs::write(mode_dir.join("manifest.json"), body).unwrap();
}

fn frame(kind: FrameKind, status: FrameStatus) -> StateFrame {
    let now = Utc::now();
    StateFrame {
        frame_id: format!("frame_{}", uuid::Uuid::new_v4().simple()),
        frame_kind: kind,
        session_id: "s".into(),
        ruleset_id: "rs".into(),
        module_id: None,
        parent_frame_id: None,
        scope: Scope { scope_type: ScopeType::Session, scope_id: "s".into() },
        status,
        title: "t".into(),
        objective: "o".into(),
        static_refs: vec![],
        working_state: json!({}),
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
}

/// lazy pool + 唯一 session id（绝不触碰真实库已有行；list 查不到 → 空）。
fn engine_request_state() -> (RuntimeEngine, ContextRequest, RuntimeState) {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://chatrpg:chatrpg@localhost:54347/chatrpg")
        .expect("lazy pool");
    let engine = RuntimeEngine::new(Db { pool });
    let session_id = format!("mode_test_{}", uuid::Uuid::new_v4().simple());
    let request = ContextRequest { ruleset_id: "rs".into(), module_id: None, session_id, turn_id: "t".into(), viewer: VisibilityProfile::gm(), token_budget: TokenBudget::default() };
    let state = RuntimeState { ruleset_id: "rs".into(), ..Default::default() };
    (engine, request, state)
}

#[test]
fn load_mode_manifest_is_fail_closed() {
    let dir = temp_data_dir();
    // 不存在 → Err。
    assert!(load_mode_manifest(&dir, "combat").is_err(), "missing manifest must Err");
    // 解析失败 → Err。
    write_manifest(&dir, "combat", "{not json");
    assert!(load_mode_manifest(&dir, "combat").is_err(), "broken JSON must Err");
    // mode_id 与文件夹名不一致 → Err（fail-closed 配置错误）。
    write_manifest(&dir, "combat", r#"{"mode_id":"downtime","frame_kind":"combat"}"#);
    assert!(load_mode_manifest(&dir, "combat").is_err(), "mode_id/folder mismatch must Err");
    // 最小合法 manifest：可选项全 serde default。
    write_manifest(&dir, "combat", r#"{"mode_id":"combat","frame_kind":"combat"}"#);
    let manifest = load_mode_manifest(&dir, "combat").expect("minimal manifest loads");
    assert_eq!(manifest.mode_id, "combat");
    assert_eq!(manifest.frame_kind, "combat");
    assert!(manifest.extra_tools.is_empty());
    assert!(manifest.exit_obligations.is_empty());
    assert!(manifest.tempo.max_tool_rounds.is_none());
    assert!(manifest.tempo.effect_closure_per_cluster.is_none());
    assert!(manifest.catalog_filter.kinds.is_empty());
    fs::remove_dir_all(dir).ok();
}

#[test]
fn current_mode_three_states() {
    // 无 frame → None（默认叙事姿态）。
    assert_eq!(current_mode(&[]), None);
    // Combat frame → Some("combat")。
    assert_eq!(current_mode(&[frame(FrameKind::Combat, FrameStatus::Active)]), Some("combat".to_string()));
    // Downtime frame → Some("downtime")。
    assert_eq!(current_mode(&[frame(FrameKind::Downtime, FrameStatus::Active)]), Some("downtime".to_string()));
    // 已关闭 frame 不算（非 active 生命周期态被过滤）。
    assert_eq!(current_mode(&[frame(FrameKind::Combat, FrameStatus::Completed)]), None);
}

#[test]
fn active_mode_manifest_skips_frames_without_mode_package() {
    let dir = temp_data_dir();
    write_manifest(&dir, "combat", r#"{"mode_id":"combat","frame_kind":"combat"}"#);
    // frame 种类无 mode 包（investigation_node）→ None（不是姿态，照常叙事）。
    let no_pkg = active_mode_manifest(&dir, &[frame(FrameKind::InvestigationNode, FrameStatus::Active)]).unwrap();
    assert!(no_pkg.is_none());
    // 有包 → Some(manifest)。
    let hit = active_mode_manifest(&dir, &[frame(FrameKind::Combat, FrameStatus::Active)]).unwrap();
    assert_eq!(hit.expect("combat manifest").mode_id, "combat");
    // 包存在但损坏 → Err（fail-closed 配置错误，绝不静默当 None）。
    write_manifest(&dir, "combat", "{broken");
    assert!(active_mode_manifest(&dir, &[frame(FrameKind::Combat, FrameStatus::Active)]).is_err());
    fs::remove_dir_all(dir).ok();
}

#[test]
fn for_mode_unknown_extra_tool_is_config_error() {
    let dir = temp_data_dir();
    write_manifest(&dir, "combat", r#"{"mode_id":"combat","frame_kind":"combat","extra_tools":["no_such_tool"]}"#);
    let err = ToolRegistry::for_mode(&dir, Some("combat")).err().expect("unknown extra tool must Err");
    assert!(err.to_string().contains("no_such_tool"), "error must name the tool: {err}");
    // extra_tools 为空 ⇒ 与 standard() schema 字节一致（基础 14 不因 mode 而变）。
    write_manifest(&dir, "combat", r#"{"mode_id":"combat","frame_kind":"combat"}"#);
    let mode_schemas = serde_json::to_vec(&ToolRegistry::for_mode(&dir, Some("combat")).unwrap().schemas()).unwrap();
    let std_schemas = serde_json::to_vec(&ToolRegistry::standard().schemas()).unwrap();
    assert_eq!(mode_schemas, std_schemas);
    // mode=None ⇒ 与 standard() 完全等同（mode=None 字节回归）。
    let none_schemas = serde_json::to_vec(&ToolRegistry::for_mode(&dir, None).unwrap().schemas()).unwrap();
    assert_eq!(none_schemas, std_schemas);
    fs::remove_dir_all(dir).ok();
}

#[test]
fn base_registry_has_fourteen_tools_with_mode_tools_at_tail() {
    let schemas = ToolRegistry::standard().schemas();
    assert_eq!(schemas.len(), 15, "基础 15 = 二期 12 + enter_mode/exit_mode + reveal_fact");
    let names: Vec<&str> = schemas.iter().filter_map(|s| s.pointer("/function/name").and_then(|v| v.as_str())).collect();
    assert_eq!(names[12], "enter_mode");
    assert_eq!(names[13], "exit_mode");
    assert_eq!(names[14], "reveal_fact");
}

fn typed_err(result: anyhow::Result<crate::tools::ToolOutput>) -> ToolError {
    match result {
        Ok(_) => panic!("expected a typed ToolError"),
        Err(e) => e.downcast_ref::<ToolError>().expect("typed ToolError").clone(),
    }
}

#[tokio::test]
async fn enter_mode_rejects_nesting_and_unknown_mode() {
    let dir = temp_data_dir();
    write_manifest(&dir, "combat", r#"{"mode_id":"combat","frame_kind":"combat"}"#);
    let (engine, request, state) = engine_request_state();
    let mut ledger = crate::ledger::TurnLedger::new();
    // 已在 combat 姿态内 enter downtime → mode_nesting_unsupported（栈深 1）。
    let ctx = ToolCtx { engine: &engine, request: &request, state: &state, scene_extractor: None, obligations: None, data_dir: Some(&dir), current_mode: Some("combat"), opposed_binding: None };
    let err = typed_err(EnterModeTool.call(&ctx, &mut ledger, json!({"mode":"downtime","reason":"rest"})).await);
    assert_eq!(err.code, "mode_nesting_unsupported");
    assert!(err.recoverable);
    assert!(err.message.contains("combat"), "must name the active mode: {}", err.message);
    // 默认姿态 enter 未安装 mode → mode_not_found。
    let ctx = ToolCtx { engine: &engine, request: &request, state: &state, scene_extractor: None, obligations: None, data_dir: Some(&dir), current_mode: None, opposed_binding: None };
    let err = typed_err(EnterModeTool.call(&ctx, &mut ledger, json!({"mode":"no_such_mode","reason":"x"})).await);
    assert_eq!(err.code, "mode_not_found");
    assert!(err.recoverable);
    // 参数缺失 → invalid_arguments。
    let err = typed_err(EnterModeTool.call(&ctx, &mut ledger, json!({"mode":"combat"})).await);
    assert_eq!(err.code, "invalid_arguments");
    fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn exit_mode_without_active_mode_is_no_active_mode() {
    let dir = temp_data_dir();
    let (engine, request, state) = engine_request_state();
    let mut ledger = crate::ledger::TurnLedger::new();
    let ctx = ToolCtx { engine: &engine, request: &request, state: &state, scene_extractor: None, obligations: None, data_dir: Some(&dir), current_mode: None, opposed_binding: None };
    let err = typed_err(ExitModeTool.call(&ctx, &mut ledger, json!({"reason":"done"})).await);
    assert_eq!(err.code, "no_active_mode");
    assert!(err.recoverable);
    fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn exit_mode_blocked_by_obligations_then_waive_releases() {
    let dir = temp_data_dir();
    write_manifest(
        &dir,
        "combat",
        r#"{"mode_id":"combat","frame_kind":"combat","exit_obligations":["所有交锋簇效果落账"]}"#,
    );
    let (engine, request, state) = engine_request_state();
    let mut turn_ledger = crate::ledger::TurnLedger::new();
    let cell = Mutex::new(ObligationLedger::default());
    {
        let mut obligations = cell.lock().unwrap();
        obligations.begin_turn("t");
        obligations.ensure_mode_exit_obligations("combat", &["所有交锋簇效果落账".to_string()]);
        // 幂等 re-seed：重复挂账不翻倍。
        obligations.ensure_mode_exit_obligations("combat", &["所有交锋簇效果落账".to_string()]);
        assert_eq!(obligations.exit_blocking().len(), 1, "deterministic id must dedupe");
        // mode_exit 义务绝不门叙事轮（B6 blocking() 不含它）。
        assert!(obligations.blocking().is_empty(), "mode_exit must not gate narration rounds");
        let view = &obligations.exit_blocking()[0];
        assert_eq!(view.kind, "mode_exit");
        assert_eq!(view.target_id, "mode_exit.combat.0");
        assert!(view.summary.contains("所有交锋簇效果落账"));
    }
    // J2 修复后的语义：自身退出义务之外还有未清机械项（这里挂一笔 due）⇒
    // exit 被拦并列出该项；外部项清空后，纯自身退出义务=确定性闭合 ⇒ 放行
    // （不再要求 waive 自身退出义务——那是 downtime frame 永久泄漏的根源）。
    {
        let mut obligations = cell.lock().unwrap();
        obligations.absorb_dues(vec![trpg_model::MechanicDue {
            due_id: "due_exit_gate".to_string(), session_id: "s".to_string(), turn_id: "t".to_string(),
            source: trpg_model::DueSource::Threshold, source_track: Some("hp".to_string()), hook_event: None,
            mechanic_id: None, threshold_desc: "未结算的交锋效果".to_string(), followup_procedure_id: None,
            owner_kind: "actor".to_string(), owner_id: "pc.current".to_string(), evidence: json!({}),
            status: trpg_model::DueStatus::Open, created_at: Utc::now(),
        }]);
    }
    let ctx = ToolCtx { engine: &engine, request: &request, state: &state, scene_extractor: None, obligations: Some(&cell), data_dir: Some(&dir), current_mode: Some("combat"), opposed_binding: None };
    // 外部债务未清 → exit 被拦（waive 通道照常可用——hint 指路）。
    let err = typed_err(ExitModeTool.call(&ctx, &mut turn_ledger, json!({"reason":"flee"})).await);
    assert_eq!(err.code, "exit_blocked_by_obligations");
    assert!(err.recoverable);
    assert!(err.message.contains("due_exit_gate"), "must list outstanding items: {}", err.message);
    assert!(err.hint.as_deref().unwrap_or("").contains("waive_obligation"));
    // 外部债务 waive 带理由 → 只剩自身退出义务 → 确定性闭合放行
    // （frame 已不在 db = 幂等成功路径，姿态下回合回落叙事）。
    {
        let mut obligations = cell.lock().unwrap();
        obligations.waive("due_exit_gate", "敌人逃散，效果并入下一幕", WaiveScope::Turn).expect("due target must be waivable");
        assert_eq!(obligations.exit_blocking().len(), 1, "own mode_exit obligation still listed pre-exit");
    }
    let output = ExitModeTool.call(&ctx, &mut turn_ledger, json!({"reason":"flee"})).await.expect("exit must pass once only own exit obligations remain");
    assert_eq!(output.result.get("exited_mode").and_then(|v| v.as_str()), Some("combat"));
    // 成功退出后该 mode 的退出义务整体清账。
    let obligations = cell.lock().unwrap();
    assert!(obligations.exit_blocking().is_empty());
    fs::remove_dir_all(dir).ok();
}

#[test]
fn clear_mode_exit_obligations_only_touches_that_mode() {
    let mut ledger = ObligationLedger::default();
    ledger.begin_turn("t");
    ledger.ensure_mode_exit_obligations("combat", &["a".to_string()]);
    ledger.ensure_mode_exit_obligations("downtime", &["b".to_string()]);
    assert_eq!(ledger.exit_blocking().len(), 2);
    ledger.clear_mode_exit_obligations("combat");
    let left = ledger.exit_blocking();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].target_id, "mode_exit.downtime.0");
}

#[test]
fn catalog_filter_matches_any_dimension_and_empty_means_no_filter() {
    // 全空 = 不过滤（恒真）。
    assert!(CatalogFilter::default().matches("combat_procedure", &[], &[]));
    let filter = CatalogFilter {
        kinds: vec!["combat_procedure".to_string()],
        hooks: vec!["development_phase".to_string()],
        semantic_tags: vec!["initiative".to_string()],
    };
    // 任一维度命中即注入。
    assert!(filter.matches("combat_procedure", &[], &[]));
    assert!(filter.matches("other", &["development_phase".to_string()], &[]));
    assert!(filter.matches("other", &[], &["initiative".to_string()]));
    // 全不命中 → 不注入。
    assert!(!filter.matches("other", &["calendar".to_string()], &["damage".to_string()]));
}

// ─── 批3 幕间 skill 单测 ────────────────────────────────────────────────────

/// 从仓库真实 data/ 目录加载 downtime manifest，断言：
/// - mode_id = "downtime"、frame_kind = "downtime"
/// - catalog_filter 至少声明了 "development_phase" 或 "calendar" 维度
///   （hooks 或 kinds 或 semantic_tags 任一含之）
/// - exit_obligations 非空（结算表落账义务存在）
/// - tempo.effect_closure_per_cluster 为 None 或 false（幕间放宽，非战斗收紧）
#[test]
fn downtime_manifest_loads_with_correct_fields() {
    let data_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
    let manifest = load_mode_manifest(&data_dir, "downtime")
        .expect("downtime manifest must load from data/agent/gm_skill/modes/downtime/manifest.json");
    assert_eq!(manifest.mode_id, "downtime");
    assert_eq!(manifest.frame_kind, "downtime");
    // exit_obligations 非空：至少声明了结算表落账义务。
    assert!(!manifest.exit_obligations.is_empty(), "downtime must declare at least one exit obligation (settlement ledger)");
    // tempo 放宽：幕间不要求每簇效果闭合（None 或 false 均可）。
    assert!(
        manifest.tempo.effect_closure_per_cluster.is_none()
            || manifest.tempo.effect_closure_per_cluster == Some(false),
        "downtime tempo must not require effect_closure_per_cluster (it is relaxed, not tight)"
    );
    // catalog_filter 至少在一个维度声明了 development_phase 或 calendar 相关钩子。
    let has_downtime_hook = manifest.catalog_filter.hooks.iter().any(|h| {
        h.eq_ignore_ascii_case("development_phase") || h.eq_ignore_ascii_case("calendar")
    }) || manifest.catalog_filter.kinds.iter().any(|k| {
        k.eq_ignore_ascii_case("downtime") || k.eq_ignore_ascii_case("development_phase")
    }) || manifest.catalog_filter.semantic_tags.iter().any(|t| {
        t.eq_ignore_ascii_case("downtime") || t.eq_ignore_ascii_case("development_phase")
    });
    assert!(has_downtime_hook, "downtime catalog_filter must reference development_phase or calendar hooks/kinds; got: {:?}", manifest.catalog_filter);
}

/// 合成目录夹具（3 条目：A5 幕间类 + 战斗类 + 其他）→
/// downtime 真实 manifest 的过滤器仅选中 A5 幕间类条目。
#[test]
fn downtime_catalog_filter_selects_only_downtime_entries() {
    let data_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
    let manifest = load_mode_manifest(&data_dir, "downtime")
        .expect("downtime manifest must load");
    let filter = &manifest.catalog_filter;
    // 合成目录夹具：三条不同类型。
    // A5 幕间条目：kind=downtime，hooks=[development_phase]，tags=[rest]。
    let a5_kind = "downtime";
    let a5_hooks = vec!["development_phase".to_string()];
    let a5_tags: Vec<String> = vec![];
    // 战斗条目：kind=combat_procedure，hooks=[combat_round]，tags=[initiative]。
    let combat_kind = "combat_procedure";
    let combat_hooks = vec!["combat_round".to_string()];
    let combat_tags = vec!["initiative".to_string()];
    // 无关条目：kind=lore，hooks=[none]，tags=[worldbuilding]。
    let other_kind = "lore";
    let other_hooks: Vec<String> = vec![];
    let other_tags = vec!["worldbuilding".to_string()];
    // 全空 filter = 不过滤，跳过该夹具场景（只针对有内容的 filter 做选择性测试）。
    if filter.kinds.is_empty() && filter.hooks.is_empty() && filter.semantic_tags.is_empty() {
        // 全空 filter 行为由 catalog_filter_matches_any_dimension_and_empty_means_no_filter 覆盖，此处跳过。
        return;
    }
    let selects_a5 = filter.matches(a5_kind, &a5_hooks, &a5_tags);
    let selects_combat = filter.matches(combat_kind, &combat_hooks, &combat_tags);
    let selects_other = filter.matches(other_kind, &other_hooks, &other_tags);
    assert!(selects_a5, "downtime filter must select A5 downtime entry; filter={:?}", filter);
    assert!(!selects_combat, "downtime filter must NOT select combat entry; filter={:?}", filter);
    assert!(!selects_other, "downtime filter must NOT select unrelated lore entry; filter={:?}", filter);
}

/// downtime exit_obligations 挂账后出现于债务清单（exit_blocking view）。
#[test]
fn downtime_exit_obligations_appear_in_exit_blocking_view() {
    let data_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
    let manifest = load_mode_manifest(&data_dir, "downtime")
        .expect("downtime manifest must load");
    assert!(!manifest.exit_obligations.is_empty(), "precondition: downtime must have exit obligations");
    let mut ledger = ObligationLedger::default();
    ledger.begin_turn("t");
    ledger.ensure_mode_exit_obligations("downtime", &manifest.exit_obligations);
    let views = ledger.exit_blocking();
    // 每条义务对应一条 exit_blocking 视图。
    assert_eq!(
        views.len(),
        manifest.exit_obligations.len(),
        "each exit obligation must appear in exit_blocking view"
    );
    for (i, view) in views.iter().enumerate() {
        assert_eq!(view.kind, "mode_exit");
        assert_eq!(view.target_id, format!("mode_exit.downtime.{i}"));
        assert!(
            view.summary.contains(&manifest.exit_obligations[i]),
            "exit_blocking summary must contain obligation description: {}",
            manifest.exit_obligations[i]
        );
    }
    // mode_exit 义务绝不进 blocking()（不堵叙事轮，只堵 exit_mode）。
    assert!(
        ledger.blocking().is_empty(),
        "mode_exit obligations must NEVER appear in blocking() (only in exit_blocking)"
    );
}

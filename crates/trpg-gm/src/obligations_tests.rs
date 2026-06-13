//! ObligationLedger 单测（从 obligations.rs 拆出，#[path] 挂载——文件 ≤400 行纪律）。
use super::*;
use crate::ledger::TurnLedger;
use crate::tools::{GmTool, ToolCtx, ToolError};
use chrono::Utc;
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use trpg_db::Db;
use trpg_model::{ContextRequest, DueSource, DueStatus, MechanicDue, RuntimeState, TokenBudget, VisibilityProfile};
use trpg_runtime::RuntimeEngine;

fn due(id: &str, desc: &str) -> MechanicDue {
    MechanicDue { due_id: id.to_string(), session_id: "s".to_string(), turn_id: "t".to_string(), source: DueSource::Threshold, source_track: Some("sanity".to_string()), hook_event: None, mechanic_id: None, threshold_desc: desc.to_string(), followup_procedure_id: Some("proc.followup".to_string()), owner_kind: "actor".to_string(), owner_id: "pc.current".to_string(), evidence: json!({"before": 38, "after": 32, "delta": -6}), status: DueStatus::Open, created_at: Utc::now() }
}

#[test]
fn blocking_view_excludes_settled_and_waived() {
    let mut ledger = ObligationLedger::default();
    ledger.begin_turn("t");
    ledger.record_open_check("check_a");
    ledger.record_open_check("check_b");
    ledger.mark_check_settled("check_a");
    ledger.absorb_dues(vec![due("due_1", "sanity threshold crossed")]);
    ledger.waive("due_1", "narrative override", WaiveScope::Turn).expect("due_1 exists");
    // record_open_check + mark_check_settled + waive 后 blocking() 只剩未处理项。
    let blocking = ledger.blocking();
    assert_eq!(blocking.len(), 1);
    assert_eq!(blocking[0].kind, "check");
    assert_eq!(blocking[0].target_id, "check_b");
    // 空清单 block_text() 返回 None。
    assert!(ObligationLedger::default().block_text().is_none());
    // 非空清单 block_text() 含工具指引。
    assert!(ledger.block_text().unwrap().contains("waive_obligation"));
}

#[tokio::test]
async fn waive_unknown_target_errs() {
    let mut ledger = ObligationLedger::default();
    ledger.begin_turn("t");
    assert!(ledger.waive("不存在的 id", "reason", WaiveScope::Turn).is_err());
    // 工具层断言折成 obligation_not_found recoverable 错误。
    let pool = PgPoolOptions::new().connect_lazy("postgres://chatrpg:chatrpg@localhost:54347/chatrpg").expect("lazy pool");
    let engine = RuntimeEngine::new(Db { pool });
    let request = ContextRequest { ruleset_id: "rs".to_string(), module_id: None, session_id: "s".to_string(), turn_id: "t".to_string(), viewer: VisibilityProfile::gm(), token_budget: TokenBudget::default() };
    let state = RuntimeState { ruleset_id: "rs".to_string(), ..Default::default() };
    let cell = std::sync::Mutex::new(ObligationLedger::default());
    let ctx = ToolCtx { engine: &engine, request: &request, state: &state, scene_extractor: None, obligations: Some(&cell), data_dir: None, current_mode: None };
    let mut turn_ledger = TurnLedger::new();
    let err = match crate::tools::mechanic::WaiveObligationTool.call(&ctx, &mut turn_ledger, json!({"target_id":"due_missing","reason":"r"})).await {
        Ok(_) => panic!("expected obligation_not_found error"),
        Err(e) => e,
    };
    let tool_err = err.downcast_ref::<ToolError>().expect("typed ToolError");
    assert_eq!(tool_err.code, "obligation_not_found");
    assert!(tool_err.recoverable);
}

#[test]
fn resolve_dues_for_mechanic_clears_matching_open_dues_only() {
    let mut ledger = ObligationLedger::default();
    ledger.begin_turn("t");
    let mut matched = due("due_san", "sanity stimulus");
    matched.mechanic_id = Some("coc.sanity_roll_and_loss".to_string());
    let mut other = due("due_other", "another mechanic");
    other.mechanic_id = Some("coc.luck_roll".to_string());
    let unbound = due("due_unbound", "no mechanic binding");
    ledger.absorb_dues(vec![matched, other, unbound]);
    let resolved = ledger.resolve_dues_for_mechanic("coc.sanity_roll_and_loss");
    assert_eq!(resolved, vec!["due_san".to_string()]);
    let remaining: Vec<String> = ledger.blocking().into_iter().map(|v| v.target_id).collect();
    assert_eq!(remaining, vec!["due_other".to_string(), "due_unbound".to_string()]);
    // 再次结算同机制：无匹配 → 空（幂等）。
    assert!(ledger.resolve_dues_for_mechanic("coc.sanity_roll_and_loss").is_empty());
}

#[test]
fn scene_waivers_cleared_on_scene_change() {
    let mut ledger = ObligationLedger::default();
    ledger.begin_turn("t");
    ledger.absorb_dues(vec![due("due_scene", "sanity threshold crossed")]);
    ledger.record_open_check("check_turn");
    ledger.waive("due_scene", "calm scene", WaiveScope::Scene).expect("due_scene exists");
    ledger.waive("check_turn", "narrative override", WaiveScope::Turn).expect("check_turn exists");
    assert!(ledger.blocking().is_empty(), "both waived → nothing blocking");
    // 场景切换重开：Scene 豁免清除（该 due 立刻回到 blocking 视图）；
    // Turn 豁免不受影响（本回合内仍有效，begin_turn 才过期）。
    ledger.clear_scene_waivers();
    let blocking = ledger.blocking();
    assert_eq!(blocking.len(), 1, "scene-waived due must block again: {blocking:?}");
    assert_eq!(blocking[0].kind, "due");
    assert_eq!(blocking[0].target_id, "due_scene");
}

#[test]
fn settle_retro_debts_consumes_each_effect_once_fifo() {
    let mut ledger = ObligationLedger::default();
    ledger.begin_turn("t");
    ledger.absorb_retro_debts(vec![
        RetroactiveEffectDebt { debt_id: "debt_a".to_string(), turn_id: "t0".to_string(), finding_detail: "d".to_string(), kind: RetroDebtKind::Effect, created_at: Utc::now() },
        RetroactiveEffectDebt { debt_id: "debt_b".to_string(), turn_id: "t0".to_string(), finding_detail: "d".to_string(), kind: RetroDebtKind::Effect, created_at: Utc::now() },
    ]);
    // 同一 effect_id 重复出现（门控块每轮重扫账本）只消费一次。
    ledger.settle_retro_debts_with_effects(&["eff_1".to_string()]);
    ledger.settle_retro_debts_with_effects(&["eff_1".to_string()]);
    let blocking = ledger.blocking();
    assert_eq!(blocking.len(), 1, "one effect settles exactly one debt: {blocking:?}");
    assert_eq!(blocking[0].target_id, "debt_b", "FIFO: oldest debt settles first");
    // 第二条 effect 结清剩余债务；多余 effect 在无债务时是 no-op。
    ledger.settle_retro_debts_with_effects(&["eff_2".to_string(), "eff_3".to_string()]);
    assert!(ledger.blocking().is_empty());
    // 下回合 begin_turn 重置消费记录（TurnLedger 每回合新建，id 不复现）。
    ledger.begin_turn("t2");
    ledger.settle_retro_debts_with_effects(&["eff_1".to_string()]);
    assert!(ledger.blocking().is_empty(), "no debts -> settle is a no-op");
}

#[test]
fn check_debts_settle_only_with_checks_and_effects_dont_cross() {
    let mut ledger = ObligationLedger::default();
    ledger.begin_turn("t");
    ledger.absorb_retro_debts(vec![
        RetroactiveEffectDebt { debt_id: "debt_eff".to_string(), turn_id: "t0".to_string(), finding_detail: "claimed damage".to_string(), kind: RetroDebtKind::Effect, created_at: Utc::now() },
        RetroactiveEffectDebt { debt_id: "debt_chk".to_string(), turn_id: "t0".to_string(), finding_detail: "missed sanity roll".to_string(), kind: RetroDebtKind::Check, created_at: Utc::now() },
    ]);
    // Check 摘要带 roll_check 修复指引（GM 看见就知道用哪个工具清偿）。
    let summaries: Vec<String> = ledger.blocking().into_iter().map(|v| v.summary).collect();
    assert!(summaries.iter().any(|s| s.contains("roll_check")), "check debt must hint roll_check: {summaries:?}");
    // effect 清偿不跨种：只消 Effect 债务，Check 债务原样留下。
    ledger.settle_retro_debts_with_effects(&["eff_1".to_string()]);
    let blocking = ledger.blocking();
    assert_eq!(blocking.len(), 1);
    assert_eq!(blocking[0].target_id, "debt_chk");
    // check 清偿结清 Check 债务；同 check_id 重复结算只消费一次。
    ledger.settle_retro_debts_with_checks(&["chk_1".to_string()]);
    ledger.settle_retro_debts_with_checks(&["chk_1".to_string()]);
    assert!(ledger.blocking().is_empty());
}

#[test]
fn carryover_block_lists_unresolved_dues_with_evidence() {
    let mut ledger = ObligationLedger::default();
    ledger.begin_turn("t");
    ledger.absorb_dues(vec![due("due_keep", "距临时疯狂阈值一步")]);
    let block = ledger.carryover_block().expect("unresolved dues must carry over");
    // 含 threshold_desc 与 evidence 摘要（前后值）+ followup_procedure_id。
    assert!(block.contains("距临时疯狂阈值一步"), "missing threshold_desc: {block}");
    assert!(block.contains("38") && block.contains("32"), "missing evidence summary: {block}");
    assert!(block.contains("proc.followup"), "missing followup_procedure_id: {block}");
    assert!(block.contains("due_keep"));
}

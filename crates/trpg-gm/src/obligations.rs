//! 机械债务清单（spec §5.3 / 计划契约 §5）：①未结算 CheckContract（roll_check
//! 落了契约但执行被 blocked；request_player_roll gate 开着不算债、已 resolve 不算）
//! ②未处理 MechanicDue（本回合新产 + 跨回合遗留）③追溯债务（InventedEffect）。
//! 门控规则：blocking() 非空 → loop 不进叙事终态轮。门控/豁免逻辑只认 id 与
//! 状态，不认机制名（零 per-ruleset 硬编码）。

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use trpg_model::{ContextRequest, DueStatus, MechanicDue, MemoryEvent, MemoryKind, Visibility};
use uuid::Uuid;

/// 回合债务清单（GmLoop 持久字段，与 errata 同生命周期跨回合存活）。
#[derive(Debug, Default)]
pub struct ObligationLedger {
    open_check_ids: Vec<String>,
    dues: Vec<MechanicDue>,
    retro_debts: Vec<RetroactiveEffectDebt>,
    /// 本回合 waive 记录（scope=scene 的跨回合豁免凭 db status=waived +
    /// 场景切换时重开；scope=turn 的下回合 begin_turn 过期 → 仍提醒）。
    waivers: Vec<WaiverRecord>,
    /// waive 记账所需的当前回合 id（begin_turn 置位）。
    current_turn_id: String,
    /// 本回合已用于清偿追溯债务的 effect_id（每条只消费一次；TurnLedger 每回合
    /// 新建，跨回合 id 不复现 → begin_turn 清空即可）。
    consumed_effect_ids: Vec<String>,
    /// 三期 §4.4 mode 退出结算义务：只门 exit_mode（exit_blocking()），绝不进
    /// blocking()——否则姿态内每个叙事轮都会被自己的退出义务堵死（B6 复用 blocking）。
    mode_exit: Vec<ModeExitObligation>,
}

/// mode 退出结算义务（manifest.exit_obligations 声明、进入姿态/回合头部挂账）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeExitObligation {
    /// 确定性 id "mode_exit.<mode_id>.<idx>"（幂等 re-seed 去重 + waive 可寻址）。
    pub obligation_id: String,
    pub mode_id: String,
    pub description: String,
}

/// 追溯债务：流后 verifier 抓到 InventedEffect（叙事已流出不可回收）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetroactiveEffectDebt {
    /// "debt_{uuid simple}"。
    pub debt_id: String,
    /// 产生债务的回合。
    pub turn_id: String,
    /// VerifierFinding::InventedEffect 的 detail 原文。
    pub finding_detail: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaiverRecord { pub target_id: String, pub reason: String, pub scope: WaiveScope, pub turn_id: String }

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaiveScope { Turn, Scene }

impl WaiveScope {
    pub fn as_str(&self) -> &'static str {
        match self { WaiveScope::Turn => "turn", WaiveScope::Scene => "scene" }
    }
}

/// blocking() 的未清债务视图条目；kind ∈ {"due","check","debt"}（开放字符串）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObligationView { pub kind: String, pub target_id: String, pub summary: String }

impl ObligationLedger {
    /// 回合开始：置当前回合 id + 过期 turn-scope 豁免（Turn=默认，下回合仍提醒；
    /// Scene=本场景豁免，场景切换清除——跨进程由 db status=waived 兜底）。
    pub fn begin_turn(&mut self, turn_id: &str) {
        self.current_turn_id = turn_id.to_string();
        self.waivers.retain(|w| w.scope == WaiveScope::Scene);
        self.consumed_effect_ids.clear();
    }

    /// 追溯债务清偿（spec §5.3"补 apply_effect 落账"半边）：本回合每条新落账的
    /// effect_id 结清一条最早的追溯债务（FIFO；每个 effect_id 只消费一次）。
    /// 配对是粗粒度的（debt detail 是文本，无法确定性配对到具体 effect）——
    /// 语义配对留给 agent 的债务观察回填，引擎只做"落账即清偿"的保守记账。
    pub fn settle_retro_debts_with_effects(&mut self, effect_ids: &[String]) {
        for eid in effect_ids {
            if self.consumed_effect_ids.iter().any(|c| c == eid) { continue; }
            self.consumed_effect_ids.push(eid.clone());
            if !self.retro_debts.is_empty() { self.retro_debts.remove(0); }
        }
    }

    /// 场景切换重开（spec §5.3：scene 豁免"场景切换时清除豁免"）：清除内存侧
    /// Scene 豁免——被豁免的债务立刻回到 blocking 视图；Turn 豁免不受影响。
    /// db 侧同名重开（status waived→open）由 navigate_scene 成功路径经
    /// `reopen_scene_waived_dues` 完成，两侧合一才闭环（跨回合/跨进程）。
    pub fn clear_scene_waivers(&mut self) {
        self.waivers.retain(|w| w.scope != WaiveScope::Scene);
    }

    pub fn record_open_check(&mut self, check_id: &str) {
        if !self.open_check_ids.iter().any(|c| c == check_id) {
            self.open_check_ids.push(check_id.to_string());
        }
    }

    /// record_execution / awaiting gate 时调（不存在则 no-op）。
    pub fn mark_check_settled(&mut self, check_id: &str) {
        self.open_check_ids.retain(|c| c != check_id);
    }

    /// 吸收 dues（db 遗留 + hook/threshold 新产）；按 due_id 去重——db 装载与
    /// watcher 返回值可能是同一条（watcher 产出即落库）。
    pub fn absorb_dues(&mut self, dues: Vec<MechanicDue>) {
        for due in dues {
            if !self.dues.iter().any(|d| d.due_id == due.due_id) {
                self.dues.push(due);
            }
        }
    }

    pub fn absorb_retro_debts(&mut self, debts: Vec<RetroactiveEffectDebt>) {
        for debt in debts {
            if !self.retro_debts.iter().any(|d| d.debt_id == debt.debt_id) {
                self.retro_debts.push(debt);
            }
        }
    }

    fn waived(&self, target_id: &str) -> bool {
        self.waivers.iter().any(|w| w.target_id == target_id)
    }

    /// 进入姿态 / 回合头部 re-seed（幂等：确定性 id "mode_exit.<mode>.<idx>"
    /// 去重）。跨进程重启后由回合头部凭 manifest 重建，不依赖内存存活。
    pub fn ensure_mode_exit_obligations(&mut self, mode_id: &str, descriptions: &[String]) {
        for (idx, description) in descriptions.iter().enumerate() {
            let obligation_id = format!("mode_exit.{mode_id}.{idx}");
            if !self.mode_exit.iter().any(|o| o.obligation_id == obligation_id) {
                self.mode_exit.push(ModeExitObligation { obligation_id, mode_id: mode_id.to_string(), description: description.clone() });
            }
        }
    }

    /// exit_mode 成功后清账（该 mode 的退出义务整体撤销；其它 mode 不受影响）。
    pub fn clear_mode_exit_obligations(&mut self, mode_id: &str) {
        self.mode_exit.retain(|o| o.mode_id != mode_id);
    }

    /// exit_mode 门控视图（三期 spec §4.4）：常规 blocking() ∪ 未豁免的
    /// mode_exit 退出义务（kind="mode_exit"）。
    pub fn exit_blocking(&self) -> Vec<ObligationView> {
        let mut out = self.blocking();
        for o in &self.mode_exit {
            if self.waived(&o.obligation_id) { continue; }
            out.push(ObligationView { kind: "mode_exit".to_string(), target_id: o.obligation_id.clone(), summary: format!("mode '{}' exit obligation: {}", o.mode_id, o.description) });
        }
        out
    }

    /// 未清债务视图（已 resolve/waive 的剔除）；空 ⇒ 放行叙事轮。
    pub fn blocking(&self) -> Vec<ObligationView> {
        let mut out = Vec::new();
        for due in &self.dues {
            if due.status != DueStatus::Open || self.waived(&due.due_id) { continue; }
            out.push(ObligationView { kind: "due".to_string(), target_id: due.due_id.clone(), summary: due_summary(due) });
        }
        for check_id in &self.open_check_ids {
            if self.waived(check_id) { continue; }
            out.push(ObligationView { kind: "check".to_string(), target_id: check_id.clone(), summary: format!("unresolved check contract {check_id}") });
        }
        for debt in &self.retro_debts {
            if self.waived(&debt.debt_id) { continue; }
            out.push(ObligationView { kind: "debt".to_string(), target_id: debt.debt_id.clone(), summary: format!("retroactive effect debt (turn {}): {}", debt.turn_id, debt.finding_detail) });
        }
        out
    }

    /// system 观察回填文本；含 due 的 threshold_desc/followup_procedure_id 与
    /// evidence 摘要。无债务 → None。
    pub fn block_text(&self) -> Option<String> {
        let views = self.blocking();
        if views.is_empty() { return None; }
        Some(format!("[obligations]\n以下机械债务必须处理（roll_check / request_player_roll / apply_effect）或 waive_obligation（必须带理由）：\n{}\n[/obligations]", render_lines(&views)))
    }

    /// waive：target_id 匹配 due_id/check_id/debt_id；不存在 → Err
    /// （工具层转 obligation_not_found）。
    pub fn waive(&mut self, target_id: &str, reason: &str, scope: WaiveScope) -> Result<WaiverRecord> {
        let exists = self.dues.iter().any(|d| d.due_id == target_id)
            || self.open_check_ids.iter().any(|c| c == target_id)
            || self.retro_debts.iter().any(|d| d.debt_id == target_id)
            || self.mode_exit.iter().any(|o| o.obligation_id == target_id);
        if !exists { return Err(anyhow!("unknown obligation target: {target_id}")); }
        let record = WaiverRecord { target_id: target_id.to_string(), reason: reason.to_string(), scope, turn_id: self.current_turn_id.clone() };
        self.waivers.push(record.clone());
        Ok(record)
    }

    /// 回合收尾未清债务 → 下回合 BP3 obligations_block 文本；无 → None。
    pub fn carryover_block(&self) -> Option<String> {
        let views = self.blocking();
        if views.is_empty() { return None; }
        Some(format!("[obligations_carryover]\n上回合遗留未清机械债务（本回合必须处理或 waive_obligation 带理由，绝不静默丢失）：\n{}\n[/obligations_carryover]", render_lines(&views)))
    }
}

fn render_lines(views: &[ObligationView]) -> String {
    views.iter().map(|v| format!("- {} {}: {}", v.kind, v.target_id, v.summary)).collect::<Vec<_>>().join("\n")
}

/// due 行摘要：threshold_desc + followup + evidence（紧凑 JSON）。
fn due_summary(due: &MechanicDue) -> String {
    let mut s = due.threshold_desc.clone();
    if let Some(proc_id) = &due.followup_procedure_id {
        s.push_str(&format!(" (followup: {proc_id})"));
    }
    s.push_str(&format!(" [evidence: {}]", due.evidence));
    s
}

/// waive 审计载荷（复用 errata.to_memory_event 样板；tags=["gm_waive"]）。
/// 调用方负责 db.save_memory_event（落库失败仅 warn，绝不反向阻断回合）。
pub fn waiver_to_memory_event(request: &ContextRequest, record: &WaiverRecord) -> MemoryEvent {
    MemoryEvent { event_id: format!("mem_waive_{}", Uuid::new_v4().simple()), session_id: request.session_id.clone(), turn_id: Some(request.turn_id.clone()), ruleset_id: request.ruleset_id.clone(), module_id: request.module_id.clone(), scene_id: None, location_id: None, actor_ids: vec![], visibility: Visibility::GmOnly, event_kind: MemoryKind::Event, summary: format!("waived obligation {} (scope={}): {}", record.target_id, record.scope.as_str(), record.reason), transcript_excerpt: None, source: json!({"source":"waive_obligation"}), tags: vec!["gm_waive".to_string()], importance: 2, occurred_at: Utc::now() }
}

/// 轮耗尽带债强制叙事后的勘误记忆载荷（债务绝不静默丢失；BP3 注入由下回合
/// carryover_block 完成）。
pub fn carryover_memory_event(request: &ContextRequest, block: &str) -> MemoryEvent {
    MemoryEvent { event_id: format!("mem_obligations_{}", Uuid::new_v4().simple()), session_id: request.session_id.clone(), turn_id: Some(request.turn_id.clone()), ruleset_id: request.ruleset_id.clone(), module_id: request.module_id.clone(), scene_id: None, location_id: None, actor_ids: vec![], visibility: Visibility::GmOnly, event_kind: MemoryKind::Event, summary: block.chars().take(280).collect(), transcript_excerpt: None, source: json!({"source":"gm_agent.obligation_carryover"}), tags: vec!["gm_obligation_carryover".to_string()], importance: 2, occurred_at: Utc::now() }
}

#[cfg(test)]
mod tests {
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
            RetroactiveEffectDebt { debt_id: "debt_a".to_string(), turn_id: "t0".to_string(), finding_detail: "d".to_string(), created_at: Utc::now() },
            RetroactiveEffectDebt { debt_id: "debt_b".to_string(), turn_id: "t0".to_string(), finding_detail: "d".to_string(), created_at: Utc::now() },
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
}

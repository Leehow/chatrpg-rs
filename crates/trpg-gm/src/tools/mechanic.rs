//! Mechanics-catalog tools (phase 2): `LookupMechanicTool`（B2，目录全文按需拉取）
//! + `WaiveObligationTool`（B6，显式豁免一条机械债务，必须带理由、全程留审计）。
//! The catalog is knowledge, not a cage: lookup pulls the full entry on demand
//! (agent pays per use), it is never projected wholesale into BP1.

use crate::ledger::TurnLedger;
use crate::obligations::{waiver_to_memory_event, WaiveScope};
use crate::tools::{GmTool, ToolCtx, ToolError, ToolOutput, ToolSpec};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use trpg_model::{expressiveness_tier, ExpressivenessTier, MechanicEntry};

/// 纯函数：目录查找，id trim + 大小写不敏感相等
/// （id 相等比较是 §3.5.4 合法字面边界）。
pub fn find_mechanic<'a>(catalog: &'a [MechanicEntry], id: &str) -> Option<&'a MechanicEntry> {
    let needle = id.trim().to_lowercase();
    if needle.is_empty() {
        return None;
    }
    catalog
        .iter()
        .find(|e| e.id.trim().to_lowercase() == needle)
}

/// ExpressivenessTier → snake_case 字符串（仅映射，不重算分类——分类逻辑归 A1 纯函数）。
fn tier_label(tier: ExpressivenessTier) -> &'static str {
    match tier {
        ExpressivenessTier::Procedure => "procedure",
        ExpressivenessTier::Hook => "hook",
        ExpressivenessTier::PassiveModifier => "passive_modifier",
        ExpressivenessTier::Semantic => "semantic",
    }
}

#[derive(Debug, Clone, Deserialize)]
struct LookupArgs {
    id: String,
}

pub struct LookupMechanicTool;

#[async_trait]
impl GmTool for LookupMechanicTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "lookup_mechanic",
            schema: json!({"type":"function","function":{"name":"lookup_mechanic","description":"Fetch the full mechanics-catalog entry (procedure, hooks, followups, sources) by id from the BP1 index.","parameters":{"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}}}),
        }
    }
    fn capability(&self) -> crate::tools::ToolCapability {
        crate::tools::ToolCapability::ReadOnly
    }

    async fn call(
        &self,
        ctx: &ToolCtx<'_>,
        _ledger: &mut TurnLedger,
        args: Value,
    ) -> Result<ToolOutput> {
        let args: LookupArgs = serde_json::from_value(args).map_err(|e| {
            ToolError::recoverable(
                "invalid_arguments",
                format!("lookup_mechanic arguments invalid: {e}"),
                Some("Provide the catalog entry id.".to_string()),
            )
        })?;
        if args.id.trim().is_empty() {
            return Err(ToolError::recoverable(
                "invalid_arguments",
                "id is required",
                Some("Provide the catalog entry id from the BP1 mechanics index.".to_string()),
            ));
        }
        let kernel = ctx
            .engine
            .db
            .load_rule_kernel(&ctx.request.ruleset_id)
            .await?;
        // miss / kernel 无目录 → 同一错误码（fail-closed，agent 可改道）。
        let entry = kernel
            .as_ref()
            .and_then(|k| find_mechanic(&k.mechanics_catalog, &args.id));
        match entry {
            // 目录条目全文直接回 JSON 不截断——agent 主动拉取按需付费，不进 BP1。
            Some(e) => Ok(ToolOutput::ok(
                json!({"entry": e, "tier": tier_label(expressiveness_tier(e))}),
            )),
            None => Err(ToolError::recoverable(
                "mechanic_not_found",
                format!("mechanic not found in catalog: {}", args.id),
                Some(
                    "Check the BP1 mechanics index for valid ids, or use retrieve_rules."
                        .to_string(),
                ),
            )),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct WaiveArgs {
    target_id: String,
    reason: String,
    #[serde(default)]
    scope: Option<String>,
}

/// scope 解析：缺省/空串 → Turn；"turn"/"scene" 大小写不敏感；其余 → invalid_arguments。
fn parse_scope(raw: Option<&str>) -> Result<WaiveScope> {
    match raw.map(|s| s.trim().to_lowercase()) {
        None => Ok(WaiveScope::Turn),
        Some(s) if s.is_empty() || s == "turn" => Ok(WaiveScope::Turn),
        Some(s) if s == "scene" => Ok(WaiveScope::Scene),
        Some(other) => Err(ToolError::recoverable(
            "invalid_arguments",
            format!("unknown waive scope: {other}"),
            Some("Use \"turn\" or \"scene\".".to_string()),
        )),
    }
}

pub struct WaiveObligationTool;

#[async_trait]
impl GmTool for WaiveObligationTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "waive_obligation",
            schema: json!({"type":"function","function":{"name":"waive_obligation","description":"Explicitly waive a pending mechanical obligation (due/check/debt) WITH a reason. Waivers are audited.","parameters":{"type":"object","properties":{"target_id":{"type":"string"},"reason":{"type":"string"},"scope":{"type":"string","enum":["turn","scene"],"default":"turn"}},"required":["target_id","reason"]}}}),
        }
    }

    async fn call(
        &self,
        ctx: &ToolCtx<'_>,
        _ledger: &mut TurnLedger,
        args: Value,
    ) -> Result<ToolOutput> {
        let args: WaiveArgs = serde_json::from_value(args).map_err(|e| {
            ToolError::recoverable(
                "invalid_arguments",
                format!("waive_obligation arguments invalid: {e}"),
                Some("Provide target_id and a non-empty reason.".to_string()),
            )
        })?;
        let target_id = args.target_id.trim().to_string();
        if target_id.is_empty() {
            return Err(ToolError::recoverable(
                "invalid_arguments",
                "target_id is required",
                Some("Use an id from the [obligations] observation.".to_string()),
            ));
        }
        // reason 空串拦截：waiver 是审计对象，无理由不豁免。
        let reason = args.reason.trim().to_string();
        if reason.is_empty() {
            return Err(ToolError::recoverable(
                "invalid_arguments",
                "reason must not be empty: waivers are audited",
                Some("State why this obligation is being waived.".to_string()),
            ));
        }
        let scope = parse_scope(args.scope.as_deref())?;
        let Some(cell) = ctx.obligations else {
            return Err(ToolError::recoverable(
                "obligation_not_found",
                "no obligation ledger is attached to this turn",
                Some("Handle the obligation via roll_check/apply_effect instead.".to_string()),
            ));
        };
        // 内存侧豁免在锁块内完成（guard 不跨 await）：只豁免未清债务（blocking
        // 视图里的 id），kind 用于"仅 due 类"落 db 状态。
        let (kind, record) = {
            let mut obligations = cell.lock().unwrap_or_else(|p| p.into_inner());
            let Some(view) = obligations
                .blocking()
                .into_iter()
                .find(|v| v.target_id == target_id)
            else {
                return Err(ToolError::recoverable(
                    "obligation_not_found",
                    format!("no outstanding obligation with id: {target_id}"),
                    Some("Use an id from the [obligations] observation.".to_string()),
                ));
            };
            let record = obligations
                .waive(&target_id, &reason, scope)
                .map_err(|e| ToolError::recoverable("obligation_not_found", e.to_string(), None))?;
            (view.kind, record)
        };
        // 副作用三连其余两步（db 失败仅 warn：叙事路径绝不被审计反向阻断，
        // MockLlm/lazy-pool 单测下同样成立）：db waive 记账（仅 due 类）→ 勘误记忆审计。
        if kind == "due" {
            if let Err(err) = ctx
                .engine
                .db
                .update_mechanic_due_status(
                    &target_id,
                    "waived",
                    Some(record.reason.as_str()),
                    Some(record.scope.as_str()),
                )
                .await
            {
                tracing::warn!(error = %err, due_id = %target_id, "waive_obligation due status persist failed");
            }
        }
        let event = waiver_to_memory_event(ctx.request, &record);
        let audit_tags = event.tags.clone();
        if let Err(err) = ctx.engine.db.save_memory_event(&event).await {
            tracing::warn!(error = %err, "waive_obligation audit memory event persist failed");
        }
        Ok(ToolOutput::ok(
            json!({"waived": {"target_id": record.target_id, "reason": record.reason, "scope": record.scope.as_str()}, "audit_tags": audit_tags}),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::TurnLedger;
    use crate::tools::{ToolCtx, ToolError};
    use serde_json::json;
    use sqlx::postgres::PgPoolOptions;
    use trpg_db::Db;
    use trpg_model::{ContextRequest, RuntimeState, TokenBudget, VisibilityProfile};
    use trpg_runtime::RuntimeEngine;

    fn dummy_ctx() -> (RuntimeEngine, ContextRequest, RuntimeState) {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://chatrpg:chatrpg@localhost:54347/chatrpg")
            .expect("lazy pool");
        let engine = RuntimeEngine::new(Db { pool });
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
        (engine, request, state)
    }

    fn entry(id: &str) -> MechanicEntry {
        MechanicEntry {
            id: id.to_string(),
            name: id.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn lookup_finds_entry_case_insensitive() {
        let catalog = vec![entry("coc.skill.jump"), entry("coc.sanity_check")];
        let hit = find_mechanic(&catalog, " COC.Sanity_Check ").expect("entry should be found");
        assert_eq!(hit.id, "coc.sanity_check");
    }

    #[tokio::test]
    async fn lookup_missing_id_is_mechanic_not_found() {
        let (engine, request, state) = dummy_ctx();
        let ctx = ToolCtx {
            engine: &engine,
            request: &request,
            state: &state,
            scene_extractor: None,
            obligations: None,
            data_dir: None,
            current_mode: None,
            opposed_binding: None,
            nominated_reveals: None,
            rejected_nominations: None,
        };
        let mut ledger = TurnLedger::new();
        // ToolOutput 无 Debug，unwrap_err 不可用——match 取 Err。
        let err = match LookupMechanicTool
            .call(&ctx, &mut ledger, json!({"id":"coc.never_exists"}))
            .await
        {
            Ok(_) => panic!("expected mechanic_not_found error"),
            Err(e) => e,
        };
        let tool_err = err.downcast_ref::<ToolError>().expect("typed ToolError");
        assert_eq!(tool_err.code, "mechanic_not_found");
        assert!(tool_err.recoverable);
        assert!(tool_err
            .hint
            .as_deref()
            .unwrap_or("")
            .contains("retrieve_rules"));
    }
}

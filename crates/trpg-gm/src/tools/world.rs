use crate::ledger::TurnLedger;
use crate::tools::{GmTool, ToolCtx, ToolError, ToolOutput, ToolSpec};
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use trpg_mechanics::watcher::HookEvent;
use trpg_mechanics::RefereeCombatService;
use trpg_model::{MechanicDue, MemoryEvent, MemoryKind, TimeAdvanceRequest, TimeAmount, TimeScale, Visibility};
use uuid::Uuid;

#[derive(Debug, Clone, Deserialize)]
pub struct RetrieveArgs { pub query: String, pub k: Option<usize> }
#[derive(Debug, Clone, Deserialize)]
pub struct AdvanceTimeArgs { pub amount: i64, pub scale: String, pub reason: String }
#[derive(Debug, Clone, Deserialize)]
pub struct RememberArgs { pub summary: String, pub importance: Option<i32>, pub tags: Option<Vec<String>> }
#[derive(Debug, Clone, Deserialize)]
pub struct NavigateArgs { pub target_node_id: String, pub reason: String }

pub fn parse_time_scale(s: &str) -> Result<TimeScale> {
    match s.trim().to_ascii_lowercase().as_str() {
        "instant" => Ok(TimeScale::Instant),
        "combat_round" => Ok(TimeScale::CombatRound),
        "scene_beat" => Ok(TimeScale::SceneBeat),
        "exploration" => Ok(TimeScale::Exploration),
        "travel" => Ok(TimeScale::Travel),
        "downtime" => Ok(TimeScale::Downtime),
        "flashback" => Ok(TimeScale::Flashback),
        other => Err(ToolError::recoverable("invalid_arguments", format!("invalid time scale: {other}"), Some("Use instant, combat_round, scene_beat, exploration, travel, downtime, or flashback.".to_string()))),
    }
}

/// PURE：hook dues → 工具结果 JSON 数组（due_id/threshold_desc/mechanic_id）。
pub fn hook_dues_json(dues: &[MechanicDue]) -> Value {
    Value::Array(dues.iter().map(|d| json!({"due_id": d.due_id, "threshold_desc": d.threshold_desc, "mechanic_id": d.mechanic_id})).collect())
}

/// PURE：advance_time 结果 JSON（B5 增 "hook_dues" 键，空数组缺省）。
pub fn advance_time_result_json(from_tick: i64, to_tick: i64, triggered_events: usize, hook_dues: &[MechanicDue]) -> Value {
    json!({"from_tick": from_tick, "to_tick": to_tick, "triggered_events": triggered_events, "hook_dues": hook_dues_json(hook_dues)})
}

pub fn parse_navigate_args(value: Value) -> Result<NavigateArgs> {
    let args: NavigateArgs = serde_json::from_value(value).map_err(|e| ToolError::recoverable("invalid_arguments", format!("navigate_scene arguments invalid: {e}"), None))?;
    if args.target_node_id.trim().is_empty() { return Err(ToolError::recoverable("invalid_arguments", "target_node_id is required", None)); }
    Ok(args)
}

pub struct RetrieveRulesTool;
#[async_trait]
impl GmTool for RetrieveRulesTool {
    fn spec(&self) -> ToolSpec { ToolSpec { name: "retrieve_rules", schema: json!({"type":"function","function":{"name":"retrieve_rules","description":"Retrieve source-backed rule snippets.","parameters":{"type":"object","properties":{"query":{"type":"string"},"k":{"type":"integer"}},"required":["query"]}}}) } }
    async fn call(&self, ctx: &ToolCtx<'_>, _ledger: &mut TurnLedger, args: Value) -> Result<ToolOutput> {
        let args: RetrieveArgs = serde_json::from_value(args).map_err(|e| ToolError::recoverable("invalid_arguments", format!("retrieve_rules arguments invalid: {e}"), None))?;
        let text = ctx.engine.retrieve_rules(&ctx.request.ruleset_id, &args.query, args.k.unwrap_or(5)).await;
        Ok(ToolOutput::ok(json!({"snippets": text})))
    }
}

pub struct AdvanceTimeTool;
#[async_trait]
impl GmTool for AdvanceTimeTool {
    fn spec(&self) -> ToolSpec { ToolSpec { name: "advance_time", schema: json!({"type":"function","function":{"name":"advance_time","description":"Advance authoritative world time.","parameters":{"type":"object","properties":{"amount":{"type":"integer"},"scale":{"type":"string"},"reason":{"type":"string"}},"required":["amount","scale","reason"]}}}) } }
    async fn call(&self, ctx: &ToolCtx<'_>, _ledger: &mut TurnLedger, args: Value) -> Result<ToolOutput> {
        let args: AdvanceTimeArgs = serde_json::from_value(args).map_err(|e| ToolError::recoverable("invalid_arguments", format!("advance_time arguments invalid: {e}"), None))?;
        let scale = parse_time_scale(&args.scale)?;
        // TimeAmount 没有 value 字段（真实字段 seconds/minutes/hours/days/
        // combat_rounds/scene_beats/label）：按 scale 路由到既有构造器。
        let amount = match scale {
            TimeScale::CombatRound => TimeAmount::combat_rounds(args.amount),
            TimeScale::SceneBeat => TimeAmount::scene_beats(args.amount),
            _ => TimeAmount::minutes(args.amount),
        };
        let result = ctx.engine.advance_world_time(TimeAdvanceRequest { session_id: ctx.request.session_id.clone(), campaign_id: None, reason: args.reason, amount, scale, mutation_kind: Default::default(), visibility: Visibility::GmOnly, caused_by_turn_id: Some(ctx.request.turn_id.clone()), caused_by_event_id: None, scene_epoch: None }).await?;
        // B5 第三触发通路：advance 后发 TimeAdvance（目录 TimeAdvance 钩子直发、
        // Calendar 钩子在同一事件内走 calendar_crossed 闸）；downtime 再发 Rest
        // （数据映射，非新工具）。失败 unwrap_or_default——工具主链不因 watcher 中断。
        let svc = RefereeCombatService::new(ctx.engine.db.clone());
        let mut hook_dues = svc
            .dues_for_hook(&ctx.request.session_id, &ctx.request.turn_id, &ctx.request.ruleset_id, &HookEvent::TimeAdvance { from_tick: result.from.world_tick, to_tick: result.to.world_tick })
            .await
            .unwrap_or_default();
        if scale == TimeScale::Downtime {
            hook_dues.extend(svc.dues_for_hook(&ctx.request.session_id, &ctx.request.turn_id, &ctx.request.ruleset_id, &HookEvent::Rest).await.unwrap_or_default());
        }
        Ok(ToolOutput::ok(advance_time_result_json(result.from.world_tick, result.to.world_tick, result.triggered_events.len(), &hook_dues)))
    }
}

pub struct RememberTool;
#[async_trait]
impl GmTool for RememberTool {
    fn spec(&self) -> ToolSpec { ToolSpec { name: "remember", schema: json!({"type":"function","function":{"name":"remember","description":"Persist a GM memory event.","parameters":{"type":"object","properties":{"summary":{"type":"string"},"importance":{"type":"integer"},"tags":{"type":"array","items":{"type":"string"}}},"required":["summary"]}}}) } }
    async fn call(&self, ctx: &ToolCtx<'_>, _ledger: &mut TurnLedger, args: Value) -> Result<ToolOutput> {
        let args: RememberArgs = serde_json::from_value(args).map_err(|e| ToolError::recoverable("invalid_arguments", format!("remember arguments invalid: {e}"), None))?;
        // MemoryEvent 的字段名是 source（serde_json::Value），不是 source_json。
        let event = MemoryEvent { event_id: format!("mem_{}", Uuid::new_v4().simple()), session_id: ctx.request.session_id.clone(), turn_id: Some(ctx.request.turn_id.clone()), ruleset_id: ctx.request.ruleset_id.clone(), module_id: ctx.request.module_id.clone(), scene_id: None, location_id: None, actor_ids: vec![], visibility: Visibility::GmOnly, event_kind: MemoryKind::Event, summary: args.summary, transcript_excerpt: None, source: json!({"source":"gm_agent.remember"}), tags: args.tags.unwrap_or_else(|| vec!["gm_agent".to_string()]), importance: args.importance.unwrap_or(2), occurred_at: Utc::now() };
        ctx.engine.db.save_memory_event(&event).await?;
        Ok(ToolOutput::ok(json!({"event_id": event.event_id})))
    }
}

pub struct NavigateSceneTool;
#[async_trait]
impl GmTool for NavigateSceneTool {
    fn spec(&self) -> ToolSpec { ToolSpec { name: "navigate_scene", schema: json!({"type":"function","function":{"name":"navigate_scene","description":"Move to another module scene node after validation.","parameters":{"type":"object","properties":{"target_node_id":{"type":"string"},"reason":{"type":"string"}},"required":["target_node_id","reason"]}}}) } }
    async fn call(&self, ctx: &ToolCtx<'_>, _ledger: &mut TurnLedger, args: Value) -> Result<ToolOutput> {
        let args = parse_navigate_args(args)?;
        let module_id = ctx.request.module_id.as_ref().ok_or_else(|| ToolError::recoverable("no_module_loaded", "navigate_scene requires a module_id", None))?;
        let graph = ctx.engine.db.load_module_graph(module_id).await?.ok_or_else(|| ToolError::recoverable("no_module_loaded", format!("module graph not loaded: {module_id}"), None))?;
        // 校验顺序对齐 trpg_api::validate_transition：先存在性，后同场景——
        // target 不存在且恰等于 current 时必须报 scene_not_found 而非语义错位的 same_as_current。
        if !graph.scenes.iter().any(|s| s.node_id == args.target_node_id) { return Err(ToolError::recoverable("scene_not_found", format!("scene not found: {}", args.target_node_id), None)); }
        if ctx.state.scene_id.as_deref() == Some(args.target_node_id.as_str()) { return Err(ToolError::recoverable("scene_same_as_current", "target scene is already current", None)); }
        ctx.engine.db.set_session_scene(&ctx.request.session_id, &args.target_node_id).await?;
        // B6 场景切换重开（spec §5.3：scene 豁免"场景切换时清除豁免"）：db 侧
        // waive_scope='scene' 的 waived 行回到 open（watcher 抑制规则①接管），
        // 内存侧 Scene waivers 清除（本回合 blocking 视图立刻恢复提醒）。
        // db 写失败 `?` 上抛——豁免重开是"债务必清"的正确性写入，吞错会把
        // scene 豁免重新变成会话级永久豁免（恰是本修复要堵的洞）。
        ctx.engine.db.reopen_scene_waived_dues(&ctx.request.session_id).await?;
        if let Some(cell) = ctx.obligations {
            cell.lock().unwrap_or_else(|p| p.into_inner()).clear_scene_waivers();
        }
        // B5 第三触发通路：set_session_scene 成功后发 SceneEnter（检测单点在
        // dues_for_hook，此处只构造事件）。失败 unwrap_or_default 不阻断导航。
        let hook_dues = RefereeCombatService::new(ctx.engine.db.clone())
            .dues_for_hook(&ctx.request.session_id, &ctx.request.turn_id, &ctx.request.ruleset_id, &HookEvent::SceneEnter { scene_id: args.target_node_id.clone() })
            .await
            .unwrap_or_default();
        let extracted = if let Some(extractor) = ctx.scene_extractor { Some(extractor(args.target_node_id.clone()).await?) } else { None };
        Ok(ToolOutput::ok(json!({"scene_id": args.target_node_id, "reason": args.reason, "deep_extracted": extracted, "hook_dues": hook_dues_json(&hook_dues)})))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn invalid_time_scale_is_fail_closed() {
        let err = parse_time_scale("moon-cycle").unwrap_err();
        assert!(err.to_string().contains("invalid_arguments"));
    }

    #[test]
    fn registry_has_twelve_tools_in_stable_order() {
        // 三期批1 加入 enter_mode / exit_mode → 基础工具集扩为 14。
        // 测试名保留（历史），断言更新到真实顺序。
        let names = crate::tools::ToolRegistry::standard().schemas().into_iter()
            .map(|v| v.pointer("/function/name").and_then(|x| x.as_str()).unwrap_or("").to_string())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["roll_check", "request_player_roll", "apply_effect", "change_track", "retrieve_rules", "get_actor", "ensure_npc_param", "navigate_scene", "advance_time", "remember", "lookup_mechanic", "waive_obligation", "enter_mode", "exit_mode"]);
    }

    #[test]
    fn navigate_args_require_target() {
        let err = parse_navigate_args(json!({"reason":"move"})).unwrap_err();
        assert!(err.to_string().contains("invalid_arguments"));
    }

    #[test]
    fn advance_time_result_carries_hook_dues_key_shape() {
        // 纯断言结果 JSON 形状：lazy pool 下 dues 为空数组缺省键即可
        // （真触发归 C5/C6 e2e）。
        let v = advance_time_result_json(0, 60, 0, &[]);
        assert_eq!(v.get("from_tick").and_then(|x| x.as_i64()), Some(0));
        assert_eq!(v.get("to_tick").and_then(|x| x.as_i64()), Some(60));
        assert_eq!(v.get("triggered_events").and_then(|x| x.as_i64()), Some(0));
        assert_eq!(v.get("hook_dues"), Some(&json!([])), "hook_dues key must default to an empty array");
    }
}

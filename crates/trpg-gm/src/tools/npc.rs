use crate::ledger::TurnLedger;
use crate::tools::{GmTool, ToolCtx, ToolError, ToolOutput, ToolSpec};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use trpg_params::RuntimeParameterService;
use trpg_runtime::npc_synth::NpcPersona;

#[derive(Debug, Clone, Deserialize)]
pub struct GetActorArgs { pub actor_id: String }
#[derive(Debug, Clone, Deserialize)]
pub struct EnsureNpcParamArgs { pub npc_id: String, pub bucket: String, pub param: String, pub context: String }

pub struct GetActorTool;
#[async_trait]
impl GmTool for GetActorTool {
    fn spec(&self) -> ToolSpec { ToolSpec { name: "get_actor", schema: json!({"type":"function","function":{"name":"get_actor","description":"Return GM-visible actor sheet and mechanical profile.","parameters":{"type":"object","properties":{"actor_id":{"type":"string"}},"required":["actor_id"]}}}) } }
    async fn call(&self, ctx: &ToolCtx<'_>, _ledger: &mut TurnLedger, args: Value) -> Result<ToolOutput> {
        let args: GetActorArgs = serde_json::from_value(args).map_err(|e| ToolError::recoverable("invalid_arguments", format!("get_actor arguments invalid: {e}"), None))?;
        let service = RuntimeParameterService::new(ctx.engine.db.clone());
        let actor = service.load_actor_parameters(&ctx.request.session_id, &args.actor_id).await?.ok_or_else(|| ToolError::recoverable("actor_not_found", format!("actor not found: {}", args.actor_id), Some("Use an established actor id or ensure the actor appears in the current scene.".to_string())))?;
        Ok(ToolOutput::ok(json!({"actor_id": actor.actor_id, "display_name": actor.display_name, "sheet_json": actor.sheet_json, "mechanical_profile": actor.mechanical_profile, "status_json": actor.status_json})))
    }
}

pub struct EnsureNpcParamTool;
#[async_trait]
impl GmTool for EnsureNpcParamTool {
    fn spec(&self) -> ToolSpec { ToolSpec { name: "ensure_npc_param", schema: json!({"type":"function","function":{"name":"ensure_npc_param","description":"Materialize an NPC parameter before contest resolution.","parameters":{"type":"object","properties":{"npc_id":{"type":"string"},"bucket":{"type":"string"},"param":{"type":"string"},"context":{"type":"string"}},"required":["npc_id","bucket","param","context"]}}}) } }
    async fn call(&self, ctx: &ToolCtx<'_>, _ledger: &mut TurnLedger, args: Value) -> Result<ToolOutput> {
        let args: EnsureNpcParamArgs = serde_json::from_value(args).map_err(|e| ToolError::recoverable("invalid_arguments", format!("ensure_npc_param arguments invalid: {e}"), None))?;
        let module_id = ctx.request.module_id.as_ref().ok_or_else(|| ToolError::recoverable("no_module_loaded", "ensure_npc_param requires a module", None))?;
        let graph = ctx.engine.db.load_module_graph(module_id).await?.ok_or_else(|| ToolError::recoverable("no_module_loaded", format!("module graph not loaded: {module_id}"), None))?;
        let persona = persona_from_graph(&graph.npcs, &args.npc_id)?;
        let value = ctx.engine.ensure_npc_parameter(&ctx.request.session_id, &ctx.request.ruleset_id, &persona, &args.bucket, &args.param, &args.context).await?;
        Ok(ToolOutput::ok(json!({"npc_id": args.npc_id, "bucket": args.bucket, "param": args.param, "value": value})))
    }
}

/// 从 ModuleGraph.npcs 解析 NPC 人设。npc_id 不存在 ⇒ 结构化 npc_not_found
/// （fail-closed：绝不兜底造空 persona 继续合成——空人设会让 persona-judge
/// 凭空编值，与 actor_not_found/scene_not_found 同风格）。
fn persona_from_graph(npcs: &[Value], npc_id: &str) -> Result<NpcPersona> {
    let raw = npcs.iter().find(|v| v.get("id").and_then(Value::as_str) == Some(npc_id)).ok_or_else(|| ToolError::recoverable(
        "npc_not_found",
        format!("npc not found in module graph: {npc_id}"),
        Some("Use get_actor to check established actors, or use an exact NPC id from the current scene's referenced NPC list.".to_string()),
    ))?;
    Ok(NpcPersona {
        actor_id: npc_id.to_string(),
        name: raw.get("name").and_then(Value::as_str).unwrap_or(npc_id).to_string(),
        prose: raw.get("body").or_else(|| raw.get("summary")).and_then(Value::as_str).unwrap_or("").to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolError;

    fn npcs() -> Vec<Value> {
        vec![json!({"id": "npc.lars", "name": "拉斯", "summary": "加油站老板", "body": "沉默寡言的退伍兵。"})]
    }

    #[test]
    fn persona_from_graph_resolves_existing_npc() {
        let persona = persona_from_graph(&npcs(), "npc.lars").unwrap();
        assert_eq!(persona.actor_id, "npc.lars");
        assert_eq!(persona.name, "拉斯");
        assert_eq!(persona.prose, "沉默寡言的退伍兵。");
    }

    #[test]
    fn missing_npc_is_structured_npc_not_found_not_fail_open() {
        // 终审 important 回归：npc_id 不在 graph.npcs ⇒ 绝不兜底造空 persona，
        // 返回 recoverable npc_not_found + hint（agent 可换路 get_actor/查场景列表）。
        let err = persona_from_graph(&npcs(), "npc.ghost").unwrap_err();
        let tool = err.downcast_ref::<ToolError>().expect("must be structured ToolError");
        assert_eq!(tool.code, "npc_not_found");
        assert!(tool.recoverable);
        assert!(tool.message.contains("npc.ghost"));
        assert!(tool.hint.as_deref().unwrap_or("").contains("get_actor"));
    }
}

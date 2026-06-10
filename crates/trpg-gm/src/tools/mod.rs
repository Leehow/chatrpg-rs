pub mod check;
pub mod effect;
pub mod npc;
pub mod world;

use crate::ledger::TurnLedger;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use trpg_llm::AggregatedToolCall;
use trpg_model::{ContextRequest, RuntimeState};
use trpg_runtime::RuntimeEngine;

/// 工具元数据：name + OpenAI function schema 整体
/// （{"type":"function","function":{"name":...,"description":...,"parameters":...}}）。
pub struct ToolSpec {
    pub name: &'static str,
    pub schema: Value,
}

/// 到场深抽回调：由装配方注入（CLI 一期在 agent_play.rs 用闭包包
/// trpg_api::extract_module_scenes(only=Some(node_id))）。trpg-gm 不依赖
/// trpg-api —— 避免二期 API 复用 run_gm_turn 时 trpg-api ↔ trpg-gm 循环依赖。
pub type SceneDeepExtractFn =
    Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<usize>> + Send>> + Send + Sync>;

/// 工具执行上下文：回合内只读的引擎/请求/状态 + 注入能力。
pub struct ToolCtx<'a> {
    pub engine: &'a RuntimeEngine,
    pub request: &'a ContextRequest,
    pub state: &'a RuntimeState,
    /// None = 无模组或装配方未提供（navigate_scene 仅校验+切换，跳过深抽）。
    pub scene_extractor: Option<&'a SceneDeepExtractFn>,
}

/// 工具单次执行的产物。
pub struct ToolOutput {
    /// 回填给模型的结果 JSON（成功语义）。
    pub result: Value,
    /// Some ⇒ request_player_roll 成功落库，loop 必须以 AwaitingPlayerRoll 终态结束回合。
    pub awaiting_player_roll: Option<AwaitingPlayerRoll>,
}

impl ToolOutput {
    pub fn ok(result: Value) -> Self { Self { result, awaiting_player_roll: None } }
    pub fn awaiting(result: Value, gate: AwaitingPlayerRoll) -> Self { Self { result, awaiting_player_roll: Some(gate) } }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AwaitingPlayerRoll {
    pub check_id: String,
    pub prompt_public: String,
}

/// 单个 GM 工具。Err 由 dispatch 统一转结构化 ToolError JSON 回填（绝不 panic、
/// 不静默、不中断 loop）；工具实现内用 ToolError::recoverable/fatal 构造类型化错误。
#[async_trait]
pub trait GmTool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    async fn call(&self, ctx: &ToolCtx<'_>, ledger: &mut TurnLedger, args: Value) -> Result<ToolOutput>;
}

/// 结构化工具错误（spec §5/§7：fail-closed 但 agent 可见可修复）。
/// 回填给模型的 JSON 形状固定为：
/// {"error":{"code":"missing_kernel_dice","message":"...","recoverable":true,"hint":"..."}}
///
/// 错误码全集（机器可读，新增需在此登记）：
///   invalid_arguments        参数缺失/形状不对（含 tested_parameter 缺失）
///   unknown_tool             模型调了不存在的工具名
///   missing_kernel_dice      kernel.dice_core 无骰式 → 改走 retrieve_rules 或纯叙事
///   unknown_parameter        tested_parameter 在角色卡/kernel 轨上都解析不到
///   actor_not_found          actor_id 无参数行
///   npc_not_found            ensure_npc_param 的 npc_id 不在 ModuleGraph.npcs
///   no_module_loaded         navigate_scene/ensure_npc_param 但会话无模组
///   scene_not_found          target_node_id 不在 ModuleGraph.scenes
///   scene_same_as_current    target == 当前场景
///   npc_synthesis_unavailable 合成门关 / 无 LLM / 无 NPC 卡
///   internal_error           db/IO 等内部错误（recoverable=false）
#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct ToolError {
    pub code: String,
    pub message: String,
    /// true ⇒ agent 应换路（retrieve_rules / request_player_roll / 纯叙事），而非重试同参数。
    pub recoverable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl ToolError {
    pub fn recoverable(code: &str, message: impl Into<String>, hint: Option<String>) -> anyhow::Error {
        anyhow::Error::new(Self { code: code.to_string(), message: message.into(), recoverable: true, hint })
    }

    pub fn fatal(code: &str, message: impl Into<String>) -> anyhow::Error {
        anyhow::Error::new(Self { code: code.to_string(), message: message.into(), recoverable: false, hint: None })
    }

    /// dispatch 出口：任意 anyhow::Error → `{"error":{...}}` 序列化文本
    /// （downcast_ref::<ToolError> 优先；否则折为 code=internal_error, recoverable=false）。
    pub fn to_tool_content(err: &anyhow::Error) -> String {
        let error = if let Some(tool) = err.downcast_ref::<ToolError>() {
            tool.clone()
        } else {
            ToolError { code: "internal_error".to_string(), message: err.to_string(), recoverable: false, hint: None }
        };
        serde_json::to_string(&json!({"error": error})).unwrap_or_else(|_| "{\"error\":{\"code\":\"internal_error\",\"message\":\"serialization failed\",\"recoverable\":false}}".to_string())
    }
}

/// 一次 dispatch 的产物：tool role 回填消息 + 可能的回合终态信号。
pub struct ToolDispatchOutcome {
    pub tool_call_id: String,
    pub name: String,
    /// tool role message 的 content（ToolOutput.result 或 ToolError JSON 的序列化文本）。
    pub content: String,
    pub awaiting_player_roll: Option<AwaitingPlayerRoll>,
}

/// 第一期 10 工具注册表。
pub struct ToolRegistry {
    tools: Vec<Box<dyn GmTool>>,
}

impl ToolRegistry {
    /// 注册全部 10 个第一期工具，注册顺序固定（= schemas() 顺序，缓存稳定）：
    /// roll_check, request_player_roll, apply_effect, change_track, retrieve_rules,
    /// get_actor, ensure_npc_param, navigate_scene, advance_time, remember。
    pub fn standard() -> Self {
        Self { tools: vec![
            Box::new(check::RollCheckTool),
            Box::new(check::RequestPlayerRollTool),
            Box::new(effect::ApplyEffectTool),
            Box::new(effect::ChangeTrackTool),
            Box::new(world::RetrieveRulesTool),
            Box::new(npc::GetActorTool),
            Box::new(npc::EnsureNpcParamTool),
            Box::new(world::NavigateSceneTool),
            Box::new(world::AdvanceTimeTool),
            Box::new(world::RememberTool),
        ] }
    }

    /// turn_loop 单测注入脚本化 GmTool 替身用（tools 字段私有，跨模块测试只能经此构造）。
    pub fn from_tools(tools: Vec<Box<dyn GmTool>>) -> Self { Self { tools } }

    /// 传给 stream_chat_with_tools 的 function schemas（确定性顺序）。
    pub fn schemas(&self) -> Vec<Value> {
        self.tools.iter().map(|t| t.spec().schema).collect()
    }

    /// 执行一次聚合后的调用：unknown_tool / arguments 非合法 JSON → ToolError 回填；
    /// 本方法自身永不 Err（错误全部折进 content），loop 不因工具失败中断。
    pub async fn dispatch(
        &self,
        ctx: &ToolCtx<'_>,
        ledger: &mut TurnLedger,
        call: &AggregatedToolCall,
    ) -> ToolDispatchOutcome {
        let Some(tool) = self.tools.iter().find(|t| t.spec().name == call.name) else {
            let err = ToolError::recoverable("unknown_tool", format!("unknown GM tool: {}", call.name), Some("Use one of the advertised function schemas.".to_string()));
            return ToolDispatchOutcome { tool_call_id: call.id.clone(), name: call.name.clone(), content: ToolError::to_tool_content(&err), awaiting_player_roll: None };
        };
        let args = match serde_json::from_str::<Value>(&call.arguments) {
            Ok(v) => v,
            Err(e) => {
                let err = ToolError::recoverable("invalid_arguments", format!("arguments are not valid JSON: {e}"), Some("Emit a valid JSON object matching the tool schema.".to_string()));
                return ToolDispatchOutcome { tool_call_id: call.id.clone(), name: call.name.clone(), content: ToolError::to_tool_content(&err), awaiting_player_roll: None };
            }
        };
        match tool.call(ctx, ledger, args).await {
            Ok(output) => ToolDispatchOutcome { tool_call_id: call.id.clone(), name: call.name.clone(), content: serde_json::to_string(&output.result).unwrap_or_else(|_| "{}".to_string()), awaiting_player_roll: output.awaiting_player_roll },
            Err(err) => ToolDispatchOutcome { tool_call_id: call.id.clone(), name: call.name.clone(), content: ToolError::to_tool_content(&err), awaiting_player_roll: None },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::TurnLedger;
    use anyhow::Result;
    use async_trait::async_trait;
    use serde_json::{json, Value};
    use sqlx::postgres::PgPoolOptions;
    use trpg_db::Db;
    use trpg_model::{ContextRequest, RuntimeState, TokenBudget, VisibilityProfile};
    use trpg_runtime::RuntimeEngine;

    struct EchoTool;

    #[async_trait]
    impl GmTool for EchoTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec { name: "echo", schema: json!({"type":"function","function":{"name":"echo","description":"echo","parameters":{"type":"object","properties":{"x":{"type":"string"}},"required":["x"]}}}) }
        }

        async fn call(&self, _ctx: &ToolCtx<'_>, _ledger: &mut TurnLedger, args: Value) -> Result<ToolOutput> {
            Ok(ToolOutput::ok(json!({"x": args.get("x").and_then(Value::as_str).unwrap_or("")})))
        }
    }

    fn dummy_ctx() -> (RuntimeEngine, ContextRequest, RuntimeState) {
        let pool = PgPoolOptions::new().connect_lazy("postgres://chatrpg:chatrpg@localhost:54347/chatrpg").expect("lazy pool");
        let engine = RuntimeEngine::new(Db { pool });
        let request = ContextRequest { ruleset_id: "rs".to_string(), module_id: None, session_id: "s".to_string(), turn_id: "t".to_string(), viewer: VisibilityProfile::gm(), token_budget: TokenBudget::default() };
        let state = RuntimeState { ruleset_id: "rs".to_string(), ..Default::default() };
        (engine, request, state)
    }

    #[tokio::test]
    async fn dispatch_unknown_tool_returns_structured_error() {
        let registry = ToolRegistry::standard();
        let (engine, request, state) = dummy_ctx();
        let ctx = ToolCtx { engine: &engine, request: &request, state: &state, scene_extractor: None };
        let mut ledger = TurnLedger::new();
        let outcome = registry.dispatch(&ctx, &mut ledger, &trpg_llm::AggregatedToolCall { id: "c1".to_string(), name: "missing".to_string(), arguments: "{}".to_string() }).await;
        let v: Value = serde_json::from_str(&outcome.content).unwrap();
        assert_eq!(v.pointer("/error/code").and_then(Value::as_str), Some("unknown_tool"));
        assert_eq!(v.pointer("/error/recoverable").and_then(Value::as_bool), Some(true));
    }

    #[tokio::test]
    async fn dispatch_invalid_arguments_returns_structured_error() {
        let registry = ToolRegistry { tools: vec![Box::new(EchoTool)] };
        let (engine, request, state) = dummy_ctx();
        let ctx = ToolCtx { engine: &engine, request: &request, state: &state, scene_extractor: None };
        let mut ledger = TurnLedger::new();
        let outcome = registry.dispatch(&ctx, &mut ledger, &trpg_llm::AggregatedToolCall { id: "c2".to_string(), name: "echo".to_string(), arguments: "not-json".to_string() }).await;
        let v: Value = serde_json::from_str(&outcome.content).unwrap();
        assert_eq!(v.pointer("/error/code").and_then(Value::as_str), Some("invalid_arguments"));
    }
}

#[cfg(test)]
mod schema_stability_tests {
    use super::*;

    #[test]
    fn schema_serialization_is_stable() {
        let a = serde_json::to_vec(&ToolRegistry::standard().schemas()).unwrap();
        let b = serde_json::to_vec(&ToolRegistry::standard().schemas()).unwrap();
        assert_eq!(a, b);
    }
}


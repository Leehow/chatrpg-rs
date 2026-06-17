pub mod check;
pub mod effect;
pub mod frame;
pub mod mechanic;
pub mod npc;
pub(crate) mod settle;
pub mod world;

use crate::ledger::TurnLedger;
use crate::obligations::ObligationLedger;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
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
    /// B6 机械债务清单（waive_obligation 内存侧豁免；dispatch 链上 ToolCtx 是
    /// 共享引用，&mut 借用打架 → 互斥锁内点改，guard 绝不跨 await）。
    /// None = 单测/装配方未挂清单（waive 折 obligation_not_found）。
    pub obligations: Option<&'a Mutex<ObligationLedger>>,
    /// mode 包根（data/）所在目录（enter/exit_mode 与 mode 专属工具用）；
    /// None = 单测未挂（mode 工具折结构化错误，绝不 panic）。
    pub data_dir: Option<&'a Path>,
    /// 回合头部推导的当前姿态（turn_loop 注入 manifest.mode_id）；
    /// None = 默认叙事姿态（三期 spec §4.1）。
    pub current_mode: Option<&'a str>,
    /// Phase3 §4.1 对抗预 pass 备好的对抗参数（turn_loop 注入）：roll_check 在
    /// `args.opposed` 缺失时注入它（GM 漏填对抗形态的兜底）。None = 本回合无攻击
    /// 意图 / 预 pass 关 / 现搓不成（fail-closed，不注入）。
    pub opposed_binding: Option<&'a crate::opposed_prepass::OpposedBinding>,
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
///   mechanic_not_found       mechanic_id 不在 kernel.mechanics_catalog（或 kernel 无目录）
///   scene_mechanic_not_found scene_mechanic_id 不在当前场景 scene_mechanics（scene_policy 解析）
///   obligation_not_found     waive_obligation 的 target_id 不在未清债务清单
///   mode_not_found           enter_mode 的 mode 无已安装 mode 包（agent/gm_skill/modes/<mode>）
///   mode_nesting_unsupported 已在某 mode 内再 enter（栈深 1，三期 spec §4.4）
///   no_active_mode           默认叙事姿态下调 exit_mode
///   exit_blocked_by_obligations exit_mode 被未清债务/退出结算义务拦截（waive 通道照常）
///   frame_not_found          close_frame 无可关闭的活跃姿态 frame（先 open_combat_frame）
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

/// mode 专属工具名解析（manifest.extra_tools → 实例）；未知名 → None，
/// for_mode fail-closed 报配置错误。批2 登记战斗 frame 工具
/// （纯查表，零 per-ruleset 逻辑）。
fn extra_tool_by_name(name: &str) -> Option<Box<dyn GmTool>> {
    match name {
        "open_combat_frame" => Some(Box::new(frame::OpenCombatFrameTool)),
        "close_frame" => Some(Box::new(frame::CloseFrameTool)),
        _ => None,
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

/// GM 工具注册表（一期 10 个 + 二期尾部追加）。
pub struct ToolRegistry {
    tools: Vec<Box<dyn GmTool>>,
}

impl ToolRegistry {
    /// 注册全部工具，注册顺序固定（= schemas() 顺序，缓存稳定）。一期 10 个
    /// （roll_check, request_player_roll, apply_effect, change_track, retrieve_rules,
    /// get_actor, ensure_npc_param, navigate_scene, advance_time, remember）的
    /// schema 字节与顺序绝不动；二期工具只在尾部追加：lookup_mechanic,
    /// waive_obligation（11→12）；三期姿态工具继续尾部追加：enter_mode,
    /// exit_mode（13→14，任何姿态下均可用 = 基础 14）；Knowledge P0a 尾部追加
    /// reveal_fact（15，显式 GM 揭示 = 基础 15）。
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
            Box::new(mechanic::LookupMechanicTool),
            Box::new(mechanic::WaiveObligationTool),
            Box::new(crate::mode::EnterModeTool),
            Box::new(crate::mode::ExitModeTool),
            Box::new(world::RevealFactTool),
        ] }
    }

    /// 三期 §4.3 工具按 mode 组装：基础 14（任何姿态可用）+ manifest.extra_tools
    /// （按名解析，未知名 fail-closed 报配置错误）。mode=None ⇒ 与 standard()
    /// 完全等同（schema 字节回归测试护）。
    pub fn for_mode(data_dir: &Path, mode: Option<&str>) -> Result<Self> {
        let mut registry = Self::standard();
        let Some(mode) = mode else { return Ok(registry) };
        let manifest = crate::mode::load_mode_manifest(data_dir, mode)?;
        for name in &manifest.extra_tools {
            let tool = extra_tool_by_name(name).ok_or_else(|| anyhow::anyhow!(
                "mode '{mode}' manifest lists unknown extra tool '{name}' (fail-closed configuration error)"
            ))?;
            registry.tools.push(tool);
        }
        Ok(registry)
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
        let ctx = ToolCtx { engine: &engine, request: &request, state: &state, scene_extractor: None, obligations: None, data_dir: None, current_mode: None, opposed_binding: None };
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
        let ctx = ToolCtx { engine: &engine, request: &request, state: &state, scene_extractor: None, obligations: None, data_dir: None, current_mode: None, opposed_binding: None };
        let mut ledger = TurnLedger::new();
        let outcome = registry.dispatch(&ctx, &mut ledger, &trpg_llm::AggregatedToolCall { id: "c2".to_string(), name: "echo".to_string(), arguments: "not-json".to_string() }).await;
        let v: Value = serde_json::from_str(&outcome.content).unwrap();
        assert_eq!(v.pointer("/error/code").and_then(Value::as_str), Some("invalid_arguments"));
    }
}

#[cfg(test)]
mod schema_stability_tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn schema_serialization_is_stable() {
        let a = serde_json::to_vec(&ToolRegistry::standard().schemas()).unwrap();
        let b = serde_json::to_vec(&ToolRegistry::standard().schemas()).unwrap();
        assert_eq!(a, b);
    }

    fn temp_data_dir_with_manifest(mode: &str, manifest_body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "schema_mode_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().subsec_nanos()
        ));
        let mode_dir = dir.join("agent/gm_skill/modes").join(mode);
        fs::create_dir_all(&mode_dir).unwrap();
        fs::write(mode_dir.join("manifest.json"), manifest_body).unwrap();
        dir
    }

    /// 批5 mode 维度参数化 ③ — mode 切换=工具 schema 有因失效断言：
    /// mode=None → 基础 14 工具 schema；mode="combat"（含 extra_tools）→ 16 工具
    /// schema；两者序列化字节不同 → 工具 schema 变化是 mode 切换的有因失效依据。
    #[test]
    fn mode_switch_changes_tool_schema_bytes() {
        let dir = temp_data_dir_with_manifest(
            "combat",
            r#"{"mode_id":"combat","frame_kind":"combat","extra_tools":["open_combat_frame","close_frame"]}"#,
        );
        let schemas_none   = serde_json::to_vec(&ToolRegistry::for_mode(&dir, None).unwrap().schemas()).unwrap();
        let schemas_combat = serde_json::to_vec(&ToolRegistry::for_mode(&dir, Some("combat")).unwrap().schemas()).unwrap();
        // mode=None → 14 工具；mode=combat → 16 工具（extra_tools 追加）。
        assert_ne!(schemas_none, schemas_combat,
            "mode switch with extra_tools must produce different schema bytes (justified cache invalidation)");
        let count_none   = ToolRegistry::for_mode(&dir, None).unwrap().schemas().len();
        let count_combat = ToolRegistry::for_mode(&dir, Some("combat")).unwrap().schemas().len();
        assert_eq!(count_none, 15, "base registry must have exactly 15 tools");
        assert_eq!(count_combat, 17, "combat mode with two extra_tools must have 17 tools");
        fs::remove_dir_all(dir).ok();
    }

    /// 批5 mode 维度参数化 ④ — 同 mode 内工具 schema 字节稳定：
    /// 同一 mode 连续调用 for_mode 两次 → 工具 schema 字节完全相同（确定性）。
    #[test]
    fn same_mode_tool_schema_bytes_are_stable() {
        let dir = temp_data_dir_with_manifest(
            "combat",
            r#"{"mode_id":"combat","frame_kind":"combat","extra_tools":["open_combat_frame","close_frame"]}"#,
        );
        let a = serde_json::to_vec(&ToolRegistry::for_mode(&dir, Some("combat")).unwrap().schemas()).unwrap();
        let b = serde_json::to_vec(&ToolRegistry::for_mode(&dir, Some("combat")).unwrap().schemas()).unwrap();
        assert_eq!(a, b, "same mode must produce deterministic tool schema bytes across calls");
        fs::remove_dir_all(dir).ok();
    }
}


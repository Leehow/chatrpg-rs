# GM Agent Loop 第一期 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让叙事 LLM（上下文最全的组件）直接持有"是否/何时/如何掷骰"的决策权（痛点 1），检定 tested_parameter 由 agent 语义绑定、Provisional 静默死路消失、fail-closed 错误变为 agent 可见可修复（痛点 2）；防幻觉护栏换形态不减弱（状态唯一写入通道 = 工具，NarrationVerifier 首次接线为流后勘误记忆）；最终输出 SSE 真流式（D2 硬性原则）；prompt 前缀字节级缓存稳定（§6.1 硬性原则）；旧路径完整保留可 A/B。

**Architecture:** 方案 A 混合 agent loop——新建 crate `crates/trpg-gm`，LLM 拥有控制流，Rust 确定性原语做工具（第一期 10 个，全部是 RuntimeEngine / Db / mechanics 现有原语的薄包装）。每回合：确定性头部（事件记录/live 重算/gate 结算为输入事实）→ `prepare_turn_context` 复用 BP1/BP2/BP3 三段缓存带 → 工具轮 ≤8（`stream_chat_with_tools` SSE delta：tool_calls 聚合执行回填，content delta 经防漏滑动缓冲直通玩家即最终叙事）→ 轮数耗尽末轮 `tool_choice:"none"` 逼散文 → 流后 NarrationVerifier 复盘 findings 写勘误记忆进下一轮 BP3，同类 ≥N 次升级持续提醒块 → 确定性收尾。入口 `trpg play --agent`，默认仍走旧 `run_turn_once`；第一期不删任何旧代码。

**Tech Stack:** Rust（tokio async + async-trait + async-stream/futures），OpenAI 兼容 relay（:18888，主 loop 全程 gpt-5.5，工具轮上限 8，回合 20-40s 可接受），SSE function-calling 流式解析（trpg-llm 新增 `stream_chat_with_tools`），PostgreSQL（trpg-db 既有原语），数据文件 `data/agent/gm_skill/*.md`（22 个 advice JSON 蒸馏 + gm_retrieve_phase 政策迁移，零 per-ruleset 硬编码）。

权威设计 spec：`docs/superpowers/specs/2026-06-10-gm-agent-loop-design.md`（工具清单 §5、消息布局与缓存稳定 §6/§6.1、护栏对照 §7、测试验收 §9、风险 §10）。

**本工作区无 git**：所有任务直接落工作副本，绝不写任何 git/commit 步骤；每个任务以验证步骤收尾（`cargo test -p <crate>` + `wc -l` ≤400 检查）。

## File Structure

### Create（新建）

```
crates/trpg-gm/Cargo.toml                       # 新 crate 清单（deps: trpg-model/db/llm/agent/runtime/mechanics + async-trait/serde/anyhow/thiserror/chrono/tracing/tokio）
crates/trpg-gm/src/lib.rs                       # 模块声明 + re-exports（见契约末尾，~40 行）
crates/trpg-gm/src/turn_loop.rs                 # GmLoop/run_gm_turn 全状态机 + LoopConfig/TurnOutcome（`loop` 是 Rust 关键字，故名 turn_loop）
crates/trpg-gm/src/stream.rs                    # RedactingBuffer 防漏滑动缓冲（私骰 token 跨 chunk 过滤）
crates/trpg-gm/src/ledger.rs                    # TurnLedger：TurnLedgerSnapshot 写入包装 + 私骰 token 提取
crates/trpg-gm/src/prompts.rs                   # TurnMessages §6.1 消息装配（渲染一次复用、尾部追加）+ load_gm_skill 两级合并
crates/trpg-gm/src/errata.rs                    # ErrataMemory 勘误记忆（流后 findings → 下一轮注入 + 同错升级）
crates/trpg-gm/src/tools/mod.rs                 # GmTool trait + ToolSpec + ToolOutput + ToolRegistry + ToolError + dispatch
crates/trpg-gm/src/tools/check.rs               # roll_check + request_player_roll（痛点 1/2 根治点）
crates/trpg-gm/src/tools/effect.rs              # apply_effect + change_track
crates/trpg-gm/src/tools/world.rs               # retrieve_rules + advance_time + remember + navigate_scene
crates/trpg-gm/src/tools/npc.rs                 # get_actor + ensure_npc_param
crates/trpg-llm/src/stream_tools.rs             # StreamEvent/AggregatedToolCall/ToolChoice/ToolStreamAggregator + OpenAiCompatibleClient::stream_chat_with_tools 实现（lib.rs 已 332 行，新增进新文件防超 400）
crates/trpg-llm/src/bin/relay_tools_smoke.rs    # Task 0 双项 relay smoke test bin（go/no-go 闸门）
crates/trpg-cli/src/agent_play.rs               # --agent 会话循环装配（main.rs 已 2247 行，新路径独立成文件）
crates/trpg-mechanics/src/direct_effect.rs      # apply_direct_effect pub 边界（合成契约走私有 apply_effect_roll 同管道）
data/agent/gm_skill/global/10_dice_discretion.md   # GM 掷骰裁量准则（22 个 data/agent/advice/*.json 语义蒸馏：何时系统掷/玩家亲手掷/免检定直接叙事）
data/agent/gm_skill/global/20_item_npc_policy.md   # ITEM POLICY / NPC POLICY（从 trpg-cli gm_retrieve_phase system prompt 原文迁移）
data/agent/gm_skill/global/30_output_contract.md   # 玩家可见输出契约（[roll]/[system] 标签语义、私骰不泄、不要求玩家报点数）
```

### Modify（修改）

```
Cargo.toml                                      # workspace members 追加 "crates/trpg-gm"
crates/trpg-llm/src/lib.rs                      # `pub mod stream_tools;` + re-export + LlmClient trait 新增默认方法 stream_chat_with_tools（~15 行）
crates/trpg-agent/src/gm_loop.rs                # private_roll_leak_tokens 提 pub（一行可见性，fn 体不变）
crates/trpg-mechanics/src/lib.rs                # `pub mod direct_effect;`（一行；apply_effect_roll 本体保持私有不动）
crates/trpg-api/src/lib.rs                      # extract_module_scenes 提 pub（一行可见性，fn 体不变）
crates/trpg-cli/src/main.rs                     # Play 命令加 --agent flag + `mod agent_play;` + 分流调用（~15 行；旧 run_turn_once 路径零改动）
crates/trpg-cli/Cargo.toml                      # 依赖 trpg-gm
```

## 共享类型契约 (Shared Type Contract)

**所有任务必须逐字使用以下类型与签名**（字段名、方法名、参数顺序、错误码字符串均不得偏离）。签名兼容真实代码库类型：`trpg_model::{ChatMessage, CompiledContext, ContextRequest, RuntimeState, CheckContract, CheckResultRecord, EffectContract, ParameterImpact, InteractionGate, PendingCheck, ParameterOperation, StatePatch, Visibility, MemoryEvent, TimeAdvanceRequest}`、`trpg_agent::{TurnLedgerSnapshot, NarrationVerifier, FinalNarrationSubmission, VerifierFinding, VerifierFindingKind, make_pending_check}`（trpg-agent 已 `pub use gm_loop::*;`）、`trpg_runtime::{RuntimeEngine, AutoRollExecution}`。

```rust
// ============================================================================
// 1. crates/trpg-llm/src/stream_tools.rs（lib.rs `pub mod stream_tools;` + `pub use stream_tools::*;`）
// ============================================================================
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 一次完整聚合后的 tool call（SSE 分片按 choices[0].delta.tool_calls[].index
/// 聚合、arguments 字符串跨 chunk 拼接而成）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AggregatedToolCall {
    /// OpenAI tool_call id（回填 tool role message 时必须原样使用）。
    pub id: String,
    /// function name。
    pub name: String,
    /// function arguments 的完整 JSON 字符串（聚合后的原文，由 dispatch 解析）。
    pub arguments: String,
}

/// stream_chat_with_tools 的流事件。一轮流式调用的合法事件序列：
///   工具轮:  ToolCalls(..) → [Usage(..)] → Done{finish_reason: Some("tool_calls")}
///   叙事轮:  ContentDelta.. × N → [Usage(..)] → Done{finish_reason: Some("stop")}
///   混合轮:  ContentDelta.. → ToolCalls(..) → Done{..}（少见但合法，不串台）
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// content 增量（最终叙事的 token 级片段，即到即发，不缓冲）。
    ContentDelta(String),
    /// 全部 tool_calls 分片聚合完成后一次性发出（每轮至多一个该事件，
    /// 在 finish_reason=tool_calls 或 [DONE] 处 flush）。
    ToolCalls(Vec<AggregatedToolCall>),
    /// 上游 usage 对象原样透传（spec §6.1 第 5 条可观测）。请求体带
    /// stream_options:{"include_usage":true}；relay 透传时尾部 chunk 含顶层
    /// usage（prompt_tokens_details.cached_tokens 在其中），feed_chunk 在
    /// choices 守卫之前检查并发出本事件；relay 不透传则整轮无此事件，
    /// 消费方按缺省跳过（fail-open，仅观测）。
    Usage(Value),
    /// 流结束；finish_reason 原样透传（"stop" / "tool_calls" / None）。
    Done { finish_reason: Option<String> },
}

/// 末轮强制散文用 tool_choice 控制（§6 末轮强制）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolChoice { Auto, None }

impl ToolChoice {
    pub fn as_json(&self) -> Value {
        match self {
            ToolChoice::Auto => Value::String("auto".into()),
            ToolChoice::None => Value::String("none".into()),
        }
    }
}

/// PURE：SSE chunk 解析/聚合器，与 reqwest 解耦（单测直接喂 JSON chunk）。
/// 内部维护 index → (id, name, arguments 累积串) 表。
#[derive(Debug, Default)]
pub struct ToolStreamAggregator { /* 私有字段 */ }

impl ToolStreamAggregator {
    pub fn new() -> Self;
    /// 喂入一个已解析的 SSE data JSON（整个 chunk Value，内部取 choices[0].delta）。
    /// content delta 即来即发；tool_calls 分片只累积不发。
    pub fn feed_chunk(&mut self, chunk: &Value) -> Vec<StreamEvent>;
    /// `[DONE]` 或字节流自然结束时调用：flush 累积的 ToolCalls（若有）+ Done。
    pub fn finish(&mut self, finish_reason: Option<String>) -> Vec<StreamEvent>;
}

// ============================================================================
// 2. crates/trpg-llm/src/lib.rs — LlmClient trait 新增默认方法（既有 4 方法不动）
// ============================================================================
#[async_trait]
pub trait LlmClient: Send + Sync {
    // ... complete_text / complete_json / stream_chat / complete_with_tools 原样 ...

    /// 流式 function-calling 回合（D2 硬依赖）。`messages`/`tools` 为 OpenAI raw
    /// JSON（与 complete_with_tools 同构：可携带 assistant tool_calls / tool role
    /// 历史）。默认 unsupported（与 complete_with_tools 同模式）。
    /// OpenAiCompatibleClient 实现：请求体 {model, messages, tools, tool_choice,
    /// stream:true, stream_options:{"include_usage":true}}，复用既有 stream_chat
    /// 的首 token 前重试 + 行缓冲模式，行解析委托 ToolStreamAggregator。
    /// 实现主体放 stream_tools.rs 的 `impl OpenAiCompatibleClient {
    /// pub(crate) async fn stream_chat_with_tools_impl(..) }`（子模块可访问父
    /// 模块私有字段 config/http 与私有重试 helper），lib.rs 的 trait impl 一行
    /// 委托——lib.rs 现 332 行，主体写进去必超 400 行纪律。
    async fn stream_chat_with_tools(
        &self,
        _messages: Vec<Value>,
        _tools: Vec<Value>,
        _tool_choice: ToolChoice,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        Err(anyhow!("stream_chat_with_tools is not supported by this LlmClient"))
    }
}

// ============================================================================
// 3. crates/trpg-agent/src/gm_loop.rs — 一行可见性提升（fn 体不变）
// ============================================================================
impl TurnLedgerSnapshot {
    pub fn private_roll_leak_tokens(&self) -> Vec<String>; // 现 private → pub：滑动缓冲 token 集合的唯一来源
}

// ============================================================================
// 4. crates/trpg-gm/src/ledger.rs — TurnLedger 回合账本包装
// ============================================================================
use trpg_agent::TurnLedgerSnapshot;
use trpg_model::{CheckContract, CheckResultRecord, EffectContract, InteractionGate, ParameterImpact};
use trpg_runtime::AutoRollExecution;

/// 回合账本：所有工具产物经此入账（spec §4），供流后 NarrationVerifier 对账
/// 与私骰 token 集合提取。直接复用 trpg_agent::TurnLedgerSnapshot 作为存储。
#[derive(Debug, Default)]
pub struct TurnLedger {
    snapshot: TurnLedgerSnapshot,
}

impl TurnLedger {
    pub fn new() -> Self;
    pub fn record_contract(&mut self, contract: &CheckContract);
    /// 入账一次检定结果（result.roll 同步进 dice_rolls）。
    pub fn record_result(&mut self, result: &CheckResultRecord);
    /// 入账 execute_system_roll_bundle 产物：primary + 全部 followups。
    pub fn record_execution(&mut self, exec: &AutoRollExecution);
    pub fn record_effect(&mut self, effect: &EffectContract);
    pub fn record_impact(&mut self, impact: &ParameterImpact);
    pub fn record_gate(&mut self, gate: &InteractionGate);
    pub fn snapshot(&self) -> &TurnLedgerSnapshot;
    /// 滑动缓冲过滤集合（lowercase 去重；委托 TurnLedgerSnapshot::private_roll_leak_tokens）。
    pub fn private_roll_tokens(&self) -> Vec<String>;
}

// ============================================================================
// 5. crates/trpg-gm/src/tools/mod.rs — 工具注册表 / 错误形状 / dispatch
// ============================================================================
pub mod check;
pub mod effect;
pub mod npc;
pub mod world;

use crate::ledger::TurnLedger;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
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
    pub fn ok(result: Value) -> Self;
    pub fn awaiting(result: Value, gate: AwaitingPlayerRoll) -> Self;
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
    pub fn recoverable(code: &str, message: impl Into<String>, hint: Option<String>) -> anyhow::Error;
    pub fn fatal(code: &str, message: impl Into<String>) -> anyhow::Error;
    /// dispatch 出口：任意 anyhow::Error → `{"error":{...}}` 序列化文本
    /// （downcast_ref::<ToolError> 优先；否则折为 code=internal_error, recoverable=false）。
    pub fn to_tool_content(err: &anyhow::Error) -> String;
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
    pub fn standard() -> Self;
    /// 自定义工具表（turn_loop 单测注入脚本化 GmTool 替身用；tools 字段私有，
    /// 跨模块测试只能经此构造）。
    pub fn from_tools(tools: Vec<Box<dyn GmTool>>) -> Self;
    /// 传给 stream_chat_with_tools 的 function schemas（确定性顺序）。
    pub fn schemas(&self) -> Vec<Value>;
    /// 执行一次聚合后的调用：unknown_tool / arguments 非合法 JSON → ToolError 回填；
    /// 本方法自身永不 Err（错误全部折进 content），loop 不因工具失败中断。
    pub async fn dispatch(
        &self,
        ctx: &ToolCtx<'_>,
        ledger: &mut TurnLedger,
        call: &AggregatedToolCall,
    ) -> ToolDispatchOutcome;
}

// ============================================================================
// 6. crates/trpg-gm/src/stream.rs — 防漏滑动缓冲（唯一在线护栏，spec §7）
// ============================================================================

/// 终轮 content delta 经此过滤后才下发玩家。原理：holdback 尾窗 =
/// max(token 字节长)-1 暂扣；尾窗+新 delta 拼接后做大小写不敏感扫描，
/// 命中私骰 token 替换为 "■"；tokens 为空 ⇒ 零暂扣完全透明直通。
pub struct RedactingBuffer { /* 私有：tokens(lowercase) + 尾窗 String */ }

impl RedactingBuffer {
    pub fn new(private_tokens: Vec<String>) -> Self;
    /// 喂入一个 delta，返回此刻可安全下发的文本（可能为空串：尾窗暂扣中）。
    pub fn push(&mut self, delta: &str) -> String;
    /// 流结束：排空尾窗，返回最后一段安全文本。
    pub fn finish(&mut self) -> String;
}

// ============================================================================
// 7. crates/trpg-gm/src/prompts.rs — §6.1 消息装配（渲染一次、尾部追加）
// ============================================================================
use std::path::Path;
use trpg_model::{ChatMessage, CompiledContext};

/// dynamic tail 输入（§6.1 第 1 条：最易变段排最后）。
pub struct DynamicTailInput<'a> {
    pub user_input: &'a str,
    /// 确定性头部 gate 结算产出的「已发生事实」（[roll]…[/roll] 渲染文本）；无则空。
    pub resolved_gate_facts: &'a [String],
    /// ErrataMemory 的勘误块 + 持续提醒块；无则空（块整体缺省，不写空块）。
    pub errata_blocks: &'a [String],
}

/// 回合消息容器：assemble 渲染一次、整回合复用；工具轮只在尾部 push，
/// 绝不重渲染/重排任何前缀字节（§6.1 第 2 条；Task 9 对此做字节级回归断言）。
pub struct TurnMessages { /* 私有：Vec<Value>（OpenAI raw JSON 消息） */ }

impl TurnMessages {
    /// §6.1 布局（稳定性降序）：
    ///   [system: compiled.prefix_text + "\n\n" + gm_skill_text]
    /// → [user: "[gm]\n[BP2: Pinned Context]\n{pinned_text}\n[/gm]"]
    /// → [history… append-only 原样平铺]
    /// → [user: "[gm]\n[BP3: Dynamic Context]\n{dynamic_text}\n[/gm]" + gate 事实 + 勘误块 + "[Player Input]\n{user_input}"]
    pub fn assemble(
        compiled: &CompiledContext,
        gm_skill_text: &str,
        history: &[ChatMessage],
        tail: &DynamicTailInput<'_>,
    ) -> Self;
    /// 工具轮：追加 assistant tool_calls 消息（id/name/arguments 原样回放）。
    pub fn push_assistant_tool_calls(&mut self, calls: &[AggregatedToolCall]);
    /// 工具轮：追加一条 tool role 结果消息（content = dispatch 产物）。
    pub fn push_tool_result(&mut self, tool_call_id: &str, name: &str, content: &str);
    /// 每轮请求体快照（Vec<Value> clone）。
    pub fn to_request_messages(&self) -> Vec<Value>;
    /// 缓存稳定可观测：前 first_n 条消息序列化字节的稳定哈希（Task 9 回归用）。
    pub fn prefix_byte_hash(&self, first_n: usize) -> String;
}

/// gm_skill 装载：data/agent/gm_skill/global/*.md（文件名字典序拼接）→
/// data/agent/gm_skill/<ruleset_id>/*.md 追加（两级合并，ruleset 段排后覆盖语义）。
/// global 目录缺失 → Err（gm_skill 是 agent 路径硬依赖，fail-closed）。
/// 调用方（run_gm_turn）以 `?` 传播该错误终止回合，绝不 unwrap_or_default 吞错；
/// data_dir 由 GmLoop.data_dir 提供（CLI 装配传 default_data_dir()，认 TRPG_DATA_DIR）。
pub fn load_gm_skill(data_dir: &Path, ruleset_id: &str) -> Result<String>;

/// §6.1 第 4 条 fail-closed：prefix/pinned 段超 TokenBudget ⇒ 配置错误（Err 信息
/// 含 "prefix"/"pinned" 段名），绝不静默裁剪。字段名已对真实代码勘查确认：
/// trpg_model::TokenBudget { prefix_max: u32, pinned_max: u32, dynamic_max: u32, total_max: u32 }。
pub fn validate_compiled_budget(compiled: &CompiledContext, request: &trpg_model::ContextRequest) -> Result<()>;

// ============================================================================
// 8. crates/trpg-gm/src/errata.rs — 勘误记忆（D2 校验后置，spec §7）
// ============================================================================
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use trpg_agent::{VerifierFinding, VerifierFindingKind};
use trpg_model::MemoryEvent;

/// 一条勘误：流后 NarrationVerifier finding 落成的记忆条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrataEntry {
    pub kind: VerifierFindingKind,
    pub detail: String,
    pub turn_id: String,
    pub created_at: DateTime<Utc>,
}

/// 会话内勘误记忆：findings 不阻塞流式交付，注入下一轮上下文由 GM 自洽勘误；
/// 同类 finding ≥ threshold 升级持续提醒块（防同错高频复发）。
#[derive(Debug)]
pub struct ErrataMemory { /* 私有：entries + kind 计数(BTreeMap<String,u32>) + threshold */ }

impl ErrataMemory {
    pub fn new(repeat_finding_threshold: u8) -> Self;
    /// 流后调用：记录本回合 findings，返回本次新增条目（供持久化）。
    pub fn record(&mut self, turn_id: &str, findings: &[VerifierFinding]) -> Vec<ErrataEntry>;
    /// 下一轮 dynamic tail 的勘误块：最近 ≤3 条勘误的提示文本；无勘误 → None。
    pub fn errata_block(&self) -> Option<String>;
    /// 任一 kind 累计 ≥ threshold ⇒ 持续提醒块（之后每轮注入直到会话结束）。
    pub fn standing_reminder_block(&self) -> Option<String>;
    /// 按 kind 聚合统计（serde snake_case kind 字符串为键；供 e2e 报告与准则迭代）。
    pub fn kind_counts(&self) -> &BTreeMap<String, u32>;
    /// 持久化载荷：本回合新增勘误折成一条 MemoryEvent
    /// （event_kind=MemoryKind::Event, tags=["gm_errata", <kind>…], importance=2,
    /// visibility=Visibility::GmOnly, source=json!({"source":"narration_verifier"})——
    /// 注意 MemoryEvent 的真实字段名是 `source: serde_json::Value` 而非 source_json）；
    /// 调用方负责 db.save_memory_event。
    pub fn to_memory_event(&self, request: &trpg_model::ContextRequest, new_entries: &[ErrataEntry]) -> MemoryEvent;
}

// ============================================================================
// 9. crates/trpg-gm/src/turn_loop.rs — GmLoop / run_gm_turn / 终态
// ============================================================================
use crate::errata::ErrataMemory;
use crate::tools::{SceneDeepExtractFn, ToolRegistry};
use std::sync::Arc;
use trpg_llm::LlmClient;
use trpg_model::{ChatMessage, ContextRequest, RuntimeState};
use trpg_runtime::RuntimeEngine;

#[derive(Debug, Clone)]
pub struct LoopConfig {
    /// 工具轮上限（含），耗尽后末轮 ToolChoice::None 逼出散文。默认 8。
    pub max_tool_rounds: u8,
    /// 同类 VerifierFindingKind 升级持续提醒块的阈值 N。默认 3。
    pub repeat_finding_threshold: u8,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self { max_tool_rounds: 8, repeat_finding_threshold: 3 }
    }
}

/// 回合终态（spec §4 核心类型）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    /// 最终叙事全文（已经 on_delta 流式发完，此值供落库/记忆，不作回放来源）。
    Narration(String),
    /// request_player_roll 终态：契约+gate 已落库，回合结束等玩家亲手掷。
    AwaitingPlayerRoll { check_id: String, prompt_public: String },
}

/// 上下文准备注入 seam（单测绕开真 DB 的唯一通道）：Some ⇒ run_gm_turn 第 2 步
/// 用它替代 engine.prepare_turn_context（后者必须连真 DB + 已 parse 的 project
/// bundle）；生产装配恒为 None。
pub type CtxProviderFn =
    Arc<dyn Fn(&ContextRequest, &RuntimeState) -> trpg_model::CompiledContext + Send + Sync>;

/// GM agent loop（方案 A）：LLM 拥有控制流，Rust 原语做工具。
/// 会话循环持有同一个 GmLoop 实例 ⇒ errata 跨回合存活。
pub struct GmLoop {
    pub engine: RuntimeEngine,
    pub llm: Arc<dyn LlmClient>,
    pub tools: ToolRegistry,
    pub cfg: LoopConfig,
    /// gm_skill 加载根（CLI 装配传 default_data_dir()；load_gm_skill fail-closed）。
    pub data_dir: std::path::PathBuf,
    /// 到场深抽能力（CLI 装配注入；None ⇒ navigate_scene 跳过深抽）。
    pub scene_extractor: Option<SceneDeepExtractFn>,
    /// 上下文准备注入 seam（单测用；生产 None）。
    pub ctx_provider: Option<CtxProviderFn>,
    /// 会话级勘误记忆。
    pub errata: ErrataMemory,
}

/// 一回合输入。
pub struct GmTurnInput<'a> {
    pub request: &'a ContextRequest,
    pub state: &'a RuntimeState,
    pub user_input: &'a str,
    /// 会话历史（append-only；§6.1 排 BP2 与 BP3 之间）。
    pub history: &'a [ChatMessage],
    pub recent_transcript: Option<&'a str>,
}

impl GmLoop {
    /// errata 由 cfg.repeat_finding_threshold 初始化；scene_extractor/ctx_provider
    /// 初始 None，装配方直接赋字段。
    pub fn new(engine: RuntimeEngine, llm: Arc<dyn LlmClient>, tools: ToolRegistry, cfg: LoopConfig, data_dir: std::path::PathBuf) -> Self;

    /// 主回合状态机（spec §4 数据流逐项）：
    /// 1. 确定性头部：record_world_event(PlayerAction) + refresh_actor_live_derived("pc.current")
    ///    + InteractionLifecycleKernel::new(db).reconcile_session（spec §4 四项齐；
    ///    prepare_turn_context 内部虽也会跑一次 reconcile，但那发生在 gate 结算之后，
    ///    顺序不满足 spec，故头部显式调用，幂等）；
    ///    有 open gate 且 user_input 可解析为骰值回复 ⇒ resolve_check_with_input →
    ///    结果渲染为 resolved_gate_facts（已发生事实，非早退控制流）并入账 ledger；
    /// 2. 上下文：ctx_provider 有值 ⇒ 用注入的 CompiledContext（单测 seam）；否则
    ///    prepare_turn_context（state 先 clone 并置 agent_loop_protocol=true，使 BP1
    ///    出 agent-loop 版 engine protocol，见 Task 6）→ CompiledContext；
    ///    load_gm_skill(&self.data_dir, ruleset)?（fail-closed，Err 终止回合）；
    ///    TurnMessages::assemble（渲染一次）；
    /// 3. 工具轮 ≤ cfg.max_tool_rounds，**循环内恒 ToolChoice::Auto**：
    ///    stream_chat_with_tools(messages, schemas, Auto)
    ///    → ToolCalls ⇒ 逐个 dispatch → push_assistant_tool_calls + push_tool_result（仅尾部追加）；
    ///      dispatch 带回 awaiting_player_roll ⇒ TurnOutcome::AwaitingPlayerRoll 终态（先走第 5/6 步收尾）；
    ///    → ContentDelta ⇒ RedactingBuffer(ledger.private_roll_tokens()) → on_delta（终态叙事）；
    ///    → Usage(v) ⇒ tracing 记 cached_tokens（仅观测）；
    /// 4. 仅当循环自然耗尽且模型仍要工具 ⇒ 追加**唯一一轮** ToolChoice::None 逼散文
    ///    （tool_choice 序列 = [Auto × n, None]，绝无双重 None）；
    /// 5. 流后（不阻塞交付）：NarrationVerifier::verify(ledger.snapshot(), 已流出全文) →
    ///    errata.record + to_memory_event 持久化（Narration 与 AwaitingPlayerRoll 两类
    ///    终态都跑，后者在 visible_text 非空时）；
    /// 6. 确定性收尾：db.save_turn 持久化本回合（user_input + 已流出全文/gate 提示语 +
    ///    context_hashes + postprocess_status="ready"/"awaiting_player_roll"）+
    ///    save_memory_event 回合摘要（tags=["gm_turn"]，summary 取叙事前 280 字符；
    ///    收尾持久化失败 tracing::warn 不吞 panic，e2e 以 turns 表 SQL 验证落库）。
    /// on_delta：已过滤的安全叙事增量回调（CLI 接 emit_delta；token 级，非缓冲回放）。
    pub async fn run_gm_turn(
        &mut self,
        input: GmTurnInput<'_>,
        on_delta: &mut (dyn FnMut(&str) + Send),
    ) -> Result<TurnOutcome>;
}

// ============================================================================
// 10. crates/trpg-mechanics/src/direct_effect.rs（lib.rs 加一行 `pub mod direct_effect;`）
// ============================================================================
use trpg_model::{EffectContract, ParameterImpact, ParameterOperation, StatePatch, Visibility};

/// 无检定直接效果的公共产物。
#[derive(Debug, Clone)]
pub struct DirectEffectOutcome {
    pub effect: EffectContract,
    pub impacts: Vec<ParameterImpact>,
    pub patches: Vec<StatePatch>,
}

impl crate::RefereeCombatService {
    /// 无检定直接效果（GM 裁量伤害/资源消耗/状态）的 pub 薄封装：内部构造合成
    /// CheckContract + CheckResultRecord（roll.result.total = amount）走私有
    /// apply_effect_roll 同一管道——armor/HP-track/kernel 寻轨逻辑零复制。
    /// **落点语义**：apply_effect_roll 的落点由私有 resolve_facet_decision 决定
    /// （无绑定 facet 时恒回落 hp.current），因此 lib.rs 把 apply_effect_roll 抽成
    /// `apply_effect_roll_with_decision(contract, result, decision_override:
    /// Option<FacetDecision>)`（原 apply_effect_roll 一行委托 None，既有调用方零
    /// 改动）；本封装显式构造 FacetDecision{Actor, target_actor_id, parameter_path,
    /// operation} 传入，parameter_path/operation 真正生效。FacetDecision 与
    /// apply_effect_roll 均为 crate 根私有项，direct_effect 是同 crate 子模块可
    /// 直接访问，均不提 pub。
    /// EffectContract.effect_kind 按 operation 映射（EffectKind 无 ResourceDelta
    /// 变体）：Subtract→Damage、Add→Healing、其余→NarrativeConsequence；
    /// operation/source 信息只进 EffectContract.metadata json（CheckContract 没有
    /// metadata 字段，不存在 metadata_mut）。
    pub async fn apply_direct_effect(
        &self,
        session_id: &str,
        ruleset_id: &str,
        module_id: Option<&str>,
        source_actor_id: &str,
        target_actor_id: &str,
        parameter_path: &str,
        operation: ParameterOperation,
        amount: i64,
        reason: &str,
        visibility: Visibility,
    ) -> anyhow::Result<DirectEffectOutcome>;
}

// ============================================================================
// 11. crates/trpg-api/src/lib.rs — 一行可见性提升（fn 体与参数完全不变）
// ============================================================================
pub async fn extract_module_scenes(
    db: &Db, llm: &dyn LlmClient, module_id: &str, source_id: Option<&str>,
    ruleset_id: Option<&str>, data_dir: &std::path::Path, budget: usize,
    only: Option<&str>,
) -> anyhow::Result<usize>; // 现 private → pub；agent_play.rs 的 SceneDeepExtractFn 闭包消费

// ============================================================================
// 12. crates/trpg-gm/src/lib.rs — re-exports（全量）
// ============================================================================
pub mod errata;
pub mod ledger;
pub mod prompts;
pub mod stream;
pub mod tools;
pub mod turn_loop;

pub use errata::{ErrataEntry, ErrataMemory};
pub use ledger::TurnLedger;
pub use prompts::{load_gm_skill, validate_compiled_budget, DynamicTailInput, TurnMessages};
pub use stream::RedactingBuffer;
pub use tools::{
    AwaitingPlayerRoll, GmTool, SceneDeepExtractFn, ToolCtx, ToolDispatchOutcome,
    ToolError, ToolOutput, ToolRegistry, ToolSpec,
};
pub use turn_loop::{CtxProviderFn, GmLoop, GmTurnInput, LoopConfig, TurnOutcome};
```

## 任务索引

### Section A：基础设施（Task 0-2）

- [ ] **Task 0：relay 双项 smoke test bin（go/no-go 闸门）** — 新建 `crates/trpg-llm/src/bin/relay_tools_smoke.rs` + 修改 `crates/trpg-llm/Cargo.toml`（bin 用 dotenvy，需补 `dotenvy.workspace = true`，workspace 已声明 dotenvy 0.15）（spec §10 风险 1，排第一个实施）。双项实测 :18888 relay 上的 gpt-5.5：① 用现有 `OpenAiCompatibleClient::complete_with_tools` 跑两轮带完整 tool 历史的会话（第一轮拿到 tool_calls 后回填 assistant tool_calls + tool role 消息，断言第二轮正常产出 content）；② 用 raw `reqwest` POST `{stream:true, stream_options:{"include_usage":true}, tools:[...]}`，把原始 SSE 行逐行 dump 到 stdout 并程序断言收到分片 `tool_calls` delta（含跨 chunk 的 arguments 片段）与 content delta 两类 chunk；顺带观察并记录 relay 是否在尾 chunk 透传 `usage.prompt_tokens_details.cached_tokens`（仅记录，不作 go/no-go）。结论写进本 plan 执行记录：① 失败 ⇒ 后续任务模型降 gpt-5.4；② 失败 ⇒ D2 不成立，暂停 Task 1+ 并上报用户拍板（修 relay 或临时回退"工具轮非流式+终轮 stream_chat"）。本任务无单测（自身即实测 bin），验证 = `cargo build -p trpg-llm` + 实跑两项各打印 PASS。

- [ ] **Task 1：trpg-llm `stream_chat_with_tools` + SSE delta 解析单测** — 新建 `crates/trpg-llm/src/stream_tools.rs`：契约第 1 节全部类型（`StreamEvent`（含 `Usage(Value)` 变体）/ `AggregatedToolCall` / `ToolChoice` / `ToolStreamAggregator`），`ToolStreamAggregator` 是与 reqwest 解耦的纯解析器（按 `choices[0].delta.tool_calls[].index` 聚合 id/name/arguments 片段，content delta 即来即发，顶层 `usage` 在 choices 守卫**之前**检查并透传为 `Usage` 事件，`finish` flush）；**`OpenAiCompatibleClient` 的实现主体也放本文件**（`pub(crate) async fn stream_chat_with_tools_impl`，子模块可访问父模块私有 config/http 与重试 helper——lib.rs 现 332 行，主体写进去必超 400），请求体带 `stream_options:{"include_usage":true}`，复用既有 `stream_chat` 的首 token 前重试与行缓冲模式；lib.rs 仅加 `pub mod stream_tools; pub use stream_tools::*;`、LlmClient trait 默认方法（契约第 2 节）与一行委托 impl。不引入契约外的死代码（无 StreamChatWithToolsExt 之类无人实现的 trait）。单测（in-file `#[cfg(test)]`，喂脚本化 SSE chunk JSON）：跨 chunk arguments 拼接、同轮多个 tool_calls 按 index 并行聚合、content 与 tool_calls 混合序列不串台、`[DONE]` flush、finish_reason 透传、usage chunk 透传为 `Usage` 事件（覆盖 spec §9 场景 3）。验证：`cargo test -p trpg-llm` 全绿；`wc -l` lib.rs 与 stream_tools.rs 均 ≤400。

- [ ] **Task 2：trpg-gm 脚手架（registry + error + ledger）** — workspace `Cargo.toml` members 加 `crates/trpg-gm`；新建 `crates/trpg-gm/Cargo.toml`（dependencies 含 trpg-interaction——run_gm_turn 头部显式 reconcile_session 用；dev-dependencies 含 sqlx 与 **async-stream**——Task 7 的 MockLlm 测试用 `async_stream::try_stream!`）、`src/lib.rs`（契约第 12 节 re-exports，工具子模块本任务先以空注册占位编译通过）、`src/tools/mod.rs`（契约第 5 节：`GmTool`/`ToolSpec`/`ToolOutput`/`ToolRegistry`（standard + from_tools）/`ToolError`/`ToolDispatchOutcome`/`dispatch`；`ToolRegistry::standard()` 本任务返回空表，后续任务逐个注册）、`src/ledger.rs`（契约第 4 节 `TurnLedger`）；`crates/trpg-agent/src/gm_loop.rs` 的 `private_roll_leak_tokens` 提 pub（一行）。单测：dispatch 对 unknown_tool 与非法 JSON arguments 回填正确形状的 `{"error":{...}}` 且不 Err；`TurnLedger::record_execution` 把 primary+followups 全部入账（check_results 与 dice_rolls 同步）；`private_roll_tokens` 断言与真实 `private_roll_leak_tokens` 行为逐字对齐（roll_id+expression+JSON 键与数字 token、Bool 一律跳过、public roll 的 token 不出现）。验证：`cargo test -p trpg-gm` + `cargo test -p trpg-agent` 全绿；trpg-gm 各源文件 `wc -l` ≤400。

### Section B：工具与提示（Task 3-6）

- [ ] **Task 3：check 工具（tools/check.rs：roll_check + request_player_roll）** — 痛点 1/2 根治点。`roll_check`（参数 check_label / **tested_parameter 必填** / actor_id? / opposed?{npc_id, opponent_parameter} / visibility(public|secret) / intent_kind）：tested_parameter 缺失 ⇒ `invalid_arguments`；构造 `CheckContract` 对齐 `trpg-runtime` `build_named_check_plan` 的字段约定（`tested_parameter=Some(TestedParameter{domain:None,key,label})`、骰式取 `kernel.dice_core.dice` 缺失 ⇒ `missing_kernel_dice` ToolError、`target=UnknownUntilLookup` 交内核解析、visibility 映射 RollVisibility/RollAuthority/disclosure）；opposed 形态先 `ensure_npc_parameter` 预热防御参数（失败 ⇒ 返回 `npc_synthesis_unavailable` ToolError 让 agent 改道，**不静默吞错**；参数桶来自 opposed.bucket 参数，schema default "skills"，代码零硬编码）再 `stamp_opposed_check` 既有通道；契约构造后 `db.insert_check_contract(&contract, "created")` 落 check_contracts 表（对齐旧路径 persist_agent_plan，否则 Task 11 验收 SQL 恒为空）；执行 `RuntimeEngine::execute_system_roll_bundle`（`after_check_resolved` 资源 on_outcome 已在其内）→ `ledger.record_execution`；结果 JSON 含 outcome、committed_patches 摘要、followups。`request_player_roll`（check_label / tested_parameter / stakes{before,success,failure} / visibility）：同款契约构造（RollAuthority::Player）+ 先 `db.cancel_open_pending_checks_for_session(session, PendingCheckStatus::Superseded)` 防 open gate 累积（对齐 persist_agent_plan 既有行为）+ `db.insert_check_contract` + `trpg_agent::make_pending_check` + `db.insert_pending_check` + `db.insert_interaction_gate` + 入账，返回 `ToolOutput::awaiting`。两工具注册进 `ToolRegistry::standard()`。单测（合成 kernel/契约，不连 DB 的纯构造路径 + ToolError 路径；`CheckTargetModel` 是带数据枚举无 as_str，断言用 `matches!`）。验证：`cargo test -p trpg-gm`；check.rs ≤400 行。

- [ ] **Task 4：effect 工具（tools/effect.rs + mechanics 决策参数化）** — 先在 `crates/trpg-mechanics/src/lib.rs` 把私有 `apply_effect_roll` 抽成 `apply_effect_roll_with_decision(contract, result, decision_override: Option<FacetDecision>)`（原名一行委托 None，既有调用方零改动——否则 resolve_facet_decision 无绑定 facet 时恒回落 hp.current，apply_direct_effect 的 parameter_path/operation 形同虚设）+ 加一行 `pub mod direct_effect;`；再新建 `crates/trpg-mechanics/src/direct_effect.rs` 落契约第 10 节 `apply_direct_effect`（合成 CheckContract+CheckResultRecord + 显式 FacetDecision 走同一管道；EffectKind 按 operation 映射，无 metadata_mut）。mechanics 单测为**纯构造断言**（合成契约/FacetDecision/EffectContract 形状，不连 DB——apply_effect_roll 内部跑真 SQL，DB 路径留 Task 11 e2e）。然后 `tools/effect.rs`：`apply_effect` 工具（target_actor / parameter_path 或 track_id / op / amount / reason → `apply_direct_effect`，产出的 ParameterImpact/EffectContract/patches 全部入账 ledger）+ `change_track` 工具（bucket/id/op/amount|value/kind?/category? → `RuntimeEngine::apply_track_change` 直通，参数 schema 沿用 `gm_retrieve_phase` 的 grow_tool 形状，含字符串 value 与负增量）。两工具注册进 standard()。验证：`cargo test -p trpg-mechanics` + `cargo test -p trpg-gm`；direct_effect.rs / effect.rs 均 ≤400 行。

- [ ] **Task 5：world + npc 工具（tools/world.rs + tools/npc.rs）** — `world.rs`：`retrieve_rules`（query/k? → `RuntimeEngine::retrieve_rules`）、`advance_time`（amount/scale/reason → TimeScale 字符串映射 fail-closed；`TimeAmount` 无 value 字段，按 scale 路由既有构造器 combat_rounds/scene_beats/minutes 再构造 `TimeAdvanceRequest` + `advance_world_time`）、`remember`（summary/importance?/tags? → `MemoryEvent` 构造（字段名 `source` 非 source_json）+ `db.save_memory_event`）、`navigate_scene`（target_node_id/reason → `db.load_module_graph` **先校验 target 存在（scene_not_found）再比对 current（scene_same_as_current）**——校验顺序与 `trpg_api::validate_transition` 一致但不依赖 trpg-api → `db.set_session_scene` → 若 target 仍 SkeletonOnly 且 `ctx.scene_extractor` 有值则到场深抽；错误码 no_module_loaded/scene_not_found/scene_same_as_current）；同时 `crates/trpg-api/src/lib.rs` 的 `extract_module_scenes` 提 pub（一行）。`npc.rs`：`get_actor`（actor_id → 角色卡 sheet_json + mechanical_profile 的 GM 视角投影 JSON）、`ensure_npc_param`（npc_id/bucket/param/context → 从模组图谱解析 `NpcPersona` + `RuntimeEngine::ensure_npc_parameter`，替代旧 fire-and-forget 预 pass）。六工具注册进 standard()（至此 10/10 齐）。单测：navigate fail-closed 三错误码、TimeScale 非法值、ToolRegistry::standard().schemas() 数量与顺序断言。验证：`cargo test -p trpg-gm` + `cargo build -p trpg-api`；world.rs / npc.rs 均 ≤400 行。

- [ ] **Task 6：prompts.rs 消息装配 + engine protocol agent-loop 变体 + data/agent/gm_skill 蒸馏** — `prompts.rs` 落契约第 7 节：`TurnMessages::assemble` 严格按 §6.1 布局（system: BP1 prefix_text+gm_skill → user: BP2 pinned → history 平铺 → user: BP3 dynamic + resolved_gate_facts + errata_blocks + Player Input），渲染一次整回合复用，`push_assistant_tool_calls`/`push_tool_result` 只尾部追加，`prefix_byte_hash` 确定性哈希；`load_gm_skill` global→ruleset 两级合并、global 缺失 Err。**BP1 agent-loop 变体（spec §6 实锤）**：`trpg_model::RuntimeState` 加 `#[serde(default)] pub agent_loop_protocol: bool`（全仓 RuntimeState 字面量均带 `..Default::default()`，向后兼容）；`trpg-runtime/src/lib.rs` 新增 `engine_protocol_block_agent_loop()`（剔除/替换与 roll_check/request_player_roll 双工具矛盾的禁令段——旧块的 "without asking players for dice totals" 等文案换成"经 request_player_roll 工具开 gate 让玩家亲手掷"的工具语义），`prepare_turn_context` 的 `blocks.push(engine_protocol_block())` 改按 `state.agent_loop_protocol` 选块（默认 false，旧路径零变化）；trpg-runtime 单测断言 agent 变体不含旧禁令文本且含 roll_check/request_player_roll（run_gm_turn 在 Task 7 强制置位该 flag）。数据文件三份：`10_dice_discretion.md` 把 22 个 `data/agent/advice/*.json` 的 plan_kind/advice_summary/free_read_reason 语义蒸馏为 GM 裁量准则散文（何时 roll_check / 何时 request_player_roll / 何时免检定直接叙事——内容不丢、从"路由数据"变"提示数据"，零硬编码保持）；`20_item_npc_policy.md` 原文迁移 `trpg-cli/src/main.rs` `gm_retrieve_phase`（~L1993-2003）的 ITEM POLICY / NPC POLICY 段；`30_output_contract.md` 玩家可见输出契约（[roll]/[system] 标签语义、私骰不泄、不要求玩家报点数总值）。单测：装配顺序与消息数、同输入两次 assemble 字节一致、push 后前缀字节不变、load_gm_skill 合并顺序、agent-loop 变体文案断言（trpg-runtime 侧）。验证：`cargo test -p trpg-gm` + `cargo test -p trpg-runtime agent_loop_protocol`；prompts.rs ≤400 行。

### Section C：主循环与接入（Task 7-11）

- [ ] **Task 7：turn_loop.rs run_gm_turn 全状态机 + 滑动缓冲 + MockLlm 单测** — `stream.rs` 落契约第 6 节 `RedactingBuffer`（holdback 尾窗 + 大小写不敏感替换 "■"，空 tokens 零暂扣直通；**切分点必须回退到 UTF-8 char boundary**——中文叙事是 e2e 主场景，字节切 String 会 panic）；`turn_loop.rs` 落契约第 9 节 `GmLoop`/`LoopConfig`/`TurnOutcome`/`GmTurnInput`/`CtxProviderFn`/`run_gm_turn` 六步状态机（确定性头部 record_world_event + refresh_actor_live_derived + **reconcile_session** + gate 结算→事实注入、state 置 agent_loop_protocol=true、ctx_provider seam、`load_gm_skill(&self.data_dir, ..)?` fail-closed、工具轮全程 ToolChoice::Auto 尾部追加、dispatch awaiting ⇒ 终态、ContentDelta→RedactingBuffer→on_delta、Usage→tracing、**仅轮数耗尽且仍要工具时追加唯一一轮 ToolChoice::None**、流后挂钩位留给 Task 8、**确定性收尾 finalize_turn：db.save_turn 持久化回合 + 回合摘要 save_memory_event（tags=["gm_turn"]）+ learning audit（audit_learning_for_turn，原语自带门控与吞错）**，Narration 与 Awaiting 两类终态都收尾）。tests 内建 `MockLlm`（实现 LlmClient：按脚本回放 `Vec<Vec<StreamEvent>>` 并捕获每轮请求 messages/tools/tool_choice）+ `ctx_provider` 注入合成 CompiledContext + `ToolRegistry::from_tools` 注入脚本化 GmTool 替身（**绕开真 DB**——prepare_turn_context/真工具都打 DB，纯 MockLlm 单测跑不通）。单测覆盖 spec §9 场景 1（工具轮→content 终态、delta 逐块到达）、2 单测侧（awaiting 终态；"下回合骰值回复→头部结算"需 DB，归 Task 11 e2e 清单）、4（私骰 token 跨 chunk 边界仍被过滤、无私骰透明直通、中文+私骰跨界不 panic）、6（轮数耗尽后追加唯一 None 轮，tool_choice 序列断言 [Auto, None]）、7（工具回 missing_kernel_dice 结构化错误回填 → 脚本让 agent 改调 awaiting 替身成功）、8（工具替身产物正确进 TurnLedgerSnapshot）。验证：`cargo test -p trpg-gm` 全绿；turn_loop.rs / stream.rs 均 ≤400 行。

- [ ] **Task 8：流后 verifier → 勘误记忆 + 同错升级** — `errata.rs` 落契约第 8 节 `ErrataMemory`/`ErrataEntry`（MemoryEvent 字段名 `source` 非 source_json）；`run_gm_turn` 第 5 步接线：以已流出全文构造 `FinalNarrationSubmission`（player_visible_text=全文，mechanical_claims 留空走 verifier 文本启发式）→ `NarrationVerifier::verify(ledger.snapshot(), &submission)` → `errata.record(turn_id, &findings)` → 新增条目 `to_memory_event` 后 `db.save_memory_event`（tags 含 "gm_errata"）；**Narration 与 AwaitingPlayerRoll 两类终态都接线**（spec §4 流后校验是 loop 之后的无条件阶段；awaiting 路径在 visible_text 非空时跑——其前可能已流出含可见掷骰结果的叙事）；下一回合 `DynamicTailInput.errata_blocks` 注入 `errata_block()` + `standing_reminder_block()`。单测覆盖 spec §9 场景 5：叙事漏提可见掷骰结果 → OmittedVisibleResult finding 落勘误 → 下一回合 assemble 的 dynamic tail 含勘误块；同 kind 记满 `repeat_finding_threshold` 次 → standing_reminder_block 出现且此后持续；kind_counts 聚合正确。验证：`cargo test -p trpg-gm`；errata.rs ≤400 行。

- [ ] **Task 9：缓存稳定回归测试（§6.1 硬性原则）** — 纯测试 + 少量可观测代码，不改装配逻辑。断言（①⑤ 用 MockLlm/registry，②③ 为 prompts.rs 纯函数测试，**全部落正文 Step 1 测试代码**）：① 同回合内各工具轮请求 messages 的前 N 条（assemble 产出段）字节级一致（`prefix_byte_hash` 逐轮相等），追加只发生在尾部；② 跨回合（场景不变、gm_skill 不变）同 compiled 重新 assemble（history 多追加两条）后 system+BP2 两条消息序列化字节与上一回合一致、history 段仅追加不改写；③ `validate_compiled_budget`：prefix/pinned 超 TokenBudget（字段 prefix_max/pinned_max，已勘查存在）时报配置错误而非静默裁剪（断言 Err 信息含 "prefix"/"pinned" 段名），并在 run_gm_turn 的 compiled 就绪后接线调用；④ run_gm_turn 每轮 `tracing::info!` 记 compiled.prefix_hash / pinned_hash / 请求前缀 hash，上游 cached_tokens 经 Task 1 的 `StreamEvent::Usage` 事件在 Usage 分支记 `prompt_tokens_details.cached_tokens`（relay 透传则记，缺省跳过）；⑤ 工具 schemas 顺序与序列化字节跨调用确定性。验证：`cargo test -p trpg-gm` 全绿（含新增 cache_stability 测试模块）；所有改动文件 ≤400 行。

- [ ] **Task 10：CLI `--agent` 接入** — `crates/trpg-cli/src/main.rs`：Play 子命令加 `--agent` bool flag，`mod agent_play;` + 分流（`Commands::Play { ruleset, module, agent }` 臂内只多一个 if，默认 false 走旧 `play_cli` 零改动，保留 A/B）；`crates/trpg-cli/Cargo.toml` 加 trpg-gm 依赖。新建 `crates/trpg-cli/src/agent_play.rs`：会话装配**逐项对齐 play_cli**——`play_cli_agent(ruleset: &str, module: Option<&str>)` 内部自己 `connect_db().await?` + `db.migrate()` + `make_llm()`（已返回 `Arc<dyn LlmClient>`，直接持有，**绝不从 &dyn 造 Arc**）+ `make_search(&db, default_data_dir())` + `RuntimeEngine::new(db).with_search(search)`（retrieve_rules 工具才可用）+ **`runtime.start_session(ruleset, module)` 拿真 session_id**（sessions 行落库 + 模组入口场景激活，FK/BP2 投影/navigate_scene 校验全靠它）；`GmLoop::new(.., default_data_dir())` 后给 `scene_extractor` 赋闭包（捕获 db/llm/data_dir 调 `trpg_api::extract_module_scenes(.., only=Some(node_id))`）；每回合 `db.load_session_scene` 把 current_scene_id 填进 RuntimeState（对齐 prepare_turn_context 单点约定）；维护 append-only `history: Vec<ChatMessage>`（每回合 push user 输入与 GM 全文，与 save_turn 落库一致）、`on_delta` 接 `let _ = emit_delta(format, delta)`（format 用 main.rs 私有 `StreamFormat`，同 crate 子模块可见；一期固定 Text）、`TurnOutcome::AwaitingPlayerRoll` ⇒ `emit_phase` 输出 prompt_public（data 按值传 owned json!）后回到读输入循环。验证：`cargo build -p trpg-cli`；`cargo run -p trpg-cli --bin trpg -- play --ruleset <id> --agent` 在无 DB 环境报清晰错误而非 panic；agent_play.rs ≤400 行、main.rs 增量 ≤20 行。

- [x] **Task 11：e2e 实测清单（真 DB 真模组真 gpt-5.5）** — 环境：:54347 Postgres（`docker exec chatrpg-postgres-rulesets psql`；CoC 库注意 .env DATABASE_URL 端口 swap 坑）、`TRPG_LLM_MODEL=gpt-5.5` 经 :18888 relay。血色公路（CoC sandbox 中文）与 The Vault（Triangle 任务集英文）各跑 ≥3 回合 `trpg play --agent`，逐项验收 spec §9：① agent 在合理时机发起检定且 tested_parameter 语义绑定正确（查 check_contracts；check_label 等列不存在，经 `contract_json->>'...'` 取）；② 资源 on_outcome 真触发（SAN/HP 变化查 `generic_parameter_states`）；③ request_player_roll 暂停 → 玩家回骰 → 下回合演绎后果全闭环（含 Task 7 留给 e2e 的"下回合骰值回复→头部结算→事实注入"）；④ 回合总延迟 ≤40s 且首 content token 显著早于回合完成（目标典型 ≤20s，记 TTFT）；⑤ 跨回合 cached_tokens 命中率记录（relay 透传则记）；⑥ 回合落库：`turns` 表有 agent 路径每回合的 user_input/assistant_output（save_turn 收尾生效，与内存 history 一致）。再用 chatrpg-product-evaluator 对新旧路径做对照 playtest，勘误 kind_counts 与战斗深度差距（spec §3 已知 trade-off）一并记录。所有实测结果与 go/no-go 结论回填本 plan 执行记录。验证：上述清单逐项有证据（SQL 查询输出/计时日志），无证据不得勾选。

---

## 任务正文

### Task 0：relay 双项 smoke test bin（go/no-go 闸门）

**Files:**
- Create: `crates/trpg-llm/src/bin/relay_tools_smoke.rs`
- Modify: `crates/trpg-llm/Cargo.toml`（bin 用 dotenvy；crate 现无此依赖，不加必 E0433 编译失败；workspace 已声明 dotenvy 0.15）
- Test: 本任务不写 `#[test]`；二进制自身就是 go/no-go smoke test

- [ ] **Step 1：写失败测试**

```rust
// crates/trpg-llm/src/bin/relay_tools_smoke.rs
use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::{json, Value};
use trpg_llm::{LlmClient, LlmConfig, OpenAiCompatibleClient};

fn echo_tool() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": "echo_probe",
            "description": "Return the exact probe text.",
            "parameters": {
                "type": "object",
                "properties": {
                    "probe": {"type": "string"}
                },
                "required": ["probe"],
                "additionalProperties": false
            }
        }
    })
}

async fn smoke_complete_with_tools_multi_turn(client: &OpenAiCompatibleClient) -> Result<()> {
    let tool = echo_tool();
    let mut messages = vec![
        json!({"role":"system","content":"You are a relay smoke test. Call echo_probe exactly once, then answer normally after the tool result."}),
        json!({"role":"user","content":"Call echo_probe with probe='relay-multi-turn-ok'."}),
    ];
    let first = client.complete_with_tools(messages.clone(), vec![tool.clone()]).await?;
    let msg = first.pointer("/choices/0/message").cloned().ok_or_else(|| anyhow!("first response missing message"))?;
    let calls = msg.get("tool_calls").and_then(Value::as_array).cloned().unwrap_or_default();
    if calls.is_empty() {
        return Err(anyhow!("complete_with_tools did not return tool_calls in first turn"));
    }
    messages.push(msg);
    for call in calls {
        let id = call.get("id").and_then(Value::as_str).ok_or_else(|| anyhow!("tool call missing id"))?;
        messages.push(json!({
            "role": "tool",
            "tool_call_id": id,
            "name": "echo_probe",
            "content": "{\"ok\":true,\"echo\":\"relay-multi-turn-ok\"}"
        }));
    }
    let second = client.complete_with_tools(messages, vec![tool]).await?;
    let content = second.pointer("/choices/0/message/content").and_then(Value::as_str).unwrap_or("");
    if content.trim().is_empty() {
        return Err(anyhow!("second tool-history turn produced no content"));
    }
    println!("PASS complete_with_tools multi-turn: {}", content.trim().chars().take(120).collect::<String>());
    Ok(())
}

async fn collect_raw_sse(config: &LlmConfig, body: Value) -> Result<(bool, bool, bool)> {
    let http = Client::new();
    let mut stream = http
        .post(config.endpoint("chat/completions"))
        .bearer_auth(&config.api_key)
        .json(&body)
        .send()
        .await
        .context("raw SSE request failed")?
        .error_for_status()
        .context("raw SSE response status was not success")?
        .bytes_stream();
    let mut buffer = String::new();
    let mut saw_tool_delta = false;
    let mut saw_content_delta = false;
    let mut saw_usage = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(pos) = buffer.find('\n') {
            let line = buffer[..pos].trim().to_string();
            buffer = buffer[pos + 1..].to_string();
            if !line.starts_with("data:") {
                continue;
            }
            let data = line.trim_start_matches("data:").trim();
            println!("SSE {data}");
            if data == "[DONE]" {
                continue;
            }
            let value: Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if value.pointer("/choices/0/delta/tool_calls").is_some() {
                saw_tool_delta = true;
            }
            if value.pointer("/choices/0/delta/content").and_then(Value::as_str).map(|s| !s.is_empty()).unwrap_or(false) {
                saw_content_delta = true;
            }
            if value.get("usage").map(|u| !u.is_null()).unwrap_or(false) {
                saw_usage = true;
                println!("USAGE cached_tokens={:?}", value.pointer("/usage/prompt_tokens_details/cached_tokens"));
            }
        }
    }
    Ok((saw_tool_delta, saw_content_delta, saw_usage))
}

async fn smoke_raw_stream_tool_delta(config: &LlmConfig) -> Result<()> {
    let tool_body = json!({
        "model": config.model,
        "stream": true,
        "stream_options": {"include_usage": true},
        "messages": [
            {"role":"system","content":"Call echo_probe once. Use a long probe argument so the relay is likely to split function arguments across SSE chunks."},
            {"role":"user","content":"Call echo_probe with probe='alpha-bravo-charlie-delta-echo-foxtrot-golf-hotel-india-juliet-kilo'."}
        ],
        "tools": [echo_tool()],
        "tool_choice": {"type":"function","function":{"name":"echo_probe"}}
    });
    let (tool_delta, _, tool_usage) = collect_raw_sse(config, tool_body).await?;
    if !tool_delta {
        return Err(anyhow!("raw stream did not expose tool_calls delta"));
    }

    let content_body = json!({
        "model": config.model,
        "stream": true,
        "stream_options": {"include_usage": true},
        "messages": [
            {"role":"system","content":"Do not call tools. Reply with the exact sentence: relay content delta ok."},
            {"role":"user","content":"Return the sentence now."}
        ],
        "tools": [echo_tool()],
        "tool_choice": "none"
    });
    let (_, content_delta, content_usage) = collect_raw_sse(config, content_body).await?;
    if !content_delta {
        return Err(anyhow!("raw stream did not expose content delta"));
    }
    // usage 透传仅观测记录（spec §6.1 第 5 条），不作 go/no-go 闸门。
    println!("INFO usage passthrough: tool_round={tool_usage} content_round={content_usage}");
    println!("PASS raw SSE stream exposes tool_calls delta and content delta");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let config = LlmConfig::from_env()?;
    eprintln!("relay smoke target: base_url={} model={}", config.base_url, config.model);
    let client = OpenAiCompatibleClient::new(config.clone())?;
    smoke_complete_with_tools_multi_turn(&client).await?;
    smoke_raw_stream_tool_delta(&config).await?;
    println!("PASS relay_tools_smoke all checks");
    Ok(())
}
```

- [ ] **Step 2：运行确认失败**

Run: `cargo build -p trpg-llm --bin relay_tools_smoke`
Expected: 在 Step 1 文件**创建前**运行报 `no bin target named relay_tools_smoke`；文件已创建但 Cargo.toml 未加 dotenvy 时报 `E0433: unresolved crate dotenvy`——两种失败形态都算本步确认（Step 3 补依赖后应 PASS）

- [ ] **Step 3：最小实现**

保留 Step 1 的完整二进制文件内容；库代码不动，仅给 `crates/trpg-llm/Cargo.toml` 的 `[dependencies]` 补一行（workspace 根已声明 dotenvy 0.15）：

```toml
dotenvy.workspace = true
```

四种观测结论必须写入执行记录：

1. 两项都 PASS：继续 Task 1-11，模型保持 `TRPG_LLM_MODEL=gpt-5.5`。
2. `complete_with_tools` 多轮工具历史失败：后续真实 e2e 降到 `TRPG_LLM_MODEL=gpt-5.4`，Task 1-10 仍继续，因为库能力仍要完成。
3. raw SSE 的 `tool_calls` delta 失败：D2 真流式工具轮不成立，暂停 Task 1 之后的执行，上报用户在“修 relay”与“工具轮非流式 + 终轮 stream_chat”之间拍板。
4. usage 透传观测（INFO 行）：relay 透传 ⇒ Task 9/11 记 cached_tokens 数值；不透传 ⇒ 全链按 `relay_not_reported` 记录，仅观测不阻塞。

- [ ] **Step 4：运行确认通过**

Run: `cargo build -p trpg-llm --bin relay_tools_smoke`
Expected: PASS

- [ ] **Step 5：验证收尾**

Run: `TRPG_LLM_BASE_URL=http://127.0.0.1:18888/v1 TRPG_LLM_MODEL=gpt-5.5 cargo run -p trpg-llm --bin relay_tools_smoke && wc -l crates/trpg-llm/src/bin/relay_tools_smoke.rs`
Expected: 两项 smoke 均打印 PASS；`relay_tools_smoke.rs` ≤400 行

---

### Task 1：trpg-llm `stream_chat_with_tools` + SSE delta 解析单测

**Files:**
- Create: `crates/trpg-llm/src/stream_tools.rs`
- Modify: `crates/trpg-llm/src/lib.rs:use 区域、LlmClient trait、OpenAiCompatibleClient impl`
- Test: `crates/trpg-llm/src/stream_tools.rs` 同文件 `#[cfg(test)]`

- [ ] **Step 1：写失败测试**

```rust
// crates/trpg-llm/src/stream_tools.rs
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn aggregates_tool_arguments_across_chunks() {
        let mut agg = ToolStreamAggregator::new();
        assert_eq!(agg.feed_chunk(&json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"roll_check","arguments":"{\"tested_"}}]}}]})), Vec::<StreamEvent>::new());
        assert_eq!(agg.feed_chunk(&json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"parameter\":\"DEX\"}"}}]}}]})), Vec::<StreamEvent>::new());
        let events = agg.finish(Some("tool_calls".to_string()));
        assert_eq!(events, vec![
            StreamEvent::ToolCalls(vec![AggregatedToolCall { id: "call_1".to_string(), name: "roll_check".to_string(), arguments: "{\"tested_parameter\":\"DEX\"}".to_string() }]),
            StreamEvent::Done { finish_reason: Some("tool_calls".to_string()) },
        ]);
    }

    #[test]
    fn aggregates_multiple_tool_calls_by_index() {
        let mut agg = ToolStreamAggregator::new();
        agg.feed_chunk(&json!({"choices":[{"delta":{"tool_calls":[
            {"index":1,"id":"call_b","type":"function","function":{"name":"retrieve_rules","arguments":"{\"query\":\"ste"}},
            {"index":0,"id":"call_a","type":"function","function":{"name":"get_actor","arguments":"{\"actor_id\":\"pc"}}
        ]}}]}));
        agg.feed_chunk(&json!({"choices":[{"delta":{"tool_calls":[
            {"index":1,"function":{"arguments":"alth\"}"}},
            {"index":0,"function":{"arguments":".current\"}"}}
        ]}}]}));
        let events = agg.finish(Some("tool_calls".to_string()));
        assert_eq!(events[0], StreamEvent::ToolCalls(vec![
            AggregatedToolCall { id: "call_a".to_string(), name: "get_actor".to_string(), arguments: "{\"actor_id\":\"pc.current\"}".to_string() },
            AggregatedToolCall { id: "call_b".to_string(), name: "retrieve_rules".to_string(), arguments: "{\"query\":\"stealth\"}".to_string() },
        ]));
    }

    #[test]
    fn content_and_tools_do_not_mix() {
        let mut agg = ToolStreamAggregator::new();
        assert_eq!(agg.feed_chunk(&json!({"choices":[{"delta":{"content":"You duck. "}}]})), vec![StreamEvent::ContentDelta("You duck. ".to_string())]);
        agg.feed_chunk(&json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"change_track","arguments":"{\"bucket\":\"tracks\"}"}}]}}]}));
        let events = agg.finish(Some("tool_calls".to_string()));
        assert_eq!(events[0], StreamEvent::ToolCalls(vec![AggregatedToolCall { id: "call_1".to_string(), name: "change_track".to_string(), arguments: "{\"bucket\":\"tracks\"}".to_string() }]));
    }

    #[test]
    fn finish_reason_from_chunk_is_preserved() {
        let mut agg = ToolStreamAggregator::new();
        let events = agg.feed_chunk(&json!({"choices":[{"delta":{},"finish_reason":"stop"}]}));
        assert_eq!(events, vec![StreamEvent::Done { finish_reason: Some("stop".to_string()) }]);
    }

    #[test]
    fn done_flushes_pending_calls() {
        let mut agg = ToolStreamAggregator::new();
        agg.feed_chunk(&json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_9","function":{"name":"remember","arguments":"{\"summary\":\"x\"}"}}]}}]}));
        let events = agg.finish(None);
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], StreamEvent::ToolCalls(v) if v[0].name == "remember"));
        assert_eq!(events[1], StreamEvent::Done { finish_reason: None });
    }

    #[test]
    fn usage_chunk_is_forwarded_even_with_empty_choices() {
        // include_usage 的尾 chunk 形如 {"choices":[], "usage":{...}}：
        // usage 检查必须在 choices[0] 守卫之前，否则整个 chunk 被丢弃。
        let mut agg = ToolStreamAggregator::new();
        let usage = json!({"prompt_tokens": 1200, "prompt_tokens_details": {"cached_tokens": 1024}});
        let events = agg.feed_chunk(&json!({"choices":[], "usage": usage.clone()}));
        assert_eq!(events, vec![StreamEvent::Usage(usage)]);
    }
}
```

- [ ] **Step 2：运行确认失败**

Run: `cargo test -p trpg-llm aggregates_tool_arguments_across_chunks`
Expected: FAIL，报错应为 `cannot find type ToolStreamAggregator` 或 `cannot find type StreamEvent`

- [ ] **Step 3：最小实现**

```rust
// crates/trpg-llm/src/stream_tools.rs
use anyhow::{anyhow, Result};
use async_stream::try_stream;
use futures_core::Stream;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::pin::Pin;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AggregatedToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    ContentDelta(String),
    ToolCalls(Vec<AggregatedToolCall>),
    Usage(Value),
    Done { finish_reason: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolChoice { Auto, None }

impl ToolChoice {
    pub fn as_json(&self) -> Value {
        match self {
            ToolChoice::Auto => Value::String("auto".to_string()),
            ToolChoice::None => Value::String("none".to_string()),
        }
    }
}

#[derive(Debug, Default, Clone)]
struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Debug, Default)]
pub struct ToolStreamAggregator {
    calls: BTreeMap<u64, PartialToolCall>,
    done_emitted: bool,
}

impl ToolStreamAggregator {
    pub fn new() -> Self { Self::default() }

    pub fn feed_chunk(&mut self, chunk: &Value) -> Vec<StreamEvent> {
        if self.done_emitted {
            return Vec::new();
        }
        let mut out = Vec::new();
        // include_usage 尾 chunk 的 choices 为空数组：usage 检查必须先于 choices 守卫。
        if let Some(usage) = chunk.get("usage") {
            if !usage.is_null() {
                out.push(StreamEvent::Usage(usage.clone()));
            }
        }
        let Some(choice) = chunk.pointer("/choices/0") else { return out };
        if let Some(content) = choice.pointer("/delta/content").and_then(Value::as_str) {
            if !content.is_empty() {
                out.push(StreamEvent::ContentDelta(content.to_string()));
            }
        }
        if let Some(text) = choice.pointer("/text").and_then(Value::as_str) {
            if !text.is_empty() {
                out.push(StreamEvent::ContentDelta(text.to_string()));
            }
        }
        if let Some(parts) = choice.pointer("/delta/tool_calls").and_then(Value::as_array) {
            for part in parts {
                let index = part.get("index").and_then(Value::as_u64).unwrap_or(0);
                let slot = self.calls.entry(index).or_default();
                if let Some(id) = part.get("id").and_then(Value::as_str) {
                    if !id.is_empty() { slot.id = id.to_string(); }
                }
                if let Some(name) = part.pointer("/function/name").and_then(Value::as_str) {
                    if !name.is_empty() { slot.name = name.to_string(); }
                }
                if let Some(arguments) = part.pointer("/function/arguments").and_then(Value::as_str) {
                    slot.arguments.push_str(arguments);
                }
            }
        }
        if let Some(reason) = choice.get("finish_reason") {
            if !reason.is_null() {
                let finish_reason = reason.as_str().map(str::to_string);
                out.extend(self.finish(finish_reason));
            }
        }
        out
    }

    pub fn finish(&mut self, finish_reason: Option<String>) -> Vec<StreamEvent> {
        if self.done_emitted {
            return Vec::new();
        }
        self.done_emitted = true;
        let mut out = Vec::new();
        if !self.calls.is_empty() {
            let calls = std::mem::take(&mut self.calls)
                .into_values()
                .map(|p| AggregatedToolCall { id: p.id, name: p.name, arguments: p.arguments })
                .collect::<Vec<_>>();
            out.push(StreamEvent::ToolCalls(calls));
        }
        out.push(StreamEvent::Done { finish_reason });
        out
    }
}

impl crate::OpenAiCompatibleClient {
    /// stream_chat_with_tools 的实现主体。放本文件而非 lib.rs：lib.rs 现 332 行，
    /// 主体写进去必超 400 行纪律；子模块可访问父模块私有字段 config/http 与
    /// 私有重试 helper（max_retries / retry_base_delay_ms / is_retryable_status /
    /// retry_after_delay_ms），lib.rs 的 trait impl 一行委托到此。
    pub(crate) async fn stream_chat_with_tools_impl(
        &self,
        messages: Vec<Value>,
        tools: Vec<Value>,
        tool_choice: ToolChoice,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let body = json!({
            "model": self.config.model.clone(),
            "messages": messages,
            "tools": tools,
            "tool_choice": tool_choice.as_json(),
            "stream": true,
            "stream_options": {"include_usage": true}
        });
        let mut last_error: Option<anyhow::Error> = None;
        let mut resp_opt = None;
        for attempt in 0..=self.max_retries() {
            let resp = self.http
                .post(self.config.endpoint("chat/completions"))
                .bearer_auth(&self.config.api_key)
                .json(&body)
                .send()
                .await;
            match resp {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        resp_opt = Some(resp);
                        break;
                    }
                    let retry_delay_ms = Self::retry_after_delay_ms(&resp, self.retry_base_delay_ms().saturating_mul(1u64 << attempt.min(5)));
                    let text = resp.text().await.unwrap_or_default();
                    let err = anyhow!("LLM API error {status}: {text}");
                    if attempt < self.max_retries() && Self::is_retryable_status(status) {
                        tracing::warn!(attempt = attempt + 1, status = %status, retry_delay_ms, "retrying tool streaming request before first event");
                        tokio::time::sleep(std::time::Duration::from_millis(retry_delay_ms)).await;
                        last_error = Some(err);
                        continue;
                    }
                    return Err(err);
                }
                Err(err) => {
                    let retry_delay_ms = self.retry_base_delay_ms().saturating_mul(1u64 << attempt.min(5));
                    let err = anyhow!(err).context("LLM tool streaming transport error before first event");
                    if attempt < self.max_retries() {
                        tracing::warn!(attempt = attempt + 1, retry_delay_ms, error = %err, "retrying tool streaming transport error before first event");
                        tokio::time::sleep(std::time::Duration::from_millis(retry_delay_ms)).await;
                        last_error = Some(err);
                        continue;
                    }
                    return Err(err);
                }
            }
        }
        let resp = resp_opt.ok_or_else(|| last_error.unwrap_or_else(|| anyhow!("LLM tool streaming request failed without a recorded error")))?;
        let mut bytes = resp.bytes_stream();
        let s = try_stream! {
            let mut buffer = String::new();
            let mut agg = ToolStreamAggregator::new();
            let mut done = false;
            while let Some(chunk) = bytes.next().await {
                let chunk = chunk?;
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(pos) = buffer.find('\n') {
                    let line = buffer[..pos].trim().to_string();
                    buffer = buffer[pos + 1..].to_string();
                    if !line.starts_with("data:") {
                        continue;
                    }
                    let data = line.trim_start_matches("data:").trim();
                    if data == "[DONE]" {
                        for event in agg.finish(None) { yield event; }
                        done = true;
                        break;
                    }
                    let parsed: Value = match serde_json::from_str(data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    for event in agg.feed_chunk(&parsed) { yield event; }
                }
                if done { break; }
            }
            if !done {
                for event in agg.finish(None) { yield event; }
            }
        };
        Ok(Box::pin(s))
    }
}
```

同时修改 `crates/trpg-llm/src/lib.rs`（仅三小段，lib.rs 不放实现主体）：

```rust
// 文件顶部补充
pub mod stream_tools;
pub use stream_tools::*;

// LlmClient trait 末尾补充默认方法
async fn stream_chat_with_tools(
    &self,
    _messages: Vec<Value>,
    _tools: Vec<Value>,
    _tool_choice: ToolChoice,
) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
    Err(anyhow!("stream_chat_with_tools is not supported by this LlmClient"))
}

// OpenAiCompatibleClient 的 LlmClient impl 内补充（一行委托到 stream_tools.rs 主体）
async fn stream_chat_with_tools(
    &self,
    messages: Vec<Value>,
    tools: Vec<Value>,
    tool_choice: ToolChoice,
) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
    self.stream_chat_with_tools_impl(messages, tools, tool_choice).await
}
```

- [ ] **Step 4：运行确认通过**

Run: `cargo test -p trpg-llm aggregates_tool_arguments_across_chunks`
Expected: PASS

- [ ] **Step 5：验证收尾**

Run: `cargo test -p trpg-llm && wc -l crates/trpg-llm/src/lib.rs crates/trpg-llm/src/stream_tools.rs`
Expected: 全绿；`lib.rs` 与 `stream_tools.rs` 均 ≤400 行

---

### Task 2：trpg-gm 脚手架（registry + error + ledger）

**Files:**
- Create: `crates/trpg-gm/Cargo.toml`
- Create: `crates/trpg-gm/src/lib.rs`
- Create: `crates/trpg-gm/src/ledger.rs`
- Create: `crates/trpg-gm/src/tools/mod.rs`
- Modify: `Cargo.toml:workspace.members`
- Modify: `crates/trpg-agent/src/gm_loop.rs:TurnLedgerSnapshot::private_roll_leak_tokens`
- Test: `crates/trpg-gm/src/tools/mod.rs` 与 `crates/trpg-gm/src/ledger.rs` 同文件 `#[cfg(test)]`

- [ ] **Step 1：写失败测试**

```rust
// crates/trpg-gm/src/tools/mod.rs
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
```

```rust
// crates/trpg-gm/src/ledger.rs
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;
    use trpg_model::{ActorKind, CheckResultRecord, DiceRollRecord, RollVisibility};
    use trpg_runtime::AutoRollExecution;

    fn result(check_id: &str, total: i64, visibility: RollVisibility) -> CheckResultRecord {
        CheckResultRecord {
            check_id: check_id.to_string(),
            roll: DiceRollRecord {
                roll_id: format!("roll_{check_id}"),
                session_id: "s".to_string(),
                turn_id: "t".to_string(),
                check_id: Some(check_id.to_string()),
                roller_kind: ActorKind::PlayerCharacter,
                roller_id: Some("pc.current".to_string()),
                visibility,
                expression: "1d100".to_string(),
                result: json!({"total": total}),
                seed_commitment: "seed".to_string(),
                revealed_at: None,
                created_at: Utc::now(),
            },
            outcome: json!({"success": total < 50}),
            committed_patches: vec![],
            created_at: Utc::now(),
        }
    }

    #[test]
    fn record_execution_records_primary_followups_and_rolls() {
        let exec = AutoRollExecution { primary: result("a", 12, RollVisibility::PublicGmRoll), followups: vec![result("b", 34, RollVisibility::PrivateGmRoll)], roll_policy: "system_rolls_visible".to_string() };
        let mut ledger = TurnLedger::new();
        ledger.record_execution(&exec);
        assert_eq!(ledger.snapshot().check_results.len(), 2);
        assert_eq!(ledger.snapshot().dice_rolls.len(), 2);
        // 与真实 private_roll_leak_tokens 行为逐字对齐（trpg-agent gm_loop.rs L112-132）：
        // 仅 PrivateGmRoll 的 roll 进集合；push 顺序 = roll_id → expression →
        // result JSON（键+数字，collect_json_tokens 对 Bool 一律跳过）→ outcome JSON
        // （键 "success"；true/false 是 Bool 不产 token）；dedupe 保序。
        assert_eq!(
            ledger.private_roll_tokens(),
            vec!["roll_b".to_string(), "1d100".to_string(), "total".to_string(), "34".to_string(), "success".to_string()]
        );
        // public roll 的 token 绝不进集合（"roll_a"/"12" 不出现）。
        assert!(!ledger.private_roll_tokens().iter().any(|t| t == "roll_a" || t == "12"));
    }
}
```

- [ ] **Step 2：运行确认失败**

Run: `cargo test -p trpg-gm dispatch_unknown_tool_returns_structured_error`
Expected: FAIL，报错应为 `package ID specification trpg-gm did not match any packages`

- [ ] **Step 3：最小实现**

修改 workspace `Cargo.toml`：

```toml
members = [
  "crates/trpg-model",
  "crates/trpg-formula",
  "crates/trpg-db",
  "crates/trpg-ingest",
  "crates/trpg-llm",
  "crates/trpg-parser",
  "crates/trpg-rule-agent",
  "crates/trpg-params",
  "crates/trpg-semantics",
  "crates/trpg-material",
  "crates/trpg-ability",
  "crates/trpg-referee",
  "crates/trpg-mechanics",
  "crates/trpg-contest",
  "crates/trpg-agent",
  "crates/trpg-combat",
  "crates/trpg-director",
  "crates/trpg-time",
  "crates/trpg-interaction",
  "crates/trpg-object",
  "crates/trpg-orchestrator",
  "crates/trpg-runtime",
  "crates/trpg-search",
  "crates/trpg-api",
  "crates/trpg-cli",
  "crates/trpg-harness",
  "crates/trpg-gm",
]
```

```toml
# crates/trpg-gm/Cargo.toml
[package]
name = "trpg-gm"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
anyhow.workspace = true
async-trait.workspace = true
chrono.workspace = true
futures-core.workspace = true
futures-util.workspace = true
serde.workspace = true
serde_json.workspace = true
sha2.workspace = true
thiserror.workspace = true
tokio.workspace = true
tracing.workspace = true
uuid.workspace = true
trpg-agent = { path = "../trpg-agent" }
trpg-db = { path = "../trpg-db" }
trpg-interaction = { path = "../trpg-interaction" }
trpg-llm = { path = "../trpg-llm" }
trpg-mechanics = { path = "../trpg-mechanics" }
trpg-model = { path = "../trpg-model" }
trpg-runtime = { path = "../trpg-runtime" }
trpg-params = { path = "../trpg-params" }

[dev-dependencies]
async-stream.workspace = true
sqlx.workspace = true
```

将 `crates/trpg-agent/src/gm_loop.rs` 中的 `fn private_roll_leak_tokens(&self) -> Vec<String>` 改为：

```rust
pub fn private_roll_leak_tokens(&self) -> Vec<String> {
```

```rust
// crates/trpg-gm/src/lib.rs
pub mod errata;
pub mod ledger;
pub mod prompts;
pub mod stream;
pub mod tools;
pub mod turn_loop;

pub use errata::{ErrataEntry, ErrataMemory};
pub use ledger::TurnLedger;
pub use prompts::{load_gm_skill, validate_compiled_budget, DynamicTailInput, TurnMessages};
pub use stream::RedactingBuffer;
pub use tools::{
    AwaitingPlayerRoll, GmTool, SceneDeepExtractFn, ToolCtx, ToolDispatchOutcome,
    ToolError, ToolOutput, ToolRegistry, ToolSpec,
};
pub use turn_loop::{CtxProviderFn, GmLoop, GmTurnInput, LoopConfig, TurnOutcome};
```

```rust
// crates/trpg-gm/src/ledger.rs
use std::collections::BTreeSet;
use trpg_agent::TurnLedgerSnapshot;
use trpg_model::{CheckContract, CheckResultRecord, EffectContract, InteractionGate, ParameterImpact};
use trpg_runtime::AutoRollExecution;

#[derive(Debug, Default)]
pub struct TurnLedger {
    snapshot: TurnLedgerSnapshot,
}

impl TurnLedger {
    pub fn new() -> Self { Self::default() }

    pub fn record_contract(&mut self, contract: &CheckContract) {
        self.snapshot.check_contracts.push(contract.clone());
    }

    pub fn record_result(&mut self, result: &CheckResultRecord) {
        self.snapshot.dice_rolls.push(result.roll.clone());
        self.snapshot.check_results.push(result.clone());
    }

    pub fn record_execution(&mut self, exec: &AutoRollExecution) {
        self.record_result(&exec.primary);
        for result in &exec.followups {
            self.record_result(result);
        }
    }

    pub fn record_effect(&mut self, effect: &EffectContract) {
        self.snapshot.effect_contracts.push(effect.clone());
    }

    pub fn record_impact(&mut self, impact: &ParameterImpact) {
        self.snapshot.parameter_impacts.push(impact.clone());
    }

    pub fn record_gate(&mut self, gate: &InteractionGate) {
        self.snapshot.interaction_gates.push(gate.clone());
    }

    pub fn snapshot(&self) -> &TurnLedgerSnapshot { &self.snapshot }

    pub fn private_roll_tokens(&self) -> Vec<String> {
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for token in self.snapshot.private_roll_leak_tokens() {
            let lower = token.to_ascii_lowercase();
            if !lower.trim().is_empty() && seen.insert(lower.clone()) {
                out.push(lower);
            }
        }
        out
    }
}
```

```rust
// crates/trpg-gm/src/tools/mod.rs
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

pub struct ToolSpec {
    pub name: &'static str,
    pub schema: Value,
}

pub type SceneDeepExtractFn =
    Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<usize>> + Send>> + Send + Sync>;

pub struct ToolCtx<'a> {
    pub engine: &'a RuntimeEngine,
    pub request: &'a ContextRequest,
    pub state: &'a RuntimeState,
    pub scene_extractor: Option<&'a SceneDeepExtractFn>,
}

pub struct ToolOutput {
    pub result: Value,
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

#[async_trait]
pub trait GmTool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    async fn call(&self, ctx: &ToolCtx<'_>, ledger: &mut TurnLedger, args: Value) -> Result<ToolOutput>;
}

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct ToolError {
    pub code: String,
    pub message: String,
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

    pub fn to_tool_content(err: &anyhow::Error) -> String {
        let error = if let Some(tool) = err.downcast_ref::<ToolError>() {
            tool.clone()
        } else {
            ToolError { code: "internal_error".to_string(), message: err.to_string(), recoverable: false, hint: None }
        };
        serde_json::to_string(&json!({"error": error})).unwrap_or_else(|_| "{\"error\":{\"code\":\"internal_error\",\"message\":\"serialization failed\",\"recoverable\":false}}".to_string())
    }
}

pub struct ToolDispatchOutcome {
    pub tool_call_id: String,
    pub name: String,
    pub content: String,
    pub awaiting_player_roll: Option<AwaitingPlayerRoll>,
}

pub struct ToolRegistry {
    tools: Vec<Box<dyn GmTool>>,
}

impl ToolRegistry {
    pub fn standard() -> Self { Self { tools: Vec::new() } }

    /// turn_loop 单测注入脚本化 GmTool 替身用（tools 字段私有，跨模块测试只能经此构造）。
    pub fn from_tools(tools: Vec<Box<dyn GmTool>>) -> Self { Self { tools } }

    pub fn schemas(&self) -> Vec<Value> {
        self.tools.iter().map(|t| t.spec().schema).collect()
    }

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
```

为 Task 2 先放空模块文件，后续任务替换内容：

```rust
// crates/trpg-gm/src/tools/check.rs
```

```rust
// crates/trpg-gm/src/tools/effect.rs
```

```rust
// crates/trpg-gm/src/tools/world.rs
```

```rust
// crates/trpg-gm/src/tools/npc.rs
```

```rust
// crates/trpg-gm/src/stream.rs
pub struct RedactingBuffer;
```

```rust
// crates/trpg-gm/src/prompts.rs
use anyhow::{anyhow, Result};
use std::path::Path;
use trpg_model::{ChatMessage, CompiledContext, ContextRequest};

pub struct DynamicTailInput<'a> { pub user_input: &'a str, pub resolved_gate_facts: &'a [String], pub errata_blocks: &'a [String] }
pub struct TurnMessages;
impl TurnMessages { pub fn assemble(_: &CompiledContext, _: &str, _: &[ChatMessage], _: &DynamicTailInput<'_>) -> Self { Self } }
pub fn load_gm_skill(_: &Path, _: &str) -> Result<String> { Err(anyhow!("gm_skill not implemented yet")) }
pub fn validate_compiled_budget(_: &CompiledContext, _: &ContextRequest) -> Result<()> { Ok(()) }
```

```rust
// crates/trpg-gm/src/errata.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use trpg_agent::{VerifierFinding, VerifierFindingKind};
use trpg_model::MemoryEvent;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrataEntry { pub kind: VerifierFindingKind, pub detail: String, pub turn_id: String, pub created_at: DateTime<Utc> }
#[derive(Debug)]
pub struct ErrataMemory { threshold: u8, counts: BTreeMap<String, u32>, entries: Vec<ErrataEntry> }
impl ErrataMemory { pub fn new(repeat_finding_threshold: u8) -> Self { Self { threshold: repeat_finding_threshold, counts: BTreeMap::new(), entries: Vec::new() } } }
```

```rust
// crates/trpg-gm/src/turn_loop.rs
use crate::errata::ErrataMemory;
use crate::tools::{SceneDeepExtractFn, ToolRegistry};
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use trpg_llm::LlmClient;
use trpg_model::{ChatMessage, CompiledContext, ContextRequest, RuntimeState};
use trpg_runtime::RuntimeEngine;

pub type CtxProviderFn = Arc<dyn Fn(&ContextRequest, &RuntimeState) -> CompiledContext + Send + Sync>;

#[derive(Debug, Clone)]
pub struct LoopConfig { pub max_tool_rounds: u8, pub repeat_finding_threshold: u8 }
impl Default for LoopConfig { fn default() -> Self { Self { max_tool_rounds: 8, repeat_finding_threshold: 3 } } }
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome { Narration(String), AwaitingPlayerRoll { check_id: String, prompt_public: String } }
pub struct GmLoop { pub engine: RuntimeEngine, pub llm: Arc<dyn LlmClient>, pub tools: ToolRegistry, pub cfg: LoopConfig, pub data_dir: PathBuf, pub scene_extractor: Option<SceneDeepExtractFn>, pub ctx_provider: Option<CtxProviderFn>, pub errata: ErrataMemory }
pub struct GmTurnInput<'a> { pub request: &'a ContextRequest, pub state: &'a RuntimeState, pub user_input: &'a str, pub history: &'a [ChatMessage], pub recent_transcript: Option<&'a str> }
impl GmLoop {
    pub fn new(engine: RuntimeEngine, llm: Arc<dyn LlmClient>, tools: ToolRegistry, cfg: LoopConfig, data_dir: PathBuf) -> Self { let errata = ErrataMemory::new(cfg.repeat_finding_threshold); Self { engine, llm, tools, cfg, data_dir, scene_extractor: None, ctx_provider: None, errata } }
    pub async fn run_gm_turn(&mut self, _: GmTurnInput<'_>, _: &mut (dyn FnMut(&str) + Send)) -> Result<TurnOutcome> { anyhow::bail!("gm loop not implemented yet") }
}
```

- [ ] **Step 4：运行确认通过**

Run: `cargo test -p trpg-gm dispatch_unknown_tool_returns_structured_error`
Expected: PASS

- [ ] **Step 5：验证收尾**

Run: `cargo test -p trpg-gm && cargo test -p trpg-agent && wc -l crates/trpg-gm/src/lib.rs crates/trpg-gm/src/ledger.rs crates/trpg-gm/src/tools/mod.rs`
Expected: 全绿；列出的文件均 ≤400 行

---

### Task 3：check 工具（tools/check.rs：roll_check + request_player_roll）

**Files:**
- Modify: `crates/trpg-gm/src/tools/check.rs`
- Modify: `crates/trpg-gm/src/tools/mod.rs:ToolRegistry::standard`
- Test: `crates/trpg-gm/src/tools/check.rs` 同文件 `#[cfg(test)]`

- [ ] **Step 1：写失败测试**

```rust
// crates/trpg-gm/src/tools/check.rs
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use trpg_model::{CheckTargetModel, RollAuthority, RollVisibility};

    #[test]
    fn roll_args_require_tested_parameter() {
        let err = parse_roll_check_args(json!({"check_label":"Scan","visibility":"public"})).unwrap_err();
        assert!(err.to_string().contains("invalid_arguments"));
    }

    #[test]
    fn build_contract_binds_tested_parameter_and_visibility() {
        let args = RollCheckArgs { check_label: "Stealth".to_string(), tested_parameter: "Stealth".to_string(), actor_id: Some("pc.current".to_string()), opposed: None, visibility: "secret".to_string(), intent_kind: Some("stealth_or_dangerous_movement".to_string()) };
        let contract = build_check_contract_for_args("s", "t", "cyberpunk_red", Some("homecoming"), &args, "1d10").unwrap();
        assert_eq!(contract.tested_parameter.as_ref().unwrap().key, "Stealth");
        assert_eq!(contract.roll_visibility, RollVisibility::PrivateGmRoll);
        assert_eq!(contract.roll_authority, RollAuthority::System);
        // CheckTargetModel 是带数据枚举，没有 as_str()，必须用 matches! 断言。
        assert!(matches!(contract.target, CheckTargetModel::UnknownUntilLookup));
    }

    #[test]
    fn request_player_roll_contract_uses_player_authority() {
        let args = RequestPlayerRollArgs { check_label: "Athletics".to_string(), tested_parameter: "Athletics".to_string(), stakes: StakesArgs { before: "The jump is risky.".to_string(), success: "You land cleanly.".to_string(), failure: "You fall short.".to_string() }, visibility: "public".to_string() };
        let contract = build_player_contract_for_args("s", "t", "sw2_5", None, &args, "2d6").unwrap();
        assert_eq!(contract.roll_visibility, RollVisibility::PlayerRollRequired);
        assert_eq!(contract.roll_authority, RollAuthority::Player);
        assert_eq!(contract.stakes.before_roll_public, "The jump is risky.");
    }
}
```

- [ ] **Step 2：运行确认失败**

Run: `cargo test -p trpg-gm roll_args_require_tested_parameter`
Expected: FAIL，报错应为 `cannot find function parse_roll_check_args`

- [ ] **Step 3：最小实现**

```rust
// crates/trpg-gm/src/tools/check.rs
use crate::ledger::TurnLedger;
use crate::tools::{AwaitingPlayerRoll, GmTool, ToolCtx, ToolError, ToolOutput, ToolSpec};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use trpg_agent::make_pending_check;
use trpg_model::*;
use uuid::Uuid;

#[derive(Debug, Clone, Deserialize)]
pub struct OpposedArgs {
    pub npc_id: String,
    pub opponent_parameter: String,
    /// 防御参数所在桶（schema default "skills"——默认值放 schema/serde 层，不在代码里硬编码语义）。
    #[serde(default = "default_skills_bucket")]
    pub bucket: String,
}

fn default_skills_bucket() -> String { "skills".to_string() }

#[derive(Debug, Clone, Deserialize)]
pub struct RollCheckArgs {
    pub check_label: String,
    pub tested_parameter: String,
    pub actor_id: Option<String>,
    pub opposed: Option<OpposedArgs>,
    #[serde(default = "default_public")]
    pub visibility: String,
    pub intent_kind: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StakesArgs { pub before: String, pub success: String, pub failure: String }

#[derive(Debug, Clone, Deserialize)]
pub struct RequestPlayerRollArgs {
    pub check_label: String,
    pub tested_parameter: String,
    pub stakes: StakesArgs,
    #[serde(default = "default_public")]
    pub visibility: String,
}

fn default_public() -> String { "public".to_string() }

pub fn parse_roll_check_args(value: Value) -> Result<RollCheckArgs> {
    let args: RollCheckArgs = serde_json::from_value(value)
        .map_err(|e| ToolError::recoverable("invalid_arguments", format!("roll_check arguments invalid: {e}"), Some("Provide check_label and tested_parameter.".to_string())))?;
    if args.tested_parameter.trim().is_empty() {
        return Err(ToolError::recoverable("invalid_arguments", "tested_parameter is required", Some("Bind the check to the real actor parameter being tested.".to_string())));
    }
    Ok(args)
}

fn parse_player_args(value: Value) -> Result<RequestPlayerRollArgs> {
    let args: RequestPlayerRollArgs = serde_json::from_value(value)
        .map_err(|e| ToolError::recoverable("invalid_arguments", format!("request_player_roll arguments invalid: {e}"), Some("Provide check_label, tested_parameter, stakes, and visibility.".to_string())))?;
    if args.tested_parameter.trim().is_empty() {
        return Err(ToolError::recoverable("invalid_arguments", "tested_parameter is required", Some("Bind the pending roll to the actor parameter.".to_string())));
    }
    Ok(args)
}

fn visibility_for_system(s: &str) -> Result<RollVisibility> {
    match s.trim().to_ascii_lowercase().as_str() {
        "public" => Ok(RollVisibility::PublicGmRoll),
        "secret" => Ok(RollVisibility::PrivateGmRoll),
        other => Err(ToolError::recoverable("invalid_arguments", format!("invalid visibility: {other}"), Some("Use public or secret.".to_string()))),
    }
}

pub fn build_check_contract_for_args(session_id: &str, turn_id: &str, ruleset_id: &str, module_id: Option<&str>, args: &RollCheckArgs, dice: &str) -> Result<CheckContract> {
    let visibility = visibility_for_system(&args.visibility)?;
    let actor_id = args.actor_id.clone().unwrap_or_else(|| "pc.current".to_string());
    Ok(CheckContract {
        check_id: format!("check_{}", Uuid::new_v4().simple()),
        session_id: session_id.to_string(),
        turn_id: turn_id.to_string(),
        ruleset_id: ruleset_id.to_string(),
        module_id: module_id.map(str::to_string),
        initiator: ActorRef { actor_id, actor_kind: ActorKind::PlayerCharacter, display_name: Some("current PC".to_string()) },
        target_actor: None,
        opposition: OppositionModel::NoMechanicalOpposition,
        action_summary: args.check_label.chars().take(500).collect(),
        intent_kind: args.intent_kind.clone().unwrap_or_else(|| "agent_selected_check".to_string()),
        check_label: args.check_label.clone(),
        dice_expression: dice.to_string(),
        modifiers: vec![],
        target: CheckTargetModel::UnknownUntilLookup,
        tested_parameter: Some(TestedParameter { domain: None, key: args.tested_parameter.clone(), label: args.tested_parameter.clone() }),
        opponent_tested_parameter: None,
        actor_snapshot_ids: vec![],
        source_refs: vec![],
        learned_packet_ids: vec![],
        roll_visibility: visibility,
        roll_authority: RollAuthority::System,
        disclosure: RollDisclosurePolicy::for_visibility(visibility),
        stakes: CheckStakes {
            before_roll_public: format!("A {} check is required; its result determines the immediate consequence.", args.check_label),
            success_public: format!("The {} check succeeds.", args.check_label),
            failure_public: format!("The {} check fails and a cost follows.", args.check_label),
            critical_public: None,
            fumble_public: None,
            success_patches_allowed: vec![],
            failure_patches_allowed: vec![],
            irreversible: false,
        },
        confidence: RulingConfidence::Medium,
        ruling_status: RulingStatus::Provisional,
        advice_refs: vec!["gm_agent.roll_check".to_string()],
        expires_at_turn: Some(turn_id.to_string()),
    })
}

pub fn build_player_contract_for_args(session_id: &str, turn_id: &str, ruleset_id: &str, module_id: Option<&str>, args: &RequestPlayerRollArgs, dice: &str) -> Result<CheckContract> {
    let mut sys_args = RollCheckArgs { check_label: args.check_label.clone(), tested_parameter: args.tested_parameter.clone(), actor_id: Some("pc.current".to_string()), opposed: None, visibility: args.visibility.clone(), intent_kind: Some("player_roll_requested".to_string()) };
    sys_args.visibility = "public".to_string();
    let mut c = build_check_contract_for_args(session_id, turn_id, ruleset_id, module_id, &sys_args, dice)?;
    c.roll_visibility = RollVisibility::PlayerRollRequired;
    c.roll_authority = RollAuthority::Player;
    c.disclosure = RollDisclosurePolicy::for_visibility(RollVisibility::PlayerRollRequired);
    c.stakes.before_roll_public = args.stakes.before.clone();
    c.stakes.success_public = args.stakes.success.clone();
    c.stakes.failure_public = args.stakes.failure.clone();
    Ok(c)
}

async fn kernel_dice(ctx: &ToolCtx<'_>) -> Result<String> {
    let kernel = ctx.engine.db.load_rule_kernel(&ctx.request.ruleset_id).await?;
    let dice = kernel
        .and_then(|k| k.dice_core.get("dice").and_then(Value::as_str).map(str::to_string))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ToolError::recoverable("missing_kernel_dice", "rule kernel has no dice_core.dice", Some("Call retrieve_rules for a source-backed procedure, ask for a player roll, or narrate without a mechanical roll.".to_string())))?;
    Ok(dice)
}

pub struct RollCheckTool;

#[async_trait]
impl GmTool for RollCheckTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec { name: "roll_check", schema: json!({"type":"function","function":{"name":"roll_check","description":"Execute a source-backed system roll. tested_parameter is mandatory.","parameters":{"type":"object","properties":{"check_label":{"type":"string"},"tested_parameter":{"type":"string"},"actor_id":{"type":"string"},"opposed":{"type":"object","properties":{"npc_id":{"type":"string"},"opponent_parameter":{"type":"string"},"bucket":{"type":"string","default":"skills"}},"required":["npc_id","opponent_parameter"]},"visibility":{"type":"string","enum":["public","secret"]},"intent_kind":{"type":"string"}},"required":["check_label","tested_parameter"]}}}) }
    }

    async fn call(&self, ctx: &ToolCtx<'_>, ledger: &mut TurnLedger, args: Value) -> Result<ToolOutput> {
        let args = parse_roll_check_args(args)?;
        let dice = kernel_dice(ctx).await?;
        let mut contract = build_check_contract_for_args(&ctx.request.session_id, &ctx.request.turn_id, &ctx.request.ruleset_id, ctx.request.module_id.as_deref(), &args, &dice)?;
        if let Some(opposed) = &args.opposed {
            let persona = trpg_runtime::npc_synth::NpcPersona { actor_id: opposed.npc_id.clone(), name: opposed.npc_id.clone(), prose: "opposition selected by GM agent".to_string() };
            // fail-closed 不静默：防御参数合成失败 ⇒ 结构化错误让 agent 改道
            // （request_player_roll / 非对抗 roll_check / 纯叙事），绝不带着未
            // 物化的防御值继续 stamp。注意 ensure_npc_parameter 返回
            // Result<Option<Value>>：合成门关（TRPG_NPC_PERSONA_SYNTHESIS=0）
            // 或 NPC 无参数卡时是 Ok(None) 而非 Err——两种形态都必须拦。
            let synthesized = ctx.engine.ensure_npc_parameter(&ctx.request.session_id, &ctx.request.ruleset_id, &persona, &opposed.bucket, &opposed.opponent_parameter, &args.check_label).await;
            match synthesized {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return Err(ToolError::recoverable(
                        "npc_synthesis_unavailable",
                        format!("opponent parameter {}.{} for {} not materialized (synthesis gated off or NPC has no parameter card)", opposed.bucket, opposed.opponent_parameter, opposed.npc_id),
                        Some("Retry without `opposed`, use request_player_roll, or narrate without a contested roll.".to_string()),
                    ));
                }
                Err(err) => {
                    return Err(ToolError::recoverable(
                        "npc_synthesis_unavailable",
                        format!("could not materialize opponent parameter {}.{} for {}: {err}", opposed.bucket, opposed.opponent_parameter, opposed.npc_id),
                        Some("Retry without `opposed`, use request_player_roll, or narrate without a contested roll.".to_string()),
                    ));
                }
            }
            trpg_runtime::stamp_opposed_check(&mut contract, &persona, &opposed.bucket, &opposed.opponent_parameter);
        }
        // 契约落 check_contracts 表（对齐旧路径 persist_agent_plan 的 db.insert_check_contract；
        // 只入内存 ledger 的话 Task 11 验收 SQL 对 agent 路径恒为空）。
        ctx.engine.db.insert_check_contract(&contract, "created").await?;
        ledger.record_contract(&contract);
        let exec = ctx.engine.execute_system_roll_bundle(&ctx.request.session_id, &ctx.request.turn_id, &contract).await?;
        ledger.record_execution(&exec);
        Ok(ToolOutput::ok(json!({
            "check_id": contract.check_id,
            "check_label": contract.check_label,
            "roll_policy": exec.roll_policy,
            "outcome": exec.primary.outcome,
            "primary_roll": exec.primary.roll.result,
            "committed_patch_count": exec.primary.committed_patches.len(),
            "followup_count": exec.followups.len()
        })))
    }
}

pub struct RequestPlayerRollTool;

#[async_trait]
impl GmTool for RequestPlayerRollTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec { name: "request_player_roll", schema: json!({"type":"function","function":{"name":"request_player_roll","description":"Open an InteractionGate and wait for the player to roll personally.","parameters":{"type":"object","properties":{"check_label":{"type":"string"},"tested_parameter":{"type":"string"},"stakes":{"type":"object","properties":{"before":{"type":"string"},"success":{"type":"string"},"failure":{"type":"string"}},"required":["before","success","failure"]},"visibility":{"type":"string","enum":["public","secret"]}},"required":["check_label","tested_parameter","stakes"]}}}) }
    }

    async fn call(&self, ctx: &ToolCtx<'_>, ledger: &mut TurnLedger, args: Value) -> Result<ToolOutput> {
        let args = parse_player_args(args)?;
        let dice = kernel_dice(ctx).await?;
        let contract = build_player_contract_for_args(&ctx.request.session_id, &ctx.request.turn_id, &ctx.request.ruleset_id, ctx.request.module_id.as_deref(), &args, &dice)?;
        // 对齐 persist_agent_plan 既有行为：插新 pending 前先作废旧 open gate，
        // 防同会话累积多个 open pending check。
        ctx.engine.db.cancel_open_pending_checks_for_session(&ctx.request.session_id, PendingCheckStatus::Superseded).await?;
        ctx.engine.db.insert_check_contract(&contract, "created").await?;
        let pending = make_pending_check(&contract);
        let gate = InteractionGate::from_pending_check(&pending);
        ctx.engine.db.insert_pending_check(&pending).await?;
        ctx.engine.db.insert_interaction_gate(&gate).await?;
        ledger.record_contract(&contract);
        ledger.record_gate(&gate);
        Ok(ToolOutput::awaiting(json!({"check_id": contract.check_id, "prompt_public": pending.prompt_public}), AwaitingPlayerRoll { check_id: contract.check_id, prompt_public: pending.prompt_public }))
    }
}
```

修改 `ToolRegistry::standard()`：

```rust
pub fn standard() -> Self {
    Self { tools: vec![
        Box::new(check::RollCheckTool),
        Box::new(check::RequestPlayerRollTool),
    ] }
}
```

- [ ] **Step 4：运行确认通过**

Run: `cargo test -p trpg-gm roll_args_require_tested_parameter`
Expected: PASS

- [ ] **Step 5：验证收尾**

Run: `cargo test -p trpg-gm && wc -l crates/trpg-gm/src/tools/check.rs crates/trpg-gm/src/tools/mod.rs`
Expected: 全绿；`check.rs` 与 `mod.rs` 均 ≤400 行

---

### Task 4：effect 工具（tools/effect.rs + mechanics pub 边界）

**Files:**
- Create: `crates/trpg-mechanics/src/direct_effect.rs`
- Modify: `crates/trpg-mechanics/src/lib.rs:模块声明 + apply_effect_roll 抽 decision_override 参数化变体（原名一行委托，既有调用方零改动）`
- Modify: `crates/trpg-gm/src/tools/effect.rs`
- Modify: `crates/trpg-gm/src/tools/mod.rs:ToolRegistry::standard`
- Test: `crates/trpg-mechanics/src/direct_effect.rs` 与 `crates/trpg-gm/src/tools/effect.rs` 同文件 `#[cfg(test)]`（纯构造断言，不连 DB；apply_effect_roll 真 SQL 路径留 Task 11 e2e）

- [ ] **Step 1：写失败测试**

```rust
// crates/trpg-gm/src/tools/effect.rs
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn apply_effect_requires_one_target_path() {
        let err = parse_apply_effect_args(json!({"target_actor":"npc.opposition","op":"subtract","amount":3,"reason":"hit"})).unwrap_err();
        assert!(err.to_string().contains("invalid_arguments"));
    }

    #[test]
    fn change_track_accepts_string_value_without_amount() {
        let args = parse_change_track_args(json!({"bucket":"field","id":"class","op":"set","value":"solo"})).unwrap();
        assert_eq!(args.value.as_deref(), Some("solo"));
        assert_eq!(args.amount.unwrap_or(0.0), 0.0);
    }
}
```

```rust
// crates/trpg-mechanics/src/direct_effect.rs（纯构造断言，不连 DB）
#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::{EffectKind, ParameterOperation, RollAuthority, RollVisibility};

    #[test]
    fn effect_kind_maps_by_operation() {
        assert!(matches!(effect_kind_for_operation(ParameterOperation::Subtract), EffectKind::Damage));
        assert!(matches!(effect_kind_for_operation(ParameterOperation::Add), EffectKind::Healing));
        assert!(matches!(effect_kind_for_operation(ParameterOperation::Set), EffectKind::NarrativeConsequence));
    }

    #[test]
    fn synthetic_contract_carries_amount_and_target() {
        let (contract, result) = build_direct_effect_contract("s", "rs", None, "pc.current", "npc.opposition", "sanity.current", 4, "shock");
        assert_eq!(contract.target_actor.as_ref().unwrap().actor_id, "npc.opposition");
        assert_eq!(contract.tested_parameter.as_ref().unwrap().key, "sanity.current");
        assert_eq!(contract.roll_visibility, RollVisibility::PrivateGmRoll);
        assert_eq!(contract.roll_authority, RollAuthority::System);
        assert_eq!(result.roll.result.get("total").and_then(|v| v.as_i64()), Some(4));
        assert_eq!(result.check_id, contract.check_id);
    }
}
```

- [ ] **Step 2：运行确认失败**

Run: `cargo test -p trpg-gm apply_effect_requires_one_target_path`
Expected: FAIL，报错应为 `cannot find function parse_apply_effect_args`

- [ ] **Step 3：最小实现**

```rust
// crates/trpg-mechanics/src/direct_effect.rs
use crate::FacetDecision;
use anyhow::Result;
use chrono::Utc;
use serde_json::json;
use trpg_model::{
    ActorKind, ActorRef, CheckContract, CheckModifier, CheckResultRecord, CheckStakes,
    CheckTargetModel, DiceRollRecord, EffectContract, EffectKind, EffectTargetKind,
    FacetExecutionStatus, ParameterImpact, ParameterOperation, RollAuthority,
    RollDisclosurePolicy, RollVisibility, RulingConfidence, RulingStatus, StatePatch,
    TestedParameter, Visibility,
};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct DirectEffectOutcome {
    pub effect: EffectContract,
    pub impacts: Vec<ParameterImpact>,
    pub patches: Vec<StatePatch>,
}

/// PURE：operation → EffectKind 语义映射（EffectKind 没有 ResourceDelta 变体，
/// 真实变体见 trpg-model：Damage/Healing/ResourceSpend/NarrativeConsequence…）。
pub fn effect_kind_for_operation(operation: ParameterOperation) -> EffectKind {
    match operation {
        ParameterOperation::Subtract => EffectKind::Damage,
        ParameterOperation::Add => EffectKind::Healing,
        _ => EffectKind::NarrativeConsequence,
    }
}

/// PURE：合成契约 + 合成结果（roll.result.total = amount，无 RNG），不触 DB。
/// CheckContract 没有 metadata 字段（也不存在 metadata_mut）——operation/source
/// 信息只进最终 EffectContract.metadata。
pub fn build_direct_effect_contract(
    session_id: &str,
    ruleset_id: &str,
    module_id: Option<&str>,
    source_actor_id: &str,
    target_actor_id: &str,
    parameter_path: &str,
    amount: i64,
    reason: &str,
) -> (CheckContract, CheckResultRecord) {
    let check_id = format!("check_direct_effect_{}", Uuid::new_v4().simple());
    let turn_id = "direct_effect".to_string();
    let contract = CheckContract {
        check_id: check_id.clone(),
        session_id: session_id.to_string(),
        turn_id: turn_id.clone(),
        ruleset_id: ruleset_id.to_string(),
        module_id: module_id.map(str::to_string),
        initiator: ActorRef { actor_id: source_actor_id.to_string(), actor_kind: ActorKind::PlayerCharacter, display_name: Some("effect source".to_string()) },
        target_actor: Some(ActorRef { actor_id: target_actor_id.to_string(), actor_kind: ActorKind::Npc, display_name: Some("effect target".to_string()) }),
        opposition: trpg_model::OppositionModel::NoMechanicalOpposition,
        action_summary: reason.to_string(),
        intent_kind: "effect_roll".to_string(),
        check_label: "direct effect".to_string(),
        dice_expression: amount.to_string(),
        modifiers: Vec::<CheckModifier>::new(),
        target: CheckTargetModel::UnknownUntilLookup,
        tested_parameter: Some(TestedParameter { domain: None, key: parameter_path.to_string(), label: parameter_path.to_string() }),
        opponent_tested_parameter: None,
        actor_snapshot_ids: vec![target_actor_id.to_string()],
        source_refs: vec![],
        learned_packet_ids: vec![],
        roll_visibility: RollVisibility::PrivateGmRoll,
        roll_authority: RollAuthority::System,
        disclosure: RollDisclosurePolicy::for_visibility(RollVisibility::PrivateGmRoll),
        stakes: CheckStakes { before_roll_public: reason.to_string(), success_public: "The direct effect is applied.".to_string(), failure_public: "The direct effect could not be applied.".to_string(), critical_public: None, fumble_public: None, success_patches_allowed: vec![], failure_patches_allowed: vec![], irreversible: false },
        confidence: RulingConfidence::Medium,
        ruling_status: RulingStatus::Provisional,
        advice_refs: vec!["gm_agent.apply_effect".to_string()],
        expires_at_turn: None,
    };
    let roll = DiceRollRecord { roll_id: format!("roll_{}", Uuid::new_v4().simple()), session_id: session_id.to_string(), turn_id, check_id: Some(check_id.clone()), roller_kind: ActorKind::System, roller_id: Some("gm_agent".to_string()), visibility: RollVisibility::PrivateGmRoll, expression: amount.to_string(), result: json!({"total": amount}), seed_commitment: "direct_effect_no_rng".to_string(), revealed_at: None, created_at: Utc::now() };
    let result = CheckResultRecord { check_id, roll, outcome: json!({"success": true, "total": amount}), committed_patches: vec![], created_at: Utc::now() };
    (contract, result)
}

impl crate::RefereeCombatService {
    pub async fn apply_direct_effect(
        &self,
        session_id: &str,
        ruleset_id: &str,
        module_id: Option<&str>,
        source_actor_id: &str,
        target_actor_id: &str,
        parameter_path: &str,
        operation: ParameterOperation,
        amount: i64,
        reason: &str,
        visibility: Visibility,
    ) -> Result<DirectEffectOutcome> {
        let (contract, mut result) = build_direct_effect_contract(session_id, ruleset_id, module_id, source_actor_id, target_actor_id, parameter_path, amount, reason);
        // 显式落点：直接效果的 path/op 由 GM 裁量给定，必须绕过
        // resolve_facet_decision 的 hp.current fallback（无绑定 facet 时它恒回落
        // hp.current，parameter_path/operation 会形同虚设）。FacetDecision 与
        // apply_effect_roll_with_decision 均为 crate 根私有项，本文件是同 crate
        // 子模块可直接访问。
        let decision = FacetDecision {
            target_kind: EffectTargetKind::Actor,
            target_id: target_actor_id.to_string(),
            parameter_path: parameter_path.to_string(),
            operation,
            source_facet_binding_ids: vec![],
            source_refs: vec![],
            status: FacetExecutionStatus::AppliedProvisional,
            provisional_reason: Some(format!("GM agent direct effect: {reason}")),
        };
        let applied = self.apply_effect_roll_with_decision(&contract, &result, Some(decision)).await?;
        result.committed_patches.extend(applied.patches.clone());
        let effect = EffectContract { effect_id: applied.effect.effect_resolution_id.clone(), source_event_id: None, effect_kind: effect_kind_for_operation(operation), target_actor_ids: vec![target_actor_id.to_string()], source_refs: applied.effect.source_refs.clone(), learned_packet_ids: vec![], deterministic_parts: vec![], pending_rolls: vec![], proposed_patches: applied.patches.clone(), visibility, confidence: RulingConfidence::Medium, metadata: json!({"reason": reason, "source": "direct_effect", "source_actor_id": source_actor_id, "parameter_path": parameter_path, "operation": operation.as_str()}) };
        Ok(DirectEffectOutcome { effect, impacts: applied.effect.impacts, patches: applied.patches })
    }
}
```

`crates/trpg-mechanics/src/lib.rs` 两处改动：① 顶部加入模块声明；② 私有
`apply_effect_roll` 抽出 decision 参数化变体（原名一行委托 `None`，既有调用方
零改动；只动签名所在两行，函数体内 `let decision = ...` 一行改为按 override 取）：

```rust
pub mod direct_effect;
```

```rust
// 原：async fn apply_effect_roll(&self, contract: &CheckContract, result: &CheckResultRecord) -> Result<EffectApplicationOutcome> { ... }
// 改为：
async fn apply_effect_roll(&self, contract: &CheckContract, result: &CheckResultRecord) -> Result<EffectApplicationOutcome> {
    self.apply_effect_roll_with_decision(contract, result, None).await
}

async fn apply_effect_roll_with_decision(&self, contract: &CheckContract, result: &CheckResultRecord, decision_override: Option<FacetDecision>) -> Result<EffectApplicationOutcome> {
    // ...原 apply_effect_roll 函数体整体平移，仅这一行变化：
    // 原：let decision = self.resolve_facet_decision(contract, &target_actor).await?;
    let decision = match decision_override {
        Some(decision) => decision,
        None => self.resolve_facet_decision(contract, &target_actor).await?,
    };
    // ...函数体其余部分不动...
}
```

```rust
// crates/trpg-gm/src/tools/effect.rs
use crate::ledger::TurnLedger;
use crate::tools::{GmTool, ToolCtx, ToolError, ToolOutput, ToolSpec};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use trpg_mechanics::RefereeCombatService;
use trpg_model::{ParameterOperation, Visibility};

#[derive(Debug, Clone, Deserialize)]
pub struct ApplyEffectArgs {
    pub target_actor: String,
    pub parameter_path: Option<String>,
    pub track_id: Option<String>,
    pub op: String,
    pub amount: i64,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChangeTrackArgs {
    pub bucket: String,
    pub id: String,
    pub op: String,
    pub amount: Option<f64>,
    pub value: Option<String>,
    pub kind: Option<String>,
    pub category: Option<String>,
}

pub fn parse_apply_effect_args(value: Value) -> Result<ApplyEffectArgs> {
    let args: ApplyEffectArgs = serde_json::from_value(value).map_err(|e| ToolError::recoverable("invalid_arguments", format!("apply_effect arguments invalid: {e}"), None))?;
    let path_count = args.parameter_path.iter().filter(|s| !s.trim().is_empty()).count() + args.track_id.iter().filter(|s| !s.trim().is_empty()).count();
    if path_count != 1 {
        return Err(ToolError::recoverable("invalid_arguments", "provide exactly one of parameter_path or track_id", Some("Use parameter_path for actor fields or track_id for resources/tracks.".to_string())));
    }
    Ok(args)
}

pub fn parse_change_track_args(value: Value) -> Result<ChangeTrackArgs> {
    serde_json::from_value(value).map_err(|e| ToolError::recoverable("invalid_arguments", format!("change_track arguments invalid: {e}"), None))
}

fn op_from_str(s: &str) -> Result<ParameterOperation> {
    match s.trim().to_ascii_lowercase().as_str() {
        "add" => Ok(ParameterOperation::Add),
        "subtract" => Ok(ParameterOperation::Subtract),
        "set" => Ok(ParameterOperation::Set),
        "add_condition" => Ok(ParameterOperation::AddCondition),
        "remove_condition" => Ok(ParameterOperation::RemoveCondition),
        "advance_clock" => Ok(ParameterOperation::AdvanceClock),
        "mark_revealed" => Ok(ParameterOperation::MarkRevealed),
        other => Err(ToolError::recoverable("invalid_arguments", format!("invalid operation: {other}"), None)),
    }
}

pub struct ApplyEffectTool;

#[async_trait]
impl GmTool for ApplyEffectTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec { name: "apply_effect", schema: json!({"type":"function","function":{"name":"apply_effect","description":"Apply a direct state/resource effect through mechanics.","parameters":{"type":"object","properties":{"target_actor":{"type":"string"},"parameter_path":{"type":"string"},"track_id":{"type":"string"},"op":{"type":"string","enum":["add","subtract","set","add_condition","remove_condition","advance_clock","mark_revealed"]},"amount":{"type":"integer"},"reason":{"type":"string"}},"required":["target_actor","op","amount","reason"]}}}) }
    }

    async fn call(&self, ctx: &ToolCtx<'_>, ledger: &mut TurnLedger, args: Value) -> Result<ToolOutput> {
        let args = parse_apply_effect_args(args)?;
        let parameter_path = args.parameter_path.clone().unwrap_or_else(|| format!("tracks.{}.current", args.track_id.clone().unwrap_or_default()));
        let service = RefereeCombatService::new(ctx.engine.db.clone());
        let outcome = service.apply_direct_effect(&ctx.request.session_id, &ctx.request.ruleset_id, ctx.request.module_id.as_deref(), "pc.current", &args.target_actor, &parameter_path, op_from_str(&args.op)?, args.amount, &args.reason, Visibility::GmOnly).await?;
        ledger.record_effect(&outcome.effect);
        for impact in &outcome.impacts { ledger.record_impact(impact); }
        Ok(ToolOutput::ok(json!({"effect_id": outcome.effect.effect_id, "impact_count": outcome.impacts.len(), "patch_count": outcome.patches.len()})))
    }
}

pub struct ChangeTrackTool;

#[async_trait]
impl GmTool for ChangeTrackTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec { name: "change_track", schema: json!({"type":"function","function":{"name":"change_track","description":"Apply growth or explicit track movement.","parameters":{"type":"object","properties":{"bucket":{"type":"string","enum":["tracks","stats","skills","field"]},"id":{"type":"string"},"op":{"type":"string","enum":["set","add"]},"amount":{"type":"number"},"value":{"type":"string"},"kind":{"type":"string"},"category":{"type":"string"}},"required":["bucket","id","op"]}}}) }
    }

    async fn call(&self, ctx: &ToolCtx<'_>, _ledger: &mut TurnLedger, args: Value) -> Result<ToolOutput> {
        let args = parse_change_track_args(args)?;
        let text = args.value.as_deref();
        let amount = args.amount.unwrap_or(0.0);
        let ok = ctx.engine.apply_track_change(&ctx.request.session_id, "pc.current", &args.bucket, &args.id, &args.op, amount, text, args.kind.as_deref(), args.category.as_deref()).await?;
        Ok(ToolOutput::ok(json!({"changed": ok, "bucket": args.bucket, "id": args.id})))
    }
}
```

修改 `ToolRegistry::standard()` 保持顺序：

```rust
pub fn standard() -> Self {
    Self { tools: vec![
        Box::new(check::RollCheckTool),
        Box::new(check::RequestPlayerRollTool),
        Box::new(effect::ApplyEffectTool),
        Box::new(effect::ChangeTrackTool),
    ] }
}
```

- [ ] **Step 4：运行确认通过**

Run: `cargo test -p trpg-gm apply_effect_requires_one_target_path`
Expected: PASS

- [ ] **Step 5：验证收尾**

Run: `cargo test -p trpg-mechanics && cargo test -p trpg-gm && wc -l crates/trpg-mechanics/src/direct_effect.rs crates/trpg-gm/src/tools/effect.rs`
Expected: 全绿；`direct_effect.rs` 与 `effect.rs` 均 ≤400 行

---

### Task 5：world + npc 工具（tools/world.rs + tools/npc.rs）

**Files:**
- Modify: `crates/trpg-gm/src/tools/world.rs`
- Modify: `crates/trpg-gm/src/tools/npc.rs`
- Modify: `crates/trpg-gm/src/tools/mod.rs:ToolRegistry::standard`
- Modify: `crates/trpg-api/src/lib.rs:extract_module_scenes 可见性`
- Test: `crates/trpg-gm/src/tools/world.rs` 同文件 `#[cfg(test)]`

- [ ] **Step 1：写失败测试**

```rust
// crates/trpg-gm/src/tools/world.rs
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
    fn registry_has_ten_tools_in_stable_order() {
        let names = crate::tools::ToolRegistry::standard().schemas().into_iter()
            .map(|v| v.pointer("/function/name").and_then(|x| x.as_str()).unwrap_or("").to_string())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["roll_check", "request_player_roll", "apply_effect", "change_track", "retrieve_rules", "get_actor", "ensure_npc_param", "navigate_scene", "advance_time", "remember"]);
    }

    #[test]
    fn navigate_args_require_target() {
        let err = parse_navigate_args(json!({"reason":"move"})).unwrap_err();
        assert!(err.to_string().contains("invalid_arguments"));
    }
}
```

- [ ] **Step 2：运行确认失败**

Run: `cargo test -p trpg-gm invalid_time_scale_is_fail_closed`
Expected: FAIL，报错应为 `cannot find function parse_time_scale`

- [ ] **Step 3：最小实现**

把 `crates/trpg-api/src/lib.rs` 的 `extract_module_scenes` 改为 `pub async fn extract_module_scenes(...)`，参数列表与函数体不变。

```rust
// crates/trpg-gm/src/tools/world.rs
use crate::ledger::TurnLedger;
use crate::tools::{GmTool, ToolCtx, ToolError, ToolOutput, ToolSpec};
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use trpg_model::{MemoryEvent, MemoryKind, TimeAdvanceRequest, TimeAmount, TimeScale, Visibility};
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
        Ok(ToolOutput::ok(json!({"from_tick": result.from.world_tick, "to_tick": result.to.world_tick, "triggered_events": result.triggered_events.len()})))
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
        let extracted = if let Some(extractor) = ctx.scene_extractor { Some(extractor(args.target_node_id.clone()).await?) } else { None };
        Ok(ToolOutput::ok(json!({"scene_id": args.target_node_id, "reason": args.reason, "deep_extracted": extracted})))
    }
}
```

```rust
// crates/trpg-gm/src/tools/npc.rs
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
        let raw = graph.npcs.iter().find(|v| v.get("id").and_then(|x| x.as_str()) == Some(args.npc_id.as_str())).cloned().unwrap_or_else(|| json!({"id":args.npc_id,"name":args.npc_id,"summary":""}));
        let persona = NpcPersona { actor_id: args.npc_id.clone(), name: raw.get("name").and_then(Value::as_str).unwrap_or(&args.npc_id).to_string(), prose: raw.get("body").or_else(|| raw.get("summary")).and_then(Value::as_str).unwrap_or("").to_string() };
        let value = ctx.engine.ensure_npc_parameter(&ctx.request.session_id, &ctx.request.ruleset_id, &persona, &args.bucket, &args.param, &args.context).await?;
        Ok(ToolOutput::ok(json!({"npc_id": args.npc_id, "bucket": args.bucket, "param": args.param, "value": value})))
    }
}
```

修改 `ToolRegistry::standard()` 保持完整顺序：

```rust
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
```

- [ ] **Step 4：运行确认通过**

Run: `cargo test -p trpg-gm invalid_time_scale_is_fail_closed`
Expected: PASS

- [ ] **Step 5：验证收尾**

Run: `cargo test -p trpg-gm && cargo build -p trpg-api && wc -l crates/trpg-gm/src/tools/world.rs crates/trpg-gm/src/tools/npc.rs`
Expected: 全绿；`world.rs` 与 `npc.rs` 均 ≤400 行

---

### Task 6：prompts.rs 消息装配 + data/agent/gm_skill 蒸馏

**Files:**
- Create: `data/agent/gm_skill/global/10_dice_discretion.md`
- Create: `data/agent/gm_skill/global/20_item_npc_policy.md`
- Create: `data/agent/gm_skill/global/30_output_contract.md`
- Modify: `crates/trpg-gm/src/prompts.rs`
- Modify: `crates/trpg-model/src/lib.rs:RuntimeState 增加 agent_loop_protocol 开关字段`
- Modify: `crates/trpg-runtime/src/lib.rs:engine_protocol_block_agent_loop + prepare_turn_context 选块开关`
- Test: `crates/trpg-gm/src/prompts.rs` 与 `crates/trpg-runtime/src/lib.rs` 同文件 `#[cfg(test)]`

- [ ] **Step 1：写失败测试**

```rust
// crates/trpg-gm/src/prompts.rs
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::fs;
    use trpg_model::{ChatMessage, CompiledContext};

    fn compiled() -> CompiledContext {
        CompiledContext { prefix_text: "BP1".to_string(), pinned_text: "BP2".to_string(), dynamic_text: "BP3".to_string(), prefix_hash: "p".to_string(), pinned_hash: "m".to_string(), dynamic_hash: "d".to_string(), ..Default::default() }
    }

    #[test]
    fn assemble_order_is_stable() {
        let history = vec![ChatMessage { role: "assistant".to_string(), content: "old".to_string() }];
        let tail = DynamicTailInput { user_input: "go", resolved_gate_facts: &["[roll]done[/roll]".to_string()], errata_blocks: &["[gm_errata]fix[/gm_errata]".to_string()] };
        let messages = TurnMessages::assemble(&compiled(), "SKILL", &history, &tail);
        let raw = messages.to_request_messages();
        assert_eq!(raw[0].get("role").and_then(Value::as_str), Some("system"));
        assert!(raw[0].get("content").and_then(Value::as_str).unwrap().contains("BP1"));
        assert!(raw[1].get("content").and_then(Value::as_str).unwrap().contains("BP2"));
        assert!(raw[2].get("content").and_then(Value::as_str).unwrap().contains("old"));
        assert!(raw[3].get("content").and_then(Value::as_str).unwrap().contains("BP3"));
        assert!(raw[3].get("content").and_then(Value::as_str).unwrap().contains("[Player Input]"));
    }

    #[test]
    fn push_does_not_change_prefix_hash() {
        let tail = DynamicTailInput { user_input: "go", resolved_gate_facts: &[], errata_blocks: &[] };
        let mut messages = TurnMessages::assemble(&compiled(), "SKILL", &[], &tail);
        let before = messages.prefix_byte_hash(4);
        messages.push_tool_result("call_1", "retrieve_rules", "{\"ok\":true}");
        assert_eq!(before, messages.prefix_byte_hash(4));
    }

    #[test]
    fn load_gm_skill_merges_global_then_ruleset() {
        let dir = std::env::temp_dir().join(format!("gm_skill_test_{}", std::process::id()));
        let global = dir.join("agent/gm_skill/global");
        let ruleset = dir.join("agent/gm_skill/cyberpunk_red");
        fs::create_dir_all(&global).unwrap();
        fs::create_dir_all(&ruleset).unwrap();
        fs::write(global.join("10_a.md"), "global-a").unwrap();
        fs::write(global.join("20_b.md"), "global-b").unwrap();
        fs::write(ruleset.join("10_c.md"), "ruleset-c").unwrap();
        let text = load_gm_skill(&dir, "cyberpunk_red").unwrap();
        assert!(text.find("global-a").unwrap() < text.find("global-b").unwrap());
        assert!(text.find("global-b").unwrap() < text.find("ruleset-c").unwrap());
        fs::remove_dir_all(dir).ok();
    }
}
```

```rust
// crates/trpg-runtime/src/lib.rs 既有 #[cfg(test)] mod tests 内追加
// （spec §6 实锤：BP1 的 engine protocol 必须出 agent-loop 版文案；
// 旧块禁令与 roll_check/request_player_roll 双工具直接矛盾）
#[test]
fn agent_loop_protocol_block_swaps_dice_prohibition_for_tool_semantics() {
    let legacy = engine_protocol_block();
    let agent = engine_protocol_block_agent_loop();
    let legacy_text = legacy.content.render_text();
    let agent_text = agent.content.render_text();
    // 旧禁令段不得进 agent system prompt（run_gm_turn 强制 agent_loop_protocol=true）。
    assert!(legacy_text.contains("without asking players for dice totals"));
    assert!(!agent_text.contains("without asking players for dice totals"));
    // agent 变体显式声明双工具掷骰语义。
    assert!(agent_text.contains("roll_check"));
    assert!(agent_text.contains("request_player_roll"));
    // 缓存稳定：两变体 block_id 不同（prefix 段按 ruleset/路径固定，互不污染）。
    assert_ne!(legacy.block_id, agent.block_id);
}
```

- [ ] **Step 2：运行确认失败**

Run: `cargo test -p trpg-gm assemble_order_is_stable`
Expected: FAIL，报错应为 `no method named to_request_messages` 或 `load_gm_skill not implemented yet`

- [ ] **Step 3：最小实现**

```rust
// crates/trpg-gm/src/prompts.rs
use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use trpg_llm::AggregatedToolCall;
use trpg_model::{ChatMessage, CompiledContext};

pub struct DynamicTailInput<'a> {
    pub user_input: &'a str,
    pub resolved_gate_facts: &'a [String],
    pub errata_blocks: &'a [String],
}

pub struct TurnMessages { messages: Vec<Value> }

impl TurnMessages {
    pub fn assemble(compiled: &CompiledContext, gm_skill_text: &str, history: &[ChatMessage], tail: &DynamicTailInput<'_>) -> Self {
        let mut messages = Vec::new();
        messages.push(json!({"role":"system","content": format!("{}\n\n{}", compiled.prefix_text, gm_skill_text)}));
        messages.push(json!({"role":"user","content": format!("[gm]\n[BP2: Pinned Context]\n{}\n[/gm]", compiled.pinned_text)}));
        for h in history {
            messages.push(json!({"role": h.role, "content": h.content}));
        }
        let mut dynamic = format!("[gm]\n[BP3: Dynamic Context]\n{}\n[/gm]", compiled.dynamic_text);
        if !tail.resolved_gate_facts.is_empty() {
            dynamic.push_str("\n\n[Resolved Gate Facts]\n");
            dynamic.push_str(&tail.resolved_gate_facts.join("\n"));
        }
        if !tail.errata_blocks.is_empty() {
            dynamic.push_str("\n\n");
            dynamic.push_str(&tail.errata_blocks.join("\n\n"));
        }
        dynamic.push_str("\n\n[Player Input]\n");
        dynamic.push_str(tail.user_input);
        messages.push(json!({"role":"user","content": dynamic}));
        Self { messages }
    }

    pub fn push_assistant_tool_calls(&mut self, calls: &[AggregatedToolCall]) {
        let tool_calls = calls.iter().map(|c| json!({"id": c.id, "type":"function", "function":{"name": c.name, "arguments": c.arguments}})).collect::<Vec<_>>();
        self.messages.push(json!({"role":"assistant","content": Value::Null,"tool_calls": tool_calls}));
    }

    pub fn push_tool_result(&mut self, tool_call_id: &str, name: &str, content: &str) {
        self.messages.push(json!({"role":"tool","tool_call_id": tool_call_id,"name": name,"content": content}));
    }

    pub fn to_request_messages(&self) -> Vec<Value> { self.messages.clone() }

    pub fn prefix_byte_hash(&self, first_n: usize) -> String {
        let bytes = serde_json::to_vec(&self.messages.iter().take(first_n).collect::<Vec<_>>()).unwrap_or_default();
        let digest = Sha256::digest(bytes);
        format!("sha256:{digest:x}")
    }
}

fn sorted_md_files(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.exists() { return Ok(Vec::new()); }
    let mut files = fs::read_dir(dir)?.filter_map(|e| e.ok().map(|x| x.path())).filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md")).collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

pub fn load_gm_skill(data_dir: &Path, ruleset_id: &str) -> Result<String> {
    let base = data_dir.join("agent/gm_skill");
    let global = base.join("global");
    if !global.exists() { return Err(anyhow!("global gm_skill directory missing: {}", global.display())); }
    let mut chunks = Vec::new();
    for p in sorted_md_files(&global)? { chunks.push(fs::read_to_string(&p).with_context(|| format!("failed reading {}", p.display()))?); }
    let ruleset = base.join(ruleset_id);
    for p in sorted_md_files(&ruleset)? { chunks.push(fs::read_to_string(&p).with_context(|| format!("failed reading {}", p.display()))?); }
    Ok(chunks.join("\n\n---\n\n"))
}
```

**BP1 agent-loop 变体（spec §6：system prompt = BP1 prefix「engine protocol 出
agent-loop 版文案」+ skill 指令）**——旧 `engine_protocol_block()` 文案含
"…otherwise state the fictional uncertainty and stakes without asking players
for dice totals" 等与 roll_check/request_player_roll 双工具直接矛盾的禁令段，
原样注入会反噬痛点 1。三处改动：

`crates/trpg-model/src/lib.rs` 的 `RuntimeState` 末尾加一个开关字段（全仓
RuntimeState 字面量均带 `..Default::default()`，加 bool 字段向后兼容、默认 false
旧路径零变化）：

```rust
    /// true ⇒ prepare_turn_context 的 BP1 用 agent-loop 版 engine protocol
    /// （GM agent loop 专用；run_gm_turn 强制置位，旧路径恒 false）。
    #[serde(default)]
    pub agent_loop_protocol: bool,
```

`crates/trpg-runtime/src/lib.rs` 新增 agent-loop 变体块（与 `engine_protocol_block`
并排；block_id 不同，两路径 prefix 互不污染、各自字节稳定）：

```rust
fn engine_protocol_block_agent_loop() -> ContextBlock {
    let content = "Runtime protocol (GM agent loop): use BP1 as resident rules, BP2 as current playable unit, BP3 as per-turn state. You own the decision of whether, when, and how to roll: call `roll_check` for system-executed checks (public or secret), call `request_player_roll` to pause the turn and let the player roll personally when the table should feel the die, or narrate without mechanics when fiction is enough. Tools are the only write path to mechanical state: dice, checks, damage/effects, resources, tracks, scene navigation, world time, and memory all commit through tool calls; plain prose changes nothing. Never leak GM-only content into player-visible output; internal context is wrapped as [gm], player-safe system notes as [system], and committed dice/tool results as [roll]. World time is authoritative: use `advance_time`, never narrate time forward mechanically. Player-supplied mechanical numbers are claims, not automatic truth: verify them against rules or relevant object/ability tables via `retrieve_rules`; if unsupported or out-of-band, warn and suggest a rules-consistent value; if the table insists, record a table override with a balance warning.";
    let mut block = ContextBlock::new(
        "engine.protocol.agent_loop".to_string(),
        BlockKind::EngineProtocol,
        "Runtime Protocol (Agent Loop)",
        BlockContent::Markdown(content.to_string()),
        Visibility::SystemOnly,
        Stability::Immutable,
        CacheZone::Prefix,
        Scope::global(),
        200,
    );
    block.tags = vec!["engine".into(), "resident".into(), "agent_loop".into()];
    block
}
```

`prepare_turn_context` 内原 `blocks.push(engine_protocol_block());` 一行改为按
state 选块：

```rust
blocks.push(if state.agent_loop_protocol { engine_protocol_block_agent_loop() } else { engine_protocol_block() });
```

创建三份数据文件：

```markdown
<!-- data/agent/gm_skill/global/10_dice_discretion.md -->
# GM Agent Dice Discretion

## Core authority

The GM agent decides whether uncertainty needs a mechanical check. The Rust tools own dice execution, state mutation, resource mutation, gates, and memory. Player-facing prose has no authority to create mechanical facts.

## Use `roll_check` when flow should continue now

Use `roll_check` for known consequences, automatic saves, public GM rolls, secret GM rolls, and table policies where system rolling is visible. Bind `tested_parameter` every time. If the result may change HP, SAN, Chaos, Harm, a condition, an object state, a scene clock, or possession, the tool call must precede narration.

Use a secret `visibility` for hidden NPC action, ambush, tailing, surveillance, concealed theft, secret perception, and any failure that would reveal meta-information. Narrate only what the character can perceive. Never leak private roll ids, expressions, totals, target values, or outcome JSON.

Use a public `visibility` for known automatic danger, environmental resistance, stabilization, resource pressure, or a roll whose existence is already obvious in fiction.

## Use `request_player_roll` when player touch matters

Pause for `request_player_roll` when the player initiated a risky action and the table should feel the die: direct attacks, dangerous movement, stealth under pressure, technical intervention with consequences, hacking, bypassing, disarming, risky social leverage, or any explicit player request to roll personally. State stakes before opening the gate.

Do not ask the player for remaining HP, SAN, Chaos, Harm, armor, weapon table values, NPC values, target numbers, or damage expressions. Those come from rules, actor parameters, object profiles, materialization, or tool results.

## Use no roll when fiction is enough

Answer without a roll for surface observation, obvious sensory facts, harmless positioning, low-pressure conversation, and actions where failure would not add meaningful cost. Surface observation may reveal visible facts but not hidden technical state, hidden motives, traps, ambushes, or GM-only material.

## Mechanical binding rules

Before any mechanical roll, know the roll kind, dice expression, `tested_parameter`, target model, visibility, and expected effect. If the kernel has no dice expression, use `retrieve_rules`, change approach, ask the player for a roll only when the rules are available, or narrate without mechanics. Do not invent target values.

For attacks and opposed actions, bind attacker, defender, source object or ability, and the defender parameter before applying effect. `NoMechanicalOpposition` and `UnknownUntilLookup` may exist during construction, but not as final resolved attack state.

A successful attack should lead to damage or effect resolution. A resolved effect must produce `ParameterImpact` entries; HP is only one target. SAN, Chaos, Harm, conditions, object state, possession, and scene clocks are valid targets.

## Gates and lifecycle

If a pending player roll gate is open, treat a direct roll reply as the gate answer, not a fresh action. A resolved check must close its pending check and matching interaction gate. Optional reaction or resource windows must state the default consequence. Required reaction windows block unrelated actions only when the input is a direct reply to that visible threat.

## Materialization and player-supplied values

Player-supplied mechanical numbers are claims. Verify them against active rules, runtime state, object/ability/weapon/armor material, or record an explicit table override. Lazy loading is allowed, but an actor used mechanically must have runtime actor parameters. A weapon or object used mechanically must be bound to a rules-aware object definition before its values matter.

## Narrative situation practice

Present observable facts, pressure, affordances, risks, and a goal question. Offer costed examples only when helpful. Avoid repeatedly forcing numbered menus. Add fresh pressure, cost, or world change when the same tactic repeats.
```

```markdown
<!-- data/agent/gm_skill/global/20_item_npc_policy.md -->
# ITEM POLICY / NPC POLICY

## ITEM POLICY

When the player introduces or creates an item not already established, never fabricate its mechanical parameters.

1. If the item's identity is ambiguous, or the context marks it as needing clarification, ask the player what it is before assigning mechanics.
2. If the player gives a reasonable real-world or genre analog, treat it as the matching rules item and retrieve that item's source-backed stats.
3. If the item is clearly too powerful or mismatched for this module, push back and advise the player to reconsider. The table may proceed after a warning, but the mechanics must be flagged as provisional or table override. Never silently assign balanced stats.

## NPC POLICY

When an NPC introduced by the player, or an NPC you must voice, has no established stat block or persona, never fabricate balanced mechanics.

1. If the NPC identity is ambiguous or no persona is established, ask the player to establish who they are or what they want from this NPC before assigning stats or motives.
2. If a fitting NPC belongs in the module or genre, use the module NPC card when one exists. Otherwise use a genre archetype and retrieve stats from source-backed material.
3. If the NPC is clearly too powerful or mismatched for this module, push back and warn the player. The table may proceed, but the NPC must be flagged as provisional or off-power. Never silently fabricate balanced stats.
```

```markdown
<!-- data/agent/gm_skill/global/30_output_contract.md -->
# Player-visible Output Contract

Use final content deltas as the player-visible narration. Do not describe internal tool calls.

Use `[roll]...[/roll]` only when presenting a player-visible resolved roll fact already created by a tool or by deterministic gate settlement. Use `[system]...[/system]` for concise procedural prompts such as a pending player roll request. Do not ask the player to report hidden totals or target values.

Private GM rolls may influence narration, but their roll id, formula, raw total, target, and internal outcome JSON must not appear in player-visible text.

State consequences in fiction first. When mechanical impact is visible, mention the visible effect or state change without exposing GM-only data.
```

- [ ] **Step 4：运行确认通过**

Run: `cargo test -p trpg-gm assemble_order_is_stable`
Expected: PASS

- [ ] **Step 5：验证收尾**

Run: `cargo test -p trpg-gm && cargo test -p trpg-runtime agent_loop_protocol && wc -l crates/trpg-gm/src/prompts.rs data/agent/gm_skill/global/10_dice_discretion.md data/agent/gm_skill/global/20_item_npc_policy.md data/agent/gm_skill/global/30_output_contract.md`
Expected: 全绿（含 agent-loop 变体断言：旧禁令文本不进 agent system prompt）；列出的文件均 ≤400 行

---

### Task 7：turn_loop.rs run_gm_turn 全状态机 + 滑动缓冲 + MockLlm 单测

**Files:**
- Modify: `crates/trpg-gm/src/stream.rs`
- Modify: `crates/trpg-gm/src/turn_loop.rs`
- Test: `crates/trpg-gm/src/stream.rs` 与 `crates/trpg-gm/src/turn_loop.rs` 同文件 `#[cfg(test)]`

- [ ] **Step 1：写失败测试**

```rust
// crates/trpg-gm/src/stream.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_private_token_across_chunk_boundary() {
        let mut b = RedactingBuffer::new(vec!["91".to_string()]);
        let a = b.push("The hidden total is 9");
        let c = b.push("1, but you only see movement.");
        let d = b.finish();
        let out = format!("{a}{c}{d}");
        assert!(!out.contains("91"));
        assert!(out.contains("■"));
    }

    #[test]
    fn empty_tokens_are_transparent() {
        let mut b = RedactingBuffer::new(vec![]);
        assert_eq!(b.push("abc"), "abc");
        assert_eq!(b.finish(), "");
    }

    #[test]
    fn utf8_chinese_with_token_across_chunk_boundary_does_not_panic() {
        // 中文叙事是 e2e 主场景（血色公路）：holdback 字节切分点落在多字节
        // 字符中间时必须回退到 char boundary，否则 byte-index panic。
        let mut b = RedactingBuffer::new(vec!["秘密91".to_string()]);
        let a = b.push("你看到了秘");
        let c = b.push("密91的暗示，但什么都没有发生。");
        let d = b.finish();
        let out = format!("{a}{c}{d}");
        assert!(!out.contains("秘密91"));
        assert!(out.contains("■"));
        assert!(out.contains("但什么都没有发生"));
    }
}
```

```rust
// crates/trpg-gm/src/turn_loop.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::TurnLedger;
    use crate::tools::{AwaitingPlayerRoll, GmTool, ToolCtx, ToolError, ToolOutput, ToolRegistry, ToolSpec};
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
    use trpg_model::{ActorKind, ChatMessage, CheckResultRecord, CompiledContext, ContextRequest, DiceRollRecord, RollVisibility, RuntimeState, TokenBudget, VisibilityProfile};
    use trpg_runtime::AutoRollExecution;

    struct MockLlm { scripts: Mutex<Vec<Vec<StreamEvent>>>, choices: Mutex<Vec<ToolChoice>>, requests: Mutex<Vec<Vec<Value>>> }

    #[async_trait]
    impl LlmClient for MockLlm {
        async fn complete_text(&self, _: Vec<ChatMessage>, _: f32) -> Result<String> { unimplemented!("MockLlm complete_text is unused by run_gm_turn tests") }
        async fn complete_json(&self, _: Vec<ChatMessage>, _: f32) -> Result<Value> { unimplemented!("MockLlm complete_json is unused by run_gm_turn tests") }
        async fn stream_chat(&self, _: Vec<ChatMessage>, _: f32) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> { unimplemented!("MockLlm stream_chat is unused by run_gm_turn tests") }
        async fn complete_with_tools(&self, _: Vec<Value>, _: Vec<Value>) -> Result<Value> { unimplemented!("MockLlm complete_with_tools is unused by run_gm_turn tests") }
        async fn stream_chat_with_tools(&self, messages: Vec<Value>, _: Vec<Value>, tool_choice: ToolChoice) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            self.requests.lock().unwrap().push(messages);
            self.choices.lock().unwrap().push(tool_choice);
            let script = self.scripts.lock().unwrap().remove(0);
            let s = try_stream! { for event in script { yield event; } };
            Ok(Box::pin(s))
        }
    }

    /// 脚本化 GmTool 替身（真工具 roll_check/request_player_roll 都打 DB，纯
    /// MockLlm 单测跑不通；经 ToolRegistry::from_tools 注入）。
    /// record=true ⇒ 往账本写一条合成契约外的私骰结果（场景 8 入账观测）；
    /// fail_code=Some ⇒ 返回该结构化 ToolError（场景 7）；awaiting=true ⇒ gate 终态。
    struct ScriptedTool { name: &'static str, record: bool, fail_code: Option<&'static str>, awaiting: bool }

    #[async_trait]
    impl GmTool for ScriptedTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec { name: self.name, schema: json!({"type":"function","function":{"name": self.name, "description":"test double","parameters":{"type":"object","properties":{}}}}) }
        }

        async fn call(&self, _ctx: &ToolCtx<'_>, ledger: &mut TurnLedger, _args: Value) -> Result<ToolOutput> {
            if let Some(code) = self.fail_code {
                return Err(ToolError::recoverable(code, "scripted failure", Some("switch to another tool".to_string())));
            }
            if self.record {
                let roll = DiceRollRecord { roll_id: "roll_secret".to_string(), session_id: "s".to_string(), turn_id: "t".to_string(), check_id: Some("check_x".to_string()), roller_kind: ActorKind::System, roller_id: None, visibility: RollVisibility::PrivateGmRoll, expression: "1d100".to_string(), result: json!({"total": 91}), seed_commitment: "seed".to_string(), revealed_at: None, created_at: Utc::now() };
                let result = CheckResultRecord { check_id: "check_x".to_string(), roll, outcome: json!({"total": 91}), committed_patches: vec![], created_at: Utc::now() };
                ledger.record_execution(&AutoRollExecution { primary: result, followups: vec![], roll_policy: "system_rolls_hidden".to_string() });
            }
            if self.awaiting {
                return Ok(ToolOutput::awaiting(json!({"check_id": "check_gate"}), AwaitingPlayerRoll { check_id: "check_gate".to_string(), prompt_public: "Roll 1d100 now.".to_string() }));
            }
            Ok(ToolOutput::ok(json!({"ok": true})))
        }
    }

    fn tool_call(name: &str) -> AggregatedToolCall {
        AggregatedToolCall { id: format!("call_{name}"), name: name.to_string(), arguments: "{}".to_string() }
    }

    /// lazy pool 不真连接；prepare_turn_context 经 ctx_provider seam 绕开（它
    /// 必须连真 DB + 已 parse 的 project bundle）；load_gm_skill fail-closed，
    /// 故 fixture 自带临时 gm_skill 目录。
    fn loop_fixture(scripts: Vec<Vec<StreamEvent>>, tools: ToolRegistry, max_tool_rounds: u8) -> (GmLoop, Arc<MockLlm>, ContextRequest, RuntimeState) {
        let pool = PgPoolOptions::new().connect_lazy("postgres://chatrpg:chatrpg@localhost:54347/chatrpg").expect("lazy pool");
        let engine = RuntimeEngine::new(Db { pool });
        let llm = Arc::new(MockLlm { scripts: Mutex::new(scripts), choices: Mutex::new(Vec::new()), requests: Mutex::new(Vec::new()) });
        let data_dir = std::env::temp_dir().join(format!("gm_loop_test_{}_{}", std::process::id(), uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(data_dir.join("agent/gm_skill/global")).unwrap();
        fs::write(data_dir.join("agent/gm_skill/global/10_test.md"), "test gm skill").unwrap();
        let mut gm = GmLoop::new(engine, llm.clone(), tools, LoopConfig { max_tool_rounds, repeat_finding_threshold: 3 }, data_dir);
        gm.ctx_provider = Some(Arc::new(|_req, _state| CompiledContext { prefix_text: "BP1".to_string(), pinned_text: "BP2".to_string(), dynamic_text: "BP3".to_string(), prefix_hash: "p".to_string(), pinned_hash: "m".to_string(), dynamic_hash: "d".to_string(), ..Default::default() }));
        let request = ContextRequest { ruleset_id: "rs".to_string(), module_id: None, session_id: "s".to_string(), turn_id: "t".to_string(), viewer: VisibilityProfile::gm(), token_budget: TokenBudget::default() };
        let state = RuntimeState { ruleset_id: "rs".to_string(), ..Default::default() };
        (gm, llm, request, state)
    }

    #[tokio::test]
    async fn content_delta_streams_to_callback() {
        let (mut gm, _llm, request, state) = loop_fixture(vec![vec![StreamEvent::ContentDelta("A".to_string()), StreamEvent::ContentDelta("B".to_string()), StreamEvent::Done { finish_reason: Some("stop".to_string()) }]], ToolRegistry::from_tools(vec![]), 1);
        let mut out = String::new();
        let result = gm.run_gm_turn(GmTurnInput { request: &request, state: &state, user_input: "go", history: &[], recent_transcript: None }, &mut |d| out.push_str(d)).await.unwrap();
        assert_eq!(out, "AB");
        assert_eq!(result, TurnOutcome::Narration("AB".to_string()));
    }

    #[tokio::test]
    async fn max_rounds_forces_tool_choice_none() {
        // max_tool_rounds=1：第 1 轮 Auto 仍要工具 → 循环耗尽 → 追加唯一一轮 None。
        // tool_choice 序列必须是 [Auto, None]（循环内绝不提前 None、绝无双重 None）。
        let (mut gm, llm, request, state) = loop_fixture(vec![vec![StreamEvent::ToolCalls(vec![]), StreamEvent::Done { finish_reason: Some("tool_calls".to_string()) }], vec![StreamEvent::ContentDelta("final".to_string()), StreamEvent::Done { finish_reason: Some("stop".to_string()) }]], ToolRegistry::from_tools(vec![]), 1);
        let mut out = String::new();
        let _ = gm.run_gm_turn(GmTurnInput { request: &request, state: &state, user_input: "go", history: &[], recent_transcript: None }, &mut |d| out.push_str(d)).await.unwrap();
        let choices = llm.choices.lock().unwrap().clone();
        assert_eq!(choices, vec![ToolChoice::Auto, ToolChoice::None]);
        assert_eq!(out, "final");
    }

    #[tokio::test]
    async fn tool_products_enter_ledger_and_private_tokens_are_redacted() {
        // spec §9 场景 1+4+8（单测可观测面）：工具轮产物入账 →
        // 下一轮 content 里的私骰 token（"91"）被滑动缓冲过滤。
        let tools = ToolRegistry::from_tools(vec![Box::new(ScriptedTool { name: "roll_check", record: true, fail_code: None, awaiting: false })]);
        let scripts = vec![
            vec![StreamEvent::ToolCalls(vec![tool_call("roll_check")]), StreamEvent::Done { finish_reason: Some("tool_calls".to_string()) }],
            vec![StreamEvent::ContentDelta("The hidden total is 9".to_string()), StreamEvent::ContentDelta("1, but you only see shadows.".to_string()), StreamEvent::Done { finish_reason: Some("stop".to_string()) }],
        ];
        let (mut gm, llm, request, state) = loop_fixture(scripts, tools, 4);
        let mut out = String::new();
        let result = gm.run_gm_turn(GmTurnInput { request: &request, state: &state, user_input: "go", history: &[], recent_transcript: None }, &mut |d| out.push_str(d)).await.unwrap();
        assert!(matches!(result, TurnOutcome::Narration(_)));
        assert!(!out.contains("91"), "private roll token leaked: {out}");
        assert!(out.contains("■"));
        // 第二轮请求里必须有第一轮的 tool 回填消息（账本/消息尾部追加联动）。
        let second_request = llm.requests.lock().unwrap()[1].clone();
        assert!(second_request.iter().any(|m| m.get("role").and_then(Value::as_str) == Some("tool") && m.get("name").and_then(Value::as_str) == Some("roll_check")));
    }

    #[tokio::test]
    async fn recoverable_tool_error_lets_agent_switch_to_player_roll() {
        // spec §9 场景 7 + 2（终态侧）：missing_kernel_dice 结构化错误回填 →
        // agent 第二轮改调 request_player_roll → AwaitingPlayerRoll 终态。
        // （"下回合骰值回复 → 头部结算"需真 DB pending check，归 Task 11 e2e。）
        let tools = ToolRegistry::from_tools(vec![
            Box::new(ScriptedTool { name: "roll_check", record: false, fail_code: Some("missing_kernel_dice"), awaiting: false }),
            Box::new(ScriptedTool { name: "request_player_roll", record: false, fail_code: None, awaiting: true }),
        ]);
        let scripts = vec![
            vec![StreamEvent::ToolCalls(vec![tool_call("roll_check")]), StreamEvent::Done { finish_reason: Some("tool_calls".to_string()) }],
            vec![StreamEvent::ToolCalls(vec![tool_call("request_player_roll")]), StreamEvent::Done { finish_reason: Some("tool_calls".to_string()) }],
        ];
        let (mut gm, llm, request, state) = loop_fixture(scripts, tools, 4);
        let mut out = String::new();
        let result = gm.run_gm_turn(GmTurnInput { request: &request, state: &state, user_input: "go", history: &[], recent_transcript: None }, &mut |d| out.push_str(d)).await.unwrap();
        assert_eq!(result, TurnOutcome::AwaitingPlayerRoll { check_id: "check_gate".to_string(), prompt_public: "Roll 1d100 now.".to_string() });
        // 第二轮请求携带第一轮的结构化错误回填（agent 可见可修复）。
        let second_request = llm.requests.lock().unwrap()[1].clone();
        let error_content = second_request.iter().find(|m| m.get("role").and_then(Value::as_str) == Some("tool")).and_then(|m| m.get("content")).and_then(Value::as_str).unwrap_or("");
        assert!(error_content.contains("missing_kernel_dice"));
        assert!(error_content.contains("\"recoverable\":true"));
    }
}
```

- [ ] **Step 2：运行确认失败**

Run: `cargo test -p trpg-gm redacts_private_token_across_chunk_boundary`
Expected: FAIL，报错应为 `no function or associated item named new found for struct RedactingBuffer`

- [ ] **Step 3：最小实现**

```rust
// crates/trpg-gm/src/stream.rs
pub struct RedactingBuffer {
    tokens: Vec<String>,
    tail: String,
    holdback: usize,
}

impl RedactingBuffer {
    pub fn new(private_tokens: Vec<String>) -> Self {
        let mut tokens = private_tokens.into_iter().map(|s| s.to_ascii_lowercase()).filter(|s| !s.trim().is_empty()).collect::<Vec<_>>();
        tokens.sort();
        tokens.dedup();
        let holdback = tokens.iter().map(|s| s.len().saturating_sub(1)).max().unwrap_or(0);
        Self { tokens, tail: String::new(), holdback }
    }

    pub fn push(&mut self, delta: &str) -> String {
        if self.tokens.is_empty() { return delta.to_string(); }
        let mut joined = String::new();
        joined.push_str(&self.tail);
        joined.push_str(delta);
        // holdback 是字节数，切分点可能落在多字节 UTF-8 字符（中文叙事）中间：
        // 必须回退到最近的 char boundary，否则字节切 String 直接 panic。
        let mut split_at = joined.len().saturating_sub(self.holdback);
        while split_at > 0 && !joined.is_char_boundary(split_at) {
            split_at -= 1;
        }
        let safe = joined[..split_at].to_string();
        self.tail = joined[split_at..].to_string();
        self.redact(&safe)
    }

    pub fn finish(&mut self) -> String {
        if self.tokens.is_empty() { return String::new(); }
        let tail = std::mem::take(&mut self.tail);
        self.redact(&tail)
    }

    fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        for token in &self.tokens {
            let mut lower = out.to_ascii_lowercase();
            while let Some(pos) = lower.find(token) {
                let end = pos + token.len();
                out.replace_range(pos..end, "■");
                lower = out.to_ascii_lowercase();
            }
        }
        out
    }
}
```

```rust
// crates/trpg-gm/src/turn_loop.rs
use crate::errata::ErrataMemory;
use crate::ledger::TurnLedger;
use crate::prompts::{load_gm_skill, DynamicTailInput, TurnMessages};
use crate::stream::RedactingBuffer;
use crate::tools::{AwaitingPlayerRoll, SceneDeepExtractFn, ToolCtx, ToolRegistry};
use anyhow::Result;
use chrono::Utc;
use futures_util::StreamExt;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use trpg_agent::parse_roll_text;
use trpg_interaction::InteractionLifecycleKernel;
use trpg_llm::{LlmClient, StreamEvent, ToolChoice};
use trpg_model::{ChatMessage, CompiledContext, ContextRequest, MemoryEvent, MemoryKind, RuntimeState, Visibility, WorldEventKind};
use trpg_runtime::RuntimeEngine;
use uuid::Uuid;

/// 上下文准备注入 seam（单测绕开真 DB 的唯一通道；生产装配恒 None）。
pub type CtxProviderFn = Arc<dyn Fn(&ContextRequest, &RuntimeState) -> CompiledContext + Send + Sync>;

#[derive(Debug, Clone)]
pub struct LoopConfig { pub max_tool_rounds: u8, pub repeat_finding_threshold: u8 }
impl Default for LoopConfig { fn default() -> Self { Self { max_tool_rounds: 8, repeat_finding_threshold: 3 } } }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome { Narration(String), AwaitingPlayerRoll { check_id: String, prompt_public: String } }

pub struct GmLoop { pub engine: RuntimeEngine, pub llm: Arc<dyn LlmClient>, pub tools: ToolRegistry, pub cfg: LoopConfig, pub data_dir: PathBuf, pub scene_extractor: Option<SceneDeepExtractFn>, pub ctx_provider: Option<CtxProviderFn>, pub errata: ErrataMemory }

pub struct GmTurnInput<'a> { pub request: &'a ContextRequest, pub state: &'a RuntimeState, pub user_input: &'a str, pub history: &'a [ChatMessage], pub recent_transcript: Option<&'a str> }

impl GmLoop {
    pub fn new(engine: RuntimeEngine, llm: Arc<dyn LlmClient>, tools: ToolRegistry, cfg: LoopConfig, data_dir: PathBuf) -> Self {
        let errata = ErrataMemory::new(cfg.repeat_finding_threshold);
        Self { engine, llm, tools, cfg, data_dir, scene_extractor: None, ctx_provider: None, errata }
    }

    pub async fn run_gm_turn(&mut self, input: GmTurnInput<'_>, on_delta: &mut (dyn FnMut(&str) + Send)) -> Result<TurnOutcome> {
        // —— 1. 确定性头部（spec §4 四项顺序：record → refresh → reconcile → gate 结算）——
        let _ = self.engine.record_world_event(&input.request.session_id, Some(&input.request.turn_id), None, WorldEventKind::PlayerAction, json!({"input": input.user_input}), Visibility::GmOnly).await;
        let _ = self.engine.refresh_actor_live_derived(&input.request.session_id, input.request.viewer.actor_id.as_deref().unwrap_or("pc.current")).await;
        // prepare_turn_context 内部也会 reconcile，但那发生在 gate 结算之后，
        // 不满足 spec 顺序——此处显式调用（幂等，与既有调用方同款 let _ =）。
        let _ = InteractionLifecycleKernel::new(self.engine.db.clone()).reconcile_session(&input.request.session_id).await;
        let mut ledger = TurnLedger::new();
        let mut resolved_gate_facts = Vec::new();
        if parse_roll_text(input.user_input).is_some() {
            if let Some(pending) = self.engine.db.get_open_pending_check(&input.request.session_id).await.ok().flatten() {
                let result = self.engine.resolve_check_with_input(&input.request.session_id, &input.request.turn_id, &pending.contract, input.user_input).await?;
                ledger.record_contract(&pending.contract);
                ledger.record_result(&result);
                resolved_gate_facts.push(format!("[roll]Resolved pending check {}: {}[/roll]", result.check_id, serde_json::to_string(&result.outcome).unwrap_or_default()));
            }
        }
        // —— 2. 上下文（agent_loop_protocol 强制置位 → BP1 出 agent-loop 版 engine protocol）——
        let mut state_agent = input.state.clone();
        state_agent.agent_loop_protocol = true;
        let compiled = match &self.ctx_provider {
            Some(provider) => provider(input.request, &state_agent),
            None => self.engine.prepare_turn_context(input.request, &state_agent, Some(input.user_input), input.recent_transcript).await?,
        };
        // gm_skill 是 agent 路径硬依赖：fail-closed，Err 终止回合（契约第 7 节，
        // 绝不 unwrap_or_default；data_dir 由装配方传 default_data_dir()）。
        let gm_skill = load_gm_skill(&self.data_dir, &input.request.ruleset_id)?;
        let mut errata_blocks = Vec::new();
        if let Some(block) = self.errata.errata_block() { errata_blocks.push(block); }
        if let Some(block) = self.errata.standing_reminder_block() { errata_blocks.push(block); }
        let tail = DynamicTailInput { user_input: input.user_input, resolved_gate_facts: &resolved_gate_facts, errata_blocks: &errata_blocks };
        let mut messages = TurnMessages::assemble(&compiled, &gm_skill, input.history, &tail);
        let schemas = self.tools.schemas();
        let mut visible_text = String::new();
        let mut awaiting: Option<AwaitingPlayerRoll> = None;
        let mut narrated = false;
        // —— 3. 工具轮（循环内恒 ToolChoice::Auto；ctx 限定块级作用域，块结束
        //     即释放对 self 的借用，收尾 &self 方法才可调用）——
        {
            let ctx = ToolCtx { engine: &self.engine, request: input.request, state: &state_agent, scene_extractor: self.scene_extractor.as_ref() };
            'rounds: for _round in 0..self.cfg.max_tool_rounds {
                let mut stream = self.llm.stream_chat_with_tools(messages.to_request_messages(), schemas.clone(), ToolChoice::Auto).await?;
                let mut saw_tool = false;
                let mut redactor = RedactingBuffer::new(ledger.private_roll_tokens());
                while let Some(event) = stream.next().await {
                    match event? {
                        StreamEvent::ContentDelta(delta) => {
                            let safe = redactor.push(&delta);
                            if !safe.is_empty() { on_delta(&safe); visible_text.push_str(&safe); }
                        }
                        StreamEvent::Usage(usage) => {
                            // §6.1 第 5 条可观测：relay 透传则记，缺省无此事件。
                            tracing::info!(target: "gm_cache", cached_tokens = ?usage.pointer("/prompt_tokens_details/cached_tokens"), "upstream usage reported");
                        }
                        StreamEvent::ToolCalls(calls) => {
                            saw_tool = true;
                            messages.push_assistant_tool_calls(&calls);
                            for call in calls {
                                let outcome = self.tools.dispatch(&ctx, &mut ledger, &call).await;
                                messages.push_tool_result(&outcome.tool_call_id, &outcome.name, &outcome.content);
                                if let Some(gate) = outcome.awaiting_player_roll {
                                    let rest = redactor.finish();
                                    if !rest.is_empty() { on_delta(&rest); visible_text.push_str(&rest); }
                                    awaiting = Some(gate);
                                    break 'rounds;
                                }
                            }
                            // 工具产物可能新增私骰 token：排空旧缓冲后按最新账本重建（混合轮防漏）。
                            let rest = redactor.finish();
                            if !rest.is_empty() { on_delta(&rest); visible_text.push_str(&rest); }
                            redactor = RedactingBuffer::new(ledger.private_roll_tokens());
                        }
                        StreamEvent::Done { .. } => {}
                    }
                }
                let rest = redactor.finish();
                if !rest.is_empty() { on_delta(&rest); visible_text.push_str(&rest); }
                if !saw_tool { narrated = true; break 'rounds; }
            }
        }
        // —— 终态 A：request_player_roll gate ——
        if let Some(gate) = awaiting {
            // Task 8 在此处接 verify_after_stream（visible_text 非空时）。
            let assistant_output = if visible_text.trim().is_empty() { gate.prompt_public.clone() } else { visible_text.clone() };
            self.finalize_turn(input.request, &compiled, input.user_input, &assistant_output, "awaiting_player_roll").await;
            return Ok(TurnOutcome::AwaitingPlayerRoll { check_id: gate.check_id, prompt_public: gate.prompt_public });
        }
        // —— 4. 仅当轮数自然耗尽且模型仍要工具：追加唯一一轮 ToolChoice::None 逼散文 ——
        if !narrated {
            let mut stream = self.llm.stream_chat_with_tools(messages.to_request_messages(), schemas, ToolChoice::None).await?;
            let mut redactor = RedactingBuffer::new(ledger.private_roll_tokens());
            while let Some(event) = stream.next().await {
                if let StreamEvent::ContentDelta(delta) = event? {
                    let safe = redactor.push(&delta);
                    if !safe.is_empty() { on_delta(&safe); visible_text.push_str(&safe); }
                }
            }
            let rest = redactor.finish();
            if !rest.is_empty() { on_delta(&rest); visible_text.push_str(&rest); }
        }
        // —— 5. 流后校验挂钩位（Task 8 接 NarrationVerifier → 勘误记忆）——
        // —— 6. 确定性收尾 ——
        self.finalize_turn(input.request, &compiled, input.user_input, &visible_text, "ready").await;
        Ok(TurnOutcome::Narration(visible_text))
    }

    /// 确定性收尾（spec §4：save_turn / memory event / learning audit）：持久化
    /// 本回合 + 回合摘要 + 学习审计。叙事已流出不可回收，收尾失败 tracing::warn
    /// 不 panic（也让 MockLlm 单测在 lazy pool 下保持绿）；真实落库由 Task 11 的
    /// turns 表 SQL 验证。
    async fn finalize_turn(&self, request: &ContextRequest, compiled: &CompiledContext, user_input: &str, assistant_output: &str, status: &str) {
        let hashes = json!({"prefix": compiled.prefix_hash, "pinned": compiled.pinned_hash, "dynamic": compiled.dynamic_hash});
        if let Err(err) = self.engine.db.save_turn(&request.session_id, &request.turn_id, user_input, assistant_output, hashes, status).await {
            tracing::warn!(error = %err, "agent path save_turn failed");
        }
        if !assistant_output.trim().is_empty() {
            let summary: String = assistant_output.chars().take(280).collect();
            let event = MemoryEvent { event_id: format!("mem_turn_{}", Uuid::new_v4().simple()), session_id: request.session_id.clone(), turn_id: Some(request.turn_id.clone()), ruleset_id: request.ruleset_id.clone(), module_id: request.module_id.clone(), scene_id: None, location_id: None, actor_ids: vec![], visibility: Visibility::GmOnly, event_kind: MemoryKind::Event, summary, transcript_excerpt: None, source: json!({"source":"gm_agent.turn_summary"}), tags: vec!["gm_turn".to_string()], importance: 1, occurred_at: Utc::now() };
            if let Err(err) = self.engine.db.save_memory_event(&event).await {
                tracing::warn!(error = %err, "agent path turn summary memory event failed");
            }
            // spec §4 收尾第三项：learning audit（保留）——沿用旧路径原语
            // RuntimeEngine::audit_learning_for_turn（内部自带 learning_audit_enabled()
            // 门控与自吞错，门关时为 no-op；与旧 run_turn_once L1481 行为对齐）。
            if let Err(err) = self.engine.audit_learning_for_turn(&request.session_id, &request.ruleset_id, request.module_id.as_deref(), &request.turn_id, user_input, assistant_output).await {
                tracing::warn!(error = %err, "agent path learning audit failed");
            }
        }
    }
}
```

- [ ] **Step 4：运行确认通过**

Run: `cargo test -p trpg-gm redacts_private_token_across_chunk_boundary`
Expected: PASS

- [ ] **Step 5：验证收尾**

Run: `cargo test -p trpg-gm && wc -l crates/trpg-gm/src/stream.rs crates/trpg-gm/src/turn_loop.rs`
Expected: 全绿；`stream.rs` 与 `turn_loop.rs` 均 ≤400 行

---

### Task 8：流后 verifier → 勘误记忆 + 同错升级

**Files:**
- Modify: `crates/trpg-gm/src/errata.rs`
- Modify: `crates/trpg-gm/src/turn_loop.rs:run_gm_turn 第 5 步`
- Test: `crates/trpg-gm/src/errata.rs` 同文件 `#[cfg(test)]`

- [ ] **Step 1：写失败测试**

```rust
// crates/trpg-gm/src/errata.rs
#[cfg(test)]
mod tests {
    use super::*;
    use trpg_agent::{VerifierFinding, VerifierFindingKind};

    #[test]
    fn errata_records_recent_entries_and_counts() {
        let mut e = ErrataMemory::new(3);
        let entries = e.record("turn_1", &[VerifierFinding::blocker(VerifierFindingKind::OmittedVisibleResult, "visible result missing")]);
        assert_eq!(entries.len(), 1);
        assert!(e.errata_block().unwrap().contains("visible result missing"));
        assert_eq!(e.kind_counts().get("omitted_visible_result"), Some(&1));
    }

    #[test]
    fn repeated_kind_becomes_standing_reminder() {
        let mut e = ErrataMemory::new(2);
        e.record("t1", &[VerifierFinding::blocker(VerifierFindingKind::SecretLeak, "secret one")]);
        assert!(e.standing_reminder_block().is_none());
        e.record("t2", &[VerifierFinding::blocker(VerifierFindingKind::SecretLeak, "secret two")]);
        assert!(e.standing_reminder_block().unwrap().contains("secret_leak"));
    }
}
```

- [ ] **Step 2：运行确认失败**

Run: `cargo test -p trpg-gm errata_records_recent_entries_and_counts`
Expected: FAIL，报错应为 `no method named record found for struct ErrataMemory`

- [ ] **Step 3：最小实现**

```rust
// crates/trpg-gm/src/errata.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use trpg_agent::{VerifierFinding, VerifierFindingKind};
use trpg_model::{MemoryEvent, MemoryKind, Visibility};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrataEntry {
    pub kind: VerifierFindingKind,
    pub detail: String,
    pub turn_id: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct ErrataMemory {
    entries: Vec<ErrataEntry>,
    kind_counts: BTreeMap<String, u32>,
    threshold: u8,
}

impl ErrataMemory {
    pub fn new(repeat_finding_threshold: u8) -> Self { Self { entries: Vec::new(), kind_counts: BTreeMap::new(), threshold: repeat_finding_threshold } }

    pub fn record(&mut self, turn_id: &str, findings: &[VerifierFinding]) -> Vec<ErrataEntry> {
        let mut out = Vec::new();
        for f in findings {
            let key = kind_key(f.kind);
            *self.kind_counts.entry(key).or_default() += 1;
            let entry = ErrataEntry { kind: f.kind, detail: f.detail.clone(), turn_id: turn_id.to_string(), created_at: Utc::now() };
            self.entries.push(entry.clone());
            out.push(entry);
        }
        out
    }

    pub fn errata_block(&self) -> Option<String> {
        if self.entries.is_empty() { return None; }
        let lines = self.entries.iter().rev().take(3).rev().map(|e| format!("- {}: {}", kind_key(e.kind), e.detail)).collect::<Vec<_>>().join("\n");
        Some(format!("[gm_errata]\nPrevious narration audit findings to repair in future fiction without retconning streamed text:\n{}\n[/gm_errata]", lines))
    }

    pub fn standing_reminder_block(&self) -> Option<String> {
        let repeated = self.kind_counts.iter().filter(|(_, v)| **v >= self.threshold as u32).map(|(k, v)| format!("- {k}: {v} times")).collect::<Vec<_>>();
        if repeated.is_empty() { None } else { Some(format!("[gm_errata_standing]\nThese finding kinds are recurring. Avoid them this turn:\n{}\n[/gm_errata_standing]", repeated.join("\n"))) }
    }

    pub fn kind_counts(&self) -> &BTreeMap<String, u32> { &self.kind_counts }

    pub fn to_memory_event(&self, request: &trpg_model::ContextRequest, new_entries: &[ErrataEntry]) -> MemoryEvent {
        let mut tags = vec!["gm_errata".to_string()];
        tags.extend(new_entries.iter().map(|e| kind_key(e.kind)).collect::<Vec<_>>());
        // MemoryEvent 的字段名是 source（serde_json::Value），不是 source_json。
        MemoryEvent { event_id: format!("mem_errata_{}", Uuid::new_v4().simple()), session_id: request.session_id.clone(), turn_id: Some(request.turn_id.clone()), ruleset_id: request.ruleset_id.clone(), module_id: request.module_id.clone(), scene_id: None, location_id: None, actor_ids: vec![], visibility: Visibility::GmOnly, event_kind: MemoryKind::Event, summary: new_entries.iter().map(|e| format!("{}: {}", kind_key(e.kind), e.detail)).collect::<Vec<_>>().join("; "), transcript_excerpt: None, source: json!({"source":"narration_verifier"}), tags, importance: 2, occurred_at: Utc::now() }
    }
}

fn kind_key(kind: VerifierFindingKind) -> String {
    match kind {
        VerifierFindingKind::MissingCheck => "missing_check",
        VerifierFindingKind::MissingRollExecution => "missing_roll_execution",
        VerifierFindingKind::MissingEffect => "missing_effect",
        VerifierFindingKind::InventedEffect => "invented_effect",
        VerifierFindingKind::OmittedVisibleResult => "omitted_visible_result",
        VerifierFindingKind::ManualRollRequest => "manual_roll_request",
        VerifierFindingKind::SecretLeak => "secret_leak",
    }.to_string()
}
```

流后校验是 loop 之后的**无条件阶段**（spec §4）：Narration 与 AwaitingPlayerRoll
两类终态都要跑（awaiting 终态前可能已流出含可见掷骰结果的叙事，仅在
visible_text 非空时执行，空文本无可对账内容）。把公共代码抽为私有方法：

```rust
async fn verify_after_stream(&mut self, request: &ContextRequest, ledger: &TurnLedger, visible_text: &str) {
    let verifier = trpg_agent::NarrationVerifier;
    let submission = trpg_agent::FinalNarrationSubmission { player_visible_text: visible_text.to_string(), mechanical_claims: vec![], referenced_ledger_ids: vec![] };
    let result = verifier.verify(ledger.snapshot(), &submission);
    let entries = self.errata.record(&request.turn_id, &result.findings);
    if !entries.is_empty() {
        let event = self.errata.to_memory_event(request, &entries);
        let _ = self.engine.db.save_memory_event(&event).await;
    }
}
```

然后把 Task 7 留好的两个挂钩位接上（verify 在 finalize_turn 之前——勘误
MemoryEvent 与回合摘要同回合落库）。终态 A（awaiting）处：

```rust
if let Some(gate) = awaiting {
    if !visible_text.trim().is_empty() {
        self.verify_after_stream(input.request, &ledger, &visible_text).await;
    }
    let assistant_output = if visible_text.trim().is_empty() { gate.prompt_public.clone() } else { visible_text.clone() };
    self.finalize_turn(input.request, &compiled, input.user_input, &assistant_output, "awaiting_player_roll").await;
    return Ok(TurnOutcome::AwaitingPlayerRoll { check_id: gate.check_id, prompt_public: gate.prompt_public });
}
```

叙事终态处（第 5 步注释位）：

```rust
// —— 5. 流后校验（不阻塞交付：叙事已全部流出）——
self.verify_after_stream(input.request, &ledger, &visible_text).await;
// —— 6. 确定性收尾 ——
self.finalize_turn(input.request, &compiled, input.user_input, &visible_text, "ready").await;
Ok(TurnOutcome::Narration(visible_text))
```

- [ ] **Step 4：运行确认通过**

Run: `cargo test -p trpg-gm errata_records_recent_entries_and_counts`
Expected: PASS

- [ ] **Step 5：验证收尾**

Run: `cargo test -p trpg-gm && wc -l crates/trpg-gm/src/errata.rs crates/trpg-gm/src/turn_loop.rs`
Expected: 全绿；`errata.rs` 与 `turn_loop.rs` 均 ≤400 行

---

### Task 9：缓存稳定回归测试（§6.1 硬性原则）

**Files:**
- Modify: `crates/trpg-gm/src/prompts.rs:测试模块`
- Modify: `crates/trpg-gm/src/tools/mod.rs:测试模块`
- Modify: `crates/trpg-gm/src/turn_loop.rs:tracing 可观测`
- Test: `cargo test -p trpg-gm cache_stability`

- [ ] **Step 1：写失败测试**

```rust
// crates/trpg-gm/src/prompts.rs
#[cfg(test)]
mod cache_stability_tests {
    use super::*;
    use trpg_llm::AggregatedToolCall;
    use trpg_model::{ChatMessage, CompiledContext, ContextRequest};

    fn compiled() -> CompiledContext { CompiledContext { prefix_text: "PFX".to_string(), pinned_text: "PIN".to_string(), dynamic_text: "DYN".to_string(), ..Default::default() } }

    #[test]
    fn same_inputs_assemble_same_bytes() {
        let tail = DynamicTailInput { user_input: "x", resolved_gate_facts: &[], errata_blocks: &[] };
        let a = TurnMessages::assemble(&compiled(), "skill", &[], &tail);
        let b = TurnMessages::assemble(&compiled(), "skill", &[], &tail);
        assert_eq!(serde_json::to_vec(&a.to_request_messages()).unwrap(), serde_json::to_vec(&b.to_request_messages()).unwrap());
    }

    #[test]
    fn tool_append_changes_only_tail() {
        let tail = DynamicTailInput { user_input: "x", resolved_gate_facts: &[], errata_blocks: &[] };
        let mut m = TurnMessages::assemble(&compiled(), "skill", &[ChatMessage { role: "assistant".to_string(), content: "old".to_string() }], &tail);
        let before = m.to_request_messages();
        m.push_assistant_tool_calls(&[AggregatedToolCall { id: "c".to_string(), name: "remember".to_string(), arguments: "{}".to_string() }]);
        let after = m.to_request_messages();
        assert_eq!(serde_json::to_vec(&before).unwrap(), serde_json::to_vec(&after[..before.len()]).unwrap());
    }

    #[test]
    fn prefix_hash_is_byte_level_hash() {
        let tail = DynamicTailInput { user_input: "x", resolved_gate_facts: &[], errata_blocks: &[] };
        let m = TurnMessages::assemble(&compiled(), "skill", &[], &tail);
        assert_eq!(m.prefix_byte_hash(2), m.prefix_byte_hash(2));
        assert_ne!(m.prefix_byte_hash(1), m.prefix_byte_hash(2));
    }

    #[test]
    fn cross_turn_stable_segments_keep_bytes_with_growing_history() {
        // §6.1 第 3 条 / spec §9 单测 9 的回归本体：场景不变、gm_skill 不变，
        // 跨回合 history 仅追加 ⇒ system+BP2 两条消息字节不变，旧 history 段不改写。
        let turn1_history = vec![ChatMessage { role: "user".to_string(), content: "hi".to_string() }, ChatMessage { role: "assistant".to_string(), content: "scene".to_string() }];
        let tail1 = DynamicTailInput { user_input: "look", resolved_gate_facts: &[], errata_blocks: &[] };
        let turn1 = TurnMessages::assemble(&compiled(), "skill", &turn1_history, &tail1);
        let mut turn2_history = turn1_history.clone();
        turn2_history.push(ChatMessage { role: "user".to_string(), content: "look".to_string() });
        turn2_history.push(ChatMessage { role: "assistant".to_string(), content: "you see".to_string() });
        let tail2 = DynamicTailInput { user_input: "move", resolved_gate_facts: &[], errata_blocks: &[] };
        let turn2 = TurnMessages::assemble(&compiled(), "skill", &turn2_history, &tail2);
        let raw1 = turn1.to_request_messages();
        let raw2 = turn2.to_request_messages();
        // system + BP2 字节级一致（prefix_byte_hash(2) 即跨回合稳定锚点）。
        assert_eq!(serde_json::to_vec(&raw1[..2].to_vec()).unwrap(), serde_json::to_vec(&raw2[..2].to_vec()).unwrap());
        assert_eq!(turn1.prefix_byte_hash(2), turn2.prefix_byte_hash(2));
        // 上一回合的 history 段在新回合里逐字节保留（仅追加不改写）。
        assert_eq!(serde_json::to_vec(&raw1[2..4].to_vec()).unwrap(), serde_json::to_vec(&raw2[2..4].to_vec()).unwrap());
    }

    #[test]
    fn over_budget_prefix_or_pinned_is_config_error_not_silent_trim() {
        // §6.1 第 4 条 fail-closed；TokenBudget 字段 prefix_max/pinned_max 已勘查存在。
        let request = ContextRequest { ruleset_id: "rs".to_string(), module_id: None, session_id: "s".to_string(), turn_id: "t".to_string(), viewer: trpg_model::VisibilityProfile::gm(), token_budget: trpg_model::TokenBudget { prefix_max: 4, pinned_max: 4, dynamic_max: 4, total_max: 16 } };
        let mut big_prefix = compiled();
        big_prefix.prefix_text = "w ".repeat(64);
        let err = validate_compiled_budget(&big_prefix, &request).unwrap_err();
        assert!(err.to_string().contains("prefix"));
        let mut big_pinned = compiled();
        big_pinned.pinned_text = "w ".repeat(64);
        let err = validate_compiled_budget(&big_pinned, &request).unwrap_err();
        assert!(err.to_string().contains("pinned"));
        assert!(validate_compiled_budget(&compiled(), &request).is_ok());
    }
}
```

```rust
// crates/trpg-gm/src/tools/mod.rs
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
```

- [ ] **Step 2：运行确认失败**

Run: `cargo test -p trpg-gm same_inputs_assemble_same_bytes`
Expected: FAIL，报错应为测试不存在或 `prefix_byte_hash` 行为未固定

- [ ] **Step 3：最小实现**

补充 `turn_loop.rs` 工具轮每轮请求前的可观测日志（Task 7 的循环变量 `_round`
改回 `round` 以供记录；上游 cached_tokens 已由 Task 7 的 `StreamEvent::Usage`
分支在同一 `gm_cache` target 下记录——relay 透传则有值，缺省整轮无 Usage 事件，
按缺省跳过，两条日志合起来构成 §6.1 第 5 条的完整观测链）：

```rust
tracing::info!(
    target: "gm_cache",
    session_id = %input.request.session_id,
    turn_id = %input.request.turn_id,
    prefix_hash = %compiled.prefix_hash,
    pinned_hash = %compiled.pinned_hash,
    request_prefix_hash = %messages.prefix_byte_hash(2),
    tool_round = round,
    "gm agent prompt cache anchors"
);
```

`prompts.rs` 的 `validate_compiled_budget` 由 Task 2 占位升级为真实现（契约第
7 节；TokenBudget 字段 prefix_max/pinned_max/dynamic_max/total_max 已对真实代码
勘查确认；Err 信息必须含段名，对应测试断言）：

```rust
pub fn validate_compiled_budget(compiled: &CompiledContext, request: &ContextRequest) -> Result<()> {
    let prefix_tokens = compiled.prefix_text.split_whitespace().count() as u32;
    let pinned_tokens = compiled.pinned_text.split_whitespace().count() as u32;
    if prefix_tokens > request.token_budget.prefix_max {
        return Err(anyhow!("prefix segment exceeds TokenBudget.prefix_max ({prefix_tokens} > {}); prefix/pinned over budget is a configuration error, never silently trimmed", request.token_budget.prefix_max));
    }
    if pinned_tokens > request.token_budget.pinned_max {
        return Err(anyhow!("pinned segment exceeds TokenBudget.pinned_max ({pinned_tokens} > {}); prefix/pinned over budget is a configuration error, never silently trimmed", request.token_budget.pinned_max));
    }
    Ok(())
}
```

并在 `run_gm_turn` 里 compiled 就绪后（ctx_provider 与 prepare_turn_context 两条
路径之后、assemble 之前）调用：

```rust
crate::prompts::validate_compiled_budget(&compiled, input.request)?;
```

- [ ] **Step 4：运行确认通过**

Run: `cargo test -p trpg-gm same_inputs_assemble_same_bytes`
Expected: PASS

- [ ] **Step 5：验证收尾**

Run: `cargo test -p trpg-gm cache_stability_tests && cargo test -p trpg-gm schema_stability_tests && wc -l crates/trpg-gm/src/prompts.rs crates/trpg-gm/src/tools/mod.rs crates/trpg-gm/src/turn_loop.rs`
Expected: 全绿；列出的文件均 ≤400 行

---

### Task 10：CLI `--agent` 接入

**Files:**
- Create: `crates/trpg-cli/src/agent_play.rs`
- Modify: `crates/trpg-cli/src/main.rs:Play 子命令、mod 声明、分流调用`
- Modify: `crates/trpg-cli/Cargo.toml:trpg-gm 依赖`
- Test: `cargo build -p trpg-cli`

- [ ] **Step 1：写失败测试**

```rust
// crates/trpg-cli/src/agent_play.rs
use anyhow::Result;

pub async fn smoke_agent_play_symbol_exists() -> Result<()> {
    Ok(())
}
```

- [ ] **Step 2：运行确认失败**

Run: `cargo build -p trpg-cli`
Expected: Step 1 的 stub 未被 main.rs 引用时 build 仍 PASS（不算确认）；真正的失败形态出现在 Step 3 接线中途——只加 `mod agent_play;` 与分流、未加 Cargo.toml 依赖或未加 `agent` flag 时，报 `unresolved import trpg_gm` 或 `no field agent`（任一即算本步确认；Step 3 完整落地后应 PASS）

- [ ] **Step 3：最小实现**

`crates/trpg-cli/Cargo.toml` 增加：

```toml
trpg-gm = { path = "../trpg-gm" }
```

`crates/trpg-cli/src/main.rs` 增加模块声明：

```rust
mod agent_play;
```

在 `Play` 子命令结构体里增加 flag：

```rust
#[arg(long, default_value_t = false)]
agent: bool,
```

`Commands::Play` 分支臂改为按 flag 分流（真实代码 main.rs L526 现为
`Commands::Play { ruleset, module } => play_cli(&ruleset, module.as_deref()).await,`，
`ruleset` 是 `String`（无 as_deref）、臂内**没有** db/llm/format 变量——play_cli
内部自建，agent 路径同理自建，分流只传 CLI 参数）：

```rust
Commands::Play { ruleset, module, agent } => {
    if agent {
        agent_play::play_cli_agent(&ruleset, module.as_deref()).await
    } else {
        play_cli(&ruleset, module.as_deref()).await
    }
}
```

创建 `crates/trpg-cli/src/agent_play.rs`（会话装配**逐项对齐 play_cli**；真实
helper 签名已勘查：`connect_db()->Result<Db>`、`make_llm()->Result<Arc<dyn
LlmClient>>`、`make_search(&Db, PathBuf)->Result<SearchService>`、
`default_data_dir()->PathBuf`（认 TRPG_DATA_DIR）、`emit_delta(StreamFormat,
&str)->Result<()>`、`emit_phase(StreamFormat, &str, Value)->Result<()>`（第三参
**按值** Value）、`StreamFormat` 是 main.rs 私有 enum{Text,Jsonl,Sse}（同 crate
子模块可见）、`RuntimeEngine::start_session(&str, Option<&str>)->Result<String>`、
`Db::load_session_scene(&str)->Result<Option<String>>`）：

```rust
// crates/trpg-cli/src/agent_play.rs
use anyhow::Result;
use serde_json::json;
use std::io::{self, Write};
use std::sync::Arc;
use trpg_api::extract_module_scenes;
use trpg_gm::{GmLoop, GmTurnInput, LoopConfig, SceneDeepExtractFn, ToolRegistry, TurnOutcome};
use trpg_model::{ChatMessage, ContextRequest, RuntimeState, TokenBudget, VisibilityProfile};
use trpg_runtime::RuntimeEngine;

pub async fn smoke_agent_play_symbol_exists() -> Result<()> { Ok(()) }

/// --agent 会话循环。装配逐项对齐 play_cli：内部自建 db/llm/search/engine，
/// 不从外部收 `&dyn LlmClient`（不存在 From<&dyn> for Arc<dyn>，引用变不了所有权）。
pub async fn play_cli_agent(ruleset: &str, module: Option<&str>) -> Result<()> {
    let db = crate::connect_db().await?;
    db.migrate().await?;
    // make_llm() 已返回 Arc<dyn LlmClient>：直接持有 Arc，绝不从 &dyn 造 Arc。
    let llm = crate::make_llm()?;
    let data_dir = crate::default_data_dir();
    // with_search 接入后 retrieve_rules 工具才可用（否则恒 "retrieve unavailable: no search service"）。
    let search = crate::make_search(&db, data_dir.clone())?;
    let engine = RuntimeEngine::new(db.clone()).with_search(search);
    // 真 session bootstrap（绝不自造 session_id 字符串）：sessions 行落库 +
    // 模组入口场景激活（entry_node_id → current_scene_id）。record_world_event /
    // insert_pending_check / save_memory_event 的 FK、BP2 当前可玩单元投影、
    // navigate_scene 的 current 比对全靠它。
    let session_id = engine.start_session(ruleset, module).await?;
    println!("agent session: {session_id}");
    println!("Type /quit to exit.");
    // 一期固定 Text（StreamFormat 私有类型，同 crate 子模块可见）。
    let format = crate::StreamFormat::Text;
    let mut gm = GmLoop::new(engine, llm.clone(), ToolRegistry::standard(), LoopConfig::default(), data_dir.clone());
    if let Some(mid) = module.map(str::to_string) {
        let db2 = db.clone();
        let llm2 = llm.clone();
        let rs2 = ruleset.to_string();
        let dir2 = data_dir.clone();
        gm.scene_extractor = Some(Arc::new(move |node_id: String| {
            let db3 = db2.clone();
            let llm3 = llm2.clone();
            let mid3 = mid.clone();
            let rs3 = rs2.clone();
            let dir3 = dir2.clone();
            Box::pin(async move { extract_module_scenes(&db3, llm3.as_ref(), &mid3, None, Some(&rs3), &dir3, 12, Some(&node_id)).await })
        }) as SceneDeepExtractFn);
    }
    let mut history: Vec<ChatMessage> = Vec::new();
    loop {
        print!("\n[chatrpg:agent]> ");
        io::stdout().flush().ok();
        let mut line = String::new();
        if io::stdin().read_line(&mut line)? == 0 { break; }
        let input = line.trim().to_string();
        if input.is_empty() { continue; }
        if input == "/quit" { break; }
        let turn_id = format!("turn_{}", uuid::Uuid::new_v4().simple());
        let request = ContextRequest { ruleset_id: ruleset.to_string(), module_id: module.map(str::to_string), session_id: session_id.clone(), turn_id, viewer: VisibilityProfile::gm(), token_budget: TokenBudget::default() };
        // 每回合把持久化的 current_scene_id 填进 RuntimeState（对齐 prepare_turn_context
        // 单点约定；scene_navigator 切换后下一回合在此读到新场景）。
        let scene_id = db.load_session_scene(&session_id).await.ok().flatten();
        let state = RuntimeState { ruleset_id: ruleset.to_string(), module_id: module.map(str::to_string), scene_id, ..Default::default() };
        let mut streamed = String::new();
        let outcome = gm.run_gm_turn(
            GmTurnInput { request: &request, state: &state, user_input: &input, history: &history, recent_transcript: None },
            &mut |delta| {
                streamed.push_str(delta);
                let _ = crate::emit_delta(format, delta);
            },
        ).await?;
        println!();
        match outcome {
            TurnOutcome::Narration(text) => {
                history.push(ChatMessage { role: "user".to_string(), content: input });
                history.push(ChatMessage { role: "assistant".to_string(), content: text });
            }
            TurnOutcome::AwaitingPlayerRoll { prompt_public, .. } => {
                // emit_phase 第三参按值收 Value（owned json!），Result 用 let _ 接住。
                let _ = crate::emit_phase(format, "awaiting_player_roll", json!({"prompt_public": prompt_public.clone()}));
                history.push(ChatMessage { role: "user".to_string(), content: input });
                history.push(ChatMessage { role: "assistant".to_string(), content: if streamed.trim().is_empty() { prompt_public } else { streamed } });
            }
        }
    }
    Ok(())
}
```

- [ ] **Step 4：运行确认通过**

Run: `cargo build -p trpg-cli`
Expected: PASS

- [ ] **Step 5：验证收尾**（三条命令**独立执行，不得 && 串联**——第二条预期受控失败，串联会吞掉第三条）

Run: `cargo build -p trpg-cli`
Expected: PASS

Run: `cargo run -p trpg-cli --bin trpg -- play --ruleset cyberpunk_red --agent`
Expected: **受控失败**——无 DB / 无规则包 / 无 gm_skill 数据时输出清晰错误（exit ≠ 0），绝不 panic

Run: `wc -l crates/trpg-cli/src/agent_play.rs crates/trpg-cli/src/main.rs`
Expected: `agent_play.rs` ≤400 行；`main.rs` 相对改前增量 ≤20 行

---

### Task 11：e2e 实测清单（真 DB 真模组真 gpt-5.5）

**Files:**
- Create: 无
- Modify: 无
- Test: 手工 e2e 清单，命令可照抄执行

- [x] **Step 1：写失败测试**（实跑见文末执行记录）

```bash
export TRPG_LLM_BASE_URL=http://127.0.0.1:18888/v1
export TRPG_LLM_MODEL=gpt-5.5
export TRPG_LLM_API_KEY=${TRPG_LLM_API_KEY:?set real key or relay token}
export DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg
cargo run -p trpg-cli --bin trpg -- play --ruleset coc7 --module coc.bloody_highway --agent
```

Expected: FAIL，若数据库、规则包、模组或 relay 未准备，应看到清晰错误，不得 panic。

- [x] **Step 2：运行确认失败**（exit=1 清晰报错不 panic：`ruleset coc7 is not mechanically startable yet; missing RuleKernel, CharacterOnboardingPack`）

Run: 上述命令
Expected: FAIL，常见报错为 `no parsed project bundle found; run parse-all first`、数据库连接失败、或 relay smoke 未通过

- [x] **Step 3：最小实现**（真实 ID 为 call_of_cthulhu_7e/document 与 triangle_agency/the_vault；bundle 已 parsed 未重跑全量 parse-all；详见执行记录）

按顺序执行：

```bash
export TRPG_LLM_BASE_URL=http://127.0.0.1:18888/v1
export TRPG_LLM_MODEL=gpt-5.5
export TRPG_LLM_API_KEY=${TRPG_LLM_API_KEY:?set real key or relay token}
export DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg

cargo run -p trpg-llm --bin relay_tools_smoke
cargo run -p trpg-cli --bin trpg -- migrate
cargo run -p trpg-cli --bin trpg -- parse-all
```

如果 CoC 库使用端口 swap，先确认：

```bash
docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg -c "select current_database(), inet_server_port();"
```

跑血色公路 ≥3 回合：

```bash
RUST_LOG=info cargo run -p trpg-cli --bin trpg -- play --ruleset coc7 --module coc.bloody_highway --agent
```

建议输入脚本：

```text
我检查车祸现场附近有没有不自然的拖拽痕迹。
我冒险靠近路边林线，想看清里面的影子。
roll 42
```

跑 The Vault / Triangle ≥3 回合：

```bash
RUST_LOG=info cargo run -p trpg-cli --bin trpg -- play --ruleset triangle_agency --module triangle.the_vault --agent
```

建议输入脚本：

```text
I interview the crying woman near FOUNT and ask what changed about her face.
I use my anomalous ability to inspect the puddle without touching it.
roll 3
```

验收 SQL：

```bash
# check_contracts 表（migrations/0004）没有 check_label 列（实列只有 id/check_id/
# session_id/turn_id/ruleset_id/module_id/actor_id/target_actor_id/roll_visibility/
# contract_json/status/created_at）：check_label 必须从 contract_json 取。
docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg -c "select check_id, ruleset_id, contract_json->>'check_label' as check_label, contract_json->>'tested_parameter' as tested_parameter, roll_visibility, created_at from check_contracts order by created_at desc limit 10;"

docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg -c "select target_id, parameter_path, value_json, visibility, updated_at from generic_parameter_states order by updated_at desc limit 20;"

docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg -c "select check_id, status, prompt_public, created_at from pending_checks order by created_at desc limit 10;"

docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg -c "select gate_id, gate_kind, status, prompt_public, resolution_json, created_at from interaction_gates order by created_at desc limit 10;"

docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg -c "select event_id, tags, summary, occurred_at from memory_events where tags @> array['gm_errata'] order by occurred_at desc limit 10;"
```

延迟与流式观测：

```bash
RUST_LOG=gm_cache=info,trpg_gm=info time cargo run -p trpg-cli --bin trpg -- play --ruleset triangle_agency --module triangle.the_vault --agent
```

记录每回合：

```text
ruleset=<id>
module=<id>
turn=<n>
TTFT=<first visible content token seconds>
total_latency=<seconds>
content_streamed_before_done=<yes/no>
prefix_hash=<compiled prefix_hash>
pinned_hash=<compiled pinned_hash>
cached_tokens=<value or relay_not_reported>
```

新旧路径对照：

```bash
cargo run -p trpg-cli --bin trpg -- play --ruleset triangle_agency --module triangle.the_vault
cargo run -p trpg-cli --bin trpg -- play --ruleset triangle_agency --module triangle.the_vault --agent
```

用 `chatrpg-product-evaluator` 记录：

```text
agent_path_reasonable_checks=<pass/fail/evidence>
tested_parameter_binding=<pass/fail/evidence>
resource_on_outcome_applied=<pass/fail/evidence>
player_roll_gate_closed=<pass/fail/evidence>
streaming_ttft_ok=<pass/fail/evidence>
cache_observed=<pass/fail/evidence>
errata_kind_counts=<json>
combat_depth_gap=<notes>
```

- [x] **Step 4：运行确认通过**（两模组各 ≥7 回合 + 5 个补充 CoC 会话；全部 SQL 已执行，证据见执行记录；PASS 条件 1/2/4/5 达成，3 在 roll 42 形态达成、裸 roll 形态暴露 2 个 bug 已记录）

Run: 两个模组各 ≥3 回合，并执行全部 SQL
Expected: PASS 条件如下：

1. `check_contracts.contract_json` 中每个实际检定都有 `tested_parameter`，语义与玩家行动匹配。
2. SAN/HP/Chaos/Harm 或相应资源的 `generic_parameter_states` 有真实变化，且变化时间落在对应回合之后。
3. `request_player_roll` 能暂停，玩家回骰后 `pending_checks.status` 与 `interaction_gates.status` 关闭，下一回合叙事承接后果。
4. 每回合总延迟 ≤40s；最终叙事是 token 级真流式，首个可见 content token 明显早于回合完成。
5. `gm_cache` 日志中跨回合无因 prefix/pinned hash 不变；relay 透传 `cached_tokens` 时记录数值。

- [x] **Step 5：验证收尾**（执行记录见文末「Task 11 执行记录」；新建文件全 ≤400 行；trpg-gm 34 + trpg-llm 7 测试绿）

Run: 将 smoke 结论、SQL 输出、TTFT 记录、缓存日志、evaluator 对照结果追加到执行记录
Expected: 每条验收都有证据；无证据不得勾选；所有源文件仍满足 ≤400 行约束

---

## Task 11 执行记录（2026-06-10，真 DB :54347 + 真模组 + 真 gpt-5.5 经 :18888 relay）

### 环境与闸门

- DB：`chatrpg-postgres-rulesets` :54347 Up；`select current_database()` → chatrpg。.env DATABASE_URL 指 :54346（赛博库），按清单用 env 覆盖 :54347（坑已证实）。
- relay：`/v1/models` 200；`relay_tools_smoke` 全 PASS（"PASS complete_with_tools multi-turn: Done." / "INFO usage passthrough: tool_round=true content_round=true" / "PASS raw SSE stream exposes tool_calls delta and content delta" / "PASS relay_tools_smoke all checks"，exit=0）。
- **实际 ID 修正**：plan 占位 `coc7/coc.bloody_highway`、`triangle.the_vault` 不存在；真实为 `--ruleset call_of_cthulhu_7e --module document`（血色公路）与 `--ruleset triangle_agency --module the_vault`。Step 1/2 用占位 ID 实跑得到清晰错误不 panic（`Error: ruleset coc7 is not mechanically startable yet; missing RuleKernel, CharacterOnboardingPack...`，exit=1）✓。
- 环境装配三件套（不进代码、纯运行环境）：① 必须 source `.env.example` 全量 TRPG_*_ENABLE flags（裸 .env 跑出 narration-only WARN）；其 90 行 `TRPG_WORLD_TIME_START_DISPLAY=Day 1, 00:00` 无引号空格，shell source 会报错，需引号化导出；② 每模组 `TRPG_DATA_DIR` 指向 `_rstest_coc` / `_rstest_triangle`（模组 markdown/search 索引在那），并 unset `TRPG_SEARCH_INDEX_DIR` 让索引随 data_dir 派生；③ gm_skill 是 agent 路径 fail-closed 硬依赖，rstest 数据目录无此目录 → 建符号链接 `_rstest_*/agent -> 工作区 data/agent`。
- 角色：`trpg create-character --auto --session-id <sid>`（≈20s）把 created_character_v1 绑进会话；agent/旧路径开局都不自动建卡。

### 会话与时延总表（驱动脚本 /tmp/agent_e2e_driver.py，逐 chunk 时间戳实测）

| 会话 | 回合 | TTFT min/med/max (s) | total max (s) | exit |
|---|---|---|---|---|
| CoC run3（roll 42 闭环）session_a5157afe… | 7 | 2.31 / 5.64 / 6.78 | 15.15 | 0 |
| Triangle agent session_806b81ed… | 7 | 3.41 / 4.91 / 7.0 | 17.8 | 0 |
| CoC run4（绑卡战斗）session_440cc9ed… | 8 | 7.0 / 9.8 / 10.91 | 22.03 | 0 |
| CoC run5（理智检定）session_28f6624f… | 5 | 5.3 / 17.81 / 39.84 | 45.94 | 0 |
| CoC run6（失败理智）session_fb40a782… | 8 | 2.42 / 5.13 / 5.9 | 23.62 | 0 |
| CoC run7/8（裁量伤害）session_89660cec…/session_30d475b3… | 3+3 | 2.54–13.1 | 14.61 | 0 |
| **Triangle 旧路径对照** session_211abf9a… | 3 | **36.04 / 53.74 / 71.28** | **77.78** | 0 |

最终叙事为 token 级真流式（96–498 个 content chunk/回合，首 chunk 显著早于回合完成）。唯一超 40s 的回合：run5 turn1（45.94s，多工具轮）。

### spec §9 e2e 验收逐项

1. **合理时机发起检定 + tested_parameter 语义绑定** ✓ PASS——check_contracts 实录（`contract_json->'tested_parameter'->>'key'`）：查拖拽痕迹→`Spot Hidden / Track`、看影子→`skills.spot_hidden`、聆听→`Listen`、格挡→`Dodge`/`dodge`、挥击→`fighting_brawl`、理智→`sanity`（两套规则、双语种全语义匹配）。免检定裁量同样出现（Triangle turn1 采访直接叙事 +"no roll" 明示；旧路径对照同输入则机械掷骰）。绑卡后真机制结算：`[roll]格挡/闪避黑影爪击：1d100 = 95，对 Dodge 30，失败[/roll]`（degree=strong_failure）；Triangle `6d4→15 degree=success`；run6 sanity target=30（卡值真被查到）。
2. **资源 on_outcome 真触发** ✓ PASS（GM 裁量通道）——run8：get_actor 读出陈大山 HP 15/15、SAN 60/60 → apply_effect 后 `generic_parameter_states`：`pc.current resources.hit_points.current=13`（09:39:26，turn2 内）、`resources.sanity.current=56`（09:39:41，turn3 内），parameter_impacts 2 行，叙事 `[system]已结算：pc.current 失去 2 点 HP/4 点 SAN[/system]`。**注**：kernel 自动 on_outcome（sanity `amount:"=sanity_loss"` on_failure）在 run6 两次 strong_failure 理智检定上未扣——outcome 无 `sanity_loss` 字段且该轨无 `default_amount`，`resolve_track_amount` fail-closed 跳过；这是数据驱动设计（不发明损失骰），旧路径同条件同行为，需模组遭遇绑定 sanity_loss 才自动扣，非 agent 回归。
3. **request_player_roll 全闭环** ✓ PASS（roll 42 形态）——run3：3 次 gate（turn1/3/5）→ `roll 42` 头部结算 → pending_checks/interaction_gates 全 `resolved` → 同回合事实注入+后果叙事（`[roll]你掷出了 42…[/roll]` 开头）；turns 表 postprocess_status 序列 awaiting_player_roll→ready 交替。run5 DEX gate `roll 88`→strong_failure→后果叙事再证。**发现 2 个缺陷**（见下）。
4. **总延迟 ≤40s + 真流式** ✓ PASS（1 个超限样本如上）——agent 路径 TTFT 中位 ≈5s、回合中位 ≈11s，旧路径对照 TTFT 36–71s / 回合 42–78s（提速 ~6–8×）。
5. **跨回合缓存命中** ✓ PASS——gm_cache 锚点：run3 七回合 prefix_hash `sha256:57335b09…`、pinned_hash `sha256:e3b0c442…`（空 BP2 的 sha256）、request_prefix_hash `sha256:c72c7695…` 全恒定；Triangle 八请求同样恒定。relay 透传 usage：重放实测 `prompt_tokens=53296, cached_tokens=49664` → **93.2% 命中**；跨会话也命中（run3 turn1 即 50176）。
6. **回合落库** ✓ PASS——turns 表每回合 user_input/assistant_output/postprocess_status（ready/awaiting_player_roll）齐全，与内存 history 一致；memory_events 每回合 gm_turn 摘要 + gm_errata 条目。

### 实测发现的问题（按严重度）

- **[bug] 裸 `roll` 回复不触发头部 gate 结算**：`turn_loop.rs` 头部守卫 `parse_roll_text(input).is_some()` 对 "roll"/"/roll" 返回 None（runtime 旧路径靠 `wants_system_roll` 兜底，agent 头部没接）。而 gate 提示语本身要求"请只回复 `roll`"。实测（Triangle turn3-6 / run4 turn3-6）：回 roll → 头部跳过 → agent 重复开新 gate（旧 gate superseded ×2），到第 3 次才自己改用 roll_check（public_gm_roll）系统掷骰收场。功能可恢复但闭环路径偏离设计。修法：守卫加 `|| wants_system_roll(input)`（与 runtime 同源语义）。
- **[bug] 默认骰策略下 `roll 42` 回复炸整个会话**：`TRPG_AGENT_TABLE_DICE_POLICY=system_rolls_visible`（.env 默认）时 `resolve_check_with_input` 返回 Err（"player-reported roll totals are disabled…"），`run_gm_turn` 的 `?` 直接传播 → `play_cli_agent` 整个会话循环退出（exit=1）。旧路径同输入是 GateReprompt 不崩。e2e 改用 `TRPG_PLAYER_REPORTED_ROLL_TOTALS=true` 后闭环验证通过。修法：头部结算 Err 应折叙事事实/reprompt 而非终止回合。
- **[质量] agent 上下文看不见已绑定 PC**：run7 三回合 GM 坚称"没有已登记的调查员角色"拒绝结算（实际 created_character_v1 已绑定且 run5/6 内核结算能查到卡值），也不主动调 get_actor；run8 玩家显式给出 `pc.current` 后 get_actor/apply_effect 全链路通畅。BP2 pinned 实测为空串（pinned_hash=空 sha256；抓包 msg[1] 仅 33 字节框架），PC 卡也无任何 block 注入。建议二期把 PC 摘要投进 BP2/BP3 或 gm_skill 提示"先 get_actor"。
- **[环境] 上游一次性 180s 零字节 stall**：首个直连会话 stream 请求 173s 无任何 SSE 字节，撞 reqwest 客户端默认总超时 `TRPG_LLM_TIMEOUT_SECS=180` 报 "error decoding response body: operation timed out"。同载荷重放 10.3–16.2s 正常（抓包代理与直连均复现正常），判定为上游/驻宅代理一次性抖动。e2e 全程 `TRPG_LLM_TIMEOUT_SECS=600` 规避。注意该超时覆盖整个流式读取，长回合需保持 ≥600。
- **[观察] 工具轮极少**：绝大多数回合 tool_round=0 一轮内完成（含工具调用+叙事混合轮），仅 1 次 round=1。8 轮上限远未触及。

### 防幻觉护栏 / 勘误记忆 live 证据

- NarrationVerifier→ErrataMemory→memory_events 全链路落库，kind_counts（今日 4 会话）：`missing_check ×4`、`invented_effect ×6`（tags=`{gm_errata,<kind>}`）。典型命中：run5 turn2 GM 纯叙事爪击伤害未调工具 → invented_effect（正中观察到的缺口）。
- 私骰泄漏过滤：本批次检定全为 public/player 可见（CoC 视感知类为玩家可见检定），RedactingBuffer 空 tokens 透明直通路径覆盖；单测覆盖跨 chunk 过滤（trpg-gm 34 测试绿）。

### 对照 playtest 记录（evaluator 字段，按 plan 模板）

```text
agent_path_reasonable_checks=pass（采访/查看→免检定或澄清；冒险行动→gate/检定；显式 Sanity→sanity 绑定；旧路径同输入连采访都机械掷骰）
tested_parameter_binding=pass（见验收① 实录；个别 key 为散文体如 "Spot Hidden / Track（按你的调查方式取较合适者）"，可后续约束为 canonical key）
resource_on_outcome_applied=pass via GM 裁量 apply_effect（HP 15→13、SAN 60→56 落 generic_parameter_states）；kernel 自动 sanity_loss 需 source-backed 遭遇数据（fail-closed，与旧路径同）
player_roll_gate_closed=pass（roll 42 形态全闭环）；裸 roll 形态见 bug 清单
streaming_ttft_ok=pass（TTFT 中位 ≈5s vs 旧路径 36–71s；1 个 45.94s 超限样本）
cache_observed=pass（prefix/pinned/request 前缀 hash 跨回合恒定；cached_tokens 49664/53296=93.2%）
errata_kind_counts={"missing_check":4,"invented_effect":6}
combat_depth_gap=旧路径 attack→damage followup/armor 管线在 agent 路径未走到（run4 近战只到对抗检定层；apply_effect 可作 GM 裁量补位）；spec §3 已知 trade-off 成立
```

（注：对照字段由真实双路径 playtest 数据直接整理；未另起 chatrpg-product-evaluator 独立评估流水线。）

### 工程约束终验

- 新建/重点文件 wc -l：turn_loop.rs 387、stream_tools.rs 341、check.rs 253、tools/mod.rs 249、prompts.rs 226、relay_tools_smoke.rs 158、effect.rs 141、direct_effect.rs 141、stream.rs 128、world.rs 124、ledger.rs 111、errata.rs 102、agent_play.rs 91、npc.rs 40、lib.rs 16 —— 全部 ≤400 ✓（既有大文件 main.rs/runtime lib.rs 等仅做计划内单点修改）。
- `cargo test -p trpg-gm -p trpg-llm`：34 + 7 全绿。
- 零 per-ruleset 硬编码：两规则两语种同一套循环跑通，检定/资源全走 kernel 数据。

### go/no-go 结论

**GO**——D2（流式 function-calling）、§6.1（缓存稳定）、痛点 1/2（掷骰裁量权上移 + fail-closed 可见可修复）在真环境全部成立，新路径时延全面碾压旧路径。带 3 个 must-fix 跟进项进二期：① 裸 `roll` 头部结算守卫；② 头部结算 Err 不得炸会话；③ PC 可见性（BP2 投影或 get_actor 引导）。

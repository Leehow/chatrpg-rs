//! Policy Plugin Host v1（T1）契约类型。
//!
//! 设计：`docs/superpowers/specs/2026-06-17-policy-plugin-host-design.md` §4.1。
//! 这里只定义**契约 + 只读上下文快照**，不接线 turn pipeline（T2/T3）。
//!
//! 复用既有单一事实源（不重定义）：
//! - `trpg_model::ContextBlock`：PromptBlock 贡献载荷（已带 visibility/cache_zone/...）。
//! - `trpg_model::SourceRef`：来源引用。
//! - `trpg_agent::VerifierFinding`：verifier 贡献载荷。
//! - `trpg_model::PluginContributionTrace`：Flight Recorder 记录（在 trpg-model，
//!   让 `TurnTrace` 能持有；保持 model 依赖洁净）。

use serde::{Deserialize, Serialize};

/// 插件挂载点（hook）。可扩枚举：v1 只接 2 个。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum PluginHook {
    /// prepare_turn_context 后、assemble 前：挂 PromptBlock + ContextFilter。
    ContextAssembly,
    /// LLM 念白成形后：挂 VerifierFinding。
    AfterLlmStream,
}

impl PluginHook {
    /// 稳定字符串（用于 Flight Recorder trace 与解释）。
    pub fn as_str(&self) -> &'static str {
        match self {
            PluginHook::ContextAssembly => "context_assembly",
            PluginHook::AfterLlmStream => "after_llm_stream",
        }
    }
}

/// 安全等级（仲裁排序权重）。`Ord`：Core < Safety < ... < Format，
/// **数值越小优先级越高**（Core 先于 Format 应用）——安全/核心贡献先生效，
/// 风格插件不得覆盖防剧透/防私造。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SafetyClass {
    Core,
    Safety,
    Rule,
    Module,
    Experience,
    Format,
}

/// 失败处置策略（v1 三档；host 不在 T1 应用，留 runtime 仲裁用）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FailPolicy {
    /// 风格类：失败忽略。
    #[default]
    Ignore,
    /// memory 类：发警告后续跑。
    WarnContinue,
    /// verifier 类：先尝试 repair，再 fail-closed（走既有 errata 提醒，不硬拦）。
    RepairThenFailClosed,
}

/// 贡献元数据。PromptBlock 的 cache_zone/visibility 复用 `ContextBlock` 自带字段，
/// 这里只携带仲裁所需的通用元（plugin_id/hook/priority/safety_class/fail_policy）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContributionMeta {
    pub plugin_id: String,
    pub hook: PluginHook,
    pub priority: i32,
    pub safety_class: SafetyClass,
    pub fail_policy: FailPolicy,
    #[serde(default)]
    pub source_refs: Vec<trpg_model::SourceRef>,
}

/// ContextFilter 贡献：插件返回"删哪些 block_id"（保守删，漏删优于过删）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextFilterSpec {
    pub drop_block_ids: Vec<String>,
    #[serde(default)]
    pub reason: String,
}

/// 贡献载荷（3 类，对应防剧透三段）。
///
/// 不派生 `Serialize`：`ContextBlock` / `VerifierFinding` 用 trace 摘要落
/// Flight Recorder（见 `PluginContribution::to_trace`），契约本身只需 `Clone+Debug`。
#[derive(Debug, Clone)]
pub enum PluginContributionKind {
    PromptBlock(trpg_model::ContextBlock),
    ContextFilter(ContextFilterSpec),
    VerifierFinding(trpg_agent::VerifierFinding),
}

impl PluginContributionKind {
    /// 种类稳定字符串（Flight Recorder trace 的 kind）。
    pub fn kind_str(&self) -> &'static str {
        match self {
            PluginContributionKind::PromptBlock(_) => "prompt_block",
            PluginContributionKind::ContextFilter(_) => "context_filter",
            PluginContributionKind::VerifierFinding(_) => "verifier_finding",
        }
    }

    /// 人读摘要（block 标题 / 删除块数 / finding 种类）。
    pub fn summary(&self) -> String {
        match self {
            PluginContributionKind::PromptBlock(b) => b.title.clone(),
            PluginContributionKind::ContextFilter(f) => {
                format!("drop {} block(s)", f.drop_block_ids.len())
            }
            PluginContributionKind::VerifierFinding(vf) => {
                serde_json::to_value(vf.kind)
                    .ok()
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
                    .unwrap_or_else(|| format!("{:?}", vf.kind))
            }
        }
    }
}

/// 一条插件贡献（元 + 载荷）。
#[derive(Debug, Clone)]
pub struct PluginContribution {
    pub meta: ContributionMeta,
    pub kind: PluginContributionKind,
}

impl PluginContribution {
    /// 折成 Flight Recorder 记录（`trpg_model::PluginContributionTrace`）。
    pub fn to_trace(&self) -> trpg_model::PluginContributionTrace {
        trpg_model::PluginContributionTrace {
            plugin_id: self.meta.plugin_id.clone(),
            hook: self.meta.hook.as_str().to_string(),
            kind: self.kind.kind_str().to_string(),
            summary: self.kind.summary(),
        }
    }
}

/// 传给插件的**只读快照**（绝不交出 DB / engine）。
///
/// 纯 owned 结构（无生命周期），便于构造与测试。T2/T3 填充可选只读视图字段。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginContext {
    pub session_id: String,
    pub turn_id: String,
    pub ruleset_id: String,
    #[serde(default)]
    pub module_id: Option<String>,
    pub hook: PluginHook,
    /// 本回合已揭示实体（(kind, id)），ContextFilter 据此放行已 surfaced（默认空）。
    #[serde(default)]
    pub surfaced_entities: Vec<(String, String)>,
    /// AfterLlmStream 的念白快照（默认 None）。
    #[serde(default)]
    pub narration: Option<String>,
    /// 当前可见 block id 列表（ContextFilter 从中择块删除）。
    #[serde(default)]
    pub compiled_block_ids: Vec<String>,
    /// 插件自配置（applies_when 余项 / 行为参数）。
    #[serde(default)]
    pub config: serde_json::Value,
}

impl Default for PluginHook {
    fn default() -> Self {
        PluginHook::ContextAssembly
    }
}

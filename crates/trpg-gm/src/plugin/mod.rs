//! Policy Plugin Host v1（T1）：统一 PromptBlock / ContextFilter / VerifierFinding
//! 契约 + `PluginHost` 骨架（propose-not-commit；本 T1 不接线 turn pipeline）。
//!
//! 设计：`docs/superpowers/specs/2026-06-17-policy-plugin-host-design.md` §4.1 / §7-1。
//! - `types`：契约类型（PluginHook/SafetyClass/FailPolicy/ContributionMeta/
//!   ContextFilterSpec/PluginContributionKind/PluginContribution/PluginContext）。
//! - `host`：`RuntimePlugin` trait + `PluginHost`（按 safety_class > priority 仲裁排序）。
//!
//! Flight Recorder 记录 `PluginContributionTrace` 定义在 trpg-model（让 `TurnTrace`
//! 能持有，保持 model 依赖洁净）；`PluginContribution::to_trace()` 返回它。

pub mod host;
pub mod types;

pub use host::{PluginHost, RuntimePlugin};
pub use types::{
    ContextFilterSpec, ContributionMeta, FailPolicy, PluginContext, PluginContribution,
    PluginContributionKind, PluginHook, SafetyClass,
};

//! Policy Plugin Host v1：统一 PromptBlock / ContextFilter / VerifierFinding
//! 契约 + `PluginHost`（propose-not-commit）+ 内置 policy 插件。
//!
//! 设计：`docs/superpowers/specs/2026-06-17-policy-plugin-host-design.md` §4.1-4.3 / §7。
//! - `types`：契约类型（PluginHook/SafetyClass/FailPolicy/ContributionMeta/
//!   ContextFilterSpec/PluginContributionKind/PluginContribution/PluginContext）。
//! - `host`：`RuntimePlugin` trait + `PluginHost`（按 safety_class > priority 仲裁排序）。
//! - `builtin_no_spoiler`：内置 `core.no_spoiler_guard`（T2 PromptBlock 半边）。
//!
//! T1 只定契约（不接线）；T2 把 ContextAssembly hook 的 PromptBlock 贡献接进
//! turn pipeline（见 `builtin_plugin_host()` + turn_loop.rs phase_context_assembly）。
//!
//! Flight Recorder 记录 `PluginContributionTrace` 定义在 trpg-model（让 `TurnTrace`
//! 能持有，保持 model 依赖洁净）；`PluginContribution::to_trace()` 返回它。

pub mod builtin_no_spoiler;
pub mod host;
pub mod types;

pub use builtin_no_spoiler::NoSpoilerGuard;
pub use host::{PluginHost, RuntimePlugin};
pub use types::{
    ContextFilterSpec, ContributionMeta, FailPolicy, PluginContext, PluginContribution,
    PluginContributionKind, PluginHook, SafetyClass,
};

use std::sync::LazyLock;

/// 进程级内置 policy 插件 host（T2 接线）：seed 全部**无状态**内置插件。
///
/// 内置 policy 插件（如 `core.no_spoiler_guard`）无任何回合/会话状态，只读
/// `PluginContext` 快照产 propose 贡献，故可安全跨回合共享一个进程级实例（与
/// turn_trace.rs 的 SHADOW_REGISTRY 同模式）。turn pipeline 经 `builtin_plugin_host()`
/// 取它跑 hook。注意：声明式 `.md` 风格插件仍走 `plugins::load_gm_skill_with_plugins`，
/// 二者并行——内置 policy 插件走本 host，prose `.md` 片段走文件路径。
static PLUGIN_HOST: LazyLock<PluginHost> = LazyLock::new(|| {
    let mut host = PluginHost::new();
    host.register(Box::new(NoSpoilerGuard));
    host
});

/// 取进程级内置 policy 插件 host（只读共享引用）。
pub fn builtin_plugin_host() -> &'static PluginHost {
    &PLUGIN_HOST
}

#[cfg(test)]
mod host_seed_tests {
    use super::*;

    /// 进程级 host 已 seed NoSpoilerGuard：模组 ContextAssembly → 1 条贡献。
    #[tokio::test]
    async fn builtin_host_seeds_no_spoiler_guard() {
        let ctx = PluginContext {
            module_id: Some("mod".into()),
            hook: PluginHook::ContextAssembly,
            ..Default::default()
        };
        let out = builtin_plugin_host().run_hook(&ctx).await;
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].meta.plugin_id, "core.no_spoiler_guard");
    }
}

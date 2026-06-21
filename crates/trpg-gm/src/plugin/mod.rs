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

pub mod builtin_no_mechanical;
pub mod builtin_no_spoiler;
pub mod builtin_pacing;
pub mod builtin_player_agency;
pub mod builtin_scene_boundary;
pub mod builtin_source_backed;
pub mod host;
pub mod spoiler_source;
pub mod types;

pub use builtin_no_mechanical::NoMechanicalInvention;
pub use builtin_no_spoiler::NoSpoilerGuard;
pub use builtin_pacing::PacingPlugin;
pub use builtin_player_agency::PlayerAgencyGuard;
pub use builtin_scene_boundary::SceneBoundaryGuard;
pub use builtin_source_backed::SourceBackedRulesGuard;
pub use host::{PluginHost, RuntimePlugin};
pub use spoiler_source::{
    derive_scene_block_view, harvest_module_secret_terms, scene_node_id_from_block,
};
pub use types::{
    BeatWeightProposal, BeatWeightTerm, ContextFilterSpec, ContributionMeta, FailPolicy,
    PluginContext, PluginContribution, PluginContributionKind, PluginHook, PrivateBlockView,
    SafetyClass, SecretTerm,
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
    // 通用纯 prompt 守卫（always-on，无模组门）：不私造机械结果 + 玩家自主权 + 规则来源可考。
    host.register(Box::new(NoMechanicalInvention));
    host.register(Box::new(PlayerAgencyGuard));
    host.register(Box::new(SourceBackedRulesGuard));
    // 模组门控纯 prompt 守卫：场景边界（只对有预设场景图的模组生效）。
    host.register(Box::new(SceneBoundaryGuard));
    host
});

/// 取进程级内置 policy 插件 host（只读共享引用）。
pub fn builtin_plugin_host() -> &'static PluginHost {
    &PLUGIN_HOST
}

#[cfg(test)]
mod host_seed_tests {
    use super::*;

    fn assembly_ctx(module: Option<&str>) -> PluginContext {
        PluginContext {
            module_id: module.map(|s| s.to_string()),
            hook: PluginHook::ContextAssembly,
            ..Default::default()
        }
    }

    /// 进程级 host 已 seed 五个内置插件。
    #[test]
    fn builtin_host_seeds_five_plugins() {
        assert_eq!(builtin_plugin_host().len(), 5);
    }

    /// 非模组 session：always-on 三守卫产 3 条（no_spoiler/scene_boundary 是模组门，不产）。
    #[tokio::test]
    async fn builtin_host_non_module_emits_universal_guards() {
        let out = builtin_plugin_host().run_hook(&assembly_ctx(None)).await;
        let ids: Vec<&str> = out.iter().map(|c| c.meta.plugin_id.as_str()).collect();
        // 同为 Safety 级，按 priority 降序：
        // no_mech(850) > player_agency(800) > source_backed(750)。
        assert_eq!(
            ids,
            vec![
                "core.no_mechanical_invention",
                "core.player_agency_guard",
                "core.source_backed_rules_guard",
            ]
        );
    }

    /// 模组 session：五守卫全产 5 条，且按 (safety_class, priority desc) 排序。
    #[tokio::test]
    async fn builtin_host_module_emits_all_ordered() {
        let out = builtin_plugin_host()
            .run_hook(&assembly_ctx(Some("mod")))
            .await;
        let ids: Vec<&str> = out.iter().map(|c| c.meta.plugin_id.as_str()).collect();
        // 全 Safety 级 → priority 降序：no_spoiler(900) > no_mech(850) >
        // player_agency(800) > source_backed(750) > scene_boundary(700)。
        assert_eq!(
            ids,
            vec![
                "core.no_spoiler_guard",
                "core.no_mechanical_invention",
                "core.player_agency_guard",
                "core.source_backed_rules_guard",
                "core.scene_boundary_guard",
            ]
        );
    }
}

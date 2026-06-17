//! 内置 policy 插件 `core.scene_boundary_guard`（纯 prompt 通用守卫）：
//! 防止 GM 在场景尚未切换前提前引入或描述未来场景/未抵达地点的具体内容。
//!
//! 设计：advisor proposal §12 first-batch 通用 solo-GM 不变量。对标
//! `builtin_no_spoiler.rs` 的 PromptBlock 半边：场景边界仅对有预设场景图的模组/
//! 剧本有意义，故 **module-gated**（`module_id.is_some()` 时才注入）。
//!
//! - 仅在 `ContextAssembly` hook 且 `module_id.is_some()` 时产出一条 PromptBlock
//!   贡献，承载场景边界守则；否则 `vec![]`。
//! - 零规则集硬编码：只按 `module_id` 是否存在（通用）触发，文本是 const 数据。
//! - 纯 prompt（只 PromptBlock，无 filter/verifier）。

use async_trait::async_trait;
use trpg_model::{
    BlockContent, BlockKind, CacheZone, ContextBlock, Scope, Stability, Visibility,
};

use super::host::RuntimePlugin;
use super::types::{
    ContributionMeta, FailPolicy, PluginContext, PluginContribution, PluginContributionKind,
    PluginHook, SafetyClass,
};

/// 本插件的稳定通用 id（非规则集/模组名）。
pub const SCENE_BOUNDARY_GUARD_ID: &str = "core.scene_boundary_guard";
/// 产出的 PromptBlock 的 block_id。
pub const SCENE_BOUNDARY_BLOCK_ID: &str = "plugin.scene_boundary_guard";

/// 场景边界守则正文（面向 GM/模型的 policy 指令，单一事实源 const 数据）。
const SCENE_BOUNDARY_GUIDANCE: &str = r#"## 场景边界守则
只用当前场景的内容来叙事；在场景尚未切换前，不要提前引入或描述未来场景/未抵达地点的具体内容：
- 当前场景能感知的、已发生的，照常描写。
- 未来章节/未抵达场景的事件、布景、NPC 出场，等真正切换到那个场景再呈现。"#;

/// 场景边界守卫（内置 policy 插件，Safety 等级，模组门控）。
pub struct SceneBoundaryGuard;

impl SceneBoundaryGuard {
    /// 构造承载场景边界守则的 PromptBlock：BP1/Prefix-stable（policy 文本跨回合稳定，
    /// 不打缓存），SystemOnly（GM/模型可见的 policy 指令，非玩家可见念白），高 priority。
    fn guidance_block() -> ContextBlock {
        ContextBlock::new(
            SCENE_BOUNDARY_BLOCK_ID,
            BlockKind::GmOnboarding,
            "场景边界守则",
            BlockContent::Markdown(SCENE_BOUNDARY_GUIDANCE.to_string()),
            Visibility::SystemOnly,
            Stability::Immutable,
            CacheZone::Prefix,
            Scope::global(),
            700,
        )
    }
}

#[async_trait]
impl RuntimePlugin for SceneBoundaryGuard {
    fn id(&self) -> &'static str {
        SCENE_BOUNDARY_GUARD_ID
    }

    fn safety_class(&self) -> SafetyClass {
        SafetyClass::Safety
    }

    fn hooks(&self) -> &'static [PluginHook] {
        // 纯 prompt：只接 ContextAssembly（PromptBlock）。
        &[PluginHook::ContextAssembly]
    }

    async fn on_hook(&self, ctx: &PluginContext) -> Vec<PluginContribution> {
        // 仅在 ContextAssembly 且有模组（预设场景图）时注入场景边界守则；否则不贡献。
        if ctx.hook != PluginHook::ContextAssembly || ctx.module_id.is_none() {
            return vec![];
        }
        vec![PluginContribution {
            meta: ContributionMeta {
                plugin_id: SCENE_BOUNDARY_GUARD_ID.to_string(),
                hook: PluginHook::ContextAssembly,
                priority: 700,
                safety_class: SafetyClass::Safety,
                fail_policy: FailPolicy::Ignore,
                source_refs: vec![],
            },
            kind: PluginContributionKind::PromptBlock(Self::guidance_block()),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module_ctx() -> PluginContext {
        PluginContext {
            module_id: Some("mod".into()),
            hook: PluginHook::ContextAssembly,
            ..Default::default()
        }
    }

    fn non_module_ctx() -> PluginContext {
        PluginContext {
            module_id: None,
            hook: PluginHook::ContextAssembly,
            ..Default::default()
        }
    }

    /// 模组 session → 1 条 PromptBlock 含场景边界守则；非模组 → 空。
    #[tokio::test]
    async fn scene_boundary_emits_for_module_only() {
        let guard = SceneBoundaryGuard;

        let out = guard.on_hook(&module_ctx()).await;
        assert_eq!(out.len(), 1, "模组 session 应产 1 条贡献");
        let c = &out[0];
        assert_eq!(c.meta.plugin_id, SCENE_BOUNDARY_GUARD_ID);
        assert_eq!(c.meta.hook, PluginHook::ContextAssembly);
        assert_eq!(c.meta.safety_class, SafetyClass::Safety);
        assert_eq!(c.meta.fail_policy, FailPolicy::Ignore);
        assert_eq!(c.meta.priority, 700);
        match &c.kind {
            PluginContributionKind::PromptBlock(b) => {
                assert_eq!(b.block_id, SCENE_BOUNDARY_BLOCK_ID);
                assert_eq!(b.cache_zone, CacheZone::Prefix);
                assert_eq!(b.stability, Stability::Immutable);
                assert_eq!(b.visibility, Visibility::SystemOnly);
                assert!(
                    b.content.render_text().contains("场景边界守则"),
                    "block 内容须含场景边界守则原文"
                );
            }
            other => panic!("expected PromptBlock, got {other:?}"),
        }

        // 非模组 session → 空（场景边界只对有预设场景图的模组生效）。
        assert!(
            guard.on_hook(&non_module_ctx()).await.is_empty(),
            "非模组 session 不应产贡献"
        );
    }

    /// 非 ContextAssembly hook（AfterLlmStream，即便有模组）→ 空。
    #[tokio::test]
    async fn scene_boundary_empty_for_other_hooks() {
        let ctx = PluginContext {
            module_id: Some("mod".into()),
            hook: PluginHook::AfterLlmStream,
            ..Default::default()
        };
        assert!(SceneBoundaryGuard.on_hook(&ctx).await.is_empty());
    }
}

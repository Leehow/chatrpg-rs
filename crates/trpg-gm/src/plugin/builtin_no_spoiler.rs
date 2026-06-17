//! 内置 policy 插件 `core.no_spoiler_guard`（T2）：防剧透守则的 **PromptBlock** 半边。
//!
//! 设计：`docs/superpowers/specs/2026-06-17-policy-plugin-host-design.md` §4.2/§4.3。
//! 把原 `data/agent/plugins/anti_spoiler.md`（applies_when:module）的正文逐字迁入
//! 本插件，使防剧透引导改走 PluginHost 路径（取代 .md，避免双注入）。
//!
//! - 仅在 `ContextAssembly` hook 且 `module_id.is_some()`（有预设剧情的模组/剧本）
//!   时产出一条 PromptBlock 贡献，承载防剧透引导文本；否则 `vec![]`。
//! - 零规则集硬编码：只按 `module_id` 是否存在（通用）触发，文本是 const 数据。
//! - T3 再补 `AfterLlmStream` 的 SecretLeak verifier 半边（本 T2 不做）。

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
pub const NO_SPOILER_GUARD_ID: &str = "core.no_spoiler_guard";
/// 产出的 PromptBlock 的 block_id。
pub const NO_SPOILER_BLOCK_ID: &str = "plugin.no_spoiler_guard";

/// 防剧透引导正文（逐字搬自原 `data/agent/plugins/anti_spoiler.md` body）。
/// 这是面向 GM/模型的 policy 指令文本——单一事实源，删 .md 后这里是唯一来源。
const ANTI_SPOILER_GUIDANCE: &str = r#"## 防剧透（模组叙事守则）

你在跑一个有预设剧情的模组/剧本。把模组中**尚未在游戏中揭示**的内容当作隐藏真相，不要提前泄露：
- 反转、幕后黑手、隐藏动机、秘密关系、未来事件、尚未被发现的线索/地点/NPC 真实身份。
- 只叙述玩家角色**当前能合理感知**的，或**此前已经得知/发现**的信息。
- 不要以旁白口吻预告背景设定或"接下来会发生什么"。

**但不要矫枉过正**：
- 该由剧情自然揭示时，正常揭示——线索被找到、NPC 被识破、场景被触发时就如实呈现，不要为了"保密"而憋着、含糊其辞或拒绝推进。
- 玩家已经知道的事可以自由复述。公开、显而易见的环境信息照常描写。
- 拿不准时，倾向于"让故事自然流动"，而非过度隐瞒——过度防剧透会让游戏僵硬难玩。"#;

/// 防剧透守卫（内置 policy 插件，Safety 等级）。
pub struct NoSpoilerGuard;

impl NoSpoilerGuard {
    /// 构造承载防剧透引导的 PromptBlock：BP1/Prefix-stable（policy 文本跨回合稳定，
    /// 不打缓存），SystemOnly（GM/模型可见的 policy 指令，非玩家可见念白），高 priority。
    fn guidance_block() -> ContextBlock {
        ContextBlock::new(
            NO_SPOILER_BLOCK_ID,
            BlockKind::GmOnboarding,
            "防剧透（模组叙事守则）",
            BlockContent::Markdown(ANTI_SPOILER_GUIDANCE.to_string()),
            Visibility::SystemOnly,
            Stability::Immutable,
            CacheZone::Prefix,
            Scope::global(),
            900,
        )
    }
}

#[async_trait]
impl RuntimePlugin for NoSpoilerGuard {
    fn id(&self) -> &'static str {
        NO_SPOILER_GUARD_ID
    }

    fn safety_class(&self) -> SafetyClass {
        SafetyClass::Safety
    }

    fn hooks(&self) -> &'static [PluginHook] {
        // T2 只接 ContextAssembly（PromptBlock）；AfterLlmStream verifier 半边留 T3。
        &[PluginHook::ContextAssembly]
    }

    async fn on_hook(&self, ctx: &PluginContext) -> Vec<PluginContribution> {
        // 仅在 ContextAssembly 且有模组（预设剧情）时注入防剧透守则；否则不贡献。
        if ctx.hook != PluginHook::ContextAssembly || ctx.module_id.is_none() {
            return vec![];
        }
        vec![PluginContribution {
            meta: ContributionMeta {
                plugin_id: NO_SPOILER_GUARD_ID.to_string(),
                hook: PluginHook::ContextAssembly,
                priority: 900,
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
    use crate::plugin::host::PluginHost;

    fn module_ctx() -> PluginContext {
        PluginContext {
            session_id: "sess".into(),
            turn_id: "turn".into(),
            ruleset_id: "rs".into(),
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

    /// 模组 session → 1 条 PromptBlock 含防剧透引导；非模组 → 空。
    #[tokio::test]
    async fn no_spoiler_guard_emits_prompt_block_for_module_only() {
        let guard = NoSpoilerGuard;

        let out = guard.on_hook(&module_ctx()).await;
        assert_eq!(out.len(), 1, "模组 session 应产 1 条贡献");
        let c = &out[0];
        assert_eq!(c.meta.plugin_id, NO_SPOILER_GUARD_ID);
        assert_eq!(c.meta.hook, PluginHook::ContextAssembly);
        assert_eq!(c.meta.safety_class, SafetyClass::Safety);
        assert_eq!(c.meta.fail_policy, FailPolicy::Ignore);
        assert_eq!(c.meta.priority, 900);
        match &c.kind {
            PluginContributionKind::PromptBlock(b) => {
                assert_eq!(b.block_id, NO_SPOILER_BLOCK_ID);
                assert_eq!(b.cache_zone, CacheZone::Prefix);
                assert_eq!(b.visibility, Visibility::SystemOnly);
                assert!(
                    b.content.render_text().contains("防剧透"),
                    "block 内容须含防剧透引导原文"
                );
            }
            other => panic!("expected PromptBlock, got {other:?}"),
        }

        // 非模组 session → 空（防剧透只对有预设剧情的模组生效）。
        assert!(
            guard.on_hook(&non_module_ctx()).await.is_empty(),
            "非模组 session 不应产贡献"
        );
    }

    /// host 集成：注册 NoSpoilerGuard 后跑 ContextAssembly(module) hook → 1 条贡献。
    #[tokio::test]
    async fn host_with_no_spoiler_guard_emits_for_module() {
        let mut host = PluginHost::new();
        host.register(Box::new(NoSpoilerGuard));
        let out = host.run_hook(&module_ctx()).await;
        assert_eq!(out.len(), 1, "host 经 NoSpoilerGuard 应产 1 条贡献");
        assert_eq!(out[0].meta.plugin_id, NO_SPOILER_GUARD_ID);

        // 非模组：host 跑同 hook → 空。
        assert!(host.run_hook(&non_module_ctx()).await.is_empty());

        // AfterLlmStream（T2 未声明该 hook）：host 不调用本插件 → 空。
        let stream_ctx = PluginContext {
            module_id: Some("mod".into()),
            hook: PluginHook::AfterLlmStream,
            ..Default::default()
        };
        assert!(
            host.run_hook(&stream_ctx).await.is_empty(),
            "T2 NoSpoilerGuard 不挂 AfterLlmStream"
        );
    }

    /// 贡献的 to_trace() 摘要合理（kind=prompt_block、summary=block 标题、hook 稳定串）。
    #[tokio::test]
    async fn no_spoiler_contribution_to_trace_is_sensible() {
        let out = NoSpoilerGuard.on_hook(&module_ctx()).await;
        let trace = out[0].to_trace();
        assert_eq!(trace.plugin_id, NO_SPOILER_GUARD_ID);
        assert_eq!(trace.hook, "context_assembly");
        assert_eq!(trace.kind, "prompt_block");
        assert_eq!(trace.summary, "防剧透（模组叙事守则）");
    }
}

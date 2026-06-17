//! 内置 policy 插件 `core.source_backed_rules_guard`（纯 prompt 通用守卫）：
//! 防止 GM 在规则不清楚或查不到时凭空编造具体规则/DV/DC/数值/判定方式。
//!
//! 设计：advisor proposal §12 first-batch 通用 solo-GM 不变量。对标
//! `builtin_no_mechanical.rs` 的 PromptBlock 半边，但 **无模组门**——规则来源可考
//! 是所有 session 通用的不变量，故 always-on（每个 session 的 gm_skill 都注入）。
//!
//! - 仅在 `ContextAssembly` hook 产出一条 PromptBlock 贡献；其它 hook → `vec![]`。
//! - 零规则集/模组硬编码：无任何分支，文本是 const 数据，对所有 session 一致。
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
pub const SOURCE_BACKED_RULES_GUARD_ID: &str = "core.source_backed_rules_guard";
/// 产出的 PromptBlock 的 block_id。
pub const SOURCE_BACKED_BLOCK_ID: &str = "plugin.source_backed_rules_guard";

/// 规则来源守则正文（面向 GM/模型的 policy 指令，单一事实源 const 数据）。
const SOURCE_BACKED_GUIDANCE: &str = r#"## 规则来源守则
规则不清楚或查不到时，不要凭空编造具体规则、DV/DC、数值或判定方式：
- 优先用来源可考的规则；缺失时进入"引导式裁定"，说明这是临时裁断，或向玩家确认。
- 不要假装存在某条精确规则；宁可承认不确定，也不编造权威数字。"#;

/// 规则来源守卫（内置 policy 插件，Safety 等级，always-on）。
pub struct SourceBackedRulesGuard;

impl SourceBackedRulesGuard {
    /// 构造承载规则来源守则的 PromptBlock：BP1/Prefix-stable（policy 文本跨回合稳定，
    /// 不打缓存），SystemOnly（GM/模型可见的 policy 指令，非玩家可见念白），高 priority。
    fn guidance_block() -> ContextBlock {
        ContextBlock::new(
            SOURCE_BACKED_BLOCK_ID,
            BlockKind::GmOnboarding,
            "规则来源守则",
            BlockContent::Markdown(SOURCE_BACKED_GUIDANCE.to_string()),
            Visibility::SystemOnly,
            Stability::Immutable,
            CacheZone::Prefix,
            Scope::global(),
            750,
        )
    }
}

#[async_trait]
impl RuntimePlugin for SourceBackedRulesGuard {
    fn id(&self) -> &'static str {
        SOURCE_BACKED_RULES_GUARD_ID
    }

    fn safety_class(&self) -> SafetyClass {
        SafetyClass::Safety
    }

    fn hooks(&self) -> &'static [PluginHook] {
        // 纯 prompt：只接 ContextAssembly（PromptBlock）。
        &[PluginHook::ContextAssembly]
    }

    async fn on_hook(&self, ctx: &PluginContext) -> Vec<PluginContribution> {
        // always-on：仅按 hook 触发（无模组门）；非 ContextAssembly → 不贡献。
        if ctx.hook != PluginHook::ContextAssembly {
            return vec![];
        }
        vec![PluginContribution {
            meta: ContributionMeta {
                plugin_id: SOURCE_BACKED_RULES_GUARD_ID.to_string(),
                hook: PluginHook::ContextAssembly,
                priority: 750,
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

    fn assembly_ctx(module: Option<&str>) -> PluginContext {
        PluginContext {
            module_id: module.map(|s| s.to_string()),
            hook: PluginHook::ContextAssembly,
            ..Default::default()
        }
    }

    /// always-on：ContextAssembly（含模组 None 与 Some）都产 1 条 PromptBlock。
    #[tokio::test]
    async fn source_backed_emits_for_all_sessions() {
        let guard = SourceBackedRulesGuard;

        for module in [None, Some("mod")] {
            let out = guard.on_hook(&assembly_ctx(module)).await;
            assert_eq!(out.len(), 1, "module={module:?} 应产 1 条贡献（always-on）");
            let c = &out[0];
            assert_eq!(c.meta.plugin_id, SOURCE_BACKED_RULES_GUARD_ID);
            assert_eq!(c.meta.hook, PluginHook::ContextAssembly);
            assert_eq!(c.meta.safety_class, SafetyClass::Safety);
            assert_eq!(c.meta.fail_policy, FailPolicy::Ignore);
            assert_eq!(c.meta.priority, 750);
            match &c.kind {
                PluginContributionKind::PromptBlock(b) => {
                    assert_eq!(b.block_id, SOURCE_BACKED_BLOCK_ID);
                    assert_eq!(b.cache_zone, CacheZone::Prefix);
                    assert_eq!(b.stability, Stability::Immutable);
                    assert_eq!(b.visibility, Visibility::SystemOnly);
                    assert!(
                        b.content.render_text().contains("规则来源守则"),
                        "block 内容须含规则来源守则原文"
                    );
                }
                other => panic!("expected PromptBlock, got {other:?}"),
            }
        }
    }

    /// 非 ContextAssembly hook（AfterLlmStream）→ 空。
    #[tokio::test]
    async fn source_backed_empty_for_other_hooks() {
        let ctx = PluginContext {
            hook: PluginHook::AfterLlmStream,
            ..Default::default()
        };
        assert!(SourceBackedRulesGuard.on_hook(&ctx).await.is_empty());
    }
}

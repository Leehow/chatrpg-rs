//! 内置 policy 插件 `core.no_mechanical_invention`（纯 prompt 通用守卫）：
//! 防止 GM 在叙事里替引擎私自宣布未经结算落账的机械结果。
//!
//! 设计：advisor proposal §12 first-batch 通用 solo-GM 不变量。对标
//! `builtin_no_spoiler.rs` 的 PromptBlock 半边，但 **无模组门**——不私造机械结果
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
pub const NO_MECHANICAL_INVENTION_ID: &str = "core.no_mechanical_invention";
/// 产出的 PromptBlock 的 block_id。
pub const NO_MECHANICAL_BLOCK_ID: &str = "plugin.no_mechanical_invention";

/// 机械结果守则正文（面向 GM/模型的 policy 指令，单一事实源 const 数据）。
const NO_MECHANICAL_GUIDANCE: &str = r#"## 机械结果守则
不要在叙事里替引擎宣布任何未经结算落账的机械结果：
- 不说"你掉了 X 点 HP / 你死了 / 检定成功或失败 / 你获得了某物 / 敌人被击杀"，除非这些已由掷骰/检定/施效工具结算并落账。
- 需要判定时，发起检定/掷骰，让工具给出结果，再据实叙述。
- 不确定数值或成败时，用可观察的描写表达不确定，不要编造具体数字或结局。"#;

/// 防私造机械结果守卫（内置 policy 插件，Safety 等级，always-on）。
pub struct NoMechanicalInvention;

impl NoMechanicalInvention {
    /// 构造承载机械结果守则的 PromptBlock：BP1/Prefix-stable（policy 文本跨回合稳定，
    /// 不打缓存），SystemOnly（GM/模型可见的 policy 指令，非玩家可见念白），高 priority。
    fn guidance_block() -> ContextBlock {
        ContextBlock::new(
            NO_MECHANICAL_BLOCK_ID,
            BlockKind::GmOnboarding,
            "机械结果守则",
            BlockContent::Markdown(NO_MECHANICAL_GUIDANCE.to_string()),
            Visibility::SystemOnly,
            Stability::Immutable,
            CacheZone::Prefix,
            Scope::global(),
            850,
        )
    }
}

#[async_trait]
impl RuntimePlugin for NoMechanicalInvention {
    fn id(&self) -> &'static str {
        NO_MECHANICAL_INVENTION_ID
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
                plugin_id: NO_MECHANICAL_INVENTION_ID.to_string(),
                hook: PluginHook::ContextAssembly,
                priority: 850,
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
    async fn no_mechanical_emits_for_all_sessions() {
        let guard = NoMechanicalInvention;

        for module in [None, Some("mod")] {
            let out = guard.on_hook(&assembly_ctx(module)).await;
            assert_eq!(out.len(), 1, "module={module:?} 应产 1 条贡献（always-on）");
            let c = &out[0];
            assert_eq!(c.meta.plugin_id, NO_MECHANICAL_INVENTION_ID);
            assert_eq!(c.meta.hook, PluginHook::ContextAssembly);
            assert_eq!(c.meta.safety_class, SafetyClass::Safety);
            assert_eq!(c.meta.fail_policy, FailPolicy::Ignore);
            assert_eq!(c.meta.priority, 850);
            match &c.kind {
                PluginContributionKind::PromptBlock(b) => {
                    assert_eq!(b.block_id, NO_MECHANICAL_BLOCK_ID);
                    assert_eq!(b.cache_zone, CacheZone::Prefix);
                    assert_eq!(b.stability, Stability::Immutable);
                    assert_eq!(b.visibility, Visibility::SystemOnly);
                    assert!(
                        b.content.render_text().contains("机械结果守则"),
                        "block 内容须含机械结果守则原文"
                    );
                }
                other => panic!("expected PromptBlock, got {other:?}"),
            }
        }
    }

    /// 非 ContextAssembly hook（AfterLlmStream）→ 空。
    #[tokio::test]
    async fn no_mechanical_empty_for_other_hooks() {
        let ctx = PluginContext {
            hook: PluginHook::AfterLlmStream,
            ..Default::default()
        };
        assert!(NoMechanicalInvention.on_hook(&ctx).await.is_empty());
    }
}

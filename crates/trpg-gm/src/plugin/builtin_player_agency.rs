//! 内置 policy 插件 `core.player_agency_guard`（纯 prompt 通用守卫）：
//! 防止 GM 替玩家角色做决定、说台词或采取重大行动（玩家自主权）。
//!
//! 设计：advisor proposal §12 first-batch 通用 solo-GM 不变量。对标
//! `builtin_no_spoiler.rs` 的 PromptBlock 半边，但 **无模组门**——尊重玩家自主权
//! 是所有 session 通用的不变量，故 always-on（每个 session 的 gm_skill 都注入）。
//!
//! - 仅在 `ContextAssembly` hook 产出一条 PromptBlock 贡献；其它 hook → `vec![]`。
//! - 零规则集/模组硬编码：无任何分支，文本是 const 数据，对所有 session 一致。
//! - 纯 prompt（只 PromptBlock，无 filter/verifier）。

use async_trait::async_trait;
use trpg_model::{BlockContent, BlockKind, CacheZone, ContextBlock, Scope, Stability, Visibility};

use super::host::RuntimePlugin;
use super::types::{
    ContributionMeta, FailPolicy, PluginContext, PluginContribution, PluginContributionKind,
    PluginHook, SafetyClass,
};

/// 本插件的稳定通用 id（非规则集/模组名）。
pub const PLAYER_AGENCY_GUARD_ID: &str = "core.player_agency_guard";
/// 产出的 PromptBlock 的 block_id。
pub const PLAYER_AGENCY_BLOCK_ID: &str = "plugin.player_agency_guard";

/// 玩家自主权守则正文（面向 GM/模型的 policy 指令，单一事实源 const 数据）。
const PLAYER_AGENCY_GUIDANCE: &str = r#"## 玩家自主权守则
不要替玩家角色做决定、说台词或采取重大行动：
- 呈现情境、给出可感知的信息和后果，让玩家自己选择如何行动。
- 不替玩家宣布他的选择、内心想法、对话或重大动作。
- 可以推进 NPC 和世界的反应，但玩家角色的行动权留给玩家。
- 玩家当前位置也是行动权的一部分：以连续性锚、玩家最近行动和既成事实为权威；玩家已经在内部、纵深、机柜旁或其他明确位置时，不把他重置回入口、外景或旧开场图景。
- 世界状态同样服从既成事实：供电切断、灯灭、设备停止、接口未接通、物件已收起/放下等状态一旦成立，后续玩家措辞或失败尝试不能把它复活、反转或无声覆盖，除非有新的 runtime/工具事实明确改变它。
- 不提供显式选项菜单，不列编号、A/B/C 或“选择其一”的行动清单；除非规则或 runtime 明确打开 required choice/reaction gate。
- 不把线索、发现或模组内容写成“关键事实”式清单；把可感知事实织进场景，让玩家自己判断含义。"#;

/// 玩家自主权守卫（内置 policy 插件，Safety 等级，always-on）。
pub struct PlayerAgencyGuard;

impl PlayerAgencyGuard {
    /// 构造承载玩家自主权守则的 PromptBlock：BP1/Prefix-stable（policy 文本跨回合稳定，
    /// 不打缓存），SystemOnly（GM/模型可见的 policy 指令，非玩家可见念白），高 priority。
    fn guidance_block() -> ContextBlock {
        ContextBlock::new(
            PLAYER_AGENCY_BLOCK_ID,
            BlockKind::GmOnboarding,
            "玩家自主权守则",
            BlockContent::Markdown(PLAYER_AGENCY_GUIDANCE.to_string()),
            Visibility::SystemOnly,
            Stability::Immutable,
            CacheZone::Prefix,
            Scope::global(),
            800,
        )
    }
}

#[async_trait]
impl RuntimePlugin for PlayerAgencyGuard {
    fn id(&self) -> &'static str {
        PLAYER_AGENCY_GUARD_ID
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
                plugin_id: PLAYER_AGENCY_GUARD_ID.to_string(),
                hook: PluginHook::ContextAssembly,
                priority: 800,
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
    async fn player_agency_emits_for_all_sessions() {
        let guard = PlayerAgencyGuard;

        for module in [None, Some("mod")] {
            let out = guard.on_hook(&assembly_ctx(module)).await;
            assert_eq!(out.len(), 1, "module={module:?} 应产 1 条贡献（always-on）");
            let c = &out[0];
            assert_eq!(c.meta.plugin_id, PLAYER_AGENCY_GUARD_ID);
            assert_eq!(c.meta.hook, PluginHook::ContextAssembly);
            assert_eq!(c.meta.safety_class, SafetyClass::Safety);
            assert_eq!(c.meta.fail_policy, FailPolicy::Ignore);
            assert_eq!(c.meta.priority, 800);
            match &c.kind {
                PluginContributionKind::PromptBlock(b) => {
                    assert_eq!(b.block_id, PLAYER_AGENCY_BLOCK_ID);
                    assert_eq!(b.cache_zone, CacheZone::Prefix);
                    assert_eq!(b.stability, Stability::Immutable);
                    assert_eq!(b.visibility, Visibility::SystemOnly);
                    assert!(
                        b.content.render_text().contains("玩家自主权守则"),
                        "block 内容须含玩家自主权守则原文"
                    );
                }
                other => panic!("expected PromptBlock, got {other:?}"),
            }
        }
    }

    /// 非 ContextAssembly hook（AfterLlmStream）→ 空。
    #[tokio::test]
    async fn player_agency_empty_for_other_hooks() {
        let ctx = PluginContext {
            hook: PluginHook::AfterLlmStream,
            ..Default::default()
        };
        assert!(PlayerAgencyGuard.on_hook(&ctx).await.is_empty());
    }

    #[test]
    fn player_agency_guidance_forbids_menus_and_dumps() {
        assert!(
            PLAYER_AGENCY_GUIDANCE.contains("选项菜单"),
            "always-on agency guard must forbid explicit option menus"
        );
        assert!(
            PLAYER_AGENCY_GUIDANCE.contains("编号"),
            "always-on agency guard must forbid numbered action lists"
        );
        assert!(
            PLAYER_AGENCY_GUIDANCE.contains("清单"),
            "always-on agency guard must forbid raw clue/content dumps"
        );
    }

    #[test]
    fn player_agency_guidance_preserves_position_and_world_state() {
        assert!(
            PLAYER_AGENCY_GUIDANCE.contains("当前位置"),
            "always-on agency guard must preserve the player's current position"
        );
        assert!(
            PLAYER_AGENCY_GUIDANCE.contains("既成事实"),
            "always-on agency guard must treat committed facts as authoritative"
        );
        assert!(
            PLAYER_AGENCY_GUIDANCE.contains("世界状态"),
            "always-on agency guard must preserve established world/object state"
        );
    }
}

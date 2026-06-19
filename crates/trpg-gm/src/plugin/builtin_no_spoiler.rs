//! 内置 policy 插件 `core.no_spoiler_guard`（v2）：防剧透三段守则。
//!
//! 设计：`docs/superpowers/specs/2026-06-17-policy-plugin-host-design.md` §4.2/§4.3
//! + 任务卡 `TC-KNOW-03-no-spoiler-plugin-v2`。把防剧透从"纯 prompt 引导"升级为三段：
//!
//! 1. **ContextAssembly（before/装配）**：除 PromptBlock 外，对私有 block 元数据快照
//!    （`PluginContext.private_blocks`）做 **fail-closed** 分类——玩家未知的 secret 块、
//!    GM-only 未来场景块产 `ContextFilter` 删除贡献；已 player-known 的 fact 必放行。
//! 2. **ContextAssembly（during/编排）**：照旧产一条高优先 Safety `PromptBlock`，承载
//!    防剧透引导文本（带 priority / cache_zone / visibility / token budget 元数据）。
//! 3. **AfterLlmStream（after/校验）**：玩家可见念白若命中**私有**泄漏术语表
//!    （`PluginContext.secret_terms`）中尚未 player-known 的 fact，产 `SecretLeak`
//!    verifier finding（走既有 errata 路径，不同步硬拦流）。
//!
//! 零规则集硬编码：只按 `module_id` 是否存在触发 PromptBlock/Filter，分类靠**显式元数据/
//! 标注**驱动（保守删，避免模糊正文扫描）。私有 secret 数据只活在 `PluginContext` 的私有
//! 视图里，绝不渲染进面向模型的 prompt。

use async_trait::async_trait;
use trpg_agent::{VerifierFinding, VerifierFindingKind, VerifierSeverity};
use trpg_model::{BlockContent, BlockKind, CacheZone, ContextBlock, Scope, Stability, Visibility};

use super::host::RuntimePlugin;
use super::types::{
    ContextFilterSpec, ContributionMeta, FailPolicy, PluginContext, PluginContribution,
    PluginContributionKind, PluginHook, PrivateBlockView, SafetyClass,
};

/// 本插件的稳定通用 id（非规则集/模组名）。
pub const NO_SPOILER_GUARD_ID: &str = "core.no_spoiler_guard";
/// 产出的 PromptBlock 的 block_id。
pub const NO_SPOILER_BLOCK_ID: &str = "plugin.no_spoiler_guard";

/// 块级显式标注约定（`ContextBlock.tags`）：玩家未知隐藏真相块。
pub const TAG_SECRET: &str = "spoiler_secret";
/// 块级显式标注约定：未来场景内容块。
pub const TAG_FUTURE_SCENE: &str = "future_scene";
/// fact 绑定标注前缀：`fact:<id>`（已 player-known 则放行该块）。
pub const TAG_FACT_PREFIX: &str = "fact:";

/// 把一个 compiled [`ContextBlock`] 折成 fail-closed 过滤用的私有元数据快照。
///
/// 分类标注（secret / future_scene / fact_id）**仅取自显式 tag**，不做正文扫描。
/// 注意：生产 block 产出方今天尚未按这些 tag 约定打标，故装配过滤在生产里恒 no-op
/// （fail-safe，零行为变更）；待后续 block 产出方打标后自动生效。实时 secret 文本裁剪
/// 仍由 runtime `spoiler_guard` 在源实体级负责，本快照只做块级 fail-closed 兜底。
pub fn private_block_view(block: &ContextBlock) -> PrivateBlockView {
    let secret = block.tags.iter().any(|t| t == TAG_SECRET);
    let future_scene = block.tags.iter().any(|t| t == TAG_FUTURE_SCENE);
    let fact_id = block
        .tags
        .iter()
        .find_map(|t| t.strip_prefix(TAG_FACT_PREFIX).map(str::to_string));
    PrivateBlockView {
        block_id: block.block_id.clone(),
        visibility: block.visibility,
        cache_zone: block.cache_zone,
        tags: block.tags.clone(),
        source_refs: block.source_refs.clone(),
        load_reason: block.load_reason.clone(),
        fact_id,
        secret,
        future_scene,
    }
}

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

    /// 该私有 block 是否应在装配前 **fail-closed** 删除。
    ///
    /// 保守规则（显式元数据驱动，漏删优于过删之外更倾向"宁删不泄"）：
    /// - 已 player-known 的 fact 永远放行（不删已揭示）。
    /// - 玩家未知的显式 `secret` 块 → 删。
    /// - 未来场景（`future_scene`）且 GM-only / 非玩家可见 → 删。
    fn should_drop(view: &PrivateBlockView, player_known: &[String]) -> bool {
        // 已揭示/已知的 fact：放行（最高优先，绝不误删已揭示内容）。
        if let Some(fid) = &view.fact_id {
            if player_known.iter().any(|k| k == fid) {
                return false;
            }
        }
        // 玩家未知的显式 secret 块。
        if view.secret {
            return true;
        }
        // 未来场景 + 仅 GM/系统/NPC 私有可见（非玩家可见）。
        let not_player_facing = matches!(
            view.visibility,
            Visibility::GmOnly | Visibility::SystemOnly | Visibility::NpcPrivate
        );
        if view.future_scene && not_player_facing {
            return true;
        }
        false
    }

    /// before/装配阶段：对 `private_blocks` 分类产一条 ContextFilter（无命中则 None）。
    /// fail_policy 取现有枚举里最贴近 fail-closed 的 `RepairThenFailClosed`（无 `FailClosed`
    /// 档；过滤的 fail-closed 语义由 `should_drop` 的保守分类保证）。
    fn context_filter(ctx: &PluginContext) -> Option<PluginContribution> {
        let drop_block_ids: Vec<String> = ctx
            .private_blocks
            .iter()
            .filter(|v| Self::should_drop(v, &ctx.player_known_fact_ids))
            .map(|v| v.block_id.clone())
            .collect();
        if drop_block_ids.is_empty() {
            return None;
        }
        let count = drop_block_ids.len();
        Some(PluginContribution {
            meta: ContributionMeta {
                plugin_id: NO_SPOILER_GUARD_ID.to_string(),
                hook: PluginHook::ContextAssembly,
                priority: 900,
                safety_class: SafetyClass::Safety,
                fail_policy: FailPolicy::RepairThenFailClosed,
                source_refs: vec![],
            },
            // reason 不含 secret 正文，只报删除块数（GM 侧可观测，玩家不可见）。
            kind: PluginContributionKind::ContextFilter(ContextFilterSpec {
                drop_block_ids,
                reason: format!("no_spoiler: drop {count} player-unknown/gm-only-future block(s)"),
            }),
        })
    }

    /// PromptBlock 贡献（防剧透引导，FailPolicy::Ignore——纯引导失败可忽略）。
    fn prompt_block() -> PluginContribution {
        PluginContribution {
            meta: ContributionMeta {
                plugin_id: NO_SPOILER_GUARD_ID.to_string(),
                hook: PluginHook::ContextAssembly,
                priority: 900,
                safety_class: SafetyClass::Safety,
                fail_policy: FailPolicy::Ignore,
                source_refs: vec![],
            },
            kind: PluginContributionKind::PromptBlock(Self::guidance_block()),
        }
    }

    /// after/校验阶段：玩家可见念白命中**未 player-known** 的私有泄漏术语 → SecretLeak finding。
    /// detail 只引用 `fact_id`（id 非 secret 正文），不回写术语文本，避免经 errata 再次外溢。
    /// 与既有 NarrationVerifier 的 SecretLeak 同口径用 `Blocker`（走 errata 提醒，不同步硬拦流）。
    fn secret_leak_findings(ctx: &PluginContext) -> Vec<PluginContribution> {
        let Some(narration) = ctx.narration.as_deref() else {
            return vec![];
        };
        if narration.is_empty() {
            return vec![];
        }
        ctx.secret_terms
            .iter()
            .filter(|t| !t.term.is_empty() && narration.contains(t.term.as_str()))
            // 该术语的 fact 已 player-known/revealed → 不算泄漏（已揭示可自由复述）。
            .filter(|t| match &t.fact_id {
                Some(fid) => !ctx.player_known_fact_ids.iter().any(|k| k == fid),
                None => true,
            })
            .map(|t| {
                let fid = t.fact_id.as_deref().unwrap_or("<unbound>");
                PluginContribution {
                    meta: ContributionMeta {
                        plugin_id: NO_SPOILER_GUARD_ID.to_string(),
                        hook: PluginHook::AfterLlmStream,
                        priority: 900,
                        safety_class: SafetyClass::Safety,
                        fail_policy: FailPolicy::RepairThenFailClosed,
                        source_refs: vec![],
                    },
                    kind: PluginContributionKind::VerifierFinding(VerifierFinding {
                        kind: VerifierFindingKind::SecretLeak,
                        severity: VerifierSeverity::Blocker,
                        detail: format!(
                            "player-visible narration exposes a private secret for unrevealed fact '{fid}'"
                        ),
                    }),
                }
            })
            .collect()
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
        // v2：ContextAssembly（PromptBlock + ContextFilter）+ AfterLlmStream（SecretLeak verifier）。
        &[PluginHook::ContextAssembly, PluginHook::AfterLlmStream]
    }

    async fn on_hook(&self, ctx: &PluginContext) -> Vec<PluginContribution> {
        match ctx.hook {
            // 装配：仅有模组（预设剧情）时注入。PromptBlock + （命中时）ContextFilter。
            PluginHook::ContextAssembly => {
                if ctx.module_id.is_none() {
                    return vec![];
                }
                let mut out = vec![Self::prompt_block()];
                if let Some(filter) = Self::context_filter(ctx) {
                    out.push(filter);
                }
                out
            }
            // 流后：私有泄漏术语命中未揭示 fact → SecretLeak finding（不门控模组：
            // 无 secret_terms 即空贡献，生产零行为变更）。
            PluginHook::AfterLlmStream => Self::secret_leak_findings(ctx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::host::PluginHost;
    use crate::plugin::types::SecretTerm;

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

    fn secret_block(block_id: &str, fact_id: Option<&str>) -> PrivateBlockView {
        PrivateBlockView {
            block_id: block_id.into(),
            visibility: Visibility::GmOnly,
            cache_zone: CacheZone::DynamicTail,
            tags: vec!["module_scene".into()],
            source_refs: vec![],
            load_reason: Some("current_scene_deep_projection".into()),
            fact_id: fact_id.map(str::to_string),
            secret: true,
            future_scene: false,
        }
    }

    fn drop_ids(out: &[PluginContribution]) -> Vec<String> {
        out.iter()
            .filter_map(|c| match &c.kind {
                PluginContributionKind::ContextFilter(f) => Some(f.drop_block_ids.clone()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    /// 模组 session → 至少 1 条 PromptBlock 含防剧透引导；非模组 → 空。
    #[tokio::test]
    async fn no_spoiler_guard_emits_prompt_block_for_module_only() {
        let guard = NoSpoilerGuard;
        let out = guard.on_hook(&module_ctx()).await;
        let pb = out
            .iter()
            .find(|c| matches!(c.kind, PluginContributionKind::PromptBlock(_)))
            .expect("模组 session 应产 PromptBlock");
        assert_eq!(pb.meta.plugin_id, NO_SPOILER_GUARD_ID);
        assert_eq!(pb.meta.safety_class, SafetyClass::Safety);
        assert!(
            guard.on_hook(&non_module_ctx()).await.is_empty(),
            "非模组 session 不应产贡献"
        );
    }

    /// 玩家未知的 secret 块 → 被 ContextFilter 删（fail-closed）。
    #[tokio::test]
    async fn no_spoiler_filters_player_unknown_context() {
        let mut ctx = module_ctx();
        ctx.private_blocks = vec![secret_block("b.secret", Some("fact_villain"))];
        ctx.player_known_fact_ids = vec![]; // 玩家什么都不知道。
        let out = NoSpoilerGuard.on_hook(&ctx).await;
        assert_eq!(
            drop_ids(&out),
            vec!["b.secret".to_string()],
            "玩家未知 secret 块必须删"
        );
        // 该 ContextFilter 贡献是 Safety 级、fail-closed 语义。
        let filter = out
            .iter()
            .find(|c| matches!(c.kind, PluginContributionKind::ContextFilter(_)))
            .expect("应产 ContextFilter");
        assert_eq!(filter.meta.safety_class, SafetyClass::Safety);
        assert_eq!(filter.meta.fail_policy, FailPolicy::RepairThenFailClosed);
    }

    /// 已揭示（player-known）的 fact 块 → 放行（不删）。
    #[tokio::test]
    async fn no_spoiler_allows_revealed_fact() {
        let mut ctx = module_ctx();
        ctx.private_blocks = vec![secret_block("b.secret", Some("fact_villain"))];
        ctx.player_known_fact_ids = vec!["fact_villain".into()]; // 已揭示。
        let out = NoSpoilerGuard.on_hook(&ctx).await;
        assert!(drop_ids(&out).is_empty(), "已揭示 fact 必放行，不得删块");
    }

    /// 未来场景 GM-only 块（无 fact_id）→ 被删。
    #[tokio::test]
    async fn no_spoiler_filters_future_scene_gm_only() {
        let mut ctx = module_ctx();
        ctx.private_blocks = vec![PrivateBlockView {
            block_id: "b.future".into(),
            visibility: Visibility::GmOnly,
            cache_zone: CacheZone::DynamicTail,
            tags: vec!["module_scene".into(), "future".into()],
            source_refs: vec![],
            load_reason: None,
            fact_id: None,
            secret: false,
            future_scene: true,
        }];
        let out = NoSpoilerGuard.on_hook(&ctx).await;
        assert_eq!(
            drop_ids(&out),
            vec!["b.future".to_string()],
            "未来场景 GM-only 块必须删"
        );
    }

    /// PromptBlock 携带 priority / cache_zone / visibility / token budget 元数据。
    #[tokio::test]
    async fn no_spoiler_prompt_block_has_metadata() {
        let out = NoSpoilerGuard.on_hook(&module_ctx()).await;
        let pb = out
            .iter()
            .find_map(|c| match &c.kind {
                PluginContributionKind::PromptBlock(b) => Some((c.meta.priority, b)),
                _ => None,
            })
            .expect("应产 PromptBlock");
        let (meta_priority, block) = pb;
        assert_eq!(meta_priority, 900, "contribution priority");
        assert_eq!(block.priority, 900, "block priority");
        assert_eq!(block.cache_zone, CacheZone::Prefix, "cache_zone");
        assert_eq!(block.visibility, Visibility::SystemOnly, "visibility");
        assert!(
            block.token_estimate.is_some_and(|t| t > 0),
            "token budget 须存在且 > 0"
        );
        assert_eq!(block.block_id, NO_SPOILER_BLOCK_ID);
    }

    /// AfterLlmStream：念白命中未揭示私密术语 → SecretLeak finding；已揭示则放行。
    #[tokio::test]
    async fn no_spoiler_verifier_catches_secret_leak() {
        let guard = NoSpoilerGuard;
        let mut ctx = PluginContext {
            module_id: Some("mod".into()),
            hook: PluginHook::AfterLlmStream,
            narration: Some("管家其实是幕后真凶。".into()),
            ..Default::default()
        };
        ctx.secret_terms = vec![SecretTerm {
            term: "幕后真凶".into(),
            fact_id: Some("fact_villain".into()),
        }];

        // 玩家未知 → 命中泄漏。
        let out = guard.on_hook(&ctx).await;
        let finding = out
            .iter()
            .find_map(|c| match &c.kind {
                PluginContributionKind::VerifierFinding(vf) => Some(vf),
                _ => None,
            })
            .expect("应产 SecretLeak finding");
        assert_eq!(finding.kind, VerifierFindingKind::SecretLeak);
        assert_eq!(finding.severity, VerifierSeverity::Blocker);
        assert!(
            !finding.detail.contains("幕后真凶"),
            "finding detail 不得回写 secret 正文"
        );

        // 该 fact 已揭示 → 不再算泄漏。
        ctx.player_known_fact_ids = vec!["fact_villain".into()];
        assert!(guard.on_hook(&ctx).await.is_empty(), "已揭示 fact 不算泄漏");

        // 念白不含术语 → 空。
        let mut clean = ctx.clone();
        clean.player_known_fact_ids = vec![];
        clean.narration = Some("管家端上了茶。".into());
        assert!(guard.on_hook(&clean).await.is_empty(), "未命中术语不报");
    }

    /// host 集成：注册后 ContextAssembly(module) 产 PromptBlock；并验证 trace 折叠。
    #[tokio::test]
    async fn host_with_no_spoiler_guard_emits_and_traces() {
        let mut host = PluginHost::new();
        host.register(Box::new(NoSpoilerGuard));

        let mut ctx = module_ctx();
        ctx.private_blocks = vec![secret_block("b.secret", None)];
        let out = host.run_hook(&ctx).await;
        // PromptBlock + ContextFilter 两条贡献，均归属本插件。
        assert!(out.iter().all(|c| c.meta.plugin_id == NO_SPOILER_GUARD_ID));
        let kinds: Vec<&str> = out.iter().map(|c| c.kind.kind_str()).collect();
        assert!(kinds.contains(&"prompt_block"));
        assert!(kinds.contains(&"context_filter"));

        // 每条贡献都能折成有意义的 trace。
        for c in &out {
            let t = c.to_trace();
            assert_eq!(t.plugin_id, NO_SPOILER_GUARD_ID);
            assert_eq!(t.hook, "context_assembly");
            assert!(!t.summary.is_empty());
        }

        // 非模组：host 跑同 hook → 空。
        assert!(host.run_hook(&non_module_ctx()).await.is_empty());
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

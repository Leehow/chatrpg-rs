//! Policy Plugin Host v1（T1）：`RuntimePlugin` trait + `PluginHost`。
//!
//! 设计：spec §4.1。host **只收集 + 排序**贡献（propose-not-commit）；
//! 应用（prompt 追加 / context 删块 / finding→ErrataMemory）是 T2/T3 turn
//! pipeline 的职责，本 T1 不接线。

use async_trait::async_trait;

use super::types::{PluginContext, PluginContribution, PluginHook, SafetyClass};

/// 运行时插件：只 propose 贡献，绝不直接改状态/HP/scene/reveal/绕 BindingResolver。
#[async_trait]
pub trait RuntimePlugin: Send + Sync {
    /// 通用行为名（零规则集硬编码：不得是规则集/模组名）。
    fn id(&self) -> &'static str;
    /// 安全等级（仲裁排序权重）。
    fn safety_class(&self) -> SafetyClass;
    /// 本插件挂载的 hook 集（host 仅对命中 hook 的插件调用）。
    fn hooks(&self) -> &'static [PluginHook];
    /// 针对 `ctx.hook` 产出贡献；不处理该 hook 返回 `vec![]`。
    async fn on_hook(&self, ctx: &PluginContext) -> Vec<PluginContribution>;
}

/// 插件 host：注册可信 Rust 插件，按 hook 收集并仲裁排序贡献。
#[derive(Default)]
pub struct PluginHost {
    plugins: Vec<Box<dyn RuntimePlugin>>,
}

impl PluginHost {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// 注册一个插件（链式）。
    pub fn register(&mut self, plugin: Box<dyn RuntimePlugin>) -> &mut Self {
        self.plugins.push(plugin);
        self
    }

    /// 已注册插件数。
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// 跑某 hook：对所有 `hooks()` 含 `ctx.hook` 的插件调用 `on_hook`，汇总贡献，
    /// 再按 **(safety_class ASC [Core 先], priority DESC)** 排序。
    ///
    /// host **不应用**贡献（T2/T3 的活），只返回排好序的 Vec。
    pub async fn run_hook(&self, ctx: &PluginContext) -> Vec<PluginContribution> {
        let mut out: Vec<PluginContribution> = Vec::new();
        for plugin in &self.plugins {
            if plugin.hooks().contains(&ctx.hook) {
                out.extend(plugin.on_hook(ctx).await);
            }
        }
        // safety_class 升序（Core 最先），同级 priority 降序（高优先先应用）。
        // 稳定排序：保留同 (safety_class, priority) 下的插件注册顺序。
        out.sort_by(|a, b| {
            a.meta
                .safety_class
                .cmp(&b.meta.safety_class)
                .then_with(|| b.meta.priority.cmp(&a.meta.priority))
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::types::{
        ContextFilterSpec, ContributionMeta, FailPolicy, PluginContributionKind,
    };
    use trpg_agent::{VerifierFinding, VerifierFindingKind, VerifierSeverity};
    use trpg_model::{
        BlockContent, BlockKind, CacheZone, ContextBlock, MemoryExtractionProposal, Scope,
        SourceRef, Stability, Visibility, WorldFactCandidate,
    };

    /// 一个最小的 WorldFact 提案：subject/object 故意夹"秘密"正文，用来证明
    /// trace 摘要只暴露稳定的 fact_id、绝不回显 secret 正文。
    fn secret_world_fact_proposal() -> MemoryExtractionProposal {
        MemoryExtractionProposal::WorldFact(WorldFactCandidate {
            fact_id: "fact.king_identity".to_string(),
            subject: "the beggar".to_string(),
            predicate: "is".to_string(),
            object: "the hidden king".to_string(),
            summary: "secret twist prose".to_string(),
            confidence: Some(0.9),
            source_event_ids: vec!["evt-1".to_string()],
            turn_id: Some("t-1".to_string()),
        })
    }

    fn meta(id: &str, safety: SafetyClass, prio: i32, hook: PluginHook) -> ContributionMeta {
        ContributionMeta {
            plugin_id: id.to_string(),
            hook,
            priority: prio,
            safety_class: safety,
            fail_policy: FailPolicy::default(),
            source_refs: vec![],
        }
    }

    fn prompt_block(id: &str, title: &str) -> ContextBlock {
        ContextBlock::new(
            id,
            BlockKind::GmOnboarding,
            title,
            BlockContent::Text("body".to_string()),
            Visibility::GmOnly,
            Stability::SceneStable,
            CacheZone::Prefix,
            Scope::global(),
            0,
        )
    }

    /// 一个可参数化的假插件：声明若干 hook，并在 on_hook 时产出一条预置贡献。
    struct FakePlugin {
        plugin_id: &'static str,
        safety: SafetyClass,
        priority: i32,
        hooks: &'static [PluginHook],
        emit_hook: PluginHook,
        kind: &'static str, // "prompt" | "filter" | "finding" | "proposal"
    }

    #[async_trait]
    impl RuntimePlugin for FakePlugin {
        fn id(&self) -> &'static str {
            self.plugin_id
        }
        fn safety_class(&self) -> SafetyClass {
            self.safety
        }
        fn hooks(&self) -> &'static [PluginHook] {
            self.hooks
        }
        async fn on_hook(&self, ctx: &PluginContext) -> Vec<PluginContribution> {
            // 只为它实际声明要产出的 hook 发贡献。
            if ctx.hook != self.emit_hook {
                return vec![];
            }
            let kind = match self.kind {
                "filter" => PluginContributionKind::ContextFilter(ContextFilterSpec {
                    drop_block_ids: vec!["secret_block".to_string()],
                    reason: "gm only".to_string(),
                }),
                "finding" => PluginContributionKind::VerifierFinding(VerifierFinding {
                    kind: VerifierFindingKind::SecretLeak,
                    severity: VerifierSeverity::Warning,
                    detail: "leak".to_string(),
                }),
                "proposal" => PluginContributionKind::Proposal(secret_world_fact_proposal()),
                _ => PluginContributionKind::PromptBlock(prompt_block(
                    self.plugin_id,
                    "anti_spoiler",
                )),
            };
            vec![PluginContribution {
                meta: meta(self.plugin_id, self.safety, self.priority, self.emit_hook),
                kind,
            }]
        }
    }

    #[tokio::test]
    async fn host_orders_by_safety_then_priority() {
        let mut host = PluginHost::new();
        // 注册顺序故意打乱；期望排序后 Core 先于 Format，同级 priority 降序。
        host.register(Box::new(FakePlugin {
            plugin_id: "format_lo",
            safety: SafetyClass::Format,
            priority: 5,
            hooks: &[PluginHook::ContextAssembly],
            emit_hook: PluginHook::ContextAssembly,
            kind: "prompt",
        }));
        host.register(Box::new(FakePlugin {
            plugin_id: "core_hi",
            safety: SafetyClass::Core,
            priority: 1,
            hooks: &[PluginHook::ContextAssembly],
            emit_hook: PluginHook::ContextAssembly,
            kind: "prompt",
        }));
        host.register(Box::new(FakePlugin {
            plugin_id: "format_hi",
            safety: SafetyClass::Format,
            priority: 50,
            hooks: &[PluginHook::ContextAssembly],
            emit_hook: PluginHook::ContextAssembly,
            kind: "prompt",
        }));

        let ctx = PluginContext {
            hook: PluginHook::ContextAssembly,
            ..Default::default()
        };
        let out = host.run_hook(&ctx).await;
        let ids: Vec<&str> = out.iter().map(|c| c.meta.plugin_id.as_str()).collect();
        // Core 最先；两个 Format 间按 priority 降序（50 在 5 前）。
        assert_eq!(ids, vec!["core_hi", "format_hi", "format_lo"]);
    }

    #[tokio::test]
    async fn plugin_only_runs_for_its_hooks() {
        let mut host = PluginHost::new();
        // 只声明 AfterLlmStream；对 ContextAssembly host 根本不会调用它。
        host.register(Box::new(FakePlugin {
            plugin_id: "after_only",
            safety: SafetyClass::Safety,
            priority: 0,
            hooks: &[PluginHook::AfterLlmStream],
            emit_hook: PluginHook::AfterLlmStream,
            kind: "finding",
        }));

        let assembly_ctx = PluginContext {
            hook: PluginHook::ContextAssembly,
            ..Default::default()
        };
        assert!(host.run_hook(&assembly_ctx).await.is_empty());

        let stream_ctx = PluginContext {
            hook: PluginHook::AfterLlmStream,
            ..Default::default()
        };
        assert_eq!(host.run_hook(&stream_ctx).await.len(), 1);
    }

    /// HeavyPostprocess hook：声明该 hook 的假插件产出一条 Proposal 贡献，
    /// host 正常收集；其它 hook 不触发它。
    #[tokio::test]
    async fn host_runs_heavy_postprocess_proposal_plugin() {
        let mut host = PluginHost::new();
        host.register(Box::new(FakePlugin {
            plugin_id: "play.memory_extractor",
            safety: SafetyClass::Experience,
            priority: 100,
            hooks: &[PluginHook::HeavyPostprocess],
            emit_hook: PluginHook::HeavyPostprocess,
            kind: "proposal",
        }));

        // 非 HeavyPostprocess hook：不调用该插件。
        let assembly_ctx = PluginContext {
            hook: PluginHook::ContextAssembly,
            ..Default::default()
        };
        assert!(host.run_hook(&assembly_ctx).await.is_empty());

        // HeavyPostprocess hook：产出 1 条 Proposal 贡献。
        let heavy_ctx = PluginContext {
            hook: PluginHook::HeavyPostprocess,
            ..Default::default()
        };
        let out = host.run_hook(&heavy_ctx).await;
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0].kind, PluginContributionKind::Proposal(_)));
        assert_eq!(out[0].meta.hook.as_str(), "heavy_postprocess");
    }

    /// Proposal 贡献的 trace：kind = "proposal"，summary 只暴露稳定 proposal_kind +
    /// fact_id，**绝不**回显 secret 正文（subject/predicate/object/summary）。
    #[test]
    fn proposal_contribution_trace_is_secret_safe() {
        let c = PluginContribution {
            meta: meta(
                "play.memory_extractor",
                SafetyClass::Experience,
                0,
                PluginHook::HeavyPostprocess,
            ),
            kind: PluginContributionKind::Proposal(secret_world_fact_proposal()),
        };
        let t = c.to_trace();
        assert_eq!(t.kind, "proposal");
        assert_eq!(t.hook, "heavy_postprocess");
        // 安全身份：只露 proposal_kind + fact_id。
        assert_eq!(t.summary, "world_fact:fact.king_identity");
        // 绝不夹带 secret 正文。
        assert!(!t.summary.contains("beggar"));
        assert!(!t.summary.contains("hidden king"));
        assert!(!t.summary.contains("secret twist prose"));
    }

    #[test]
    fn contribution_to_trace_for_each_kind() {
        // prompt_block：summary = block 标题。
        let pb = PluginContribution {
            meta: meta("p1", SafetyClass::Safety, 0, PluginHook::ContextAssembly),
            kind: PluginContributionKind::PromptBlock(prompt_block("b1", "anti_spoiler")),
        };
        let t = pb.to_trace();
        assert_eq!(t.kind, "prompt_block");
        assert_eq!(t.summary, "anti_spoiler");
        assert_eq!(t.hook, "context_assembly");
        assert_eq!(t.plugin_id, "p1");

        // context_filter：summary = 删除块数。
        let cf = PluginContribution {
            meta: meta("p2", SafetyClass::Safety, 0, PluginHook::ContextAssembly),
            kind: PluginContributionKind::ContextFilter(ContextFilterSpec {
                drop_block_ids: vec!["a".to_string(), "b".to_string()],
                reason: String::new(),
            }),
        };
        let t = cf.to_trace();
        assert_eq!(t.kind, "context_filter");
        assert_eq!(t.summary, "drop 2 block(s)");

        // verifier_finding：summary = finding 种类（snake_case）。
        let vf = PluginContribution {
            meta: meta("p3", SafetyClass::Safety, 0, PluginHook::AfterLlmStream),
            kind: PluginContributionKind::VerifierFinding(VerifierFinding {
                kind: VerifierFindingKind::SecretLeak,
                severity: VerifierSeverity::Warning,
                detail: "x".to_string(),
            }),
        };
        let t = vf.to_trace();
        assert_eq!(t.kind, "verifier_finding");
        assert_eq!(t.summary, "secret_leak");
        assert_eq!(t.hook, "after_llm_stream");
    }

    #[test]
    fn contribution_meta_serde() {
        let m = ContributionMeta {
            plugin_id: "core.no_spoiler_guard".to_string(),
            hook: PluginHook::AfterLlmStream,
            priority: 7,
            safety_class: SafetyClass::Safety,
            fail_policy: FailPolicy::RepairThenFailClosed,
            source_refs: vec![SourceRef {
                source_id: "src".to_string(),
                ..Default::default()
            }],
        };
        let json = serde_json::to_string(&m).expect("serialize meta");
        let back: ContributionMeta = serde_json::from_str(&json).expect("deserialize meta");
        assert_eq!(m, back);
    }

    #[test]
    fn safety_class_ord() {
        // Core < Safety < Rule < Module < Experience < Format（Core 优先级最高）。
        assert!(SafetyClass::Core < SafetyClass::Safety);
        assert!(SafetyClass::Safety < SafetyClass::Rule);
        assert!(SafetyClass::Rule < SafetyClass::Module);
        assert!(SafetyClass::Module < SafetyClass::Experience);
        assert!(SafetyClass::Experience < SafetyClass::Format);
    }

    #[test]
    fn fail_policy_default_is_ignore() {
        assert_eq!(FailPolicy::default(), FailPolicy::Ignore);
    }

    /// 证明 trait 可被外部 impl 并经 host 使用（最小可用性证据）。
    #[tokio::test]
    async fn trivial_plugin_is_usable() {
        struct Trivial;
        #[async_trait]
        impl RuntimePlugin for Trivial {
            fn id(&self) -> &'static str {
                "trivial"
            }
            fn safety_class(&self) -> SafetyClass {
                SafetyClass::Format
            }
            fn hooks(&self) -> &'static [PluginHook] {
                &[PluginHook::ContextAssembly]
            }
            async fn on_hook(&self, _ctx: &PluginContext) -> Vec<PluginContribution> {
                vec![]
            }
        }
        let mut host = PluginHost::new();
        host.register(Box::new(Trivial));
        assert_eq!(host.len(), 1);
        let ctx = PluginContext::default();
        assert!(host.run_hook(&ctx).await.is_empty());
    }
}

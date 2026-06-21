//! L9.2 — 示例 pacing 插件 `core.pacing_plugin`：演示 `BeatWeight` 贡献种类。
//!
//! pacing/foreshadowing 这类**体验级**插件需要影响 Director 的 beat 选择，但**绝不能**持有
//! 核心 story state（§十四）或直接改 DirectorPlan/StoryState。本插件只读 `PluginContext` 快照里
//! 的 pacing 信号（`config["tension"]`），**提议**如何重权候选 beat（`BeatWeightProposal`）；
//! host 收集，Director 评分时择用——propose-not-commit。
//!
//! - 通用，零规则集/模组名分支（§二-⑪）：只按 `tension` 数值（0..=1）分三档重权。
//! - 纯只读 + fail-closed：缺/坏 `config["tension"]` ⇒ 中性档（0.5）。
//! - **不接进 always-on `builtin_plugin_host()`**：注册它会改每回合行为并破坏
//!   `builtin_host_seeds_five_plugins`；它是示例，经独立 host 在测试中演示 ⇒ OFF 基线字节等价。

use async_trait::async_trait;

use super::host::RuntimePlugin;
use super::types::{
    BeatWeightProposal, BeatWeightTerm, ContributionMeta, FailPolicy, PluginContext,
    PluginContribution, PluginContributionKind, PluginHook, SafetyClass,
};
use trpg_model::BeatKind;

/// 本插件的稳定通用 id（非规则集/模组名）。
pub const PACING_PLUGIN_ID: &str = "core.pacing_plugin";

/// 高/低张力阈值（高于=该松一口气；低于=该加压）。
const TENSION_HIGH: f32 = 0.7;
const TENSION_LOW: f32 = 0.3;

/// 示例 pacing 插件（体验级，依张力重权候选 beat）。无任何回合/会话状态。
pub struct PacingPlugin;

impl PacingPlugin {
    /// 从只读 `PluginContext.config` 取张力信号；缺/坏 ⇒ 中性 0.5（fail-closed）。
    fn tension(ctx: &PluginContext) -> f32 {
        ctx.config
            .get("tension")
            .and_then(|v| v.as_f64())
            .map(|f| f as f32)
            .unwrap_or(0.5)
            .clamp(0.0, 1.0)
    }

    /// 依张力把张力曲线折成一份 beat 重权提议（纯函数，确定性）。
    ///
    /// - 高张力：压低 `Escalate`、抬高 `Relief`/`Payoff`，并约束场景 `relieve_tension`。
    /// - 低张力：抬高 `Escalate`/`Complicate`（该建张力了）。
    /// - 中性：轻微抬高 `Respond`（保持推进，不强行加压/泄压）。
    fn proposal_for(tension: f32) -> BeatWeightProposal {
        if tension >= TENSION_HIGH {
            BeatWeightProposal {
                beat_weights: vec![
                    BeatWeightTerm { beat_kind: BeatKind::Escalate, weight_delta: -0.5 },
                    BeatWeightTerm { beat_kind: BeatKind::Relief, weight_delta: 0.4 },
                    BeatWeightTerm { beat_kind: BeatKind::Payoff, weight_delta: 0.3 },
                ],
                scene_constraints: vec!["relieve_tension".to_string()],
                rationale: "tension_high_relieve".to_string(),
            }
        } else if tension <= TENSION_LOW {
            BeatWeightProposal {
                beat_weights: vec![
                    BeatWeightTerm { beat_kind: BeatKind::Escalate, weight_delta: 0.4 },
                    BeatWeightTerm { beat_kind: BeatKind::Complicate, weight_delta: 0.3 },
                ],
                scene_constraints: Vec::new(),
                rationale: "tension_low_build".to_string(),
            }
        } else {
            BeatWeightProposal {
                beat_weights: vec![BeatWeightTerm {
                    beat_kind: BeatKind::Respond,
                    weight_delta: 0.1,
                }],
                scene_constraints: Vec::new(),
                rationale: "tension_neutral".to_string(),
            }
        }
    }
}

#[async_trait]
impl RuntimePlugin for PacingPlugin {
    fn id(&self) -> &'static str {
        PACING_PLUGIN_ID
    }

    fn safety_class(&self) -> SafetyClass {
        // 体验级：不是安全守卫，排序靠后（绝不抢 Safety 守卫的优先）。
        SafetyClass::Experience
    }

    fn hooks(&self) -> &'static [PluginHook] {
        // beat 重权在上下文装配时择用 ⇒ 接 ContextAssembly。
        &[PluginHook::ContextAssembly]
    }

    async fn on_hook(&self, ctx: &PluginContext) -> Vec<PluginContribution> {
        if ctx.hook != PluginHook::ContextAssembly {
            return vec![];
        }
        let proposal = Self::proposal_for(Self::tension(ctx));
        vec![PluginContribution {
            meta: ContributionMeta {
                plugin_id: PACING_PLUGIN_ID.to_string(),
                hook: PluginHook::ContextAssembly,
                priority: 200,
                safety_class: SafetyClass::Experience,
                fail_policy: FailPolicy::Ignore,
                source_refs: vec![],
            },
            kind: PluginContributionKind::BeatWeight(proposal),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::host::PluginHost;
    use trpg_model::StoryState;

    fn ctx_with_tension(tension: f64) -> PluginContext {
        PluginContext {
            hook: PluginHook::ContextAssembly,
            config: serde_json::json!({ "tension": tension }),
            ..Default::default()
        }
    }

    fn beat_weight(c: &PluginContribution) -> &BeatWeightProposal {
        match &c.kind {
            PluginContributionKind::BeatWeight(bw) => bw,
            other => panic!("expected BeatWeight, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn high_tension_reweights_toward_relief() {
        let out = PacingPlugin.on_hook(&ctx_with_tension(0.9)).await;
        assert_eq!(out.len(), 1, "应产 1 条 BeatWeight 贡献");
        let bw = beat_weight(&out[0]);
        // Escalate 被压低、Relief/Payoff 被抬高（候选 beat 被重权）。
        let escalate = bw.beat_weights.iter().find(|t| t.beat_kind == BeatKind::Escalate).unwrap();
        assert!(escalate.weight_delta < 0.0, "高张力应压低 Escalate");
        assert!(bw.beat_weights.iter().any(|t| t.beat_kind == BeatKind::Relief && t.weight_delta > 0.0));
        assert!(bw.scene_constraints.contains(&"relieve_tension".to_string()));
        assert_eq!(out[0].meta.plugin_id, PACING_PLUGIN_ID);
        assert_eq!(out[0].meta.safety_class, SafetyClass::Experience);
    }

    #[tokio::test]
    async fn low_tension_reweights_toward_escalation() {
        let bw = {
            let out = PacingPlugin.on_hook(&ctx_with_tension(0.1)).await;
            beat_weight(&out[0]).clone()
        };
        let escalate = bw.beat_weights.iter().find(|t| t.beat_kind == BeatKind::Escalate).unwrap();
        assert!(escalate.weight_delta > 0.0, "低张力应抬高 Escalate");
    }

    #[tokio::test]
    async fn missing_config_is_neutral_fail_closed() {
        // 无 config["tension"] ⇒ 中性档（0.5），仍产一条提议（不空转、不报错）。
        let ctx = PluginContext { hook: PluginHook::ContextAssembly, ..Default::default() };
        let out = PacingPlugin.on_hook(&ctx).await;
        assert_eq!(out.len(), 1);
        assert_eq!(beat_weight(&out[0]).rationale, "tension_neutral");
    }

    #[tokio::test]
    async fn empty_for_non_context_assembly_hook() {
        let ctx = PluginContext {
            hook: PluginHook::AfterLlmStream,
            config: serde_json::json!({ "tension": 0.9 }),
            ..Default::default()
        };
        assert!(PacingPlugin.on_hook(&ctx).await.is_empty());
    }

    /// 核心不变量（L9.2 验收）：经 host 跑 PacingPlugin 重权候选 beat，**绝不**触碰核心 story
    /// state——插件从不拿到可变 handle，旁置的 `StoryState` 在跑前/跑后字节等价；host 只收集
    /// 提议（propose-not-commit），不落任何东西。
    #[tokio::test]
    async fn reweights_without_touching_core_state() {
        let mut host = PluginHost::new();
        host.register(Box::new(PacingPlugin));

        // 旁置一份核心 story state；它绝不被插件路径触碰。
        let core_before = StoryState {
            active_threads: vec![trpg_model::StoryThread {
                thread_id: "t.core".into(),
                premise: "core thread".into(),
                status: trpg_model::StoryThreadStatus::Escalating,
                ..Default::default()
            }],
            ..Default::default()
        };
        let core_snapshot = serde_json::to_vec(&core_before).unwrap();

        let contributions = host.run_hook(&ctx_with_tension(0.9)).await;

        // host 只返回提议（BeatWeight），不应用、不落库。
        assert_eq!(contributions.len(), 1);
        assert!(matches!(contributions[0].kind, PluginContributionKind::BeatWeight(_)));
        // 核心 story state 字节不变（插件无 handle 可改）。
        assert_eq!(
            serde_json::to_vec(&core_before).unwrap(),
            core_snapshot,
            "PacingPlugin 重权 beat 绝不触碰核心 story state"
        );
    }

    /// trace 安全：summary 只露结构计数 + 稳定 beat 种类 token，绝不夹带 rationale 正文之外的秘密。
    #[tokio::test]
    async fn contribution_trace_is_structural() {
        let out = PacingPlugin.on_hook(&ctx_with_tension(0.9)).await;
        let t = out[0].to_trace();
        assert_eq!(t.kind, "beat_weight");
        assert_eq!(t.hook, "context_assembly");
        assert!(t.summary.starts_with("beat_weight:3 term(s)"), "summary={}", t.summary);
        assert!(t.summary.contains("escalate"), "应含稳定 beat 种类 token");
    }
}

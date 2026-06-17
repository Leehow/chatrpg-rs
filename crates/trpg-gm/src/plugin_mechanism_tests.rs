//! Policy Plugin Host 机制收尾测试（Part 1 ContextFilter apply + Part 2 verifier 入 trace）。
//!
//! 这两条机制在生产里今天**无 emitter**（无内置插件产 ContextFilter / AfterLlmStream verifier），
//! 故生产零行为变更；这里用**本地** PluginHost + 假插件验证机制本身正确（host → 汇总 drop →
//! `trpg_runtime::apply_context_filter` → 块消失 + trace 记录），并单测 NarrationVerifier findings
//! 折成 `core.no_mechanical_invention` trace 的 surfacing helper。

use crate::plugin::{
    ContextFilterSpec, ContributionMeta, FailPolicy, PluginContext, PluginContribution,
    PluginContributionKind, PluginHook, PluginHost, RuntimePlugin, SafetyClass,
};
use crate::turn_loop::surface_verifier_finding_trace;
use async_trait::async_trait;
use trpg_agent::{VerifierFinding, VerifierFindingKind, VerifierSeverity};
use trpg_model::{
    BlockContent, BlockKind, CacheZone, CompiledContext, ContextBlock, ContextRequest, Scope,
    Stability, TokenBudget, Visibility, VisibilityProfile,
};

fn block(id: &str, title: &str, body: &str, zone: CacheZone) -> ContextBlock {
    ContextBlock::new(
        id,
        BlockKind::GmOnboarding,
        title,
        BlockContent::Text(body.to_string()),
        Visibility::GmOnly,
        Stability::SceneStable,
        zone,
        Scope::global(),
        0,
    )
}

fn request() -> ContextRequest {
    ContextRequest {
        ruleset_id: "rs".to_string(),
        module_id: None,
        session_id: "s".to_string(),
        turn_id: "t".to_string(),
        viewer: VisibilityProfile::gm(),
        token_budget: TokenBudget::default(),
    }
}

/// 经 runtime 真路径装配一份含两个 dynamic block 的 CompiledContext。
fn compiled_with_two_dynamic() -> CompiledContext {
    let planned = trpg_runtime::PlannedContext {
        prefix_blocks: vec![block("p.keep", "Prefix", "prefix body", CacheZone::Prefix)],
        pinned_blocks: vec![],
        dynamic_blocks: vec![
            block("d.keep", "Keep", "keep body", CacheZone::DynamicTail),
            block("d.secret", "Secret", "secret body", CacheZone::DynamicTail),
        ],
    };
    trpg_runtime::ContextBuilder
        .build(planned, &request())
        .expect("build compiled")
}

/// 假插件：在 ContextAssembly hook 产一条 ContextFilter，删掉 `d.secret`。
struct FilterPlugin;

#[async_trait]
impl RuntimePlugin for FilterPlugin {
    fn id(&self) -> &'static str {
        "test.filter_plugin"
    }
    fn safety_class(&self) -> SafetyClass {
        SafetyClass::Safety
    }
    fn hooks(&self) -> &'static [PluginHook] {
        &[PluginHook::ContextAssembly]
    }
    async fn on_hook(&self, _ctx: &PluginContext) -> Vec<PluginContribution> {
        vec![PluginContribution {
            meta: ContributionMeta {
                plugin_id: "test.filter_plugin".to_string(),
                hook: PluginHook::ContextAssembly,
                priority: 500,
                safety_class: SafetyClass::Safety,
                fail_policy: FailPolicy::default(),
                source_refs: vec![],
            },
            kind: PluginContributionKind::ContextFilter(ContextFilterSpec {
                drop_block_ids: vec!["d.secret".to_string()],
                reason: "gm-only secret".to_string(),
            }),
        }]
    }
}

/// Part 1 集成：ContextAssembly hook 的 ContextFilter 贡献 → 汇总 drop_block_ids →
/// `apply_context_filter` 删块（ctx.compiled 失去该块）+ 每条贡献折 trace 记录。
/// 镜像 `apply_context_assembly_plugins` 里的 ContextFilter 处置逻辑。
#[tokio::test]
async fn context_filter_contribution_drops_block_and_records_trace() {
    let mut host = PluginHost::new();
    host.register(Box::new(FilterPlugin));

    let mut compiled = compiled_with_two_dynamic();
    let prefix_text_before = compiled.prefix_text.clone();
    let prefix_hash_before = compiled.prefix_hash.clone();

    // —— 复刻 helper 的处置：跑 hook → 汇总 drop_block_ids + 折 trace → apply_context_filter ——
    let plugin_ctx = PluginContext {
        hook: PluginHook::ContextAssembly,
        ..Default::default()
    };
    let contributions = host.run_hook(&plugin_ctx).await;

    let mut traces: Vec<trpg_model::PluginContributionTrace> = Vec::new();
    let mut drop_block_ids: Vec<String> = Vec::new();
    for c in &contributions {
        traces.push(c.to_trace());
        if let PluginContributionKind::ContextFilter(spec) = &c.kind {
            drop_block_ids.extend(spec.drop_block_ids.iter().cloned());
        }
    }
    if !drop_block_ids.is_empty() {
        trpg_runtime::apply_context_filter(&mut compiled, &drop_block_ids);
    }

    // 被删块消失、保留块仍在。
    let ids: Vec<&str> = compiled
        .dynamic_blocks
        .iter()
        .map(|b| b.block_id.as_str())
        .collect();
    assert_eq!(ids, vec!["d.keep"], "d.secret must be dropped");
    assert!(
        !compiled.dynamic_text.contains("secret body"),
        "dropped block text must be gone after re-render"
    );
    // 未受影响 band byte-stable。
    assert_eq!(compiled.prefix_text, prefix_text_before);
    assert_eq!(compiled.prefix_hash, prefix_hash_before);
    // ContextFilter 贡献被 trace 记录（含 kind/hook/summary）。
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].kind, "context_filter");
    assert_eq!(traces[0].hook, "context_assembly");
    assert_eq!(traces[0].plugin_id, "test.filter_plugin");
    assert_eq!(traces[0].summary, "drop 1 block(s)");
}

/// 空 drop（无 ContextFilter 贡献，即生产现状）→ apply 不被调用 → ctx.compiled byte-stable。
#[tokio::test]
async fn no_context_filter_contribution_leaves_compiled_byte_stable() {
    let host = PluginHost::new(); // 无插件 → 无贡献。
    let mut compiled = compiled_with_two_dynamic();
    let before = compiled.clone();

    let plugin_ctx = PluginContext {
        hook: PluginHook::ContextAssembly,
        ..Default::default()
    };
    let contributions = host.run_hook(&plugin_ctx).await;
    let drop_block_ids: Vec<String> = contributions
        .iter()
        .filter_map(|c| match &c.kind {
            PluginContributionKind::ContextFilter(spec) => Some(spec.drop_block_ids.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    assert!(drop_block_ids.is_empty());
    if !drop_block_ids.is_empty() {
        trpg_runtime::apply_context_filter(&mut compiled, &drop_block_ids);
    }

    assert_eq!(compiled.dynamic_text, before.dynamic_text);
    assert_eq!(compiled.dynamic_hash, before.dynamic_hash);
    assert_eq!(compiled.prefix_text, before.prefix_text);
}

/// Part 2 surfacing helper：一条 NarrationVerifier finding 折成 `core.no_mechanical_invention`
/// 的 `verifier_finding` / `after_llm_stream` trace，summary 含 kind + detail。
#[test]
fn verifier_finding_surfaces_as_no_mechanical_invention_trace() {
    let finding = VerifierFinding {
        kind: VerifierFindingKind::InventedEffect,
        severity: VerifierSeverity::Warning,
        detail: "narration claims 3 sanity loss without a ledger effect".to_string(),
    };
    let trace = surface_verifier_finding_trace(&finding);
    assert_eq!(trace.plugin_id, "core.no_mechanical_invention");
    assert_eq!(trace.hook, "after_llm_stream");
    assert_eq!(trace.kind, "verifier_finding");
    assert!(
        trace.summary.starts_with("invented_effect"),
        "summary must lead with finding kind: {}",
        trace.summary
    );
    assert!(
        trace.summary.contains("narration claims 3 sanity loss"),
        "summary must include detail: {}",
        trace.summary
    );
}

/// surfacing helper：空 detail → summary 即 kind 字符串（不带尾随冒号）。
#[test]
fn verifier_finding_surface_empty_detail_is_kind_only() {
    let finding = VerifierFinding {
        kind: VerifierFindingKind::MissingCheck,
        severity: VerifierSeverity::Blocker,
        detail: String::new(),
    };
    let trace = surface_verifier_finding_trace(&finding);
    assert_eq!(trace.summary, "missing_check");
    assert_eq!(trace.kind, "verifier_finding");
    assert_eq!(trace.plugin_id, "core.no_mechanical_invention");
}

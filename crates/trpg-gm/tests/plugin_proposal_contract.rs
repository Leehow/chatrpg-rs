//! Contract test for the HeavyPostprocess plugin proposal path (设计3.md §12 / §P2).
//!
//! Proves the public plugin contract lets a plugin **propose** a memory/knowledge/NPC
//! delta via the `HeavyPostprocess` hook without committing DB state: the host collects
//! the `Proposal` contribution, the trace exposes only safe kind/identity (never secret
//! prose), and proposals stay inert data (propose-not-commit). Mirrors the in-crate host
//! unit tests but pins the behavior through the crate's public API surface.

use async_trait::async_trait;
use trpg_gm::plugin::{
    ContributionMeta, FailPolicy, PluginContext, PluginContribution, PluginContributionKind,
    PluginHook, PluginHost, RuntimePlugin, SafetyClass,
};
use trpg_model::{MemoryExtractionProposal, WorldFactCandidate};

/// A fake heavy-postprocess extractor. It declares the new hook and emits a single
/// WorldFact proposal whose body deliberately carries "secret" prose, so the trace
/// assertions can prove no secret text leaks into the flight recorder summary.
struct FakeMemoryExtractor;

#[async_trait]
impl RuntimePlugin for FakeMemoryExtractor {
    fn id(&self) -> &'static str {
        "play.memory_extractor"
    }
    fn safety_class(&self) -> SafetyClass {
        SafetyClass::Experience
    }
    fn hooks(&self) -> &'static [PluginHook] {
        &[PluginHook::HeavyPostprocess]
    }
    async fn on_hook(&self, ctx: &PluginContext) -> Vec<PluginContribution> {
        if ctx.hook != PluginHook::HeavyPostprocess {
            return vec![];
        }
        let proposal = MemoryExtractionProposal::WorldFact(WorldFactCandidate {
            fact_id: "fact.king_identity".to_string(),
            subject: "the beggar".to_string(),
            predicate: "is".to_string(),
            object: "the hidden king".to_string(),
            summary: "secret twist prose".to_string(),
            confidence: Some(0.9),
            source_event_ids: vec!["evt-1".to_string()],
            turn_id: Some(ctx.turn_id.clone()),
        });
        vec![PluginContribution {
            meta: ContributionMeta {
                plugin_id: self.id().to_string(),
                hook: PluginHook::HeavyPostprocess,
                priority: 100,
                safety_class: self.safety_class(),
                fail_policy: FailPolicy::WarnContinue,
                source_refs: vec![],
            },
            kind: PluginContributionKind::Proposal(proposal),
        }]
    }
}

#[tokio::test]
async fn heavy_postprocess_plugin_proposes_without_committing() {
    let mut host = PluginHost::new();
    host.register(Box::new(FakeMemoryExtractor));

    // Other hooks never trigger the extractor.
    let assembly_ctx = PluginContext {
        hook: PluginHook::ContextAssembly,
        ..Default::default()
    };
    assert!(host.run_hook(&assembly_ctx).await.is_empty());

    // HeavyPostprocess hook yields exactly one Proposal contribution.
    let heavy_ctx = PluginContext {
        hook: PluginHook::HeavyPostprocess,
        turn_id: "t-42".to_string(),
        ..Default::default()
    };
    let out = host.run_hook(&heavy_ctx).await;
    assert_eq!(out.len(), 1, "extractor should propose one delta");

    // The contribution is a proposal carrying the model-layer proposal type, and it stays
    // inert data — the host never applies/commits it (propose-not-commit).
    let proposal = match &out[0].kind {
        PluginContributionKind::Proposal(p) => p,
        other => panic!("expected Proposal, got {other:?}"),
    };
    assert_eq!(proposal.kind_token(), "world_fact");
    // The proposal still validates as a real, evidence-backed candidate (not committed).
    assert!(proposal.validated().is_ok());

    // Trace is secret-safe: kind/identity only, never the fact body.
    let trace = out[0].to_trace();
    assert_eq!(trace.kind, "proposal");
    assert_eq!(trace.hook, "heavy_postprocess");
    assert_eq!(trace.summary, "world_fact:fact.king_identity");
    for secret in ["beggar", "hidden king", "secret twist prose"] {
        assert!(
            !trace.summary.contains(secret),
            "trace summary must not leak secret prose: {secret}"
        );
    }
}

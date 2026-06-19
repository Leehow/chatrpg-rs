//! P7.1 + P7.2 acceptance — Port traits + thin adapters.
//!
//! Two flavors of test:
//! - **Delegation-equivalence (pure ports):** the adapter must produce the SAME value
//!   as calling the underlying real function directly (PolicyPort, DirectorPort's pure
//!   `build_brief_packet`). This is the zero-behavior-change proof.
//! - **Compile/trait-bound wiring (async DB ports):** the adapter must satisfy the trait
//!   bound and construct over a `&RuntimeEngine` / `&GmLoop`. Live-DB delegation behavior
//!   is exercised by the integration lanes (P7.3 / P7.8); here we pin the contract shape.

use trpg_agent::{
    NarrationVerifierResult, VerifierFinding, VerifierFindingKind, VerifierSeverity,
};
use trpg_director::{build_director_brief_packet, DirectorMode};
use trpg_gm::ports::{
    DirectorAdapter, DirectorPort, EngineKernelAdapter, EngineRulesAdapter, EngineWorldAdapter,
    GmLoopNarratorAdapter, KernelPort, NarratorPort, PolicyPort, PresentationPolicyAdapter,
    RulesPort, WorldPort,
};
use trpg_model::{SpotlightState, StoryState, WorldReactionCandidate};

// ---------------------------------------------------------------------------
// PolicyPort — pure delegation equivalence (Allow + Block).
// ---------------------------------------------------------------------------

fn result_with(kind: VerifierFindingKind, severity: VerifierSeverity) -> NarrationVerifierResult {
    NarrationVerifierResult {
        accepted: severity != VerifierSeverity::Blocker,
        findings: vec![VerifierFinding {
            kind,
            severity,
            detail: "t".into(),
        }],
        next_required_action: None,
    }
}

#[test]
fn policy_port_block_equiv() {
    // SecretLeak + Blocker is in the locked BLOCKING_KINDS whitelist → Block.
    let r = result_with(VerifierFindingKind::SecretLeak, VerifierSeverity::Blocker);
    let adapter = PresentationPolicyAdapter;
    assert_eq!(
        adapter.presentation_gate(&r),
        trpg_gm::presentation_gate_decision(&r),
        "adapter must equal the real fn (Block path)"
    );
    assert!(adapter.presentation_gate(&r).is_block());
}

#[test]
fn policy_port_allow_equiv() {
    // MissingCheck (RetroDebt path) is NOT a blocking kind → Allow even at Blocker severity.
    let r = result_with(VerifierFindingKind::MissingCheck, VerifierSeverity::Blocker);
    let adapter = PresentationPolicyAdapter;
    assert_eq!(
        adapter.presentation_gate(&r),
        trpg_gm::presentation_gate_decision(&r),
        "adapter must equal the real fn (Allow path)"
    );
    assert!(!adapter.presentation_gate(&r).is_block());
}

// ---------------------------------------------------------------------------
// DirectorPort — pure `build_brief_packet` delegation equivalence.
// ---------------------------------------------------------------------------

#[test]
fn director_port_disabled_equiv() {
    // Disabled mode → DirectorPlan::default() in BOTH the adapter and the real fn.
    let adapter = DirectorAdapter;
    let story = StoryState::default();
    let spots: Vec<SpotlightState> = vec![];
    let via_port = adapter.build_brief_packet(
        DirectorMode::Disabled,
        &[],
        &story,
        None,
        None,
        &spots,
        &[],
        "actor_pc",
    );
    let direct = build_director_brief_packet(
        DirectorMode::Disabled,
        &[],
        &story,
        None,
        None,
        &spots,
        &[],
        "actor_pc",
    );
    assert_eq!(via_port, direct, "adapter must equal the real pure fn (Disabled)");
}

#[test]
fn director_port_empty_pool_equiv() {
    // OnDemand with an empty candidate pool exercises the non-trivial empty-pool branch
    // (world_query) — adapter and real fn must still agree byte-for-byte.
    let adapter = DirectorAdapter;
    let story = StoryState::default();
    let spots: Vec<SpotlightState> = vec![];
    let candidates: Vec<WorldReactionCandidate> = vec![];
    let pk = vec!["fact_a".to_string()];
    let gm = vec!["fact_a".to_string(), "fact_b".to_string()];
    let via_port = adapter.build_brief_packet(
        DirectorMode::OnDemand,
        &candidates,
        &story,
        Some(&pk),
        Some(&gm),
        &spots,
        &[],
        "actor_pc",
    );
    let direct = build_director_brief_packet(
        DirectorMode::OnDemand,
        &candidates,
        &story,
        Some(&pk),
        Some(&gm),
        &spots,
        &[],
        "actor_pc",
    );
    assert_eq!(via_port, direct, "adapter must equal the real pure fn (empty pool)");
}

// ---------------------------------------------------------------------------
// Async DB ports — trait-bound / construction wiring (live DB is P7.3 / P7.8).
// These pin that each adapter satisfies its Port trait with the exact real types.
// ---------------------------------------------------------------------------

fn assert_narrator<T: NarratorPort>() {}
fn assert_rules<T: RulesPort>() {}
fn assert_world<T: WorldPort>() {}
fn assert_director<T: DirectorPort>() {}
fn assert_policy<T: PolicyPort>() {}
fn assert_kernel<T: KernelPort>() {}

#[test]
fn adapters_satisfy_port_bounds() {
    // Compile-time proof: every adapter implements its Port trait with the real signatures.
    assert_narrator::<GmLoopNarratorAdapter<'_>>();
    assert_rules::<EngineRulesAdapter<'_>>();
    assert_world::<EngineWorldAdapter<'_>>();
    assert_director::<DirectorAdapter>();
    assert_policy::<PresentationPolicyAdapter>();
    assert_kernel::<EngineKernelAdapter<'_>>();
}

//! P7.1 + P7.2 acceptance — Port traits + thin adapters.
//!
//! Two flavors of test:
//! - **Delegation-equivalence (pure ports):** the adapter must produce the SAME value
//!   as calling the underlying real function directly (PolicyPort, DirectorPort's pure
//!   `build_brief_packet`). This is the zero-behavior-change proof.
//! - **Compile/trait-bound wiring (async DB ports):** the adapter must satisfy the trait
//!   bound and construct over a `&RuntimeEngine` / `&GmLoop`. Live-DB delegation behavior
//!   is exercised by the integration lanes (P7.3 / P7.8); here we pin the contract shape.

use trpg_agent::{NarrationVerifierResult, VerifierFinding, VerifierFindingKind, VerifierSeverity};
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
    assert_eq!(
        via_port, direct,
        "adapter must equal the real pure fn (Disabled)"
    );
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
    assert_eq!(
        via_port, direct,
        "adapter must equal the real pure fn (empty pool)"
    );
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

// ---------------------------------------------------------------------------
// P7.3 — control-plane dispatch guard. A Port trait with NO production caller is a
// dead trait (this project has been bitten 4×). These source-grep guards PIN that
// the real turn_loop control plane dispatches through each WIRED adapter, so a future
// refactor that quietly bypasses the Port back to the raw fn is caught by CI.
// P7.3b: RulesPort is now wired too — its real production callers (gate.rs head-gate
// resolution + npc_action.rs World-attack auto-roll) dispatch through EngineRulesAdapter,
// completing 6/6 Port dispatch. See `rules_port_has_production_caller` below.
// ---------------------------------------------------------------------------

const TURN_LOOP_SRC: &str = include_str!("../src/turn_loop.rs");
const GATE_SRC: &str = include_str!("../src/gate.rs");
const NPC_ACTION_SRC: &str = include_str!("../src/npc_action.rs");

#[test]
fn narrator_port_has_production_caller() {
    // run_narrator_phase + repair-ladder dispatch P1 prose through the NarratorPort adapter.
    assert!(
        TURN_LOOP_SRC.contains("GmLoopNarratorAdapter(self)")
            && TURN_LOOP_SRC.contains("GmLoopNarratorAdapter(&*self)"),
        "NarratorPort must be dispatched through GmLoopNarratorAdapter at BOTH narrator call sites"
    );
    assert!(
        TURN_LOOP_SRC.contains(".narrate("),
        "NarratorPort::narrate must be the production call (not raw run_narrator)"
    );
}

#[test]
fn rules_port_has_production_caller() {
    // P7.3b: gate.rs head-gate resolution AND npc_action.rs World-attack auto-roll both
    // dispatch the Rules-layer resolve through EngineRulesAdapter (not the raw engine
    // method), so RulesPort has a real production caller — no longer a forwarded dead trait.
    assert!(
        GATE_SRC.contains("EngineRulesAdapter(engine)")
            && GATE_SRC.contains(".resolve_check_with_input("),
        "RulesPort must be dispatched through EngineRulesAdapter::resolve_check_with_input in gate.rs"
    );
    assert!(
        NPC_ACTION_SRC.contains("EngineRulesAdapter(engine)")
            && NPC_ACTION_SRC.contains(".execute_system_roll_bundle("),
        "RulesPort must be dispatched through EngineRulesAdapter::execute_system_roll_bundle in npc_action.rs"
    );
}

#[test]
fn policy_port_has_production_caller() {
    // verify_after_stream computes the PresentationGate through the PolicyPort adapter.
    assert!(
        TURN_LOOP_SRC.contains("PresentationPolicyAdapter.presentation_gate("),
        "PolicyPort must be dispatched through PresentationPolicyAdapter (not raw presentation_gate_decision)"
    );
}

#[test]
fn world_port_has_production_caller() {
    // build_npc_behavior_guidance loads reaction plans through the WorldPort adapter.
    assert!(
        TURN_LOOP_SRC.contains("EngineWorldAdapter(&self.engine)")
            && TURN_LOOP_SRC.contains(".load_reaction_plans("),
        "WorldPort must be dispatched through EngineWorldAdapter::load_reaction_plans"
    );
}

#[test]
fn director_port_has_production_caller() {
    // build_npc_behavior_guidance renders the director brief through the DirectorPort adapter.
    assert!(
        TURN_LOOP_SRC.contains("DirectorAdapter") && TURN_LOOP_SRC.contains(".prepare_brief("),
        "DirectorPort must be dispatched through DirectorAdapter::prepare_brief"
    );
}

#[test]
fn kernel_port_has_production_caller() {
    // The verifier private-view fold AND the reveal commit primitive both go through KernelPort.
    assert!(
        TURN_LOOP_SRC.contains("EngineKernelAdapter(&self.engine)"),
        "KernelPort must be constructed at its production call sites"
    );
    assert!(
        TURN_LOOP_SRC.contains(".verifier_private_view(")
            && TURN_LOOP_SRC.contains(".commit_reveal_fact("),
        "KernelPort must dispatch BOTH the private-view fold and the reveal commit primitive"
    );
}

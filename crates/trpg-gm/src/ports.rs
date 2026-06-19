//! P7.1 + P7.2 — the layered-runtime **Port contract layer**.
//!
//! Six `async` Port traits formalize the contract of the six REAL layer entry
//! points P1–P6 shipped; six thin `*Adapter`s DELEGATE to those real functions with
//! **zero behavior change**. ADDITIVE: no production caller is rewired (that is P7.3).
//! NO parallel shell types — every signature is typed to the real P1–P6 types
//! reused from `trpg-model` / `trpg-runtime` / `trpg-director`; nothing empty invented.
//!
//! ## Two view-fidelity invariants the ports PIN (do not regress in P7.3)
//!
//! 1. **pure-fold view vs side-channel-written view.** The projections here are pure
//!    folds over DB-read state ([`build_verifier_private_view`], the `project_for_*`
//!    family); they commit nothing. This is DISTINCT from the side-channel writes some
//!    legacy paths perform — e.g. `knowledge_edges` carries a `PlayerPartyEdge`
//!    side-write when relationship facts commit. A view-only Port method MUST NOT
//!    acquire that side-write; the `KernelPort` projections are read-only folds and the
//!    only mutation surface is the explicit [`KernelPort::commit_reveal_fact`].
//! 2. **lossy typed candidate vs lossless render source.** [`WorldPort`] returns
//!    `(WorldReactionSet, Vec<NpcBehaviorPlan>)`. The set's reaction candidates are a
//!    LOSSY action-intent projection; the GM prompt still renders byte-identically from
//!    the RETAINED `NpcBehaviorPlan`s. So the port wraps `load_world_reaction_plans`
//!    (tuple-returning), NOT `load_world_reaction_set` (which would lose render fidelity).
//!
//! ## KernelPort commit semantics (PINNED by the P6 reflection)
//!
//! `PresentationCommit` has a REAL reveal commit primitive: the engine's `reveal_fact`
//! write path ([`KernelPort::commit_reveal_fact`]). `ResolutionCommit` is an ADVISORY
//! audit seam — NOT a transactional `commit_resolution()` and deliberately NOT modeled.
//! Check-resolution durability already lives in `RulesPort` (the `resolve_*` calls insert
//! their own `CheckResultRecord`); there is no separate kernel transaction to wrap.

use async_trait::async_trait;
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

use trpg_director::{build_director_brief_packet, DirectorMode};
use trpg_model::{
    CheckContract, CheckResultRecord, DirectorPlan, NpcBehaviorPlan, NpcProfile,
    NpcRelationshipTarget, SpotlightState, StoryState, WorldReactionCandidate, WorldReactionSet,
};
use trpg_runtime::world::reaction::load_world_reaction_plans;
use trpg_runtime::{
    build_verifier_private_view, prepare_director_brief, project_for_npc_action,
    project_for_npc_speech, project_for_player_narration, AutoRollExecution, NpcActionProjection,
    NpcSpeechProjection, PlayerNarrationProjection, RuntimeEngine, VerifierPrivateView,
};
use trpg_agent::NarrationVerifierResult;

use crate::packet::NarrationPacket;
use crate::presentation_gate::{presentation_gate_decision, PresentationGate};
use crate::turn_event::TurnEvent;
use crate::turn_loop::GmLoop;

// --- NarratorPort — wraps `GmLoop::run_narrator` (P1). READ-ONLY over the packet. ---

/// The Narrator layer entry. Streams player-visible prose from a [`NarrationPacket`]
/// and exposes **no DB / KnowledgeStore mutation surface** — the packet is the only
/// input (mirroring `ToolChoice::None`: the Narrator is physically mutation-free).
/// Returns `None` on LLM failure (caller fail-soft).
#[async_trait]
pub trait NarratorPort {
    async fn narrate(
        &self,
        packet: &NarrationPacket,
        private_tokens: &[String],
        tx: &Sender<TurnEvent>,
        cancel: Option<&CancellationToken>,
    ) -> Option<String>;
}

/// Thin adapter over a live [`GmLoop`]. Delegates to `GmLoop::run_narrator`.
pub struct GmLoopNarratorAdapter<'a>(pub &'a GmLoop);

#[async_trait]
impl NarratorPort for GmLoopNarratorAdapter<'_> {
    async fn narrate(
        &self,
        packet: &NarrationPacket,
        private_tokens: &[String],
        tx: &Sender<TurnEvent>,
        cancel: Option<&CancellationToken>,
    ) -> Option<String> {
        self.0.run_narrator(packet, private_tokens, tx, cancel).await
    }
}

// --- RulesPort — wraps `resolve_check_with_input` + `execute_system_roll_bundle` (P2/P3). ---

/// The Rules layer entry. Each method resolves a [`CheckContract`] against the
/// product-mode dice policy and **persists its own `CheckResultRecord`** (this is
/// where check-resolution durability lives — there is no separate kernel commit).
#[async_trait]
pub trait RulesPort {
    /// Resolve a check whose dice come from a player/table `input` string.
    async fn resolve_check_with_input(
        &self,
        session_id: &str,
        turn_id: &str,
        contract: &CheckContract,
        input: &str,
    ) -> anyhow::Result<CheckResultRecord>;

    /// Auto-roll a check and consume any system-rollable damage/effect follow-ups.
    async fn execute_system_roll_bundle(
        &self,
        session_id: &str,
        turn_id: &str,
        contract: &CheckContract,
    ) -> anyhow::Result<AutoRollExecution>;
}

/// Thin adapter over a [`RuntimeEngine`]. Delegates to the real resolve entries.
pub struct EngineRulesAdapter<'a>(pub &'a RuntimeEngine);

#[async_trait]
impl RulesPort for EngineRulesAdapter<'_> {
    async fn resolve_check_with_input(
        &self,
        session_id: &str,
        turn_id: &str,
        contract: &CheckContract,
        input: &str,
    ) -> anyhow::Result<CheckResultRecord> {
        self.0
            .resolve_check_with_input(session_id, turn_id, contract, input)
            .await
    }

    async fn execute_system_roll_bundle(
        &self,
        session_id: &str,
        turn_id: &str,
        contract: &CheckContract,
    ) -> anyhow::Result<AutoRollExecution> {
        self.0
            .execute_system_roll_bundle(session_id, turn_id, contract)
            .await
    }
}

// --- WorldPort — wraps `load_world_reaction_plans` (P4); set is lossy, plans are lossless. ---

/// The World layer entry. Returns `(WorldReactionSet, Vec<NpcBehaviorPlan>)`: the
/// set's candidates are a LOSSY action-intent projection, while the retained plans
/// are the lossless byte-identity source the GM prompt renders from. The World layer
/// RESOLVES nothing — it only proposes posture/intent (fail-closed: a per-NPC load
/// error skips only that NPC; an empty set proposes nothing).
#[async_trait]
pub trait WorldPort {
    async fn load_reaction_plans(
        &self,
        session_id: &str,
        active_npc_ids: &[String],
        profiles: &[NpcProfile],
        relationship_targets: &[NpcRelationshipTarget],
        player_known_fact_ids: &[String],
    ) -> (WorldReactionSet, Vec<NpcBehaviorPlan>);
}

/// Thin adapter over a [`RuntimeEngine`]. Delegates to `load_world_reaction_plans`.
pub struct EngineWorldAdapter<'a>(pub &'a RuntimeEngine);

#[async_trait]
impl WorldPort for EngineWorldAdapter<'_> {
    async fn load_reaction_plans(
        &self,
        session_id: &str,
        active_npc_ids: &[String],
        profiles: &[NpcProfile],
        relationship_targets: &[NpcRelationshipTarget],
        player_known_fact_ids: &[String],
    ) -> (WorldReactionSet, Vec<NpcBehaviorPlan>) {
        load_world_reaction_plans(
            &self.0.db,
            session_id,
            active_npc_ids,
            profiles,
            relationship_targets,
            player_known_fact_ids,
        )
        .await
    }
}

// --- DirectorPort — wraps pure `build_director_brief_packet` + async `prepare_director_brief` (P5). ---

/// The Director layer entry. `build_brief_packet` is PURE / commit-nothing (fail-closed
/// reveal: a `None` knowledge input collapses the reveal set to empty). `prepare_brief`
/// is the thin-async runtime seam (flag-gated `TRPG_DIRECTOR_PACKET`; OFF ⇒ `None`,
/// byte-identical baseline) that loads the per-turn surfaces and renders the GM-only block.
#[async_trait]
pub trait DirectorPort {
    /// PURE: derive the typed [`DirectorPlan`] for one turn (no IO).
    #[allow(clippy::too_many_arguments)]
    fn build_brief_packet(
        &self,
        mode: DirectorMode,
        candidates: &[WorldReactionCandidate],
        story: &StoryState,
        player_known: Option<&[String]>,
        gm_truth: Option<&[String]>,
        spotlights: &[SpotlightState],
        rejected_thread_ids: &[String],
        acting_actor_id: &str,
    ) -> DirectorPlan;

    /// Thin-async: render the GM-only `[director_packet]` block, or `None` (flag OFF / no-op).
    async fn prepare_brief(
        &self,
        engine: &RuntimeEngine,
        session_id: &str,
        candidates: &[WorldReactionCandidate],
        acting_actor_id: &str,
    ) -> Option<String>;
}

/// Thin adapter delegating to the real Director entries. Stateless (the pure core
/// takes all inputs as args; `prepare_brief` takes the engine explicitly).
pub struct DirectorAdapter;

#[async_trait]
impl DirectorPort for DirectorAdapter {
    fn build_brief_packet(
        &self,
        mode: DirectorMode,
        candidates: &[WorldReactionCandidate],
        story: &StoryState,
        player_known: Option<&[String]>,
        gm_truth: Option<&[String]>,
        spotlights: &[SpotlightState],
        rejected_thread_ids: &[String],
        acting_actor_id: &str,
    ) -> DirectorPlan {
        build_director_brief_packet(
            mode,
            candidates,
            story,
            player_known,
            gm_truth,
            spotlights,
            rejected_thread_ids,
            acting_actor_id,
        )
    }

    async fn prepare_brief(
        &self,
        engine: &RuntimeEngine,
        session_id: &str,
        candidates: &[WorldReactionCandidate],
        acting_actor_id: &str,
    ) -> Option<String> {
        prepare_director_brief(&engine.db, session_id, candidates, acting_actor_id).await
    }
}

// --- PolicyPort — wraps pure `presentation_gate_decision` (P2/P6). ---

/// The Policy layer entry. PURE: folds a [`NarrationVerifierResult`] into a
/// [`PresentationGate`] via the LOCKED `BLOCKING_KINDS` whitelist. Advisory in the OFF
/// path (no behavior change); the only widening point is `presentation_gate.rs`.
pub trait PolicyPort {
    fn presentation_gate(&self, result: &NarrationVerifierResult) -> PresentationGate;
}

/// Thin adapter delegating to `presentation_gate_decision`. Stateless.
pub struct PresentationPolicyAdapter;

impl PolicyPort for PresentationPolicyAdapter {
    fn presentation_gate(&self, result: &NarrationVerifierResult) -> PresentationGate {
        presentation_gate_decision(result)
    }
}

// --- KernelPort — projections + the ONE real reveal commit primitive (P6). ---

/// The Kernel layer entry. Exposes the per-turn projection FOLDS (read-only;
/// commit-nothing — see invariant 1 about the `PlayerPartyEdge` side-write that these
/// pure projections must NEVER acquire) plus the single real commit primitive
/// ([`KernelPort::commit_reveal_fact`], the `PresentationCommit` reveal write).
/// `ResolutionCommit` is an advisory audit seam and is intentionally absent.
#[async_trait]
pub trait KernelPort {
    /// Assemble the verifier's private (GM-truth + per-NPC speech) view. Read-only fold.
    async fn verifier_private_view(
        &self,
        session_id: &str,
        active_npc_ids: &[String],
    ) -> VerifierPrivateView;

    /// Player-narration projection (player-known facts only). Read-only fold.
    async fn project_player_narration(
        &self,
        session_id: &str,
    ) -> anyhow::Result<PlayerNarrationProjection>;

    /// NPC-speech projection for one NPC (secret-gated by player-known facts). Read-only.
    async fn project_npc_speech(
        &self,
        session_id: &str,
        npc_actor_id: &str,
        profile: &NpcProfile,
        relationship_targets: &[NpcRelationshipTarget],
        player_known_fact_ids: &[String],
    ) -> anyhow::Result<NpcSpeechProjection>;

    /// NPC-action projection for one NPC (same load path as speech). Read-only.
    async fn project_npc_action(
        &self,
        session_id: &str,
        npc_actor_id: &str,
        profile: &NpcProfile,
        relationship_targets: &[NpcRelationshipTarget],
        player_known_fact_ids: &[String],
    ) -> anyhow::Result<NpcActionProjection>;

    /// The ONE real commit primitive: record a fact as revealed (idempotent per
    /// session+fact). GM/player-driven; the engine never keyword-matches reveals.
    async fn commit_reveal_fact(
        &self,
        session_id: &str,
        turn_id: &str,
        fact_id: &str,
        reason: Option<&str>,
    ) -> anyhow::Result<()>;
}

/// Thin adapter over a [`RuntimeEngine`]. Delegates to the projection/commit entries.
pub struct EngineKernelAdapter<'a>(pub &'a RuntimeEngine);

#[async_trait]
impl KernelPort for EngineKernelAdapter<'_> {
    async fn verifier_private_view(
        &self,
        session_id: &str,
        active_npc_ids: &[String],
    ) -> VerifierPrivateView {
        build_verifier_private_view(&self.0.db, session_id, active_npc_ids).await
    }

    async fn project_player_narration(
        &self,
        session_id: &str,
    ) -> anyhow::Result<PlayerNarrationProjection> {
        project_for_player_narration(&self.0.db, session_id).await
    }

    async fn project_npc_speech(
        &self,
        session_id: &str,
        npc_actor_id: &str,
        profile: &NpcProfile,
        relationship_targets: &[NpcRelationshipTarget],
        player_known_fact_ids: &[String],
    ) -> anyhow::Result<NpcSpeechProjection> {
        project_for_npc_speech(
            &self.0.db,
            session_id,
            npc_actor_id,
            profile,
            relationship_targets,
            player_known_fact_ids,
        )
        .await
    }

    async fn project_npc_action(
        &self,
        session_id: &str,
        npc_actor_id: &str,
        profile: &NpcProfile,
        relationship_targets: &[NpcRelationshipTarget],
        player_known_fact_ids: &[String],
    ) -> anyhow::Result<NpcActionProjection> {
        project_for_npc_action(
            &self.0.db,
            session_id,
            npc_actor_id,
            profile,
            relationship_targets,
            player_known_fact_ids,
        )
        .await
    }

    async fn commit_reveal_fact(
        &self,
        session_id: &str,
        turn_id: &str,
        fact_id: &str,
        reason: Option<&str>,
    ) -> anyhow::Result<()> {
        self.0
            .reveal_fact(session_id, turn_id, fact_id, reason)
            .await
    }
}

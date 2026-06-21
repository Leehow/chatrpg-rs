//! World-layer reaction assembly (P4.4).
//!
//! Two layers, mirroring the npc_behavior / verifier_private_view split:
//! - [`assemble_world_reaction_set`] — PURE. No async, no DB. Maps already-loaded
//!   [`NpcBehaviorPlan`]s through their P4.3 projection
//!   ([`NpcBehaviorPlan::to_reaction_candidate`]) into a deterministic
//!   [`WorldReactionSet`]. Table-driven testable.
//! - [`load_world_reaction_set`] — thin-async. REUSES the existing secret-gated load
//!   path [`load_active_npc_guidance`] per active NPC, then delegates to the pure
//!   assembler.
//!
//! ## fail-closed (mirror verifier_private_view.rs, field-independent)
//! A per-NPC load error skips ONLY that NPC (the rest of the set still assembles). A
//! total inability to proceed yields an EMPTY set — an empty set proposes nothing and
//! commits nothing, which is the SAFE direction.
//!
//! ## disclosure safety (§二十四-#4, P3 ⑤反思 forward constraint)
//! `player_known_fact_ids` is passed THROUGH to [`load_active_npc_guidance`] so the
//! secret gate runs against fresh player-known knowledge (never a stale ContextAssembly
//! snapshot). Reaction candidates' `knowledge_basis` is already narrowed to
//! `facts_can_reveal` by the P4.3 projection; this layer never widens it. Fail-closed
//! empty here never treats unknown-as-known.
//!
//! NOT yet wired into the turn loop (P4.6); adds no behavior to existing turns.
use trpg_db::Db;
use trpg_model::{
    NpcActionIntent, NpcActionKind, NpcBehaviorPlan, NpcProfile, NpcRelationshipTarget,
    RelationshipStance, WorldReactionSet,
};

use crate::npc_behavior::load_active_npc_guidance;

/// Minimum `willingness_to_fight` for the World layer to even PROPOSE an attack intent.
/// This is the World action gate's OWN threshold (P3 ⑤反思: a dedicated trigger, NOT
/// bound to secret_terms). A bare reaction proposes posture only; only a genuinely
/// combative plan (hostile stance + high fight willingness) yields an `Attack` intent.
/// The World layer still resolves NOTHING — it only emits the typed proposal that the
/// Rules/Kernel path settles downstream.
const ATTACK_INTENT_FIGHT_THRESHOLD: i16 = 60;

/// PURE: project each loaded [`NpcBehaviorPlan`] into a [`WorldReactionSet`].
///
/// No async, no DB, no mutation: same plans in ⇒ same set out. Candidate ordering
/// follows the input plan order (deterministic; caller controls it). `clock_advances`
/// and `knowledge_deltas` are intentionally left empty here — a bare reaction set
/// proposes posture only (clock/knowledge channels are wired later, P4.6).
pub fn assemble_world_reaction_set(plans: &[NpcBehaviorPlan]) -> WorldReactionSet {
    WorldReactionSet {
        reactions: plans.iter().map(|p| p.to_reaction_candidate()).collect(),
        clock_advances: Vec::new(),
        knowledge_deltas: Vec::new(),
    }
}

/// thin-async: load each active NPC's secret-gated [`NpcBehaviorPlan`] via the existing
/// [`load_active_npc_guidance`] path, then assemble the set with the pure projector.
///
/// fail-closed (mirrors `build_verifier_private_view`): a per-NPC load error warns and
/// skips ONLY that NPC; the remaining NPCs still contribute. There is no total-DB abort
/// path distinct from this — if every NPC fails (or `active_npc_ids` is empty / no
/// profiles), the result is simply an empty set, which proposes nothing.
///
/// `player_known_fact_ids` flows through to the secret gate; do NOT reuse a stale
/// ContextAssembly snapshot. `profiles` are matched to `active_npc_ids` by `actor_id`;
/// an NPC with no matching profile is skipped (fail-closed).
pub async fn load_world_reaction_set(
    db: &Db,
    session_id: &str,
    active_npc_ids: &[String],
    profiles: &[NpcProfile],
    relationship_targets: &[NpcRelationshipTarget],
    player_known_fact_ids: &[String],
) -> WorldReactionSet {
    let mut plans: Vec<NpcBehaviorPlan> = Vec::new();
    for npc_id in active_npc_ids {
        let Some(profile) = profiles.iter().find(|p| &p.actor_id == npc_id) else {
            tracing::debug!(
                npc_id = %npc_id,
                "world::reaction: no matching profile for active NPC, skipping (fail-closed)"
            );
            continue;
        };
        match load_active_npc_guidance(
            db,
            session_id,
            npc_id,
            profile,
            relationship_targets,
            player_known_fact_ids,
        )
        .await
        {
            Ok(plan) => plans.push(plan),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    npc_id = %npc_id,
                    "world::reaction: guidance load failed, skipping NPC (fail-closed)"
                );
            }
        }
    }
    assemble_world_reaction_set(&plans)
}

/// thin-async: same single-load path as [`load_world_reaction_set`] but RETAINS the
/// derived [`NpcBehaviorPlan`]s alongside the typed set, so the GM-context render reads
/// from the lossless plans (the typed candidate is a lossy projection).
///
/// CRITICAL (FRAMEWORK §2 reflection ①): this performs EXACTLY the same per-NPC DB loads
/// as the legacy `build_npc_behavior_guidance` path (one `load_active_npc_guidance` per
/// active NPC) — no extra DB reads. The retained plans are the byte-identity source for
/// [`render_world_reaction_block`]; the [`WorldReactionSet`] is the typed World view used
/// for action-intent derivation. Same fail-closed discipline as
/// [`load_world_reaction_set`] (per-NPC skip; empty ⇒ commit nothing).
///
/// M3 decision #4 (memory-wiring): the World layer reads WORLD memory only — per-NPC profiles /
/// mind / behavior guidance — and **never reads story pressure** (StoryState threads/promises/
/// beats). The only story-adjacent input is `player_known_fact_ids`, used purely as a SECRET GATE
/// (withhold what the player has not learned), not as story steering. story→world influence flows
/// one-way through the Director, never directly. Enforced structurally: this signature carries no
/// `StoryState`/story_state argument and the module references none.
pub async fn load_world_reaction_plans(
    db: &Db,
    session_id: &str,
    active_npc_ids: &[String],
    profiles: &[NpcProfile],
    relationship_targets: &[NpcRelationshipTarget],
    player_known_fact_ids: &[String],
) -> (WorldReactionSet, Vec<NpcBehaviorPlan>) {
    let mut plans: Vec<NpcBehaviorPlan> = Vec::new();
    for npc_id in active_npc_ids {
        let Some(profile) = profiles.iter().find(|p| &p.actor_id == npc_id) else {
            tracing::debug!(
                npc_id = %npc_id,
                "world::reaction: no matching profile for active NPC, skipping (fail-closed)"
            );
            continue;
        };
        match load_active_npc_guidance(
            db,
            session_id,
            npc_id,
            profile,
            relationship_targets,
            player_known_fact_ids,
        )
        .await
        {
            Ok(plan) => plans.push(plan),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    npc_id = %npc_id,
                    "world::reaction: guidance load failed, skipping NPC (fail-closed)"
                );
            }
        }
    }
    let set = assemble_world_reaction_set(&plans);
    (set, plans)
}

/// PURE byte-identical render: concatenate each plan's `to_guidance_block()` with `\n\n`,
/// EXACTLY as the legacy per-NPC `blocks.join("\n\n")` did. No plans ⇒ `None` (dynamic
/// tail writes no block, non-NPC turns stay byte-stable / cache-stable).
///
/// This routes the GM-context NPC guidance THROUGH the typed World layer (the caller now
/// builds a [`WorldReactionSet`] + retained plans) while keeping the player-visible /
/// GM-context bytes provably identical: the render reads the SAME plans via the SAME
/// `to_guidance_block` method as before. Locked by a byte-equality test.
pub fn render_world_reaction_block(plans: &[NpcBehaviorPlan]) -> Option<String> {
    if plans.is_empty() {
        return None;
    }
    let blocks: Vec<String> = plans.iter().map(|p| p.to_guidance_block()).collect();
    Some(blocks.join("\n\n"))
}

/// PURE (P4.6 Part B, gated by the CALLER): derive `Attack` action intents from the
/// ALREADY-LOADED plans (no double-load — P3 ⑤反思 forward constraint) and attach them to
/// the matching candidates in `set`. The World layer ONLY emits the typed intent; it
/// rolls no dice and resolves nothing (§二-⑤/⑥) — the Rules/Kernel path settles it.
///
/// Trigger gate is the World action gate's OWN (`willingness_to_fight` ≥ threshold AND a
/// hostile stance) — deliberately NOT bound to secret_terms. A plan below threshold yields
/// no intent (the candidate stays posture-only). `target_ref` names the player party (the
/// reaction's focus); `description` is a prose hint only, never a mechanical number.
pub fn derive_attack_intents(plans: &[NpcBehaviorPlan], set: &mut WorldReactionSet) {
    for plan in plans {
        if plan.willingness_to_fight < ATTACK_INTENT_FIGHT_THRESHOLD {
            continue;
        }
        if plan.stance != RelationshipStance::Hostile {
            continue;
        }
        if let Some(cand) = set.reactions.iter_mut().find(|c| c.npc_id == plan.npc_id) {
            cand.action_intent = Some(NpcActionIntent {
                kind: NpcActionKind::Attack,
                target_ref: Some("player_party".to_string()),
                description: "hostile NPC moves to attack the party".to_string(),
            });
        }
    }
}

#[cfg(test)]
#[path = "reaction_tests.rs"]
mod tests;

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
use trpg_model::{NpcBehaviorPlan, NpcProfile, NpcRelationshipTarget, WorldReactionSet};

use crate::npc_behavior::load_active_npc_guidance;

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

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::{
        NpcBehaviorContext, NpcKnowledgeEntry, NpcMindView, NpcRelationship, NpcRelationshipDelta,
    };

    fn profile(id: &str) -> NpcProfile {
        NpcProfile {
            actor_id: id.into(),
            name: id.into(),
            ..Default::default()
        }
    }

    fn rel(id: &str, trust: i16, fear: i16, hostility: i16) -> NpcRelationship {
        let mut r = NpcRelationship::new("s", id, NpcRelationshipTarget::PlayerParty).unwrap();
        r.apply_delta(&NpcRelationshipDelta {
            trust,
            fear,
            hostility,
            evidence_event_ids: vec!["e".into()],
            ..Default::default()
        })
        .unwrap();
        r
    }

    fn plan(id: &str, trust: i16, fear: i16, hostility: i16, entries: &[NpcKnowledgeEntry]) -> NpcBehaviorPlan {
        let view = NpcMindView::build("s", id, &profile(id), &[rel(id, trust, fear, hostility)], entries).unwrap();
        NpcBehaviorPlan::derive(&view, &NpcBehaviorContext::default())
    }

    #[test]
    fn assemble_empty_plans_yields_empty_set() {
        let set = assemble_world_reaction_set(&[]);
        assert!(set.is_empty());
    }

    #[test]
    fn assemble_preserves_plan_order_and_count() {
        let plans = vec![plan("npc_a", 0, 0, 70, &[]), plan("npc_b", 80, 0, 0, &[])];
        let set = assemble_world_reaction_set(&plans);
        assert_eq!(set.reactions.len(), 2);
        assert_eq!(set.reactions[0].npc_id, "npc_a");
        assert_eq!(set.reactions[1].npc_id, "npc_b");
        // clock/knowledge channels stay empty in a bare projection.
        assert!(set.clock_advances.is_empty());
        assert!(set.knowledge_deltas.is_empty());
    }

    #[test]
    fn assemble_matches_projection_per_plan() {
        // The set's candidates must equal each plan's own to_reaction_candidate output
        // (single source of truth is the P4.3 projection; no re-derivation here).
        let plans = vec![plan("npc_a", 20, 10, 40, &[]), plan("npc_b", 60, 0, 0, &[])];
        let set = assemble_world_reaction_set(&plans);
        for (p, cand) in plans.iter().zip(set.reactions.iter()) {
            assert_eq!(*cand, p.to_reaction_candidate());
        }
    }

    #[test]
    fn assemble_never_widens_knowledge_basis_beyond_revealable() {
        // §24-#4: a withheld secret id must never appear in any candidate's basis.
        let entries = vec![
            NpcKnowledgeEntry {
                fact_id: "shared".into(),
                state: trpg_model::KnowledgeState::KnowsTrue,
            },
            NpcKnowledgeEntry {
                fact_id: "secret".into(),
                state: trpg_model::KnowledgeState::KnowsTrue,
            },
        ];
        // Hostile + a context that marks "secret" as withheld.
        let view = NpcMindView::build(
            "s",
            "npc_a",
            &profile("npc_a"),
            &[rel("npc_a", 0, 0, 80)],
            &entries,
        )
        .unwrap();
        let ctx = NpcBehaviorContext {
            secret_fact_ids: vec!["secret".into()],
            ..Default::default()
        };
        let p = NpcBehaviorPlan::derive(&view, &ctx);
        assert!(p.facts_will_withhold.iter().any(|f| f == "secret"));
        let set = assemble_world_reaction_set(&[p]);
        let basis = &set.reactions[0].knowledge_basis;
        assert!(!basis.iter().any(|f| f == "secret"), "withheld secret leaked into World basis");
    }
}

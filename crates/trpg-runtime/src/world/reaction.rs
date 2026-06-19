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

    fn plan(
        id: &str,
        trust: i16,
        fear: i16,
        hostility: i16,
        entries: &[NpcKnowledgeEntry],
    ) -> NpcBehaviorPlan {
        let view = NpcMindView::build(
            "s",
            id,
            &profile(id),
            &[rel(id, trust, fear, hostility)],
            entries,
        )
        .unwrap();
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
        assert!(
            !basis.iter().any(|f| f == "secret"),
            "withheld secret leaked into World basis"
        );
    }

    // ── Part A: render byte-identity to the legacy per-NPC join ──────────────────────

    #[test]
    fn render_empty_plans_is_none() {
        // No plans ⇒ no block (dynamic tail writes nothing; non-NPC turns byte-stable).
        assert!(render_world_reaction_block(&[]).is_none());
    }

    #[test]
    fn render_is_byte_identical_to_legacy_join() {
        // The new render MUST equal the legacy `blocks.push(plan.to_guidance_block())` +
        // `blocks.join("\n\n")` path, byte-for-byte. This is the Part A behavior-preserving
        // lock: the GM-context bytes do not change on the default path.
        let plans = vec![
            plan("npc_a", 0, 0, 80, &[]),
            plan("npc_b", 70, 0, 0, &[]),
            plan("npc_c", 10, 60, 10, &[]),
        ];
        let legacy: Vec<String> = plans.iter().map(|p| p.to_guidance_block()).collect();
        let legacy_joined = legacy.join("\n\n");
        let rendered = render_world_reaction_block(&plans).expect("non-empty plans render");
        assert_eq!(
            rendered, legacy_joined,
            "render diverged from legacy join bytes"
        );
    }

    #[test]
    fn render_single_plan_matches_its_block() {
        let plans = vec![plan("npc_solo", 0, 0, 75, &[])];
        let rendered = render_world_reaction_block(&plans).unwrap();
        assert_eq!(rendered, plans[0].to_guidance_block());
        // A single block carries no "\n\n" separator (no trailing/leading join artifact).
        assert!(!rendered.starts_with("\n\n") && !rendered.ends_with("\n\n"));
    }

    // ── Part B: Attack intent derivation (World emits intent, never resolves) ─────────

    #[test]
    fn attack_intent_derived_for_hostile_combative_plan() {
        // Hostility 90 ⇒ hostile stance + high willingness_to_fight (≥ threshold) ⇒ Attack.
        let plans = vec![plan("npc_brute", 0, 0, 90, &[])];
        assert!(
            plans[0].willingness_to_fight >= ATTACK_INTENT_FIGHT_THRESHOLD,
            "fixture must clear the fight threshold"
        );
        assert_eq!(plans[0].stance, RelationshipStance::Hostile);
        let mut set = assemble_world_reaction_set(&plans);
        derive_attack_intents(&plans, &mut set);
        let intent = set.reactions[0]
            .action_intent
            .as_ref()
            .expect("hostile combative plan yields an Attack intent");
        assert_eq!(intent.kind, NpcActionKind::Attack);
        assert_eq!(intent.target_ref.as_deref(), Some("player_party"));
        // The intent carries NO mechanical number — prose hint only (World resolves nothing).
        assert!(!intent.description.chars().any(|c| c.is_ascii_digit()));
    }

    #[test]
    fn no_attack_intent_for_friendly_or_passive_plan() {
        // Trusting/warm NPC: not hostile, low fight willingness ⇒ candidate stays posture-only.
        let plans = vec![plan("npc_friend", 80, 0, 0, &[])];
        assert!(plans[0].willingness_to_fight < ATTACK_INTENT_FIGHT_THRESHOLD);
        let mut set = assemble_world_reaction_set(&plans);
        derive_attack_intents(&plans, &mut set);
        assert!(
            set.reactions[0].action_intent.is_none(),
            "friendly plan must not propose an attack"
        );
    }

    #[test]
    fn attack_intent_gate_is_independent_of_knowledge() {
        // P3 ⑤反思: the action gate is its OWN trigger — secret/knowledge state must not
        // change whether an Attack is proposed. Two hostile combative plans, one holding a
        // withheld secret, both still get the intent (gate keys on fight willingness/stance).
        let entries = vec![NpcKnowledgeEntry {
            fact_id: "secret".into(),
            state: trpg_model::KnowledgeState::KnowsTrue,
        }];
        let view = NpcMindView::build(
            "s",
            "npc_secretkeeper",
            &profile("npc_secretkeeper"),
            &[rel("npc_secretkeeper", 0, 0, 90)],
            &entries,
        )
        .unwrap();
        let ctx = NpcBehaviorContext {
            secret_fact_ids: vec!["secret".into()],
            ..Default::default()
        };
        let p_secret = NpcBehaviorPlan::derive(&view, &ctx);
        let plans = vec![plan("npc_brute", 0, 0, 90, &[]), p_secret];
        let mut set = assemble_world_reaction_set(&plans);
        derive_attack_intents(&plans, &mut set);
        assert!(set.reactions[0].action_intent.is_some());
        assert!(
            set.reactions[1].action_intent.is_some(),
            "withheld-secret holder still attacks; gate keyed on fight willingness, not secrets"
        );
    }

    // ── §二-⑤/⑥ guard: the World layer resolves NOTHING ─────────────────────────────

    #[test]
    fn world_dir_contains_no_dice_or_resolution_calls() {
        // The World layer MUST NOT roll dice or resolve outcomes — it only emits typed
        // proposals (NpcActionIntent). Mechanical settlement lives in Rules/Kernel. This
        // scans every .rs under crates/trpg-runtime/src/world/ for resolution primitives.
        // (Scoped to non-test code lines + comments; the markers below would only appear if
        // someone wired settlement INTO the World layer, which is the boundary violation.)
        use std::fs;
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/world");
        let banned = [
            "resolve_check_with_input",
            "thread_rng",
            "roll_seeded",
            "resolve_outcome",
        ];
        let mut hits: Vec<String> = Vec::new();
        for entry in fs::read_dir(dir).expect("world dir readable") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let src = fs::read_to_string(&path).expect("read world source");
            for needle in &banned {
                // Allow the needle inside THIS guard test's own `banned` array literal.
                let occurrences = src.matches(needle).count();
                let in_guard = src.contains("let banned = [") && path.ends_with("reaction.rs");
                let allowed = if in_guard && *needle != "" { 1 } else { 0 };
                if occurrences > allowed {
                    hits.push(format!("{}: {needle}", path.display()));
                }
            }
        }
        assert!(
            hits.is_empty(),
            "World layer must not resolve mechanics (§二-⑤/⑥); offending refs: {hits:?}"
        );
    }
}

//! Runtime orchestration for NPC Behavior Plan v1 (TC-NPC-04).
//!
//! The model ([`trpg_model::npc_behavior`]) owns the pure, deterministic derivation: from
//! a prompt-safe [`NpcMindView`] + a non-truth [`NpcBehaviorContext`] it produces an
//! [`NpcBehaviorPlan`] proposal. This adapter exposes the pure derivation plus the helper
//! that builds the non-truth context from the player party's known facts.
//!
//! It commits nothing: no DB writes, no events, no relationship mutation.
//!
//! Scope note (P0b foundation): the DB-bound load-and-plan entry points from the original
//! port (`load_npc_behavior_plan` / `load_active_npc_guidance`) depended on the gated
//! NPC-as-holder mind-view DB read (see runtime `npc_mind::assemble_npc_mind_view` scope
//! note and `docs/superpowers/specs/2026-06-17-npc-holder-identity-gate.md`). They are
//! intentionally NOT provided here; a caller assembles the mind view (supplying the NPC's
//! knowledge entries) and then derives the plan via [`derive_npc_behavior_plan`].
use std::collections::HashSet;

use trpg_model::{NpcBehaviorContext, NpcBehaviorPlan, NpcMindView};

/// Pure derivation step: project the behavior plan from an already-built mind view and a
/// non-truth context. No IO. Deterministic.
pub fn derive_npc_behavior_plan(view: &NpcMindView, ctx: &NpcBehaviorContext) -> NpcBehaviorPlan {
    NpcBehaviorPlan::derive(view, ctx)
}

/// Build the non-truth [`NpcBehaviorContext`] used when planning an active NPC's turn,
/// from the NPC's own mind view and the player party's known/revealed fact ids.
///
/// v1 secret gate (conservative, fail-closed): a fact this NPC knows as TRUE that the
/// player party does NOT already know is treated as a secret id. The derivation then
/// withholds those ids unless reveal willingness clears the threshold. Only ids the NPC
/// actually knows are passed (ids the NPC merely believes or never encountered cannot be
/// "secrets"). No fact prose ever enters the context — ids only.
pub fn viewer_behavior_context(
    view: &NpcMindView,
    player_known_fact_ids: &[String],
) -> NpcBehaviorContext {
    let player_known: HashSet<&str> = player_known_fact_ids.iter().map(String::as_str).collect();
    let secret_fact_ids = view
        .known_fact_ids()
        .into_iter()
        .filter(|fid| !player_known.contains(fid))
        .map(str::to_string)
        .collect();
    NpcBehaviorContext {
        secret_fact_ids,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::{
        KnowledgeState, NpcKnowledgeEntry, NpcProfile, NpcRelationship, NpcRelationshipDelta,
        NpcRelationshipTarget,
    };

    fn profile() -> NpcProfile {
        NpcProfile {
            actor_id: "npc_alice".into(),
            name: "Alice".into(),
            ..Default::default()
        }
    }

    fn view(rel: NpcRelationship, entries: &[NpcKnowledgeEntry]) -> NpcMindView {
        NpcMindView::build("s", "npc_alice", &profile(), &[rel], entries).unwrap()
    }

    fn hostile_rel() -> NpcRelationship {
        let mut r =
            NpcRelationship::new("s", "npc_alice", NpcRelationshipTarget::PlayerParty).unwrap();
        r.apply_delta(&NpcRelationshipDelta {
            hostility: 80,
            evidence_event_ids: vec!["e".into()],
            ..Default::default()
        })
        .unwrap();
        r
    }

    #[test]
    fn adapter_matches_model_derivation() {
        let v = view(hostile_rel(), &[]);
        let ctx = NpcBehaviorContext::default();
        assert_eq!(
            derive_npc_behavior_plan(&v, &ctx),
            NpcBehaviorPlan::derive(&v, &ctx),
            "runtime adapter must be a pure passthrough to the model"
        );
    }

    fn known(id: &str) -> NpcKnowledgeEntry {
        NpcKnowledgeEntry { fact_id: id.into(), state: KnowledgeState::KnowsTrue }
    }

    #[test]
    fn viewer_context_marks_player_unknown_known_facts_secret() {
        // npc knows two true facts; the party already knows one of them. Only the fact the
        // party does NOT know becomes a secret id; the shared one is freely revealable.
        let entries = vec![known("shared"), known("hidden")];
        let v = view(hostile_rel(), &entries);
        let ctx = viewer_behavior_context(&v, &["shared".to_string()]);
        assert_eq!(ctx.secret_fact_ids, vec!["hidden".to_string()]);

        let plan = derive_npc_behavior_plan(&v, &ctx);
        assert!(plan.facts_will_withhold.iter().any(|f| f == "hidden"));
        assert!(plan.facts_can_reveal.iter().any(|f| f == "shared"));
        assert!(!plan.facts_can_reveal.iter().any(|f| f == "hidden"));
    }

    #[test]
    fn viewer_context_ignores_beliefs_and_unknown_ids() {
        // A belief (not known truth) cannot be a secret, and an empty player-known set must
        // not invent secrets from facts the NPC does not hold as true.
        let entries = vec![
            known("k"),
            NpcKnowledgeEntry { fact_id: "rumor".into(), state: KnowledgeState::BelievesFalse },
        ];
        let v = view(hostile_rel(), &entries);
        let ctx = viewer_behavior_context(&v, &[]);
        assert_eq!(ctx.secret_fact_ids, vec!["k".to_string()]);
        assert!(!ctx.secret_fact_ids.iter().any(|f| f == "rumor"));
    }

    #[test]
    fn hostile_plan_lowers_help_and_withholds_secret() {
        let entries = vec![NpcKnowledgeEntry {
            fact_id: "s1".into(),
            state: KnowledgeState::KnowsTrue,
        }];
        let ctx = NpcBehaviorContext {
            secret_fact_ids: vec!["s1".into()],
            ..Default::default()
        };
        let plan = derive_npc_behavior_plan(&view(hostile_rel(), &entries), &ctx);
        assert!(plan.willingness_to_help < 50);
        assert!(plan.facts_will_withhold.iter().any(|f| f == "s1"));
        assert!(!plan.facts_can_reveal.iter().any(|f| f == "s1"));
    }
}

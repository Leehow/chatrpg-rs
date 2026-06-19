//! P4.6 Part B — NpcActionIntent{Attack} vertical slice (flag-gated, default OFF).
//!
//! ## Layer boundary (设计4补充 §二-⑤/⑥)
//! The **World layer emits the typed intent only** (`NpcActionIntent{Attack}` on a
//! [`trpg_model::WorldReactionCandidate`]); it rolls no dice and resolves nothing. THIS
//! module is the **Rules/Kernel side**: it takes a World-emitted Attack intent and routes
//! it through the SAME mechanical-resolution entry the GM `roll_check` tool uses
//! ([`RuntimeEngine::resolve_check_with_input`]). The dice/outcome live entirely in Rules.
//!
//! ## Flag gate (behavior-preserving)
//! All of this is behind `TRPG_WORLD_NPC_ACTION` (default OFF). OFF ⇒ this module is never
//! entered ⇒ zero new behavior ⇒ byte-identical turns. ON ⇒ a hostile NPC's Attack intent
//! reaches `roll_check`; the resolution is folded as an advisory gate fact (it never
//! aborts the turn, never overrides the GM).
//!
//! ## Mirror of the existing opposed prepass
//! [`prepare_npc_attack_binding`] mirrors `opposed_prepass::prepare_binding`'s discipline
//! (fail-closed, source-backed, kernel-defaulted dice) but is NPC-initiated: the initiator
//! is the reacting NPC, not the player. For the v1 slice the opposition is
//! `NoMechanicalOpposition` (an attack roll against the kernel's core mechanic); a full
//! opposed defender binding is a documented follow-up (it would re-use the same
//! `attack_defense_param` + NeedBus defender synth the player path already has).
use trpg_model::{
    ActorKind, ActorRef, CheckContract, CheckStakes, CheckTargetModel, OppositionModel,
    RollAuthority, RollDisclosurePolicy, RollVisibility, RulingConfidence, RulingStatus,
    WorldReactionSet,
};
use trpg_runtime::RuntimeEngine;

/// Env gate `TRPG_WORLD_NPC_ACTION` — default **OFF**. Mirrors the `TRPG_NARRATOR_SPLIT` /
/// `TRPG_PRESENTATION_GATE` idiom (explicit `1`/`true`). OFF ⇒ no NPC action behavior.
pub fn world_npc_action_enabled() -> bool {
    std::env::var("TRPG_WORLD_NPC_ACTION")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Build the Rules-side [`CheckContract`] for one NPC-initiated attack. PURE (no IO): the
/// World layer has already decided (via its own gate) that this NPC attacks; here we only
/// shape the mechanical contract. The kernel's core dice/target are filled in downstream
/// by `apply_kernel_defaults_if_unsourced` (we leave `UnknownUntilLookup` +
/// `RulingStatus::Provisional` so the kernel binds them — never a hardcoded DV).
///
/// `initiator` is the attacking NPC (NOT the player). Opposition is
/// `NoMechanicalOpposition` for the v1 slice (attack vs. the kernel core mechanic).
pub fn prepare_npc_attack_binding(
    session_id: &str,
    turn_id: &str,
    ruleset_id: &str,
    npc_id: &str,
    npc_display_name: Option<&str>,
) -> CheckContract {
    CheckContract {
        check_id: format!("npc_attack:{turn_id}:{npc_id}"),
        session_id: session_id.into(),
        turn_id: turn_id.into(),
        ruleset_id: ruleset_id.into(),
        module_id: None,
        initiator: ActorRef {
            actor_id: npc_id.into(),
            actor_kind: ActorKind::Npc,
            display_name: npc_display_name.map(str::to_string),
        },
        target_actor: None,
        opposition: OppositionModel::NoMechanicalOpposition,
        action_summary: "NPC attacks the party".into(),
        intent_kind: "world:npc_attack".into(),
        check_label: "NPC attack (World reaction)".into(),
        dice_expression: String::new(),
        modifiers: vec![],
        target: CheckTargetModel::UnknownUntilLookup,
        tested_parameter: None,
        opponent_tested_parameter: None,
        actor_snapshot_ids: vec![],
        source_refs: vec![],
        learned_packet_ids: vec![],
        roll_visibility: RollVisibility::PublicGmRoll,
        roll_authority: RollAuthority::System,
        disclosure: RollDisclosurePolicy::for_visibility(RollVisibility::PublicGmRoll),
        stakes: CheckStakes::default(),
        confidence: RulingConfidence::Medium,
        ruling_status: RulingStatus::Provisional,
        advice_refs: vec![],
        expires_at_turn: None,
    }
}

/// Route every World-emitted Attack intent in `set` through Rules mechanical resolution.
///
/// For each candidate carrying an `Attack` intent: build the NPC-attack contract and call
/// [`RuntimeEngine::resolve_check_with_input`] — the SAME entry the GM `roll_check` tool
/// uses (so the roll happens in Rules, never in World). Returns a fact line per resolved
/// attack (advisory; the caller folds it into the dynamic tail). fail-closed/fail-soft: a
/// missing display name or a resolution error skips that NPC and never aborts the turn.
pub async fn resolve_world_attack_intents(
    engine: &RuntimeEngine,
    session_id: &str,
    turn_id: &str,
    ruleset_id: &str,
    set: &WorldReactionSet,
) -> Vec<String> {
    use trpg_model::NpcActionKind;
    let mut facts = Vec::new();
    for cand in &set.reactions {
        let Some(intent) = &cand.action_intent else {
            continue;
        };
        if intent.kind != NpcActionKind::Attack {
            continue;
        }
        let contract =
            prepare_npc_attack_binding(session_id, turn_id, ruleset_id, &cand.npc_id, None);
        match engine
            .resolve_check_with_input(session_id, turn_id, &contract, "")
            .await
        {
            Ok(result) => {
                facts.push(format!(
                    "[roll]World NPC attack ({}) resolved: {}[/roll]",
                    cand.npc_id,
                    serde_json::to_string(&result.outcome).unwrap_or_default()
                ));
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    npc_id = %cand.npc_id,
                    "world npc attack: resolution failed (fail-soft, turn continues)"
                );
            }
        }
    }
    facts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_defaults_off() {
        // No env override ⇒ OFF. (Process-global env: we only assert the default branch via
        // a guaranteed-unset companion check on a truthy string.)
        assert!(!world_npc_action_enabled() || std::env::var("TRPG_WORLD_NPC_ACTION").is_ok());
    }

    #[test]
    fn binding_initiator_is_the_npc_not_the_player() {
        let c = prepare_npc_attack_binding("s1", "t1", "coc", "npc_lars", Some("Lars"));
        assert_eq!(c.initiator.actor_id, "npc_lars");
        assert_eq!(c.initiator.actor_kind, ActorKind::Npc);
        assert_eq!(c.initiator.display_name.as_deref(), Some("Lars"));
        assert_eq!(c.session_id, "s1");
        assert_eq!(c.ruleset_id, "coc");
        assert_eq!(c.intent_kind, "world:npc_attack");
    }

    #[test]
    fn binding_carries_no_hardcoded_dv_or_dice() {
        // Source-backed discipline: the World slice must NOT hardcode a DV or dice — the
        // kernel binds them downstream. Contract leaves them unbound/provisional.
        let c = prepare_npc_attack_binding("s1", "t1", "coc", "npc_x", None);
        assert!(c.dice_expression.is_empty(), "no hardcoded dice");
        assert!(matches!(c.target, CheckTargetModel::UnknownUntilLookup));
        assert!(matches!(c.ruling_status, RulingStatus::Provisional));
        assert!(c.source_refs.is_empty());
    }

    #[test]
    fn check_id_is_deterministic_per_turn_and_npc() {
        let a = prepare_npc_attack_binding("s", "turn_9", "coc", "npc_a", None);
        let b = prepare_npc_attack_binding("s", "turn_9", "coc", "npc_a", None);
        assert_eq!(a.check_id, b.check_id);
        let c = prepare_npc_attack_binding("s", "turn_9", "coc", "npc_b", None);
        assert_ne!(a.check_id, c.check_id);
    }
}

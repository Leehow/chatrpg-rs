//! P4.6 Part B — NpcActionIntent{Attack} vertical slice (flag-gated, default OFF).
//!
//! ## Layer boundary (设计4补充 §二-⑤/⑥)
//! The **World layer emits the typed intent only** (`NpcActionIntent{Attack}` on a
//! [`trpg_model::WorldReactionCandidate`]); it rolls no dice and resolves nothing. THIS
//! module is the **Rules/Kernel side**: it takes a World-emitted Attack intent and routes
//! it through the SAME mechanical-resolution entry the GM `roll_check` tool uses
//! ([`RuntimeEngine::execute_system_roll_bundle`] — exactly what `tools::settle` calls). It
//! hydrates the ruleset's core mechanic via `apply_kernel_defaults_if_unsourced`, so an
//! NPC-initiated attack against a CoC-style kernel resolves to a REAL hit/miss band (not a
//! `blocked: missing_source_backed_parameters` stub). The dice/outcome live entirely in
//! Rules; the World layer never rolls.
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
    ActorKind, ActorRef, CheckContract, CheckStakes, CheckTargetModel, DomainEvent,
    DomainEventKind, OppositionModel, RollAuthority, RollDisclosurePolicy, RollVisibility,
    RulingConfidence, RulingStatus, WorldReactionSet,
};
use crate::ports::{EngineRulesAdapter, RulesPort};
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

/// One World-NPC-attack resolution outcome (Rules-side), returned so the caller can both
/// (a) fold a player-perceivable fact into the turn and (b) assert the real mechanics. This
/// is the typed, observably-consumed result — NOT a silently discarded side effect.
#[derive(Debug, Clone)]
pub struct WorldAttackOutcome {
    /// The attacking NPC id (initiator of the System roll).
    pub npc_id: String,
    /// The deterministic `check_results` row id this resolution committed.
    pub check_id: String,
    /// `true`/`false` once the kernel resolves a hit/miss; `None` if the kernel left it
    /// unresolved (e.g. degree-only) or the contract genuinely blocked.
    pub success: Option<bool>,
    /// The named success band/tier when the kernel emits one (e.g. CoC "常规成功"); else None.
    pub success_tier: Option<String>,
    /// `true` when Rules returned the `blocked: missing_source_backed_parameters` stub
    /// (the kernel could not hydrate source-backed params for this attack).
    pub blocked: bool,
    /// The full outcome JSON (the source of truth; the fields above are a typed digest).
    pub outcome: serde_json::Value,
}

impl WorldAttackOutcome {
    /// A single player-perceivable `[roll]…[/roll]` fact line for the dynamic tail. This is
    /// the load-bearing consumption: it flows into `resolved_gate_facts` →
    /// `player_perceivable_facts`, so the resolved attack actually reaches the GM context.
    pub fn to_gate_fact(&self) -> String {
        let verdict = match (self.blocked, self.success) {
            (true, _) => "paused (source-backed params pending)".to_string(),
            (false, Some(true)) => match &self.success_tier {
                Some(t) => format!("HIT ({t})"),
                None => "HIT".to_string(),
            },
            (false, Some(false)) => match &self.success_tier {
                Some(t) => format!("MISS ({t})"),
                None => "MISS".to_string(),
            },
            (false, None) => "resolved (no hit/miss verdict)".to_string(),
        };
        format!("[roll]World NPC attack ({}): {}[/roll]", self.npc_id, verdict)
    }
}

/// P6.3 PURE constructor: typed [`WorldAttackOutcome`] → [`DomainEvent`] of kind
/// [`DomainEventKind::NpcActionResolved`]. The data digest is read **directly from the
/// struct fields** (npc_id / check_id / success / success_tier / blocked) — NEVER parsed
/// back out of [`WorldAttackOutcome::to_gate_fact`]'s display string.
///
/// `event_id = de_npcaction_{check_id}`. `check_id` is the deterministic `check_results`
/// row id (`npc_attack:{turn}:{npc}`), so a replay of the same resolution folds to one row
/// (`append_domain_event` is `on conflict (event_id) do nothing`).
pub fn npc_action_resolved_event(
    o: &WorldAttackOutcome,
    session_id: &str,
    turn_id: &str,
) -> DomainEvent {
    DomainEvent::new(
        format!("de_npcaction_{}", o.check_id),
        session_id,
        turn_id,
        DomainEventKind::NpcActionResolved,
        serde_json::json!({
            "npc_id": o.npc_id,
            "check_id": o.check_id,
            "success": o.success,
            "success_tier": o.success_tier,
            "blocked": o.blocked,
        }),
    )
}

/// P6.3 emit seam: append one [`DomainEventKind::NpcActionResolved`] per resolved attack,
/// fail-soft. Append failure only warns — the narration/turn already proceeded, the event
/// log is advisory and must never reach back into control flow. Used by the turn loop's
/// outcome-fold AND by the live test (so the ON-path row is directly assertable).
pub async fn emit_npc_action_resolved(
    db: &trpg_db::Db,
    outcomes: &[WorldAttackOutcome],
    session_id: &str,
    turn_id: &str,
) {
    for o in outcomes {
        let ev = npc_action_resolved_event(o, session_id, turn_id);
        if let Err(e) = db.append_domain_event(&ev).await {
            tracing::warn!(error = %e, check_id = %o.check_id, "append NpcActionResolved domain event failed (non-fatal)");
        }
    }
}

/// Route every World-emitted Attack intent in `set` through Rules mechanical resolution.
///
/// For each candidate carrying an `Attack` intent: build the NPC-attack contract and call
/// [`RuntimeEngine::execute_system_roll_bundle`] — the SAME entry the GM `roll_check` tool
/// uses via `tools::settle` (so the roll happens in Rules, never in World, AND the kernel
/// core mechanic is hydrated → a real hit/miss band rather than a blocked stub). Returns a
/// typed [`WorldAttackOutcome`] per resolved attack so the caller can fold a gate fact AND
/// assert the mechanics. fail-soft: a resolution error skips that NPC, never aborts the turn.
pub async fn resolve_world_attack_intents(
    engine: &RuntimeEngine,
    session_id: &str,
    turn_id: &str,
    ruleset_id: &str,
    set: &WorldReactionSet,
) -> Vec<WorldAttackOutcome> {
    use trpg_model::NpcActionKind;
    let mut outcomes = Vec::new();
    for cand in &set.reactions {
        let Some(intent) = &cand.action_intent else {
            continue;
        };
        if intent.kind != NpcActionKind::Attack {
            continue;
        }
        let contract =
            prepare_npc_attack_binding(session_id, turn_id, ruleset_id, &cand.npc_id, None);
        // P7.3b: dispatch the Rules-layer auto-roll through the RulesPort adapter
        // (EngineRulesAdapter) — a thin byte-identical delegate, but a real production
        // caller of RulesPort::execute_system_roll_bundle.
        match EngineRulesAdapter(engine)
            .execute_system_roll_bundle(session_id, turn_id, &contract)
            .await
        {
            Ok(bundle) => {
                let outcome = &bundle.primary.outcome;
                outcomes.push(WorldAttackOutcome {
                    npc_id: cand.npc_id.clone(),
                    check_id: bundle.primary.check_id.clone(),
                    success: outcome.get("success").and_then(|v| v.as_bool()),
                    success_tier: outcome
                        .get("success_tier")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    blocked: outcome
                        .get("blocked")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    outcome: outcome.clone(),
                });
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
    outcomes
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
    fn npc_action_resolved_event_is_typed_digest_not_string_scrape() {
        // P6.3: the domain event's data must read the typed struct fields directly — NOT a
        // parse of to_gate_fact()'s display string. event_id is keyed on check_id (replay-
        // idempotent, since check_id is the deterministic check_results row id).
        let o = WorldAttackOutcome {
            npc_id: "npc_lars".into(),
            check_id: "npc_attack:t1:npc_lars".into(),
            success: Some(false),
            success_tier: Some("失败".into()),
            blocked: false,
            outcome: serde_json::json!({"success": false}),
        };
        let ev = npc_action_resolved_event(&o, "sess_a", "t1");
        assert_eq!(ev.event_id, "de_npcaction_npc_attack:t1:npc_lars");
        assert_eq!(ev.kind, DomainEventKind::NpcActionResolved);
        assert_eq!(ev.session_id, "sess_a");
        assert_eq!(ev.turn_id, "t1");
        // Typed fields, not a string scrape.
        assert_eq!(ev.data["npc_id"].as_str(), Some("npc_lars"));
        assert_eq!(ev.data["check_id"].as_str(), Some("npc_attack:t1:npc_lars"));
        assert_eq!(ev.data["success"].as_bool(), Some(false));
        assert_eq!(ev.data["success_tier"].as_str(), Some("失败"));
        assert_eq!(ev.data["blocked"].as_bool(), Some(false));
        // None success ⇒ JSON null (typed Option propagation, no string fallback).
        let o2 = WorldAttackOutcome {
            success: None,
            success_tier: None,
            ..o.clone()
        };
        let ev2 = npc_action_resolved_event(&o2, "sess_a", "t1");
        assert!(ev2.data["success"].is_null());
        assert!(ev2.data["success_tier"].is_null());
        // Same check_id ⇒ same idempotent event_id (replay folds to one row in db).
        assert_eq!(ev.event_id, ev2.event_id);
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

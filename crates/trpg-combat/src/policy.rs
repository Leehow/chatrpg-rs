//! P0-2 engine-dehardcode: combat strategy as DATA, not Rust branches.
//!
//! These pure functions replace trpg-combat's eight ruleset/module name
//! branches. The per-ruleset/module specific VALUES now live in data files
//! (`data/parsed/rules/{ruleset}.rule_kernel.override.json` →
//! `RuleKernel.{combat_mode_policy,check_label_policy,dice_qualification,combat_profile}`;
//! `data/modules/{id}.module_config.json` → `ModuleConfig`). When a kernel/module
//! field is None the engine uses the NEUTRAL `trpg_model::GENERIC_*` defaults,
//! which are exact equivalents of the old generic fallthrough — never a ruleset
//! branch. The CombatMode variant names below are mode names, not ruleset names.

use crate::{ReactionAdvice, RulesetCombatProfile};
use serde_json::{json, Value};
use trpg_model::{
    ActorKind, ActorRef, CheckLabelPolicy, CombatMode, CombatModePolicy, CombatProfile,
    ConflictIntent, DiceQualification, ModuleConfig, SituationActionKind,
    GENERIC_CHECK_LABEL_POLICY, GENERIC_COMBAT_MODE_POLICY, GENERIC_DICE_QUALIFICATION,
};

/// The single-slot, non-persistent combat opposition placeholder. It is reused
/// across turns and scenes (the "串台坑 §6③" the runtime comments call out) and
/// must NEVER be persisted as a knowledge holder id. NPC-holder identity gate:
/// `docs/superpowers/specs/2026-06-17-npc-holder-identity-gate.md`.
pub const COMBAT_OPPOSITION_PLACEHOLDER: &str = "npc.opposition";

/// Recover the stable module-graph NPC id behind a resolved combat opposition
/// actor, or `None` when none exists. Returns `None` for the `npc.opposition`
/// placeholder, empty ids, and non-`Npc` actors — it never fabricates a
/// persistent id. This is the frame-time seed the future
/// `persistent_npc_holder_id` resolver (gate spec, slice 2) consumes to reject
/// the placeholder and recover the real graph id when one exists.
pub fn persistent_opposition_graph_id(actor: Option<&ActorRef>) -> Option<String> {
    let actor = actor?;
    if actor.actor_kind != ActorKind::Npc {
        return None;
    }
    let id = actor.actor_id.trim();
    if id.is_empty() || id == COMBAT_OPPOSITION_PLACEHOLDER {
        return None;
    }
    Some(id.to_string())
}

/// Build the explicit identity binding recorded on the combat opposition
/// participant's `metadata`. When a stable module-graph id exists it is carried
/// as `persistent_graph_id` with `persistent: true`; the generic fallback
/// (no module binding matched / no combat intent) is marked `persistent: false`
/// with a null id — never a fabricated holder. A future
/// `persistent_npc_holder_id` resolver reads this off the active frame to reject
/// the placeholder and recover the real id without reopening the knowledge schema.
pub fn opposition_identity_binding(resolved: Option<&ActorRef>) -> Value {
    match persistent_opposition_graph_id(resolved) {
        Some(graph_id) => json!({
            "source": "runtime_parameter_hydrator_v1_8",
            "npc_identity_binding": {
                "placeholder_actor_id": COMBAT_OPPOSITION_PLACEHOLDER,
                "persistent_graph_id": graph_id,
                "persistent": true,
                "binding_source": "module_config.npc_actor_bindings",
            }
        }),
        None => json!({
            "source": "runtime_parameter_hydrator_v1_8",
            "npc_identity_binding": {
                "placeholder_actor_id": COMBAT_OPPOSITION_PLACEHOLDER,
                "persistent_graph_id": Value::Null,
                "persistent": false,
                "binding_source": "generic_opposition_fallback",
            }
        }),
    }
}

/// Resolve the CombatMode for an intent from the kernel's data policy.
/// Replaces `infer_combat_mode_from_intent`'s ruleset_id.contains branches:
/// first matching rule wins (action AND evidence gated), else the policy
/// fallback_mode. None policy → GENERIC_COMBAT_MODE_POLICY (→ TheaterOfMind).
pub fn combat_mode_from_policy(
    policy: Option<&CombatModePolicy>,
    intent: &ConflictIntent,
) -> CombatMode {
    let pol = policy.unwrap_or(&GENERIC_COMBAT_MODE_POLICY);
    let action = intent.action_kind.as_str();
    for rule in &pol.rules {
        let action_ok =
            rule.when_action_kinds.is_empty() || rule.when_action_kinds.iter().any(|a| a == action);
        let evidence_ok = rule.when_evidence_contains.is_empty()
            || rule
                .when_evidence_contains
                .iter()
                .any(|e| intent.evidence_terms.iter().any(|t| t.contains(e.as_str())));
        if action_ok && evidence_ok {
            return rule.mode;
        }
    }
    pol.fallback_mode
}

/// Check-label for an action family. Replaces the `contains("cyberpunk")` label
/// special-case: a kernel CheckLabelPolicy entry (keyed by action.as_str()) wins;
/// otherwise the neutral per-family fallback (identical to the old `else` arms).
pub fn check_label_for(policy: Option<&CheckLabelPolicy>, action: SituationActionKind) -> String {
    let pol = policy.unwrap_or(&GENERIC_CHECK_LABEL_POLICY);
    if let Some(label) = pol.labels.get(action.as_str()) {
        return label.clone();
    }
    match action {
        SituationActionKind::Hack | SituationActionKind::DisableDevice => {
            "appropriate technical conflict check"
        }
        SituationActionKind::Attack => "appropriate attack/conflict check",
        SituationActionKind::UnderAttack
        | SituationActionKind::EnemyInitiatedConflict
        | SituationActionKind::SceneEntersConflict => "appropriate defense/reaction check",
        SituationActionKind::Defend | SituationActionKind::Dodge => {
            "appropriate defense/evasion check"
        }
        SituationActionKind::InvestigateDuringConflict => {
            "appropriate perception/investigation-under-pressure check"
        }
        SituationActionKind::Intimidate => "appropriate intimidation/social pressure check",
        _ => "appropriate situation check",
    }
    .to_string()
}

/// Qualify a bare dice expression (e.g. "1d10" → "1d10+0") from the kernel's
/// DiceQualification template. Replaces the hardcoded "1d10+0" cyberpunk degrade.
/// None policy → GENERIC_DICE_QUALIFICATION ("{dice}+0"). Returns None when the
/// template is empty (no qualification) so callers leave the expression untouched.
pub fn qualify_bare_dice(q: Option<&DiceQualification>, bare_dice: &str) -> Option<String> {
    let pol = q.unwrap_or(&GENERIC_DICE_QUALIFICATION);
    let tmpl = pol.bare_dice_template.trim();
    if tmpl.is_empty() {
        return None;
    }
    let dice = bare_dice.trim();
    if dice.is_empty() {
        return None;
    }
    Some(tmpl.replace("{dice}", dice))
}

/// Resolve the combat target NPC actor from the module's npc_actor_bindings.
/// Replaces the hardcoded npc.scav_boss / npc.athena_drone literals: the first
/// binding whose matcher substring hits (case-insensitive) wins, else the
/// generic `npc.opposition`. Non-attack intents return None (unchanged).
pub fn actor_for_combat_input(
    cfg: Option<&ModuleConfig>,
    input: &str,
    intent: &ConflictIntent,
) -> Option<ActorRef> {
    if !matches!(
        intent.action_kind,
        SituationActionKind::Attack
            | SituationActionKind::Counterattack
            | SituationActionKind::UnderAttack
            | SituationActionKind::EnemyInitiatedConflict
            | SituationActionKind::SceneEntersConflict
    ) {
        return None;
    }
    let lower = input.to_ascii_lowercase();
    let (actor_id, display_name) = cfg
        .and_then(|c| {
            c.npc_actor_bindings
                .iter()
                .find(|b| {
                    b.matcher
                        .iter()
                        .any(|k| lower.contains(&k.to_ascii_lowercase()))
                })
                .map(|b| {
                    (
                        b.actor_id.clone(),
                        b.display_name
                            .clone()
                            .unwrap_or_else(|| "opposition".to_string()),
                    )
                })
        })
        .unwrap_or_else(|| ("npc.opposition".to_string(), "opposition".to_string()));
    Some(ActorRef {
        actor_id,
        actor_kind: ActorKind::Npc,
        display_name: Some(display_name),
    })
}

/// Resolve a technical-option DV from the module's technical_option_table.
/// Replaces `inferred_homecoming_tech_dv`'s 14/12: first matcher hit wins.
/// None module / no match → None (unchanged: the target stays UnknownUntilLookup).
pub fn tech_dv_from_config(cfg: Option<&ModuleConfig>, input: &str) -> Option<i32> {
    let lower = input.to_ascii_lowercase();
    cfg?.technical_option_table
        .as_ref()?
        .iter()
        .find(|t| {
            t.matcher
                .iter()
                .any(|k| lower.contains(&k.to_ascii_lowercase()))
        })
        .map(|t| t.dv)
}

/// Whether an `investigate` declaration should open a new situation frame.
/// Replaces `classify_situation_intent`'s `contains("triangle")` gate: driven by
/// the profile's `investigate_opens_frame` flag (false == old non-triangle).
pub fn investigate_opens_frame_relation(investigate_opens: bool, investigate_score: f32) -> bool {
    investigate_opens && investigate_score >= 0.42
}

/// Whether a Low-confidence intent may still start a frame.
/// Replaces `should_start_frame`'s `contains("triangle")` gate: driven by the
/// profile's `low_confidence_frame_start` flag (false == old non-triangle).
pub fn low_confidence_frame_start_ok(
    low_confidence_ok: bool,
    confidence: trpg_model::RulingConfidence,
) -> bool {
    confidence != trpg_model::RulingConfidence::Low || low_confidence_ok
}

/// Overlay a kernel `CombatProfile` (Value-shaped subset) onto a base typed
/// `RulesetCombatProfile`. Only the mirrored strategy sub-blocks are overlaid;
/// any block the kernel leaves at its serde default is skipped so the base
/// (the engine's neutral generic profile) shows through. `reaction_windows` and
/// `search_recipes` are per-element deserialized from their serialized shapes.
/// None kernel profile → the base unchanged (== GENERIC behavior).
pub fn overlay_combat_profile(
    mut base: RulesetCombatProfile,
    kp: Option<&CombatProfile>,
) -> RulesetCombatProfile {
    let Some(kp) = kp else { return base };
    if !kp.profile_id.is_empty() {
        base.profile_id = kp.profile_id.clone();
    }
    if !kp.default_mode.is_empty() {
        base.default_mode = kp.default_mode.clone();
    }
    if !kp.applies_to_modes.is_empty() {
        base.applies_to_modes = kp.applies_to_modes.clone();
    }
    if !kp.action_economy.is_null() {
        base.action_economy = kp.action_economy.clone();
    }
    if !kp.initiative.is_null() {
        base.initiative = kp.initiative.clone();
    }
    if !kp.frame_exit_policy.is_null() {
        base.frame_exit_policy = kp.frame_exit_policy.clone();
    }
    if !kp.stalemate_policy.is_null() {
        base.stalemate_policy = kp.stalemate_policy.clone();
    }
    if !kp.npc_drive_policy.is_null() {
        base.npc_drive_policy = kp.npc_drive_policy.clone();
    }
    if !kp.reaction_windows.is_empty() {
        base.reaction_windows = kp
            .reaction_windows
            .iter()
            .filter_map(|v| serde_json::from_value::<ReactionAdvice>(v.clone()).ok())
            .collect();
    }
    if !kp.search_recipes.is_empty() {
        base.search_recipes = kp.search_recipes.clone();
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{default_generic_profile, RulesetCombatProfile};
    use serde_json::json;
    use trpg_model::{
        CombatModeRule, EscalationLevel, FrameRelation, NpcActorBinding, RulingConfidence,
        TechOption,
    };

    // ---- intent construction helper (ConflictIntent has no Default) ----
    fn base_intent() -> ConflictIntent {
        ConflictIntent {
            intent_id: "t".into(),
            language: None,
            relation_to_active_frame: FrameRelation::InsideFrameAction,
            action_kind: SituationActionKind::Attack,
            escalation_level: EscalationLevel::Medium,
            target_refs: vec![],
            desired_outcome: None,
            confidence: RulingConfidence::High,
            evidence_terms: vec![],
            classifier: "test".into(),
        }
    }
    fn intent_with(kind: SituationActionKind, evidence: &[&str]) -> ConflictIntent {
        ConflictIntent {
            action_kind: kind,
            evidence_terms: evidence.iter().map(|s| s.to_string()).collect(),
            ..base_intent()
        }
    }

    // -------------------------------------------------------------------
    // 2B: combat mode policy equivalence (vs legacy infer_combat_mode_from_intent)
    // -------------------------------------------------------------------
    fn cyberpunk_mode_policy() -> CombatModePolicy {
        CombatModePolicy {
            rules: vec![CombatModeRule {
                mode: CombatMode::Netrun,
                when_action_kinds: vec!["hack".into(), "disable_device".into()],
                when_evidence_contains: vec![],
            }],
            fallback_mode: CombatMode::Firefight,
        }
    }

    #[test]
    fn combat_mode_uses_kernel_policy_then_generic() {
        let cp = cyberpunk_mode_policy();
        // cyberpunk: hack -> netrun, attack -> firefight (== legacy)
        assert_eq!(
            combat_mode_from_policy(Some(&cp), &intent_with(SituationActionKind::Hack, &[])),
            CombatMode::Netrun
        );
        assert_eq!(
            combat_mode_from_policy(
                Some(&cp),
                &intent_with(SituationActionKind::DisableDevice, &[])
            ),
            CombatMode::Netrun
        );
        assert_eq!(
            combat_mode_from_policy(Some(&cp), &intent_with(SituationActionKind::Attack, &[])),
            CombatMode::Firefight
        );
        // absent policy -> GENERIC default (theater_of_mind), no ruleset branch
        assert_eq!(
            combat_mode_from_policy(None, &intent_with(SituationActionKind::Attack, &[])),
            CombatMode::TheaterOfMind
        );
    }

    #[test]
    fn combat_mode_coc_dnd_triangle_match_legacy_defaults() {
        let coc = CombatModePolicy {
            rules: vec![],
            fallback_mode: CombatMode::HorrorEncounter,
        };
        assert_eq!(
            combat_mode_from_policy(Some(&coc), &intent_with(SituationActionKind::Attack, &[])),
            CombatMode::HorrorEncounter
        );
        let dnd = CombatModePolicy {
            rules: vec![],
            fallback_mode: CombatMode::TacticalCombat,
        };
        assert_eq!(
            combat_mode_from_policy(Some(&dnd), &intent_with(SituationActionKind::Attack, &[])),
            CombatMode::TacticalCombat
        );
        // triangle: always anomaly_encounter (legacy: contains("triangle") => anomaly)
        let tri = CombatModePolicy {
            rules: vec![CombatModeRule {
                mode: CombatMode::AnomalyEncounter,
                when_action_kinds: vec!["disable_device".into()],
                when_evidence_contains: vec!["anomaly".into()],
            }],
            fallback_mode: CombatMode::AnomalyEncounter,
        };
        assert_eq!(
            combat_mode_from_policy(Some(&tri), &intent_with(SituationActionKind::Attack, &[])),
            CombatMode::AnomalyEncounter
        );
        assert_eq!(
            combat_mode_from_policy(
                Some(&tri),
                &intent_with(SituationActionKind::DisableDevice, &["anomaly:0.6"])
            ),
            CombatMode::AnomalyEncounter
        );
    }

    // 2C: check label + bare dice
    #[test]
    fn check_label_from_kernel_then_generic_fallback() {
        let pol = CheckLabelPolicy {
            labels: [(
                "hack".to_string(),
                "appropriate TECH / Interface / Basic Tech check".to_string(),
            )]
            .into_iter()
            .collect(),
        };
        assert_eq!(
            check_label_for(Some(&pol), SituationActionKind::Hack),
            "appropriate TECH / Interface / Basic Tech check"
        );
        // no policy entry -> neutral generic technical label (NOT a ruleset branch)
        assert_eq!(
            check_label_for(None, SituationActionKind::Hack),
            "appropriate technical conflict check"
        );
        assert_eq!(
            check_label_for(None, SituationActionKind::Attack),
            "appropriate attack/conflict check"
        );
        assert_eq!(
            check_label_for(None, SituationActionKind::Defend),
            "appropriate defense/evasion check"
        );
        assert_eq!(
            check_label_for(None, SituationActionKind::Intimidate),
            "appropriate intimidation/social pressure check"
        );
    }

    #[test]
    fn bare_dice_qualified_from_policy() {
        let q = DiceQualification {
            bare_dice_template: "{dice}+0".into(),
        };
        assert_eq!(
            qualify_bare_dice(Some(&q), "1d10"),
            Some("1d10+0".to_string())
        );
        assert_eq!(
            qualify_bare_dice(Some(&q), " 1d10 "),
            Some("1d10+0".to_string())
        );
        // empty template -> no qualification
        let empty = DiceQualification {
            bare_dice_template: "".into(),
        };
        assert_eq!(qualify_bare_dice(Some(&empty), "1d10"), None);
        // None -> GENERIC ("{dice}+0") still qualifies (caller gates by is_some())
        assert_eq!(qualify_bare_dice(None, "1d10"), Some("1d10+0".to_string()));
    }

    // 2D: module npc binding + tech DV (== legacy scav_boss/athena_drone/14/12)
    fn homecoming_cfg() -> ModuleConfig {
        ModuleConfig {
            npc_actor_bindings: vec![
                NpcActorBinding {
                    matcher: vec![
                        "scav_boss".into(),
                        "boss".into(),
                        "shotgun".into(),
                        "霰弹".into(),
                        "头目".into(),
                        "首领".into(),
                        "scav 老大".into(),
                        "scav leader".into(),
                    ],
                    actor_id: "npc.scav_boss".into(),
                    display_name: Some("shotgun boss".into()),
                },
                NpcActorBinding {
                    matcher: vec![
                        "drone".into(),
                        "无人机".into(),
                        "athena".into(),
                        "雅典娜".into(),
                    ],
                    actor_id: "npc.athena_drone".into(),
                    display_name: Some("rogue drone".into()),
                },
            ],
            technical_option_table: Some(vec![
                TechOption {
                    matcher: vec![
                        "basic tech".into(),
                        "cut off".into(),
                        "power".into(),
                        "cable".into(),
                        "线缆".into(),
                        "切断".into(),
                        "供电".into(),
                    ],
                    dv: 14,
                },
                TechOption {
                    matcher: vec![
                        "hack".into(),
                        "interface".into(),
                        "net".into(),
                        "server".into(),
                        "athena".into(),
                        "黑入".into(),
                        "服务器".into(),
                        "无人机".into(),
                    ],
                    dv: 12,
                },
            ]),
            scene_entity_aliases: vec![],
            module_search_profile: None,
            director: None,
        }
    }

    #[test]
    fn npc_binding_from_module_config_then_generic() {
        let cfg = homecoming_cfg();
        let atk = ConflictIntent {
            action_kind: SituationActionKind::Attack,
            ..base_intent()
        };
        assert_eq!(
            actor_for_combat_input(Some(&cfg), "shoot the shotgun boss", &atk).map(|a| a.actor_id),
            Some("npc.scav_boss".into())
        );
        assert_eq!(
            actor_for_combat_input(Some(&cfg), "攻击无人机", &atk).map(|a| a.actor_id),
            Some("npc.athena_drone".into())
        );
        // unmatched -> generic opposition (NOT a hardcoded module npc)
        assert_eq!(
            actor_for_combat_input(Some(&cfg), "attack the thing", &atk).map(|a| a.actor_id),
            Some("npc.opposition".into())
        );
        // no module config -> generic opposition fallback
        assert_eq!(
            actor_for_combat_input(None, "shoot the boss", &atk).map(|a| a.actor_id),
            Some("npc.opposition".into())
        );
        // non-attack intent -> None (unchanged)
        let inv = ConflictIntent {
            action_kind: SituationActionKind::InvestigateDuringConflict,
            ..base_intent()
        };
        assert!(actor_for_combat_input(Some(&cfg), "look around", &inv).is_none());
    }

    // 2F: NPC-holder identity gate (combat half). The combat opposition slot keeps
    // the non-persistent `npc.opposition` placeholder, but the frame must carry an
    // EXPLICIT binding to the resolved module-graph id when one exists, and stay
    // explicitly non-persistent (no fabricated id) when none does.
    fn npc(actor_id: &str) -> ActorRef {
        ActorRef {
            actor_id: actor_id.into(),
            actor_kind: ActorKind::Npc,
            display_name: None,
        }
    }

    #[test]
    fn persistent_opposition_graph_id_recovers_real_id_rejects_placeholder() {
        // a real module-graph binding id is persistent
        assert_eq!(
            persistent_opposition_graph_id(Some(&npc("npc.scav_boss"))),
            Some("npc.scav_boss".to_string())
        );
        // the single-slot placeholder is NEVER a persistent holder id
        assert_eq!(
            persistent_opposition_graph_id(Some(&npc("npc.opposition"))),
            None
        );
        assert_eq!(
            persistent_opposition_graph_id(Some(&npc("  npc.opposition  "))),
            None
        );
        // empty / None never fabricate an id
        assert_eq!(persistent_opposition_graph_id(Some(&npc(""))), None);
        assert_eq!(persistent_opposition_graph_id(None), None);
        // a non-NPC actor is not an opposition holder
        let pc = ActorRef {
            actor_id: "pc.current".into(),
            actor_kind: ActorKind::PlayerCharacter,
            display_name: None,
        };
        assert_eq!(persistent_opposition_graph_id(Some(&pc)), None);
    }

    #[test]
    fn opposition_identity_binding_marks_persistence_explicitly() {
        // resolved real id -> persistent binding carrying the graph id
        let bound = opposition_identity_binding(Some(&npc("npc.scav_boss")));
        let b = &bound["npc_identity_binding"];
        assert_eq!(b["placeholder_actor_id"], "npc.opposition");
        assert_eq!(b["persistent_graph_id"], "npc.scav_boss");
        assert_eq!(b["persistent"], true);

        // generic fallback (placeholder) -> explicitly non-persistent, no fabricated id
        let fallback = opposition_identity_binding(Some(&npc("npc.opposition")));
        let f = &fallback["npc_identity_binding"];
        assert_eq!(f["placeholder_actor_id"], "npc.opposition");
        assert!(f["persistent_graph_id"].is_null());
        assert_eq!(f["persistent"], false);

        // no resolved actor at all -> also explicitly non-persistent
        let none = opposition_identity_binding(None);
        assert_eq!(none["npc_identity_binding"]["persistent"], false);
        assert!(none["npc_identity_binding"]["persistent_graph_id"].is_null());
    }

    #[test]
    fn tech_dv_from_module_config() {
        let cfg = homecoming_cfg();
        assert_eq!(
            tech_dv_from_config(Some(&cfg), "cut off the cable"),
            Some(14)
        );
        assert_eq!(tech_dv_from_config(Some(&cfg), "hack the server"), Some(12));
        assert_eq!(tech_dv_from_config(Some(&cfg), "open the door"), None);
        assert_eq!(tech_dv_from_config(None, "cut off the cable"), None); // no module -> None
    }

    // 2E: triangle threshold flags
    #[test]
    fn triangle_thresholds_are_profile_driven() {
        // investigate opens a frame only when the profile opts in (was contains("triangle"))
        assert!(investigate_opens_frame_relation(true, 0.45));
        assert!(!investigate_opens_frame_relation(false, 0.45));
        assert!(!investigate_opens_frame_relation(true, 0.30)); // below 0.42
                                                                // low-confidence frame start gated by profile flag (was contains("triangle"))
        assert!(low_confidence_frame_start_ok(true, RulingConfidence::Low));
        assert!(!low_confidence_frame_start_ok(false, RulingConfidence::Low));
        // non-low confidence always passes regardless of flag
        assert!(low_confidence_frame_start_ok(
            false,
            RulingConfidence::Medium
        ));
        assert!(low_confidence_frame_start_ok(false, RulingConfidence::High));
    }

    // -------------------------------------------------------------------
    // 2A: profile overlay equivalence vs legacy Rust (mirrored strategy blocks).
    // The legacy *_profile() bodies are kept here ONLY as the equivalence
    // baseline (#[cfg(test)]); the engine no longer hardcodes them.
    // -------------------------------------------------------------------
    fn legacy_cyberpunk() -> RulesetCombatProfile {
        RulesetCombatProfile {
            profile_id: "cyberpunk_red.firefight.v1_3".into(),
            ruleset_id: "cyberpunk_red".into(),
            applies_to_modes: vec!["firefight".into(), "netrun".into(), "chase".into()],
            default_mode: "firefight".into(),
            action_economy: json!({"turn_slots":[{"slot_id":"move_action","count":1},{"slot_id":"action","count":1}],"notes":"Combat Time uses 1 Move Action + 1 Action; exact rules should be loaded as packets."}),
            initiative: json!({"kind":"formula","expression":"REF + 1d10"}),
            reaction_windows: vec![crate::generic_required_defense()],
            frame_exit_policy: json!({"allow_disengage":true,"allow_deescalation":true,"objective_driven":true,"stalemate_after_non_decisive_turns":3}),
            npc_drive_policy: json!({"mook_morale":45,"self_interest":70,"break_actions":["flee","surrender","call_backup","negotiate"]}),
            search_recipes: vec![
                json!({"query":"Friday Night Firefight Actions Ranged Combat Melee Combat Before You Take Damage Mooks and Grunts Encounters"}),
            ],
            ..Default::default()
        }
    }

    /// Build the Value-shaped kernel CombatProfile that, overlaid on the generic
    /// base, must reproduce legacy_cyberpunk()'s mirrored strategy blocks.
    fn cyberpunk_kernel_profile() -> CombatProfile {
        CombatProfile {
            profile_id: "cyberpunk_red.firefight.v1_3".into(),
            default_mode: "firefight".into(),
            applies_to_modes: vec!["firefight".into(), "netrun".into(), "chase".into()],
            action_economy: json!({"turn_slots":[{"slot_id":"move_action","count":1},{"slot_id":"action","count":1}],"notes":"Combat Time uses 1 Move Action + 1 Action; exact rules should be loaded as packets."}),
            initiative: json!({"kind":"formula","expression":"REF + 1d10"}),
            reaction_windows: vec![serde_json::to_value(crate::generic_required_defense()).unwrap()],
            frame_exit_policy: json!({"allow_disengage":true,"allow_deescalation":true,"objective_driven":true,"stalemate_after_non_decisive_turns":3}),
            stalemate_policy: serde_json::Value::Null,
            npc_drive_policy: json!({"mook_morale":45,"self_interest":70,"break_actions":["flee","surrender","call_backup","negotiate"]}),
            search_recipes: vec![
                json!({"query":"Friday Night Firefight Actions Ranged Combat Melee Combat Before You Take Damage Mooks and Grunts Encounters"}),
            ],
        }
    }

    #[test]
    fn overlay_combat_profile_reproduces_legacy_mirrored_blocks() {
        let legacy = legacy_cyberpunk();
        let overlaid =
            overlay_combat_profile(default_generic_profile(), Some(&cyberpunk_kernel_profile()));
        // Mirrored strategy blocks must byte-match the legacy Rust values.
        assert_eq!(overlaid.profile_id, legacy.profile_id, "profile_id");
        assert_eq!(overlaid.default_mode, legacy.default_mode, "default_mode");
        assert_eq!(
            overlaid.applies_to_modes, legacy.applies_to_modes,
            "applies_to_modes"
        );
        assert_eq!(
            overlaid.action_economy, legacy.action_economy,
            "action_economy"
        );
        assert_eq!(overlaid.initiative, legacy.initiative, "initiative");
        assert_eq!(
            overlaid.frame_exit_policy, legacy.frame_exit_policy,
            "frame_exit_policy"
        );
        assert_eq!(
            overlaid.npc_drive_policy, legacy.npc_drive_policy,
            "npc_drive_policy"
        );
        assert_eq!(
            overlaid.search_recipes, legacy.search_recipes,
            "search_recipes"
        );
        // reaction_windows round-trip (serialized ReactionAdvice -> typed)
        let lhs = serde_json::to_value(&overlaid.reaction_windows).unwrap();
        let rhs = serde_json::to_value(&legacy.reaction_windows).unwrap();
        assert_eq!(lhs, rhs, "reaction_windows");
    }

    #[test]
    fn overlay_none_returns_base_unchanged() {
        let base = default_generic_profile();
        let overlaid = overlay_combat_profile(base.clone(), None);
        assert_eq!(overlaid.ruleset_id, base.ruleset_id);
        assert_eq!(overlaid.profile_id, base.profile_id);
        assert_eq!(overlaid.default_mode, base.default_mode);
    }
}

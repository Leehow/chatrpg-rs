//! L4.1 — typed [`ScenePlan`] + pure derivation, flag-gated.
//!
//! Replaces the hardcoded `SceneFramePurpose` vectors (the constant `progress_signals` /
//! `exit_conditions` / `fail_forward_options` / `pacing_budget_turns: Some(4)` literals at
//! `trpg-director/src/lib.rs:424`) with fields DERIVED from the live [`StoryThread`]s +
//! [`DirectorModuleConfig`] + scene context. Pure, proposal-only, DB-free, deterministic, and
//! GENERIC — classification is a function of typed thread/config data, NEVER branched on
//! `ruleset_id`/`module_id` (§二-⑪). The emitted strings are short STRUCTURAL tokens (not secret
//! prose), consistent with the §codex packet-safety discipline.
//!
//! Scope split: this lane lands the type, the pure [`derive_scene_plan`], the
//! [`scene_plan_enabled`] flag reader, and the [`ScenePlan::to_scene_frame_purpose`] conversion,
//! all proven to vary with thread input. The SceneChanged-trigger wiring + `ScenePlanCreated`
//! emit (consuming the L3.2 event) land in L4.2; nothing here is wired into the hot path yet, so
//! OFF is byte-identical (no caller).

use trpg_model::{
    DirectorModuleConfig, SceneFramePurpose, ScenePurpose, StoryThread, StoryThreadStatus,
};

/// Flag gate for the derived scene-plan path. Default OFF ⇒ the legacy hardcoded
/// `SceneFramePurpose` is used (byte-identical baseline); ON (wired in L4.2) ⇒ the derived plan
/// drives scene framing. Mirrors the `env_bool` semantics of the other director flags.
pub fn scene_plan_enabled() -> bool {
    std::env::var("TRPG_DIRECTOR_SCENE_PLAN")
        .ok()
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// A typed, DERIVED scene-framing proposal. Every field is a deterministic function of the scene's
/// live threads (+ optional module config) — NOT a constant. Proposal-only: the Kernel decides
/// whether/how to adopt it; this commits nothing.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScenePlan {
    pub scene_id: String,
    /// The scene's dominant dramatic purpose, derived from the spotlight threads' statuses.
    pub scene_purpose: ScenePurpose,
    /// The non-terminal threads this scene serves (ordered as in `threads`).
    pub spotlight_thread_ids: Vec<String>,
    /// Structural "what counts as progress" tokens, one per spotlight thread (+ pressure signal).
    pub progress_signals: Vec<String>,
    /// Structural "when does the scene end" tokens.
    pub exit_conditions: Vec<String>,
    /// Structural fail-forward levers, derived from the scene purpose.
    pub fail_forward_options: Vec<String>,
    /// Turn budget, tightened by thread load + urgency (never a constant 4).
    pub pacing_budget_turns: u32,
    /// L4.3 — facts the scene wants kept hidden PROACTIVELY: the `related_fact_ids` of live threads
    /// still BUILDING toward payoff (status not yet `ReadyForPayoff`). These are the scene's
    /// secrets-in-waiting — the Narrator must not reveal them as new until the owning thread ripens.
    /// Plumbed into `NarrationPacket.forbidden_reveals` (L4.3); the reactive gate stays the backstop.
    pub forbidden_fact_ids: Vec<String>,
}

/// True when a thread is still "live" — eligible to anchor a scene. Terminal/dormant threads do
/// not drive scene purpose.
fn is_live(status: StoryThreadStatus) -> bool {
    !matches!(
        status,
        StoryThreadStatus::Resolved
            | StoryThreadStatus::Abandoned
            | StoryThreadStatus::Transformed
            | StoryThreadStatus::Dormant
    )
}

/// Derive the scene purpose from the live threads + config (priority order, generic):
/// a ripe thread wants resolution; an escalating one wants pressure; reveal material (thread facts
/// or module scene facts) wants information; an active thread wants a choice; nothing live ⇒ tone.
fn derive_purpose(live: &[&StoryThread], config: Option<&DirectorModuleConfig>) -> ScenePurpose {
    if live
        .iter()
        .any(|t| t.status == StoryThreadStatus::ReadyForPayoff)
    {
        ScenePurpose::ResolveConflict
    } else if live
        .iter()
        .any(|t| t.status == StoryThreadStatus::Escalating)
    {
        ScenePurpose::RaisePressure
    } else if live.iter().any(|t| !t.related_fact_ids.is_empty())
        || config.map(|c| !c.scene_facts.is_empty()).unwrap_or(false)
    {
        ScenePurpose::RevealInformation
    } else if live.iter().any(|t| t.status == StoryThreadStatus::Active) {
        ScenePurpose::ForceChoice
    } else {
        ScenePurpose::EstablishTone
    }
}

/// Generic fail-forward levers for a purpose. Structural tokens (no prose); deterministic.
fn fail_forward_for(purpose: ScenePurpose) -> Vec<String> {
    let head = match purpose {
        ScenePurpose::RevealInformation => "clue_with_cost",
        ScenePurpose::ResolveConflict => "partial_resolution",
        ScenePurpose::RaisePressure => "advance_clock",
        ScenePurpose::ForceChoice => "success_with_cost",
        _ => "reframe_situation",
    };
    vec![
        head.to_string(),
        "success_with_cost".to_string(),
        "fail_reveals_risk".to_string(),
    ]
}

/// L4.1 — PURE derivation of a [`ScenePlan`] from a scene id + its live threads + optional module
/// config. Deterministic and generic (no name-branch). Fields VARY with the thread input: the
/// spotlight set, the progress signals, the purpose, and the pacing budget all move as threads do.
pub fn derive_scene_plan(
    scene_id: &str,
    threads: &[StoryThread],
    config: Option<&DirectorModuleConfig>,
) -> ScenePlan {
    let live: Vec<&StoryThread> = threads.iter().filter(|t| is_live(t.status)).collect();
    let spotlight_thread_ids: Vec<String> = live.iter().map(|t| t.thread_id.clone()).collect();
    let scene_purpose = derive_purpose(&live, config);

    // Progress signals: one structural "advance:{id}" per spotlight thread, plus a pressure signal
    // when the module carries pressure items. Order-stable, varies with the thread set.
    let mut progress_signals: Vec<String> = live
        .iter()
        .map(|t| format!("advance:{}", t.thread_id))
        .collect();
    if config
        .map(|c| !c.pressure_items.is_empty())
        .unwrap_or(false)
    {
        progress_signals.push("pressure_shift".to_string());
    }
    if progress_signals.is_empty() {
        progress_signals.push("player_states_intent".to_string());
    }

    // Exit conditions: resolve the primary (first live) thread + generic departure/clock exits.
    let mut exit_conditions = Vec::new();
    if let Some(primary) = live.first() {
        exit_conditions.push(format!("resolve:{}", primary.thread_id));
    }
    exit_conditions.push("player_departs".to_string());
    exit_conditions.push("clock_triggers".to_string());

    // Pacing budget: base 6, tightened by thread load (more live threads ⇒ less time each) and by
    // high urgency (a pressing scene runs hot). Clamped to a sane 2..=6 — never the old constant 4.
    let load = spotlight_thread_ids.len().min(4) as u32;
    let max_urgency = live.iter().map(|t| t.urgency).fold(0.0_f32, f32::max);
    let mut budget = 6u32.saturating_sub(load);
    if max_urgency >= 0.7 {
        budget = budget.saturating_sub(1);
    }
    let pacing_budget_turns = budget.clamp(2, 6);

    // L4.3 proactive forbidden set: a live thread still BUILDING toward its payoff (any live status
    // except ReadyForPayoff) keeps its facts hidden — they are the scene's secrets-in-waiting. A
    // ripe (ReadyForPayoff) thread's facts are NOT forbidden (the scene is here to pay them off).
    // Order-stable, de-duplicated; empty when no building thread has facts (fail-closed no-op).
    let mut forbidden_fact_ids: Vec<String> = Vec::new();
    for t in live
        .iter()
        .filter(|t| t.status != StoryThreadStatus::ReadyForPayoff)
    {
        for f in &t.related_fact_ids {
            if !forbidden_fact_ids.contains(f) {
                forbidden_fact_ids.push(f.clone());
            }
        }
    }

    ScenePlan {
        scene_id: scene_id.to_string(),
        scene_purpose,
        spotlight_thread_ids,
        progress_signals,
        exit_conditions,
        fail_forward_options: fail_forward_for(scene_purpose),
        pacing_budget_turns,
        forbidden_fact_ids,
    }
}

impl ScenePlan {
    /// Project this derived plan into the existing [`SceneFramePurpose`] carrier — the shape the
    /// `ActionableSituationBrief` already speaks (L4.2 swaps the hardcoded literal for this).
    pub fn to_scene_frame_purpose(&self) -> SceneFramePurpose {
        SceneFramePurpose {
            scene_id: Some(self.scene_id.clone()),
            scene_purpose: self.scene_purpose,
            enter_condition: None,
            progress_signals: self.progress_signals.clone(),
            exit_conditions: self.exit_conditions.clone(),
            fail_forward_options: self.fail_forward_options.clone(),
            pacing_budget_turns: Some(self.pacing_budget_turns),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread(id: &str, status: StoryThreadStatus, urgency: f32, facts: &[&str]) -> StoryThread {
        StoryThread {
            thread_id: id.into(),
            status,
            urgency,
            related_fact_ids: facts.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    // The acceptance: fields VARY with the thread input — two different thread sets yield two
    // different plans (spotlight set, signals, purpose, and pacing all move).
    #[test]
    fn fields_vary_with_thread_input() {
        let a = derive_scene_plan(
            "scene_1",
            &[thread("t_a", StoryThreadStatus::Active, 0.2, &["f1"])],
            None,
        );
        let b = derive_scene_plan(
            "scene_1",
            &[
                thread("t_b", StoryThreadStatus::Active, 0.9, &[]),
                thread("t_c", StoryThreadStatus::Active, 0.9, &[]),
            ],
            None,
        );
        assert_ne!(a, b, "different threads ⇒ different plan");
        assert_ne!(a.spotlight_thread_ids, b.spotlight_thread_ids);
        assert_ne!(a.progress_signals, b.progress_signals);
        // a has a thread with a fact ⇒ RevealInformation; b has none ⇒ ForceChoice.
        assert_eq!(a.scene_purpose, ScenePurpose::RevealInformation);
        assert_eq!(b.scene_purpose, ScenePurpose::ForceChoice);
    }

    // Purpose priority: a ripe thread ⇒ ResolveConflict; escalating ⇒ RaisePressure.
    #[test]
    fn purpose_priority_ripe_then_escalating() {
        let ripe = derive_scene_plan(
            "s",
            &[
                thread("t1", StoryThreadStatus::ReadyForPayoff, 0.5, &["f"]),
                thread("t2", StoryThreadStatus::Escalating, 0.5, &[]),
            ],
            None,
        );
        assert_eq!(ripe.scene_purpose, ScenePurpose::ResolveConflict);
        let esc = derive_scene_plan(
            "s",
            &[thread("t2", StoryThreadStatus::Escalating, 0.5, &[])],
            None,
        );
        assert_eq!(esc.scene_purpose, ScenePurpose::RaisePressure);
    }

    // Terminal/dormant threads do not anchor a scene; no live thread ⇒ EstablishTone + a generic
    // progress signal + pacing falls back to the loose end.
    #[test]
    fn no_live_threads_yields_tone_and_loose_pacing() {
        let plan = derive_scene_plan(
            "s",
            &[
                thread("t_done", StoryThreadStatus::Resolved, 0.9, &["f"]),
                thread("t_dorm", StoryThreadStatus::Dormant, 0.9, &[]),
            ],
            None,
        );
        assert!(plan.spotlight_thread_ids.is_empty());
        assert_eq!(plan.scene_purpose, ScenePurpose::EstablishTone);
        assert_eq!(
            plan.progress_signals,
            vec!["player_states_intent".to_string()]
        );
        assert_eq!(
            plan.pacing_budget_turns, 6,
            "no load, no urgency ⇒ loose budget"
        );
    }

    // Pacing tightens with load + urgency, and is never the old constant 4 by accident.
    #[test]
    fn pacing_tightens_with_load_and_urgency() {
        let loose = derive_scene_plan(
            "s",
            &[thread("t1", StoryThreadStatus::Active, 0.1, &[])],
            None,
        );
        assert_eq!(loose.pacing_budget_turns, 5); // 6 - 1 load, low urgency
        let hot = derive_scene_plan(
            "s",
            &[
                thread("t1", StoryThreadStatus::Active, 0.9, &[]),
                thread("t2", StoryThreadStatus::Active, 0.9, &[]),
                thread("t3", StoryThreadStatus::Active, 0.9, &[]),
            ],
            None,
        );
        assert_eq!(hot.pacing_budget_turns, 2); // 6 - 3 load - 1 urgency = 2
    }

    // The conversion carries every derived field into the existing SceneFramePurpose carrier.
    #[test]
    fn to_scene_frame_purpose_carries_derived_fields() {
        let plan = derive_scene_plan(
            "scene_market",
            &[thread("t1", StoryThreadStatus::Active, 0.3, &["f"])],
            None,
        );
        let sfp = plan.to_scene_frame_purpose();
        assert_eq!(sfp.scene_id.as_deref(), Some("scene_market"));
        assert_eq!(sfp.scene_purpose, plan.scene_purpose);
        assert_eq!(sfp.progress_signals, plan.progress_signals);
        assert_eq!(sfp.exit_conditions, plan.exit_conditions);
        assert_eq!(sfp.fail_forward_options, plan.fail_forward_options);
        assert_eq!(sfp.pacing_budget_turns, Some(plan.pacing_budget_turns));
    }

    // L4.3 — a BUILDING thread's facts are proactively forbidden; a RIPE thread's facts are not.
    #[test]
    fn forbidden_facts_from_building_threads_only() {
        let plan = derive_scene_plan(
            "s",
            &[
                thread(
                    "t_build",
                    StoryThreadStatus::Active,
                    0.3,
                    &["secret_a", "secret_b"],
                ),
                thread(
                    "t_ripe",
                    StoryThreadStatus::ReadyForPayoff,
                    0.3,
                    &["payoff_c"],
                ),
                thread("t_dorm", StoryThreadStatus::Dormant, 0.9, &["dormant_d"]),
            ],
            None,
        );
        // Building thread's facts are forbidden; ripe thread's payoff fact is NOT; dormant (not
        // live) contributes nothing.
        assert_eq!(
            plan.forbidden_fact_ids,
            vec!["secret_a".to_string(), "secret_b".to_string()]
        );
        assert!(!plan.forbidden_fact_ids.contains(&"payoff_c".to_string()));
        assert!(!plan.forbidden_fact_ids.contains(&"dormant_d".to_string()));
    }

    // No building thread with facts ⇒ empty forbidden set (fail-closed no-op).
    #[test]
    fn no_building_facts_yields_empty_forbidden() {
        let plan = derive_scene_plan(
            "s",
            &[thread(
                "t_ripe",
                StoryThreadStatus::ReadyForPayoff,
                0.3,
                &["payoff"],
            )],
            None,
        );
        assert!(plan.forbidden_fact_ids.is_empty());
    }

    // Module config contributes derived signals (scene facts ⇒ reveal; pressure ⇒ extra signal).
    #[test]
    fn config_contributes_derived_signals() {
        let config = DirectorModuleConfig {
            scene_facts: vec![Default::default()],
            pressure_items: vec![Default::default()],
            ..Default::default()
        };
        let plan = derive_scene_plan(
            "s",
            &[thread("t1", StoryThreadStatus::Active, 0.2, &[])],
            Some(&config),
        );
        // scene_facts present ⇒ RevealInformation even though the thread has no facts.
        assert_eq!(plan.scene_purpose, ScenePurpose::RevealInformation);
        assert!(plan
            .progress_signals
            .contains(&"pressure_shift".to_string()));
    }
}

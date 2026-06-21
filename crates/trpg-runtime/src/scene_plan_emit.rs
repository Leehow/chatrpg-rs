//! L4.2 — scene-start [`ScenePlan`] trigger + `ScenePlanCreated` emit.
//!
//! The L4.1 lane landed the pure [`trpg_director::derive_scene_plan`] derivation but wired it into
//! nothing (OFF byte-identical, no caller). This lane closes that gap: on a COMMITTED scene change
//! (the SceneNavigate phase's [`WorldEventKind::SceneChanged`] write, `tiered.rs`), derive the
//! scene's plan from the session's live story threads (+ the module's director config) and emit
//! exactly one [`DomainEventKind::ScenePlanCreated`] ledger event (the L3.2 constructor).
//!
//! ## Discipline (§0)
//! - **Flag-gated.** Gated by [`trpg_director::scene_plan_enabled`] (`TRPG_DIRECTOR_SCENE_PLAN`,
//!   default OFF). OFF ⇒ [`emit_scene_plan_on_change`] returns IMMEDIATELY with zero DB reads/
//!   writes ⇒ the scene transition is byte-identical to the P0 baseline (no new event row).
//! - **Additive + fail-soft.** The event is append-only observability; a load/append failure only
//!   warns and NEVER reverses the already-committed scene switch (mirrors the `ClockAdvanced` and
//!   `commit_story_writes` write-through discipline). The snapshot stays the source of truth (R2
//!   sovereign-event-log promotion is OutOfScope).
//! - **Exactly one.** The event id is keyed on the scene (`de_scene_{session}_{scene}_created`,
//!   `scene_plan_created_event`) and `append_domain_event` is `on conflict do nothing`, so a replay
//!   of the same scene change folds to ONE row.
//! - **Generic.** The derivation is a pure function of typed thread/config data — never branched on
//!   `ruleset_id`/`module_id` (§二-⑪). The plan is proposal material, not a script.

use trpg_db::Db;
use trpg_director::{derive_scene_plan, ScenePlan};
use trpg_model::{DomainEvent, StoryThread};

use crate::story_events::scene_plan_created_event;

/// PURE (DB-free) builder: derive the scene's [`ScenePlan`] from its live threads (+ optional
/// director config) and pair it with the additive `ScenePlanCreated` ledger event. Deterministic
/// and generic — no env read, no name-branch. Kept separate from the async wiring so the
/// derive→event mapping is unit-testable without a live Postgres.
pub fn build_scene_plan_emission(
    session_id: &str,
    turn_id: &str,
    scene_id: &str,
    threads: &[StoryThread],
    config: Option<&trpg_model::DirectorModuleConfig>,
) -> (ScenePlan, DomainEvent) {
    let plan = derive_scene_plan(scene_id, threads, config);
    let event = scene_plan_created_event(session_id, turn_id, scene_id);
    (plan, event)
}

/// L4.2 production seam: on a committed scene change, derive + emit the scene plan. Flag-gated by
/// `TRPG_DIRECTOR_SCENE_PLAN` (default OFF ⇒ returns `false` immediately, zero reads/writes,
/// byte-identical baseline). ON ⇒ loads the persisted [`StoryState`] (fail-soft ⇒ empty story) +
/// the module's [`trpg_model::ModuleConfig`] (fail-soft ⇒ generic, no director config), derives the
/// [`ScenePlan`] from the live threads, and appends exactly one `ScenePlanCreated` event.
///
/// Returns `true` iff a plan was derived and its event was appended successfully (for caller
/// observability/tests); a flag-OFF short-circuit or any fail-soft load/append failure ⇒ `false`.
/// Never returns an error — story-plan observability must never reverse a delivered scene switch.
pub async fn emit_scene_plan_on_change(
    db: &Db,
    session_id: &str,
    module_id: &str,
    turn_id: &str,
    scene_id: &str,
) -> bool {
    // Flag gate FIRST: OFF ⇒ no reads, no writes, byte-identical baseline.
    if !trpg_director::scene_plan_enabled() {
        return false;
    }
    // Fail-soft loads: an absent row OR a deserialize error ⇒ empty story; an absent module config
    // ⇒ generic (None) — the derivation is total and never panics.
    let story = db
        .load_story_state(session_id)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    let module_cfg = db.load_module_config(module_id).await;
    let director_cfg = module_cfg.as_ref().and_then(|m| m.director.as_ref());

    let (_plan, event) =
        build_scene_plan_emission(session_id, turn_id, scene_id, &story.active_threads, director_cfg);

    // Additive + fail-soft write-through: appending the ledger row must NEVER reverse the committed
    // scene switch, so a failure only warns.
    match db.append_domain_event(&event).await {
        Ok(()) => true,
        Err(err) => {
            tracing::warn!(error = %err, event_id = %event.event_id, "append ScenePlanCreated domain event failed (non-fatal)");
            false
        }
    }
}

/// L4.3 production seam: the PROACTIVE forbidden-reveal set for the session's CURRENT scene. Flag-
/// gated by `TRPG_DIRECTOR_SCENE_PLAN` (default OFF ⇒ returns an EMPTY vec immediately, zero reads,
/// byte-identical baseline). ON ⇒ loads the current scene id + persisted [`StoryState`] + module
/// director config (all fail-soft), derives the [`ScenePlan`], and returns its `forbidden_fact_ids`
/// (the facts of still-building threads). Composed into `NarrationPacket.forbidden_reveals` at the
/// narrator phase; the reactive narrowing/regen ladder stays the backstop. Never errors —
/// proactive guarding must never break a turn (a failure fail-closes to an EMPTY proactive set,
/// leaving the reactive gate fully in charge).
pub async fn scene_forbidden_reveals(
    db: &Db,
    session_id: &str,
    module_id: Option<&str>,
) -> Vec<String> {
    if !trpg_director::scene_plan_enabled() {
        return Vec::new();
    }
    let Some(scene_id) = db.load_session_scene(session_id).await.ok().flatten() else {
        return Vec::new();
    };
    if scene_id.is_empty() {
        return Vec::new();
    }
    let story = db
        .load_story_state(session_id)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    let module_cfg = match module_id {
        Some(mid) => db.load_module_config(mid).await,
        None => None,
    };
    let director_cfg = module_cfg.as_ref().and_then(|m| m.director.as_ref());
    derive_scene_plan(&scene_id, &story.active_threads, director_cfg).forbidden_fact_ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::{DomainEventKind, StoryThreadStatus};

    fn thread(id: &str, status: StoryThreadStatus) -> StoryThread {
        StoryThread {
            thread_id: id.into(),
            status,
            ..Default::default()
        }
    }

    // The pure builder pairs a DERIVED plan (varies with threads) with the scene-keyed event.
    #[test]
    fn builder_pairs_derived_plan_with_scene_event() {
        let threads = vec![
            thread("t_a", StoryThreadStatus::Active),
            thread("t_b", StoryThreadStatus::Escalating),
        ];
        let (plan, event) =
            build_scene_plan_emission("sess1", "turn3", "scene_market", &threads, None);

        // Plan is derived from the live threads (both are live ⇒ both spotlighted).
        assert_eq!(plan.scene_id, "scene_market");
        assert_eq!(
            plan.spotlight_thread_ids,
            vec!["t_a".to_string(), "t_b".to_string()]
        );

        // Event is the scene-keyed ScenePlanCreated (idempotent on scene, turn carried as data).
        assert_eq!(event.kind, DomainEventKind::ScenePlanCreated);
        assert_eq!(event.event_id, "de_scene_sess1_scene_market_created");
        assert_eq!(event.turn_id, "turn3");
        assert_eq!(event.data["scene_id"], "scene_market");
    }

    // No live threads ⇒ a still-valid plan (EstablishTone) + the same scene-keyed event: the seam
    // never panics on an empty story (fail-soft total derivation).
    #[test]
    fn builder_total_on_empty_threads() {
        let (plan, event) = build_scene_plan_emission("s", "t", "scene_empty", &[], None);
        assert!(plan.spotlight_thread_ids.is_empty());
        assert_eq!(event.event_id, "de_scene_s_scene_empty_created");
    }

    // The event id is keyed ONLY on (session, scene) — two different turns crossing into the SAME
    // scene fold to one idempotent row (the `on conflict do nothing` contract).
    #[test]
    fn event_key_is_scene_idempotent_across_turns() {
        let (_p1, e1) = build_scene_plan_emission("s", "turn1", "scene_x", &[], None);
        let (_p2, e2) = build_scene_plan_emission("s", "turn2", "scene_x", &[], None);
        assert_eq!(e1.event_id, e2.event_id, "same scene ⇒ same idempotent key");
    }
}

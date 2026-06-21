//! L3.2 — PURE (DB-free) promise + scene/beat ledger-event derivations.
//!
//! Sibling to [`crate::story_write`] (which owns the StoryThread status diff). This module owns
//! the additive ledger-event derivations for the OTHER story surfaces: [`StoryPromise`] maturity
//! and the scene/beat planning events. Same discipline as `ClockAdvanced` and
//! `story_write::thread_status_events`:
//!
//! - **Additive + fail-soft.** The persisted [`StoryState`] snapshot stays the single source of
//!   truth (R2 sovereign-event-log promotion is OutOfScope); these events only add append-only
//!   observability. The committer appends them fail-soft (a per-event failure only warns).
//! - **Flag-gated.** Emission lives strictly inside the `TRPG_STORY_WRITE_LOOP` ON path, so OFF is
//!   byte-identical to baseline (zero new events).
//! - **Generic.** No `ruleset_id`/`module_id` name-branching (§二-⑪) — classification is purely a
//!   function of the typed status/maturity transition.
//!
//! Emission sites by lane:
//! - [`promise_status_events`] — wired into `director_brief::commit_story_writes` NOW (alongside
//!   the thread diff); it fires once a promise actually advances. The production promise-advancement
//!   source is the Story Observer (L7.1); until then the diff is empty (fail-closed no-op).
//! - [`scene_plan_created_event`] — emit site is the SceneChanged trigger (L4.2).
//! - [`beat_planned_event`] / [`beat_observed_event`] — emit sites are the narrator compose path
//!   (L6.x) and the Story Observer (L7.x). This lane lands the vocabulary + pure constructors.

use trpg_model::{BeatKind, BeatRecord, DomainEvent, DomainEventKind, PromiseStatus, StoryState};

/// Stable DB token for a promise status — single source for the idempotent event key and the
/// event `data`. Local match (not serde) so a future `#[serde(rename)]` can never silently drift
/// the persisted key. Generic per-status mapping; NOT branched on ruleset/module (§二-⑪).
fn promise_status_token(status: PromiseStatus) -> &'static str {
    match status {
        PromiseStatus::Planted => "Planted",
        PromiseStatus::Developing => "Developing",
        PromiseStatus::Ripe => "Ripe",
        PromiseStatus::PaidOff => "PaidOff",
        PromiseStatus::Broken => "Broken",
    }
}

/// Classify one promise transition into its ledger event kind. `prior == None` ⇒ the promise is
/// newly planted this turn ⇒ `StoryPromiseCreated`. A move INTO `PaidOff` (from anything else) ⇒
/// `StoryPromisePaidOff`. Any other forward change — status advanced (but not to PaidOff) OR
/// maturity increased — ⇒ `StoryPromiseReinforced`. Otherwise `None` (no event).
fn classify_promise_transition(
    prior: Option<PromiseStatus>,
    next: PromiseStatus,
    maturity_increased: bool,
) -> Option<DomainEventKind> {
    match prior {
        None => Some(DomainEventKind::StoryPromiseCreated),
        Some(p) => {
            if next == PromiseStatus::PaidOff && p != PromiseStatus::PaidOff {
                Some(DomainEventKind::StoryPromisePaidOff)
            } else if next != PromiseStatus::PaidOff && (p != next || maturity_increased) {
                Some(DomainEventKind::StoryPromiseReinforced)
            } else {
                None // unchanged (or maturity flat at a settled status) emits nothing
            }
        }
    }
}

/// L3.2 — derive the additive StoryPromise ledger events from a committed promise diff (PURE,
/// DB-free). Compares each promise in `after` against its `before` counterpart (absent in `before`
/// ⇒ newly created) and emits ONE [`DomainEvent`] per transition.
///
/// Idempotent key keyed on the RESULTING state: `de_promise_{session}_{promise}_created` for the
/// first appearance, else `de_promise_{session}_{promise}_{status}` (the resulting status token) —
/// replaying the same transition folds to one row; a move to a new status lands a new row.
/// Order-stable (follows `after.promises` order).
pub fn promise_status_events(
    before: &StoryState,
    after: &StoryState,
    session_id: &str,
    turn_id: &str,
) -> Vec<DomainEvent> {
    let prior: std::collections::HashMap<&str, (PromiseStatus, f32)> = before
        .promises
        .iter()
        .map(|p| (p.promise_id.as_str(), (p.status, p.maturity)))
        .collect();
    let mut events = Vec::new();
    for p in &after.promises {
        let prior_entry = prior.get(p.promise_id.as_str()).copied();
        let maturity_increased = prior_entry.map(|(_, m)| p.maturity > m).unwrap_or(false);
        let Some(kind) =
            classify_promise_transition(prior_entry.map(|(s, _)| s), p.status, maturity_increased)
        else {
            continue;
        };
        let key_token = if kind == DomainEventKind::StoryPromiseCreated {
            "created"
        } else {
            promise_status_token(p.status)
        };
        events.push(DomainEvent::new(
            format!("de_promise_{}_{}_{}", session_id, p.promise_id, key_token),
            session_id.to_string(),
            turn_id.to_string(),
            kind,
            serde_json::json!({
                "promise_id": p.promise_id,
                "thread_id": p.thread_id,
                "new_status": promise_status_token(p.status),
                "maturity": p.maturity,
                "prior_status": prior_entry.map(|(s, _)| promise_status_token(s)),
            }),
        ));
    }
    events
}

/// Stable DB token for a beat kind — used in the event `data` and the beat-event key. Local match
/// (not serde) for key stability; generic, not ruleset-branched (§二-⑪).
fn beat_kind_token(kind: BeatKind) -> &'static str {
    match kind {
        BeatKind::Respond => "Respond",
        BeatKind::Reveal => "Reveal",
        BeatKind::Complicate => "Complicate",
        BeatKind::Consequence => "Consequence",
        BeatKind::Choice => "Choice",
        BeatKind::Reaction => "Reaction",
        BeatKind::Callback => "Callback",
        BeatKind::Foreshadow => "Foreshadow",
        BeatKind::Escalate => "Escalate",
        BeatKind::Relief => "Relief",
        BeatKind::Payoff => "Payoff",
        BeatKind::Transition => "Transition",
    }
}

/// L3.2 — construct a `ScenePlanCreated` ledger event for a scene that just had a plan built. The
/// emit site is the SceneChanged trigger (L4.2); this is the pure constructor. Idempotent key on
/// the scene: `de_scene_{session}_{scene_id}_created` (one plan-created row per scene).
pub fn scene_plan_created_event(session_id: &str, turn_id: &str, scene_id: &str) -> DomainEvent {
    DomainEvent::new(
        format!("de_scene_{}_{}_created", session_id, scene_id),
        session_id.to_string(),
        turn_id.to_string(),
        DomainEventKind::ScenePlanCreated,
        serde_json::json!({ "scene_id": scene_id }),
    )
}

/// L3.2 — construct a `BeatPlanned` ledger event for a post-adjudication DirectorPlan beat. The
/// emit site is the narrator compose path (L6.x); this is the pure constructor. Keyed on the
/// turn + resulting beat kind: `de_beat_planned_{session}_{turn}_{kind}`.
pub fn beat_planned_event(
    session_id: &str,
    turn_id: &str,
    thread_id: &str,
    beat_kind: BeatKind,
) -> DomainEvent {
    DomainEvent::new(
        format!(
            "de_beat_planned_{}_{}_{}",
            session_id,
            turn_id,
            beat_kind_token(beat_kind)
        ),
        session_id.to_string(),
        turn_id.to_string(),
        DomainEventKind::BeatPlanned,
        serde_json::json!({
            "thread_id": thread_id,
            "beat_kind": beat_kind_token(beat_kind),
        }),
    )
}

/// L3.2 — construct a `BeatObserved` ledger event from a recorded [`BeatRecord`]. The emit site is
/// the Story Observer (L7.x); this is the pure constructor. Keyed on the recorded turn + kind:
/// `de_beat_observed_{session}_{turn}_{kind}`.
pub fn beat_observed_event(session_id: &str, beat: &BeatRecord) -> DomainEvent {
    DomainEvent::new(
        format!(
            "de_beat_observed_{}_{}_{}",
            session_id,
            beat.turn_id,
            beat_kind_token(beat.beat_kind)
        ),
        session_id.to_string(),
        beat.turn_id.clone(),
        DomainEventKind::BeatObserved,
        serde_json::json!({
            "turn_id": beat.turn_id,
            "thread_id": beat.thread_id,
            "beat_kind": beat_kind_token(beat.beat_kind),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::StoryPromise;

    fn promise(id: &str, status: PromiseStatus, maturity: f32) -> StoryPromise {
        StoryPromise {
            promise_id: id.into(),
            status,
            maturity,
            ..Default::default()
        }
    }

    // StoryPromiseCreated: a promise absent in `before` emits one Created event keyed `_created`.
    #[test]
    fn promise_events_created_on_first_appearance() {
        let before = StoryState::default();
        let after = StoryState {
            promises: vec![promise("p1", PromiseStatus::Planted, 0.0)],
            ..Default::default()
        };
        let events = promise_status_events(&before, &after, "sess1", "t1");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, DomainEventKind::StoryPromiseCreated);
        assert_eq!(events[0].event_id, "de_promise_sess1_p1_created");
        assert_eq!(events[0].data["promise_id"], "p1");
        assert!(events[0].data["prior_status"].is_null());
    }

    // StoryPromiseReinforced: a status advance (not to PaidOff) AND a maturity bump each reinforce;
    // key is on the RESULTING status.
    #[test]
    fn promise_events_reinforced_on_advance_or_maturity() {
        let before = StoryState {
            promises: vec![
                promise("p_adv", PromiseStatus::Planted, 0.2),
                promise("p_mat", PromiseStatus::Developing, 0.3),
                promise("p_flat", PromiseStatus::Developing, 0.5),
            ],
            ..Default::default()
        };
        let after = StoryState {
            promises: vec![
                promise("p_adv", PromiseStatus::Developing, 0.2), // status advanced
                promise("p_mat", PromiseStatus::Developing, 0.6), // maturity increased only
                promise("p_flat", PromiseStatus::Developing, 0.5), // unchanged ⇒ no event
            ],
            ..Default::default()
        };
        let events = promise_status_events(&before, &after, "s", "t");
        assert_eq!(events.len(), 2);
        let by_id: std::collections::HashMap<_, _> = events
            .iter()
            .map(|e| (e.data["promise_id"].as_str().unwrap().to_string(), e))
            .collect();
        assert_eq!(
            by_id["p_adv"].kind,
            DomainEventKind::StoryPromiseReinforced
        );
        assert_eq!(by_id["p_adv"].event_id, "de_promise_s_p_adv_Developing");
        assert_eq!(
            by_id["p_mat"].kind,
            DomainEventKind::StoryPromiseReinforced
        );
        assert!(!by_id.contains_key("p_flat"), "flat promise emits nothing");
    }

    // StoryPromisePaidOff: a move into PaidOff wins regardless of maturity; keyed `_PaidOff`.
    #[test]
    fn promise_events_paid_off() {
        let before = StoryState {
            promises: vec![promise("p1", PromiseStatus::Ripe, 0.9)],
            ..Default::default()
        };
        let after = StoryState {
            promises: vec![promise("p1", PromiseStatus::PaidOff, 1.0)],
            ..Default::default()
        };
        let events = promise_status_events(&before, &after, "s", "t");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, DomainEventKind::StoryPromisePaidOff);
        assert_eq!(events[0].event_id, "de_promise_s_p1_PaidOff");
        assert_eq!(events[0].data["prior_status"], "Ripe");
    }

    // No change ⇒ no events (fail-closed no-op, OFF==baseline byte-equality at the seam).
    #[test]
    fn promise_events_no_change_emits_nothing() {
        let story = StoryState {
            promises: vec![promise("p1", PromiseStatus::Developing, 0.5)],
            ..Default::default()
        };
        assert!(promise_status_events(&story, &story, "s", "t").is_empty());
    }

    // ScenePlanCreated constructor: kind + idempotent scene key + scene_id data.
    #[test]
    fn scene_plan_created_constructor() {
        let ev = scene_plan_created_event("s", "t1", "scene_market");
        assert_eq!(ev.kind, DomainEventKind::ScenePlanCreated);
        assert_eq!(ev.event_id, "de_scene_s_scene_market_created");
        assert_eq!(ev.data["scene_id"], "scene_market");
    }

    // BeatPlanned constructor: kind + key on turn+beat-kind + thread/kind data.
    #[test]
    fn beat_planned_constructor() {
        let ev = beat_planned_event("s", "t1", "thr_x", BeatKind::Complicate);
        assert_eq!(ev.kind, DomainEventKind::BeatPlanned);
        assert_eq!(ev.event_id, "de_beat_planned_s_t1_Complicate");
        assert_eq!(ev.data["thread_id"], "thr_x");
        assert_eq!(ev.data["beat_kind"], "Complicate");
    }

    // BeatObserved constructor: built from a recorded BeatRecord; key on its turn+kind.
    #[test]
    fn beat_observed_constructor() {
        let beat = BeatRecord {
            beat_kind: BeatKind::Payoff,
            turn_id: "t9".into(),
            thread_id: "thr_y".into(),
            ..Default::default()
        };
        let ev = beat_observed_event("s", &beat);
        assert_eq!(ev.kind, DomainEventKind::BeatObserved);
        assert_eq!(ev.event_id, "de_beat_observed_s_t9_Payoff");
        assert_eq!(ev.turn_id, "t9");
        assert_eq!(ev.data["thread_id"], "thr_y");
        assert_eq!(ev.data["beat_kind"], "Payoff");
    }
}

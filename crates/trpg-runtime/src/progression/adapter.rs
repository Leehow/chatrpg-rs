//! Adventure IR runtime — DomainEvent→ProgressEvent adapter (P1-3 glue).
//!
//! The ProgressionEngine ([`super::engine::evaluate`]) consumes the typed,
//! DB-free [`ProgressEvent`]; the live runtime records [`DomainEvent`]s (the
//! append-only ledger). This pure function bridges the two so the engine stays
//! unit-testable without the DB while still being drivable from real turns.
//!
//! Design law honored here:
//! - **Fail-closed**: an event whose `data` lacks the field a mapping needs is
//!   SKIPPED, never guessed. Under-emitting a progress event is safe (the engine
//!   simply observes less); inventing one is not (it could auto-advance state).
//! - **No ruleset/module name branching**: the mapping is purely by event kind +
//!   structural data shape.
//! - Mirrors the established [`crate::event_fold`] fold pattern (typed-parse
//!   `ev.data`, defensive `as_str`).
//!
//! Mappings covered by this slice (grounded in the real emission sites):
//! - [`DomainEventKind::SceneTransitioned`] `{to}` → [`ProgressEvent::Entered`]
//!   (entering a unit starts any timer anchored there; emits LocationChanged).
//! - [`DomainEventKind::PlayerLearnedFact`] / [`DomainEventKind::FactRevealed`]
//!   `{fact_id}` → [`ProgressEvent::WorldFactChanged`] (`fact_id` = true): a
//!   learned/revealed fact becoming true is the carrier Homecoming objectives are
//!   authored against.
//!
//! Deferred (their live sources are not yet a clean per-turn signal; wiring lands
//! with the relay-verified pass): world-time `minutes` → `TimeAdvanced`, and
//! choice records → `ChoiceRecorded`. They are intentionally NOT guessed here.
use super::engine::{evaluate, ProgressionProgram};
use super::state::{ProgressEvent, ProgressionState};
use trpg_model::adventure_ir::{IrValue, ProgressSignal};
use trpg_model::{DomainEvent, DomainEventKind};

/// Map one turn's [`DomainEvent`]s to the [`ProgressEvent`]s the engine consumes.
/// Pure, deterministic, order-preserving; fail-closed (unmappable → dropped).
pub fn progress_events_from_domain(events: &[DomainEvent]) -> Vec<ProgressEvent> {
    events.iter().filter_map(map_one).collect()
}

/// Rebuild the runtime [`ProgressionState`] by replaying a session's
/// [`DomainEvent`]s through the engine **one turn-batch at a time** (preserving
/// causality: a rule whose guard depends on a later turn's fact must not fire
/// early). `events` are expected in `seq` order (as [`crate::Db::list_domain_events`]
/// returns them); consecutive events sharing a `turn_id` form one batch. Returns
/// the folded state and every [`ProgressSignal`] emitted across the replay.
///
/// Pure (no IO/clock/env): the live driver loads the events, this folds them.
pub fn replay_domain_events(
    events: &[DomainEvent],
    program: &ProgressionProgram<'_>,
) -> (ProgressionState, Vec<ProgressSignal>) {
    let mut state = ProgressionState::default();
    let mut signals = Vec::new();
    let mut i = 0;
    while i < events.len() {
        let turn = events[i].turn_id.as_str();
        let mut j = i;
        while j < events.len() && events[j].turn_id == turn {
            j += 1;
        }
        let batch = progress_events_from_domain(&events[i..j]);
        if !batch.is_empty() {
            signals.extend(evaluate(&mut state, &batch, program));
        }
        i = j;
    }
    (state, signals)
}

/// Map a single [`DomainEvent`] to a [`ProgressEvent`], or `None` when it is not a
/// progression-bearing event or its data lacks the required field (fail-closed).
fn map_one(ev: &DomainEvent) -> Option<ProgressEvent> {
    match ev.kind {
        DomainEventKind::SceneTransitioned => {
            let to = non_empty_str(ev, "to")?;
            Some(ProgressEvent::Entered(to))
        }
        DomainEventKind::PlayerLearnedFact | DomainEventKind::FactRevealed => {
            let fact = non_empty_str(ev, "fact_id")?;
            Some(ProgressEvent::WorldFactChanged {
                fact,
                value: IrValue::Bool(true),
            })
        }
        _ => None,
    }
}

/// Read a non-empty string field from the event `data`, or `None` (fail-closed).
fn non_empty_str(ev: &DomainEvent, field: &str) -> Option<String> {
    let s = ev.data.get(field).and_then(|v| v.as_str())?;
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progression::{evaluate, ProgressionProgram, ProgressionState};
    use chrono::{DateTime, Utc};
    use serde_json::json;
    use trpg_model::adventure_ir::{EventPattern, ProgressSignalKind, TrackerKind, TrackerSpec};

    fn ev(kind: DomainEventKind, data: serde_json::Value) -> DomainEvent {
        ev_t(kind, "t", data)
    }

    fn ev_t(kind: DomainEventKind, turn: &str, data: serde_json::Value) -> DomainEvent {
        DomainEvent {
            event_id: format!("e_{turn}_{:?}", kind),
            session_id: "s".into(),
            turn_id: turn.into(),
            kind,
            data,
            source_refs: Vec::new(),
            created_at: DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
        }
    }

    #[test]
    fn scene_transitioned_maps_to_entered() {
        let evs = vec![ev(
            DomainEventKind::SceneTransitioned,
            json!({"from": "sc.a", "to": "sc.b", "reason": "authored flow"}),
        )];
        assert_eq!(
            progress_events_from_domain(&evs),
            vec![ProgressEvent::Entered("sc.b".into())]
        );
    }

    #[test]
    fn player_learned_fact_maps_to_world_fact_true() {
        let evs = vec![ev(
            DomainEventKind::PlayerLearnedFact,
            json!({"fact_id": "fact.athena_disabled"}),
        )];
        assert_eq!(
            progress_events_from_domain(&evs),
            vec![ProgressEvent::WorldFactChanged {
                fact: "fact.athena_disabled".into(),
                value: IrValue::Bool(true),
            }]
        );
    }

    #[test]
    fn fact_revealed_maps_to_world_fact_true() {
        let evs = vec![ev(
            DomainEventKind::FactRevealed,
            json!({"fact_id": "fact.coords"}),
        )];
        assert_eq!(
            progress_events_from_domain(&evs),
            vec![ProgressEvent::WorldFactChanged {
                fact: "fact.coords".into(),
                value: IrValue::Bool(true),
            }]
        );
    }

    #[test]
    fn missing_or_empty_field_is_skipped_fail_closed() {
        let evs = vec![
            // SceneTransitioned with no `to` → dropped (never guessed).
            ev(DomainEventKind::SceneTransitioned, json!({"from": "sc.a"})),
            // empty fact_id → dropped.
            ev(DomainEventKind::PlayerLearnedFact, json!({"fact_id": ""})),
        ];
        assert!(progress_events_from_domain(&evs).is_empty());
    }

    #[test]
    fn unmapped_kinds_are_dropped_not_guessed() {
        // Kinds whose live progression mapping is deferred must NOT be invented.
        let evs = vec![
            ev(DomainEventKind::TurnStarted, json!({})),
            ev(
                DomainEventKind::TurnFinalized,
                json!({"signal": "Narration"}),
            ),
            ev(
                DomainEventKind::ClockAdvanced,
                json!({"clock_id": "c", "new_value": 1, "delta": 1}),
            ),
            ev(DomainEventKind::DiceRolled, json!({})),
        ];
        assert!(progress_events_from_domain(&evs).is_empty());
    }

    #[test]
    fn order_preserved_across_mixed_events() {
        let evs = vec![
            ev(DomainEventKind::TurnStarted, json!({})),
            ev(DomainEventKind::SceneTransitioned, json!({"to": "sc.b"})),
            ev(DomainEventKind::PlayerLearnedFact, json!({"fact_id": "f1"})),
        ];
        assert_eq!(
            progress_events_from_domain(&evs),
            vec![
                ProgressEvent::Entered("sc.b".into()),
                ProgressEvent::WorldFactChanged {
                    fact: "f1".into(),
                    value: IrValue::Bool(true),
                },
            ]
        );
    }

    /// Replay folds a session's events through the engine and surfaces the next
    /// beat: entering A (via SceneTransitioned) fires the authored ridge A→B.
    #[test]
    fn replay_folds_session_and_activates_next() {
        use trpg_model::adventure_ir::{EffectExpr, PredicateExpr, ProgressRule};
        let when = PredicateExpr::All(vec![]);
        let then = vec![EffectExpr::Activate("B".into())];
        let rule = ProgressRule {
            id: "rule.flow.A__B".into(),
            on: EventPattern::Entered("A".into()),
            when: when.clone(),
            then: then.clone(),
            source_evidence: vec![],
            normalization: ProgressRule::classify(&when, &then),
        };
        let rules = vec![rule];
        let program = ProgressionProgram {
            rules: &rules,
            objectives: &[],
            trackers: &[],
        };
        let events = vec![ev_t(
            DomainEventKind::SceneTransitioned,
            "t1",
            json!({"to": "A"}),
        )];
        let (state, signals) = replay_domain_events(&events, &program);
        assert!(state.active_units.contains("B"), "ridge A→B activated B");
        assert!(signals
            .iter()
            .any(|s| s.kind == ProgressSignalKind::BeatActivated && s.id == "B"));
    }

    /// Causality: a rule fires only if its guard held *at the turn of its ON event*.
    /// A gate fact learned in a LATER turn must NOT retro-fire the earlier Entered
    /// rule — which a naive whole-slice (ingest-all-then-match) map would wrongly do.
    #[test]
    fn replay_respects_per_turn_causality() {
        use trpg_model::adventure_ir::{EffectExpr, IrValue, PredicateExpr, ProgressRule};
        let when = PredicateExpr::FactEquals {
            fact: "gate".into(),
            value: IrValue::Bool(true),
        };
        let then = vec![EffectExpr::Activate("secret".into())];
        let rule = ProgressRule {
            id: "rule.gated".into(),
            on: EventPattern::Entered("A".into()),
            when: when.clone(),
            then: then.clone(),
            source_evidence: vec![],
            normalization: ProgressRule::classify(&when, &then),
        };
        let rules = vec![rule];
        let program = ProgressionProgram {
            rules: &rules,
            objectives: &[],
            trackers: &[],
        };
        let events = vec![
            // Turn 1: enter A — but the gate is not yet true.
            ev_t(DomainEventKind::SceneTransitioned, "t1", json!({"to": "A"})),
            // Turn 2: gate becomes true — but there is no Entered(A) event this turn.
            ev_t(
                DomainEventKind::PlayerLearnedFact,
                "t2",
                json!({"fact_id": "gate"}),
            ),
        ];
        let (state, _) = replay_domain_events(&events, &program);
        assert!(
            !state.active_units.contains("secret"),
            "gate learned after entering must not retro-fire the Entered rule"
        );
    }

    #[test]
    fn replay_empty_is_default_state() {
        let program = ProgressionProgram {
            rules: &[],
            objectives: &[],
            trackers: &[],
        };
        let (state, signals) = replay_domain_events(&[], &program);
        assert!(state.active_units.is_empty() && signals.is_empty());
    }

    /// Integration: the adapter's output is engine-consumable and drives real
    /// progression — a SceneTransitioned into Foxwell, run through the adapter and
    /// the engine, starts the Foxwell timer anchor and emits a LocationChanged
    /// ProgressSignal (the glue actually moves the engine, not just shapes data).
    #[test]
    fn adapter_output_drives_engine_and_starts_timer_anchor() {
        let tracker = TrackerSpec {
            id: "tracker.scavvs_timer".into(),
            kind: TrackerKind::Timer,
            label: "Foxwell".into(),
            start: 0,
            threshold: 15,
            anchor: Some(EventPattern::Entered("unit.foxwell_services".into())),
            at_threshold: vec![],
            visible_to_players: false,
            source_evidence: vec![],
            rungs: vec![],
        };
        let trackers = vec![tracker];
        let program = ProgressionProgram {
            rules: &[],
            objectives: &[],
            trackers: &trackers,
        };

        let domain = vec![ev(
            DomainEventKind::SceneTransitioned,
            json!({"to": "unit.foxwell_services"}),
        )];
        let progress = progress_events_from_domain(&domain);

        let mut state = ProgressionState::default();
        let signals = evaluate(&mut state, &progress, &program);

        assert!(state
            .ctx
            .entered_locations
            .iter()
            .any(|l| l == "unit.foxwell_services"));
        assert!(signals
            .iter()
            .any(|s| s.kind == ProgressSignalKind::LocationChanged
                && s.id == "unit.foxwell_services"));
    }
}

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
use super::state::ProgressEvent;
use trpg_model::adventure_ir::IrValue;
use trpg_model::{DomainEvent, DomainEventKind};

/// Map one turn's [`DomainEvent`]s to the [`ProgressEvent`]s the engine consumes.
/// Pure, deterministic, order-preserving; fail-closed (unmappable → dropped).
pub fn progress_events_from_domain(events: &[DomainEvent]) -> Vec<ProgressEvent> {
    events.iter().filter_map(map_one).collect()
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
    use trpg_model::adventure_ir::{
        EventPattern, ProgressSignalKind, TrackerKind, TrackerSpec,
    };

    fn ev(kind: DomainEventKind, data: serde_json::Value) -> DomainEvent {
        DomainEvent {
            event_id: "e".into(),
            session_id: "s".into(),
            turn_id: "t".into(),
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
            ev(DomainEventKind::TurnFinalized, json!({"signal": "Narration"})),
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
            ev(
                DomainEventKind::SceneTransitioned,
                json!({"to": "sc.b"}),
            ),
            ev(
                DomainEventKind::PlayerLearnedFact,
                json!({"fact_id": "f1"}),
            ),
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

        assert!(state.ctx.entered_locations.iter().any(|l| l == "unit.foxwell_services"));
        assert!(signals
            .iter()
            .any(|s| s.kind == ProgressSignalKind::LocationChanged
                && s.id == "unit.foxwell_services"));
    }
}

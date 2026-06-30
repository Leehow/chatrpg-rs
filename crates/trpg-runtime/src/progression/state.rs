//! Adventure IR runtime — ProgressionState + ProgressEvent (P1-3).
//!
//! ProgressionState is the runtime progress dimension whose absence is the proven
//! J3 stall root cause. `current_scene_id` keeps meaning "where the focus is";
//! this holds *what has advanced*. It is pure data — the engine
//! ([`super::engine`]) mutates it deterministically; the LLM never does.
use std::collections::{BTreeMap, BTreeSet};
use trpg_model::adventure_ir::{EvalContext, EventPattern, IrValue, ObjectiveStatus};

/// A typed runtime input. The engine consumes these; a (deferred, relay-verified)
/// adapter maps `DomainEvent` → `ProgressEvent` so the core stays pure and
/// unit-testable without the DB. Mirrors [`EventPattern`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgressEvent {
    /// Player/focus entered a unit/location (starts any timer anchored here).
    Entered(String),
    /// A choice was recorded.
    ChoiceRecorded { key: String, value: String },
    /// A world fact changed.
    WorldFactChanged { fact: String, value: IrValue },
    /// World time advanced by `minutes` (advances all running timers).
    TimeAdvanced { minutes: i64 },
    /// An objective was externally resolved (notification).
    ObjectiveResolved(String),
}

impl ProgressEvent {
    /// Whether this concrete event matches a rule's `ON` [`EventPattern`].
    pub fn matches(&self, pat: &EventPattern) -> bool {
        matches!(
            (self, pat),
            (
                ProgressEvent::TimeAdvanced { .. },
                EventPattern::TimeAdvanced
            ) | (
                ProgressEvent::WorldFactChanged { .. },
                EventPattern::WorldFactChanged
            ) | (
                ProgressEvent::ChoiceRecorded { .. },
                EventPattern::ChoiceRecorded
            )
        ) || match (self, pat) {
            (ProgressEvent::Entered(a), EventPattern::Entered(b)) => a == b,
            (ProgressEvent::ObjectiveResolved(a), EventPattern::ObjectiveResolved(b)) => a == b,
            _ => false,
        }
    }
}

/// The authoritative runtime progress state.
#[derive(Debug, Clone, Default)]
pub struct ProgressionState {
    /// Units (beats/encounters/outcomes) currently active.
    pub active_units: BTreeSet<String>,
    /// Units that have been resolved/completed.
    pub resolved_units: BTreeSet<String>,
    /// Units explicitly foreclosed (e.g. the disabled branch).
    pub disabled_units: BTreeSet<String>,
    /// Trackers that have already fired their threshold (dedup re-firing).
    pub fired_trackers: BTreeSet<String>,
    /// Fact/objective/clock/elapsed/choice/location store the guards read from.
    pub ctx: EvalContext,
}

impl ProgressionState {
    /// Current status of an objective (Inactive if unseen).
    pub fn objective_status(&self, id: &str) -> ObjectiveStatus {
        self.ctx
            .objectives
            .get(id)
            .copied()
            .unwrap_or(ObjectiveStatus::Inactive)
    }

    pub(super) fn set_objective(&mut self, id: &str, status: ObjectiveStatus) {
        self.ctx.objectives.insert(id.to_string(), status);
    }

    /// Elapsed world-minutes recorded against an event anchor, if it has occurred.
    pub(super) fn elapsed_for(&self, ev: &EventPattern) -> Option<i64> {
        self.ctx
            .elapsed
            .iter()
            .find(|(e, _)| e == ev)
            .map(|(_, m)| *m)
    }

    pub(super) fn start_anchor(&mut self, ev: EventPattern) {
        if !self.ctx.elapsed.iter().any(|(e, _)| *e == ev) {
            self.ctx.elapsed.push((ev, 0));
        }
    }

    pub(super) fn advance_all_timers(&mut self, minutes: i64) {
        for (_, m) in self.ctx.elapsed.iter_mut() {
            *m += minutes;
        }
    }

    pub(super) fn clocks_mut(&mut self) -> &mut BTreeMap<String, i32> {
        &mut self.ctx.clocks
    }
}

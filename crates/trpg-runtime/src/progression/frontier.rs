//! Adventure IR runtime — AdvancementFrontier (P1-3). The set of *currently legal
//! next content* the Director selects focus from. The LLM ranks/chooses within
//! this frontier; it never invents what is unlocked nor teleports from the whole
//! book (设计评审 §七). Rust computes the frontier; the LLM picks.
use super::state::ProgressionState;
use trpg_model::adventure_ir::{ObjectiveSpec, ObjectiveStatus, PredicateValue, TrackerSpec};

/// What is legally available to advance to right now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdvancementFrontier {
    /// Newly/currently active, not-yet-resolved units (just-unlocked beats etc).
    pub active_units: Vec<String>,
    /// Trackers that have reached their threshold this run (due clocks/timers).
    pub due_trackers: Vec<String>,
    /// Active objectives still open — candidate goals to focus.
    pub open_objectives: Vec<String>,
}

impl AdvancementFrontier {
    pub fn is_empty(&self) -> bool {
        self.active_units.is_empty()
            && self.due_trackers.is_empty()
            && self.open_objectives.is_empty()
    }

    pub fn len(&self) -> usize {
        self.active_units.len() + self.due_trackers.len() + self.open_objectives.len()
    }
}

/// Compute the frontier from current state. Pure, deterministic, sorted output.
pub fn compute_frontier(
    state: &ProgressionState,
    objectives: &[ObjectiveSpec],
    trackers: &[TrackerSpec],
) -> AdvancementFrontier {
    // Active units that are neither resolved nor disabled are the legal next beats.
    let active_units: Vec<String> = state
        .active_units
        .iter()
        .filter(|u| !state.resolved_units.contains(*u) && !state.disabled_units.contains(*u))
        .cloned()
        .collect();

    let due_trackers: Vec<String> = trackers
        .iter()
        .filter(|t| state.fired_trackers.contains(&t.id))
        .map(|t| t.id.clone())
        .collect();

    // An objective is "open" if it is not terminal. We only surface objectives the
    // engine can reason about: those whose success guard is executable (opaque
    // guards stay GM-facing, never auto-driven), per fail-closed at execution.
    let open_objectives: Vec<String> = objectives
        .iter()
        .filter(|o| {
            let st = state.objective_status(&o.id);
            !matches!(st, ObjectiveStatus::Completed | ObjectiveStatus::Failed)
                && o.success_when.eval(&state.ctx) != PredicateValue::NonExecutable
        })
        .map(|o| o.id.clone())
        .collect();

    AdvancementFrontier {
        active_units,
        due_trackers,
        open_objectives,
    }
}

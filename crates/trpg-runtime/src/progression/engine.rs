//! Adventure IR runtime — ProgressionEngine (P1-3). Each turn, after Rules/World
//! commit facts: ingest DomainEvents → fire authored ProgressRules whose guards
//! Rust can decide → re-derive objectives/trackers → emit [`ProgressSignal`]s.
//! Then [`super::frontier::compute_frontier`] yields the legal frontier the
//! Director selects focus from.
//!
//! Fail-closed at the execution layer: a rule auto-fires only when
//! [`ProgressRule::may_autofire`] holds; an objective auto-completes only when its
//! success guard is executable AND evaluates `True`. Opaque authored conditions
//! are stored for the GM and never auto-fire.
use super::state::{ProgressEvent, ProgressionState};
use trpg_model::adventure_ir::{
    EffectExpr, ObjectiveSpec, ObjectiveStatus, PredicateValue, ProgressRule, ProgressSignal,
    ProgressSignalKind, TrackerKind, TrackerSpec,
};

/// The authored program the engine executes against runtime state. Borrowed each
/// turn; produced by the AuthoredFlowHead/Objective extractors (P1-2).
pub struct ProgressionProgram<'a> {
    pub rules: &'a [ProgressRule],
    pub objectives: &'a [ObjectiveSpec],
    pub trackers: &'a [TrackerSpec],
}

/// Apply one turn's events to `state`, returning the progress signals emitted.
/// Pure given its inputs (no IO, no clock, no env). Deterministic ordering.
pub fn evaluate(
    state: &mut ProgressionState,
    events: &[ProgressEvent],
    program: &ProgressionProgram<'_>,
) -> Vec<ProgressSignal> {
    let mut signals = Vec::new();

    // 1) Ingest events into the fact store (and surface spatial/phase signals).
    for ev in events {
        ingest(state, ev, &mut signals);
    }

    // 2) Timers/trackers: fire thresholds reached after this turn's time advance.
    for t in program.trackers {
        check_tracker(state, t, &mut signals);
    }

    // 3) Authored ECA rules: fire those matching an event whose guard Rust can
    //    decide as True (fail-closed: may_autofire gates opaque guards/effects).
    for ev in events {
        for r in program.rules {
            if ev.matches(&r.on)
                && r.may_autofire()
                && r.when.eval(&state.ctx) == PredicateValue::True
            {
                apply_effects(state, &r.then, &mut signals);
            }
        }
    }

    // 4) Re-derive objective statuses from their guards (success/failure).
    for o in program.objectives {
        update_objective(state, o, &mut signals);
    }

    signals
}

fn ingest(state: &mut ProgressionState, ev: &ProgressEvent, signals: &mut Vec<ProgressSignal>) {
    match ev {
        ProgressEvent::Entered(id) => {
            if !state.ctx.entered_locations.iter().any(|l| l == id) {
                state.ctx.entered_locations.push(id.clone());
            }
            // Starting an anchor lets timers measure from here.
            state.start_anchor(trpg_model::adventure_ir::EventPattern::Entered(id.clone()));
            signals.push(ProgressSignal::new(ProgressSignalKind::LocationChanged, id));
        }
        ProgressEvent::ChoiceRecorded { key, value } => {
            state.ctx.choices.insert(key.clone(), value.clone());
        }
        ProgressEvent::WorldFactChanged { fact, value } => {
            state.ctx.facts.insert(fact.clone(), value.clone());
        }
        ProgressEvent::TimeAdvanced { minutes } => {
            state.advance_all_timers(*minutes);
        }
        ProgressEvent::ObjectiveResolved(_) => {}
    }
}

fn check_tracker(state: &mut ProgressionState, t: &TrackerSpec, signals: &mut Vec<ProgressSignal>) {
    if state.fired_trackers.contains(&t.id) {
        return;
    }
    let reached = match t.kind {
        TrackerKind::Timer => t
            .anchor
            .as_ref()
            .and_then(|a| state.elapsed_for(a))
            .map(|elapsed| elapsed >= t.threshold)
            .unwrap_or(false),
        TrackerKind::Countdown => state
            .ctx
            .clocks
            .get(&t.id)
            .map(|c| (*c as i64) >= t.threshold)
            .unwrap_or(false),
    };
    if reached {
        state.fired_trackers.insert(t.id.clone());
        signals.push(ProgressSignal::new(
            ProgressSignalKind::ClockAdvanced,
            &t.id,
        ));
        // Threshold effects are executable-only (fail-closed).
        apply_effects(state, &t.at_threshold, signals);
    }
}

fn apply_effects(
    state: &mut ProgressionState,
    effects: &[EffectExpr],
    signals: &mut Vec<ProgressSignal>,
) {
    for e in effects {
        if !e.is_executable() {
            // Opaque authored effect — stored elsewhere for the GM, never applied.
            continue;
        }
        match e {
            EffectExpr::Activate(id) => {
                state.active_units.insert(id.clone());
                signals.push(ProgressSignal::new(ProgressSignalKind::BeatActivated, id));
            }
            EffectExpr::Complete(id) => {
                state.set_objective(id, ObjectiveStatus::Completed);
                state.resolved_units.insert(id.clone());
                signals.push(ProgressSignal::new(
                    ProgressSignalKind::ObjectiveCompleted,
                    id,
                ));
            }
            EffectExpr::Fail(id) => {
                state.set_objective(id, ObjectiveStatus::Failed);
                signals.push(
                    ProgressSignal::new(ProgressSignalKind::ObjectiveAdvanced, id)
                        .with_detail("failed"),
                );
            }
            EffectExpr::Disable(id) => {
                state.active_units.remove(id);
                state.disabled_units.insert(id.clone());
            }
            EffectExpr::Reveal(id) => {
                state
                    .ctx
                    .known_revelations
                    .push((id.clone(), trpg_model::adventure_ir::KnowledgeHolder::Party));
                signals.push(ProgressSignal::new(
                    ProgressSignalKind::RevelationUnlocked,
                    id,
                ));
            }
            EffectExpr::AdvanceClock { id, by } => {
                let entry = state.clocks_mut().entry(id.clone()).or_insert(0);
                *entry += *by;
                signals.push(ProgressSignal::new(ProgressSignalKind::ClockAdvanced, id));
            }
            EffectExpr::SetFact { fact, value } => {
                state.ctx.facts.insert(fact.clone(), value.clone());
            }
            EffectExpr::OpaqueAuthoredText { .. } => {}
        }
    }
}

fn update_objective(
    state: &mut ProgressionState,
    o: &ObjectiveSpec,
    signals: &mut Vec<ProgressSignal>,
) {
    let st = state.objective_status(&o.id);
    if matches!(st, ObjectiveStatus::Completed | ObjectiveStatus::Failed) {
        return;
    }
    // Failure first: a met failure guard forecloses the objective.
    if let Some(fw) = &o.failure_when {
        if fw.eval(&state.ctx) == PredicateValue::True {
            state.set_objective(&o.id, ObjectiveStatus::Failed);
            signals.push(
                ProgressSignal::new(ProgressSignalKind::ObjectiveAdvanced, &o.id)
                    .with_detail("failed"),
            );
            return;
        }
    }
    // Success only auto-completes when Rust can fully decide it as True.
    if o.success_when.eval(&state.ctx) == PredicateValue::True {
        state.set_objective(&o.id, ObjectiveStatus::Completed);
        state.resolved_units.insert(o.id.clone());
        signals.push(ProgressSignal::new(
            ProgressSignalKind::ObjectiveCompleted,
            &o.id,
        ));
    }
}

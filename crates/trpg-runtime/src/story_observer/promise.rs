//! L7.1 — promise maturity advancement (`Planted → Developing → Ripe → PaidOff`). Maturity rises
//! with payoff-candidate support and never regresses; an explicit committed payoff event is the
//! authoritative path to `PaidOff`. Terminal promises are untouched.

use std::collections::{HashMap, HashSet};
use trpg_model::{PromiseStatus, StoryPromise, StoryState};

/// Ladder rank of a promise status (terminal `PaidOff`/`Broken` share the top).
fn promise_rank(s: PromiseStatus) -> u8 {
    match s {
        PromiseStatus::Planted => 0,
        PromiseStatus::Developing => 1,
        PromiseStatus::Ripe => 2,
        PromiseStatus::PaidOff | PromiseStatus::Broken => 3,
    }
}

/// Maturity-derived promise status (forward target). `>= 0.8` ⇒ Ripe, `>= 0.3` ⇒ Developing.
fn status_from_maturity(maturity: f32) -> PromiseStatus {
    if maturity >= 0.8 {
        PromiseStatus::Ripe
    } else if maturity >= 0.3 {
        PromiseStatus::Developing
    } else {
        PromiseStatus::Planted
    }
}

/// Advance one promise from the signal. Explicit `paid_off` wins (→ `PaidOff`, maturity 1.0,
/// `payoff_event_id` set); otherwise maturity rises (never regresses) with payoff-candidate
/// support and the status floors up to its maturity target. Terminal promises are untouched.
fn advance_promise(
    p: &mut StoryPromise,
    known: &HashSet<&str>,
    paid_off: &HashMap<&str, &str>,
) -> bool {
    if matches!(p.status, PromiseStatus::PaidOff | PromiseStatus::Broken) {
        return false; // terminal
    }
    if let Some(ev) = paid_off.get(p.promise_id.as_str()) {
        p.status = PromiseStatus::PaidOff;
        p.maturity = 1.0;
        p.payoff_event_id = Some((*ev).to_string());
        return true;
    }
    let support = if p.payoff_candidate_fact_ids.is_empty() {
        0.0
    } else {
        p.payoff_candidate_fact_ids
            .iter()
            .filter(|f| known.contains(f.as_str()))
            .count() as f32
            / p.payoff_candidate_fact_ids.len() as f32
    };
    let mut changed = false;
    let new_maturity = support.max(p.maturity); // never regress
    if new_maturity > p.maturity {
        p.maturity = new_maturity;
        changed = true;
    }
    let target = status_from_maturity(p.maturity);
    if promise_rank(target) > promise_rank(p.status) {
        p.status = target;
        changed = true;
    }
    changed
}

/// Advance every promise from the turn's known facts + explicit payoff events. Returns whether
/// any promise changed.
pub(super) fn advance_promises(
    story: &mut StoryState,
    known: &HashSet<&str>,
    paid_off: &[(String, String)],
) -> bool {
    let paid: HashMap<&str, &str> = paid_off
        .iter()
        .map(|(p, e)| (p.as_str(), e.as_str()))
        .collect();
    let mut changed = false;
    for p in &mut story.promises {
        if advance_promise(p, known, &paid) {
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(story: StoryState, known: &[&str], paid: &[(&str, &str)]) -> (StoryState, bool) {
        let mut story = story;
        let kn: HashSet<&str> = known.iter().copied().collect();
        let paid: Vec<(String, String)> = paid
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
        let changed = advance_promises(&mut story, &kn, &paid);
        (story, changed)
    }

    fn promise(id: &str, status: PromiseStatus, cands: &[&str]) -> StoryPromise {
        StoryPromise {
            promise_id: id.into(),
            status,
            payoff_candidate_fact_ids: cands.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn l71_promise_matures_to_ripe() {
        let story = StoryState {
            promises: vec![promise("pr", PromiseStatus::Planted, &["a", "b", "c", "d"])],
            ..Default::default()
        };
        let (story, _) = run(story, &["a"], &[]); // 1/4 ⇒ .25 ⇒ Planted
        assert_eq!(story.promises[0].status, PromiseStatus::Planted);
        let (story, _) = run(story, &["a", "b"], &[]); // 2/4 ⇒ .5 ⇒ Developing
        assert_eq!(story.promises[0].status, PromiseStatus::Developing);
        let (story, _) = run(story, &["a", "b", "c", "d"], &[]); // 4/4 ⇒ 1.0 ⇒ Ripe
        assert_eq!(story.promises[0].status, PromiseStatus::Ripe);
        assert!((story.promises[0].maturity - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn l71_explicit_payoff_marks_paid_off() {
        let story = StoryState {
            promises: vec![promise("pr", PromiseStatus::Ripe, &[])],
            ..Default::default()
        };
        let (story, c) = run(story, &[], &[("pr", "ev_42")]);
        assert!(c);
        assert_eq!(story.promises[0].status, PromiseStatus::PaidOff);
        assert_eq!(story.promises[0].payoff_event_id.as_deref(), Some("ev_42"));
        let (story, c2) = run(story, &[], &[]); // terminal ⇒ no-op
        assert!(!c2);
        assert_eq!(story.promises[0].status, PromiseStatus::PaidOff);
    }

    #[test]
    fn l71_maturity_never_regresses() {
        let mut p = promise("pr", PromiseStatus::Developing, &["a", "b"]);
        p.maturity = 0.5;
        let story = StoryState {
            promises: vec![p],
            ..Default::default()
        };
        let (story, c) = run(story, &[], &[]); // no known ⇒ support 0, maturity holds at .5
        assert!(!c);
        assert!((story.promises[0].maturity - 0.5).abs() < f32::EPSILON);
        assert_eq!(story.promises[0].status, PromiseStatus::Developing);
    }
}

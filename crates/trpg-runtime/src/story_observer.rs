//! L7.1 — PURE (DB-free) Story Observer: deterministic thread-status ADVANCEMENT + promise
//! maturity from committed turn signals.
//!
//! This is the de-stub of the "advancement OutOfScope" limit (P6.8b): where
//! [`crate::story_write::apply_thread_opened`] only FLOORS `Dormant → Introduced` on a fact
//! becoming known, the observer here advances an already-opened thread along the status ladder
//! (`Introduced → Active → Escalating → ReadyForPayoff`) and matures promises
//! (`Planted → Developing → Ripe → PaidOff`) from the turn's committed signals.
//!
//! Discipline (RUN_SPEC §0):
//! - PURE / DB-free — no DB handle, no I/O. The async committer lives in
//!   [`crate::director_brief::commit_story_writes`], gated by `TRPG_STORY_WRITE_LOOP` (default OFF).
//! - GENERIC — every transition is keyed on momentum/maturity signals, NEVER on
//!   `ruleset_id`/`module_id` (§二-⑪).
//! - FORWARD-ONLY + FAIL-CLOSED — the observer only ADVANCES a status (never regresses), never
//!   touches a TERMINAL status (`Resolved`/`Abandoned`/`Transformed`; `PaidOff`/`Broken`), and
//!   never advances a `Dormant` thread (opening is `apply_thread_opened`'s job — knowledge alone
//!   only OPENS; ENGAGEMENT drives advancement). A signal-free turn ⇒ no change ⇒ no event.
//! - ADDITIVE — the snapshot stays the single source of truth (R2 promotion is OutOfScope); the
//!   committer derives the L3.1/L3.2 ledger events from the resulting status/maturity diff.

use std::collections::{HashMap, HashSet};
use trpg_model::{InterestSignal, PromiseStatus, StoryPromise, StoryState, StoryThreadStatus};

/// The committed, player-visible signals one turn produces, projected for the observer. All
/// fields default-empty ⇒ a quiet turn is a no-op. Built by the committer (DB side); the observer
/// itself stays pure.
#[derive(Debug, Default, Clone)]
pub struct ObserverSignal {
    /// Thread ids the player ACTIVELY engaged this turn (the positive counterpart of a rejection
    /// — GM/Director proposes; generic). Combined with the persisted positive
    /// `player_interests` to decide engagement. Engagement is REQUIRED to advance past
    /// `Introduced`.
    pub engaged_thread_ids: Vec<String>,
    /// Cumulative facts now player-known. Drives thread fulfillment (known-ratio) + promise
    /// maturity (payoff-candidate support).
    pub known_fact_ids: Vec<String>,
    /// `(promise_id, payoff_event_id)` pairs an explicit committed payoff event paid off this
    /// turn — the authoritative, non-derived path to `PaidOff`.
    pub paid_off: Vec<(String, String)>,
}

/// Ladder rank of a thread status. Open statuses ascend `1..=4`; terminal statuses share `5`
/// (never derived-advanced). Local match (not serde) so a future rename can't drift the ladder.
fn thread_rank(s: StoryThreadStatus) -> u8 {
    match s {
        StoryThreadStatus::Dormant => 0,
        StoryThreadStatus::Introduced => 1,
        StoryThreadStatus::Active => 2,
        StoryThreadStatus::Escalating => 3,
        StoryThreadStatus::ReadyForPayoff => 4,
        StoryThreadStatus::Resolved
        | StoryThreadStatus::Abandoned
        | StoryThreadStatus::Transformed => 5,
    }
}

/// The status an OPENED, ENGAGED thread is momentum-eligible for. Knowledge alone never advances
/// past `Introduced` (an opened-but-ignored thread stays put); engagement lifts it to at least
/// `Active`, and the known-ratio of its related facts carries it toward payoff. Generic.
fn target_thread_status(
    related_fact_ids: &[String],
    engaged: bool,
    known: &HashSet<&str>,
) -> StoryThreadStatus {
    if !engaged {
        return StoryThreadStatus::Introduced;
    }
    let known_ratio = if related_fact_ids.is_empty() {
        0.0
    } else {
        related_fact_ids
            .iter()
            .filter(|f| known.contains(f.as_str()))
            .count() as f32
            / related_fact_ids.len() as f32
    };
    let momentum = 0.5 + 0.5 * known_ratio; // engaged ⇒ ∈ [0.5, 1.0]
    if momentum >= 0.9 {
        StoryThreadStatus::ReadyForPayoff
    } else if momentum >= 0.6 {
        StoryThreadStatus::Escalating
    } else {
        StoryThreadStatus::Active
    }
}

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

/// Observe a committed turn: advance thread statuses + promise maturities from `signal`, returning
/// the updated story and whether anything actually changed (so the committer can skip a no-op
/// upsert). PURE + forward-only + fail-closed.
pub fn observe_story(mut story: StoryState, signal: &ObserverSignal) -> (StoryState, bool) {
    // Engagement = this turn's proposals ∪ the persisted positive interest signals (already
    // committed). A rejected interest never counts as engagement.
    let mut engaged: HashSet<String> = signal.engaged_thread_ids.iter().cloned().collect();
    for s in &story.player_interests {
        if !s.rejected
            && matches!(
                s.signal,
                InterestSignal::Engaged | InterestSignal::Curious | InterestSignal::Invested
            )
        {
            engaged.insert(s.thread_id.clone());
        }
    }
    let known: HashSet<&str> = signal.known_fact_ids.iter().map(String::as_str).collect();
    let paid_off: HashMap<&str, &str> = signal
        .paid_off
        .iter()
        .map(|(p, e)| (p.as_str(), e.as_str()))
        .collect();

    let mut changed = false;
    for t in &mut story.active_threads {
        let rank = thread_rank(t.status);
        if rank == 0 || rank >= 5 {
            continue; // Dormant (opening is apply_thread_opened's job) or terminal — skip
        }
        let target = target_thread_status(
            &t.related_fact_ids,
            engaged.contains(&t.thread_id),
            &known,
        );
        if thread_rank(target) > rank {
            t.status = target; // forward-only
            changed = true;
        }
    }
    for p in &mut story.promises {
        if advance_promise(p, &known, &paid_off) {
            changed = true;
        }
    }
    (story, changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::{PlayerInterestSignal, StoryThread};

    fn thread(id: &str, status: StoryThreadStatus, facts: &[&str]) -> StoryThread {
        StoryThread {
            thread_id: id.into(),
            status,
            related_fact_ids: facts.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn engaged_signal(thread_id: &str, known: &[&str]) -> ObserverSignal {
        ObserverSignal {
            engaged_thread_ids: vec![thread_id.to_string()],
            known_fact_ids: known.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    // The acceptance ladder: Introduced → Active → ReadyForPayoff across engaged turns.
    #[test]
    fn l71_thread_advances_introduced_active_ready_for_payoff() {
        let story = StoryState {
            active_threads: vec![thread("thr", StoryThreadStatus::Introduced, &["f1", "f2"])],
            ..Default::default()
        };
        // Turn 1: engaged, no facts known ⇒ Active.
        let (story, c1) = observe_story(story, &engaged_signal("thr", &[]));
        assert!(c1);
        assert_eq!(story.active_threads[0].status, StoryThreadStatus::Active);
        // Turn 2: engaged, ALL related facts known ⇒ jumps to ReadyForPayoff.
        let (story, c2) = observe_story(story, &engaged_signal("thr", &["f1", "f2"]));
        assert!(c2);
        assert_eq!(
            story.active_threads[0].status,
            StoryThreadStatus::ReadyForPayoff
        );
        // Idempotent: re-observing at the ceiling changes nothing.
        let (story, c3) = observe_story(story, &engaged_signal("thr", &["f1", "f2"]));
        assert!(!c3);
        assert_eq!(
            story.active_threads[0].status,
            StoryThreadStatus::ReadyForPayoff
        );
    }

    // Partial knowledge with engagement lands the middle rung (Escalating).
    #[test]
    fn l71_partial_known_ratio_yields_escalating() {
        let story = StoryState {
            active_threads: vec![thread("thr", StoryThreadStatus::Active, &["f1", "f2"])],
            ..Default::default()
        };
        let (story, c) = observe_story(story, &engaged_signal("thr", &["f1"])); // ratio .5
        assert!(c);
        assert_eq!(story.active_threads[0].status, StoryThreadStatus::Escalating);
    }

    // Knowledge WITHOUT engagement only opens — an Introduced-but-ignored thread never advances.
    #[test]
    fn l71_knowledge_without_engagement_does_not_advance() {
        let story = StoryState {
            active_threads: vec![thread("thr", StoryThreadStatus::Introduced, &["f1", "f2"])],
            ..Default::default()
        };
        // All facts known, but the thread is NOT engaged this turn and has no interest signal.
        let signal = ObserverSignal {
            known_fact_ids: vec!["f1".into(), "f2".into()],
            ..Default::default()
        };
        let (story, c) = observe_story(story, &signal);
        assert!(!c, "fact-knowledge alone must not advance an unengaged thread");
        assert_eq!(story.active_threads[0].status, StoryThreadStatus::Introduced);
    }

    // Persisted positive interest counts as engagement (no explicit turn signal needed).
    #[test]
    fn l71_persisted_interest_drives_engagement() {
        let story = StoryState {
            active_threads: vec![thread("thr", StoryThreadStatus::Introduced, &[])],
            player_interests: vec![PlayerInterestSignal {
                thread_id: "thr".into(),
                signal: InterestSignal::Invested,
                rejected: false,
                ..Default::default()
            }],
            ..Default::default()
        };
        let (story, c) = observe_story(story, &ObserverSignal::default());
        assert!(c);
        assert_eq!(story.active_threads[0].status, StoryThreadStatus::Active);
    }

    // A REJECTED interest never counts as engagement (anti-railroad respected at the observer).
    #[test]
    fn l71_rejected_interest_is_not_engagement() {
        let story = StoryState {
            active_threads: vec![thread("thr", StoryThreadStatus::Introduced, &[])],
            player_interests: vec![PlayerInterestSignal {
                thread_id: "thr".into(),
                signal: InterestSignal::Engaged,
                rejected: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let (story, c) = observe_story(story, &ObserverSignal::default());
        assert!(!c);
        assert_eq!(story.active_threads[0].status, StoryThreadStatus::Introduced);
    }

    // Dormant and terminal threads are never touched by the observer.
    #[test]
    fn l71_dormant_and_terminal_threads_untouched() {
        let story = StoryState {
            active_threads: vec![
                thread("thr_dorm", StoryThreadStatus::Dormant, &["f1"]),
                thread("thr_done", StoryThreadStatus::Resolved, &["f1"]),
            ],
            ..Default::default()
        };
        let signal = ObserverSignal {
            engaged_thread_ids: vec!["thr_dorm".into(), "thr_done".into()],
            known_fact_ids: vec!["f1".into()],
            ..Default::default()
        };
        let (story, c) = observe_story(story, &signal);
        assert!(!c);
        assert_eq!(story.active_threads[0].status, StoryThreadStatus::Dormant);
        assert_eq!(story.active_threads[1].status, StoryThreadStatus::Resolved);
    }

    // Promise matures Planted → Developing → Ripe as its payoff candidates become known.
    #[test]
    fn l71_promise_matures_to_ripe() {
        let story = StoryState {
            promises: vec![StoryPromise {
                promise_id: "pr".into(),
                status: PromiseStatus::Planted,
                payoff_candidate_fact_ids: vec!["a".into(), "b".into(), "c".into(), "d".into()],
                ..Default::default()
            }],
            ..Default::default()
        };
        // 1/4 known ⇒ maturity .25 ⇒ still Planted (< .3).
        let (story, _) = observe_story(
            story,
            &ObserverSignal {
                known_fact_ids: vec!["a".into()],
                ..Default::default()
            },
        );
        assert_eq!(story.promises[0].status, PromiseStatus::Planted);
        // 2/4 known ⇒ maturity .5 ⇒ Developing.
        let (story, _) = observe_story(
            story,
            &ObserverSignal {
                known_fact_ids: vec!["a".into(), "b".into()],
                ..Default::default()
            },
        );
        assert_eq!(story.promises[0].status, PromiseStatus::Developing);
        // 4/4 known ⇒ maturity 1.0 ⇒ Ripe.
        let (story, _) = observe_story(
            story,
            &ObserverSignal {
                known_fact_ids: vec!["a".into(), "b".into(), "c".into(), "d".into()],
                ..Default::default()
            },
        );
        assert_eq!(story.promises[0].status, PromiseStatus::Ripe);
        assert!((story.promises[0].maturity - 1.0).abs() < f32::EPSILON);
    }

    // An explicit committed payoff event pays a promise off and records the event id.
    #[test]
    fn l71_explicit_payoff_marks_paid_off() {
        let story = StoryState {
            promises: vec![StoryPromise {
                promise_id: "pr".into(),
                status: PromiseStatus::Ripe,
                ..Default::default()
            }],
            ..Default::default()
        };
        let (story, c) = observe_story(
            story,
            &ObserverSignal {
                paid_off: vec![("pr".into(), "ev_42".into())],
                ..Default::default()
            },
        );
        assert!(c);
        assert_eq!(story.promises[0].status, PromiseStatus::PaidOff);
        assert_eq!(story.promises[0].payoff_event_id.as_deref(), Some("ev_42"));
        // Terminal: a later observe is a no-op.
        let (story, c2) = observe_story(story, &ObserverSignal::default());
        assert!(!c2);
        assert_eq!(story.promises[0].status, PromiseStatus::PaidOff);
    }

    // Maturity never regresses even if support drops on a later turn.
    #[test]
    fn l71_maturity_never_regresses() {
        let story = StoryState {
            promises: vec![StoryPromise {
                promise_id: "pr".into(),
                status: PromiseStatus::Developing,
                maturity: 0.5,
                payoff_candidate_fact_ids: vec!["a".into(), "b".into()],
                ..Default::default()
            }],
            ..Default::default()
        };
        // No facts known this turn ⇒ support 0.0, but maturity must hold at 0.5.
        let (story, c) = observe_story(story, &ObserverSignal::default());
        assert!(!c);
        assert!((story.promises[0].maturity - 0.5).abs() < f32::EPSILON);
        assert_eq!(story.promises[0].status, PromiseStatus::Developing);
    }

    // A fully signal-free turn is a no-op (quiet turn writes nothing).
    #[test]
    fn l71_quiet_turn_is_noop() {
        let story = StoryState {
            active_threads: vec![thread("thr", StoryThreadStatus::Active, &["f1"])],
            promises: vec![StoryPromise {
                promise_id: "pr".into(),
                status: PromiseStatus::Developing,
                ..Default::default()
            }],
            ..Default::default()
        };
        let (out, c) = observe_story(story.clone(), &ObserverSignal::default());
        assert!(!c);
        assert_eq!(out, story);
    }
}

//! Story Observer: deterministic per-turn evolution of the persisted [`StoryState`] from a
//! turn's committed signals. PURE / DB-free — the async committer lives in
//! [`crate::director_brief::commit_story_writes`], gated by `TRPG_STORY_WRITE_LOOP` (default OFF).
//!
//! - **L7.1** ([`thread`], [`promise`]) — advances OPENED threads along the status ladder
//!   (`Introduced → Active → Escalating → ReadyForPayoff`) and matures promises
//!   (`Planted → Developing → Ripe → PaidOff`). De-stubs the former P6.8b "advancement
//!   OutOfScope" limit.
//! - **L7.2** ([`interest`]) — evolves player-interest signals + the `PacingState` history from
//!   the committed engagement + beat of the turn.
//!
//! Discipline (RUN_SPEC §0): GENERIC — every transition keys on momentum/maturity/beat signals,
//! NEVER on `ruleset_id`/`module_id` (§二-⑪). FORWARD-ONLY + FAIL-CLOSED — only ADVANCE a status
//! (never regress), never touch a TERMINAL status, never advance a `Dormant` thread (opening is
//! [`crate::story_write::apply_thread_opened`]'s job — knowledge alone only OPENS; ENGAGEMENT
//! drives advancement). A signal-free turn ⇒ no change ⇒ no event. ADDITIVE — the snapshot stays
//! the single source of truth (R2 promotion OutOfScope); the committer derives the L3.1/L3.2
//! ledger events from the resulting diff. Split into a directory module so each file stays < 400
//! lines (§0.5).

mod interest;
mod promise;
mod thread;

use std::collections::HashSet;
use trpg_model::{InterestSignal, StoryState};

/// The committed, player-visible signals one turn produces, projected for the observer. All
/// fields default-empty/`None` ⇒ a quiet turn is a no-op. Built by the committer (DB side); the
/// observer itself stays pure.
#[derive(Debug, Default, Clone)]
pub struct ObserverSignal {
    /// Thread ids the player ACTIVELY engaged this turn (the positive counterpart of a rejection
    /// — GM/Director proposes; generic). Combined with the persisted positive `player_interests`
    /// to decide thread engagement (required to advance past `Introduced`) AND to evolve the
    /// interest signal (L7.2).
    pub engaged_thread_ids: Vec<String>,
    /// Cumulative facts now player-known. Drives thread fulfillment (known-ratio) + promise
    /// maturity (payoff-candidate support).
    pub known_fact_ids: Vec<String>,
    /// `(promise_id, payoff_event_id)` pairs an explicit committed payoff event paid off this
    /// turn — the authoritative, non-derived path to `PaidOff`.
    pub paid_off: Vec<(String, String)>,
    /// L7.2 — the beat kind the Director committed this turn, if any. Drives the pacing-state
    /// update (tension/relief/escalation history). `None` ⇒ pacing untouched.
    pub committed_beat: Option<trpg_model::story::BeatKind>,
}

/// Compute the engaged-thread set for this turn = the turn's explicit proposals ∪ the persisted
/// positive interest signals (already committed). A rejected interest never counts as engagement.
fn engaged_set(story: &StoryState, signal: &ObserverSignal) -> HashSet<String> {
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
    engaged
}

/// Observe a committed turn: advance thread statuses + promise maturities (L7.1) and evolve
/// interest + pacing (L7.2) from `signal`, returning the updated story and whether anything
/// actually changed (so the committer can skip a no-op upsert). PURE + forward-only + fail-closed.
pub fn observe_story(mut story: StoryState, signal: &ObserverSignal) -> (StoryState, bool) {
    let engaged = engaged_set(&story, signal);
    let known: HashSet<&str> = signal.known_fact_ids.iter().map(String::as_str).collect();

    let mut changed = false;
    changed |= thread::advance_threads(&mut story, &engaged, &known);
    changed |= promise::advance_promises(&mut story, &known, &signal.paid_off);
    // L7.2: interest + pacing evolve from the same committed turn (engagement reuses `engaged`).
    changed |= interest::evolve_interest(&mut story, &signal.engaged_thread_ids);
    changed |= interest::evolve_pacing(&mut story, signal.committed_beat);
    (story, changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::{PlayerInterestSignal, StoryPromise, StoryThread, StoryThreadStatus};

    fn thread(id: &str, status: StoryThreadStatus, facts: &[&str]) -> StoryThread {
        StoryThread {
            thread_id: id.into(),
            status,
            related_fact_ids: facts.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    // Orchestration smoke: a quiet turn is a no-op across all four sub-observers.
    #[test]
    fn observe_quiet_turn_is_noop() {
        let story = StoryState {
            active_threads: vec![thread("thr", StoryThreadStatus::Active, &["f1"])],
            promises: vec![StoryPromise {
                promise_id: "pr".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let (out, c) = observe_story(story.clone(), &ObserverSignal::default());
        assert!(!c);
        assert_eq!(out, story);
    }

    // Engaged set unions turn proposals with persisted positive interest, excluding rejections.
    #[test]
    fn engaged_set_unions_proposals_and_positive_interest() {
        let story = StoryState {
            player_interests: vec![
                PlayerInterestSignal {
                    thread_id: "thr_pos".into(),
                    signal: InterestSignal::Invested,
                    ..Default::default()
                },
                PlayerInterestSignal {
                    thread_id: "thr_rej".into(),
                    signal: InterestSignal::Engaged,
                    rejected: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let signal = ObserverSignal {
            engaged_thread_ids: vec!["thr_turn".into()],
            ..Default::default()
        };
        let engaged = engaged_set(&story, &signal);
        assert!(engaged.contains("thr_turn"));
        assert!(engaged.contains("thr_pos"));
        assert!(!engaged.contains("thr_rej"), "rejected interest is not engagement");
    }
}

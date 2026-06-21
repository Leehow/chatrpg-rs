//! P6.8a — PURE (DB-free) story_state WRITE-loop derivations.
//!
//! This module owns the deterministic transforms that the runtime (System Kernel) commits
//! into the persisted [`StoryState`]. It is the deliverable half of the §宪法④ discipline:
//! the LLM/Director **proposes** (e.g. classifies a player utterance as a rejection, or the
//! engine emits a `PlayerLearnedFact`); the Kernel **commits** the resulting state delta.
//! Everything here is pure — no DB handle, no I/O — so the full §二十四-#13 chain
//! (reject → persist → not-selected) is provable without a live Postgres.
//!
//! Two transforms, both additive and order-stable:
//!
//! 1. [`merge_rejections`] — fold a set of rejection PROPOSALS
//!    ([`PlayerInterestSignal`] with `rejected: true`) into `story.player_interests`. The
//!    persisted `rejected` flag is exactly what the P5.3 selector reads to drop a thread
//!    (`rejected_thread_ids` → dominating railroad penalty). This is the WRITE side that
//!    closes the read/write asymmetry (P5⑤#3): P5 only LOADED `rejected`; nothing produced
//!    it. A proposal whose `thread_id` already has a signal is upgraded in place to
//!    `rejected: true` (idempotent — replaying the same rejection yields the same row).
//!
//! 2. [`apply_thread_opened`] — when a fact first becomes player-known (a `PlayerLearnedFact`
//!    edge), every [`StoryThread`] whose `related_fact_ids` contains that fact gets its
//!    status FLOORED from `Dormant` to `Introduced`. This is a status FLOOR keyed on the
//!    event, NOT advancement: a thread already `Introduced`/`Active`/… is left untouched, and
//!    we never invent a transition rule (P6.8b advancement is OutOfScope — blueprint-undefined).
//!
//! The async committer that loads the story, applies these, and calls `apply_story_proposals`
//! lives in [`crate::director_brief`]; it is gated by `TRPG_STORY_WRITE_LOOP` (default OFF).

use trpg_model::{
    DomainEvent, DomainEventKind, PlayerInterestSignal, StoryState, StoryThreadStatus,
};

/// Flag gate for the whole story_state WRITE loop. Default OFF ⇒ NO story_state writes from
/// the P6.8a path (byte-identical to the P5 baseline, whose LOAD side is untouched). ON ⇒ the
/// rejection-persist + StoryThreadOpened commits run. Mirrors the `env_bool` semantics used by
/// the Director packet flag (`1`/`true`/`yes`/`on`, case-insensitive).
pub fn story_write_loop_enabled() -> bool {
    std::env::var("TRPG_STORY_WRITE_LOOP")
        .ok()
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// Fold rejection PROPOSALS into a loaded story's `player_interests`, returning the updated
/// story. For each proposal flagged `rejected` with a non-empty `thread_id`:
///
/// - if a signal for that `thread_id` already exists, set its `rejected = true` in place
///   (preserving the other fields — we only assert the rejection, never overwrite strength);
/// - otherwise append the proposal verbatim.
///
/// Empty `thread_id` or `rejected == false` proposals are ignored (an un-addressable or
/// non-rejection signal must never gate a thread). Idempotent + order-stable: replaying the
/// same proposal set yields the same `player_interests`. Pure — mutates only the moved `story`.
pub fn merge_rejections(mut story: StoryState, proposals: &[PlayerInterestSignal]) -> StoryState {
    for p in proposals {
        if !p.rejected || p.thread_id.is_empty() {
            continue;
        }
        if let Some(existing) = story
            .player_interests
            .iter_mut()
            .find(|s| s.thread_id == p.thread_id)
        {
            existing.rejected = true;
        } else {
            story.player_interests.push(p.clone());
        }
    }
    story
}

/// Floor every thread whose `related_fact_ids` intersects `newly_known_fact_ids` from
/// `Dormant` to `Introduced` (StoryThreadOpened). Returns the updated story and whether any
/// thread actually changed (so the committer can skip a no-op upsert).
///
/// STATUS FLOOR, not advancement: only `Dormant → Introduced` is applied. A thread already at
/// `Introduced` or beyond is left exactly as-is — we never run a transition rule or invent a
/// higher status (P6.8b advancement = OutOfScope). Idempotent: re-running with the same facts
/// after the floor has been applied changes nothing and reports `false`.
pub fn apply_thread_opened(
    mut story: StoryState,
    newly_known_fact_ids: &[String],
) -> (StoryState, bool) {
    if newly_known_fact_ids.is_empty() {
        return (story, false);
    }
    let known: std::collections::HashSet<&str> =
        newly_known_fact_ids.iter().map(String::as_str).collect();
    let mut changed = false;
    for t in &mut story.active_threads {
        if t.status != StoryThreadStatus::Dormant {
            continue; // floor only lifts Dormant; never advances an already-open thread
        }
        if t.related_fact_ids
            .iter()
            .any(|f| known.contains(f.as_str()))
        {
            t.status = StoryThreadStatus::Introduced;
            changed = true;
        }
    }
    (story, changed)
}

/// Stable DB token for a thread status — a single source for the idempotent event key and the
/// event `data`. Local match (not serde) so a future `#[serde(rename)]` can never silently drift
/// the persisted key. Generic per-status mapping; NOT branched on ruleset/module (§二-⑪).
fn status_token(status: StoryThreadStatus) -> &'static str {
    match status {
        StoryThreadStatus::Dormant => "Dormant",
        StoryThreadStatus::Introduced => "Introduced",
        StoryThreadStatus::Active => "Active",
        StoryThreadStatus::Escalating => "Escalating",
        StoryThreadStatus::ReadyForPayoff => "ReadyForPayoff",
        StoryThreadStatus::Resolved => "Resolved",
        StoryThreadStatus::Abandoned => "Abandoned",
        StoryThreadStatus::Transformed => "Transformed",
    }
}

/// Classify one thread status transition into its ledger event kind (generic, terminal-target
/// first). `None` ⇒ no event (no change). Returns `None` only when `prior == next`.
fn classify_thread_transition(
    prior: Option<StoryThreadStatus>,
    next: StoryThreadStatus,
) -> Option<DomainEventKind> {
    if prior == Some(next) {
        return None; // unchanged thread emits nothing
    }
    Some(match next {
        // terminal/explicit targets win regardless of where we came from
        StoryThreadStatus::Resolved => DomainEventKind::StoryThreadResolved,
        StoryThreadStatus::Dormant => DomainEventKind::StoryThreadDormant,
        // opened: from Dormant (or first-appearance treated as Dormant) into any open status
        _ if prior.unwrap_or(StoryThreadStatus::Dormant) == StoryThreadStatus::Dormant => {
            DomainEventKind::StoryThreadOpened
        }
        // any other forward move among open statuses
        _ => DomainEventKind::StoryThreadAdvanced,
    })
}

/// L3.1 — derive the additive StoryThread ledger events from a committed status diff (PURE, DB-free).
///
/// Compares each thread in `after` against its `before` counterpart (a thread absent in `before`
/// is treated as newly appearing from `Dormant`) and emits ONE [`DomainEvent`] per actual status
/// transition. The story_state snapshot stays the single source of truth (R2 promotion is
/// OutOfScope); these events only add append-only observability — exactly the additive/fail-soft
/// write-through pattern of `ClockAdvanced`.
///
/// Idempotent key (codex pattern, keyed on the RESULTING state): `de_thread_{session}_{thread}_{new}`
/// — replaying the same transition folds to one row; a real move to a different status lands a new
/// row. Order-stable (follows `after.active_threads` order).
pub fn thread_status_events(
    before: &StoryState,
    after: &StoryState,
    session_id: &str,
    turn_id: &str,
) -> Vec<DomainEvent> {
    let prior_status: std::collections::HashMap<&str, StoryThreadStatus> = before
        .active_threads
        .iter()
        .map(|t| (t.thread_id.as_str(), t.status))
        .collect();
    let mut events = Vec::new();
    for t in &after.active_threads {
        let prior = prior_status.get(t.thread_id.as_str()).copied();
        let Some(kind) = classify_thread_transition(prior, t.status) else {
            continue;
        };
        let new_token = status_token(t.status);
        let ev = DomainEvent::new(
            format!("de_thread_{}_{}_{}", session_id, t.thread_id, new_token),
            session_id.to_string(),
            turn_id.to_string(),
            kind,
            serde_json::json!({
                "thread_id": t.thread_id,
                "new_status": new_token,
                "prior_status": prior.map(status_token),
            }),
        );
        events.push(ev);
    }
    events
}

/// Convenience constructor for a structured rejection proposal: the typed signal the
/// GM/Director PRODUCES once it has classified a player utterance as a thread refusal. The
/// natural-language → proposal step is LLM-mediated (propose-only); this is just the typed
/// carrier the deterministic commit path consumes.
pub fn rejection_proposal(thread_id: impl Into<String>) -> PlayerInterestSignal {
    PlayerInterestSignal {
        thread_id: thread_id.into(),
        rejected: true,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::{InterestSignal, StoryThread};

    fn thread(id: &str, status: StoryThreadStatus, facts: &[&str]) -> StoryThread {
        StoryThread {
            thread_id: id.into(),
            status,
            related_fact_ids: facts.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    // A fresh rejection proposal for a thread with no prior signal appends a rejected signal.
    #[test]
    fn merge_rejections_appends_new_signal() {
        let story = StoryState::default();
        let out = merge_rejections(story, &[rejection_proposal("thr_x")]);
        assert_eq!(out.player_interests.len(), 1);
        assert!(out.player_interests[0].rejected);
        assert_eq!(out.player_interests[0].thread_id, "thr_x");
    }

    // An existing non-rejected signal for the thread is upgraded in place (not duplicated),
    // preserving its other fields (here `signal`/`strength`).
    #[test]
    fn merge_rejections_upgrades_existing_in_place() {
        let story = StoryState {
            player_interests: vec![PlayerInterestSignal {
                thread_id: "thr_x".into(),
                signal: InterestSignal::Engaged,
                strength: 0.7,
                rejected: false,
                ..Default::default()
            }],
            ..Default::default()
        };
        let out = merge_rejections(story, &[rejection_proposal("thr_x")]);
        assert_eq!(out.player_interests.len(), 1, "no duplicate signal");
        assert!(out.player_interests[0].rejected, "upgraded to rejected");
        assert_eq!(
            out.player_interests[0].signal,
            InterestSignal::Engaged,
            "other fields preserved"
        );
        assert_eq!(out.player_interests[0].strength, 0.7);
    }

    // Idempotent: replaying the same rejection yields the same single rejected signal.
    #[test]
    fn merge_rejections_is_idempotent() {
        let p = [rejection_proposal("thr_x")];
        let once = merge_rejections(StoryState::default(), &p);
        let twice = merge_rejections(once.clone(), &p);
        assert_eq!(once, twice);
        assert_eq!(twice.player_interests.len(), 1);
    }

    // Empty thread_id or non-rejection proposals are ignored (never gate a thread).
    #[test]
    fn merge_rejections_ignores_empty_and_non_rejection() {
        let proposals = vec![
            PlayerInterestSignal {
                thread_id: String::new(),
                rejected: true,
                ..Default::default()
            },
            PlayerInterestSignal {
                thread_id: "thr_engaged".into(),
                rejected: false,
                ..Default::default()
            },
        ];
        let out = merge_rejections(StoryState::default(), &proposals);
        assert!(out.player_interests.is_empty());
    }

    // StoryThreadOpened: a Dormant thread whose related fact becomes player-known floors to
    // Introduced; an unrelated Dormant thread and an already-open thread are untouched.
    #[test]
    fn apply_thread_opened_floors_only_matching_dormant() {
        let story = StoryState {
            active_threads: vec![
                thread("thr_match", StoryThreadStatus::Dormant, &["fact_1"]),
                thread("thr_other", StoryThreadStatus::Dormant, &["fact_9"]),
                thread("thr_active", StoryThreadStatus::Active, &["fact_1"]),
            ],
            ..Default::default()
        };
        let (out, changed) = apply_thread_opened(story, &["fact_1".to_string()]);
        assert!(changed);
        assert_eq!(out.active_threads[0].status, StoryThreadStatus::Introduced);
        assert_eq!(
            out.active_threads[1].status,
            StoryThreadStatus::Dormant,
            "unrelated thread untouched"
        );
        assert_eq!(
            out.active_threads[2].status,
            StoryThreadStatus::Active,
            "already-open thread is NOT advanced (P6.8b OutOfScope)"
        );
    }

    // Idempotent + no-op reporting: re-running after the floor reports no change.
    #[test]
    fn apply_thread_opened_idempotent_no_change() {
        let story = StoryState {
            active_threads: vec![thread("thr_match", StoryThreadStatus::Dormant, &["fact_1"])],
            ..Default::default()
        };
        let (out, changed1) = apply_thread_opened(story, &["fact_1".to_string()]);
        assert!(changed1);
        let (out2, changed2) = apply_thread_opened(out.clone(), &["fact_1".to_string()]);
        assert!(!changed2, "second run is a no-op");
        assert_eq!(out, out2);
    }

    // ── L3.1 thread_status_events: generic status-diff → ledger events ──────────────────────
    #[test]
    fn thread_events_opened_on_dormant_to_introduced() {
        let before = StoryState {
            active_threads: vec![thread("thr_x", StoryThreadStatus::Dormant, &["f1"])],
            ..Default::default()
        };
        let (after, _) = apply_thread_opened(before.clone(), &["f1".to_string()]);
        let events = thread_status_events(&before, &after, "sess1", "t1");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, DomainEventKind::StoryThreadOpened);
        // idempotent key is on the RESULTING status.
        assert_eq!(events[0].event_id, "de_thread_sess1_thr_x_Introduced");
        assert_eq!(events[0].data["new_status"], "Introduced");
        assert_eq!(events[0].data["prior_status"], "Dormant");
    }

    #[test]
    fn thread_events_classify_advanced_resolved_dormant() {
        let before = StoryState {
            active_threads: vec![
                thread("thr_adv", StoryThreadStatus::Active, &[]),
                thread("thr_res", StoryThreadStatus::ReadyForPayoff, &[]),
                thread("thr_dorm", StoryThreadStatus::Active, &[]),
                thread("thr_same", StoryThreadStatus::Active, &[]),
            ],
            ..Default::default()
        };
        let after = StoryState {
            active_threads: vec![
                thread("thr_adv", StoryThreadStatus::Escalating, &[]),
                thread("thr_res", StoryThreadStatus::Resolved, &[]),
                thread("thr_dorm", StoryThreadStatus::Dormant, &[]),
                thread("thr_same", StoryThreadStatus::Active, &[]),
            ],
            ..Default::default()
        };
        let events = thread_status_events(&before, &after, "s", "t");
        // thr_same is unchanged ⇒ no event; the other three each emit their classified kind.
        assert_eq!(events.len(), 3);
        let by_id: std::collections::HashMap<_, _> = events
            .iter()
            .map(|e| (e.data["thread_id"].as_str().unwrap().to_string(), e.kind))
            .collect();
        assert_eq!(by_id["thr_adv"], DomainEventKind::StoryThreadAdvanced);
        assert_eq!(by_id["thr_res"], DomainEventKind::StoryThreadResolved);
        assert_eq!(by_id["thr_dorm"], DomainEventKind::StoryThreadDormant);
    }

    #[test]
    fn thread_events_new_thread_appears_as_opened() {
        let before = StoryState::default();
        let after = StoryState {
            active_threads: vec![thread("thr_new", StoryThreadStatus::Active, &[])],
            ..Default::default()
        };
        let events = thread_status_events(&before, &after, "s", "t");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, DomainEventKind::StoryThreadOpened);
        assert!(events[0].data["prior_status"].is_null());
    }

    #[test]
    fn thread_events_no_change_emits_nothing() {
        let story = StoryState {
            active_threads: vec![thread("thr_x", StoryThreadStatus::Active, &["f1"])],
            ..Default::default()
        };
        assert!(thread_status_events(&story, &story, "s", "t").is_empty());
    }

    // No newly-known facts ⇒ no change.
    #[test]
    fn apply_thread_opened_empty_facts_no_change() {
        let story = StoryState {
            active_threads: vec![thread("thr_match", StoryThreadStatus::Dormant, &["fact_1"])],
            ..Default::default()
        };
        let (out, changed) = apply_thread_opened(story.clone(), &[]);
        assert!(!changed);
        assert_eq!(out, story);
    }
}

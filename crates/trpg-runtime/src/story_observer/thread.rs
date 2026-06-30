//! L7.1 — thread-status ADVANCEMENT along the open ladder. Knowledge alone only OPENS (that is
//! [`crate::story_write::apply_thread_opened`]'s job); ENGAGEMENT drives advancement here.

use std::collections::HashSet;
use trpg_model::{StoryState, StoryThreadStatus};

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

/// Advance every opened, engaged thread toward its momentum target (forward-only). Dormant
/// (rank 0 — opening is `apply_thread_opened`'s job) and terminal (rank 5) threads are skipped.
/// Returns whether any thread changed.
pub(super) fn advance_threads(
    story: &mut StoryState,
    engaged: &HashSet<String>,
    known: &HashSet<&str>,
) -> bool {
    let mut changed = false;
    for t in &mut story.active_threads {
        let rank = thread_rank(t.status);
        if rank == 0 || rank >= 5 {
            continue;
        }
        let target =
            target_thread_status(&t.related_fact_ids, engaged.contains(&t.thread_id), known);
        if thread_rank(target) > rank {
            t.status = target; // forward-only
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::StoryThread;

    fn thread(id: &str, status: StoryThreadStatus, facts: &[&str]) -> StoryThread {
        StoryThread {
            thread_id: id.into(),
            status,
            related_fact_ids: facts.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn run(story: StoryState, engaged: &[&str], known: &[&str]) -> (StoryState, bool) {
        let mut story = story;
        let eng: HashSet<String> = engaged.iter().map(|s| s.to_string()).collect();
        let kn: HashSet<&str> = known.iter().copied().collect();
        let changed = advance_threads(&mut story, &eng, &kn);
        (story, changed)
    }

    // The acceptance ladder: Introduced → Active → ReadyForPayoff across engaged turns.
    #[test]
    fn l71_thread_advances_introduced_active_ready_for_payoff() {
        let story = StoryState {
            active_threads: vec![thread("thr", StoryThreadStatus::Introduced, &["f1", "f2"])],
            ..Default::default()
        };
        let (story, c1) = run(story, &["thr"], &[]); // engaged, 0 known ⇒ Active
        assert!(c1);
        assert_eq!(story.active_threads[0].status, StoryThreadStatus::Active);
        let (story, c2) = run(story, &["thr"], &["f1", "f2"]); // all known ⇒ ReadyForPayoff
        assert!(c2);
        assert_eq!(
            story.active_threads[0].status,
            StoryThreadStatus::ReadyForPayoff
        );
        let (story, c3) = run(story, &["thr"], &["f1", "f2"]); // idempotent at ceiling
        assert!(!c3);
        assert_eq!(
            story.active_threads[0].status,
            StoryThreadStatus::ReadyForPayoff
        );
    }

    #[test]
    fn l71_partial_known_ratio_yields_escalating() {
        let story = StoryState {
            active_threads: vec![thread("thr", StoryThreadStatus::Active, &["f1", "f2"])],
            ..Default::default()
        };
        let (story, c) = run(story, &["thr"], &["f1"]); // ratio .5 ⇒ Escalating
        assert!(c);
        assert_eq!(
            story.active_threads[0].status,
            StoryThreadStatus::Escalating
        );
    }

    #[test]
    fn l71_knowledge_without_engagement_does_not_advance() {
        let story = StoryState {
            active_threads: vec![thread("thr", StoryThreadStatus::Introduced, &["f1", "f2"])],
            ..Default::default()
        };
        let (story, c) = run(story, &[], &["f1", "f2"]); // all known but NOT engaged
        assert!(
            !c,
            "fact-knowledge alone must not advance an unengaged thread"
        );
        assert_eq!(
            story.active_threads[0].status,
            StoryThreadStatus::Introduced
        );
    }

    #[test]
    fn l71_dormant_and_terminal_threads_untouched() {
        let story = StoryState {
            active_threads: vec![
                thread("thr_dorm", StoryThreadStatus::Dormant, &["f1"]),
                thread("thr_done", StoryThreadStatus::Resolved, &["f1"]),
            ],
            ..Default::default()
        };
        let (story, c) = run(story, &["thr_dorm", "thr_done"], &["f1"]);
        assert!(!c);
        assert_eq!(story.active_threads[0].status, StoryThreadStatus::Dormant);
        assert_eq!(story.active_threads[1].status, StoryThreadStatus::Resolved);
    }
}

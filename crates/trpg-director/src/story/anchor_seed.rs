//! L8.2 — seed proposal `StoryThread`s from module [`NarrativeAnchor`]s.
//!
//! L8.1 extracted deterministic `NarrativeAnchor`s as director MATERIAL (not a script). This
//! module lets the Director PROMOTE a latent [`NarrativeAnchorKind::PotentialThread`] anchor into a
//! proposal [`StoryThread`] so the existing 12-term scorer (`build_director_brief_packet`) can pick
//! it up alongside the live threads — closing the "anchors never become selectable" gap.
//!
//! Discipline (§0):
//! - **Proposal-only.** A seeded thread is a *proposal* — it carries
//!   [`StoryThreadOrigin::ModuleAnchor`] + the source `module_anchor` id, commits nothing, and the
//!   crate stays DB-free.
//! - **PURE / deterministic / idempotent.** [`seed_threads_from_anchors`] keys only on the anchor's
//!   stable id, never invents fields, and skips anchors already represented in `existing` — so
//!   re-running over an already-seeded story yields nothing new.
//! - **Generic.** No `ruleset_id`/`module_id` name-branch (§二-⑪); we key only on
//!   [`NarrativeAnchorKind`].
//! - **Fail-closed + flag-gated.** [`anchor_seed_enabled`] reads `TRPG_DIRECTOR_ANCHOR_SEED`
//!   (default OFF). [`augment_story_with_anchor_seeds`] with empty anchors is an exact clone, so the
//!   OFF / no-anchor path is byte-identical to the snapshot path.

use trpg_model::{
    NarrativeAnchor, NarrativeAnchorKind, StoryState, StoryThread, StoryThreadOrigin,
    StoryThreadStatus,
};

/// Flag reader for the anchor-seeding path. Default OFF (mirrors `scene_plan_enabled` /
/// `campaign_plan_enabled`): only `"1"` enables seeding so OFF stays byte-identical.
pub fn anchor_seed_enabled() -> bool {
    std::env::var("TRPG_DIRECTOR_ANCHOR_SEED")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// Deterministic thread id for an anchor-seeded proposal thread. Stable & unique because anchor ids
/// are themselves deterministic (`anchor_thread_{node}_{to}` etc.).
fn seeded_thread_id(anchor: &NarrativeAnchor) -> String {
    format!("seed_{}", anchor.anchor_id)
}

/// Whether `existing` already carries a thread seeded from `anchor` (by `module_anchor` id or a
/// colliding `thread_id`). Used to keep seeding idempotent.
fn already_seeded(anchor: &NarrativeAnchor, existing: &[StoryThread]) -> bool {
    let id = seeded_thread_id(anchor);
    existing
        .iter()
        .any(|t| t.thread_id == id || t.module_anchor.as_deref() == Some(anchor.anchor_id.as_str()))
}

/// PURE: promote [`NarrativeAnchorKind::PotentialThread`] anchors into proposal [`StoryThread`]s,
/// skipping any anchor already represented in `existing` (idempotent). Order-stable, deterministic.
///
/// Only `PotentialThread` anchors seed threads — they are the kind the L8.1 doc calls "a latent
/// storyline the Director could promote into a `StoryThread`". Other kinds (scene function, reveal
/// candidate, climax condition…) are material for other lanes and are NOT turned into threads here.
///
/// The seeded thread starts at [`StoryThreadStatus::Introduced`] (a freshly surfaced, not-yet-live
/// thread — `status_relevance` 0.5, so it is selectable but does not outrank an escalating live
/// thread) with the anchor's `related_ids` as `related_fact_ids` (giving it causal validity in the
/// scorer). All pressure signals (urgency/momentum/player_interest) stay at the 0.0 default — a
/// proposal earns pressure only once the player pulls on it.
pub fn seed_threads_from_anchors(
    anchors: &[NarrativeAnchor],
    existing: &[StoryThread],
) -> Vec<StoryThread> {
    let mut out = Vec::new();
    let mut seeded_ids = std::collections::HashSet::new();
    for anchor in anchors {
        if anchor.kind != NarrativeAnchorKind::PotentialThread {
            continue;
        }
        if already_seeded(anchor, existing) {
            continue;
        }
        let thread_id = seeded_thread_id(anchor);
        // Dedup within this batch too (two anchors can't yield the same thread_id, but stay safe).
        if !seeded_ids.insert(thread_id.clone()) {
            continue;
        }
        out.push(StoryThread {
            thread_id,
            premise: anchor.summary.clone(),
            title: anchor.summary.clone(),
            status: StoryThreadStatus::Introduced,
            origin: StoryThreadOrigin::ModuleAnchor,
            module_anchor: Some(anchor.anchor_id.clone()),
            related_fact_ids: anchor.related_ids.clone(),
            ..Default::default()
        });
    }
    out
}

/// PURE: return a clone of `story` with anchor-seeded proposal threads appended to
/// `active_threads`. Fail-closed: empty anchors (the OFF / no-anchor path) ⇒ an exact clone, so the
/// scorer sees a byte-identical [`StoryState`]. Commits nothing (proposal-only).
pub fn augment_story_with_anchor_seeds(
    story: &StoryState,
    anchors: &[NarrativeAnchor],
) -> StoryState {
    let mut augmented = story.clone();
    let seeded = seed_threads_from_anchors(anchors, &augmented.active_threads);
    augmented.active_threads.extend(seeded);
    augmented
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{build_director_brief_packet, DirectorMode};
    use trpg_model::WorldReactionCandidate;

    fn potential_thread_anchor(id: &str, to: &str) -> NarrativeAnchor {
        NarrativeAnchor {
            anchor_id: format!("anchor_thread_{id}_{to}"),
            kind: NarrativeAnchorKind::PotentialThread,
            summary: format!("顺着线索通向 {to}"),
            related_ids: vec![to.to_string()],
            source_node_id: Some(id.to_string()),
        }
    }

    fn generic_candidate(npc: &str) -> WorldReactionCandidate {
        WorldReactionCandidate {
            npc_id: npc.into(),
            stance: String::new(),
            emotional_state: String::new(),
            urgency: 0.7,
            feasibility: 0.7,
            risk: 0.3,
            action_intent: None,
            knowledge_basis: vec![],
            source_event_ids: vec![format!("evt_{npc}")],
        }
    }

    #[test]
    fn only_potential_thread_anchors_seed() {
        let anchors = vec![
            potential_thread_anchor("s1", "s2"),
            NarrativeAnchor {
                anchor_id: "anchor_scene_s1".into(),
                kind: NarrativeAnchorKind::SceneFunction,
                ..Default::default()
            },
            NarrativeAnchor {
                anchor_id: "anchor_climax_s9".into(),
                kind: NarrativeAnchorKind::ClimaxCondition,
                ..Default::default()
            },
        ];
        let seeded = seed_threads_from_anchors(&anchors, &[]);
        assert_eq!(
            seeded.len(),
            1,
            "only the PotentialThread anchor seeds a thread"
        );
        let t = &seeded[0];
        assert_eq!(t.thread_id, "seed_anchor_thread_s1_s2");
        assert_eq!(t.origin, StoryThreadOrigin::ModuleAnchor);
        assert_eq!(t.module_anchor.as_deref(), Some("anchor_thread_s1_s2"));
        assert_eq!(t.status, StoryThreadStatus::Introduced);
        assert_eq!(t.related_fact_ids, vec!["s2".to_string()]);
    }

    #[test]
    fn seeding_is_idempotent() {
        let anchors = vec![potential_thread_anchor("s1", "s2")];
        let once = seed_threads_from_anchors(&anchors, &[]);
        assert_eq!(once.len(), 1);
        // Re-running with the already-seeded thread in `existing` yields nothing new.
        let twice = seed_threads_from_anchors(&anchors, &once);
        assert!(
            twice.is_empty(),
            "an already-seeded anchor must not re-seed"
        );
        // Also idempotent through augment (the live merge path).
        let story = augment_story_with_anchor_seeds(&StoryState::default(), &anchors);
        let again = augment_story_with_anchor_seeds(&story, &anchors);
        assert_eq!(story.active_threads.len(), again.active_threads.len());
    }

    #[test]
    fn empty_anchors_is_exact_clone() {
        let mut story = StoryState::default();
        story.active_threads.push(StoryThread {
            thread_id: "t.live".into(),
            premise: "the live thread".into(),
            status: StoryThreadStatus::Active,
            ..Default::default()
        });
        let augmented = augment_story_with_anchor_seeds(&story, &[]);
        assert_eq!(
            serde_json::to_vec(&story).unwrap(),
            serde_json::to_vec(&augmented).unwrap(),
            "no anchors ⇒ byte-identical story (OFF byte-equal)"
        );
    }

    #[test]
    fn anchor_seeded_thread_becomes_selectable_in_scorer() {
        // The acceptance: an anchor-seeded thread is selectable by the 12-term scorer. With an empty
        // base story, the seeded thread is the only candidate ⇒ it becomes primary.
        let anchors = vec![potential_thread_anchor("s1", "s2")];
        let augmented = augment_story_with_anchor_seeds(&StoryState::default(), &anchors);
        let candidates = vec![generic_candidate("npc.broker")];
        let plan = build_director_brief_packet(
            DirectorMode::OnDemand,
            &candidates,
            &augmented,
            None,
            None,
            &[],
            &[],
            "pc.current",
        );
        assert_eq!(
            plan.primary_thread_id.as_deref(),
            Some("seed_anchor_thread_s1_s2"),
            "the anchor-seeded thread must be selectable as primary"
        );
    }

    #[test]
    fn seeded_thread_does_not_outrank_an_escalating_live_thread() {
        // A proposal thread (Introduced, no pressure) must not steamroll a live escalating thread —
        // seeding adds an option, it does not railroad.
        let anchors = vec![potential_thread_anchor("s1", "s2")];
        let mut story = StoryState::default();
        story.active_threads.push(StoryThread {
            thread_id: "t.live".into(),
            premise: "the escalating live thread".into(),
            status: StoryThreadStatus::Escalating,
            urgency: 0.9,
            momentum: 0.8,
            player_interest: 0.9,
            participant_ids: vec!["npc.broker".into()],
            related_fact_ids: vec!["fact.x".into()],
            ..Default::default()
        });
        let augmented = augment_story_with_anchor_seeds(&story, &anchors);
        let candidates = vec![generic_candidate("npc.broker")];
        let plan = build_director_brief_packet(
            DirectorMode::OnDemand,
            &candidates,
            &augmented,
            None,
            None,
            &[],
            &[],
            "pc.current",
        );
        assert_eq!(plan.primary_thread_id.as_deref(), Some("t.live"));
        // …but the seeded thread is now in the pool (selectable as a secondary option).
        assert!(
            augmented
                .active_threads
                .iter()
                .any(|t| t.thread_id == "seed_anchor_thread_s1_s2"),
            "the seeded proposal thread joins the candidate pool"
        );
    }
}

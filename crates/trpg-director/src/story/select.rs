//! P5.3 — pure Director brief-packet selection.
//!
//! [`build_director_brief_packet`] scores the live story threads with the blueprint
//! §11.7 12-term linear formula, picks a primary + secondary threads, selects the World
//! candidates that actually belong to the passed pool, and computes a fail-closed reveal
//! set. It commits nothing (§11.8) — output is a typed [`DirectorPlan`] only.
//!
//! Four pure invariants, each locked by a guard test in this file:
//! 1. **pool-filter (§二十四-#3)**: `selected_world_candidates` retains ONLY canonical keys
//!    present in the passed pool. Keys that canonicalize ambiguously (e.g. empty
//!    `source_event_ids` colliding for the same `{npc_id, action_kind}`) are dropped —
//!    we never emit a key we cannot uniquely resolve back to one pool member (codex fold
//!    #1, fail-closed).
//! 2. **reveal gating (§二十四-#10/#8) — fail-closed**: `reveal_candidate_fact_ids =
//!    gm_truth ∖ player_known`, BUT if either input is `None` ⇒ EMPTY reveal (a DB
//!    read-failure must NEVER over-reveal). Only ids ∈ `gm_truth` may ever appear.
//! 3. **pool-empty → WorldQuery (§11.7)**: an empty candidate pool yields no selection and
//!    a `WorldQuery` instead — out-of-pool directions are *asked for*, never invented.
//! 4. **anti-railroad (§二十四-#13)**: a thread in `rejected_thread_ids` carries a
//!    dominating `railroading_risk` penalty so it can never become primary/secondary.
//!
//! ## GENERIC scoring weights (FRAMEWORK §1, codex fold #6)
//! The weight vector below is a single UNIVERSAL default — NOT branched on
//! `ruleset_id`/`module_id` (§二-⑪ forbidden) and NOT read from `DirectorModuleConfig`
//! (no weight fields there → reading it would be scope creep). A future data-driven
//! override hook can multiply/replace these once the config schema grows such a field;
//! until then these GENERIC defaults are the single source.

use std::collections::HashMap;

use trpg_model::{
    BeatKind, DirectorPlan, StoryState, StoryThread, StoryThreadStatus, WorldCandidateRef,
    WorldQuery, WorldQueryIntent, WorldReactionCandidate,
};

use crate::DirectorMode;

// ── GENERIC default scoring weights (§11.7 12-term linear score) ─────────────────────
// Positive terms pull a thread toward selection; negative terms push it away. These are
// universal defaults, intentionally simple and ruleset-agnostic. TODO(data-driven): when
// a weight surface exists on the module/system config, allow overriding this vector here
// (multiplicatively) — until that field exists we do NOT read config (scope creep).
const W_CAUSAL_VALIDITY: f32 = 1.0;
const W_CURRENT_RELEVANCE: f32 = 1.2;
const W_PLAYER_INTEREST: f32 = 1.5;
const W_URGENCY: f32 = 1.3;
const W_PAYOFF_VALUE: f32 = 1.1;
const W_CHARACTER_RELEVANCE: f32 = 0.8;
const W_GENRE_FIT: f32 = 0.5;
const W_NOVELTY: f32 = 0.7;
const W_REPETITION: f32 = 0.9;
const W_SPOILER_RISK: f32 = 1.0;
const W_RAILROADING_RISK: f32 = 1.4;
const W_PLAUSIBILITY_COST: f32 = 0.6;

/// A dominating penalty applied to a rejected thread's `railroading_risk` term so it can
/// never outscore a non-rejected thread (§二十四-#13). Large enough to swamp every
/// positive term at their max (Σ positives < this).
const REJECTED_RAILROAD_PENALTY: f32 = 1000.0;

/// Build the Director's typed brief packet for one turn (PURE, commit-nothing).
///
/// `player_known` / `gm_truth` are `Option` so the caller signals a DB read failure as
/// `None` — which fail-closes the reveal set to empty rather than over-revealing.
/// `mode == Disabled` returns a minimal no-op plan (the caller won't render it); the
/// function stays total.
#[allow(clippy::too_many_arguments)]
pub fn build_director_brief_packet(
    mode: DirectorMode,
    candidates: &[WorldReactionCandidate],
    story: &StoryState,
    player_known: Option<&[String]>,
    gm_truth: Option<&[String]>,
    _spotlights: &[trpg_model::SpotlightState],
    rejected_thread_ids: &[String],
    acting_actor_id: &str,
) -> DirectorPlan {
    if mode == DirectorMode::Disabled {
        return DirectorPlan::default();
    }

    // Canonicalize the pool into a multiset of keys; an ambiguous (colliding) key is one
    // that more than one pool member canonicalizes to — we drop those from selection
    // because we can't uniquely resolve the key back to a single candidate (codex fold #1,
    // fail-closed). `uniquely_resolvable` holds only keys that map to exactly one member.
    let uniquely_resolvable = unambiguous_pool_keys(candidates);

    // Reveal gating — fail-closed: any None input ⇒ empty reveal (§二十四-#10/#8).
    let reveal_candidate_fact_ids = compute_reveal(player_known, gm_truth);

    // Pool empty ⇒ ask the World instead of inventing a selection (§11.7).
    if candidates.is_empty() {
        return DirectorPlan {
            beat_kind: BeatKind::Respond,
            world_query: Some(empty_pool_query(&reveal_candidate_fact_ids)),
            reveal_candidate_fact_ids,
            ..Default::default()
        };
    }

    // Score every live thread; rejected threads take a dominating railroad penalty so they
    // never win (§二十四-#13).
    let mut scored: Vec<(f32, &StoryThread)> = story
        .active_threads
        .iter()
        .map(|t| (score_thread(t, story, rejected_thread_ids), t))
        .collect();
    // Deterministic order: score desc, then thread_id asc as a stable tiebreak.
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.thread_id.cmp(&b.1.thread_id))
    });

    let rejected: std::collections::HashSet<&str> =
        rejected_thread_ids.iter().map(String::as_str).collect();

    let mut primary_thread_id = None;
    let mut secondary_thread_ids = Vec::new();
    for (_, thread) in &scored {
        if rejected.contains(thread.thread_id.as_str()) {
            continue; // never spotlight a rejected thread
        }
        if primary_thread_id.is_none() {
            primary_thread_id = Some(thread.thread_id.clone());
        } else if secondary_thread_ids.len() < 2 {
            secondary_thread_ids.push(thread.thread_id.clone());
        }
    }

    // Select pool candidates relevant to the spotlighted threads, retaining only keys that
    // are uniquely resolvable in the pool (pool-filter, §二十四-#3 + codex fold #1).
    let spotlighted: Vec<&StoryThread> = scored
        .iter()
        .map(|(_, t)| *t)
        .filter(|t| {
            Some(t.thread_id.clone()) == primary_thread_id
                || secondary_thread_ids.contains(&t.thread_id)
        })
        .collect();

    let selected_world_candidates =
        select_candidates(candidates, &uniquely_resolvable, &spotlighted);

    let focus_actor_ids = selected_world_candidates
        .iter()
        .map(|r| r.npc_id.clone())
        .collect();

    DirectorPlan {
        beat_kind: choose_beat_kind(story, &reveal_candidate_fact_ids),
        selected_world_candidates,
        primary_thread_id,
        secondary_thread_ids,
        focus_actor_ids,
        reveal_candidate_fact_ids,
        // Generic STRUCTURAL strings only — never leaked secret prose (codex fold #2/#3).
        dramatic_function: "advance_primary_thread".into(),
        desired_change: "shift_situation".into(),
        spotlight_target: super::fallback::pick_spotlight_target(_spotlights, acting_actor_id),
        ..Default::default()
    }
}

/// Canonical keys that resolve to EXACTLY ONE pool member. Keys reached by >1 candidate
/// (ambiguous collision, e.g. empty `source_event_ids`) are excluded — fail-closed.
fn unambiguous_pool_keys(candidates: &[WorldReactionCandidate]) -> HashMap<WorldCandidateRef, usize> {
    let mut counts: HashMap<WorldCandidateRef, usize> = HashMap::new();
    for c in candidates {
        *counts.entry(WorldCandidateRef::from_candidate(c)).or_insert(0) += 1;
    }
    counts.retain(|_, n| *n == 1);
    counts
}

/// `gm_truth ∖ player_known`, fail-closed: either input `None` ⇒ empty; result retains
/// only ids that are in `gm_truth` (never reveal a fact not known-true to the GM).
fn compute_reveal(player_known: Option<&[String]>, gm_truth: Option<&[String]>) -> Vec<String> {
    let (Some(known), Some(truth)) = (player_known, gm_truth) else {
        return Vec::new();
    };
    let known_set: std::collections::HashSet<&str> = known.iter().map(String::as_str).collect();
    let mut out: Vec<String> = truth
        .iter()
        .filter(|id| !known_set.contains(id.as_str()))
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The WorldQuery emitted when the candidate pool is empty: ask whether a fitting NPC /
/// thread exists rather than inventing one. Carries only fact ids as constraints (no
/// prose). If there's a reveal candidate, prefer an NPC who could plausibly know it.
fn empty_pool_query(reveal_fact_ids: &[String]) -> WorldQuery {
    let (intent, constraint_fact_ids) = if reveal_fact_ids.is_empty() {
        (WorldQueryIntent::ExistsMotivatedNpc, Vec::new())
    } else {
        (WorldQueryIntent::ExistsKnowingNpc, reveal_fact_ids.to_vec())
    };
    WorldQuery {
        intent,
        // Generic structural question — no secret prose.
        question: "no_candidate_in_pool".into(),
        constraint_fact_ids,
    }
}

/// Select pool candidates whose NPC participates in a spotlighted thread, retaining only
/// uniquely-resolvable canonical keys. If no thread is spotlighted (e.g. empty story but
/// non-empty pool), select all uniquely-resolvable candidates so the turn still has actors.
fn select_candidates(
    candidates: &[WorldReactionCandidate],
    resolvable: &HashMap<WorldCandidateRef, usize>,
    spotlighted: &[&StoryThread],
) -> Vec<WorldCandidateRef> {
    let relevant_npc: std::collections::HashSet<&str> = spotlighted
        .iter()
        .flat_map(|t| t.participant_ids.iter().map(String::as_str))
        .collect();
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for c in candidates {
        let key = WorldCandidateRef::from_candidate(c);
        if !resolvable.contains_key(&key) {
            continue; // ambiguous / not uniquely in pool → fail-closed drop
        }
        let npc_relevant = relevant_npc.is_empty() || relevant_npc.contains(c.npc_id.as_str());
        if npc_relevant && seen.insert(key.clone()) {
            out.push(key);
        }
    }
    out
}

/// The §11.7 12-term linear score for one thread. Rejected threads take a dominating
/// railroad penalty (§二十四-#13). All `StoryThread` score inputs are normalized 0..=1.
fn score_thread(t: &StoryThread, story: &StoryState, rejected_thread_ids: &[String]) -> f32 {
    let causal_validity = if t.related_fact_ids.is_empty() && t.participant_ids.is_empty() {
        0.0
    } else {
        1.0
    };
    let current_relevance = status_relevance(t.status);
    let player_interest = t.player_interest;
    let urgency = t.urgency;
    let payoff_value = ripe_payoff_signal(t, story);
    let character_relevance = if t.participant_ids.is_empty() { 0.0 } else { 0.6 };
    let genre_fit = 0.5; // GENERIC neutral prior (no ruleset branch).
    let novelty = novelty_signal(t, story);
    let repetition = 1.0 - novelty;
    let spoiler_risk = 0.0; // structural plan carries no spoiler; reveal-gating handles it.
    let mut railroading_risk = 1.0 - t.player_interest;
    if rejected_thread_ids.iter().any(|r| r == &t.thread_id) {
        railroading_risk += REJECTED_RAILROAD_PENALTY;
    }
    let plausibility_cost = 1.0 - t.momentum;

    W_CAUSAL_VALIDITY * causal_validity
        + W_CURRENT_RELEVANCE * current_relevance
        + W_PLAYER_INTEREST * player_interest
        + W_URGENCY * urgency
        + W_PAYOFF_VALUE * payoff_value
        + W_CHARACTER_RELEVANCE * character_relevance
        + W_GENRE_FIT * genre_fit
        + W_NOVELTY * novelty
        - W_REPETITION * repetition
        - W_SPOILER_RISK * spoiler_risk
        - W_RAILROADING_RISK * railroading_risk
        - W_PLAUSIBILITY_COST * plausibility_cost
}

/// Map thread lifecycle to a current-relevance prior (0..=1). `Dormant`/terminal states
/// read low; `Active`/`Escalating`/`ReadyForPayoff` read high.
fn status_relevance(status: StoryThreadStatus) -> f32 {
    match status {
        StoryThreadStatus::Dormant => 0.1,
        StoryThreadStatus::Introduced => 0.5,
        StoryThreadStatus::Active => 0.8,
        StoryThreadStatus::Escalating => 1.0,
        StoryThreadStatus::ReadyForPayoff => 0.95,
        StoryThreadStatus::Resolved
        | StoryThreadStatus::Abandoned
        | StoryThreadStatus::Transformed => 0.0,
    }
}

/// How much a mature, ripe promise tied to this thread's facts argues for payoff (0..=1).
fn ripe_payoff_signal(t: &StoryThread, story: &StoryState) -> f32 {
    story
        .promises
        .iter()
        .filter(|p| {
            p.payoff_candidate_fact_ids
                .iter()
                .any(|f| t.related_fact_ids.contains(f))
        })
        .map(|p| p.maturity)
        .fold(0.0_f32, f32::max)
}

/// Novelty: lower when this thread was the subject of a recent beat. 1.0 when untouched.
fn novelty_signal(t: &StoryThread, story: &StoryState) -> f32 {
    let recent_hits = story
        .recent_beats
        .iter()
        .filter(|b| b.thread_id == t.thread_id)
        .count();
    (1.0 - 0.25 * recent_hits as f32).clamp(0.0, 1.0)
}

/// Choose a structural beat kind. A non-empty reveal set argues for `Reveal`; otherwise a
/// plain `Respond` (fail-closed — never invents escalation).
fn choose_beat_kind(_story: &StoryState, reveal: &[String]) -> BeatKind {
    if reveal.is_empty() {
        BeatKind::Respond
    } else {
        BeatKind::Reveal
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::{NpcActionIntent, NpcActionKind, PlayerInterestSignal};

    fn cand(npc: &str, events: Vec<&str>, action: Option<NpcActionKind>) -> WorldReactionCandidate {
        WorldReactionCandidate {
            npc_id: npc.into(),
            stance: String::new(),
            emotional_state: String::new(),
            urgency: 0.0,
            feasibility: 0.0,
            risk: 0.0,
            action_intent: action.map(|kind| NpcActionIntent {
                kind,
                target_ref: None,
                description: String::new(),
            }),
            knowledge_basis: vec![],
            source_event_ids: events.into_iter().map(String::from).collect(),
        }
    }

    fn thread(id: &str, npc: &str, interest: f32) -> StoryThread {
        StoryThread {
            thread_id: id.into(),
            participant_ids: vec![npc.into()],
            related_fact_ids: vec![format!("{id}_fact")],
            status: StoryThreadStatus::Active,
            urgency: 0.5,
            momentum: 0.5,
            player_interest: interest,
            ..Default::default()
        }
    }

    // ── Invariant 1: pool-filter (§24-#3) ────────────────────────────────────────────
    #[test]
    fn selected_candidates_only_from_pool() {
        let pool = vec![cand("npc_a", vec!["e1"], Some(NpcActionKind::Speak))];
        let mut story = StoryState::default();
        story.active_threads.push(thread("t1", "npc_a", 0.9));
        let plan = build_director_brief_packet(
            DirectorMode::OnDemand,
            &pool,
            &story,
            None,
            None,
            &[],
            &[],
            "pc_1",
        );
        // The selected key must canonicalize back to a pool member.
        let pool_keys: std::collections::HashSet<_> =
            pool.iter().map(WorldCandidateRef::from_candidate).collect();
        assert!(!plan.selected_world_candidates.is_empty());
        for r in &plan.selected_world_candidates {
            assert!(pool_keys.contains(r), "selected a ref not in the pool");
        }
        // An out-of-pool NPC never appears.
        assert!(plan
            .selected_world_candidates
            .iter()
            .all(|r| r.npc_id != "npc_ghost"));
    }

    // codex fold #1: colliding empty-events keys are dropped (fail-closed).
    #[test]
    fn colliding_empty_event_keys_are_dropped() {
        // Two candidates, same npc + same action + EMPTY events → identical canonical key.
        let pool = vec![
            cand("npc_a", vec![], Some(NpcActionKind::Speak)),
            cand("npc_a", vec![], Some(NpcActionKind::Speak)),
        ];
        let mut story = StoryState::default();
        story.active_threads.push(thread("t1", "npc_a", 0.9));
        let plan = build_director_brief_packet(
            DirectorMode::OnDemand,
            &pool,
            &story,
            None,
            None,
            &[],
            &[],
            "pc_1",
        );
        // The ambiguous key resolves to 2 members → cannot be uniquely resolved → dropped.
        assert!(
            plan.selected_world_candidates.is_empty(),
            "colliding ambiguous key must be fail-closed dropped, not double-counted"
        );
    }

    // ── Invariant 2: reveal gating fail-closed (§24-#10/#8) ───────────────────────────
    #[test]
    fn reveal_is_gm_truth_minus_player_known() {
        let pool = vec![cand("npc_a", vec!["e1"], Some(NpcActionKind::Speak))];
        let story = StoryState::default(); // empty pool path not taken (pool non-empty)
        let known = ["fact_known".to_string()];
        let truth = ["fact_known".to_string(), "fact_secret".to_string()];
        let plan = build_director_brief_packet(
            DirectorMode::OnDemand,
            &pool,
            &story,
            Some(&known),
            Some(&truth),
            &[],
            &[],
            "pc_1",
        );
        assert_eq!(plan.reveal_candidate_fact_ids, vec!["fact_secret".to_string()]);
    }

    #[test]
    fn reveal_empty_when_player_known_is_none() {
        let pool = vec![cand("npc_a", vec!["e1"], Some(NpcActionKind::Speak))];
        let story = StoryState::default();
        let truth = ["fact_secret".to_string()];
        let plan = build_director_brief_packet(
            DirectorMode::OnDemand,
            &pool,
            &story,
            None,
            Some(&truth),
            &[],
            &[],
            "pc_1",
        );
        assert!(
            plan.reveal_candidate_fact_ids.is_empty(),
            "player_known=None must NOT over-reveal"
        );
    }

    #[test]
    fn reveal_empty_when_gm_truth_is_none() {
        let pool = vec![cand("npc_a", vec!["e1"], Some(NpcActionKind::Speak))];
        let story = StoryState::default();
        let known = ["fact_known".to_string()];
        let plan = build_director_brief_packet(
            DirectorMode::OnDemand,
            &pool,
            &story,
            Some(&known),
            None,
            &[],
            &[],
            "pc_1",
        );
        assert!(plan.reveal_candidate_fact_ids.is_empty());
    }

    #[test]
    fn reveal_never_includes_fact_not_in_gm_truth() {
        // player_known carries an id not in gm_truth; reveal still ⊆ gm_truth.
        let pool = vec![cand("npc_a", vec!["e1"], Some(NpcActionKind::Speak))];
        let story = StoryState::default();
        let known = ["fact_x".to_string()];
        let truth = ["fact_secret".to_string()];
        let plan = build_director_brief_packet(
            DirectorMode::OnDemand,
            &pool,
            &story,
            Some(&known),
            Some(&truth),
            &[],
            &[],
            "pc_1",
        );
        for id in &plan.reveal_candidate_fact_ids {
            assert!(truth.contains(id), "revealed a fact not in gm_truth");
        }
    }

    // ── Invariant 3: pool-empty → WorldQuery (§11.7) ─────────────────────────────────
    #[test]
    fn empty_pool_yields_world_query_and_no_selection() {
        let pool: Vec<WorldReactionCandidate> = vec![];
        let mut story = StoryState::default();
        story.active_threads.push(thread("t1", "npc_a", 0.9));
        let plan = build_director_brief_packet(
            DirectorMode::OnDemand,
            &pool,
            &story,
            None,
            None,
            &[],
            &[],
            "pc_1",
        );
        assert!(plan.selected_world_candidates.is_empty());
        assert!(plan.world_query.is_some(), "empty pool must ask the World");
    }

    // ── Invariant 4: anti-railroad (§24-#13) ─────────────────────────────────────────
    #[test]
    fn rejected_thread_never_becomes_primary_or_secondary() {
        // The rejected thread has the highest natural interest, so without the penalty it
        // would win. It must be excluded from primary AND secondary.
        let pool = vec![cand("npc_a", vec!["e1"], Some(NpcActionKind::Speak))];
        let mut story = StoryState::default();
        let mut hot = thread("t_rejected", "npc_a", 1.0);
        hot.urgency = 1.0;
        hot.status = StoryThreadStatus::Escalating;
        story.active_threads.push(hot);
        story.active_threads.push(thread("t_ok", "npc_b", 0.3));
        story.player_interests.push(PlayerInterestSignal {
            thread_id: "t_rejected".into(),
            rejected: true,
            ..Default::default()
        });
        let plan = build_director_brief_packet(
            DirectorMode::OnDemand,
            &pool,
            &story,
            None,
            None,
            &[],
            &["t_rejected".to_string()],
            "pc_1",
        );
        assert_ne!(plan.primary_thread_id.as_deref(), Some("t_rejected"));
        assert!(!plan
            .secondary_thread_ids
            .contains(&"t_rejected".to_string()));
        assert_eq!(plan.primary_thread_id.as_deref(), Some("t_ok"));
    }

    // ── Disabled mode → minimal no-op plan ───────────────────────────────────────────
    #[test]
    fn disabled_mode_returns_noop_plan() {
        let pool = vec![cand("npc_a", vec!["e1"], Some(NpcActionKind::Speak))];
        let story = StoryState::default();
        let plan = build_director_brief_packet(
            DirectorMode::Disabled,
            &pool,
            &story,
            Some(&["k".into()]),
            Some(&["k".into(), "s".into()]),
            &[],
            &[],
            "pc_1",
        );
        assert_eq!(plan, DirectorPlan::default());
        assert!(plan.reveal_candidate_fact_ids.is_empty());
        assert!(plan.selected_world_candidates.is_empty());
    }

    // packet safety (codex fold #2/#3): structural strings only, no prose leak.
    #[test]
    fn packet_carries_only_structural_strings() {
        let pool = vec![cand("npc_a", vec!["e1"], Some(NpcActionKind::Speak))];
        let mut story = StoryState::default();
        story.active_threads.push(thread("t1", "npc_a", 0.9));
        let plan = build_director_brief_packet(
            DirectorMode::OnDemand,
            &pool,
            &story,
            None,
            None,
            &[],
            &[],
            "pc_1",
        );
        // dramatic_function / desired_change are generic structural tokens (snake_case),
        // never free secret prose.
        assert!(plan.dramatic_function.chars().all(|c| c.is_ascii()));
        assert!(!plan.dramatic_function.contains(' '));
        assert!(!plan.desired_change.contains(' '));
    }
}

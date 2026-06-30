//! P5.7 — externalized guard tests for the pure Director brief-packet selection
//! ([`trpg_director::build_director_brief_packet`]). Moved out of
//! `src/story/select.rs` to honor the ≤400-line discipline (consistent with
//! P5.1/P5.2). All four pure invariants are exercised through the public API only;
//! no private helper of `select.rs` is referenced.

use trpg_director::{build_director_brief_packet, DirectorMode};
use trpg_model::{
    BeatKind, CharacterArcState, DirectorPlan, NpcActionIntent, NpcActionKind,
    PlayerInterestSignal, SpotlightState, StoryState, StoryThread, StoryThreadStatus,
    WorldCandidateRef, WorldReactionCandidate,
};

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
    assert_eq!(
        plan.reveal_candidate_fact_ids,
        vec!["fact_secret".to_string()]
    );
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

// ── §24-#2: empty-story fallback is wired into build_director_brief_packet ─────────
// Gap 1 regression: an EMPTY StoryState (no usable threads) + a NON-empty candidate
// pool must NOT silently fall through to a bare Respond with zero affordances. The pure
// `fallback_beat_plan` must be folded in: a fallback beat_kind ∈ {Respond, Consequence,
// Choice} AND ≥2 open_player_affordances, while still attaching pool actors.
#[test]
fn empty_story_folds_in_fallback_beat_plan() {
    let pool = vec![cand("npc_a", vec!["e1"], Some(NpcActionKind::Speak))];
    let story = StoryState::default(); // no active_threads ⇒ no selectable primary
    let plan = build_director_brief_packet(
        DirectorMode::OnDemand,
        &pool,
        &story,
        None,
        None,
        &[],
        &[],
        "pc_acting",
    );
    assert!(
        matches!(
            plan.beat_kind,
            BeatKind::Respond | BeatKind::Consequence | BeatKind::Choice
        ),
        "empty story must carry a fallback beat_kind, got {:?}",
        plan.beat_kind
    );
    assert!(
        plan.open_player_affordances.len() >= 2,
        "empty story fallback must offer ≥2 affordances, got {}",
        plan.open_player_affordances.len()
    );
    assert!(
        plan.primary_thread_id.is_none(),
        "empty story has no primary thread to spotlight"
    );
    // Pool actors are still attached so the turn has concrete NPCs to react.
    assert!(
        !plan.selected_world_candidates.is_empty(),
        "fallback must still attach the resolvable pool candidates"
    );
    assert!(plan.focus_actor_ids.contains(&"npc_a".to_string()));
}

// Gap 1: when EVERY thread is rejected (no selectable primary), the same fallback fold
// applies — anti-railroad must not leave the player with a bare empty plan.
#[test]
fn all_rejected_threads_fall_back_to_playable_plan() {
    let pool = vec![cand("npc_a", vec!["e1"], Some(NpcActionKind::Speak))];
    let mut story = StoryState::default();
    story
        .active_threads
        .push(thread("t_rejected", "npc_a", 1.0));
    let plan = build_director_brief_packet(
        DirectorMode::OnDemand,
        &pool,
        &story,
        None,
        None,
        &[],
        &["t_rejected".to_string()],
        "pc_acting",
    );
    assert!(plan.primary_thread_id.is_none());
    assert!(
        plan.open_player_affordances.len() >= 2,
        "all-rejected must still yield ≥2 affordances via fallback"
    );
}

// Gap 1: the folded-in fallback still surfaces a spotlight target when a roster exists,
// proving the empty-story path keeps multi-player rotation alive.
#[test]
fn empty_story_fallback_still_surfaces_spotlight_target() {
    let pool = vec![cand("npc_a", vec!["e1"], Some(NpcActionKind::Speak))];
    let story = StoryState::default();
    let roster = vec![
        SpotlightState {
            player_id: "pc_acting".into(),
            spotlight_count: 5,
            ..Default::default()
        },
        SpotlightState {
            player_id: "pc_quiet".into(),
            spotlight_count: 0,
            ..Default::default()
        },
    ];
    let plan = build_director_brief_packet(
        DirectorMode::OnDemand,
        &pool,
        &story,
        None,
        None,
        &roster,
        &[],
        "pc_acting",
    );
    assert_eq!(
        plan.spotlight_target.as_deref(),
        Some("pc_quiet"),
        "fallback must spotlight the under-spotlighted non-acting player"
    );
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

// ── L5.2: arc-aware character_relevance flips selection vs the stubbed constant ────
#[test]
fn l52_arc_spotlight_debt_flips_primary_pick() {
    let pool = vec![
        cand("npc_low", vec!["e1"], Some(NpcActionKind::Speak)),
        cand("npc_high", vec!["e2"], Some(NpcActionKind::Speak)),
    ];
    // Two threads identical in every scored term except their id/participant ⇒ a TIE that the
    // deterministic tiebreak (thread_id asc) resolves to "t_aaa".
    let mut story = StoryState::default();
    story.active_threads.push(thread("t_aaa", "npc_low", 0.5));
    story.active_threads.push(thread("t_zzz", "npc_high", 0.5));
    let baseline = build_director_brief_packet(
        DirectorMode::OnDemand,
        &pool,
        &story,
        None,
        None,
        &[],
        &[],
        "pc_1",
    );
    assert_eq!(
        baseline.primary_thread_id.as_deref(),
        Some("t_aaa"),
        "no arc data ⇒ both character_relevance terms equal (old stub) ⇒ tiebreak picks t_aaa"
    );

    // L5.2: npc_high is badly overdue for spotlight. The arc-weighted term lifts t_zzz's score
    // above the tie and FLIPS the primary pick — provably different from the stubbed baseline.
    story.character_arcs.push(CharacterArcState {
        character_id: "npc_high".into(),
        spotlight_debt: 1.0,
        ..Default::default()
    });
    let arc_weighted = build_director_brief_packet(
        DirectorMode::OnDemand,
        &pool,
        &story,
        None,
        None,
        &[],
        &[],
        "pc_1",
    );
    assert_eq!(
        arc_weighted.primary_thread_id.as_deref(),
        Some("t_zzz"),
        "high spotlight_debt lifts character_relevance and flips the primary pick to t_zzz"
    );
}

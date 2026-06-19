use super::*;
use trpg_model::{NpcActionIntent, NpcActionKind};

fn cand(npc: &str, events: Vec<&str>) -> WorldReactionCandidate {
    WorldReactionCandidate {
        npc_id: npc.into(),
        stance: String::new(),
        emotional_state: String::new(),
        urgency: 0.0,
        feasibility: 0.0,
        risk: 0.0,
        action_intent: Some(NpcActionIntent {
            kind: NpcActionKind::Speak,
            target_ref: None,
            description: String::new(),
        }),
        knowledge_basis: vec![],
        source_event_ids: events.into_iter().map(String::from).collect(),
    }
}

// Disabled mode ⇒ None, regardless of inputs (the byte-identical baseline seam).
#[test]
fn disabled_mode_yields_none() {
    let block = build_director_block(
        DirectorMode::Disabled,
        &[cand("npc_a", vec!["e1"])],
        &StoryState::default(),
        Some(&["truth".into()]),
        Some(&[]),
        &[],
        &[],
        "pc_1",
    );
    assert!(block.is_none(), "Disabled must produce no packet block");
}

// ON ⇒ a packet block carrying the beat_kind signature token reaches the caller.
#[test]
fn on_mode_yields_block_with_beat_kind_token() {
    let block = build_director_block(
        DirectorMode::OnDemand,
        &[cand("npc_a", vec!["e1"])],
        &StoryState::default(),
        None,
        None,
        &[],
        &[],
        "pc_1",
    )
    .expect("ON mode with a candidate must yield a block");
    assert!(block.contains("[director_packet]"));
    assert!(block.contains("beat_kind: "));
}

// codex fold #4: a DB failure on EITHER knowledge surface (modeled as `None`) fail-closes
// the reveal set to empty — never over-reveals. Here gm_truth has a secret but
// player_known is None ⇒ no reveal id appears.
#[test]
fn none_player_known_fail_closes_reveal() {
    let block = build_director_block(
        DirectorMode::OnDemand,
        &[cand("npc_a", vec!["e1"])],
        &StoryState::default(),
        Some(&["fact_secret".into()]),
        None, // player_known read "failed"
        &[],
        &[],
        "pc_1",
    )
    .expect("ON mode yields a block");
    assert!(
        !block.contains("fact_secret"),
        "player_known=None must NOT over-reveal the gm-truth secret"
    );
    assert!(!block.contains("reveal_candidate_fact_ids"));
}

// Reveal is gm_truth ∖ player_known when BOTH sides resolve (codex fold #4 happy path).
#[test]
fn both_sides_present_reveals_difference_as_ids() {
    let block = build_director_block(
        DirectorMode::OnDemand,
        &[cand("npc_a", vec!["e1"])],
        &StoryState::default(),
        Some(&["fact_known".into(), "fact_secret".into()]),
        Some(&["fact_known".into()]),
        &[],
        &[],
        "pc_1",
    )
    .expect("ON mode yields a block");
    assert!(block.contains("reveal_candidate_fact_ids: fact_secret"));
    assert!(!block.contains("fact_known"));
}

// P5.5: rejected_thread_ids reads ONLY the persisted player_interests flagged rejected,
// drops empty ids, and dedups — the §24-#13 selector input now mirrors real persistence.
#[test]
fn rejected_thread_ids_reads_persisted_rejection() {
    use trpg_model::PlayerInterestSignal;
    let story = StoryState {
        player_interests: vec![
            PlayerInterestSignal {
                thread_id: "thr_rejected".into(),
                rejected: true,
                ..Default::default()
            },
            PlayerInterestSignal {
                thread_id: "thr_engaged".into(),
                rejected: false,
                ..Default::default()
            },
            // duplicate rejection + an empty id ⇒ both must not leak into the output.
            PlayerInterestSignal {
                thread_id: "thr_rejected".into(),
                rejected: true,
                ..Default::default()
            },
            PlayerInterestSignal {
                thread_id: String::new(),
                rejected: true,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let got = rejected_thread_ids(&story);
    assert_eq!(
        got,
        vec!["thr_rejected".to_string()],
        "only non-empty, deduped rejected thread ids survive"
    );
}

// P5.5: an empty/default story yields no rejected ids (the fail-soft fallback input).
#[test]
fn rejected_thread_ids_empty_for_default_story() {
    assert!(rejected_thread_ids(&StoryState::default()).is_empty());
}

// P5 revision (Gap 2): when REAL spotlight states are present, a non-acting,
// under-spotlighted player is rendered as the packet's spotlight_target. Absent roster
// ⇒ no target line (the existing safe `None`). This proves the runtime now feeds the
// pure core a live roster instead of the old hardcoded empty `Vec`.
#[test]
fn present_spotlight_states_select_non_acting_target() {
    let spotlights = vec![
        SpotlightState {
            player_id: "pc_1".into(),
            spotlight_count: 4,
            ..Default::default()
        },
        SpotlightState {
            player_id: "pc_quiet".into(),
            spotlight_count: 0,
            ..Default::default()
        },
    ];
    let block = build_director_block(
        DirectorMode::OnDemand,
        &[cand("npc_a", vec!["e1"])],
        &StoryState::default(),
        None,
        None,
        &[],
        &spotlights,
        "pc_1", // acting actor is heavily spotlighted; must be skipped
    )
    .expect("ON mode yields a block");
    assert!(
        block.contains("spotlight_target: pc_quiet"),
        "present roster must surface the under-spotlighted non-acting target"
    );
}

// P5 revision (Gap 2): absent spotlight states ⇒ no target line (fail-soft `None`),
// matching the load-error path (`load_spotlight_states` Err ⇒ empty roster).
#[test]
fn absent_spotlight_states_yield_no_target() {
    let block = build_director_block(
        DirectorMode::OnDemand,
        &[cand("npc_a", vec!["e1"])],
        &StoryState::default(),
        None,
        None,
        &[],
        &[], // empty roster (load error or genuinely no roster)
        "pc_1",
    )
    .expect("ON mode yields a block");
    assert!(
        !block.contains("spotlight_target:"),
        "empty roster must NOT emit a spotlight_target (fail-soft None)"
    );
}

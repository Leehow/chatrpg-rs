//! P5.1 story.rs tests: serde round-trip, default→empty, and `validated()` fail-closed
//! (clamp NaN/out-of-range + drop empty-id entries). Kept external to keep src/story.rs
//! under the ≤400-line discipline.
use trpg_model::story::*;

#[test]
fn story_state_default_is_empty() {
    let s = StoryState::default();
    assert!(s.is_empty());
}

#[test]
fn empty_object_round_trips_to_empty_state() {
    let s: StoryState = serde_json::from_str("{}").unwrap();
    assert!(s.is_empty());
}

#[test]
fn enums_default_fail_closed() {
    assert_eq!(StoryThreadStatus::default(), StoryThreadStatus::Dormant);
    assert_eq!(PayoffKind::default(), PayoffKind::Unspecified);
    assert_eq!(PromiseStatus::default(), PromiseStatus::Planted);
    assert_eq!(BeatKind::default(), BeatKind::Respond);
    assert_eq!(InterestSignal::default(), InterestSignal::Neutral);
}

#[test]
fn enums_serialize_snake_case() {
    assert_eq!(
        serde_json::to_string(&StoryThreadStatus::ReadyForPayoff).unwrap(),
        "\"ready_for_payoff\""
    );
    assert_eq!(
        serde_json::to_string(&BeatKind::Foreshadow).unwrap(),
        "\"foreshadow\""
    );
}

#[test]
fn full_state_round_trips() {
    let s = StoryState {
        active_threads: vec![StoryThread {
            thread_id: "thread_1".into(),
            premise: "the missing heir".into(),
            dramatic_question: "who killed the duke?".into(),
            stakes: vec!["succession".into()],
            participant_ids: vec!["npc_a".into()],
            related_fact_ids: vec!["fact_x".into()],
            status: StoryThreadStatus::Escalating,
            urgency: 0.7,
            momentum: 0.4,
            player_interest: 0.9,
            unresolved_questions: vec!["where is the will?".into()],
            unresolved_consequences: vec!["civil war".into()],
            last_touched_turn: Some("turn_12".into()),
        }],
        promises: vec![StoryPromise {
            promise_id: "promise_1".into(),
            setup_event_ids: vec!["evt_3".into()],
            expected_payoff_kind: PayoffKind::Revelation,
            status: PromiseStatus::Ripe,
            maturity: 0.8,
            payoff_candidate_fact_ids: vec!["fact_y".into()],
        }],
        character_arcs: vec![CharacterArcState {
            character_id: "pc_1".into(),
            arc_premise: "coward to hero".into(),
            current_stage: "refusal".into(),
            progress: 0.3,
            want: "safety".into(),
            need: "courage".into(),
            related_thread_ids: vec!["thread_1".into()],
        }],
        recent_beats: vec![BeatRecord {
            beat_kind: BeatKind::Complicate,
            turn_id: "turn_12".into(),
            thread_id: "thread_1".into(),
            summary: "ambush".into(),
            source_event_ids: vec!["evt_4".into()],
        }],
        pacing: PacingState {
            tension: 0.6,
            time_since_relief: 0.5,
            beats_since_escalation: 2,
            phase: "rising".into(),
        },
        player_interests: vec![PlayerInterestSignal {
            thread_id: "thread_2".into(),
            signal: InterestSignal::Avoidant,
            rejected: true,
            strength: 0.9,
            source_event_ids: vec!["evt_5".into()],
        }],
    };
    let json = serde_json::to_string(&s).unwrap();
    let back: StoryState = serde_json::from_str(&json).unwrap();
    assert_eq!(s, back);
    assert!(!s.is_empty());
}

#[test]
fn rejected_field_round_trips() {
    let sig: PlayerInterestSignal =
        serde_json::from_str(r#"{"thread_id":"t1","rejected":true}"#).unwrap();
    assert!(sig.rejected);
    // Default fail-closed: absent ⇒ not rejected.
    let sig2: PlayerInterestSignal = serde_json::from_str(r#"{"thread_id":"t1"}"#).unwrap();
    assert!(!sig2.rejected);
}

#[test]
fn validated_clamps_nan_and_out_of_range() {
    let s = StoryState {
        active_threads: vec![StoryThread {
            thread_id: "t1".into(),
            urgency: f32::NAN,
            momentum: 5.0,
            player_interest: -3.0,
            ..Default::default()
        }],
        promises: vec![StoryPromise {
            promise_id: "p1".into(),
            maturity: f32::NAN,
            ..Default::default()
        }],
        pacing: PacingState {
            tension: 9.0,
            time_since_relief: f32::NAN,
            ..Default::default()
        },
        ..Default::default()
    }
    .validated();
    let t = &s.active_threads[0];
    assert_eq!(t.urgency, 0.0);
    assert_eq!(t.momentum, 1.0);
    assert_eq!(t.player_interest, 0.0);
    assert_eq!(s.promises[0].maturity, 0.0);
    assert_eq!(s.pacing.tension, 1.0);
    assert_eq!(s.pacing.time_since_relief, 0.0);
}

#[test]
fn validated_drops_empty_id_entries() {
    let s = StoryState {
        active_threads: vec![
            StoryThread {
                thread_id: "".into(),
                ..Default::default()
            },
            StoryThread {
                thread_id: "keep".into(),
                ..Default::default()
            },
        ],
        promises: vec![StoryPromise {
            promise_id: "".into(),
            ..Default::default()
        }],
        character_arcs: vec![CharacterArcState {
            character_id: "".into(),
            ..Default::default()
        }],
        player_interests: vec![PlayerInterestSignal {
            thread_id: "".into(),
            ..Default::default()
        }],
        ..Default::default()
    }
    .validated();
    assert_eq!(s.active_threads.len(), 1);
    assert_eq!(s.active_threads[0].thread_id, "keep");
    assert!(s.promises.is_empty());
    assert!(s.character_arcs.is_empty());
    assert!(s.player_interests.is_empty());
}

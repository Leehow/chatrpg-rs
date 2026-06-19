//! P5.6 — runtime thin-async Director brief-packet preparation + GM-tail rendering.
//!
//! [`prepare_director_brief`] is the single runtime seam that wires the PURE Director core
//! ([`trpg_director::build_director_brief_packet`]) into the GM-only BP3 context. It is
//! deliberately thin: it loads the per-turn knowledge surfaces, hands an already-loaded
//! World candidate pool + an (for now empty) [`StoryState`] to the pure core, and renders
//! the typed plan into the `[director_packet]` block string. It commits nothing.
//!
//! ## Flag (default OFF, fail-closed)
//! Gated by [`trpg_director::director_packet_mode_from_env`] (`TRPG_DIRECTOR_PACKET`, default
//! OFF ⇒ `DirectorMode::Disabled`). When Disabled this returns `None` *immediately* — no DB
//! reads, no packet — so the GM tail appends nothing and the turn is byte-identical baseline.
//! We never call `ActionableSituationDirector::from_env_or_default()` (its enable flag
//! defaults true — LIVING_PLAN §1).
//!
//! ## Independent Option knowledge loaders (codex fold #4)
//! `gm_truth` and `player_known` are loaded via **two separate** DB calls
//! ([`Db::gm_truth_view`] / [`Db::player_knowledge_view`]), each mapped to its OWN `Option`
//! (`Err ⇒ None`). We deliberately do NOT use the aggregate `project_for_gm_adjudication`
//! (which folds both reads into one Result and would let a DB failure masquerade as an empty
//! set → reopening over-reveal). A `None` on either side fail-closes the pure core's reveal
//! set to empty.
//!
//! ## Player-invisible (§11.4 / codex fold #2/#3)
//! The returned block is appended ONLY to the GM-only `[gm][BP3]` user message (mirroring
//! `npc_guidance_block`). The SYSTEM never copies it into the player-visible narration path.
//! The block carries only ids / enum tokens / short structural strings (the renderer owns
//! that content-safety guard — it is a GM-tail string that bypasses the ContextFilter).
//!
//! ## StoryState / rejected — P5.5 wired
//! `prepare_director_brief` now loads the persisted [`StoryState`] via
//! [`Db::load_story_state`] (fail-soft: an absent row OR a deserialize error ⇒ `None` ⇒
//! `StoryState::default()`, so the pure core still takes its `fallback`/empty-story path and
//! the turn never panics). `rejected_thread_ids` is derived from the loaded story's
//! `player_interests` where `rejected == true` (§二十四-#13 selector now reads REAL persisted
//! rejection). The pure `build_director_block` core stays DB-free — it receives the resolved
//! `&StoryState` + `&[rejected]` as arguments, so the wiring proof stays testable without a
//! live Postgres. Director only proposes; the runtime-owned commit path is
//! [`apply_story_proposals`] (设计4补充 §二-④).
//!
//! ## Story WRITE loop — P6.8a (flag `TRPG_STORY_WRITE_LOOP`, default OFF)
//! [`commit_story_writes`] is the production caller that closes the read/write asymmetry
//! (P5⑤#3). OFF ⇒ no story_state writes (byte-identical baseline). ON ⇒ it persists player
//! thread-REJECTION proposals (§二十四-#13 commit half) and StoryThreadOpened status floors,
//! routed through [`apply_story_proposals`]. The pure derivations live in
//! [`crate::story_write`]; advancement semantics (P6.8b) are deliberately NOT implemented.

use trpg_db::Db;
use trpg_director::{
    build_director_brief_packet, director_packet_mode_from_env, render_director_packet_block,
    DirectorMode,
};
use trpg_model::{SpotlightState, StoryState, WorldReactionCandidate};

/// Build the rendered Director packet block for one turn, or `None` when the flag is OFF or
/// the plan is a no-op (nothing to append). Thin-async, fail-closed per field.
///
/// `candidates` is the pool ALREADY loaded by the caller's ContextAssembly (the World
/// reaction candidates) — we never re-load NPCs here. `acting_actor_id` drives spotlight
/// target selection.
///
/// This is the DB-bound shell: flag gate → two SEPARATE Option knowledge reads → delegate to
/// the pure, DB-free [`build_director_block`]. Keeping the decision/render logic pure makes
/// the wiring proof testable without a live Postgres.
pub async fn prepare_director_brief(
    db: &Db,
    session_id: &str,
    candidates: &[WorldReactionCandidate],
    acting_actor_id: &str,
) -> Option<String> {
    // Flag gate FIRST: Disabled ⇒ no DB reads, no packet, byte-identical baseline.
    let mode = director_packet_mode_from_env();
    if mode == DirectorMode::Disabled {
        return None;
    }

    // codex fold #4: two SEPARATE reads, each its own Option (Err ⇒ None ⇒ fail-closed). We
    // intentionally avoid the aggregate `project_for_gm_adjudication` so a DB failure on one
    // surface can never masquerade as an empty set and reopen over-reveal.
    let gm_truth: Option<Vec<String>> = db.gm_truth_view(session_id).await.ok();
    let player_known: Option<Vec<String>> = db.player_knowledge_view(session_id).await.ok();

    // P5.5: load the persisted StoryState (fail-soft). `Err`/parse-failure ⇒ `None` (handled
    // inside `load_story_state`), absent row ⇒ `Ok(None)`; either way `unwrap_or_default()`
    // yields the empty story so the pure core takes its fallback path and never panics.
    let story = db
        .load_story_state(session_id)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    // §二十四-#13: rejected threads come from the REAL persisted player-interest signals.
    let rejected: Vec<String> = rejected_thread_ids(&story);

    // P5 revision (Gap 2): load the REAL persisted spotlight roster so a non-acting,
    // under-spotlighted player can actually be selected (§二十四-#2). `load_spotlight_states`
    // is the durable `SpotlightState` surface (carries `spotlight_count`) — exactly what
    // `pick_spotlight_target` consumes. Fail-soft: a read error ⇒ `Err` ⇒ `unwrap_or_default`
    // ⇒ empty roster ⇒ `pick_spotlight_target` returns `None` (the existing safe behavior).
    let spotlights: Vec<SpotlightState> = db
        .load_spotlight_states(session_id)
        .await
        .unwrap_or_default();

    build_director_block(
        mode,
        candidates,
        &story,
        gm_truth.as_deref(),
        player_known.as_deref(),
        &rejected,
        &spotlights,
        acting_actor_id,
    )
}

/// Derive the §24-#13 rejected-thread id set from a loaded [`StoryState`]: every
/// `player_interests` signal flagged `rejected` contributes its `thread_id`. Pure + DB-free so
/// the selector reads exactly the persisted rejection (empty ids are dropped — an
/// un-addressable rejection can never gate a thread). Deduped, order-stable.
pub fn rejected_thread_ids(story: &StoryState) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in &story.player_interests {
        if s.rejected && !s.thread_id.is_empty() && !out.contains(&s.thread_id) {
            out.push(s.thread_id.clone());
        }
    }
    out
}

/// Runtime-owned commit path (设计4补充 §二-④): persist a Director-produced [`StoryState`]
/// delta. The Director core only PROPOSES a story snapshot; only the runtime (System Kernel)
/// commits it. Minimal by design — `validated()` fail-closes a corrupt proposal before the
/// idempotent upsert (same packet replay ⇒ same single row). Returns the DB error untouched
/// so the caller's turn-commit path decides fatality (story persistence is non-blocking).
///
/// P6.8a: this now HAS a production caller — [`commit_story_writes`] (gated by
/// `TRPG_STORY_WRITE_LOOP`, default OFF) loads the story, applies the pure
/// rejection-persist + StoryThreadOpened derivations ([`crate::story_write`]), and commits
/// through here. The READ side ([`prepare_director_brief`] → `load_story_state`) was already
/// load-bearing in P5; the WRITE side is now closed too (P5⑤#3 read/write asymmetry resolved).
/// Round-trip tests still exercise it directly; it is no longer write-only-by-tests.
pub async fn apply_story_proposals(
    db: &Db,
    session_id: &str,
    proposed: StoryState,
    updated_turn: &str,
) -> Result<(), anyhow::Error> {
    let story = proposed.validated();
    db.upsert_story_state(session_id, &story, updated_turn)
        .await
}

/// P6.8a production caller for [`apply_story_proposals`] — the deterministic story_state WRITE
/// loop. Flag-gated by `TRPG_STORY_WRITE_LOOP` (default OFF ⇒ this returns `Ok(())` IMMEDIATELY
/// with zero DB reads/writes, so the P5 baseline is byte-identical and `prepare_director_brief`
/// is unaffected). ON ⇒ it:
///
/// 1. loads the persisted [`StoryState`] (fail-soft ⇒ empty story);
/// 2. folds in the player thread-REJECTION proposals
///    ([`crate::story_write::merge_rejections`]) — the §宪法④ commit half of §二十四-#13: the
///    LLM/Director PROPOSES a `PlayerInterestSignal { rejected: true }`; the Kernel persists it
///    so next turn the P5.3 selector (which reads persisted `player_interests`) drops it;
/// 3. applies StoryThreadOpened ([`crate::story_write::apply_thread_opened`]) for every fact in
///    `newly_known_fact_ids` (a thread's `related_fact_ids` first receiving a `PlayerLearnedFact`
///    floors that thread `Dormant → Introduced` — a status FLOOR, never advancement; P6.8b is
///    OutOfScope);
/// 4. commits the merged snapshot via [`apply_story_proposals`] iff something actually changed
///    (skips the upsert on a pure no-op so a quiet turn writes nothing).
///
/// Non-blocking: the DB error is returned untouched for the caller's turn-commit path to decide
/// fatality — story persistence must never reverse a delivered narration.
pub async fn commit_story_writes(
    db: &Db,
    session_id: &str,
    rejection_proposals: &[trpg_model::PlayerInterestSignal],
    newly_known_fact_ids: &[String],
    updated_turn: &str,
) -> Result<(), anyhow::Error> {
    // Flag gate FIRST: OFF ⇒ no reads, no writes, byte-identical baseline.
    if !crate::story_write::story_write_loop_enabled() {
        return Ok(());
    }
    let before = db
        .load_story_state(session_id)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();

    let after = crate::story_write::merge_rejections(before.clone(), rejection_proposals);
    let (after, opened_changed) =
        crate::story_write::apply_thread_opened(after, newly_known_fact_ids);

    // Skip the upsert on a pure no-op (a quiet turn must not churn the row). `validated()`
    // is idempotent, so compare the would-be-committed snapshot against the loaded one.
    let merged_changed = after != before;
    if !merged_changed && !opened_changed {
        return Ok(());
    }
    apply_story_proposals(db, session_id, after, updated_turn).await
}

/// Pure (DB-free) core of [`prepare_director_brief`]: given the resolved knowledge `Option`s,
/// the loaded `story`, and the derived `rejected` thread ids, run the pure Director selection
/// and render the GM-tail block. `Disabled` ⇒ `None`.
///
/// P5.5: `story` + `rejected` are now the REAL persisted state (resolved by the async caller).
/// An empty/default story (the fail-soft fallback) still drives the empty-story path here, so
/// the proof stays identical when persistence yields nothing.
pub fn build_director_block(
    mode: DirectorMode,
    candidates: &[WorldReactionCandidate],
    story: &StoryState,
    gm_truth: Option<&[String]>,
    player_known: Option<&[String]>,
    rejected: &[String],
    spotlights: &[SpotlightState],
    acting_actor_id: &str,
) -> Option<String> {
    if mode == DirectorMode::Disabled {
        return None;
    }
    // P5 revision (Gap 2): `spotlights` is now the REAL persisted roster resolved by the async
    // caller (`load_spotlight_states`, fail-soft to empty). An empty roster still degrades to
    // `pick_spotlight_target` → None (the existing safe behavior). Keeping this core DB-free
    // lets the wiring proof stay testable without a live Postgres.
    let plan = build_director_brief_packet(
        mode,
        candidates,
        story,
        player_known,
        gm_truth,
        spotlights,
        rejected,
        acting_actor_id,
    );

    let block = render_director_packet_block(&plan);
    if block.trim().is_empty() {
        None
    } else {
        Some(block)
    }
}

#[cfg(test)]
mod tests {
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
}

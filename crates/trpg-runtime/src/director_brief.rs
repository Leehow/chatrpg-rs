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
//! ## StoryState / rejected — pending P5.5
//! Persistence (`load_story_state`) is P5.5. Until then `StoryState::default()` (empty) and
//! `rejected_thread_ids = []` are used, so the pure core takes its `fallback`/empty-story
//! path. P5.5 will swap in the loaded state + rejected ids here without touching the wiring.

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

    build_director_block(
        mode,
        candidates,
        gm_truth.as_deref(),
        player_known.as_deref(),
        acting_actor_id,
    )
}

/// Pure (DB-free) core of [`prepare_director_brief`]: given the resolved knowledge `Option`s,
/// run the pure Director selection and render the GM-tail block. `Disabled` ⇒ `None`.
///
/// StoryState persistence is P5.5; until then `StoryState::default()` (empty story) drives
/// the fallback path and `rejected_thread_ids`/spotlights are empty.
pub fn build_director_block(
    mode: DirectorMode,
    candidates: &[WorldReactionCandidate],
    gm_truth: Option<&[String]>,
    player_known: Option<&[String]>,
    acting_actor_id: &str,
) -> Option<String> {
    if mode == DirectorMode::Disabled {
        return None;
    }
    // StoryState persistence is P5.5; until then the empty story drives the fallback path.
    let story = StoryState::default();
    // No persisted rejected threads yet (P5.5). Empty ⇒ no anti-railroad penalties applied.
    let rejected: Vec<String> = Vec::new();
    // Spotlight roster persistence is P5.5; empty roster ⇒ `pick_spotlight_target` → None.
    let spotlights: Vec<SpotlightState> = Vec::new();

    let plan = build_director_brief_packet(
        mode,
        candidates,
        &story,
        player_known,
        gm_truth,
        &spotlights,
        &rejected,
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
            Some(&["truth".into()]),
            Some(&[]),
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
            None,
            None,
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
            Some(&["fact_secret".into()]),
            None, // player_known read "failed"
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
            Some(&["fact_known".into(), "fact_secret".into()]),
            Some(&["fact_known".into()]),
            "pc_1",
        )
        .expect("ON mode yields a block");
        assert!(block.contains("reveal_candidate_fact_ids: fact_secret"));
        assert!(!block.contains("fact_known"));
    }
}

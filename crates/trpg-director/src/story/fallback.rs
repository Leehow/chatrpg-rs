//! P5.4 — pure fallback beat + spotlight-target selection.
//!
//! When the [`StoryState`] is empty the Director still owes the player a playable turn:
//! [`fallback_beat_plan`] produces a minimal [`DirectorPlan`] with an allowed beat kind,
//! ≥2 open affordances, and ZERO reveals (an empty story has no established secret, and we
//! NEVER introduce an un-established one). [`pick_spotlight_target`] picks the least
//! spotlighted non-acting player, fail-closing to `None` (§二十四-#2). Both are PURE,
//! commit-nothing, zero-LLM.

use trpg_model::{BeatKind, DirectorPlan, SpotlightState, StoryState};

/// Beat kinds the empty-story fallback is allowed to emit. All are low-commitment,
/// player-facing beats that never assert un-established lore.
const FALLBACK_BEAT_KINDS: [BeatKind; 3] = [BeatKind::Respond, BeatKind::Consequence, BeatKind::Choice];

/// Build a playable plan for an empty [`StoryState`] (§二十四-#2: Director-off / empty
/// story still yields a playable turn). Guarantees:
/// - `beat_kind ∈ {Respond, Consequence, Choice}` (here: `Respond`, the lowest-commitment);
/// - `open_player_affordances.len() >= 2` (≥2 generic directions — never a single path);
/// - `reveal_candidate_fact_ids == []` (empty story ⇒ no established secret to reveal).
///
/// `acting_actor_id` + `spotlights` let the fallback still surface a spotlight target.
pub fn fallback_beat_plan(
    story: &StoryState,
    spotlights: &[SpotlightState],
    acting_actor_id: &str,
) -> DirectorPlan {
    debug_assert!(FALLBACK_BEAT_KINDS.contains(&BeatKind::Respond));
    let _ = story; // empty-story fallback ignores story content by design.
    DirectorPlan {
        beat_kind: BeatKind::Respond,
        // ≥2 GENERIC structural affordances — never a single correct path, never secrets.
        open_player_affordances: vec![
            "act_on_situation".into(),
            "ask_question".into(),
            "observe_surroundings".into(),
        ],
        reveal_candidate_fact_ids: Vec::new(), // never introduce an un-established secret
        dramatic_function: "open_scene".into(),
        desired_change: "invite_player_action".into(),
        spotlight_target: pick_spotlight_target(spotlights, acting_actor_id),
        ..Default::default()
    }
}

/// Pick the player with the MINIMUM `spotlight_count` that is NOT the acting actor. Ties
/// break deterministically by `player_id`. Returns `None` when the roster is empty or the
/// only candidate is the acting actor (fail-closed, §二十四-#2 — never spotlight the player
/// who just acted, never invent a target).
pub fn pick_spotlight_target(
    spotlights: &[SpotlightState],
    acting_actor_id: &str,
) -> Option<String> {
    spotlights
        .iter()
        .filter(|s| s.player_id != acting_actor_id && s.character_id.as_deref() != Some(acting_actor_id))
        .min_by(|a, b| {
            a.spotlight_count
                .cmp(&b.spotlight_count)
                .then_with(|| a.player_id.cmp(&b.player_id))
        })
        .map(|s| s.player_id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spot(player: &str, count: u32) -> SpotlightState {
        SpotlightState {
            player_id: player.into(),
            character_id: None,
            last_spotlight_turn: None,
            spotlight_count: count,
            character_strengths: vec![],
            preferred_playstyle: vec![],
            pending_personal_hook: None,
        }
    }

    #[test]
    fn fallback_yields_playable_plan() {
        let story = StoryState::default();
        let plan = fallback_beat_plan(&story, &[], "pc_acting");
        assert!(FALLBACK_BEAT_KINDS.contains(&plan.beat_kind));
        assert!(
            plan.open_player_affordances.len() >= 2,
            "fallback must offer ≥2 directions"
        );
        assert!(
            plan.reveal_candidate_fact_ids.is_empty(),
            "empty story ⇒ zero reveal, never an un-established secret"
        );
    }

    #[test]
    fn spotlight_picks_min_count_non_acting() {
        // counts {alice:2, bob:0, carol:1} → bob (min, non-acting).
        let roster = vec![spot("alice", 2), spot("bob", 0), spot("carol", 1)];
        assert_eq!(
            pick_spotlight_target(&roster, "pc_acting").as_deref(),
            Some("bob")
        );
    }

    #[test]
    fn spotlight_empty_roster_is_none() {
        assert_eq!(pick_spotlight_target(&[], "pc_acting"), None);
    }

    #[test]
    fn spotlight_only_acting_is_none() {
        let roster = vec![spot("solo", 0)];
        assert_eq!(pick_spotlight_target(&roster, "solo"), None);
    }

    #[test]
    fn spotlight_skips_acting_even_if_min() {
        // acting actor has min count but must be skipped → next-min non-acting wins.
        let roster = vec![spot("acting", 0), spot("other", 5)];
        assert_eq!(
            pick_spotlight_target(&roster, "acting").as_deref(),
            Some("other")
        );
    }

    #[test]
    fn spotlight_tie_breaks_by_player_id() {
        let roster = vec![spot("zoe", 0), spot("amy", 0)];
        assert_eq!(
            pick_spotlight_target(&roster, "pc_acting").as_deref(),
            Some("amy")
        );
    }

    #[test]
    fn fallback_surfaces_spotlight_target() {
        let story = StoryState::default();
        let roster = vec![spot("alice", 3), spot("bob", 1)];
        let plan = fallback_beat_plan(&story, &roster, "alice");
        assert_eq!(plan.spotlight_target.as_deref(), Some("bob"));
    }
}

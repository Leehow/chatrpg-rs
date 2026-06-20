use serde::{Deserialize, Serialize};
use trpg_model::SpotlightState;

use crate::DirectorInput;

/// One real player-character in the session, consumed by the spotlight tracker.
/// Carries real ids (never placeholders) plus the optional character colour the
/// tracker surfaces. `acted_this_turn` marks the player who took this turn's
/// action so the tracker increments exactly their spotlight count.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct SpotlightParticipant {
    pub player_id: String,
    pub character_id: Option<String>,
    #[serde(default)]
    pub character_strengths: Vec<String>,
    #[serde(default)]
    pub preferred_playstyle: Vec<String>,
    #[serde(default)]
    pub pending_personal_hook: Option<String>,
    #[serde(default)]
    pub acted_this_turn: bool,
}

/// Build one `SpotlightState` per real session player, tracking `spotlight_count`
/// and `last_spotlight_turn` across turns. Replaces the old single-literal stub.
///
/// Roster source, in priority: explicit `input.participants` (multi-player), else
/// a single participant derived from the real `request.viewer` ids (solo). When no
/// real id is available we return an empty vec (fail-closed) rather than inventing
/// placeholder ids like `"current_player"` / `"pc.current"`.
///
/// Counts carry forward from `input.prior_spotlights` (the persisted state surface
/// the runtime loads before the turn): the player who acted this turn gets `+1`
/// and `last_spotlight_turn = this turn`; everyone else carries their prior values.
pub(crate) fn build_spotlight(input: DirectorInput<'_>) -> Vec<SpotlightState> {
    let turn_id = &input.request.turn_id;
    resolve_roster(input)
        .into_iter()
        .map(|p| {
            let prior = input
                .prior_spotlights
                .iter()
                .find(|s| s.player_id == p.player_id);
            let prior_count = prior.map(|s| s.spotlight_count).unwrap_or(0);
            let (spotlight_count, last_spotlight_turn) = if p.acted_this_turn {
                (prior_count.saturating_add(1), Some(turn_id.clone()))
            } else {
                (
                    prior_count,
                    prior.and_then(|s| s.last_spotlight_turn.clone()),
                )
            };
            // Fresh character colour wins; otherwise carry whatever we knew before
            // so it persists across turns even when not re-supplied this turn.
            SpotlightState {
                player_id: p.player_id,
                character_id: p.character_id,
                last_spotlight_turn,
                spotlight_count,
                character_strengths: pick_vec(
                    p.character_strengths,
                    prior.map(|s| &s.character_strengths),
                ),
                preferred_playstyle: pick_vec(
                    p.preferred_playstyle,
                    prior.map(|s| &s.preferred_playstyle),
                ),
                pending_personal_hook: p
                    .pending_personal_hook
                    .or_else(|| prior.and_then(|s| s.pending_personal_hook.clone())),
            }
        })
        .collect()
}

fn pick_vec(fresh: Vec<String>, prior: Option<&Vec<String>>) -> Vec<String> {
    if !fresh.is_empty() {
        fresh
    } else {
        prior.cloned().unwrap_or_default()
    }
}

/// Resolve the real player roster. Explicit participants take precedence; otherwise
/// derive a solo participant from the viewer's real ids. Returns empty when no real
/// id exists (fail-closed, never a placeholder).
fn resolve_roster(input: DirectorInput<'_>) -> Vec<SpotlightParticipant> {
    if !input.participants.is_empty() {
        return input.participants.to_vec();
    }
    let viewer = &input.request.viewer;
    // Prefer a real player_id; fall back to the real actor/character id as the
    // stable spotlight key. Both absent ⇒ no real identity ⇒ no spotlight.
    match viewer.player_id.clone().or_else(|| viewer.actor_id.clone()) {
        Some(player_id) => vec![SpotlightParticipant {
            player_id,
            character_id: viewer.actor_id.clone(),
            acted_this_turn: true,
            ..Default::default()
        }],
        None => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::{
        CompiledContext, ContextRequest, RuntimeState, TokenBudget, VisibilityProfile,
    };

    fn req(turn: &str) -> ContextRequest {
        ContextRequest {
            ruleset_id: "coc".into(),
            module_id: None,
            session_id: "s1".into(),
            turn_id: turn.into(),
            viewer: VisibilityProfile::gm(),
            token_budget: TokenBudget::default(),
        }
    }
    fn state() -> RuntimeState {
        RuntimeState {
            ruleset_id: "coc".into(),
            ..Default::default()
        }
    }
    fn compiled() -> CompiledContext {
        CompiledContext::default()
    }

    fn input<'a>(
        r: &'a ContextRequest,
        s: &'a RuntimeState,
        c: &'a CompiledContext,
        parts: &'a [SpotlightParticipant],
        prior: &'a [SpotlightState],
    ) -> DirectorInput<'a> {
        DirectorInput {
            request: r,
            state: s,
            compiled: c,
            user_input: "x",
            conflict: None,
            module_config: None,
            participants: parts,
            prior_spotlights: prior,
            leverage_npc_ids: None,
        }
    }

    // Multi-player: one SpotlightState per real player, real ids, incrementing count.
    #[test]
    fn tracks_one_state_per_real_player_with_incrementing_count() {
        let r = req("t1");
        let s = state();
        let c = compiled();
        let parts = vec![
            SpotlightParticipant {
                player_id: "player.alice".into(),
                character_id: Some("pc.alice".into()),
                character_strengths: vec!["medicine".into()],
                preferred_playstyle: vec!["cautious".into()],
                pending_personal_hook: Some("find her brother".into()),
                acted_this_turn: true,
            },
            SpotlightParticipant {
                player_id: "player.bob".into(),
                character_id: Some("pc.bob".into()),
                acted_this_turn: false,
                ..Default::default()
            },
        ];
        let prior = vec![
            SpotlightState {
                player_id: "player.alice".into(),
                character_id: Some("pc.alice".into()),
                last_spotlight_turn: Some("t0".into()),
                spotlight_count: 2,
                ..Default::default()
            },
            SpotlightState {
                player_id: "player.bob".into(),
                character_id: Some("pc.bob".into()),
                last_spotlight_turn: Some("t0".into()),
                spotlight_count: 1,
                ..Default::default()
            },
        ];
        let out = build_spotlight(input(&r, &s, &c, &parts, &prior));

        assert_eq!(out.len(), 2, "one SpotlightState per real player");
        assert!(
            out.iter().all(|x| x.player_id != "current_player"),
            "no placeholder player_id literal"
        );
        assert!(
            out.iter()
                .all(|x| x.character_id.as_deref() != Some("pc.current")),
            "no placeholder character_id literal"
        );

        let alice = out
            .iter()
            .find(|x| x.player_id == "player.alice")
            .expect("alice present");
        assert_eq!(alice.character_id.as_deref(), Some("pc.alice"));
        assert_eq!(alice.spotlight_count, 3, "acted this turn → prior 2 + 1");
        assert_eq!(alice.last_spotlight_turn.as_deref(), Some("t1"));
        assert_eq!(alice.character_strengths, vec!["medicine".to_string()]);
        assert_eq!(alice.preferred_playstyle, vec!["cautious".to_string()]);
        assert_eq!(
            alice.pending_personal_hook.as_deref(),
            Some("find her brother")
        );

        let bob = out
            .iter()
            .find(|x| x.player_id == "player.bob")
            .expect("bob present");
        assert_eq!(
            bob.spotlight_count, 1,
            "did not act → count carried forward"
        );
        assert_eq!(
            bob.last_spotlight_turn.as_deref(),
            Some("t0"),
            "carried prior turn, not bumped"
        );
    }

    // Solo: no explicit roster → derive a single participant from real viewer ids.
    #[test]
    fn solo_session_uses_viewer_real_ids() {
        let mut r = req("t1");
        r.viewer = VisibilityProfile::player("player.solo", "pc.solo");
        let s = state();
        let c = compiled();
        let out = build_spotlight(input(&r, &s, &c, &[], &[]));
        assert_eq!(
            out.len(),
            1,
            "solo session still produces a valid spotlight"
        );
        assert_eq!(out[0].player_id, "player.solo");
        assert_eq!(out[0].character_id.as_deref(), Some("pc.solo"));
        assert_ne!(out[0].player_id, "current_player");
        assert_eq!(out[0].spotlight_count, 1, "first spotlight for this player");
        assert_eq!(out[0].last_spotlight_turn.as_deref(), Some("t1"));
    }

    // Character colour persists across turns when the participant doesn't re-supply it.
    #[test]
    fn carries_prior_character_colour_when_not_resupplied() {
        let r = req("t2");
        let s = state();
        let c = compiled();
        let parts = vec![SpotlightParticipant {
            player_id: "p1".into(),
            character_id: Some("pc1".into()),
            acted_this_turn: true,
            ..Default::default()
        }];
        let prior = vec![SpotlightState {
            player_id: "p1".into(),
            character_id: Some("pc1".into()),
            last_spotlight_turn: Some("t1".into()),
            spotlight_count: 1,
            character_strengths: vec!["stealth".into()],
            preferred_playstyle: vec!["aggressive".into()],
            pending_personal_hook: Some("old debt".into()),
        }];
        let out = build_spotlight(input(&r, &s, &c, &parts, &prior));
        assert_eq!(out[0].spotlight_count, 2);
        assert_eq!(
            out[0].character_strengths,
            vec!["stealth".to_string()],
            "carried from prior state"
        );
        assert_eq!(out[0].preferred_playstyle, vec!["aggressive".to_string()]);
        assert_eq!(out[0].pending_personal_hook.as_deref(), Some("old debt"));
    }

    // Fail-closed: no real id anywhere → no spotlight, never a placeholder.
    #[test]
    fn fail_closed_no_real_id_yields_no_placeholder() {
        let r = req("t1"); // viewer = gm() → player_id None, actor_id None
        let s = state();
        let c = compiled();
        let out = build_spotlight(input(&r, &s, &c, &[], &[]));
        assert!(
            out.is_empty(),
            "no real identity → no spotlight, never a 'current_player' placeholder"
        );
    }
}

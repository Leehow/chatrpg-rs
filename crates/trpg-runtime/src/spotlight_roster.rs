//! Build the real spotlight roster from a session's player-character actors.
//!
//! `prepare_actionable_situation` used to pass `participants: &[]` to the
//! director, so the spotlight tracker's multi-player path was never driven by
//! real data. This module maps the session's `runtime_actor_parameters` rows of
//! kind `player_character` into `SpotlightParticipant`s so multi-player rotation
//! + personal-hook surfacing run whenever a roster exists, degrading to solo.
//!
//! Source of truth: there is no session→PC roster table (solo-focused product),
//! so the per-session actor surface `runtime_actor_parameters` IS the roster.
//! Pure logic only — the DB read lives in `trpg-params`; this module is a pure,
//! unit-testable mapping over already-loaded rows.

use serde_json::Value;
use trpg_director::SpotlightParticipant;
use trpg_params::RuntimeActorParameters;

/// Map loaded player-character actor rows into spotlight participants.
///
/// - `player_id` = `character_id` = `actor_id` (solo product has no separate
///   player identity; mirrors the director's viewer-derived solo fallback which
///   keys on player_id OR actor_id, character_id = actor_id).
/// - `acted_this_turn` is true for the row whose `actor_id` matches the acting
///   actor for this turn; everyone else carries forward (count not bumped).
/// - strengths / playstyle / hook come from the sheet's optional `spotlight`
///   sub-bucket (fail-soft empty); the tracker carries learned colour forward.
pub(crate) fn build_spotlight_participants(
    players: &[RuntimeActorParameters],
    acting_actor_id: &str,
) -> Vec<SpotlightParticipant> {
    players
        .iter()
        .map(|p| {
            let (character_strengths, preferred_playstyle, pending_personal_hook) =
                extract_spotlight_fields(&p.sheet_json);
            SpotlightParticipant {
                player_id: p.actor_id.clone(),
                character_id: Some(p.actor_id.clone()),
                character_strengths,
                preferred_playstyle,
                pending_personal_hook,
                acted_this_turn: p.actor_id == acting_actor_id,
            }
        })
        .collect()
}

/// Read the conventional, ruleset-agnostic `spotlight` sub-bucket off a sheet:
/// `spotlight.character_strengths[]`, `spotlight.preferred_playstyle[]`,
/// `spotlight.pending_personal_hook`. Missing / wrong-typed ⇒ empty / None.
fn extract_spotlight_fields(sheet_json: &Value) -> (Vec<String>, Vec<String>, Option<String>) {
    let bucket = sheet_json.get("spotlight");
    let string_list = |key: &str| -> Vec<String> {
        bucket
            .and_then(|b| b.get(key))
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    };
    let strengths = string_list("character_strengths");
    let playstyle = string_list("preferred_playstyle");
    let hook = bucket
        .and_then(|b| b.get("pending_personal_hook"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    (strengths, playstyle, hook)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use trpg_model::ActorKind;

    fn pc(actor_id: &str, sheet_json: Value) -> RuntimeActorParameters {
        RuntimeActorParameters {
            actor_id: actor_id.into(),
            actor_kind: ActorKind::PlayerCharacter,
            sheet_json,
            ..Default::default()
        }
    }

    // Multi-player: one participant per row, ids from actor_id, only the acting
    // actor flagged acted_this_turn.
    #[test]
    fn maps_each_pc_row_and_flags_only_the_acting_actor() {
        let players = vec![pc("pc.alice", json!({})), pc("pc.bob", json!({}))];
        let out = build_spotlight_participants(&players, "pc.bob");

        assert_eq!(out.len(), 2, "one participant per player-character row");

        let alice = out.iter().find(|p| p.player_id == "pc.alice").expect("alice");
        assert_eq!(alice.character_id.as_deref(), Some("pc.alice"), "character_id = actor_id");
        assert!(!alice.acted_this_turn, "alice did not act this turn");

        let bob = out.iter().find(|p| p.player_id == "pc.bob").expect("bob");
        assert_eq!(bob.character_id.as_deref(), Some("pc.bob"));
        assert!(bob.acted_this_turn, "bob is the acting actor → flagged");
    }

    // sheet_json spotlight bucket is surfaced into participant colour fields.
    #[test]
    fn extracts_spotlight_bucket_from_sheet() {
        let sheet = json!({
            "spotlight": {
                "character_strengths": ["medicine", "stealth"],
                "preferred_playstyle": ["cautious"],
                "pending_personal_hook": "find her brother"
            }
        });
        let out = build_spotlight_participants(&[pc("pc.alice", sheet)], "pc.alice");
        let alice = &out[0];
        assert_eq!(alice.character_strengths, vec!["medicine".to_string(), "stealth".to_string()]);
        assert_eq!(alice.preferred_playstyle, vec!["cautious".to_string()]);
        assert_eq!(alice.pending_personal_hook.as_deref(), Some("find her brother"));
    }

    // No spotlight bucket (or wrong-typed) ⇒ empty / None, never a panic.
    #[test]
    fn missing_spotlight_bucket_yields_empty_colour() {
        let out = build_spotlight_participants(&[pc("pc.solo", json!({"resources": {"hp": 10}}))], "pc.solo");
        let solo = &out[0];
        assert!(solo.character_strengths.is_empty());
        assert!(solo.preferred_playstyle.is_empty());
        assert_eq!(solo.pending_personal_hook, None);
        assert!(solo.acted_this_turn, "solo PC is the acting actor");
    }

    // Acting actor absent from roster ⇒ nobody flagged (counts carry forward).
    #[test]
    fn acting_actor_absent_flags_nobody() {
        let players = vec![pc("pc.alice", json!({})), pc("pc.bob", json!({}))];
        let out = build_spotlight_participants(&players, "pc.ghost");
        assert!(out.iter().all(|p| !p.acted_this_turn), "no row matches the acting actor");
    }

    // Empty roster ⇒ empty participants (caller then falls back to viewer-solo).
    #[test]
    fn empty_roster_yields_empty_participants() {
        let out = build_spotlight_participants(&[], "pc.current");
        assert!(out.is_empty());
    }

    // extract_spotlight_fields directly: only well-typed entries survive.
    #[test]
    fn extract_drops_wrong_typed_entries() {
        let (strengths, playstyle, hook) = extract_spotlight_fields(&json!({
            "spotlight": {
                "character_strengths": ["a", 7, "b"],
                "preferred_playstyle": "not-an-array",
                "pending_personal_hook": 42
            }
        }));
        assert_eq!(strengths, vec!["a".to_string(), "b".to_string()], "non-string array entries dropped");
        assert!(playstyle.is_empty(), "non-array playstyle ignored");
        assert_eq!(hook, None, "non-string hook ignored");
    }
}

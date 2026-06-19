//! Pre-turn character-card entry gate.
//!
//! "给 GM 上一道锁，没有角色卡不能进入游戏" — a hard gate that refuses to start
//! the GM turn pipeline unless the session has a bound, source-backed,
//! mechanically resolvable player-character card. This is the single reusable
//! predicate that CLI (`trpg turn`, `trpg play`) and API
//! (`/api/sessions/{id}/turn`) all call BEFORE any LLM/GM narration, so the gate
//! is enforced once, in the runtime, not duplicated as ad-hoc JSON checks or
//! prompt-only guidance.
//!
//! A row is playable only when it is the materialized `created_character_v1`
//! shape written by [`crate::RuntimeEngine::create_and_bind_character`]. Missing
//! rows, unresolved (`unresolved_source_backed_required_v1_15_4`) rows,
//! synthetic placeholder (`runtime_placeholder_v1_15_4`) rows, and any row whose
//! `status_json.not_mechanically_resolvable` is `true` are all blocked.

use serde_json::Value;
use trpg_model::ActorKind;
use trpg_params::RuntimeActorParameters;

/// Why a session may not enter the GM turn loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryGateBlock {
    /// No player-character actor row is bound to the session at all.
    MissingCharacterCard,
    /// Player-character rows exist but none is a source-backed, mechanically
    /// resolvable card (only unresolved / placeholder rows).
    UnresolvedCharacterCard,
}

/// Decision of the entry gate for one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryGate {
    /// The session has at least one playable player-character card; `actor_id`
    /// is the card the turn should play as.
    Playable { actor_id: String },
    /// The session must not start a GM turn.
    Blocked(EntryGateBlock),
}

impl EntryGate {
    /// True only when the session may enter the GM turn loop.
    pub fn is_playable(&self) -> bool {
        matches!(self, EntryGate::Playable { .. })
    }

    /// The blocking reason, if any.
    pub fn block(&self) -> Option<&EntryGateBlock> {
        match self {
            EntryGate::Blocked(b) => Some(b),
            EntryGate::Playable { .. } => None,
        }
    }
}

impl EntryGateBlock {
    /// Stable machine-readable code for structured CLI phase / API error bodies.
    pub fn code(&self) -> &'static str {
        match self {
            EntryGateBlock::MissingCharacterCard => "missing_character_card",
            EntryGateBlock::UnresolvedCharacterCard => "unresolved_character_card",
        }
    }

    /// Human-facing, actionable hint pointing at character creation.
    pub fn hint(&self) -> String {
        let why = match self {
            EntryGateBlock::MissingCharacterCard => {
                "This session has no bound player character card."
            }
            EntryGateBlock::UnresolvedCharacterCard => {
                "This session's player character is an unresolved placeholder \
                 with no source-backed stats/skills/hp."
            }
        };
        format!(
            "{why} Create and bind a character first: \
             `trpg create-character --ruleset <ruleset> --session-id <session_id>` \
             (or the interactive create-character flow), then start the turn."
        )
    }
}

/// True when `v` is an object with `key` set to the boolean `true`.
fn json_flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// True when the mechanical profile carries at least one non-empty mechanical
/// view (`stats`, `skills`, or the narrative `fields` map). The
/// `created_character_v1` materialization always populates `fields` from the
/// full sheet, so narrative-only rulesets (empty stats/skills) still pass, while
/// a bare metadata-only profile does not.
fn has_useful_mechanical_profile(mech: &Value) -> bool {
    ["stats", "skills", "fields"]
        .iter()
        .any(|k| match mech.get(*k) {
            Some(Value::Object(m)) => !m.is_empty(),
            Some(Value::Array(a)) => !a.is_empty(),
            _ => false,
        })
}

/// Pure predicate: is this single actor-parameter row a playable,
/// source-backed, mechanically resolvable player character?
pub fn actor_params_playable(p: &RuntimeActorParameters) -> bool {
    if !matches!(p.actor_kind, ActorKind::PlayerCharacter) {
        return false;
    }
    // Unresolved rows announce themselves explicitly.
    if json_flag(&p.status_json, "not_mechanically_resolvable") {
        return false;
    }
    // Placeholder rows demand source-backed materialization first.
    if json_flag(&p.status_json, "requires_source_backed_materialization")
        || json_flag(
            &p.mechanical_profile,
            "requires_source_backed_materialization",
        )
        || json_flag(&p.mechanical_profile, "requires_materialization")
    {
        return false;
    }
    // Must be the source-backed created shape.
    let source_backed = p.source_kind == "created_character_v1"
        || p.mechanical_profile
            .get("source_quality")
            .and_then(Value::as_str)
            == Some("created_character_v1");
    if !source_backed {
        return false;
    }
    has_useful_mechanical_profile(&p.mechanical_profile)
}

/// Pure core of the entry gate over a session's player-character roster.
///
/// `rows` is the session's player-character actor-parameter roster (e.g. from
/// [`trpg_params::RuntimeParameterService::list_player_actor_parameters`]). The
/// gate passes when at least one row is playable, preferring the conventional
/// active card `pc.current`.
pub fn evaluate_entry_gate(rows: &[RuntimeActorParameters]) -> EntryGate {
    let players: Vec<&RuntimeActorParameters> = rows
        .iter()
        .filter(|p| matches!(p.actor_kind, ActorKind::PlayerCharacter))
        .collect();
    if players.is_empty() {
        return EntryGate::Blocked(EntryGateBlock::MissingCharacterCard);
    }
    let chosen = players
        .iter()
        .find(|p| p.actor_id == "pc.current" && actor_params_playable(p))
        .or_else(|| players.iter().find(|p| actor_params_playable(p)));
    match chosen {
        Some(p) => EntryGate::Playable {
            actor_id: p.actor_id.clone(),
        },
        None => EntryGate::Blocked(EntryGateBlock::UnresolvedCharacterCard),
    }
}

impl crate::RuntimeEngine {
    /// Evaluate the pre-turn character-card gate for a session by loading its
    /// player-character roster and applying [`evaluate_entry_gate`]. Callers
    /// (CLI turn/play, API turn) must check this BEFORE building any GM turn
    /// loop or starting LLM narration.
    pub async fn evaluate_session_entry_gate(&self, session_id: &str) -> anyhow::Result<EntryGate> {
        let rows = trpg_params::RuntimeParameterService::new(self.db.clone())
            .list_player_actor_parameters(session_id)
            .await?;
        Ok(evaluate_entry_gate(&rows))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use trpg_params::RuntimeActorParameters;

    fn created_pc(actor_id: &str) -> RuntimeActorParameters {
        RuntimeActorParameters {
            actor_id: actor_id.into(),
            actor_kind: ActorKind::PlayerCharacter,
            source_kind: "created_character_v1".into(),
            mechanical_profile: json!({
                "ruleset": "dnd5e",
                "source_quality": "created_character_v1",
                "stats": {"STR": 14, "DEX": 12},
                "skills": {"Athletics": 4},
                "fields": {"name": "Kara", "class": "Fighter"},
            }),
            status_json: json!({"actor_id": actor_id, "conditions": [], "source_quality": "created_character_v1"}),
            ..Default::default()
        }
    }

    fn unresolved_pc(actor_id: &str) -> RuntimeActorParameters {
        RuntimeActorParameters {
            actor_id: actor_id.into(),
            actor_kind: ActorKind::PlayerCharacter,
            source_kind: "unresolved_source_backed_required_v1_15_4".into(),
            mechanical_profile: json!({
                "hydration": "unresolved_source_required",
                "requires_materialization": true,
                "missing_source_backed_fields": ["stats", "skills", "defense", "hp", "equipment_refs"],
            }),
            status_json: json!({
                "actor_id": actor_id,
                "hp_current": null,
                "not_mechanically_resolvable": true,
                "unknown_parameters": ["hp_max", "defense", "skills", "equipment_refs"],
            }),
            ..Default::default()
        }
    }

    fn placeholder_pc(actor_id: &str) -> RuntimeActorParameters {
        RuntimeActorParameters {
            actor_id: actor_id.into(),
            actor_kind: ActorKind::PlayerCharacter,
            source_kind: "runtime_placeholder_v1_15_4".into(),
            mechanical_profile: json!({
                "ruleset": "dnd5e",
                "stats": {},
                "skills": {},
                "source_quality": "placeholder_not_source_backed",
                "requires_source_backed_materialization": true,
            }),
            status_json: json!({
                "actor_id": actor_id,
                "source_quality": "placeholder_not_source_backed",
                "requires_source_backed_materialization": true,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn created_character_is_playable() {
        assert!(actor_params_playable(&created_pc("pc.current")));
    }

    #[test]
    fn unresolved_row_is_not_playable() {
        assert!(!actor_params_playable(&unresolved_pc("pc.current")));
    }

    #[test]
    fn placeholder_row_is_not_playable() {
        assert!(!actor_params_playable(&placeholder_pc("pc.current")));
    }

    #[test]
    fn npc_row_is_never_playable() {
        let mut p = created_pc("npc.guard");
        p.actor_kind = ActorKind::Npc;
        assert!(!actor_params_playable(&p));
    }

    #[test]
    fn created_with_only_metadata_is_not_playable() {
        // created_character_v1 marker but no stats/skills/fields → not useful.
        let mut p = created_pc("pc.current");
        p.mechanical_profile =
            json!({"ruleset": "dnd5e", "source_quality": "created_character_v1"});
        assert!(!actor_params_playable(&p));
    }

    #[test]
    fn empty_roster_blocks_with_missing_card() {
        assert_eq!(
            evaluate_entry_gate(&[]),
            EntryGate::Blocked(EntryGateBlock::MissingCharacterCard)
        );
    }

    #[test]
    fn only_unresolved_rows_block_with_unresolved_card() {
        let rows = vec![unresolved_pc("pc.current")];
        assert_eq!(
            evaluate_entry_gate(&rows),
            EntryGate::Blocked(EntryGateBlock::UnresolvedCharacterCard)
        );
    }

    #[test]
    fn created_pc_current_passes_the_gate() {
        let rows = vec![created_pc("pc.current")];
        assert_eq!(
            evaluate_entry_gate(&rows),
            EntryGate::Playable {
                actor_id: "pc.current".into()
            }
        );
    }

    #[test]
    fn prefers_pc_current_among_playable_rows() {
        let rows = vec![created_pc("pc.ally"), created_pc("pc.current")];
        assert_eq!(
            evaluate_entry_gate(&rows),
            EntryGate::Playable {
                actor_id: "pc.current".into()
            }
        );
    }

    #[test]
    fn falls_back_to_any_playable_when_pc_current_unresolved() {
        let rows = vec![unresolved_pc("pc.current"), created_pc("pc.hero")];
        assert_eq!(
            evaluate_entry_gate(&rows),
            EntryGate::Playable {
                actor_id: "pc.hero".into()
            }
        );
    }

    #[test]
    fn block_codes_and_hints_are_actionable() {
        assert_eq!(
            EntryGateBlock::MissingCharacterCard.code(),
            "missing_character_card"
        );
        assert_eq!(
            EntryGateBlock::UnresolvedCharacterCard.code(),
            "unresolved_character_card"
        );
        let missing_hint = EntryGateBlock::MissingCharacterCard.hint();
        assert!(missing_hint.contains("create-character"));
        assert!(missing_hint.contains("--session-id <session_id>"));
    }
}

//! Parse a recorded Flight Recorder trail (蓝图 §九 EvalTurnRecord) into the
//! per-turn contribution list the layered attributor reads.
//!
//! Input is the runtime's own deterministic capture
//! (`…flight-recorder-*.deterministic.json`): each turn carries the structured
//! products of every stage (`{plugin_id, hook, kind, summary}`) — NOT just the
//! final GM text. The attributor (§十 第三阶段) keys on these, so it can say
//! *which layer* produced a defect instead of "GM 回复不好".

use serde::Deserialize;

/// One stage contribution, mirroring `trpg_model::PluginContributionTrace`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Contribution {
    #[serde(default)]
    pub plugin_id: String,
    #[serde(default)]
    pub hook: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct FlightRecorder {
    #[serde(default)]
    contributions: Vec<Contribution>,
    #[serde(default)]
    ledger_event_kinds: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct RawTurn {
    #[serde(default)]
    user_input: String,
    #[serde(default)]
    player_visible_body: String,
    #[serde(default)]
    flight_recorder: FlightRecorder,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct RawRecording {
    #[serde(default)]
    turns: Vec<RawTurn>,
}

/// A parsed turn: the player-visible text plus the stage contribution trail.
#[derive(Debug, Clone, Default)]
pub struct FlightTurn {
    /// 1-based turn number.
    pub index: u32,
    pub user_input: String,
    pub player_visible_body: String,
    pub contributions: Vec<Contribution>,
    pub ledger_event_kinds: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct FlightRecording {
    pub turns: Vec<FlightTurn>,
}

impl FlightRecording {
    /// Parse from the deterministic JSON capture. Returns a serde error string.
    pub fn parse(json: &str) -> Result<Self, String> {
        let raw: RawRecording = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let turns = raw
            .turns
            .into_iter()
            .enumerate()
            .map(|(i, t)| FlightTurn {
                index: (i + 1) as u32,
                user_input: t.user_input,
                player_visible_body: t.player_visible_body,
                contributions: t.flight_recorder.contributions,
                ledger_event_kinds: t.flight_recorder.ledger_event_kinds,
            })
            .collect();
        Ok(FlightRecording { turns })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_flight_recorder() {
        let j = r#"{"turns":[{"user_input":"看","player_visible_body":"你看见门",
          "flight_recorder":{"contributions":[
            {"plugin_id":"core.view_loader","hook":"context_assembly","kind":"view_load","summary":"gm_context: 6 compiled block(s)"}],
          "ledger_event_kinds":["TurnFinalized"]}}]}"#;
        let r = FlightRecording::parse(j).unwrap();
        assert_eq!(r.turns.len(), 1);
        assert_eq!(r.turns[0].index, 1);
        assert_eq!(r.turns[0].contributions.len(), 1);
        assert_eq!(r.turns[0].contributions[0].kind, "view_load");
        assert_eq!(r.turns[0].ledger_event_kinds, vec!["TurnFinalized"]);
    }

    #[test]
    fn tolerates_missing_fields() {
        let r = FlightRecording::parse(r#"{"turns":[{}]}"#).unwrap();
        assert_eq!(r.turns.len(), 1);
        assert!(r.turns[0].contributions.is_empty());
    }
}

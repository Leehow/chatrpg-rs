//! One scenario spec + one evidence schema driving three executors —
//! deterministic, live, and replay (TC-JRNY-02).
//!
//! Everything here is pure and provider-free: the deterministic and replay
//! executors classify check checkpoints from a recorded fixture/cassette with NO
//! live fallback, and the live executor reuses the same [`CheckEvidence`] schema.
//! The fail-closed classifier ([`classify_check_checkpoint`]) is shared by all
//! three modes, so a perception action stays `TRIGGERED_NO_MECHANISM` while a
//! source-backed technical action can reach `PASS` in every mode.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::checkpoint::{
    build_check_evidence, human_player_input_findings, player_visible_effect_present, CheckEvidence,
};

/// Version tag for the shared check-evidence schema. Deterministic, live, and
/// replay outputs all stamp this so the lead can confirm the three modes used the
/// same evidence contract.
pub const EVIDENCE_SCHEMA_VERSION: &str = "tc-jrny-02.evidence.v1";

/// Which executor produced (or should produce) an evidence bundle.
///
/// - `Deterministic`: classify from an authored, seeded, provider-free fixture.
/// - `Live`: drive the real public CLI/GM/DB path.
/// - `Replay`: classify from a cassette recorded by an accepted live run, with
///   no live fallback — a missing/unexpected recorded turn is a hard failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    Deterministic,
    Live,
    Replay,
}

impl ExecutionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ExecutionMode::Deterministic => "deterministic",
            ExecutionMode::Live => "live",
            ExecutionMode::Replay => "replay",
        }
    }

    /// True only for the live executor; deterministic and replay must never spawn
    /// a provider or fall back to one.
    pub fn allows_live_provider(self) -> bool {
        matches!(self, ExecutionMode::Live)
    }

    /// True when the executor classifies from a recorded/authored fixture instead
    /// of spawning the CLI.
    pub fn is_fixture_driven(self) -> bool {
        matches!(self, ExecutionMode::Deterministic | ExecutionMode::Replay)
    }

    /// The `recorded_mode` provenance a fixture must carry to be consumed by this
    /// mode. Deterministic fixtures are authored/seeded; replay cassettes must
    /// come from a real live recording. `None` for the live executor.
    pub fn required_fixture_provenance(self) -> Option<&'static str> {
        match self {
            ExecutionMode::Deterministic => Some("deterministic"),
            ExecutionMode::Replay => Some("live"),
            ExecutionMode::Live => None,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "deterministic" | "det" => Some(ExecutionMode::Deterministic),
            "live" => Some(ExecutionMode::Live),
            "replay" => Some(ExecutionMode::Replay),
            _ => None,
        }
    }
}

/// Stable identity that ties every mode's evidence to the same scenario and
/// schema version. Deterministic, live, and replay outputs must all carry an
/// equal identity for the run to be considered the "same scenario".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenarioIdentity {
    pub scenario_id: String,
    pub scenario_version: String,
    #[serde(default = "default_evidence_schema_version")]
    pub evidence_schema_version: String,
}

fn default_evidence_schema_version() -> String {
    EVIDENCE_SCHEMA_VERSION.to_string()
}

impl ScenarioIdentity {
    pub fn new(scenario_id: impl Into<String>, scenario_version: impl Into<String>) -> Self {
        Self {
            scenario_id: scenario_id.into(),
            scenario_version: scenario_version.into(),
            evidence_schema_version: EVIDENCE_SCHEMA_VERSION.to_string(),
        }
    }

    /// JSON header attached to an evidence bundle so all three modes are
    /// self-describing and comparable.
    pub fn evidence_header(&self, mode: ExecutionMode) -> Value {
        serde_json::json!({
            "scenario_id": self.scenario_id,
            "scenario_version": self.scenario_version,
            "evidence_schema_version": self.evidence_schema_version,
            "mode": mode.as_str(),
        })
    }
}

/// One recorded/authored turn in a fixture (cassette). It carries exactly the
/// inputs the pure check classifier needs, so deterministic and replay runs are
/// fully reproducible without a process, network, or LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixtureTurn {
    pub user_input: String,
    #[serde(default)]
    pub count_delta: BTreeMap<String, i64>,
    /// Newest `contest_resolution_events` row observed this turn (or null).
    #[serde(default)]
    pub newest_contest_row: Option<Value>,
    /// Player-visible narration body for the turn.
    #[serde(default)]
    pub player_visible_body: String,
}

/// A recorded or authored cassette: the identity it belongs to, its provenance
/// (`deterministic` authored vs `live` recorded), and its per-turn evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fixture {
    pub identity: ScenarioIdentity,
    /// `deterministic` for authored/seeded fixtures, `live` for cassettes
    /// recorded from an accepted live run.
    pub recorded_mode: String,
    pub turns: Vec<FixtureTurn>,
}

/// Build check evidence for one fixture turn, reusing the same distillation the
/// live executor uses so the evidence schema is identical across modes.
pub fn fixture_turn_evidence(turn: &FixtureTurn, effect_any: &[String]) -> CheckEvidence {
    let natural = !turn.user_input.trim().is_empty()
        && human_player_input_findings(&turn.user_input, false).is_empty();
    let visible_effect = player_visible_effect_present(effect_any, &turn.player_visible_body);
    build_check_evidence(
        natural,
        &turn.count_delta,
        turn.newest_contest_row.as_ref(),
        visible_effect,
    )
}

/// A reason a fixture cannot legitimately drive a deterministic/replay run.
/// Any mismatch is a hard failure: the replay executor must NOT fall back to a
/// live provider when the cassette is wrong, missing, or for another scenario.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayMismatch {
    /// The cassette belongs to a different scenario/version/schema.
    Identity { expected: String, found: String },
    /// The cassette's provenance does not match the requested mode.
    Provenance { expected: String, found: String },
    /// The scenario expects a turn the cassette never recorded (a missing
    /// external call under replay).
    MissingTurn {
        index: usize,
        expected_input: String,
    },
    /// The cassette recorded a turn the scenario never asked for.
    UnexpectedTurn { index: usize, found_input: String },
    /// The cassette's recorded input does not match the scenario's turn input.
    InputMismatch {
        index: usize,
        expected_input: String,
        found_input: String,
    },
}

impl ReplayMismatch {
    pub fn message(&self) -> String {
        match self {
            ReplayMismatch::Identity { expected, found } => {
                format!("fixture identity mismatch: expected {expected}, found {found}")
            }
            ReplayMismatch::Provenance { expected, found } => format!(
                "fixture provenance mismatch: mode requires recorded_mode={expected}, fixture is {found}"
            ),
            ReplayMismatch::MissingTurn {
                index,
                expected_input,
            } => format!(
                "missing recorded turn {index}: scenario expects `{expected_input}` but the fixture has no such turn (no live fallback)"
            ),
            ReplayMismatch::UnexpectedTurn { index, found_input } => format!(
                "unexpected recorded turn {index}: fixture has `{found_input}` with no matching scenario turn"
            ),
            ReplayMismatch::InputMismatch {
                index,
                expected_input,
                found_input,
            } => format!(
                "turn {index} input mismatch: scenario `{expected_input}` vs fixture `{found_input}`"
            ),
        }
    }
}

fn identity_label(identity: &ScenarioIdentity) -> String {
    format!(
        "{}@{} [{}]",
        identity.scenario_id, identity.scenario_version, identity.evidence_schema_version
    )
}

/// Verify a fixture can legitimately drive `mode` for `expected` identity and the
/// scenario's `expected_inputs` (in order). A non-empty result means the run must
/// fail without ever touching a live provider.
pub fn verify_fixture_plan(
    fixture: &Fixture,
    expected: &ScenarioIdentity,
    expected_inputs: &[String],
    mode: ExecutionMode,
) -> Vec<ReplayMismatch> {
    let mut mismatches = Vec::new();
    if &fixture.identity != expected {
        mismatches.push(ReplayMismatch::Identity {
            expected: identity_label(expected),
            found: identity_label(&fixture.identity),
        });
    }
    if let Some(req) = mode.required_fixture_provenance() {
        if fixture.recorded_mode != req {
            mismatches.push(ReplayMismatch::Provenance {
                expected: req.to_string(),
                found: fixture.recorded_mode.clone(),
            });
        }
    }
    let max = expected_inputs.len().max(fixture.turns.len());
    for index in 0..max {
        match (expected_inputs.get(index), fixture.turns.get(index)) {
            (Some(expected_input), Some(turn)) => {
                if expected_input.trim() != turn.user_input.trim() {
                    mismatches.push(ReplayMismatch::InputMismatch {
                        index,
                        expected_input: expected_input.clone(),
                        found_input: turn.user_input.clone(),
                    });
                }
            }
            (Some(expected_input), None) => mismatches.push(ReplayMismatch::MissingTurn {
                index,
                expected_input: expected_input.clone(),
            }),
            (None, Some(turn)) => mismatches.push(ReplayMismatch::UnexpectedTurn {
                index,
                found_input: turn.user_input.clone(),
            }),
            (None, None) => {}
        }
    }
    mismatches
}

/// A cassette built from a finished live run, plus whether it is an *accepted*
/// replay cassette. Only a successful (`ok`) live run yields an accepted
/// `recorded_mode: "live"` cassette; a failed run yields a diagnostic cassette
/// whose provenance the replay executor rejects (no false replay-accept).
#[derive(Debug, Clone)]
pub struct CassetteBuild {
    pub fixture: Fixture,
    pub accepted: bool,
}

/// Provenance tag used for a cassette recorded from a run that did NOT pass. It
/// is intentionally not `"live"`, so `verify_fixture_plan` flags a provenance
/// mismatch and the replay executor refuses to treat it as accepted.
pub const DIAGNOSTIC_FIXTURE_PROVENANCE: &str = "diagnostic_failed";

/// Convert a finished live run into a cassette. `result_ok` gates acceptance:
/// `true` → `recorded_mode: "live"` (accepted, replay-consumable); `false` →
/// `recorded_mode: "diagnostic_failed"` (diagnostic only, replay rejects).
pub fn build_cassette(
    result_ok: bool,
    identity: &ScenarioIdentity,
    turns: Vec<FixtureTurn>,
) -> CassetteBuild {
    let recorded_mode = if result_ok {
        "live".to_string()
    } else {
        DIAGNOSTIC_FIXTURE_PROVENANCE.to_string()
    };
    CassetteBuild {
        fixture: Fixture {
            identity: identity.clone(),
            recorded_mode,
            turns,
        },
        accepted: result_ok,
    }
}

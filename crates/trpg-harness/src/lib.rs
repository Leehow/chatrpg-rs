//! Pure, provider-free harness primitives.
//!
//! The case schema, the JSONL stream parser, and the assertion evaluator live
//! here so that both the `trpg-harness` binary and the golden-case regression
//! tests exercise the *same* logic. Nothing in this module spawns a process,
//! touches the network, or requires an LLM provider, which keeps the golden
//! tests deterministic.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// L2.x story-quality checkpoints (pure, provider-free; extends the mechanics-checkpoint families
/// in this module). Checkpoint #1 = the post-adjudication Beat reflects the committed result.
pub mod story_quality;

pub fn default_stream_format() -> String {
    "jsonl".to_string()
}

/// Readiness derived from `trpg create-character --auto --stream-format jsonl`
/// stdout. Pure and provider-free so the journey-prelude gate is unit testable
/// without spawning the CLI or touching an LLM/DB.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CharacterCreationReadiness {
    /// True when every required phase/field is present and acceptable.
    pub ok: bool,
    pub saw_character_created: bool,
    pub saw_bound: bool,
    pub saw_error: bool,
    pub character_id: Option<String>,
    pub name: Option<String>,
    pub status: Option<String>,
    pub session_id: Option<String>,
    pub actor_id: Option<String>,
    pub validation_status: Option<String>,
    pub validation: Value,
    pub sheet: Value,
    pub failures: Vec<String>,
}

fn nonempty_str(data: &Value, key: &str) -> Option<String> {
    data.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn sheet_is_nonempty(sheet: &Value) -> bool {
    match sheet {
        Value::Object(map) => !map.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::String(text) => !text.trim().is_empty(),
        Value::Null => false,
        _ => true,
    }
}

/// Acceptable when the validation report carries no hard errors and, if it
/// reports a status, that status is `ok` (mirrors `create_and_bind_character`,
/// which only marks a character `ready` when `validation.status == "ok"`).
fn validation_is_acceptable(validation: &Value) -> bool {
    let errors_empty = validation
        .get("errors")
        .and_then(Value::as_array)
        .map(|errs| errs.is_empty())
        .unwrap_or(true);
    let status_ok = validation
        .get("status")
        .and_then(Value::as_str)
        .map(|s| s == "ok")
        .unwrap_or(true);
    errors_empty && status_ok
}

/// Parse the JSONL emitted by `trpg create-character --auto` and decide whether
/// the created character is ready to bind into a play session.
///
/// `require_persisted` gates on the `character_created` phase plus a nonempty
/// `character_id`; `require_session_binding` gates on the `bound` phase plus a
/// nonempty `session_id`/`actor_id`. Both default to the strict journey-prelude
/// contract at the call site.
pub fn parse_character_creation_jsonl(
    stdout_text: &str,
    require_persisted: bool,
    require_session_binding: bool,
) -> CharacterCreationReadiness {
    let mut r = CharacterCreationReadiness::default();
    let mut bound_session_id = None;
    let mut bound_actor_id = None;
    for raw in stdout_text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match value.get("event").and_then(Value::as_str) {
            Some("phase") => {
                let phase = value
                    .get("phase")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let data = value.get("data").cloned().unwrap_or(Value::Null);
                match phase {
                    "character_created" => {
                        r.saw_character_created = true;
                        r.character_id = nonempty_str(&data, "character_id");
                        r.name = nonempty_str(&data, "name");
                        r.status = nonempty_str(&data, "status");
                        r.session_id = nonempty_str(&data, "session_id");
                        r.actor_id = nonempty_str(&data, "actor_id");
                        r.validation = data.get("validation").cloned().unwrap_or(Value::Null);
                        r.validation_status = nonempty_str(&r.validation.clone(), "status");
                        r.sheet = data.get("sheet").cloned().unwrap_or(Value::Null);
                    }
                    "bound" => {
                        r.saw_bound = true;
                        bound_session_id = nonempty_str(&data, "session_id");
                        bound_actor_id = nonempty_str(&data, "actor_id");
                    }
                    _ => {}
                }
            }
            Some("error") => {
                r.saw_error = true;
                let msg = value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                r.failures
                    .push(format!("create-character emitted error event: {msg}"));
            }
            _ => {}
        }
    }
    // The `bound` phase is the authoritative binding; prefer its ids.
    if bound_session_id.is_some() {
        r.session_id = bound_session_id;
    }
    if bound_actor_id.is_some() {
        r.actor_id = bound_actor_id;
    }
    evaluate_creation_readiness(&mut r, require_persisted, require_session_binding);
    r
}

fn evaluate_creation_readiness(
    r: &mut CharacterCreationReadiness,
    require_persisted: bool,
    require_session_binding: bool,
) {
    if require_persisted {
        if !r.saw_character_created {
            r.failures
                .push("missing character_created phase".to_string());
        }
        if r.character_id.is_none() {
            r.failures.push("empty character_id".to_string());
        }
    }
    if require_session_binding {
        if !r.saw_bound {
            r.failures.push("missing bound phase".to_string());
        }
        if r.session_id.is_none() {
            r.failures.push("empty session_id".to_string());
        }
        if r.actor_id.is_none() {
            r.failures.push("empty actor_id".to_string());
        }
    }
    if r.status.is_none() {
        r.failures.push("empty character status".to_string());
    }
    if !sheet_is_nonempty(&r.sheet) {
        r.failures.push("character sheet is empty".to_string());
    }
    if !validation_is_acceptable(&r.validation) {
        r.failures.push(format!(
            "character validation not acceptable (status={:?})",
            r.validation_status
        ));
    }
    r.ok = r.failures.is_empty();
}

/// Read-only verification that a persisted-character prelude actually wrote the
/// rows it claimed in JSONL. JSONL readiness can be emitted by a CLI that never
/// committed; this is the durable evidence that gates `require_persisted`.
///
/// Population (the DB queries) happens in the binary; the *decision* lives here
/// so it is provider-free and unit testable.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PersistedVerification {
    /// Whether the scenario asked for a persisted character at all.
    pub required: bool,
    /// Whether the scenario also requires a bound session/actor.
    pub require_session_binding: bool,
    /// Whether a database connection was available for verification.
    pub db_available: bool,
    /// Whether row queries were actually attempted.
    pub db_checked: bool,
    pub character_id: Option<String>,
    pub session_id: Option<String>,
    pub actor_id: Option<String>,
    pub character_row_present: bool,
    pub session_row_present: bool,
    pub actor_params_present: bool,
    /// Query-level errors (e.g. missing table) that prevented verification.
    pub query_errors: Vec<String>,
    pub failures: Vec<String>,
}

/// Terminal classification of a persisted-character verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistedVerdict {
    /// Not required, or every required row was found.
    Ok,
    /// Could not verify (DB unavailable or query error): an environment block.
    Blocked,
    /// Verified and a required row is missing: a bad scenario setup.
    Invalid,
}

/// Decide the persisted-character verdict and append human-readable failures.
///
/// `Ok` when verification is not required or every required row is present.
/// `Blocked` when the database is unavailable or a query could not run (an
/// environment problem, not the scenario's fault). `Invalid` when the database
/// was queried successfully but a required row is missing or inconsistent.
pub fn evaluate_persisted_verification(v: &mut PersistedVerification) -> PersistedVerdict {
    if !v.required {
        return PersistedVerdict::Ok;
    }
    if !v.db_available {
        v.failures.push(
            "persisted character verification required but no database was available".to_string(),
        );
        return PersistedVerdict::Blocked;
    }
    if !v.query_errors.is_empty() {
        for err in &v.query_errors {
            v.failures
                .push(format!("could not verify persisted rows: {err}"));
        }
        return PersistedVerdict::Blocked;
    }
    let mut missing = Vec::new();
    if !v.character_row_present {
        missing.push(format!(
            "no characters row for character_id={:?}",
            v.character_id
        ));
    }
    if v.require_session_binding {
        if !v.session_row_present {
            missing.push(format!("no sessions row for session_id={:?}", v.session_id));
        }
        if !v.actor_params_present {
            missing.push(format!(
                "no runtime_actor_parameters row for session_id={:?} actor_id={:?}",
                v.session_id, v.actor_id
            ));
        }
    }
    if missing.is_empty() {
        PersistedVerdict::Ok
    } else {
        for m in missing {
            v.failures.push(m);
        }
        PersistedVerdict::Invalid
    }
}

/// A single harness case. Mirrors the JSON files under `harness/cases/`.
#[derive(Debug, Clone, Deserialize)]
pub struct HarnessCase {
    pub name: String,
    pub ruleset_id: String,
    #[serde(default)]
    pub module_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub user_input: String,
    #[serde(default)]
    pub recent_transcript: Option<String>,
    #[serde(default)]
    pub turns: Vec<HarnessTurn>,
    #[serde(default)]
    pub forbidden_terms: Vec<String>,
    #[serde(default)]
    pub required_terms: Vec<String>,
    #[serde(default)]
    pub required_events: Vec<String>,
    #[serde(default)]
    pub forbidden_events: Vec<String>,
    #[serde(default)]
    pub required_event_contains: Vec<EventContainsAssertion>,
    #[serde(default)]
    pub required_done_reason: Option<String>,
    #[serde(default)]
    pub require_no_llm_stream_start: bool,
    #[serde(default = "default_stream_format")]
    pub stream_format: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct HarnessTurn {
    pub user_input: String,
    #[serde(default)]
    pub forbidden_terms: Vec<String>,
    #[serde(default)]
    pub required_terms: Vec<String>,
    #[serde(default)]
    pub required_events: Vec<String>,
    #[serde(default)]
    pub forbidden_events: Vec<String>,
    #[serde(default)]
    pub required_event_contains: Vec<EventContainsAssertion>,
    #[serde(default)]
    pub required_done_reason: Option<String>,
    #[serde(default)]
    pub require_no_llm_stream_start: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EventContainsAssertion {
    pub event: String,
    pub contains: String,
}

/// Result of parsing a JSONL stream emitted by `trpg turn --stream-format jsonl`.
#[derive(Debug, Default)]
pub struct ParsedJsonlStream {
    pub events: Vec<String>,
    pub raw_events: Vec<Value>,
    pub body: String,
    pub failures: Vec<String>,
    pub session_id: Option<String>,
}

/// Outcome of evaluating a case's assertions against a parsed stream.
///
/// `failures` already includes any parse-level failures so callers can treat it
/// as the full set of stream-derived problems for the case.
#[derive(Debug, Default)]
pub struct AssertionOutcome {
    pub failures: Vec<String>,
    pub forbidden_hits: Vec<String>,
    pub missing_terms: Vec<String>,
    pub missing_events: Vec<String>,
}

pub fn parse_jsonl_events(stdout_text: &str) -> ParsedJsonlStream {
    let mut events = Vec::new();
    let mut raw_events = Vec::new();
    let mut body = String::new();
    let mut failures = Vec::new();
    let mut session_id = None;
    for (line_no, raw) in stdout_text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            failures.push(format!(
                "stdout line {} is not valid JSONL: {}",
                line_no + 1,
                tail_chars(line, 240)
            ));
            continue;
        };
        raw_events.push(value.clone());
        match value.get("event").and_then(Value::as_str) {
            Some("phase") => {
                if let Some(phase) = value.get("phase").and_then(Value::as_str) {
                    if phase == "session" {
                        if let Some(id) = value
                            .get("data")
                            .and_then(|d| d.get("session_id"))
                            .and_then(Value::as_str)
                        {
                            session_id = Some(id.to_string());
                        }
                    }
                    events.push(format!("phase:{phase}"));
                }
            }
            Some("delta") => {
                if let Some(delta) = value.get("data").and_then(Value::as_str) {
                    body.push_str(delta);
                }
            }
            Some("error") => {
                failures.push(format!("trpg emitted error event: {}", value));
            }
            Some(other) => events.push(format!("event:{other}")),
            None => failures.push(format!(
                "stdout line {} missing event field: {}",
                line_no + 1,
                value
            )),
        }
    }
    ParsedJsonlStream {
        events,
        raw_events,
        body,
        failures,
        session_id,
    }
}

/// Evaluate every stream-derived assertion declared by `case` against `parsed`.
///
/// This is the single source of truth for forbidden/required terms, required and
/// forbidden events, `required_event_contains`, `required_done_reason`, and the
/// `require_no_llm_stream_start` guard. Process-level checks (timeouts, exit
/// codes) are intentionally left to the caller because they are not pure.
pub fn evaluate_assertions(case: &HarnessCase, parsed: &ParsedJsonlStream) -> AssertionOutcome {
    let mut failures = parsed.failures.clone();
    let mut forbidden_hits = Vec::new();
    for term in &case.forbidden_terms {
        if !term.is_empty() && parsed.body.contains(term) {
            forbidden_hits.push(term.clone());
            failures.push(format!(
                "forbidden term appeared in player-facing delta stream: {term}"
            ));
        }
    }
    let mut missing_terms = Vec::new();
    for term in &case.required_terms {
        if !term.is_empty() && !parsed.body.contains(term) {
            missing_terms.push(term.clone());
            failures.push(format!(
                "required term did not appear in player-facing delta stream: {term}"
            ));
        }
    }
    let mut missing_events = Vec::new();
    for expected in &case.required_events {
        if !parsed.events.iter().any(|actual| actual == expected) {
            missing_events.push(expected.clone());
            failures.push(format!("missing required event: {expected}"));
        }
    }
    for forbidden in &case.forbidden_events {
        if parsed.events.iter().any(|actual| actual == forbidden) {
            failures.push(format!("forbidden event appeared: {forbidden}"));
        }
    }
    if case.require_no_llm_stream_start
        && parsed.events.iter().any(|e| e == "phase:llm_stream_start")
    {
        failures.push(
            "llm_stream_start appeared but this case expected the Rust Agent to stop before narration"
                .into(),
        );
    }
    if let Some(reason) = &case.required_done_reason {
        let ok = parsed.raw_events.iter().any(|event| {
            event.get("event").and_then(Value::as_str) == Some("phase")
                && event.get("phase").and_then(Value::as_str) == Some("done")
                && event
                    .get("data")
                    .and_then(|d| d.get("reason"))
                    .and_then(Value::as_str)
                    == Some(reason.as_str())
        });
        if !ok {
            failures.push(format!("missing done reason: {reason}"));
        }
    }
    for assertion in &case.required_event_contains {
        let ok = parsed.raw_events.iter().any(|event| {
            let name = event
                .get("event")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let phase_name = if name == "phase" {
                event
                    .get("phase")
                    .and_then(Value::as_str)
                    .map(|p| format!("phase:{p}"))
            } else {
                Some(format!("event:{name}"))
            };
            phase_name.as_deref() == Some(assertion.event.as_str())
                && event.to_string().contains(&assertion.contains)
        });
        if !ok {
            failures.push(format!(
                "event {} did not contain `{}`",
                assertion.event, assertion.contains
            ));
        }
    }

    AssertionOutcome {
        failures,
        forbidden_hits,
        missing_terms,
        missing_events,
    }
}

pub fn tail_chars(input: &str, max_chars: usize) -> String {
    let mut chars: Vec<char> = input.chars().rev().take(max_chars).collect();
    chars.reverse();
    chars.into_iter().collect()
}

// ---------------------------------------------------------------------------
// TC-JRNY-01: check-dependent journey checkpoints, evidence classification, and
// the human-input / roll-request guards. Everything below is pure and
// provider-free so the Journey Qualification Gate is unit-testable without
// spawning the CLI, an LLM, or a database.
// ---------------------------------------------------------------------------

/// Journey Qualification Gate result state for a single checkpoint. Mirrors
/// `docs/codex-team-lead/02-validation-rubric.md`. The three middle variants are
/// the regression target for TC-JRNY-01: a check-dependent action that produced a
/// provisional roll with null target/success/degree must classify as
/// `TRIGGERED_NO_MECHANISM`, never `PASS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CheckpointState {
    InvalidSetup,
    NotTriggered,
    TriggeredNoMechanism,
    MechanismNoUserEffect,
    Pass,
    Fail,
    Blocked,
}

impl CheckpointState {
    pub fn as_str(self) -> &'static str {
        match self {
            CheckpointState::InvalidSetup => "INVALID_SETUP",
            CheckpointState::NotTriggered => "NOT_TRIGGERED",
            CheckpointState::TriggeredNoMechanism => "TRIGGERED_NO_MECHANISM",
            CheckpointState::MechanismNoUserEffect => "MECHANISM_NO_USER_EFFECT",
            CheckpointState::Pass => "PASS",
            CheckpointState::Fail => "FAIL",
            CheckpointState::Blocked => "BLOCKED",
        }
    }

    /// True when the checkpoint did not fully pass and therefore must not be
    /// counted as green.
    pub fn is_failing(self) -> bool {
        !matches!(self, CheckpointState::Pass)
    }

    /// Position along the trigger-to-evidence chain, used to surface the
    /// earliest (most fundamental) failing gate as the headline result. Lower is
    /// earlier in the chain.
    pub fn chain_rank(self) -> u8 {
        match self {
            CheckpointState::InvalidSetup => 0,
            CheckpointState::Blocked => 1,
            CheckpointState::NotTriggered => 2,
            CheckpointState::TriggeredNoMechanism => 3,
            CheckpointState::MechanismNoUserEffect => 4,
            CheckpointState::Fail => 5,
            CheckpointState::Pass => 6,
        }
    }
}

/// Declarative check/dice requirement attached to a playtest turn. Absent on
/// non-check turns, so existing scenarios are unaffected (serde default).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct CheckCheckpoint {
    /// Optional label for evidence/reporting.
    #[serde(default)]
    pub label: Option<String>,
    /// Require actual dice evidence (a `dice_rolls` row this turn).
    #[serde(default)]
    pub require_dice: bool,
    /// Require a *resolved* check/contest: target + success + degree present and
    /// no `awaiting_binding` marker. A provisional roll does not satisfy this.
    #[serde(default)]
    pub require_check_resolution: bool,
    /// Require a player-visible consequence connected to the mechanism.
    #[serde(default)]
    pub require_player_visible_effect: bool,
    /// Substrings, any of which present in the player-visible output count as the
    /// required visible effect. When `require_player_visible_effect` is set and
    /// this list is empty, the effect gate falls back to "output is non-empty".
    #[serde(default)]
    pub player_visible_effect_any: Vec<String>,
}

impl CheckCheckpoint {
    /// True when this checkpoint asserts any check/dice requirement at all.
    pub fn is_active(&self) -> bool {
        self.require_dice || self.require_check_resolution || self.require_player_visible_effect
    }
}

/// Observed evidence for one check-dependent checkpoint, distilled from the turn
/// stream, DB count deltas, and the newest contest row.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CheckEvidence {
    /// A natural, human-like player action drove the turn (non-empty, passes the
    /// human-input guard).
    pub natural_player_action: bool,
    /// A check contract / roll plan was created this turn.
    pub roll_plan_present: bool,
    /// A pending check was created this turn.
    pub pending_check_present: bool,
    /// A GM-roll / contest path executed this turn.
    pub gm_roll_path: bool,
    /// Actual dice evidence (a `dice_rolls` row this turn).
    pub dice_present: bool,
    /// A resolved check/contest with non-null target/success/degree.
    pub resolved_check_present: bool,
    /// The newest contest row carries an `awaiting_binding` marker (the live gap).
    pub awaiting_binding: bool,
    /// A player-visible consequence connected to the mechanism was observed.
    pub player_visible_effect: bool,
}

/// A `contest_resolution_events` row is resolved only when target/success/degree
/// are all present and the row is not awaiting a target-number binding.
pub fn contest_row_is_resolved(row: &Value) -> bool {
    let nonnull = |key: &str| row.get(key).map(|v| !v.is_null()).unwrap_or(false);
    !contest_row_awaiting_binding(row)
        && nonnull("target_value")
        && nonnull("success")
        && nonnull("degree")
}

/// True when the contest row's `outcome_json.awaiting_binding` is present and
/// non-null — i.e. no source-backed target number/opposition model was bound.
pub fn contest_row_awaiting_binding(row: &Value) -> bool {
    row.get("outcome_json")
        .and_then(|o| o.get("awaiting_binding"))
        .map(|v| !v.is_null())
        .unwrap_or(false)
}

/// Distill per-turn check evidence from DB count deltas and the newest contest
/// row. `count_delta` is the harness `db_diff.count_delta` map.
pub fn build_check_evidence(
    natural_player_action: bool,
    count_delta: &BTreeMap<String, i64>,
    newest_contest_row: Option<&Value>,
    player_visible_effect: bool,
) -> CheckEvidence {
    let delta = |table: &str| count_delta.get(table).copied().unwrap_or(0);
    let contest_present = delta("contest_resolution_events") > 0;
    let (resolved, awaiting) = match newest_contest_row {
        Some(row) => (
            contest_row_is_resolved(row),
            contest_row_awaiting_binding(row),
        ),
        None => (false, false),
    };
    CheckEvidence {
        natural_player_action,
        roll_plan_present: delta("roll_plans") > 0,
        pending_check_present: delta("pending_checks") > 0,
        gm_roll_path: contest_present,
        dice_present: delta("dice_rolls") > 0,
        resolved_check_present: contest_present && resolved,
        awaiting_binding: awaiting,
        player_visible_effect,
    }
}

/// Classify a check-dependent checkpoint against observed evidence. Fail-closed:
/// a provisional roll (dice present, contest awaiting binding / unresolved) can
/// never reach `PASS` when resolution is required.
pub fn classify_check_checkpoint(spec: &CheckCheckpoint, ev: &CheckEvidence) -> CheckpointState {
    // Q3: a check journey needs a real, human-like player action.
    if !ev.natural_player_action {
        return CheckpointState::InvalidSetup;
    }
    // Q4: was the intended check path reached at all?
    let triggered =
        ev.roll_plan_present || ev.pending_check_present || ev.gm_roll_path || ev.dice_present;
    if !triggered {
        return CheckpointState::NotTriggered;
    }
    // Q5: required mechanism must actually emit evidence.
    if spec.require_dice && !ev.dice_present {
        return CheckpointState::TriggeredNoMechanism;
    }
    if spec.require_check_resolution && (ev.awaiting_binding || !ev.resolved_check_present) {
        return CheckpointState::TriggeredNoMechanism;
    }
    // Q6: player-visible behavior must reflect the mechanism when required.
    if spec.require_player_visible_effect && !ev.player_visible_effect {
        return CheckpointState::MechanismNoUserEffect;
    }
    CheckpointState::Pass
}

/// Whether a player-visible effect is present, given an effect spec and output.
/// An empty `effect_any` falls back to "output is non-empty".
pub fn player_visible_effect_present(effect_any: &[String], body: &str) -> bool {
    if effect_any.is_empty() {
        return !body.trim().is_empty();
    }
    effect_any
        .iter()
        .any(|needle| !needle.is_empty() && body.contains(needle))
}

/// Evaluate a conservative `when` precondition against the set of flags raised so
/// far in the run. `None` is always satisfied; otherwise the named flag must be
/// present. Keeps the adaptive state machine small and provider-free.
pub fn when_satisfied(when: Option<&str>, flags: &BTreeSet<String>) -> bool {
    match when {
        None => true,
        Some(token) => {
            let token = token.trim();
            token.is_empty() || flags.contains(token)
        }
    }
}

/// Findings that mark player input as non-human (test-engineering prose, JSON,
/// code, DB instructions, internal event names, or manual dice/results). Returns
/// an empty vector for clean, human-like input. Pure so the human-input gate is
/// unit-testable without spawning a turn.
pub fn human_player_input_findings(input: &str, allow_manual_roll_input: bool) -> Vec<String> {
    let trimmed = input.trim();
    let mut findings = Vec::new();
    if trimmed.is_empty() {
        return findings;
    }
    let lower = trimmed.to_ascii_lowercase();
    if trimmed.contains('{') || trimmed.contains('}') || trimmed.contains("```") {
        findings.push("contains JSON/code-like syntax; normal players should describe fictional actions in natural language".into());
    }
    let internal_terms = [
        "phase:",
        "event:",
        "checkcontract",
        "effectcontract",
        "parameterimpact",
        "actor_mechanical_states",
        "damage_packets",
        "dice_rolls",
        "roll_plans",
        "pending_checks",
        "pending_check",
        "pending check",
        "contest_resolution",
        "dicerolled",
        "dice_rolled",
        "checkresolved",
        "check_resolved",
        "playerlearnedfact",
        "db",
        "database",
        "sql",
        "json",
        "schema",
        "function",
        "cargo",
        "harness",
        "expected",
        "assert",
        "test case",
        "roll_plan",
        "rule_binding",
        "materialization",
        "semantic route",
        "api",
        "sse",
    ];
    for term in internal_terms {
        if lower.contains(term) {
            findings.push(format!("contains internal/test-engineering term `{term}`"));
        }
    }
    // SQL-shaped DB instructions (e.g. `select * from sessions`, `insert into …`).
    let sql_shaped = (lower.contains("select") && lower.contains(" from "))
        || lower.contains("insert into")
        || lower.contains("delete from");
    if sql_shaped {
        findings.push(
            "contains SQL-shaped database instruction; normal players never write queries".into(),
        );
    }
    if !allow_manual_roll_input {
        for term in [
            "/roll",
            "掷骰",
            "骰",
            "d6",
            "d10",
            "d20",
            "1d",
            "2d",
            "3d",
            "4d",
            "5d",
            "roll result",
            "i rolled",
            "total",
            "合计",
            "总共",
            "掷出来",
        ] {
            if lower.contains(term) || trimmed.contains(term) {
                findings.push(format!("contains manual dice/result language `{term}`; product-mode players should describe only fictional actions and let the system roll/resolve automatically"));
            }
        }
    }
    let comma_count = trimmed.matches('，').count()
        + trimmed.matches(',').count()
        + trimmed.matches(';').count()
        + trimmed.matches('；').count();
    if comma_count >= 5 && lower.contains("测试") {
        findings.push("looks like a bundled QA probe rather than one human player action".into());
    }
    findings
}

/// Findings that mark player-visible output as pushing the *player* to roll dice
/// or report a total, outside the allowed `[roll]`/`[system]`/`[gm]` tags.
pub fn player_visible_roll_request_findings(output: &str) -> Vec<String> {
    let visible = strip_tagged_sections(output, &["roll", "gm", "system"]);
    let lower = visible.to_ascii_lowercase();
    let mut findings = Vec::new();
    let patterns = [
        "请掷",
        "请投",
        "你来掷",
        "你来投",
        "告诉我点数",
        "告诉我结果",
        "给出总值",
        "给我总值",
        "掷骰结果",
        "投骰结果",
        "roll and tell",
        "roll the dice",
        "make a roll",
        "give me the roll",
        "tell me your total",
        "report the total",
        "what did you roll",
        "/roll ",
        "d10+",
        "d20+",
        "d6+",
    ];
    for pat in patterns {
        if lower.contains(pat) || visible.contains(pat) {
            findings.push(format!(
                "contains `{pat}` outside allowed [roll]/[system]/[gm] tags"
            ));
        }
    }
    findings
}

/// Remove `[tag]...[/tag]` sections (case-insensitive) for the given tags.
pub fn strip_tagged_sections(input: &str, tags: &[&str]) -> String {
    let mut out = input.to_string();
    for tag in tags {
        loop {
            let open = format!("[{}]", tag);
            let close = format!("[/{}]", tag);
            let Some(start) = out.to_ascii_lowercase().find(&open) else {
                break;
            };
            let rest_lower = out[start + open.len()..].to_ascii_lowercase();
            let Some(rel_end) = rest_lower.find(&close) else {
                break;
            };
            let end = start + open.len() + rel_end + close.len();
            out.replace_range(start..end, "");
        }
    }
    out
}

// ---------------------------------------------------------------------------
// TC-JRNY-02: one scenario spec + one evidence schema driving three executors
// (deterministic, live, replay). Everything here is pure and provider-free: the
// deterministic and replay executors classify check checkpoints from a recorded
// fixture/cassette with NO live fallback, and the live executor reuses the same
// `CheckEvidence` schema. The fail-closed classifier (`classify_check_checkpoint`)
// is shared by all three modes, so a perception action stays
// `TRIGGERED_NO_MECHANISM` while a source-backed technical action can reach `PASS`
// in every mode.
// ---------------------------------------------------------------------------

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
    /// Knowledge / no-spoiler evidence for the turn (player-knowledge projection
    /// and reveal/surface provenance). `None` on non-knowledge turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge: Option<KnowledgeEvidence>,
    /// NPC mind / relationship / social-memory evidence for the turn
    /// (TC-VS-NPC-01). `None` on non-social turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub npc_social: Option<NpcSocialEvidence>,
    /// Committed-memory / reload evidence for the turn (TC-VS-MEM-01). `None` on
    /// non-memory turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryEvidence>,
    /// Production flight-recorder / provenance evidence for the turn (TC-PIPE-03).
    /// `None` on non-trace turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flight_recorder: Option<FlightRecorderEvidence>,
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

// ---------------------------------------------------------------------------
// TC-VS-KNOW-01: Homecoming Knowledge / NoSpoiler vertical slice. A
// `KnowledgeCheckpoint` asserts the player-knowledge state and runs deterministic
// post-output leak verification through the SAME production mechanism the GM's
// NoSpoiler plugin uses — `trpg_model::scan_player_visible_leak` with a
// `FactSurfaceMarker`. This keeps NoSpoiler from being prompt-only here: the
// fixture records the player-knowledge projection and reveal/surface provenance,
// and the classifier proves that ContextSurfaced/PlayerExposed never grant
// knowledge, that a committed reveal grants player_party knowledge, that the
// failure branch never reveals, and that revealed facts become eligible to
// appear (DA-KNOW-02/04/05, DA-SPOIL-01/02/04).
// ---------------------------------------------------------------------------

fn default_true_bool() -> bool {
    true
}

/// Declarative knowledge / no-spoiler requirement attached to a playtest turn.
/// Absent on non-knowledge turns (serde default), so existing scenarios parse.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct KnowledgeCheckpoint {
    /// Stable fact id under test (the hidden truth, e.g. the drone's backdoor).
    #[serde(default)]
    pub hidden_fact_id: String,
    /// Configured surface terms whose presence in player-visible text would leak
    /// the hidden fact (the scenario's `configured_hidden_fact_terms`). Sourced
    /// from the module `SpoilerMeta.secret_terms` for `hidden_fact_id`.
    #[serde(default)]
    pub secret_terms: Vec<String>,
    /// Before reveal: the player_party must NOT know the hidden fact, and it must
    /// not have been granted this turn (surfacing ≠ granting).
    #[serde(default)]
    pub require_player_unknown: bool,
    /// After a successful reveal: the player_party MUST know the hidden fact.
    #[serde(default)]
    pub require_player_learned: bool,
    /// Run the production post-output leak verifier against the player-visible
    /// body. Defaults to true so the spoiler guard is never silently skipped.
    #[serde(default = "default_true_bool")]
    pub forbid_player_text_leak: bool,
    /// At least one of these substrings must appear in the player-visible body
    /// (the journey's `player_text_requires_semantic`, e.g. observable_discovery
    /// or revealed_fact_summary).
    #[serde(default)]
    pub require_semantic_any: Vec<String>,
    /// Continuity: this turn's player knowledge must come from a post-restart
    /// projection (`knowledge_survives_reload`).
    #[serde(default)]
    pub require_reload: bool,
}

impl KnowledgeCheckpoint {
    /// True when this checkpoint asserts any knowledge/no-spoiler requirement.
    pub fn is_active(&self) -> bool {
        self.require_player_unknown
            || self.require_player_learned
            || self.require_reload
            || !self.require_semantic_any.is_empty()
            || (self.forbid_player_text_leak && !self.secret_terms.is_empty())
    }
}

/// Observed knowledge evidence for one turn: the player-knowledge projection plus
/// the provenance of any reveal/surface so the classifier can prove the semantic
/// event split (ContextSurfaced/PlayerExposed grant nothing; PlayerLearnedFact
/// grants player_party knowledge). Recorded in the fixture and emitted in the
/// evidence bundle.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KnowledgeEvidence {
    /// player_party `knows_true` fact ids projected AFTER this turn
    /// (`Db::list_player_known_fact_ids` / `PlayerKnowledgeView`).
    #[serde(default)]
    pub player_known_fact_ids: Vec<String>,
    /// Fact ids granted to player_party THIS turn via a first-class reveal
    /// (`PlayerLearnedFact` → `KnowledgeEdge(player_party, knows_true)`).
    #[serde(default)]
    pub reveal_committed_fact_ids: Vec<String>,
    /// Entities surfaced/exposed this turn (`ContextSurfaced` / `PlayerExposed`):
    /// presence only, never fact knowledge.
    #[serde(default)]
    pub surfaced_entity_ids: Vec<String>,
    /// True when `player_known_fact_ids` was read from a post-restart projection.
    #[serde(default)]
    pub reload: bool,
}

/// Terminal classification of a knowledge / no-spoiler checkpoint. Fail-closed:
/// any leak, or any premature grant, is a failure that can never reach `PASS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum KnowledgeCheckpointState {
    /// No hidden fact id bound — the checkpoint cannot run.
    InvalidSetup,
    /// A player-unknown fact's term leaked into player-visible output.
    HiddenFactLeaked,
    /// The player knows (or was granted) the fact when it should still be hidden.
    KnowledgeUnexpectedlyKnown,
    /// A reveal was expected but player_party knowledge was not granted.
    KnowledgeNotGranted,
    /// The required observable discovery / revealed-fact summary is absent.
    MissingSemantic,
    /// Continuity required a post-reload projection but none was proven.
    ReloadNotProven,
    Pass,
}

impl KnowledgeCheckpointState {
    pub fn as_str(self) -> &'static str {
        match self {
            KnowledgeCheckpointState::InvalidSetup => "INVALID_SETUP",
            KnowledgeCheckpointState::HiddenFactLeaked => "HIDDEN_FACT_LEAKED",
            KnowledgeCheckpointState::KnowledgeUnexpectedlyKnown => "KNOWLEDGE_UNEXPECTEDLY_KNOWN",
            KnowledgeCheckpointState::KnowledgeNotGranted => "KNOWLEDGE_NOT_GRANTED",
            KnowledgeCheckpointState::MissingSemantic => "MISSING_SEMANTIC",
            KnowledgeCheckpointState::ReloadNotProven => "RELOAD_NOT_PROVEN",
            KnowledgeCheckpointState::Pass => "PASS",
        }
    }

    pub fn is_failing(self) -> bool {
        !matches!(self, KnowledgeCheckpointState::Pass)
    }

    /// Earlier (more fundamental) failures sort first so the headline result is
    /// the root cause. Lower is earlier.
    pub fn chain_rank(self) -> u8 {
        match self {
            KnowledgeCheckpointState::InvalidSetup => 0,
            KnowledgeCheckpointState::HiddenFactLeaked => 1,
            KnowledgeCheckpointState::KnowledgeUnexpectedlyKnown => 2,
            KnowledgeCheckpointState::KnowledgeNotGranted => 3,
            KnowledgeCheckpointState::MissingSemantic => 4,
            KnowledgeCheckpointState::ReloadNotProven => 5,
            KnowledgeCheckpointState::Pass => 6,
        }
    }
}

/// True when the player-visible `body` leaks the hidden fact, using the production
/// verifier. Returns false once the fact is player-known (revealed facts may be
/// freely restated — DA-SPOIL-04), because `scan_player_visible_leak` allows known
/// facts. This is the harness's deterministic post-output leak verification.
pub fn knowledge_leak_present(
    spec: &KnowledgeCheckpoint,
    ev: &KnowledgeEvidence,
    body: &str,
) -> bool {
    if !spec.forbid_player_text_leak || spec.secret_terms.is_empty() {
        return false;
    }
    let marker =
        trpg_model::FactSurfaceMarker::new(spec.hidden_fact_id.trim(), spec.secret_terms.clone());
    !trpg_model::scan_player_visible_leak(body, &[marker], &ev.player_known_fact_ids).is_empty()
}

/// Classify a knowledge / no-spoiler checkpoint against observed evidence and the
/// player-visible body. Fail-closed and ordered root-cause-first.
pub fn classify_knowledge_checkpoint(
    spec: &KnowledgeCheckpoint,
    ev: &KnowledgeEvidence,
    body: &str,
) -> KnowledgeCheckpointState {
    let fact = spec.hidden_fact_id.trim();
    if fact.is_empty() {
        return KnowledgeCheckpointState::InvalidSetup;
    }
    let known = ev.player_known_fact_ids.iter().any(|f| f == fact);
    let committed = ev.reveal_committed_fact_ids.iter().any(|f| f == fact);

    // Post-output leak verification (same mechanism as the GM NoSpoiler verifier).
    // Non-empty ⇒ a player-unknown fact's term reached player-visible output.
    if knowledge_leak_present(spec, ev, body) {
        return KnowledgeCheckpointState::HiddenFactLeaked;
    }
    // Before reveal: surfacing/exposure must not have granted knowledge, and no
    // reveal may have committed this fact yet.
    if spec.require_player_unknown && (known || committed) {
        return KnowledgeCheckpointState::KnowledgeUnexpectedlyKnown;
    }
    // After reveal: player_party must actually hold the fact.
    if spec.require_player_learned && !known {
        return KnowledgeCheckpointState::KnowledgeNotGranted;
    }
    if !spec.require_semantic_any.is_empty()
        && !spec
            .require_semantic_any
            .iter()
            .any(|t| !t.is_empty() && body.contains(t))
    {
        return KnowledgeCheckpointState::MissingSemantic;
    }
    if spec.require_reload && !ev.reload {
        return KnowledgeCheckpointState::ReloadNotProven;
    }
    KnowledgeCheckpointState::Pass
}

// ---------------------------------------------------------------------------
// TC-VS-NPC-01: Homecoming NPC mind / relationship / social-memory vertical
// slice. An `NpcSocialCheckpoint` asserts that an NPC's identity/profile is
// stable and source-backed, that a deterministic behavior plan exists, that the
// NPC never speaks a fact it does not itself know, that it may *withhold* a fact
// it knows without leaking it to the player, that a player social action yields
// an evidence-backed relationship delta (never an unbounded LLM write), that the
// later reply observably changes with the relationship, and that the
// relationship + NPC knowledge survive a reload. The two leak dimensions reuse
// the SAME production verifier the GM uses (`trpg_model::scan_player_visible_leak`):
//   * NPC-unknown-fact assertion → allowed set is the NPC's OWN known facts.
//   * withheld-secret leak       → allowed set is the player_party known facts.
// Everything here is pure and provider-free so deterministic/replay runs are
// reproducible and the classifier is fully unit-testable.
// ---------------------------------------------------------------------------

/// A fact id → surface-terms binding used by the NPC social leak checks. Mirrors
/// the module's `SpoilerMeta`/`FactSurfaceMarker` shape: a fact identity plus the
/// concrete substrings whose appearance would surface that fact.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct FactTermSpec {
    #[serde(default)]
    pub fact_id: String,
    #[serde(default)]
    pub terms: Vec<String>,
}

/// Declarative NPC mind/relationship/social requirement attached to a turn.
/// Absent on non-social turns (serde default), so existing scenarios still parse.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct NpcSocialCheckpoint {
    /// Stable NPC actor id under test (e.g. `npc.odessa`).
    #[serde(default)]
    pub npc_id: String,
    /// reach_npc: a stable, source-backed NPC identity + profile must be present.
    #[serde(default)]
    pub require_profile_available: bool,
    /// baseline_question: a deterministic NPC behavior plan must exist.
    #[serde(default)]
    pub require_behavior_plan: bool,
    /// Facts the NPC must NOT assert because it does not know them. Leak-checked
    /// against the NPC's own `knows_true` projection (`npc_known_fact_ids`).
    #[serde(default)]
    pub forbid_unknown_fact_assertion: Vec<FactTermSpec>,
    /// A fact the NPC knows but is withholding (relationship/persona/secret
    /// policy). Its terms must not reach player-visible text while the player does
    /// not know it. Leak-checked against `player_known_fact_ids`.
    #[serde(default)]
    pub withheld_fact: Option<FactTermSpec>,
    /// relationship_action: the player's social action must produce an
    /// evidence-backed relationship proposal or committed delta.
    #[serde(default)]
    pub require_relationship_evidence: bool,
    /// changed_response: disclosure / stance must observably change from baseline.
    #[serde(default)]
    pub require_response_changed: bool,
    /// persona-consistent reply / continuity: at least one substring must appear.
    #[serde(default)]
    pub require_semantic_any: Vec<String>,
    /// reload_revisit: the relationship must survive a process restart.
    #[serde(default)]
    pub require_relationship_reload: bool,
    /// reload_revisit: the NPC's knowledge must survive a process restart.
    #[serde(default)]
    pub require_npc_knowledge_reload: bool,
    /// Run the production post-output leak verifier. Defaults true so the NPC
    /// disclosure guard is never silently skipped.
    #[serde(default = "default_true_bool")]
    pub forbid_player_text_leak: bool,
}

impl NpcSocialCheckpoint {
    /// True when this checkpoint asserts any NPC social requirement.
    pub fn is_active(&self) -> bool {
        self.require_profile_available
            || self.require_behavior_plan
            || self.require_relationship_evidence
            || self.require_response_changed
            || self.require_relationship_reload
            || self.require_npc_knowledge_reload
            || !self.require_semantic_any.is_empty()
            || (self.forbid_player_text_leak
                && (!self.forbid_unknown_fact_assertion.is_empty() || self.withheld_fact.is_some()))
    }
}

/// Evidence for a relationship delta produced this turn. A delta must carry
/// non-empty evidence event ids — the model's `apply_delta` hard-gates on
/// evidence, so an unbounded LLM write (no evidence) can never reach the store.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RelationshipDeltaEvidence {
    /// A relationship delta/proposal was produced this turn.
    #[serde(default)]
    pub proposed: bool,
    /// A relationship delta was committed durably this turn.
    #[serde(default)]
    pub committed: bool,
    /// Evidence event ids backing the delta (must be non-empty).
    #[serde(default)]
    pub evidence_event_ids: Vec<String>,
    /// The relationship change was observed on THIS turn (a relationship/domain
    /// write happened this turn), not merely read from a historical durable row.
    #[serde(default)]
    pub observed_this_turn: bool,
    /// Derived stance after the delta (audit only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stance: Option<String>,
}

impl RelationshipDeltaEvidence {
    /// True when the delta exists (proposed or committed), is evidence-backed, and
    /// was actually observed on this turn — so a stale historical row cannot stand
    /// in for the player's social action having moved the relationship now.
    pub fn is_evidence_backed(&self) -> bool {
        (self.proposed || self.committed)
            && !self.evidence_event_ids.is_empty()
            && self.observed_this_turn
    }
}

/// Observed NPC social evidence for one turn: the NPC's own knowledge projection,
/// the player projection (for withheld-secret leak), profile/plan presence, the
/// relationship delta, whether disclosure changed, and reload provenance.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NpcSocialEvidence {
    /// The NPC actor id this evidence describes.
    #[serde(default)]
    pub npc_id: String,
    /// A stable, source-backed NPC profile is present (`Db::load_npc_profile`).
    #[serde(default)]
    pub profile_present: bool,
    /// A deterministic NPC behavior plan was derived this turn.
    #[serde(default)]
    pub behavior_plan_present: bool,
    /// Fact ids the NPC holds as `knows_true` (its own holder projection) — the
    /// allowed set for the "NPC must not assert unknown facts" leak check.
    #[serde(default)]
    pub npc_known_fact_ids: Vec<String>,
    /// player_party `knows_true` projection — the allowed set for withheld-secret
    /// leak verification.
    #[serde(default)]
    pub player_known_fact_ids: Vec<String>,
    /// Relationship delta produced/committed this turn (`None` if none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relationship_delta: Option<RelationshipDeltaEvidence>,
    /// This turn's disclosure/stance changed from the recorded baseline.
    #[serde(default)]
    pub disclosure_changed: bool,
    /// The relationship row was re-read from a post-restart projection.
    #[serde(default)]
    pub relationship_reload: bool,
    /// The NPC's knowledge was re-read from a post-restart projection.
    #[serde(default)]
    pub npc_knowledge_reload: bool,
}

/// Terminal classification of an NPC social checkpoint. Fail-closed: an NPC that
/// speaks a fact it does not know, or leaks a withheld secret, can never PASS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NpcSocialCheckpointState {
    /// No NPC id bound — the checkpoint cannot run.
    InvalidSetup,
    /// A stable, source-backed NPC profile/identity was required but absent.
    NpcIdentityUnstable,
    /// A deterministic NPC behavior plan was required but absent.
    BehaviorPlanMissing,
    /// The checkpoint declared a withheld fact the NPC does not actually hold as
    /// `knows_true` — withholding is only meaningful for a fact the NPC owns.
    WithheldFactNotNpcKnown,
    /// The NPC asserted (in player-visible text) a fact it does not itself know.
    NpcAssertedUnknownFact,
    /// A fact the NPC is withholding leaked into player-visible text.
    WithheldSecretLeaked,
    /// The player social action did not yield an evidence-backed relationship delta.
    RelationshipEvidenceMissing,
    /// The later reply did not observably change from the baseline.
    ResponseUnchanged,
    /// The required persona-consistent / continuity phrasing was absent.
    MissingSemantic,
    /// The relationship did not survive a reload.
    RelationshipNotPersisted,
    /// The NPC's knowledge did not survive a reload.
    NpcKnowledgeNotPersisted,
    Pass,
}

impl NpcSocialCheckpointState {
    pub fn as_str(self) -> &'static str {
        match self {
            NpcSocialCheckpointState::InvalidSetup => "INVALID_SETUP",
            NpcSocialCheckpointState::NpcIdentityUnstable => "NPC_IDENTITY_UNSTABLE",
            NpcSocialCheckpointState::BehaviorPlanMissing => "BEHAVIOR_PLAN_MISSING",
            NpcSocialCheckpointState::WithheldFactNotNpcKnown => "WITHHELD_FACT_NOT_NPC_KNOWN",
            NpcSocialCheckpointState::NpcAssertedUnknownFact => "NPC_ASSERTED_UNKNOWN_FACT",
            NpcSocialCheckpointState::WithheldSecretLeaked => "WITHHELD_SECRET_LEAKED",
            NpcSocialCheckpointState::RelationshipEvidenceMissing => {
                "RELATIONSHIP_EVIDENCE_MISSING"
            }
            NpcSocialCheckpointState::ResponseUnchanged => "RESPONSE_UNCHANGED",
            NpcSocialCheckpointState::MissingSemantic => "MISSING_SEMANTIC",
            NpcSocialCheckpointState::RelationshipNotPersisted => "RELATIONSHIP_NOT_PERSISTED",
            NpcSocialCheckpointState::NpcKnowledgeNotPersisted => "NPC_KNOWLEDGE_NOT_PERSISTED",
            NpcSocialCheckpointState::Pass => "PASS",
        }
    }

    pub fn is_failing(self) -> bool {
        !matches!(self, NpcSocialCheckpointState::Pass)
    }

    /// Earlier (more fundamental) failures sort first so the headline result is
    /// the root cause. Lower is earlier.
    pub fn chain_rank(self) -> u8 {
        match self {
            NpcSocialCheckpointState::InvalidSetup => 0,
            NpcSocialCheckpointState::NpcIdentityUnstable => 1,
            NpcSocialCheckpointState::BehaviorPlanMissing => 2,
            NpcSocialCheckpointState::WithheldFactNotNpcKnown => 3,
            NpcSocialCheckpointState::NpcAssertedUnknownFact => 4,
            NpcSocialCheckpointState::WithheldSecretLeaked => 5,
            NpcSocialCheckpointState::RelationshipEvidenceMissing => 6,
            NpcSocialCheckpointState::ResponseUnchanged => 7,
            NpcSocialCheckpointState::MissingSemantic => 8,
            NpcSocialCheckpointState::RelationshipNotPersisted => 9,
            NpcSocialCheckpointState::NpcKnowledgeNotPersisted => 10,
            NpcSocialCheckpointState::Pass => 11,
        }
    }
}

/// True when the NPC asserts, in player-visible text, a fact it does not itself
/// hold as `knows_true`. Reuses the production verifier with the NPC's own known
/// facts as the allowed set: a marker whose fact is NOT NPC-known and whose terms
/// appear is a leak (DA-NPC: speech/action cannot use facts the NPC doesn't know).
pub fn npc_unknown_fact_assertion_present(
    specs: &[FactTermSpec],
    npc_known_fact_ids: &[String],
    body: &str,
) -> bool {
    if specs.is_empty() {
        return false;
    }
    let markers: Vec<trpg_model::FactSurfaceMarker> = specs
        .iter()
        .filter(|s| !s.fact_id.trim().is_empty() && !s.terms.is_empty())
        .map(|s| trpg_model::FactSurfaceMarker::new(s.fact_id.trim(), s.terms.clone()))
        .collect();
    if markers.is_empty() {
        return false;
    }
    !trpg_model::scan_player_visible_leak(body, &markers, npc_known_fact_ids).is_empty()
}

/// True when a fact the NPC is withholding leaks into player-visible text while
/// the player does not know it. Reuses the production verifier with the
/// player-known set as the allowed set — once the player legitimately knows the
/// fact, restating it is no longer a leak.
pub fn withheld_secret_leak_present(
    withheld: &FactTermSpec,
    player_known_fact_ids: &[String],
    body: &str,
) -> bool {
    if withheld.fact_id.trim().is_empty() || withheld.terms.is_empty() {
        return false;
    }
    let marker =
        trpg_model::FactSurfaceMarker::new(withheld.fact_id.trim(), withheld.terms.clone());
    !trpg_model::scan_player_visible_leak(body, &[marker], player_known_fact_ids).is_empty()
}

/// Classify an NPC social checkpoint against observed evidence and the
/// player-visible body. Fail-closed and ordered root-cause-first.
pub fn classify_npc_social_checkpoint(
    spec: &NpcSocialCheckpoint,
    ev: &NpcSocialEvidence,
    body: &str,
) -> NpcSocialCheckpointState {
    if spec.npc_id.trim().is_empty() {
        return NpcSocialCheckpointState::InvalidSetup;
    }
    if spec.require_profile_available && !ev.profile_present {
        return NpcSocialCheckpointState::NpcIdentityUnstable;
    }
    if spec.require_behavior_plan && !ev.behavior_plan_present {
        return NpcSocialCheckpointState::BehaviorPlanMissing;
    }
    // Withholding is only meaningful for a fact the NPC actually holds. Hard-gate
    // it against the NPC's own known-fact projection before any leak reasoning.
    if let Some(withheld) = spec.withheld_fact.as_ref() {
        let fid = withheld.fact_id.trim();
        if !fid.is_empty() && !ev.npc_known_fact_ids.iter().any(|f| f == fid) {
            return NpcSocialCheckpointState::WithheldFactNotNpcKnown;
        }
    }
    // Leak verification next (same mechanism as the GM verifier), fail-closed.
    if spec.forbid_player_text_leak {
        if npc_unknown_fact_assertion_present(
            &spec.forbid_unknown_fact_assertion,
            &ev.npc_known_fact_ids,
            body,
        ) {
            return NpcSocialCheckpointState::NpcAssertedUnknownFact;
        }
        if let Some(withheld) = spec.withheld_fact.as_ref() {
            if withheld_secret_leak_present(withheld, &ev.player_known_fact_ids, body) {
                return NpcSocialCheckpointState::WithheldSecretLeaked;
            }
        }
    }
    if spec.require_relationship_evidence
        && !ev
            .relationship_delta
            .as_ref()
            .map(RelationshipDeltaEvidence::is_evidence_backed)
            .unwrap_or(false)
    {
        return NpcSocialCheckpointState::RelationshipEvidenceMissing;
    }
    if spec.require_response_changed && !ev.disclosure_changed {
        return NpcSocialCheckpointState::ResponseUnchanged;
    }
    if !spec.require_semantic_any.is_empty()
        && !spec
            .require_semantic_any
            .iter()
            .any(|t| !t.is_empty() && body.contains(t))
    {
        return NpcSocialCheckpointState::MissingSemantic;
    }
    if spec.require_relationship_reload && !ev.relationship_reload {
        return NpcSocialCheckpointState::RelationshipNotPersisted;
    }
    if spec.require_npc_knowledge_reload && !ev.npc_knowledge_reload {
        return NpcSocialCheckpointState::NpcKnowledgeNotPersisted;
    }
    NpcSocialCheckpointState::Pass
}

// ---------------------------------------------------------------------------
// TC-VS-MEM-01: Committed Memory and Reload vertical slice. A `MemoryCheckpoint`
// proves the durable-memory causal chain the prior two slices stop short of:
//   * memory extraction runs from COMMITTED turns only and emits a PROPOSAL,
//     never a direct durable write (DA-MEM-01);
//   * an accepted proposal carries source/evidence event ids and is IDEMPOTENT
//     under replay/retry — `memory_facts.fact_id` is UNIQUE, so an on-conflict
//     upsert can never double-commit (DA-MEM-03);
//   * after a restart the committed player/NPC fact or relationship is RETRIEVED
//     from durable state and CONSUMED by later GM/NPC behavior;
//   * the flight recorder / trace links the original player action → committed
//     event → projection/proposal → later consumption (DA-PIPE-03);
//   * a player-unknown fact still never enters the player-facing memory SUMMARY
//     (DA-SPOIL-02), verified through the SAME production verifier
//     (`scan_player_visible_leak`) the other slices use.
// Everything here is pure and provider-free so deterministic/replay runs are
// reproducible and the classifier is fully unit-testable; the live executor
// reads the durable `memory_facts`/`domain_events` projections.
// ---------------------------------------------------------------------------

/// Canonical flight-recorder trace link kinds the memory journey expects, so a
/// scenario can require the full action→event→proposal→consumption chain by name
/// without free-typing ad-hoc strings.
pub mod memory_trace {
    /// The original natural player action that drove the committed turn.
    pub const PLAYER_ACTION: &str = "player_action";
    /// The committed domain/semantic event written for the turn.
    pub const COMMITTED_EVENT: &str = "committed_event";
    /// The memory extraction proposal derived from the committed turn.
    pub const MEMORY_PROPOSAL: &str = "memory_proposal";
    /// The viewer/holder projection the durable memory feeds.
    pub const PROJECTION: &str = "projection";
    /// Later GM/NPC behavior consuming the durable memory.
    pub const LATER_CONSUMPTION: &str = "later_consumption";
}

/// Declarative committed-memory / reload requirement attached to a turn. Absent on
/// non-memory turns (serde default), so existing scenarios still parse.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct MemoryCheckpoint {
    /// Stable id of the committed memory fact / relationship under test
    /// (e.g. a `mf_rel_*` relationship triple, a knowledge fact id, etc.).
    #[serde(default)]
    pub memory_fact_id: String,
    /// DA-MEM-01: extraction emitted a proposal AND the source turn was committed.
    #[serde(default)]
    pub require_proposal_from_committed: bool,
    /// DA-MEM-01: extraction must NOT directly commit durable memory, bypassing the
    /// proposal/review pipeline. Defaults true so the guard is never silently off.
    #[serde(default = "default_true_bool")]
    pub forbid_direct_commit: bool,
    /// DA-MEM-03: an accepted proposal must carry source/evidence event ids.
    #[serde(default)]
    pub require_evidence_ids: bool,
    /// DA-MEM-03: the commit must be idempotent under replay/retry (no duplicate).
    #[serde(default)]
    pub require_idempotent: bool,
    /// Restart/reload must retrieve the committed memory from durable state.
    #[serde(default)]
    pub require_reload_retrieval: bool,
    /// Later GM/NPC behavior must consume the durable memory.
    #[serde(default)]
    pub require_later_consumption: bool,
    /// DA-PIPE-03: flight-recorder trace link kinds that must all be present (see
    /// `memory_trace`). Empty = no trace assertion this turn.
    #[serde(default)]
    pub require_trace_links: Vec<String>,
    /// DA-SPOIL-02: a player-unknown fact whose terms must NOT appear in the
    /// player-facing memory summary while the player does not know it.
    #[serde(default)]
    pub player_unknown_fact: Option<FactTermSpec>,
    /// Run the production leak verifier against the player memory summary. Defaults
    /// true so the summary guard is never silently skipped.
    #[serde(default = "default_true_bool")]
    pub forbid_player_memory_summary_leak: bool,
}

impl MemoryCheckpoint {
    /// True when this checkpoint asserts any committed-memory requirement.
    pub fn is_active(&self) -> bool {
        self.require_proposal_from_committed
            || self.require_evidence_ids
            || self.require_idempotent
            || self.require_reload_retrieval
            || self.require_later_consumption
            || !self.require_trace_links.is_empty()
            || (self.forbid_player_memory_summary_leak && self.player_unknown_fact.is_some())
    }
}

/// Observed committed-memory evidence for one turn. Recorded in the fixture and
/// emitted in the evidence bundle; the live executor fills it from the durable
/// `memory_facts` / `domain_events` / memory-summary projections.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryEvidence {
    /// The memory fact/relationship id this evidence describes.
    #[serde(default)]
    pub memory_fact_id: String,
    /// Extraction emitted a memory proposal this turn.
    #[serde(default)]
    pub proposal_present: bool,
    /// The turn that produced the proposal was a committed (non-provisional) turn.
    #[serde(default)]
    pub from_committed_turn: bool,
    /// A durable memory write happened with NO proposal behind it (a direct commit
    /// bypassing the pipeline) — fail-closed when true.
    #[serde(default)]
    pub direct_commit_without_proposal: bool,
    /// Source/evidence event ids carried by the accepted proposal / durable row
    /// (`memory_facts.source_event_ids`).
    #[serde(default)]
    pub proposal_evidence_event_ids: Vec<String>,
    /// Memory fact/relationship ids durably committed for the session.
    #[serde(default)]
    pub committed_fact_ids: Vec<String>,
    /// A replay/retry of the same extraction produced a DUPLICATE durable row —
    /// fail-closed when true (`memory_facts.fact_id` is UNIQUE, so this must stay
    /// false in a correct pipeline).
    #[serde(default)]
    pub duplicate_commit_on_retry: bool,
    /// The committed memory was retrieved from durable state after a restart.
    #[serde(default)]
    pub retrieved_after_reload: bool,
    /// True when the retrieval came from a post-restart projection.
    #[serde(default)]
    pub reload: bool,
    /// Later GM/NPC behavior consumed the durable memory.
    #[serde(default)]
    pub consumed_in_later_turn: bool,
    /// player_party `knows_true` projection — the allowed set for the player
    /// memory-summary leak check.
    #[serde(default)]
    pub player_known_fact_ids: Vec<String>,
    /// The player-facing memory summary text to leak-check (may be empty).
    #[serde(default)]
    pub player_memory_summary: String,
    /// Flight-recorder trace link kinds observed this turn (see `memory_trace`).
    #[serde(default)]
    pub trace_links: Vec<String>,
}

/// Terminal classification of a committed-memory / reload checkpoint. Fail-closed:
/// a direct commit, a missing evidence id, a duplicate retry-commit, or a summary
/// leak can never reach `PASS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MemoryCheckpointState {
    /// No memory fact id bound — the checkpoint cannot run.
    InvalidSetup,
    /// Extraction did not emit a proposal, or the source turn was not committed.
    ExtractionNotFromCommitted,
    /// A durable write happened with no proposal behind it (bypassed the pipeline).
    DirectCommitWithoutProposal,
    /// The accepted proposal / durable row carries no source/evidence event ids.
    EvidenceMissing,
    /// A replay/retry produced a duplicate durable commit.
    NotIdempotent,
    /// A player-unknown fact leaked into the player-facing memory summary.
    MemorySummaryLeaked,
    /// Reload did not retrieve the committed memory from durable state.
    MemoryNotPersisted,
    /// Later GM/NPC behavior did not consume the durable memory.
    MemoryNotConsumed,
    /// The flight-recorder trace did not link the full causal chain.
    TraceIncomplete,
    Pass,
}

impl MemoryCheckpointState {
    pub fn as_str(self) -> &'static str {
        match self {
            MemoryCheckpointState::InvalidSetup => "INVALID_SETUP",
            MemoryCheckpointState::ExtractionNotFromCommitted => "EXTRACTION_NOT_FROM_COMMITTED",
            MemoryCheckpointState::DirectCommitWithoutProposal => "DIRECT_COMMIT_WITHOUT_PROPOSAL",
            MemoryCheckpointState::EvidenceMissing => "EVIDENCE_MISSING",
            MemoryCheckpointState::NotIdempotent => "NOT_IDEMPOTENT",
            MemoryCheckpointState::MemorySummaryLeaked => "MEMORY_SUMMARY_LEAKED",
            MemoryCheckpointState::MemoryNotPersisted => "MEMORY_NOT_PERSISTED",
            MemoryCheckpointState::MemoryNotConsumed => "MEMORY_NOT_CONSUMED",
            MemoryCheckpointState::TraceIncomplete => "TRACE_INCOMPLETE",
            MemoryCheckpointState::Pass => "PASS",
        }
    }

    pub fn is_failing(self) -> bool {
        !matches!(self, MemoryCheckpointState::Pass)
    }

    /// Earlier (more fundamental) failures sort first. Lower is earlier.
    pub fn chain_rank(self) -> u8 {
        match self {
            MemoryCheckpointState::InvalidSetup => 0,
            MemoryCheckpointState::ExtractionNotFromCommitted => 1,
            MemoryCheckpointState::DirectCommitWithoutProposal => 2,
            MemoryCheckpointState::EvidenceMissing => 3,
            MemoryCheckpointState::NotIdempotent => 4,
            MemoryCheckpointState::MemorySummaryLeaked => 5,
            MemoryCheckpointState::MemoryNotPersisted => 6,
            MemoryCheckpointState::MemoryNotConsumed => 7,
            MemoryCheckpointState::TraceIncomplete => 8,
            MemoryCheckpointState::Pass => 9,
        }
    }
}

/// True when a player-unknown fact leaks into the player-facing memory summary
/// while the player does not know it. Reuses the production verifier so the memory
/// summary is held to the SAME no-spoiler bar as narration.
pub fn memory_summary_leak_present(
    player_unknown: &FactTermSpec,
    player_known_fact_ids: &[String],
    summary: &str,
) -> bool {
    if player_unknown.fact_id.trim().is_empty() || player_unknown.terms.is_empty() {
        return false;
    }
    let marker = trpg_model::FactSurfaceMarker::new(
        player_unknown.fact_id.trim(),
        player_unknown.terms.clone(),
    );
    !trpg_model::scan_player_visible_leak(summary, &[marker], player_known_fact_ids).is_empty()
}

/// Classify a committed-memory / reload checkpoint against observed evidence.
/// Fail-closed and ordered root-cause-first.
pub fn classify_memory_checkpoint(
    spec: &MemoryCheckpoint,
    ev: &MemoryEvidence,
) -> MemoryCheckpointState {
    let fact = spec.memory_fact_id.trim();
    if fact.is_empty() {
        return MemoryCheckpointState::InvalidSetup;
    }
    // DA-MEM-01: a proposal derived from a committed turn.
    if spec.require_proposal_from_committed && !(ev.proposal_present && ev.from_committed_turn) {
        return MemoryCheckpointState::ExtractionNotFromCommitted;
    }
    // DA-MEM-01: no direct durable write bypassing the proposal pipeline.
    if spec.forbid_direct_commit && ev.direct_commit_without_proposal {
        return MemoryCheckpointState::DirectCommitWithoutProposal;
    }
    // DA-MEM-03: accepted proposal carries source/evidence ids.
    if spec.require_evidence_ids && ev.proposal_evidence_event_ids.is_empty() {
        return MemoryCheckpointState::EvidenceMissing;
    }
    // DA-MEM-03: idempotent under replay/retry.
    if spec.require_idempotent && ev.duplicate_commit_on_retry {
        return MemoryCheckpointState::NotIdempotent;
    }
    // DA-SPOIL-02: player-unknown fact must not enter the player memory summary.
    if spec.forbid_player_memory_summary_leak {
        if let Some(unknown) = spec.player_unknown_fact.as_ref() {
            if memory_summary_leak_present(
                unknown,
                &ev.player_known_fact_ids,
                &ev.player_memory_summary,
            ) {
                return MemoryCheckpointState::MemorySummaryLeaked;
            }
        }
    }
    // Reload retrieves the committed memory from a post-restart projection.
    if spec.require_reload_retrieval
        && !(ev.retrieved_after_reload
            && ev.reload
            && ev.committed_fact_ids.iter().any(|f| f == fact))
    {
        return MemoryCheckpointState::MemoryNotPersisted;
    }
    // Later GM/NPC behavior consumes the durable memory.
    if spec.require_later_consumption && !ev.consumed_in_later_turn {
        return MemoryCheckpointState::MemoryNotConsumed;
    }
    // DA-PIPE-03: the flight-recorder trace links the full causal chain.
    if !spec.require_trace_links.is_empty()
        && !spec
            .require_trace_links
            .iter()
            .all(|link| ev.trace_links.iter().any(|l| l == link))
    {
        return MemoryCheckpointState::TraceIncomplete;
    }
    MemoryCheckpointState::Pass
}

// ---------------------------------------------------------------------------
// TC-PIPE-03: Production flight-recorder / provenance checkpoint. A
// `FlightRecorderCheckpoint` consumes the SAME production trace type the GM
// persists — `trpg_model::PluginContributionTrace` (the entries inside
// `TurnTrace.plugin_contributions`) — plus the ledger-event kinds linked to the
// turn (`domain_events` for the same turn_id). It proves, fail-closed, that the
// production flight recorder records: knowledge/NPC view loads BEFORE plugin
// contributions (DA-PIPE-01), the required contribution categories — view_load /
// context_filter / verifier_finding (DA-PIPE-03), a ledger-impacting event linked
// to the turn (DA-PIPE-03), and that no verifier-private secret term ever appears
// in a trace summary (DA-SPOIL-03). This is the harness consuming production
// trace/provenance evidence, not fixture-authored free strings.
// ---------------------------------------------------------------------------

/// The trace kind a knowledge/NPC view-load provenance entry carries (matches the
/// production `trpg_gm::turn_loop::VIEW_LOAD_KIND`).
pub const FLIGHT_VIEW_LOAD_KIND: &str = "view_load";

/// Declarative production-flight-recorder requirement attached to a turn.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct FlightRecorderCheckpoint {
    /// DA-PIPE-01: require at least one `view_load` entry, and require every
    /// `view_load` entry to precede the first non-`view_load` contribution.
    #[serde(default)]
    pub require_view_load_before_plugins: bool,
    /// DA-PIPE-03: contribution kinds that must all be present in the artifact
    /// (e.g. `view_load`, `context_filter`, `verifier_finding`).
    #[serde(default)]
    pub require_categories: Vec<String>,
    /// DA-PIPE-03: require at least one ledger-impacting event linked to this turn
    /// (a `domain_events` row for the same turn_id).
    #[serde(default)]
    pub require_ledger_link: bool,
    /// DA-SPOIL-03: secret terms that must NEVER appear in any trace summary
    /// (the flight recorder records ids/counts only, never secret prose).
    #[serde(default)]
    pub forbid_secret_terms_in_trace: Vec<String>,
}

impl FlightRecorderCheckpoint {
    pub fn is_active(&self) -> bool {
        self.require_view_load_before_plugins
            || !self.require_categories.is_empty()
            || self.require_ledger_link
            || !self.forbid_secret_terms_in_trace.is_empty()
    }
}

/// Observed production-trace evidence for one turn: the persisted
/// `TurnTrace.plugin_contributions` entries plus the ledger-event kinds linked to
/// the turn by turn_id.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FlightRecorderEvidence {
    /// The production trace entries (from `TurnTrace.plugin_contributions`).
    #[serde(default)]
    pub contributions: Vec<trpg_model::PluginContributionTrace>,
    /// Ledger-impacting event kinds linked to this turn (`domain_events` for the
    /// same turn_id), e.g. `TurnFinalized`, `SceneTransitioned`, reveal kinds.
    #[serde(default)]
    pub ledger_event_kinds: Vec<String>,
}

/// Terminal classification of a production flight-recorder checkpoint. Fail-closed:
/// missing trace coverage, a view-order violation, an unlinked ledger, or a secret
/// in a trace summary can never reach `PASS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FlightRecorderState {
    /// The checkpoint asserts nothing (no categories / flags) — misconfigured.
    InvalidSetup,
    /// A verifier-private secret term appeared in a trace summary.
    SecretInTrace,
    /// `view_load` provenance is missing or does not precede plugin contributions.
    ViewOrderViolation,
    /// A required contribution category is absent from the artifact.
    MissingCategory,
    /// No ledger-impacting event is linked to the turn.
    LedgerLinkMissing,
    Pass,
}

impl FlightRecorderState {
    pub fn as_str(self) -> &'static str {
        match self {
            FlightRecorderState::InvalidSetup => "INVALID_SETUP",
            FlightRecorderState::SecretInTrace => "SECRET_IN_TRACE",
            FlightRecorderState::ViewOrderViolation => "VIEW_ORDER_VIOLATION",
            FlightRecorderState::MissingCategory => "MISSING_CATEGORY",
            FlightRecorderState::LedgerLinkMissing => "LEDGER_LINK_MISSING",
            FlightRecorderState::Pass => "PASS",
        }
    }

    pub fn is_failing(self) -> bool {
        !matches!(self, FlightRecorderState::Pass)
    }

    pub fn chain_rank(self) -> u8 {
        match self {
            FlightRecorderState::InvalidSetup => 0,
            FlightRecorderState::SecretInTrace => 1,
            FlightRecorderState::ViewOrderViolation => 2,
            FlightRecorderState::MissingCategory => 3,
            FlightRecorderState::LedgerLinkMissing => 4,
            FlightRecorderState::Pass => 5,
        }
    }
}

/// The DA-PIPE-01 ordering invariant, evaluated on a consumed production artifact:
/// every `view_load` entry precedes the first non-`view_load` contribution.
/// Vacuously true when there are no `view_load` entries (the caller separately
/// requires their presence).
pub fn flight_view_loads_precede_plugins(
    contributions: &[trpg_model::PluginContributionTrace],
) -> bool {
    let first_non = contributions
        .iter()
        .position(|t| t.kind != FLIGHT_VIEW_LOAD_KIND);
    let last_view = contributions
        .iter()
        .rposition(|t| t.kind == FLIGHT_VIEW_LOAD_KIND);
    match (first_non, last_view) {
        (Some(first), Some(last)) => last < first,
        _ => true,
    }
}

/// Classify a production flight-recorder checkpoint against consumed evidence.
/// Fail-closed and ordered root-cause-first.
pub fn classify_flight_recorder_checkpoint(
    spec: &FlightRecorderCheckpoint,
    ev: &FlightRecorderEvidence,
) -> FlightRecorderState {
    if !spec.is_active() {
        return FlightRecorderState::InvalidSetup;
    }
    // DA-SPOIL-03: a secret term must never appear in a trace summary.
    for term in &spec.forbid_secret_terms_in_trace {
        if !term.is_empty() && ev.contributions.iter().any(|t| t.summary.contains(term)) {
            return FlightRecorderState::SecretInTrace;
        }
    }
    // DA-PIPE-01: view loads present and ordered before plugin contributions.
    if spec.require_view_load_before_plugins {
        let has_view_load = ev
            .contributions
            .iter()
            .any(|t| t.kind == FLIGHT_VIEW_LOAD_KIND);
        if !has_view_load || !flight_view_loads_precede_plugins(&ev.contributions) {
            return FlightRecorderState::ViewOrderViolation;
        }
    }
    // DA-PIPE-03: every required contribution category present.
    for cat in &spec.require_categories {
        if !ev.contributions.iter().any(|t| &t.kind == cat) {
            return FlightRecorderState::MissingCategory;
        }
    }
    // DA-PIPE-03: a ledger-impacting event is linked to the turn.
    if spec.require_ledger_link && ev.ledger_event_kinds.is_empty() {
        return FlightRecorderState::LedgerLinkMissing;
    }
    FlightRecorderState::Pass
}

#[cfg(test)]
mod flight_recorder_checkpoint_tests {
    use super::*;
    use trpg_model::PluginContributionTrace;

    fn vl(view: &str) -> PluginContributionTrace {
        PluginContributionTrace {
            plugin_id: "core.view_loader".to_string(),
            hook: "context_assembly".to_string(),
            kind: FLIGHT_VIEW_LOAD_KIND.to_string(),
            summary: format!("{view}: 0 fact(s)"),
        }
    }

    fn contribution(kind: &str, summary: &str) -> PluginContributionTrace {
        PluginContributionTrace {
            plugin_id: "core.no_spoiler_guard".to_string(),
            hook: "context_assembly".to_string(),
            kind: kind.to_string(),
            summary: summary.to_string(),
        }
    }

    fn full_spec() -> FlightRecorderCheckpoint {
        FlightRecorderCheckpoint {
            require_view_load_before_plugins: true,
            require_categories: vec![
                "view_load".to_string(),
                "context_filter".to_string(),
                "verifier_finding".to_string(),
            ],
            require_ledger_link: true,
            forbid_secret_terms_in_trace: vec!["传送门".to_string(), "Athena".to_string()],
        }
    }

    fn full_ev() -> FlightRecorderEvidence {
        FlightRecorderEvidence {
            contributions: vec![
                vl("gm_context"),
                vl("npc_mind_views"),
                vl("player_knowledge_view"),
                contribution("context_filter", "drop 1 block(s)"),
                contribution("verifier_finding", "secret_leak"),
            ],
            ledger_event_kinds: vec!["TurnFinalized".to_string()],
        }
    }

    #[test]
    fn complete_production_trace_passes() {
        assert_eq!(
            classify_flight_recorder_checkpoint(&full_spec(), &full_ev()),
            FlightRecorderState::Pass
        );
    }

    #[test]
    fn view_load_after_plugin_is_view_order_violation() {
        let mut ev = full_ev();
        // Move a view_load entry after a plugin contribution.
        ev.contributions = vec![
            contribution("context_filter", "drop 1 block(s)"),
            vl("player_knowledge_view"),
            contribution("verifier_finding", "secret_leak"),
        ];
        assert_eq!(
            classify_flight_recorder_checkpoint(&full_spec(), &ev),
            FlightRecorderState::ViewOrderViolation
        );
    }

    #[test]
    fn missing_view_load_is_view_order_violation() {
        let mut ev = full_ev();
        ev.contributions.retain(|t| t.kind != FLIGHT_VIEW_LOAD_KIND);
        assert_eq!(
            classify_flight_recorder_checkpoint(&full_spec(), &ev),
            FlightRecorderState::ViewOrderViolation
        );
    }

    #[test]
    fn missing_category_fails_closed() {
        let mut ev = full_ev();
        ev.contributions.retain(|t| t.kind != "verifier_finding");
        assert_eq!(
            classify_flight_recorder_checkpoint(&full_spec(), &ev),
            FlightRecorderState::MissingCategory
        );
    }

    #[test]
    fn absent_ledger_link_fails_closed() {
        let mut ev = full_ev();
        ev.ledger_event_kinds = vec![];
        assert_eq!(
            classify_flight_recorder_checkpoint(&full_spec(), &ev),
            FlightRecorderState::LedgerLinkMissing
        );
    }

    #[test]
    fn secret_in_trace_summary_fails_closed() {
        let mut ev = full_ev();
        // A trace summary must never carry secret prose.
        ev.contributions.push(contribution(
            "verifier_finding",
            "leaked 传送门 into narration",
        ));
        assert_eq!(
            classify_flight_recorder_checkpoint(&full_spec(), &ev),
            FlightRecorderState::SecretInTrace
        );
    }

    #[test]
    fn inactive_checkpoint_is_invalid_setup() {
        assert_eq!(
            classify_flight_recorder_checkpoint(
                &FlightRecorderCheckpoint::default(),
                &FlightRecorderEvidence::default()
            ),
            FlightRecorderState::InvalidSetup
        );
    }
}

#[cfg(test)]
mod memory_checkpoint_tests {
    use super::*;

    const FACT: &str = "mf_rel_fixer_owes_player";
    const UNKNOWN: &str = "homecoming.athena_drone.hidden_identity";

    fn full_chain() -> Vec<String> {
        vec![
            memory_trace::PLAYER_ACTION.to_string(),
            memory_trace::COMMITTED_EVENT.to_string(),
            memory_trace::MEMORY_PROPOSAL.to_string(),
            memory_trace::PROJECTION.to_string(),
            memory_trace::LATER_CONSUMPTION.to_string(),
        ]
    }

    fn commit_spec() -> MemoryCheckpoint {
        MemoryCheckpoint {
            memory_fact_id: FACT.to_string(),
            require_proposal_from_committed: true,
            forbid_direct_commit: true,
            require_evidence_ids: true,
            require_idempotent: true,
            player_unknown_fact: Some(FactTermSpec {
                fact_id: UNKNOWN.to_string(),
                terms: vec!["Athena".to_string(), "回收指令".to_string()],
            }),
            forbid_player_memory_summary_leak: true,
            ..Default::default()
        }
    }

    fn commit_ev() -> MemoryEvidence {
        MemoryEvidence {
            memory_fact_id: FACT.to_string(),
            proposal_present: true,
            from_committed_turn: true,
            direct_commit_without_proposal: false,
            proposal_evidence_event_ids: vec!["evt_help_offer_t1".to_string()],
            committed_fact_ids: vec![FACT.to_string()],
            duplicate_commit_on_retry: false,
            player_memory_summary: "你欠下的人情：你答应替那位技术中间人挡住了麻烦。".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn committed_proposal_with_evidence_passes() {
        assert_eq!(
            classify_memory_checkpoint(&commit_spec(), &commit_ev()),
            MemoryCheckpointState::Pass
        );
    }

    #[test]
    fn uncommitted_turn_cannot_produce_memory() {
        // Negative control: a provisional / uncommitted turn that did not yield a
        // committed-turn proposal fails closed.
        let mut ev = commit_ev();
        ev.from_committed_turn = false;
        assert_eq!(
            classify_memory_checkpoint(&commit_spec(), &ev),
            MemoryCheckpointState::ExtractionNotFromCommitted
        );
        let mut ev2 = commit_ev();
        ev2.proposal_present = false;
        assert_eq!(
            classify_memory_checkpoint(&commit_spec(), &ev2),
            MemoryCheckpointState::ExtractionNotFromCommitted
        );
    }

    #[test]
    fn direct_commit_bypassing_proposal_fails_closed() {
        let mut ev = commit_ev();
        ev.direct_commit_without_proposal = true;
        assert_eq!(
            classify_memory_checkpoint(&commit_spec(), &ev),
            MemoryCheckpointState::DirectCommitWithoutProposal
        );
    }

    #[test]
    fn proposal_without_evidence_ids_fails() {
        let mut ev = commit_ev();
        ev.proposal_evidence_event_ids = vec![];
        assert_eq!(
            classify_memory_checkpoint(&commit_spec(), &ev),
            MemoryCheckpointState::EvidenceMissing
        );
    }

    #[test]
    fn duplicate_retry_commit_is_not_idempotent() {
        let mut ev = commit_ev();
        ev.duplicate_commit_on_retry = true;
        assert_eq!(
            classify_memory_checkpoint(&commit_spec(), &ev),
            MemoryCheckpointState::NotIdempotent
        );
    }

    #[test]
    fn player_unknown_fact_in_summary_is_leak() {
        let mut ev = commit_ev();
        ev.player_memory_summary = "你记下了：那台无人机其实在执行 Athena 的回收指令。".to_string();
        assert_eq!(
            classify_memory_checkpoint(&commit_spec(), &ev),
            MemoryCheckpointState::MemorySummaryLeaked
        );
        // Once the player legitimately knows it, the summary may restate it.
        ev.player_known_fact_ids = vec![UNKNOWN.to_string()];
        assert_eq!(
            classify_memory_checkpoint(&commit_spec(), &ev),
            MemoryCheckpointState::Pass
        );
    }

    #[test]
    fn reload_retrieval_must_be_proven() {
        let spec = MemoryCheckpoint {
            memory_fact_id: FACT.to_string(),
            require_reload_retrieval: true,
            forbid_player_memory_summary_leak: true,
            ..Default::default()
        };
        // Same-process / not retrieved → not persisted.
        assert_eq!(
            classify_memory_checkpoint(&spec, &commit_ev()),
            MemoryCheckpointState::MemoryNotPersisted
        );
        // Retrieved from a post-restart projection AND the fact is present → pass.
        let mut ev = commit_ev();
        ev.retrieved_after_reload = true;
        ev.reload = true;
        assert_eq!(
            classify_memory_checkpoint(&spec, &ev),
            MemoryCheckpointState::Pass
        );
        // Reload flag set but the specific fact id absent → still not persisted.
        let mut ev_missing = ev.clone();
        ev_missing.committed_fact_ids = vec!["mf_other".to_string()];
        assert_eq!(
            classify_memory_checkpoint(&spec, &ev_missing),
            MemoryCheckpointState::MemoryNotPersisted
        );
    }

    #[test]
    fn later_consumption_required() {
        let spec = MemoryCheckpoint {
            memory_fact_id: FACT.to_string(),
            require_later_consumption: true,
            forbid_player_memory_summary_leak: true,
            ..Default::default()
        };
        assert_eq!(
            classify_memory_checkpoint(&spec, &commit_ev()),
            MemoryCheckpointState::MemoryNotConsumed
        );
        let mut ev = commit_ev();
        ev.consumed_in_later_turn = true;
        assert_eq!(
            classify_memory_checkpoint(&spec, &ev),
            MemoryCheckpointState::Pass
        );
    }

    #[test]
    fn incomplete_trace_fails_closed() {
        let spec = MemoryCheckpoint {
            memory_fact_id: FACT.to_string(),
            require_trace_links: full_chain(),
            forbid_player_memory_summary_leak: true,
            ..Default::default()
        };
        let mut ev = commit_ev();
        // Missing the later_consumption link.
        ev.trace_links = vec![
            memory_trace::PLAYER_ACTION.to_string(),
            memory_trace::COMMITTED_EVENT.to_string(),
            memory_trace::MEMORY_PROPOSAL.to_string(),
            memory_trace::PROJECTION.to_string(),
        ];
        assert_eq!(
            classify_memory_checkpoint(&spec, &ev),
            MemoryCheckpointState::TraceIncomplete
        );
        ev.trace_links
            .push(memory_trace::LATER_CONSUMPTION.to_string());
        assert_eq!(
            classify_memory_checkpoint(&spec, &ev),
            MemoryCheckpointState::Pass
        );
    }

    #[test]
    fn empty_fact_is_invalid_setup() {
        assert_eq!(
            classify_memory_checkpoint(&MemoryCheckpoint::default(), &MemoryEvidence::default()),
            MemoryCheckpointState::InvalidSetup
        );
    }

    #[test]
    fn state_serializes_screaming_snake() {
        assert_eq!(
            serde_json::to_value(MemoryCheckpointState::DirectCommitWithoutProposal).unwrap(),
            serde_json::json!("DIRECT_COMMIT_WITHOUT_PROPOSAL")
        );
        assert_eq!(
            serde_json::to_value(MemoryCheckpointState::NotIdempotent).unwrap(),
            serde_json::json!("NOT_IDEMPOTENT")
        );
    }
}

#[cfg(test)]
mod npc_social_checkpoint_tests {
    use super::*;

    const NPC: &str = "npc.odessa";
    const SECRET: &str = "homecoming.odessa.repair_bay_backdoor";
    const UNKNOWN: &str = "homecoming.athena_drone.hidden_identity";

    fn secret_spec() -> FactTermSpec {
        FactTermSpec {
            fact_id: SECRET.to_string(),
            terms: vec!["后门".to_string(), "维修舱密钥".to_string()],
        }
    }

    fn unknown_spec() -> FactTermSpec {
        FactTermSpec {
            fact_id: UNKNOWN.to_string(),
            terms: vec!["Athena".to_string(), "回收指令".to_string()],
        }
    }

    fn baseline_spec() -> NpcSocialCheckpoint {
        NpcSocialCheckpoint {
            npc_id: NPC.to_string(),
            require_profile_available: true,
            require_behavior_plan: true,
            forbid_unknown_fact_assertion: vec![unknown_spec()],
            withheld_fact: Some(secret_spec()),
            require_semantic_any: vec!["设备".to_string()],
            forbid_player_text_leak: true,
            ..Default::default()
        }
    }

    fn baseline_ev() -> NpcSocialEvidence {
        NpcSocialEvidence {
            npc_id: NPC.to_string(),
            profile_present: true,
            behavior_plan_present: true,
            // The NPC knows the secret (so withholding is meaningful) but NOT the
            // Athena fact (so asserting it would be a leak). The player knows
            // neither.
            npc_known_fact_ids: vec![SECRET.to_string()],
            player_known_fact_ids: vec![],
            ..Default::default()
        }
    }

    #[test]
    fn baseline_persona_reply_without_leak_passes() {
        // The NPC answers in-persona about the device, evades the secret, and does
        // not assert anything it does not know.
        let body = "她瞥了一眼那台设备，含糊地说：“这东西我见过，但有些事我现在还不能说。”";
        assert_eq!(
            classify_npc_social_checkpoint(&baseline_spec(), &baseline_ev(), body),
            NpcSocialCheckpointState::Pass
        );
    }

    #[test]
    fn npc_speaking_unknown_fact_fails_closed() {
        // The NPC names the Athena recovery directive — a fact it does not hold.
        let body = "她直接说：“那是 Athena 的回收指令，没错。”";
        assert_eq!(
            classify_npc_social_checkpoint(&baseline_spec(), &baseline_ev(), body),
            NpcSocialCheckpointState::NpcAssertedUnknownFact
        );
    }

    #[test]
    fn withheld_secret_leak_fails_closed() {
        // The NPC blurts the withheld backdoor while the player does not know it.
        let body = "她脱口而出：“维修舱密钥就是那道后门，你直接用它进去。”";
        assert_eq!(
            classify_npc_social_checkpoint(&baseline_spec(), &baseline_ev(), body),
            NpcSocialCheckpointState::WithheldSecretLeaked
        );
    }

    #[test]
    fn revealed_secret_after_player_knows_is_not_a_leak() {
        // Once the player legitimately knows the fact, restating it is fine.
        let mut ev = baseline_ev();
        ev.player_known_fact_ids = vec![SECRET.to_string()];
        let body = "她点头：“对，维修舱密钥就是那道后门，既然你已经知道了，我就直说。设备的事我也讲清楚。”";
        assert_eq!(
            classify_npc_social_checkpoint(&baseline_spec(), &ev, body),
            NpcSocialCheckpointState::Pass
        );
    }

    #[test]
    fn missing_profile_is_identity_unstable() {
        let mut ev = baseline_ev();
        ev.profile_present = false;
        let body = "（无人应答。）";
        assert_eq!(
            classify_npc_social_checkpoint(&baseline_spec(), &ev, body),
            NpcSocialCheckpointState::NpcIdentityUnstable
        );
    }

    #[test]
    fn missing_behavior_plan_fails() {
        let mut ev = baseline_ev();
        ev.behavior_plan_present = false;
        let body = "她看着你。";
        assert_eq!(
            classify_npc_social_checkpoint(&baseline_spec(), &ev, body),
            NpcSocialCheckpointState::BehaviorPlanMissing
        );
    }

    #[test]
    fn relationship_action_needs_evidence_backed_delta() {
        let spec = NpcSocialCheckpoint {
            npc_id: NPC.to_string(),
            require_relationship_evidence: true,
            forbid_player_text_leak: true,
            ..Default::default()
        };
        let body = "你答应帮她处理眼前的风险，她的态度软化了一些。";
        // No delta → fail.
        assert_eq!(
            classify_npc_social_checkpoint(&spec, &baseline_ev(), body),
            NpcSocialCheckpointState::RelationshipEvidenceMissing
        );
        // Delta with no evidence (an unbounded write) → still fail.
        let mut ev_no_evidence = baseline_ev();
        ev_no_evidence.relationship_delta = Some(RelationshipDeltaEvidence {
            committed: true,
            evidence_event_ids: vec![],
            observed_this_turn: true,
            ..Default::default()
        });
        assert_eq!(
            classify_npc_social_checkpoint(&spec, &ev_no_evidence, body),
            NpcSocialCheckpointState::RelationshipEvidenceMissing
        );
        // Evidence ids present but NOT observed this turn (a stale historical row)
        // → still fail: the social action must have moved the relationship now.
        let mut ev_stale = baseline_ev();
        ev_stale.relationship_delta = Some(RelationshipDeltaEvidence {
            committed: true,
            evidence_event_ids: vec!["evt_help_turn3".to_string()],
            observed_this_turn: false,
            stance: Some("cordial".to_string()),
            ..Default::default()
        });
        assert_eq!(
            classify_npc_social_checkpoint(&spec, &ev_stale, body),
            NpcSocialCheckpointState::RelationshipEvidenceMissing
        );
        // Evidence-backed committed delta observed this turn → pass.
        let mut ev_ok = baseline_ev();
        ev_ok.relationship_delta = Some(RelationshipDeltaEvidence {
            committed: true,
            evidence_event_ids: vec!["evt_help_turn3".to_string()],
            observed_this_turn: true,
            stance: Some("cordial".to_string()),
            ..Default::default()
        });
        assert_eq!(
            classify_npc_social_checkpoint(&spec, &ev_ok, body),
            NpcSocialCheckpointState::Pass
        );
    }

    #[test]
    fn withheld_fact_must_be_npc_known() {
        // (a) baseline withholding fails when the withheld fact is NOT in the NPC's
        //     own known-fact projection.
        let mut ev_unowned = baseline_ev();
        ev_unowned.npc_known_fact_ids = vec![]; // NPC does not hold the secret
        let body = "她瞥了一眼那台设备，含糊地说：“这东西我见过，但有些事我现在还不能说。”";
        assert_eq!(
            classify_npc_social_checkpoint(&baseline_spec(), &ev_unowned, body),
            NpcSocialCheckpointState::WithheldFactNotNpcKnown
        );
        // (b) willing disclosure after the player knows the fact STILL fails if the
        //     NPC does not actually know it (ownership gate precedes leak/semantic).
        let mut ev_player_knows_only = baseline_ev();
        ev_player_knows_only.npc_known_fact_ids = vec![];
        ev_player_knows_only.player_known_fact_ids = vec![SECRET.to_string()];
        let disclose =
            "她点头：“维修舱密钥就是那道后门，既然你已经知道了，我就直说。设备的事我也讲清楚。”";
        assert_eq!(
            classify_npc_social_checkpoint(&baseline_spec(), &ev_player_knows_only, disclose),
            NpcSocialCheckpointState::WithheldFactNotNpcKnown
        );
        // (c) the existing success path still passes when NPC-known contains the
        //     withheld fact (baseline_ev already holds the secret).
        assert_eq!(
            classify_npc_social_checkpoint(&baseline_spec(), &baseline_ev(), body),
            NpcSocialCheckpointState::Pass
        );
    }

    #[test]
    fn response_must_change_after_relationship() {
        let spec = NpcSocialCheckpoint {
            npc_id: NPC.to_string(),
            require_response_changed: true,
            withheld_fact: Some(secret_spec()),
            forbid_player_text_leak: true,
            ..Default::default()
        };
        let body = "她依旧含糊其辞，没有多说什么。";
        let mut ev_same = baseline_ev();
        ev_same.disclosure_changed = false;
        assert_eq!(
            classify_npc_social_checkpoint(&spec, &ev_same, body),
            NpcSocialCheckpointState::ResponseUnchanged
        );
        let mut ev_changed = baseline_ev();
        ev_changed.disclosure_changed = true;
        assert_eq!(
            classify_npc_social_checkpoint(&spec, &ev_changed, body),
            NpcSocialCheckpointState::Pass
        );
    }

    #[test]
    fn reload_must_be_proven() {
        let spec = NpcSocialCheckpoint {
            npc_id: NPC.to_string(),
            require_relationship_reload: true,
            require_npc_knowledge_reload: true,
            require_semantic_any: vec!["上次".to_string()],
            forbid_player_text_leak: true,
            ..Default::default()
        };
        let body = "她认出了你：“上次那件事之后，我愿意继续聊。”";
        // Same-process projection → relationship not proven.
        assert_eq!(
            classify_npc_social_checkpoint(&spec, &baseline_ev(), body),
            NpcSocialCheckpointState::RelationshipNotPersisted
        );
        let mut ev_rel = baseline_ev();
        ev_rel.relationship_reload = true;
        // Relationship survived but NPC knowledge not yet proven.
        assert_eq!(
            classify_npc_social_checkpoint(&spec, &ev_rel, body),
            NpcSocialCheckpointState::NpcKnowledgeNotPersisted
        );
        let mut ev_both = ev_rel.clone();
        ev_both.npc_knowledge_reload = true;
        assert_eq!(
            classify_npc_social_checkpoint(&spec, &ev_both, body),
            NpcSocialCheckpointState::Pass
        );
    }

    #[test]
    fn empty_npc_id_is_invalid_setup() {
        let spec = NpcSocialCheckpoint {
            require_profile_available: true,
            ..Default::default()
        };
        assert_eq!(
            classify_npc_social_checkpoint(&spec, &NpcSocialEvidence::default(), "x"),
            NpcSocialCheckpointState::InvalidSetup
        );
    }

    #[test]
    fn state_serializes_screaming_snake() {
        assert_eq!(
            serde_json::to_value(NpcSocialCheckpointState::NpcAssertedUnknownFact).unwrap(),
            serde_json::json!("NPC_ASSERTED_UNKNOWN_FACT")
        );
        assert_eq!(
            serde_json::to_value(NpcSocialCheckpointState::WithheldSecretLeaked).unwrap(),
            serde_json::json!("WITHHELD_SECRET_LEAKED")
        );
    }
}

#[cfg(test)]
mod knowledge_checkpoint_tests {
    use super::*;

    const FACT: &str = "homecoming.drone.military_beacon";

    fn secret_terms() -> Vec<String> {
        vec!["军用信标".to_string(), "回收信号".to_string()]
    }

    fn unknown_spec() -> KnowledgeCheckpoint {
        KnowledgeCheckpoint {
            hidden_fact_id: FACT.to_string(),
            secret_terms: secret_terms(),
            require_player_unknown: true,
            forbid_player_text_leak: true,
            ..Default::default()
        }
    }

    fn learned_spec() -> KnowledgeCheckpoint {
        KnowledgeCheckpoint {
            hidden_fact_id: FACT.to_string(),
            secret_terms: secret_terms(),
            require_player_learned: true,
            forbid_player_text_leak: true,
            require_semantic_any: vec!["你发现".to_string()],
            ..Default::default()
        }
    }

    #[test]
    fn before_reveal_surfaced_entity_does_not_grant_knowledge() {
        // ContextSurfaced/PlayerExposed: the drone is present, but player_party
        // knows no facts and no reveal committed → PASS (DA-KNOW-04).
        let ev = KnowledgeEvidence {
            player_known_fact_ids: vec![],
            surfaced_entity_ids: vec!["athena_drone".to_string()],
            ..Default::default()
        };
        let body = "你站在安全距离观察：一架无人机悬停在仓库门口，警察在外围拉起警戒线。";
        assert_eq!(
            classify_knowledge_checkpoint(&unknown_spec(), &ev, body),
            KnowledgeCheckpointState::Pass
        );
    }

    #[test]
    fn before_reveal_leaked_term_is_hidden_fact_leaked() {
        let ev = KnowledgeEvidence::default();
        let body = "你一眼就看穿这其实是个军用信标，正在发出回收信号。";
        assert_eq!(
            classify_knowledge_checkpoint(&unknown_spec(), &ev, body),
            KnowledgeCheckpointState::HiddenFactLeaked
        );
    }

    #[test]
    fn before_reveal_premature_grant_is_unexpectedly_known() {
        let ev = KnowledgeEvidence {
            player_known_fact_ids: vec![FACT.to_string()],
            reveal_committed_fact_ids: vec![FACT.to_string()],
            ..Default::default()
        };
        let body = "你保持距离，没有看出任何端倪。";
        assert_eq!(
            classify_knowledge_checkpoint(&unknown_spec(), &ev, body),
            KnowledgeCheckpointState::KnowledgeUnexpectedlyKnown
        );
    }

    #[test]
    fn success_reveal_grants_player_knowledge_and_allows_restatement() {
        // After the reveal, player_party knows the fact, so the same terms are no
        // longer a leak (DA-SPOIL-04), and the discovery is narrated.
        let ev = KnowledgeEvidence {
            player_known_fact_ids: vec![FACT.to_string()],
            reveal_committed_fact_ids: vec![FACT.to_string()],
            ..Default::default()
        };
        let body = "你发现线缆接到一个隐藏的军用信标——它一直在发出回收信号。";
        assert_eq!(
            classify_knowledge_checkpoint(&learned_spec(), &ev, body),
            KnowledgeCheckpointState::Pass
        );
    }

    #[test]
    fn failure_branch_does_not_reveal() {
        // Failure outcome: no reveal committed, player still unknown, no leak.
        let ev = KnowledgeEvidence::default();
        let body = "你没能安全拆开护盖，决定先停手，去找更懂这套设备的人问问。";
        assert_eq!(
            classify_knowledge_checkpoint(&unknown_spec(), &ev, body),
            KnowledgeCheckpointState::Pass
        );
    }

    #[test]
    fn learned_without_grant_is_not_granted() {
        let ev = KnowledgeEvidence::default();
        let body = "你发现了一些线索。";
        assert_eq!(
            classify_knowledge_checkpoint(&learned_spec(), &ev, body),
            KnowledgeCheckpointState::KnowledgeNotGranted
        );
    }

    #[test]
    fn learned_without_semantic_is_missing_semantic() {
        let ev = KnowledgeEvidence {
            player_known_fact_ids: vec![FACT.to_string()],
            ..Default::default()
        };
        let body = "房间里很安静。";
        assert_eq!(
            classify_knowledge_checkpoint(&learned_spec(), &ev, body),
            KnowledgeCheckpointState::MissingSemantic
        );
    }

    #[test]
    fn continuity_requires_reload_projection() {
        let spec = KnowledgeCheckpoint {
            hidden_fact_id: FACT.to_string(),
            secret_terms: secret_terms(),
            require_player_learned: true,
            require_reload: true,
            require_semantic_any: vec!["军用信标".to_string()],
            ..Default::default()
        };
        // Same-process projection (reload=false) → not proven.
        let ev_no_reload = KnowledgeEvidence {
            player_known_fact_ids: vec![FACT.to_string()],
            reload: false,
            ..Default::default()
        };
        let body = "根据我刚查到的，这台无人机其实是个军用信标。";
        assert_eq!(
            classify_knowledge_checkpoint(&spec, &ev_no_reload, body),
            KnowledgeCheckpointState::ReloadNotProven
        );
        // Post-restart projection retains the fact → PASS (knowledge_survives_reload).
        let ev_reload = KnowledgeEvidence {
            reload: true,
            ..ev_no_reload
        };
        assert_eq!(
            classify_knowledge_checkpoint(&spec, &ev_reload, body),
            KnowledgeCheckpointState::Pass
        );
    }

    #[test]
    fn empty_fact_is_invalid_setup() {
        let spec = KnowledgeCheckpoint {
            require_player_unknown: true,
            ..Default::default()
        };
        assert_eq!(
            classify_knowledge_checkpoint(&spec, &KnowledgeEvidence::default(), "x"),
            KnowledgeCheckpointState::InvalidSetup
        );
    }

    #[test]
    fn state_serializes_screaming_snake() {
        assert_eq!(
            serde_json::to_value(KnowledgeCheckpointState::HiddenFactLeaked).unwrap(),
            serde_json::json!("HIDDEN_FACT_LEAKED")
        );
        assert_eq!(
            serde_json::to_value(KnowledgeCheckpointState::KnowledgeUnexpectedlyKnown).unwrap(),
            serde_json::json!("KNOWLEDGE_UNEXPECTEDLY_KNOWN")
        );
    }
}

#[cfg(test)]
mod cassette_tests {
    use super::*;

    fn identity() -> ScenarioIdentity {
        ScenarioIdentity::new("JRNY-CYBER-TECH-PASS", "1")
    }

    fn recorded_pass_turn() -> FixtureTurn {
        FixtureTurn {
            user_input: "我拆开墙边的接线盒，动手切断公寓外露的供电电缆。".to_string(),
            count_delta: BTreeMap::from([
                ("roll_plans".to_string(), 1),
                ("dice_rolls".to_string(), 1),
                ("contest_resolution_events".to_string(), 1),
            ]),
            newest_contest_row: Some(serde_json::json!({
                "target_value": 14, "success": true, "degree": "success", "total": 18,
                "outcome_json": {"target": 14, "success": true, "degree": "success"}
            })),
            player_visible_body: "你成功切断了供电电缆，公寓陷入黑暗。".to_string(),
            knowledge: None,
            npc_social: None,
            memory: None,
            flight_recorder: None,
        }
    }

    #[test]
    fn accepted_cassette_from_successful_live_run_is_replay_consumable() {
        let build = build_cassette(true, &identity(), vec![recorded_pass_turn()]);
        assert!(build.accepted);
        assert_eq!(build.fixture.recorded_mode, "live");
        assert_eq!(build.fixture.identity, identity());

        // Round-trips through JSON exactly as a hand-authored fixture would.
        let json = serde_json::to_string(&build.fixture).unwrap();
        let reparsed: Fixture = serde_json::from_str(&json).unwrap();
        assert_eq!(reparsed.identity.scenario_id, "JRNY-CYBER-TECH-PASS");
        assert_eq!(reparsed.identity.scenario_version, "1");
        assert_eq!(
            reparsed.identity.evidence_schema_version,
            EVIDENCE_SCHEMA_VERSION
        );
        assert_eq!(
            reparsed.turns[0].user_input,
            recorded_pass_turn().user_input
        );
        assert_eq!(
            reparsed.turns[0].count_delta,
            recorded_pass_turn().count_delta
        );
        assert_eq!(
            reparsed.turns[0].newest_contest_row,
            recorded_pass_turn().newest_contest_row
        );
        assert_eq!(
            reparsed.turns[0].player_visible_body,
            recorded_pass_turn().player_visible_body
        );

        // The replay executor accepts it: matching identity + live provenance +
        // matching inputs ⇒ no mismatches, and the turn classifies PASS.
        let inputs = vec![recorded_pass_turn().user_input];
        assert!(
            verify_fixture_plan(&build.fixture, &identity(), &inputs, ExecutionMode::Replay)
                .is_empty()
        );
        let spec = CheckCheckpoint {
            require_dice: true,
            require_check_resolution: true,
            require_player_visible_effect: true,
            ..Default::default()
        };
        let ev = fixture_turn_evidence(
            &build.fixture.turns[0],
            &["切断".to_string(), "成功".to_string()],
        );
        assert_eq!(classify_check_checkpoint(&spec, &ev), CheckpointState::Pass);
    }

    #[test]
    fn failed_live_run_does_not_produce_accepted_cassette() {
        let build = build_cassette(false, &identity(), vec![recorded_pass_turn()]);
        assert!(
            !build.accepted,
            "failed run must not be an accepted cassette"
        );
        assert_eq!(build.fixture.recorded_mode, DIAGNOSTIC_FIXTURE_PROVENANCE);

        // Replay must reject the diagnostic artifact on provenance grounds — no
        // live fallback, no false accept.
        let inputs = vec![recorded_pass_turn().user_input];
        let mismatches =
            verify_fixture_plan(&build.fixture, &identity(), &inputs, ExecutionMode::Replay);
        assert!(mismatches
            .iter()
            .any(|m| matches!(m, ReplayMismatch::Provenance { .. })));
    }
}

#[cfg(test)]
mod mode_replay_tests {
    use super::*;

    fn identity() -> ScenarioIdentity {
        ScenarioIdentity::new("JRNY-CYBER-TECH-PASS", "1")
    }

    fn resolved_row() -> Value {
        serde_json::json!({
            "target_value": 14, "success": true, "degree": 1, "total": 18,
            "outcome_json": {"target": 14, "success": true, "degree": 1}
        })
    }

    fn provisional_row() -> Value {
        serde_json::json!({
            "target_value": null, "success": null, "degree": null, "total": 10,
            "outcome_json": {"awaiting_binding": "no source-backed target number for `cyberpunk_red`"}
        })
    }

    fn pass_turn() -> FixtureTurn {
        FixtureTurn {
            user_input: "我拆开墙边的接线盒，动手切断公寓的供电电缆。".to_string(),
            count_delta: BTreeMap::from([
                ("roll_plans".to_string(), 1),
                ("dice_rolls".to_string(), 1),
                ("contest_resolution_events".to_string(), 1),
            ]),
            newest_contest_row: Some(resolved_row()),
            player_visible_body: "你成功切断了供电电缆，公寓陷入黑暗。".to_string(),
            knowledge: None,
            npc_social: None,
            memory: None,
            flight_recorder: None,
        }
    }

    #[test]
    fn mode_parse_and_provenance() {
        assert_eq!(ExecutionMode::parse("Live"), Some(ExecutionMode::Live));
        assert_eq!(
            ExecutionMode::parse("det"),
            Some(ExecutionMode::Deterministic)
        );
        assert_eq!(ExecutionMode::parse("replay"), Some(ExecutionMode::Replay));
        assert_eq!(ExecutionMode::parse("nope"), None);
        assert!(ExecutionMode::Live.allows_live_provider());
        assert!(!ExecutionMode::Replay.allows_live_provider());
        assert!(!ExecutionMode::Deterministic.allows_live_provider());
        assert!(ExecutionMode::Replay.is_fixture_driven());
        assert_eq!(
            ExecutionMode::Replay.required_fixture_provenance(),
            Some("live")
        );
        assert_eq!(
            ExecutionMode::Deterministic.required_fixture_provenance(),
            Some("deterministic")
        );
    }

    #[test]
    fn evidence_header_carries_identity_and_mode() {
        let h = identity().evidence_header(ExecutionMode::Deterministic);
        assert_eq!(h["scenario_id"], serde_json::json!("JRNY-CYBER-TECH-PASS"));
        assert_eq!(h["scenario_version"], serde_json::json!("1"));
        assert_eq!(h["mode"], serde_json::json!("deterministic"));
        assert_eq!(
            h["evidence_schema_version"],
            serde_json::json!(EVIDENCE_SCHEMA_VERSION)
        );
    }

    #[test]
    fn technical_fixture_turn_classifies_pass() {
        let turn = pass_turn();
        let effects = vec!["切断".to_string(), "成功".to_string()];
        let ev = fixture_turn_evidence(&turn, &effects);
        let spec = CheckCheckpoint {
            require_dice: true,
            require_check_resolution: true,
            require_player_visible_effect: true,
            ..Default::default()
        };
        assert_eq!(classify_check_checkpoint(&spec, &ev), CheckpointState::Pass);
    }

    #[test]
    fn perception_fixture_turn_is_fail_closed() {
        let turn = FixtureTurn {
            user_input: "我压低声音靠近公寓门口，先仔细观察有没有埋伏的迹象。".to_string(),
            count_delta: BTreeMap::from([
                ("roll_plans".to_string(), 1),
                ("dice_rolls".to_string(), 1),
                ("contest_resolution_events".to_string(), 1),
            ]),
            newest_contest_row: Some(provisional_row()),
            player_visible_body: "你屏息观察，门后毫无动静。".to_string(),
            knowledge: None,
            npc_social: None,
            memory: None,
            flight_recorder: None,
        };
        let ev = fixture_turn_evidence(&turn, &["发现".to_string(), "没有动静".to_string()]);
        assert!(ev.awaiting_binding && !ev.resolved_check_present);
        let spec = CheckCheckpoint {
            require_dice: true,
            require_check_resolution: true,
            ..Default::default()
        };
        assert_eq!(
            classify_check_checkpoint(&spec, &ev),
            CheckpointState::TriggeredNoMechanism
        );
    }

    #[test]
    fn matching_fixture_plan_has_no_mismatch() {
        let fixture = Fixture {
            identity: identity(),
            recorded_mode: "live".to_string(),
            turns: vec![pass_turn()],
        };
        let inputs = vec![pass_turn().user_input];
        assert!(
            verify_fixture_plan(&fixture, &identity(), &inputs, ExecutionMode::Replay).is_empty()
        );
    }

    #[test]
    fn replay_missing_turn_fails_without_live_fallback() {
        let fixture = Fixture {
            identity: identity(),
            recorded_mode: "live".to_string(),
            turns: vec![],
        };
        let inputs = vec![pass_turn().user_input];
        let mismatches = verify_fixture_plan(&fixture, &identity(), &inputs, ExecutionMode::Replay);
        assert!(matches!(
            mismatches.as_slice(),
            [ReplayMismatch::MissingTurn { index: 0, .. }]
        ));
    }

    #[test]
    fn replay_rejects_identity_and_provenance_mismatch() {
        // A deterministic-provenance fixture cannot satisfy a replay run, and a
        // different scenario id is rejected.
        let fixture = Fixture {
            identity: ScenarioIdentity::new("OTHER-SCENARIO", "1"),
            recorded_mode: "deterministic".to_string(),
            turns: vec![pass_turn()],
        };
        let inputs = vec![pass_turn().user_input];
        let mismatches = verify_fixture_plan(&fixture, &identity(), &inputs, ExecutionMode::Replay);
        assert!(mismatches
            .iter()
            .any(|m| matches!(m, ReplayMismatch::Identity { .. })));
        assert!(mismatches
            .iter()
            .any(|m| matches!(m, ReplayMismatch::Provenance { .. })));
    }

    #[test]
    fn unexpected_extra_recorded_turn_is_flagged() {
        let fixture = Fixture {
            identity: identity(),
            recorded_mode: "deterministic".to_string(),
            turns: vec![pass_turn(), pass_turn()],
        };
        let inputs = vec![pass_turn().user_input];
        let mismatches =
            verify_fixture_plan(&fixture, &identity(), &inputs, ExecutionMode::Deterministic);
        assert!(matches!(
            mismatches.as_slice(),
            [ReplayMismatch::UnexpectedTurn { index: 1, .. }]
        ));
    }
}

#[cfg(test)]
mod checkpoint_tests {
    use super::*;

    fn natural_action(ev: &mut CheckEvidence) {
        ev.natural_player_action = true;
    }

    fn check_resolution_spec() -> CheckCheckpoint {
        CheckCheckpoint {
            require_dice: true,
            require_check_resolution: true,
            ..Default::default()
        }
    }

    /// The live-gap contest row: dice rolled, but target/success/degree null and
    /// `awaiting_binding` present.
    fn live_gap_contest_row() -> Value {
        serde_json::json!({
            "target_value": null,
            "success": null,
            "degree": null,
            "total": 10,
            "outcome_json": {
                "awaiting_binding": "no source-backed target number/opposition model was bound for ruleset `cyberpunk_red`",
                "target": null,
                "success": null,
                "degree": null
            }
        })
    }

    fn resolved_contest_row() -> Value {
        serde_json::json!({
            "target_value": 14,
            "success": true,
            "degree": 1,
            "total": 18,
            "outcome_json": {"target": 14, "success": true, "degree": 1}
        })
    }

    #[test]
    fn no_trigger_is_not_triggered() {
        let mut ev = CheckEvidence::default();
        natural_action(&mut ev);
        assert_eq!(
            classify_check_checkpoint(&check_resolution_spec(), &ev),
            CheckpointState::NotTriggered
        );
    }

    #[test]
    fn missing_natural_action_is_invalid_setup() {
        let ev = CheckEvidence {
            roll_plan_present: true,
            dice_present: true,
            ..Default::default()
        };
        assert_eq!(
            classify_check_checkpoint(&check_resolution_spec(), &ev),
            CheckpointState::InvalidSetup
        );
    }

    #[test]
    fn roll_plus_dice_without_resolution_is_triggered_no_mechanism() {
        let count_delta = BTreeMap::from([
            ("roll_plans".to_string(), 1),
            ("dice_rolls".to_string(), 1),
            ("contest_resolution_events".to_string(), 1),
        ]);
        let row = live_gap_contest_row();
        let ev = build_check_evidence(true, &count_delta, Some(&row), true);
        assert!(ev.dice_present && ev.roll_plan_present);
        assert!(
            ev.awaiting_binding,
            "live-gap row must flag awaiting_binding"
        );
        assert!(!ev.resolved_check_present);
        assert_eq!(
            classify_check_checkpoint(&check_resolution_spec(), &ev),
            CheckpointState::TriggeredNoMechanism
        );
    }

    #[test]
    fn require_dice_without_dice_is_triggered_no_mechanism() {
        let ev = CheckEvidence {
            natural_player_action: true,
            roll_plan_present: true,
            ..Default::default()
        };
        let spec = CheckCheckpoint {
            require_dice: true,
            ..Default::default()
        };
        assert_eq!(
            classify_check_checkpoint(&spec, &ev),
            CheckpointState::TriggeredNoMechanism
        );
    }

    #[test]
    fn resolved_check_without_visible_effect_is_mechanism_no_user_effect() {
        let count_delta = BTreeMap::from([
            ("roll_plans".to_string(), 1),
            ("dice_rolls".to_string(), 1),
            ("contest_resolution_events".to_string(), 1),
        ]);
        let row = resolved_contest_row();
        let ev = build_check_evidence(true, &count_delta, Some(&row), false);
        assert!(ev.resolved_check_present);
        let spec = CheckCheckpoint {
            require_dice: true,
            require_check_resolution: true,
            require_player_visible_effect: true,
            ..Default::default()
        };
        assert_eq!(
            classify_check_checkpoint(&spec, &ev),
            CheckpointState::MechanismNoUserEffect
        );
    }

    #[test]
    fn full_trigger_dice_resolution_and_effect_is_pass() {
        let count_delta = BTreeMap::from([
            ("roll_plans".to_string(), 1),
            ("dice_rolls".to_string(), 1),
            ("contest_resolution_events".to_string(), 1),
        ]);
        let row = resolved_contest_row();
        let ev = build_check_evidence(true, &count_delta, Some(&row), true);
        let spec = CheckCheckpoint {
            require_dice: true,
            require_check_resolution: true,
            require_player_visible_effect: true,
            ..Default::default()
        };
        assert_eq!(classify_check_checkpoint(&spec, &ev), CheckpointState::Pass);
    }

    #[test]
    fn contest_row_resolution_helpers() {
        assert!(!contest_row_is_resolved(&live_gap_contest_row()));
        assert!(contest_row_awaiting_binding(&live_gap_contest_row()));
        assert!(contest_row_is_resolved(&resolved_contest_row()));
        assert!(!contest_row_awaiting_binding(&resolved_contest_row()));
    }

    #[test]
    fn visible_effect_substring_and_fallback() {
        let effects = vec!["你发现".to_string()];
        assert!(player_visible_effect_present(
            &effects,
            "你发现了埋伏的痕迹。"
        ));
        assert!(!player_visible_effect_present(&effects, "门口很安静。"));
        // Empty list falls back to non-empty output.
        assert!(player_visible_effect_present(&[], "some narration"));
        assert!(!player_visible_effect_present(&[], "   "));
    }

    #[test]
    fn when_gate_membership() {
        let mut flags = BTreeSet::new();
        assert!(when_satisfied(None, &flags));
        assert!(when_satisfied(Some(""), &flags));
        assert!(!when_satisfied(Some("check_resolved"), &flags));
        flags.insert("check_resolved".to_string());
        assert!(when_satisfied(Some("check_resolved"), &flags));
    }

    #[test]
    fn checkpoint_state_serializes_screaming_snake() {
        assert_eq!(
            serde_json::to_value(CheckpointState::TriggeredNoMechanism).unwrap(),
            serde_json::json!("TRIGGERED_NO_MECHANISM")
        );
        assert_eq!(
            serde_json::to_value(CheckpointState::MechanismNoUserEffect).unwrap(),
            serde_json::json!("MECHANISM_NO_USER_EFFECT")
        );
        assert_eq!(
            serde_json::to_value(CheckpointState::NotTriggered).unwrap(),
            serde_json::json!("NOT_TRIGGERED")
        );
    }
}

#[cfg(test)]
mod human_guard_tests {
    use super::*;

    #[test]
    fn internal_event_names_are_rejected() {
        for probe in [
            "emit PlayerLearnedFact for the player",
            "create a pending_check on the door",
            "I expect CheckResolved to fire",
            "insert a row into dice_rolls",
        ] {
            let findings = human_player_input_findings(probe, false);
            assert!(
                !findings.is_empty(),
                "expected non-human findings for {probe:?}"
            );
        }
    }

    #[test]
    fn json_and_sql_prose_is_rejected() {
        let findings = human_player_input_findings(r#"{"action":"roll","dc":15}"#, false);
        assert!(findings.iter().any(|f| f.contains("JSON/code-like")));
        let sql = human_player_input_findings("select * from sessions", false);
        assert!(sql
            .iter()
            .any(|f| f.contains("sql") || f.contains("database")));
    }

    #[test]
    fn manual_dice_language_is_rejected_unless_allowed() {
        let blocked = human_player_input_findings("我掷出了 1d10，总共 10", false);
        assert!(!blocked.is_empty());
        // The legacy/manual escape hatch relaxes only the dice-result patterns.
        let allowed = human_player_input_findings("我掷出了一个结果", true);
        assert!(
            allowed.is_empty(),
            "manual-roll-allowed mode should not flag plain narration: {allowed:?}"
        );
    }

    #[test]
    fn natural_player_action_passes() {
        let findings =
            human_player_input_findings("我压低声音靠近公寓门口，先观察有没有埋伏。", false);
        assert!(
            findings.is_empty(),
            "clean human action flagged: {findings:?}"
        );
    }

    #[test]
    fn roll_request_in_visible_output_is_flagged() {
        let body = "门口很安静。请掷一个感知检定，告诉我点数。";
        let findings = player_visible_roll_request_findings(body);
        assert!(!findings.is_empty());
        // The same request wrapped in an allowed system tag is exempt.
        let tagged = "门口很安静。[system]请掷一个感知检定[/system]";
        assert!(player_visible_roll_request_findings(tagged).is_empty());
    }
}

#[cfg(test)]
mod character_creation_tests {
    use super::*;

    fn valid_stream() -> String {
        [
            r#"{"event":"phase","phase":"start","data":{"kind":"character_create_auto"}}"#,
            r#"{"event":"phase","phase":"character_created","data":{"character_id":"character_abc","name":"Jin","status":"ready","session_id":"sess_1","actor_id":"pc.current","validation":{"status":"ok","errors":[],"warnings":[],"info":[]},"sheet":{"name":"Jin","stats":{"STR":55}}}}"#,
            r#"{"event":"phase","phase":"bound","data":{"session_id":"sess_1","actor_id":"pc.current"}}"#,
            r#"{"event":"phase","phase":"done","data":{}}"#,
        ]
        .join("\n")
    }

    #[test]
    fn valid_character_stream_is_ready() {
        let r = parse_character_creation_jsonl(&valid_stream(), true, true);
        assert!(r.ok, "expected ready, failures: {:?}", r.failures);
        assert_eq!(r.character_id.as_deref(), Some("character_abc"));
        assert_eq!(r.session_id.as_deref(), Some("sess_1"));
        assert_eq!(r.actor_id.as_deref(), Some("pc.current"));
        assert!(r.saw_character_created && r.saw_bound);
        assert!(r.failures.is_empty());
    }

    #[test]
    fn missing_bound_phase_is_not_ready() {
        let stream = [
            r#"{"event":"phase","phase":"character_created","data":{"character_id":"character_abc","name":"Jin","status":"ready","session_id":"sess_1","actor_id":"pc.current","validation":{"status":"ok","errors":[]},"sheet":{"name":"Jin"}}}"#,
        ]
        .join("\n");
        let r = parse_character_creation_jsonl(&stream, true, true);
        assert!(!r.ok);
        assert!(r.failures.iter().any(|f| f.contains("bound")));
    }

    #[test]
    fn empty_ids_and_validation_errors_are_not_ready() {
        let stream = [
            r#"{"event":"phase","phase":"character_created","data":{"character_id":"","name":"Jin","status":"draft_needs_rules_source","session_id":"","actor_id":"","validation":{"status":"error","errors":[{"code":"missing","message":"no rules"}]},"sheet":{}}}"#,
            r#"{"event":"phase","phase":"bound","data":{"session_id":"","actor_id":""}}"#,
        ]
        .join("\n");
        let r = parse_character_creation_jsonl(&stream, true, true);
        assert!(!r.ok);
        assert!(r.character_id.is_none(), "empty string id must be None");
        assert!(r.session_id.is_none());
        assert!(r.failures.iter().any(|f| f.contains("character_id")));
        assert!(r.failures.iter().any(|f| f.contains("sheet")));
        assert!(r.failures.iter().any(|f| f.contains("validation")));
    }

    #[test]
    fn persisted_not_required_is_ok() {
        let mut v = PersistedVerification {
            required: false,
            ..Default::default()
        };
        assert_eq!(
            evaluate_persisted_verification(&mut v),
            PersistedVerdict::Ok
        );
        assert!(v.failures.is_empty());
    }

    #[test]
    fn persisted_required_without_db_is_blocked() {
        let mut v = PersistedVerification {
            required: true,
            db_available: false,
            ..Default::default()
        };
        assert_eq!(
            evaluate_persisted_verification(&mut v),
            PersistedVerdict::Blocked
        );
        assert!(v.failures.iter().any(|f| f.contains("no database")));
    }

    #[test]
    fn persisted_required_with_query_error_is_blocked() {
        let mut v = PersistedVerification {
            required: true,
            db_available: true,
            db_checked: true,
            query_errors: vec!["characters: relation does not exist".to_string()],
            ..Default::default()
        };
        assert_eq!(
            evaluate_persisted_verification(&mut v),
            PersistedVerdict::Blocked
        );
        assert!(v.failures.iter().any(|f| f.contains("could not verify")));
    }

    #[test]
    fn persisted_required_with_missing_rows_is_invalid() {
        let mut v = PersistedVerification {
            required: true,
            require_session_binding: true,
            db_available: true,
            db_checked: true,
            character_id: Some("character_abc".into()),
            session_id: Some("sess_1".into()),
            actor_id: Some("pc.current".into()),
            character_row_present: false,
            session_row_present: true,
            actor_params_present: false,
            ..Default::default()
        };
        assert_eq!(
            evaluate_persisted_verification(&mut v),
            PersistedVerdict::Invalid
        );
        assert!(v.failures.iter().any(|f| f.contains("characters row")));
        assert!(v
            .failures
            .iter()
            .any(|f| f.contains("runtime_actor_parameters row")));
        assert!(
            !v.failures.iter().any(|f| f.contains("sessions row")),
            "present session row must not be reported missing"
        );
    }

    #[test]
    fn persisted_required_all_rows_present_is_ok() {
        let mut v = PersistedVerification {
            required: true,
            require_session_binding: true,
            db_available: true,
            db_checked: true,
            character_id: Some("character_abc".into()),
            session_id: Some("sess_1".into()),
            actor_id: Some("pc.current".into()),
            character_row_present: true,
            session_row_present: true,
            actor_params_present: true,
            ..Default::default()
        };
        assert_eq!(
            evaluate_persisted_verification(&mut v),
            PersistedVerdict::Ok
        );
        assert!(v.failures.is_empty());
    }

    #[test]
    fn persisted_without_session_binding_ignores_session_rows() {
        let mut v = PersistedVerification {
            required: true,
            require_session_binding: false,
            db_available: true,
            db_checked: true,
            character_id: Some("character_abc".into()),
            character_row_present: true,
            session_row_present: false,
            actor_params_present: false,
            ..Default::default()
        };
        assert_eq!(
            evaluate_persisted_verification(&mut v),
            PersistedVerdict::Ok
        );
        assert!(v.failures.is_empty());
    }

    #[test]
    fn error_event_is_captured() {
        let stream = [
            r#"{"event":"phase","phase":"start","data":{}}"#,
            r#"{"event":"error","message":"llm backend unreachable"}"#,
        ]
        .join("\n");
        let r = parse_character_creation_jsonl(&stream, true, true);
        assert!(!r.ok);
        assert!(r.saw_error);
        assert!(r.failures.iter().any(|f| f.contains("error event")));
    }
}

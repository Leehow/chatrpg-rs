//! Journey-prelude character/session readiness (TC-JRNY-00).
//!
//! Pure and provider-free: the *decisions* that gate a `journey-prelude-
//! character-session` run live here so they are unit-testable without spawning
//! the CLI, an LLM, or a database. The CLI spawn and the DB row queries live in
//! the binary; only the verdicts are here.

use serde::Serialize;
use serde_json::Value;

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

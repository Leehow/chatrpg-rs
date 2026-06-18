//! Check-dependent journey checkpoints, evidence classification, and the
//! human-input / roll-request guards (TC-JRNY-01).
//!
//! Everything here is pure and provider-free so the Journey Qualification Gate is
//! unit-testable without spawning the CLI, an LLM, or a database. The classifier
//! fails closed: a provisional roll with null target/success/degree can never
//! reach `PASS`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Journey Qualification Gate result state for a single checkpoint. The three
/// middle variants are the regression target for TC-JRNY-01: a check-dependent
/// action that produced a provisional roll with null target/success/degree must
/// classify as `TRIGGERED_NO_MECHANISM`, never `PASS`.
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

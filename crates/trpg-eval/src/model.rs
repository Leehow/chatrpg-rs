//! Data model for the static-transcript evaluator (蓝图 §八 EvalFinding 证据化).

use serde::Serialize;
use std::collections::HashSet;
use std::fmt;

/// One parsed turn of a 战报 transcript.
#[derive(Debug, Clone, Default)]
pub struct Turn {
    /// 1-based turn number as written in the report header.
    pub index: u32,
    /// The player's submitted action (`**玩家**：…`), prefix/punct stripped.
    pub player_action: String,
    /// Full GM raw narration (the ```fenced``` block) — the highest-fidelity
    /// player-visible text; the audience recap below it is lossy.
    pub gm_raw: String,
    /// Concatenated player-visible prose (`[玩家可见]` 散文 blocks).
    pub visible_prose: String,
    /// First narrative paragraph — the scene "opening" for reset detection.
    pub scene_opening: String,
    /// Raw `[玩家可见] [roll]` lines for this turn.
    pub roll_lines: Vec<String>,
    /// Lines anywhere in the turn that mention a pending/unsettled mechanic (`待结算`).
    pub debt_lines: Vec<String>,
    /// The `<sub>体检…</sub>` self-check line, if present.
    pub self_check: String,
}

impl Turn {
    /// A check this turn resolved as a success (and not a failure).
    pub fn has_success_roll(&self) -> bool {
        self.roll_lines.iter().any(|l| {
            (l.contains("成功") || l.contains("对抗获胜")) && !l.contains("失败")
        })
    }
    pub fn has_any_roll(&self) -> bool {
        !self.roll_lines.is_empty()
    }
    /// Self-check claims "no pending roll" — used to catch a lying health-check.
    pub fn claims_no_pending(&self) -> bool {
        self.self_check.contains("无未定")
    }
    /// Full player-facing text for semantic checks: GM raw + the audience recap.
    pub fn visible_text(&self) -> String {
        format!("{}\n{}", self.gm_raw, self.visible_prose)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Transcript {
    pub title: String,
    pub turns: Vec<Turn>,
}

/// The documented milestone-1 root causes (RUN_SPEC_V2 验收1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum RootCause {
    PlayerActionLoop,
    SceneReset,
    SemanticNoop,
    UnresolvedMechanicalDebt,
    SuccessWithoutInformation,
    ResponseIntentMismatch,
}

impl RootCause {
    /// Stable screaming-snake id used in reports + asserted by the blueprint.
    pub fn id(&self) -> &'static str {
        match self {
            RootCause::PlayerActionLoop => "PLAYER_ACTION_LOOP",
            RootCause::SceneReset => "SCENE_RESET",
            RootCause::SemanticNoop => "SEMANTIC_NOOP",
            RootCause::UnresolvedMechanicalDebt => "UNRESOLVED_MECHANICAL_DEBT",
            RootCause::SuccessWithoutInformation => "SUCCESS_WITHOUT_INFORMATION",
            RootCause::ResponseIntentMismatch => "RESPONSE_INTENT_MISMATCH",
        }
    }
    /// Runtime layer this defect attributes to (蓝图 §九 分层归因).
    pub fn root_layer(&self) -> &'static str {
        match self {
            RootCause::PlayerActionLoop => "PLAYER_POLICY",
            RootCause::SceneReset => "WORLD",
            RootCause::SemanticNoop => "NARRATOR",
            RootCause::UnresolvedMechanicalDebt => "RULES",
            RootCause::SuccessWithoutInformation => "DIRECTOR",
            RootCause::ResponseIntentMismatch => "NARRATOR",
        }
    }
}

impl fmt::Display for RootCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Severity {
    /// Informational; never on its own fails a run.
    Info,
    /// Quality concern; fails when count crosses the probe threshold.
    High,
    /// Hard gate (蓝图 §七 硬门槛); a single occurrence fails the run.
    Hard,
}

/// One evidenced finding (蓝图 §八). expected/actual/turn_ids/evidence all required.
#[derive(Debug, Clone, Serialize)]
pub struct EvalFinding {
    pub cause: RootCause,
    pub severity: Severity,
    pub root_layer: &'static str,
    pub turn_ids: Vec<u32>,
    pub expected: String,
    pub actual: String,
    pub evidence: Vec<String>,
}

impl EvalFinding {
    pub fn new(
        cause: RootCause,
        severity: Severity,
        turn_ids: Vec<u32>,
        expected: impl Into<String>,
        actual: impl Into<String>,
        evidence: Vec<String>,
    ) -> Self {
        EvalFinding {
            cause,
            severity,
            root_layer: cause.root_layer(),
            turn_ids,
            expected: expected.into(),
            actual: actual.into(),
            evidence,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Verdict {
    pub findings: Vec<EvalFinding>,
}

impl Verdict {
    /// FAIL when any finding is High or Hard severity (蓝图 §七).
    pub fn is_fail(&self) -> bool {
        self.findings.iter().any(|f| f.severity >= Severity::High)
    }
    pub fn label(&self) -> &'static str {
        if self.is_fail() {
            "FAIL"
        } else {
            "PASS"
        }
    }
    pub fn root_causes(&self) -> HashSet<RootCause> {
        self.findings.iter().map(|f| f.cause).collect()
    }
}

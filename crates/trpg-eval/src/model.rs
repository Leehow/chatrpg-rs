use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalFixture {
    pub fixture_id: String,
    pub title: String,
    #[serde(default)]
    pub turns: Vec<EvalTurn>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalTurn {
    pub turn: u32,
    pub player_decision: PlayerDecision,
    pub gm_response: GmResponse,
    #[serde(default)]
    pub trace: TraceObservation,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlayerDecision {
    #[serde(
        default,
        alias = "gm_player_visible_reply",
        alias = "previous_gm_visible_reply"
    )]
    pub gm_visible_reply: String,
    #[serde(default)]
    pub perceived_facts: Vec<String>,
    #[serde(default)]
    pub active_goal: String,
    #[serde(default)]
    pub hypotheses: Vec<String>,
    #[serde(default)]
    pub last_action_result: String,
    #[serde(default)]
    pub risk_assessment: Vec<String>,
    #[serde(default, alias = "resources_considered")]
    pub resource_assessment: Vec<String>,
    #[serde(default)]
    pub open_questions: Vec<String>,
    #[serde(default)]
    pub candidate_actions: Vec<String>,
    #[serde(default, alias = "selection_reason")]
    pub selection_rationale: String,
    #[serde(default, alias = "selected_action")]
    pub declared_action: String,
    #[serde(default, alias = "gm_input")]
    pub sent_to_gm: String,
    #[serde(default)]
    #[serde(alias = "expected_response_contract")]
    pub response_contract: ResponseContract,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseContract {
    #[serde(default)]
    pub intent: String,
    #[serde(default)]
    pub requested_information: Vec<String>,
    #[serde(default)]
    pub acceptable_resolutions: Vec<String>,
    #[serde(default)]
    pub unacceptable: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GmResponse {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub resolution_status: String,
    #[serde(default)]
    pub check_outcome: Option<String>,
    #[serde(default)]
    pub facts_added: Vec<String>,
    #[serde(default)]
    pub facts_exposed_to_player: Vec<String>,
    #[serde(default)]
    pub state_delta: Vec<String>,
    #[serde(default)]
    pub choices_opened: Vec<String>,
    #[serde(default)]
    pub pending: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TraceObservation {
    #[serde(default)]
    pub mechanical_debt_markers: Vec<String>,
    #[serde(default)]
    pub pending_id: Option<String>,
    #[serde(default)]
    pub rolls: Vec<RollTrace>,
    #[serde(default)]
    pub continuity_violation: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RollTrace {
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub skill: Option<String>,
    #[serde(default)]
    pub stat: Option<String>,
    #[serde(default)]
    pub die: Option<String>,
    #[serde(default)]
    pub die_result: Option<i64>,
    #[serde(default)]
    pub base: Option<i64>,
    #[serde(default)]
    pub modifier: Option<i64>,
    #[serde(default)]
    pub total: Option<i64>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default)]
    pub source_refs: Vec<String>,
    #[serde(default)]
    pub state_delta: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FindingCategory {
    ResponseIntentMismatch,
    SuccessWithoutInformation,
    PendingMechanicalDebt,
    SemanticNoop,
    PlayerScriptLoop,
    PlayerPolicyViolation,
    StateContinuityFail,
    RulesTraceIncomplete,
    PlayerAgencyViolation,
}

impl FindingCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ResponseIntentMismatch => "RESPONSE_INTENT_MISMATCH",
            Self::SuccessWithoutInformation => "SUCCESS_WITHOUT_INFORMATION",
            Self::PendingMechanicalDebt => "PENDING_MECHANICAL_DEBT",
            Self::SemanticNoop => "SEMANTIC_NOOP",
            Self::PlayerScriptLoop => "PLAYER_SCRIPT_LOOP",
            Self::PlayerPolicyViolation => "PLAYER_POLICY_VIOLATION",
            Self::StateContinuityFail => "STATE_CONTINUITY_FAIL",
            Self::RulesTraceIncomplete => "RULES_TRACE_INCOMPLETE",
            Self::PlayerAgencyViolation => "PLAYER_AGENCY_VIOLATION",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RubricDimension {
    RulesResolution,
    Continuity,
    Responsiveness,
    NarrativeAgency,
    PlayerSimulation,
    Language,
}

impl RubricDimension {
    pub const ALL: [Self; 6] = [
        Self::RulesResolution,
        Self::Continuity,
        Self::Responsiveness,
        Self::NarrativeAgency,
        Self::PlayerSimulation,
        Self::Language,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::RulesResolution => "rules_resolution",
            Self::Continuity => "continuity",
            Self::Responsiveness => "responsiveness",
            Self::NarrativeAgency => "narrative_agency",
            Self::PlayerSimulation => "player_simulation",
            Self::Language => "language",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::RulesResolution => "规则与结算完整性",
            Self::Continuity => "前后文和世界连续性",
            Self::Responsiveness => "对玩家行动的响应性",
            Self::NarrativeAgency => "剧情推进和玩家能动性",
            Self::PlayerSimulation => "玩家模拟真实性",
            Self::Language => "语言表现",
        }
    }

    pub fn weight(self) -> i32 {
        match self {
            Self::RulesResolution => 25,
            Self::Continuity => 20,
            Self::Responsiveness => 20,
            Self::NarrativeAgency => 15,
            Self::PlayerSimulation => 15,
            Self::Language => 5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RubricScore {
    pub dimension: RubricDimension,
    pub label: String,
    pub weight: i32,
    pub score: i32,
    #[serde(default)]
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FindingSeverity {
    S1,
    S2,
    S3,
}

impl FindingSeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::S1 => "S1",
            Self::S2 => "S2",
            Self::S3 => "S3",
        }
    }

    pub fn weight(self) -> i32 {
        match self {
            Self::S1 => 35,
            Self::S2 => 20,
            Self::S3 => 10,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalFinding {
    pub finding_id: String,
    pub category: FindingCategory,
    pub severity: FindingSeverity,
    pub turns: Vec<u32>,
    pub root_layer: String,
    pub expected: String,
    pub actual: String,
    #[serde(default)]
    pub evidence: Vec<String>,
    pub confidence: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalReport {
    pub fixture_id: String,
    pub title: String,
    pub verdict: Verdict,
    pub score: i32,
    #[serde(default)]
    pub rubric_scores: Vec<RubricScore>,
    #[serde(default)]
    pub findings: Vec<EvalFinding>,
}

impl EvalReport {
    pub fn has_category(&self, category: FindingCategory) -> bool {
        self.findings.iter().any(|f| f.category == category)
    }
}

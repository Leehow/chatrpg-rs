use anyhow::{Context, Result};
use chrono::Utc;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use trpg_model::*;

pub mod gm_loop;
pub use gm_loop::*;

/// Agent skills are Rust-registered bounded tasks. A skill may be pure Rust or
/// internally call an LLM JSON prompt, but the Rust Agent invokes it explicitly
/// and validates its typed output. This prevents an unbounded ReAct loop.
#[async_trait::async_trait]
pub trait AgentSkill<I: Send + 'static, O: Send + 'static>: Send + Sync {
    fn skill_id(&self) -> &'static str;
    async fn run(&self, input: I) -> Result<O>;
}

/// Agent tools are deterministic or side-effecting capabilities implemented in
/// Rust: dice, search, context loading, patch validation, persistence, and audit.
/// The LLM cannot call these directly; it can only request a structure that the
/// Rust Agent chooses to route through a tool.
#[async_trait::async_trait]
pub trait AgentTool<I: Send + 'static, O: Send + 'static>: Send + Sync {
    fn tool_id(&self) -> &'static str;
    async fn call(&self, input: I) -> Result<O>;
}

/// Rust-owned GM agent orchestrator.
///
/// The agent is not an external framework. It is a Rust state machine that may
/// call LLM skills elsewhere, but all control flow, tool permissions, roll
/// visibility, persistence, and audit decisions remain in Rust.
#[derive(Debug, Clone)]
pub struct GmAgent {
    pub policy: AgentPolicyPack,
}

impl GmAgent {
    pub fn from_env_or_default() -> Self {
        let dir = std::env::var("TRPG_AGENT_ADVICE_DIR").unwrap_or_else(|_| "./data/agent/advice".to_string());
        match AgentPolicyPack::load_dir(&dir) {
            Ok(policy) if !policy.advice_layers.is_empty() => Self { policy },
            Ok(policy) => {
                tracing::warn!("agent advice dir loaded but no advice layers were found; using empty policy");
                Self { policy }
            }
            Err(err) => {
                tracing::warn!(error = %err, "failed to load agent advice dir; using empty policy");
                Self { policy: AgentPolicyPack::default() }
            }
        }
    }

}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentPolicyPack {
    pub advice_layers: Vec<AgentAdviceLayer>,
}

impl AgentPolicyPack {
    pub fn load_dir(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::default());
        }
        let mut layers = Vec::new();
        for entry in fs::read_dir(path).with_context(|| format!("failed to read agent advice dir {}", path.display()))? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") { continue; }
            let text = fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
            let mut layer: AgentAdviceLayer = serde_json::from_str(&text).with_context(|| format!("invalid advice JSON {}", path.display()))?;
            layer.source_path = Some(path.to_string_lossy().to_string());
            layers.push(layer);
        }
        layers.sort_by(|a, b| a.priority.cmp(&b.priority).then(a.layer_id.cmp(&b.layer_id)));
        Ok(Self { advice_layers: layers })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentAdviceLayer {
    pub layer_id: String,
    pub title: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub applies_to_rulesets: Vec<String>,
    #[serde(default)]
    pub applies_to_modules: Vec<String>,
    #[serde(default)]
    pub notes_for_humans: String,
    #[serde(default)]
    pub source_path: Option<String>,
    #[serde(default)]
    pub rules: Vec<AgentAdviceRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentAdviceRule {
    pub rule_id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub match_conditions: MatchConditions,
    pub plan_kind: AgentPlanAdviceKind,
    #[serde(default)]
    pub intent_kind: Option<String>,
    #[serde(default)]
    pub check_label_hint: Option<String>,
    #[serde(default)]
    pub dice_expression_hint: Option<String>,
    #[serde(default)]
    pub static_target: Option<i32>,
    #[serde(default)]
    pub target_label: Option<String>,
    #[serde(default)]
    pub before_roll_public: Option<String>,
    #[serde(default)]
    pub success_public: Option<String>,
    #[serde(default)]
    pub failure_public: Option<String>,
    #[serde(default)]
    pub critical_public: Option<String>,
    #[serde(default)]
    pub fumble_public: Option<String>,
    #[serde(default)]
    pub free_read_reason: Option<String>,
    #[serde(default)]
    pub irreversible: Option<bool>,
    #[serde(default)]
    pub confidence: Option<RulingConfidence>,
    #[serde(default)]
    pub advice_summary: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPlanAdviceKind {
    NarrateOnly,
    FreeRead,
    AskPlayerRoll,
    GmPublicRoll,
    GmSecretRoll,
    PassiveResolution,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MatchConditions {
    #[serde(default)]
    pub any_terms: Vec<String>,
    #[serde(default)]
    pub all_terms: Vec<String>,
    #[serde(default)]
    pub not_terms: Vec<String>,
    #[serde(default)]
    pub regex_any: Vec<String>,
}

pub fn pending_check_prompt(contract: &CheckContract) -> String {
    let target = match &contract.target {
        CheckTargetModel::StaticNumber { label, .. } => format!("难度类型：{}。", label),
        CheckTargetModel::Opposed { opponent_id, opponent_check } => format!("对抗：{} 的 {}。", opponent_id, opponent_check),
        CheckTargetModel::DegreeOnly => "按成功程度结算。".into(),
        CheckTargetModel::SuccessCount { .. } => "按成功数结算。".into(),
        CheckTargetModel::DicePoolCount { target_face, threshold, .. } => format!("掷骰池：数出显示 {} 的骰子，≥{} 个即成功。", target_face, threshold),
        CheckTargetModel::UnknownUntilLookup => "目标值暂不公开；按当前规则包裁定。".into(),
    };
    format!(
        "[system]这里需要一次检定：{}。{}
{}
成功：{}
失败：{}
请只回复 `roll`，系统会调用骰子工具并写入结果；不要自行给出点数或总值。[/system]",
        &contract.check_label,
        target,
        &contract.stakes.before_roll_public,
        &contract.stakes.success_public,
        &contract.stakes.failure_public,
    )
}

pub fn is_probably_roll_input(input: &str) -> bool {
    parse_roll_text(input).is_some()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParsedRollText {
    DiceExpression(String),
    ReportedTotal(i32),
    ReportedDieAndComponents { die: i32, components: Vec<i32>, total: i32 },
}

impl ParsedRollText {
    pub fn expression_for_record(&self) -> String {
        match self {
            ParsedRollText::DiceExpression(expr) => expr.clone(),
            ParsedRollText::ReportedTotal(total) => total.to_string(),
            ParsedRollText::ReportedDieAndComponents { die, components, total } => {
                format!("reported die {die} + components {:?} = {total}", components)
            }
        }
    }
}

/// Parse player roll replies in both terse CLI form and natural table speech.
///
/// Supported examples:
/// - `/roll 1d10+7`
/// - `17`
/// - `1d10+7`
/// - `我掷了 8`
/// - `我 TECH 6，Basic Tech 4，掷 1d10 出来是 8`
pub fn parse_roll_text(input: &str) -> Option<ParsedRollText> {
    let trimmed = input.trim();
    if trimmed.is_empty() { return None; }
    if let Some(rest) = trimmed.strip_prefix("/roll") {
        let expr = rest.trim();
        if expr.is_empty() { return None; }
        return Some(ParsedRollText::DiceExpression(expr.to_string()));
    }
    if Regex::new(r"^\s*\d{1,3}\s*$").ok()?.is_match(trimmed) {
        return trimmed.parse::<i32>().ok().map(ParsedRollText::ReportedTotal);
    }
    if Regex::new(r"(?i)^\s*\d*d\d+([+-]\d+)?\s*$").ok()?.is_match(trimmed) {
        return Some(ParsedRollText::DiceExpression(trimmed.to_string()));
    }

    // Common table shorthand: "3d6=11", "3d6 = 11, SP 0". The dice
    // expression identifies the pending effect roll, but the number after '='
    // is the result to commit; do not accidentally pick the SP/armor number.
    if let Some(cap) = Regex::new(r"(?i)\b\d*d\d+(?:[+-]\d+)?\s*[=＝:]\s*(\d{1,3})").ok()?.captures(trimmed) {
        if let Some(m) = cap.get(1) {
            if let Ok(total) = m.as_str().parse::<i32>() {
                return Some(ParsedRollText::ReportedTotal(total));
            }
        }
    }

    let lower = trimmed.to_lowercase();
    let roll_words = ["掷", "骰", "投", "roll", "rolled", "result", "total", "结果", "总计", "出来", "出了", "出目", "点数"];
    if !roll_words.iter().any(|w| lower.contains(w)) {
        return None;
    }

    let dice_re = Regex::new(r"(?i)(\d*)d(\d+)([+-]\d+)?").ok()?;
    let scrubbed = dice_re.replace_all(trimmed, " ").to_string();
    let num_re = Regex::new(r"-?\d+").ok()?;
    let nums: Vec<i32> = num_re.find_iter(&scrubbed).filter_map(|m| m.as_str().parse::<i32>().ok()).collect();
    if nums.is_empty() { return None; }

    // If the player reports both stat/skill components and a die result, use the
    // final number as the die and earlier numbers as components. This handles
    // natural replies such as: "TECH 6, Basic Tech 4, d10 出来是 8" => 18.
    if dice_re.is_match(trimmed) && nums.len() >= 2 {
        let die = *nums.last()?;
        let components = nums[..nums.len() - 1].to_vec();
        let total = components.iter().sum::<i32>() + die;
        return Some(ParsedRollText::ReportedDieAndComponents { die, components, total });
    }

    nums.last().copied().map(ParsedRollText::ReportedTotal)
}

/// A conservative classifier for inputs that look like the player is abandoning
/// an open gate and starting a different action. The data-driven gate decides
/// whether this should cancel or re-prompt; this helper only identifies common
/// natural phrases.
pub fn looks_like_new_action_or_abandon(input: &str) -> bool {
    let lower = input.to_lowercase();
    let terms = [
        "算了", "不投", "不做", "放弃", "取消", "改", "换", "绕", "离开", "撤", "先", "直接", "继续", "我去", "我想",
        "instead", "rather", "cancel", "abandon", "never mind", "do something else", "i go", "i move", "i try",
    ];
    terms.iter().any(|term| lower.contains(term))
}

pub fn make_pending_check(contract: &CheckContract) -> PendingCheck {
    PendingCheck {
        check_id: contract.check_id.clone(),
        session_id: contract.session_id.clone(),
        expected_input_kind: "roll_total_or_dice_expression".into(),
        prompt_public: pending_check_prompt(contract),
        contract: contract.clone(),
        expires_at: None,
        status: PendingCheckStatus::Open,
        interaction_context_id: None,
        owner_frame_id: contract.actor_snapshot_ids.first().cloned(),
        gate_id: Some(format!("gate_{}", contract.check_id)),
        generation: 0,
        superseded_reason: None,
        closed_at_tick: None,
        created_at: Utc::now(),
    }
}

fn default_true() -> bool { true }

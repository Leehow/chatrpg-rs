use serde::{Deserialize, Serialize};
use serde_json::Value;
use trpg_model::*;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TurnLedgerSnapshot {
    #[serde(default)]
    pub check_contracts: Vec<CheckContract>,
    #[serde(default)]
    pub dice_rolls: Vec<DiceRollRecord>,
    #[serde(default)]
    pub check_results: Vec<CheckResultRecord>,
    #[serde(default)]
    pub effect_contracts: Vec<EffectContract>,
    #[serde(default)]
    pub parameter_impacts: Vec<ParameterImpact>,
    #[serde(default)]
    pub interaction_gates: Vec<InteractionGate>,
    #[serde(default)]
    pub committed_world_facts: Vec<CommittedWorldFactEvidence>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommittedWorldFactEvidence {
    pub fact_id: String,
    pub summary: String,
    #[serde(default)]
    pub truth_status: Option<String>,
}

impl TurnLedgerSnapshot {
    pub fn has_check_fact(&self) -> bool {
        !self.check_contracts.is_empty()
            || !self.check_results.is_empty()
            || self.has_open_player_roll_gate()
    }

    pub fn has_check_fact_for_text(&self, text: &str) -> bool {
        self.has_check_fact() || self.committed_world_fact_quotes_check_text(text)
    }

    fn committed_world_fact_quotes_check_text(&self, text: &str) -> bool {
        let text = normalize_text(text);
        self.committed_world_facts.iter().any(|fact| {
            fact_truth_is_true(fact.truth_status.as_deref())
                && world_fact_check_fragments(&normalize_text(&fact.summary))
                    .iter()
                    .any(|fragment| text.contains(fragment))
        })
    }

    pub fn has_executed_check(&self) -> bool {
        !self.check_results.is_empty()
            || self
                .dice_rolls
                .iter()
                .any(|roll| roll.check_id.as_deref().is_some())
    }

    pub fn has_open_player_roll_gate(&self) -> bool {
        self.interaction_gates.iter().any(|gate| {
            gate.status == GateStatus::Open && gate.gate_kind == GateKind::PlayerRollRequired
        })
    }

    pub fn has_effect_evidence(&self) -> bool {
        !self.effect_contracts.is_empty()
            || !self.parameter_impacts.is_empty()
            || self
                .check_results
                .iter()
                .any(|result| !result.committed_patches.is_empty())
    }

    pub fn has_effect_evidence_for_text(&self, text: &str) -> bool {
        self.has_effect_evidence() || self.committed_world_fact_supports_effect_text(text)
    }

    fn committed_world_fact_supports_effect_text(&self, text: &str) -> bool {
        let text = normalize_text(text);
        self.committed_world_facts.iter().any(|fact| {
            fact_truth_is_true(fact.truth_status.as_deref())
                && !fact.summary.trim().is_empty()
                && effect_family_overlap(&normalize_text(&fact.summary), &text)
        })
    }

    fn visible_evidence_tokens(&self) -> Vec<String> {
        let mut tokens = Vec::new();
        for check in &self.check_contracts {
            push_token(&mut tokens, &check.check_id);
            push_token(&mut tokens, &check.check_label);
            push_token(&mut tokens, &check.dice_expression);
        }
        for result in &self.check_results {
            if !roll_is_player_visible(result.roll.visibility) {
                continue;
            }
            push_token(&mut tokens, &result.check_id);
            push_token(&mut tokens, &result.roll.roll_id);
            push_token(&mut tokens, &result.roll.expression);
            collect_json_tokens(&result.roll.result, &mut tokens);
            collect_json_tokens(&result.outcome, &mut tokens);
            for patch in &result.committed_patches {
                collect_json_tokens(
                    &serde_json::to_value(patch).unwrap_or(Value::Null),
                    &mut tokens,
                );
            }
        }
        for effect in &self.effect_contracts {
            if !visibility_is_player_visible(effect.visibility) {
                continue;
            }
            push_token(&mut tokens, &effect.effect_id);
            push_token(&mut tokens, effect.effect_kind.as_str());
            for target in &effect.target_actor_ids {
                push_token(&mut tokens, target);
            }
            for patch in &effect.proposed_patches {
                collect_json_tokens(
                    &serde_json::to_value(patch).unwrap_or(Value::Null),
                    &mut tokens,
                );
            }
        }
        for impact in &self.parameter_impacts {
            if !visibility_is_player_visible(impact.visibility) {
                continue;
            }
            push_token(&mut tokens, &impact.impact_id);
            push_token(&mut tokens, impact.target_kind.as_str());
            push_token(&mut tokens, &impact.target_id);
            push_token(&mut tokens, &impact.parameter_path);
            push_token(&mut tokens, impact.operation.as_str());
            collect_json_tokens(&impact.value, &mut tokens);
        }
        dedupe_tokens(tokens)
    }

    fn has_visible_player_result(&self) -> bool {
        self.check_results
            .iter()
            .any(|result| roll_is_player_visible(result.roll.visibility))
            || self
                .effect_contracts
                .iter()
                .any(|effect| visibility_is_player_visible(effect.visibility))
            || self
                .parameter_impacts
                .iter()
                .any(|impact| visibility_is_player_visible(impact.visibility))
    }

    pub fn private_roll_leak_tokens(&self) -> Vec<String> {
        let mut tokens = Vec::new();
        for result in &self.check_results {
            if result.roll.visibility != RollVisibility::PrivateGmRoll {
                continue;
            }
            push_token(&mut tokens, &result.roll.roll_id);
            push_token(&mut tokens, &result.roll.expression);
            collect_json_tokens(&result.roll.result, &mut tokens);
            collect_json_tokens(&result.outcome, &mut tokens);
        }
        for roll in &self.dice_rolls {
            if roll.visibility != RollVisibility::PrivateGmRoll {
                continue;
            }
            push_token(&mut tokens, &roll.roll_id);
            push_token(&mut tokens, &roll.expression);
            collect_json_tokens(&roll.result, &mut tokens);
        }
        dedupe_tokens(tokens)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MechanicalClaimKind {
    Check,
    Roll,
    Damage,
    Resource,
    Condition,
    Object,
    Clock,
    State,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MechanicalClaim {
    pub kind: MechanicalClaimKind,
    pub text: String,
}

impl MechanicalClaim {
    pub fn new(kind: MechanicalClaimKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FinalNarrationSubmission {
    pub player_visible_text: String,
    #[serde(default)]
    pub mechanical_claims: Vec<MechanicalClaim>,
    #[serde(default)]
    pub referenced_ledger_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NarrationVerifier;

impl NarrationVerifier {
    pub fn verify(
        &self,
        ledger: &TurnLedgerSnapshot,
        submission: &FinalNarrationSubmission,
    ) -> NarrationVerifierResult {
        let player_visible_text = strip_gm_only_sidecars(&submission.player_visible_text);
        let text = normalize_text(&player_visible_text);
        let mut findings = Vec::new();

        for claim in &submission.mechanical_claims {
            match claim.kind {
                MechanicalClaimKind::Check => {
                    if !ledger.has_check_fact() {
                        findings.push(VerifierFinding::blocker(
                            VerifierFindingKind::MissingCheck,
                            format!(
                                "mechanical claim is unsupported by any check contract, check result, or player-roll gate: {}",
                                claim.text
                            ),
                        ));
                    }
                }
                MechanicalClaimKind::Roll => {
                    if !ledger.has_check_fact() {
                        findings.push(VerifierFinding::blocker(
                            VerifierFindingKind::MissingCheck,
                            format!("roll claim has no check contract or result: {}", claim.text),
                        ));
                    } else if !ledger.has_executed_check() {
                        findings.push(VerifierFinding::blocker(
                            VerifierFindingKind::MissingRollExecution,
                            format!(
                                "roll claim resolves a check, but no dice/check result exists: {}",
                                claim.text
                            ),
                        ));
                    }
                }
                MechanicalClaimKind::Damage
                | MechanicalClaimKind::Resource
                | MechanicalClaimKind::Condition
                | MechanicalClaimKind::Object
                | MechanicalClaimKind::Clock
                | MechanicalClaimKind::State => {
                    if !ledger.has_effect_evidence_for_text(&claim.text) {
                        findings.push(VerifierFinding::blocker(
                            VerifierFindingKind::InventedEffect,
                            format!(
                                "effect/state claim is unsupported by effect contracts, parameter impacts, or committed patches: {}",
                                claim.text
                            ),
                        ));
                    }
                }
            }
        }

        if !submission.referenced_ledger_ids.is_empty() {
            // B7 结构化核对优先（护栏 §3.5.5）：每个引用 id 必须 ∈ 账本 id 全集
            // （check_id ∪ roll_id ∪ effect_id ∪ impact_id）；不存在 → InventedEffect。
            let known = ledger_id_set(ledger);
            for id in &submission.referenced_ledger_ids {
                if !known.contains(id) {
                    findings.push(VerifierFinding::blocker(
                        VerifierFindingKind::InventedEffect,
                        format!("referenced ledger id not found in turn ledger: {id}"),
                    ));
                }
            }
        }

        if submission.mechanical_claims.is_empty() {
            // 无结构化 claim → 回退子串扫描（可观测技术债：detail 带标注前缀，不静默）。
            // 即使 final narration 引用了合法 check/roll ledger id，也不能让这些引用
            // 掩盖额外声称的资源/伤害/状态变化；effect 仍必须有 apply_effect /
            // parameter impact 证据。
            let luck_spend_adjustment = text_says_luck_spend_adjustment(&text)
                && ledger.has_effect_evidence_for_text(&text);
            if text_says_check_or_roll(&text)
                && !luck_spend_adjustment
                && !ledger.has_check_fact_for_text(&text)
            {
                findings.push(VerifierFinding::blocker(
                    VerifierFindingKind::MissingCheck,
                    "fallback:substring_scan: final narration mentions a check/roll, but no ledger check fact exists",
                ));
            }
            if text_says_effect(&text) && !ledger.has_effect_evidence_for_text(&text) {
                findings.push(VerifierFinding::blocker(
                    VerifierFindingKind::InventedEffect,
                    "fallback:substring_scan: final narration mentions a mechanical effect, but no ledger effect evidence exists",
                ));
            }
        }

        if ledger.has_visible_player_result()
            && !text_contains_any(&text, &ledger.visible_evidence_tokens())
        {
            findings.push(VerifierFinding::blocker(
                VerifierFindingKind::OmittedVisibleResult,
                "player-visible roll/effect evidence exists in the ledger, but final narration does not mention any visible ledger token",
            ));
        }

        if text_asks_player_for_manual_roll(&text) && system_rolls_visible_policy() {
            findings.push(VerifierFinding::blocker(
                VerifierFindingKind::ManualRollRequest,
                "final narration asks the player to provide dice totals under system-rolls-visible policy",
            ));
        }

        if text_mislabels_static_dv_as_opposed(&text, ledger) {
            findings.push(VerifierFinding::blocker(
                VerifierFindingKind::InventedEffect,
                "final narration labels a static DV/DC check as opposed, but the ledger has no opposed check target",
            ));
        }

        if blocked_movement_check_narrated_as_arrival(&text, ledger) {
            findings.push(VerifierFinding::blocker(
                VerifierFindingKind::InventedEffect,
                "final narration treats a blocked movement/location check as completed arrival, but the ledger has no resolved movement result",
            ));
        }

        if let Some(detail) = player_agency_violation_detail(&player_visible_text) {
            findings.push(VerifierFinding::blocker(
                VerifierFindingKind::PlayerAgencyViolation,
                detail,
            ));
        }

        let private_tokens = ledger.private_roll_leak_tokens();
        if !private_tokens.is_empty() && text_contains_any(&text, &private_tokens) {
            findings.push(VerifierFinding::blocker(
                VerifierFindingKind::SecretLeak,
                "final narration appears to expose private GM roll details",
            ));
        }

        let accepted = findings
            .iter()
            .all(|finding| finding.severity != VerifierSeverity::Blocker);
        let next_required_action = if accepted {
            None
        } else {
            next_action_for(&findings)
        };
        NarrationVerifierResult {
            accepted,
            findings,
            next_required_action,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NarrationVerifierResult {
    pub accepted: bool,
    #[serde(default)]
    pub findings: Vec<VerifierFinding>,
    pub next_required_action: Option<VerifierNextAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerifierFinding {
    pub kind: VerifierFindingKind,
    pub severity: VerifierSeverity,
    pub detail: String,
}

impl VerifierFinding {
    pub fn blocker(kind: VerifierFindingKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            severity: VerifierSeverity::Blocker,
            detail: detail.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerifierFindingKind {
    MissingCheck,
    MissingRollExecution,
    MissingEffect,
    InventedEffect,
    OmittedVisibleResult,
    ManualRollRequest,
    PlayerAgencyViolation,
    SecretLeak,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerifierSeverity {
    Blocker,
    Warning,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerifierNextAction {
    ReviseText,
    CallProposeCheck,
    CallExecuteCheck,
    CallApplyEffect,
    OpenPlayerGate,
    FallbackToLegacy,
}

fn next_action_for(findings: &[VerifierFinding]) -> Option<VerifierNextAction> {
    if findings
        .iter()
        .any(|f| f.kind == VerifierFindingKind::PlayerAgencyViolation)
    {
        Some(VerifierNextAction::ReviseText)
    } else if findings
        .iter()
        .any(|f| f.kind == VerifierFindingKind::MissingCheck)
    {
        Some(VerifierNextAction::CallProposeCheck)
    } else if findings
        .iter()
        .any(|f| f.kind == VerifierFindingKind::MissingRollExecution)
    {
        Some(VerifierNextAction::CallExecuteCheck)
    } else if findings.iter().any(|f| {
        matches!(
            f.kind,
            VerifierFindingKind::MissingEffect | VerifierFindingKind::InventedEffect
        )
    }) {
        Some(VerifierNextAction::CallApplyEffect)
    } else {
        Some(VerifierNextAction::ReviseText)
    }
}

/// 账本 id 全集（B7 结构化核对）：check_contracts 的 check_id ∪ dice_rolls 的
/// roll_id ∪ effect_contracts 的 effect_id ∪ parameter_impacts 的 impact_id。
/// pub 仅为 trpg-gm verify_after_stream 以"已落账事实全集"代填 referenced_ledger_ids。
pub fn ledger_id_set(ledger: &TurnLedgerSnapshot) -> std::collections::HashSet<String> {
    let mut ids = std::collections::HashSet::new();
    for check in &ledger.check_contracts {
        ids.insert(check.check_id.clone());
    }
    for roll in &ledger.dice_rolls {
        ids.insert(roll.roll_id.clone());
    }
    for effect in &ledger.effect_contracts {
        ids.insert(effect.effect_id.clone());
    }
    for impact in &ledger.parameter_impacts {
        ids.insert(impact.impact_id.clone());
    }
    ids
}

fn roll_is_player_visible(visibility: RollVisibility) -> bool {
    matches!(
        visibility,
        RollVisibility::PublicGmRoll | RollVisibility::PlayerRollRequired
    )
}

fn visibility_is_player_visible(visibility: Visibility) -> bool {
    matches!(visibility, Visibility::Public | Visibility::PlayerVisible)
}

fn normalize_text(text: &str) -> String {
    text.to_lowercase()
}

fn strip_tagged_block(text: &str, tag: &str) -> String {
    let open = format!("[{tag}]");
    let close = format!("[/{tag}]");
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(start) = rest.find(&open) {
        out.push_str(&rest[..start]);
        let after_open = &rest[start + open.len()..];
        if let Some(end) = after_open.find(&close) {
            rest = &after_open[end + close.len()..];
        } else {
            out.push_str(&rest[start..]);
            return out;
        }
    }
    out.push_str(rest);
    out
}

fn strip_gm_only_sidecars(text: &str) -> String {
    [
        "evidence_audit",
        "evidence_attempts",
        "progress_claims",
        "materialized_content",
        "hide",
        "meta",
    ]
    .iter()
    .fold(text.to_string(), |acc, tag| strip_tagged_block(&acc, tag))
}

fn fact_truth_is_true(truth_status: Option<&str>) -> bool {
    matches!(truth_status, Some("true"))
}

fn text_has_any(text: &str, terms: &[&str]) -> bool {
    terms.iter().any(|term| text.contains(term))
}

fn effect_family_overlap(fact_text: &str, narration_text: &str) -> bool {
    const FAMILIES: &[&[&str]] = &[
        &[
            "power",
            "powered",
            "breaker",
            "emergency stop",
            "shut down",
            "shutdown",
            "server cable",
            "供电",
            "断电",
            "电闸",
            "急停",
            "电源",
            "缆线",
        ],
        &[
            "hp",
            "hit point",
            "生命值",
            "damage",
            "伤害",
            "wound",
            "injury",
        ],
        &["luck", "幸运", "resource", "资源", "sanity", "san", "理智"],
        &[
            "condition",
            "status",
            "disabled",
            "broken",
            "状态",
            "关闭",
            "离线",
            "停机",
            "破坏",
            "摧毁",
        ],
        &["clock", "progress", "进度"],
    ];

    FAMILIES
        .iter()
        .any(|family| text_has_any(fact_text, family) && text_has_any(narration_text, family))
}

fn world_fact_check_fragments(fact_text: &str) -> Vec<String> {
    fact_text
        .split(|c| matches!(c, '.' | '。' | ';' | '；' | '\n' | '\r'))
        .map(str::trim)
        .filter(|fragment| fragment.chars().count() >= 16)
        .filter(|fragment| fragment.contains("check") || fragment.contains("检定"))
        .map(ToOwned::to_owned)
        .collect()
}

fn push_token(tokens: &mut Vec<String>, token: &str) {
    let token = token.trim();
    if token.is_empty() {
        return;
    }
    let normalized = normalize_text(token);
    if normalized.chars().count() >= 2 {
        tokens.push(normalized);
    }
}

fn collect_json_tokens(value: &Value, tokens: &mut Vec<String>) {
    match value {
        Value::Null | Value::Bool(_) => {}
        Value::Number(number) => push_token(tokens, &number.to_string()),
        Value::String(text) => push_token(tokens, text),
        Value::Array(items) => {
            for item in items {
                collect_json_tokens(item, tokens);
            }
        }
        Value::Object(map) => {
            for (key, value) in map {
                push_token(tokens, key);
                collect_json_tokens(value, tokens);
            }
        }
    }
}

fn dedupe_tokens(tokens: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for token in tokens {
        if !out.contains(&token) {
            out.push(token);
        }
    }
    out
}

fn text_contains_any(text: &str, tokens: &[String]) -> bool {
    tokens.iter().any(|token| text.contains(token))
}

fn chars_before(text: &str, byte_idx: usize, count: usize) -> String {
    text[..byte_idx]
        .chars()
        .rev()
        .take(count)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn chars_after(text: &str, byte_idx: usize, count: usize) -> String {
    text[byte_idx..].chars().take(count).collect()
}

fn text_window(text: &str, byte_idx: usize, before: usize, after: usize) -> String {
    format!(
        "{}{}",
        chars_before(text, byte_idx, before),
        chars_after(text, byte_idx, after)
    )
}

fn mechanical_term_is_negated(text: &str, byte_idx: usize) -> bool {
    let window = text_window(text, byte_idx, 24, 8);
    [
        "没有自动触发",
        "没有触发",
        "未触发",
        "不会触发",
        "不触发",
        "不需要",
        "无需",
        "无须",
        "不用",
        "不必",
        "免于",
        "不造成",
        "未造成",
        "没有造成",
        "没有损失",
        "未损失",
        "没有失去",
        "未失去",
        "没有伤害",
        "无伤害",
    ]
    .iter()
    .any(|marker| window.contains(marker))
}

fn term_followed_by_check_word(text: &str, byte_idx: usize, term: &str) -> bool {
    let after_idx = byte_idx + term.len();
    let after = chars_after(text, after_idx, 8);
    ["检定", "豁免", "check", "test", "roll"]
        .iter()
        .any(|word| after.contains(word))
}

fn is_ascii_word_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn term_occurrence_has_word_boundary(text: &str, byte_idx: usize, term: &str) -> bool {
    if !term
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return true;
    }
    let before = text[..byte_idx].chars().next_back();
    let after = text[byte_idx + term.len()..].chars().next();
    before.is_none_or(|ch| !is_ascii_word_char(ch))
        && after.is_none_or(|ch| !is_ascii_word_char(ch))
}

fn line_around_byte(text: &str, byte_idx: usize) -> &str {
    let start = text[..byte_idx].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    let end = text[byte_idx..]
        .find('\n')
        .map(|idx| byte_idx + idx)
        .unwrap_or(text.len());
    &text[start..end]
}

fn line_looks_like_option(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed.starts_with("•")
        || trimmed.chars().next().is_some_and(|ch| ch.is_ascii_digit())
            && trimmed
                .chars()
                .nth(1)
                .is_some_and(|ch| ch == '.' || ch == '、')
}

fn mechanical_term_is_future_option(text: &str, byte_idx: usize) -> bool {
    let line = line_around_byte(text, byte_idx);
    let before = chars_before(text, byte_idx, 240);
    let context = format!("{before}{line}");
    let has_option_shape = line_looks_like_option(line);
    let has_future_cue = [
        "你可以",
        "可以立刻",
        "可以立刻做的有",
        "可以立刻选择",
        "能立刻做的有",
        "下一步",
        "接下来",
        "从这里",
        "要定的是",
        "要哪一个",
        "你要",
        "选择",
        "要么",
        "或者",
        "what do you do",
        "you could",
        "from here",
        "or ",
    ]
    .iter()
    .any(|cue| context.contains(cue));
    has_option_shape && has_future_cue
}

fn mechanical_term_is_nonmechanical_homograph(text: &str, byte_idx: usize, term: &str) -> bool {
    let window = text_window(text, byte_idx, 8, 8);
    match term {
        "扣" => [
            "连接扣",
            "固定扣",
            "卡扣",
            "搭扣",
            "衣扣",
            "扣上",
            "扣住",
            "扣紧",
            "扣动",
            "扣下",
            "扣回",
            "扣稳",
            "扣着",
        ]
        .iter()
        .any(|marker| window.contains(marker)),
        "状态" => ["状态面板", "状态灯", "状态栏", "状态显示", "状态读数"]
            .iter()
            .any(|marker| window.contains(marker)),
        _ => false,
    }
}

fn environment_activation_context_is_negated_or_hypothetical(context: &str) -> bool {
    [
        "还没真正启动",
        "没真正启动",
        "没有真正启动",
        "未真正启动",
        "并未启动",
        "没有启动",
        "还没启动",
        "未启动",
        "怀疑会重新启动",
        "可能重新启动",
        "可能会重新启动",
        "会重新启动",
    ]
    .iter()
    .any(|marker| context.contains(marker))
}

fn context_has_number(text: &str) -> bool {
    text.chars().any(|c| c.is_ascii_digit())
        || [
            "一点", "两点", "三点", "四点", "五点", "六点", "七点", "八点", "九点", "十点",
        ]
        .iter()
        .any(|word| text.contains(word))
}

fn text_says_environment_activation_effect(text: &str) -> bool {
    let anchors = [
        "电动门",
        "闸门",
        "舱门",
        "门",
        "机构",
        "设备",
        "炮塔",
        "无人机",
        "drone",
        "turret",
    ];
    let activation_verbs = [
        "打开",
        "开启",
        "开了一截",
        "启动",
        "激活",
        "苏醒",
        "复位",
        "露头",
        "开始动作",
        "转动起来",
        "重新启动",
        "重新苏醒",
    ];
    let causation_cues = [
        "你",
        "这一下",
        "立刻",
        "随即",
        "随后",
        "结果",
        "相反",
        "回应",
        "触发",
        "导致",
        "失败",
        "误触",
        "重新",
        "开始",
    ];

    anchors.iter().any(|anchor| {
        text.match_indices(anchor).any(|(idx, _)| {
            if mechanical_term_is_negated(text, idx) {
                return false;
            }
            if mechanical_term_is_future_option(text, idx) {
                return false;
            }
            let context = text_window(text, idx, 32, 48);
            if environment_activation_context_is_negated_or_hypothetical(&context) {
                return false;
            }
            let has_activation = activation_verbs.iter().any(|verb| context.contains(verb));
            let has_causation = causation_cues.iter().any(|cue| context.contains(cue));
            has_activation && has_causation
        })
    })
}

fn text_says_check_or_roll(text: &str) -> bool {
    [
        "检定",
        "掷骰",
        "骰",
        "投掷",
        "roll",
        "check",
        "test",
        "saving throw",
        "difficulty",
        "dv",
    ]
    .iter()
    .any(|term| {
        text.match_indices(term).any(|(idx, _)| {
            term_occurrence_has_word_boundary(text, idx, term)
                && !mechanical_term_is_negated(text, idx)
                && !mechanical_term_is_future_option(text, idx)
        })
    })
}

fn text_says_luck_spend_adjustment(text: &str) -> bool {
    let mentions_luck = text.contains("luck") || text.contains("幸运");
    let spends_luck = ["花费", "消耗", "扣除", "花掉", "spend", "spent"]
        .iter()
        .any(|term| text.contains(term));
    let adjusts_result = [
        "改为",
        "变为",
        "调整",
        "降到",
        "降至",
        "提高到",
        "提高至",
        "adjust",
        "turn",
    ]
    .iter()
    .any(|term| text.contains(term));
    let names_result = ["检定结果", "结果", "成功", "失败", "roll result", "outcome"]
        .iter()
        .any(|term| text.contains(term));
    mentions_luck && spends_luck && adjusts_result && names_result
}

fn text_says_effect(text: &str) -> bool {
    let effect_terms = [
        "hp",
        "生命值",
        "damage",
        "伤害",
        "损失",
        "失去",
        "扣",
        "san",
        "sanity",
        "理智",
        "luck",
        "幸运",
        "resource",
        "资源",
        "condition",
        "状态",
        "clock",
        "进度",
        "破坏",
        "摧毁",
    ];
    let effect_verbs = [
        "受到", "造成", "失去", "损失", "扣", "扣除", "花费", "消耗", "减少", "降低", "恢复",
        "回复", "增加", "治疗", "获得", "陷入", "移除", "推进", "改变", "变为", "变成", "破坏",
        "摧毁",
    ];

    text_says_environment_activation_effect(text)
        || effect_terms.iter().any(|term| {
            text.match_indices(term).any(|(idx, _)| {
                if !term_occurrence_has_word_boundary(text, idx, term) {
                    return false;
                }
                if mechanical_term_is_negated(text, idx) {
                    return false;
                }
                if mechanical_term_is_future_option(text, idx)
                    || mechanical_term_is_nonmechanical_homograph(text, idx, term)
                {
                    return false;
                }
                if matches!(*term, "伤害" | "理智" | "san" | "sanity")
                    && term_followed_by_check_word(text, idx, term)
                {
                    return false;
                }

                let context = text_window(text, idx, 18, 18);
                let has_effect_verb = effect_verbs.iter().any(|verb| context.contains(verb));
                let object_state_term = matches!(*term, "condition" | "状态" | "clock" | "进度");
                let destructive_term = matches!(*term, "破坏" | "摧毁");
                destructive_term
                    || (has_effect_verb && (object_state_term || context_has_number(&context)))
            })
        })
}

fn text_asks_player_for_manual_roll(text: &str) -> bool {
    let asks_roll = ["请", "需要", "告诉", "回复", "roll", "掷", "投"]
        .iter()
        .any(|term| text.contains(term));
    let asks_total = ["点数", "总值", "total", "result", "结果数值", "数字结果"]
        .iter()
        .any(|term| text.contains(term));
    asks_roll && asks_total
}

fn text_mislabels_static_dv_as_opposed(text: &str, ledger: &TurnLedgerSnapshot) -> bool {
    let says_opposed_static_target = ["对抗 dv", "对抗dv", "对抗 dc", "对抗dc"]
        .iter()
        .any(|needle| text.contains(needle));
    if !says_opposed_static_target {
        return false;
    }
    let has_static = ledger
        .check_contracts
        .iter()
        .any(|check| matches!(check.target, CheckTargetModel::StaticNumber { .. }))
        || ledger
            .check_results
            .iter()
            .any(check_result_records_static_target);
    let has_opposed = ledger
        .check_contracts
        .iter()
        .any(|check| matches!(check.target, CheckTargetModel::Opposed { .. }))
        || ledger
            .check_results
            .iter()
            .any(check_result_records_opposed);
    has_static && !has_opposed
}

fn check_result_records_static_target(result: &CheckResultRecord) -> bool {
    result
        .outcome
        .pointer("/opposition_kind")
        .and_then(Value::as_str)
        == Some("static_check")
        || result
            .outcome
            .pointer("/resolution_model/kind")
            .and_then(Value::as_str)
            == Some("static_target_number")
}

fn check_result_records_opposed(result: &CheckResultRecord) -> bool {
    result.outcome.pointer("/opposed").is_some()
        || result
            .outcome
            .pointer("/opposition_kind")
            .and_then(Value::as_str)
            == Some("opposed_check")
        || matches!(
            result
                .outcome
                .pointer("/resolution_model/kind")
                .and_then(Value::as_str),
            Some("opposed_roll" | "dice_pool_opposed")
        )
}

fn blocked_movement_check_narrated_as_arrival(text: &str, ledger: &TurnLedgerSnapshot) -> bool {
    ledger
        .check_results
        .iter()
        .any(check_result_is_blocked_movement)
        && text_claims_completed_movement(text)
}

fn check_result_is_blocked_movement(result: &CheckResultRecord) -> bool {
    if result.outcome.pointer("/blocked").and_then(Value::as_bool) != Some(true) {
        return false;
    }
    let label = result
        .outcome
        .pointer("/check_label")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    [
        "move", "dash", "cross", "reach", "approach", "enter", "sprint", "run", "冲", "移动",
        "穿过", "抵达", "到达", "进入", "靠近", "贴",
    ]
    .iter()
    .any(|cue| label.contains(cue))
}

fn text_claims_completed_movement(text: &str) -> bool {
    [
        "你已经不在",
        "你人稳在",
        "你稳在",
        "你已经到了",
        "你已经到达",
        "你到达",
        "你抵达",
        "你来到",
        "你进入",
        "你冲进",
        "你穿过",
        "你移动到",
        "你挪到",
        "你把身体收进",
        "把身体收进",
        "你稳稳藏进",
        "稳稳藏进",
        "你已经伏在",
        "你伏在",
        "你贴上",
        "你贴到",
        "你贴住",
        "贴住门框",
    ]
    .iter()
    .any(|cue| text.contains(cue))
}

fn player_agency_violation_detail(text: &str) -> Option<String> {
    let lowered = normalize_text(text);
    if hollow_suspension_of_declared_action(&lowered) {
        return Some(
            "player-facing narration suspends or negates a concrete player-declared action instead of resolving it into a current fictional result"
                .to_string(),
        );
    }
    let action_cues = [
        "选择其一",
        "可以选择",
        "你现在可以立刻",
        "你接下来可以",
        "下一步你可以",
        "下一步可以",
        "可以直接做",
        "可以立刻做",
        "可以立刻做的有",
        "做的有",
        "几条路",
        "选哪一个",
    ];
    if text_has_any(&lowered, &["选择其一", "选哪一个"]) {
        return Some("player-facing narration presents an explicit choice prompt".to_string());
    }
    if text_has_any(
        &lowered,
        &[
            "下一步可以",
            "你下一步可以",
            "接下来可以",
            "你接下来可以",
            "your next move could be",
        ],
    ) {
        return Some(
            "player-facing narration ends by prompting the player for a menu-like next action"
                .to_string(),
        );
    }
    if inline_followup_action_menu_violation(&lowered) {
        return Some(
            "player-facing narration presents inline follow-up opportunities as an action menu"
                .to_string(),
        );
    }

    let listed_lines = text
        .lines()
        .filter(|line| line_is_player_facing_list_item(line))
        .count();
    if listed_lines < 2 {
        return None;
    }

    if text_has_any(&lowered, &action_cues) {
        return Some(
            "player-facing narration lists possible player actions instead of presenting diegetic affordances"
                .to_string(),
        );
    }

    let dump_cues = [
        "关键事实",
        "你已经确认",
        "你已经能确定",
        "能确定的是",
        "现在能确认",
        "你已经知道",
        "你得到的信息",
        "可你已经能确定",
        "你可以确定",
    ];
    if text_has_any(text, &dump_cues) || listed_lines >= 3 {
        return Some(
            "player-facing narration dumps clues or findings as a manifest list".to_string(),
        );
    }

    None
}

fn inline_followup_action_menu_violation(text: &str) -> bool {
    if prepared_action_menu_violation(text) {
        return true;
    }
    if prose_branch_action_menu_violation(text) {
        return true;
    }
    if either_or_action_menu_violation(text) {
        return true;
    }
    let menu_frame = [
        "后续机会",
        "实际的后续机会",
        "实际机会",
        "你现在有几个",
        "你眼前有几个",
        "你面前有几个",
        "几个很近",
        "你把这些观察串在一起",
        "观察串在一起",
    ];
    if !text_has_any(text, &menu_frame) || !text.contains('：') && !text.contains(':') {
        return false;
    }

    let branch_count =
        text.matches('；').count() + text.matches("或者").count() + text.matches("或是").count();
    if branch_count < 2 {
        return false;
    }

    inline_player_action_branch_count(text) >= 2
}

fn prepared_action_menu_violation(text: &str) -> bool {
    let tail = ["接下来你是准备", "接下来你准备"]
        .iter()
        .find_map(|cue| text.split_once(cue).map(|(_, rest)| rest));
    let Some(tail) = tail else {
        return false;
    };
    if !["还是", "或者", "或是"]
        .iter()
        .any(|connector| tail.contains(connector))
    {
        return false;
    }
    let branch_count = tail.matches("还是").count()
        + tail.matches("或者").count()
        + tail.matches("或是").count()
        + tail.matches('；').count()
        + tail.matches(';').count();
    if branch_count < 2 {
        return false;
    }
    tail.split('；')
        .flat_map(|part| part.split(';'))
        .flat_map(|part| part.split("还是"))
        .flat_map(|part| part.split("或者"))
        .flat_map(|part| part.split("或是"))
        .filter(|part| branch_looks_like_player_action(part))
        .count()
        >= 2
}

fn either_or_action_menu_violation(text: &str) -> bool {
    let has_frame = text.matches("要么").count() >= 2
        || text.contains("要么") && (text.contains("或者") || text.contains("或是"));
    if !has_frame {
        return false;
    }
    text.split("要么")
        .flat_map(|part| part.split("或者"))
        .flat_map(|part| part.split("或是"))
        .filter(|part| branch_looks_like_player_action(part))
        .count()
        >= 2
}

fn inline_player_action_branch_count(text: &str) -> usize {
    let tail = text
        .split_once('：')
        .map(|(_, rest)| rest)
        .or_else(|| text.split_once(':').map(|(_, rest)| rest))
        .unwrap_or(text);
    tail.split(|c| c == '；')
        .flat_map(|part| part.split("或者"))
        .flat_map(|part| part.split("或是"))
        .filter(|part| branch_looks_like_player_action(part))
        .count()
}

fn branch_looks_like_player_action(part: &str) -> bool {
    let s = part.trim_matches(|c: char| c.is_whitespace() || "，。；:：、".contains(c));
    if s.is_empty() {
        return false;
    }
    if let Some((head, rest)) = s.split_once('，') {
        if head.chars().count() <= 16 && branch_looks_like_player_action(rest) {
            return true;
        }
    }
    let action_starts = [
        "先",
        "再",
        "继续",
        "有人",
        "下车",
        "试着",
        "尝试",
        "冒险",
        "顺着",
        "确认",
        "切断",
        "断线",
        "撬开",
        "靠近",
        "贴",
        "冲",
        "转向",
        "追",
        "开火",
        "射击",
        "喊话",
        "撤",
        "进入",
        "压制",
        "破坏",
        "扑近",
        "沿墙",
        "摸到",
        "想办法",
    ];
    if action_starts.iter().any(|prefix| s.starts_with(prefix)) {
        return true;
    }
    s.starts_with('对')
        && ["喊话", "发号施令", "开火", "射击"]
            .iter()
            .any(|verb| s.contains(verb))
}

fn prose_branch_action_menu_violation(text: &str) -> bool {
    let frames = ["无论你是想", "无论你想", "无论你是要", "无论你要"];
    if !text_has_any(text, &frames) || !text.contains("还是") {
        return false;
    }
    let branch_count = text.matches('、').count()
        + text.matches('，').count()
        + text.matches('；').count()
        + text.matches("或者").count()
        + text.matches("或是").count()
        + text.matches("还是").count();
    if branch_count < 3 {
        return false;
    }
    let action_verbs = [
        "继续", "试着", "尝试", "冒险", "顺着", "确认", "切断", "断线", "撬开", "靠近", "贴", "冲",
        "开火", "射击", "喊话", "撤", "进入", "压制", "破坏", "扑近", "沿墙", "摸到",
    ];
    action_verbs
        .iter()
        .filter(|verb| text.contains(**verb))
        .count()
        >= 3
}

fn hollow_suspension_of_declared_action(text: &str) -> bool {
    let action_context = [
        "冲",
        "扑",
        "跑",
        "sprint",
        "burst",
        "warehouse",
        "仓库",
        "侧门",
        "门把",
        "警车",
        "火线",
        "掩体",
        "墙根",
        "线缆",
        "走向",
        "可接近",
    ]
    .iter()
    .any(|term| text.contains(term));
    if !action_context {
        return false;
    }

    let still_in_starting_cover = text_has_any(
        text,
        &[
            "还伏在",
            "仍伏在",
            "仍旧伏在",
            "仍然伏在",
            "依旧伏在",
            "依然伏在",
            "还压在",
            "仍压在",
            "仍然压在",
            "依旧压在",
            "依然压在",
            "还蹲在",
            "还躲在",
        ],
    ) && text_has_any(
        text,
        &[
            "掩护后",
            "警车后",
            "巡逻车后",
            "车后",
            "这点掩护",
            "这片掩护",
        ],
    );
    let future_window = text_has_any(
        text,
        &[
            "下一次",
            "下一个",
            "下轮",
            "火力空隙",
            "火力轮转",
            "武器循环",
            "空档",
            "空隙",
        ],
    );
    let wait_or_prepare = text_has_any(
        text,
        &["等", "等待", "准备", "随时", "就会", "将会", "才会", "就要"],
    );
    let departure_action = text_has_any(
        text,
        &[
            "扑出去",
            "冲出去",
            "猛冲出去",
            "跑出去",
            "钻过去",
            "扑向仓库",
            "冲向仓库",
            "离开掩体",
        ],
    );
    let waiting_for_future_departure = text_has_any(
        text,
        &[
            "等着下一个",
            "等待下一个",
            "等着下一次",
            "等待下一次",
            "随时准备抓住下一次",
            "准备抓住下一次",
            "下一次火力",
            "下一个火力",
            "下一次武器循环",
        ],
    ) && text_has_any(
        text,
        &[
            "扑出去",
            "冲出去",
            "猛冲出去",
            "跑出去",
            "钻过去",
            "扑向仓库",
            "冲向仓库",
        ],
    );
    let future_departure_from_cover =
        text_has_any(text, &["下一次", "下一个", "火力稍稍回落", "火力轮转"])
            && text_has_any(text, &["你就会", "才会", "将会", "就要从"])
            && text_has_any(text, &["掩护后", "警车后", "巡逻车后", "这点掩护"])
            && text_has_any(
                text,
                &[
                    "冲出去",
                    "猛冲出去",
                    "扑出去",
                    "跑出去",
                    "扑向仓库",
                    "冲向仓库",
                ],
            );
    let no_result_suspension =
        text_has_any(
            text,
            &[
                "没有出现任何可见的变化",
                "没有可见的变化",
                "没有推进到新的位置",
                "还没有明确显露",
                "还没有从眼前",
            ],
        ) && text_has_any(text, &["线缆", "走向", "可接近", "机会或风险", "局面"]);

    (text.contains("真正把身体甩出去") && text.contains("没有发生"))
        || text.contains("这一扑还没有")
        || text.contains("这一扑还没")
        || text.contains("没有离开这辆警车")
        || text.contains("没有离开警车")
        || (text.contains("还没有摸到") && text.contains("门"))
        || (text.contains("还没摸到") && text.contains("门"))
        || (text.contains("只差那一瞬") && (text.contains("扑过去") || text.contains("冲出去")))
        || (still_in_starting_cover && future_window && wait_or_prepare && departure_action)
        || (still_in_starting_cover && waiting_for_future_departure)
        || future_departure_from_cover
        || no_result_suspension
}

fn line_is_player_facing_list_item(line: &str) -> bool {
    let s = line.trim_start();
    s.starts_with("- ") || s.starts_with("* ") || s.starts_with("• ") || line_starts_numbered(s)
}

fn line_starts_numbered(s: &str) -> bool {
    let mut chars = s.chars().peekable();
    let mut saw_digit = false;
    while matches!(chars.peek(), Some(ch) if ch.is_ascii_digit()) {
        saw_digit = true;
        chars.next();
    }
    if !saw_digit {
        return false;
    }
    match chars.next() {
        Some('.') | Some('、') | Some(')') => match chars.peek() {
            None => true,
            Some(ch) => ch.is_whitespace() || *ch == '*',
        },
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;

    #[test]
    fn rejects_check_claim_without_contract_or_gate() {
        let ledger = TurnLedgerSnapshot::default();
        let submission = FinalNarrationSubmission {
            player_visible_text: "这里需要一次潜行检定。".into(),
            mechanical_claims: vec![MechanicalClaim::new(
                MechanicalClaimKind::Check,
                "需要潜行检定",
            )],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(!result.accepted);
        assert_eq!(
            result.next_required_action,
            Some(VerifierNextAction::CallProposeCheck)
        );
        assert!(result
            .findings
            .iter()
            .any(|f| f.kind == VerifierFindingKind::MissingCheck));
    }

    #[test]
    fn rejects_resolved_roll_claim_when_check_was_not_executed() {
        let ledger = TurnLedgerSnapshot {
            check_contracts: vec![sample_check("check_stealth", "潜行检定")],
            ..Default::default()
        };
        let submission = FinalNarrationSubmission {
            player_visible_text: "你的潜行检定成功了。".into(),
            mechanical_claims: vec![MechanicalClaim::new(
                MechanicalClaimKind::Roll,
                "潜行检定成功",
            )],
            referenced_ledger_ids: vec!["check_stealth".into()],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(!result.accepted);
        assert_eq!(
            result.next_required_action,
            Some(VerifierNextAction::CallExecuteCheck)
        );
        assert!(result
            .findings
            .iter()
            .any(|f| f.kind == VerifierFindingKind::MissingRollExecution));
    }

    #[test]
    fn rejects_invented_damage_without_effect_or_impact() {
        let ledger = TurnLedgerSnapshot {
            check_contracts: vec![sample_check("check_attack", "手枪攻击")],
            check_results: vec![sample_result(
                "check_attack",
                RollVisibility::PublicGmRoll,
                "1d10+12",
                json!({"total": 18}),
            )],
            ..Default::default()
        };
        let submission = FinalNarrationSubmission {
            player_visible_text: "手枪攻击命中，清道夫损失 8 点 HP。".into(),
            mechanical_claims: vec![MechanicalClaim::new(
                MechanicalClaimKind::Damage,
                "损失 8 点 HP",
            )],
            referenced_ledger_ids: vec!["check_attack".into()],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(!result.accepted);
        assert_eq!(
            result.next_required_action,
            Some(VerifierNextAction::CallApplyEffect)
        );
        assert!(result
            .findings
            .iter()
            .any(|f| f.kind == VerifierFindingKind::InventedEffect));
    }

    #[test]
    fn rejects_final_text_that_omits_visible_roll_result() {
        let ledger = TurnLedgerSnapshot {
            check_contracts: vec![sample_check("check_attack", "手枪攻击")],
            check_results: vec![sample_result(
                "check_attack",
                RollVisibility::PublicGmRoll,
                "1d10+12",
                json!({"total": 18, "band": "success"}),
            )],
            ..Default::default()
        };
        let submission = FinalNarrationSubmission {
            player_visible_text: "你抬枪压住对方，空气紧了一瞬。".into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(!result.accepted);
        assert_eq!(
            result.next_required_action,
            Some(VerifierNextAction::ReviseText)
        );
        assert!(result
            .findings
            .iter()
            .any(|f| f.kind == VerifierFindingKind::OmittedVisibleResult));
    }

    #[test]
    fn accepts_consistent_check_effect_and_visible_result() {
        let ledger = TurnLedgerSnapshot {
            check_contracts: vec![sample_check("check_attack", "手枪攻击")],
            check_results: vec![sample_result(
                "check_attack",
                RollVisibility::PublicGmRoll,
                "1d10+12",
                json!({"total": 18, "band": "success"}),
            )],
            effect_contracts: vec![sample_effect("effect_damage", "npc.scav")],
            parameter_impacts: vec![sample_impact("impact_hp", "npc.scav", "hp.current", -8)],
            ..Default::default()
        };
        let submission = FinalNarrationSubmission {
            player_visible_text: "手枪攻击 1d10+12 掷出 18，命中。effect_damage 生效，npc.scav 的 hp.current 受到 -8 影响。".into(),
            mechanical_claims: vec![
                MechanicalClaim::new(MechanicalClaimKind::Roll, "手枪攻击 18"),
                MechanicalClaim::new(MechanicalClaimKind::Damage, "hp.current -8"),
            ],
            referenced_ledger_ids: vec!["check_attack".into(), "effect_damage".into(), "impact_hp".into()],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(result.accepted, "{:?}", result.findings);
        assert!(result.findings.is_empty());
        assert_eq!(result.next_required_action, None);
    }

    #[test]
    fn accepts_system_roll_gate_prompt_that_does_not_request_manual_totals() {
        std::env::set_var("TRPG_AGENT_TABLE_DICE_POLICY", "system_rolls_visible");
        let check = sample_check("check_stealth", "潜行检定");
        let ledger = TurnLedgerSnapshot {
            check_contracts: vec![check.clone()],
            interaction_gates: vec![InteractionGate::from_pending_check(
                &crate::make_pending_check(&check),
            )],
            ..Default::default()
        };
        let submission = FinalNarrationSubmission {
            player_visible_text:
                "这里需要一次潜行检定。请只回复 `roll`，系统会调用骰子工具并写入结果。".into(),
            mechanical_claims: vec![MechanicalClaim::new(
                MechanicalClaimKind::Check,
                "需要潜行检定",
            )],
            referenced_ledger_ids: vec!["check_stealth".into()],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(result.accepted, "{:?}", result.findings);
    }

    #[test]
    fn verifier_structured_refs_accept_known_ids() {
        // B7 测试 1：引用账本真实 id 全集 → accepted。OmittedVisibleResult 语义
        // 保留（可见结果 token 检查独立运行），故构造 token 在文本中存在的用例。
        let result_record = sample_result(
            "check_attack",
            RollVisibility::PublicGmRoll,
            "1d10+12",
            json!({"total": 18, "band": "success"}),
        );
        let ledger = TurnLedgerSnapshot {
            check_contracts: vec![sample_check("check_attack", "手枪攻击")],
            dice_rolls: vec![result_record.roll.clone()],
            check_results: vec![result_record],
            effect_contracts: vec![sample_effect("effect_damage", "npc.scav")],
            parameter_impacts: vec![sample_impact("impact_hp", "npc.scav", "hp.current", -8)],
            ..Default::default()
        };
        let submission = FinalNarrationSubmission {
            player_visible_text: "手枪攻击 1d10+12 掷出 18，命中。effect_damage 生效，npc.scav 的 hp.current 受到 -8 影响。".into(),
            mechanical_claims: vec![
                MechanicalClaim::new(MechanicalClaimKind::Roll, "手枪攻击 18"),
                MechanicalClaim::new(MechanicalClaimKind::Damage, "hp.current -8"),
            ],
            referenced_ledger_ids: vec![
                "check_attack".into(),
                "roll_check_attack".into(),
                "effect_damage".into(),
                "impact_hp".into(),
            ],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(result.accepted, "{:?}", result.findings);
        assert!(result.findings.is_empty());
        assert_eq!(result.next_required_action, None);
    }

    #[test]
    fn verifier_structured_refs_reject_unknown_id() {
        // B7 测试 2：引用不存在的账本 id → InventedEffect finding 含该 id。
        let ledger = TurnLedgerSnapshot {
            check_contracts: vec![sample_check("check_attack", "手枪攻击")],
            ..Default::default()
        };
        let submission = FinalNarrationSubmission {
            player_visible_text: "手枪攻击蓄势待发。".into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec!["check_不存在".into()],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(!result.accepted);
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.kind == VerifierFindingKind::InventedEffect
                    && f.detail.contains("check_不存在")),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_empty_refs_falls_back_with_marker() {
        // B7 测试 3：引用为空 + 文本声称伤害无账本证据 → 回退子串扫描，
        // finding detail 以 `fallback:substring_scan: ` 开头（可观测技术债）。
        let ledger = TurnLedgerSnapshot::default();
        let submission = FinalNarrationSubmission {
            player_visible_text: "清道夫受到 8 点伤害，踉跄后退。".into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(!result.accepted);
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.kind == VerifierFindingKind::InventedEffect
                    && f.detail.starts_with("fallback:substring_scan: ")),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_fallback_ignores_negated_no_roll_no_effect_text() {
        // Live CoC regression: the GM may explicitly say no roll/effect is
        // triggered. That sentence must not create retroactive mechanical debt.
        let ledger = TurnLedgerSnapshot::default();
        let submission = FinalNarrationSubmission {
            player_visible_text:
                "此刻没有自动触发的理智检定或伤害检定；你只是抵达并观察小镇入口，基础可见信息不需要掷骰。"
                    .into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(result.accepted, "{:?}", result.findings);
        assert!(result.findings.is_empty(), "{:?}", result.findings);
    }

    #[test]
    fn verifier_fallback_allows_luck_spend_adjusting_prior_roll_with_effect_evidence() {
        // Live CoC regression: spending Luck to adjust an already-rolled check
        // is an effect/resource spend in this turn, not a second fresh check.
        let mut effect = sample_effect("effect_luck_spend", "pc.current");
        effect.visibility = Visibility::GmOnly;
        let mut impact = sample_impact("impact_luck", "pc.current", "resources.luck.current", 36);
        impact.visibility = Visibility::GmOnly;
        let ledger = TurnLedgerSnapshot {
            effect_contracts: vec![effect],
            parameter_impacts: vec![impact],
            ..Default::default()
        };
        let submission = FinalNarrationSubmission {
            player_visible_text:
                "[system]你花费 36 点 Luck，将手枪检定结果从 91 降到 55，刚好改为普通成功。[/system]\n[roll]上一枪经 Luck 花费改为成功：子弹命中追踪皮卡前轮。[/roll]"
                    .into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(result.accepted, "{:?}", result.findings);
        assert!(
            result
                .findings
                .iter()
                .all(|f| f.kind != VerifierFindingKind::MissingCheck),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_structured_check_refs_still_reject_effect_claim_without_effect_evidence() {
        // Live playtest regression: a turn can correctly reference a rolled check
        // while the final narration also claims HP loss. A valid check id must not
        // mask the missing apply_effect / parameter-impact ledger evidence.
        let result_record = sample_result(
            "check_heat",
            RollVisibility::PublicGmRoll,
            "1d100",
            json!({"total": 87, "band": "failure"}),
        );
        let ledger = TurnLedgerSnapshot {
            check_contracts: vec![sample_check("check_heat", "高温与疲惫")],
            dice_rolls: vec![result_record.roll.clone()],
            check_results: vec![result_record],
            ..Default::default()
        };
        let submission = FinalNarrationSubmission {
            player_visible_text: "高温与疲惫造成体力损耗：林岚失去 1 点生命值。".into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec!["check_heat".into(), "roll_check_heat".into()],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(!result.accepted, "{:?}", result.findings);
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.kind == VerifierFindingKind::InventedEffect),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_fallback_accepts_prior_committed_power_fact_as_effect_evidence() {
        // Live Homecoming regression: a later turn may restate an already
        // committed world fact. The verifier should not treat that restatement
        // as a new invented effect just because the current-turn ledger is empty.
        let ledger = TurnLedgerSnapshot {
            committed_world_facts: vec![CommittedWorldFactEvidence {
                fact_id: "fact_cut_power".into(),
                summary:
                    "Found the main breaker / emergency stop and cut power to the server cables."
                        .into(),
                truth_status: Some("true".into()),
            }],
            ..Default::default()
        };
        let submission = FinalNarrationSubmission {
            player_visible_text:
                "此前找到并拍下总电闸/急停、切断服务器缆线供电已经成功成立；后续追加处理失败不会撤销这个既成事实。"
                    .into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(result.accepted, "{:?}", result.findings);
        assert!(
            result
                .findings
                .iter()
                .all(|f| f.kind != VerifierFindingKind::InventedEffect),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_fallback_rejects_unrelated_world_fact_for_damage_claim() {
        // A committed fact is evidence only for matching prior state, not a
        // blanket waiver for fresh mechanics such as HP loss.
        let ledger = TurnLedgerSnapshot {
            committed_world_facts: vec![CommittedWorldFactEvidence {
                fact_id: "fact_signage".into(),
                summary: "The office sign on the door reads ED-02.".into(),
                truth_status: Some("true".into()),
            }],
            ..Default::default()
        };
        let submission = FinalNarrationSubmission {
            player_visible_text: "清道夫受到 8 点伤害，踉跄后退。".into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(!result.accepted, "{:?}", result.findings);
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.kind == VerifierFindingKind::InventedEffect),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_fallback_allows_observed_power_status_without_effect_evidence() {
        // Live Homecoming regression: "供电状态" can be an observed UI label or
        // scene description. The substring "状态" alone is not a mechanical
        // condition/effect claim.
        let ledger = TurnLedgerSnapshot::default();
        let submission = FinalNarrationSubmission {
            player_visible_text:
                "屏幕上显示多路连接和供电状态；这地方像个临时充电巢，设备还在工作。".into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(result.accepted, "{:?}", result.findings);
        assert!(
            result
                .findings
                .iter()
                .all(|f| f.kind != VerifierFindingKind::InventedEffect),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_fallback_allows_residual_power_observation_without_activation() {
        // Live Homecoming regression: observing residual-power device status is
        // not the same as causing doors/turrets/drones to activate.
        let ledger = TurnLedgerSnapshot::default();
        let submission = FinalNarrationSubmission {
            player_visible_text:
                "仓库深处另外几台接在系统上的老旧设备还带着残电：一台小型转台摄像头正一抽一抽地缓慢摆头；旁边一块状态面板还在闪黄灯；几具旧无人机外壳偶尔会抖一下，像只是被残余电流带得痉挛，还没真正启动。"
                    .into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(result.accepted, "{:?}", result.findings);
        assert!(
            result
                .findings
                .iter()
                .all(|f| f.kind != VerifierFindingKind::InventedEffect),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_fallback_rejects_environment_activation_without_effect_evidence() {
        // Live Homecoming regression: a failed check may have a cost, but
        // persistent environment changes still need effect/world-state evidence.
        let ledger = TurnLedgerSnapshot::default();
        let submission = FinalNarrationSubmission {
            player_visible_text:
                "你拍下按钮后，深处的老旧电动门缓缓又开了一截，某个转动机构在黑暗里重新苏醒。"
                    .into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(!result.accepted, "{:?}", result.findings);
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.kind == VerifierFindingKind::InventedEffect),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_fallback_ignores_gm_only_evidence_audit_sidecar() {
        // GM-only sidecars can contain ids such as `advance_time_1`; the substring
        // `dv` inside those ids must not be parsed as a player-visible DV/check claim.
        let ledger = TurnLedgerSnapshot::default();
        let submission = FinalNarrationSubmission {
            player_visible_text:
                "No movement in the shadows.\n[evidence_audit]{\"basis\":[\"commit:advance_time_1\"]}[/evidence_audit]"
                    .into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(result.accepted, "{:?}", result.findings);
        assert!(
            result
                .findings
                .iter()
                .all(|f| f.kind != VerifierFindingKind::MissingCheck),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_rejects_future_action_menu_as_player_agency_violation() {
        // Player-facing action menus are a constitution violation even when
        // they are phrased as future possibilities rather than committed effects.
        let ledger = TurnLedgerSnapshot::default();
        let submission = FinalNarrationSubmission {
            player_visible_text:
                "你现在已经确认目标方向。下一步你可以立刻：\n- 继续贴门边，先朝亮着的设备打几枪压制/破坏\n- 改为找掩体"
                    .into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(!result.accepted, "{:?}", result.findings);
        assert_eq!(
            result.next_required_action,
            Some(VerifierNextAction::ReviseText)
        );
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.kind == VerifierFindingKind::PlayerAgencyViolation),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_rejects_late_action_menu_as_player_agency_violation() {
        // The option header may be several bullets above a destructive word.
        let ledger = TurnLedgerSnapshot::default();
        let submission = FinalNarrationSubmission {
            player_visible_text:
                "你眼前这半秒窗口里，可以立刻做的有：\n\n- 再狠狠干一次，赌下一把把接头彻底拽断\n- 直接顺着缆线扑进仓库侧的服务器面板，从里面断电\n- 趁它失衡贴近本体，冒险近身控制或破坏\n- 立刻翻回墙后"
                    .into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(!result.accepted, "{:?}", result.findings);
        assert_eq!(
            result.next_required_action,
            Some(VerifierNextAction::ReviseText)
        );
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.kind == VerifierFindingKind::PlayerAgencyViolation),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_rejects_inline_followup_opportunities_as_player_agency_violation() {
        let ledger = TurnLedgerSnapshot::default();
        let submission = FinalNarrationSubmission {
            player_visible_text:
                "你已经撬开维护盖板，看见里面一粗一细两组活线。你现在有几个很近、很实际的后续机会：继续顺着这道缝看清哪根线是电；试着从这里做更精细的技术处理；或者冒险再往前贴一点确认背后的接口。"
                    .into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(!result.accepted, "{:?}", result.findings);
        assert_eq!(
            result.next_required_action,
            Some(VerifierNextAction::ReviseText)
        );
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.kind == VerifierFindingKind::PlayerAgencyViolation),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_rejects_prose_rewritten_action_list_as_player_agency_violation() {
        let ledger = TurnLedgerSnapshot::default();
        let submission = FinalNarrationSubmission {
            player_visible_text:
                "你现在有了一个短窗口。你把这些观察串在一起：趁它还没完全停摆，冲出去控制、拖拽或缴械；转向仓库内侧，追服务器和那根线的源头；对警员发号施令，争取把他们从射界里撤出来；先观察它接下来几秒还保不保持火控与瞄准能力。"
                    .into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(!result.accepted, "{:?}", result.findings);
        assert_eq!(
            result.next_required_action,
            Some(VerifierNextAction::ReviseText)
        );
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.kind == VerifierFindingKind::PlayerAgencyViolation),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_allows_fact_summary_after_observation_frame() {
        let ledger = TurnLedgerSnapshot::default();
        let submission = FinalNarrationSubmission {
            player_visible_text:
                "你把这些观察串在一起：那台人形无人机和仓库之间，确实连着一根明显的线缆；它的火力扇区主要咬着正前方开阔地；两名警员都还活着，但伤得不轻；它和仓库里的东西之间，很可能不只是拴着，更像是被供电、控制，或者两者都有。"
                    .into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(
            !result
                .findings
                .iter()
                .any(|f| f.kind == VerifierFindingKind::PlayerAgencyViolation),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_rejects_no_colon_prose_branch_menu_as_player_agency_violation() {
        let ledger = TurnLedgerSnapshot::default();
        let submission = FinalNarrationSubmission {
            player_visible_text:
                "你现在已经贴上仓库外墙，离那台东西更近。下一步，无论你是想继续沿墙摸到门口、扑近它后背那块异常装配区、冲进仓库内侧，还是先朝警员喊话配合，时机都比刚才好多了。"
                    .into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(!result.accepted, "{:?}", result.findings);
        assert_eq!(
            result.next_required_action,
            Some(VerifierNextAction::ReviseText)
        );
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.kind == VerifierFindingKind::PlayerAgencyViolation),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_rejects_clue_dump_manifest_as_player_agency_violation() {
        let ledger = TurnLedgerSnapshot::default();
        let submission = FinalNarrationSubmission {
            player_visible_text:
                "你暂时还没定位到能一把切进去的控制口，可你已经能确定：\n\n- 这玩意儿背后有可侵入的本地系统；\n- 真正的控制核心不在街面上，在仓库内部更深处；\n- 你如果想夺控制权，得更近。"
                    .into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(!result.accepted, "{:?}", result.findings);
        assert_eq!(
            result.next_required_action,
            Some(VerifierNextAction::ReviseText)
        );
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.kind == VerifierFindingKind::PlayerAgencyViolation),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_rejects_hollow_suspension_of_declared_player_action() {
        let ledger = TurnLedgerSnapshot::default();
        let submission = FinalNarrationSubmission {
            player_visible_text:
                "你已经不再犹豫要不要冲了，仓库右侧的外墙和门把手都被你钉得很清楚。可真正把身体甩出去的那一下，仍旧没有发生。你没有离开这辆警车后的窄缝，也还没有摸到右侧仓库门的把手。"
                    .into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(!result.accepted, "{:?}", result.findings);
        assert_eq!(
            result.next_required_action,
            Some(VerifierNextAction::ReviseText)
        );
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.kind == VerifierFindingKind::PlayerAgencyViolation),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_rejects_no_result_suspension_of_cable_observation() {
        let ledger = TurnLedgerSnapshot::default();
        let submission = FinalNarrationSubmission {
            player_visible_text:
                "你没有贸然探身出去，只是贴着掩体的边缘，一点点挪动视线，试着沿那根被拖向仓库内部的线缆去辨认它贴地延伸的方向，想找出它经过的位置，以及是否有哪一段落在火线之外、足够接近。\n\n但这一轮里，局面没有出现任何可见的变化。你没有因此暴露自己，也没有推进到新的位置；线缆的走向、可接近与否，以及周围是否存在新的机会或风险，都还没有从眼前这片紧绷的静默中明确显露出来。"
                    .into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(!result.accepted, "{:?}", result.findings);
        assert_eq!(
            result.next_required_action,
            Some(VerifierNextAction::ReviseText)
        );
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.kind == VerifierFindingKind::PlayerAgencyViolation),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_rejects_future_tense_delay_after_declared_sprint() {
        let ledger = TurnLedgerSnapshot::default();
        for player_visible_text in [
            "你还伏在这片掩护后，重心已经前送，等着下一个火力轮转里真正能扑出去的空隙。",
            "你不再继续伏着等下去。下一次火力稍稍回落，你就会从这点掩护后猛冲出去，扑向仓库右侧那片阴影。",
            "你把每一处可能让你脱离火线的落点都迅速过了一遍，身体绷在起跑前的那一下，重心压低，随时准备抓住下一次武器循环的空档扑出去。可就在这一刻，你依旧伏在巡逻车后的冷硬掩体边，枪口朝前，目光钉着仓库那一侧可供切入的黑暗。",
            "你依然压在警车后，准备等火力空隙冲出去，视线锁住仓库侧墙。",
        ] {
            let submission = FinalNarrationSubmission {
                player_visible_text: player_visible_text.into(),
                mechanical_claims: vec![],
                referenced_ledger_ids: vec![],
            };

            let result = NarrationVerifier::default().verify(&ledger, &submission);

            assert!(!result.accepted, "{:?}", result.findings);
            assert_eq!(
                result.next_required_action,
                Some(VerifierNextAction::ReviseText)
            );
            assert!(
                result
                    .findings
                    .iter()
                    .any(|f| f.kind == VerifierFindingKind::PlayerAgencyViolation),
                "{:?}",
                result.findings
            );
        }
    }

    #[test]
    fn verifier_fallback_rejects_actual_equipment_destruction() {
        let ledger = TurnLedgerSnapshot::default();
        let submission = FinalNarrationSubmission {
            player_visible_text: "你已经破坏了设备，监控屏跟着熄灭。".into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(!result.accepted, "{:?}", result.findings);
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.kind == VerifierFindingKind::InventedEffect),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_fallback_ignores_connector_latch_homograph() {
        // `连接扣` contains the character `扣`, but it is a physical latch, not a
        // resource deduction such as "扣 2 HP".
        let ledger = TurnLedgerSnapshot::default();
        let submission = FinalNarrationSubmission {
            player_visible_text: "2. **直接开枪**：既然连接扣已经基本废了，就改打别的暴露部位。"
                .into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(result.accepted, "{:?}", result.findings);
        assert!(
            result
                .findings
                .iter()
                .all(|f| f.kind != VerifierFindingKind::InventedEffect),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_fallback_accepts_quoted_prior_check_world_fact() {
        // A true world fact that literally records an earlier successful check
        // may be quoted in a later recap. That quote is not a new current-turn
        // request to propose/execute a check.
        let ledger = TurnLedgerSnapshot {
            committed_world_facts: vec![CommittedWorldFactEvidence {
                fact_id: "fact_prior_check".into(),
                summary:
                    "The Cut power to server cabling by finding and hitting the main breaker/emergency stop check succeeds."
                        .into(),
                truth_status: Some("true".into()),
            }],
            ..Default::default()
        };
        let submission = FinalNarrationSubmission {
            player_visible_text:
                "已提交事实包括 `The Cut power to server cabling by finding and hitting the main breaker/emergency stop check succeeds.`"
                    .into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(result.accepted, "{:?}", result.findings);
        assert!(
            result
                .findings
                .iter()
                .all(|f| f.kind != VerifierFindingKind::MissingCheck),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_fallback_rejects_fresh_check_despite_unrelated_prior_check_fact() {
        let ledger = TurnLedgerSnapshot {
            committed_world_facts: vec![CommittedWorldFactEvidence {
                fact_id: "fact_prior_check".into(),
                summary: "The Perception to read the door sign check succeeds.".into(),
                truth_status: Some("true".into()),
            }],
            ..Default::default()
        };
        let submission = FinalNarrationSubmission {
            player_visible_text: "这里需要一次新的潜行检定。".into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(!result.accepted, "{:?}", result.findings);
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.kind == VerifierFindingKind::MissingCheck),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_rejects_opposed_label_for_static_dv_check() {
        let ledger = TurnLedgerSnapshot {
            check_contracts: vec![sample_check("check_observe", "观察线缆")],
            check_results: vec![sample_result(
                "check_observe",
                RollVisibility::PublicGmRoll,
                "1d10",
                json!({"target": 14, "success": true, "degree": "success"}),
            )],
            ..Default::default()
        };
        let submission = FinalNarrationSubmission {
            player_visible_text:
                "[roll]观察线缆：1d10=2；总计 16，对抗 DV14，成功。[/roll]你看清了线缆走向。".into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(!result.accepted, "{:?}", result.findings);
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.kind == VerifierFindingKind::InventedEffect),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_rejects_opposed_label_when_static_target_is_only_in_result() {
        let ledger = TurnLedgerSnapshot {
            check_results: vec![sample_result(
                "check_observe",
                RollVisibility::PublicGmRoll,
                "1d10",
                json!({
                    "target": 14,
                    "success": true,
                    "degree": "success",
                    "opposition_kind": "static_check",
                    "resolution_model": {
                        "kind": "static_target_number",
                        "label": "source-backed module technical option DV",
                        "value": 14
                    }
                }),
            )],
            ..Default::default()
        };
        let submission = FinalNarrationSubmission {
            player_visible_text:
                "[roll]侦察扫描：1d10=7；总计 21，对抗 DV 14，成功。[/roll]你看清了线缆。".into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(!result.accepted, "{:?}", result.findings);
        assert!(
            result.findings.iter().any(|f| {
                f.kind == VerifierFindingKind::InventedEffect && f.detail.contains("static DV/DC")
            }),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_allows_trailing_chinese_what_do_you_do_prompt() {
        let submission = FinalNarrationSubmission {
            player_visible_text: "你看清了仓库门前的开阔带。接下来你要怎么做？".into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result =
            NarrationVerifier::default().verify(&TurnLedgerSnapshot::default(), &submission);

        assert!(
            !result
                .findings
                .iter()
                .any(|f| f.kind == VerifierFindingKind::PlayerAgencyViolation),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_allows_trailing_chinese_do_what_prompt() {
        let submission = FinalNarrationSubmission {
            player_visible_text: "你把车停在加油站外缘，退路还在。接下来你要做什么？".into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result =
            NarrationVerifier::default().verify(&TurnLedgerSnapshot::default(), &submission);

        assert!(
            !result
                .findings
                .iter()
                .any(|f| f.kind == VerifierFindingKind::PlayerAgencyViolation),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_rejects_prepared_action_menu_prompt() {
        let submission = FinalNarrationSubmission {
            player_visible_text: "油开始往车里灌。接下来你是准备继续坐在车里指挥他们加油，还是有人下车去开油箱、买水，或者再追问晚上别在路上的事？"
                .into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result =
            NarrationVerifier::default().verify(&TurnLedgerSnapshot::default(), &submission);

        assert!(!result.accepted, "{:?}", result.findings);
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.kind == VerifierFindingKind::PlayerAgencyViolation),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_allows_trailing_you_next_step_prompt() {
        let submission = FinalNarrationSubmission {
            player_visible_text: "无人机仍在扫街，线缆通进仓库。你下一步要怎么做？".into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result =
            NarrationVerifier::default().verify(&TurnLedgerSnapshot::default(), &submission);

        assert!(
            !result
                .findings
                .iter()
                .any(|f| f.kind == VerifierFindingKind::PlayerAgencyViolation),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_allows_trailing_how_to_take_next_step_prompt() {
        let submission = FinalNarrationSubmission {
            player_visible_text:
                "仓库门口仍在火线边缘，线缆钻进里面的设备区。眼下你要怎么接下一步？".into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result =
            NarrationVerifier::default().verify(&TurnLedgerSnapshot::default(), &submission);

        assert!(
            !result
                .findings
                .iter()
                .any(|f| f.kind == VerifierFindingKind::PlayerAgencyViolation),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_rejects_english_next_move_could_be_menu() {
        let submission = FinalNarrationSubmission {
            player_visible_text: concat!(
                "You make the doorframe's blind side. ",
                "From here, your next move could be to peek deeper into the warehouse, ",
                "go for the server side, try to help the cops, or make a play on the cable."
            )
            .into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result =
            NarrationVerifier::default().verify(&TurnLedgerSnapshot::default(), &submission);

        assert!(!result.accepted, "{:?}", result.findings);
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.kind == VerifierFindingKind::PlayerAgencyViolation),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_rejects_inline_either_or_action_branches() {
        let submission = FinalNarrationSubmission {
            player_visible_text: concat!(
                "最危险的是仓库门前那段开阔带。",
                "要么继续借掩体贴过去，要么想办法先让那台 drone 转火、断电，或者失去行动能力。"
            )
            .into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result =
            NarrationVerifier::default().verify(&TurnLedgerSnapshot::default(), &submission);

        assert!(!result.accepted, "{:?}", result.findings);
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.kind == VerifierFindingKind::PlayerAgencyViolation),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_rejects_single_sentence_next_step_action_menu() {
        let submission = FinalNarrationSubmission {
            player_visible_text:
                "从这里，你下一步可以更近地观察门内、扑向线缆、冲进仓库，或者继续贴墙等待更好的时机。"
                    .into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result =
            NarrationVerifier::default().verify(&TurnLedgerSnapshot::default(), &submission);

        assert!(!result.accepted, "{:?}", result.findings);
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.kind == VerifierFindingKind::PlayerAgencyViolation),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_rejects_blocked_movement_check_narrated_as_arrival() {
        let ledger = TurnLedgerSnapshot {
            check_results: vec![sample_result(
                "check_move",
                RollVisibility::PublicGmRoll,
                "1d10",
                json!({
                    "blocked": true,
                    "success": null,
                    "target": null,
                    "reason": "missing_source_backed_parameters",
                    "check_label": "Time the dash through cover to the warehouse door blind spot"
                }),
            )],
            ..Default::default()
        };
        let submission = FinalNarrationSubmission {
            player_visible_text: concat!(
                "现在你已经不在警车后了。",
                "你人稳在仓库门框旁的盲区里，左侧是被打得坑坑洼洼的墙。"
            )
            .into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(!result.accepted, "{:?}", result.findings);
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.kind == VerifierFindingKind::InventedEffect),
            "{:?}",
            result.findings
        );
    }

    #[test]
    fn verifier_rejects_blocked_movement_check_narrated_as_tucked_into_new_cover() {
        let ledger = TurnLedgerSnapshot {
            check_results: vec![sample_result(
                "check_move",
                RollVisibility::PublicGmRoll,
                "1d10",
                json!({
                    "blocked": true,
                    "success": null,
                    "target": null,
                    "reason": "missing_source_backed_parameters",
                    "check_label": "Time the dash through cover to the warehouse door blind spot"
                }),
            )],
            ..Default::default()
        };
        let submission = FinalNarrationSubmission {
            player_visible_text: concat!(
                "你没有去碰旁边的线缆，只把身体收进仓库门框旁那块刚看出来的死角里，",
                "肩背贴住门框与墙边的硬面，先把自己稳稳藏进掩护。",
                "此刻，你已经伏在门框边的阴影里。"
            )
            .into(),
            mechanical_claims: vec![],
            referenced_ledger_ids: vec![],
        };

        let result = NarrationVerifier::default().verify(&ledger, &submission);

        assert!(!result.accepted, "{:?}", result.findings);
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.kind == VerifierFindingKind::InventedEffect),
            "{:?}",
            result.findings
        );
    }

    fn sample_check(check_id: &str, label: &str) -> CheckContract {
        CheckContract {
            check_id: check_id.into(),
            session_id: "session_test".into(),
            turn_id: "turn_test".into(),
            ruleset_id: "ruleset_test".into(),
            module_id: None,
            initiator: ActorRef {
                actor_id: "pc.current".into(),
                actor_kind: ActorKind::PlayerCharacter,
                display_name: Some("PC".into()),
            },
            target_actor: None,
            opposition: OppositionModel::NoMechanicalOpposition,
            action_summary: label.into(),
            intent_kind: "attack".into(),
            check_label: label.into(),
            dice_expression: "1d10+12".into(),
            modifiers: vec![],
            target: CheckTargetModel::StaticNumber {
                value: 15,
                label: "DV".into(),
            },
            tested_parameter: None,
            opponent_tested_parameter: None,
            actor_snapshot_ids: vec![],
            source_refs: vec![],
            learned_packet_ids: vec![],
            roll_visibility: RollVisibility::PublicGmRoll,
            roll_authority: RollAuthority::System,
            disclosure: RollDisclosurePolicy::for_visibility(RollVisibility::PublicGmRoll),
            stakes: CheckStakes {
                before_roll_public: "有风险。".into(),
                success_public: "成功。".into(),
                failure_public: "失败。".into(),
                critical_public: None,
                fumble_public: None,
                success_patches_allowed: vec![],
                failure_patches_allowed: vec![],
                irreversible: false,
            },
            confidence: RulingConfidence::High,
            ruling_status: RulingStatus::SourceBacked,
            advice_refs: vec![],
            expires_at_turn: Some("turn_test".into()),
        }
    }

    fn sample_result(
        check_id: &str,
        visibility: RollVisibility,
        expression: &str,
        outcome: serde_json::Value,
    ) -> CheckResultRecord {
        CheckResultRecord {
            check_id: check_id.into(),
            roll: DiceRollRecord {
                roll_id: format!("roll_{check_id}"),
                session_id: "session_test".into(),
                turn_id: "turn_test".into(),
                check_id: Some(check_id.into()),
                roller_kind: ActorKind::PlayerCharacter,
                roller_id: Some("pc.current".into()),
                visibility,
                expression: expression.into(),
                result: json!({"total": 18}),
                seed_commitment: "seed".into(),
                revealed_at: Some(Utc::now()),
                created_at: Utc::now(),
            },
            outcome,
            committed_patches: vec![],
            created_at: Utc::now(),
        }
    }

    fn sample_effect(effect_id: &str, target_actor_id: &str) -> EffectContract {
        EffectContract {
            effect_id: effect_id.into(),
            target_actor_ids: vec![target_actor_id.into()],
            visibility: Visibility::PlayerVisible,
            ..Default::default()
        }
    }

    fn sample_impact(
        impact_id: &str,
        target_id: &str,
        parameter_path: &str,
        amount: i64,
    ) -> ParameterImpact {
        ParameterImpact {
            impact_id: impact_id.into(),
            target_kind: EffectTargetKind::Actor,
            target_id: target_id.into(),
            parameter_path: parameter_path.into(),
            operation: ParameterOperation::Add,
            value: json!(amount),
            visibility: Visibility::PlayerVisible,
            validator_status: "validated".into(),
            ..Default::default()
        }
    }
}

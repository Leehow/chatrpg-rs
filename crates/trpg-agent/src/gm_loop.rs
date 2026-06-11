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
}

impl TurnLedgerSnapshot {
    pub fn has_check_fact(&self) -> bool {
        !self.check_contracts.is_empty()
            || !self.check_results.is_empty()
            || self.has_open_player_roll_gate()
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
                collect_json_tokens(&serde_json::to_value(patch).unwrap_or(Value::Null), &mut tokens);
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
                collect_json_tokens(&serde_json::to_value(patch).unwrap_or(Value::Null), &mut tokens);
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
        let text = normalize_text(&submission.player_visible_text);
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
                    if !ledger.has_effect_evidence() {
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
        } else if submission.mechanical_claims.is_empty() {
            // 引用为空 → 回退子串扫描（可观测技术债：detail 带标注前缀，不静默）。
            if text_says_check_or_roll(&text) && !ledger.has_check_fact() {
                findings.push(VerifierFinding::blocker(
                    VerifierFindingKind::MissingCheck,
                    "fallback:substring_scan: final narration mentions a check/roll, but no ledger check fact exists",
                ));
            }
            if text_says_effect(&text) && !ledger.has_effect_evidence() {
                findings.push(VerifierFinding::blocker(
                    VerifierFindingKind::InventedEffect,
                    "fallback:substring_scan: final narration mentions a mechanical effect, but no ledger effect evidence exists",
                ));
            }
        }

        if ledger.has_visible_player_result() && !text_contains_any(&text, &ledger.visible_evidence_tokens()) {
            findings.push(VerifierFinding::blocker(
                VerifierFindingKind::OmittedVisibleResult,
                "player-visible roll/effect evidence exists in the ledger, but final narration does not mention any visible ledger token",
            ));
        }

        if text_asks_player_for_manual_roll(&text) && table_policy_system_rolls_visible() {
            findings.push(VerifierFinding::blocker(
                VerifierFindingKind::ManualRollRequest,
                "final narration asks the player to provide dice totals under system-rolls-visible policy",
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
        .any(|f| f.kind == VerifierFindingKind::MissingCheck)
    {
        Some(VerifierNextAction::CallProposeCheck)
    } else if findings
        .iter()
        .any(|f| f.kind == VerifierFindingKind::MissingRollExecution)
    {
        Some(VerifierNextAction::CallExecuteCheck)
    } else if findings
        .iter()
        .any(|f| matches!(f.kind, VerifierFindingKind::MissingEffect | VerifierFindingKind::InventedEffect))
    {
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

fn text_says_check_or_roll(text: &str) -> bool {
    [
        "检定", "掷骰", "骰", "投掷", "roll", "check", "test", "saving throw", "difficulty", "dv",
    ]
    .iter()
    .any(|term| text.contains(term))
}

fn text_says_effect(text: &str) -> bool {
    [
        "hp", "damage", "伤害", "损失", "失去", "扣", "san", "sanity", "理智", "resource", "资源",
        "condition", "状态", "clock", "进度", "破坏", "摧毁",
    ]
    .iter()
    .any(|term| text.contains(term))
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

fn table_policy_system_rolls_visible() -> bool {
    let value = std::env::var("TRPG_AGENT_TABLE_DICE_POLICY")
        .unwrap_or_else(|_| "system_rolls_visible".into())
        .to_ascii_lowercase();
    matches!(
        value.as_str(),
        "system_rolls_visible" | "system" | "gm_rolls_visible" | "auto" | "auto_visible"
    )
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
            player_visible_text: "这里需要一次潜行检定。请只回复 `roll`，系统会调用骰子工具并写入结果。"
                .into(),
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
            result.findings.iter().any(|f| f.kind == VerifierFindingKind::InventedEffect
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
            result.findings.iter().any(|f| f.kind == VerifierFindingKind::InventedEffect
                && f.detail.starts_with("fallback:substring_scan: ")),
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

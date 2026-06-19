use serde_json::Value;
use std::collections::BTreeSet;
use trpg_agent::TurnLedgerSnapshot;
use trpg_model::{
    CheckContract, CheckResultRecord, EffectContract, InteractionGate, ParameterImpact,
    RollVisibility,
};
use trpg_runtime::AutoRollExecution;

/// 回合账本：所有工具产物经此入账（spec §4），供流后 NarrationVerifier 对账
/// 与私骰 token 集合提取。直接复用 trpg_agent::TurnLedgerSnapshot 作为存储。
#[derive(Debug, Default)]
pub struct TurnLedger {
    snapshot: TurnLedgerSnapshot,
}

impl TurnLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_contract(&mut self, contract: &CheckContract) {
        self.snapshot.check_contracts.push(contract.clone());
    }

    /// 入账一次检定结果（result.roll 同步进 dice_rolls）。
    pub fn record_result(&mut self, result: &CheckResultRecord) {
        self.snapshot.dice_rolls.push(result.roll.clone());
        self.snapshot.check_results.push(result.clone());
    }

    /// 入账 execute_system_roll_bundle 产物：primary + 全部 followups。
    pub fn record_execution(&mut self, exec: &AutoRollExecution) {
        self.record_result(&exec.primary);
        for result in &exec.followups {
            self.record_result(result);
        }
    }

    pub fn record_effect(&mut self, effect: &EffectContract) {
        self.snapshot.effect_contracts.push(effect.clone());
    }

    pub fn record_impact(&mut self, impact: &ParameterImpact) {
        self.snapshot.parameter_impacts.push(impact.clone());
    }

    pub fn record_gate(&mut self, gate: &InteractionGate) {
        self.snapshot.interaction_gates.push(gate.clone());
    }

    pub fn snapshot(&self) -> &TurnLedgerSnapshot {
        &self.snapshot
    }

    /// 滑动缓冲过滤集合（收窄版，lowercase 去重）：仅 roll_id 与原始骰面点数
    /// （roll.result JSON 中的数值 token）。不再逐字复用
    /// private_roll_leak_tokens——其含通用 JSON 键名（"total"/"success"/"band"）
    /// 与骰式表达式（"1d100"），子串替换会把叙事 "successfully" 涂成 "■fully"。
    /// 泄密方向仍 fail-closed：私骰点数与 roll_id 全过滤；短数字误伤由
    /// RedactingBuffer 的数字词边界匹配兜住。
    ///
    /// Deliberately do NOT collect `outcome` numbers: target values, degrees,
    /// thresholds, HP deltas, and other adjudication numbers can coincide with
    /// public roll text (`目标 40`) and must not black-box unrelated public facts.
    pub fn private_roll_tokens(&self) -> Vec<String> {
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        let mut push = |token: String| {
            let lower = token.to_ascii_lowercase();
            if !lower.trim().is_empty() && seen.insert(lower.clone()) {
                out.push(lower);
            }
        };
        for result in &self.snapshot.check_results {
            if result.roll.visibility != RollVisibility::PrivateGmRoll {
                continue;
            }
            push(result.roll.roll_id.clone());
            collect_numeric_tokens(&result.roll.result, &mut push);
        }
        for roll in &self.snapshot.dice_rolls {
            if roll.visibility != RollVisibility::PrivateGmRoll {
                continue;
            }
            push(roll.roll_id.clone());
            collect_numeric_tokens(&roll.result, &mut push);
        }
        out
    }
}

/// 递归收集 JSON 里的数值 token（仅 Number；键名/字符串/布尔一律不进过滤集）。
fn collect_numeric_tokens(value: &Value, sink: &mut impl FnMut(String)) {
    match value {
        Value::Number(n) => sink(n.to_string()),
        Value::Array(items) => {
            for item in items {
                collect_numeric_tokens(item, sink);
            }
        }
        Value::Object(map) => {
            for item in map.values() {
                collect_numeric_tokens(item, sink);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;
    use trpg_model::{ActorKind, CheckResultRecord, DiceRollRecord, RollVisibility};
    use trpg_runtime::AutoRollExecution;

    fn result(check_id: &str, total: i64, visibility: RollVisibility) -> CheckResultRecord {
        CheckResultRecord {
            check_id: check_id.to_string(),
            roll: DiceRollRecord {
                roll_id: format!("roll_{check_id}"),
                session_id: "s".to_string(),
                turn_id: "t".to_string(),
                check_id: Some(check_id.to_string()),
                roller_kind: ActorKind::PlayerCharacter,
                roller_id: Some("pc.current".to_string()),
                visibility,
                expression: "1d100".to_string(),
                result: json!({"total": total}),
                seed_commitment: "seed".to_string(),
                revealed_at: None,
                created_at: Utc::now(),
            },
            outcome: json!({"success": total < 50}),
            committed_patches: vec![],
            created_at: Utc::now(),
        }
    }

    #[test]
    fn record_execution_records_primary_followups_and_rolls() {
        let exec = AutoRollExecution {
            primary: result("a", 12, RollVisibility::PublicGmRoll),
            followups: vec![result("b", 34, RollVisibility::PrivateGmRoll)],
            roll_policy: "system_rolls_visible".to_string(),
        };
        let mut ledger = TurnLedger::new();
        ledger.record_execution(&exec);
        assert_eq!(ledger.snapshot().check_results.len(), 2);
        assert_eq!(ledger.snapshot().dice_rolls.len(), 2);
        // 收窄版过滤集（终审 important）：仅 PrivateGmRoll 的 roll_id + 原始点数
        // （roll.result JSON 数值；outcome "success" 是 Bool 不产 token）。
        // 通用 JSON 键名（"total"/"success"/"band"）与骰式表达式（"1d100"）
        // 一律剔除——它们是叙事高频词，子串替换会把 "successfully" 涂成 "■fully"。
        assert_eq!(
            ledger.private_roll_tokens(),
            vec!["roll_b".to_string(), "34".to_string()]
        );
        assert!(!ledger
            .private_roll_tokens()
            .iter()
            .any(|t| t == "total" || t == "success" || t == "1d100" || t == "band"));
        // public roll 的 token 绝不进集合（"roll_a"/"12" 不出现）。
        assert!(!ledger
            .private_roll_tokens()
            .iter()
            .any(|t| t == "roll_a" || t == "12"));
    }

    #[test]
    fn private_roll_redaction_does_not_collect_outcome_target_numbers() {
        let mut r = result("secret", 91, RollVisibility::PrivateGmRoll);
        r.outcome = json!({"success": false, "target": 40, "degree": "failure"});
        let mut ledger = TurnLedger::new();
        ledger.record_result(&r);

        let tokens = ledger.private_roll_tokens();
        assert!(tokens.iter().any(|t| t == "91"));
        assert!(tokens.iter().any(|t| t == "roll_secret"));
        assert!(
            !tokens.iter().any(|t| t == "40"),
            "outcome target 40 must not redact public roll text like `目标 40`: {tokens:?}"
        );
    }
}

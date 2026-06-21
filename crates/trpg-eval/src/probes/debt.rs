//! UNRESOLVED_MECHANICAL_DEBT (蓝图 §五.3 机械债务审计).
//!
//! The decisive signal the blueprint calls out (cyber turn 70): the GM marks a
//! check `待结算` yet the turn's own health-check line claims `✅无未定[roll]`.
//! A debt that the report itself classifies as "resolved" is the worst case.

use super::snippet;
use crate::model::{EvalFinding, RootCause, Severity, Transcript};

pub fn unresolved_mechanical_debt(t: &Transcript) -> Vec<EvalFinding> {
    let mut lying_ids = Vec::new();
    let mut evidence = Vec::new();
    let mut debt_turns = 0usize;
    for turn in &t.turns {
        if turn.debt_lines.is_empty() {
            continue;
        }
        debt_turns += 1;
        if turn.claims_no_pending() {
            lying_ids.push(turn.index);
            if evidence.len() < 4 {
                let debt = turn.debt_lines.first().map(|d| snippet(d, 40)).unwrap_or_default();
                evidence.push(format!(
                    "turn {}：留下「{}」但体检自称无未定[roll]",
                    turn.index, debt
                ));
            }
        }
    }
    if lying_ids.is_empty() {
        return vec![];
    }
    vec![EvalFinding::new(
        RootCause::UnresolvedMechanicalDebt,
        Severity::Hard,
        lying_ids.clone(),
        "创建债务必须带 continuation_token 并在下一合法阶段结清；会话末债务为零",
        format!(
            "{} 个回合留下待结算债务却被体检判为已解决(全程 {} 个负债回合)",
            lying_ids.len(),
            debt_turns
        ),
        evidence,
    )]
}

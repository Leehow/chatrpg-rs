//! SEMANTIC_NOOP + SUCCESS_WITHOUT_INFORMATION (蓝图 §五.4 语义空壳).
//! RESPONSE_INTENT_MISMATCH moved to field-level in `crate::contract` (V2-P2).

use super::{contains_any, snippet, NO_INFO_MARKERS};
use crate::model::{EvalFinding, RootCause, Severity, Transcript};

const MIN_PROSE_CHARS: usize = 40;

/// Long player-visible prose that resolves nothing and carries a no-info marker.
pub fn semantic_noop(t: &Transcript) -> Vec<EvalFinding> {
    let mut ids = Vec::new();
    let mut evidence = Vec::new();
    for turn in &t.turns {
        if turn.visible_text().chars().count() < MIN_PROSE_CHARS {
            continue;
        }
        // A resolved success this turn means it is not a no-op (it may instead be a
        // success-without-information case, handled separately).
        if turn.has_success_roll() {
            continue;
        }
        if let Some(marker) = contains_any(&turn.visible_text(), NO_INFO_MARKERS) {
            ids.push(turn.index);
            if evidence.len() < 4 {
                evidence.push(format!("turn {}「{}」", turn.index, marker));
            }
        }
    }
    if ids.is_empty() {
        return vec![];
    }
    vec![EvalFinding::new(
        RootCause::SemanticNoop,
        Severity::High,
        ids.clone(),
        "成功/失败应带来新事实、状态变化或新选择",
        format!("{} 个回合大段散文但无新信息/状态/后果", ids.len()),
        evidence,
    )]
}

/// A check succeeds but the player-visible result still yields no concrete info.
pub fn success_without_information(t: &Transcript) -> Vec<EvalFinding> {
    let mut ids = Vec::new();
    let mut evidence = Vec::new();
    for turn in &t.turns {
        if !turn.has_success_roll() {
            continue;
        }
        if let Some(marker) = contains_any(&turn.visible_text(), NO_INFO_MARKERS) {
            ids.push(turn.index);
            if evidence.len() < 4 {
                let roll = turn.roll_lines.first().map(|r| snippet(r, 30)).unwrap_or_default();
                evidence.push(format!("turn {} 成功[{}] 却「{}」", turn.index, roll, marker));
            }
        }
    }
    if ids.is_empty() {
        return vec![];
    }
    vec![EvalFinding::new(
        RootCause::SuccessWithoutInformation,
        Severity::Hard,
        ids.clone(),
        "成功检定应给出与成功等级相称的具体信息",
        format!("{} 个成功检定没有产出任何具体信息", ids.len()),
        evidence,
    )]
}

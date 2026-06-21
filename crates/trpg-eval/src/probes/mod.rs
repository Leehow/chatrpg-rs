//! Deterministic root-cause probes over a parsed [`Transcript`] (蓝图 §五/§六).
//! Each probe emits evidenced [`EvalFinding`]s; thresholds are env-overridable so
//! the verdict is demonstrably metric-driven, not a hardcoded RED.

mod debt;
mod repetition;
mod semantic;

use crate::model::{EvalFinding, Transcript};

/// Phrases that signal "long prose, but no new fact / no answer" (蓝图 §四/§五 语义空壳).
pub(crate) const NO_INFO_MARKERS: &[&str] = &[
    "没有别的变化",
    "尚未给出任何明白的回答",
    "局面没有变化",
    "没有哪一样东西",
    "没有哪条线索",
    "还没开口",
    "没有任何回应",
    "并没有真的",
    "什么都没能真正抓住",
    "没能真正抓住",
    "尚未给出任何",
    "没有别的",
    "确实在隐瞒什么",
    "没有立刻把答案",
];

/// Player actions that request information / a concrete resolution (蓝图 §四 intent).
pub(crate) const QUESTION_INTENT_MARKERS: &[&str] = &[
    "逼问", "盘问", "质问", "交代", "是谁", "核对", "打听", "询问", "确认", "问他", "问清",
    "数清", "评估", "看清", "分辨", "判断",
];

/// Env override helpers — defaults chosen to separate the bad fixtures from the
/// clean control; overriding them flips findings, proving metric-driven behavior.
pub(crate) fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

pub(crate) fn contains_any(haystack: &str, needles: &[&str]) -> Option<String> {
    needles
        .iter()
        .find(|n| haystack.contains(*n))
        .map(|n| n.to_string())
}

/// Establishing-entity fingerprint: the set of CJK 3-grams of a scene opening
/// (whitespace/ASCII filtered). A reset re-uses turn-1's entity set; a paraphrase
/// has near-zero raw char overlap yet shares these entity grams.
pub(crate) fn entity_grams(s: &str) -> std::collections::HashSet<String> {
    let chars: Vec<char> = s
        .chars()
        .filter(|c| !c.is_whitespace() && !c.is_ascii())
        .collect();
    chars.windows(3).map(|w| w.iter().collect::<String>()).collect()
}

/// Snippet for evidence quotes — first `n` chars, ellipsized.
pub(crate) fn snippet(s: &str, n: usize) -> String {
    let trimmed: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        format!("{trimmed}…")
    } else {
        trimmed
    }
}

/// Run every probe and collect findings.
pub fn run_all(t: &Transcript) -> Vec<EvalFinding> {
    let mut out = Vec::new();
    out.extend(repetition::player_action_loop(t));
    out.extend(repetition::scene_reset(t));
    out.extend(semantic::semantic_noop(t));
    out.extend(semantic::success_without_information(t));
    out.extend(semantic::response_intent_mismatch(t));
    out.extend(debt::unresolved_mechanical_debt(t));
    out
}


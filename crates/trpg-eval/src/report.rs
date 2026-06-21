//! Evidenced red-board (蓝图 §八 末尾示例的格式) + 软加权质量分 (蓝图 §七).

use crate::contract::contracts;
use crate::model::{Severity, Transcript, Verdict};
use crate::score::score_card;

pub fn redboard(t: &Transcript, v: &Verdict) -> String {
    let mut s = String::new();
    s.push_str(&format!("# EVAL {} · {}\n\n", v.label(), t.title));
    s.push_str(&format!(
        "turns={}  findings={}  root_causes={}\n\n",
        t.turns.len(),
        v.findings.len(),
        v.root_causes().len()
    ));
    if v.findings.is_empty() {
        s.push_str("无阻断性发现。\n\n");
        s.push_str(&score_section(v));
        return s;
    }
    for f in &v.findings {
        let sev = match f.severity {
            Severity::Hard => "S1·硬门槛",
            Severity::High => "S2·质量",
            Severity::Info => "S3·提示",
        };
        s.push_str(&format!(
            "## [{}] {} ({})\n",
            f.cause.id(),
            sev,
            f.root_layer
        ));
        s.push_str(&format!("  turns: {:?}\n", f.turn_ids));
        s.push_str(&format!("  expected: {}\n", f.expected));
        s.push_str(&format!("  actual:   {}\n", f.actual));
        for e in &f.evidence {
            s.push_str(&format!("  evidence: {e}\n"));
        }
        s.push('\n');
    }
    s.push_str(&contract_section(t));
    s.push_str(&score_section(v));
    s
}

/// Response Contract field-level audit (蓝图 §四, V2-P2). Lists every action that
/// requested information and which fields the GM left unanswered.
fn contract_section(t: &Transcript) -> String {
    let cs = contracts(t);
    if cs.is_empty() {
        return String::new();
    }
    let asked: usize = cs.iter().map(|c| c.requests.len()).sum();
    let unanswered: usize = cs.iter().map(|c| c.unanswered().count()).sum();
    let mut s = String::new();
    s.push_str(&format!(
        "\n## Response Contract (蓝图 §四 逐字段): {} 回合提请求, {} 字段被请求, {} 未获答复\n",
        cs.len(),
        asked,
        unanswered
    ));
    for c in cs.iter().filter(|c| c.unanswered().count() > 0) {
        let fields = c
            .unanswered()
            .map(|r| format!("{}「{}」", r.field.id(), r.marker))
            .collect::<Vec<_>>()
            .join(" + ");
        s.push_str(&format!("  turn {} 未答: {}\n", c.turn, fields));
    }
    s
}

/// 软加权质量分 (蓝图 §七): 0-100, language weight lowest, with the un-probed
/// language band reported as uncertainty so the number is never overclaimed.
fn score_section(v: &Verdict) -> String {
    let card = score_card(v);
    let mut s = String::new();
    s.push_str(&format!(
        "## 软加权质量分 (蓝图 §七): {:.0}/100  ±{:.0} 不确定\n",
        card.total, card.uncertainty
    ));
    for d in &card.dimensions {
        let note = if d.assessed { "" } else { "  (未确定性评估)" };
        s.push_str(&format!(
            "  {:<8} {:>4.0}/{:<2.0}  ({} 项发现){}\n",
            d.dimension.label(),
            d.earned,
            d.weight,
            d.finding_count,
            note
        ));
    }
    s
}

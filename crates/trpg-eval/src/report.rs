//! Evidenced red-board (蓝图 §八 末尾示例的格式).

use crate::model::{Severity, Transcript, Verdict};

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
        s.push_str("无阻断性发现。\n");
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
    s
}

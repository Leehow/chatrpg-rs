use crate::model::{EvalReport, Verdict};

pub fn render_markdown_report(report: &EvalReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("VERDICT: {}\n", verdict_label(report.verdict)));
    out.push_str(&format!("fixture: {}\n", report.fixture_id));
    out.push_str(&format!("score: {}\n", report.score));
    if !report.rubric_scores.is_empty() {
        out.push_str("\n| dimension | score | weight | evidence |\n");
        out.push_str("|-----------|-------|--------|----------|\n");
        for rubric in &report.rubric_scores {
            out.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                rubric.label,
                rubric.score,
                rubric.weight,
                rubric.evidence.join("; ")
            ));
        }
    }
    for finding in &report.findings {
        out.push('\n');
        out.push_str(&format!(
            "{} {} {}\n",
            severity_label(finding.severity),
            finding.category.as_str(),
            finding.finding_id
        ));
        out.push_str(&format!("turns: {:?}\n", finding.turns));
        out.push_str(&format!("expected: {}\n", finding.expected));
        out.push_str(&format!("actual: {}\n", finding.actual));
        out.push_str(&format!("root_layer: {}\n", finding.root_layer));
        out.push_str(&format!("confidence: {:.2}\n", finding.confidence));
        for evidence in &finding.evidence {
            out.push_str(&format!("- evidence: {evidence}\n"));
        }
    }
    out
}

fn verdict_label(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Pass => "PASS",
        Verdict::Warn => "WARN",
        Verdict::Fail => "FAIL",
    }
}

fn severity_label(severity: crate::model::FindingSeverity) -> &'static str {
    match severity {
        crate::model::FindingSeverity::S1 => "S1",
        crate::model::FindingSeverity::S2 => "S2",
        crate::model::FindingSeverity::S3 => "S3",
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        render_markdown_report, EvalFinding, EvalReport, FindingCategory, FindingSeverity,
        RubricDimension, RubricScore, Verdict,
    };

    #[test]
    fn markdown_report_contains_evidence_fields() {
        let report = EvalReport {
            fixture_id: "fixture".into(),
            title: "Fixture".into(),
            verdict: Verdict::Fail,
            score: 0,
            rubric_scores: vec![RubricScore {
                dimension: RubricDimension::Responsiveness,
                label: "对玩家行动的响应性".into(),
                weight: 20,
                score: 0,
                evidence: vec!["S1 RESPONSE_INTENT_MISMATCH".into()],
            }],
            findings: vec![EvalFinding {
                finding_id: "F-00001".into(),
                category: FindingCategory::ResponseIntentMismatch,
                severity: FindingSeverity::S1,
                turns: vec![5],
                root_layer: "Narrator / Director".into(),
                expected: "specific answer".into(),
                actual: "generic answer".into(),
                evidence: vec!["player asked a concrete question".into()],
                confidence: 0.91,
            }],
        };
        let md = render_markdown_report(&report);
        assert!(md.starts_with("VERDICT: FAIL"));
        assert!(md.contains("RESPONSE_INTENT_MISMATCH"));
        assert!(md.contains("expected: specific answer"));
        assert!(md.contains("actual: generic answer"));
        assert!(md.contains("root_layer: Narrator / Director"));
        assert!(md.contains("confidence: 0.91"));
        assert!(md.contains("player asked a concrete question"));
        assert!(md.contains("| 对玩家行动的响应性 | 0 | 20 |"));
    }
}

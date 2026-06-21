//! Layered attribution (蓝图 §九/§十 第三阶段 = 验收3).
//!
//! Reads the ACTUAL per-turn contribution trail and attributes each defect to
//! the runtime layer that produced it — PLAYER_POLICY / INTENT / RULES / WORLD /
//! DIRECTOR / NARRATOR / POLICY / REPORTER — so a failing run says *which layer*
//! to fix, not "GM 回复不好".
//!
//! Two trail-driven defect classes (both metric-driven, not hardcoded):
//!   1. MissingLayerContribution — a required stage signal a healthy turn shows
//!      (gm_context view→DIRECTOR, npc_mind view→WORLD, spoiler guard→POLICY) is
//!      absent this turn ⇒ that layer did not contribute.
//!   2. EscapedDefect — a POLICY verifier finding whose summary says the defect
//!      reached player-visible narration ⇒ the NARRATOR produced disallowed text
//!      that POLICY only caught post-hoc.
//!
//! No-fake: present signal ⇒ no finding, absent signal ⇒ finding. The good and
//! failbranch fixtures diverge purely on their recorded trail content.

use crate::flight::record::{Contribution, FlightRecording, FlightTurn};
use crate::model::{Layer, Severity};
use serde::Serialize;

/// Map one contribution to the runtime layer that produced it.
pub fn layer_of(c: &Contribution) -> Layer {
    match c.kind.as_str() {
        "view_load" => {
            if c.summary.starts_with("gm_context") {
                Layer::Director
            } else if c.summary.starts_with("npc_mind") {
                Layer::World
            } else if c.summary.starts_with("player_knowledge") {
                // The knowledge/memory projection surfaced into the world view.
                Layer::World
            } else {
                Layer::Director
            }
        }
        // Guidance injection / context filtering / post-hoc verifier are all the
        // Policy layer's job (设计4补充 §二: Policy filters-rejects-repairs).
        "prompt_block" | "context_filter" | "verifier_finding" => Layer::Policy,
        _ => Layer::Reporter,
    }
}

/// A POLICY verifier finding whose summary indicates the defect actually reached
/// the player-visible narration (escaped), vs a sanitized kind label (caught).
fn verifier_escaped(c: &Contribution) -> bool {
    c.kind == "verifier_finding"
        && (c.summary.contains("narration") || c.summary.contains("into "))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum AttributionKind {
    /// A required stage signal was absent this turn (the layer did not contribute).
    MissingLayerContribution,
    /// A defect reached player-visible output; the producing layer is to blame.
    EscapedDefect,
}

/// One layered finding (蓝图 §八 证据化: expected/actual/turn + evidence).
#[derive(Debug, Clone, Serialize)]
pub struct LayerFinding {
    pub layer: Layer,
    pub kind: AttributionKind,
    pub severity: Severity,
    pub turn: u32,
    pub expected: String,
    pub actual: String,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct LayerVerdict {
    pub turns: usize,
    pub findings: Vec<LayerFinding>,
}

impl LayerVerdict {
    pub fn is_fail(&self) -> bool {
        self.findings.iter().any(|f| f.severity >= Severity::High)
    }
    pub fn label(&self) -> &'static str {
        if self.is_fail() {
            "FAIL"
        } else {
            "PASS"
        }
    }
    /// Distinct layers carrying at least one finding (the "fix these" set).
    pub fn implicated_layers(&self) -> Vec<Layer> {
        let mut out = Vec::new();
        for l in Layer::ALL {
            if self.findings.iter().any(|f| f.layer == l) {
                out.push(l);
            }
        }
        out
    }
}

/// A required stage signal a healthy turn exhibits, and the layer it belongs to.
struct RequiredSignal {
    name: &'static str,
    layer: Layer,
    present: fn(&Contribution) -> bool,
}

const REQUIRED: [RequiredSignal; 3] = [
    RequiredSignal {
        name: "gm_context view_load",
        layer: Layer::Director,
        present: |c| c.kind == "view_load" && c.summary.starts_with("gm_context"),
    },
    RequiredSignal {
        name: "npc_mind_views view_load",
        layer: Layer::World,
        present: |c| c.kind == "view_load" && c.summary.starts_with("npc_mind"),
    },
    RequiredSignal {
        name: "spoiler-guard activity",
        layer: Layer::Policy,
        present: |c| c.plugin_id.contains("spoiler") || c.plugin_id.contains("guard"),
    },
];

fn attribute_turn(t: &FlightTurn, out: &mut Vec<LayerFinding>) {
    for req in REQUIRED.iter() {
        if !t.contributions.iter().any(|c| (req.present)(c)) {
            let seen: Vec<String> = t
                .contributions
                .iter()
                .map(|c| format!("{}/{}/{}", c.plugin_id, c.hook, c.kind))
                .collect();
            out.push(LayerFinding {
                layer: req.layer,
                kind: AttributionKind::MissingLayerContribution,
                severity: Severity::High,
                turn: t.index,
                expected: format!("turn {} 应有 {} ({} 层)", t.index, req.name, req.layer),
                actual: format!("缺失 {}", req.name),
                evidence: vec![format!("本回合贡献: [{}]", seen.join(", "))],
            });
        }
    }
    for c in t.contributions.iter().filter(|c| verifier_escaped(c)) {
        // POLICY caught it, but it had already reached narration ⇒ NARRATOR defect.
        out.push(LayerFinding {
            layer: Layer::Narrator,
            kind: AttributionKind::EscapedDefect,
            severity: Severity::Hard,
            turn: t.index,
            expected: "禁忌内容不得进入玩家可见叙述 (NARRATOR 不写剧透)".into(),
            actual: format!("NARRATOR 输出泄漏, POLICY 仅事后检出: {}", c.summary),
            evidence: vec![format!("verifier_finding[{}]: {}", c.plugin_id, c.summary)],
        });
    }
}

/// Attribute every defect in a recording to its producing layer.
pub fn attribute(rec: &FlightRecording) -> LayerVerdict {
    let mut findings = Vec::new();
    for t in &rec.turns {
        attribute_turn(t, &mut findings);
    }
    LayerVerdict {
        turns: rec.turns.len(),
        findings,
    }
}

/// Per-layer evidenced red-board for the Flight-Recorder arm.
pub fn redboard(rec: &FlightRecording, v: &LayerVerdict) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "# FLIGHT ATTRIBUTION {} (蓝图 §九/§十 第三阶段 = 验收3)\n\n",
        v.label()
    ));
    s.push_str(&format!(
        "turns={}  findings={}  implicated_layers={:?}\n\n",
        v.turns,
        v.findings.len(),
        v.implicated_layers()
            .iter()
            .map(|l| l.id())
            .collect::<Vec<_>>()
    ));
    if v.findings.is_empty() {
        s.push_str("逐层贡献齐全, 无泄漏: 每层都按职责产出 (no missing layer, no escape).\n");
        return s;
    }
    for f in &v.findings {
        let k = match f.kind {
            AttributionKind::MissingLayerContribution => "层缺失贡献",
            AttributionKind::EscapedDefect => "缺陷逃逸到玩家",
        };
        s.push_str(&format!("## [{}] {} (turn {})\n", f.layer.id(), k, f.turn));
        if let Some(t) = rec.turns.iter().find(|t| t.index == f.turn) {
            if !t.user_input.is_empty() {
                s.push_str(&format!("  action:   {}\n", t.user_input));
            }
        }
        s.push_str(&format!("  expected: {}\n", f.expected));
        s.push_str(&format!("  actual:   {}\n", f.actual));
        for e in &f.evidence {
            s.push_str(&format!("  evidence: {e}\n"));
        }
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contrib(plugin: &str, hook: &str, kind: &str, summary: &str) -> Contribution {
        Contribution {
            plugin_id: plugin.into(),
            hook: hook.into(),
            kind: kind.into(),
            summary: summary.into(),
        }
    }

    #[test]
    fn maps_contribution_to_producing_layer() {
        assert_eq!(
            layer_of(&contrib("core.view_loader", "context_assembly", "view_load", "gm_context: 6")),
            Layer::Director
        );
        assert_eq!(
            layer_of(&contrib("core.view_loader", "context_assembly", "view_load", "npc_mind_views: 1")),
            Layer::World
        );
        assert_eq!(
            layer_of(&contrib("core.no_spoiler_guard", "after_llm_stream", "verifier_finding", "secret_leak")),
            Layer::Policy
        );
    }

    #[test]
    fn missing_signal_attributes_to_that_layer_present_does_not() {
        // WITH gm_context ⇒ no Director-missing; WITHOUT ⇒ Director-missing.
        let with = FlightTurn {
            index: 1,
            contributions: vec![
                contrib("core.view_loader", "context_assembly", "view_load", "gm_context: 6"),
                contrib("core.view_loader", "context_assembly", "view_load", "npc_mind_views: 1"),
                contrib("core.no_spoiler_guard", "context_assembly", "prompt_block", "no_spoiler_guidance"),
            ],
            ..Default::default()
        };
        let mut f = Vec::new();
        attribute_turn(&with, &mut f);
        assert!(f.is_empty(), "full coverage must not attribute: {f:?}");

        let without = FlightTurn { index: 1, contributions: vec![], ..Default::default() };
        let mut f2 = Vec::new();
        attribute_turn(&without, &mut f2);
        let layers: Vec<_> = f2.iter().map(|x| x.layer).collect();
        assert!(layers.contains(&Layer::Director));
        assert!(layers.contains(&Layer::World));
        assert!(layers.contains(&Layer::Policy));
    }

    #[test]
    fn escaped_leak_blames_narrator_caught_does_not() {
        let escaped = contrib(
            "core.no_spoiler_guard",
            "after_llm_stream",
            "verifier_finding",
            "leaked Athena 回收指令 into narration",
        );
        let caught = contrib("core.no_spoiler_guard", "after_llm_stream", "verifier_finding", "secret_leak");
        assert!(verifier_escaped(&escaped));
        assert!(!verifier_escaped(&caught), "a sanitized kind label is caught, not escaped");
    }
}

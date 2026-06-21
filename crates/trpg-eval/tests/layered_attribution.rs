//! V2-P5 — Flight-Recorder layered attribution (蓝图 §九/§十 第三阶段 = 验收3).
//!
//! Reads the runtime's own recorded contribution trail and proves each defect is
//! attributed to the producing layer. The good (`provenance`) and bad
//! (`failbranch`) deterministic fixtures diverge purely on their recorded trail,
//! so PASS/FAIL is metric-driven, not hardcoded.

use trpg_eval::flight::attribution::AttributionKind;
use trpg_eval::model::Layer;
use trpg_eval::{attribute, FlightRecording};

fn load(name: &str) -> FlightRecording {
    let p = format!("{}/fixtures/flight/{name}", env!("CARGO_MANIFEST_DIR"));
    let j = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {p}: {e}"));
    FlightRecording::parse(&j).unwrap_or_else(|e| panic!("parse {p}: {e}"))
}

/// Healthy trail: every required layer contributes, nothing leaks ⇒ PASS.
#[test]
fn good_provenance_passes_with_no_layer_findings() {
    let v = attribute(&load("good_provenance.json"));
    assert!(!v.is_fail(), "good fixture must PASS, got: {:?}", v.findings);
    assert!(v.findings.is_empty(), "no defect expected: {:?}", v.findings);
}

/// failbranch is missing the DIRECTOR (gm_context) + WORLD (npc_mind) view-loads
/// and leaks a secret into narration ⇒ FAIL attributed to DIRECTOR, WORLD, NARRATOR.
#[test]
fn failbranch_attributes_each_defect_to_its_layer() {
    let v = attribute(&load("bad_failbranch.json"));
    assert!(v.is_fail(), "failbranch must FAIL");
    let layers = v.implicated_layers();
    assert!(layers.contains(&Layer::Director), "missing gm_context ⇒ DIRECTOR: {layers:?}");
    assert!(layers.contains(&Layer::World), "missing npc_mind ⇒ WORLD: {layers:?}");
    assert!(layers.contains(&Layer::Narrator), "escaped leak ⇒ NARRATOR: {layers:?}");

    // The leak is an EscapedDefect (reached narration), not a missing layer.
    assert!(v
        .findings
        .iter()
        .any(|f| f.layer == Layer::Narrator && f.kind == AttributionKind::EscapedDefect));
    // The missing view-loads are MissingLayerContribution.
    assert!(v
        .findings
        .iter()
        .any(|f| f.layer == Layer::Director && f.kind == AttributionKind::MissingLayerContribution));
}

/// NO-FAKE discriminator: the SAME attributor gives DIFFERENT results on the two
/// fixtures — the good one does NOT implicate DIRECTOR/WORLD/NARRATOR, the bad one
/// does. Attribution is therefore driven by the actual recorded trail.
#[test]
fn attribution_is_trail_driven_not_hardcoded() {
    let good = attribute(&load("good_provenance.json"));
    let bad = attribute(&load("bad_failbranch.json"));
    for l in [Layer::Director, Layer::World, Layer::Narrator] {
        assert!(!good.implicated_layers().contains(&l), "good must NOT implicate {l}");
        assert!(bad.implicated_layers().contains(&l), "bad MUST implicate {l}");
    }
}

/// Every finding carries §八 evidence (turn + expected/actual + evidence lines).
#[test]
fn findings_are_evidenced() {
    let v = attribute(&load("bad_failbranch.json"));
    for f in &v.findings {
        assert!(f.turn >= 1);
        assert!(!f.expected.is_empty());
        assert!(!f.actual.is_empty());
        assert!(!f.evidence.is_empty(), "finding lacks evidence: {f:?}");
    }
}

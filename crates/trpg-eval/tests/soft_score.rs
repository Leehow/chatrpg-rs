//! Soft-weighted quality score (蓝图 §七 软加权分 + 不确定性).
//!
//! The blueprint's central indictment: the old harness rewarded prose length, so
//! "只要散文足够长，就被判定为真叙事". §七 fixes the weighting — 语言表现 must be the
//! LOWEST weight (5), mechanics the highest (25). These tests pin that inversion-fix
//! and prove the score is severity-driven, not a hardcoded number.

use std::collections::HashMap;
use trpg_eval::model::{EvalFinding, RootCause, Severity, Verdict};
use trpg_eval::score::{score_card, Dimension};

fn finding(cause: RootCause, sev: Severity) -> EvalFinding {
    EvalFinding::new(cause, sev, vec![1], "exp", "act", vec!["ev".into()])
}

fn verdict(findings: Vec<EvalFinding>) -> Verdict {
    Verdict { findings }
}

fn by_dim(card: &trpg_eval::score::ScoreCard) -> HashMap<Dimension, f32> {
    card.dimensions.iter().map(|d| (d.dimension, d.earned)).collect()
}

#[test]
fn clean_verdict_scores_full_with_language_uncertainty() {
    let card = score_card(&verdict(vec![]));
    // No findings → every assessed dimension keeps full weight.
    assert_eq!(card.total.round() as i32, 100);
    // Language (5) is not deterministically measured by the static arm → it is the
    // uncertainty band: we GAVE the 5 points but cannot verify them.
    assert_eq!(card.uncertainty.round() as i32, 5);
}

#[test]
fn language_is_lowest_weight_mechanics_highest() {
    // §七: 语言表现权重必须最低. Mechanics is the heaviest.
    let weights: Vec<f32> = Dimension::ALL.iter().map(|d| d.weight()).collect();
    let lang = Dimension::Language.weight();
    let rules = Dimension::RulesMechanics.weight();
    assert_eq!(lang, *weights.iter().min_by(|a, b| a.total_cmp(b)).unwrap());
    assert_eq!(rules, *weights.iter().max_by(|a, b| a.total_cmp(b)).unwrap());
    assert!(lang < rules);
    // Weights sum to 100.
    assert_eq!(weights.iter().sum::<f32>().round() as i32, 100);
}

#[test]
fn hard_finding_zeros_its_dimension() {
    // A hard-门槛 mechanical-debt breach zeros the Rules dimension's earned points.
    let card = score_card(&verdict(vec![finding(
        RootCause::UnresolvedMechanicalDebt,
        Severity::Hard,
    )]));
    let dims = by_dim(&card);
    assert_eq!(dims[&Dimension::RulesMechanics].round() as i32, 0);
    // Untouched dimensions keep their full weight.
    assert_eq!(dims[&Dimension::WorldContinuity].round() as i32, 20);
}

#[test]
fn score_is_severity_driven_not_hardcoded() {
    // Same dimension, same cause, only severity differs → a High finding must score
    // STRICTLY higher than a Hard one. Proves the number tracks the metric.
    let hard = score_card(&verdict(vec![finding(
        RootCause::UnresolvedMechanicalDebt,
        Severity::Hard,
    )]));
    let high = score_card(&verdict(vec![finding(
        RootCause::UnresolvedMechanicalDebt,
        Severity::High,
    )]));
    let info = score_card(&verdict(vec![finding(
        RootCause::UnresolvedMechanicalDebt,
        Severity::Info,
    )]));
    assert!(high.total > hard.total, "High {} should beat Hard {}", high.total, hard.total);
    assert!(info.total > high.total, "Info {} should beat High {}", info.total, high.total);
}

#[test]
fn total_equals_sum_of_earned_dimensions() {
    let card = score_card(&verdict(vec![
        finding(RootCause::SceneReset, Severity::Hard),
        finding(RootCause::PlayerActionLoop, Severity::High),
    ]));
    let sum: f32 = card.dimensions.iter().map(|d| d.earned).sum();
    assert!((card.total - sum).abs() < 0.001);
}

#[test]
fn bad_fixtures_score_below_clean_with_layer_zeros() {
    let dir = env!("CARGO_MANIFEST_DIR");
    let read = |p: &str| std::fs::read_to_string(format!("{dir}/{p}")).unwrap();

    let cyber = score_card(&trpg_eval::evaluate(&trpg_eval::parse_transcript(&read(
        "fixtures/bad/cyber_repetition.md",
    ))));
    let coc = score_card(&trpg_eval::evaluate(&trpg_eval::parse_transcript(&read(
        "fixtures/bad/coc_semantic_shell.md",
    ))));
    let clean = score_card(&trpg_eval::evaluate(&trpg_eval::parse_transcript(&read(
        "fixtures/good/clean_min.md",
    ))));

    // The clean control scores full; the bad reports score materially lower.
    assert_eq!(clean.total.round() as i32, 100);
    assert!(cyber.total < 50.0, "cyber {} should be deeply failing", cyber.total);
    assert!(coc.total < clean.total, "coc {} < clean {}", coc.total, clean.total);

    // Cyber's hard breaches zero exactly the layers the blueprint named.
    let cd = by_dim(&cyber);
    assert_eq!(cd[&Dimension::RulesMechanics].round() as i32, 0); // 待结算 debt
    assert_eq!(cd[&Dimension::WorldContinuity].round() as i32, 0); // scene reset
    assert_eq!(cd[&Dimension::Responsiveness].round() as i32, 0); // intent mismatch

    // COC's failure is concentrated: progression + responsiveness, NOT rules/world.
    let od = by_dim(&coc);
    assert_eq!(od[&Dimension::Progression].round() as i32, 0); // success-without-info
    assert_eq!(od[&Dimension::Responsiveness].round() as i32, 0); // intent mismatch
    assert_eq!(od[&Dimension::RulesMechanics].round() as i32, 25); // rules never flagged
    assert_eq!(od[&Dimension::WorldContinuity].round() as i32, 20); // world never flagged
}

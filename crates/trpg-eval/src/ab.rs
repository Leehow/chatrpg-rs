//! A/B baseline-vs-candidate paired comparison (蓝图 §十 第四阶段 = 验收4).
//!
//! 绝对分意义小; "候选在同一输入下是否**稳定优于**基线" 才可靠 (§十). So this is a
//! BLIND, PAIRED, REPRODUCIBLE comparator: the two runs are opaque arms `A`/`B`
//! (the comparator never reads which is baseline vs candidate — the caller maps),
//! and the result is purely a function of the two evaluated runs, so repeating it
//! yields the identical winner (reproducible) and swapping the arms mirrors the
//! winner (no positional bias).
//!
//! It answers the four §十 questions by mapping each to the dimension that bears
//! it, plus a composite "更像真实GM" from the soft-weighted total.

use crate::aggregate::evaluate;
use crate::model::Transcript;
use crate::score::{score_card, Dimension, ScoreCard};
use serde::Serialize;

const EPS: f32 = 0.01;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Arm {
    A,
    B,
    Tie,
}

impl Arm {
    pub fn id(&self) -> &'static str {
        match self {
            Arm::A => "A",
            Arm::B => "B",
            Arm::Tie => "TIE",
        }
    }
}

/// Pick the better arm by a higher-is-better metric (Tie within EPS).
fn pick(a: f32, b: f32) -> Arm {
    if (a - b).abs() < EPS {
        Arm::Tie
    } else if a > b {
        Arm::A
    } else {
        Arm::B
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DimDelta {
    pub dimension: Dimension,
    pub a: f32,
    pub b: f32,
    /// b − a (positive ⇒ B better on this dimension).
    pub delta: f32,
    pub winner: Arm,
}

/// One of the four §十 blind-comparison questions and which arm wins it.
#[derive(Debug, Clone, Serialize)]
pub struct QuestionVerdict {
    pub question: &'static str,
    pub winner: Arm,
}

#[derive(Debug, Clone, Serialize)]
pub struct PairedComparison {
    pub a_total: f32,
    pub b_total: f32,
    pub a_hard_fail: bool,
    pub b_hard_fail: bool,
    pub dims: Vec<DimDelta>,
    pub questions: Vec<QuestionVerdict>,
    /// The stably-better arm under the same input (§十 的真正验收).
    pub overall: Arm,
    /// |a_total − b_total| — the score margin behind `overall`.
    pub margin: f32,
}

fn dim_earned(card: &ScoreCard, dim: Dimension) -> f32 {
    card.dimensions
        .iter()
        .find(|d| d.dimension == dim)
        .map(|d| d.earned)
        .unwrap_or(0.0)
}

/// The four §十 questions, each routed to the dimension that bears it.
const QUESTIONS: [(&str, Dimension); 3] = [
    ("哪一版更能回应玩家?", Dimension::Responsiveness),
    ("哪一版更愿意让人继续玩?", Dimension::Progression),
    ("哪一版规则更公平且可理解?", Dimension::RulesMechanics),
];

/// Compare two evaluated transcripts. BLIND (arms are opaque), pure ⇒ reproducible.
pub fn compare(a: &Transcript, b: &Transcript) -> PairedComparison {
    let (va, vb) = (evaluate(a), evaluate(b));
    let (ca, cb) = (score_card(&va), score_card(&vb));

    let dims = Dimension::ALL
        .iter()
        .map(|&dim| {
            let (ea, eb) = (dim_earned(&ca, dim), dim_earned(&cb, dim));
            DimDelta { dimension: dim, a: ea, b: eb, delta: eb - ea, winner: pick(ea, eb) }
        })
        .collect();

    let mut questions: Vec<QuestionVerdict> = QUESTIONS
        .iter()
        .map(|&(q, dim)| QuestionVerdict {
            question: q,
            winner: pick(dim_earned(&ca, dim), dim_earned(&cb, dim)),
        })
        .collect();
    // "更像真实GM" is the holistic question → the soft-weighted total.
    questions.push(QuestionVerdict {
        question: "哪一版更像真实 GM?",
        winner: pick(ca.total, cb.total),
    });

    let (a_hard_fail, b_hard_fail) = (va.is_fail(), vb.is_fail());
    // A run that breaches the hard 门槛 cannot be the stably-better arm.
    let overall = match (a_hard_fail, b_hard_fail) {
        (false, true) => Arm::A,
        (true, false) => Arm::B,
        _ => pick(ca.total, cb.total),
    };

    PairedComparison {
        a_total: ca.total,
        b_total: cb.total,
        a_hard_fail,
        b_hard_fail,
        dims,
        questions,
        overall,
        margin: (ca.total - cb.total).abs(),
    }
}

/// Reproducibility check (验收4): run the paired comparison `n` times and confirm
/// the winner is identical every time. The comparator is pure, so a stable winner
/// here is the deterministic guarantee the §十 A/B harness rests on.
pub fn is_reproducible(a: &Transcript, b: &Transcript, n: usize) -> bool {
    let first = compare(a, b).overall;
    (0..n).all(|_| compare(a, b).overall == first)
}

/// Render a blind paired-comparison board.
pub fn ab_board(label_a: &str, label_b: &str, c: &PairedComparison) -> String {
    let mut s = String::new();
    s.push_str("# A/B 盲测配对 (蓝图 §十 第四阶段 = 验收4)\n\n");
    s.push_str(&format!("A = {label_a}\nB = {label_b}\n\n"));
    s.push_str(&format!(
        "A 总分 {:.0} (hard_fail={})  vs  B 总分 {:.0} (hard_fail={})  margin={:.0}\n\n",
        c.a_total, c.a_hard_fail, c.b_total, c.b_hard_fail, c.margin
    ));
    s.push_str("## 逐维度\n");
    for d in &c.dims {
        s.push_str(&format!(
            "  {:<22} A {:>4.0} | B {:>4.0}  Δ{:+.0}  → {}\n",
            d.dimension.label(),
            d.a,
            d.b,
            d.delta,
            d.winner.id()
        ));
    }
    s.push_str("\n## 四问\n");
    for q in &c.questions {
        s.push_str(&format!("  {:<26} → {}\n", q.question, q.winner.id()));
    }
    s.push_str(&format!("\nVERDICT: 稳定更优 = {}\n", c.overall.id()));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_transcript;

    fn fixt(rel: &str) -> Transcript {
        let p = format!("{}/fixtures/{rel}", env!("CARGO_MANIFEST_DIR"));
        parse_transcript(&std::fs::read_to_string(p).unwrap())
    }

    #[test]
    fn candidate_clean_beats_baseline_bad() {
        let bad = fixt("bad/cyber_repetition.md");
        let clean = fixt("good/clean_min.md");
        let c = compare(&bad, &clean); // A=bad, B=clean
        assert_eq!(c.overall, Arm::B, "clean candidate must win");
        assert!(c.b_total > c.a_total);
        assert!(c.a_hard_fail && !c.b_hard_fail);
    }

    #[test]
    fn no_positional_bias_swapping_arms_mirrors_winner() {
        let bad = fixt("bad/coc_semantic_shell.md");
        let clean = fixt("good/clean_min.md");
        let ab = compare(&bad, &clean).overall; // clean is B
        let ba = compare(&clean, &bad).overall; // clean is A
        assert_eq!(ab, Arm::B);
        assert_eq!(ba, Arm::A); // same transcript wins regardless of position
    }

    #[test]
    fn identical_arms_tie() {
        let clean = fixt("good/clean_min.md");
        let c = compare(&clean, &clean);
        assert_eq!(c.overall, Arm::Tie);
        assert!(c.margin < EPS);
    }

    #[test]
    fn comparison_is_reproducible() {
        let bad = fixt("bad/cyber_repetition.md");
        let clean = fixt("good/clean_min.md");
        assert!(is_reproducible(&bad, &clean, 20));
    }

    #[test]
    fn per_question_winner_is_metric_driven() {
        // The bad cyber run zeros Responsiveness; clean retains it ⇒ B wins that Q.
        let bad = fixt("bad/cyber_repetition.md");
        let clean = fixt("good/clean_min.md");
        let c = compare(&bad, &clean);
        let resp = c
            .questions
            .iter()
            .find(|q| q.question.contains("回应玩家"))
            .unwrap();
        assert_eq!(resp.winner, Arm::B);
    }
}

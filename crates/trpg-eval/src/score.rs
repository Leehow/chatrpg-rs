//! Soft-weighted quality score (蓝图 §七 软加权分 + 不确定性).
//!
//! The hard 门槛 (in `aggregate`) decides PASS/FAIL. This layer adds a 0-100 quality
//! number whose dimension weights match the blueprint so a longer-prose report can
//! NOT out-score a mechanically sound one — the exact inversion §七 calls out
//! ("语言表现权重必须最低. 当前恰恰相反：只要散文足够长，就被判定为真叙事").
//!
//! Deterministic only: each [`RootCause`] maps to one dimension; severity decides how
//! much of that dimension's weight survives. 语言表现 (5) has no deterministic probe
//! in the static arm, so it is granted full credit but surfaced as the uncertainty band.

use crate::model::{RootCause, Severity, Verdict};
use serde::Serialize;

/// The six §七 scoring dimensions with their blueprint weights (sum = 100).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Dimension {
    /// 规则与机械完整性 — 25 (heaviest).
    RulesMechanics,
    /// 世界状态与因果连续性 — 20.
    WorldContinuity,
    /// 对玩家行动的响应性 — 20.
    Responsiveness,
    /// 剧情推进与玩家能动性 — 15.
    Progression,
    /// 玩家模拟真实性 — 15.
    PlayerSimRealism,
    /// 语言表现 — 5 (lowest, by mandate).
    Language,
}

impl Dimension {
    pub const ALL: [Dimension; 6] = [
        Dimension::RulesMechanics,
        Dimension::WorldContinuity,
        Dimension::Responsiveness,
        Dimension::Progression,
        Dimension::PlayerSimRealism,
        Dimension::Language,
    ];

    pub fn weight(&self) -> f32 {
        match self {
            Dimension::RulesMechanics => 25.0,
            Dimension::WorldContinuity => 20.0,
            Dimension::Responsiveness => 20.0,
            Dimension::Progression => 15.0,
            Dimension::PlayerSimRealism => 15.0,
            Dimension::Language => 5.0,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Dimension::RulesMechanics => "规则与机械完整性",
            Dimension::WorldContinuity => "世界状态与因果连续性",
            Dimension::Responsiveness => "对玩家行动的响应性",
            Dimension::Progression => "剧情推进与玩家能动性",
            Dimension::PlayerSimRealism => "玩家模拟真实性",
            Dimension::Language => "语言表现",
        }
    }

    /// Whether the static arm has a deterministic probe for this dimension. Language
    /// is the only un-probed one — it becomes the uncertainty band.
    fn assessed(&self) -> bool {
        !matches!(self, Dimension::Language)
    }
}

/// Which dimension a defect debits (蓝图 §五/§九 与 §七 权重对齐).
fn dimension_of(cause: RootCause) -> Dimension {
    match cause {
        RootCause::UnresolvedMechanicalDebt => Dimension::RulesMechanics,
        RootCause::SceneReset => Dimension::WorldContinuity,
        RootCause::SemanticNoop => Dimension::Responsiveness,
        RootCause::ResponseIntentMismatch => Dimension::Responsiveness,
        RootCause::SuccessWithoutInformation => Dimension::Progression,
        RootCause::PlayerActionLoop => Dimension::PlayerSimRealism,
    }
}

/// Fraction of a dimension's weight that SURVIVES one finding of this severity.
/// A hard-门槛 breach zeros the dimension; quality issues erode it multiplicatively.
fn survive_factor(s: Severity) -> f32 {
    match s {
        Severity::Hard => 0.0,
        Severity::High => 0.6,
        Severity::Info => 0.95,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DimensionScore {
    pub dimension: Dimension,
    pub weight: f32,
    /// Surviving fraction in 0..=1.
    pub retained: f32,
    /// `weight * retained` — points contributed to the total.
    pub earned: f32,
    pub finding_count: usize,
    /// False for Language (no deterministic probe) — contributes to uncertainty.
    pub assessed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScoreCard {
    pub dimensions: Vec<DimensionScore>,
    /// Weighted quality score, 0..=100.
    pub total: f32,
    /// Points granted but not deterministically verified (un-probed dimensions).
    pub uncertainty: f32,
}

/// Compute the soft-weighted scorecard for a verdict.
pub fn score_card(v: &Verdict) -> ScoreCard {
    let mut dimensions = Vec::with_capacity(Dimension::ALL.len());
    let mut total = 0.0;
    let mut uncertainty = 0.0;

    for dim in Dimension::ALL {
        let matching: Vec<&_> = v
            .findings
            .iter()
            .filter(|f| dimension_of(f.cause) == dim)
            .collect();
        // Multiplicatively erode: each finding multiplies the surviving fraction, so
        // many quality issues drive the dimension toward 0 while staying monotone.
        let retained: f32 = matching.iter().map(|f| survive_factor(f.severity)).product();
        let earned = dim.weight() * retained;
        total += earned;
        if !dim.assessed() {
            uncertainty += dim.weight();
        }
        dimensions.push(DimensionScore {
            dimension: dim,
            weight: dim.weight(),
            retained,
            earned,
            finding_count: matching.len(),
            assessed: dim.assessed(),
        });
    }

    ScoreCard { dimensions, total, uncertainty }
}

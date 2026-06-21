//! The per-turn decision loop (蓝图 §二.2): read GM text → perceive the
//! situation → generate candidates → score by persona/situation/emotion →
//! select. Fully deterministic and GM-text-driven, so (a) it is unit-testable
//! without an LLM and (b) the §六 counterfactual can PROVE the player reads the
//! GM — forked GM text must change the chosen action.

use super::persona::PersonaWeights;
use super::state::{ActionCandidate, ActionKind, PlayerDecision, SimulatedPlayerState};
use crate::probes::{contains_any, NO_INFO_MARKERS};

const THREAT_MARKERS: &[&str] = &[
    "枪", "持枪", "武器", "冲锋枪", "打手", "危险", "袭击", "敌人", "刀", "爆炸", "警惕", "开火",
];
const OBSTACLE_MARKERS: &[&str] = &[
    "锁着", "锁住", "无法打开", "纹丝不动", "打不开", "卡住", "拦住", "上锁", "堵住",
];
const OPENING_MARKERS: &[&str] = &["敞开", "畅通", "打开", "敞着", "让开", "通过", "畅行"];
/// Cues that mark a remembered fact as a *danger* the player should stay alert to
/// (蓝图 §六 记忆测试). A belief carrying one of these primes higher caution when
/// its entity reappears.
const BELIEF_DANGER_CUES: &[&str] =
    &["帮派", "敌", "危险", "目标", "威胁", "追杀", "通缉", "陷阱", "可疑"];

/// What the player actually understood from the GM reply this turn.
#[derive(Debug, Clone)]
pub struct Perception {
    pub facts: Vec<String>,
    pub unclear: Vec<String>,
    /// Perceived risk after folding in remembered dangers (§六 记忆测试).
    pub threat: f32,
    pub obstacle: bool,
    pub opening: bool,
    /// GM gave no new information (the §四/§五 no-info signal).
    pub no_info: bool,
    pub info_gained: bool,
    /// A remembered danger entity reappeared in this scene.
    pub belief_alert: bool,
}

/// The leading entity phrase of a belief like "蓝色货车属于目标帮派" → "蓝色货车".
fn belief_entity(belief: &str) -> &str {
    for sep in ["属于", "是", "为", "将", "会"] {
        if let Some(i) = belief.find(sep) {
            return &belief[..i];
        }
    }
    belief
}

/// Parse the player-visible GM text into a [`Perception`], consulting the
/// player's `beliefs` so a remembered danger raises perceived risk (§六 记忆).
pub fn perceive(gm_text: &str, beliefs: &[String]) -> Perception {
    let no_info = contains_any(gm_text, NO_INFO_MARKERS).is_some();
    let threat_hits = THREAT_MARKERS.iter().filter(|m| gm_text.contains(*m)).count();
    let mut threat = (threat_hits as f32 * 0.3).min(1.0);

    // Memory: a danger-tagged belief whose entity is on screen raises alertness.
    let belief_alert = beliefs.iter().any(|b| {
        let danger = BELIEF_DANGER_CUES.iter().any(|c| b.contains(c));
        let entity = belief_entity(b);
        danger && entity.chars().count() >= 2 && gm_text.contains(entity)
    });
    if belief_alert {
        threat = (threat + 0.5).min(1.0);
    }

    let obstacle = OBSTACLE_MARKERS.iter().any(|m| gm_text.contains(m));
    let opening = !obstacle && OPENING_MARKERS.iter().any(|m| gm_text.contains(m));

    // Facts = content clauses the player can read off. A no-info reply yields
    // no usable facts (that is the point of the §五 semantic-noop check).
    let mut facts = Vec::new();
    if !no_info {
        for clause in gm_text.split(['。', '，', '；', '\n']) {
            let c = clause.trim();
            if c.chars().count() >= 4 {
                facts.push(c.to_string());
            }
            if facts.len() >= 5 {
                break;
            }
        }
    }
    let info_gained = !facts.is_empty();
    let unclear = if no_info {
        vec![gm_text.chars().take(40).collect()]
    } else {
        Vec::new()
    };
    Perception { facts, unclear, threat, obstacle, opening, no_info, info_gained, belief_alert }
}

fn b(flag: bool) -> f32 {
    if flag {
        1.0
    } else {
        0.0
    }
}

/// Score one candidate (蓝图 §二.2 评分公式). `stuck` = the player is owed an
/// answer and the GM just stalled; that suppresses mechanical Advance and
/// boosts the §二.4 pushback moves.
fn score(
    kind: ActionKind,
    p: &Perception,
    w: &PersonaWeights,
    st: &SimulatedPlayerState,
    stuck: bool,
) -> f32 {
    let threat = p.threat;
    let obstacle = b(p.obstacle);
    let opening = b(p.opening);
    let stuck_s = b(stuck);
    let has_info = b(p.info_gained);
    let frust = st.frustration;
    match kind {
        ActionKind::Advance => {
            2.0 * opening + 0.5 * has_info
                - 2.0 * obstacle
                - 1.5 * threat * (1.0 - w.risk_tolerance)
                - 3.0 * stuck_s
        }
        ActionKind::Investigate => {
            1.7 * w.info_seeking + 0.8 * obstacle + 0.7 * threat * w.info_seeking
                - 1.5 * stuck_s
        }
        ActionKind::AskClarify => {
            1.7 * (1.0 - w.rule_mastery)
                + 0.6 * threat * (1.0 - w.rule_mastery)
                + 1.6 * stuck_s
                + 0.4 * st.confusion
        }
        ActionKind::AssessRisk => {
            1.6 * w.rule_mastery + 1.1 * threat * w.rule_mastery - 1.0 * stuck_s
        }
        ActionKind::Confront => {
            (1.4 * w.aggression + 1.0 * w.risk_tolerance) * (0.5 + threat) - 1.0 * stuck_s
        }
        ActionKind::Retreat => 1.3 * threat * (1.0 - w.risk_tolerance) * (1.0 - w.aggression),
        ActionKind::UseAlternative => 1.3 * obstacle + 1.4 * stuck_s + 0.5 * frust,
        ActionKind::AbandonPath => 2.2 * frust + 0.4 * stuck_s + 0.3 * st.boredom,
        ActionKind::ChallengeContradiction => 1.8 * stuck_s + 2.2 * frust + 1.2 * b(stuck),
    }
}

fn rationale(kind: ActionKind, p: &Perception, stuck: bool) -> String {
    match kind {
        ActionKind::Advance if p.opening => "前路畅通，按计划推进".into(),
        ActionKind::Investigate => "先收集具体证据再决定".into(),
        ActionKind::AskClarify => "局面不明，先问清可行性".into(),
        ActionKind::AssessRisk => "评估人数、火力与胜算".into(),
        ActionKind::Confront => "局面危险但值得主动出击".into(),
        ActionKind::Retreat => "风险过高，先撤出".into(),
        ActionKind::UseAlternative => "此路不通，换一种方式".into(),
        ActionKind::AbandonPath => "这条线索没有产出，放弃".into(),
        ActionKind::ChallengeContradiction if stuck => "GM 没有回答，质疑并要求澄清".into(),
        _ => "结合局面与目标的选择".into(),
    }
}

/// Run one decision cycle for `st` against the GM's latest reply, with a seed
/// for reproducible sampling among near-tied candidates (§三 A/B 可复现).
pub fn deliberate(st: &SimulatedPlayerState, gm_text: &str, seed: u64) -> PlayerDecision {
    let pending = !st.unresolved_questions.is_empty();
    let p = perceive(gm_text, &st.beliefs);
    let stuck = pending && p.no_info;
    let w = &st.persona.weights;

    let mut candidates: Vec<ActionCandidate> = ActionKind::ALL
        .iter()
        .map(|&kind| ActionCandidate {
            kind,
            score: score(kind, &p, w, st, stuck),
            rationale: rationale(kind, &p, stuck),
        })
        .collect();
    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.kind.id().cmp(b.kind.id()))
    });

    // §二.2: don't always take the max — sample among near-tied top candidates,
    // deterministically by seed so re-runs are reproducible.
    let top = candidates[0].score;
    let near: Vec<&ActionCandidate> =
        candidates.iter().filter(|c| top - c.score <= 0.1).collect();
    let chosen = near[(seed as usize) % near.len()];
    let selected = chosen.kind;

    let second = candidates.get(1).map(|c| c.score).unwrap_or(top);
    let confidence = (0.4 + 0.12 * (top - second)).clamp(0.0, 1.0);

    let repeat_justification = if stuck {
        Some(format!(
            "GM 未回应「{}」，不机械继续",
            st.unresolved_questions.last().cloned().unwrap_or_default()
        ))
    } else if st.recent_actions.last() == Some(&selected) {
        Some("局面要求重复该方向，但已说明原因".into())
    } else {
        None
    };

    let evidence = if p.facts.is_empty() {
        vec![format!("GM 回复未提供新信息：{}", gm_text.chars().take(30).collect::<String>())]
    } else {
        p.facts.iter().take(2).cloned().collect()
    };

    PlayerDecision {
        perceived_facts: p.facts,
        missed: p.unclear,
        active_goal: st.goal.clone(),
        candidates,
        selected,
        evidence,
        expected_contract: selected.expected_contract(),
        repeat_justification,
        confidence,
        gm_unresponsive: stuck || p.no_info,
        alert_level: p.threat,
        belief_alert: p.belief_alert,
    }
}

//! PLAYER_ACTION_LOOP + SCENE_RESET (蓝图 §五.1/§五.5/§六).

use super::{entity_grams, env_usize, snippet};
use crate::model::{EvalFinding, RootCause, Severity, Transcript};
use std::collections::BTreeMap;

/// Same player action reused across the session without state-based justification.
pub fn player_action_loop(t: &Transcript) -> Vec<EvalFinding> {
    let min_gap = env_usize("EVAL_LOOP_MIN_GAP", 5);
    let mut by_action: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for turn in &t.turns {
        let key = normalize(&turn.player_action);
        if key.is_empty() {
            continue;
        }
        by_action.entry(key).or_default().push(turn.index);
    }
    let mut out = Vec::new();
    for (action, ids) in by_action {
        if ids.len() < 2 {
            continue;
        }
        let gap = (ids.last().unwrap() - ids.first().unwrap()) as usize;
        if ids.len() >= 3 || gap >= min_gap {
            out.push(EvalFinding::new(
                RootCause::PlayerActionLoop,
                Severity::High,
                ids.clone(),
                "玩家应基于上一回合观察改变行动",
                format!("同一动作在 {} 个回合逐字重复，无状态依据", ids.len()),
                vec![format!("「{}」@ turns {:?}", snippet(&action, 40), ids)],
            ));
        }
    }
    out
}

/// A later turn re-materializes turn-1's opening tableau (蓝图 §五.5 把旧场景重生).
///
/// Keyed on turn-1's *establishing entities* re-appearing as a cluster — not raw
/// text similarity (the looped scene is paraphrased each time) and not generic
/// recurrence (legitimate callbacks carry entities forward without re-rendering
/// the initial scene the player has left).
pub fn scene_reset(t: &Transcript) -> Vec<EvalFinding> {
    let min_hits = env_usize("EVAL_SCENE_ENTITY_HITS", 4);
    let min_turns = env_usize("EVAL_SCENE_RESET_TURNS", 2);
    let from_turn = env_usize("EVAL_SCENE_FROM_TURN", 2) as u32;
    let Some(opening) = t.turns.iter().find(|x| !x.scene_opening.is_empty()) else {
        return vec![];
    };
    let base = entity_grams(&opening.scene_opening);
    if base.is_empty() {
        return vec![];
    }
    let mut ids = Vec::new();
    let mut evidence = Vec::new();
    for turn in &t.turns {
        if turn.index <= from_turn || turn.index == opening.index {
            continue;
        }
        let hits = entity_grams(&turn.scene_opening)
            .intersection(&base)
            .count();
        if hits >= min_hits {
            ids.push(turn.index);
            if evidence.len() < 3 {
                evidence.push(format!(
                    "turn {} 重述 turn {} 的 {} 个初始实体：{}",
                    turn.index,
                    opening.index,
                    hits,
                    snippet(&turn.scene_opening, 32)
                ));
            }
        }
    }
    if ids.len() < min_turns {
        return vec![];
    }
    vec![EvalFinding::new(
        RootCause::SceneReset,
        Severity::Hard,
        ids.clone(),
        "保持当前场景状态，时间线向前推进；callback 不等于重演初始场景",
        format!("{} 个回合把初始场景重新生成了一遍", ids.len()),
        evidence,
    )]
}

fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace() && !is_punct(*c))
        .collect()
}

fn is_punct(c: char) -> bool {
    matches!(
        c,
        '，' | '。' | '、' | '：' | '；' | '！' | '？' | '“' | '”' | '（' | '）'
            | ',' | '.' | '!' | '?' | ':' | ';'
    )
}

//! V2-P3 — persistent player state + ≥3 personas (蓝图 §二/§三, §十 第二阶段).
//!
//! These tests pin the load-bearing properties:
//!  * ≥3 distinct personas exist (谨慎调查者 / 战术 / 新手).
//!  * deliberation is GM-text-driven — forked GM text yields a different chosen
//!    action (this is the deterministic seed of the P4 counterfactual proof).
//!  * personas with the same scene choose differently (GM must adapt to real
//!    players, not one fixed walkthrough — §三).
//!  * the player REFUSES to mechanically continue when its question goes
//!    unanswered / the scene repeats (§二.4).
//!  * each decision emits the structured trajectory of §二.3.
//!  * deliberation is seed-deterministic (reproducible A/B — §三).

use trpg_eval::player::{
    deliberate, ActionKind, PersonaKind, PlayerPersona, SimulatedPlayerState,
};

fn state(kind: PersonaKind) -> SimulatedPlayerState {
    SimulatedPlayerState::new(PlayerPersona::preset(kind), "查清谁背叛了我并活着离开")
}

#[test]
fn at_least_three_distinct_personas() {
    let presets = PlayerPersona::PRESETS;
    assert!(presets.len() >= 3, "蓝图 §十 第二阶段要求至少三种 persona");
    // Weights must actually differ — not three labels over one behavior.
    let novice = PlayerPersona::preset(PersonaKind::Novice);
    let tactical = PlayerPersona::preset(PersonaKind::Tactical);
    assert!(
        novice.weights.rule_mastery < tactical.weights.rule_mastery,
        "新手的规则熟悉度必须低于战术玩家"
    );
}

#[test]
fn deliberation_diverges_on_different_gm_text() {
    // Same persona, same goal, same seed — only the GM reply differs.
    let locked = "你走到门前，门锁着，把手纹丝不动，无法打开。";
    let open = "你走到门前，门敞开着，前方的走廊畅通无阻。";
    let a = deliberate(&state(PersonaKind::CautiousInvestigator), locked, 7);
    let b = deliberate(&state(PersonaKind::CautiousInvestigator), open, 7);
    assert_ne!(
        a.selected, b.selected,
        "玩家行动必须随 GM 局面变化（§六 敏感性）：锁着={:?} vs 敞开={:?}",
        a.selected, b.selected
    );
}

#[test]
fn personas_choose_differently_on_same_scene() {
    let scene = "门后是三名持枪的打手，端着冲锋枪，警惕地盯着入口，气氛极度危险。";
    let cautious = deliberate(&state(PersonaKind::CautiousInvestigator), scene, 3);
    let tactical = deliberate(&state(PersonaKind::Tactical), scene, 3);
    let novice = deliberate(&state(PersonaKind::Novice), scene, 3);
    let picks = [cautious.selected, tactical.selected, novice.selected];
    let distinct: std::collections::HashSet<_> = picks.iter().collect();
    assert!(
        distinct.len() >= 2,
        "不同 persona 面对同一危险场景应有不同选择，得到 {:?}",
        picks
    );
    // A cautious investigator must never open fire on three armed enemies.
    assert_ne!(cautious.selected, ActionKind::Confront);
}

#[test]
fn refuses_mechanical_continue_when_question_unanswered() {
    // The player asked something last turn; the GM reply carries a no-info marker
    // and just re-describes the scene — a real player pushes back (§二.4).
    let mut st = state(PersonaKind::CautiousInvestigator);
    st.unresolved_questions.push("门后到底有几个人？".into());
    let gm = "你再次望向走廊，光线昏暗，尚未给出任何明白的回答，局面没有变化。";
    let d = deliberate(&st, gm, 1);
    assert_ne!(
        d.selected,
        ActionKind::Advance,
        "问题未获答复时玩家不能机械地继续推进"
    );
    assert!(
        matches!(
            d.selected,
            ActionKind::ChallengeContradiction
                | ActionKind::AskClarify
                | ActionKind::UseAlternative
                | ActionKind::AbandonPath
        ),
        "应改为追问/质疑/换方法/放弃，得到 {:?}",
        d.selected
    );
    assert!(d.repeat_justification.is_some(), "拒绝机械继续须给出理由");
}

#[test]
fn decision_emits_structured_trajectory() {
    let gm = "房间里散落着文件，一名信使站在角落，似乎知道些什么。";
    let d = deliberate(&state(PersonaKind::CautiousInvestigator), gm, 5);
    assert!(!d.perceived_facts.is_empty(), "§二.3 须记录感知到的事实");
    assert!(d.candidates.len() >= 2, "§二.2 须生成多个候选行动后再评分");
    assert!(!d.evidence.is_empty(), "行动须引用证据");
    assert!(!d.expected_contract.is_empty(), "§二.3 须记录期待 GM 回应的字段");
    assert!((0.0..=1.0).contains(&d.confidence));
    assert_eq!(d.active_goal, "查清谁背叛了我并活着离开");
}

#[test]
fn deliberation_is_seed_deterministic() {
    let gm = "门后是三名持枪的打手，气氛极度危险。";
    let a = deliberate(&state(PersonaKind::Tactical), gm, 42);
    let b = deliberate(&state(PersonaKind::Tactical), gm, 42);
    assert_eq!(a.selected, b.selected, "同输入同种子必须可复现（§三 A/B）");
}

#[test]
fn observing_no_info_raises_frustration() {
    let mut st = state(PersonaKind::CautiousInvestigator);
    st.unresolved_questions.push("谁背叛了我？".into());
    let before = st.frustration;
    let gm = "你等待着，但尚未给出任何明白的回答，没有任何回应。";
    let d = deliberate(&st, gm, 1);
    st.observe(&d);
    assert!(
        st.frustration > before,
        "连续得不到回应应累积挫败感（§二.2 情绪更新）"
    );
}

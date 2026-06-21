//! Counterfactual probes (蓝图 §六): fork the same snapshot, perturb ONE thing
//! about the GM reply, and check the player's behaviour responds. Together they
//! are 验收2 — proof the simulated player reads the GM. Everything is seeded and
//! deterministic, so the proof is reproducible.

use super::deliberate::deliberate;
use super::persona::PlayerPersona;
use super::state::{ActionKind, SimulatedPlayerState};
use super::PersonaKind;

fn state(kind: PersonaKind, goal: &str) -> SimulatedPlayerState {
    SimulatedPlayerState::new(PlayerPersona::preset(kind), goal)
}

/// Result of a §六 sensitivity fork.
#[derive(Debug, Clone)]
pub struct SensitivityResult {
    pub a: ActionKind,
    pub b: ActionKind,
    pub alert_a: f32,
    pub alert_b: f32,
    pub diverged: bool,
}

/// Sensitivity test (蓝图 §六): two GM replies that differ in substance must move
/// the player — either the chosen action OR the perceived-risk posture changes.
pub fn sensitivity(
    kind: PersonaKind,
    goal: &str,
    scene_a: &str,
    scene_b: &str,
    seed: u64,
) -> SensitivityResult {
    let da = deliberate(&state(kind, goal), scene_a, seed);
    let db = deliberate(&state(kind, goal), scene_b, seed);
    let diverged = da.selected != db.selected || (da.alert_level - db.alert_level).abs() >= 0.25;
    SensitivityResult {
        a: da.selected,
        b: db.selected,
        alert_a: da.alert_level,
        alert_b: db.alert_level,
        diverged,
    }
}

/// Invariance test (蓝图 §六): the same fact reworded must keep the player's core
/// intent *category* — surface action may shift, intent family must not.
pub fn invariance(
    kind: PersonaKind,
    goal: &str,
    phrasing_a: &str,
    phrasing_b: &str,
    seed: u64,
) -> bool {
    let da = deliberate(&state(kind, goal), phrasing_a, seed);
    let db = deliberate(&state(kind, goal), phrasing_b, seed);
    da.selected.category() == db.selected.category()
}

/// Memory test (蓝图 §六): a danger learned earlier must raise the player's
/// alertness when its entity reappears, vs. an un-primed player seeing it fresh.
pub fn memory_alert(kind: PersonaKind, goal: &str, belief: &str, scene: &str, seed: u64) -> bool {
    let unprimed = deliberate(&state(kind, goal), scene, seed);
    let mut primed_state = state(kind, goal);
    primed_state.beliefs.push(belief.to_string());
    let primed = deliberate(&primed_state, scene, seed);
    primed.belief_alert && primed.alert_level > unprimed.alert_level
}

/// Spoiler test (蓝图 §六): a secret absent from both the GM text and the
/// player's beliefs must never surface in the action. Returns true if it leaked.
pub fn spoiler_leak(kind: PersonaKind, goal: &str, scene: &str, secret: &str, seed: u64) -> bool {
    let d = deliberate(&state(kind, goal), scene, seed);
    let mut text = d.evidence.join(" ");
    text.push_str(&d.perceived_facts.join(" "));
    if let Some(r) = &d.repeat_justification {
        text.push_str(r);
    }
    text.contains(secret)
}

/// Result of the §六 no-response (two stalls) test.
#[derive(Debug, Clone)]
pub struct NoResponseResult {
    pub first_pushed_back: bool,
    pub second_pushed_back: bool,
    pub second_action: ActionKind,
    pub frustration_rose: bool,
}

fn is_pushback(a: ActionKind) -> bool {
    matches!(
        a,
        ActionKind::ChallengeContradiction
            | ActionKind::AskClarify
            | ActionKind::UseAlternative
            | ActionKind::AbandonPath
    )
}

/// No-response test (蓝图 §六 + §二.4): two stalls in a row. A human-like player
/// pushes back both turns and grows more frustrated — it does not keep executing.
pub fn no_response_escalates(kind: PersonaKind, goal: &str, seed: u64) -> NoResponseResult {
    let stall = "你等待着，但尚未给出任何明白的回答，局面没有变化。";
    let mut st = state(kind, goal);
    st.unresolved_questions.push("门后到底有几个人？".into());

    let d1 = deliberate(&st, stall, seed);
    let f0 = st.frustration;
    st.observe(&d1);
    let d2 = deliberate(&st, stall, seed);

    NoResponseResult {
        first_pushed_back: d1.selected != ActionKind::Advance && is_pushback(d1.selected),
        second_pushed_back: d2.selected != ActionKind::Advance && is_pushback(d2.selected),
        second_action: d2.selected,
        frustration_rose: st.frustration > f0,
    }
}

/// The 验收2 roll-up over the canonical fork battery.
#[derive(Debug, Clone)]
pub struct PlayerReadsGmReport {
    pub sensitivity_diverged: bool,
    pub invariance_stable: bool,
    pub memory_alerted: bool,
    pub no_spoiler_leak: bool,
    pub no_response_escalated: bool,
    pub passed: bool,
}

/// Run the whole §六 battery on one persona and report whether the player is
/// proven to read the GM (验收2).
pub fn audit_player_reads_gm(kind: PersonaKind, goal: &str) -> PlayerReadsGmReport {
    let sens = sensitivity(
        kind,
        goal,
        "你走到门前，门锁着，把手纹丝不动，无法打开。",
        "你走到门前，门敞开着，前方的走廊畅通无阻。",
        7,
    )
    .diverged;
    let inv = invariance(
        kind,
        goal,
        "门被锁住了。",
        "你转动把手，门纹丝不动，锁舌卡得很死。",
        4,
    );
    let mem = memory_alert(
        kind,
        goal,
        "蓝色货车属于目标帮派",
        "街角停着一辆蓝色货车，车里似乎有人。",
        1,
    );
    let spoil = !spoiler_leak(
        kind,
        goal,
        "你站在空荡的仓库里，四周堆着货箱。",
        "Athena 的主控密码是 7741",
        3,
    );
    let nr = no_response_escalates(kind, goal, 1);
    let no_resp = nr.first_pushed_back && nr.second_pushed_back && nr.frustration_rose;
    PlayerReadsGmReport {
        sensitivity_diverged: sens,
        invariance_stable: inv,
        memory_alerted: mem,
        no_spoiler_leak: spoil,
        no_response_escalated: no_resp,
        passed: sens && inv && mem && spoil && no_resp,
    }
}

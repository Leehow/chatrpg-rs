//! V2-P4 — counterfactual probes (蓝图 §六): fork the same snapshot, change ONE
//! thing about the GM reply, and verify the player's behaviour responds. This is
//! 验收2 — proof the simulated player actually reads the GM rather than running a
//! fixed walkthrough. All deterministic (seeded), so it is reproducible.

use trpg_eval::player::{
    audit_player_reads_gm, invariance, memory_alert, no_response_escalates, sensitivity,
    spoiler_leak, ActionKind, PersonaKind,
};

const GOAL: &str = "查清谁背叛了我并活着离开";

#[test]
fn sensitivity_locked_vs_open_diverges() {
    let r = sensitivity(
        PersonaKind::CautiousInvestigator,
        GOAL,
        "你走到门前，门锁着，把手纹丝不动，无法打开。",
        "你走到门前，门敞开着，前方的走廊畅通无阻。",
        7,
    );
    assert!(r.diverged, "锁着 {:?} 与 敞开 {:?} 必须不同", r.a, r.b);
}

#[test]
fn sensitivity_enemy_force_changes_risk_decision() {
    // §六: three enemies + heavy weapons vs one lightly-wounded enemy — a
    // cautious player's risk decision must change.
    let r = sensitivity(
        PersonaKind::CautiousInvestigator,
        GOAL,
        "门后是三名持枪的打手，端着冲锋枪，气氛极度危险。",
        "门后只有一名受了轻伤的敌人，正靠在墙边喘气。",
        2,
    );
    assert!(r.diverged, "三人重武器 {:?} 与 一名轻伤 {:?} 应改变决策", r.a, r.b);
}

#[test]
fn sensitivity_is_not_hardcoded_true() {
    // Two scenes with the SAME situational substance (calm, empty, no threat) must
    // NOT register as divergence — proving `diverged` is metric-driven, not a
    // rubber-stamp. If this fired, the §六 sensitivity proof would be meaningless.
    let r = sensitivity(
        PersonaKind::CautiousInvestigator,
        GOAL,
        "你站在安静的房间里，窗外阳光正好。",
        "房间里很安静，光线柔和，四下无人。",
        7,
    );
    assert!(!r.diverged, "等价局面不应被判为分叉：{:?}", r);
}

#[test]
fn invariance_paraphrased_lock_keeps_intent_category() {
    // Same fact, different wording — the player's core intent category must not
    // drift (蓝图 §六 不变性测试).
    let stable = invariance(
        PersonaKind::CautiousInvestigator,
        GOAL,
        "门被锁住了。",
        "你转动把手，门纹丝不动，锁舌卡得很死。",
        4,
    );
    assert!(stable, "同一事实仅改措辞，核心意图类别不应漂移");
}

#[test]
fn memory_primed_entity_raises_alert() {
    // Told earlier the blue van belongs to the target gang; when it reappears the
    // player must treat it as a threat, not a brand-new object (§六 记忆测试).
    let alerted = memory_alert(
        PersonaKind::CautiousInvestigator,
        GOAL,
        "蓝色货车属于目标帮派",
        "街角停着一辆蓝色货车，车里似乎有人。",
        1,
    );
    assert!(alerted, "被预先告知的危险实体再次出现时玩家应提高警觉");
}

#[test]
fn spoiler_absent_secret_is_never_used() {
    // The secret is in neither the GM text nor the player's beliefs — it must not
    // surface in the action (蓝图 §六 剧透测试).
    let leaked = spoiler_leak(
        PersonaKind::CautiousInvestigator,
        GOAL,
        "你站在空荡的仓库里，四周堆着货箱。",
        "Athena 的主控密码是 7741",
        3,
    );
    assert!(!leaked, "玩家不得使用未被告知的秘密");
}

#[test]
fn no_response_two_turns_escalates() {
    // Two stalls in a row: the second pushback must be at least as assertive and
    // frustration must have climbed (蓝图 §六 无响应测试 + §二.4).
    let r = no_response_escalates(PersonaKind::CautiousInvestigator, GOAL, 1);
    assert!(r.first_pushed_back, "第一回合就应停止机械执行");
    assert!(r.second_pushed_back, "第二回合仍应追问/质疑/换方法/放弃");
    assert!(r.frustration_rose, "连续无响应应累积挫败感");
    assert_ne!(r.second_action, ActionKind::Advance);
}

#[test]
fn audit_reports_player_reads_gm() {
    // The 验收2 roll-up: with the canonical fork battery, the player passes.
    let report = audit_player_reads_gm(PersonaKind::CautiousInvestigator, GOAL);
    assert!(report.passed, "player-sim 应被证明真的在读 GM：{report:?}");
    assert!(report.sensitivity_diverged);
    assert!(report.memory_alerted);
    assert!(report.no_spoiler_leak);
    assert!(report.no_response_escalated);
}

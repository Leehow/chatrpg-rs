//! Proves the probes are metric-driven, not hardcoded RED: identical structure
//! with one signal toggled flips the verdict. (蓝图 §八 Unknown≠Pass; v1
//! test_judges 的 "not a rubber stamp" 等价证明。)

use trpg_eval::{evaluate, RootCause, Transcript, Turn};

// Mirror the parser's derived fields (roll_lines from [roll]…[/roll], debt_lines
// from 待结算) so the probes see parser-shaped input.
fn turn(index: u32, action: &str, gm: &str, check: &str) -> Turn {
    let roll_lines = gm
        .split("[roll]")
        .skip(1)
        .filter_map(|s| s.split("[/roll]").next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let debt_lines = gm
        .lines()
        .filter(|l| l.contains("待结算"))
        .map(str::to_string)
        .collect();
    Turn {
        index,
        player_action: action.into(),
        gm_raw: gm.into(),
        scene_opening: gm.lines().next().unwrap_or("").into(),
        self_check: check.into(),
        roll_lines,
        debt_lines,
        ..Default::default()
    }
}

fn causes(turns: Vec<Turn>) -> std::collections::HashSet<RootCause> {
    evaluate(&Transcript { title: "t".into(), turns }).root_causes()
}

#[test]
fn player_loop_fires_on_repeat_silent_on_distinct() {
    // Same action 7 turns apart → loop.
    let looped = causes(vec![
        turn(1, "我朝门口开火压制敌人", "战斗继续，火光四溅。", ""),
        turn(8, "我朝门口开火压制敌人", "战斗继续，火光四溅。", ""),
    ]);
    assert!(looped.contains(&RootCause::PlayerActionLoop));

    // Distinct actions → no loop.
    let distinct = causes(vec![
        turn(1, "我朝门口开火压制敌人", "战斗继续，火光四溅。", ""),
        turn(8, "我撤回走廊重新装弹", "你退入走廊，弹匣咔哒入膛。", ""),
    ]);
    assert!(!distinct.contains(&RootCause::PlayerActionLoop));
}

#[test]
fn lying_health_check_fires_only_when_debt_and_claim_disagree() {
    // 待结算 in GM raw + self-check claims 无未定 → debt finding.
    let lying = causes(vec![turn(
        3,
        "我搜索线索",
        "[meta]裁定:Library Search DV13→待结算[/meta]",
        "<sub>体检：✅无未定[roll]</sub>",
    )]);
    assert!(lying.contains(&RootCause::UnresolvedMechanicalDebt));

    // Same debt but the self-check honestly does NOT claim 无未定 → no finding.
    let honest = causes(vec![turn(
        3,
        "我搜索线索",
        "[meta]裁定:Library Search DV13→待结算[/meta]",
        "<sub>体检：⏳ 有未定[roll] 待下回合结算</sub>",
    )]);
    assert!(!honest.contains(&RootCause::UnresolvedMechanicalDebt));
}

#[test]
fn success_without_information_needs_both_success_and_hollow_result() {
    let g = "[roll]侦查 1d100=10 ≤ 60，成功[/roll]\n你看清门后并没有真的有人，但确实在隐瞒什么。";
    let hollow = causes(vec![turn(2, "我侦查门后", g, "")]);
    assert!(hollow.contains(&RootCause::SuccessWithoutInformation));

    // Success with a concrete result → no finding.
    let concrete =
        "[roll]侦查 1d100=10 ≤ 60，成功[/roll]\n你看清门后有两名持枪守卫，左侧那人正在换弹。";
    let ok = causes(vec![turn(2, "我侦查门后", concrete, "")]);
    assert!(!ok.contains(&RootCause::SuccessWithoutInformation));
}

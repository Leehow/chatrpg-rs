//! GM-craft prompt overlays (EXAM_QUALITY_BAR Q-1 / Q-4 / Q-6, design-philosophy §2a.1/§2b.1/§2b.2).
//!
//! All additions are **flag-gated** behind `TRPG_GM_CRAFT` (default OFF). When OFF the
//! prompt bytes are identical to baseline (the appenders return the base string unchanged),
//! so `OFF==baseline` holds. When ON (exam runs) the craft overlays are appended to the
//! relevant system prompt:
//!   - narrator overlay → Q-1 Chinese-only output + Q-4 no option-menus / no content-dumps;
//!   - adjudicator overlay → Q-6 the GM adjudicates (referee) and does not rubber-stamp.
//!
//! The overlays are pure source-backed instruction text — no ruleset_id/module_id branching.

/// `TRPG_GM_CRAFT` is ON (`1`/`true`). Default OFF ⇒ byte-identical baseline.
pub(crate) fn enabled() -> bool {
    std::env::var("TRPG_GM_CRAFT")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Q-1 (§2a.1 single-language) + Q-4 (§2b.1 no-menu / no-dump). Appended to the
/// Narrator system prompt — the Narrator is the player-facing voice on the layered-ON path.
pub(crate) const NARRATOR_CRAFT: &str = "\
输出语言：必须全程使用简体中文叙述；即使玩家输入夹带英文，也绝不输出整句或整段英文。\n\
杜绝选项菜单与清单：绝不向玩家罗列「你可以选择 A/B/C」式备选项，也绝不把线索或发现写成编号清单（①②③）或逐条罗列；把可能性与发现编织进场景——借 NPC 的反常反应、一个神情、一处不对劲的细节去暗示，让玩家自行体会与决定（展示而非告知）。\n\
标记规范：机械数值、掷骰算式、裁定推理等「台下」信息一律不写进散文（如需保留交给系统，用 [system]…[/system] 包裹）；[roll]…[/roll] 只用于包裹一次真实的骰子检定（含点数与结果），绝不把纯叙事或资源增减塞进 [roll]。";

/// Q-6 (§2b.2 referee, not yes-man). Appended to the adjudicator/GM system prompt — this is
/// where the decision to run a check vs. just narrate, and how the world resists, is made.
pub(crate) const ADJUDICATOR_CRAFT: &str = "\
[裁判守则] 你是规则优先的公正裁判，不是顺着玩家的应声虫。玩家只声明「意图」，绝不替玩家声明「结果」。\n\
对任何不确定/对抗/有风险的行动：①先让世界按自己的逻辑回应（相关 NPC/阵营/时钟/环境据 World 层与模组自主反应，不因玩家一句话就让步）；②调用真实机械检定，依角色真实参数与真实难度/对抗来裁定（掷骰与参数说了算）；③结果可以不如玩家所愿——失败、部分成功、代价高昂的成功都合法且应当常见，干净的成功要靠挣得。绝不因玩家「声称」成功就盖章通过；绝不在没有世界阻力与规则裁定的情况下让对抗/风险行动顺玩家心意收场。琐碎、无对抗的行动不必检定，直接发生即可。";

/// Append the narrator craft overlay when `craft_on`. OFF ⇒ returns `base` unchanged (byte-equal).
pub(crate) fn narrator_system(base: String, craft_on: bool) -> String {
    if craft_on {
        format!("{base}\n{NARRATOR_CRAFT}")
    } else {
        base
    }
}

/// Append the adjudicator craft overlay when `craft_on`. OFF ⇒ returns `base` unchanged.
pub(crate) fn adjudicator_system(base: String, craft_on: bool) -> String {
    if craft_on {
        format!("{base}\n\n{ADJUDICATOR_CRAFT}")
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_is_byte_identical_baseline() {
        let base = "你是 TRPG 叙事者。".to_string();
        assert_eq!(narrator_system(base.clone(), false), base);
        assert_eq!(adjudicator_system(base.clone(), false), base);
    }

    #[test]
    fn on_appends_narrator_craft_q1_q4() {
        let out = narrator_system("BASE".to_string(), true);
        assert!(out.starts_with("BASE\n"));
        // Q-1 Chinese-only directive present.
        assert!(out.contains("必须全程使用简体中文"));
        // Q-4 no-menu / no-dump directive present.
        assert!(out.contains("杜绝选项菜单"));
        assert!(out.contains("绝不把线索或发现写成编号清单"));
    }

    #[test]
    fn on_appends_adjudicator_craft_q6() {
        let out = adjudicator_system("BASE".to_string(), true);
        assert!(out.starts_with("BASE\n\n"));
        // Q-6 referee / intent-not-outcome / world-resists.
        assert!(out.contains("裁判守则"));
        assert!(out.contains("玩家只声明「意图」"));
        assert!(out.contains("先让世界按自己的逻辑回应"));
        assert!(out.contains("结果可以不如玩家所愿"));
    }

    #[test]
    fn overlays_are_distinct() {
        assert_ne!(NARRATOR_CRAFT, ADJUDICATOR_CRAFT);
    }
}

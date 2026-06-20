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
标记规范：机械数值、掷骰算式、裁定推理等「台下」信息一律不写进散文（如需留给系统，用 [meta]…[/meta] 包裹——[meta] 玩家不可见，专放台下元信息）；[system]…[/system] 只用于玩家可见的流程/操作提示（如「请投骰」「等待回应」），绝不在其中写台下数值或推理；[roll]…[/roll] 只用于包裹一次真实的骰子检定（含点数与结果），绝不把纯叙事或资源增减塞进 [roll]。";

/// Q-6 (§2b.2 referee, not yes-man). Appended to the adjudicator/GM system prompt — this is
/// where the decision to run a check vs. just narrate, and how the world resists, is made.
/// Strengthened for Q-6 check-frequency: the GM under-fired in the v1 samples (0–1 checks /
/// 10 turns), narrating "nothing happens / you find nothing" for clearly uncertain/opposed
/// actions instead of adjudicating them. The overlay now MANDATES a real resolution-tool call
/// (`roll_check` for a GM-side system roll, or `request_player_roll` to open the gate) before
/// narrating any uncertain/opposed/risky outcome, names concrete action→check triggers, and
/// declares the silent no-op ("没发现异常 / 局面没有变化" without a check) itself a failure.
pub(crate) const ADJUDICATOR_CRAFT: &str = "\
[裁判守则] 你是规则优先的公正裁判，不是顺着玩家的应声虫。玩家只声明「意图」，绝不替玩家声明「结果」。\n\
[必须检定] 对任何不确定/对抗/有风险的行动，在叙述结果之前必须先真实掷骰裁定——调用 `roll_check`（你方系统掷骰）或 `request_player_roll`（开检定门让玩家掷，或由引擎代掷结算），依角色真实参数（见 §2c）与真实难度/对抗来定，绝不凭空叙述成败。下列一律算「需要检定」，不得跳过：\n\
- 紧张/危险场景下的察觉、观察可疑人物、留意异常（不要直接说「你没发现异常」——掷感知/侦查检定，由骰子决定发现与否及发现什么）；\n\
- 黑客入侵/技术侵入/破解终端、撬锁、潜行隐匿；\n\
- 对有自身目的之 NPC 的说服/威逼/欺骗（社交对抗，对方有意志会反制）；\n\
- 任何战斗、拔武器示威/威慑、攻击或防御动作；\n\
- 在时间压力或危险下搜查、盘问可疑交易、抢救/抢修。\n\
[严禁静默放过] 把一个不确定/对抗的行动以「你没发现异常 / 局面没有变化 / 你一无所获」之类轻描淡写收场、却没有先跑检定，这本身就是裁判失职；不确定即掷骰，让结果（含「失败地发现/没发现」）来自真实裁定而非你的回避。\n\
[世界反制] 掷骰之外，世界必须按自己的逻辑机械地反推：相关 NPC/阵营/时钟/环境据 World 层与模组自主反应（必要时推进时钟、改变状态轨、触发对方行动），不因玩家一句话就让步。\n\
[结果归属] 结果可以不如玩家所愿——失败、部分成功、代价高昂的成功都合法且应当常见（这是 TRPG 应有的检定密度），干净的成功要靠挣得。绝不因玩家「声称」成功就盖章通过。\n\
[唯一豁免] 只有真正安全、必然成功、无人对抗的琐碎行动（走进一扇没上锁的门、拿起桌上自己的杯子、平静环境下的闲聊）才免检直接发生；不要把「我懒得掷骰」伪装成「这事很琐碎」。";

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
        // A.0 cross-lane fix: 台下 meta must route to [meta] (player-invisible), NOT [system]
        // (which is now player-visible under the TurnDocument A.0 correction).
        assert!(out.contains("[meta]…[/meta]"));
        assert!(out.contains("台下元信息"));
        // [system] is mentioned only as the player-visible process-prompt channel.
        assert!(out.contains("只用于玩家可见的流程"));
    }

    #[test]
    fn on_appends_adjudicator_craft_q6() {
        let out = adjudicator_system("BASE".to_string(), true);
        assert!(out.starts_with("BASE\n\n"));
        // Q-6 referee / intent-not-outcome / world-resists.
        assert!(out.contains("裁判守则"));
        assert!(out.contains("玩家只声明「意图」"));
        assert!(out.contains("结果可以不如玩家所愿"));
    }

    #[test]
    fn on_mandates_check_with_resolution_tools_q6() {
        let out = adjudicator_system("BASE".to_string(), true);
        // Must name the real resolution tools so the model actually fires a check.
        assert!(out.contains("必须检定"));
        assert!(out.contains("roll_check"));
        assert!(out.contains("request_player_roll"));
        // Real params / real difficulty drive the dice (ties to §2c).
        assert!(out.contains("真实参数"));
    }

    #[test]
    fn on_forbids_silent_no_op_q6() {
        let out = adjudicator_system("BASE".to_string(), true);
        // The under-firing failure mode: narrating "nothing happens" without a check.
        assert!(out.contains("严禁静默放过"));
        assert!(out.contains("没发现异常"));
    }

    #[test]
    fn on_lists_concrete_check_triggers_q6() {
        let out = adjudicator_system("BASE".to_string(), true);
        // Concrete action→check examples from the under-fired v1 samples.
        assert!(out.contains("黑客入侵"));
        assert!(out.contains("拔武器"));
        assert!(out.contains("说服/威逼/欺骗"));
        assert!(out.contains("观察可疑人物"));
    }

    #[test]
    fn on_requires_world_pushback_q6() {
        let out = adjudicator_system("BASE".to_string(), true);
        assert!(out.contains("世界反制"));
        assert!(out.contains("NPC/阵营/时钟/环境"));
    }

    #[test]
    fn on_narrows_trivial_escape_q6() {
        let out = adjudicator_system("BASE".to_string(), true);
        // The escape clause is narrowed to genuinely safe/unopposed actions only.
        assert!(out.contains("唯一豁免"));
        assert!(out.contains("无人对抗的琐碎行动"));
    }

    #[test]
    fn overlays_are_distinct() {
        assert_ne!(NARRATOR_CRAFT, ADJUDICATOR_CRAFT);
    }
}

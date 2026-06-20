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
标记规范：机械数值、掷骰算式、裁定推理等「台下」信息一律不写进散文（如需留给系统，用 [meta]…[/meta] 包裹——[meta] 专放台下元信息）；[system]…[/system] 只用于玩家可见的流程/操作提示（如「请投骰」「等待回应」），绝不在其中写台下数值或推理。\n\
检定可见化（每次真实检定都必须做）：每当本回合发生一次真实的骰子检定，必须输出一个 [roll]…[/roll] 块，写明骰子算式、目标值/难度与结果（成功/失败/部分成功/大成功/大失败），例如 [roll]侦查 1d100=63 ≤ 65 通常成功[/roll]；[roll] 只包裹这一次真实检定，绝不把纯叙事、资源增减或「没有检定」的内容塞进 [roll]，也绝不输出空的 [roll]。\n\
[roll] 必须是绑定完整的真实检定（R-1，零容忍）：一个 [roll] 必须同时含有骰子算式、明确的目标值/难度（如 ≤65 或 ≥DV13）与已定的结果；严禁在 [roll] 里写「未定」「未知」「目标：?」「DV?」「结果：未定」等未绑定占位——这种未定检定一律视为无效。若此刻目标值/难度尚未绑定，就不要用 [roll] 包裹：要么从角色卡/规则/模组取到真实难度后作为一次绑定检定结算，要么改用普通散文叙述，绝不输出结果未定的 [roll]。\n\
检定必有叙事（绝不空壳）：凡有检定发生的回合，[roll] 之外必须另写真实的第二人称中文散文，把这次检定的结果作为故事呈现出来——角色此刻看到/听到/感受到什么、世界如何回应、因果如何推进；严禁只丢一行机械结果或「（机械结果）」之类占位而没有真正的情节叙述。";

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
[检定必须绑定真实目标（R-1）] 你开的每一次检定都必须绑定到真实的目标值/难度与真实参数，绝不允许出现「结果未定 / 目标未知 / DV?」之类未绑定的检定。难度来源优先级：角色卡参数 → 规则层难度表/对抗值 → 模组给定值；若一时取不到现成难度，就依规则与情境给出一个有据可循的临时难度（并说明依据），或撤回该检定改以普通叙述处理——绝不抛出一个结果未定的检定。先绑定、再掷骰、后叙述。\n\
[提高检定密度] 真实剧本里不确定/对抗/有风险的行动远多于免检琐事；过去的样本检定过疏（约 2/10），这是失职。默认倾向于「开检定」：只要行动结果对玩家有意义且非必然，就掷骰裁定，把检定密度拉到与情境风险相称的水平，而不是把大多数行动当作免检直接放行。\n\
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
    fn on_mandates_roll_block_and_real_narration_q5_q3() {
        let out = narrator_system("BASE".to_string(), true);
        // Q-5-REVISED: a real check MUST emit a [roll] block (dice + target + outcome).
        assert!(out.contains("检定可见化"));
        assert!(out.contains("必须输出一个 [roll]"));
        assert!(out.contains("目标值/难度与结果"));
        // No empty / non-roll content inside [roll].
        assert!(out.contains("绝不输出空的 [roll]"));
        // Q-3-REINFORCE: every check turn must produce real fiction; kill the placeholder stub.
        assert!(out.contains("检定必有叙事"));
        assert!(out.contains("（机械结果）"));
        assert!(out.contains("严禁只丢一行机械结果"));
    }

    #[test]
    fn on_forbids_unbound_roll_r1() {
        let out = narrator_system("BASE".to_string(), true);
        // R-1: a [roll] must be a bound real check; never 未定 / unbound target inside [roll].
        assert!(out.contains("必须是绑定完整的真实检定"));
        assert!(out.contains("未定"));
        assert!(out.contains("严禁在 [roll] 里写"));
        // If unbound: do NOT wrap in [roll] — resolve as a bound check or narrate plainly.
        assert!(out.contains("就不要用 [roll] 包裹"));
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
    fn on_binds_check_to_real_target_r1_r2() {
        let out = adjudicator_system("BASE".to_string(), true);
        // R-1 tie-in: every fired check binds to a real target/difficulty + real params; zero 未定.
        assert!(out.contains("检定必须绑定真实目标"));
        assert!(out.contains("未绑定"));
        assert!(out.contains("先绑定、再掷骰、后叙述"));
        // R-2: raise check density on uncertain/opposed/risky actions (was ~2/10).
        assert!(out.contains("提高检定密度"));
        assert!(out.contains("默认倾向于「开检定」"));
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

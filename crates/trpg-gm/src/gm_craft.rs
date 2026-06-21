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
检定数据必须忠实（R-3，零容忍）：[roll] 里的骰子算式、骰值与目标值，必须逐字采用系统在「本回合机械事实」中提供的那一行真实检定数据（形如「检定[...] 6d4=[2,2,3,2,1,1] 目标:面值≥3 需≥1个，结果:成功」）——直接照搬其中的骰子算式（如 6d4）、骰值与目标，绝不自行编造或按看到的点数反推骰型（严禁把 6d4 写成 6d6/6d3/6d?，也严禁把不存在的检定凭空写成 [roll]）。系统没有给出某次检定的真实数据，就不要为它编一个 [roll]。\n\
检定必有叙事（绝不空壳）：凡有检定发生的回合，[roll] 之外必须另写真实的第二人称中文散文，把这次检定的结果作为故事呈现出来——角色此刻看到/听到/感受到什么、世界如何回应、因果如何推进；严禁只丢一行机械结果或「（机械结果）」之类占位而没有真正的情节叙述。\n\
NPC 台词标记（[dialogue]）：当在场且玩家可见的 NPC 真正开口说话时，把这句台词用 [dialogue actor=\"NPC的名字或称谓\"]……[/dialogue] 包裹（actor 取该角色在故事里的名字或身份，例如「郊狼麦克」「酒保」「警员」）；台词本身仍是自然口语、随情境流动，不要因加了标记就变成一问一答的机械对白；没有人真正说话的回合就不要硬塞 [dialogue]。\n\
可选行动提示（[choice]，须极克制）：仅当本回合自然收束于一个真实、当下、二到三选一级别的关键抉择点（如：是否冒险一搏、走哪条路、是否当面摊牌）时，才可在散文之后附最多 2-3 个 [choice]……[/choice] 作为【可选】提示；这绝不是强制菜单——玩家永远可以无视它自由行动，你也绝不能因此停止用散文把世界推进下去。严禁用 [choice] 罗列线索、把调查/探索拆成选单、或在琐碎回合给选项（那会退化成菜单，违反「展示而非告知」）。";

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
[唯一豁免] 只有真正安全、必然成功、无人对抗的琐碎行动（走进一扇没上锁的门、拿起桌上自己的杯子、平静环境下的闲聊）才免检直接发生；不要把「我懒得掷骰」伪装成「这事很琐碎」。\n\
[台下隐藏信息（[hide]）——本回合若满足触发条件就必须写] 对玩家此刻【尚不可见】的台下事实用 [hide kind=\"暗骰|npc_action|secret\"]……[/hide] 记录。下列任一发生时，必须各留至少一个对应 [hide]：① 本回合有对抗/暗骰（对方或环境的私下掷骰、防守方对抗结果的内部细节）→ [hide kind=\"暗骰\"]；② 镜头外或玩家未注意到的 NPC/阵营/时钟在私下行动、换位、集结、推进 → [hide kind=\"npc_action\"]；③ 存在玩家尚未察觉的秘密线索/真相/状态变化 → [hide kind=\"secret\"]。[hide] 是写给战役 canon 的【提案】(authority=Proposal)，绝不写进玩家可见散文，也绝不替系统提交状态（状态只由 Kernel 提交）。玩家已经亲眼看到/亲耳听到的内容不要塞进 [hide]。\n\
[台下决策摘要（[meta]）——每个有裁定的回合必须写一条] 只要本回合发生了检定/对抗/重要 GM 决策，就必须用 [meta kind=\"decision_summary\"]……[/meta] 留一句台下摘要：写明你为何开（或不开）检定、用了哪条难度/对抗值依据、世界为何这样反制。[meta] 绝不出现在玩家可见散文里，只走台下审计通道。\n\
[台下标记位置] [hide] 与 [meta] 写在你这一轮台下输出的末尾即可，一律使用上面的尖括号标记包裹，不要用其它格式。";

/// Phase B (OB-hide / OB-meta): extract the RAW `[hide…]…[/hide]` and `[meta…]…[/meta]` blocks
/// (verbatim, including any `kind="…"` attribute) from the adjudicator (台下) prose, so they
/// survive the split-mode narrator replacement of `visible_text`. Returns the concatenation, or
/// an empty string when none are present. The caller is `TRPG_GM_CRAFT`-gated and only invokes
/// this on the split-ON path, so OFF==baseline holds (this never runs on the OFF byte-equal path).
pub(crate) fn extract_offstage_blocks(text: &str) -> String {
    let mut out = String::new();
    for tag in ["hide", "meta"] {
        let open_prefix = format!("[{tag}");
        let close = format!("[/{tag}]");
        let mut from = 0usize;
        while let Some(rel) = text[from..].find(&open_prefix) {
            let start = from + rel;
            let next = text[start + open_prefix.len()..].chars().next();
            // a genuine open tag is `[hide]` or `[hide ...]` — next char is ']' or whitespace.
            if !matches!(next, Some(']') | Some(' ') | Some('\t')) {
                from = start + open_prefix.len();
                continue;
            }
            match text[start..].find(&close) {
                Some(crel) => {
                    let end = start + crel + close.len();
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(&text[start..end]);
                    from = end;
                }
                None => break, // unterminated → leave it (treated as literal upstream)
            }
        }
    }
    out
}

/// Phase B (OB-meta / OB-hide), deterministic floor: synthesize the off-stage
/// `[meta kind="decision_summary"]` audit line and `[hide kind="暗骰"]` private-roll records
/// straight from the REAL mechanical turn data (the ledger snapshot of committed checks/rolls),
/// so these channels emit TRUTHFULLY every adjudicated turn even when the adjudicator LLM omits
/// the tags. Engine-authored from committed facts ⇒ zero invention (constitution: structured
/// facts come from the kernel/ledger, never narrated guesswork). Returns "" when the turn had no
/// adjudicated check. The caller is `TRPG_GM_CRAFT`-gated and split-ON only ⇒ OFF==baseline holds.
pub(crate) fn synthesize_offstage_from_ledger(
    snap: &trpg_agent::TurnLedgerSnapshot,
) -> String {
    use trpg_model::RollVisibility;
    let mut out = String::new();
    if snap.check_results.is_empty() {
        return out; // no committed check this turn ⇒ nothing truthful to record
    }
    // [meta] decision summary — one line per committed check (label + expression + outcome band).
    let mut summaries = Vec::new();
    // [hide kind="暗骰"] candidates harvested from the committed opposed outcomes (the defender's
    // private roll lives in outcome.opposed, never in the player-visible dice_rolls vec).
    let mut opposed_hides = Vec::new();
    for r in &snap.check_results {
        let oc = &r.outcome;
        let label = oc
            .get("check_label")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(r.check_id.as_str());
        let expr = r.roll.expression.trim();
        let band = oc
            .get("degree")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                oc.get("success")
                    .and_then(|v| v.as_bool())
                    .map(|b| if b { "成功".into() } else { "失败".into() })
            })
            .unwrap_or_else(|| "待结算".into());
        summaries.push(format!("{label}({expr})→{band}"));
        // opposed defender roll is private (台下) by default — surface it as a truthful [hide 暗骰].
        if let Some(op) = oc.get("opposed") {
            let hidden = oc
                .get("resolution_model")
                .and_then(|m| m.get("defender_roll_visibility"))
                .and_then(|v| v.as_str())
                .map(|v| v != "public_gm_roll")
                .unwrap_or(true);
            if hidden {
                let who = op
                    .get("defender_actor_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("对手");
                let dexpr = oc
                    .get("resolution_model")
                    .and_then(|m| m.get("defender_expression"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let dval = op
                    .get("defender_value")
                    .and_then(|v| v.as_i64())
                    .map(|n| n.to_string())
                    .unwrap_or_default();
                let winner = op.get("winner").and_then(|v| v.as_str()).unwrap_or("");
                opposed_hides.push(format!(
                    "{who} 台下对抗私骰 {dexpr}={dval}（玩家不可见），对抗胜方:{winner}"
                ));
            }
        }
    }
    if !summaries.is_empty() {
        out.push_str(&format!(
            "[meta kind=\"decision_summary\"]本回合裁定:{}[/meta]",
            summaries.join("；")
        ));
    }
    for h in opposed_hides {
        out.push_str(&format!("\n[hide kind=\"暗骰\"]{h}[/hide]"));
    }
    // Additional [hide kind="暗骰"] — any private/passive roll already present in the dice_rolls
    // ledger (genuine off-screen die, not invention). One [hide] per hidden roll.
    for d in &snap.dice_rolls {
        if matches!(
            d.visibility,
            RollVisibility::PublicGmRoll | RollVisibility::PlayerRollRequired
        ) {
            continue; // player already saw it
        }
        let total = d
            .result
            .get("total")
            .and_then(|v| v.as_i64())
            .map(|t| t.to_string())
            .unwrap_or_default();
        let who = match d.roller_kind {
            trpg_model::ActorKind::PlayerCharacter => "玩家角色",
            trpg_model::ActorKind::Npc => "NPC/对手",
            _ => "环境/系统",
        };
        let expr = d.expression.trim();
        let tail = if total.is_empty() {
            String::new()
        } else {
            format!("={total}")
        };
        out.push_str(&format!(
            "\n[hide kind=\"暗骰\"]{who}台下私骰 {expr}{tail}（玩家不可见）[/hide]"
        ));
    }
    out
}

/// L-J 防回合内复述/单步推进:观测到 Narrator 在**同一回合内**把同一段经过(到达/移动/发讯/
/// 敲门)反复描述 2–3 遍 + 偶发时间倒退 ⇒ player-sim 判位置/连续性 AMNESIA。此 flag 开时给
/// Narrator overlay 追加「向前讲一次、禁回合内复述、时间单向」纪律。**默认 ON**(eval 不设 ⇒ 自动吃到),
/// 仅显式关值 OFF ⇒ 与既有 NARRATOR_CRAFT 字节等价。零 ruleset 分支。
pub(crate) fn narrator_single_advance_enabled() -> bool {
    !matches!(
        std::env::var("TRPG_NARRATOR_SINGLE_ADVANCE")
            .ok()
            .map(|v| v.to_ascii_lowercase())
            .as_deref(),
        Some("0") | Some("false") | Some("off") | Some("no")
    )
}

/// L-J 单步推进纪律(追加在 NARRATOR_CRAFT 之后)。直击观测失败模式:回合内同一段经过被讲 2–3 遍。
pub(crate) const NARRATOR_SINGLE_ADVANCE: &str = "\
【单步推进 · 禁回合内复述】本回合只把玩家声明的动作与世界的回应**向前讲一次**：\n\
- 凡你在**本回合内**已经叙述过的动作、到达、移动、发讯、敲门、检定结果等经过，绝不在同一回合里再描述第二遍，也绝不把玩家拉回去把它重做一遍；\n\
- 从「连续性锚」给出的既成局面**向前**续写，不要回头重新铺陈已经发生过的事，叙述要收束于一个清晰的当前结果；\n\
- 一次只解决玩家本回合真正声明的动作及其直接后果，不要为凑篇幅而循环复述同一段经过；\n\
- 时间线只向前推进：绝不把场景时间倒回更早的时段(如已是夜晚就不要写回傍晚)。";

/// L-N 收束于既成结果 · 不悬置(理念§3 no-hollow):smokeGND turn1 实证,Narrator 把一回合收尾在
/// 「没有任何资源被实际消耗 / 局势只是更紧张 / 一切悬而未决」的悬置态(空壳非推进),该编辑性负面断言
/// 被 player-sim 当作既成事实记入 facts,下一回合玩家真正行动即与之矛盾 ⇒ 误判 AMNESIA(turn2)。此 flag
/// 开时给 Narrator overlay 追加「每回合收束在真实发生了的具体当前结果上、不以悬置语收尾」纪律(不强令捏造
/// 消耗/后果——没发生的别写,只是落在真实发生的那点变化上)。**默认 ON**,OFF 字节等价,零 ruleset 分支。
pub(crate) fn narrator_no_hollow_suspension_enabled() -> bool {
    !matches!(
        std::env::var("TRPG_NARRATOR_NO_HOLLOW_SUSPENSION")
            .ok()
            .map(|v| v.to_ascii_lowercase())
            .as_deref(),
        Some("0") | Some("false") | Some("off") | Some("no")
    )
}

/// L-N 不悬置纪律(追加在 NARRATOR_SINGLE_ADVANCE 之后)。直击 smokeGND turn1 悬置收尾→turn2 误判失忆。
pub(crate) const NARRATOR_NO_SUSPENSION: &str = "\
【收束于既成结果 · 不悬置】本回合叙述必须收束在一个**本回合真实发生了的、具体可感的当前结果或既成改变**上(谁做了什么、世界如何回应、此刻局面变成了什么样)；绝不以「本回合什么都还没真正发生 / 没有任何资源被实际消耗 / 没有新的变化 / 局势只是更紧张 / 一切悬而未决」这类**无实质推进的悬置语**收尾，也不要为强调'尚未'而把本回合未发生的状态写成一条断言式事实(那会被后续回合当作既成事实而自相矛盾)。这不是要你凭空捏造消耗或后果——没发生的别写——而是把本回合**确实发生了的**那一点变化讲清、讲实，落地为当前局面，而不是停在'还没发生'的真空里。";

/// L-O 叙述地点忠实(理念§二.8 Narrator 表达已决定的、不发明事实):smokeGND3 turn8 实证,即便委派
/// 场景已由 content-gravity 合法转入 `scene_warehouse_hacking`(committed,转移 reason 明记「角色仍在掩体后」=
/// 仍在仓库外墙缆线处),Narrator 仍把玩家**凭空写进一个既成场景未确立的内部空间**(「狭窄的设备间里」)⇒
/// player-sim 判位置 AMNESIA(与既定地点不符)。此 flag 开时给 Narrator overlay 追加「只渲染当前 committed
/// 场景 + 连续性锚确立的地点,绝不发明玩家并未进入的新房间/内部/区域」纪律——合法场景转移由 committed 状态
/// 带来,Narrator 据之渲染新地点,但不得在叙述里自行把玩家挪进未确立的空间。**默认 ON**,OFF 字节等价,零 ruleset 分支。
pub(crate) fn narrator_location_fidelity_enabled() -> bool {
    !matches!(
        std::env::var("TRPG_NARRATOR_LOCATION_FIDELITY")
            .ok()
            .map(|v| v.to_ascii_lowercase())
            .as_deref(),
        Some("0") | Some("false") | Some("off") | Some("no")
    )
}

/// L-O 地点忠实纪律(追加在 NARRATOR_NO_SUSPENSION 之后)。直击 smokeGND3 turn8:叙述把玩家凭空写进设备间。
pub(crate) const NARRATOR_LOCATION_FIDELITY: &str = "\
【地点忠实 · 只渲染既定场景】本回合叙述里玩家所处的地点、空间与周遭环境,必须忠于「连续性锚」与当前 committed 场景所确立的所在(玩家此刻在哪、在什么掩护/位置)。绝不在散文里凭空把玩家**写进一个既定场景尚未确立的新房间、内部空间、设备间、走廊或区域**(例如既定他仍在仓库外墙掩体后,就不要叙述成他已身处「狭窄的设备间里」);也不要因为剧情想推进就让叙述擅自把玩家带进/穿过一道门或一处内部。地点的改变只能来自既成的场景转移——当场景确实已转入新地点时按新地点渲染,否则一律把玩家留在其既定所在处续写。";

/// L-Q NPC 既定台词忠实(理念§二.8 Narrator 表达已决定的、不发明/不改写既成事实):smokeq3(L-P 引擎开场投递
/// 后)实证,player-sim 拿到权威开场 NPC 台词后,Narrator 在后续回合**复述同一句既定台词时改写其措辞或身份指称**
/// (t3「你听见长官怎么说了」→「你听见局长怎么说了」改身份指称;t4「弗兰基,滚出去!」→「弗兰基!滚出来!」改措辞)
/// ⇒ player-sim 判 AMNESIA(与既定台词正面不一致)。这是 L-O 地点忠实在**口语台词**维度的同构残面——
/// 既定 NPC 台词与其说话者身份是已确立事实,复述须忠实引用,不得改写;推进只能靠让 NPC 说**新台词**。
/// **默认 ON**(eval 不设 ⇒ 自动吃到),OFF 字节等价,零 ruleset 分支。
pub(crate) fn narrator_dialogue_fidelity_enabled() -> bool {
    !matches!(
        std::env::var("TRPG_NARRATOR_DIALOGUE_FIDELITY")
            .ok()
            .map(|v| v.to_ascii_lowercase())
            .as_deref(),
        Some("0") | Some("false") | Some("off") | Some("no")
    )
}

/// L-Q 台词忠实纪律(追加在 NARRATOR_LOCATION_FIDELITY 之后)。直击 smokeq3 t3/t4:复述既定 NPC 喊话时改写措辞/指称。
pub(crate) const NARRATOR_DIALOGUE_FIDELITY: &str = "\
【台词忠实 · 既定台词与说话者不改写】当本回合需要**回指或复述一句此前已经说出口的 NPC 台词**(开场念白里 NPC 喊的话、连续性锚或前文已确立的对白)时,必须**忠实保留其原措辞与说话者的身份指称**——既定的称谓、用词、语气不得在复述里被换成近义的另一种说法(例如既定喊的是「长官」就不要复述成「局长」,既定是「滚出去!」就不要改写成「滚出来!」),也不要把同一个说话者改换成另一身份,**也不要在复述时把这句话原本喊向的对象改成另一个对象**(既定是喊给玩家的警告就不要改写成对火线里另一个人的嘶吼)。已说出的台词、其说话者与其喊话对象都是**已确立的事实**,与既定地点同样不可在叙述里被悄悄改写。需要推进对话时,让 NPC 说**一句新的台词**,而不是把那句旧台词重写成不同的版本。";

/// L-S 既成状态变更持久 · 不复活已终止的世界状态(理念§二.8 Narrator 表达已决定的、§二.3 Projection 是视图不是第二真相):
/// aliveFull30v2 turn10 实证——玩家在 turn~8 切断喂向无人机的供电、已确立事实记「供电已被切断,粗缆震动和低频嗡鸣停止」,
/// 但 Narrator 在 turn10 把同一根粗缆重新描述成「低沉的电流嗡鸣贴着货架和地面传过来」⇒ 把一个**已被既成事实确立为终止**
/// 的世界状态(通电/嗡鸣)在叙述里复活,正面矛盾既成事实 ⇒ player-sim 判 AMNESIA。根因=场景的环境定场框架(嗡鸣的活缆)
/// 在玩家造成的状态变更之上**惯性复投**。这是 L-O 地点忠实在**世界/环境状态变更**维度的同构残面:committed 既成事实里
/// 一旦记录某状态发生了**变更/终止**(供电切断→静默、灯熄、门开、火灭、嗡鸣/震动停止、设备关停、警报消音),Narrator 须按
/// 其**新状态**续写,绝不把它的**变更前旧状态**当作仍在持续而复活。**默认 ON**(eval 不设 ⇒ 自动吃到),OFF 字节等价,零 ruleset 分支。
pub(crate) fn narrator_state_persistence_enabled() -> bool {
    !matches!(
        std::env::var("TRPG_NARRATOR_STATE_PERSISTENCE")
            .ok()
            .map(|v| v.to_ascii_lowercase())
            .as_deref(),
        Some("0") | Some("false") | Some("off") | Some("no")
    )
}

/// L-S 既成状态持久纪律(追加在 NARRATOR_DIALOGUE_FIDELITY 之后)。**双向**:turn10 已切断的供电嗡鸣被叙述复活(终止态复活);
/// turn13 仍在发热的粗缆被叙述成「硬冷」(持续态被反转)。两向同根——叙述把既成世界状态改写成与既成事实不符的样子。
pub(crate) const NARRATOR_STATE_PERSISTENCE: &str = "\
【既成状态忠实 · 持久不复活、不反转】本回合叙述里任何已被「连续性锚」或「本回合机械事实」确立的世界/环境/物体状态——它的有无、开关、通断、冷热温度、运转/停止、明灭、损坏/完好、在场/离场——都必须与既成事实**保持一致**,不得在叙述里被悄悄改写成不符的样子。这有**两个方向**都要守:① **不把已完成的变更倒回旧状态**:既成事实记录某状态已**变更/终止**(供电切断、电流/嗡鸣/震动停止、灯熄、火灭、门开、设备关停、警报消音、某物被破坏移除)后,就按其**新状态**续写,绝不再把它**变更前的旧状态**当作仍在持续而复活(既定「供电已切断、粗缆嗡鸣停止」后就不要再写那根缆'仍有低沉电流嗡鸣传来')。② **不把仍持续的既成状态反转**:既成事实确立某状态**仍在持续**(粗缆仍在发热、设备仍在运转、某门仍开着)时,就不要在叙述里把它写成它的反面(既定粗缆'仍从设备暗处带出热量、正在发热'就不要写成'隔着布料仍能感觉到外皮硬冷')。残留的**物理痕迹**可以如实描写(切断后的缆线仍有余温、焦味、断口),但不要借此把**已终止的活动状态**(通电、运转、发声)写成死灰复燃,也不要拿它否定一个**仍持续的状态**。若确有一个**另外的、新的**来源产生类似现象(更深处另有一台仍通电的设备),须明确叙述为一个**区别于既成那个状态的新来源**,而非让旧状态本身复活或反转;且不得为此凭空捏造既成事实之外的来源。世界状态只朝既成事实记录的方向走,绝不在叙述里被无声地倒回或反转。";

/// L-X 不复述场景定场 · 从玩家当前所在处续写(理念§二.8 Narrator 表达已决定不发明、§4 把冲突带到玩家所在处不挪玩家):
/// aliveFull30LW full-run census 实证(t9–t12,**同一 J1 子类连续 4 次硬位置失忆**)——玩家 t5–t8 已自述钻入仓库通风道、
/// 沿缆线向**仓库内部**潜行(既成当前位置=通风道内),但 Narrator 在 t9 起**每回合以场景到达定场框架重新开篇**
/// (「警笛把你引到这座小仓库外」「警车横在建筑两侧」=入口场景 `scene_warehouse_arrival` 的**到达/外景定场图景**),
/// 把玩家从既成的通风道内位置**逐回合冲刷回仓库外/门口**,直到 t13 player-sim 自身也被带回门口。根因=连续性锚的权威内容
/// 是上两回合 GM 散文(lib.rs:1003),而 GM 习惯于每回合用场景到达定场图景开篇 ⇒ 该外景框架被当作"当前局面"逐回合累积、
/// 压过玩家已确立的内部潜入。这是 L-O 地点忠实在**"每回合重painting场景到达定场框架"**维度的同构残面——L-O 治"凭空写进新房间",
/// L-X 治"把开篇重置回场景的到达/入口定场图景"。**默认 ON**(eval 不设 ⇒ 自动吃到),OFF 字节等价,零 ruleset 分支。
pub(crate) fn narrator_no_scene_reestablish_enabled() -> bool {
    !matches!(
        std::env::var("TRPG_NARRATOR_NO_SCENE_REESTABLISH")
            .ok()
            .map(|v| v.to_ascii_lowercase())
            .as_deref(),
        Some("0") | Some("false") | Some("off") | Some("no")
    )
}

/// L-X 不复述场景定场纪律(追加在 NARRATOR_STATE_PERSISTENCE 之后)。直击 aliveFull30LW t9–t12:每回合用入口场景外景到达图景开篇,把玩家冲刷回仓库外。
pub(crate) const NARRATOR_NO_SCENE_REESTABLISH: &str = "\
【不复述场景定场 · 从玩家当前所在处续写】场景的**到达/入口定场图景**(玩家如何抵达此地、初见此地的那幅固定外景/环境画面——例如「警笛把你引到小仓库外、警车横在建筑两侧、地上倒着人影」这类**首次进入时**铺陈的定场描写)**只在玩家首次进入该场景时铺陈一次**,绝不在其后的每个回合开篇被重新painting。本回合的叙述必须**从「连续性锚」与玩家自己最近一次声明所确立的当前所在位置继续向前**,而不是把开篇重置回场景的到达/入口图景。具体:① 若玩家已自述移动到场景内部更深处(钻进通风道、进入内室、绕到建筑另一侧、深入走廊),就以其**当前的内部/纵深位置**为准续写,绝不在开篇又把镜头拉回入口外景、把玩家写成仍在/又回到入口或建筑外;② 入口处的固定景物(门外的警车、外墙、街道、到场时的人影)不要每回合重复铺陈成「此刻就在眼前」——除非玩家本回合确实回到了那个位置;③ 推进氛围靠描写玩家**当前所在处**此刻的新感官与新变化,而不是靠复述那幅已交付过的到达定场图景。一句话:开场定场只念一次,之后每回合都从玩家此刻真正所在的地方往前讲。";

/// Append the narrator craft overlay when `craft_on`. OFF ⇒ returns `base` unchanged (byte-equal).
/// 全部追加 flag 关时 ⇒ `base\n{NARRATOR_CRAFT}`(历史基线字节等价)。
pub(crate) fn narrator_system(base: String, craft_on: bool) -> String {
    if !craft_on {
        return base;
    }
    let mut out = format!("{base}\n{NARRATOR_CRAFT}");
    if narrator_single_advance_enabled() {
        out.push('\n');
        out.push_str(NARRATOR_SINGLE_ADVANCE);
    }
    if narrator_no_hollow_suspension_enabled() {
        out.push('\n');
        out.push_str(NARRATOR_NO_SUSPENSION);
    }
    if narrator_location_fidelity_enabled() {
        out.push('\n');
        out.push_str(NARRATOR_LOCATION_FIDELITY);
    }
    if narrator_dialogue_fidelity_enabled() {
        out.push('\n');
        out.push_str(NARRATOR_DIALOGUE_FIDELITY);
    }
    if narrator_state_persistence_enabled() {
        out.push('\n');
        out.push_str(NARRATOR_STATE_PERSISTENCE);
    }
    if narrator_no_scene_reestablish_enabled() {
        out.push('\n');
        out.push_str(NARRATOR_NO_SCENE_REESTABLISH);
    }
    out
}

/// L-K 玩家位置主权(§4 relocation-toward-player,GM 核心守则层):观测到即便有连续性锚 + L-I
/// 场景降格两道 §4 软指令,GM 仍会在单回合里**替玩家移动**到模组核心冲突处(smoke8 turn3:玩家在
/// 老陈家门前等待,GM 擅自叙述其下楼潜行靠近仓库交火)⇒ 位置失忆 + 下游连锁。此守则把「玩家只声明
/// 意图、GM 绝不替玩家声明结果」扩展到**移动/位置**:玩家选择留在原地就留在原地;beat 在别处则按§4
/// 把压力搬到玩家处,不把玩家搬到 beat 处。**默认 ON**(eval 不设 ⇒ 自动吃到),OFF 字节等价。零 ruleset 分支。
pub(crate) fn gm_no_relocate_player_enabled() -> bool {
    !matches!(
        std::env::var("TRPG_GM_NO_RELOCATE_PLAYER")
            .ok()
            .map(|v| v.to_ascii_lowercase())
            .as_deref(),
        Some("0") | Some("false") | Some("off") | Some("no")
    )
}

/// L-K 守则(追加在 ADJUDICATOR_CRAFT 之后)。直击 smoke8 turn3:GM 替玩家移动到仓库交火处。
pub(crate) const ADJUDICATOR_NO_RELOCATE: &str = "\
[玩家位置主权 · §4] 「绝不替玩家声明结果」同样适用于**移动与位置**:玩家声明意图，你绝不替他声明未经其声明的移动、潜行、前往或位置改变。玩家选择留在原地、观察、等待或停在某门前时，就让他**留在其当前所在处**，绝不擅自把他下楼、绕路、靠近或瞬移到剧情冲突/核心 beat 的发生地。若模组核心 beat 或冲突在别处而本回合需要推进，按§4 让那股压力**主动波及玩家当前所在处**（消息、声响、NPC 闯入、势力或时钟外溢到此地）——把 beat 搬到玩家处，绝不把玩家搬到 beat 处。";

/// L-N 物体/世界状态主权(理念§6 裁判非应声虫 + §二.9 Policy 可拒绝修复不造新事实):smokeGND 实证,
/// 即便位置不再漂移(L-K 生效),GM 仍会**默认采纳玩家声明里预设的、与既成事实矛盾的物体/世界状态**——
/// 把已被甩开的干扰器当作还握在手里(turn3)、把仅刮出浅痕未断的缆线当作已露线芯(turn4)⇒ player-sim 判
/// AMNESIA(GM 矛盾既成事实)。此守则把「绝不替玩家声明结果」从位置(L-K)扩展到**物体/世界状态**:以
/// 既成事实(连续性锚 + 本回合机械事实)为权威,据之裁定玩家的错误前提,绝不凭措辞径直翻转世界状态。
/// **默认 ON**(eval 不设 ⇒ 自动吃到),OFF 字节等价,零 ruleset 分支。
pub(crate) fn gm_object_state_authority_enabled() -> bool {
    !matches!(
        std::env::var("TRPG_GM_OBJECT_STATE_AUTHORITY")
            .ok()
            .map(|v| v.to_ascii_lowercase())
            .as_deref(),
        Some("0") | Some("false") | Some("off") | Some("no")
    )
}

/// L-N 守则(追加在 ADJUDICATOR_NO_RELOCATE 之后)。直击 smokeGND turn3/turn4:GM 采纳玩家虚构的物体状态。
/// **L-N+(smokeGND2 turn3 残留扩面)**:同根 class 的子case——玩家预设「既成事实尚未确认存在」的地点特征
/// (入口/侧门/线缆接点)为已存在并径直通过(established「未确认入口」⇒ player「滑到最近的侧门」⇒ GM 坐实)。
/// 同 flag/同 overlay 扩词覆盖,additive,OFF 仍字节等价。
pub(crate) const ADJUDICATOR_OBJECT_STATE: &str = "\
[物体/世界状态主权 · §6] 「绝不替玩家声明结果」同样适用于**物体与世界的状态、以及地点特征是否存在**:以「连续性锚」与「本回合机械事实」中既成的物体/世界/场景状态为唯一权威。当玩家本回合的声明**预设了一个与既成事实矛盾、或既成事实尚未确认的状态/存在**——例如把已被丢出/甩开/脱手的工具当作还握在手里、把仅被刮伤/未断/未开启/未损坏的东西当作已断/已破/已开/已露芯、把一次尚未成功的结果当作已经达成、**或把一个既成事实尚未确认存在的入口/出口/侧门/通道/线缆接点或其他地点特征当作已经存在·已被找到·可直接通过（例如既成事实仅记「未确认入口」时玩家径直「滑到最近的侧门」并摸索门缝）**——你绝不默认采纳这个虚构前提，也绝不因玩家这样指称就把它确认为既成存在。要么据既成事实当场纠正它（指出工具已不在手、缆线仍未断、该侧门尚未被发现，玩家须先重新够到、重做或先去搜寻确认），要么把玩家的真实**意图**当作一次按既成状态进行的**新尝试**——包括一次去寻找/确认该地点特征是否存在的探查——其有无与成败由你依既成事实裁定；绝不因玩家这样措辞就把世界状态径直翻转、或把未确认的特征凭空坐实。你只依据 Kernel 既成事实推进，不创造玩家凭空声称的状态改变或地点特征。";

/// Append the adjudicator craft overlay when `craft_on`. OFF ⇒ returns `base` unchanged.
/// 全部追加 flag 关时 ⇒ `base\n\n{ADJUDICATOR_CRAFT}`(历史基线字节等价)。
pub(crate) fn adjudicator_system(base: String, craft_on: bool) -> String {
    if !craft_on {
        return base;
    }
    let mut out = format!("{base}\n\n{ADJUDICATOR_CRAFT}");
    if gm_no_relocate_player_enabled() {
        out.push('\n');
        out.push_str(ADJUDICATOR_NO_RELOCATE);
    }
    if gm_object_state_authority_enabled() {
        out.push('\n');
        out.push_str(ADJUDICATOR_OBJECT_STATE);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These overlay tests mutate process-global env vars; serialize them so parallel
    /// threads don't race the same flag (poison-tolerant). Byte-equal baselines depend
    /// on no other test toggling a sibling flag mid-assertion.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn narrator_single_advance_flag_and_overlay() {
        let _g = env_guard();
        let prev = std::env::var("TRPG_NARRATOR_SINGLE_ADVANCE").ok();
        let prev_ns = std::env::var("TRPG_NARRATOR_NO_HOLLOW_SUSPENSION").ok();
        let prev_lf = std::env::var("TRPG_NARRATOR_LOCATION_FIDELITY").ok();
        let prev_df = std::env::var("TRPG_NARRATOR_DIALOGUE_FIDELITY").ok();
        let prev_sp = std::env::var("TRPG_NARRATOR_STATE_PERSISTENCE").ok();
        let prev_re = std::env::var("TRPG_NARRATOR_NO_SCENE_REESTABLISH").ok();
        // 默认 ON(未设)⇒ craft_on 路径在 NARRATOR_CRAFT 之后追加单步推进纪律。
        std::env::remove_var("TRPG_NARRATOR_SINGLE_ADVANCE");
        assert!(narrator_single_advance_enabled(), "未设 ⇒ 默认 ON");
        let on = narrator_system("BASE".to_string(), true);
        assert!(on.contains("单步推进 · 禁回合内复述"), "ON 应追加单步推进纪律: {on}");
        assert!(on.contains("不要回头重新铺陈"), "ON 应含禁复述子句");
        assert!(on.contains("时间线只向前推进"), "ON 应含时间单向子句");
        assert!(on.contains(NARRATOR_CRAFT), "ON 仍保留既有 NARRATOR_CRAFT");

        // 显式 OFF(全部追加 flag 关)⇒ 与既有 NARRATOR_CRAFT 字节等价(不追加任何 overlay)。
        std::env::set_var("TRPG_NARRATOR_SINGLE_ADVANCE", "off");
        std::env::set_var("TRPG_NARRATOR_NO_HOLLOW_SUSPENSION", "off");
        std::env::set_var("TRPG_NARRATOR_LOCATION_FIDELITY", "off");
        std::env::set_var("TRPG_NARRATOR_DIALOGUE_FIDELITY", "off");
        std::env::set_var("TRPG_NARRATOR_STATE_PERSISTENCE", "off");
        std::env::set_var("TRPG_NARRATOR_NO_SCENE_REESTABLISH", "off");
        assert!(!narrator_single_advance_enabled(), "off ⇒ OFF");
        let off = narrator_system("BASE".to_string(), true);
        assert_eq!(off, format!("BASE\n{NARRATOR_CRAFT}"), "OFF 须与历史 craft overlay 字节等价");
        assert!(!off.contains("单步推进 · 禁回合内复述"), "OFF 不得追加单步推进");

        // craft_on=false ⇒ 永远 base(我 flag 不触此路径)。
        std::env::remove_var("TRPG_NARRATOR_SINGLE_ADVANCE");
        assert_eq!(narrator_system("B2".to_string(), false), "B2", "craft OFF 永远 base");

        match prev {
            Some(v) => std::env::set_var("TRPG_NARRATOR_SINGLE_ADVANCE", v),
            None => std::env::remove_var("TRPG_NARRATOR_SINGLE_ADVANCE"),
        }
        match prev_ns {
            Some(v) => std::env::set_var("TRPG_NARRATOR_NO_HOLLOW_SUSPENSION", v),
            None => std::env::remove_var("TRPG_NARRATOR_NO_HOLLOW_SUSPENSION"),
        }
        match prev_lf {
            Some(v) => std::env::set_var("TRPG_NARRATOR_LOCATION_FIDELITY", v),
            None => std::env::remove_var("TRPG_NARRATOR_LOCATION_FIDELITY"),
        }
        match prev_df {
            Some(v) => std::env::set_var("TRPG_NARRATOR_DIALOGUE_FIDELITY", v),
            None => std::env::remove_var("TRPG_NARRATOR_DIALOGUE_FIDELITY"),
        }
        match prev_sp {
            Some(v) => std::env::set_var("TRPG_NARRATOR_STATE_PERSISTENCE", v),
            None => std::env::remove_var("TRPG_NARRATOR_STATE_PERSISTENCE"),
        }
        match prev_re {
            Some(v) => std::env::set_var("TRPG_NARRATOR_NO_SCENE_REESTABLISH", v),
            None => std::env::remove_var("TRPG_NARRATOR_NO_SCENE_REESTABLISH"),
        }
    }

    #[test]
    fn narrator_no_scene_reestablish_flag_and_overlay() {
        let _g = env_guard();
        let prev = std::env::var("TRPG_NARRATOR_NO_SCENE_REESTABLISH").ok();
        std::env::remove_var("TRPG_NARRATOR_NO_SCENE_REESTABLISH");
        assert!(narrator_no_scene_reestablish_enabled(), "未设 ⇒ 默认 ON");
        let on = narrator_system("BASE".to_string(), true);
        assert!(on.contains("不复述场景定场 · 从玩家当前所在处续写"), "ON 应追加不复述场景定场纪律: {on}");
        assert!(
            on.contains("只在玩家首次进入该场景时铺陈一次"),
            "ON 应含'到达定场只念一次'子句(直击每回合重painting开篇)"
        );
        assert!(
            on.contains("以其**当前的内部/纵深位置**为准续写"),
            "ON 应含'玩家已深入内部则按内部续写'子句(直击 t9–t12 把玩家冲刷回仓库外)"
        );
        assert!(
            on.contains("推进氛围靠描写玩家**当前所在处**此刻的新感官与新变化"),
            "ON 应含'靠当前位置新感官推进而非复述到达图景'子句(不伤氛围推进)"
        );

        std::env::set_var("TRPG_NARRATOR_NO_SCENE_REESTABLISH", "off");
        assert!(!narrator_no_scene_reestablish_enabled(), "off ⇒ OFF");
        let off = narrator_system("BASE".to_string(), true);
        assert!(!off.contains("不复述场景定场 · 从玩家当前所在处续写"), "OFF 不得追加不复述场景定场纪律");

        match prev {
            Some(v) => std::env::set_var("TRPG_NARRATOR_NO_SCENE_REESTABLISH", v),
            None => std::env::remove_var("TRPG_NARRATOR_NO_SCENE_REESTABLISH"),
        }
    }

    #[test]
    fn narrator_state_persistence_flag_and_overlay() {
        let _g = env_guard();
        let prev = std::env::var("TRPG_NARRATOR_STATE_PERSISTENCE").ok();
        std::env::remove_var("TRPG_NARRATOR_STATE_PERSISTENCE");
        assert!(narrator_state_persistence_enabled(), "未设 ⇒ 默认 ON");
        let on = narrator_system("BASE".to_string(), true);
        assert!(on.contains("既成状态忠实 · 持久不复活、不反转"), "ON 应追加既成状态忠实纪律: {on}");
        assert!(on.contains("不把已完成的变更倒回旧状态"), "ON 应含方向①(终止态不复活,直击 turn10)");
        assert!(on.contains("不把仍持续的既成状态反转"), "ON 应含方向②(持续态不反转,直击 turn13)");
        assert!(on.contains("残留的**物理痕迹**可以如实描写"), "ON 应含'物理痕迹可写但不复活活动状态'子句(不伤真实描写)");
        assert!(on.contains("区别于既成那个状态的新来源"), "ON 应含'新来源须明确区分而非旧状态复活'子句(不伤 J3 新事件)");

        std::env::set_var("TRPG_NARRATOR_STATE_PERSISTENCE", "off");
        assert!(!narrator_state_persistence_enabled(), "off ⇒ OFF");
        let off = narrator_system("BASE".to_string(), true);
        assert!(!off.contains("既成状态忠实 · 持久不复活、不反转"), "OFF 不得追加既成状态忠实纪律");

        match prev {
            Some(v) => std::env::set_var("TRPG_NARRATOR_STATE_PERSISTENCE", v),
            None => std::env::remove_var("TRPG_NARRATOR_STATE_PERSISTENCE"),
        }
    }

    #[test]
    fn narrator_dialogue_fidelity_flag_and_overlay() {
        let _g = env_guard();
        let prev = std::env::var("TRPG_NARRATOR_DIALOGUE_FIDELITY").ok();
        std::env::remove_var("TRPG_NARRATOR_DIALOGUE_FIDELITY");
        assert!(narrator_dialogue_fidelity_enabled(), "未设 ⇒ 默认 ON");
        let on = narrator_system("BASE".to_string(), true);
        assert!(on.contains("台词忠实 · 既定台词与说话者不改写"), "ON 应追加台词忠实纪律: {on}");
        assert!(
            on.contains("让 NPC 说**一句新的台词**"),
            "ON 应含'推进靠新台词而非改写旧台词'子句(不伤对话推进)"
        );
        assert!(
            on.contains("把这句话原本喊向的对象改成另一个对象"),
            "ON 应含'喊话对象不改写'子句(L-Q 扩面,直击 aliveFull30v2 turn5 addressee slip)"
        );

        std::env::set_var("TRPG_NARRATOR_DIALOGUE_FIDELITY", "off");
        assert!(!narrator_dialogue_fidelity_enabled(), "off ⇒ OFF");
        let off = narrator_system("BASE".to_string(), true);
        assert!(!off.contains("台词忠实 · 既定台词与说话者不改写"), "OFF 不得追加台词忠实纪律");

        match prev {
            Some(v) => std::env::set_var("TRPG_NARRATOR_DIALOGUE_FIDELITY", v),
            None => std::env::remove_var("TRPG_NARRATOR_DIALOGUE_FIDELITY"),
        }
    }

    #[test]
    fn narrator_location_fidelity_flag_and_overlay() {
        let _g = env_guard();
        let prev = std::env::var("TRPG_NARRATOR_LOCATION_FIDELITY").ok();
        std::env::remove_var("TRPG_NARRATOR_LOCATION_FIDELITY");
        assert!(narrator_location_fidelity_enabled(), "未设 ⇒ 默认 ON");
        let on = narrator_system("BASE".to_string(), true);
        assert!(on.contains("地点忠实 · 只渲染既定场景"), "ON 应追加地点忠实纪律: {on}");
        assert!(on.contains("狭窄的设备间里"), "ON 应含 smokeGND3 turn8 失败模式示例");
        assert!(on.contains("地点的改变只能来自既成的场景转移"), "ON 应含'地点改变源自committed场景转移'子句(不伤 J3)");

        std::env::set_var("TRPG_NARRATOR_LOCATION_FIDELITY", "off");
        assert!(!narrator_location_fidelity_enabled(), "off ⇒ OFF");
        let off = narrator_system("BASE".to_string(), true);
        assert!(!off.contains("地点忠实 · 只渲染既定场景"), "OFF 不得追加地点忠实纪律");

        match prev {
            Some(v) => std::env::set_var("TRPG_NARRATOR_LOCATION_FIDELITY", v),
            None => std::env::remove_var("TRPG_NARRATOR_LOCATION_FIDELITY"),
        }
    }

    #[test]
    fn narrator_no_hollow_suspension_flag_and_overlay() {
        let _g = env_guard();
        let prev = std::env::var("TRPG_NARRATOR_NO_HOLLOW_SUSPENSION").ok();
        std::env::remove_var("TRPG_NARRATOR_NO_HOLLOW_SUSPENSION");
        assert!(narrator_no_hollow_suspension_enabled(), "未设 ⇒ 默认 ON");
        let on = narrator_system("BASE".to_string(), true);
        assert!(on.contains("收束于既成结果 · 不悬置"), "ON 应追加不悬置纪律: {on}");
        assert!(on.contains("一切悬而未决"), "ON 应含悬置语禁止子句");
        assert!(on.contains("没发生的别写"), "ON 应含'不捏造未发生'子句(理念 no-invention)");

        std::env::set_var("TRPG_NARRATOR_NO_HOLLOW_SUSPENSION", "off");
        assert!(!narrator_no_hollow_suspension_enabled(), "off ⇒ OFF");
        let off = narrator_system("BASE".to_string(), true);
        assert!(!off.contains("收束于既成结果 · 不悬置"), "OFF 不得追加不悬置纪律");

        match prev {
            Some(v) => std::env::set_var("TRPG_NARRATOR_NO_HOLLOW_SUSPENSION", v),
            None => std::env::remove_var("TRPG_NARRATOR_NO_HOLLOW_SUSPENSION"),
        }
    }

    #[test]
    fn gm_no_relocate_flag_and_overlay() {
        let _g = env_guard();
        let prev = std::env::var("TRPG_GM_NO_RELOCATE_PLAYER").ok();
        let prev_os = std::env::var("TRPG_GM_OBJECT_STATE_AUTHORITY").ok();
        std::env::remove_var("TRPG_GM_NO_RELOCATE_PLAYER");
        assert!(gm_no_relocate_player_enabled(), "未设 ⇒ 默认 ON");
        let on = adjudicator_system("BASE".to_string(), true);
        assert!(on.contains("玩家位置主权"), "ON 应追加位置主权守则: {on}");
        assert!(on.contains("把 beat 搬到玩家处"), "ON 应含§4 relocation-toward-player");
        assert!(on.contains(ADJUDICATOR_CRAFT), "ON 仍保留既有 ADJUDICATOR_CRAFT");

        // 两个追加 flag 全关 ⇒ 与既有 ADJUDICATOR_CRAFT 字节等价。
        std::env::set_var("TRPG_GM_NO_RELOCATE_PLAYER", "off");
        std::env::set_var("TRPG_GM_OBJECT_STATE_AUTHORITY", "off");
        assert!(!gm_no_relocate_player_enabled(), "off ⇒ OFF");
        let off = adjudicator_system("BASE".to_string(), true);
        assert_eq!(off, format!("BASE\n\n{ADJUDICATOR_CRAFT}"), "OFF 须与历史 adjudicator overlay 字节等价");
        assert!(!off.contains("玩家位置主权"), "OFF 不得追加位置主权守则");
        assert_eq!(adjudicator_system("B2".to_string(), false), "B2", "craft OFF 永远 base");

        match prev {
            Some(v) => std::env::set_var("TRPG_GM_NO_RELOCATE_PLAYER", v),
            None => std::env::remove_var("TRPG_GM_NO_RELOCATE_PLAYER"),
        }
        match prev_os {
            Some(v) => std::env::set_var("TRPG_GM_OBJECT_STATE_AUTHORITY", v),
            None => std::env::remove_var("TRPG_GM_OBJECT_STATE_AUTHORITY"),
        }
    }

    #[test]
    fn gm_object_state_authority_flag_and_overlay() {
        let _g = env_guard();
        let prev = std::env::var("TRPG_GM_OBJECT_STATE_AUTHORITY").ok();
        std::env::remove_var("TRPG_GM_OBJECT_STATE_AUTHORITY");
        assert!(gm_object_state_authority_enabled(), "未设 ⇒ 默认 ON");
        let on = adjudicator_system("BASE".to_string(), true);
        assert!(on.contains("物体/世界状态主权"), "ON 应追加物体状态主权守则: {on}");
        assert!(on.contains("已被丢出/甩开/脱手的工具当作还握在手里"), "ON 应含 turn3 失败模式");
        assert!(on.contains("尚未确认存在的入口/出口/侧门/通道/线缆接点"), "ON 应含 smokeGND2 turn3 地点特征虚构模式");
        assert!(on.contains("不创造玩家凭空声称的状态改变或地点特征"), "ON 应含§二.9 不造新事实子句");

        std::env::set_var("TRPG_GM_OBJECT_STATE_AUTHORITY", "off");
        assert!(!gm_object_state_authority_enabled(), "off ⇒ OFF");
        let off = adjudicator_system("BASE".to_string(), true);
        assert!(!off.contains("物体/世界状态主权"), "OFF 不得追加物体状态主权守则");

        match prev {
            Some(v) => std::env::set_var("TRPG_GM_OBJECT_STATE_AUTHORITY", v),
            None => std::env::remove_var("TRPG_GM_OBJECT_STATE_AUTHORITY"),
        }
    }

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

    // ---- Phase B channels (overnight OB-*) ----

    #[test]
    fn on_instructs_dialogue_channel_ob_dialogue() {
        let out = narrator_system("BASE".to_string(), true);
        // Narrator (player-facing) wraps present-NPC speech in [dialogue actor="…"].
        assert!(out.contains("[dialogue actor="));
        assert!(out.contains("NPC 真正开口说话"));
        // Must NOT degrade into mechanical Q&A, and not forced on silent turns.
        assert!(out.contains("不要硬塞 [dialogue]"));
    }

    #[test]
    fn on_instructs_choice_channel_conservatively_ob_system_choice() {
        let out = narrator_system("BASE".to_string(), true);
        // [choice] is an OPTIONAL affordance at a real decision point — never a forced menu (Q-4).
        assert!(out.contains("[choice]"));
        assert!(out.contains("绝不是强制菜单"));
        assert!(out.contains("玩家永远可以无视它自由行动"));
        // [system] remains the player-visible flow channel (A.0) — still present.
        assert!(out.contains("只用于玩家可见的流程"));
    }

    #[test]
    fn on_instructs_hide_channel_as_proposal_ob_hide() {
        // [hide] lives on the ADJUDICATOR (台下 layer that knows secrets), NOT the player-facing
        // Narrator — respects constitution ⑧ (Narrator never authors hidden facts).
        let adj = adjudicator_system("BASE".to_string(), true);
        assert!(adj.contains("[hide kind="));
        assert!(adj.contains("Proposal"));
        assert!(adj.contains("绝不替系统提交状态"));
        let narr = narrator_system("BASE".to_string(), true);
        assert!(!narr.contains("[hide kind="), "Narrator must not author [hide]");
    }

    #[test]
    fn on_instructs_meta_decision_summary_ob_meta() {
        let adj = adjudicator_system("BASE".to_string(), true);
        assert!(adj.contains("[meta kind=\"decision_summary\"]"));
        assert!(adj.contains("台下摘要"));
        assert!(adj.contains("绝不出现在玩家可见散文"));
    }

    #[test]
    fn extract_offstage_blocks_keeps_hide_and_meta_raw() {
        let prose = "你看到门厅空无一人。[hide kind=\"npc_action\"]守卫悄悄换了班[/hide]\
                     更深处传来脚步声。[meta kind=\"decision_summary\"]开感知检定 DV13[/meta]";
        let off = extract_offstage_blocks(prose);
        assert!(off.contains("[hide kind=\"npc_action\"]守卫悄悄换了班[/hide]"));
        assert!(off.contains("[meta kind=\"decision_summary\"]开感知检定 DV13[/meta]"));
        // player-visible prose must NOT be carried over.
        assert!(!off.contains("门厅空无一人"));
        assert!(!off.contains("脚步声"));
    }

    #[test]
    fn extract_offstage_blocks_empty_when_none() {
        assert_eq!(extract_offstage_blocks("纯散文，没有任何台下标记。"), "");
        // not a real tag (no ']' or space after) → ignored, no panic.
        assert_eq!(extract_offstage_blocks("[hidden]不是真标签"), "");
    }

    // ---- deterministic offstage floor (OB-meta / OB-hide) ----

    fn mk_result(
        check_id: &str,
        vis: trpg_model::RollVisibility,
        label: &str,
        degree: &str,
    ) -> trpg_model::CheckResultRecord {
        trpg_model::CheckResultRecord {
            check_id: check_id.into(),
            roll: trpg_model::DiceRollRecord {
                roll_id: format!("roll_{check_id}"),
                session_id: "s1".into(),
                turn_id: "t1".into(),
                check_id: Some(check_id.into()),
                roller_kind: trpg_model::ActorKind::default(),
                roller_id: None,
                visibility: vis,
                expression: "1d100".into(),
                result: serde_json::json!({"total": 42}),
                seed_commitment: String::new(),
                revealed_at: None,
                created_at: chrono::Utc::now(),
            },
            outcome: serde_json::json!({"check_label": label, "degree": degree, "success": true}),
            committed_patches: vec![],
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn synth_emits_meta_for_every_committed_check() {
        let mut snap = trpg_agent::TurnLedgerSnapshot::default();
        snap.check_results.push(mk_result(
            "chk1",
            trpg_model::RollVisibility::PublicGmRoll,
            "侦查",
            "通常成功",
        ));
        let out = synthesize_offstage_from_ledger(&snap);
        assert!(out.contains("[meta kind=\"decision_summary\"]"), "{out}");
        assert!(out.contains("侦查(1d100)→通常成功"), "{out}");
        // a public roll is NOT recorded as a hidden 暗骰.
        assert!(!out.contains("[hide"), "{out}");
    }

    #[test]
    fn synth_emits_hide_anggu_for_private_dice_roll() {
        // an opposed defender's private roll lives in dice_rolls (not as a check_result) — it must
        // surface as a [hide kind="暗骰"] off-screen record, never shown to the player.
        let mut snap = trpg_agent::TurnLedgerSnapshot::default();
        snap.check_results.push(mk_result(
            "atk1",
            trpg_model::RollVisibility::PublicGmRoll,
            "开火",
            "失败",
        ));
        let mut def = mk_result(
            "atk1",
            trpg_model::RollVisibility::PrivateGmRoll,
            "防御",
            "成功",
        )
        .roll;
        def.roller_kind = trpg_model::ActorKind::Npc;
        def.result = serde_json::json!({"total": 9});
        snap.dice_rolls.push(def);
        let out = synthesize_offstage_from_ledger(&snap);
        // public attack ⇒ [meta]; private defender roll ⇒ [hide 暗骰].
        assert!(out.contains("[meta kind=\"decision_summary\"]"), "{out}");
        assert!(out.contains("[hide kind=\"暗骰\"]"), "{out}");
        assert!(out.contains("NPC/对手"), "{out}");
        assert!(out.contains("玩家不可见"), "{out}");
    }

    #[test]
    fn synth_emits_hide_anggu_from_opposed_outcome() {
        // the defender's private roll lives in outcome.opposed (NOT dice_rolls) — surface it.
        let mut snap = trpg_agent::TurnLedgerSnapshot::default();
        let mut r = mk_result(
            "atk1",
            trpg_model::RollVisibility::PublicGmRoll,
            "开火对抗",
            "失败",
        );
        r.outcome = serde_json::json!({
            "check_label":"开火对抗","degree":"defender_wins","success":false,
            "opposed":{"winner":"defender","defender_actor_id":"npc_athena","defender_value":10},
            "resolution_model":{"defender_expression":"1d10","defender_roll_visibility":"private_gm_roll"}
        });
        snap.check_results.push(r);
        let out = synthesize_offstage_from_ledger(&snap);
        assert!(out.contains("[meta kind=\"decision_summary\"]"), "{out}");
        assert!(out.contains("[hide kind=\"暗骰\"]"), "{out}");
        assert!(out.contains("npc_athena"), "{out}");
        assert!(out.contains("1d10=10"), "{out}");
        assert!(out.contains("玩家不可见"), "{out}");
    }

    #[test]
    fn synth_empty_when_no_committed_check() {
        let snap = trpg_agent::TurnLedgerSnapshot::default();
        assert_eq!(synthesize_offstage_from_ledger(&snap), "");
    }
}

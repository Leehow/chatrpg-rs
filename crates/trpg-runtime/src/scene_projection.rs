use crate::npc_synth;
use trpg_model::*;

/// SkeletonOnly 降级块内容（N2）：投影骨架元数据 + 禁编造指令，不含正文。
pub(crate) fn skeleton_fallback_block(
    module_id: &str,
    n: &ScenarioNode,
    scenes: &[ScenarioNode],
) -> ContextBlock {
    let mut body = format!(
        "【场景骨架·未深抽】status=SkeletonOnly\n标题：{}\n",
        n.title
    );
    if !n.summary.trim().is_empty() {
        body.push_str(&format!("摘要：{}\n", n.summary));
    }
    match (n.page_start, n.page_end) {
        (Some(s), Some(e)) => body.push_str(&format!("页码：{s}–{e}\n")),
        (Some(s), None) => body.push_str(&format!("页码：{s}\n")),
        _ => {}
    }
    // 已知实体 id（骨架阶段 LLM 已识别的引用，供 GM 检索用）
    let mut refs: Vec<String> = Vec::new();
    refs.extend(n.referenced_npc_ids.iter().map(|id| format!("npc:{id}")));
    refs.extend(n.referenced_clue_ids.iter().map(|id| format!("clue:{id}")));
    refs.extend(
        n.referenced_location_ids
            .iter()
            .map(|id| format!("loc:{id}")),
    );
    refs.extend(
        n.referenced_encounter_ids
            .iter()
            .map(|id| format!("enc:{id}")),
    );
    if !refs.is_empty() {
        body.push_str(&format!("已知实体：{}\n", refs.join(", ")));
    }
    // 出口（与 DeepExtracted 路径相同的 fail-closed 投影）
    let exits: Vec<String> = n
        .links
        .iter()
        .filter_map(|l| {
            let title = scenes
                .iter()
                .find(|s| s.node_id == l.to_node_id)
                .map(|s| s.title.as_str())
                .unwrap_or("");
            if title.trim().is_empty() {
                None
            } else {
                Some(format!("- {title} → {}", l.reason))
            }
        })
        .collect();
    if !exits.is_empty() {
        body.push_str(&format!("【已知出口】\n{}\n", exits.join("\n")));
    }
    // GM 防编造指令
    body.push_str(
        "\n⚠️ 此场景尚未深抽：叙述具体内容前先检索源文档；不要编造 read_aloud/数值/人物细节。\n",
    );
    let mut block = ContextBlock::new(
        format!("module.{module_id}.scene.{}.skeleton", n.node_id),
        BlockKind::SceneStatic,
        format!("{}（骨架）", n.title),
        BlockContent::Text(body),
        Visibility::GmOnly,
        Stability::SceneStable,
        CacheZone::DynamicTail,
        Scope {
            scope_type: ScopeType::Scene,
            scope_id: n.node_id.clone(),
        },
        20, // 低 token：骨架块只有元数据，不占满 context
    );
    block.expires_at_scene = Some(n.node_id.clone());
    block.load_reason = Some("skeleton_fallback".into());
    block.tags = vec![
        "module_scene".into(),
        "scene_skeleton".into(),
        "skeleton_only".into(),
    ];
    block
}

/// N3: DeepExtracted 当前场景正文块的缓存区，受 `TRPG_SCENE_DEEP_BLOCK_CACHE_ZONE` 控制。
/// 默认 `dynamic_tail`=当前行为（零变化）；设 `pinned_middle` 时正文（read_aloud/gm_notes/
/// fixed exits）落 PinnedMiddle + expires_at_scene，使切场景才变 pinned_hash、普通输入
/// 只变 dynamic_hash（更优缓存）。无法识别的值 fail-closed 回退 DynamicTail（不编造）。
/// 注意：仅作用于 DeepExtracted 正文块；SkeletonOnly 降级块（N2，临时降级）恒 DynamicTail。
pub(crate) fn scene_deep_block_cache_zone() -> CacheZone {
    match std::env::var("TRPG_SCENE_DEEP_BLOCK_CACHE_ZONE") {
        Ok(v) if v.trim().eq_ignore_ascii_case("pinned_middle") => CacheZone::PinnedMiddle,
        _ => CacheZone::DynamicTail,
    }
}

/// L-G 失忆锚:场景开场定场文(read_aloud/可念)是否只在玩家**首次进入该场景**时投放。
/// **默认 ON**(eval profile 不设 ⇒ 自动吃到),OFF 字节等价基线(恒投 read_aloud=旧行为)。
/// 仅显式 `0/false/off/no` 关。与 BUG-1 dice_core / L-E continuity anchor 同模(默认 ON / OFF baseline)。
/// 病灶:read_aloud 是 SceneStable 块,玩家在同一场景的每个回合都被复投 ⇒ GM 每回合重述开场、把玩家
/// 挪回场景入口 ⇒ player-sim 判 AMNESIA。首进才投 ⇒ 后续回合 GM 续写既成局面(NPC/出口/GM 注记仍每
/// 回合在,durable 参考不丢)。
pub(crate) fn scene_read_aloud_first_entry_only_enabled() -> bool {
    !matches!(
        std::env::var("TRPG_SCENE_READ_ALOUD_FIRST_ENTRY_ONLY")
            .ok()
            .map(|v| v.to_ascii_lowercase())
            .as_deref(),
        Some("0") | Some("false") | Some("off") | Some("no")
    )
}

/// L-I 位置失忆结构杠杆:玩家**已首次进入**本场景后(include_read_aloud=false),本场景的
/// `gm_notes`(开场战斗布置)与 NPC 到场散文是**首进框定**——每回合复投会把 GM 念白拽回该场景
/// 的到场点位 ⇒ 玩家既定当前位置被吞 ⇒ player-sim 判位置 AMNESIA。此 flag 开时把这些静态框定
/// **降格为持久背景参考、显式从属于「连续性锚」**(durable 机制由 `scene_mechanics` 块另投,不丢)。
/// **默认 ON**(eval profile 不设 ⇒ 自动吃到),OFF 字节等价(gated body 与历史一致)。零 ruleset 分支。
pub(crate) fn scene_static_framing_subordinate_enabled() -> bool {
    !matches!(
        std::env::var("TRPG_SCENE_STATIC_FRAMING_SUBORDINATE")
            .ok()
            .map(|v| v.to_ascii_lowercase())
            .as_deref(),
        Some("0") | Some("false") | Some("off") | Some("no")
    )
}

/// L-M 位置失忆结构杠杆(主管 Q1_ANSWER 批准的「committed-scene 与玩家叙事位置背离时背景化冻结
/// 场景定场/NPC 支配」的彻底版):L-I 只把冻结场景的 `gm_notes`(开场战斗布置散文)/NPC 到场散文
/// **降格**为背景参考——但全文仍每回合注入,GM 仍据此把仓库交火现场与玩家既定位置**叠加**(smokeK7
/// 实证 4/4 回合「GM 把玩家位置同时写成仓库前和家门口」=硬位置失忆)。本杠杆在 subordinate 路径上
/// **彻底压制**这些静态散文正文:gm_notes 正文/NPC 到场散文不再投放(其可机械化部分由 `scene_mechanics`
/// 块另投,不丢;NPC 仅留名册名),只留一行「以连续性锚为位置权威」指令 ⇒ GM 上下文里不再有可被复述/
/// 叠加的仓库交火正文。**默认 ON**(eval profile 不设 ⇒ 自动吃到),OFF ⇒ 退回 L-I 降格行为(字节等价)。
/// 零 ruleset 分支。durable 机制(grab/cut-power/hack)经 scene_mechanics 投,J2 不回退。
pub(crate) fn scene_frozen_framing_suppress_enabled() -> bool {
    !matches!(
        std::env::var("TRPG_SCENE_FROZEN_FRAMING_SUPPRESS")
            .ok()
            .map(|v| v.to_ascii_lowercase())
            .as_deref(),
        Some("0") | Some("false") | Some("off") | Some("no")
    )
}

pub(crate) fn scene_node_to_blocks(
    module_id: &str,
    n: &ScenarioNode,
    npcs: &[serde_json::Value],
    scenes: &[ScenarioNode],
) -> Vec<ContextBlock> {
    // 默认 include_read_aloud=true ⇒ 与历史行为字节等价(所有既有调用方/单测不变)。
    scene_node_to_blocks_with_opts(module_id, n, npcs, scenes, true)
}

/// L-G:同 `scene_node_to_blocks`,但 `include_read_aloud=false` 时**跳过开场定场文(可念)正文**,
/// 改投一行非剧透锚提示(让 GM 知道开场已交付、勿复述),其余(GM 注记/NPC/出口/机制意图)原样每回合在。
/// `include_read_aloud=true` 与 `scene_node_to_blocks` 字节等价。整块仍 GmOnly,锚提示不进玩家可见输出。
pub(crate) fn scene_node_to_blocks_with_opts(
    module_id: &str,
    n: &ScenarioNode,
    npcs: &[serde_json::Value],
    scenes: &[ScenarioNode],
    include_read_aloud: bool,
) -> Vec<ContextBlock> {
    if n.extraction_status != SceneExtractionStatus::DeepExtracted {
        // N2: SkeletonOnly（及其他非 DeepExtracted 状态）产出轻量降级块，
        // 防 GM 完全失去"我在哪个场景"。fail-closed：不含正文、不编造。
        return vec![skeleton_fallback_block(module_id, n, scenes)];
    }
    // L-I:首进已交付后(include_read_aloud=false)且 flag 开 ⇒ 把开场战斗布置/到场散文降格为
    // 从属于连续性锚的持久背景参考(防位置失忆)。OFF 或首进路径 ⇒ subordinate=false ⇒ 字节等价。
    let subordinate = !include_read_aloud && scene_static_framing_subordinate_enabled();
    // L-M:subordinate 路径上彻底压制冻结场景静态散文(gm_notes 正文/NPC 到场散文),只留位置权威
    // 指令——OFF ⇒ suppress=false ⇒ 退回 L-I 降格行为(字节等价)。
    let suppress = subordinate && scene_frozen_framing_suppress_enabled();
    let mut body = String::new();
    if let Some(ra) = &n.read_aloud {
        if !ra.trim().is_empty() {
            if include_read_aloud {
                body.push_str("【可念】\n");
                body.push_str(ra);
                body.push('\n');
            } else {
                // L-G 已交付开场:不复投定场文正文,仅留一行锚提示(防 GM 重述/把玩家挪回入口)。
                body.push_str(
                    "【场景开场定场文已于首次进入时交付，请勿复述；从玩家当前既成局面续写】\n",
                );
            }
        }
    }
    if subordinate {
        // 把下面的 GM 注记/NPC 散文显式标注为持久背景参考、从属于连续性锚 —— 直接嵌在
        // 会"渗血"的内容旁(高显著度),压过单独连续性锚块被并列内容盖过的弱效。
        body.push_str(
            "【场景静态参考 · 非当前时刻】下面的 GM 注记与 NPC 描述是本场景的**持久背景参考**，\
             写的是首次进入时的到场/开场布置，**不是本回合正在发生的事**。\n\
             ⚠️ 玩家**当前所在位置与既成局面一律以「连续性锚」为准**：严禁据此把玩家挪回本场景的\
             到场/开场布置、严禁重述开场、严禁把身处别处的玩家瞬移到此处；若本场景核心冲突需要介入，\
             按§4 让冲突**主动来到玩家当前所在处**（威胁外溢／NPC 闯入／时钟波及——改 setting 保实质）。\n",
        );
    }
    if let Some(g) = &n.gm_notes {
        if !g.trim().is_empty() {
            if suppress {
                // L-M:不投 gm_notes 开场布置散文正文(其可机械化部分由 scene_mechanics 块另投);
                // 仅留一行——彻底移除可被 GM 复述/与玩家既定位置叠加的仓库交火正文。
                body.push_str(
                    "\n【GM · 本场景开场布置/注记已于首次进入时交付，其可机械化部分由「场景机制」块投放；此处不复述正文。玩家当前所在位置与既成局面一律以「连续性锚」为准；若本场景核心冲突需要介入，按§4 让冲突主动来到玩家当前所在处，绝不把玩家挪回此场景的到场/开场点位。】\n",
                );
            } else {
                body.push_str(if subordinate {
                    "\n【GM · 场景持久参考（背景设定，非当前时刻）】\n"
                } else {
                    "\n【GM】\n"
                });
                body.push_str(g);
                body.push('\n');
            }
        }
    }
    if subordinate && !n.referenced_npc_ids.is_empty() {
        body.push_str(if suppress {
            "\n[场景角色名册 · 仅备名；谁在场/与玩家的距离/关系一律以当前既成局面（连续性锚）为准，勿据下表把任何 NPC 重新拉到玩家面前]"
        } else {
            "\n[场景角色名册 · 持久参考；谁在场、与玩家的距离/关系一律以当前既成局面（连续性锚）为准]"
        });
    }
    for id in &n.referenced_npc_ids {
        if let Some(v) = npcs
            .iter()
            .find(|v| v.get("id").and_then(|x| x.as_str()) == Some(id.as_str()))
        {
            let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("");
            // Deep-extracted entities (reader DEEP_SYS) carry prose in `body`
            // (or the locale-variant key `正文`, env-gated); shallow/index
            // entities use `summary`. Single source: entity_body_prose.
            if suppress {
                // L-M:仅留 NPC 名,不投到场散文正文(防 GM 据散文把 NPC 拉回玩家面前 / 叠加场景)。
                body.push_str(&format!("\n[NPC] {name}"));
            } else {
                let sum = entity_body_prose(v).unwrap_or("");
                body.push_str(&format!("\n[NPC] {name}: {sum}"));
            }
        }
    }
    // §Phase4 出口投影：把本场景 links 解析成『目标场景标题 → 通往理由』，让 GM
    // 知道当前场景可去哪里(场景导航的语义素材)。目标 node_id 在 scenes 中查不到、
    // 或目标无标题 → 跳过该出口(fail-closed，不投空标题、不编造)。
    let exits: Vec<String> = n
        .links
        .iter()
        .filter_map(|l| {
            let title = scenes
                .iter()
                .find(|s| s.node_id == l.to_node_id)
                .map(|s| s.title.as_str())
                .unwrap_or("");
            if title.trim().is_empty() {
                None
            } else {
                Some(format!("- {} → {}", title, l.reason))
            }
        })
        .collect();
    if !exits.is_empty() {
        body.push_str("\n【出口】\n");
        body.push_str(&exits.join("\n"));
        body.push('\n');
    }
    let mut block = ContextBlock::new(
        format!("module.{module_id}.scene.{}", n.node_id),
        BlockKind::SceneStatic,
        n.title.clone(),
        BlockContent::Text(body),
        Visibility::GmOnly,
        Stability::SceneStable,
        // N3: 默认 DynamicTail（零变化）；TRPG_SCENE_DEEP_BLOCK_CACHE_ZONE=pinned_middle
        // 时落 PinnedMiddle，配合下方 expires_at_scene 让切场景才变 pinned_hash。
        scene_deep_block_cache_zone(),
        Scope {
            scope_type: ScopeType::Scene,
            scope_id: n.node_id.clone(),
        },
        60,
    );
    block.expires_at_scene = Some(n.node_id.clone());
    block.load_reason = Some("current_scene_deep_projection".into());
    block.tags = vec![
        "module_scene".into(),
        "scene_static".into(),
        "deep_extracted".into(),
    ];
    let mut blocks = vec![block];
    // C3：当前场景机制意图索引（BP2）。渲染纯函数在 trpg-model（scene_intents_text，
    // 每条一行 id|description|tested_parameter|difficulty 摘要，不含 effect_policy 全文
    // ——结算才用，C4 按 intent_id 从图谱取）。空 intents → None → 不出块（旧模组零变化）。
    if let Some(text) = trpg_model::scene_intents_text(&n.scene_mechanics) {
        let mut b = ContextBlock::new(
            format!("module.{module_id}.scene.{}.mechanics", n.node_id),
            BlockKind::SceneStatic, // 复用既有 kind：不动 trpg-db 的 block_kind 词表
            format!("{} —— 场景机制意图", n.title),
            BlockContent::Text(text),
            Visibility::GmOnly,
            Stability::SceneStable,
            CacheZone::PinnedMiddle, // 骨架契约：pinned_hash 场景内稳定，场景切换换块
            Scope {
                scope_type: ScopeType::Scene,
                scope_id: n.node_id.clone(),
            },
            58, // 略低于正文块（60）
        );
        b.expires_at_scene = Some(n.node_id.clone());
        b.load_reason = Some("current_scene_mechanic_intents".into());
        b.tags = vec!["module_scene".into(), "scene_mechanics".into()];
        blocks.push(b);
    }
    blocks
}

pub fn module_entry_scene_id(graph: &ModuleGraph) -> Option<String> {
    if let Some(id) = graph
        .spine
        .get("entry_node_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if graph.scenes.iter().any(|s| s.node_id == id) {
            return Some(id.to_string());
        }
    }
    if let Some(s) = graph
        .scenes
        .iter()
        .find(|s| s.extraction_status == SceneExtractionStatus::DeepExtracted)
    {
        return Some(s.node_id.clone());
    }
    if let Some(id) = graph.scenes.first().map(|s| s.node_id.clone()) {
        return Some(id);
    }
    // Fallback: 某些 module reader 把开局流程写进 spine.entry_hooks 而**未**归一化到
    // graph.scenes（例：anthology/scenario_collection 模组）。此时上面全空 → 整局
    // current_scene_id=NULL，director/narrator 无场景锚点而每回合"无新内容"空叙事。
    // 从 spine 派生一个稳定的 synthetic entry scene id（`spine:<hook_id>`）解锁开局；
    // 纯函数、不依赖 ruleset/module 名，对任意 spine 有 entry_hooks 的模组都生效。
    spine_entry_scene_id(&graph.spine)
}

/// 从 spine 派生 synthetic entry scene id（graph.scenes 为空时的兜底）。
/// 偏好 mission_briefing > standard_mission_start > 首个 entry_hook；都无则 None。
/// 返回 `spine:<hook_id>` 形态，稳定可复现，不与真实 node_id 冲突。
fn spine_entry_scene_id(spine: &serde_json::Value) -> Option<String> {
    let hooks = spine.get("entry_hooks").and_then(|v| v.as_array())?;
    let hook_id = |h: &serde_json::Value| {
        h.get("id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let typed = |want: &str| {
        hooks
            .iter()
            .find(|h| h.get("type").and_then(|v| v.as_str()) == Some(want))
            .and_then(hook_id)
    };
    let id = typed("mission_briefing")
        .or_else(|| typed("standard_mission_start"))
        .or_else(|| hooks.iter().find_map(hook_id))?;
    Some(format!("spine:{id}"))
}

/// 决定本回合生效的 scene_id：已有非空则保留；否则仅当有 module 时用载入值。
pub(crate) fn resolve_turn_scene_id(
    state_scene: Option<&str>,
    module_id: Option<&str>,
    loaded: Option<String>,
) -> Option<String> {
    match state_scene {
        Some(s) if !s.trim().is_empty() => Some(s.to_string()),
        _ => {
            if module_id.is_some() {
                loaded
            } else {
                None
            }
        }
    }
}

/// §Task5/Phase3-gap Pure mapping: typed semantic action kind → the `(bucket, param)`
/// the contest will need from the **opposition NPC**. None for kinds needing no NPC param.
/// Semantic over keyword (philosophy §2): match on the typed enum, not strings.
///
/// Direction matters — the contest tests *the opponent*, so who is the aggressor decides
/// which NPC key is needed (mirrors the runtime's own split at lib.rs `desired_outcome`:
/// `Attack|Counterattack`="resolve the declared attack" vs
/// `UnderAttack|EnemyInitiatedConflict|SceneEntersConflict`="respond to incoming danger",
/// and combat's `is_attack`/`is_defense`):
/// - **active attack** (player strikes): opponent must *defend* → its defense value
///   (meet_or_beat → derived defense DV; roll_under → its evasion/dodge skill).
/// - **passive defense** (player defends/dodges/takes cover, or is under attack — the NPC
///   is the one striking): opponent must *land its hit* → its **attack skill** (Fighting/
///   Firearms…). Attack is a skill in both compare models, so it's compare-agnostic here.
///
/// Zero per-ruleset hardcoding: keys are generic semantic identifiers the npc_synth +
/// contest layer resolve against the kernel; the only data-driven branch is kernel.compare.
pub(crate) fn map_check_param_need(
    action_kind: &SituationActionKind,
    compare: &str,
) -> Option<(String, String)> {
    use SituationActionKind::*;
    // 玩家主动攻击 NPC → 对手要防御 → 测它的防御值。
    let active_attack = matches!(action_kind, Attack | Counterattack | CastOrUsePower);
    // 玩家被动防御(NPC 在攻击你)→ 对手要命中 → 测它的攻击技能。
    let passive_defense = matches!(
        action_kind,
        Defend | Dodge | TakeCover | UnderAttack | EnemyInitiatedConflict | SceneEntersConflict
    );
    let stealth_family = matches!(
        action_kind,
        Hide | Hack | DisableDevice | Intimidate | Negotiate | InvestigateDuringConflict
    );
    if active_attack {
        return Some(if compare == "meet_or_beat" {
            ("stats".to_string(), "defense".to_string())
        } else {
            ("skills".to_string(), "dodge".to_string())
        });
    }
    if passive_defense {
        // 对手(NPC)是攻击方,测它的命中能力。攻击在 roll_under/meet_or_beat 两套模型
        // 里都是技能桶,故不随 compare 变;现搓由 npc_synth persona-judge 落卡。
        return Some(("skills".to_string(), "attack".to_string()));
    }
    if stealth_family {
        return Some(("skills".to_string(), "perception".to_string()));
    }
    None
}

/// 对抗契约判定:必须同时有 target_actor 与 opponent_tested_parameter。
pub fn contract_is_opposed(c: &CheckContract) -> bool {
    c.target_actor.is_some() && c.opponent_tested_parameter.is_some()
}

/// 盖章:把对抗所需的 target_actor + 防御方 tested key 写进契约(B2 通道)。
/// 攻击方 tested key 走 contract.tested_parameter(named 路径已设)。纯函数,易测。
pub fn stamp_opposed_check(
    check: &mut CheckContract,
    npc: &npc_synth::NpcPersona,
    _bucket: &str,
    param: &str,
) {
    check.target_actor = Some(ActorRef {
        actor_id: npc.actor_id.clone(),
        actor_kind: ActorKind::Npc,
        display_name: Some(npc.name.clone()),
    });
    check.opponent_tested_parameter = Some(TestedParameter {
        domain: None,
        key: param.to_string(),
        label: param.to_string(),
    });
}

/// 跨测试模块共享的 env 串行锁：`TRPG_SCENE_DEEP_BLOCK_CACHE_ZONE` 是进程级全局，
/// 凡读/写它的测试都必须在同一把锁上串行，否则并行跑时 env 变更会和别处的"两次读
/// env 期望字节一致"断言竞争。提升到 crate 级（`pub(crate)`）使 scene_need_resolver
/// 的字节等价测试也能锁同一把。仅 `#[cfg(test)]`，零生产路径。
#[cfg(test)]
pub(crate) static N3_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod module_scene_proj_tests {
    use super::*;
    use trpg_model::{BlockKind, CacheZone, ScenarioNode, SceneExtractionStatus};

    #[test]
    fn deep_scene_projects_scenestatic_dynamictail() {
        // N3: 此测试断言默认（env 未设）DynamicTail，须持锁+unset 隔离并行的 env 写测试。
        let _g = DeepZoneEnvGuard::unset();
        let mut n = ScenarioNode::default();
        n.node_id = "loc1".into();
        n.title = "加油站".into();
        n.read_aloud = Some("你们看到一个褪色的广告牌……".into());
        n.gm_notes = Some("老板拉斯藏着钥匙。".into());
        n.extraction_status = SceneExtractionStatus::DeepExtracted;
        n.referenced_npc_ids = vec!["npc1".into()];
        let npcs = vec![serde_json::json!({"id":"npc1","name":"拉斯","summary":"老板"})];
        let blocks = scene_node_to_blocks("mod1", &n, &npcs, &[]);
        assert!(!blocks.is_empty());
        let sb = &blocks[0];
        assert_eq!(sb.kind, BlockKind::SceneStatic);
        assert_eq!(sb.cache_zone, CacheZone::DynamicTail);
        assert_eq!(sb.scope.scope_type, trpg_model::ScopeType::Scene);
        assert_eq!(sb.scope.scope_id, "loc1");
        assert_eq!(sb.expires_at_scene.as_deref(), Some("loc1"));
        let text = sb.content.render_text();
        assert!(text.contains("拉斯"), "在场 NPC 应被并入: {text}");
        assert!(text.contains("褪色的广告牌"), "read_aloud 应并入: {text}");
        assert!(text.contains("钥匙"), "gm_notes 应并入: {text}");
    }

    /// L-G:include_read_aloud=true 与裸 scene_node_to_blocks 字节等价(基线保护);false ⇒ 跳过
    /// 定场文正文、改投锚提示,但 NPC/GM 注记/出口照常(durable 参考不丢)。
    #[test]
    fn read_aloud_first_entry_gate_suppresses_prose_but_keeps_reference() {
        let _g = DeepZoneEnvGuard::unset();
        // 本测专验 L-G 定场文门控保留 gm_notes/NPC 参考 ⇒ 关 L-M 彻底压制(否则散文被抹)。
        let prev_lm = std::env::var("TRPG_SCENE_FROZEN_FRAMING_SUPPRESS").ok();
        std::env::set_var("TRPG_SCENE_FROZEN_FRAMING_SUPPRESS", "off");
        let mut n = ScenarioNode::default();
        n.node_id = "loc1".into();
        n.title = "加油站".into();
        n.read_aloud = Some("你们看到一个褪色的广告牌……".into());
        n.gm_notes = Some("老板拉斯藏着钥匙。".into());
        n.extraction_status = SceneExtractionStatus::DeepExtracted;
        n.referenced_npc_ids = vec!["npc1".into()];
        let npcs = vec![serde_json::json!({"id":"npc1","name":"拉斯","summary":"老板"})];

        // include=true 字节等价裸函数(OFF/首进路径 = 历史行为)。
        let base = scene_node_to_blocks("mod1", &n, &npcs, &[]);
        let incl = scene_node_to_blocks_with_opts("mod1", &n, &npcs, &[], true);
        assert_eq!(
            serde_json::to_vec(&base[0]).unwrap(),
            serde_json::to_vec(&incl[0]).unwrap(),
            "include_read_aloud=true 必须与 scene_node_to_blocks 字节等价"
        );

        // include=false ⇒ 定场文正文消失,锚提示出现,durable 参考保留。
        let gated = scene_node_to_blocks_with_opts("mod1", &n, &npcs, &[], false);
        let text = gated[0].content.render_text();
        assert!(
            !text.contains("褪色的广告牌"),
            "已交付场景不得复投定场文正文: {text}"
        );
        assert!(text.contains("请勿复述"), "应留锚提示防 GM 重述: {text}");
        assert!(text.contains("拉斯"), "NPC 参考仍每回合在: {text}");
        assert!(text.contains("钥匙"), "gm_notes 仍每回合在: {text}");
        match prev_lm {
            Some(v) => std::env::set_var("TRPG_SCENE_FROZEN_FRAMING_SUPPRESS", v),
            None => std::env::remove_var("TRPG_SCENE_FROZEN_FRAMING_SUPPRESS"),
        }
    }

    /// L-G flag 默认 ON;仅显式关值 OFF(镜像 BUG-1/L-E)。env 进程级 ⇒ 串行 set/remove 并复原。
    #[test]
    fn read_aloud_first_entry_flag_defaults_on() {
        let _lock = N3_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("TRPG_SCENE_READ_ALOUD_FIRST_ENTRY_ONLY").ok();
        std::env::remove_var("TRPG_SCENE_READ_ALOUD_FIRST_ENTRY_ONLY");
        assert!(
            scene_read_aloud_first_entry_only_enabled(),
            "未设 ⇒ 默认 ON"
        );
        for off in ["0", "false", "OFF", "no"] {
            std::env::set_var("TRPG_SCENE_READ_ALOUD_FIRST_ENTRY_ONLY", off);
            assert!(!scene_read_aloud_first_entry_only_enabled(), "{off} ⇒ OFF");
        }
        for on in ["1", "true", "on", "garbage"] {
            std::env::set_var("TRPG_SCENE_READ_ALOUD_FIRST_ENTRY_ONLY", on);
            assert!(scene_read_aloud_first_entry_only_enabled(), "{on} ⇒ ON");
        }
        match prev {
            Some(v) => std::env::set_var("TRPG_SCENE_READ_ALOUD_FIRST_ENTRY_ONLY", v),
            None => std::env::remove_var("TRPG_SCENE_READ_ALOUD_FIRST_ENTRY_ONLY"),
        }
    }

    /// L-I:首进后(include_read_aloud=false)默认 ON ⇒ gm_notes/NPC 降格为从属连续性锚的背景
    /// 参考(防位置失忆);显式 OFF ⇒ gated body 与历史(裸 GM/NPC + read_aloud 锚提示)字节等价。
    /// gm_notes/NPC 文本本身始终保留(durable 参考不丢)。env 进程级 ⇒ 持锁串行 + 复原。
    #[test]
    fn static_framing_subordinate_flag_and_behavior() {
        let _lock = N3_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("TRPG_SCENE_STATIC_FRAMING_SUBORDINATE").ok();
        // 本测专验 L-I 降格(保留 gm_notes/NPC 散文)语义 ⇒ 关 L-M 彻底压制,否则散文被 L-M 抹掉。
        let prev_lm = std::env::var("TRPG_SCENE_FROZEN_FRAMING_SUPPRESS").ok();
        std::env::set_var("TRPG_SCENE_FROZEN_FRAMING_SUPPRESS", "off");
        let mut n = ScenarioNode::default();
        n.node_id = "loc1".into();
        n.title = "仓库".into();
        n.read_aloud = Some("警笛把你引向第四街的小仓库……".into());
        n.gm_notes = Some("开场战斗:无人机被警方围攻，PC 别无选择只能应战。".into());
        n.extraction_status = SceneExtractionStatus::DeepExtracted;
        n.referenced_npc_ids = vec!["npc1".into()];
        let npcs = vec![serde_json::json!({"id":"npc1","name":"雅典娜","summary":"失控无人机"})];

        // 默认 ON(未设)⇒ 注入从属指令,但 gm_notes/NPC 文本仍在。
        std::env::remove_var("TRPG_SCENE_STATIC_FRAMING_SUBORDINATE");
        assert!(scene_static_framing_subordinate_enabled(), "未设 ⇒ 默认 ON");
        let on = scene_node_to_blocks_with_opts("mod1", &n, &npcs, &[], false);
        let on_t = on[0].content.render_text();
        assert!(on_t.contains("场景静态参考"), "ON 应注入从属框定: {on_t}");
        assert!(on_t.contains("连续性锚"), "ON 应指向连续性锚为权威: {on_t}");
        assert!(
            on_t.contains("主动来到玩家当前所在处"),
            "ON 应含§4 relocation-toward-player: {on_t}"
        );
        assert!(
            on_t.contains("场景角色名册"),
            "ON 应注入 NPC 名册从属注记: {on_t}"
        );
        assert!(!on_t.contains("【GM】"), "ON 应改用从属 GM 头: {on_t}");
        assert!(
            on_t.contains("别无选择只能应战"),
            "ON 仍保留 gm_notes 文本(durable): {on_t}"
        );
        assert!(
            on_t.contains("雅典娜"),
            "ON 仍保留 NPC 文本(durable): {on_t}"
        );

        // OFF ⇒ gated body 与历史字节等价(原 read_aloud 锚提示 + 裸 【GM】 + 无名册注记)。
        for off in ["0", "false", "OFF", "no"] {
            std::env::set_var("TRPG_SCENE_STATIC_FRAMING_SUBORDINATE", off);
            assert!(!scene_static_framing_subordinate_enabled(), "{off} ⇒ OFF");
        }
        std::env::set_var("TRPG_SCENE_STATIC_FRAMING_SUBORDINATE", "off");
        let off = scene_node_to_blocks_with_opts("mod1", &n, &npcs, &[], false);
        let off_t = off[0].content.render_text();
        assert!(off_t.contains("【GM】"), "OFF 应保留原 GM 头: {off_t}");
        assert!(
            !off_t.contains("场景静态参考"),
            "OFF 不得注入从属指令: {off_t}"
        );
        assert!(
            !off_t.contains("场景角色名册"),
            "OFF 不得注入名册注记: {off_t}"
        );
        assert!(
            off_t.contains("请勿复述"),
            "OFF 仍保留 L-G read_aloud 锚提示: {off_t}"
        );

        match prev {
            Some(v) => std::env::set_var("TRPG_SCENE_STATIC_FRAMING_SUBORDINATE", v),
            None => std::env::remove_var("TRPG_SCENE_STATIC_FRAMING_SUBORDINATE"),
        }
        match prev_lm {
            Some(v) => std::env::set_var("TRPG_SCENE_FROZEN_FRAMING_SUPPRESS", v),
            None => std::env::remove_var("TRPG_SCENE_FROZEN_FRAMING_SUPPRESS"),
        }
    }

    /// L-M(主管批准的位置失忆结构杠杆彻底版):subordinate 路径默认 ON ⇒ **彻底压制** gm_notes 开场
    /// 散文正文 + NPC 到场散文(防 GM 把仓库交火与玩家既定位置叠加=硬失忆);仅留 NPC 名 + 位置权威
    /// 指令。显式 OFF ⇒ 退回 L-I 降格(保留散文)字节等价。durable 机制经 scene_mechanics 投,不验此。
    #[test]
    fn frozen_framing_suppress_flag_and_behavior() {
        let _lock = N3_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("TRPG_SCENE_FROZEN_FRAMING_SUPPRESS").ok();
        let prev_sub = std::env::var("TRPG_SCENE_STATIC_FRAMING_SUBORDINATE").ok();
        // subordinate 必须 ON(L-M 仅在 subordinate 路径生效);用默认 ON。
        std::env::remove_var("TRPG_SCENE_STATIC_FRAMING_SUBORDINATE");
        let mut n = ScenarioNode::default();
        n.node_id = "loc1".into();
        n.title = "仓库".into();
        n.read_aloud = Some("警笛把你引向第四街的小仓库……".into());
        n.gm_notes = Some("开场战斗:无人机被警方围攻，PC 别无选择只能应战。".into());
        n.extraction_status = SceneExtractionStatus::DeepExtracted;
        n.referenced_npc_ids = vec!["npc1".into()];
        let npcs = vec![serde_json::json!({"id":"npc1","name":"雅典娜","summary":"失控无人机"})];

        // 默认 ON(未设)⇒ 压制:gm_notes 开场散文 + NPC 到场散文均不投,仅留 NPC 名 + 位置权威。
        std::env::remove_var("TRPG_SCENE_FROZEN_FRAMING_SUPPRESS");
        assert!(scene_frozen_framing_suppress_enabled(), "未设 ⇒ 默认 ON");
        let on = scene_node_to_blocks_with_opts("mod1", &n, &npcs, &[], false);
        let on_t = on[0].content.render_text();
        assert!(
            !on_t.contains("别无选择只能应战"),
            "L-M ON 应彻底压制 gm_notes 开场散文正文: {on_t}"
        );
        assert!(
            !on_t.contains("失控无人机"),
            "L-M ON 应彻底压制 NPC 到场散文正文: {on_t}"
        );
        assert!(on_t.contains("雅典娜"), "L-M ON 仍留 NPC 名(名册): {on_t}");
        assert!(
            on_t.contains("连续性锚"),
            "L-M ON 应留位置权威指令(以连续性锚为准): {on_t}"
        );
        assert!(
            on_t.contains("此处不复述正文"),
            "L-M ON 应留不复述指令: {on_t}"
        );

        // OFF ⇒ 退回 L-I 降格:gm_notes/NPC 散文保留。
        std::env::set_var("TRPG_SCENE_FROZEN_FRAMING_SUPPRESS", "off");
        assert!(!scene_frozen_framing_suppress_enabled(), "off ⇒ OFF");
        let off = scene_node_to_blocks_with_opts("mod1", &n, &npcs, &[], false);
        let off_t = off[0].content.render_text();
        assert!(
            off_t.contains("别无选择只能应战"),
            "L-M OFF 应退回 L-I 保留 gm_notes 散文: {off_t}"
        );
        assert!(
            off_t.contains("失控无人机"),
            "L-M OFF 应退回 L-I 保留 NPC 散文: {off_t}"
        );

        match prev {
            Some(v) => std::env::set_var("TRPG_SCENE_FROZEN_FRAMING_SUPPRESS", v),
            None => std::env::remove_var("TRPG_SCENE_FROZEN_FRAMING_SUPPRESS"),
        }
        match prev_sub {
            Some(v) => std::env::set_var("TRPG_SCENE_STATIC_FRAMING_SUBORDINATE", v),
            None => std::env::remove_var("TRPG_SCENE_STATIC_FRAMING_SUBORDINATE"),
        }
    }

    #[test]
    fn skeleton_scene_projects_nothing() {
        let mut n = ScenarioNode::default();
        n.node_id = "loc2".into();
        n.extraction_status = SceneExtractionStatus::SkeletonOnly;
        // After N2: must return a SceneSkeleton fallback block, NOT empty.
        let blocks = scene_node_to_blocks("mod1", &n, &[], &[]);
        assert!(
            !blocks.is_empty(),
            "N2: SkeletonOnly 场景应产出降级块（非空）"
        );
    }

    /// N2: SkeletonOnly 降级块完整验收——不含编造内容，含骨架元数据与指令。
    #[test]
    fn skeleton_scene_fallback_block_has_no_readout_has_instruction() {
        use trpg_model::{BlockKind, CacheZone, LinkType, ScenarioLink};
        let mut n = ScenarioNode::default();
        n.node_id = "sc02".into();
        n.title = "镇中心广场".into();
        n.summary = "玩家抵达镇中心，这里是全镇的心脏。".into();
        n.page_start = Some(12);
        n.page_end = Some(14);
        n.referenced_npc_ids = vec!["npc_mayor".into()];
        n.referenced_clue_ids = vec!["clue_letter".into()];
        // 有出口链接
        n.links = vec![ScenarioLink {
            to_node_id: "sc03".into(),
            reason: "通往邮局".into(),
            clue_id: None,
            link_type: LinkType::Spatial,
            source_anchor: None,
        }];
        n.extraction_status = SceneExtractionStatus::SkeletonOnly;
        // 相邻场景用于解析出口标题
        let mut sc03 = ScenarioNode::default();
        sc03.node_id = "sc03".into();
        sc03.title = "邮局".into();
        let scenes = vec![n.clone(), sc03];

        let blocks = scene_node_to_blocks("mod_test", &n, &[], &scenes);
        assert!(!blocks.is_empty(), "SkeletonOnly 应产降级块");
        let b = &blocks[0];
        // 复用 SceneStatic kind，放 DynamicTail
        assert_eq!(b.kind, BlockKind::SceneStatic, "应复用 SceneStatic kind");
        assert_eq!(b.cache_zone, CacheZone::DynamicTail, "应放 DynamicTail");
        assert_eq!(b.scope.scope_type, trpg_model::ScopeType::Scene);
        assert_eq!(b.scope.scope_id, "sc02");
        assert_eq!(b.expires_at_scene.as_deref(), Some("sc02"));

        let text = b.content.render_text();
        // 含 summary / page 范围 / 出口 / 实体 id / 指令
        assert!(text.contains("镇中心广场"), "应含 title: {text}");
        assert!(text.contains("心脏"), "应含 summary: {text}");
        assert!(text.contains("12"), "应含 page_start: {text}");
        assert!(text.contains("14"), "应含 page_end: {text}");
        assert!(
            text.contains("npc_mayor"),
            "应含 referenced_npc_ids: {text}"
        );
        assert!(
            text.contains("clue_letter"),
            "应含 referenced_clue_ids: {text}"
        );
        assert!(text.contains("邮局"), "应含出口目标标题: {text}");
        assert!(
            text.contains("SkeletonOnly"),
            "应含 status=SkeletonOnly: {text}"
        );
        // 不含实际 read_aloud 内容——指令里提到 "read_aloud" 作关键词是允许的，
        // 但不应有 【可念】 标题（DeepExtracted 路径才产这个标题）。
        assert!(!text.contains("【可念】"), "不应含可念正文段落: {text}");
        // 含 GM 指令防编造
        assert!(
            text.contains("先检索") || text.contains("检索"),
            "应含检索指令: {text}"
        );
        assert!(text.contains("编造"), "应含禁止编造提示: {text}");
    }

    /// N2: DeepExtracted 路径行为不变（现有测试守护，新增专项冒烟）。
    #[test]
    fn deep_extracted_path_unchanged_after_n2() {
        let mut n = ScenarioNode::default();
        n.node_id = "loc1".into();
        n.title = "加油站".into();
        n.read_aloud = Some("你们看到一个褪色的广告牌……".into());
        n.gm_notes = Some("老板拉斯藏着钥匙。".into());
        n.extraction_status = SceneExtractionStatus::DeepExtracted;
        n.referenced_npc_ids = vec!["npc1".into()];
        let npcs = vec![serde_json::json!({"id":"npc1","name":"拉斯","summary":"老板"})];
        let blocks = scene_node_to_blocks("mod1", &n, &npcs, &[]);
        assert!(!blocks.is_empty());
        let text = blocks[0].content.render_text();
        assert!(
            text.contains("褪色的广告牌"),
            "DeepExtracted read_aloud 应存在: {text}"
        );
        assert!(text.contains("拉斯"), "DeepExtracted NPC 应存在: {text}");
        // 不含 SkeletonOnly 降级指令
        assert!(
            !text.contains("先检索"),
            "DeepExtracted 不应含降级指令: {text}"
        );
        assert!(
            !text.contains("SkeletonOnly"),
            "DeepExtracted 不应含 status 标记: {text}"
        );
    }

    #[test]
    fn deep_scene_block_includes_exit_titles() {
        use trpg_model::{LinkType, ScenarioLink, ScenarioNode, SceneExtractionStatus};
        let mut entry = ScenarioNode::default();
        entry.node_id = "loc1".into();
        entry.title = "加油站".into();
        entry.read_aloud = Some("你们停车。".into());
        entry.extraction_status = SceneExtractionStatus::DeepExtracted;
        entry.links = vec![ScenarioLink {
            to_node_id: "loc2".into(),
            reason: "主路通往".into(),
            clue_id: None,
            link_type: LinkType::Spatial,
            source_anchor: None,
        }];
        let mut town = ScenarioNode::default();
        town.node_id = "loc2".into();
        town.title = "镇中心".into();
        let scenes = vec![entry.clone(), town];
        let blocks = scene_node_to_blocks("m1", &entry, &[], &scenes);
        let body = match &blocks[0].content {
            trpg_model::BlockContent::Text(t) => t.clone(),
            _ => String::new(),
        };
        assert!(body.contains("镇中心"), "出口应含目标场景标题");
    }

    // ===== C3: 当前场景 intents 投影（BP2）=====

    fn mech_intent(id: &str) -> trpg_model::SceneMechanicIntent {
        trpg_model::SceneMechanicIntent {
            intent_id: id.into(),
            description: "玩家试图强行剪断缆线".into(),
            tested_parameter: "brawling".into(),
            difficulty: Some(serde_json::json!({"kind":"dv","value":13})),
            // 非空 effect_policy：若实现误把全文投影，下方 on_success 断言会抓住。
            effect_policy: trpg_model::EffectPolicy {
                on_success: vec![trpg_model::EffectPatchIntent::CreateFact {
                    target: "scene.lawmen".into(),
                    fact: serde_json::json!({"cable":"cut"}),
                }],
                on_failure: vec![],
            },
            source_anchor: "p.12 原文锚点".into(),
        }
    }

    fn deep_scene_with_intents(node_id: &str) -> ScenarioNode {
        let mut n = ScenarioNode::default();
        n.node_id = node_id.into();
        n.title = "执法者驾到".into();
        n.read_aloud = Some("警笛由远而近。".into());
        n.extraction_status = SceneExtractionStatus::DeepExtracted;
        n.scene_mechanics = vec![mech_intent("homecoming.lawmen.cut_cable_force")];
        n
    }

    #[test]
    fn scene_with_mechanics_projects_intents_block() {
        let n = deep_scene_with_intents("loc1");
        let blocks = scene_node_to_blocks("mod1", &n, &[], &[]);
        assert_eq!(blocks.len(), 2, "正文块 + intents 块");
        let b = &blocks[1];
        assert!(
            b.block_id.ends_with(".mechanics"),
            "block_id 应以 .mechanics 结尾: {}",
            b.block_id
        );
        assert_eq!(b.cache_zone, CacheZone::PinnedMiddle);
        assert_eq!(b.stability, trpg_model::Stability::SceneStable);
        assert_eq!(b.expires_at_scene.as_deref(), Some("loc1"));
        let text = b.content.render_text();
        assert!(
            text.contains("homecoming.lawmen.cut_cable_force"),
            "内容应含 intent_id: {text}"
        );
        assert!(
            !text.contains("effect_policy"),
            "effect_policy 全文不投影: {text}"
        );
        assert!(
            !text.contains("on_success"),
            "on_success 全文不投影: {text}"
        );
    }

    #[test]
    fn scene_without_mechanics_projects_single_block_unchanged() {
        let mut n = deep_scene_with_intents("loc1");
        n.scene_mechanics = Vec::new(); // 旧模组：无 intents
        let blocks = scene_node_to_blocks("mod1", &n, &[], &[]);
        assert_eq!(
            blocks.len(),
            1,
            "空 intents → 仍单块（旧模组零变化=fail-closed）"
        );
        let text = blocks[0].content.render_text();
        assert!(
            !text.contains("机制意图"),
            "首块内容不得混入机制意图: {text}"
        );
    }

    #[test]
    fn intents_block_bytes_stable_across_calls() {
        let n = deep_scene_with_intents("loc1");
        let first = scene_node_to_blocks("mod1", &n, &[], &[]);
        let second = scene_node_to_blocks("mod1", &n, &[], &[]);
        let a = serde_json::to_vec(&first[1]).expect("intents 块可序列化");
        let b = serde_json::to_vec(&second[1]).expect("intents 块可序列化");
        assert_eq!(
            a, b,
            "同节点两次投影的 intents 块字节必须一致（pinned_hash 场景内稳定的函数级前提）"
        );
    }

    // ===== N3: SceneStatic 缓存区可配 PinnedMiddle =====

    // env 是进程级全局：所有读/写 TRPG_SCENE_DEEP_BLOCK_CACHE_ZONE 的测试串行执行，
    // 且每个写测试用 guard 在退出时恢复原值，避免污染默认行为断言（并行跑）。
    // 锁现已提升到 crate 级 super::N3_ENV_LOCK，与 scene_need_resolver 的字节等价测试共用。
    use super::N3_ENV_LOCK;

    struct DeepZoneEnvGuard {
        prev: Option<String>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }
    impl DeepZoneEnvGuard {
        /// 持锁 + 把 env 设为 value，drop 时恢复原值（None→remove）。
        fn set(value: &str) -> Self {
            let lock = N3_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var("TRPG_SCENE_DEEP_BLOCK_CACHE_ZONE").ok();
            std::env::set_var("TRPG_SCENE_DEEP_BLOCK_CACHE_ZONE", value);
            Self { prev, _lock: lock }
        }
        /// 持锁 + 把 env 清空（模拟"未设"默认），drop 时恢复原值。
        fn unset() -> Self {
            let lock = N3_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var("TRPG_SCENE_DEEP_BLOCK_CACHE_ZONE").ok();
            std::env::remove_var("TRPG_SCENE_DEEP_BLOCK_CACHE_ZONE");
            Self { prev, _lock: lock }
        }
    }
    impl Drop for DeepZoneEnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var("TRPG_SCENE_DEEP_BLOCK_CACHE_ZONE", v),
                None => std::env::remove_var("TRPG_SCENE_DEEP_BLOCK_CACHE_ZONE"),
            }
        }
    }

    fn deep_scene_node(node_id: &str) -> ScenarioNode {
        let mut n = ScenarioNode::default();
        n.node_id = node_id.into();
        n.title = "加油站".into();
        n.read_aloud = Some("你们看到一个褪色的广告牌……".into());
        n.gm_notes = Some("老板拉斯藏着钥匙。".into());
        n.extraction_status = SceneExtractionStatus::DeepExtracted;
        n
    }

    /// N3 默认=零变化：env 未设时 DeepExtracted 正文块必须仍落 DynamicTail，
    /// 且块字节与"显式无 env"投影完全一致（字节回归，证明默认路径未被改动）。
    #[test]
    fn deep_block_default_zone_is_dynamic_tail_byte_regression() {
        let _g = DeepZoneEnvGuard::unset();
        let n = deep_scene_node("loc1");
        let blocks = scene_node_to_blocks("mod1", &n, &[], &[]);
        assert_eq!(
            blocks[0].cache_zone,
            CacheZone::DynamicTail,
            "默认（env 未设）DeepExtracted 正文块必须落 DynamicTail=零行为变化"
        );
        // expires_at_scene 保持不变
        assert_eq!(blocks[0].expires_at_scene.as_deref(), Some("loc1"));
    }

    /// N3：TRPG_SCENE_DEEP_BLOCK_CACHE_ZONE=pinned_middle → DeepExtracted 正文块落
    /// PinnedMiddle + 保留 expires_at_scene（切场景失效语义）。
    #[test]
    fn deep_block_pinned_middle_when_configured() {
        let _g = DeepZoneEnvGuard::set("pinned_middle");
        let n = deep_scene_node("loc1");
        let blocks = scene_node_to_blocks("mod1", &n, &[], &[]);
        let sb = &blocks[0];
        assert_eq!(
            sb.cache_zone,
            CacheZone::PinnedMiddle,
            "pinned_middle 配置下正文块应进 PinnedMiddle"
        );
        assert_eq!(
            sb.expires_at_scene.as_deref(),
            Some("loc1"),
            "仍带 expires_at_scene（切场景才失效）"
        );
        assert_eq!(sb.kind, BlockKind::SceneStatic);
        // 正文内容不变（仅缓存区变）
        let text = sb.content.render_text();
        assert!(text.contains("褪色的广告牌"));
        assert!(text.contains("钥匙"));
    }

    /// N3：大小写/前后空白容错走 pinned_middle；无法识别值 fail-closed 回退 DynamicTail。
    #[test]
    fn deep_block_zone_parsing_is_tolerant_and_fail_closed() {
        {
            let _g = DeepZoneEnvGuard::set("  Pinned_Middle  ");
            let n = deep_scene_node("loc1");
            let blocks = scene_node_to_blocks("mod1", &n, &[], &[]);
            assert_eq!(
                blocks[0].cache_zone,
                CacheZone::PinnedMiddle,
                "大小写+空白容错"
            );
        }
        {
            let _g = DeepZoneEnvGuard::set("garbage_value");
            let n = deep_scene_node("loc1");
            let blocks = scene_node_to_blocks("mod1", &n, &[], &[]);
            assert_eq!(
                blocks[0].cache_zone,
                CacheZone::DynamicTail,
                "无法识别值 fail-closed 回退 DynamicTail"
            );
        }
    }

    /// N3 不变量：SkeletonOnly 降级块（N2，临时降级）即便开 pinned_middle 也恒 DynamicTail。
    #[test]
    fn skeleton_fallback_stays_dynamic_tail_even_when_pinned_middle() {
        let _g = DeepZoneEnvGuard::set("pinned_middle");
        let mut n = ScenarioNode::default();
        n.node_id = "sk1".into();
        n.title = "未深抽场景".into();
        n.extraction_status = SceneExtractionStatus::SkeletonOnly;
        let blocks = scene_node_to_blocks("mod1", &n, &[], &[]);
        assert_eq!(
            blocks[0].cache_zone,
            CacheZone::DynamicTail,
            "SkeletonOnly 降级块是临时降级，不该进 pinned（恒 DynamicTail）"
        );
    }

    /// N3 缓存收益验证：pinned_middle 下，经 ContextBuilder.build 编译——
    /// 切场景 → pinned_hash 变；只改 DynamicTail 输入块 → pinned_hash 不变（dynamic_hash 变）。
    #[test]
    fn pinned_middle_scene_switch_changes_pinned_hash_input_does_not() {
        let _g = DeepZoneEnvGuard::set("pinned_middle");
        let req = ContextRequest {
            ruleset_id: "call_of_cthulhu_7e".into(),
            module_id: Some("mod1".into()),
            session_id: "s".into(),
            turn_id: "t".into(),
            viewer: VisibilityProfile::player("p", "pc.current"),
            token_budget: TokenBudget::default(),
        };
        let builder = crate::ContextBuilder;

        // 一个 DynamicTail 的"玩家输入"块（模拟每回合变化的尾部）。
        let input_block = |text: &str| {
            ContextBlock::new(
                "turn.player_input",
                BlockKind::SceneStatic,
                "玩家输入",
                BlockContent::Text(text.to_string()),
                Visibility::GmOnly,
                Stability::TurnDynamic,
                CacheZone::DynamicTail,
                Scope {
                    scope_type: ScopeType::Global,
                    scope_id: "*".into(),
                },
                10,
            )
        };

        let scene_a = deep_scene_node("loc1");
        let mut scene_b = deep_scene_node("loc2");
        scene_b.title = "镇中心".into();
        scene_b.read_aloud = Some("广场上人来人往。".into());

        let pinned_a = scene_node_to_blocks("mod1", &scene_a, &[], &[]);
        let pinned_b = scene_node_to_blocks("mod1", &scene_b, &[], &[]);
        // 确认正文块确实进了 PinnedMiddle（前提成立）
        assert_eq!(pinned_a[0].cache_zone, CacheZone::PinnedMiddle);
        assert_eq!(pinned_b[0].cache_zone, CacheZone::PinnedMiddle);

        let build = |scene_blocks: &[ContextBlock], input: ContextBlock| {
            let planned = crate::PlannedContext {
                prefix_blocks: vec![],
                pinned_blocks: scene_blocks.to_vec(),
                dynamic_blocks: vec![input],
            };
            builder.build(planned, &req).expect("compile ok")
        };

        let c1 = build(&pinned_a, input_block("我环顾四周"));
        let c2 = build(&pinned_a, input_block("我走向门口")); // 同场景，仅输入变
        let c3 = build(&pinned_b, input_block("我环顾四周")); // 切场景，输入回到原值

        // 1) 只改 DynamicTail 输入：pinned_hash 不变，dynamic_hash 变
        assert_eq!(
            c1.pinned_hash, c2.pinned_hash,
            "普通输入只动 dynamic_hash，pinned_hash 应稳定"
        );
        assert_ne!(
            c1.dynamic_hash, c2.dynamic_hash,
            "输入变 → dynamic_hash 应变"
        );
        // 2) 切场景：pinned_hash 应变（场景正文进了 pinned 区）
        assert_ne!(c1.pinned_hash, c3.pinned_hash, "切场景 → pinned_hash 应变");
    }

    /// N3 对照：默认 DynamicTail 下，切场景反而是 dynamic_hash 变、pinned_hash 不变
    /// （场景正文在 dynamic 区）——这正是规划里要改善的现状，作对照锚点保留。
    #[test]
    fn default_dynamic_tail_scene_switch_only_changes_dynamic_hash() {
        let _g = DeepZoneEnvGuard::unset();
        let req = ContextRequest {
            ruleset_id: "call_of_cthulhu_7e".into(),
            module_id: Some("mod1".into()),
            session_id: "s".into(),
            turn_id: "t".into(),
            viewer: VisibilityProfile::player("p", "pc.current"),
            token_budget: TokenBudget::default(),
        };
        let builder = crate::ContextBuilder;
        let scene_a = deep_scene_node("loc1");
        let mut scene_b = deep_scene_node("loc2");
        scene_b.read_aloud = Some("广场上人来人往。".into());
        let a = scene_node_to_blocks("mod1", &scene_a, &[], &[]);
        let b = scene_node_to_blocks("mod1", &scene_b, &[], &[]);
        assert_eq!(
            a[0].cache_zone,
            CacheZone::DynamicTail,
            "默认正文在 dynamic 区"
        );

        let build = |blk: &[ContextBlock]| {
            let planned = crate::PlannedContext {
                prefix_blocks: vec![],
                pinned_blocks: vec![],
                dynamic_blocks: blk.to_vec(),
            };
            builder.build(planned, &req).expect("compile ok")
        };
        let ca = build(&a);
        let cb = build(&b);
        assert_eq!(
            ca.pinned_hash, cb.pinned_hash,
            "默认下切场景 pinned_hash 不变（正文不在 pinned 区）"
        );
        assert_ne!(
            ca.dynamic_hash, cb.dynamic_hash,
            "默认下切场景动 dynamic_hash"
        );
    }

    #[test]
    fn module_entry_scene_id_prefers_spine_then_deep_then_first() {
        use trpg_model::{ModuleGraph, ScenarioNode, SceneExtractionStatus};
        let mk = |id: &str, st: SceneExtractionStatus| {
            let mut n = ScenarioNode::default();
            n.node_id = id.into();
            n.extraction_status = st;
            n
        };
        let mut g = ModuleGraph::default();
        g.scenes = vec![
            mk("preface", SceneExtractionStatus::SkeletonOnly),
            mk("prologue", SceneExtractionStatus::DeepExtracted),
        ];
        g.spine = serde_json::json!({"entry_node_id": "prologue"});
        assert_eq!(
            module_entry_scene_id(&g).as_deref(),
            Some("prologue"),
            "spine.entry_node_id 优先"
        );
        g.spine = serde_json::json!({});
        assert_eq!(
            module_entry_scene_id(&g).as_deref(),
            Some("prologue"),
            "无 spine → 首个 deep"
        );
        g.scenes[1].extraction_status = SceneExtractionStatus::SkeletonOnly;
        assert_eq!(
            module_entry_scene_id(&g).as_deref(),
            Some("preface"),
            "无 deep → scenes[0]"
        );
        assert_eq!(
            module_entry_scene_id(&ModuleGraph::default()),
            None,
            "空 → None"
        );
    }

    #[test]
    fn module_entry_scene_id_falls_back_to_spine_entry_hooks_when_scenes_empty() {
        use trpg_model::ModuleGraph;
        // graph.scenes 空，但 spine 有 entry_hooks（anthology/scenario_collection 形态）。
        let mut g = ModuleGraph::default();
        g.spine = serde_json::json!({
            "entry_hooks": [
                {"id": "vault_frame", "type": "campaign_frame"},
                {"id": "agency_assignment", "type": "standard_mission_start"},
                {"id": "se_briefing", "type": "mission_briefing", "mission": "Springs Eternal"}
            ]
        });
        assert_eq!(
            module_entry_scene_id(&g).as_deref(),
            Some("spine:se_briefing"),
            "scenes 空 → 偏好 mission_briefing 钩子"
        );
        // 无 mission_briefing → 退到 standard_mission_start。
        g.spine = serde_json::json!({
            "entry_hooks": [
                {"id": "vault_frame", "type": "campaign_frame"},
                {"id": "agency_assignment", "type": "standard_mission_start"}
            ]
        });
        assert_eq!(
            module_entry_scene_id(&g).as_deref(),
            Some("spine:agency_assignment"),
            "无 briefing → standard_mission_start"
        );
        // 都无优先类型 → 首个钩子。
        g.spine = serde_json::json!({
            "entry_hooks": [{"id": "only_hook", "type": "campaign_frame"}]
        });
        assert_eq!(
            module_entry_scene_id(&g).as_deref(),
            Some("spine:only_hook"),
            "无优先类型 → 首个钩子"
        );
        // spine 无 entry_hooks → 仍 None（保持旧行为）。
        g.spine = serde_json::json!({"synopsis": {}});
        assert_eq!(
            module_entry_scene_id(&g),
            None,
            "spine 无 entry_hooks → None"
        );
        // 有真实 scenes 时不走 spine 兜底（旧路径优先，字节不变）。
        let mut n = trpg_model::ScenarioNode::default();
        n.node_id = "real_scene".into();
        g.scenes = vec![n];
        g.spine = serde_json::json!({"entry_hooks": [{"id": "h", "type": "mission_briefing"}]});
        assert_eq!(
            module_entry_scene_id(&g).as_deref(),
            Some("real_scene"),
            "有真实 scenes → 不走 spine 兜底"
        );
    }

    #[test]
    fn resolve_turn_scene_id_loads_when_absent_with_module() {
        assert_eq!(
            resolve_turn_scene_id(Some("loc1"), Some("m"), Some("loaded".into())).as_deref(),
            Some("loc1"),
            "已有 scene → 不覆盖"
        );
        assert_eq!(
            resolve_turn_scene_id(None, Some("m"), Some("loaded".into())).as_deref(),
            Some("loaded"),
            "缺+有模组 → 载入"
        );
        assert_eq!(
            resolve_turn_scene_id(None, None, Some("loaded".into())),
            None,
            "无模组 → 不载"
        );
        assert_eq!(
            resolve_turn_scene_id(Some(""), Some("m"), Some("loaded".into())).as_deref(),
            Some("loaded"),
            "空串视为缺"
        );
    }

    #[test]
    fn map_check_param_need_selects_by_compare() {
        use trpg_model::SituationActionKind::*;
        // meet_or_beat:攻击族 → 被动 defense DV。
        assert_eq!(
            map_check_param_need(&Attack, "meet_or_beat"),
            Some(("stats".into(), "defense".into()))
        );
        // roll_under:攻击族 → 对抗技能 dodge。
        assert_eq!(
            map_check_param_need(&Attack, "roll_under"),
            Some(("skills".into(), "dodge".into()))
        );
        // 潜行/盗窃/对抗社交/冲突中调查 → perception(两种 compare 一致)。
        for ak in [Hide, Hack, Intimidate, InvestigateDuringConflict] {
            assert_eq!(
                map_check_param_need(&ak, "roll_under"),
                Some(("skills".into(), "perception".into()))
            );
        }
        // 不需 NPC 参数 → None。
        for ak in [AskQuestion, Move, LeaveScene, Unknown] {
            assert_eq!(map_check_param_need(&ak, "roll_under"), None);
            assert_eq!(map_check_param_need(&ak, "meet_or_beat"), None);
        }
    }

    #[test]
    fn passive_defense_maps_to_npc_attack_skill() {
        use trpg_model::SituationActionKind::*;
        // 被动防御:玩家防御/闪避/找掩护/被攻击时,是 NPC 在攻击你 → 对手该测的不是
        // 它的防御值,而是它的"攻击技能"(命中你的能力)。攻击在 roll_under 与
        // meet_or_beat 两套模型里都是技能桶,故两种 compare 一致;零规则集硬编码。
        for ak in [
            Defend,
            Dodge,
            TakeCover,
            UnderAttack,
            EnemyInitiatedConflict,
            SceneEntersConflict,
        ] {
            assert_eq!(
                map_check_param_need(&ak, "meet_or_beat"),
                Some(("skills".into(), "attack".into())),
                "{ak:?} 被动防御 meet_or_beat → 对手测攻击技能"
            );
            assert_eq!(
                map_check_param_need(&ak, "roll_under"),
                Some(("skills".into(), "attack".into())),
                "{ak:?} 被动防御 roll_under → 对手测攻击技能"
            );
        }
        // 回归守卫:主动攻击仍测对手防御值,方向不反。
        assert_eq!(
            map_check_param_need(&Attack, "meet_or_beat"),
            Some(("stats".into(), "defense".into()))
        );
        assert_eq!(
            map_check_param_need(&Counterattack, "roll_under"),
            Some(("skills".into(), "dodge".into()))
        );
        // CastOrUsePower = 主动放有害异能 → 仍走主动支(对手防御)。
        assert_eq!(
            map_check_param_need(&CastOrUsePower, "meet_or_beat"),
            Some(("stats".into(), "defense".into()))
        );
    }
}

#[cfg(test)]
mod stamp_tests {
    use super::*;
    use trpg_model::*;

    #[test]
    fn stamp_opposed_check_sets_target_and_opponent_param() {
        let mut check: CheckContract = serde_json::from_value(serde_json::json!({
            "check_id":"c","session_id":"s","turn_id":"t",
            "ruleset_id":"call_of_cthulhu_7e","module_id":null,
            "initiator":{"actor_id":"pc.current","actor_kind":"player_character","display_name":null},
            "target_actor":null,
            "opposition":{"kind":"no_mechanical_opposition"},
            "action_summary":"潜行","intent_kind":"hide","check_label":"潜行 check",
            "dice_expression":"1d100","modifiers":[],
            "target":{"kind":"unknown_until_lookup"},
            "tested_parameter":{"domain":null,"key":"Stealth","label":"Stealth"},
            "opponent_tested_parameter":null,
            "actor_snapshot_ids":[],"source_refs":[],"learned_packet_ids":[],
            "roll_visibility":"public_gm_roll","roll_authority":"system",
            "disclosure":{
                "show_roll_to_player":true,"show_formula_to_player":true,
                "show_dc_to_player":true,"show_success_failure_to_player":true,
                "reveal_after_scene":false,"reveal_after_session":false
            },
            "stakes":{
                "before_roll_public":"","success_public":"","failure_public":"",
                "critical_public":null,"fumble_public":null,
                "success_patches_allowed":[],"failure_patches_allowed":[],"irreversible":false
            },
            "confidence":"medium","ruling_status":"source_backed",
            "advice_refs":[],"expires_at_turn":null
        })).unwrap();
        let npc = crate::npc_synth::NpcPersona {
            actor_id: "npc.opposition".into(),
            name: "拉斯".into(),
            prose: "".into(),
        };
        stamp_opposed_check(&mut check, &npc, "skills", "perception");
        assert_eq!(
            check.target_actor.as_ref().map(|a| a.actor_id.as_str()),
            Some("npc.opposition")
        );
        assert_eq!(
            check
                .opponent_tested_parameter
                .as_ref()
                .map(|t| t.key.as_str()),
            Some("perception")
        );
    }
}

#[cfg(test)]
mod opposed_tests {
    use super::*;
    use trpg_model::*;

    #[test]
    fn contract_is_opposed_only_with_target_and_opponent_param() {
        let mut c: CheckContract = serde_json::from_value(serde_json::json!({
            "check_id":"c","session_id":"s","turn_id":"t","ruleset_id":"r","module_id":null,
            "initiator":{"actor_id":"pc.current","actor_kind":"player_character","display_name":null},
            "target_actor":null,"opposition":{"kind":"no_mechanical_opposition"},
            "action_summary":"","intent_kind":"","check_label":"","dice_expression":"1d100","modifiers":[],
            "target":{"kind":"unknown_until_lookup"},"tested_parameter":null,"opponent_tested_parameter":null,
            "actor_snapshot_ids":[],"source_refs":[],"learned_packet_ids":[],
            "roll_visibility":"public_gm_roll","roll_authority":"system",
            "disclosure":{"show_roll_to_player":true,"show_formula_to_player":true,"show_dc_to_player":true,"show_success_failure_to_player":true,"reveal_after_scene":false,"reveal_after_session":false},
            "stakes":{"before_roll_public":"","success_public":"","failure_public":"","critical_public":null,
                "fumble_public":null,"success_patches_allowed":[],"failure_patches_allowed":[],"irreversible":false},
            "confidence":"medium","ruling_status":"provisional","advice_refs":[],"expires_at_turn":null
        })).unwrap();
        assert!(!contract_is_opposed(&c));
        c.target_actor = Some(ActorRef {
            actor_id: "npc.opposition".into(),
            actor_kind: ActorKind::Npc,
            display_name: None,
        });
        assert!(!contract_is_opposed(&c), "只有 target_actor 还不够");
        c.opponent_tested_parameter = Some(TestedParameter {
            domain: None,
            key: "perception".into(),
            label: "perception".into(),
        });
        assert!(contract_is_opposed(&c));
    }
}

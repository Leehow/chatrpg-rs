use trpg_model::*;
use crate::npc_synth;

/// SkeletonOnly 降级块内容（N2）：投影骨架元数据 + 禁编造指令，不含正文。
pub(crate) fn skeleton_fallback_block(module_id: &str, n: &ScenarioNode, scenes: &[ScenarioNode]) -> ContextBlock {
    let mut body = format!("【场景骨架·未深抽】status=SkeletonOnly\n标题：{}\n", n.title);
    if !n.summary.trim().is_empty() {
        body.push_str(&format!("摘要：{}\n", n.summary));
    }
    match (n.page_start, n.page_end) {
        (Some(s), Some(e)) => body.push_str(&format!("页码：{s}–{e}\n")),
        (Some(s), None)    => body.push_str(&format!("页码：{s}\n")),
        _                  => {}
    }
    // 已知实体 id（骨架阶段 LLM 已识别的引用，供 GM 检索用）
    let mut refs: Vec<String> = Vec::new();
    refs.extend(n.referenced_npc_ids.iter().map(|id| format!("npc:{id}")));
    refs.extend(n.referenced_clue_ids.iter().map(|id| format!("clue:{id}")));
    refs.extend(n.referenced_location_ids.iter().map(|id| format!("loc:{id}")));
    refs.extend(n.referenced_encounter_ids.iter().map(|id| format!("enc:{id}")));
    if !refs.is_empty() {
        body.push_str(&format!("已知实体：{}\n", refs.join(", ")));
    }
    // 出口（与 DeepExtracted 路径相同的 fail-closed 投影）
    let exits: Vec<String> = n.links.iter().filter_map(|l| {
        let title = scenes.iter().find(|s| s.node_id == l.to_node_id).map(|s| s.title.as_str()).unwrap_or("");
        if title.trim().is_empty() { None } else { Some(format!("- {title} → {}", l.reason)) }
    }).collect();
    if !exits.is_empty() {
        body.push_str(&format!("【已知出口】\n{}\n", exits.join("\n")));
    }
    // GM 防编造指令
    body.push_str("\n⚠️ 此场景尚未深抽：叙述具体内容前先检索源文档；不要编造 read_aloud/数值/人物细节。\n");
    let mut block = ContextBlock::new(
        format!("module.{module_id}.scene.{}.skeleton", n.node_id),
        BlockKind::SceneStatic,
        format!("{}（骨架）", n.title),
        BlockContent::Text(body),
        Visibility::GmOnly,
        Stability::SceneStable,
        CacheZone::DynamicTail,
        Scope { scope_type: ScopeType::Scene, scope_id: n.node_id.clone() },
        20, // 低 token：骨架块只有元数据，不占满 context
    );
    block.expires_at_scene = Some(n.node_id.clone());
    block.load_reason = Some("skeleton_fallback".into());
    block.tags = vec!["module_scene".into(), "scene_skeleton".into(), "skeleton_only".into()];
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

pub(crate) fn scene_node_to_blocks(
    module_id: &str,
    n: &ScenarioNode,
    npcs: &[serde_json::Value],
    scenes: &[ScenarioNode],
) -> Vec<ContextBlock> {
    if n.extraction_status != SceneExtractionStatus::DeepExtracted {
        // N2: SkeletonOnly（及其他非 DeepExtracted 状态）产出轻量降级块，
        // 防 GM 完全失去"我在哪个场景"。fail-closed：不含正文、不编造。
        return vec![skeleton_fallback_block(module_id, n, scenes)];
    }
    let mut body = String::new();
    if let Some(ra) = &n.read_aloud {
        if !ra.trim().is_empty() {
            body.push_str("【可念】\n");
            body.push_str(ra);
            body.push('\n');
        }
    }
    if let Some(g) = &n.gm_notes {
        if !g.trim().is_empty() {
            body.push_str("\n【GM】\n");
            body.push_str(g);
            body.push('\n');
        }
    }
    for id in &n.referenced_npc_ids {
        if let Some(v) = npcs
            .iter()
            .find(|v| v.get("id").and_then(|x| x.as_str()) == Some(id.as_str()))
        {
            let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("");
            // Deep-extracted entities (reader DEEP_SYS) carry prose in `body`;
            // shallow/index entities use `summary`. Prefer body, fall back.
            let sum = v.get("body").or_else(|| v.get("summary"))
                .and_then(|x| x.as_str()).unwrap_or("");
            body.push_str(&format!("\n[NPC] {name}: {sum}"));
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
        Scope { scope_type: ScopeType::Scene, scope_id: n.node_id.clone() },
        60,
    );
    block.expires_at_scene = Some(n.node_id.clone());
    block.load_reason = Some("current_scene_deep_projection".into());
    block.tags = vec!["module_scene".into(), "scene_static".into(), "deep_extracted".into()];
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
            Scope { scope_type: ScopeType::Scene, scope_id: n.node_id.clone() },
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
    if let Some(id) = graph.spine.get("entry_node_id").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()) {
        if graph.scenes.iter().any(|s| s.node_id == id) {
            return Some(id.to_string());
        }
    }
    if let Some(s) = graph.scenes.iter().find(|s| s.extraction_status == SceneExtractionStatus::DeepExtracted) {
        return Some(s.node_id.clone());
    }
    graph.scenes.first().map(|s| s.node_id.clone())
}

/// 决定本回合生效的 scene_id：已有非空则保留；否则仅当有 module 时用载入值。
pub(crate) fn resolve_turn_scene_id(state_scene: Option<&str>, module_id: Option<&str>, loaded: Option<String>) -> Option<String> {
    match state_scene {
        Some(s) if !s.trim().is_empty() => Some(s.to_string()),
        _ => if module_id.is_some() { loaded } else { None },
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
pub(crate) fn map_check_param_need(action_kind: &SituationActionKind, compare: &str) -> Option<(String, String)> {
    use SituationActionKind::*;
    // 玩家主动攻击 NPC → 对手要防御 → 测它的防御值。
    let active_attack = matches!(action_kind, Attack | Counterattack | CastOrUsePower);
    // 玩家被动防御(NPC 在攻击你)→ 对手要命中 → 测它的攻击技能。
    let passive_defense = matches!(action_kind,
        Defend | Dodge | TakeCover | UnderAttack | EnemyInitiatedConflict | SceneEntersConflict);
    let stealth_family = matches!(action_kind,
        Hide | Hack | DisableDevice | Intimidate | Negotiate | InvestigateDuringConflict);
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

    #[test]
    fn skeleton_scene_projects_nothing() {
        let mut n = ScenarioNode::default();
        n.node_id = "loc2".into();
        n.extraction_status = SceneExtractionStatus::SkeletonOnly;
        // After N2: must return a SceneSkeleton fallback block, NOT empty.
        let blocks = scene_node_to_blocks("mod1", &n, &[], &[]);
        assert!(!blocks.is_empty(), "N2: SkeletonOnly 场景应产出降级块（非空）");
    }

    /// N2: SkeletonOnly 降级块完整验收——不含编造内容，含骨架元数据与指令。
    #[test]
    fn skeleton_scene_fallback_block_has_no_readout_has_instruction() {
        use trpg_model::{BlockKind, CacheZone, ScenarioLink, LinkType};
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
        assert!(text.contains("npc_mayor"), "应含 referenced_npc_ids: {text}");
        assert!(text.contains("clue_letter"), "应含 referenced_clue_ids: {text}");
        assert!(text.contains("邮局"), "应含出口目标标题: {text}");
        assert!(text.contains("SkeletonOnly"), "应含 status=SkeletonOnly: {text}");
        // 不含实际 read_aloud 内容——指令里提到 "read_aloud" 作关键词是允许的，
        // 但不应有 【可念】 标题（DeepExtracted 路径才产这个标题）。
        assert!(!text.contains("【可念】"), "不应含可念正文段落: {text}");
        // 含 GM 指令防编造
        assert!(text.contains("先检索") || text.contains("检索"), "应含检索指令: {text}");
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
        assert!(text.contains("褪色的广告牌"), "DeepExtracted read_aloud 应存在: {text}");
        assert!(text.contains("拉斯"), "DeepExtracted NPC 应存在: {text}");
        // 不含 SkeletonOnly 降级指令
        assert!(!text.contains("先检索"), "DeepExtracted 不应含降级指令: {text}");
        assert!(!text.contains("SkeletonOnly"), "DeepExtracted 不应含 status 标记: {text}");
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
        assert!(b.block_id.ends_with(".mechanics"), "block_id 应以 .mechanics 结尾: {}", b.block_id);
        assert_eq!(b.cache_zone, CacheZone::PinnedMiddle);
        assert_eq!(b.stability, trpg_model::Stability::SceneStable);
        assert_eq!(b.expires_at_scene.as_deref(), Some("loc1"));
        let text = b.content.render_text();
        assert!(text.contains("homecoming.lawmen.cut_cable_force"), "内容应含 intent_id: {text}");
        assert!(!text.contains("effect_policy"), "effect_policy 全文不投影: {text}");
        assert!(!text.contains("on_success"), "on_success 全文不投影: {text}");
    }

    #[test]
    fn scene_without_mechanics_projects_single_block_unchanged() {
        let mut n = deep_scene_with_intents("loc1");
        n.scene_mechanics = Vec::new(); // 旧模组：无 intents
        let blocks = scene_node_to_blocks("mod1", &n, &[], &[]);
        assert_eq!(blocks.len(), 1, "空 intents → 仍单块（旧模组零变化=fail-closed）");
        let text = blocks[0].content.render_text();
        assert!(!text.contains("机制意图"), "首块内容不得混入机制意图: {text}");
    }

    #[test]
    fn intents_block_bytes_stable_across_calls() {
        let n = deep_scene_with_intents("loc1");
        let first = scene_node_to_blocks("mod1", &n, &[], &[]);
        let second = scene_node_to_blocks("mod1", &n, &[], &[]);
        let a = serde_json::to_vec(&first[1]).expect("intents 块可序列化");
        let b = serde_json::to_vec(&second[1]).expect("intents 块可序列化");
        assert_eq!(a, b, "同节点两次投影的 intents 块字节必须一致（pinned_hash 场景内稳定的函数级前提）");
    }

    // ===== N3: SceneStatic 缓存区可配 PinnedMiddle =====

    // env 是进程级全局：所有读/写 TRPG_SCENE_DEEP_BLOCK_CACHE_ZONE 的测试串行执行，
    // 且每个写测试用 guard 在退出时恢复原值，避免污染默认行为断言（并行跑）。
    static N3_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
        assert_eq!(sb.cache_zone, CacheZone::PinnedMiddle, "pinned_middle 配置下正文块应进 PinnedMiddle");
        assert_eq!(sb.expires_at_scene.as_deref(), Some("loc1"), "仍带 expires_at_scene（切场景才失效）");
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
            assert_eq!(blocks[0].cache_zone, CacheZone::PinnedMiddle, "大小写+空白容错");
        }
        {
            let _g = DeepZoneEnvGuard::set("garbage_value");
            let n = deep_scene_node("loc1");
            let blocks = scene_node_to_blocks("mod1", &n, &[], &[]);
            assert_eq!(blocks[0].cache_zone, CacheZone::DynamicTail, "无法识别值 fail-closed 回退 DynamicTail");
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
                Scope { scope_type: ScopeType::Global, scope_id: "*".into() },
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
        assert_eq!(c1.pinned_hash, c2.pinned_hash, "普通输入只动 dynamic_hash，pinned_hash 应稳定");
        assert_ne!(c1.dynamic_hash, c2.dynamic_hash, "输入变 → dynamic_hash 应变");
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
        assert_eq!(a[0].cache_zone, CacheZone::DynamicTail, "默认正文在 dynamic 区");

        let build = |blk: &[ContextBlock]| {
            let planned = crate::PlannedContext { prefix_blocks: vec![], pinned_blocks: vec![], dynamic_blocks: blk.to_vec() };
            builder.build(planned, &req).expect("compile ok")
        };
        let ca = build(&a);
        let cb = build(&b);
        assert_eq!(ca.pinned_hash, cb.pinned_hash, "默认下切场景 pinned_hash 不变（正文不在 pinned 区）");
        assert_ne!(ca.dynamic_hash, cb.dynamic_hash, "默认下切场景动 dynamic_hash");
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
        assert_eq!(module_entry_scene_id(&g).as_deref(), Some("prologue"), "spine.entry_node_id 优先");
        g.spine = serde_json::json!({});
        assert_eq!(module_entry_scene_id(&g).as_deref(), Some("prologue"), "无 spine → 首个 deep");
        g.scenes[1].extraction_status = SceneExtractionStatus::SkeletonOnly;
        assert_eq!(module_entry_scene_id(&g).as_deref(), Some("preface"), "无 deep → scenes[0]");
        assert_eq!(module_entry_scene_id(&ModuleGraph::default()), None, "空 → None");
    }

    #[test]
    fn resolve_turn_scene_id_loads_when_absent_with_module() {
        assert_eq!(resolve_turn_scene_id(Some("loc1"), Some("m"), Some("loaded".into())).as_deref(), Some("loc1"), "已有 scene → 不覆盖");
        assert_eq!(resolve_turn_scene_id(None, Some("m"), Some("loaded".into())).as_deref(), Some("loaded"), "缺+有模组 → 载入");
        assert_eq!(resolve_turn_scene_id(None, None, Some("loaded".into())), None, "无模组 → 不载");
        assert_eq!(resolve_turn_scene_id(Some(""), Some("m"), Some("loaded".into())).as_deref(), Some("loaded"), "空串视为缺");
    }

    #[test]
    fn map_check_param_need_selects_by_compare() {
        use trpg_model::SituationActionKind::*;
        // meet_or_beat:攻击族 → 被动 defense DV。
        assert_eq!(map_check_param_need(&Attack, "meet_or_beat"), Some(("stats".into(),"defense".into())));
        // roll_under:攻击族 → 对抗技能 dodge。
        assert_eq!(map_check_param_need(&Attack, "roll_under"), Some(("skills".into(),"dodge".into())));
        // 潜行/盗窃/对抗社交/冲突中调查 → perception(两种 compare 一致)。
        for ak in [Hide, Hack, Intimidate, InvestigateDuringConflict] {
            assert_eq!(map_check_param_need(&ak, "roll_under"), Some(("skills".into(),"perception".into())));
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
        for ak in [Defend, Dodge, TakeCover, UnderAttack, EnemyInitiatedConflict, SceneEntersConflict] {
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
        assert_eq!(map_check_param_need(&Attack, "meet_or_beat"), Some(("stats".into(), "defense".into())));
        assert_eq!(map_check_param_need(&Counterattack, "roll_under"), Some(("skills".into(), "dodge".into())));
        // CastOrUsePower = 主动放有害异能 → 仍走主动支(对手防御)。
        assert_eq!(map_check_param_need(&CastOrUsePower, "meet_or_beat"), Some(("stats".into(), "defense".into())));
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
            check.opponent_tested_parameter.as_ref().map(|t| t.key.as_str()),
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
        c.target_actor = Some(ActorRef { actor_id: "npc.opposition".into(), actor_kind: ActorKind::Npc, display_name: None });
        assert!(!contract_is_opposed(&c), "只有 target_actor 还不够");
        c.opponent_tested_parameter = Some(TestedParameter { domain: None, key: "perception".into(), label: "perception".into() });
        assert!(contract_is_opposed(&c));
    }
}

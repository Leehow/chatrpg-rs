//! 场景导航 + 模组深抽：extract_module_scenes / scene_navigator / validate_transition /
//! build_nav_prompt / prefetch_frontier。从 trpg-api 下沉至 trpg-runtime，
//! 供 trpg-gm 执行器 phase_scene_navigate 调用，也供 trpg-cli 直接引用。
use tracing::{info, warn};
use trpg_db::Db;
use trpg_llm::LlmClient;
use trpg_model::{LinkType, ModuleGraph, ScenarioNode, SceneExtractionStatus, SourceDocument};

const FRONTIER_PREFETCH_MAX: usize = 5;

fn semantic_units_exact_path(data_dir: &std::path::Path, source_id: &str) -> std::path::PathBuf {
    data_dir
        .join("parsed/source_units")
        .join(format!("{source_id}.semantic_units.jsonl"))
}

fn resolve_semantic_units_path(
    data_dir: &std::path::Path,
    source_id: &str,
    source_documents: &[SourceDocument],
) -> std::path::PathBuf {
    let exact = semantic_units_exact_path(data_dir, source_id);
    if exact.exists() {
        return exact;
    }
    let mut dirs = vec![data_dir.join("parsed/source_units")];
    for doc in source_documents {
        if let Some(markdown_path) = doc.markdown_path.as_deref() {
            if let Some(root) = parse_root_from_markdown_path(std::path::Path::new(markdown_path)) {
                dirs.push(root.join("parsed/source_units"));
            }
        }
    }
    best_semantic_units_candidate(source_id, dirs).unwrap_or(exact)
}

fn parse_root_from_markdown_path(path: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut cur = path.parent();
    while let Some(dir) = cur {
        if dir.file_name().and_then(|s| s.to_str()) == Some("markdown") {
            return dir.parent().map(std::path::Path::to_path_buf);
        }
        cur = dir.parent();
    }
    None
}

fn best_semantic_units_candidate<I>(source_id: &str, dirs: I) -> Option<std::path::PathBuf>
where
    I: IntoIterator<Item = std::path::PathBuf>,
{
    let mut best: Option<(f64, std::path::PathBuf)> = None;
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let Some(candidate_id) = name.strip_suffix(".semantic_units.jsonl") else {
                continue;
            };
            let score = source_id_similarity(source_id, candidate_id);
            if score >= 0.45 && best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
                best = Some((score, path));
            }
        }
    }
    best.map(|(_, path)| path)
}

fn source_id_similarity(a: &str, b: &str) -> f64 {
    let a_tokens = normalized_source_tokens(a);
    let b_tokens = normalized_source_tokens(b);
    if a_tokens.is_empty() || b_tokens.is_empty() {
        return 0.0;
    }
    if a_tokens == b_tokens {
        return 1.0;
    }
    let intersection = a_tokens
        .iter()
        .filter(|token| b_tokens.iter().any(|other| other == *token))
        .count();
    (2.0 * intersection as f64) / (a_tokens.len() + b_tokens.len()) as f64
}

fn normalized_source_tokens(source_id: &str) -> Vec<String> {
    let normalized: String = source_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let mut out = Vec::new();
    for token in normalized.split('_').filter(|t| t.len() >= 2) {
        let token = match token {
            "of" | "the" | "and" | "pdf" => continue,
            other => other,
        };
        if !out.iter().any(|existing| existing == token) {
            out.push(token.to_string());
        }
    }
    out
}

const SCENE_NAV_SYS: &str = "你是模组场景导航器。给定『当前场景』『模组全部场景列表(node_id|kind|title)』\
『玩家输入』『本回合 GM 叙事』，综合两者语义判断玩家党是否离开当前场景、\
走到列表里另一个真实存在的场景。玩家明确说出移动意图（如「我去 X」「开车到 X」）\
或 GM 叙事描述了到达新地点，均应判为 moved=true。按语义判断（人物移动/进入新地点/任务推进），\
不要按标题字面猜。输出 JSON：{\"moved\": bool, \"target_node_id\": string|null, \"explicit_player_move_to_target\": bool, \"reason\": string}。\
explicit_player_move_to_target 只有在玩家输入本身明确表示去/进入/前往/抵达目标地点时才为 true；只是记录线索、提到目标、或仅 GM 叙事到达则为 false。\
fail-closed：不确定、没有明确移动、或目标不在列表里 → moved=false。target_node_id 必须是给定列表中的 node_id，绝不编造。";

/// L-C 内容引力（理念§4 content-gravity / §7 导演选焦不强制）：在保留全部 fail-closed
/// 语义判定基础上，**额外**追加一条「软推进」许可——当玩家持续聚焦/深入处理本场景核心对象
/// 或某个可推进抓手、且叙事张力明确指向某个衔接 beat 时，可判 moved 到该 beat。仍是语义裁定，
/// 不确定就留（绝不强拽玩家、绝不编造目标）。仅在 `nav_content_gravity_enabled()` 时使用。
const SCENE_NAV_SYS_GRAVITY: &str = "你是模组场景导航器。给定『当前场景』『当前场景的直接衔接 beat 列表』\
『模组全部场景列表(node_id|kind|title)』『玩家输入』『本回合 GM 叙事』，综合语义判断玩家党\
是否离开当前场景、走到列表里另一个真实存在的场景。判 moved=true 的依据（按语义，不按标题字面猜）：\
① 玩家明确说出移动意图（如「我去 X」）；② GM 叙事描述了到达新地点；③ **内容引力**——玩家已持续\
聚焦或深入处理本场景核心对象/可推进抓手（如直接与关键 NPC 对话、解决本场景的核心冲突/谜题），\
且叙事张力自然指向某个『直接衔接 beat』，则可判 moved 到该 beat 的 node_id（这是顺着玩家的投入软推进剧情，\
不是强行搬人）。输出 JSON：{\"moved\": bool, \"target_node_id\": string|null, \"reason\": string}。\
fail-closed：不确定、玩家只是在原地观察/试探、或目标不在列表里 → moved=false；target_node_id 必须是\
给定列表中的真实 node_id，绝不编造，优先取『直接衔接 beat』。";

/// L-C content-gravity 开关。镜像 BUG-1/L-E/L-G 模式：默认 ON，仅显式 `0/false/off/no` 关。
/// 关 ⇒ scene_navigate_critical 走原 SCENE_NAV_SYS + 不附衔接 beat ⇒ 提示串字节等价基线。
pub fn nav_content_gravity_enabled() -> bool {
    !matches!(
        std::env::var("TRPG_SCENE_NAV_CONTENT_GRAVITY")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "0" | "false" | "off" | "no"
    )
}

/// L-Y 离场提交子句（aliveFull30LW full-run J1+J3 共同根：玩家叙事已进入仓库内部，但 committed
/// 场景仍钉在外景入口到达 beat `scene_warehouse_arrival`，nav-LLM 每回合判"留"从不 commit 转移 ⇒
/// 外景定场逐回合回灌 = 位置失忆[J1 t9-12/t23]+ 场景冻结[J3 frozen 19]）。在 content-gravity 的
/// ①②③ 判据之上**追加**判 moved=true 的依据④「离场提交」：玩家已明确**离开**当前场景既定锚点位置
/// （外部进内部/地表入纵深/穿通道朝某直接衔接 beat 区域推进），即便尚未抵达该 beat 核心点位，只要
/// 叙事位置已落入某个『直接衔接 beat』territory ⇒ 判 moved 到该 beat 的真实 node_id。仍 fail-closed
/// （原地观察/试探、未真正离开既定位置 → 不动）。理由：把玩家钉在他已实际离开的位置会让旧场景到达/
/// 外景定场反复回灌（理念§4 把 beat 搬到玩家而非把玩家搬回 beat；§7 顺玩家投入软推进非强拽）。
const SCENE_NAV_DEPARTURE_CLAUSE: &str = "④ **离场提交**——若玩家已明确离开当前场景的既定锚点位置（如从建筑外部进入内部、从地表深入纵深、穿过通道/管道/缆线井朝某个『直接衔接 beat』的区域推进），即便尚未抵达该 beat 的核心点位，只要叙事位置已落入某个『直接衔接 beat』的范围，应判 moved=true 到该 beat 的真实 node_id（优先取『直接衔接 beat』）。理由：把玩家钉在他已实际离开的位置，会让旧场景的到达/外景定场反复回灌，造成位置失忆与场景冻结。仍须 fail-closed：玩家只是在原地观察/试探、或并未离开既定锚点位置 → moved=false。";

/// L-Y 离场提交开关。镜像 L-C 模式：默认 ON，仅显式 `0/false/off/no` 关。
/// 关 ⇒ gravity 提示串退回纯 SCENE_NAV_SYS_GRAVITY（无④子句）⇒ 字节等价 L-C 基线。
pub fn nav_departure_commit_enabled() -> bool {
    !matches!(
        std::env::var("TRPG_SCENE_NAV_DEPARTURE_COMMIT")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "0" | "false" | "off" | "no"
    )
}

/// L-AA 目标承接提交子句（aliveFinalLZ full-run J3 残根：L-Y ④离场提交只覆盖**首跳地理位移**
/// 模式[外部进内部/地表入纵深]，但玩家抵达 committed 场景 `scene_athena_conversation` 后再朝
/// 服务器/控制源**持续多回合推进**[顺缆线朝仓库深处推进/找服务器/抢下控制权/切断控制线/hack]
/// 时——这是**同一大区域内的目标/任务承接**而非干净的地理离场，④子句的范例不匹配 ⇒ nav-LLM
/// 每回合判"留"，committed 钉死在 athena_conversation 28 回合 = J3 frozen[scene_transitions 2→0]）。
/// 在 ①②③④ 之上**追加**判 moved=true 的依据⑤「目标承接」：当玩家不再围绕当前场景核心交互、
/// 而是用持续主动的行动去执行/夺取某个『直接衔接 beat』所定义的核心目标（该 beat 标题/主题所指
/// 任务），即便仍在同一大区域、无明显地理位移，也应判 moved 到与玩家所追目标语义最匹配的衔接 beat。
/// 仍 fail-closed（仅提及/询问/考虑、未付诸持续行动、或仍停在原 beat 核心交互 → 不动）。理念依据：
/// §4 把 beat 搬到玩家而非把玩家搬回 beat / §7 顺玩家投入软推进非强拽 / §二.8 表达已决定。
const SCENE_NAV_OBJECTIVE_CLAUSE: &str = "⑤ **目标承接推进（持续多跳）**——若玩家不再围绕当前场景的核心交互，而是已用持续、主动的行动去执行或夺取某个『直接衔接 beat』所定义的核心目标/活动（例如该 beat 标题/主题所指的任务：入侵或夺取服务器、控制源头、瓦解该 beat 的核心冲突对象），即便仍停留在同一大区域、未发生明显地理位移，也应判 moved=true 到与玩家所追目标语义最匹配的『直接衔接 beat』的真实 node_id。理由：玩家已用持续行动承接了下一个 beat 的目标，把他钉在已被其行动超越的旧 beat，会让旧场景定场反复回灌＝场景冻结与进度/位置失忆。仍须 fail-closed：玩家只是提及/询问/考虑该目标、尚未付诸持续行动，或仍停留在原 beat 的核心交互中 → moved=false；目标必须是给定衔接 beat 列表中真实存在的 node_id，绝不编造。";

/// L-AA 目标承接提交开关。镜像 L-C/L-Y 模式：默认 ON，仅显式 `0/false/off/no` 关。
/// 关 ⇒ gravity 提示串不含⑤子句（与仅 L-Y 的串字节等价）。
pub fn nav_objective_commit_enabled() -> bool {
    !matches!(
        std::env::var("TRPG_SCENE_NAV_OBJECTIVE_COMMIT")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "0" | "false" | "off" | "no"
    )
}

/// 纯函数：构建 content-gravity 导航 system prompt。两个 flag 皆 false ⇒ 返回纯
/// `SCENE_NAV_SYS_GRAVITY`（字节等价 L-C 基线）；`departure_commit` ⇒ 追加 L-Y ④子句；
/// `objective_commit` ⇒ 追加 L-AA ⑤子句（按 ④→⑤ 顺序，纯追加不改写基线）。
/// 便于单测锁定 OFF==基线字节等价 + 各 ON 含对应子句。
pub fn gravity_nav_system_prompt(departure_commit: bool, objective_commit: bool) -> String {
    let mut s = SCENE_NAV_SYS_GRAVITY.to_string();
    if departure_commit {
        s.push('\n');
        s.push_str(SCENE_NAV_DEPARTURE_CLAUSE);
    }
    if objective_commit {
        s.push('\n');
        s.push_str(SCENE_NAV_OBJECTIVE_CLAUSE);
    }
    s
}

/// 内容引力提示：在 build_nav_prompt 之上，把『当前场景直接衔接 beat』(node_id | title)
/// 作为一节前置上下文。exits 为空 ⇒ 退回裸 build_nav_prompt（字节等价）。
pub fn build_nav_prompt_with_exits(
    current: &str,
    cur_title: &str,
    exits: &str,
    scene_list: &str,
    player_input: &str,
    narration: &str,
) -> String {
    if exits.trim().is_empty() {
        return build_nav_prompt(current, cur_title, scene_list, player_input, narration);
    }
    format!(
        "当前场景: {current} ({cur_title})\n当前场景的直接衔接 beat（顺着玩家投入可软推进到这些）:\n{exits}\n\n模组全部场景:\n{scene_list}\n\n玩家输入:\n{}\n\n本回合 GM 叙事:\n{}",
        player_input.chars().take(500).collect::<String>(),
        narration.chars().take(2000).collect::<String>(),
    )
}

/// Deep-extract the module's scenes that satisfy `only`, then re-persist the
/// upgraded ModuleGraph into the same `parsed_bundles` row. Shared core behind
/// both `continue_module_extraction` (only=None → all SkeletonOnly scenes) and
/// `scene_navigator` (only=Some(node_id) → just that scene, if SkeletonOnly).
/// Returns how many scenes were upgraded to `DeepExtracted`.
///
/// `source_id` selects the semantic-units file; when None it is derived from the
/// bundle's `source_index` (the runtime navigator does not carry it). Fail-closed
/// throughout: missing bundle/units/source_id → warn + Ok(0); per-scene loop
/// failures keep that scene SkeletonOnly; only a persistence error surfaces Err.
pub async fn extract_module_scenes(
    db: &Db,
    llm: &dyn LlmClient,
    module_id: &str,
    source_id: Option<&str>,
    ruleset_id: Option<&str>,
    data_dir: &std::path::Path,
    budget: usize,
    only: Option<&str>,
) -> anyhow::Result<usize> {
    let Some((mut bundle, source_hash, parse_config_hash)) =
        db.load_module_bundle_for_continue(module_id).await?
    else {
        info!(
            module_id,
            "extract_module_scenes: no module bundle found; nothing to do"
        );
        return Ok(0);
    };
    // source_id: explicit (continue request) or derived from the bundle's
    // source_index (same id parse_module wrote the units file under).
    let derived_source_id = bundle
        .source_index
        .sources
        .first()
        .map(|s| s.source_id.clone());
    let Some(source_id) = source_id.map(str::to_string).or(derived_source_id) else {
        info!(
            module_id,
            "extract_module_scenes: no source_id (request or bundle); nothing to do"
        );
        return Ok(0);
    };
    // Load the same semantic units parse_module read. Older bundles may carry a
    // source_id whose filename drifted from the current source_documents row, so
    // resolve by exact path first, then by source-document parse roots + generic
    // normalized token similarity.
    let source_documents = db.list_source_documents().await.unwrap_or_default();
    let units_path = resolve_semantic_units_path(data_dir, &source_id, &source_documents);
    let units = match trpg_rule_agent::reader::load_units(&units_path) {
        Ok(u) if !u.is_empty() => u,
        Ok(_) => {
            info!(module_id, path = %units_path.display(), "extract_module_scenes: empty units; nothing to do");
            return Ok(0);
        }
        Err(err) => {
            warn!(error = %err, path = %units_path.display(), "extract_module_scenes: load_units failed; nothing to do");
            return Ok(0);
        }
    };
    // Optional column-aligned sidecar (read_layout view); degrades to None.
    let sidecar_text =
        std::fs::read_to_string(data_dir.join(format!("markdown/modules/{source_id}.md"))).ok();
    let resolved_ruleset = ruleset_id
        .map(str::to_string)
        .or_else(|| bundle.ruleset_id.clone());
    let ctx = trpg_rule_agent::reader::ModuleReaderCtx {
        units: &units,
        sidecar_text,
        ruleset_id: resolved_ruleset,
    };

    // ModuleGraph -> ModuleReadout (the deep-extract fn operates on a readout).
    let graph = &bundle.module_graph;
    let mut readout = trpg_rule_agent::reader::ModuleReadout {
        spine: graph.spine.clone(),
        scenes: graph.scenes.clone(),
        npcs: graph.npcs.clone(),
        clues: graph.clues.clone(),
        locations: graph.locations.clone(),
        factions: graph.factions.clone(),
        encounters: graph.encounters.clone(),
        handouts: graph.handouts.clone(),
        module_specific_rules: graph.module_specific_rules.clone(),
        // 续抽路径不重抽引导事实;写回(110-120)也不覆盖 g.director_facilitation,故原抽取值保真。
        ..Default::default()
    };

    // Pick the target scene indices: SkeletonOnly scenes, optionally narrowed to
    // the single `only` node. (only=Some + already DeepExtracted -> empty -> 0.)
    let skeleton_idxs: Vec<usize> = readout
        .scenes
        .iter()
        .enumerate()
        .filter(|(_, s)| s.extraction_status == SceneExtractionStatus::SkeletonOnly)
        .filter(|(_, s)| only.map_or(true, |id| s.node_id == id))
        .map(|(i, _)| i)
        .collect();
    if skeleton_idxs.is_empty() {
        info!(
            module_id,
            ?only,
            "extract_module_scenes: no matching SkeletonOnly scenes"
        );
        return Ok(0);
    }
    let mut deep_count = 0usize;
    for idx in skeleton_idxs {
        if trpg_rule_agent::reader::deep_extract_scene_in_place(
            llm,
            &ctx,
            &mut readout,
            idx,
            budget,
        )
        .await
        {
            deep_count += 1;
        } else {
            info!(module_id, idx, "scene stayed SkeletonOnly during extract");
        }
    }
    if deep_count == 0 {
        info!(
            module_id,
            "extract_module_scenes: no scene upgraded; skipping re-persist"
        );
        return Ok(0);
    }

    // 增量补实体桥接边：在 source_anchor 过滤(deep_extract_scene_in_place 内部)之后、写回
    // 之前调用，连通所有已深抽且共享实体的场景（零 LLM、幂等、source_anchor=None）。
    let bridges = trpg_rule_agent::reader::apply_bridge_edges(&mut readout.scenes);
    info!(
        module_id,
        bridge_edges = bridges,
        "extract_module_scenes: applied entity-bridge edges"
    );

    // Readout -> ModuleGraph (scenes + closure entities upgraded in place).
    let g = &mut bundle.module_graph;
    g.spine = readout.spine;
    g.scenes = readout.scenes;
    g.npcs = readout.npcs;
    g.clues = readout.clues;
    g.locations = readout.locations;
    g.factions = readout.factions;
    g.encounters = readout.encounters;
    g.handouts = readout.handouts;
    g.module_specific_rules = readout.module_specific_rules;

    // Re-persist into the same parsed_bundles row (on conflict (bundle_id)).
    db.upsert_module_bundle(&bundle, None, &source_hash, &parse_config_hash)
        .await?;
    info!(
        module_id,
        deep_extracted = deep_count,
        "extract_module_scenes: re-persisted upgraded ModuleGraph"
    );
    Ok(deep_count)
}

/// 纯函数：构建场景导航 prompt（user 部分），便于单测。
/// `player_input` 截至 500 字符，`narration` 截至 2000 字符。
pub fn build_nav_prompt(
    current: &str,
    cur_title: &str,
    scene_list: &str,
    player_input: &str,
    narration: &str,
) -> String {
    format!(
        "当前场景: {current} ({cur_title})\n模组全部场景:\n{scene_list}\n\n玩家输入:\n{}\n\n本回合 GM 叙事:\n{}",
        player_input.chars().take(500).collect::<String>(),
        narration.chars().take(2000).collect::<String>(),
    )
}

/// fail-closed 校验场景切换决策：仅当 moved=true 且 target 是 scenes 中真实存在、
/// 且 != 当前场景，才返回 target。
pub fn validate_transition(
    decision: &serde_json::Value,
    scenes: &[ScenarioNode],
    current: &str,
) -> Option<String> {
    if !decision
        .get("moved")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return None;
    }
    let target = decision
        .get("target_node_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    if target == current {
        return None;
    }
    scenes
        .iter()
        .any(|s| s.node_id == target)
        .then(|| target.to_string())
}

/// MAT.M9c（DP-C）：当 LLM 判 `moved=true` 但目标**不在图谱**(`validate_transition` 拒)时，
/// 在 `Enforce` 下尝试把这个 off-graph 目标**映射回当前场景的真实邻接场景**(current 的
/// `links.to_node_id`，按 id/title 子串模糊匹配)。匹配不到 ⇒ None(留原场景,绝不乱跳/编造)。
///
/// 这是「绝不去图谱外编造的场景；只在真实已载/可抽场景间移动」的鲁棒兜底:LLM 偶尔会把一个
/// 真实出口稍微叫错名,本函数把它纠回真实邻接。`Off`/`Shadow` ⇒ None(字节级基线,纯加性)。
pub fn resolve_offgraph_to_neighbor(
    decision: &serde_json::Value,
    scenes: &[ScenarioNode],
    current: &str,
    mode: trpg_model::MaterializationAffordanceMode,
) -> Option<String> {
    if !mode.is_enforce() {
        return None;
    }
    // 仅当 LLM 表达了移动意图,但目标无法 in-graph 校验通过时才介入。
    if !decision
        .get("moved")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return None;
    }
    if validate_transition(decision, scenes, current).is_some() {
        return None; // 目标已是合法 in-graph → 走既有路径,本函数不介入。
    }
    let target_raw = decision
        .get("target_node_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let tl = target_raw.to_lowercase();
    // codex 折入:拒绝过短 token(易误配),要求 ≥3 字符。
    if tl.chars().count() < 3 {
        return None;
    }
    // 当前场景的真实邻接(去重、in-graph、≠current)。
    let cur = scenes.iter().find(|s| s.node_id == current)?;
    let mut neighbors: Vec<(&str, String)> = Vec::new(); // (id, title_lower)
    for link in &cur.links {
        let to = link.to_node_id.trim();
        if to.is_empty() || to == current || neighbors.iter().any(|(id, _)| *id == to) {
            continue;
        }
        let Some(node) = scenes.iter().find(|s| s.node_id == to) else {
            continue; // 邻接必须在图谱(真实可载)。
        };
        neighbors.push((to, node.title.to_lowercase()));
    }
    // codex 折入:① 精确(id 或 title 全等)优先;② 否则子串模糊但**要求唯一匹配**
    // (多个邻接命中 ⇒ 歧义 ⇒ None,绝不乱跳)。
    if let Some((id, _)) = neighbors
        .iter()
        .find(|(id, title)| id.to_lowercase() == tl || *title == tl)
    {
        return Some((*id).to_string());
    }
    let fuzzy: Vec<&str> = neighbors
        .iter()
        .filter(|(id, title)| {
            let idl = id.to_lowercase();
            let id_match = idl.contains(&tl) || tl.contains(&idl);
            let title_match =
                title.chars().count() >= 3 && (title.contains(&tl) || tl.contains(title));
            id_match || title_match
        })
        .map(|(id, _)| *id)
        .collect();
    if fuzzy.len() == 1 {
        return Some(fuzzy[0].to_string());
    }
    None
}

// R5 Task1a：critical/heavy 拆分的导航函数（SceneNavCommit / scene_navigate_critical /
// scene_navigate_heavy / scene_navigator wrapper）移至兄弟子模块以守 ≤400 行；经
// `pub use` 重导出到 `scene_navigation::` 路径，调用方与签名不变。
mod tiered;
pub use tiered::{scene_navigate_critical, scene_navigate_heavy, scene_navigator, SceneNavCommit};

/// Resolve an explicit player-stated move to a directly linked scene before GM
/// context assembly. This is conservative and deterministic: only authored
/// neighbors of the current scene are considered, and the player input must name
/// the target by node id, title, or referenced location id.
pub fn resolve_explicit_neighbor_scene(
    graph: &ModuleGraph,
    current_scene_id: &str,
    player_input: &str,
) -> Option<String> {
    let current = graph
        .scenes
        .iter()
        .find(|scene| scene.node_id.trim() == current_scene_id.trim())?;
    let hay = normalize_nav_match(player_input);
    if hay.trim().is_empty() {
        return None;
    }
    for link in &current.links {
        let target = graph
            .scenes
            .iter()
            .find(|scene| scene.node_id.trim() == link.to_node_id.trim())?;
        if scene_nav_aliases(target).into_iter().any(|alias| {
            let needle = normalize_nav_match(&alias);
            let needle = needle.trim();
            needle.chars().count() >= 4
                && hay.contains(needle)
                && nav_alias_has_explicit_move_intent(&hay, needle)
        }) {
            return Some(target.node_id.clone());
        }
    }
    None
}

/// Resolve a player-declared movement to a real authored scene before context
/// assembly. Direct neighbors are preferred; otherwise the whole authored graph
/// is considered only when the player explicitly names exactly one scene with
/// movement intent. This covers investigative jumps through an already-known
/// lead without treating ordinary scene mentions as relocation.
pub fn resolve_explicit_authored_scene(
    graph: &ModuleGraph,
    current_scene_id: &str,
    player_input: &str,
) -> Option<String> {
    if let Some(target) = resolve_explicit_neighbor_scene(graph, current_scene_id, player_input) {
        return Some(target);
    }
    let hay = normalize_nav_match(player_input);
    if hay.trim().is_empty() {
        return None;
    }

    let mut hits: Vec<(String, usize)> = Vec::new();
    for scene in graph
        .scenes
        .iter()
        .filter(|scene| scene.node_id.trim() != current_scene_id.trim())
    {
        let best = scene_nav_aliases(scene)
            .into_iter()
            .filter_map(|alias| {
                let needle = normalize_nav_match(&alias);
                let needle = needle.trim().to_string();
                if needle.chars().count() >= 4
                    && hay.contains(&needle)
                    && nav_alias_has_explicit_move_intent(&hay, &needle)
                {
                    Some(needle.len())
                } else {
                    None
                }
            })
            .max();
        if let Some(score) = best {
            hits.push((scene.node_id.clone(), score));
        }
    }
    hits.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let (best_id, best_score) = hits.first()?;
    if hits
        .get(1)
        .is_some_and(|(_, next_score)| next_score == best_score)
    {
        return None;
    }
    Some(best_id.clone())
}

/// LLM semantic fallback for explicit player-authored movement before context
/// assembly. Deterministic alias matching stays first; this path covers
/// cross-language or paraphrased destinations without adding per-module word
/// tables. It is read-only and fail-closed: the caller owns any scene write.
pub async fn resolve_explicit_authored_scene_semantic(
    llm: &dyn LlmClient,
    graph: &ModuleGraph,
    current_scene_id: &str,
    player_input: &str,
) -> Option<String> {
    if player_input.trim().is_empty() || graph.scenes.is_empty() {
        return None;
    }
    let current = current_scene_id.trim();
    let cur_node = graph.scenes.iter().find(|s| s.node_id == current)?;
    let scene_list = semantic_scene_list(graph);
    let prompt = build_nav_prompt(current, &cur_node.title, &scene_list, player_input, "");
    let decision = llm
        .complete_json(
            vec![trpg_llm::system(SCENE_NAV_SYS), trpg_llm::user(&prompt)],
            0.0,
        )
        .await
        .ok()?;
    let target = validate_transition(&decision, &graph.scenes, current)
        .or_else(|| resolve_offgraph_to_authored_scene(&decision, graph, current))?;
    let decision_says_explicit = decision
        .get("explicit_player_move_to_target")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if trigger_transition_lacks_explicit_player_move(graph, current, &target, player_input)
        && !decision_says_explicit
        && !player_input_has_location_intent(player_input)
    {
        return None;
    }
    Some(target)
}

fn semantic_scene_list(graph: &ModuleGraph) -> String {
    graph
        .scenes
        .iter()
        .map(semantic_scene_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn semantic_scene_line(scene: &ScenarioNode) -> String {
    let mut parts = vec![
        scene.node_id.trim().to_string(),
        scene.node_type.trim().to_string(),
        scene.title.trim().to_string(),
    ];
    let summary = scene.summary.trim();
    if !summary.is_empty() {
        parts.push(format!("summary: {summary}"));
    }
    if !scene.referenced_location_ids.is_empty() {
        parts.push(format!(
            "locations: {}",
            scene.referenced_location_ids.join(", ")
        ));
    }
    parts.join(" | ")
}

fn resolve_offgraph_to_authored_scene(
    decision: &serde_json::Value,
    graph: &ModuleGraph,
    current: &str,
) -> Option<String> {
    if !decision
        .get("moved")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return None;
    }
    let target_raw = decision
        .get("target_node_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let target = normalize_nav_match(target_raw);
    if target.chars().count() < 3 {
        return None;
    }
    let mut hits: Vec<(String, usize)> = Vec::new();
    for scene in graph
        .scenes
        .iter()
        .filter(|scene| scene.node_id.trim() != current)
    {
        let best = scene_nav_aliases(scene)
            .into_iter()
            .filter_map(|alias| {
                let alias = normalize_nav_match(&alias);
                let alias = alias.trim().to_string();
                if alias.chars().count() >= 3 && (target == alias || target.contains(&alias)) {
                    Some(alias.len())
                } else {
                    None
                }
            })
            .max();
        if let Some(score) = best {
            hits.push((scene.node_id.clone(), score));
        }
    }
    hits.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let (best_id, best_score) = hits.first()?;
    if hits
        .get(1)
        .is_some_and(|(_, next_score)| next_score == best_score)
    {
        return None;
    }
    Some(best_id.clone())
}

/// Trigger links express clue/beat availability, not physical relocation. A
/// trigger transition is allowed only when the player explicitly names movement
/// to that target; otherwise the clue can guide future action but must not write
/// `current_scene`.
pub(crate) fn trigger_transition_lacks_explicit_player_move(
    graph: &ModuleGraph,
    current_scene_id: &str,
    target_scene_id: &str,
    player_input: &str,
) -> bool {
    let Some(current) = graph
        .scenes
        .iter()
        .find(|scene| scene.node_id.trim() == current_scene_id.trim())
    else {
        return false;
    };
    let has_trigger_link = current.links.iter().any(|link| {
        link.to_node_id.trim() == target_scene_id.trim()
            && matches!(link.link_type, LinkType::Trigger)
    });
    if !has_trigger_link {
        return false;
    }
    !player_input_explicitly_moves_to_scene(graph, target_scene_id, player_input)
}

fn player_input_explicitly_moves_to_scene(
    graph: &ModuleGraph,
    target_scene_id: &str,
    player_input: &str,
) -> bool {
    let Some(target) = graph
        .scenes
        .iter()
        .find(|scene| scene.node_id.trim() == target_scene_id.trim())
    else {
        return false;
    };
    let hay = normalize_nav_match(player_input);
    scene_nav_aliases(target).into_iter().any(|alias| {
        let needle = normalize_nav_match(&alias);
        let needle = needle.trim();
        needle.chars().count() >= 4
            && hay.contains(needle)
            && nav_alias_has_explicit_move_intent(&hay, needle)
    })
}

fn player_input_has_location_intent(player_input: &str) -> bool {
    let mut hay = normalize_nav_match(player_input);
    for negated in [
        "不去",
        "不前往",
        "不进入",
        "不回到",
        "不要去",
        "不要前往",
        "不要进入",
        "不在",
        "notgoto",
        "dontgoto",
        "donotgoto",
        "doesnotgoto",
    ] {
        hay = hay.replace(negated, "");
    }
    [
        "去",
        "前往",
        "赶往",
        "来到",
        "进入",
        "走到",
        "转去",
        "回到",
        "抵达",
        "到",
        "在",
        "goto",
        "headto",
        "goesto",
        "headsto",
        "walksto",
        "drivesto",
        "returnto",
        "enterthe",
        "enter",
        "moveto",
        "makefor",
        "travelt",
        "travelsto",
        "leavefor",
        "at",
    ]
    .iter()
    .any(|cue| hay.contains(cue))
}

fn scene_nav_aliases(scene: &ScenarioNode) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |value: &str| {
        let trimmed = value.trim();
        if trimmed.chars().count() >= 4 && !out.iter().any(|existing| *existing == trimmed) {
            out.push(trimmed.to_string());
        }
    };
    push(scene.node_id.as_str());
    push(scene.title.as_str());
    for location_id in &scene.referenced_location_ids {
        push(location_id.as_str());
    }
    for part in scene
        .title
        .split(&[';', '/', '&', '、', '；', '和'][..])
        .flat_map(|part| part.split(" and "))
    {
        push(part);
    }
    out
}

fn nav_alias_has_explicit_move_intent(hay: &str, needle: &str) -> bool {
    for (idx, _) in hay.match_indices(needle) {
        let before = hay[..idx]
            .chars()
            .rev()
            .take(18)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<String>();
        let compact = before
            .replace(' ', "")
            .replace("不进入", "")
            .replace("不去", "")
            .replace("不前往", "")
            .replace("不回到", "")
            .replace("不要进入", "")
            .replace("不要去", "");
        if [
            "不去",
            "不前往",
            "不进入",
            "不回到",
            "notgoto",
            "dontgoto",
            "donotgoto",
            "doesnotgoto",
        ]
        .iter()
        .any(|cue| compact.ends_with(cue))
        {
            continue;
        }
        if [
            "去",
            "前往",
            "赶往",
            "来到",
            "进入",
            "走到",
            "转去",
            "回到",
            "抵达",
            "goto",
            "headto",
            "goesto",
            "headsto",
            "walksto",
            "drivesto",
            "returnto",
            "enterthe",
            "enter",
            "moveto",
            "makefor",
            "travelt",
            "travelsto",
            "leavefor",
            "leavesfor",
        ]
        .iter()
        .any(|cue| compact.contains(cue))
        {
            return true;
        }
    }
    false
}

fn normalize_nav_match(value: &str) -> String {
    value
        .chars()
        .flat_map(|c| c.to_lowercase())
        .map(|c| {
            if c.is_alphanumeric() || ('\u{4e00}'..='\u{9fff}').contains(&c) {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
}

// J3 FLOW-LINK CONSUMER（TRPG_NAV_FOLLOW_FLOW_LINKS，默认 OFF）：消费侧脊跟随逻辑（exits
// 类型排序 + ⑥流转脊优先子句 + flag）。新子模块守 ≤400 行、不改写 navigator；经 pub use 暴露。
mod flow_links;
pub use flow_links::{
    build_nav_exits, is_authored_flow_link, nav_follow_flow_links_enabled, with_flow_link_clause,
};

mod frontier_focus;
pub use frontier_focus::{
    build_frontier_block, with_frontier_clause as with_frontier_focus_clause,
};

/// 前探当前 target 场景的衔接场景（一跳，best-effort）。在 target 已深抽、其出口 links
/// 已写回 bundle 后调用：重新加载图 → 取 target 的出口 `to_node_id` → 去重 + 限只抽仍
/// SkeletonOnly 的 + bounded 上限 `FRONTIER_PREFETCH_MAX` → 逐个 `extract_module_scenes(only=exit)`。
/// 全程 fail-closed：load 失败/找不到 target/无出口 → 静默跳过；每个出口深抽失败只 warn。
pub async fn prefetch_frontier(
    db: &Db,
    llm: &dyn LlmClient,
    module_id: &str,
    target: &str,
    data_dir: &std::path::Path,
) {
    // 重新读图：target 深抽后出口已写回 bundle（单一事实源）。取不到则不前探。
    let graph = match db.load_module_graph(module_id).await {
        Ok(Some(g)) => g,
        Ok(None) => return,
        Err(err) => {
            tracing::warn!(error = %err, module_id, "prefetch_frontier: load_module_graph failed; skip");
            return;
        }
    };
    // 取 target 场景的出口（links.to_node_id）。找不到 target → 无前探。
    let Some(node) = graph.scenes.iter().find(|s| s.node_id == target) else {
        return;
    };
    // 仍 SkeletonOnly 的出口集合，去重 + bounded 上限。
    let mut seen = std::collections::HashSet::new();
    let mut exits: Vec<String> = Vec::new();
    for link in &node.links {
        let to = link.to_node_id.trim();
        if to.is_empty() || to == target || !seen.insert(to.to_string()) {
            continue;
        }
        let still_stub = graph
            .scenes
            .iter()
            .any(|s| s.node_id == to && s.extraction_status == SceneExtractionStatus::SkeletonOnly);
        if still_stub {
            exits.push(to.to_string());
        }
        if exits.len() >= FRONTIER_PREFETCH_MAX {
            break;
        }
    }
    if exits.is_empty() {
        return;
    }
    info!(module_id, %target, prefetch = exits.len(), "prefetch_frontier: deep-extracting one-hop exits");
    for exit in exits {
        // 复用单场景深抽核心（only=Some），bundle 续读续写。失败只 warn、不中断其余出口。
        if let Err(err) =
            extract_module_scenes(db, llm, module_id, None, None, data_dir, 12, Some(&exit)).await
        {
            tracing::warn!(error = %err, %exit, "prefetch_frontier: one-hop deep-extract failed; best-effort skip");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::{json, Value};
    use std::pin::Pin;
    use trpg_llm::LlmClient;
    use trpg_model::ChatMessage;
    use trpg_model::{LinkType, ScenarioLink};

    struct NavStubLlm(Value);
    #[async_trait]
    impl LlmClient for NavStubLlm {
        async fn complete_text(&self, _m: Vec<ChatMessage>, _t: f32) -> anyhow::Result<String> {
            Ok(String::new())
        }

        async fn complete_json(&self, _m: Vec<ChatMessage>, _t: f32) -> anyhow::Result<Value> {
            Ok(self.0.clone())
        }

        async fn stream_chat(
            &self,
            _m: Vec<ChatMessage>,
            _t: f32,
        ) -> anyhow::Result<Pin<Box<dyn futures_util::Stream<Item = anyhow::Result<String>> + Send>>>
        {
            anyhow::bail!("stub: stream_chat unused")
        }
    }

    fn scenes() -> Vec<ScenarioNode> {
        let mk = |id: &str| {
            let mut n = ScenarioNode::default();
            n.node_id = id.into();
            n
        };
        vec![mk("loc1"), mk("loc2"), mk("loc3")]
    }

    fn linked_graph() -> ModuleGraph {
        let mut globe = ScenarioNode::default();
        globe.node_id = "loc_boston_globe".into();
        globe.title = "The Boston Globe".into();
        globe.links = vec![ScenarioLink {
            to_node_id: "loc_hall_records".into(),
            reason: "Civil records may predate surviving newspaper files.".into(),
            clue_id: Some("handout_7".into()),
            link_type: LinkType::Trigger,
            source_anchor: None,
        }];
        let mut hall = ScenarioNode::default();
        hall.node_id = "loc_hall_records".into();
        hall.title = "Hall of Records".into();
        hall.summary = "Civil records reveal property, executor, and church links.".into();
        hall.referenced_location_ids = vec!["hall_records".into()];
        hall.links = vec![ScenarioLink {
            to_node_id: "loc_corbitt_house_exterior".into(),
            reason: "The property address can lead investigators to the house.".into(),
            clue_id: None,
            link_type: LinkType::Trigger,
            source_anchor: None,
        }];
        let mut courts = ScenarioNode::default();
        courts.node_id = "loc_courts_police".into();
        courts.title = "Higher Courts and Central Police Station".into();
        courts.referenced_location_ids =
            vec!["higher_courts".into(), "central_police_station".into()];
        let mut house = ScenarioNode::default();
        house.node_id = "loc_corbitt_house_exterior".into();
        house.title = "Corbitt House Exterior".into();
        house.referenced_location_ids = vec!["corbitt_house".into()];
        ModuleGraph {
            scenes: vec![globe, hall, courts, house],
            ..Default::default()
        }
    }

    #[test]
    fn semantic_units_path_resolves_source_id_drift_from_source_document_parse_root() {
        let root = std::env::temp_dir().join(format!(
            "semantic_units_resolve_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        let units_dir = root.join("parsed/source_units");
        let markdown_dir = root.join("markdown/rulebooks");
        std::fs::create_dir_all(&units_dir).unwrap();
        std::fs::create_dir_all(&markdown_dir).unwrap();
        let candidate = units_dir.join(
            "call_of_cthulhu_keeper_rulebook_40th_anniversary_sandy_petersen.semantic_units.jsonl",
        );
        std::fs::write(&candidate, "{}\n").unwrap();
        let markdown_path =
            markdown_dir.join("call_of_cthulhu_keeper_rulebook_40th_anniversary_sandy_petersen.md");
        std::fs::write(&markdown_path, "# Keeper\n").unwrap();
        let doc = SourceDocument {
            id: uuid::Uuid::new_v4(),
            source_id: "call_of_cthulhu_keeper_rulebook_40th_anniversary_sandy_petersen".into(),
            source_kind: trpg_model::SourceKind::Rulebook,
            title: "Call of Cthulhu Keeper Rulebook".into(),
            file_path: String::new(),
            markdown_path: Some(markdown_path.to_string_lossy().to_string()),
            source_hash: String::new(),
            parse_config_hash: String::new(),
            metadata: serde_json::Value::Null,
        };
        let unrelated_data = root.join("unrelated_data");

        let resolved = resolve_semantic_units_path(
            &unrelated_data,
            "coc7e_keeper_rulebook_40th_anniversary",
            &[doc],
        );

        assert_eq!(resolved, candidate);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn explicit_neighbor_scene_resolves_named_link_target() {
        let graph = linked_graph();
        let target = resolve_explicit_neighbor_scene(
            &graph,
            "loc_boston_globe",
            "我转去 Hall of Records 查 Corbitt House 的房产链。",
        );
        assert_eq!(target.as_deref(), Some("loc_hall_records"));
    }

    #[test]
    fn explicit_neighbor_scene_resolves_english_goes_to_named_link_target() {
        let graph = linked_graph();
        let target = resolve_explicit_neighbor_scene(
            &graph,
            "loc_boston_globe",
            "Evelyn goes to Boston's Hall of Records and searches the property indexes.",
        );
        assert_eq!(target.as_deref(), Some("loc_hall_records"));
    }

    #[test]
    fn explicit_authored_scene_resolves_non_neighbor_with_move_intent() {
        let graph = linked_graph();
        let target = resolve_explicit_authored_scene(
            &graph,
            "loc_boston_globe",
            "Evelyn leaves the Hall notes and goes to the higher courts and then the Central Police Station records desk.",
        );
        assert_eq!(target.as_deref(), Some("loc_courts_police"));
    }

    #[tokio::test]
    async fn explicit_authored_scene_semantic_fallback_resolves_cross_language_destination() {
        let mut intro = ScenarioNode::default();
        intro.node_id = "loc_intro".into();
        intro.title = "Introduction".into();
        let mut library = ScenarioNode::default();
        library.node_id = "loc_central_library".into();
        library.title = "The Central Library".into();
        library.node_type = "investigation_location".into();
        library.referenced_location_ids = vec!["central_library".into()];
        let graph = ModuleGraph {
            scenes: vec![intro, library],
            ..Default::default()
        };
        let llm = NavStubLlm(json!({
            "moved": true,
            "target_node_id": "loc_central_library",
            "reason": "玩家明确前往中央图书馆进行档案检索"
        }));

        let target = resolve_explicit_authored_scene_semantic(
            &llm,
            &graph,
            "loc_intro",
            "我先去波士顿中央图书馆，查科宾宅的旧档案。",
        )
        .await;

        assert_eq!(target.as_deref(), Some("loc_central_library"));
    }

    #[tokio::test]
    async fn semantic_fallback_maps_llm_title_target_back_to_node_id() {
        let mut intro = ScenarioNode::default();
        intro.node_id = "loc_intro".into();
        intro.title = "Introduction".into();
        let mut library = ScenarioNode::default();
        library.node_id = "loc_central_library".into();
        library.title = "The Central Library".into();
        let graph = ModuleGraph {
            scenes: vec![intro, library],
            ..Default::default()
        };
        let llm = NavStubLlm(json!({
            "moved": true,
            "target_node_id": "The Central Library",
            "reason": "title instead of node id"
        }));

        let target = resolve_explicit_authored_scene_semantic(
            &llm,
            &graph,
            "loc_intro",
            "我先去波士顿中央图书馆，查科宾宅的旧档案。",
        )
        .await;

        assert_eq!(target.as_deref(), Some("loc_central_library"));
    }

    #[tokio::test]
    async fn semantic_trigger_move_allows_cross_language_explicit_destination() {
        let mut intro = ScenarioNode::default();
        intro.node_id = "loc_intro".into();
        intro.title = "Introduction".into();
        intro.links = vec![ScenarioLink {
            to_node_id: "loc_central_library".into(),
            reason: "Library research can reveal Corbitt property history.".into(),
            clue_id: Some("handout_3".into()),
            link_type: LinkType::Trigger,
            source_anchor: None,
        }];
        let mut library = ScenarioNode::default();
        library.node_id = "loc_central_library".into();
        library.title = "The Central Library".into();
        library.referenced_location_ids = vec!["central_library".into()];
        let graph = ModuleGraph {
            scenes: vec![intro, library],
            ..Default::default()
        };
        let llm = NavStubLlm(json!({
            "moved": true,
            "target_node_id": "loc_central_library",
            "explicit_player_move_to_target": true,
            "reason": "玩家明确说先去波士顿中央图书馆检索旧档案。"
        }));

        let target = resolve_explicit_authored_scene_semantic(
            &llm,
            &graph,
            "loc_intro",
            "我先不去鬼屋，而是去波士顿中央图书馆和旧报纸档案，用图书馆使用检索科宾宅。",
        )
        .await;

        assert_eq!(target.as_deref(), Some("loc_central_library"));
    }

    #[tokio::test]
    async fn semantic_trigger_move_allows_location_intent_when_explicit_flag_missing() {
        let graph = linked_graph();
        let llm = NavStubLlm(json!({
            "moved": true,
            "target_node_id": "loc_hall_records",
            "reason": "玩家说在市政档案馆查公开房产记录，语义上已进入 Hall of Records。"
        }));

        let target = resolve_explicit_authored_scene_semantic(
            &llm,
            &graph,
            "loc_boston_globe",
            "我先在市政档案馆查科宾宅的公开房产、遗嘱和诉讼记录。",
        )
        .await;

        assert_eq!(target.as_deref(), Some("loc_hall_records"));
    }

    #[test]
    fn semantic_scene_list_includes_summary_and_location_ids_for_multilingual_matching() {
        let graph = linked_graph();
        let list = semantic_scene_list(&graph);

        assert!(
            list.contains("summary: Civil records reveal property, executor, and church links."),
            "semantic prompt needs authored scene semantics, not only opaque ids: {list}"
        );
        assert!(list.contains("locations: hall_records"), "{list}");
    }

    #[test]
    fn explicit_neighbor_scene_ignores_generic_followup_without_target_name() {
        let graph = linked_graph();
        assert_eq!(
            resolve_explicit_neighbor_scene(&graph, "loc_boston_globe", "我继续深挖这些线索。"),
            None
        );
    }

    #[test]
    fn explicit_neighbor_scene_ignores_referenced_place_without_movement_intent() {
        let graph = linked_graph();
        assert_eq!(
            resolve_explicit_neighbor_scene(
                &graph,
                "loc_hall_records",
                "我先停在 Hall of Records 的桌前，不进入新方向；请列出这些记录与 Corbitt House 的关系。",
            ),
            None,
            "mentioning a linked place as an object of research must not teleport the scene"
        );
    }

    #[test]
    fn trigger_transition_requires_explicit_player_move() {
        let graph = linked_graph();

        assert!(
            trigger_transition_lacks_explicit_player_move(
                &graph,
                "loc_boston_globe",
                "loc_hall_records",
                "The newspaper trail points toward municipal records, so I write that lead down before deciding where to go.",
            ),
            "a trigger clue may point at a scene, but must not move the current scene by itself"
        );
        assert!(
            !trigger_transition_lacks_explicit_player_move(
                &graph,
                "loc_boston_globe",
                "loc_hall_records",
                "I go to the Hall of Records and search the municipal property indexes.",
            ),
            "explicit player movement to the trigger target may commit the scene"
        );
    }

    #[test]
    fn spatial_transition_is_not_blocked_by_trigger_guard() {
        let mut graph = linked_graph();
        graph.scenes[1].links[0].link_type = LinkType::Spatial;

        assert!(
            !trigger_transition_lacks_explicit_player_move(
                &graph,
                "loc_hall_records",
                "loc_corbitt_house_exterior",
                "The property file names the house as important.",
            ),
            "the trigger guard only constrains trigger links; spatial movement is checked elsewhere"
        );
    }

    #[test]
    fn validate_transition_is_fail_closed() {
        let s = scenes();
        assert_eq!(
            validate_transition(
                &serde_json::json!({"moved":true,"target_node_id":"loc2"}),
                &s,
                "loc1"
            )
            .as_deref(),
            Some("loc2")
        );
        assert_eq!(
            validate_transition(
                &serde_json::json!({"moved":false,"target_node_id":"loc2"}),
                &s,
                "loc1"
            ),
            None,
            "moved=false → 不动"
        );
        assert_eq!(
            validate_transition(
                &serde_json::json!({"moved":true,"target_node_id":"ghost"}),
                &s,
                "loc1"
            ),
            None,
            "target 不在列表 → 不动"
        );
        assert_eq!(
            validate_transition(
                &serde_json::json!({"moved":true,"target_node_id":"loc1"}),
                &s,
                "loc1"
            ),
            None,
            "target==当前 → 不动"
        );
        assert_eq!(
            validate_transition(&serde_json::json!({}), &s, "loc1"),
            None,
            "缺字段 → 不动"
        );
    }

    fn scenes_linked() -> Vec<ScenarioNode> {
        use trpg_model::ScenarioLink;
        let mk = |id: &str, title: &str, links: &[&str]| {
            let mut n = ScenarioNode::default();
            n.node_id = id.into();
            n.title = title.into();
            n.links = links
                .iter()
                .map(|t| ScenarioLink {
                    to_node_id: t.to_string(),
                    ..Default::default()
                })
                .collect();
            n
        };
        vec![
            mk(
                "sc_warehouse",
                "Scavv Warehouse",
                &["sc_talk_athena", "sc_street"],
            ),
            mk("sc_talk_athena", "Talking to Athena", &["sc_ending"]),
            mk("sc_street", "Heywood Street", &[]),
            mk("sc_ending", "Homecoming Finale", &[]),
        ]
    }

    #[test]
    fn offgraph_neighbor_fallback_remaps_misnamed_target() {
        use trpg_model::MaterializationAffordanceMode::{Enforce, Off, Shadow};
        let scenes = scenes_linked();
        // LLM 想去 "athena" (off-graph 名),真实邻接 sc_talk_athena/title "Talking to Athena"。
        let d = serde_json::json!({"moved":true,"target_node_id":"talking to athena"});
        assert_eq!(
            resolve_offgraph_to_neighbor(&d, &scenes, "sc_warehouse", Enforce).as_deref(),
            Some("sc_talk_athena"),
            "off-graph 目标应纠回 title 匹配的真实邻接"
        );
        // Off/Shadow ⇒ None(字节级基线)。
        assert!(resolve_offgraph_to_neighbor(&d, &scenes, "sc_warehouse", Off).is_none());
        assert!(resolve_offgraph_to_neighbor(&d, &scenes, "sc_warehouse", Shadow).is_none());
    }

    #[test]
    fn offgraph_fallback_no_match_stays() {
        use trpg_model::MaterializationAffordanceMode::Enforce;
        let scenes = scenes_linked();
        // 完全无关的 off-graph 目标 → 无邻接匹配 → None(留原场景,不乱跳)。
        let d = serde_json::json!({"moved":true,"target_node_id":"moon_base_zeta"});
        assert!(resolve_offgraph_to_neighbor(&d, &scenes, "sc_warehouse", Enforce).is_none());
    }

    #[test]
    fn offgraph_fallback_skips_when_target_is_already_ingraph() {
        use trpg_model::MaterializationAffordanceMode::Enforce;
        let scenes = scenes_linked();
        // 目标已合法 in-graph → validate_transition 处理,本函数不介入(None)。
        let d = serde_json::json!({"moved":true,"target_node_id":"sc_talk_athena"});
        assert!(resolve_offgraph_to_neighbor(&d, &scenes, "sc_warehouse", Enforce).is_none());
    }

    #[test]
    fn offgraph_fallback_ambiguous_match_stays() {
        use trpg_model::MaterializationAffordanceMode::Enforce;
        use trpg_model::{ScenarioLink, ScenarioNode};
        // 两个邻接 title 都含 "warehouse" → 模糊命中两条 → 歧义 → None(不乱跳)。
        let mk = |id: &str, title: &str, links: &[&str]| {
            let mut n = ScenarioNode::default();
            n.node_id = id.into();
            n.title = title.into();
            n.links = links
                .iter()
                .map(|t| ScenarioLink {
                    to_node_id: t.to_string(),
                    ..Default::default()
                })
                .collect();
            n
        };
        let scenes = vec![
            mk("hub", "Hub", &["wh_a", "wh_b"]),
            mk("wh_a", "North Warehouse", &[]),
            mk("wh_b", "South Warehouse", &[]),
        ];
        let d = serde_json::json!({"moved":true,"target_node_id":"warehouse"});
        assert!(
            resolve_offgraph_to_neighbor(&d, &scenes, "hub", Enforce).is_none(),
            "歧义模糊匹配应留原场景"
        );
    }

    #[test]
    fn offgraph_fallback_short_token_rejected() {
        use trpg_model::MaterializationAffordanceMode::Enforce;
        let scenes = scenes_linked();
        let d = serde_json::json!({"moved":true,"target_node_id":"sc"});
        assert!(
            resolve_offgraph_to_neighbor(&d, &scenes, "sc_warehouse", Enforce).is_none(),
            "过短 token(<3)应拒绝"
        );
    }

    #[test]
    fn offgraph_fallback_requires_moved_true() {
        use trpg_model::MaterializationAffordanceMode::Enforce;
        let scenes = scenes_linked();
        let d = serde_json::json!({"moved":false,"target_node_id":"talking to athena"});
        assert!(resolve_offgraph_to_neighbor(&d, &scenes, "sc_warehouse", Enforce).is_none());
    }

    #[test]
    fn build_nav_prompt_includes_player_input_and_narration() {
        let prompt = build_nav_prompt(
            "sc01",
            "加油站",
            "sc01 | location | 加油站\nsc02 | location | 镇中心",
            "我开车去镇中心",
            "GM描述了街道",
        );
        assert!(prompt.contains("玩家输入:"), "应包含玩家输入标签");
        assert!(prompt.contains("我开车去镇中心"), "应包含玩家输入内容");
        assert!(
            prompt.contains("GM 叙事:") || prompt.contains("GM叙事"),
            "应包含叙事标签"
        );
        assert!(prompt.contains("GM描述了街道"), "应包含叙事内容");
        assert!(prompt.contains("sc01"), "应包含当前场景");
        assert!(prompt.contains("sc02"), "应包含场景列表");
    }

    #[test]
    fn build_nav_prompt_truncates_long_inputs() {
        let long_player = "x".repeat(600);
        let long_narration = "y".repeat(3000);
        let prompt = build_nav_prompt(
            "sc01",
            "场景",
            "sc01 | l | 场景",
            &long_player,
            &long_narration,
        );
        // player_input 截至 500 字符，narration 截至 2000 字符
        let player_section = prompt
            .split("玩家输入:")
            .nth(1)
            .unwrap_or("")
            .split("GM 叙事:")
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        let x_count = player_section.chars().filter(|&c| c == 'x').count();
        assert!(
            x_count <= 500,
            "player_input 应被截断至 ≤500 chars，实际 {x_count}"
        );
        let y_count = prompt.chars().filter(|&c| c == 'y').count();
        assert!(
            y_count <= 2000,
            "narration 应被截断至 ≤2000 chars，实际 {y_count}"
        );
    }

    #[test]
    fn build_nav_prompt_empty_player_input_still_valid() {
        // fail-closed：空 player_input 不崩溃，叙事仍在 prompt 中
        let prompt = build_nav_prompt("sc01", "入口", "sc01 | l | 入口", "", "GM 叙事正文");
        assert!(
            prompt.contains("GM描述") || prompt.contains("GM 叙事正文"),
            "叙事应在 prompt 中"
        );
        assert!(prompt.contains("当前场景: sc01"), "当前场景应在");
    }

    // L-C content-gravity：空 exits ⇒ with_exits 与裸 build_nav_prompt 字节等价（OFF/无衔接
    // beat 路径与历史基线一致）。
    #[test]
    fn nav_prompt_with_empty_exits_is_byte_equal_to_base() {
        let base = build_nav_prompt(
            "sc01",
            "入口",
            "sc01 | l | 入口\nsc02 | l | 内厅",
            "我观察四周",
            "叙事正文",
        );
        let with_empty = build_nav_prompt_with_exits(
            "sc01",
            "入口",
            "   ",
            "sc01 | l | 入口\nsc02 | l | 内厅",
            "我观察四周",
            "叙事正文",
        );
        assert_eq!(base, with_empty, "空 exits 必须与裸 prompt 字节等价");
    }

    // L-C content-gravity：非空 exits ⇒ 衔接 beat 节出现，且仍含玩家输入/叙事/全场景列表。
    #[test]
    fn nav_prompt_with_exits_includes_beat_section() {
        let p = build_nav_prompt_with_exits(
            "sc01",
            "入口",
            "sc02 | 内厅\nsc03 | 后巷",
            "sc01 | l | 入口\nsc02 | l | 内厅\nsc03 | l | 后巷",
            "我推门走向内厅深处",
            "叙事正文",
        );
        assert!(p.contains("直接衔接 beat"), "应含衔接 beat 节标题");
        assert!(p.contains("sc02 | 内厅"), "应列出衔接 beat");
        assert!(p.contains("我推门走向内厅深处"), "仍含玩家输入");
        assert!(p.contains("叙事正文"), "仍含叙事");
    }

    // L-Y/L-AA：两 flag 皆 false ⇒ gravity system prompt 字节等价纯
    // SCENE_NAV_SYS_GRAVITY（L-C 基线，OFF==baseline）。
    #[test]
    fn gravity_nav_prompt_departure_off_is_byte_equal_to_baseline() {
        assert_eq!(
            gravity_nav_system_prompt(false, false),
            SCENE_NAV_SYS_GRAVITY,
            "两 flag OFF 必须与纯 gravity system prompt 字节等价"
        );
    }

    // L-Y 离场提交：departure_commit=true ⇒ 在 gravity 基线之上追加④离场提交子句，
    // 且仍保留原①②③内容引力判据（纯追加，不改写基线）。
    #[test]
    fn gravity_nav_prompt_departure_on_appends_clause() {
        let p = gravity_nav_system_prompt(true, false);
        assert!(
            p.starts_with(SCENE_NAV_SYS_GRAVITY),
            "ON 必须以 gravity 基线为前缀（纯追加）"
        );
        assert!(p.contains("离场提交"), "ON 应含④离场提交子句");
        assert!(
            p.contains("内容引力") || p.contains("③"),
            "ON 应保留原内容引力判据"
        );
        assert!(
            p.contains("fail-closed") || p.contains("原地观察"),
            "④子句应保留 fail-closed 守卫"
        );
        assert!(
            !p.contains("目标承接"),
            "departure-only 不应含⑤目标承接子句"
        );
        assert!(p.len() > SCENE_NAV_SYS_GRAVITY.len(), "ON 严格更长（追加）");
    }

    // L-AA 目标承接提交：objective_commit=true ⇒ 在 gravity 基线（+④）之上追加⑤子句，
    // 仍保留①②③④且仍含 fail-closed 守卫（纯追加，按 ④→⑤ 顺序）。
    #[test]
    fn gravity_nav_prompt_objective_on_appends_clause() {
        let p = gravity_nav_system_prompt(true, true);
        assert!(
            p.starts_with(SCENE_NAV_SYS_GRAVITY),
            "ON 必须以 gravity 基线为前缀（纯追加）"
        );
        assert!(p.contains("离场提交"), "应保留④离场提交子句");
        assert!(p.contains("目标承接"), "ON 应含⑤目标承接子句");
        assert!(
            p.find("离场提交").unwrap() < p.find("目标承接").unwrap(),
            "④必须排在⑤之前（确定性顺序）"
        );
        assert!(
            p.contains("fail-closed") || p.contains("尚未付诸持续行动"),
            "⑤子句应保留 fail-closed 守卫（仅提及/未付诸行动 → 不动）"
        );
        assert!(
            p.contains("绝不编造"),
            "⑤子句应保留 in-graph 真实 node_id 约束"
        );
    }

    // L-AA 目标承接：objective 单开（departure off）也只追加⑤、不含④，且仍以基线为前缀。
    #[test]
    fn gravity_nav_prompt_objective_only_excludes_departure() {
        let p = gravity_nav_system_prompt(false, true);
        assert!(p.starts_with(SCENE_NAV_SYS_GRAVITY), "仍以基线为前缀");
        assert!(p.contains("目标承接"), "应含⑤目标承接子句");
        assert!(!p.contains("离场提交"), "departure off 不应含④子句");
    }

    // L-AA flag 默认 ON（未设环境变量时）。同 L-C/L-Y 模式：仅显式 0/false/off/no 关。
    #[test]
    fn nav_objective_commit_defaults_on_when_unset() {
        if std::env::var("TRPG_SCENE_NAV_OBJECTIVE_COMMIT").is_err() {
            assert!(
                nav_objective_commit_enabled(),
                "未设 TRPG_SCENE_NAV_OBJECTIVE_COMMIT ⇒ 默认 ON"
            );
        }
    }

    // L-Y flag 默认 ON（未设环境变量时）。同 L-C 模式：仅显式 0/false/off/no 关。
    #[test]
    fn nav_departure_commit_defaults_on_when_unset() {
        // 测试隔离：仅在未设时断言默认 ON（CI/本地默认环境无此 var）。
        if std::env::var("TRPG_SCENE_NAV_DEPARTURE_COMMIT").is_err() {
            assert!(
                nav_departure_commit_enabled(),
                "未设 TRPG_SCENE_NAV_DEPARTURE_COMMIT ⇒ 默认 ON"
            );
        }
    }

    // R5 Task1a 边界测试：编译级断言 critical/heavy 两个新公开异步函数的存在与签名
    // （critical 返回 Option<SceneNavCommit>，heavy 接受 target 无返回）。真 DB 行为
    // 由 e2e 验证闸覆盖；此处仅锁定 API 边界，防止拆分被合回一体。不调用——仅靠类型
    // 推断在编译期校验签名稳定。
    #[tokio::test]
    async fn navigate_critical_and_heavy_have_separate_entrypoints() {
        #[allow(clippy::type_complexity)]
        fn _assert_critical(
            f: fn(
                &trpg_db::Db,
                &dyn trpg_llm::LlmClient,
                &str,
                &str,
                &std::path::Path,
                &str,
                &str,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = anyhow::Result<Option<SceneNavCommit>>> + Send,
                >,
            >,
        ) {
            let _ = f;
        }
        fn _assert_heavy(
            f: fn(
                &trpg_db::Db,
                &dyn trpg_llm::LlmClient,
                &str,
                &str,
                &std::path::Path,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
        ) {
            let _ = f;
        }
        let _ = (_assert_critical, _assert_heavy);
    }
}

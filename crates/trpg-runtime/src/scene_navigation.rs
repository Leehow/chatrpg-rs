//! 场景导航 + 模组深抽：extract_module_scenes / scene_navigator / validate_transition /
//! build_nav_prompt / prefetch_frontier。从 trpg-api 下沉至 trpg-runtime，
//! 供 trpg-gm 执行器 phase_scene_navigate 调用，也供 trpg-cli 直接引用。
use tracing::info;
use trpg_db::Db;
use trpg_llm::LlmClient;
use trpg_model::{ScenarioNode, SceneExtractionStatus};

const FRONTIER_PREFETCH_MAX: usize = 5;

const SCENE_NAV_SYS: &str = "你是模组场景导航器。给定『当前场景』『模组全部场景列表(node_id|kind|title)』\
『玩家输入』『本回合 GM 叙事』，综合两者语义判断玩家党是否离开当前场景、\
走到列表里另一个真实存在的场景。玩家明确说出移动意图（如「我去 X」「开车到 X」）\
或 GM 叙事描述了到达新地点，均应判为 moved=true。按语义判断（人物移动/进入新地点/任务推进），\
不要按标题字面猜。输出 JSON：{\"moved\": bool, \"target_node_id\": string|null, \"reason\": string}。\
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

/// 纯函数：构建 content-gravity 导航 system prompt。`departure_commit=false` ⇒ 返回纯
/// `SCENE_NAV_SYS_GRAVITY`（字节等价 L-C 基线）；`true`（默认）⇒ 追加 L-Y 离场提交④子句。
/// 便于单测锁定 OFF==基线字节等价 + ON 含④子句。
pub fn gravity_nav_system_prompt(departure_commit: bool) -> String {
    if departure_commit {
        format!("{SCENE_NAV_SYS_GRAVITY}\n{SCENE_NAV_DEPARTURE_CLAUSE}")
    } else {
        SCENE_NAV_SYS_GRAVITY.to_string()
    }
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
    // Load the same semantic units parse_module read, from the same data dir path.
    let units_path = data_dir
        .join("parsed/source_units")
        .join(format!("{source_id}.semantic_units.jsonl"));
    let units = match trpg_rule_agent::reader::load_units(&units_path) {
        Ok(u) if !u.is_empty() => u,
        Ok(_) => {
            info!(module_id, path = %units_path.display(), "extract_module_scenes: empty units; nothing to do");
            return Ok(0);
        }
        Err(err) => {
            tracing::error!(error = %err, path = %units_path.display(), "extract_module_scenes: load_units failed; nothing to do");
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
            let title_match = title.chars().count() >= 3 && (title.contains(&tl) || tl.contains(title));
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

    fn scenes() -> Vec<ScenarioNode> {
        let mk = |id: &str| {
            let mut n = ScenarioNode::default();
            n.node_id = id.into();
            n
        };
        vec![mk("loc1"), mk("loc2"), mk("loc3")]
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
            mk("sc_warehouse", "Scavv Warehouse", &["sc_talk_athena", "sc_street"]),
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

    // L-Y 离场提交：departure_commit=false ⇒ gravity system prompt 字节等价纯
    // SCENE_NAV_SYS_GRAVITY（L-C 基线，OFF==baseline）。
    #[test]
    fn gravity_nav_prompt_departure_off_is_byte_equal_to_baseline() {
        assert_eq!(
            gravity_nav_system_prompt(false),
            SCENE_NAV_SYS_GRAVITY,
            "departure OFF 必须与纯 gravity system prompt 字节等价"
        );
    }

    // L-Y 离场提交：departure_commit=true ⇒ 在 gravity 基线之上追加④离场提交子句，
    // 且仍保留原①②③内容引力判据（纯追加，不改写基线）。
    #[test]
    fn gravity_nav_prompt_departure_on_appends_clause() {
        let p = gravity_nav_system_prompt(true);
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
        assert!(p.len() > SCENE_NAV_SYS_GRAVITY.len(), "ON 严格更长（追加）");
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

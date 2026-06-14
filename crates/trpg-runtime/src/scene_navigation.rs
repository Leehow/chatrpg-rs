//! 场景导航 + 模组深抽：extract_module_scenes / scene_navigator / validate_transition /
//! build_nav_prompt / prefetch_frontier。从 trpg-api 下沉至 trpg-runtime，
//! 供 trpg-gm 执行器 phase_scene_navigate 调用，也供 trpg-cli 直接引用。
use serde_json::json;
use tracing::info;
use trpg_db::Db;
use trpg_llm::LlmClient;
use trpg_model::{ScenarioNode, SceneExtractionStatus, Visibility, WorldEventKind};
use trpg_time::WorldTimeService;

const FRONTIER_PREFETCH_MAX: usize = 5;

const SCENE_NAV_SYS: &str = "你是模组场景导航器。给定『当前场景』『模组全部场景列表(node_id|kind|title)』\
『玩家输入』『本回合 GM 叙事』，综合两者语义判断玩家党是否离开当前场景、\
走到列表里另一个真实存在的场景。玩家明确说出移动意图（如「我去 X」「开车到 X」）\
或 GM 叙事描述了到达新地点，均应判为 moved=true。按语义判断（人物移动/进入新地点/任务推进），\
不要按标题字面猜。输出 JSON：{\"moved\": bool, \"target_node_id\": string|null, \"reason\": string}。\
fail-closed：不确定、没有明确移动、或目标不在列表里 → moved=false。target_node_id 必须是给定列表中的 node_id，绝不编造。";

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
    let Some((mut bundle, source_hash, parse_config_hash)) = db.load_module_bundle_for_continue(module_id).await? else {
        info!(module_id, "extract_module_scenes: no module bundle found; nothing to do");
        return Ok(0);
    };
    // source_id: explicit (continue request) or derived from the bundle's
    // source_index (same id parse_module wrote the units file under).
    let derived_source_id = bundle.source_index.sources.first().map(|s| s.source_id.clone());
    let Some(source_id) = source_id.map(str::to_string).or(derived_source_id) else {
        info!(module_id, "extract_module_scenes: no source_id (request or bundle); nothing to do");
        return Ok(0);
    };
    // Load the same semantic units parse_module read, from the same data dir path.
    let units_path = data_dir.join("parsed/source_units").join(format!("{source_id}.semantic_units.jsonl"));
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
    let sidecar_text = std::fs::read_to_string(data_dir.join(format!("markdown/modules/{source_id}.md"))).ok();
    let resolved_ruleset = ruleset_id.map(str::to_string).or_else(|| bundle.ruleset_id.clone());
    let ctx = trpg_rule_agent::reader::ModuleReaderCtx { units: &units, sidecar_text, ruleset_id: resolved_ruleset };

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
    };

    // Pick the target scene indices: SkeletonOnly scenes, optionally narrowed to
    // the single `only` node. (only=Some + already DeepExtracted -> empty -> 0.)
    let skeleton_idxs: Vec<usize> = readout.scenes.iter().enumerate()
        .filter(|(_, s)| s.extraction_status == SceneExtractionStatus::SkeletonOnly)
        .filter(|(_, s)| only.map_or(true, |id| s.node_id == id))
        .map(|(i, _)| i)
        .collect();
    if skeleton_idxs.is_empty() {
        info!(module_id, ?only, "extract_module_scenes: no matching SkeletonOnly scenes");
        return Ok(0);
    }
    let mut deep_count = 0usize;
    for idx in skeleton_idxs {
        if trpg_rule_agent::reader::deep_extract_scene_in_place(llm, &ctx, &mut readout, idx, budget).await {
            deep_count += 1;
        } else {
            info!(module_id, idx, "scene stayed SkeletonOnly during extract");
        }
    }
    if deep_count == 0 {
        info!(module_id, "extract_module_scenes: no scene upgraded; skipping re-persist");
        return Ok(0);
    }

    // 增量补实体桥接边：在 source_anchor 过滤(deep_extract_scene_in_place 内部)之后、写回
    // 之前调用，连通所有已深抽且共享实体的场景（零 LLM、幂等、source_anchor=None）。
    let bridges = trpg_rule_agent::reader::apply_bridge_edges(&mut readout.scenes);
    info!(module_id, bridge_edges = bridges, "extract_module_scenes: applied entity-bridge edges");

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
    db.upsert_module_bundle(&bundle, None, &source_hash, &parse_config_hash).await?;
    info!(module_id, deep_extracted = deep_count, "extract_module_scenes: re-persisted upgraded ModuleGraph");
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

/// turn_postprocess 内的语义场景导航：用一次 LLM 判定党是否移动到模组某真实场景，
/// 校验通过则更新 `sessions.current_scene_id` 并对目标场景到场深抽（若仍 SkeletonOnly）。
/// 切换成功时写一条 `WorldEventKind::SceneChanged` world event（kind/from/to/reason）。
///
/// 全程 fail-closed：取不到图/空图/LLM 失败/校验不过 → warn + Ok(())（留原场景，不乱跳、
/// 不编造 target）。source_id/ruleset_id 由 `extract_module_scenes` 从 module bundle 自行
/// 推导，故本函数无需调用方提供（运行时只持有 module_id）。
/// pub：供 trpg-gm execute_turn 的 scene_navigate phase（CLI/API 统一回合路径）调用，
/// 与 API turn_postprocess 共享同一语义导航。
pub async fn scene_navigator(
    db: &Db,
    llm: &dyn LlmClient,
    session_id: &str,
    module_id: &str,
    data_dir: &std::path::Path,
    player_input: &str,
    narration: &str,
) -> anyhow::Result<()> {
    let Some(graph) = db.load_module_graph(module_id).await? else { return Ok(()); };
    if graph.scenes.is_empty() {
        return Ok(());
    }
    let current = db.load_session_scene(session_id).await?.unwrap_or_default();
    let list = graph.scenes.iter()
        .map(|s| format!("{} | {} | {}", s.node_id, s.node_type, s.title))
        .collect::<Vec<_>>().join("\n");
    let cur_title = graph.scenes.iter()
        .find(|s| s.node_id == current)
        .map(|s| s.title.as_str())
        .unwrap_or("(未定)");
    let usr = build_nav_prompt(&current, cur_title, &list, player_input, narration);
    let decision = match llm.complete_json(vec![trpg_llm::system(SCENE_NAV_SYS), trpg_llm::user(&usr)], 0.0).await {
        Ok(v) => v,
        Err(err) => { tracing::warn!(error = %err, "scene_navigator llm failed; stay"); return Ok(()); }
    };
    let Some(target) = validate_transition(&decision, &graph.scenes, &current) else { return Ok(()); };
    db.set_session_scene(session_id, &target).await?;
    let reason = decision.get("reason").and_then(|v| v.as_str()).unwrap_or("").to_string();
    info!(session_id, from = %current, to = %target, %reason, "scene transition");
    // 写 scene_transition world event（kind/from/to/reason），best-effort 失败只 warn。
    let event_data = json!({"kind": "scene_transition", "from": current, "to": target, "reason": reason, "module_id": module_id});
    if let Err(err) = WorldTimeService::new(db.clone())
        .record_event(session_id, None, None, WorldEventKind::SceneChanged, event_data, Visibility::GmOnly)
        .await
    {
        tracing::warn!(error = %err, "scene_navigator: world event write failed; scene already switched");
    }
    // 到场深抽（目标若 SkeletonOnly）；已预抽则内部判定 0、无害。source_id=None →
    // extract_module_scenes 从 bundle.source_index 推导。失败不回滚切换（场景已更）。
    if let Err(err) = extract_module_scenes(db, llm, module_id, None, None, data_dir, 12, Some(&target)).await {
        tracing::warn!(error = %err, %target, "on-arrival deep-extract failed; scene already switched");
    }
    // Frontier 前探一跳（best-effort）：target 深抽后其出口 links 已写回 bundle，重新
    // load_module_graph 取 target 的出口 to_node_id，对每个仍 SkeletonOnly 的出口（去重、
    // bounded 上限）逐个 only=Some 深抽。玩家移动到衔接场景时即时；没去的场景永停 stub。
    prefetch_frontier(db, llm, module_id, &target, data_dir).await;
    Ok(())
}

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
    let Some(node) = graph.scenes.iter().find(|s| s.node_id == target) else { return };
    // 仍 SkeletonOnly 的出口集合，去重 + bounded 上限。
    let mut seen = std::collections::HashSet::new();
    let mut exits: Vec<String> = Vec::new();
    for link in &node.links {
        let to = link.to_node_id.trim();
        if to.is_empty() || to == target || !seen.insert(to.to_string()) {
            continue;
        }
        let still_stub = graph.scenes.iter()
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
        if let Err(err) = extract_module_scenes(db, llm, module_id, None, None, data_dir, 12, Some(&exit)).await {
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
        assert!(prompt.contains("GM 叙事:") || prompt.contains("GM叙事"), "应包含叙事标签");
        assert!(prompt.contains("GM描述了街道"), "应包含叙事内容");
        assert!(prompt.contains("sc01"), "应包含当前场景");
        assert!(prompt.contains("sc02"), "应包含场景列表");
    }

    #[test]
    fn build_nav_prompt_truncates_long_inputs() {
        let long_player = "x".repeat(600);
        let long_narration = "y".repeat(3000);
        let prompt = build_nav_prompt("sc01", "场景", "sc01 | l | 场景", &long_player, &long_narration);
        // player_input 截至 500 字符，narration 截至 2000 字符
        let player_section = prompt.split("玩家输入:").nth(1).unwrap_or("").split("GM 叙事:").next().unwrap_or("").trim().to_string();
        let x_count = player_section.chars().filter(|&c| c == 'x').count();
        assert!(x_count <= 500, "player_input 应被截断至 ≤500 chars，实际 {x_count}");
        let y_count = prompt.chars().filter(|&c| c == 'y').count();
        assert!(y_count <= 2000, "narration 应被截断至 ≤2000 chars，实际 {y_count}");
    }

    #[test]
    fn build_nav_prompt_empty_player_input_still_valid() {
        // fail-closed：空 player_input 不崩溃，叙事仍在 prompt 中
        let prompt = build_nav_prompt("sc01", "入口", "sc01 | l | 入口", "", "GM 叙事正文");
        assert!(prompt.contains("GM描述") || prompt.contains("GM 叙事正文"), "叙事应在 prompt 中");
        assert!(prompt.contains("当前场景: sc01"), "当前场景应在");
    }
}

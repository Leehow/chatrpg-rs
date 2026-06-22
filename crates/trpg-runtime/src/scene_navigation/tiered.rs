//! R5 Task1a：场景导航的 critical/heavy 拆分。
//! `scene_navigate_critical`（决策 + set_session_scene + SceneChanged，同步）/
//! `scene_navigate_heavy`（到场深抽 + frontier，后台）/ `scene_navigator`（薄 wrapper，
//! critical→heavy 串行，字节等价旧逻辑）。复用父模块的 build_nav_prompt / validate_transition /
//! extract_module_scenes / prefetch_frontier / SCENE_NAV_SYS。
use super::{
    build_nav_exits, build_nav_prompt, build_nav_prompt_with_exits, extract_module_scenes,
    gravity_nav_system_prompt, nav_content_gravity_enabled, nav_departure_commit_enabled,
    nav_follow_flow_links_enabled, nav_objective_commit_enabled, prefetch_frontier,
    resolve_offgraph_to_neighbor, validate_transition, with_flow_link_clause, SCENE_NAV_SYS,
};
use serde_json::json;
use tracing::info;
use trpg_db::Db;
use trpg_llm::LlmClient;
use trpg_model::{Visibility, WorldEventKind};
use trpg_time::WorldTimeService;

/// R5 critical 切场景的产出：成功切换才 Some，携带新 target 供 heavy 深抽。
/// from/reason 供调用方折成 SceneTransition 事件（execute.rs）。
#[derive(Debug, Clone)]
pub struct SceneNavCommit {
    pub from: String,
    pub to: String,
    pub reason: String,
}

/// R5 critical 半：语义判定 + set_session_scene + SceneChanged world event。**不**深抽/
/// 前探（那是 heavy，下一回合 prepare_turn_context 不强依赖——SkeletonOnly 降级块兜底）。
/// 全程 fail-closed：无图/空图/LLM 失败/校验不过 → Ok(None)（留原场景，不乱跳、不编造）。
/// 返回 Some(commit) 仅当真切换；调用方据此发 SceneTransition 事件 + 排 heavy 深抽。
pub async fn scene_navigate_critical(
    db: &Db,
    llm: &dyn LlmClient,
    session_id: &str,
    module_id: &str,
    _data_dir: &std::path::Path,
    player_input: &str,
    narration: &str,
) -> anyhow::Result<Option<SceneNavCommit>> {
    let Some(graph) = db.load_module_graph(module_id).await? else {
        return Ok(None);
    };
    if graph.scenes.is_empty() {
        return Ok(None);
    }
    let current = db.load_session_scene(session_id).await?.unwrap_or_default();
    let list = graph
        .scenes
        .iter()
        .map(|s| format!("{} | {} | {}", s.node_id, s.node_type, s.title))
        .collect::<Vec<_>>()
        .join("\n");
    let cur_node = graph.scenes.iter().find(|s| s.node_id == current);
    let cur_title = cur_node.map(|s| s.title.as_str()).unwrap_or("(未定)");
    // L-C content-gravity（理念§4/§7，flag 默认 ON / OFF 字节等价）：把当前场景的真实直接
    // 衔接 beat（去重、in-graph、≠current）作为软推进上下文喂给导航器，并换用追加了「软推进」
    // 许可的 system prompt。OFF ⇒ 走原 SCENE_NAV_SYS + 裸 build_nav_prompt（提示串字节等价基线）。
    let gravity = nav_content_gravity_enabled();
    let (sys, usr) = if gravity {
        // J3 FLOW-LINK CONSUMER（TRPG_NAV_FOLLOW_FLOW_LINKS，默认 OFF）：exits 现经
        // build_nav_exits 构建——flag ON 时已授权有向脊边（sequential/trigger/branch+anchor）
        // 排在 spatial 桥之前并标注其 link_type；flag OFF ⇒ 与历史内联逐字节一致（OFF==baseline）。
        let follow_flow = nav_follow_flow_links_enabled();
        let exits = build_nav_exits(cur_node, &graph.scenes, &current, follow_flow);
        // L-Y/L-AA：gravity 串在 ①②③ 之上按各自 flag 追加④离场提交+⑤目标承接（皆默认 ON）；
        // ⑥流转脊优先（with_flow_link_clause）再按本 flag 叠加（仅玩家驱动才 commit 脊边、反铁路
        // fail-closed）。三 flag 皆退回基线 ⇒ 纯 SCENE_NAV_SYS_GRAVITY 字节等价 L-C 基线。
        (
            with_flow_link_clause(
                gravity_nav_system_prompt(
                    nav_departure_commit_enabled(),
                    nav_objective_commit_enabled(),
                ),
                follow_flow,
            ),
            build_nav_prompt_with_exits(&current, cur_title, &exits, &list, player_input, narration),
        )
    } else {
        (
            SCENE_NAV_SYS.to_string(),
            build_nav_prompt(&current, cur_title, &list, player_input, narration),
        )
    };
    let decision = match llm
        .complete_json(vec![trpg_llm::system(&sys), trpg_llm::user(&usr)], 0.0)
        .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(error = %err, "scene_navigate_critical llm failed; stay");
            return Ok(None);
        }
    };
    let target = match validate_transition(&decision, &graph.scenes, &current) {
        Some(t) => t,
        None => {
            // MAT.M9c（DP-C）：LLM 想移动但目标 off-graph → Enforce 下纠回当前场景真实邻接
            // （绝不去图谱外编造场景）；匹配不到 ⇒ 留原场景（既有 fail-closed 行为）。
            // Off/Shadow ⇒ resolve_offgraph_to_neighbor 恒 None ⇒ 字节级基线。
            let mode = trpg_model::MaterializationAffordanceMode::from_env();
            match resolve_offgraph_to_neighbor(&decision, &graph.scenes, &current, mode) {
                Some(t) => {
                    info!(session_id, from = %current, neighbor = %t, "MAT.M9c: off-graph nav target remapped to in-graph neighbor");
                    t
                }
                None => return Ok(None),
            }
        }
    };
    db.set_session_scene(session_id, &target).await?;
    let reason = decision
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    info!(session_id, from = %current, to = %target, %reason, "scene transition (critical)");
    // 写 scene_transition world event（kind/from/to/reason），best-effort 失败只 warn。
    let event_data = json!({"kind": "scene_transition", "from": current, "to": target, "reason": reason, "module_id": module_id});
    if let Err(err) = WorldTimeService::new(db.clone())
        .record_event(
            session_id,
            None,
            None,
            WorldEventKind::SceneChanged,
            event_data,
            Visibility::GmOnly,
        )
        .await
    {
        tracing::warn!(error = %err, "scene_navigate_critical: world event write failed; scene already switched");
    }
    Ok(Some(SceneNavCommit {
        from: current,
        to: target,
        reason,
    }))
}

/// R5 heavy 半：到场深抽 + frontier 前探（best-effort，后台跑，下一回合不强依赖）。
/// 全程 fail-closed：深抽/前探失败只 warn，绝不回滚已由 critical 切换的场景。
pub async fn scene_navigate_heavy(
    db: &Db,
    llm: &dyn LlmClient,
    module_id: &str,
    target: &str,
    data_dir: &std::path::Path,
) {
    // 到场深抽（目标若 SkeletonOnly）；已预抽则内部判定 0、无害。source_id=None →
    // extract_module_scenes 从 bundle.source_index 推导。失败不回滚切换（场景已更）。
    if let Err(err) =
        extract_module_scenes(db, llm, module_id, None, None, data_dir, 12, Some(target)).await
    {
        tracing::warn!(error = %err, %target, "on-arrival deep-extract failed; scene already switched");
    }
    // Frontier 前探一跳（best-effort）：target 深抽后其出口 links 已写回 bundle，重新
    // load_module_graph 取 target 的出口 to_node_id，对每个仍 SkeletonOnly 的出口（去重、
    // bounded 上限）逐个 only=Some 深抽。玩家移动到衔接场景时即时；没去的场景永停 stub。
    prefetch_frontier(db, llm, module_id, target, data_dir).await;
}

/// 旧入口（critical→heavy 串行一体）：用一次 LLM 判定党是否移动到模组某真实场景，
/// 校验通过则更新 `sessions.current_scene_id`、写 `WorldEventKind::SceneChanged` world
/// event，再对目标场景到场深抽（若仍 SkeletonOnly）+ frontier 前探。
///
/// R5 拆分后保留为薄 wrapper（critical 后串 heavy），行为字节等价旧逻辑，供尚未拆分的
/// 调用方与单 transport 临时复用；新执行器（execute.rs）走拆分路径（critical 同步、heavy
/// 后台）。全程 fail-closed：取不到图/空图/LLM 失败/校验不过 → Ok(())（留原场景）。
pub async fn scene_navigator(
    db: &Db,
    llm: &dyn LlmClient,
    session_id: &str,
    module_id: &str,
    data_dir: &std::path::Path,
    player_input: &str,
    narration: &str,
) -> anyhow::Result<()> {
    if let Some(commit) = scene_navigate_critical(
        db,
        llm,
        session_id,
        module_id,
        data_dir,
        player_input,
        narration,
    )
    .await?
    {
        scene_navigate_heavy(db, llm, module_id, &commit.to, data_dir).await;
    }
    Ok(())
}

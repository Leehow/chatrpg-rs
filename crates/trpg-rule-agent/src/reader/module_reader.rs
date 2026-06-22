//! Agentic 模组 reader —— 规则 agent 的聚焦"模组刀"，对标 chargen_compile.rs：
//! 自带 Ctx + SYS prompt + tools::submit_tool + 私有 run loop + 私有 dispatch +
//! fail-closed guardrail（提交解析失败/页面无内容 → 返回已得部分，绝不编造）。
//! Pass A 读 TOC+前言建全书骨架；Pass B（loop 文件的 deep_extract_scene_in_place）深抽场景。
//! 设计：docs/superpowers/specs/2026-06-09-module-reader-redesign-design.md
// 入口选择纯函数已外移 module_graph_edges.rs 守 ≤400 行；再导出保持调用点/测试可见性不变。
#[cfg(test)]
pub(super) use super::module_graph_edges::entry_scene_index;
pub(super) use super::module_graph_edges::resolve_entry_index;
use super::module_reader_loop::run_module_loop;
use super::tools;
use super::units::Unit;
use serde_json::{json, Value};
use std::time::Instant;
use trpg_llm::LlmClient;
use trpg_model::{
    DirectorModuleConfig, LinkType, ScenarioLink, ScenarioNode, SceneExtractionStatus,
};

// ---- Context + readout ----

/// 两遍模组抽取的共享上下文（镜像 `CompileCtx`/`ObjectCtx`）。
pub struct ModuleReaderCtx<'a> {
    pub units: &'a [Unit],
    /// duotext 合并的 page-anchored `.md` —— read_layout 的列对齐视图。
    pub sidecar_text: Option<String>,
    /// 预留：parse_module 接 background-continue job 时用（spec §6.5）；本 slice 暂不读。
    pub ruleset_id: Option<String>,
}

/// reader 两遍产出（装入 ModuleGraph 的各 vec）。
#[derive(Default)]
pub struct ModuleReadout {
    pub spine: Value,
    pub scenes: Vec<ScenarioNode>,
    pub npcs: Vec<Value>,
    pub clues: Vec<Value>,
    pub locations: Vec<Value>,
    pub factions: Vec<Value>,
    pub encounters: Vec<Value>,
    pub handouts: Vec<Value>,
    pub module_specific_rules: Vec<Value>,
    /// 模组级引导事实(facilitation.rs 从已解析开场场景+spine 抽取);装入
    /// `ModuleGraph.director_facilitation`,退役 director sidecar 的 REQUIRED 注入。None = 没抽到。
    pub facilitation_facts: Option<DirectorModuleConfig>,
}

// ---- Deterministic helpers (round-trip / fail-closed; unit-tested) ----

/// 给定首场景，收集"依赖闭包" id 集合（首场景引用的 npc/clue/location + 出边目标）。
/// 通用、确定性、可单测。保持首次出现顺序、去重。
pub fn dependency_closure(entry: &ScenarioNode) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for v in [
        &entry.referenced_npc_ids,
        &entry.referenced_clue_ids,
        &entry.referenced_location_ids,
    ] {
        for id in v {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
    }
    for l in &entry.links {
        if !ids.contains(&l.to_node_id) {
            ids.push(l.to_node_id.clone());
        }
    }
    ids
}

/// 把 reader 提交的一条 skeleton stub（JSON）转成 SkeletonOnly 的 ScenarioNode。
/// 缺字段一律取空/默认，绝不编造 read_aloud（保持 None）。
pub fn stub_to_node(v: &Value) -> Option<ScenarioNode> {
    let node_id = v
        .get("node_id")
        .and_then(|x| x.as_str())?
        .trim()
        .to_string();
    if node_id.is_empty() {
        return None;
    }
    let mut n = ScenarioNode::default();
    n.node_id = node_id;
    n.title = v
        .get("title")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    n.node_type = v
        .get("kind")
        .and_then(|x| x.as_str())
        .unwrap_or("scene")
        .to_string();
    n.summary = v
        .get("summary")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    n.page_start = v
        .get("page_start")
        .and_then(|x| x.as_u64())
        .map(|p| p as u32);
    n.page_end = v.get("page_end").and_then(|x| x.as_u64()).map(|p| p as u32);
    let ids = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|e| e.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    n.referenced_npc_ids = ids("referenced_npc_ids");
    n.referenced_clue_ids = ids("referenced_clue_ids");
    n.referenced_location_ids = ids("referenced_location_ids");
    n.extraction_status = SceneExtractionStatus::SkeletonOnly; // Pass B 才翻 Deep
    Some(n)
}

/// fail-closed：把 Pass B 提交的 deep payload 就地填进入口 ScenarioNode。
/// read_aloud 为空/缺 → 保持 None（绝不编造）；有内容才翻 DeepExtracted。
pub(super) fn apply_deep_to_node(node: &mut ScenarioNode, deep: &Value) {
    let scene = deep.get("scene").unwrap_or(deep);
    let nonempty = |k: &str| {
        scene
            .get(k)
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    node.read_aloud = nonempty("read_aloud");
    node.gm_notes = nonempty("gm_notes");
    // Pass B 深抽 links：**合并不覆盖**。骨架 Pass A 的 links（读 TOC 得出的导航边）
    // 是基线；深抽读了正文，能给出更精确的带 source_anchor 出口 → 同 target 时深抽优先、
    // 新 target 追加。绝不因深抽 source_anchor 严格过滤后交空数组就把骨架边冲掉
    // （sandbox 模组正文常无字面出口锚 → 深抽 links 为空，但骨架边仍有效）。
    if let Some(links) = scene.get("links").and_then(|x| x.as_array()) {
        let deep: Vec<ScenarioLink> = links
            .iter()
            .filter_map(|l| serde_json::from_value(l.clone()).ok())
            .collect();
        merge_links(&mut node.links, deep);
    }
    let ids = |k: &str| {
        scene.get(k).and_then(|x| x.as_array()).map(|a| {
            a.iter()
                .filter_map(|e| e.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
    };
    if let Some(v) = ids("referenced_npc_ids") {
        node.referenced_npc_ids = v;
    }
    if let Some(v) = ids("referenced_clue_ids") {
        node.referenced_clue_ids = v;
    }
    if let Some(v) = ids("referenced_location_ids") {
        node.referenced_location_ids = v;
    }
    if let Some(v) = ids("referenced_encounter_ids") {
        node.referenced_encounter_ids = v;
    }
    // 深抽交了非空 scene_mechanics 才覆盖；空/缺 → 保留既有（再抽不冲掉已得，
    // 对标 referenced_*_ids 的 Some 才覆盖样板）。
    let mechanics = super::scene_mechanics::parse_scene_mechanics(scene);
    if !mechanics.is_empty() {
        node.scene_mechanics = mechanics;
    }
    // 补充（非覆盖）：LLM 常把 NPC/线索放进 `deep.entities` + gm_notes 文字，却把
    // scene.referenced_* 留空 → 场景与实体断链、bridge_edges 连不出边。这里遍历
    // entities 按 kind 把 id 追加进对应列表（去重、跳空），让结构化链接补齐。
    if let Some(entities) = deep.get("entities").and_then(|x| x.as_array()) {
        let append = |list: &mut Vec<String>, id: &str| {
            let id = id.trim();
            if !id.is_empty() && !list.iter().any(|x| x == id) {
                list.push(id.to_string());
            }
        };
        for e in entities {
            let id = e.get("id").and_then(|x| x.as_str()).unwrap_or("");
            match e.get("kind").and_then(|x| x.as_str()).unwrap_or("") {
                "npc" => append(&mut node.referenced_npc_ids, id),
                "clue" => append(&mut node.referenced_clue_ids, id),
                "location" => append(&mut node.referenced_location_ids, id),
                "encounter" => append(&mut node.referenced_encounter_ids, id),
                _ => {}
            }
        }
    }
    // 仅当确有正文（念白或 GM 笔记）才翻 Deep；否则保持 SkeletonOnly。
    if node.read_aloud.is_some() || node.gm_notes.is_some() {
        node.extraction_status = SceneExtractionStatus::DeepExtracted;
    }
}

/// 合并深抽 links 进既有（骨架）links：同 to_node_id 去重；同 target 时深抽（带 source_anchor、
/// 读过正文，更精确）覆盖骨架那条，否则保留骨架；深抽的新 target 追加。空 to_node_id 丢弃。
pub(super) fn merge_links(base: &mut Vec<ScenarioLink>, incoming: Vec<ScenarioLink>) {
    for inc in incoming {
        if inc.to_node_id.trim().is_empty() {
            continue;
        }
        let has_anchor = inc
            .source_anchor
            .as_deref()
            .map(str::trim)
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        if let Some(slot) = base.iter_mut().find(|l| l.to_node_id == inc.to_node_id) {
            if has_anchor {
                *slot = inc; // 深抽带非空锚 → 升级为更精确的出口
            }
        } else {
            base.push(inc);
        }
    }
}

/// 入口兜底连通：入口深抽+桥接后仍无出边（纯序幕常无实体共享）→ 按骨架顺序连下一场景
/// （Sequential，source_anchor=None）保证图从入口可进入。已有出边/入口不存在/无下一场景 → 不动。
/// 返回是否补了边。确定、纯、fail-closed。
fn ensure_entry_connected(scenes: &mut [ScenarioNode], entry_idx: usize) -> bool {
    if scenes
        .get(entry_idx)
        .map(|s| !s.links.is_empty())
        .unwrap_or(true)
    {
        return false; // 已有出边，或入口越界
    }
    let Some(next_id) = scenes.get(entry_idx + 1).map(|s| s.node_id.clone()) else {
        return false; // 没有下一场景可连
    };
    scenes[entry_idx].links.push(ScenarioLink {
        to_node_id: next_id,
        reason: "入口顺序衔接（兜底）".into(),
        clue_id: None,
        link_type: LinkType::Sequential,
        source_anchor: None,
    });
    true
}

/// 把 Pass B 提交里的闭包实体（按 kind 路由）并入 readout 的对应 vec。去重 by id。
pub(super) fn merge_deep_entities(out: &mut ModuleReadout, deep: &Value) {
    let push = |vec: &mut Vec<Value>, e: &Value| {
        let id = e
            .get("id")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if id.is_empty() {
            return;
        }
        if let Some(slot) = vec
            .iter_mut()
            .find(|x| x.get("id").and_then(|v| v.as_str()) == Some(id.as_str()))
        {
            *slot = e.clone(); // 深抽详情覆盖骨架索引
        } else {
            vec.push(e.clone());
        }
    };
    let Some(entities) = deep.get("entities").and_then(|x| x.as_array()) else {
        return;
    };
    for e in entities {
        match e.get("kind").and_then(|x| x.as_str()).unwrap_or("") {
            "npc" => push(&mut out.npcs, e),
            "clue" => push(&mut out.clues, e),
            "location" => push(&mut out.locations, e),
            "faction" => push(&mut out.factions, e),
            "encounter" => push(&mut out.encounters, e),
            "handout" => push(&mut out.handouts, e),
            _ => {}
        }
    }
}

// Pass A SYS prompt（零硬编码：无规则/语言/模组专名）；Pass B 的 DEEP_SYS 在 loop 文件。
const SKELETON_SYS: &str = "你是模组结构抽取器。读目录(TOC)与前言，产出全书有序的可玩单元骨架\
（场景/地点/任务/时间线节点）与实体索引（npc/clue/location/encounter/handout，各带 id+name+\
content_class+page）与自定义规则清单。content_class∈{story,bp2_custom_rule,bp3_index}：模组专属\
规则子系统=bp2_custom_rule；物品/怪物/名册=bp3_index；剧情=story。每个场景骨架带 node_id+title+\
kind+summary+page_start/page_end+referenced_npc_ids/clue_ids/location_ids。\
【入口·必给】另给 entry_node_id：从骨架里语义选出『一桌真正会先开始玩』的那个单元的 node_id——\
通常是序幕/开场/第一场景/第一任务/第一地点；要跳过前言、安全提示/预警、致谢、目录、如何使用本书\
这类非可玩的前置内容（按语义判断该单元是否可玩，不要按标题字面）。\
【页码·必抽】每个场景的 page_start/page_end 必须从你读到的正文页锚『# Page N』推导（定位该单元\
内容出现的页范围）；即使目录没列页码也要靠页锚定位，不要留空。\
只输出文本里真实存在的内容，其余缺失留空，绝不编造。用 get_toc/search/read/read_layout 检索，最后 submit_skeleton。";

/// env 真值判定：变量存在且非空、非 "0"/"false"（忽略大小写/首尾空白）→ true；其余 → false。
/// 用于门控可选前台行为（默认关），零硬编码。
fn env_truthy(key: &str) -> bool {
    match std::env::var(key) {
        Ok(v) => {
            let v = v.trim();
            !v.is_empty() && !v.eq_ignore_ascii_case("0") && !v.eq_ignore_ascii_case("false")
        }
        Err(_) => false,
    }
}

// ---- Agentic two-pass entry ----

/// 两遍 agentic 模组抽取。镜像 chargen_compile 的 compile_chargen_formulas 编排：
/// 各 pass 建 submit_tool + nav_tools(+read_layout) → 私有 run loop(模拟 run_compile_loop)
/// → dispatch(模拟 compile_dispatch，仅 get_toc/search/read/read_layout) → fail-closed 解析。
/// 任一遍失败 → 返回已得部分（至少骨架），调用方再决定回退。
pub async fn run_module_reader(
    client: &dyn LlmClient,
    ctx: ModuleReaderCtx<'_>,
    budget: usize,
) -> anyhow::Result<ModuleReadout> {
    let mut out = ModuleReadout::default();
    let t0 = Instant::now(); // 计时：最小可用单元 = Pass A 骨架 + Pass B 首场景深抽

    // 模组抽取（Pass A 骨架 / gleaning / Pass B 深抽）统一用传入的单一 client
    //（parse_module 默认配 gpt-5.4，质量优先、后台跑对玩家无感），不再模型分层。

    // ---- Pass A：骨架 ----
    let submit_skeleton = tools::submit_tool(
        "submit_skeleton",
        "Submit the full-book ordered scene skeleton + entity index + custom-rule list.",
        json!({
            "entry_node_id": {"type": "string", "description": "node_id of the first PLAYABLE unit (skip front-matter prefaces/safety/TOC; semantic judgement)"},
            "spine": {"type": "object"},
            "scenes": {"type": "array", "items": {"type": "object"}},
            "npcs": {"type": "array", "items": {"type": "object"}},
            "clues": {"type": "array", "items": {"type": "object"}},
            "locations": {"type": "array", "items": {"type": "object"}},
            "factions": {"type": "array", "items": {"type": "object"}},
            "encounters": {"type": "array", "items": {"type": "object"}},
            "handouts": {"type": "array", "items": {"type": "object"}},
            "module_specific_rules": {"type": "array", "items": {"type": "object"}}
        }),
        &["scenes", "entry_node_id"],
    );
    let skeleton_seed = "读这本模组的目录与前言，产出全书有序场景骨架 + 实体索引 + 自定义规则清单，最后 submit_skeleton。".to_string();
    let skeleton = match run_module_loop(
        client,
        SKELETON_SYS,
        &skeleton_seed,
        &ctx,
        budget,
        submit_skeleton,
        "submit_skeleton",
    )
    .await
    {
        Some(v) => v,
        // fail-closed：Pass A 没拿到任何提交 → 返回空 readout，调用方回退。
        None => {
            tracing::warn!(target: "module_reader", elapsed_ms = t0.elapsed().as_millis() as u64, "Pass A skeleton produced nothing; empty readout");
            return Ok(out);
        }
    };

    out.spine = skeleton.get("spine").cloned().unwrap_or(Value::Null);
    let collect = |k: &str| {
        skeleton
            .get(k)
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default()
    };
    // RW3：前台不再 gleaning（移后台 stub-only job）。Pass A = 单遍骨架直接装配。
    out.scenes = collect("scenes").iter().filter_map(stub_to_node).collect();
    // RW3.2：用 units 页码兜底没拿到 page_start 的场景 → 让入口一次性切页深抽生效
    //（fail-closed：补不到→保持 None，深抽自动走 ReAct 回退）。
    let resolved_pages = super::module_graph_edges::resolve_scene_pages(&mut out.scenes, ctx.units);
    if resolved_pages > 0 {
        tracing::info!(target: "module_reader", phase = "resolve_pages", resolved = resolved_pages, total = out.scenes.len(), "filled scene pages from units");
    }
    // RW3 可选前台 gleaning（env 门控，默认关）：env `TRPG_MODULE_GLEANING` 真值时对照 TOC 补漏，
    // 用同一 client；补全后再 resolve_scene_pages 一次给新场景补页码。默认不跑（移后台 job）。
    if env_truthy("TRPG_MODULE_GLEANING") {
        let added = super::module_reader_loop::complete_skeleton_stubs(
            client,
            &ctx,
            &mut out.scenes,
            budget,
        )
        .await;
        if added > 0 {
            let repaged =
                super::module_graph_edges::resolve_scene_pages(&mut out.scenes, ctx.units);
            tracing::info!(target: "module_reader", phase = "gleaning", added = added, repaged = repaged, total = out.scenes.len(), "front-stage gleaning filled missing scenes");
        }
    }
    out.npcs = collect("npcs");
    out.clues = collect("clues");
    out.locations = collect("locations");
    out.factions = collect("factions");
    out.encounters = collect("encounters");
    out.handouts = collect("handouts");
    out.module_specific_rules = collect("module_specific_rules");

    let skeleton_ms = t0.elapsed().as_millis() as u64;
    tracing::info!(target: "module_reader", phase = "skeleton", elapsed_ms = skeleton_ms, scenes = out.scenes.len(), npcs = out.npcs.len(), custom_rules = out.module_specific_rules.len(), "Pass A skeleton done");

    // RW7：图谱装配只 dedup 实体 + validate 诊断（log 连通度）；全局边/补漏门已删
    //（出口在 deep_extract per-scene 局部算，gleaning 移后台）。entry_id/空图跳过在 wrapper 内。
    super::module_graph_build::build_graph_with_quality_gate(
        client, &ctx, &mut out, &skeleton, budget,
    )
    .await;
    // ---- Pass B：深抽入口场景 + 依赖闭包 ----（语义优先用 reader 的 entry_node_id；缺/失效则确定性兜底）
    let entry_node_id = skeleton.get("entry_node_id").and_then(|x| x.as_str());
    let Some(entry_idx) = resolve_entry_index(&out.scenes, entry_node_id) else {
        // 没有任何场景 → 只返回骨架（实体/规则索引），fail-closed。
        tracing::info!(target: "module_reader", phase = "skeleton_only", total_ms = t0.elapsed().as_millis() as u64, "no entry scene; skeleton-only readout");
        return Ok(out);
    };
    // 复用单场景深抽一刀（与 background-continue job 续抽剩余场景同源）。
    super::module_reader_loop::deep_extract_scene_in_place(
        client, &ctx, &mut out, entry_idx, budget,
    )
    .await;
    // 入口深抽后再补一遍实体桥接边：入口此刻有了 deep referenced_*_ids，可桥到共享实体的场景
    //（零 LLM、幂等、按 to_node_id 去重，不与既有边重复）。
    let bridges = super::module_graph_edges::apply_bridge_edges(&mut out.scenes);
    // 入口兜底连通：纯序幕常无实体共享 → 桥接后仍 0 出边 → 整图从入口不可达。按骨架顺序连下一
    // 场景（Sequential）保证可进入。fail-closed：已有出边/无下一场景 → 不动。
    let entry_patched = ensure_entry_connected(&mut out.scenes, entry_idx);
    // TRPG_MODULE_FLOW_LINKS（默认 OFF）：生产侧"已授权流转脊"——逐场景从正文抽
    // sequential/trigger/branch 有向边（每条带摘自原文的 source_anchor），与上面的实体共享
    // spatial 桥**并存**（桥保留为 fallback）。OFF ⇒ 此 pass 不被调用 ⇒ bundle 字节级不变。
    if super::module_flow_links::flow_links_enabled() {
        let typed =
            super::module_flow_links::extract_flow_links_all(client, &ctx, &mut out).await;
        tracing::info!(
            target: "module_reader", phase = "flow_links",
            typed_authored_links = typed, scenes = out.scenes.len(),
            "typed inter-scene flow links extracted (producer-side spine)"
        );
    }
    // 连通诊断**此刻**才测（入口已深抽+桥接+兜底），反映真实可达性（Pass B 前测会误报 reachable=1）。
    let entry_id = out.scenes[entry_idx].node_id.clone();
    let h = super::module_graph_validator::validate_graph(&out.scenes, &entry_id);
    let edges: usize = out.scenes.iter().map(|s| s.links.len()).sum();
    tracing::info!(
        target: "module_reader", phase = "graph_diag",
        score = h.score, reachable = h.reachable, orphans = h.orphans.len(),
        scenes = out.scenes.len(), edges, entry_bridges = bridges, entry_patched,
        "graph connectivity (post entry deep-extract + bridges)"
    );
    let total_ms = t0.elapsed().as_millis() as u64;
    tracing::info!(
        target: "module_reader",
        phase = "minimal_playable_unit",
        skeleton_ms,
        deep_ms = total_ms - skeleton_ms,
        total_ms,
        deep_extracted = out.scenes[entry_idx].extraction_status == SceneExtractionStatus::DeepExtracted,
        "minimal playable unit done (Pass A + Pass B)"
    );
    // ---- 收尾：模组级引导事实抽取（从已解析开场场景 + spine，存 director_facilitation）----
    // env 门 TRPG_MODULE_FACILITATION 默认开；fail-closed：抽不到 → None，零行为倒退
    //（无 sidecar 时 director 仍回退到通用兜底，与今天一致）。
    if super::facilitation::facilitation_enabled() {
        out.facilitation_facts =
            super::facilitation::extract_facilitation_facts(client, &out, entry_idx).await;
        tracing::info!(
            target: "module_reader", phase = "facilitation",
            extracted = out.facilitation_facts.is_some(),
            "facilitation facts extraction done"
        );
    }
    // L8.1 — deterministic narrative-anchor extraction (additive MATERIAL; flag-gated, default OFF
    // via TRPG_MODULE_ANCHORS). Pure/idempotent over the parsed scenes (no LLM, no name-branch);
    // attaches anchors onto the module's DirectorModuleConfig so L8.2 can seed threads from them.
    // OFF ⇒ never run ⇒ byte-identical bundle.
    if super::facilitation::narrative_anchors_enabled() {
        let anchors = trpg_model::extract_narrative_anchors(&out.scenes);
        if !anchors.is_empty() {
            let cfg = out.facilitation_facts.get_or_insert_with(Default::default);
            cfg.narrative_anchors = anchors;
            tracing::info!(
                target: "module_reader", phase = "narrative_anchors",
                count = cfg.narrative_anchors.len(), "narrative anchors extracted (deterministic)"
            );
        }
    }
    // Pass B 失败 → 入口保持 SkeletonOnly（fail-closed），已得骨架照常返回。
    Ok(out)
}

// ---- Tests (deterministic parts only; the LLM loop is validated live in Phase 6) ----

#[cfg(test)]
#[path = "module_reader_tests.rs"]
mod tests;

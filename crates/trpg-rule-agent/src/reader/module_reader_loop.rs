//! module_reader 的私有 ReAct 循环 + 工具分发，从 module_reader.rs 拆出以守 ≤400 行。
//! 逐行镜像 chargen_compile.rs 的 `run_compile_loop`(loop) / `compile_dispatch`(dispatch)
//! 与 object_compile.rs 的 `run_object_loop` / `object_dispatch`：同一个 ReAct 套路、
//! 同一组 nav 工具(+read_layout)、submit 工具名命中即返回、未提交即 None（fail-closed）。
use super::chargen_compile::read_layout;
use super::module_reader::{
    apply_deep_to_node, merge_deep_entities, stub_to_node, ModuleReaderCtx, ModuleReadout,
};
use super::tools;
use serde_json::{json, Value};
use std::collections::HashSet;
use trpg_llm::LlmClient;
use trpg_model::{ScenarioNode, SceneExtractionStatus};

/// Pass B 深抽器 SYS prompt（零硬编码：无规则/语言/模组专名）。
/// 一次性路径与 ReAct 回退路径共用：① 念白只按锚句抓、无则 null 绝不编造（RW8）；
/// ② links 的 to_node_id 只能从给定候选选、每条必须带 source_anchor 摘当前页原文，
/// 抽不出锚点就不输出该边（fail-closed 抗幻觉）。
const DEEP_SYS: &str = "你是模组场景深抽器。对给定的入口场景及其依赖闭包，读相关页，填 read_aloud\
（仅当文本有可念的 boxed/念白文本时；优先锚句如『read, or paraphrase the following text:』或第二\
人称『你们』的 boxed 段，没有就留 null，绝不编造）、gm_notes、referenced_*_ids、闭包实体详情\
（entities，每个带 id+kind+name+正文）。\
links（出口）规则：to_node_id **只能从给定候选 node_id 里选**（不在候选里的一律不输出）；\
每条 link 必须带 link_type∈{spatial,trigger,timeline,sequential,branch} 与 **source_anchor\
（摘当前页原文片段，证明这条通路/触发存在）**，抽不出锚点就**不要**输出该条边，绝不编造。\
scene_mechanics：仅当场景文本**明确写出**检定（技能/难度/后果）时编译为结构化条目 \
{intent_id, description, tested_parameter, difficulty, effect_policy{on_success,on_failure}, \
source_anchor}；source_anchor 必须摘当前页原文片段；文本没写明的**绝不编造**，没有就交空数组。\
difficulty **必须**是结构化对象 {\"kind\":\"dv\"|\"static\"|\"target_number\",\"value\":整数}\
——文本给了目标数就抽成数字进 value；条件变体/原文措辞写进 note 字段，绝不塞进 value；\
只有文本确实给不出数字时才保留原文字符串（字符串不绑定结算目标，引擎 fail-closed 走规则核默认）。\
effect_policy 条目字段**逐字对齐**：create_fact={kind,target,fact}（fact 是对象，没有 value 字段）；\
modify_track={kind,owner_kind,owner_id?,track_id,op∈{add,subtract,set},amount 整数}；\
set_object_state={kind,object_id,patch}；start_countdown={kind,label,amount,scale,payload}；\
字段名/形态写错的条目引擎判不可执行（记录但不生效）。\
用 get_toc/search/read/read_layout 检索（若已给页面文本则直接据此抽），最后 submit_deep。";

/// 聚焦工具循环（镜像 `run_compile_loop`/`run_object_loop`）。返回 submit 工具的 args。
/// 任一步失败（LLM 错误、循环耗尽未提交）→ None，由调用方 fail-closed 处理。
pub(super) async fn run_module_loop(
    client: &dyn LlmClient,
    system: &str,
    seed: &str,
    ctx: &ModuleReaderCtx<'_>,
    budget: usize,
    submit: Value,
    submit_name: &str,
) -> Option<Value> {
    let mut tool_schemas = tools::nav_tools();
    tool_schemas.push(json!({"type":"function","function":{
        "name":"read_layout",
        "description":"aligned-table / column-aware view of page(s) like \"16\" or \"16-18\" — use for boxed read-aloud or stat tables that look misaligned in read().",
        "parameters":{"type":"object","properties":{"pages":{"type":"string"}},"required":["pages"]}
    }}));
    tool_schemas.push(submit);

    let mut msgs = vec![
        json!({"role": "system", "content": system}),
        json!({"role": "user", "content": seed}),
    ];
    for _ in 0..(budget + 8) {
        let resp = client.complete_with_tools(msgs.clone(), tool_schemas.clone()).await.ok()?;
        let message = resp.pointer("/choices/0/message").cloned().unwrap_or_else(|| json!({}));
        let tcs = message.get("tool_calls").and_then(Value::as_array).cloned().unwrap_or_default();
        if tcs.is_empty() {
            msgs.push(message);
            msgs.push(json!({"role": "user", "content": format!("Use the tools, then call {submit_name}.")}));
            continue;
        }
        msgs.push(message);
        for tc in &tcs {
            let name = tc.pointer("/function/name").and_then(Value::as_str).unwrap_or("");
            let args: Value = tc
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_else(|| json!({}));
            let id = tc.get("id").and_then(Value::as_str).unwrap_or("");
            if name == submit_name {
                return Some(args);
            }
            let out = module_dispatch(ctx, name, &args);
            msgs.push(json!({"role": "tool", "tool_call_id": id, "content": out}));
        }
    }
    None
}

/// 工具分发（镜像 `compile_dispatch`/`object_dispatch`，仅 get_toc/search/read/read_layout）。
fn module_dispatch(ctx: &ModuleReaderCtx<'_>, name: &str, args: &Value) -> String {
    let cap = |s: String| if s.len() <= 3000 { s } else { s.chars().take(3000).collect() };
    match name {
        "get_toc" => tools::toc(ctx.units, 40),
        "search" => {
            let kws: Vec<String> = args
                .get("keywords")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            cap(tools::search(ctx.units, &kws, 8))
        }
        "read" => cap(tools::read(ctx.units, args.get("pages").and_then(Value::as_str).unwrap_or(""))),
        "read_layout" => match &ctx.sidecar_text {
            Some(s) => cap(read_layout(s, args.get("pages").and_then(Value::as_str).unwrap_or(""))),
            None => "[no layout view available for this book — read() instead]".into(),
        },
        _ => format!("unknown tool {name}"),
    }
}

/// submit_deep 工具 schema（一次性路径与 ReAct 回退路径共用，单一事实源）。
/// scene_mechanics 的字段形态 schema 在 scene_mechanics.rs（与解析器同源，C7）；
/// scene 其余键保持开放对象（properties 不收口 additional 键）。
fn submit_deep_tool() -> Value {
    tools::submit_tool(
        "submit_deep",
        "Submit the entry scene's deep content (read_aloud/gm_notes/links/referenced_*_ids/scene_mechanics) + closure entity details.",
        json!({
            "scene": {"type": "object", "properties": {
                "scene_mechanics": super::scene_mechanics::scene_mechanics_schema()
            }},
            "entities": {"type": "array", "items": {"type": "object"}}
        }),
        &["scene"],
    )
}

/// 深抽 `readout.scenes[idx]` 场景及其依赖闭包，就地填入 readout
/// （apply_deep_to_node + merge_deep_entities）。返回是否翻成 DeepExtracted。
///
/// **一次性路径（RW1）**：场景已知 page_start 且 ctx 有 sidecar_text →
/// 切 [page_start, page_end] 那几页 + 一次 complete_with_tools(submit_deep)，
/// **不进多轮 ReAct**；DEEP prompt 带 RW2 候选出口列表 + RW8 锚句提示。
/// **回退路径**：无页码/无 sidecar → 沿用 run_module_loop ReAct（保持原行为）。
///
/// 两条路径都在 apply/merge 后对 node.links 做 **fail-closed 过滤**：
/// 丢掉 source_anchor 为空、或 to_node_id 不在 readout.scenes 的野边/幻觉边。
///
/// 复用：run_module_reader 首场景、background-continue job、scene_navigator 共用此函数。
/// fail-closed：调用失败/无正文（无 read_aloud 且无 gm_notes）→ 保持 SkeletonOnly、
/// 返回 false，不编造。idx 越界 → false。
pub async fn deep_extract_scene_in_place(
    client: &dyn LlmClient,
    ctx: &ModuleReaderCtx<'_>,
    readout: &mut ModuleReadout,
    idx: usize,
    budget: usize,
) -> bool {
    let Some(entry) = readout.scenes.get(idx) else {
        return false;
    };
    // RW2 候选出口（node_id），连标题一起喂给 LLM 当出口候选集合。
    let candidates = super::module_graph_edges::candidate_neighbors(&readout.scenes, idx);
    // 一次性路径 vs ReAct 回退（fail-closed：缺页码/无 sidecar → 回退）。
    let oneshot = entry.page_start.is_some() && ctx.sidecar_text.is_some();
    let deep = if oneshot {
        oneshot_deep_extract(client, ctx, &readout.scenes, idx, &candidates).await
    } else {
        react_deep_extract(client, ctx, &readout.scenes[idx], budget).await
    };
    let Some(deep) = deep else {
        // fail-closed：未提交 → 保持 SkeletonOnly。
        return false;
    };
    // 深抽前快照既有边 target（骨架 Pass A 的 TOC 导航边 / 全局桥接边——建时已校验、可信、
    // source_anchor=None）。收口时这些既有边豁免"必须带 anchor"，只有深抽**新提交**的边才需带
    // anchor（防幻觉出口）。否则 retain 会把可信的骨架/桥接边连同无锚一起误删（深抽即清空场景边）。
    let pre_existing: HashSet<String> =
        readout.scenes[idx].links.iter().map(|l| l.to_node_id.clone()).collect();
    apply_deep_to_node(&mut readout.scenes[idx], &deep);
    merge_deep_entities(readout, &deep);
    // fail-closed 收口：to 必须是已知场景；且「既有边」OR「深抽新边带非空 anchor」才保留。
    let known: HashSet<String> =
        readout.scenes.iter().map(|s| s.node_id.clone()).collect();
    readout.scenes[idx].links.retain(|l| {
        known.contains(&l.to_node_id)
            && (pre_existing.contains(&l.to_node_id)
                || l.source_anchor.as_deref().map(str::trim).map(|s| !s.is_empty()).unwrap_or(false))
    });
    readout.scenes[idx].extraction_status == SceneExtractionStatus::DeepExtracted
}

/// 一次性切页深抽（RW1）：已知页码 → 一次 complete_with_tools(submit_deep)，不进 ReAct。
/// 切 [page_start, page_end]（end 缺则 = start）那几页 layout 文本喂入；候选出口列表 +
/// 锚句提示进 prompt。返回 submit_deep 的 args（从单次响应的 tool_calls 取）；
/// LLM 错误/未调 submit_deep → None（由调用方 fail-closed）。
async fn oneshot_deep_extract(
    client: &dyn LlmClient,
    ctx: &ModuleReaderCtx<'_>,
    scenes: &[trpg_model::ScenarioNode],
    idx: usize,
    candidates: &[String],
) -> Option<Value> {
    let entry = scenes.get(idx)?;
    let sidecar = ctx.sidecar_text.as_deref()?;
    let start = entry.page_start?;
    let end = entry.page_end.unwrap_or(start).max(start);
    let pages = if end > start { format!("{start}-{end}") } else { format!("{start}") };
    let page_text = read_layout(sidecar, &pages);
    if page_text.trim().is_empty() {
        return None; // 切不到页内容 → fail-closed 回 None。
    }
    // 候选出口（node_id|title），供 LLM 从中选实际出口（只能选给定 id）。
    let cand_view: Vec<Value> = candidates
        .iter()
        .filter_map(|id| scenes.iter().find(|s| &s.node_id == id))
        .map(|s| json!({"node_id": s.node_id, "title": s.title}))
        .collect();
    let closure = super::module_reader::dependency_closure(entry);
    let entry_json = serde_json::to_string(&json!({
        "node_id": entry.node_id, "title": entry.title, "kind": entry.node_type,
        "summary": entry.summary, "page_start": entry.page_start, "page_end": entry.page_end,
    }))
    .unwrap_or_default();
    let user = format!(
        "入口场景：\n{entry_json}\n依赖闭包 id（请深抽这些实体的详情）：{closure:?}\n\
候选出口（links 的 to_node_id 只能从这里选；node_id|title）：\n{}\n\n\
下面是该场景所在页（{pages}）的版面文本。据此填该场景的 read_aloud/gm_notes/referenced_*_ids\
与 links（每条 link 的 to_node_id 必须取自上面候选、且必须带 source_anchor 摘自下文原文），\
以及 scene_mechanics（仅当下文明确写出检定时才编译，source_anchor 摘自下文原文，没有就交空数组），\
再补闭包实体详情，最后 **必须调用 submit_deep** 提交。\n\n=== 页面文本 ===\n{page_text}",
        serde_json::to_string(&cand_view).unwrap_or_default(),
    );
    let submit_deep = submit_deep_tool();
    let msgs = vec![
        json!({"role": "system", "content": DEEP_SYS}),
        json!({"role": "user", "content": user}),
    ];
    let resp = client.complete_with_tools(msgs, vec![submit_deep]).await.ok()?;
    let tcs = resp.pointer("/choices/0/message/tool_calls").and_then(Value::as_array)?;
    for tc in tcs {
        if tc.pointer("/function/name").and_then(Value::as_str) == Some("submit_deep") {
            return tc
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .and_then(|s| serde_json::from_str(s).ok());
        }
    }
    None // 模型没调 submit_deep → fail-closed。
}

/// ReAct 回退路径：无页码/无 sidecar 时沿用原多轮 run_module_loop(submit_deep) 行为。
async fn react_deep_extract(
    client: &dyn LlmClient,
    ctx: &ModuleReaderCtx<'_>,
    entry: &trpg_model::ScenarioNode,
    budget: usize,
) -> Option<Value> {
    let closure = super::module_reader::dependency_closure(entry);
    let entry_json = serde_json::to_string(&json!({
        "node_id": entry.node_id, "title": entry.title, "kind": entry.node_type,
        "summary": entry.summary, "page_start": entry.page_start, "page_end": entry.page_end,
    }))
    .unwrap_or_default();
    let deep_seed = format!(
        "入口场景：\n{entry_json}\n依赖闭包 id（请深抽这些实体的详情）：{closure:?}\n读相关页，填该场景的 read_aloud/gm_notes/links/referenced_*_ids 与闭包实体详情，最后 submit_deep。"
    );
    run_module_loop(client, DEEP_SYS, &deep_seed, ctx, budget, submit_deep_tool(), "submit_deep").await
}

// === Task 5：Pass A gleaning 回环（骨架补漏）—— 纯合并 helper + bounded loop。 ===

/// gleaning 补漏 SYS prompt（零硬编码：无规则/语言/模组专名）。
const GLEAN_SYS: &str = "你在做骨架补漏。下面给你『已抽出的场景列表』和『模组目录(TOC)』。\
对照目录,找出目录里出现、但已抽列表里缺失的可玩单元/场景/章节。只补缺失的,绝不重复已有的。\
输出 JSON {\"scenes\":[{node_id,title,kind,page_start,page_end,...}], \"still_missing\":\"YES|NO\"};\
没有遗漏就 scenes 空 + still_missing NO。绝不编造目录里没有的场景。";

/// 合并 gleaning 补抽结果到已抽 scene 列表（按 node_id 去重），返回是否还需继续（still_missing==YES）。
pub fn merge_gleaned(have: &mut Vec<Value>, glean: &Value) -> bool {
    if let Some(arr) = glean.get("scenes").and_then(|v| v.as_array()) {
        let seen: HashSet<String> = have
            .iter()
            .filter_map(|s| s.get("node_id").and_then(|v| v.as_str()).map(str::to_string))
            .collect();
        for s in arr {
            if let Some(id) = s.get("node_id").and_then(|v| v.as_str()) {
                if !seen.contains(id) {
                    have.push(s.clone());
                }
            }
        }
    }
    glean
        .get("still_missing")
        .and_then(|v| v.as_str())
        .map(|s| s.eq_ignore_ascii_case("yes"))
        .unwrap_or(false)
}

/// Pass A 骨架补漏回环：把 `scenes`（reader 已提交的 stub JSON 列表）就地补全。
/// 每轮喂「已抽 scene 列表(node_id+title) + TOC」让 reader 找漏 → `merge_gleaned` 合并；
/// `still_missing!=YES` 或达 `max_gleanings` 上限停。fail-closed：LLM/解析失败→停用已得。
/// 返回新增的 stub 数（仅用于日志/判定）。
/// 被 `complete_skeleton_stubs` 调用（前台 env 门控补全 + 后续 trpg-api 后台 job）。
pub(super) async fn run_gleaning_rounds(
    client: &dyn LlmClient,
    ctx: &ModuleReaderCtx<'_>,
    scenes: &mut Vec<Value>,
    budget: usize,
    max_gleanings: usize,
) -> usize {
    let before = scenes.len();
    let submit = tools::submit_tool(
        "submit_gleaned",
        "Submit scenes present in the TOC but missing from the already-extracted list (+ still_missing).",
        json!({
            "scenes": {"type": "array", "items": {"type": "object"}},
            "still_missing": {"type": "string", "description": "YES if the TOC likely still has uncovered playable units, else NO"}
        }),
        &["scenes", "still_missing"],
    );
    for _ in 0..max_gleanings {
        // 已抽列表的精简视图（node_id+title）；TOC 由 reader 经 get_toc 工具自取。
        let have_view: Vec<Value> = scenes
            .iter()
            .map(|s| {
                json!({
                    "node_id": s.get("node_id").and_then(Value::as_str).unwrap_or(""),
                    "title": s.get("title").and_then(Value::as_str).unwrap_or(""),
                })
            })
            .collect();
        let seed = format!(
            "已抽出的场景列表（node_id|title）：\n{}\n请用 get_toc 读目录,找出目录里有、但上面缺失的可玩单元,最后 submit_gleaned（没有遗漏就 scenes 空 + still_missing NO）。",
            serde_json::to_string(&have_view).unwrap_or_default()
        );
        // fail-closed：本轮没拿到提交 → 停，沿用已得。
        let Some(glean) =
            run_module_loop(client, GLEAN_SYS, &seed, ctx, budget, submit.clone(), "submit_gleaned").await
        else {
            break;
        };
        let still = merge_gleaned(scenes, &glean);
        if !still {
            break;
        }
    }
    scenes.len() - before
}

/// 骨架补全（前台 env 门控 + 后台 trpg-api job 共用）：把已抽 `scenes`（ScenarioNode）
/// 投成 stub Value 列表、跑 `run_gleaning_rounds` 对照 TOC 补漏，再把**新增**的 stub
/// （node_id 不在原集合）经 `stub_to_node` 转成 SkeletonOnly 节点追加进 `scenes`（按 node_id 去重）。
/// 返回新增节点数。fail-closed：无新增/转换失败 → 0，`scenes` 不被破坏（只追加、不改动原节点）。
pub async fn complete_skeleton_stubs(
    client: &dyn LlmClient,
    ctx: &ModuleReaderCtx<'_>,
    scenes: &mut Vec<ScenarioNode>,
    budget: usize,
) -> usize {
    let mut existing: HashSet<String> = scenes.iter().map(|s| s.node_id.clone()).collect();
    // 已抽场景 → stub Value（node_id+title），喂给 gleaning 回环当「已有」基线。
    let mut stubs: Vec<Value> = scenes
        .iter()
        .map(|s| json!({"node_id": s.node_id, "title": s.title}))
        .collect();
    run_gleaning_rounds(client, ctx, &mut stubs, budget, 2).await;
    // 只追加原集合里没有的新 stub（gleaning 内部已按 node_id 去重，这里再防野/重）。
    let mut added = 0usize;
    for stub in &stubs {
        let Some(id) = stub.get("node_id").and_then(Value::as_str) else { continue };
        if existing.contains(id) {
            continue;
        }
        if let Some(node) = stub_to_node(stub) {
            existing.insert(node.node_id.clone());
            scenes.push(node);
            added += 1;
        }
    }
    added
}

#[cfg(test)]
#[path = "module_reader_loop_tests.rs"]
mod tests;

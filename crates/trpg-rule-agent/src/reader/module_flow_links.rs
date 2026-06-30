//! TRPG_MODULE_FLOW_LINKS (默认 OFF)：生产侧"已授权场景流转脊"抽取。
//!
//! 现状根因：Pass A 骨架从不问 inter-scene links、Pass B 深抽只跑入口一个场景，
//! 于是落地的唯一邻接是 `apply_bridge_edges` 的纯实体共享桥（全 `Spatial`、
//! `source_anchor=None`）——一张无方向脊的平面网，navigator 无结构可循。
//!
//! 本 pass 是 **additive / flag-gated**：开关真值时，逐场景读其所在页正文，让 LLM 只抽
//! 该场景**指向其他已知场景**的已授权流转出边（`link_type∈{sequential,trigger,branch}`，
//! 每条必须带摘自原文的 `source_anchor`），经 fail-closed 过滤后 `merge_links` 并入
//! `scene.links`——与既有 spatial 桥**并存**（桥保留为 fallback，不再是唯一邻接）。
//! OFF ⇒ 整个 pass 不被调用 ⇒ bundle 字节级不变。
//!
//! 复用（绝不重造）：`read_layout`(chargen_compile)、`merge_links`/`ScenarioLink`/`LinkType`、
//! `tools::submit_tool`；不复制 `apply_bridge_edges`。零硬编码、无 ruleset_id/module_id 分支。

use super::chargen_compile::read_layout;
use super::module_reader::{merge_links, ModuleReaderCtx, ModuleReadout};
use super::tools;
use serde_json::{json, Value};
use std::collections::HashSet;
use trpg_llm::LlmClient;
use trpg_model::{LinkType, ScenarioLink, ScenarioNode};

/// 流转脊抽取门：**默认 OFF**（additive、opt-in）。`TRPG_MODULE_FLOW_LINKS=1/true/on/yes` 开。
/// 对标 `facilitation::narrative_anchors_enabled` 的默认关形态。OFF ⇒ 字节级不变。
pub fn flow_links_enabled() -> bool {
    std::env::var("TRPG_MODULE_FLOW_LINKS")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "on" | "yes"
            )
        })
        .unwrap_or(false)
}

/// 字符串 → 流转型 `LinkType`。只接受**有向已授权**型（sequential/trigger/branch/timeline）；
/// `spatial`（=实体共享桥的活儿）与未知一律 `None`（本 pass 不产出 spatial）。
fn flow_link_type(s: &str) -> Option<LinkType> {
    match s.trim().to_ascii_lowercase().as_str() {
        "sequential" => Some(LinkType::Sequential),
        "trigger" => Some(LinkType::Trigger),
        "branch" => Some(LinkType::Branch),
        "timeline" => Some(LinkType::Timeline),
        _ => None,
    }
}

/// 从 LLM 提交的 `{links:[...]}` 解析 + fail-closed 过滤出可信的"已授权流转出边"。
/// 保留条件（全满足）：`to_node_id` ∈ `known` 且非自身；`source_anchor` 摘原文非空；
/// `link_type` 是流转型（见 `flow_link_type`）。anchorless / 野 target / 自指 / spatial / 未知型
/// 一律丢弃，绝不编造。纯、确定、可单测。
pub(super) fn parse_flow_links(
    submitted: &Value,
    self_id: &str,
    known: &HashSet<String>,
) -> Vec<ScenarioLink> {
    let Some(arr) = submitted.get("links").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for l in arr {
        let to = l
            .get("to_node_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if to.is_empty() || to == self_id || !known.contains(&to) {
            continue;
        }
        let anchor = l
            .get("source_anchor")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let Some(anchor) = anchor else {
            continue; // fail-closed：无锚不收。
        };
        let Some(lt) = flow_link_type(l.get("link_type").and_then(Value::as_str).unwrap_or(""))
        else {
            continue; // 非流转型（spatial/未知）不收。
        };
        let reason = l
            .get("reason")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| "authored inter-scene flow".to_string());
        out.push(ScenarioLink {
            to_node_id: to,
            reason,
            clue_id: None,
            link_type: lt,
            source_anchor: Some(anchor.to_string()),
        });
    }
    out
}

const FLOW_SYS: &str = "你是模组场景流转抽取器。给定一个场景及候选目标场景列表，读该场景所在页的\
正文，只抽该场景**指向其他场景的已授权导航/流转出边**：\
① to_node_id 只能从候选里选，不在候选里的一律不输出，不要指向自身；\
② link_type ∈ {sequential（读完/章节顺序进入下一节）, trigger（剧情条件触发，如『若 PC 做了 X 则…』）, \
branch（玩家选择/分歧导致不同去向）}；\
③ 每条边必须带 source_anchor，摘自下文原文片段、证明该通路/条件存在\
（如『If the PCs let the lawmen take Athena…』『Once they reach the rooftop, go to…』）；\
④ 抽不出锚点的边一律不输出，绝不编造；正文没写明任何流转就交空 links 数组。\
最后必须调用 submit_flow_links。";

/// `submit_flow_links` 工具 schema（单一事实源）。
fn submit_flow_tool() -> Value {
    tools::submit_tool(
        "submit_flow_links",
        "Submit authored inter-scene flow links (sequential/trigger/branch) found in this scene's text — each with a source_anchor quoting the cue.",
        json!({
            "links": {"type": "array", "items": {"type": "object", "properties": {
                "to_node_id": {"type": "string"},
                "link_type": {"type": "string", "enum": ["sequential", "trigger", "branch"]},
                "source_anchor": {"type": "string", "description": "verbatim cue/heading text from the page proving this transition exists"},
                "reason": {"type": "string"}
            }}}
        }),
        &["links"],
    )
}

/// 单场景一刀：切该场景所在页版面文本 + 全场景目标菜单 → 一次 `submit_flow_links`。
/// 返回提交的 args（`{links:[...]}`）；LLM 错误 / 未调 submit / 无页文本 → None（fail-closed）。
async fn solicit_flow_links(
    client: &dyn LlmClient,
    scene: &ScenarioNode,
    menu_str: &str,
    page_text: &str,
) -> Option<Value> {
    let user = format!(
        "当前场景：{}|{}\n候选目标场景（to_node_id 只能从这里选；node_id|title）：\n{menu_str}\n\n\
下面是当前场景所在页的版面文本。只抽该场景**指向其他场景的导航/流转出边**（每条带 link_type 与 \
source_anchor 摘自下文原文），抽不出锚点的边不输出，没有就交空 links 数组。最后必须调用 submit_flow_links。\
\n\n=== 页面文本 ===\n{page_text}",
        scene.node_id, scene.title,
    );
    let msgs = vec![
        json!({"role": "system", "content": FLOW_SYS}),
        json!({"role": "user", "content": user}),
    ];
    let resp = client
        .complete_with_tools(msgs, vec![submit_flow_tool()])
        .await
        .ok()?;
    let tcs = resp
        .pointer("/choices/0/message/tool_calls")
        .and_then(Value::as_array)?;
    for tc in tcs {
        if tc.pointer("/function/name").and_then(Value::as_str) == Some("submit_flow_links") {
            return tc
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .and_then(|s| serde_json::from_str(s).ok());
        }
    }
    None
}

/// 逐场景抽取已授权流转出边并 `merge_links` 并入（与 spatial 桥并存）。返回**接受**的
/// typed anchored 边数（日志用）。fail-closed：无 sidecar / 场景无页码 / 切不到页文 /
/// LLM 失败 → 跳过该场景（绝不编造）。idempotent：merge_links 按 to_node_id 去重。
pub async fn extract_flow_links_all(
    client: &dyn LlmClient,
    ctx: &ModuleReaderCtx<'_>,
    readout: &mut ModuleReadout,
) -> usize {
    let Some(sidecar) = ctx.sidecar_text.as_deref() else {
        return 0; // 无版面文本 → 无法摘锚 → fail-closed 不产出。
    };
    let known: HashSet<String> = readout.scenes.iter().map(|s| s.node_id.clone()).collect();
    // 目标菜单一次成型（node_id|title）；parse 阶段再排除自身/野 target。
    let menu: Vec<Value> = readout
        .scenes
        .iter()
        .map(|s| json!({"node_id": s.node_id, "title": s.title}))
        .collect();
    let menu_str = serde_json::to_string(&menu).unwrap_or_default();
    let mut accepted = 0usize;
    for idx in 0..readout.scenes.len() {
        let Some(start) = readout.scenes[idx].page_start else {
            continue; // 无页码 → 跳过（fail-closed）。
        };
        let end = readout.scenes[idx].page_end.unwrap_or(start).max(start);
        let pages = if end > start {
            format!("{start}-{end}")
        } else {
            format!("{start}")
        };
        let page_text = read_layout(sidecar, &pages);
        if page_text.trim().is_empty() {
            continue;
        }
        let self_id = readout.scenes[idx].node_id.clone();
        let Some(submitted) =
            solicit_flow_links(client, &readout.scenes[idx], &menu_str, &page_text).await
        else {
            continue;
        };
        let links = parse_flow_links(&submitted, &self_id, &known);
        if links.is_empty() {
            continue;
        }
        accepted += links.len();
        merge_links(&mut readout.scenes[idx].links, links);
    }
    accepted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known_set(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn keeps_anchored_typed_drops_anchorless_and_wild() {
        let known = known_set(&["self", "a", "b", "c"]);
        let submitted = json!({"links": [
            {"to_node_id": "b", "link_type": "trigger", "source_anchor": "If the PCs call Foxwell, he warns them…"},
            {"to_node_id": "c", "link_type": "branch", "source_anchor": "Once they reach the rooftop, go to the finale."},
            {"to_node_id": "a", "link_type": "sequential", "source_anchor": ""},      // anchorless → drop
            {"to_node_id": "a", "link_type": "sequential"},                            // missing anchor → drop
            {"to_node_id": "zzz", "link_type": "trigger", "source_anchor": "x"},      // unknown target → drop
            {"to_node_id": "self", "link_type": "trigger", "source_anchor": "x"},     // self → drop
            {"to_node_id": "b", "link_type": "spatial", "source_anchor": "x"},        // spatial not a flow type → drop
        ]});
        let got = parse_flow_links(&submitted, "self", &known);
        assert_eq!(got.len(), 2, "only the 2 anchored typed links survive");
        assert_eq!(got[0].to_node_id, "b");
        assert_eq!(got[0].link_type, LinkType::Trigger);
        assert!(got[0].source_anchor.as_deref().unwrap().contains("Foxwell"));
        assert_eq!(got[1].to_node_id, "c");
        assert_eq!(got[1].link_type, LinkType::Branch);
        assert!(
            got.iter().all(|l| l
                .source_anchor
                .as_deref()
                .map(|s| !s.is_empty())
                .unwrap_or(false)),
            "every surviving link carries a non-empty source_anchor (fail-closed)"
        );
    }

    #[test]
    fn empty_or_missing_links_yields_none() {
        let known = known_set(&["a"]);
        assert!(parse_flow_links(&json!({}), "self", &known).is_empty());
        assert!(parse_flow_links(&json!({"links": []}), "self", &known).is_empty());
    }

    #[test]
    fn flow_link_type_accepts_directional_rejects_spatial() {
        assert_eq!(flow_link_type("sequential"), Some(LinkType::Sequential));
        assert_eq!(flow_link_type("TRIGGER"), Some(LinkType::Trigger));
        assert_eq!(flow_link_type(" branch "), Some(LinkType::Branch));
        assert_eq!(flow_link_type("timeline"), Some(LinkType::Timeline));
        assert_eq!(
            flow_link_type("spatial"),
            None,
            "spatial is the bridge's job, not a flow link"
        );
        assert_eq!(flow_link_type("garbage"), None);
    }

    #[test]
    fn reason_defaults_when_missing() {
        let known = known_set(&["a"]);
        let submitted = json!({"links": [
            {"to_node_id": "a", "link_type": "trigger", "source_anchor": "If the PCs flee…"}
        ]});
        let got = parse_flow_links(&submitted, "self", &known);
        assert_eq!(got.len(), 1);
        assert!(
            !got[0].reason.is_empty(),
            "missing reason falls back to a non-empty label"
        );
    }

    #[test]
    fn gate_defaults_off_when_unset() {
        // env is process-global; save/restore around the assertion.
        let saved = std::env::var("TRPG_MODULE_FLOW_LINKS").ok();
        std::env::remove_var("TRPG_MODULE_FLOW_LINKS");
        assert!(
            !flow_links_enabled(),
            "unset → default OFF (byte-identical baseline)"
        );
        std::env::set_var("TRPG_MODULE_FLOW_LINKS", "1");
        assert!(flow_links_enabled(), "\"1\" → ON");
        std::env::set_var("TRPG_MODULE_FLOW_LINKS", "0");
        assert!(!flow_links_enabled(), "\"0\" → OFF");
        match saved {
            Some(v) => std::env::set_var("TRPG_MODULE_FLOW_LINKS", v),
            None => std::env::remove_var("TRPG_MODULE_FLOW_LINKS"),
        }
    }
}

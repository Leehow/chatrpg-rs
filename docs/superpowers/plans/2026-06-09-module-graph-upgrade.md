# ModuleGraph 图谱升级 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`).

**Goal:** 把模组抽取从"点+实体索引、边全空、骨架飘"升级成连通(双源边)、完整(gleaning)、可验证(validator 质量门)、去重对齐的场景知识图谱,边抗幻觉(source-anchor)。

**Architecture:** `run_module_reader` 流水线扩展:Pass A 骨架+gleaning 回环 → 实体去重 → 双源建边(实体桥接纯函数 + LLM 线索边 Pass C 带 source-anchor)合并进 `ScenarioNode.links` → validator(入口 BFS 连通性,不 ok 触发一次重 gleaning)→ Pass B 深抽首场景。边只告知(P4 出口+validator),不约束 scene_navigator。

**Tech Stack:** Rust(trpg-model / trpg-rule-agent reader),LLM via codex-relay(gpt-5.4-mini),复用 ScenarioLink/LinkType/ModuleGraph。

**护栏:** 语义优先零硬编码 · fail-closed(无 anchor/to 不存在→丢边,绝不编造;LLM 失败→降级)· 文件 ≤400 行 · 复用优先。

**Git:** 工作副本无 git,commit 步骤跳过。

**并行(用户建议多用 subagent):** T1(trpg-model)、T2(module_graph_edges.rs 新文件)、T3(module_graph_validator.rs 新文件)互不冲突→可并行一波。T4-T7 同 module_reader*.rs 串行。T8 e2e 最后。

**Spec:** `docs/superpowers/specs/2026-06-09-module-graph-upgrade-design.md`

---

## File Structure

| 文件 | 责任 | 动作 |
|---|---|---|
| `crates/trpg-model/src/lib.rs` | ScenarioLink + source_anchor | Modify |
| `crates/trpg-rule-agent/src/reader/module_graph_edges.rs` | 实体桥接边(纯) | Create |
| `crates/trpg-rule-agent/src/reader/module_graph_validator.rs` | 连通性 validator(纯) | Create |
| `crates/trpg-rule-agent/src/reader/module_reader.rs` `_loop.rs` | gleaning + Pass C 线索边 + dedup + 编排 | Modify |
| `crates/trpg-rule-agent/src/reader/mod.rs` | 导出 | Modify |

---

## Task 1: 数据模型 — ScenarioLink 加 source_anchor

**Files:** Modify `crates/trpg-model/src/lib.rs`(ScenarioLink ~1519 区;扩展测试)

- [ ] **Step 1: 写失败测试**(加到 module_graph_compat_tests)

```rust
#[test]
fn scenario_link_source_anchor_roundtrips_and_defaults() {
    // 旧 link(无 source_anchor)→ None
    let old: ScenarioLink = serde_json::from_str(r#"{"to_node_id":"b","reason":"r","clue_id":null}"#).unwrap();
    assert!(old.source_anchor.is_none(), "旧 link 无 source_anchor → None");
    // 新 link round-trip
    let l = ScenarioLink { to_node_id: "b".into(), reason: "门".into(), clue_id: None,
        link_type: LinkType::Trigger, source_anchor: Some("原文片段".into()) };
    let back: ScenarioLink = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
    assert_eq!(back.source_anchor.as_deref(), Some("原文片段"));
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p trpg-model scenario_link_source_anchor 2>&1 | tail -6`
Expected: 缺字段编译失败。

- [ ] **Step 3: 加字段**(ScenarioLink struct,在 link_type 后)

```rust
    #[serde(default)] pub source_anchor: Option<String>,
```

- [ ] **Step 4: 跑测试 + 全工作区编译**(捕捉 ScenarioLink 字面构造点)

Run: `cargo test -p trpg-model scenario_link_source_anchor 2>&1 | tail -6 && cargo build 2>&1 | tail -20`
Expected: 测试过;若别处 `ScenarioLink { ... }` 字面构造报缺字段 → 加 `source_anchor: None`(grep `ScenarioLink {` 全工作区,逐个补)。**注意 trpg-runtime 测试里有 ScenarioLink 构造**(scene-nav 的测试),补 None。

- [ ] **Step 5: Commit**(无 git → 跳过)

---

## Task 2: 实体桥接边(纯函数,零 LLM)

**Files:** Create `crates/trpg-rule-agent/src/reader/module_graph_edges.rs`;Modify `mod.rs`(加 `pub mod module_graph_edges;`)

- [ ] **Step 1: 建文件 + 失败测试**

```rust
//! 实体桥接边:两场景共享 referenced 实体即连边(Gorinski&Lapata,零 LLM)。纯、确定、可单测。
use trpg_model::{LinkType, ScenarioLink, ScenarioNode};
use std::collections::HashSet;

/// 两场景共享 ≥1 referenced 实体(npc/clue/location/encounter)→ 候选边 (i, j, shared_count)。
pub fn bridge_edges(scenes: &[ScenarioNode]) -> Vec<(usize, usize, usize)> {
    let ent = |n: &ScenarioNode| -> HashSet<&str> {
        n.referenced_npc_ids.iter().chain(&n.referenced_clue_ids)
            .chain(&n.referenced_location_ids).chain(&n.referenced_encounter_ids)
            .map(|s| s.as_str()).collect()
    };
    let sets: Vec<HashSet<&str>> = scenes.iter().map(ent).collect();
    let mut out = Vec::new();
    for i in 0..scenes.len() {
        for j in (i + 1)..scenes.len() {
            let shared = sets[i].intersection(&sets[j]).count();
            if shared > 0 { out.push((i, j, shared)); }
        }
    }
    out
}

/// 把桥接边就地填进双方 links(双向;按 to_node_id 去重,不覆盖已有边)。返回新增边数。
pub fn apply_bridge_edges(scenes: &mut Vec<ScenarioNode>) -> usize {
    let edges = bridge_edges(scenes);
    let ids: Vec<String> = scenes.iter().map(|s| s.node_id.clone()).collect();
    let mut added = 0;
    for (i, j, shared) in edges {
        for (a, b) in [(i, j), (j, i)] {
            let to = ids[b].clone();
            if scenes[a].links.iter().any(|l| l.to_node_id == to) { continue; }
            scenes[a].links.push(ScenarioLink {
                to_node_id: to, reason: format!("共享 {shared} 个实体"),
                clue_id: None, link_type: LinkType::Spatial, source_anchor: None,
            });
            added += 1;
        }
    }
    added
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scene(id: &str, npcs: &[&str]) -> ScenarioNode {
        let mut n = ScenarioNode::default();
        n.node_id = id.into();
        n.referenced_npc_ids = npcs.iter().map(|s| s.to_string()).collect();
        n
    }
    #[test]
    fn shared_entity_makes_edge_none_otherwise() {
        let scenes = vec![scene("a", &["npc1"]), scene("b", &["npc1"]), scene("c", &["npc2"])];
        let e = bridge_edges(&scenes);
        assert_eq!(e, vec![(0, 1, 1)], "a,b 共享 npc1 → 一条边;c 无共享");
    }
    #[test]
    fn apply_is_bidirectional_and_dedups() {
        let mut scenes = vec![scene("a", &["npc1"]), scene("b", &["npc1"])];
        let n1 = apply_bridge_edges(&mut scenes);
        assert_eq!(n1, 2, "双向各一条");
        assert_eq!(scenes[0].links[0].to_node_id, "b");
        assert_eq!(scenes[0].links[0].link_type, LinkType::Spatial);
        let n2 = apply_bridge_edges(&mut scenes);
        assert_eq!(n2, 0, "重复调用不重复加(去重)");
    }
}
```

- [ ] **Step 2: 注册 mod**：`crates/trpg-rule-agent/src/reader/mod.rs` 加 `pub mod module_graph_edges;`

- [ ] **Step 3: 跑测试**

Run: `cargo test -p trpg-rule-agent module_graph_edges 2>&1 | tail -8`
Expected: 2 passed。

---

## Task 3: 连通性 validator(纯函数)

**Files:** Create `crates/trpg-rule-agent/src/reader/module_graph_validator.rs`;Modify `mod.rs`

- [ ] **Step 1: 建文件 + 失败测试**

```rust
//! 倒置三线索 validator(The Alexandrian):入口 BFS 连通性 + 孤岛/死胡同/不可达检测。纯、可单测。
use trpg_model::ScenarioNode;
use std::collections::{HashSet, VecDeque};

#[derive(Debug, Clone, PartialEq)]
pub struct GraphHealth {
    pub total: usize,
    pub reachable: usize,
    pub orphans: Vec<String>,    // 入口不可达
    pub islands: Vec<String>,    // 无入边
    pub dead_ends: Vec<String>,  // 无出边
    pub score: f64,              // reachable / total
    pub ok: bool,                // score>=0.6 且 orphans 不过半
}

/// 从 entry_id 沿 links BFS,算连通健康度。entry 不存在或空图 → score 0、ok=false。
pub fn validate_graph(scenes: &[ScenarioNode], entry_id: &str) -> GraphHealth {
    let total = scenes.len();
    let idx: std::collections::HashMap<&str, usize> =
        scenes.iter().enumerate().map(|(i, s)| (s.node_id.as_str(), i)).collect();
    // BFS
    let mut seen: HashSet<usize> = HashSet::new();
    if let Some(&start) = idx.get(entry_id) {
        let mut q = VecDeque::from([start]);
        seen.insert(start);
        while let Some(u) = q.pop_front() {
            for l in &scenes[u].links {
                if let Some(&v) = idx.get(l.to_node_id.as_str()) {
                    if seen.insert(v) { q.push_back(v); }
                }
            }
        }
    }
    // 入边集
    let mut has_inbound: HashSet<usize> = HashSet::new();
    for s in scenes {
        for l in &s.links {
            if let Some(&v) = idx.get(l.to_node_id.as_str()) { has_inbound.insert(v); }
        }
    }
    let orphans: Vec<String> = scenes.iter().enumerate()
        .filter(|(i, _)| !seen.contains(i)).map(|(_, s)| s.node_id.clone()).collect();
    let islands: Vec<String> = scenes.iter().enumerate()
        .filter(|(i, _)| !has_inbound.contains(i)).map(|(_, s)| s.node_id.clone()).collect();
    let dead_ends: Vec<String> = scenes.iter()
        .filter(|s| s.links.is_empty()).map(|s| s.node_id.clone()).collect();
    let score = if total == 0 { 0.0 } else { seen.len() as f64 / total as f64 };
    let ok = total > 0 && score >= 0.6 && orphans.len() * 2 <= total;
    GraphHealth { total, reachable: seen.len(), orphans, islands, dead_ends, score, ok }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::{LinkType, ScenarioLink};
    fn linked(id: &str, to: &[&str]) -> ScenarioNode {
        let mut n = ScenarioNode::default();
        n.node_id = id.into();
        n.links = to.iter().map(|t| ScenarioLink { to_node_id: t.to_string(),
            reason: "".into(), clue_id: None, link_type: LinkType::Spatial, source_anchor: None }).collect();
        n
    }
    #[test]
    fn connected_graph_is_ok() {
        let scenes = vec![linked("a", &["b"]), linked("b", &["c"]), linked("c", &[])];
        let h = validate_graph(&scenes, "a");
        assert_eq!(h.reachable, 3);
        assert!((h.score - 1.0).abs() < 1e-9);
        assert!(h.ok);
        assert_eq!(h.dead_ends, vec!["c"], "c 无出边=死胡同");
        assert_eq!(h.islands, vec!["a"], "a 无入边=island");
    }
    #[test]
    fn disconnected_flags_orphans_not_ok() {
        // a→b 连通;c,d 孤立(入口不可达)
        let scenes = vec![linked("a", &["b"]), linked("b", &[]), linked("c", &[]), linked("d", &[])];
        let h = validate_graph(&scenes, "a");
        assert_eq!(h.reachable, 2);
        assert_eq!(h.orphans, vec!["c", "d"]);
        assert!(!h.ok, "半数不可达 → 不 ok");
    }
    #[test]
    fn empty_or_missing_entry_not_ok() {
        assert!(!validate_graph(&[], "x").ok);
        assert!(!validate_graph(&[linked("a", &[])], "nope").ok);
    }
}
```

- [ ] **Step 2: 注册 mod**：`mod.rs` 加 `pub mod module_graph_validator;`

- [ ] **Step 3: 跑测试**

Run: `cargo test -p trpg-rule-agent module_graph_validator 2>&1 | tail -8`
Expected: 3 passed。

---

## Task 4: 实体去重(纯归一 + 重写 referenced_ids)

**Files:** Modify `crates/trpg-rule-agent/src/reader/module_graph_edges.rs`(加 dedup 纯函数,与桥接边同文件——都属"图前处理")

- [ ] **Step 1: 写失败测试**(加到 module_graph_edges tests)

```rust
    #[test]
    fn sanitize_key_normalizes() {
        assert_eq!(sanitize_key("拉斯 (Russell)!"), sanitize_key("拉斯russell"));
        assert_eq!(sanitize_key("Dr. Brenner"), "drbrenner");
    }
    #[test]
    fn dedup_merges_by_key_and_rewrites_refs() {
        let mut npcs = vec![
            serde_json::json!({"id":"npc_brenner","name":"Dr. Brenner"}),
            serde_json::json!({"id":"npc_brenner2","name":"Dr Brenner"}),
        ];
        let mut scenes = vec![{ let mut n=ScenarioNode::default(); n.node_id="s1".into();
            n.referenced_npc_ids=vec!["npc_brenner2".into()]; n }];
        let merged = dedup_entities_by_key(&mut npcs, &mut scenes);
        assert_eq!(merged, 1, "两个 Brenner 归一");
        assert_eq!(npcs.len(), 1);
        // 场景引用被重写到规范 id(保留的那个)
        let canon = npcs[0]["id"].as_str().unwrap().to_string();
        assert_eq!(scenes[0].referenced_npc_ids, vec![canon]);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p trpg-rule-agent dedup_merges 2>&1 | tail -6`
Expected: 缺函数。

- [ ] **Step 3: 实现**(加到 module_graph_edges.rs)

```rust
/// 实体名/id 归一 key:小写,仅保留字母数字(含 CJK)。
pub fn sanitize_key(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

/// 按 name 的 sanitize_key 精确合并重复实体(保留首个为规范),并把所有场景 referenced_npc_ids
/// 里指向被合并 id 的引用重写到规范 id。返回合并掉的实体数。纯、确定。
/// (模糊对的 LLM 二次确认是 reader 集成步,不在此纯函数。)
pub fn dedup_entities_by_key(entities: &mut Vec<serde_json::Value>, scenes: &mut [ScenarioNode]) -> usize {
    use std::collections::HashMap;
    let mut canon: HashMap<String, String> = HashMap::new(); // key -> 规范 id
    let mut remap: HashMap<String, String> = HashMap::new();  // 旧 id -> 规范 id
    let mut keep = Vec::new();
    let mut merged = 0;
    for e in entities.drain(..) {
        let id = e.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let name = e.get("name").and_then(|v| v.as_str()).unwrap_or(&id);
        let k = sanitize_key(name);
        if k.is_empty() { keep.push(e); continue; }
        if let Some(cid) = canon.get(&k) {
            remap.insert(id, cid.clone());
            merged += 1;
        } else {
            canon.insert(k, id.clone());
            keep.push(e);
        }
    }
    *entities = keep;
    if !remap.is_empty() {
        let rewrite = |v: &mut Vec<String>| { for x in v.iter_mut() { if let Some(c) = remap.get(x) { *x = c.clone(); } } };
        for s in scenes.iter_mut() {
            rewrite(&mut s.referenced_npc_ids);
            rewrite(&mut s.referenced_clue_ids);
            rewrite(&mut s.referenced_location_ids);
            rewrite(&mut s.referenced_encounter_ids);
        }
    }
    merged
}
```

- [ ] **Step 4: 跑测试**

Run: `cargo test -p trpg-rule-agent -- module_graph_edges sanitize dedup 2>&1 | tail -8`
Expected: 全过(bridge 2 + sanitize + dedup)。

---

## Task 5: Pass A gleaning 回环

**Files:** Modify `crates/trpg-rule-agent/src/reader/module_reader_loop.rs`(纯解析 helper + TDD)+ `module_reader.rs`(Pass A 后接入循环)

- [ ] **Step 1: 写失败测试**(gleaning 解析合并 helper,纯,放 module_reader_loop.rs tests)

```rust
    #[test]
    fn merge_gleaned_dedups_by_node_id() {
        let mut have = vec![serde_json::json!({"node_id":"s1","title":"A"})];
        let glean = serde_json::json!({"still_missing":"YES",
            "scenes":[{"node_id":"s1","title":"dup"},{"node_id":"s2","title":"B"}]});
        let still = merge_gleaned(&mut have, &glean);
        assert!(still, "still_missing YES");
        assert_eq!(have.len(), 2, "s1 去重、s2 新增");
        assert_eq!(have[1]["node_id"], "s2");
    }
```

- [ ] **Step 2: 跑确认失败 → 实现 `merge_gleaned`**

```rust
/// 合并 gleaning 补抽结果到已抽 scene 列表(按 node_id 去重),返回是否还需继续(still_missing==YES)。
pub fn merge_gleaned(have: &mut Vec<serde_json::Value>, glean: &serde_json::Value) -> bool {
    if let Some(arr) = glean.get("scenes").and_then(|v| v.as_array()) {
        let seen: std::collections::HashSet<String> = have.iter()
            .filter_map(|s| s.get("node_id").and_then(|v| v.as_str()).map(str::to_string)).collect();
        for s in arr {
            if let Some(id) = s.get("node_id").and_then(|v| v.as_str()) {
                if !seen.contains(id) { have.push(s.clone()); }
            }
        }
    }
    glean.get("still_missing").and_then(|v| v.as_str()).map(|s| s.eq_ignore_ascii_case("yes")).unwrap_or(false)
}
```
Run: `cargo test -p trpg-rule-agent merge_gleaned 2>&1 | tail -6` → 1 passed。

- [ ] **Step 3: Pass A 接入 gleaning(module_reader.rs,run_module_reader Pass A 解析 scenes 之后)**

骨架 submit 解析出 `scenes` array 后、`stub_to_node` 之前,加最多 `max_gleanings`(默认 2)轮:每轮用一个 GLEAN_SYS prompt + 「已抽 scene 列表(node_id+title) + TOC(tools::toc)」让 reader 补漏并返回 `{still_missing, scenes:[新增 stub]}`,`merge_gleaned` 合并;`still_missing!=YES` 或达上限停。GLEAN_SYS 常量(零硬编码):
```rust
const GLEAN_SYS: &str = "你在做骨架补漏。下面给你『已抽出的场景列表』和『模组目录(TOC)』。\
对照目录,找出目录里出现、但已抽列表里缺失的可玩单元/场景/章节。只补缺失的,绝不重复已有的。\
输出 JSON {\"scenes\":[{node_id,title,kind,page_start,page_end,...}], \"still_missing\":\"YES|NO\"};\
没有遗漏就 scenes 空 + still_missing NO。绝不编造目录里没有的场景。";
```
用 `llm.complete_json` 或复用 run loop(以现有 Pass A 调用方式为准)。**fail-closed**:LLM 失败/解析失败→停用已得;循环必带 max 上限。

- [ ] **Step 4: 编译**

Run: `cargo build -p trpg-rule-agent 2>&1 | tail -8`
Expected: 通过。

---

## Task 6: Pass C LLM 线索边 + 双源合并

**Files:** Modify `crates/trpg-rule-agent/src/reader/module_reader_loop.rs`(finalize_edges 纯 helper + TDD)+ `module_reader.rs`(Pass C)

- [ ] **Step 1: 写失败测试**(线索边 finalize:无 anchor/to 不存在→丢)

```rust
    #[test]
    fn finalize_clue_edges_drops_anchorless_and_unknown_to() {
        let ids: std::collections::HashSet<String> = ["s1","s2"].iter().map(|s| s.to_string()).collect();
        let raw = serde_json::json!({"edges":[
            {"from":"s1","to":"s2","link_type":"trigger","reason":"门","source_anchor":"穿过铁门"},
            {"from":"s1","to":"s2","link_type":"trigger","reason":"无锚","source_anchor":""},
            {"from":"s1","to":"ghost","link_type":"trigger","reason":"幽灵","source_anchor":"x"}
        ]});
        let edges = finalize_clue_edges(&raw, &ids);
        assert_eq!(edges.len(), 1, "无 anchor 丢、to 不存在丢,只留 1 条");
        assert_eq!(edges[0].0, "s1"); assert_eq!(edges[0].1.to_node_id, "s2");
        assert_eq!(edges[0].1.source_anchor.as_deref(), Some("穿过铁门"));
    }
```

- [ ] **Step 2: 跑确认失败 → 实现 `finalize_clue_edges`**

```rust
/// 把 Pass C 提交的 edges 解析成 (from_id, ScenarioLink)。fail-closed:source_anchor 空 → 丢;
/// from/to 不在已知节点 id 集 → 丢。link_type 解析失败 → 默认 Trigger。绝不编造。
pub fn finalize_clue_edges(raw: &serde_json::Value, ids: &std::collections::HashSet<String>)
    -> Vec<(String, trpg_model::ScenarioLink)> {
    use trpg_model::{LinkType, ScenarioLink};
    let mut out = Vec::new();
    let Some(arr) = raw.get("edges").and_then(|v| v.as_array()) else { return out };
    for e in arr {
        let from = e.get("from").and_then(|v| v.as_str()).unwrap_or("");
        let to = e.get("to").and_then(|v| v.as_str()).unwrap_or("");
        let anchor = e.get("source_anchor").and_then(|v| v.as_str()).map(str::trim).unwrap_or("");
        if from.is_empty() || to.is_empty() || anchor.is_empty() { continue; }
        if !ids.contains(from) || !ids.contains(to) { continue; }
        let lt = e.get("link_type").and_then(|v| serde_json::from_value::<LinkType>(v.clone()).ok())
            .unwrap_or(LinkType::Trigger);
        out.push((from.to_string(), ScenarioLink {
            to_node_id: to.to_string(),
            reason: e.get("reason").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            clue_id: None, link_type: lt, source_anchor: Some(anchor.to_string()),
        }));
    }
    out
}
```
Run: `cargo test -p trpg-rule-agent finalize_clue_edges 2>&1 | tail -6` → 1 passed。

- [ ] **Step 3: Pass C 接入(module_reader.rs,骨架+gleaning+dedup 之后)**

新 CLUE_EDGE_SYS prompt:给定全场景节点列表(id+title+summary),让 reader 找「A 内指向 B 的可发现线索/通路(门/NPC 提示/物证/地图出口)」,submit `{edges:[{from,to,link_type,reason,source_anchor}]}`。`tools::submit_tool("submit_edges", ...)` + 私有 loop(同 Pass A/B 套路)。提交后 `finalize_clue_edges(raw, &ids)` → 把每条 link push 进 `scenes[from].links`(按 to_node_id 去重,LLM 边优先覆盖桥接边的同目标边)。fail-closed:Pass C 失败→只保留桥接边。
```rust
const CLUE_EDGE_SYS: &str = "你在为已抽出的场景之间连『线索边』。给定全部场景(node_id|title|summary)。\
找出每个场景内**指向另一个场景的可发现线索/通路**:门、出口、NPC 的指引、物证、地图连接、剧情触发。\
每条边:from/to 必须是给定列表里的 node_id;link_type ∈ spatial|trigger|timeline|sequential|branch;\
**source_anchor 必须摘录原文片段证明这条连接存在**——抽不出原文就不要输出这条边。绝不编造。\
输出 JSON {\"edges\":[{from,to,link_type,reason,source_anchor}]}。";
```

- [ ] **Step 4: 编译**

Run: `cargo build -p trpg-rule-agent 2>&1 | tail -8`
Expected: 通过。

---

## Task 7: run_module_reader 编排 + validator 质量门

**Files:** Modify `crates/trpg-rule-agent/src/reader/module_reader.rs`

- [ ] **Step 1: 编排接入**(在 Pass A+gleaning 解析出 `out.scenes` + 实体 vec 之后、Pass B 之前)

顺序:
```rust
// P3 去重(字符串归一,重写 referenced_ids)
module_graph_edges::dedup_entities_by_key(&mut out.npcs, &mut out.scenes);
module_graph_edges::dedup_entities_by_key(&mut out.clues, &mut out.scenes);
module_graph_edges::dedup_entities_by_key(&mut out.locations, &mut out.scenes);
// ① 实体桥接边
module_graph_edges::apply_bridge_edges(&mut out.scenes);
// 进阶 Pass C LLM 线索边(失败只保桥接边)
// ... run Pass C → finalize_clue_edges → 合并 links（见 Task 6 Step 3）
// ③ validator 质量门
let entry = out.scenes.iter().find(|s| ...spine.entry_node_id...).map(|s| s.node_id.clone())
    .or_else(|| out.scenes.first().map(|s| s.node_id.clone())).unwrap_or_default();
let health = module_graph_validator::validate_graph(&out.scenes, &entry);
tracing::info!(target:"module_reader", score=health.score, reachable=health.reachable,
    orphans=health.orphans.len(), ok=health.ok, "graph health");
if !health.ok && /* 未重试过 */ { /* 触发一次额外 gleaning 重抽 Pass A,bounded 一次 */ }
```
entry 取法复用 spine `entry_node_id`(`skeleton.get("entry_node_id")`),无则 scenes[0]。重抽只一次(布尔 guard)。

- [ ] **Step 2: 编译 + 全 reader 测试**

Run: `cargo test -p trpg-rule-agent --lib 2>&1 | tail -8 && cargo build 2>&1 | tail -6`
Expected: 所有纯函数测试 + 既有 module_reader 测试全过;全工作区编译通过。

- [ ] **Step 3: 文件行数检查**

Run: `wc -l crates/trpg-rule-agent/src/reader/module_reader.rs crates/trpg-rule-agent/src/reader/module_reader_loop.rs crates/trpg-rule-agent/src/reader/module_graph_edges.rs crates/trpg-rule-agent/src/reader/module_graph_validator.rs`
Expected: 各 ≤400;若 module_reader.rs 超,把 Pass C/gleaning 的 loop 体移进 module_reader_loop.rs。

---

## Task 8: e2e — 重抽 CoC/Vault 验图谱

**Files:** 无(运行 + docker exec/proto 验证)

- [ ] **Step 1: proto 重抽 CoC + dump**

```bash
cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
cargo build -q -p trpg-rule-agent --bin module_reader_proto 2>&1 | tail -3
set -a; . ./.env; set +a; B=/Users/haoli/leehow/code/chatrpgv2/_rstest_coc
TRPG_LLM_MODEL=gpt-5.4-mini TRPG_PROTO_DUMP=/tmp/coc_graph2.json ./target/debug/module_reader_proto \
  "$B/parsed/source_units/document.semantic_units.jsonl" "$B/markdown/modules/document.md" 8 call_of_cthulhu_7e 2>&1 | grep -E "MINIMAL|scenes=|graph health|dumped"
```
Expected: 日志含 `graph health score=...`;dump 成功。

- [ ] **Step 2: 验 links 非空 + source_anchor + 连通性**

```bash
python3 - <<'PY'
import json
g=json.load(open('/tmp/coc_graph2.json')); sc=g['scenes']
edges=sum(len(s.get('links',[])) for s in sc)
anchored=sum(1 for s in sc for l in s.get('links',[]) if l.get('source_anchor'))
print(f"场景{len(sc)} 边{edges}(其中带source_anchor的LLM边{anchored})")
PY
```
Expected: 边数 >0(修前 0);桥接边 + 部分带 anchor 的 LLM 边。

- [ ] **Step 3: Vault 同样重抽验证**(换 _rstest_triangle 路径 + ruleset triangle_agency)。
Expected: 边非空、validator 报告连通性。

- [ ] **Step 4: 通用性 grep(零硬编码)**

```bash
grep -niE "cthulhu|triangle|血色|阿巴托尔|fount" crates/trpg-rule-agent/src/reader/module_graph_edges.rs crates/trpg-rule-agent/src/reader/module_graph_validator.rs
```
Expected: 无命中。

- [ ] **Step 5: 记录 e2e 结果**(中文,追加进总报告 `docs/模组reader重设计与场景导航_2026-06-09.md` 或新报告)。

---

## Self-Review（已对照 spec）

- **Spec 覆盖**:§5.1 gleaning→T5;§5.2 dedup→T4;§5.3 桥接边→T2 + LLM线索边→T6;§5.4 validator→T3;§5.5 数据模型→T1;§5.6 边角色(scene_node_to_blocks 已投影 links,自动受益,无需改)+ 编排→T7;§8 测试→各 TDD + T8 e2e。✅
- **占位符**:纯函数(bridge_edges/apply_bridge_edges/validate_graph/dedup_entities_by_key/sanitize_key/merge_gleaned/finalize_clue_edges)全给完整代码+测试;集成步(T5/T6/T7)给 prompt 常量 + 接入位置 + 顺序 + fail-closed,"以现有 Pass A/B 调用方式为准"是精确指引非真空。
- **类型一致**:`ScenarioLink.source_anchor`、`bridge_edges`/`apply_bridge_edges`/`validate_graph`/`GraphHealth`/`dedup_entities_by_key`/`sanitize_key`/`merge_gleaned`/`finalize_clue_edges` 跨 Task 命名一致;复用 `ScenarioNode`/`ScenarioLink`/`LinkType`/`tools::submit_tool` 与既有签名一致。

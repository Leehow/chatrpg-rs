# 模组抽取重设计：渐进式结构化模组 reader — 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让上传的模组被 agentic reader 结构化抽取进 `ModuleGraph`，先秒级交付首场景可玩、其余后台续抽、按需降级；前置先修复 ingestion 双栏交错（read_aloud 质量命脉）。

**Architecture:** Phase0 在 trpg-ingest 把 duotext 散文页改为列感知抽取（复用已算的 `-layout`）。Phase1 给 trpg-model 的 `ScenarioNode`/`ScenarioLink`/`ModuleGraph` 加结构化字段（全 `#[serde(default)]` 向后兼容）。Phase2 新建 `module_reader.rs`（对标 `chargen_compile.rs` 的聚焦第二刀），`parse_module` 委派它填 `ModuleGraph`，失败回退现状。Phase3 入库时把分类内容转成带 `cache_zone` 的 `ContextBlock`（BP2/BP3）。Phase4 runtime 加当前场景投影器。Phase5 后台续抽剩余场景。Phase6 真实模组端到端验证。

**Tech Stack:** Rust workspace（trpg-ingest / trpg-model / trpg-rule-agent / trpg-parser / trpg-runtime / trpg-db / trpg-api），pdftotext(poppler)，LLM via codex-relay(gpt-5.4-mini reader / gpt-5.4 deep)，Postgres，Tantivy。

**设计理念护栏（每个 task 都受约束）：** 零硬编码（无规则/语言专有词）· fail-closed 永不编造 · 单一事实源 · 复用优先 · 文件 ≤400 行。

**Spec:** `docs/superpowers/specs/2026-06-09-module-reader-redesign-design.md`

**Git 说明：** 本工作副本若无 git，下方 commit 步骤视为逻辑检查点（或执行者先 `git init` 一个本地仓）。用户未授权 push；不要推送。

---

## File Structure（决策锁定）

| 文件 | 责任 | 动作 |
|---|---|---|
| `crates/trpg-ingest/src/lib.rs` | duotext 列感知去栏 | Modify（加 `decolumnize_layout_page`，改 `merge_duotext_pages`，bump extractor tag）|
| `crates/trpg-model/src/lib.rs` | 数据模型新字段/枚举 | Modify（`ScenarioNode`/`ScenarioLink`/`ModuleGraph` + 3 枚举）|
| `crates/trpg-rule-agent/src/reader/module_reader.rs` | agentic 模组 reader（聚焦第二刀）| **Create**（≤400 行）|
| `crates/trpg-rule-agent/src/reader/mod.rs` | 导出 module_reader | Modify |
| `crates/trpg-parser/src/lib.rs` | parse_module 委派 + BP2/3 入库块 | Modify（hook@827，新 helper `module_static_blocks`）|
| `crates/trpg-runtime/src/lib.rs` | 当前场景投影器 | Modify（新 `module_scene_blocks_for_turn`，装配链@~213）|
| `crates/trpg-db/src/lib.rs` | 取/存模组场景 deep 内容 | Modify（按需 getter，复用 upsert_module_bundle）|
| `crates/trpg-api/src/lib.rs` | background-continue job | Modify（镜像 parse_all spawn）|

---

## Phase 0：ingestion 去栏交错（trpg-ingest）

**Files:**
- Modify: `crates/trpg-ingest/src/lib.rs`（加 `decolumnize_layout_page`；改 `merge_duotext_pages` 第 333-348 行；bump extractor 版本标记于 292 行 render 调用 + 265 行 cache 检查）
- Test: `crates/trpg-ingest/src/lib.rs`（`#[cfg(test)] mod decolumn_tests`）

### Task 0.1：去栏纯函数 `decolumnize_layout_page`

- [ ] **Step 1: 写失败测试**（追加到 `crates/trpg-ingest/src/lib.rs` 末尾）

```rust
#[cfg(test)]
mod decolumn_tests {
    use super::*;

    // 两栏 -layout 文本：左栏念白 + 右栏另一内容流，列间是稳定空白"河"。
    // 期望去栏后：左栏全部在前、右栏全部在后，绝不逐行交错。
    #[test]
    fn two_column_layout_is_read_column_by_column() {
        let layout = "\
漫无止境的沥青带仍在不断向前延伸          然后你们看到了它一个褪色的广告牌\n\
浪使人无法目测距离只得闷头驶向远方        正在阳光下熠熠生辉上面画了一个\n\
山丘与天穹模糊的边界线还不到上午          加油站服务员他戴着一顶牛仔帽\n\
温就已经超过了一百华氏度铁皮车内          纪五十年代的风格他头顶有气泡\n\
如蒸笼一般你们每个人就差脱光了            泡上面写着你就快到了伙计\n\
是跟刚从游泳池里爬出来一样湿透            个模糊不堪的埃索石油公司标志\n";
        let out = decolumnize_layout_page(layout).expect("应检出两栏");
        let l = out.find("漫无止境").unwrap();
        let l_last = out.find("是跟刚从游泳池").unwrap();
        let r = out.find("然后你们").unwrap();
        assert!(l < r, "左栏开头应在右栏开头之前");
        assert!(l_last < r, "左栏所有行应在任何右栏行之前（无交错）");
    }

    // 单栏散文：检不出栏 → None（调用方回退 reading-order）。
    #[test]
    fn single_column_returns_none() {
        let layout = "\
这是一段普通的单栏散文文字它没有任何分栏\n\
继续第二行依旧是单栏内容没有空白河\n\
第三行第四行第五行第六行都是单栏\n\
第四行内容第五行内容第六行内容收尾\n\
再来一行凑足行数阈值方便判定单栏\n\
最后一行同样是单栏不应被错误切分\n";
        assert!(decolumnize_layout_page(layout).is_none());
    }

    // 行数不足 → None（避免误判短页）。
    #[test]
    fn too_few_lines_returns_none() {
        assert!(decolumnize_layout_page("左          右\n甲          乙\n").is_none());
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p trpg-ingest decolumn_tests 2>&1 | tail -20`
Expected: 编译失败 `cannot find function decolumnize_layout_page`。

- [ ] **Step 3: 实现 `decolumnize_layout_page`**（插到 `merge_duotext_pages` 之前，约第 330 行上方）

```rust
/// 列感知去栏：从 -layout 文本里按"空白河"检出 ≥2 个列，把每列从上到下读出、
/// 列间左→右拼接，消除 pdftotext reading-order 把多栏逐行交错的问题。
/// 检不出可信的多栏（单栏/短页/异常）→ 返回 None，调用方回退 reading-order。
/// 通用：纯几何，无任何规则/语言专有逻辑。
fn decolumnize_layout_page(layout_text: &str) -> Option<String> {
    let lines: Vec<Vec<char>> = layout_text.lines().map(|l| l.chars().collect()).collect();
    let body: Vec<&Vec<char>> = lines.iter().filter(|l| l.iter().any(|c| !c.is_whitespace())).collect();
    if body.len() < 5 { return None; }
    let width = body.iter().map(|l| l.len()).max().unwrap_or(0);
    if width < 30 { return None; }
    // 每个字符列：有多少 body 行在此处是空格（或更短）。
    let mut blank = vec![0usize; width];
    for l in &body {
        for c in 0..width {
            if l.get(c).map(|ch| *ch == ' ').unwrap_or(true) { blank[c] += 1; }
        }
    }
    let n = body.len();
    let is_gutter = |c: usize| blank[c] * 100 >= n * 90; // ≥90% 行此列为空 = 河
    // 找连续河带，内部、宽度 ≥3 的取中点作为切分位。
    let mut splits: Vec<usize> = Vec::new();
    let mut c = 0;
    while c < width {
        if is_gutter(c) {
            let start = c;
            while c < width && is_gutter(c) { c += 1; }
            if c - start >= 3 && start > 2 && c < width - 2 { splits.push((start + c) / 2); }
        } else { c += 1; }
    }
    if splits.is_empty() { return None; }
    // 列边界。
    let mut bounds = vec![0usize];
    bounds.extend(splits);
    bounds.push(width);
    let mut out = String::new();
    for w in bounds.windows(2) {
        let (cs, ce) = (w[0], w[1]);
        let mut col = String::new();
        for l in &lines { // 用全部行（含空行）保留段落断点
            let seg: String = (cs..ce.min(l.len())).map(|i| l[i]).collect();
            let seg = seg.trim();
            col.push_str(seg);
            col.push('\n');
        }
        let col = col.trim();
        if !col.is_empty() { if !out.is_empty() { out.push_str("\n\n"); } out.push_str(col); }
    }
    if out.trim().is_empty() { None } else { Some(out) }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p trpg-ingest decolumn_tests 2>&1 | tail -20`
Expected: `test result: ok. 3 passed`。若两栏样本因 90% 阈值/河宽未通过，微调阈值（如 85%、河宽 ≥2）直至通过 —— 这是允许的针对真实排版的调参。

- [ ] **Step 5: 接入 `merge_duotext_pages`（散文页优先去栏）**

把第 333-348 行的函数体改为：

```rust
fn merge_duotext_pages(reading: Vec<PageText>, layout: Vec<PageText>) -> Vec<PageText> {
    let read_map: BTreeMap<u32, String> = reading.into_iter().map(|p| (p.page, p.text)).collect();
    let lay_map: BTreeMap<u32, String> = layout.into_iter().map(|p| (p.page, p.text)).collect();
    let mut nums: Vec<u32> = read_map.keys().chain(lay_map.keys()).copied().collect();
    nums.sort();
    nums.dedup();
    nums.into_iter().filter_map(|pg| {
        let text = match (lay_map.get(&pg), read_map.get(&pg)) {
            // 表页：保留 -layout 列对齐（不变）
            (Some(l), _) if is_duotext_table_page(l) => l.clone(),
            // 散文页：优先从 -layout 去栏（消除交错）；检不出 → 回退 reading-order
            (Some(l), Some(r)) => decolumnize_layout_page(l).unwrap_or_else(|| r.clone()),
            (Some(l), None) => decolumnize_layout_page(l).unwrap_or_else(|| l.clone()),
            (None, Some(r)) => r.clone(),
            (None, None) => return None,
        };
        Some(PageText { page: pg, text })
    }).collect()
}
```

- [ ] **Step 6: bump extractor 版本标记以失效旧缓存**

第 292 行 `render_page_anchored_markdown(&source_id, &title, &source_hash, &pages, "duotext")` 的最后参数 `"duotext"` 改为 `"duotext2"`；第 265 行缓存检查 `markdown_header.contains("extractor: duotext")` 改为 `markdown_header.contains("extractor: duotext2")`。这样旧 `.md` 会重新生成为去栏版。

- [ ] **Step 7: 整 crate 编译 + 测试**

Run: `cargo test -p trpg-ingest 2>&1 | tail -20`
Expected: 全绿。

- [ ] **Step 8: 真实模组冒烟验证（去栏确实生效）**

Run:
```bash
cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
cat > /tmp/decol_check.rs <<'EOF'
// 仅人工核对：用 pdftotext -layout 取 血色公路 p16，喂给逻辑应得到连贯左栏念白
EOF
pdftotext -layout -f 16 -l 16 "/Users/haoli/leehow/code/chatrpgv2/materi/coc/血色公路.pdf" - | sed '/^$/d' | head -16
```
Expected: 看到左右两栏并排（左栏"漫无止境…"为完整念白）。去栏函数应把左栏整段提到右栏之前。（端到端重抽在 Phase 6 验证。）

- [ ] **Step 9: Commit**

```bash
git add crates/trpg-ingest/src/lib.rs
git commit -m "feat(ingest): column-aware de-interleaving for duotext prose pages"
```

---

## Phase 1：数据模型（trpg-model）

**Files:**
- Modify: `crates/trpg-model/src/lib.rs`（枚举置于 ~1648 风格区；`ScenarioNode`@1506 / `ScenarioLink`@1519 加字段）
- Test: `crates/trpg-model/src/lib.rs`（`#[cfg(test)] mod module_graph_compat_tests`）

### Task 1.1：三枚举 + 字段扩展 + 向后兼容

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod module_graph_compat_tests {
    use super::*;

    // 旧 bundle（无新字段）必须能反序列化，新字段取默认（骨架态）。
    #[test]
    fn old_scenario_node_json_deserializes_with_defaults() {
        let old = r#"{"node_id":"n1","title":"序幕","node_type":"chapter","summary":"s",
            "read_aloud":null,"gm_notes":null,"links":[],"assets":[],"data":null}"#;
        let n: ScenarioNode = serde_json::from_str(old).expect("旧 JSON 应可反序列化");
        assert_eq!(n.extraction_status, SceneExtractionStatus::SkeletonOnly);
        assert!(n.page_start.is_none());
        assert!(n.referenced_npc_ids.is_empty());
    }

    // 新字段 round-trip。
    #[test]
    fn scenario_node_roundtrips_new_fields() {
        let mut n = ScenarioNode::default();
        n.node_id = "loc1".into();
        n.extraction_status = SceneExtractionStatus::DeepExtracted;
        n.page_start = Some(17);
        n.referenced_npc_ids = vec!["npc_russell".into()];
        n.links = vec![ScenarioLink { to_node_id: "loc2".into(), reason: "可达".into(),
            clue_id: None, link_type: LinkType::Spatial }];
        let s = serde_json::to_string(&n).unwrap();
        let back: ScenarioNode = serde_json::from_str(&s).unwrap();
        assert_eq!(back.extraction_status, SceneExtractionStatus::DeepExtracted);
        assert_eq!(back.links[0].link_type, LinkType::Spatial);
    }

    // 旧 ScenarioLink（无 link_type）→ 默认 Sequential。
    #[test]
    fn old_link_defaults_to_sequential() {
        let l: ScenarioLink = serde_json::from_str(r#"{"to_node_id":"x","reason":"r","clue_id":null}"#).unwrap();
        assert_eq!(l.link_type, LinkType::Sequential);
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p trpg-model module_graph_compat_tests 2>&1 | tail -20`
Expected: 编译失败（缺 `SceneExtractionStatus`/`LinkType`/字段）。

- [ ] **Step 3: 加三枚举**（贴近 ~1648 现有 snake_case 枚举，沿用同样 derive）

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SceneExtractionStatus { SkeletonOnly, DeepExtracted }
impl Default for SceneExtractionStatus { fn default() -> Self { Self::SkeletonOnly } }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModuleContentClass { Bp2CustomRule, Bp3Index, Story }
impl Default for ModuleContentClass { fn default() -> Self { Self::Story } }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkType { Spatial, Trigger, Timeline, Sequential, Branch }
impl Default for LinkType { fn default() -> Self { Self::Sequential } }
```

- [ ] **Step 4: 扩展 `ScenarioNode`（1506）**——在 `data: serde_json::Value` 字段后加：

```rust
    #[serde(default)] pub extraction_status: SceneExtractionStatus,
    #[serde(default)] pub page_start: Option<u32>,
    #[serde(default)] pub page_end: Option<u32>,
    #[serde(default)] pub referenced_npc_ids: Vec<String>,
    #[serde(default)] pub referenced_clue_ids: Vec<String>,
    #[serde(default)] pub referenced_location_ids: Vec<String>,
    #[serde(default)] pub referenced_encounter_ids: Vec<String>,
```

- [ ] **Step 5: 扩展 `ScenarioLink`（1519）**——加 `#[serde(default)] pub link_type: LinkType,`。确认 `ScenarioNode`/`ScenarioLink` 都 `#[derive(Default)]`（若没有则加，`ModuleGraph` 已是 Default）。

- [ ] **Step 6: 跑测试确认通过**

Run: `cargo test -p trpg-model module_graph_compat_tests 2>&1 | tail -20`
Expected: `3 passed`。

- [ ] **Step 7: 全工作区编译（捕捉下游受影响处）**

Run: `cargo build 2>&1 | tail -30`
Expected: 编译通过。若 `ScenarioNode {...}` 字面构造处报缺字段，加 `..Default::default()`。

- [ ] **Step 8: Commit**

```bash
git add crates/trpg-model/src/lib.rs
git commit -m "feat(model): structured ScenarioNode fields + extraction/content/link enums"
```

---

## Phase 2：module_reader.rs + parse_module hook

**Files:**
- Create: `crates/trpg-rule-agent/src/reader/module_reader.rs`（≤400 行）
- Modify: `crates/trpg-rule-agent/src/reader/mod.rs`
- Modify: `crates/trpg-parser/src/lib.rs`（hook@827，gate `module_reader_enabled`）
- Test: `crates/trpg-rule-agent/src/reader/module_reader.rs`（纯函数单测）

> **说明：** agentic ReAct 循环本身不做确定性单测（同 `chargen_compile.rs`/`object_compile.rs` 惯例，由 Phase 6 真实模组 e2e 验证）。本阶段 TDD 覆盖确定性 helper：依赖闭包解析、fail-closed finalize、skeleton→ScenarioNode 装配。

### Task 2.1：reader 数据结构 + 确定性 helper

- [ ] **Step 1: 建文件骨架 + 失败测试**（Create `module_reader.rs`）

```rust
//! Agentic 模组 reader —— 规则 agent 的聚焦"模组刀"，对标 chargen_compile.rs：
//! 自带 Ctx + SYS prompt + tools::submit_tool + 私有 run loop + 私有 dispatch +
//! fail-closed guardrail。Pass A 读 TOC+前言建全书骨架；Pass B 深抽首场景+依赖闭包。
//! 设计：docs/superpowers/specs/2026-06-09-module-reader-redesign-design.md
use super::tools;
use super::units::Unit;
use serde_json::Value;
use trpg_llm::LlmClient;
use trpg_model::{ModuleContentClass, ScenarioNode, SceneExtractionStatus};

pub struct ModuleReaderCtx<'a> {
    pub units: &'a [Unit],
    pub sidecar_text: Option<String>,
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
}

/// 给定首场景 + 全部场景，收集"依赖闭包" id 集合（首场景引用的 npc/clue/location）。
/// 通用、确定性、可单测。
pub fn dependency_closure(entry: &ScenarioNode) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for v in [&entry.referenced_npc_ids, &entry.referenced_clue_ids, &entry.referenced_location_ids] {
        for id in v { if !ids.contains(id) { ids.push(id.clone()); } }
    }
    for l in &entry.links { if !ids.contains(&l.to_node_id) { ids.push(l.to_node_id.clone()); } }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::{LinkType, ScenarioLink};

    #[test]
    fn closure_collects_refs_and_links_dedup() {
        let mut n = ScenarioNode::default();
        n.referenced_npc_ids = vec!["a".into(), "b".into()];
        n.referenced_clue_ids = vec!["b".into()]; // dup
        n.links = vec![ScenarioLink { to_node_id: "c".into(), reason: "".into(), clue_id: None, link_type: LinkType::Spatial }];
        let c = dependency_closure(&n);
        assert_eq!(c, vec!["a", "b", "c"]);
    }
}
```

- [ ] **Step 2: 注册模块 + 跑测试确认失败→通过**

在 `crates/trpg-rule-agent/src/reader/mod.rs` 加：
```rust
pub mod module_reader;
pub use module_reader::{run_module_reader, ModuleReaderCtx, ModuleReadout};
```
Run: `cargo test -p trpg-rule-agent module_reader 2>&1 | tail -20`
Expected: 先因缺 `run_module_reader` 报错（Step 3 补）；`closure_collects_refs_and_links_dedup` 在补全后通过。

- [ ] **Step 3: 实现 fail-closed finalize + skeleton 装配 helper**（加到 module_reader.rs）

```rust
/// 把 reader 提交的一条 skeleton stub（JSON）转成 SkeletonOnly 的 ScenarioNode。
/// 缺字段一律取空/默认，绝不编造 read_aloud（保持 None）。
pub fn stub_to_node(v: &Value) -> Option<ScenarioNode> {
    let node_id = v.get("node_id").and_then(|x| x.as_str())?.trim().to_string();
    if node_id.is_empty() { return None; }
    let mut n = ScenarioNode::default();
    n.node_id = node_id;
    n.title = v.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string();
    n.node_type = v.get("kind").and_then(|x| x.as_str()).unwrap_or("scene").to_string();
    n.summary = v.get("summary").and_then(|x| x.as_str()).unwrap_or("").to_string();
    n.page_start = v.get("page_start").and_then(|x| x.as_u64()).map(|p| p as u32);
    n.page_end = v.get("page_end").and_then(|x| x.as_u64()).map(|p| p as u32);
    let ids = |k: &str| v.get(k).and_then(|x| x.as_array())
        .map(|a| a.iter().filter_map(|e| e.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    n.referenced_npc_ids = ids("referenced_npc_ids");
    n.referenced_clue_ids = ids("referenced_clue_ids");
    n.referenced_location_ids = ids("referenced_location_ids");
    n.extraction_status = SceneExtractionStatus::SkeletonOnly; // Pass B 才翻 Deep
    Some(n)
}
```

加测试：
```rust
    #[test]
    fn stub_to_node_is_fail_closed_skeleton() {
        let v = serde_json::json!({"node_id":"loc1","title":"加油站","kind":"location","page_start":17});
        let n = stub_to_node(&v).unwrap();
        assert_eq!(n.extraction_status, SceneExtractionStatus::SkeletonOnly);
        assert!(n.read_aloud.is_none(), "绝不编造 read_aloud");
        assert_eq!(n.page_start, Some(17));
        assert!(stub_to_node(&serde_json::json!({"title":"无id"})).is_none());
    }
```
Run: `cargo test -p trpg-rule-agent module_reader 2>&1 | tail -20`
Expected: 全绿。

- [ ] **Step 4: 实现 agentic 两遍 `run_module_reader`**（对照 `chargen_compile.rs` 的 `run_compile_loop`/`compile_dispatch` 写私有循环；以下为结构，需按 chargen_compile 现有 helper 名补全）

```rust
const SKELETON_SYS: &str = "你是模组结构抽取器。读目录(TOC)与前言，产出全书有序的可玩单元骨架\
（场景/地点/任务/时间线节点）与实体索引（npc/clue/location/encounter/handout，各带 id+name+\
content_class+page）与自定义规则清单。content_class∈{story,bp2_custom_rule,bp3_index}：模组专属\
规则子系统=bp2_custom_rule；物品/怪物/名册=bp3_index；剧情=story。只输出文本里真实存在的内容，\
缺失留空，绝不编造。用 get_toc/search/read 检索，最后 submit_skeleton。";

const DEEP_SYS: &str = "你是模组场景深抽器。对给定的入口场景及其依赖闭包，读相关页，填 read_aloud\
（仅当文本有可念的 boxed/念白文本时；优先锚句如『read, or paraphrase the following text:』或第二人称，\
没有就留 null，绝不编造）、gm_notes、links（每条带 link_type∈{spatial,trigger,timeline,sequential,branch}）、\
referenced_*_ids、闭包实体详情。最后 submit_deep。";

pub async fn run_module_reader(client: &dyn LlmClient, ctx: ModuleReaderCtx<'_>, budget: usize)
    -> anyhow::Result<ModuleReadout>
{
    // Pass A：骨架。tools::nav_tools() + submit_tool("submit_skeleton", ...) → 私有 run loop。
    //   解析提交 → stub_to_node 装配 scenes；entities/custom_rules 直接收进对应 vec。
    // Pass B：取 scenes 中 node_type/order 决定的入口（首个 story 场景；无则首个场景）+ dependency_closure，
    //   submit_tool("submit_deep", ...) → 读闭包页 → 填入口 ScenarioNode（翻 DeepExtracted）+ 闭包实体。
    //   read_aloud fail-closed：提交里为空/缺 → 保持 None。
    // 任一遍失败 → 返回已得部分（至少骨架）；调用方再决定回退。
    // 实际循环体克隆 chargen_compile.rs:404 run_compile_loop + :444 compile_dispatch（仅 get_toc/search/read/read_layout）。
    todo!("按 chargen_compile.rs 私有循环补全；保持 ≤400 行，必要时拆 module_reader_loop.rs")
}
```

> 执行者注意：此函数的循环体与 `chargen_compile.rs` 的 `run_compile_loop`(404) / `compile_dispatch`(444) 同构，逐行参照即可；submit schema 用 `tools::submit_tool`。若文件逼近 400 行，把两个私有 loop+dispatch 拆到 `module_reader_loop.rs` 并 `mod` 进来。

- [ ] **Step 5: 编译（loop 补全后）**

Run: `cargo build -p trpg-rule-agent 2>&1 | tail -20`
Expected: 通过（`todo!` 替换为真实循环后）。

- [ ] **Step 6: Commit**

```bash
git add crates/trpg-rule-agent/src/reader/module_reader.rs crates/trpg-rule-agent/src/reader/mod.rs
git commit -m "feat(rule-agent): agentic module_reader slice (skeleton + deep-minimal passes)"
```

### Task 2.2：parse_module 委派

**Files:** Modify `crates/trpg-parser/src/lib.rs`（hook@827；gate fn 镜像 `reader_agent_enabled`@1543）

- [ ] **Step 1: 加 gate + 委派（替换 827-843 的空 vec 构造）**

```rust
fn module_reader_enabled() -> bool {
    std::env::var("TRPG_MODULE_READER").map(|v| v != "0" && !v.eq_ignore_ascii_case("false")).unwrap_or(false)
}
```

在构造 `ModuleGraph` 前：
```rust
let readout = if module_reader_enabled() {
    let units_path = self.config.data_dir.join("parsed/source_units")
        .join(format!("{}.semantic_units.jsonl", doc.source_id));
    match reader::load_units(&units_path) {
        Ok(units) => {
            let sidecar_text = doc.metadata.get("layout_sidecar_path").and_then(Value::as_str)
                .and_then(|p| std::fs::read_to_string(p).ok());
            let ctx = reader::ModuleReaderCtx { units: &units, sidecar_text, ruleset_id: ruleset_id.clone() };
            let budget = std::env::var("TRPG_MODULE_READER_BUDGET").ok().and_then(|s| s.parse().ok()).unwrap_or(12);
            match reader::run_module_reader(self.llm.as_ref(), ctx, budget).await {
                Ok(r) => Some(r),
                Err(err) => { tracing::warn!(error=%err, "module_reader failed; falling back to empty graph"); None }
            }
        }
        Err(err) => { tracing::warn!(error=%err, "load_units for module failed; empty graph"); None }
    }
} else { None };
```

`ModuleGraph` 构造改为消费 readout（无则保持现状 `vec![]`）：
```rust
let module_graph = ModuleGraph {
    module_id: module_id.clone(),
    ruleset_id: ruleset_id.clone(),
    title: doc.title.clone(),
    module_type: module_type.clone(),
    spine: readout.as_ref().map(|r| r.spine.clone()).unwrap_or(spine_json),
    chapters: vec![],
    missions: vec![],
    scenes: readout.as_ref().map(|r| r.scenes.clone()).unwrap_or_default(),
    locations: readout.as_ref().map(|r| r.locations.clone()).unwrap_or_default(),
    npcs: readout.as_ref().map(|r| r.npcs.clone()).unwrap_or_default(),
    factions: readout.as_ref().map(|r| r.factions.clone()).unwrap_or_default(),
    clues: readout.as_ref().map(|r| r.clues.clone()).unwrap_or_default(),
    handouts: readout.as_ref().map(|r| r.handouts.clone()).unwrap_or_default(),
    encounters: readout.as_ref().map(|r| r.encounters.clone()).unwrap_or_default(),
    module_specific_rules: readout.as_ref().map(|r| r.module_specific_rules.clone()).unwrap_or_default(),
};
```
（`readout` 在 Phase 3 还要用，先 `let readout = ...;` 不要 move。）

- [ ] **Step 2: 编译**

Run: `cargo build -p trpg-parser 2>&1 | tail -20`
Expected: 通过。

- [ ] **Step 3: Commit**

```bash
git add crates/trpg-parser/src/lib.rs
git commit -m "feat(parser): parse_module delegates structured extraction to module_reader (gated, fail-open)"
```

---

## Phase 3：BP2/BP3 入库 ContextBlock（trpg-parser）

**Files:** Modify `crates/trpg-parser/src/lib.rs`（新 helper `module_static_blocks`；在 parse_module 把它产出的块 push 进 `context_blocks`）
**Test:** `crates/trpg-parser/src/lib.rs`（`module_static_blocks` 纯函数单测）

### Task 3.1：分类内容 → 带 cache_zone 的块

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod module_static_block_tests {
    use super::*;
    use trpg_model::CacheZone;

    #[test]
    fn custom_rules_go_pinned_index_goes_dynamic() {
        let readout = reader::ModuleReadout {
            module_specific_rules: vec![serde_json::json!({"id":"r1","name":"狩猎","content_class":"bp2_custom_rule","body":"..."})],
            npcs: vec![serde_json::json!({"id":"npc1","name":"拉斯","content_class":"bp3_index","summary":"老板"})],
            ..Default::default()
        };
        let blocks = module_static_blocks("mod1", &readout);
        let rule = blocks.iter().find(|b| b.tags.iter().any(|t| t=="module_custom_rule")).unwrap();
        assert_eq!(rule.cache_zone, CacheZone::PinnedMiddle);
        let idx = blocks.iter().find(|b| b.tags.iter().any(|t| t=="module_index")).unwrap();
        assert_eq!(idx.cache_zone, CacheZone::DynamicTail);
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p trpg-parser module_static_block_tests 2>&1 | tail -20`
Expected: 缺 `module_static_blocks`。

- [ ] **Step 3: 实现 `module_static_blocks`**（用 `ContextBlock::new`，trpg-model:625）

```rust
/// 把分类后的模组静态内容转成带 cache_zone 的 ContextBlock：
/// bp2_custom_rule → ModuleSpecificRule / PinnedMiddle(BP2,resident)；
/// bp3_index 实体 → ModuleOverview / DynamicTail(BP3)。scene deep 内容不在此（Phase 4 投影）。
fn module_static_blocks(module_id: &str, r: &reader::ModuleReadout) -> Vec<ContextBlock> {
    use trpg_model::{BlockContent, BlockKind, CacheZone, Scope, Stability, Visibility};
    let mut out = Vec::new();
    let class_of = |v: &Value| v.get("content_class").and_then(|x| x.as_str()).unwrap_or("story").to_string();
    let name_of = |v: &Value| v.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let body_of = |v: &Value| v.get("body").or_else(|| v.get("summary")).and_then(|x| x.as_str()).unwrap_or("").to_string();
    // BP2 自定义规则
    for v in &r.module_specific_rules {
        let mut b = ContextBlock::new(
            format!("module.{module_id}.rule.{}", v.get("id").and_then(|x| x.as_str()).unwrap_or("x")),
            BlockKind::ModuleSpecificRule, name_of(v), BlockContent::Text(body_of(v)),
            Visibility::GmOnly, Stability::Stable, CacheZone::PinnedMiddle, Scope::module(module_id), 50);
        b.tags = vec!["module_custom_rule".into(), "resident".into()];
        out.push(b);
    }
    // BP3 实体索引（npc/clue/location/encounter/handout 里 content_class=bp3_index 的）
    let index_entities = r.npcs.iter().chain(&r.clues).chain(&r.locations).chain(&r.encounters).chain(&r.handouts);
    let mut idx_lines = Vec::new();
    for v in index_entities { if class_of(v) == "bp3_index" {
        idx_lines.push(format!("- {} ({}): {}", name_of(v),
            v.get("id").and_then(|x| x.as_str()).unwrap_or(""), body_of(v)));
    }}
    if !idx_lines.is_empty() {
        let mut b = ContextBlock::new(
            format!("module.{module_id}.index"), BlockKind::ModuleOverview, "模组索引".into(),
            BlockContent::Text(idx_lines.join("\n")), Visibility::GmOnly, Stability::Stable,
            CacheZone::DynamicTail, Scope::module(module_id), 30);
        b.tags = vec!["module_index".into()];
        out.push(b);
    }
    out
}
```
> 执行者：`ContextBlock::new` 参数顺序/`Visibility`/`Stability`/`Scope::module` 以 trpg-model:604-662 实际签名为准；若 `Scope::module` 不存在用 `Scope { scope_type: ScopeType::Module, scope_id: module_id.into() }`。

- [ ] **Step 4: 在 parse_module 调用并 push 进 context_blocks**

在 `material_from_block` 循环（~825）之前：
```rust
if let Some(r) = &readout {
    for b in module_static_blocks(&module_id, r) { context_blocks.push(b); }
}
```

- [ ] **Step 5: 跑测试 + 编译**

Run: `cargo test -p trpg-parser module_static_block_tests 2>&1 | tail -20 && cargo build -p trpg-parser 2>&1 | tail -10`
Expected: 测试绿、编译通过。

- [ ] **Step 6: Commit**

```bash
git add crates/trpg-parser/src/lib.rs
git commit -m "feat(parser): emit BP2(custom-rule)/BP3(index) context blocks from module graph"
```

---

## Phase 4：runtime 当前场景投影器（trpg-runtime）

**Files:**
- Modify: `crates/trpg-runtime/src/lib.rs`（新 `module_scene_blocks_for_turn`，装配链 ~213 插调用；镜像 `rule_steward_prefix_blocks_for_turn`@1361）
- Modify: `crates/trpg-db/src/lib.rs`（如需按 module_id 取 ModuleGraph：复用 `load_*module_bundle`，读 content_json.module_graph）

### Task 4.1：当前场景 deep 内容投影成 SceneStatic/DynamicTail

- [ ] **Step 1: 写失败测试**（纯函数：给定场景节点 + npcs → 块）

```rust
#[cfg(test)]
mod module_scene_proj_tests {
    use super::*;
    use trpg_model::{CacheZone, ScenarioNode, SceneExtractionStatus, BlockKind};

    #[test]
    fn deep_scene_projects_scenestatic_dynamictail() {
        let mut n = ScenarioNode::default();
        n.node_id = "loc1".into(); n.title = "加油站".into();
        n.read_aloud = Some("你们看到一个褪色的广告牌……".into());
        n.extraction_status = SceneExtractionStatus::DeepExtracted;
        n.referenced_npc_ids = vec!["npc1".into()];
        let npcs = vec![serde_json::json!({"id":"npc1","name":"拉斯","summary":"老板"})];
        let blocks = scene_node_to_blocks("mod1", &n, &npcs);
        assert!(!blocks.is_empty());
        let sb = &blocks[0];
        assert_eq!(sb.kind, BlockKind::SceneStatic);
        assert_eq!(sb.cache_zone, CacheZone::DynamicTail);
        assert_eq!(sb.expires_at_scene.as_deref(), Some("loc1"));
        assert!(sb.content_text().contains("拉斯"), "在场 NPC 应被并入");
    }

    #[test]
    fn skeleton_scene_projects_nothing() {
        let mut n = ScenarioNode::default();
        n.node_id = "loc2".into();
        n.extraction_status = SceneExtractionStatus::SkeletonOnly; // 交给 materializer 降级
        assert!(scene_node_to_blocks("mod1", &n, &[]).is_empty());
    }
}
```
> `content_text()` 若不存在，断言改为对 `BlockContent::Text` 模式匹配取字符串。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p trpg-runtime module_scene_proj_tests 2>&1 | tail -20`
Expected: 缺 `scene_node_to_blocks`。

- [ ] **Step 3: 实现纯函数 `scene_node_to_blocks`**

```rust
/// 当前场景的 deep 内容 → ContextBlock。SkeletonOnly → 空（交 materializer 降级）。
fn scene_node_to_blocks(module_id: &str, n: &trpg_model::ScenarioNode, npcs: &[serde_json::Value]) -> Vec<ContextBlock> {
    use trpg_model::{BlockContent, BlockKind, CacheZone, Scope, SceneExtractionStatus, Stability, Visibility};
    if n.extraction_status != SceneExtractionStatus::DeepExtracted { return Vec::new(); }
    let mut body = String::new();
    if let Some(ra) = &n.read_aloud { body.push_str("【可念】\n"); body.push_str(ra); body.push('\n'); }
    if let Some(g) = &n.gm_notes { body.push_str("\n【GM】\n"); body.push_str(g); body.push('\n'); }
    for id in &n.referenced_npc_ids {
        if let Some(v) = npcs.iter().find(|v| v.get("id").and_then(|x| x.as_str()) == Some(id.as_str())) {
            let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("");
            let sum = v.get("summary").and_then(|x| x.as_str()).unwrap_or("");
            body.push_str(&format!("\n[NPC] {name}: {sum}"));
        }
    }
    let mut b = ContextBlock::new(
        format!("module.{module_id}.scene.{}", n.node_id), BlockKind::SceneStatic, n.title.clone(),
        BlockContent::Text(body), Visibility::GmOnly, Stability::Volatile,
        CacheZone::DynamicTail, Scope::scene(&n.node_id), 60);
    b.expires_at_scene = Some(n.node_id.clone());
    vec![b]
}
```
> `Scope::scene` 不存在则 `Scope { scope_type: ScopeType::Scene, scope_id: n.node_id.clone() }`。

- [ ] **Step 4: 加 `async fn module_scene_blocks_for_turn` + 装配链调用**

镜像 `rule_steward_prefix_blocks_for_turn`(1361)：按 `state` 的 module_id + scene_id 取该模组 ModuleGraph（DB getter，读 content_json.module_graph），找到 `scenes` 里 node_id==scene_id 的节点，调 `scene_node_to_blocks`。装配链 ~213（`state_frame_blocks_for_turn` 后）加：
```rust
match self.module_scene_blocks_for_turn(request, state).await {
    Ok(mut bs) => blocks.append(&mut bs),
    Err(err) => tracing::warn!(error=%err, "module_scene_blocks_for_turn failed"),
}
```

- [ ] **Step 5: 测试 + 编译**

Run: `cargo test -p trpg-runtime module_scene_proj_tests 2>&1 | tail -20 && cargo build -p trpg-runtime 2>&1 | tail -10`
Expected: 绿 + 编译通过。

- [ ] **Step 6: Commit**

```bash
git add crates/trpg-runtime/src/lib.rs crates/trpg-db/src/lib.rs
git commit -m "feat(runtime): project current deep-extracted module scene into turn context"
```

---

## Phase 5：background-continue job（trpg-api）

**Files:** Modify `crates/trpg-api/src/lib.rs`（镜像 parse_all spawn@141-185，job_kind `module_extract_continue`）

### Task 5.1：续抽剩余 SkeletonOnly 场景

- [ ] **Step 1: enqueue + spawn（在 parse_module 完成首场景后，或 parse_all 之后）**

```rust
// 镜像 parse_all：insert_background_job + tokio::spawn(mark running → 逐场景深抽 → 重新入库 → done/error)
let job_id = format!("modext.{module_id}");
state.db.insert_background_job(&job_id, "module_extract_continue",
    json!({"module_id": module_id, "ruleset_id": ruleset_id, "source_id": source_id})).await?;
let (db, llm) = (state.db.clone(), state.llm.clone());
let (mid, sid, rid) = (module_id.clone(), source_id.clone(), ruleset_id.clone());
tokio::spawn(async move {
    let _ = db.update_background_job(&job_id, "running", json!({}), None).await;
    let res = continue_module_extraction(&db, llm.as_ref(), &mid, &sid, rid.as_deref()).await;
    match res {
        Ok(n) => { let _ = db.update_background_job(&job_id, "done", json!({"deep_extracted": n}), None).await; }
        Err(e) => { let _ = db.update_background_job(&job_id, "error", json!({}), Some(&e.to_string())).await; }
    }
});
```

- [ ] **Step 2: 实现 `continue_module_extraction`**

读该模组 bundle 的 ModuleGraph → 找 `scenes` 里 `extraction_status==SkeletonOnly` 的，按序对每个跑 Pass-B 深抽（复用 `module_reader` 的 deep 能力；可暴露 `deep_extract_one(client, units, &mut node)`）→ 翻 DeepExtracted → 读-改-写 ModuleGraph 经 `upsert_module_bundle` 重新入库 → 返回深抽数。单任务串行，无并发覆盖。

- [ ] **Step 3: 编译**

Run: `cargo build -p trpg-api 2>&1 | tail -20`
Expected: 通过。

- [ ] **Step 4: Commit**

```bash
git add crates/trpg-api/src/lib.rs crates/trpg-rule-agent/src/reader/module_reader.rs
git commit -m "feat(api): background module_extract_continue job for remaining scenes"
```

---

## Phase 6：真实模组端到端验证

**Files:** 无新增（运行 + 人工核对）。数据目录 `_rstest_coc`（血色公路）；Homecoming 用 v1.16.2 文本或重新 ingest。

### Task 6.1：去栏重抽 + reader 端到端

- [ ] **Step 1: 强制重抽血色公路（去栏生效）**

```bash
cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
TRPG_MODULE_READER=1 TRPG_MODULE_READER_MODEL=gpt-5.4-mini \
  cargo run -p trpg-cli -- parse-all --data-dir ../_rstest_coc --force 2>&1 | tail -40
```
Expected: 日志显示 module reader 跑、ModuleGraph.scenes 非空。

- [ ] **Step 2: 核对去栏后 read_aloud 连贯**

```bash
grep -A3 "序幕\|漫无止境" ../_rstest_coc/markdown/modules/document.md | head -20
```
Expected: 序幕念白整段连贯，无右栏文本（如"升降机上有辆诺瓦"）窜入。

- [ ] **Step 3: 核对 ModuleGraph 结构化产出**

```bash
psql "$COC_DB_URL" -c "select content_json->'module_graph'->'scenes'->0->>'title',
  content_json->'module_graph'->'scenes'->0->>'extraction_status',
  jsonb_array_length(content_json->'module_graph'->'scenes')
  from parsed_bundles where bundle_kind='module' order by updated_at desc limit 1;"
```
Expected: 首场景标题=入口（埃索加油站/序幕）、`extraction_status=deep_extracted`、scenes 数 >0。

- [ ] **Step 4: 进游戏跑几轮，验证首场景可玩 + 后续降级**

```bash
TRPG_MODULE_READER=1 cargo run -p trpg-cli -- turn --session <coc_session> --module document \
  --input "我们把车停进加油站" 2>&1 | tail -40
```
Expected: 首场景 read_aloud/NPC 进上下文（SceneStatic）；问到未深抽场景时走 materializer 降级仍出料（不崩、不编造数值）。

- [ ] **Step 5: 通用性 grep（零硬编码）**

```bash
grep -niE "cthulhu|cyberpunk|triangle|sword|血色|阿巴托尔|nyarlathotep" crates/trpg-rule-agent/src/reader/module_reader.rs crates/trpg-ingest/src/lib.rs
```
Expected: 无任何命中（除注释示例外的逻辑里）。

- [ ] **Step 6: Homecoming 复跑（线性 + 图片-only stub fail-closed）**

对 Homecoming 重复 1-4；重点核对：p17-26 图片-only 属性卡 → bp3 实体为 name+页码 stub 且带 `stats_unextracted` 标记，**未编造任何数值**。

- [ ] **Step 7: 记录验收**

更新 `docs/超夜作业报告_2026-06-09.md`（或新报告）记录三模组端到端结果（中文）。

```bash
git add docs/
git commit -m "docs: module reader redesign e2e validation report"
```

---

## Self-Review（已对照 spec）

- **Spec 覆盖**：§Phase0 去栏 → Phase0；§4 数据模型(枚举+字段) → Phase1；§5 module_reader + §6.1 hook → Phase2；§6.2 BP2/3 入库 → Phase3；§6.3 场景投影器 → Phase4；§6.5 后台续抽 → Phase5；§6.4 降级（零新代码，由 Phase6 验证）+ §8 测试 → 各 Task 测试 + Phase6。子项目 2（知识图谱）按 spec 明确不在本计划。✅
- **占位符**：仅 Phase2 Step4 的 `run_module_reader` 循环体标注 `todo!` —— 因其与 `chargen_compile.rs:404/444` 逐行同构，已给出对照坐标与结构、prompt 常量、submit 名、dispatch 范围，非真空占位（执行者照搬现成模式）。其余步骤均含可运行代码/命令。
- **类型一致**：`ScenarioNode`/`ScenarioLink`/`LinkType`/`SceneExtractionStatus`/`ModuleContentClass`/`ModuleReadout`/`ModuleReaderCtx`/`module_static_blocks`/`scene_node_to_blocks`/`dependency_closure`/`stub_to_node` 跨阶段命名统一。`ContextBlock::new`/`Scope`/`Visibility`/`Stability` 标注"以 trpg-model 实际签名为准"。

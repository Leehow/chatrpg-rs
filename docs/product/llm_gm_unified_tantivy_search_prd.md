# LLM GM 统一检索系统产品设计书

版本：v0.7  
范围：规则 / 模组 / 记忆 / 裁定 / learned packets / Markdown / JSONL 解析产物  
技术实现：Rust + Tantivy + PostgreSQL + Axum + CLI  
前端策略：接口优先，无 Web 页面

---

## 1. 背景

当前项目已经从“规则书和模组必须全量解析后才能开跑”转向“先 onboard-and-play，再边跑边查、边查边记、常用后晋升”的 LLM GM 设计。

这个方向下，检索不再是辅助功能，而是 LLM GM 的核心能力：

```text
玩家行动
  -> 判断是否需要规则/模组/记忆证据
  -> 搜索规则、模组、记忆、裁定、learned packet
  -> 选取证据
  -> 编译临时 packet
  -> 做 source-backed 或 provisional ruling
  -> 记录 lookup_event / ruling_log
  -> 高频内容晋升 learned_packet
```

如果没有统一检索，LLM GM 会退回两个坏路径：

1. 启动前试图全量结构化规则书和模组，速度慢、成本高、容易过拟合。
2. 运行时靠模型记忆和 prompt 猜规则，容易幻觉、污染上下文、破坏缓存稳定性。

所以 v0.7 目标是让统一检索系统进入 live runtime：它既能手动搜索，也能在 GM 回合里自动检索、加载、记录和缓存，类似 `ripgrep` 的使用体验，但搜索范围覆盖文件、数据库、解析产物和运行时记忆。

---

## 2. 产品目标

### 2.1 面向用户

用户可以在终端直接搜索：

```bash
trpg rg "Hacking Athena" --domain modules --module cyberpunk_red.homecoming
trpg rg "Basic Tech" --domain rules --ruleset cyberpunk_red
trpg rg "Foxwell" --domain memory --session session_123
trpg rg "Athena" --jsonl --explain
```

输出应包含：

```text
分数
来源类型
领域 domain
逻辑类型 kind
标题
片段 snippet
scope 信息
source_refs
可选 explain
```

### 2.2 面向 LLM GM

LLM GM 可以通过统一 API 检索：

```text
规则：核心规则、procedure、book locator、cold data locator
模组：module overview、current session packet、scene、NPC、location、clue
记忆：memory_events、memory_facts、memory_snapshots
裁定：rulings_log
学习结果：learned_packets
源文：Markdown / JSONL artifacts
```

然后把搜索命中转成 TTL `ContextBlock`，交给 RuntimeMaterialPlanner 决定进入 BP2 还是 BP3。

### 2.3 面向工程

搜索范围不能写死在 Rust 代码里。随着项目变大，数据库表和解析产物会不断变化；搜索系统必须通过配置注册搜索源。

因此：

```text
Rust search engine 只认识 SearchDocument。
具体搜哪些表、哪些文件、怎么映射字段，由 search_source_configs 决定。
```

---

## 3. 非目标

v0.7 仍不做：

```text
Web 页面
向量检索
语义 reranking
自动规则复盘
自动 learned packet 晋升
分布式搜索服务
复杂权限后台
```

这些留到后续版本。

---

## 4. 核心设计

### 4.1 SearchDocument

所有可检索内容先归一化为：

```rust
SearchDocument {
  search_doc_id,
  origin,
  domain,
  logical_kind,
  title,
  body,
  tags,
  scopes,
  visibility,
  stability,
  source_refs,
  metadata,
  updated_at,
}
```

这里最重要的是：

```text
origin/domain/logical_kind 都是 String，不是 enum。
scopes 是 BTreeMap<String, String>。
metadata 是 JSON。
```

这样未来新增：

```text
spell_index
monster_index
campaign_timeline
clue_graph
handout_archive
player_notes
audio_transcript
```

都不需要改 Rust enum。

### 4.2 SearchSourceConfig

搜索源配置表：

```sql
search_source_configs (
  source_config_id text unique,
  source_kind text,
  label text,
  enabled boolean,
  priority integer,
  config_json jsonb
)
```

支持三类源：

```text
sql_query   从 PostgreSQL 任意查询返回 SearchDocument 列
file_glob   从 data/ 下的 Markdown/plain text 文件切块索引
jsonl       从 JSONL artifacts 按行索引
```

SQL 源必须返回这些列：

```text
search_doc_id
origin
domain
logical_kind
title
body
tags
visibility
stability
scope_json
source_refs
metadata
updated_at
```

Rust 不知道这些数据来自 `context_blocks`、`memory_events`、还是未来的 `campaign_clue_edges`。它只消费这组标准列。

### 4.3 Tantivy Index Schema

Tantivy 字段：

```text
search_doc_id  STRING | STORED
origin         STRING | STORED
domain         STRING | STORED
logical_kind   STRING | STORED
title          TEXT   | STORED
body           TEXT   | STORED
tags           TEXT   | STORED
facets         STRING | STORED
scopes_json    STRING | STORED
visibility     STRING | STORED
source_refs    STRING | STORED
metadata_json  STRING | STORED
updated_at     STRING | STORED
```

检索主要靠：

```text
title boost
body normal
tags boost
facets filter/routing
```

facets 是从 domain、kind、visibility、tags、scopes 生成的稳定字符串，例如：

```text
domain__rules
kind__procedure
tag__athena
ruleset_id__cyberpunk_red
module_id__cyberpunk_red.homecoming
session_id__session_123
```

---

## 5. 默认搜索源

migration 默认注册：

```text
db.context_blocks
db.material_index
db.book_locator_entries
db.memory_events
db.memory_facts
db.memory_snapshots
db.rulings_log
db.learned_packets
files.markdown
files.parsed_jsonl
```

这些默认源只是初始配置，不是代码限制。

未来要加新表，只需要：

```sql
insert into search_source_configs (...)
values (..., 'sql_query', ..., '{"sql":"select ... as search_doc_id, ..."}'::jsonb)
on conflict ...;
```

---

## 6. 用户体验

### 6.1 普通搜索

```bash
trpg rg "Athena" --domain modules --module cyberpunk_red.homecoming
```

输出：

```text
1. [12.503] modules / book_locator / Hacking Athena
   scopes: {"owner_id":"cyberpunk_red.homecoming",...}
   sources: [{"source_id":"cpr_homecoming","page":5,...}]
   ...snippet...
```

### 6.2 JSONL 输出

```bash
trpg rg "Basic Tech" --domain rules --jsonl
```

输出：

```jsonl
{"event":"hit","hit":{...}}
{"event":"done","query_id":"search_...","considered":12}
```

方便 LLM、脚本、CI、测试程序消费。

### 6.3 查询搜索源

```bash
trpg search sources
```

显示当前所有启用的 source config。

### 6.4 重建索引

```bash
trpg search reindex
```

或 API：

```text
POST /api/search/reindex
```

### 6.5 Generic scope/filter

```bash
trpg rg "Athena" \
  --scope module_id=cyberpunk_red.homecoming \
  --filter origin=book_locator \
  --explain
```

`scope` 和 `filter` 都是 key=value，不绑定任何特定 schema。

---

## 7. API 设计

### 7.1 POST /api/search

请求：

```json
{
  "query": "Hacking Athena",
  "domains": ["modules"],
  "scopes": {"module_id":"cyberpunk_red.homecoming"},
  "limit": 8,
  "explain": true,
  "viewer": {
    "viewer_kind": "gm",
    "player_id": null,
    "actor_id": null,
    "can_see_gm_only": true
  }
}
```

响应：

```json
{
  "query_id": "search_...",
  "hits": [
    {
      "hit_id": "hit_...",
      "search_doc_id": "book_locator:...",
      "origin": "book_locator",
      "domain": "modules",
      "logical_kind": "scene",
      "title": "Hacking Athena",
      "snippet": "...",
      "score": 12.5,
      "scopes": {},
      "source_refs": [],
      "metadata": {},
      "explain": {}
    }
  ]
}
```

### 7.2 GET /api/search/sources

列出启用 source configs。

### 7.3 POST /api/search/sources

新增或更新 source config。用于 schema 迭代，不需要重新编译。

### 7.4 POST /api/search/load

把 SearchHit 转换为 TTL ContextBlock。

```json
{
  "hit": {...},
  "session_id": "session_123",
  "cache_zone": "dynamic_tail",
  "ttl": "turn",
  "load_reason": "player is hacking Athena"
}
```

返回 `ContextBlock`。

---

## 8. 与 Runtime 的边界

搜索系统只做三件事：

```text
1. index
2. search
3. hit -> context block conversion helper
```

它不负责：

```text
最终是否加载进 prompt
加载进 BP2 还是 BP3
是否提交状态
是否裁定规则
是否晋升 learned packet
```

边界：

```text
SearchEngine
  -> SearchHit
MaterialResolver / PacketCompiler
  -> ContextBlock
RuntimeMaterialPlanner
  -> BP1/BP2/BP3
Validator
  -> commit state
```

---

## 9. 缓存稳定性

搜索不会自动改变上下文。

```text
只搜索：不改变 BP1/BP2/BP3
加载临时搜索结果：默认 BP3，TTL=turn
将搜索结果 pin 到当前场景：BP2，TTL=scene
learned packet stable：BP2
learned packet memorized：BP1 候选，但需要 review
```

这保证：

```text
只改玩家输入 -> prefix_hash/pinned_hash 不变
查一次规则 -> prefix_hash/pinned_hash 不变
临时 lookup block -> 只影响 dynamic_hash
stable learned packet -> pinned_hash 改变
resident core 修订 -> prefix_hash 改变
```

---

## 10. 可见性策略

搜索必须先做 visibility 过滤。

```text
GM/System：可见全部
Player：只可见 public/player_visible
NPC：public/player_visible/npc_private
```

这对 Triangle Agency 这类显式区分 Field Agent Manual、GM Toolkit、Playwalled Documents 的资料尤其重要。

---

## 11. 数据生命周期

### 11.1 Ingest / Parse

```text
pdftotext -> page anchored markdown
parser -> onboarding / locators / prep packets
DB write
JSON/JSONL export
Tantivy reindex
```

### 11.2 Turn

v0.6 当前：

```text
turn -> memory_event 写 DB
需要手动或 API reindex 才进入搜索
```

v0.7 目标：

```text
turn -> memory_event 写 DB -> incremental Tantivy upsert
ruling -> ruling_log 写 DB -> incremental Tantivy upsert
learned packet -> DB write -> incremental Tantivy upsert
```

### 11.3 Clean rebuild

v0.6 是 upsert indexing。未来需要：

```text
trpg search reset
trpg search reindex --clean
source_generation marker
source tombstone
```

---

## 12. 验收标准

### 12.1 Homecoming

```bash
trpg rg "Hacking Athena" --domain modules --module cyberpunk_red.homecoming --reindex
```

应命中 Homecoming 当前章节、Athena、Hacking Athena 或相近 locator/source block。

### 12.2 Cyberpunk RED

```bash
trpg rg "Netrunning" --domain rules --ruleset cyberpunk_red
```

应命中 Cyberpunk RED 的 Netrunning locator / source / rule block。

### 12.3 Memory

```bash
trpg turn --ruleset cyberpunk_red --module cyberpunk_red.homecoming --input "我检查无人机背后的线缆。"
trpg search reindex
trpg rg "无人机 线缆" --domain memory
```

应命中刚才的 memory_event。

### 12.4 Schema evolution

新增一张表，例如：

```text
campaign_clues
```

只添加一条 `search_source_configs` SQL config，然后：

```bash
trpg search reindex
trpg rg "clue text"
```

应能搜到，不改 Rust。

---

## 13. 风险

| 风险 | 影响 | 缓解 |
|---|---|---|
| Tantivy API minor version 变化 | 本地第一次编译可能需要小修 | 固定 `tantivy = "0.26"`，后续 cargo check 锁定版本 |
| source config SQL 写错 | 某类源无法索引 | `search_index_runs.stats_json.errors` 记录错误；CLI/API 返回 warnings |
| stale docs | 删除的源可能仍在索引里 | v0.7 做 clean rebuild |
| player search 泄漏 GM-only | 剧透 | visibility filtering 内置，不允许 player 搜 gm_only |
| 搜索结果直接污染 prompt | 破坏缓存和状态 | SearchHit 不自动进入 prompt，必须转 ContextBlock 并由 Planner 加载 |

---

## 14. 路线图

### v0.6 已实现

```text
trpg-search crate
Tantivy index
SearchDocument model
search_source_configs registry
SQL/file/JSONL source adapters
trpg rg
trpg search query/reindex/sources
/api/search
/api/search/reindex
/api/search/sources
/api/search/load
/api/rules/lookup 迁移到 unified search
parse-all 后自动 reindex
```

### v0.7

```text
incremental indexing after memory/ruling/learned writes
trpg search reset / --clean
RuleDemandDetector
SearchHit -> rule packet compiler
turn runtime 自动检索并加载 BP3 lookup block
```

### v0.8

```text
post-session audit
learned packet auto-promotion
ranker policy per ruleset/module
query expansion from book_locator
```

### v0.9+

```text
hybrid vector search
semantic rerank
multi-user player visibility partition
source citation UX
search trace visualization
```

---

## 15. 最终判断

统一检索系统是 LLM GM 从“能回答”走向“能长期跑团”的基础设施。

它让 GM 的知识形态从：

```text
一次性全量解析 + prompt 塞满
```

变成：

```text
onboarding familiarity
locator map
runtime search
source-backed ruling
provisional ruling
learned packet
cache-stable memory
```

这更像真实 GM 的工作方式，也更符合 BP1/BP2/BP3 的缓存设计。


---

## 13. v0.7 增量更新

v0.7 在 v0.6 Tantivy 统一索引之上新增运行时闭环：

```text
玩家输入
  -> RuntimeEngine 轻量识别规则/模组敏感动作
  -> SearchService.search_async
  -> lookup_events 记录检索证据
  -> SearchLoadRequest 转换为 TTL ContextBlock
  -> BP3 turn lookup 或 BP2 scene-pinned packet
  -> ContextBuilder 编译稳定三段上下文
```

### 13.1 Search/load API

`POST /api/search/load` 现在返回：

```json
{
  "block": {},
  "persisted": true,
  "lookup_event_id": "lookup_..."
}
```

如果 `persist=true` 且包含 `session_id`，block 会写入 runtime bundle：

```text
bundle_id = runtime.<session_id>
```

运行时下一轮会从该 bundle 加载 still-valid TTL blocks。

### 13.2 Per-source incremental indexing

`trpg search reindex --incremental` 使用 `search_index_watermarks` 按 source_config 记录水位。新增业务表时，只要插入新的 `search_source_configs`，就能独立建立水位，不需要改搜索核心。

### 13.3 CJK bridge

Tantivy index 新增 `cjk_ngrams` 字段，并对中文/日文/韩文字符建立单字和 bigram 索引。同时查询会扩展常见 TRPG 中文需求：

```text
黑客/黑入 -> netrunning, hack, net, architecture
无人机 -> drone, vehicle, robot, athena
线缆 -> cable, tech, basic, repair
检定 -> check, skill, roll, dv, dc
```

这不是最终中文分词器，但能让中文终端输入命中英文规则书和模组原文。

### 13.4 Cache invariant

搜索自动化不能破坏缓存稳定性：

```text
一次性检索 -> BP3 DynamicTail, ttl=turn
当前场景可复用规则 -> BP2 PinnedMiddle, ttl=scene
稳定 learned packet -> BP2
BP1 只能人工 review 后变化
```

只改玩家输入时，`prefix_hash` 和 `pinned_hash` 应保持不变；只有 scene-pinned packet 或 memory snapshot 更新时，`pinned_hash` 才变化。

# LLM GM v0.7 产品设计书：运行时检索、TTL ContextBlock 与自学习 GM

版本：v0.7  
范围：chatrpg-rs runtime / search / API / CLI  
状态：接口优先，无 Web UI

---

## 1. 一句话定位

v0.7 把 v0.6 的 Tantivy 统一检索层接入运行时主路径：玩家回合中出现规则、模组、线索、NPC、地点、装备、黑客、战斗、检定等需求时，LLM GM 会自动触发统一检索，把命中的 SearchHit 编译为带 TTL 的 ContextBlock，并由 BP1/BP2/BP3 planner 决定进入动态尾部或半稳定场景层。

核心原则不变：搜索结果不能直接污染聊天历史。搜索结果必须先成为可见性受控、来源可追溯、TTL 明确、缓存层明确的 ContextBlock。

```text
Player input
  → rule/module demand heuristic
  → Tantivy SearchRequest
  → SearchHit[]
  → SearchLoadRequest
  → TTL ContextBlock
  → RuntimeMaterialPlanner
  → BP1/BP2/BP3
  → LLM GM response
  → lookup_events / memory / rulings / learned_packets
```

---

## 2. 背景

v0.5 已经把系统从 full-parse-first 改为 onboard-and-play：规则书和模组先生成 GM Onboarding Bundle、Book Locator、Cold Data Locator 和 First Session Packet。v0.6 引入 Tantivy 统一检索，支持从动态 source configs 索引数据库表、Markdown 文件和 JSONL 解析产物。

v0.7 补上关键闭环：检索不只是给人查，也要能服务 GM runtime。

用户在跑团中不会总是显式输入 `/search`。他们会自然说：

```text
我检查无人机后面的线缆，看能不能切断。
我想黑进去看看 Athena 是不是被谁控制。
我攻击最近的 Scavv。
我追问 Foxwell 有没有私藏电池。
这时候要不要做 Basic Tech 检定？
```

这类输入对 GM 来说是规则敏感或模组敏感。v0.7 的目标就是自动定位相关规则/模组材料，尽可能给 GM 一个 source-backed ruling；证据不足时，明确记录 provisional ruling 的上下文。

---

## 3. 产品目标

### 3.1 运行时自动检索

LLM GM 不再只依赖 BP1 resident + BP2 current packet。每一轮在编译上下文前，runtime 可以根据玩家输入触发检索：

```text
规则敏感：检定、攻击、战斗、伤害、治疗、法术、黑客、装备、技能等。
模组敏感：当前 NPC、地点、章节、线索、遭遇、任务目标、手册、地图等。
记忆敏感：之前承诺、NPC 关系、已发现线索、玩家行动后果等。
```

### 3.2 检索结果可缓存、可回收

SearchHit 必须转换为 ContextBlock：

```text
turn TTL      → BP3 DynamicTail
scene TTL     → BP2 PinnedMiddle
session TTL   → BP2 PinnedMiddle
manual/none   → 不自动过期，但仍需要显式 persist
```

### 3.3 缓存稳定

默认一次检索只影响 BP3；只有高置信度的当前场景材料或 stable learned packet 才进入 BP2。BP1 不会被自动检索污染。

### 3.4 中文/英文混合查询更可用

TRPG 桌面输入常常是中文动词 + 英文术语 + 模组专名混合。v0.7 增加 CJK n-gram 字段和中英别名扩展：

```text
黑客 / 黑入  → hack, netrunning, NET Architecture
无人机       → drone, robot, Athena
线缆         → cable, tech, Basic Tech, repair
检定         → check, roll, skill, DV, DC
线索         → clue, lead, revelation, investigation
```

---

## 4. 用户故事

### 4.1 GM 自动查当前场景规则

作为 GM runtime，当玩家说“我检查无人机后面的线缆，看能不能切断”，系统应该：

1. 判断输入包含“检查 / 线缆 / 能不能”等规则或模组敏感信号。
2. 搜索当前 module、ruleset、learned packets、rulings、source docs。
3. 找到 Athena / drone / cable / Basic Tech / Netrunning 相关命中。
4. 生成临时 LookupResult block。
5. 如果命中当前 scene/module 且适合重复使用，则 pin 到 BP2 scene TTL。
6. 写入 lookup_events。

### 4.2 调试者显式加载 SearchHit

作为测试者，我可以先：

```bash
trpg rg "Hacking Athena" --module cyberpunk_red.homecoming --jsonl
```

再把返回的 hit 通过 API 加载：

```text
POST /api/search/load
```

并指定：

```json
{
  "cache_zone": "pinned_middle",
  "ttl": "scene",
  "persist": true
}
```

### 4.3 数据库结构变化后不改搜索核心

作为开发者，我新增一个 `campaign_clocks` 或 `npc_relationship_edges` 表时，不需要改 Rust 搜索引擎。只需要新增一条 `search_source_configs`：

```json
{
  "source_kind": "sql_query",
  "config_json": {
    "sql": "select ... as search_doc_id, ... as body, ... as updated_at from campaign_clocks"
  }
}
```

---

## 5. 核心对象

### 5.1 SearchRequest

新增/强化字段：

```rust
pub struct SearchRequest {
    pub query: String,
    pub mode: SearchMode,
    pub domains: Vec<String>,
    pub kinds: Vec<String>,
    pub tags: Vec<String>,
    pub scopes: BTreeMap<String, String>,
    pub filters: BTreeMap<String, Vec<String>>,
    pub limit: u32,
    pub explain: bool,
    pub viewer: VisibilityProfile,
    pub rewrite_query: bool,
    pub intent: Option<String>,
}
```

`intent` 用于 lookup_events 和后续 audit，例如：

```text
runtime_auto_rule_module_lookup
rule_lookup
cli_search
search_load
```

### 5.2 SearchHit

SearchHit 是检索结果，不直接进入 prompt。它包含：

```text
search_doc_id
origin
domain
logical_kind
title
snippet
score
scopes
tags
visibility
source_refs
metadata
explain
```

### 5.3 SearchLoadRequest

SearchHit 被加载时必须显式声明目标缓存层和 TTL：

```rust
pub struct SearchLoadRequest {
    pub hit: SearchHit,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub scene_id: Option<String>,
    pub ruleset_id: Option<String>,
    pub module_id: Option<String>,
    pub demand_id: Option<String>,
    pub query_text: Option<String>,
    pub cache_zone: CacheZone,
    pub ttl: String,
    pub load_reason: String,
    pub persist: bool,
}
```

### 5.4 Runtime Search ContextBlock

生成的 block 形态：

```text
block_id: runtime.search.{session_id}.{stable_hash}
kind: lookup_result
cache_zone: dynamic_tail | pinned_middle
stability: turn_dynamic | scene_stable
expires_at_turn: turn_id when ttl=turn
expires_at_scene: scene_id when ttl=scene
visibility: inherited from SearchHit
source_refs: inherited from SearchHit
load_reason: auto_turn_lookup | auto_scene_rule_packet | api_search_load
```

---

## 6. BP1/BP2/BP3 策略

### 6.1 BP1 Prefix

v0.7 不允许自动搜索结果进入 BP1。BP1 只能来自：

```text
engine protocol
ruleset onboarding bundle
game identity
play loop
ruleset kernel
character sheet map
book locator summary
reviewed resident learned packets
```

### 6.2 BP2 Pinned Middle

以下材料可以进入 BP2：

```text
current module overview
current session packet
current scene/location/NPC/clue slice
stable memory snapshot
stable/memorized learned packet
scene TTL search packet
```

v0.7 自动 pin 的默认上限是每轮 1 个 scene search packet，可通过：

```env
TRPG_RUNTIME_AUTO_SEARCH_MAX_SCENE_PINS=1
```

### 6.3 BP3 Dynamic Tail

默认检索结果进入 BP3：

```text
turn TTL search hit
lookup result
provisional ruling evidence
current input
recent transcript
world state projection
retrieved memory
```

---

## 7. 自动检索启发式

v0.7 使用轻量启发式，不做昂贵 LLM demand detector：

```text
英文关键词：check, roll, rule, dc, dv, skill, attack, combat, damage, heal, netrun, hack, scene, npc, clue, where, how
中文关键词：检定, 判定, 规则, 技能, 攻击, 战斗, 伤害, 治疗, 黑客, 无人机, 线缆, 线索, 调查, 地点, 怎么, 能不能
长度启发：长输入也可能包含具体行动需求
```

未来 v0.8 可以把这个替换成 LLM / classifier demand detector。

---

## 8. Tantivy CJK 与双语查询

v0.7 的 Tantivy schema 增加 `cjk_ngrams` 字段。索引时把 title/body/tags 中的 CJK 字符生成 unigram/bigram；查询时如果包含 CJK，则进入 CJK n-gram query。

同时内置少量 TRPG 常用中英别名：

```text
检定 → check, skill, roll, dv, dc
攻击 → attack, combat, weapon, ranged, melee
黑客 → netrunning, hack, net, architecture
无人机 → drone, vehicle, robot, athena
线索 → clue, lead, revelation, investigation
```

这些不是语义向量检索，而是可解释的 lexical query expansion。

---

## 9. 增量索引

v0.7 增加：

```bash
trpg search reindex --incremental
trpg rg "Athena" --reindex --incremental-reindex
```

API：

```http
POST /api/search/reindex
{
  "incremental": true
}
```

当前实现使用 `search_index_watermarks` 记录每个 source_config 的 watermark。每个 SQL source 在增量模式会被包成：

```sql
select * from (<source_sql>) as trpg_search_source
where updated_at > $1
```

文件和 JSONL source 通过文件 modified time 过滤。

full rebuild 会清空 Tantivy index 并重置所有 watermark。

---

## 10. API

### 10.1 POST /api/search

执行统一检索，并在带 `session_id` scope 时记录 lookup_events。

### 10.2 POST /api/search/reindex

```json
{
  "incremental": true
}
```

### 10.3 GET /api/search/sources

列出当前启用的 source configs。

### 10.4 POST /api/search/sources

新增或修改 source config。

### 10.5 POST /api/search/load

把 SearchHit 转成 TTL ContextBlock，并可选持久化到 runtime bundle。

---

## 11. CLI

```bash
trpg search reindex
trpg search reindex --incremental
trpg search sources
trpg rg "Hacking Athena" --module cyberpunk_red.homecoming
trpg rg "黑入无人机线缆" --module cyberpunk_red.homecoming --jsonl
trpg rg "Basic Tech" --domain rules --ruleset cyberpunk_red --reindex --incremental-reindex
```

---

## 12. 配置

```env
TRPG_SEARCH_INDEX_DIR=./data/search/tantivy_v3
TRPG_SEARCH_WRITER_MEMORY_BYTES=96000000
TRPG_RUNTIME_AUTO_SEARCH=true
TRPG_RUNTIME_AUTO_SEARCH_LIMIT=5
TRPG_RUNTIME_AUTO_SEARCH_MAX_SCENE_PINS=1
TRPG_RUNTIME_AUTO_SEARCH_DOMAINS=learned,rules,modules,rulings,source,parsed
TRPG_RUNTIME_AUTOPIN_SCENE_RULES=true
TRPG_SEARCH_CJK_EXPANSION=true
```

---

## 13. 验收测试

### 13.1 缓存稳定性

1. 同一 scene，只改玩家输入，不触发 scene pin：prefix_hash 和 pinned_hash 不变，dynamic_hash 变化。
2. 触发一次普通 lookup：prefix_hash 和 pinned_hash 不变。
3. 触发 scene pin：prefix_hash 不变，pinned_hash 变化。
4. 下一轮同一 scene：scene-pinned search block 自动从 runtime ContextBlockStore 加载。
5. 切 scene：旧 scene block 不再加载。

### 13.2 检索

1. `trpg rg "Hacking Athena" --module cyberpunk_red.homecoming` 命中 Homecoming 模组相关材料。
2. `trpg rg "黑入无人机" --module cyberpunk_red.homecoming` 能通过 CJK expansion 命中 Athena/drone/hack 相关材料。
3. `trpg search sources` 显示 DB/file/JSONL source configs。
4. 新增 source config 后，reindex 能把新 source 纳入 Tantivy，无需改搜索核心代码。

### 13.3 运行时

1. `trpg turn` 输入“我检查无人机后面的线缆，看能不能切断。”后，context_compiled 事件中 dynamic/pinned hash 反映检索加载。
2. lookup_events 有对应 `runtime_auto_rule_module_lookup` 记录。
3. `/api/search/load` 持久化后，同一 scene 后续回合能加载该 block。

---

## 14. 非目标

v0.7 不做：

```text
向量检索
完整 LLM demand detector
自动规则复盘与晋升
Tantivy 删除墓碑清理
更细的 row-level deletion tracking
专业中文 tokenizer
Web UI
```

这些进入 v0.8+。

---

## 15. 风险

### 15.1 自动 pin 过度导致 BP2 不稳定

缓解：默认每轮最多 pin 1 个 scene block，并可用 env 关闭：

```env
TRPG_RUNTIME_AUTOPIN_SCENE_RULES=false
```

### 15.2 CJK n-gram 噪声

缓解：只作为辅助字段，不替代 title/body/tags/facets；并保留 query rewrite fallback。

### 15.3 source config SQL 不安全或太慢

缓解：source configs 是本地开发/GM 管理能力，不面向玩家；后续可加入 query timeout 和 source doctor。

### 15.4 可见性泄漏

缓解：SearchRequest 带 VisibilityProfile；matches_request 会在 search hit 过滤阶段执行 visibility check。player viewer 不允许看到 gm_only / system_only。

---

## 16. 版本路线

```text
v0.7
  Runtime automatic search
  SearchHit → TTL ContextBlock
  /api/search/load persistence
  lookup_events integration
  CJK n-gram + bilingual expansion
  incremental reindex

v0.8
  LLM/rule-demand classifier
  source excerpt expansion
  search doctor
  row-level deletion tracking
  automatic post-session audit
  learned packet promotion policy

v0.9
  semantic/vector reranker as optional second-stage
  campaign graph search
  clue/revelation graph integration
  stronger CJK tokenizer
```

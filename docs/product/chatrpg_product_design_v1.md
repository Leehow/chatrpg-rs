# ChatRPG Product Design Document

版本：v1.0  
目标实现版本：Rust v0.8 及后续  
产品代号：Onboard-and-Play LLM GM  
状态：产品设计稿，不是接口说明书，也不是代码实现说明书

---

## 0. 产品一句话

**ChatRPG 是一个面向 TRPG 的 LLM GM / KP / DM 运行系统。它允许用户把规则书和模组 PDF 放进本地项目，然后让系统像人类 GM 一样先建立可开团的最小理解，边跑边查规则、边裁定、边记忆、边学习，最终长期成长为熟悉该规则和该团历史的智能主持人。**

它不是一个“PDF 转 JSON 工具”，也不是一个“把整本规则书塞进 Prompt 的聊天机器人”。

它的核心能力是：

```text
导入资料 → 建立规则/模组定位图 → 准备第一场 → 创建角色 → 开始跑团
        → 运行中检索规则/模组/记忆 → 做有来源的裁定
        → 记录本桌发生的事 → 压缩成稳定记忆 → 高频内容晋升为 learned packet
```

---

## 1. 产品背景

TRPG 的真正困难不在于“有没有规则文本”，而在于：

1. 规则书很长，但开团时并不需要全懂。
2. 模组很长，但当前只需要跑好下一场。
3. 规则裁定必须可靠，不能让 LLM 胡编。
4. 玩家希望输出快，不愿等系统整理完所有后台任务。
5. 跑团历史很重要，但如果把全部历史塞进上下文，会破坏缓存、拖慢响应、制造幻觉。
6. 不同规则书、不同模组、不同桌的裁定会逐步形成“本桌习惯”。

传统做法通常是：

```text
读完整本规则书 → 整理笔记 → 读完整个模组 → 准备角色 → 开跑
```

但真实 GM/KP/DM 通常并不是这样工作。更常见的工作方式是：

```text
先知道这是什么游戏；
知道基本玩法循环；
知道角色卡怎么看；
知道常用规则在哪里；
知道当前模组第一场怎么开；
边跑边查、边查边记、赛后复盘。
```

ChatRPG 把这种人类 GM 的工作方式产品化。

---

## 2. 产品要解决的问题

### 2.1 对玩家

玩家想要的是：

- 不用自己背规则。
- 可以快速开始一场游戏。
- GM 能记得之前发生过的事。
- GM 不要乱编规则。
- 角色创建能被系统辅助。
- 模组能正常推进，隐藏信息不要泄漏。
- 响应要快，最好流式输出。

### 2.2 对 GM / KP / DM

GM 想要的是：

- 规则书和模组可以被系统吸收。
- 系统知道“哪里查什么”。
- 系统能生成开团准备，而不是要求全量解析。
- 系统能保存本桌裁定。
- 系统能管理线索、NPC、地点、当前场景。
- 系统能把跑团历史整理成稳定记忆。
- 系统能解释为什么加载了某个规则块。

### 2.3 对开发者

开发者想要的是：

- 数据结构可测试。
- 搜索范围不要写死。
- 数据库结构可迭代。
- API-first，方便终端、测试、未来前端复用。
- 上下文缓存稳定，可观测。
- LLM 输出和后台后处理解耦。

---

## 3. 产品理念

### 3.1 Onboard-and-Play，而不是 Parse-All-and-Play

旧思路：

```text
规则书 + 模组 → 完整结构化 → 才能开团
```

新思路：

```text
规则书 + 模组 → GM Onboarding Bundle → 可以开团
跑团中查规则 → 编译临时 packet → 裁定 → 记录 → 学习
```

完整解析不是启动条件。启动条件是 **operational familiarity**：系统知道这是什么游戏、基本循环是什么、角色卡字段大概代表什么、常用规则在哪、当前模组第一场如何开始。

### 3.2 Source-backed ruling，所有关键裁定要有来源

LLM 可以描述、提议、解释、组织信息，但涉及以下内容时必须有来源、已学习 packet，或明确标记为临时裁定：

- 改变 HP / SAN / Humanity / Chaos / MP / 金钱 / 装备。
- 判定角色死亡、重伤、疯狂、永久损失。
- 给出奖励、线索、道具、法术、职业能力。
- 推进模组关键节点。
- 修改 NPC 态度或世界状态。

裁定状态分为：

```text
source_backed   已经查到来源
learned         本桌已经学习并稳定使用
provisional     临时裁定，赛后复核
table_ruling    本桌有意采用的桌规
```

### 3.3 记忆要稳定，不要把历史全塞进上下文

LLM GM 需要记住历史，但历史不应该每轮全量进入 Prompt。

记忆分三层：

```text
memory_events      每轮发生的原始事件，append-only
memory_facts       已确认的长期事实
memory_snapshots   压缩后的稳定记忆快照，进入 BP2
```

新增事件不会破坏 BP2 缓存；只有执行 compact memory 生成新 snapshot 时，BP2 才变化。

### 3.4 检索是 GM 的“翻书能力”

统一检索不是辅助功能，而是 LLM GM 的核心能力。

它要检索：

```text
规则书原文
模组原文
ContextBlock
MaterialIndex
BookLocator
ModulePrepPacket
MemoryEvent
MemoryFact
MemorySnapshot
RulingLog
LearnedPacket
角色卡模板
ProcedureRegistry
```

用户和 runtime 都只面对一个统一检索层。以后数据库表变了，只要新增 search source spec，就能被索引。

### 3.5 输出优先，后台后处理不要阻塞用户

用户看到 GM 输出的速度应该接近 LLM 首 token 速度。

运行时采用两段式：

```text
第一段：SSE 直接流式输出 GM 叙述 / 提问 / 结果
第二段：后台保存、解析 state proposal、验证 patch、写记忆、更新搜索索引
```

如果后处理失败，不能影响已经给用户的可读输出，但必须记录错误。

### 3.6 缓存稳定是产品体验，不只是工程优化

上下文分三层：

```text
BP1 Prefix        规则核心、GM 协议、游戏身份、玩法循环
BP2 Pinned        当前模组、当前场景、稳定记忆、稳定 learned packet
BP3 Dynamic       当前输入、骰子、状态差分、临时检索、临时裁定
```

缓存稳定意味着：

```text
只改玩家输入 → BP1/BP2 hash 不变
查一次规则 → BP1/BP2 hash 不变，BP3 变化
切场景 → BP1 不变，BP2 变化
更新规则核心 → BP1 变化
```

---

## 4. 产品用户

### 4.1 Solo Player

一个人想体验 TRPG 模组。TA 有 PDF，但不想自己通读规则书。TA 希望系统帮助创建角色、主持场景、裁定规则，并记住长期剧情。

成功体验：

```text
我放入 Cyberpunk RED + Homecoming → 系统准备第一场 → 我创建角色 → 我说“检查无人机线缆” → 系统查到当前模组和相关规则 → 给出行动选项和检定建议。
```

### 4.2 Human GM / KP / DM

人类 GM 想让系统做副驾。系统负责查规则、查模组、整理记忆、提醒线索、生成 NPC 快速卡，但最终 GM 可以 override。

成功体验：

```text
GM 输入 /search Athena cable → 系统返回 Hacking Athena、Athena Behavior、Basic Tech / Netrunning 相关材料，并说明来源和可加载层级。
```

### 4.3 Rules Hacker / Translator

用户想把不同规则书变成可检索、可运行的资料包。

成功体验：

```text
系统不要求完美全量结构化，而是先建立 locator、角色卡模板、核心 procedure。后续通过实际使用逐步学习。
```

### 4.4 Developer / Tester

开发者希望通过 CLI 和 API 自动化测试，不依赖交互式终端。

成功体验：

```bash
printf '{"ruleset_id":"cyberpunk_red","module_id":"homecoming","user_input":"我检查无人机线缆"}' \
| trpg turn --request-json - --stream-format jsonl
```

---

## 5. 产品主流程

### 5.1 初始化项目

用户创建本地项目：

```bash
trpg init
trpg auth set
trpg migrate
```

项目目录：

```text
data/
  rulebooks/     规则书 PDF
  modules/       模组 PDF
  markdown/      PDF 转换后的锚点文本
  parsed/        JSON / JSONL 解析产物
  search/        Tantivy 索引
  exports/       角色、会话、trace
```

### 5.2 放入 PDF

用户放入：

```text
data/rulebooks/Cyberpunk RED Core.pdf
data/modules/CPR One Shot - Homecoming.pdf
```

### 5.3 Ingest：PDF → 可检索材料

v0.8 默认使用 oxidize-pdf。

```text
PDF
  → oxidize-pdf rag_chunks()
  → page/chunk anchored Markdown
  → raw chunks JSONL
  → cleanup manifest
  → LLM cleanup for noisy chunks
  → indexable source material
```

oxidize-pdf 的定位是原始提取和 chunk 生成，不是最终规则解析器。它提供 chunk、页码、heading context、token estimate、element types、bounding boxes 等信息，系统再用 LLM 做保守清洗。

### 5.4 Onboard Ruleset

系统读取目录、介绍、玩法章节、角色创建章节、核心检定章节、GM 章节，生成：

```text
gm_onboarding_bundle
  game_identity
  play_loop
  ruleset_kernel
  character_sheet_map
  book_locator
  starter_procedures
  lookup_recipes
  cold_data_locator
```

它的目标不是完整数据库，而是让 LLM GM 达到“能开第一场”的熟悉度。

### 5.5 Prep Module

系统读取模组 synopsis、目录、开场章节、第一场相关 NPC/地点/冲突/线索，生成：

```text
module_prep_packet
  module_overview
  current_session_packet
  required_rule_demands
```

后续章节默认是 cold data，只做 locator，不塞进 BP2。

### 5.6 Create Character

用户可以交互式或非交互式创建角色：

```bash
trpg create-character --ruleset cyberpunk_red --module homecoming --preferences "netrunner, fixer tie-in"
```

系统使用：

```text
CharacterTemplate
ruleset character_sheet_map
module tone and entry hooks
LLM draft
validator
```

输出角色草案，后台校验字段和规则。

### 5.7 Play Turn

每一轮：

```text
玩家输入
  → 规则/模组需求探测
  → Tantivy 统一检索
  → SearchHit → ContextBlock
  → RuntimeMaterialPlanner 选择 BP1/BP2/BP3
  → LLM SSE 流式输出
  → 后台 state proposal / memory / indexing
```

---

## 6. 产品架构

### 6.1 分层架构

```text
┌─────────────────────────────────────────┐
│ Interface Layer                         │
│ CLI / API / SSE / future UI              │
└─────────────────────────────────────────┘
                    ↓
┌─────────────────────────────────────────┐
│ Runtime Layer                           │
│ Session / ContextBuilder / Procedure     │
│ PatchValidator / Memory / Learning       │
└─────────────────────────────────────────┘
                    ↓
┌─────────────────────────────────────────┐
│ Material Layer                          │
│ ContextBlock / MaterialIndex / Locator   │
│ OnboardingBundle / ModulePrepPacket      │
└─────────────────────────────────────────┘
                    ↓
┌─────────────────────────────────────────┐
│ Retrieval Layer                         │
│ Tantivy unified search / SearchSource    │
│ SearchHit / SearchLoad / LearnedPacket   │
└─────────────────────────────────────────┘
                    ↓
┌─────────────────────────────────────────┐
│ Ingest Layer                            │
│ oxidize-pdf / pdftotext fallback         │
│ LLM cleanup / JSONL artifacts            │
└─────────────────────────────────────────┘
                    ↓
┌─────────────────────────────────────────┐
│ Storage Layer                           │
│ PostgreSQL JSONB / files / search index  │
└─────────────────────────────────────────┘
```

### 6.2 核心对象

#### ContextBlock

能进入 Prompt 的最小上下文单位。带：

```text
block_id
kind
visibility
stability
cache_zone
scope
source_refs
content_hash
token_estimate
TTL
```

#### MaterialIndexEntry

不是 Prompt 内容，而是“可加载材料目录”。告诉系统什么材料存在、何时加载、依赖什么、默认进入哪层缓存。

#### BookLocatorEntry

表示“这类信息在哪里”。例如：

```text
combat rules → Core Rulebook, Friday Night Firefight
netrunning → Core Rulebook, Netrunning
items → equipment chapter
spells → magic data section
```

#### ModulePrepPacket

表示“下一场怎么跑”。包括 strong start、当前场景、当前 NPC、当前线索、当前冲突、当前规则需求。

#### LearnedPacket

表示系统已经查过、用过、验证过的经验性规则包或本桌裁定。

---

## 7. PDF 处理设计

### 7.1 为什么使用 oxidize-pdf

v0.8 把 oxidize-pdf 作为默认 PDF ingest backend，因为它能直接产生适合 RAG/LLM 的 chunk，并提供页码、标题上下文、元素类型、token estimate 和位置框等信息。

### 7.2 为什么还需要 LLM cleanup

PDF 的视觉结构不等于阅读结构。尤其是中文模组、表格目录、双栏页面、地图注解和 NPC stat block，会出现：

```text
目录表格串成一行
标题重复
中文词中出现布局空格
同一地点描述跨页
Keeper-only 信息和正文混在一起
```

因此，系统将 oxidize 输出视为 raw extraction，而不是 final material。

### 7.3 Cleanup 原则

LLM cleanup 不是总结器，而是保守校对员。

必须遵守：

```text
不新增规则。
不删机械数字。
不删 NPC 名、地点名、线索、内容警告。
不改变 chunk_id / page_numbers。
不把 GM-only 信息改成 player-visible。
不把不确定信息写成确定事实。
```

输出目标是更清晰的 chunk 文本、section path、visibility hint、content kind hint、uncertainty flags。

---

## 8. 规则书设计模式

规则书不是平均解析。

系统先抽：

```text
Game Feel
Play Loop
Character Sheet Map
Core Resolution
Procedure Seeds
Book Locator
Cold Data Locator
```

数据表默认 cold：

```text
item list
spell list
monster list
skill detailed entries
class features
cyberware
mounts
programs
vehicles
```

使用时才查；查过才学；高频才晋升。

---

## 9. 模组设计模式

模组也不全量展开。系统把模组分成：

```text
Module Overview
Current Session Packet
Cold Module Data
```

对 one-shot：准备 synopsis、开场、第一章、关键 NPC/地点/冲突。

对 mission collection：准备当前 mission，而不是 12 个任务全部塞进上下文。

对大型 campaign：准备 campaign graph、chapter locator、clue graph、current chapter packet。

---

## 10. GM 记忆产品设计

### 10.1 用户期待

玩家说：“你还记得我们之前救过那个 NPC 吗？”系统应该能记得。

但它不应该每轮把全部 transcript 塞进去。

### 10.2 记忆层级

```text
memory_events
  原始事件：本轮发生了什么。

memory_facts
  已确认事实：谁欠谁人情、哪条线索已发现、哪个地点已改变。

memory_snapshots
  稳定摘要：当前 session / scene / arc 的压缩记忆。
```

### 10.3 记忆进入上下文

```text
BP2:
  stable memory snapshots

BP3:
  retrieved memory events/facts for current input
```

---

## 11. 搜索产品设计

### 11.1 统一检索的用户心智

用户不应该想：这是搜文件？搜数据库？搜记忆？搜规则？

用户只应该想：

```bash
trpg rg "Athena cable"
trpg rg "Basic Tech" --ruleset cyberpunk_red
trpg rg "Foxwell" --session current
```

### 11.2 搜索源不写死

搜索通过 source specs 定义。任何新表、JSONL、Markdown、memory store，只要能投影成 SearchDocument，就能被索引。

SearchDocument 的心智模型：

```text
一个可搜索对象 = 来源 + 类型 + 标题 + 正文 + 标签 + 可见性 + 作用域 + source_refs
```

### 11.3 搜索不会直接污染 Prompt

```text
SearchHit
  → SearchLoadRequest
  → ContextBlock
  → Planner
  → BP1/BP2/BP3
```

搜索结果默认进入 BP3，只有 scene/session/stable packet 才进入 BP2。

---

## 12. 可见性与防剧透

TRPG 产品必须保护隐藏信息。

### 12.1 可见性等级

```text
public
player_visible
gm_only
npc_private
system_only
playwalled
```

### 12.2 关键规则

```text
player prompt 不能检索 gm_only / playwalled。
GM prompt 可以检索 gm_only，但仍要尊重 table policy。
LLM 不能把 gm_private narration 直接输出给玩家。
Search 必须先过滤 visibility 再排名。
```

---

## 13. 接口产品设计

### 13.1 CLI-first

当前不做 Web 页面。CLI 是第一前端：

```bash
trpg init
trpg auth set
trpg migrate
trpg parse-all
trpg create-character
trpg play
trpg turn
trpg rg
trpg search reindex
```

### 13.2 API-first

CLI 不是唯一接口。所有核心能力都应该有 API：

```text
POST /api/ingest/parse-all
POST /api/characters/create
POST /api/sessions/{id}/turn
POST /api/search
POST /api/search/load
POST /api/memory/compact
```

### 13.3 Non-interactive testing

每个交互命令必须有 JSON request + JSONL stream 等价模式。

---

## 14. SSE 输出设计

用户体验优先：先输出，再后处理。

SSE 事件：

```text
event: phase   start
event: phase   context_compiled
event: phase   llm_stream_start
event: delta   token text
event: phase   postprocess_scheduled
event: done
```

后台任务：

```text
save_turn
parse_state_proposal
validate_patch
commit_world_state
write_memory_event
update_search_index
```

---

## 15. 产品成功指标

### 15.1 启动成功

```text
用户放入规则书 + 模组 PDF 后，不需要等待全书完整解析，也能开始第一场。
```

### 15.2 查规则成功

```text
玩家说一个规则敏感动作，系统能查到相关规则或明确标记 provisional ruling。
```

### 15.3 缓存稳定

```text
只改玩家输入时，BP1/BP2 hash 不变。
新增 memory_event 时，BP1/BP2 hash 不变。
compact memory 后，BP2 hash 变化。
```

### 15.4 记忆有效

```text
系统能回忆上次发生的关键事件，但不会把完整 transcript 长期塞上下文。
```

### 15.5 防剧透有效

```text
player-visible 输出不会泄漏 GM-only / playwalled 内容。
```

---

## 16. 典型验收场景

### 16.1 Cyberpunk RED + Homecoming

用户上传 Cyberpunk RED 规则书和 Homecoming 模组。

系统应能：

```text
识别 Cyberpunk RED 的核心章节定位：角色、技能、Getting It Done、Friday Night Firefight、Netrunning、Trauma Team、Running Cyberpunk。
识别 Homecoming 的三段结构。
准备第一场 Upper Marina / rogue drone / Athena cable。
当玩家检查线缆或黑入无人机时，自动检索 Hacking Athena / Netrunning / Basic Tech 相关材料。
```

### 16.2 Triangle Agency + The Vault

系统应能：

```text
识别 Triangle 的 Field Agent Manual、GM Toolkit、Playwalled Documents 可见性边界。
识别 The Vault 的 mission template：Introduction、Pre-Investigation、Chaos Effects、Investigation、Encounter、Aftermath。
当前只加载正在跑的 mission。
Chaos / Ask the Agency / Anomaly behavior 可按需进入上下文。
```

### 16.3 Sword World 2.5 Core I-III

系统应能：

```text
识别 Core I 是基础规则书。
识别 Core II/III 是扩展补充。
先建立角色创建、2d6 检定、战斗、魔法、数据表 locator。
装备、法术、怪物默认 on-demand。
```

### 16.4 Call of Cthulhu + Masks of Nyarlathotep

系统应能：

```text
识别大型 campaign 的 chapter graph。
建立 clue / NPC / location locator。
只准备当前 chapter，不把 600+ 页全部放入 BP2。
Keeper-only 信息默认 gm_only。
```

---

## 17. 非目标

短期内不做：

```text
Web UI。
扫描版 OCR。
完整规则编译器。
所有规则百分百确定性执行。
完全自动无监督规则学习。
公开云端服务。
```

短期产品重点是本地、接口优先、可跑、可查、可记、可学。

---

## 18. 路线图

### v0.8

```text
oxidize-pdf ingest
raw chunks JSONL
cleanup manifest
LLM cleanup pass
真实产品设计文档
```

### v0.9

```text
SearchHit 持久化 TTL block 完善
runtime 自动 rule-demand search 更强
CJK tokenizer / bilingual query rewrite
增量索引完善
post-session audit
```

### v1.0

```text
稳定 CLI/API
Cyberpunk + Homecoming 验收
Triangle + The Vault 验收
GM memory compact 稳定
基础 validator 稳定
```

### v1.1+

```text
更多规则 family
更强 deterministic procedure engine
human GM co-pilot mode
可视化调试面板
可选 Web UI
```

---

## 19. 产品总结

ChatRPG 的产品目标不是“让 LLM 背下一切”。

它的目标是做一个真正可成长的 LLM GM：

```text
知道这是什么游戏；
知道怎么开局；
知道去哪里查；
查到后能做有来源的裁定；
裁定后能记住；
用多了能熟练；
长期保持本桌连续性；
同时保持输出快、缓存稳、防剧透。
```

这就是 ChatRPG 的产品核心。

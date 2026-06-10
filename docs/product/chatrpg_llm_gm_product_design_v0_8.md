# ChatRPG LLM GM 产品设计说明书 v0.8

> 文档类型：产品设计说明书 / Product Design Specification  
> 产品代号：ChatRPG LLM GM  
> 当前版本：v0.8 — Oxidize PDF Ingest + LLM Cleaning + Runtime Search  
> 主要读者：产品负责人、系统设计者、规则解析工程师、LLM Runtime 工程师、测试/评测人员、未来前端设计人员  
> 状态：草案，可用于下一阶段实现、评审和测试计划拆解  

---

## 0. 文档定位

这不是 API 文档，也不是 crate 说明书。本文回答的是产品问题：

1. 这个产品是做什么用的？
2. 它解决谁的问题？
3. 它为什么要这样设计？
4. 用户如何从一堆规则书和模组开始，最终真的跑起来？
5. 系统如何像一个人类 GM/KP/DM 一样学习、查阅、裁定和记住？
6. 为什么不是“把整本规则书全量结构化完再开团”？
7. 为什么要把 PDF ingestion、检索、记忆、缓存、裁定和角色创建作为同一个产品闭环设计？

工程细节只在必要时出现，用来解释产品能力的边界，而不是替代产品设计。

---

# 第一部分：产品总览

## 1. 一句话说明

ChatRPG LLM GM 是一个面向 TRPG 跑团的本地优先 AI GM/KP/DM 系统。它可以读取用户提供的规则书 PDF 和模组 PDF，把它们转化为可检索、可引用、可学习、可缓存的材料；然后通过终端或 API 让玩家创建角色、进入模组、进行对话、检定、战斗、调查和长期记忆管理。

它的目标不是“复刻一本规则数据库”，而是“让 LLM 获得类似人类 GM 的 operational familiarity”：知道这是什么游戏，第一场怎么开，角色卡怎么读，常用规则在哪里，遇到问题能查，查完能裁，裁完能记住。

## 2. 产品愿景

长期愿景：

> 任何一组玩家，只要有合法持有的规则书和模组 PDF，就能用一个本地运行的 AI GM 在较短时间内开团。系统不要求开局完全理解所有规则，而是在游玩过程中逐步建立熟练度、规则包、模组记忆和本桌裁定。

这意味着产品应像“人类 GM 学新系统”一样运作：

- 先读目录、导言、核心循环和角色卡。
- 先准备第一场而不是准备所有未来可能发生的场景。
- 技能、装备、法术、怪物、NPC stat block 等冷数据先定位，后按需查。
- 游戏过程中，规则查不到时可以临时裁定，但必须记录为 provisional。
- 查过并用过的规则逐渐变成 learned packet。
- 多次稳定使用的 learned packet 可以进入半稳定上下文。
- 只有经过确认的高频内核才有资格成为常驻规则。

## 3. 产品不是做什么

ChatRPG LLM GM 不是：

- 不是 PDF OCR 产品。
- 不是版权内容分发平台。
- 不是规则书全文数据库销售工具。
- 不是一个保证 100% RAW 裁定正确的专家系统。
- 不是必须联网运行的 SaaS。
- 不是一开始就试图结构化所有装备、法术、怪物、职业、NPC 和世界设定的全量知识图谱。
- 不是单纯的 RAG chatbot。

它是一个“跑团运行系统”。所有 ingestion、检索、缓存、记忆和裁定的设计，都必须服务于“让一场团顺畅地跑下去”。

## 4. 核心价值主张

### 4.1 快速开团

用户把规则书 PDF 放到 `data/rulebooks/`，模组 PDF 放到 `data/modules/`，设置 API key 后执行：

```bash
trpg parse-all
trpg create-character --ruleset cyberpunk_red --module cyberpunk_red.homecoming
trpg play --ruleset cyberpunk_red --module cyberpunk_red.homecoming
```

系统默认只做 onboarding、locator、first-session prep 和搜索索引，不要求全量解析。

### 4.2 可持续学习

每次查规则、每次临时裁定、每次玩家行动后生成的事实，都能进入学习系统：

- lookup_event：查了什么。
- ruling_log：怎么裁的。
- learned_packet：这个规则/模组机制已经用过并可复用。
- memory_event：这轮发生了什么。
- memory_fact：已经确认的事实。
- memory_snapshot：稳定上下文摘要。

### 4.3 缓存稳定

LLM 成本和延迟受上下文缓存影响极大。因此产品从一开始就把上下文分为：

- BP1：常驻稳定前缀。
- BP2：当前场景/模组/记忆半稳定层。
- BP3：每轮动态尾部。

每一次搜索、记忆、裁定、状态变化都必须明确进入其中一层，不能随意污染历史消息。

### 4.4 可解释和可追溯

GM 说“按这个规则裁定”时，系统内部必须知道来源：

- 来自哪个 PDF。
- 哪一页。
- 哪个 chunk。
- 是否经过 LLM 清洗。
- 是 source-backed、learned、provisional 还是 table ruling。

### 4.5 本地优先，接口优先

当前阶段不做 Web 页面，保留接口和终端：

- CLI 用于本地工作流和测试。
- Axum API 用于自动化、未来前端、SSE 流式输出。
- PostgreSQL 用于状态和结构化数据。
- JSON/JSONL 用于产物导出。
- Tantivy 用于统一检索。

---

# 第二部分：用户与场景

## 5. 目标用户

### 5.1 Solo 玩家

想独自体验 TRPG 模组。痛点：没有 GM，规则书太厚，模组准备耗时。目标：上传规则书和模组，创建角色，直接进入故事。

### 5.2 小团体 GM 辅助

有人类 GM，但希望 AI 帮助检索规则、整理模组、记录记忆、生成摘要和 NPC 反应。目标：AI 不是取代 GM，而是成为“第二大脑”。

### 5.3 规则黑客 / 自制系统作者

希望把自己写的规则或公开 SRD/ORC 内容转换成可运行材料。目标：快速测试规则循环、角色卡、检定流程。

### 5.4 LLM TRPG 系统开发者

需要一个可测试、可扩展、可插拔的 runtime。目标：验证上下文缓存、检索、记忆、SSE、规则学习和多规则系统适配。

## 6. 典型使用场景

### 场景 A：Cyberpunk RED + Homecoming

用户把 Cyberpunk RED Core 放入 rulebooks，把 Homecoming one-shot 放入 modules。系统先建立 Cyberpunk 的游戏感、技能检定、战斗、Netrunning、治疗和 Night City 风格 locator；模组侧准备 synopsis、Chapter 1、Athena drone、仓库、Hacking Athena、Athena 行为和第一场可能需要的规则。用户创建一个 Netrunner 或 Tech 角色，然后开始。

### 场景 B：Triangle Agency + The Vault

用户把 Triangle Agency 和 The Vault 放入系统。系统识别 Triangle 的 Field Agent Manual、GM Toolkit、Playwalled Documents 可见性边界；识别 The Vault 的 mission collection 结构。选择 Springs Eternal 后，系统只加载当前 mission 的 Anomaly Profile、Pre-Investigation、Chaos Effects、Investigation、Encounter 和 Aftermath packet。

### 场景 C：Sword World 2.5 三本核心书

系统识别 Core I 是基础规则和低级角色，Core II/III 是补充规则、高等级、追加职业、追加魔法、追加物品、追加怪物。开局不会全量解析所有 spell/item/monster，而是建立 locator 和角色卡地图，用到时查。

### 场景 D：Masks of Nyarlathotep 大型沙盒

系统不会把 600+ 页 campaign 编译成一个巨大剧情树，而是建立 campaign graph、chapter locator、clue/revelation ledger、NPC/faction/place index 和当前 chapter packet。玩家从 Peru 或 New York 开始，系统只加载当前 playable unit。

---

# 第三部分：设计理念

## 7. 人类 GM 学习模型

一个人类 GM 拿到新系统时，通常不是先把整本书完全结构化，而是形成五层熟悉度：

### L0 Game Feel / 游戏感

这是什么类型的游戏？玩家主要做什么？系统鼓励什么行为？失败意味着什么？GM 的叙事语气是什么？

### L1 Play Loop / 基本循环

游戏一轮怎么推进？什么时候描述？什么时候让玩家声明行动？什么时候掷骰？什么时候产生后果？

### L2 Character Sheet Map / 角色卡地图

角色卡上有哪些字段？哪些是身份/背景？哪些是检定资源？哪些会在游戏中频繁变化？哪些字段派生其他值？

### L3 Book Locator / 书本定位地图

检定规则在哪？战斗在哪？装备在哪？法术在哪？怪物在哪？GM 指导在哪？模组第一场在哪？

### L4 Learned Packets / 经验性规则包

已经查过、用过、验证过的规则片段；当前桌形成的裁定；当前角色常用能力；当前场景反复使用的机制。

ChatRPG LLM GM 的启动条件是 L0-L3，而不是完整 L4。

## 8. Onboard-and-Play，而不是 Parse-All-and-Wait

旧思路：

```text
大 PDF → 完整结构化规则数据库 → 才能开团
```

新思路：

```text
大 PDF → GM Onboarding Bundle → 可以开团
        → 运行时查规则
        → 记录裁定
        → 晋升 learned packet
        → 逐步熟练
```

这个设计直接决定了产品形态：

- `parse-all` 名字保留，但默认行为是 onboard + index。
- full parse 变成 opt-in。
- 搜索和记忆成为核心产品能力，不是补丁。
- Parser 的产出不是一个巨大 ruleset，而是一组可检索、可加载、可缓存的材料。

## 9. MaterialBlock-first / ContextBlock-first

系统不应该把规则、模组、记忆直接拼进 prompt。所有长期或半长期材料都先转化为 ContextBlock：

```text
规则书材料
模组材料
角色卡模板
当前场景
NPC 卡
线索
记忆摘要
learned packet
检索结果
```

每个 ContextBlock 必须带：

- block_id
- kind
- content
- visibility
- stability
- cache_zone
- scope
- priority
- source_refs
- content_hash
- token_estimate
- TTL / expires_at_turn / expires_at_scene

然后 RuntimeMaterialPlanner 决定它进入 BP1、BP2 还是 BP3。

## 10. Source-backed Ruling

LLM 可以讲故事，但涉及状态、资源、伤害、线索、奖励、死亡、分支和规则判定时，不能凭空自信。它的裁定必须是：

- source_backed：有来源。
- learned：来自已确认 learned packet。
- provisional：临时裁定，之后复盘。
- table_ruling：本桌明确采用的 house rule。

对玩家可以自然表达；对系统必须结构化记录。

---

# 第四部分：核心产品架构

## 11. 产品架构总览

```text
PDF / Markdown / JSONL / DB
        ↓
Ingest Layer
  oxidize-pdf rag_chunks()
  deterministic cleanup
  optional LLM cleaning
        ↓
Source Material Layer
  source_documents
  source_chunks
  page/chunk anchors
        ↓
Onboarding Layer
  game_identity
  play_loop
  ruleset_kernel
  character_sheet_map
  book_locator
  starter_procedures
        ↓
Preparation Layer
  module_overview
  first_session_packet
  current_scene_packet
  cold_data_locator
        ↓
Search Layer
  Tantivy unified index
  dynamic source configs
  rule/module/memory/ruling/learned search
        ↓
Runtime Planner
  ContextBlockStore
  MaterialPlanner
  BP1/BP2/BP3 compiler
        ↓
LLM GM Runtime
  SSE response stream
  deterministic dice/procedure engine
  state proposal validator
        ↓
Learning & Memory
  lookup_events
  rulings_log
  learned_packets
  memory_events
  memory_facts
  memory_snapshots
```

## 12. 子系统边界

### 12.1 Ingest

职责：把 PDF 变成带来源锚点的文本和 chunk。它不负责理解规则。它只负责：

- 打开 PDF。
- 提取文本。
- 保留页码、chunk、heading、类型等元数据。
- 输出 Markdown 和 JSONL sidecar。
- 标记明显抽取质量问题。

### 12.2 Cleaner

职责：修复 PDF 抽取带来的阅读顺序、标题重复、表格分隔符、乱码、跨栏混杂问题。它不应该总结，不应该改写规则，不应该删除敏感内容，只能做 conservative repair。

### 12.3 Parser / Onboarder

职责：从文档中抽出“开团所需的最小心智模型”：

- game_identity
- play_loop
- ruleset_kernel
- character_sheet_map
- book_locator
- cold_data_locator
- module_overview
- first_session_packet

### 12.4 Search

职责：统一检索规则、模组、记忆、裁定和 learned packets。它输出 SearchHit，不直接写 prompt。

### 12.5 MaterialResolver

职责：把 SearchHit 转换成可加载材料，例如 ContextBlock，且带 cache_zone 和 TTL。

### 12.6 RuntimeMaterialPlanner

职责：决定每轮加载哪些 ContextBlock，并保证 BP1/BP2/BP3 稳定。

### 12.7 LLM GM

职责：叙事、提问、解释、提出 state proposal。它不能直接提交状态。

### 12.8 Validator

职责：校验 LLM 的 state proposal、角色卡、资源变化、隐藏信息可见性和规则成本。

### 12.9 Memory

职责：让 GM 记住发生过的事，但不破坏缓存。事件进 memory_events，事实进 memory_facts，稳定摘要进 memory_snapshots。

---

# 第五部分：PDF Ingestion 产品设计

## 13. 为什么采用 oxidize-pdf

v0.8 选择 oxidize-pdf 作为优先 PDF ingestion backend。原因不是“它能完美解析 PDF”，而是它更适合我们的产品形态：

- 它是 Rust-native，更适合当前技术栈。
- 它能直接输出 RAG chunks，而不是只有整页文本。
- chunk 带 page_numbers、element_types、heading_context、token_estimate 等信息。
- chunk 比整页文本更适合后续检索、清洗、locator 和 ContextBlock 编译。
- 它的输出即使乱，也比普通纯文本更有结构线索。

## 14. Ingest 产物

对于一个 PDF，系统输出：

```text
data/markdown/rulebooks/{source_id}.oxidize.md
data/markdown/modules/{source_id}.oxidize.md
data/parsed/source_chunks/{source_id}.raw_chunks.jsonl
data/parsed/source_chunks/{source_id}.llm_cleaned_chunks.jsonl   # 可选
data/markdown/cleaned/{source_id}.llm_cleaned.md                # 可选
```

每个 chunk 保留：

- chunk_id
- source_id
- chunk_index
- text
- full_text
- page_numbers
- element_types
- heading_context
- token_estimate
- is_oversized
- text_hash
- clean_status
- metadata

## 15. 清洗策略

### 15.1 Heuristic clean

默认启用。用于修复：

- 多余空格。
- 连续表格竖线。
- 重复标题。
- 明显乱码标记。

### 15.2 LLM clean

默认关闭，可通过环境变量启用：

```env
TRPG_INGEST_LLM_CLEAN=true
TRPG_INGEST_LLM_CLEAN_MAX_CHUNKS=24
```

LLM clean 只处理疑似脏 chunk：

- oversized chunk。
- 出现大量 `| |`。
- 出现 mojibake。
- 头部几行特别长。
- 没有 heading_context 但文本很长。

LLM clean 的产品契约：

- 不总结。
- 不删规则。
- 不删安全警告。
- 不删 GM-only 信息。
- 不改数值。
- 不把表格解释成新规则。
- 只修复文本顺序、重复、分隔符和 heading。

### 15.3 Parser clean vs Runtime clean

清洗发生在 parser 前，不发生在 runtime 中。Runtime 搜索到的材料必须已经有 clean_status，避免每轮临时清洗导致延迟和缓存不稳定。

## 16. 质量问题如何被产品化

PDF 抽取质量不是一个纯技术问题，而是用户体验问题。产品必须让用户知道：

- 这本 PDF 是否可用。
- 哪些页/块很乱。
- 是否启用了 LLM clean。
- 哪些 chunk 被清洗。
- 清洗是否失败。
- 是否需要人工检查。

未来可增加：

```bash
trpg ingest doctor
trpg inspect chunk --source blood_highway --chunk 244
trpg clean-chunks --source blood_highway --llm
```

---

# 第六部分：规则书产品模型

## 17. 规则书不等于规则数据库

规则书通常包含：

- 游戏介绍。
- 角色创建。
- 核心检定。
- 战斗。
- 魔法/能力。
- 装备/物品。
- 世界设定。
- GM 指导。
- 怪物/敌人。
- 索引。

产品处理方式不同：

| 信息类型 | 启动时处理 | 运行时处理 |
|---|---|---|
| 游戏感 | BP1 常驻 | 很少变化 |
| 核心循环 | BP1 常驻 | 很少变化 |
| 角色卡地图 | BP1/角色创建 | 创建角色时展开 |
| 基础检定 | starter procedure | 可确定性执行 |
| 战斗入口 | locator + starter | 用到时查 |
| 装备/法术/怪物 | cold data locator | on demand search |
| 世界风格 | BP1 风格约束 | 叙事中持续使用 |
| GM 指导 | director policy | 按场景加载 |

## 18. GM Onboarding Bundle

规则书解析的第一产物是：

```json
{
  "game_identity": {},
  "play_loop": {},
  "ruleset_kernel": {},
  "character_sheet_map": {},
  "book_locator": [],
  "starter_procedures": [],
  "lookup_recipes": [],
  "cold_data_locator": [],
  "learned_packets_seed": []
}
```

这个 bundle 的判断标准是：是否能让 GM 开始第一场，而不是是否完整覆盖整本书。

## 19. 角色卡模板

角色卡模板是规则书解析的一等产物。它用于：

- 交互式创建角色。
- 自动补全角色。
- 校验派生值。
- 判断玩家动作需要什么属性/技能。
- 判断当前角色有哪些常用能力可加载进 BP2。

角色创建不是“LLM 随便写一个角色”，而是：

```text
CharacterTemplate
  → 用户偏好
  → LLM CharacterDraft
  → Validator
  → 修复/确认
  → CharacterSheet
```

---

# 第七部分：模组产品模型

## 20. 模组不是剧情摘要

模组必须能运行，但不等于开局全量展开。产品上分三层：

### Module Overview

- premise
- tone
- genre
- secrets
- major NPC/factions
- chapter/mission graph

### Current Session Packet

- strong start
- current scenes
- current NPCs
- current locations
- current clues
- current encounters
- required rule demands

### Cold Module Data

- later chapters
- optional locations
- handouts
- maps
- rewards
- stat blocks
- alternative endings

## 21. 调查型模组：Clue/Revelation 优先

调查模组最重要的不是剧情线，而是：

- 玩家需要知道什么。
- 每个信息从哪里获得。
- 哪些 NPC/地点/手段可以给出线索。
- 玩家错过线索时如何补救。
- 哪些信息是 GM-only。

因此系统未来应优先构建 revelation ledger，而不是固定剧情树。

## 22. Mission Collection

The Vault 这种 mission collection 最适合模板化。每个 mission 都可以映射为：

```json
{
  "introduction": {},
  "anomaly_profile": {
    "history": "",
    "focus": "",
    "domain": "",
    "appearance": "",
    "impulse": "",
    "current_situation": ""
  },
  "pre_investigation": {},
  "chaos_effects": [],
  "investigation": [],
  "encounter": {},
  "aftermath": {},
  "requisitions": [],
  "anomaly_abilities": []
}
```

但默认只准备当前 mission，不展开全部 mission 的每个细节。

---

# 第八部分：统一检索产品设计

## 23. 为什么检索是一等能力

如果产品不全量解析规则和模组，那么检索不是附加功能，而是核心运行能力。GM 在游戏中必须能快速回答：

- 这个技能怎么判？
- 这个地点在哪一页？
- 这个 NPC 之前说过什么？
- 当前任务有哪些线索？
- 这个怪物 stat block 在哪里？
- 我们上次怎么裁定这个规则？

## 24. Tantivy 统一索引

Tantivy 索引的不只是文件，而是所有可检索材料：

- raw/cleaned source chunks
- ContextBlocks
- MaterialIndex
- BookLocatorEntries
- ModulePrepPackets
- MemoryEvents
- MemoryFacts
- MemorySnapshots
- RulingsLog
- LearnedPackets
- CharacterTemplates
- ProcedureRegistry

搜索范围不写死在代码里，而是由 `search_source_configs` 控制。新表、新 JSONL、新数据类型，只要投影成 SearchDocument，就能进入索引。

## 25. SearchHit 不直接进 Prompt

SearchHit 是检索结果，不是上下文材料。用户或 runtime 需要显式 load：

```text
SearchHit
  → SearchLoadRequest
  → ContextBlock
  → RuntimeMaterialPlanner
  → BP1/BP2/BP3
```

这可以防止搜索结果污染历史和破坏缓存。

---

# 第九部分：GM 记忆产品设计

## 26. 记忆的产品目标

GM 记忆要同时满足两个相反目标：

1. 记得之前发生过什么。
2. 不让每一轮新记忆打碎缓存。

因此系统分三类记忆：

- memory_events：每轮发生的原始事件，append-only。
- memory_facts：确认事实。
- memory_snapshots：稳定摘要，进入 BP2。

## 27. 记忆进入上下文的规则

| 记忆类型 | 默认位置 | 缓存影响 |
|---|---|---|
| 新 memory_event | 不进 BP2，可检索进 BP3 | 只影响 dynamic |
| retrieved memory | BP3 | 每轮变化 |
| memory_snapshot | BP2 | compact 时才变化 |
| 长期事实 | DB/检索层 | 按需加载 |

## 28. 记忆用户体验

终端中：

```text
/memory
/compact-memory
```

未来产品形态：

- 玩家问“上次我们答应了谁？”系统查 memory。
- GM 切场景前自动 compact。
- 章节结束后生成 session recap。
- 冲突事实标记为待确认。

---

# 第十部分：Runtime 产品设计

## 29. 每轮主循环

```text
玩家输入
  → 识别意图和规则敏感性
  → 检索规则/模组/记忆/learned packet
  → 编译 BP1/BP2/BP3
  → LLM SSE 流式输出
  → 后台后处理
  → 状态 proposal 校验
  → 写入 turn/memory/ruling/lookup
```

## 30. SSE 两阶段体验

用户最先感知的是响应速度。产品必须保证：

- 不等后处理才输出。
- LLM token 到达后立即流式给用户。
- postprocess_scheduled 只作为事件通知。
- 保存、记忆、校验、索引刷新都在后台。

SSE 事件：

```text
event: phase start
event: phase context_compiled
event: phase llm_stream_start
event: delta ...
event: phase postprocess_scheduled
event: phase done
```

## 31. 缓存稳定性

产品验收标准：

```text
只改玩家输入：
  prefix_hash 不变
  pinned_hash 不变
  dynamic_hash 变化

查一次规则：
  prefix_hash 不变
  pinned_hash 不变
  dynamic_hash 变化

pin 当前场景规则：
  prefix_hash 不变
  pinned_hash 变化

compact memory：
  prefix_hash 不变
  pinned_hash 变化

修 ruleset resident：
  prefix_hash 变化
```

---

# 第十一部分：安全、可见性与剧透控制

## 32. 可见性模型

材料按可见性分为：

- public
- player_visible
- gm_only
- npc_private
- system_only

搜索、上下文构建、LLM 输出都必须先做 visibility projection。

## 33. Playwalled / Keeper-only 内容

有些系统天然包含只给 GM/KP 的内容。产品上必须支持：

- 文档区段可见性。
- source chunk 可见性。
- ContextBlock 可见性。
- WorldState 字段级可见性。
- Player prompt 不检索 GM-only。

## 34. 成人内容和安全词

模组可能包含成人内容、恐怖、暴力、创伤主题。系统应：

- ingestion 阶段保留 content warning。
- onboarding 阶段识别安全提示。
- play 阶段允许用户声明雷区。
- 生成内容时遵守内容控制，不把安全说明当成剧情可忽略内容。

---

# 第十二部分：接口与终端产品形态

## 35. 当前不做 Web 页面

现阶段产品重点是内核和接口：

- CLI 用于真实用户和测试。
- API 用于未来前端和自动化。
- SSE 用于流式体验。
- JSONL 用于机器测试。

## 36. 关键 CLI

```bash
trpg init
trpg auth set
trpg migrate
trpg parse-all --pdf-backend oxidize
trpg search reindex
trpg rg "Hacking Athena" --module cyberpunk_red.homecoming
trpg create-character --ruleset cyberpunk_red --module cyberpunk_red.homecoming
trpg turn --request-json - --stream-format jsonl
trpg play --ruleset cyberpunk_red --module cyberpunk_red.homecoming
```

## 37. 关键 API

```text
POST /api/ingest/parse-all
POST /api/search
POST /api/search/load
POST /api/characters/create
POST /api/sessions/start
POST /api/sessions/{session_id}/turn
GET  /api/sessions/{session_id}/memory
POST /api/sessions/{session_id}/memory/compact
POST /api/rulings
POST /api/learned-packets
```

---

# 第十三部分：产品度量

## 38. 成功指标

### 38.1 开团速度

- 从 PDF 放入目录到可进入第一场的时间。
- 默认 parse-all 是否明显快于 full parse。
- Homecoming / The Vault 这类模组是否能在 first-session packet 下启动。

### 38.2 检索质量

- 玩家中文输入能否命中英文规则。
- 当前 scene 的搜索结果是否排在前列。
- SearchHit 是否有可用 source_refs。

### 38.3 裁定质量

- source-backed ruling 占比。
- provisional ruling 复盘完成率。
- 同类规则重复查询次数是否下降。

### 38.4 缓存稳定性

- 只改玩家输入时 BP1/BP2 hash 稳定率。
- memory_event 是否不污染 BP2。
- SearchHit 是否默认只进入 BP3。

### 38.5 角色创建质量

- CharacterTemplate 抽取成功率。
- Validator 自动修正率。
- 用户需要手动补字段次数。

## 39. 质量门槛

v0.8 可接受：

- PDF 抽取不是完美，但必须有 chunk sidecar。
- LLM clean 只处理有限 chunk。
- Runtime search 仍是启发式。
- Learned packet 仍需人工/接口晋升。

v1.0 前必须完成：

- 稳定 source chunk schema。
- SearchHit → ContextBlock TTL 完整持久化。
- 角色卡模板真实可用。
- 至少两个规则系统 + 模组组合跑通。
- 可见性测试覆盖 GM-only/player-visible。

---

# 第十四部分：路线图

## v0.8

- oxidize-pdf 默认 ingestion。
- raw chunks JSONL。
- LLM cleaning opt-in。
- 更真实的产品设计书。
- README 和配置更新。

## v0.9

- `trpg ingest doctor`。
- chunk 质量报告。
- CJK tokenizer / 更强中英 query rewrite。
- Runtime demand detector 可测试化。
- SearchHit load 持久化和 TTL 清理。

## v1.0

- Cyberpunk RED + Homecoming 可完整开团。
- Triangle Agency + The Vault 可完整开团。
- 角色创建模板稳定。
- 规则检索和 learned packet 闭环稳定。
- API 契约冻结。

## v1.1+

- Web 前端。
- 多玩家可见性。
- 自动 session recap。
- 手动 GM review UI。
- 嵌入模型/向量检索可选。
- 更多规则系统 profile。

---

# 第十五部分：验收场景

## 40. Cyberpunk RED + Homecoming

目标：玩家能处理 Athena drone 开场。

验收：

1. parse-all 输出 Cyberpunk onboarding。
2. Homecoming first-session packet 包含 Upper Marina、rogue drone、Hacking Athena。
3. 玩家说“我检查无人机背后的线缆，看能不能黑进去”。
4. Runtime 自动搜索模块和规则。
5. BP1/BP2 hash 稳定。
6. GM 输出自然，后台记录 lookup_event。

## 41. Triangle Agency + The Vault

目标：玩家能开始 Springs Eternal。

验收：

1. Triangle 可见性区分 Field Agent Manual / GM Toolkit / Playwalled。
2. The Vault mission structure 被识别。
3. 当前 mission 的 Anomaly Profile 进入 BP2。
4. Chaos Effects 按需加载。
5. Player prompt 不泄露 Playwalled 内容。

## 42. Sword World 2.5

目标：角色创建和 2d6 基础检定能跑。

验收：

1. Core I 识别角色创建流程和 Skill Checks。
2. Core II/III 作为补充 locator，不覆盖 Core I。
3. Spell/item/monster 不开局全量解析。
4. 常用 spell 被查询后生成 learned packet。

## 43. Masks of Nyarlathotep

目标：大型 campaign 不崩。

验收：

1. Campaign chapter locator 成功。
2. Peru / New York 等章节作为 playable units。
3. Dramatis Personae / clue / location 可检索。
4. 当前 chapter 之外的隐藏内容不进入 player context。

---

# 第十六部分：开放问题

1. LLM cleaning 的默认策略是否应按文件质量自动开启？
2. PDF chunk 清洗是否需要人工 review workflow？
3. learned packet 自动晋升是否需要阈值和撤销机制？
4. 玩家可见性是否需要从单用户模式提前扩展到多人模式？
5. 角色创建 validator 是否应支持 ruleset-specific plugin？
6. 是否需要引入 embedding 检索，还是 lexical + Tantivy 足够？
7. 什么时候把 API 文档从轻量 OpenAPI index 升级为完整 schema？

---

# 附录 A：产品词汇表

- GM/KP/DM：主持人角色，不同系统称谓不同。
- Onboarding Bundle：让 LLM GM 达到可开团状态的最小心智模型。
- Locator：知道内容在哪里，不代表已经结构化。
- Learned Packet：查过、用过、可复用的规则/模组/裁定包。
- ContextBlock：可进入 prompt 的最小材料单位。
- BP1：稳定前缀，常驻缓存。
- BP2：半稳定上下文，场景/章节/记忆快照。
- BP3：动态尾部，每轮输入、检索、骰子、状态变化。
- Provisional Ruling：临时裁定，之后需要复盘。
- Source-backed Ruling：有来源支持的裁定。
- Oxidize Chunk：oxidize-pdf rag_chunks() 输出的结构化文本块。

---

# 附录 B：文档依据与设计来源

本产品设计参考了常见 PRD 结构：产品目的、背景、目标用户、用户故事、功能范围、设计交互、假设、问题、成功指标和发布范围。它也结合了项目已有 Material Pipeline 设计、当前 v0.7 runtime search 代码、用户给出的 oxidize-pdf 输出样例，以及上传规则书/模组的结构特征。

文档中所有产品判断都服务于同一个目标：

> 让 LLM GM 能够像人类 GM 一样先跑起来，边跑边查，边查边记，常用后熟练，同时保持上下文缓存稳定、来源可追溯、可见性安全。

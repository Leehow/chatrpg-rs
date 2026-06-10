# 模组抽取重设计：渐进式结构化模组 reader

> Date: 2026-06-09 · Status: design (approved architecture, pending spec review)
> 设计理念护栏：乐高/数据驱动零硬编码 · 语义优先 · fail-closed 永不编造 · 单一事实源 · 复用优先 · 文件 ≤400 行。

## 1. 问题

`parse_module`(trpg-parser:733)产出的 `ModuleGraph`(trpg-model:1527)目前
`scenes/npcs/clues/...` 全是 `vec![]`(trpg-parser:827-843,grep 确认从未被填充),
且该结构**没有任何消费方**。模组抽取实际只产出 spine(摘要)+ prep_packet +
文本块索引 + 页码定位。后果：

- 进游戏后 materialization 在没有结构化场景时只能靠文本检索硬撑，开局体验差。
- 玩家上传模组要等一次性长解析，迟迟玩不到。

用户重设计愿景（本设计的需求来源，逐字保留意图）：做一个**类似规则抽取那样的
智能 agent** —— 先读 TOC 懂结构、读前言懂大体 → 定抽取方案 → subagent 抽取；
**先抽最小可跑单元（首场景）让玩家秒进游戏**；自定义规则存 BP2 持久记忆、
物品怪物只做索引进 BP3；后续按需现检索；主 agent 在剧情出现实体时后处理建关系图谱。

## 2. 目标 / 非目标

**目标（本 spec = 子项目 1）**
- 模组上传后**秒级可玩**：reader 读 TOC+前言 → 即建全书骨架 → 深抽**首场景+依赖闭包** → 交付开玩。
- 其余场景 **background_job 顺序续抽**；玩到未就绪场景**降级**到现有按需检索（已能跑）。
- 内容分类：自定义规则 → **BP2(PinnedMiddle)**；物品/怪物/实体索引 → **BP3(DynamicTail)**。
- `ModuleGraph` 被真正填充并被 runtime 消费。
- 零硬编码、跨 6 套规则通用、fail-closed。

**非目标（→ 子项目 2，单独 spec）**
- play 中**会话涌现知识图谱**（实体出现→后处理检索→写 `memory_facts` 三元组）。
- 重写 materializer 检索管线（复用现有 Tantivy + duotext grep）。
- background_jobs 加持久化队列/重启续跑/状态端点（见 §7 降级策略，v1 接受降级）。

## 2.5 Feasibility spike 结论（2026-06-09，3 个真实模组实测）

用"Claude 当 reader 钻进壳"在 3 个差异极大的真实模组上跑通了 toc→前言→定入口→骨架→深抽最小单元：

| 模组 | 规则/语种 | 结构 | 骨架 | 入口 read_aloud | 读页(A+B) | 判定 |
|---|---|---|---|---|---|---|
| 血色公路 | CoC 中文 | sandbox+时间线 | 23 | 580字真实 | 18 | feasible_w_caveats(中) |
| The Vault | Triangle 英文 | 任务集 | 17 | 598字真实 | 22 | feasible_w_caveats(高) |
| Homecoming | CPR 英文 | 线性 one-shot | 24 | 529字真实 | 14 | feasible_w_caveats(高) |

**结论：agentic 方案成立**（正确入口 + 非编造 read_aloud + 完整骨架 + BP2/BP3 分类，预算 14-22 页）。
**但三个 reader 独立得出同一头号瓶颈：文本质量（ingestion），不是 reader。**

### Phase 0（先行）：ingestion 去栏交错（trpg-ingest）

实测：duotext 的 reading-order `<id>.md` 把多栏页**逐行交错**（血色公路 p16 序幕念白与右栏广告牌
描述一行一行穿插；p17 序幕收音机与加油站描述两个内容流交错），read_aloud 必须按列重组才能还原 ——
这是 read_aloud 质量命脉，生产用 gpt-5.4-mini 难稳抽。

**落地性已验证**：`pdftotext -bbox` 给 word 级 xMin，血色公路 p16 干净聚成两簇（左 ~47-52 / 右 ~379，
最大相邻间隙 326）；`pdftotext -layout` 本就把双栏并排还原。

**方案**：在 trpg-ingest 把 reading-order `<id>.md` 的生成改为**列感知**：按 word bbox 的 xMin 聚类成
K 列（间隙分析定 K，支持 2/3 栏）→ 每列内按 y 从上到下 → 列间左→右拼接。`.layout.md`（已并排）保留。
fail-closed：聚类不可靠（单栏/表格/异常）→ 回退当前 reading-order，不强行切。
受益面：read_aloud、Tantivy 索引、grep 检索全部变干净，不止 reader。

### reader 侧派生改动（spike 实测）
- `ScenarioLink` 加 `link_type` 枚举（spatial/trigger/timeline/sequential/branch）—— 三个模组都需区分
  "物理可达 / 剧情触发 / 时间线 / 顺序 / 分支"，否则下游无法据 links 排程。
- `ScenarioNode.node_type` 定枚举值（scene/location/mission/chapter/timeline_event/encounter）。
- read_aloud 检测：优先锚句（"read, or paraphrase the following text:" / 第二人称"你们"）→ 语义兜底
  → **fail-closed：拿不准 = None，绝不编造**。
- 图片-only 属性卡（Homecoming p17-26 整段缺失）→ bp3 实体存 name+页码 stub + `stats_unextracted` 标记，
  按需 OCR（未来），**绝不编造数值**。
- 实体跨页去重（场景内联 + 名册 + 附录 → 同一 id）；bp2 自定义规则先按文本块存，结构化子建模（楼层/费用档）延后。

## 3. 架构

```
上传模组
  │  trpg-parser::parse_module  (hook @ lib.rs:827，门控 TRPG_MODULE_READER)
  ▼  → 委派 trpg-rule-agent::reader::module_reader  (新, 复用 super::tools + pub(crate) run_loop)
┌─ Pass A 定位+骨架 (便宜) ───────────────────────────────────┐
│  tools::toc(units) + 前言 → 提交:                            │
│   · 有序场景骨架: ScenarioNode{node_id,title,page_start/end, │
│       referenced_*_ids, extraction_status=SkeletonOnly}      │
│   · 实体索引: npcs/clues/locations/encounters (Value+id+     │
│       content_class) · module_specific_rules (=BP2 自定义规则)│
│  → 写入 ModuleGraph (骨架态), 入库                            │
└──────────────────────────────────────────────────────────────┘
  ▼  Pass B 深抽最小单元 (首场景 + 依赖闭包)
│   tools::read / read_layout 读相关页 → 填 read_aloud/gm_notes/│
│   links + 闭包内 NPC/线索详情 → 翻 DeepExtracted。玩家可开玩 ✅ │
  ▼  入库后 enqueue background_job "module_extract_continue"
     顺序深抽剩余 SkeletonOnly 场景 → 就地升级 ModuleGraph 重新入库

── play 时 ──
runtime 出料:
  · 入库时模组内容已是 ContextBlock(带 cache_zone) → list_context_blocks_for_bundles
    自动按桶进上下文: 自定义规则=PinnedMiddle(BP2,resident) · 实体索引=DynamicTail(BP3)
  · 新增 module_scene_blocks_for_turn 投影器: 当前场景 deep 内容(read_aloud/在场NPC)
    → SceneStatic, scope=Scene, expires_at_scene (逐场景轮换)
  · 未深抽场景 → materializer (Tantivy + duotext grep) 现读现抽 (已能跑, 零新代码)
```

reader 实现选择 **A**：新建 focused slice 完全对标 `chargen_compile.rs` / `object_compile.rs`
（自带 Ctx + SYS prompt + `tools::submit_tool` + 私有 run loop + 私有 dispatch + fail-closed
guardrail）。放在 `reader/` 内即可直接复用 `pub(crate) run_loop`(agent.rs:48)。

## 4. 数据模型改动（trpg-model/src/lib.rs）

全部 `#[serde(default)]`，保证旧 bundle 反序列化与 `ModuleGraph::default()` 都得到廉价骨架态。

```rust
// 新枚举，置于其他 snake_case 枚举旁 (~lib.rs:1648 风格区)
#[serde(rename_all="snake_case")] enum SceneExtractionStatus { SkeletonOnly, DeepExtracted }
impl Default for SceneExtractionStatus { fn default()->Self { Self::SkeletonOnly } }

#[serde(rename_all="snake_case")] enum ModuleContentClass { Bp2CustomRule, Bp3Index, Story }
impl Default for ModuleContentClass { fn default()->Self { Self::Story } }

// 扩展 ScenarioNode (lib.rs:1506) —— 复用现有 read_aloud/gm_notes/links, 仅加:
#[serde(default)] extraction_status: SceneExtractionStatus,
#[serde(default)] page_start: Option<u32>,
#[serde(default)] page_end: Option<u32>,
#[serde(default)] referenced_npc_ids: Vec<String>,
#[serde(default)] referenced_clue_ids: Vec<String>,
#[serde(default)] referenced_location_ids: Vec<String>,
#[serde(default)] referenced_encounter_ids: Vec<String>,

// spike 派生：ScenarioLink (lib.rs:1519) 加 link_type 区分链接语义，否则下游无法据 links 排程
#[serde(rename_all="snake_case")] enum LinkType { Spatial, Trigger, Timeline, Sequential, Branch }
impl Default for LinkType { fn default()->Self { Self::Sequential } }
// 加到 ScenarioLink: #[serde(default)] link_type: LinkType,
// node_type 维持 String 但约定枚举值: scene|location|mission|chapter|timeline_event|encounter
```

**单一事实源决策**：不引入独立 `SceneStub` 结构。"骨架"= `scenes: Vec<ScenarioNode>`
中 `extraction_status==SkeletonOnly` 的节点（只填 title/page/refs，`read_aloud=None`）；
深抽即就地升级同一节点为 `DeepExtracted`。避免双存储与同步。

**实体 vec 保持 `Vec<serde_json::Value>`**（locations/npcs/factions/clues/handouts/encounters/
module_specific_rules），但约定每条 Value 携带 `{id, name, content_class, page_start, page_end,
summary, body}`。`module_specific_rules` 条目天然 `content_class=bp2_custom_rule`。保持 Value 既
符合数据驱动理念、又免 schema 大改；`content_class` 字段驱动 §6 的 cache_zone 映射。

## 5. module_reader 组件（crates/trpg-rule-agent/src/reader/module_reader.rs，新，≤400 行）

```rust
pub struct ModuleReaderCtx<'a> { units: &'a [Unit], sidecar_text: Option<String>, ruleset_id: Option<String> }
pub struct ModuleReadout { spine, scenes, npcs, clues, locations, factions, encounters, handouts, module_specific_rules }
pub async fn run_module_reader(client:&dyn LlmClient, ctx:ModuleReaderCtx<'_>, budget:usize) -> Result<ModuleReadout>
```

- **Pass A（骨架）**：seed = `tools::toc(units, 40)` + 前言页文本；submit schema（`tools::submit_tool`）
  = `{scenes:[stub], entities:{npcs,clues,locations,encounters,handouts}, custom_rules:[...]}`。
  产全书有序场景骨架 + 实体索引 + 自定义规则清单。预算小（一个聚焦 loop）。
- **Pass B（深抽最小单元）**：取 `scenes[0]` + 其 `referenced_*_ids` 闭包 → `tools::read`/`read_layout`
  读相关页 → 填 read_aloud/gm_notes/links + 闭包实体详情 → 翻 `DeepExtracted`。
- **dispatch**：私有 match（克隆 chargen_compile.rs:444），仅 get_toc/search/read/read_layout，截断 ~3000 字符。
- **guardrail（fail-closed）**：页面无内容 → 保持 `SkeletonOnly`，**绝不编造** read_aloud/实体；
  submit 反序列化失败 → 整 readout 降级，parse_module 回退 `vec![]`（保持当前行为）。
- **模型**：reader pass = `TRPG_MODULE_READER_MODEL`（默认 `gpt-5.4-mini`）；深抽 pass 可经
  `build_compiler_llm` 式切更强模型。全 env 驱动，不硬编码。
- 复用：`super::tools::{toc,search,read,dedup,nav_tools,submit_tool}`、`pub(crate) run_loop`、
  `ReaderResult`、`Unit`/`load_units`、`chargen_compile::read_layout`。

`mod.rs` 加 `pub mod module_reader; pub use module_reader::{run_module_reader, ModuleReaderCtx, ModuleReadout};`

## 6. 集成点（精确）

**6.1 parse_module hook（trpg-parser/src/lib.rs:827，替换空 vec 块）**
- 门控 `module_reader_enabled()`（env `TRPG_MODULE_READER`，镜像 `reader_agent_enabled`:1543）。
- units：`reader::load_units(data_dir.join("parsed/source_units").join(format!("{}.semantic_units.jsonl", doc.source_id)))`（同 1497-1498）。
- sidecar：`doc.metadata["layout_sidecar_path"]` → `read_to_string`（同 1538-1540）。
- 调 `run_module_reader` → 把 readout 装入 `ModuleGraph` 各 vec；**err → `vec![]` 回退**（镜像 rulebook reader 回退 1506-1509）。
- `extract_module_spine`(959) 保留作 plan 种子（或并入 readout.spine）。
- Pass B 完成后 enqueue `module_extract_continue` job（剩余 SkeletonOnly 场景）。

**6.2 BP2/BP3 入库（复用现有 block→context 分桶流）**
parse_module 内新增小 helper `module_static_blocks(readout)`，把**分类后的 ModuleGraph 静态内容**
转成 `ContextBlock`（用 `ContextBlock::new`，**显式按 `content_class` 设 cache_zone**，不走
LLM-chunk 路径 `blocks_and_materials_from_llm`、也不依赖其 3011 PinnedMiddle 默认），push 进
现有 `context_blocks` 后随 bundle 持久化：
- 自定义规则（`content_class=bp2_custom_rule`，含 `module_specific_rules`）→ `BlockKind::ModuleSpecificRule`,
  `cache_zone=PinnedMiddle`(BP2), `scope=Scope::module(id)`, tag `"resident"`（持久常驻、靠前）。
- 实体索引（`content_class=bp3_index`，npcs/locations/encounters/handouts 的 id+name+summary 紧凑索引）
  → `BlockKind::ModuleOverview`, `cache_zone=DynamicTail`(BP3), `scope=Scope::module(id)`。
- 注意：**场景 deep 内容不在此**（否则全场景常驻）—— 仅由 §6.3 投影器按当前场景注入。
- 这些静态块每回合经 `list_context_blocks_for_bundles`(runtime:179)自动加载并由
  `plan_blocks`(1599)按 `cache_zone` 分桶 —— **BP2/BP3 放置=设对 cache_zone，无新机制**。

**6.3 当前场景投影器（trpg-runtime/src/lib.rs，新 `module_scene_blocks_for_turn`）**
- 在装配链 ~213（`state_frame_blocks_for_turn` 后）插一行 `match self.module_scene_blocks_for_turn(request,state).await {...}`。
- 实现镜像 `rule_steward_prefix_blocks_for_turn`(1361)：按 `state.scene_id` 取 deep `ScenarioNode`，
  read_aloud/gm_notes + 在场 NPC（按该节点 `referenced_npc_ids` 查 `npcs` vec 取 name+summary）
  → `ContextBlock::new(.., SceneStatic, .., DynamicTail, Scope{Scene,scene_id}, ..)`，
  `expires_at_scene = scene_id`（逐场景轮换）。`extraction_status!=DeepExtracted` → 跳过（交给 6.4 降级）。

**6.4 按需降级（已能跑，可选硬化）**
- 未深抽场景：`materializer`(trpg-material:65)的 Tantivy(`domain=modules/module_id`) + `grep_layout_tables`
  现读现抽原文 —— **零新代码**。locators 始终以 `parse_policy="on_demand"` 产出(parser:801)。
- 可选硬化（本 spec 内，低风险高收益）：把 `queries_for_step(&qp, ModuleCard)` 接入
  `collect_evidence` 检索循环(lib.rs:263-266，现仅 ExactEntity/Field/Locator)，强化模组本地召回。

**6.5 background-continue job（trpg-api 或 parser，镜像 parse_all:141-185）**
- `insert_background_job(job_id, "module_extract_continue", {module_id,ruleset_id,source_id,remaining_scene_ids,parse_config_hash})`。
- `tokio::spawn`：mark running → 按序对每场景跑 Pass-B 深抽 → 读-改-写 `ModuleGraph` 经
  `upsert_module_bundle`(trpg-db:133) 重新入库（单任务串行，无并发覆盖）→ mark done/error。
- job_kind 为自由文本仅作记录，dispatch 隐式（哪个 handler insert 就 spawn 对应代码）。

## 7. 错误处理与降级

- **fail-closed**：reader 任何环节失败 → 该单元保持 SkeletonOnly / parse_module 回退 vec![]，绝不编造。
- **background job 无持久化**（现状）：进程重启则 `running` 行悬挂、不自动续跑。因
  `extraction_status` 已持久化 + §6.4 降级兜底，掉 job 仅表现为部分场景停在骨架（仍可玩）。
  v1 接受此降级；可选增强：启动时扫描 `SkeletonOnly` 场景重新 enqueue（不在本 spec 范围）。
- **读-改-写竞争**：每模组仅一个续抽任务、串行升级，避免 content_json 覆盖。

## 8. 测试

- 单元（module_reader）：Pass A 产有序骨架且全 `SkeletonOnly`；Pass B 把首场景+闭包翻
  `DeepExtracted` 且 read_aloud 已填；空页 → 保持 SkeletonOnly 不编造。
- 数据模型：`ModuleGraph` 带新字段 round-trip；旧 bundle 反序列化 → `SkeletonOnly` 默认。
- parse_module：开门控时 `ModuleGraph.scenes` 非空；reader err 时回退 vec![] 不 panic。
- runtime：`module_scene_blocks_for_turn` 输出 SceneStatic 落 DynamicTail；自定义规则块落
  PinnedMiddle；scope/zone 经 `plan_blocks` 正确分桶。
- 集成（按用户验收口径）：解析一个真实模组（CoC Nyarlathotep 或中文模组）→ 首场景立即可玩
  → 跑几轮 → 后台未抽到的后续场景走 §6.4 降级仍出料。
- 通用性：grep 确认 module_reader 无任一规则专有词（零硬编码）。
- **Phase 0 去栏**：黄金样本测试 —— 血色公路 p16/p17、Homecoming p5、The Vault p11 去栏后，
  序幕念白/boxed 文本应**整段连贯**（无隔壁列文本窜入）；单栏页 + 表格页**幂等不被破坏**；
  bbox 缺失页回退 reading-order 不 panic。

## 9. 子项目 2 预览（会话涌现知识图谱，单独 spec）

扩展**已存在的** `turn_postprocess` job：每回合叙事后，从 ModuleGraph 实体索引 + 轻量实体识别
检出本回合出现的实体 → 对**新出现**实体检索其模组关系 → `upsert_memory_fact` 写三元组
（subject=实体, predicate=关系, object=目标）, `scope=Scene/Session`。喂 GM 连贯性 + 驱动下一步检索。
**零新存储**（`memory_facts` 已是三元组库）。已知 gap：多跳遍历需应用层（现仅 ILIKE 检索）。

## 10. 已决策开放项

| 项 | 决策 |
|---|---|
| 最小可跑单元边界 | 首场景 + 依赖闭包（在场 NPC/线索/read-aloud/该场生效自定义规则）|
| 首场景后其余 | 骨架即建 + background_job 顺序续抽；未就绪降级按需 |
| 知识图谱层次 | 会话涌现、模组懒填种子（子项目 2）|
| reader 实现 | A：focused slice，对标 chargen_compile，复用共享 harness |
| 场景骨架建模 | 单一 `scenes: Vec<ScenarioNode>` + `extraction_status`，不引入 SceneStub |
| BP2/BP3 放置 | 入库时设 ContextBlock.cache_zone（PinnedMiddle/DynamicTail），复用现有分桶 |
| **去栏交错排序**（spike）| **Phase 0 先行**：trpg-ingest 列感知重排 reading-order `.md`，再建 reader |
| **去栏实现**（spike）| word bbox xMin 聚类成 K 列 → 列内按 y → 列间左→右；不可靠则回退，fail-closed |
| **links 语义**（spike）| `ScenarioLink` 加 `link_type`（spatial/trigger/timeline/sequential/branch）|
| **node_type**（spike）| 约定枚举值 scene/location/mission/chapter/timeline_event/encounter |
| **read_aloud 抽取**（spike）| 锚句优先→语义兜底→fail-closed（拿不准 None，绝不编造）|
| **图片-only 属性**（spike）| bp3 存 name+页码 stub + `stats_unextracted` 标记，绝不编造数值 |

**实现阶段顺序（→ writing-plans）**：Phase 0 去栏(trpg-ingest) → Phase 1 数据模型(trpg-model) →
Phase 2 module_reader + parse_module hook → Phase 3 BP2/BP3 入库块 → Phase 4 runtime 场景投影器 →
Phase 5 background-continue → Phase 6 真实模组端到端验证（血色公路/Homecoming）。

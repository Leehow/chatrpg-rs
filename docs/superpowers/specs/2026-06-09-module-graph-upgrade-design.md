# ModuleGraph 图谱升级设计

> Date: 2026-06-09 · Status: design approved
> 调研依据 `docs/模组图谱抽取_方法调研_2026-06-09.md`。护栏:语义优先零硬编码 · fail-closed 永不编造/不乱跳 · 复用现有 module_reader/ModuleGraph/ScenarioLink/scene_node_to_blocks · 文件 ≤400 行。

## 1. 问题(3 模组实测)

当前 ModuleGraph 是"点 + 实体索引",四个缺陷:
- **P1 边全空**:scene→scene `links` 三模组全 0,图不连通,P4 出口无料。
- **P2 骨架完整度飘**:Homecoming 同模组这次 3 场景、上次 24;长文档单遍欠抽。
- **P3 跨页实体未去重**:同实体多页出现 id 不齐,桥接边算不准。
- **P4 关系/边语义缺失** + 抗幻觉。

## 2. 目标 / 非目标

**目标**:把 ModuleGraph 升级成**连通(有边)、完整(骨架不漏)、可验证(连通性质量门)、去重对齐**的场景知识图谱;边抗幻觉(source-anchor)。
**非目标**:子项目2 会话涌现知识图谱(play 中 memory_facts 三元组,单独);重写 scene_navigator(保持全场景语义导航,边只告知);改 VTT/外部格式。

## 3. 已决策(brainstorm 确认)

| 决策 | 选择 |
|---|---|
| 边的角色 | **只告知不约束**:喂 P4 出口投影 + validator;scene_navigator 保持全场景语义导航(边缺不影响) |
| 边来源 | **双源合并**:实体桥接边(零 LLM,召回)+ LLM 线索边(精度+语义),去重进 `links` |
| 实体去重 | 字符串归一先筛 + 模糊对 **LLM 二次确认**(EDC,纯 embedding 会误合) |

## 4. 架构:升级后的抽取流水线

```
Pass A 骨架 + gleaning 回环(②完整度)
   → 实体去重 pass(P3,对齐 id + referenced_*_ids)
   → 建边双源:实体桥接(①纯)+ LLM 线索边(Pass C,带 source-anchor)→ 合并进 links
   → validator(③连通性质量门;分低→log+触发 gleaning 重抽)
   → Pass B 深抽首场景(原样)
```
全程在 `run_module_reader` 内编排;新增两个纯函数文件 + Pass C/gleaning 在 reader。

## 5. 组件

### 5.1 Pass A gleaning 回环(②;module_reader_loop.rs)
骨架 submit 后:把"已抽 scene 列表(node_id+title)+ TOC 文本"喂回,prompt「对照目录,还有哪些可玩单元/场景没进骨架?**只补不重复**;并回答 still_missing: YES/NO」。解析 YES→合并新 stub 再循环,NO 或达 `max_gleanings`(默认 2)停。**fail-closed**:每轮回灌上轮结果;max 兜底防 YES 偏好死循环;解析失败→停用已得。

### 5.2 实体去重 pass(P3;新 helper `dedup_entities`,纯+可选 LLM)
抽取后独立阶段:① `sanitize_key`(小写去标点空白)归一,精确同 key 合并;② 剩余模糊对(归一后高相似)交 LLM 二次确认「这俩是同一实体吗 YES/NO」合并。合并后**重写所有场景的 referenced_*_ids 指向规范 id**(让桥接边算得准)。fail-closed:LLM 失败→只做字符串归一,不强合。

### 5.3 建边-双源
**① 实体桥接边(`module_graph_edges.rs`,纯函数,零 LLM)**
`bridge_edges(scenes) -> Vec<(from,to,shared_count)>`:两场景共享 ≥1 个 referenced 实体(npc/clue/location)→ 候选无向边,shared_count 为权重。转成 `ScenarioLink{to_node_id, reason="共享实体: …", link_type=Spatial, source_anchor=None}` 填进双方 links。纯、确定、可单测。
**进阶 LLM 线索边(Pass C;module_reader,先点后边 iText2KG)**
给定全场景节点列表(id+title+summary),prompt「找出每个场景内**指向另一场景的可发现线索/通路**(门/NPC 提示/物证/地图出口),每条给 from/to(必须是列表里的 id)/link_type/**source_anchor(原文片段)**」。submit schema `edges:[{from,to,link_type,reason,source_anchor}]`。**fail-closed 抗幻觉**:source_anchor 空或 to 不在节点列表→丢该边;绝不编造。
**合并**:两源 union by (from,to),LLM 边覆盖桥接边(精度优先,带 anchor),填进 `ScenarioNode.links`。

### 5.4 validator(`module_graph_validator.rs`,纯函数)
`validate_graph(graph) -> GraphHealth{reachable, islands, dead_ends, orphans, score, ok}`:
- 从 `module_entry_scene_id` BFS(沿 links)→ reachable 集;不可达场景=orphans。
- island=无入边、dead_end=无出边。
- score=reachable/总场景;按 spine 结构调期望(linear 期望近链、sandbox 期望网,阈值不同)。
- **质量门**:`ok=false`(score<阈值 或 orphans 过多)→ run_module_reader log warn + **触发一次额外 gleaning 重抽**(bounded:只重抽一次)。纯函数可单测。

### 5.5 数据模型(trpg-model)
`ScenarioLink` 加 `#[serde(default)] pub source_anchor: Option<String>`(LLM 线索边的原文锚点;桥接边为 None)。其余复用(to_node_id/reason/clue_id/link_type 已有)。

### 5.6 边的角色(理念)
边落 `ScenarioNode.links` → **`scene_node_to_blocks`(已投影 links 出口)自动让 GM 看到"可去之处"**;validator 用 links 查连通。**`scene_navigator` 不变**(全场景列表语义导航,边不硬约束)。fail-closed:links 空,导航照常。

## 6. 数据流

```
parse_module → run_module_reader:
  Pass A skeleton → gleaning loop(补全)→ dedup_entities(对齐 id)
  → bridge_edges(桥接)+ Pass C LLM 线索边(带 anchor)→ 合并 links
  → validate_graph(连通性;不 ok 则一次重 gleaning)
  → Pass B 深抽首场景 → 入库 ModuleGraph(now 连通+完整)
play: scene_node_to_blocks 投影当前场景 links 出口给 GM
```

## 7. 错误处理(全程 fail-closed)

- gleaning:回灌上轮 + max 轮兜底;解析失败用已得不崩。
- dedup:LLM 失败→只字符串归一;不确定不强合。
- LLM 线索边:无 source_anchor / to 不存在 → 丢边,绝不编造。
- validator:只报告 + 至多一次重抽;不阻断入库(连通差也照常出 graph,scene_navigator 兜底)。
- 桥接边/validator 纯函数,空输入→空/默认,不 panic。

## 8. 测试

- 纯函数 TDD:`bridge_edges`(共享实体→边、无共享→无边、权重)、`validate_graph`(孤岛/死胡同/不可达检测 + score + 结构感知阈值)、`sanitize_key`/`dedup_entities`(归一合并 + referenced_ids 重写)、gleaning 解析(YES/NO + 合并去重)、LLM 线索边 finalize(无 anchor/to 不存在→丢)。
- 集成 e2e:重抽 CoC/Vault → links 非空(桥接+LLM)、validator 报告连通性、source_anchor 有值;P4 出口投影 GM 可见。

## 9. 文件清单(各 ≤400 行)

- `trpg-model/src/lib.rs`:ScenarioLink + source_anchor。
- `trpg-rule-agent/src/reader/module_graph_edges.rs`(新,桥接边纯函数)、`module_graph_validator.rs`(新,validator 纯函数)。
- `trpg-rule-agent/src/reader/module_reader.rs` / `module_reader_loop.rs`:gleaning 回环、Pass C 线索边、dedup_entities、编排接入。
- `trpg-rule-agent/src/reader/mod.rs`:导出。
- runtime `scene_node_to_blocks` 已投影 links 出口,自动受益(若需展示 source_anchor 可小调)。

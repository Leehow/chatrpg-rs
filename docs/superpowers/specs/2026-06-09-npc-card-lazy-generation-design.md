# NPC 卡懒生成设计 v2 (lazy, persona-driven, per-parameter)

> 日期: 2026-06-09 · 状态: 经一轮对抗评审重写, 待 writing-plans · 主线: v1.20-formula
> 前置: 物化 (real_materialization_extractor_v1_10) + 参数 seed (trpg-params) + 模组 reader + 物品 smart-GM (§2b)
> v2 修订: 修掉评审 4 个 BLOCKER(strict-gate 矛盾 / 拆塌缩重构量 / target_refs 绑定不存在 / T2 是新增非复用),
> 用户拍板 "T3 默认开 + flagged 值驱动结算"(翻转 fail-closed 默认, 自觉接受), 并**分两期**落地。

## 1. 背景与缺口 (代码勘查实证)

进模组遇到 NPC, 按懒加载理念应即时给一张可用最小卡。现状:
- **触发懒但塌缩**: NPC 参数懒建(`ensure_actor_parameters`, params:34), 但**所有 NPC 塌缩成一个 id
  `npc.opposition`**(runtime:1519, combat:627, mechanics:530, contest:357, material:545, object — 共 **25 处/7 crate**);
  触发用关键词 `mentions_runtime_npc`(runtime:1705, 违背 §2 语义优先)。
- **缺源 = fail-closed 空壳**: 默认合成种子全关 → `unresolved_actor_parameters`(params:201): HP/stats 全 null,
  `not_mechanically_resolvable`; contest 缺源 DV 回 `Provisional`-null(contest:336)。**不是可用卡**。
- **模组 NPC 数据没接进 actor 物化**: `referenced_npc_ids`/`ModuleGraph.npcs`(untyped `Vec<Value>`)只投影成 GM
  prose(scene_node_to_blocks, runtime:1972), `ensure_actor_parameters` 从**玩家 template** 种、不读 `graph.npcs`。
- **物化能抽 NPC stat, 但"按人设兜底"缺失**: 6 段 kernel 有 actor 路径(`writeback_actor`, material:544;
  `NpcCard` 抽取器 schema), 能抽模组卡/bestiary。但**ladder 末档只有 provisional 占位, 没有"按人设现搓值"**。

目标: NPC 进场即时给**懒积累、人设驱动、按参数现搓**的可用卡。

## 2. 设计决策 (已逐条敲定)

1. **三层 hybrid (顺序铁律)**: T1 源真参数 > T2 规则书原型 > T3 按人设 GM 判。前档命中绝不用后档。
2. **两条懒轴**: 懒到 **NPC × 参数** —— 卡按 NPC 身份懒落、参数按事件懒搓、钉卡持久化、后续事件再补。
3. **T3 默认开 + flagged 值驱动结算 (用户 2026-06-09 拍板)**: NPC 缺源时按人设搓一个值, `status=provisional`
   + 审计, 且**真的参与检定结算**(不再静默挡)。这是对 `FAIL_ON_MISSING_SOURCE_BACKED_PARAMS=true` 默认的
   **自觉翻转**(仅对 NPC opposition 参数)。永远 flagged + 可被真源**升级**。可关 → 回退纯 fail-closed。
4. **拆塌缩** + 语义 target 解析: 替掉 `npc.opposition` 单槽 + `mentions_runtime_npc` 关键词门。**(Phase 2,
   见 §3 分期)**

## 3. 分期 (评审纠正: 全量太大, 拆两期, 高价值先落)

- **Phase 1 — 人设驱动的 per-参数合成 (核心价值, 不动大重构)**: 在**现有 opposition actor**上做 T1/T2/T3 +
  per-参数积累; 卡按**已解析的 NPC 身份**轻量缓存(同一 NPC 复用其参数; 切换目标按身份取/换)。交付
  "偷警察≠偷普通人"对**当前目标**成立 + 跨事件累积。**不需要** 25 处 de-collapse, **不需要** classifier
  schema 改动(用 target_refs 现有内容 + 当前场景 NPC 尽力解析)。
- **Phase 2 — 全量 per-NPC**: de-collapse `npc.opposition`→per-NPC id(7 crate 重构, 含战斗 frame 改多人) +
  结构化 `target_refs`(classifier schema/prompt 改, 仿 object_search_keys) + 同场多个 NPC 各自独立卡 +
  两套 HP 存储按 per-NPC id 对齐。

## 4. 核心模型: 懒积累的 NPC 卡 (两条懒轴)

NPC 卡起步≈空(只身份/人设), 被事件逐步点亮:
- **轴 A 按 NPC**: 事件引用到某 NPC → 给"这个 NPC"落一张持久卡。人设来源: `graph.npcs[id]` 的 name/summary/role;
  叙事临场引入(模组没有)的由 GM 给人设(smart-GM, §7.7)。Phase 1 用身份**缓存键**(解析出的 NPC ref),
  Phase 2 升级为独立 actor_id。
- **轴 B 按参数**: 规则要某值(偷窃→反侦察 DV; 战斗→命中/HP/防御)才搓那一个, 写卡; 同参数复用; 新事件补新参数。

**走查 (用户例子)**: 偷窃警察 → 解析目标 NPC「警察」(人设落卡) → 需 `反侦察 DV` → 卡无 → hybrid: 源无 →
原型(守卫)多半也无印好的对抗 DV(见 §5 诚实) → **T3 按人设判一个偏高值**, 钉卡(provisional+审计), **参与结算**。
下一幕开打 → 卡无 `命中/HP` → 现搓(源→原型→人设)补进同卡。普通人 NPC 同流程, 人设低 → 反侦察天然低。

存储: `runtime_actor_parameters`(复用, `actor_kind=npc`); 已点亮参数累积在 `sheet_json`(单一真相),
`mechanical_profile` 是其物化视图(`refresh_mechanical_profile` 重投影)。**per-参数 provenance 存 `sheet_json`
里(不是 mechanical_profile 视图, 否则被 refresh 冲掉, 见 §7.6)**。

## 5. 每参数合成: hybrid 三档 (诚实标注现实命中率)

1. **T1 源 (复用物化)**: 抽取器去"模组 NPC/stat 卡 → 规则书 bestiary 精确条目"找该 NPC 真值。命中→真值,
   `source_backed`。**走现有物化 6 段 kernel + strict writeback 门**。
2. **T2 原型 (新增, 非复用 — 评审纠正)**: 代码里**今天没有** archetype/mook 检索 rung(只是 ladder 文档一句话)。
   本期**新建**: 人设→原型名的**语义映射**(警察→"守卫/受训", 平民→"commoner") + 去规则书取该原型该参数的源行。
   命中→`source_backed_archetype`。**诚实**: 对**对抗检定 DV**(反侦察那类), 规则书多半**不印**(那是对方
   Stealth/察觉的对抗, 非印好的标量)→ T2 多半 whiff → 落 T3。T2 主要救**印好的标量**(HP/怪物 stat block)。
3. **T3 人设判 (新增, 唯一理念敏感)**: 前两档空 → GM 按 `(人设 + 这次检定语义 + 规则书语境)` 判**单参数**值,
   `status=provisional` + 审计(谁判/哪段人设/哪次检定/可升级)。**T3 是对抗 DV 类的主力, 不是罕见兜底(诚实)。**
   **写路径独立, 绕开 strict materialization 门**(strict 只放 VerifiedExact; T3 非 verified → 走专用 provisional
   写入, 不经 `write_back`)。**升级**: 后续 T1/T2 命中更高优先级来源 → 覆盖升级(provisional→source_backed);
   升级在**场景/回合边界**生效, **不回溯改写已结算的过去检定**(§7.8 公平性)。

绝不复活硬编码 `label→数值` 表(boss=35 / 假 DV 50, 项目已删)。区分: **禁止**=套标签编平衡数; **允许**=人设
落地 + flagged provisional + 可审计 + 可升级 的语义判断。

## 6. 关键集成 (按分期)

### Phase 1
- **per-参数触发**: contest/combat 要某参数且卡上没有 → 调合成(T1→T2→T3)写卡再读。**T3 作为"结算前预 pass"**
  跑(写好值, 让 contest 读), **不**把 LLM 调用塞进 `trpg-contest`(它无 LLM 依赖、纯同步 — 评审纠正)。
- **人设接线**: 落 NPC 卡时读 `graph.npcs[id]` 人设注入(现在不读)。
- **身份缓存**: 用解析出的 NPC ref 作缓存键, 同 NPC 复用已点亮参数; 仍写现有 opposition actor 行(Phase 1 不动 id)。
- **NpcCard 专路**: NPC 物化时设 `material_target_kind="npc_stat_block"`(现 dormant、无 caller 设它), 让搜索
  优先模组卡。(注: NpcCard 与 ActorProfile 抽取 schema 相同, 差别只在搜索 query plan, 不夸大。)

### Phase 2
- **de-collapse**(评审纠正: 跨 7 crate/25 处, 非一处): `create_frame` 双人结构改多 defender/多 initiative;
  `record_attack` 默认 target、`defender_for_contract` 兜底、`writeback_actor` 默认 id 全部吃**解析出的 per-NPC id**;
  保留单 opposition 作"未指名敌对方"退化默认。
- **结构化 `target_refs`**(评审纠正: 现为无结构 `Vec<String>`, classifier prompt 没说填啥): 加 classifier
  schema/prompt(仿 `object_search_keys`), 使 target_refs 带可解析到 `graph.npcs` id 的信息, 区分 cop vs civilian。
- **两套 HP 存储对齐**(评审): `runtime_actor_parameters` ↔ `actor_mechanical_states`(`actor_hp_from_params`
  按 actor_id 桥)在 per-NPC id 下必须一致, 否则战斗 HP 与参数 HP 静默脱钩。
- **退役 `mentions_runtime_npc`**: **在语义 target 解析证明可靠之后**(评审 E-2: 先退役会导致非战斗场景卡建不出来;
  保留关键词触发作 fallback 直到语义解析可靠)。

## 7. 不变量 / 护栏

1. **顺序铁律**: T1 > T2 > T3。能 source/archetype 就绝不 persona-judge。
2. **T3 永远 flagged + 可审计 + 可升级**: `status=provisional` + provenance; 可被更高优先级来源覆盖升级; 不降级。
3. **不复活硬编码捏造**: 无 Rust `label→数值` 表; 无假平衡默认。
4. **语义而非关键词**: Phase 2 身份/target 走 classifier(非 `mentions_runtime_npc` 扫词)。(诚实: 消费侧
   `defender_for_contract`/`is_attack_contract` 仍含关键词启发, 本期不全量语义化, 标记为后续。)
5. **fail-closed 可回退**: T3 是**可配置门**(`TRPG_NPC_PERSONA_SYNTHESIS`); 关掉→回退现有 unresolved 空壳。
   **默认开**(用户拍板); strict materialization 门对 T1/T2 writeback 仍生效, **T3 走独立 provisional 写路径绕开它**。
6. **single source of truth + provenance 不丢**: 卡存 `runtime_actor_parameters`; `mechanical_profile` 是
   `sheet_json` 视图(`refresh_mechanical_profile` 会整体重投影)→ **per-参数 provenance 必须存 `sheet_json`**(refresh
   的来源), 不存 mechanical_profile(会被冲)。Phase 2 还须覆盖 `actor_mechanical_states` 桥按 per-NPC id 一致。
7. **NPC 版 smart-GM 三分支**(对齐物品 §2b): 临场引入、模组没有的 NPC → ① 模糊/没人设→GM 问/补; ② 合理→套相似
   原型; ③ 超模/不合 genre→GM 劝退 + 可强用但 flagged。
8. **升级不回溯**: provisional→source 升级在场景/回合边界, 不改写已结算的过去检定(公平性, 评审 E-5)。
9. **文件 ≤ ~400 行**: 拆小模块。

## 8. 范围

**Phase 1 内**: per-参数 T1/T2/T3 合成(T3 默认开+驱动结算+独立写路径)、人设接线、身份缓存、NpcCard 专路点亮、
provenance(存 sheet_json)、升级(边界生效不回溯)、NPC smart-GM 三分支(prompt)、T2 archetype 检索(新建)。
**Phase 2 内**: de-collapse(7 crate)、结构化 target_refs(classifier 改)、同场多 NPC 独立卡、两套 HP 对齐、
退役关键词门。
**两期皆外**: typed `CreatureDefinition` 模型(仍用 ActorProfile/Value)、NPC→`loot_contents` 桥、模组卡 OCR、
NPC 自身成长(用 Track 层)、消费侧 `defender_for_contract` 全量语义化。

## 9. 组件落点 (每文件小)

| 模块 | Phase | 改动 |
|---|---|---|
| (新) persona-judge slice | 1 | `synthesize_npc_parameter`(LLM, 单参数, provisional+审计, **独立写路径绕 strict**); 作结算前预 pass |
| (新) archetype 检索 | 1 | 人设→原型名语义映射 + 取原型该参数源行(诚实: 对抗 DV 多 whiff) |
| `trpg-runtime/src/lib.rs` | 1 | per-参数"缺则合成"触发; 人设从 graph.npcs 注入; 身份缓存 |
| `trpg-material/src/lib.rs` | 1 | NPC 物化设 `material_target_kind="npc_stat_block"` 点亮 NpcCard 专路 |
| `trpg-cli/src/main.rs` | 1 | GM prompt 加 NPC 策略(对齐 ITEM POLICY); `TRPG_NPC_PERSONA_SYNTHESIS` 门(默认开) |
| classifier (trpg-semantics) | 2 | `target_refs` schema/prompt(仿 object_search_keys), 带 NPC-id 可解析信息 |
| `trpg-contest`/`combat`/`mechanics` | 2 | de-collapse: 吃 per-NPC id(frame 多人/attack/defender/writeback 默认); HP 双存储对齐 |
| `trpg-runtime` | 2 | `resolve_npc_actor`(target_ref→per-NPC id); 退役 `mentions_runtime_npc`(语义可靠后) |

## 10. 风险 / 开放问题

1. **T2 命中率低(已诚实)**: 对抗检定 DV 规则书多不印 → T3 是主力。可接受(用户要 persona 差异), 但"source-informed"
   主要适用于印好的标量(HP/stat block)。
2. **Phase 1 跨场景一致性**: 身份缓存按 NPC 实例; 同类 NPC(一城所有警察)跨场景不共享 → 可能漂移。Phase 1 接受,
   "同类共享原型缓存"留后续(诚实: Phase 1 会产生它想避免的不一致, 标记之)。
3. **target_refs 可靠性(Phase 2)**: classifier 区分 cop vs civilian 并绑 graph.npcs id 的稳定性; 匹配不到的退化。
4. **升级时机**: 场景/回合边界生效、不回溯(§7.8); 背景深抽到模组卡后的覆盖路径。

## 11. 验证矩阵

**Phase 1**: ① T1 源命中(模组带真 stat)→用真值。② T2 原型(规则书有"守卫"且印了相关标量)→取原型值。
③ **T3 人设判(对抗 DV)**: 偷警察→反侦察高(provisional+审计+**参与结算**), 偷普通人→低。④ 两条懒轴: 同 NPC
先偷窃(点亮反侦察)再战斗(补命中/HP)同卡累积, 反侦察复用不重搓。⑤ T3 独立写路径: strict 门开时 T3 值仍落卡
(不被 strict 吃掉)。⑥ `TRPG_NPC_PERSONA_SYNTHESIS=off`→回退 unresolved 空壳。⑦ 升级: T3 provisional 被后续真源
覆盖, 且不回溯改已结算检定。
**Phase 2**: ⑧ 同场「警察」「平民」各独立卡, target 解析按语义打对人。⑨ 战斗 frame 多 defender。⑩ 两套 HP 按
per-NPC id 一致(战斗扣血与参数 HP 不脱钩)。
**不变量**: 无 `if ruleset`; 无硬编码 label 表; T3 永远 flagged; provenance 存 sheet_json 不被 refresh 冲。

## 12. 修订记录
- v1: 初稿(两条懒轴 + hybrid 三档 + 拆塌缩)。
- v2 (对抗评审后): T3 默认开+驱动结算(用户拍板, 翻转 fail-closed 自觉接受); 修 strict-gate 矛盾(T3 独立写路径);
  T2 标新增非复用 + 诚实命中率; T3 作结算前预 pass(非 contest 内 LLM); **分两期**(Phase 1 不动 de-collapse,
  Phase 2 才做 25 处重构 + 结构化 target_refs); 迁移顺序(语义可靠后再退役关键词门); 两套 HP 对齐; provenance
  存 sheet_json 防 refresh 冲; 升级不回溯。

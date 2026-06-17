# 模组引导事实(facilitation facts)自动抽取 — 设计

> 状态: APPROVED(架构经用户/架构师口头批准 2026-06-17)
> 退役目标: 让 `{id}.module_config.json` 的 `director` 块从 **REQUIRED** 降为 **OPTIONAL override**。

## 1. 问题

`DirectorModuleConfig`(scene_facts / pressure_items / affordance_items / risk_items /
npc_advice / known_facts / open_question(+points_to) / place_summary_fallback)挂在
`ModuleConfig.director`(`crates/trpg-model/src/lib.rs:3821-3850`),它替换了 trpg-director 旧的
`is_homecoming()` 硬编码。但这些值**目前只能**由手写的 `{id}.module_config.json` sidecar 注入:

- `Db::load_module_config(module_id)`(`crates/trpg-db/src/lib.rs:732`)→ `read_module_config_file`
  (`:4318`)只读 sidecar 文件(`{TRPG_DATA_DIR}/modules/{id}.module_config.json`)/ embedded 兜底。
- 模组解析产物 `ModuleGraph`(`trpg-model:1721`,存 `parsed_bundles.content_json->'module_graph'`)
  **没有模组级引导事实容器**——只有 scene 级 prose(`read_aloud`/`gm_notes`)。

后果:每个新模组都要手写一份 config,否则 director 退化到通用兜底。`DirectorModuleConfig` 的文档注释
(DONE_WITH_CONCERNS)已写明"未来 module-reader pass 应从解析后的 scene 数据自动抽取并退役 override"。

## 2. 目标 / 非目标

**目标**
1. module reader 在抽取期从解析后的 scene/skeleton 数据**自动抽取**引导事实,填充 `DirectorModuleConfig`,
   存进模组 bundle。
2. director 从自动抽取的 config 读取;sidecar override 路径保留为**最高优先级、可选**(override > extracted)。
3. `{id}.module_config.json` 变为可选。
4. 数据化、fail-closed(缺的事实省略,不编造);跨 sandbox/quest-set/linear 三种结构通用,零 per-module 硬编码。

**非目标(YAGNI)**
- 不做 per-scene 随进度变化的引导事实(本任务是退役**模组级** sidecar;per-scene 是另一个 scope)。
- 不碰 ModuleConfig 的非 director 字段(npc_actor_bindings / technical_option_table /
  scene_entity_aliases / module_search_profile)——它们仍只来自 sidecar。
- 不做字段级混合合并(见 §4 合并粒度)。

## 3. 架构

三个改动点 + director 消费侧零改动。

### 3.1 存储:`ModuleGraph` 上加模组级容器
```rust
// crates/trpg-model/src/lib.rs  ModuleGraph
#[serde(default)]
pub director_facilitation: Option<DirectorModuleConfig>,
```
`#[serde(default)]` → 旧 bundle 反序列化为 `None`,向后兼容。这是模组解析数据的**单一事实源**,经现成的
`Db::load_module_graph(module_id)`(`trpg-db:461`,读 `content_json->'module_graph'`)即可读到,**无需新增
DB 列/迁移**。同步在 `ModuleReadout`(reader 中间产物)加同名字段,解析装配时透传到 `ModuleGraph`。

### 3.2 抽取:reader Pass B 之后跑引导抽取
新文件 `crates/trpg-rule-agent/src/reader/facilitation.rs`:
```rust
pub async fn extract_facilitation_facts(
    client: &dyn LlmClient,
    ctx: &ModuleReaderCtx<'_>,
    readout: &ModuleReadout,
    budget: usize,
) -> Option<DirectorModuleConfig>
```
- **复刻 `oneshot_deep_extract` 的 one-shot submit 契约**(`module_reader_loop.rs:187-240` 模式):
  构造一个 `submit_facilitation` 工具,schema **镜像 `DirectorModuleConfig`**(见 §5 字段映射),
  单次 `complete_with_tools` 调用,解析 submit 参数。
- **输入**:入口场景(`readout.scenes[entry]`)的 `read_aloud` + `gm_notes` + `summary`、
  被引用 NPC(`readout.npcs` 按 `referenced_npc_ids` 过滤,供 npc_advice 偏见人设)、`spine` 总览。
- **fail-closed**:prompt 明确"只抽文本里明写/强烈隐含的;没有就留空,绝不编造";整步失败/无工具调用 → 返回 `None`。
- **env 门** `TRPG_MODULE_FACILITATION` 默认开(`0/false/off/no` 关),与 reader 其它门一致。
- 在 `run_module_reader`(`module_reader.rs`,Pass B `deep_extract_scene_in_place` 之后、graph 收尾之前)
  调用,结果写入 `readout.facilitation_facts`。

### 3.3 合并:`load_module_config` = `extracted ⊕ override`(override 胜)
合并逻辑抽成**纯函数**(便于确定性单测):
```rust
// crates/trpg-db/src/lib.rs  (pure, 可单测)
fn merge_module_config(
    sidecar: Option<ModuleConfig>,
    extracted: Option<DirectorModuleConfig>,
) -> Option<ModuleConfig> {
    match (sidecar, extracted) {
        (Some(mut cfg), ext) => {
            if cfg.director.is_none() { cfg.director = ext; } // override 胜:仅当无 override 才用 extracted
            Some(cfg)
        }
        (None, Some(ext)) => Some(ModuleConfig { director: Some(ext), ..Default::default() }),
        (None, None) => None,
    }
}

pub async fn load_module_config(&self, module_id: &str) -> Option<ModuleConfig> {
    let extracted = self.load_module_graph(module_id).await.ok()
        .flatten().and_then(|g| g.director_facilitation);
    merge_module_config(read_module_config_file(module_id), extracted)
}
```
- **director 字段整体优先级**:sidecar.director 存在则用 sidecar,否则用 extracted。
- ModuleConfig 其它字段仍只来自 sidecar(本任务不碰)。
- director 消费侧(`trpg-director` `build_brief`/`biased_npc_advice`/`current_place_summary`)**零改动**——
  照旧读 `ModuleConfig.director`,只是现在没 sidecar 时也能拿到值。

> **embedded 兜底坑(影响验收)**:`read_module_config_file`(`:4318`)文件不存在时会
> `embedded_module_config` 兜底——homecoming / masks / the_vault 三个**有 embedded 副本**。
> 所以"移走 filesystem sidecar"**不**会让这三个露出 extracted(embedded 仍 shadow)。本任务**不动
> embedded 兜底**(那是这三个 shipped 模组的开箱保障,且正好验证"override 胜")。验收见 §6。

## 4. 已拍的非显然决定
- **粒度=模组级、锚定开场**:懒抽取下解析时只有入口场景被深抽,引导事实自然锚定"开场情境 + 模组总览"——
  正好等价于 homecoming 那份手写 config(它本来就是开场)。不做 per-scene。
- **合并粒度=整块 director 优先**:有 sidecar.director 就整份盖掉 extracted(非字段级混合)。现有三份 config
  都是全字段,行为等价,且最不易出错。

## 5. 字段映射(submit schema → DirectorModuleConfig)
| schema 字段 | 类型 | → DirectorModuleConfig |
|---|---|---|
| `scene_facts[]` | `{text, source?}` | `Vec<DirectorSceneFact>` |
| `pressure_items[]` | `{text, clock_id?, severity?, consequence_hint?}` | `Vec<DirectorPressureItem>` |
| `affordance_items[]` | `{description, implies_vectors[]?}` | `Vec<DirectorAffordanceItem>` |
| `risk_items[]` | `{text, related_vectors[]?, severity?}` | `Vec<DirectorRiskItem>` |
| `npc_advice[]` | `{npc_id, speaker_label, advice_text, bias_or_goal, not_official_solution}` | `Vec<NpcBiasedAdvice>` |
| `known_facts[]` | `string` | `Vec<String>` |
| `open_question` | `string?` | `Option<String>` |
| `open_question_points_to[]` | `string` | `Vec<String>` |
| `place_summary_fallback` | `string?` | `Option<String>` |

`implies_vectors`/`related_vectors` 是 `ActionVector` serde 名;消费侧 `parse_vectors` 已 fail-soft
(未知名丢弃、空则用中性默认),抽取侧不强约束枚举值。

## 6. 测试 / 验收
1. **抽取(单测,trpg-rule-agent)**:用 fake LlmClient 喂一份带 read_aloud/gm_notes 的 readout,
   断言 `extract_facilitation_facts` 解析出非空 scene_facts/pressure;喂空响应 → `None`(fail-closed)。
2. **合并(纯函数单测,trpg-db)**:`merge_module_config` 三态——extracted-only → director=extracted;
   有 sidecar.director → sidecar 胜;都无 → None。这是验收(2) override 优先的确定性证明。
3. **序列化兼容(单测,trpg-model)**:旧 ModuleGraph JSON(无 director_facilitation)反序列化为 None。
4. **真库 e2e(验收 1)— 用无 config 模组**:取一个**既无 filesystem sidecar 也无 embedded** 的真实模组
   (CoC sandbox「血色公路」是理想目标——extraction 是 director 唯一来源)重解析 → `load_module_config`
   返回非空 director,scene_facts/pressure/affordances 来自 extraction。覆盖 sandbox 结构。
5. **真库 e2e(验收 2+3)**:① homecoming(线性 CPR,**有 embedded config**)重解析后 `load_module_config`
   仍返回 sidecar 值(override/embedded 胜,证 extraction 不抢占);② 再取一个 quest-set 结构模组重解析验
   extraction 路径。三结构(sandbox / quest-set / linear)各跑一遍,零 per-module 硬编码。
6. `cargo build` + `cargo test` 全绿;新文件 ≤400 行。

> 注:验收原文「移走 hand-authored config 后仍非空」对**有 embedded 的三个模组**不可直接做(embedded shadow);
> 故拆成「无 config 模组证 extraction 独立可用(§6.4)」+「有 config 模组证 override 胜(§6.5)」,
> 二者合起来精确满足验收 1+2 的**意图**,且不破坏 shipped 模组的 embedded 开箱保障。

## 7. 影响面 / 风险
- `load_module_config` 现在多一次 `load_module_graph` DB 查询(原本纯文件读)。它非热点(per-turn 由
  runtime/combat/material 调用),成本可接受;若后续成瓶颈再缓存。
- 抽取是 additive、fail-closed:抽取失败 → director_facilitation=None → 行为退回今天的"无 sidecar 即通用兜底",
  零行为倒退。
- 文件行数:facilitation.rs 控制在 ~250 行以内;trpg-model/trpg-db/module_reader.rs 仅小增。

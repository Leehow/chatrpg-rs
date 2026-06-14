# R2 设计：统一 Need 总线（回合内数据获取收口到 typed Need + NeedBus）

日期：2026-06-14
状态：设计已获架构师拍板（全 Need 总线 B / 分阶段安全落地 / 落新 crate trpg-need / 同步解析 / enum+dyn NeedResolver）
前置：R1（agent 路径升默认 + 数据化 execute_turn，已合 main `8c69ba9`，见 `2026-06-14-r1-agent-path-default-design.md`）。本期 = 外部评审 §2-C/§4-隐患1 的收敛：回合内"数据获取"从各自直连 service/search 改为发 typed Need 经统一 NeedBus 路由到 resolver。

## 1. 背景：回合内数据获取分散、规则检索绕过 steward

R1 后回合上下文装配在 `RuntimeEngine::prepare_turn_context`（trpg-runtime/src/lib.rs:175），它调 ~15 个 `*_blocks_for_turn` 方法拼 context blocks。这些方法分两类：

- **数据获取类**（向 search/service/DB 取外部数据）：`auto_search_blocks_for_turn`(:1304，直连 `search.search_async`，**绕过 RuleSteward**)、`rule_steward_prefix_blocks_for_turn`(:1428)、`learned_packet_blocks_for_turn`(:1418)、`module_scene_blocks_for_turn`(:1528)、`materialization_blocks_for_turn`(:1831)、`actor_parameter_blocks_for_turn`(:1777)、NPC 现搓(`npc_synth`/`ensure_npc_parameter`)。
- **状态投影类**（渲染 kernel 已算出的状态，不取外部数据）：`ability_blocks_for_turn`/`rule_binding_blocks_for_turn`/`world_time_blocks_for_turn`/`object_blocks_for_turn`/`state_frame_blocks_for_turn`/`player_value_referee_blocks_for_turn`/`referee_combat_blocks_for_turn`/`contest_blocks_for_turn`。

问题（§2-C/§4-隐患1）：**获取类各自直连**，无统一总线；规则检索绕过 `RuleStewardAgent::assist`（已查实 assist 是无 LLM 的确定性更聪明检索：learned 匹配 + locator/rg 兜底 + source_refs 接地）；其余 Need 类型（Material/Scene/Entity/Parameter）概念上存在但无统一表达。

### 1.1 范围精化（比 brainstorm 概览更准）
Need 总线**只收口"获取类"**。"投影类"是 kernel 计算输出、非"需求"，**不进总线、保持不动**。即"所有 kernel 发 Need"精化为"所有回合内数据获取经 Need"。

## 2. 已拍板决策

| 决策点 | 结论 |
|---|---|
| 范围 | **全 Need 总线 B**：5 种 typed Need（Rule/Material/Scene/Entity/Parameter）统一总线，回合内获取类全收口；各 service 退成 resolver 内部纯 retriever |
| 落点 | **新 crate `trpg-need`**（低层，仅依赖 trpg-model）放 Need enum + NeedResolver trait + NeedBus；**resolver 落 trpg-runtime**（已依赖各 service + trpg-need，最小新 crate 足迹）|
| 抽象 | **enum Need + dyn NeedResolver 注册表**；NeedBus 持 `Vec<Box<dyn NeedResolver>>`，按 kind 路由 |
| 解析时机 | **同步**（context 必须在 LLM 回合前就绪，对齐现 auto_search 同步语义；后台/分层是 R5，本期不碰）|
| 落地方式 | **分阶段**（哪怕"一次建"也分阶段验证、旧路径留到新路径验过——execution 纪律同 R1）|

## 3. 目标 / 非目标

**目标**：回合内 5 类数据获取（规则/材料/场景/实体/参数）统一为 `bus.emit(Need)` + `bus.resolve_all()`；RuleSteward 成唯一 rule authority（规则检索经 assist，得 source_refs 接地增益）；SearchService 退成 resolver 内部纯 retriever，kernel 不再直连；全程零规则集硬编码、fail-closed、SSE 流式 + 缓存稳定不破；context 块与迁移前等价（内容/hash 可对比验证）。

**非目标（本期）**：投影类 block builder 改造（它们渲染状态非获取数据，保持不动）；R5 postprocess 临界/重活分层与后台解析（本期解析保持同步）；MemoryNeed（`memory_blocks_for_turn` 也是获取，但本期 5 类外，总线**可扩展**留后续）；退役 R1 保留的 run_gm_turn。

## 4. 设计

### 4.1 核心抽象（trpg-need crate）
```
pub enum Need { Rule(RuleNeed), Material(MaterialNeed), Scene(SceneNeed), Entity(EntityNeed), Parameter(ParameterNeed) }
// 各 payload 带统一 scopes（ruleset/module/session/scene/turn_id）+ kind 专属字段
pub struct NeedOutcome { pub blocks: Vec<ContextBlock>, pub state_patches: Vec<StatePatch>, pub source_refs: Vec<SourceRef> }

#[async_trait] pub trait NeedResolver: Send + Sync {
    fn handles(&self, need: &Need) -> bool;           // 按 kind 认领
    async fn resolve(&self, need: &Need) -> anyhow::Result<NeedOutcome>;
}

pub struct NeedBus { resolvers: Vec<Box<dyn NeedResolver>>, pending: Vec<Need> }
impl NeedBus {
    pub fn register(&mut self, r: Box<dyn NeedResolver>);
    pub fn emit(&mut self, need: Need);                // 收集
    pub async fn resolve_all(&mut self) -> Vec<NeedOutcome>; // 路由→resolver，fail-closed：未认领/失败→warn+空，不中断
}
```
- `trpg-need` 仅依赖 trpg-model（ContextBlock/SourceRef/scopes），低层无环。
- `RuleNeed` 复用现有 `trpg_rule_agent::RuleNeed`（已存在）——trpg-need 的 `Need::Rule` 包它，或 re-export 对齐（实施时定，避免双定义）。

### 4.2 5 个 resolver（trpg-runtime，包既有 service 为纯 retriever，零检索逻辑重写）
| Need | resolver 包的现有 service / 方法 | 替代的现直连 |
|---|---|---|
| Rule | `RuleStewardAgent::assist`（无 LLM）| auto_search_blocks_for_turn + rule_steward_prefix + learned_packet |
| Material | `MaterializationService` | materialization_blocks_for_turn |
| Scene | `module_scene_blocks_for_turn`/scene_navigation（R1 已下沉 runtime）| module_scene_blocks_for_turn |
| Entity | `npc_synth`/`ensure_npc_parameter` | NPC 现搓直连 |
| Parameter | `RuntimeParameterService`/param facets | actor_parameter_blocks_for_turn |

每 resolver 是薄 adapter：把现有 service 调用包进 `resolve(Need)->NeedOutcome`，不改 service 内部检索/合成逻辑。

### 4.3 回合内接入（耦合 R1 的 phase_context_assembly）
`prepare_turn_context` 的**获取类**块装配改为：构造对应 Need → `bus.emit()` → `bus.resolve_all()` → 把 NeedOutcome.blocks 并入 context。**投影类**块装配（ability/rule_binding/world_time/object/frame/referee/contest）**原样保留**。SearchService 不再被 kernel 直连，只在 RuleNeedResolver 内部经 steward.assist 触达。同步解析（resolve_all 在 context 装配内 await 完成，块就绪后才进 LLM 回合）。

### 4.4 分阶段（安全落地，旧路径留 env fallback 到新路径验过）
1. 建 `trpg-need`（Need/NeedResolver/NeedBus）+ 单测（emit/route/fail-closed/未认领）。
2. **RuleNeed resolver 先行**：phase_context_assembly 规则检索改走 bus→RuleNeedResolver→steward.assist；旧 auto_search 留 env `TRPG_NEED_BUS_RULE` fallback，真库 e2e 对比 context 块等价（+ 验 source_refs 接地增益）。
3. 逐个迁 Scene/Material/Parameter/Entity resolver（每个：包 service→resolver→该获取点改发 Need→对比验证→移除旧直连）。
4. 全 5 收口后删获取类直连调用，SearchService/各 service 仅经 resolver 触达；移除 env fallback。
5. 验证闸：真库回合 context 块与迁移前等价（内容/hash 对比）+ 两 transport(CLI&API)真库 live + 零回归 + 缓存稳定（只改输入只动 dynamic_hash）。

## 5. 理念守卫（MUST）
1. **数据驱动**：Need 是 typed 数据，bus 路由非硬编码分支；
2. **零规则集硬编码**：Need/resolver 通用，grep 不得出现规则集名于总线逻辑；
3. **AI 决策/Rust 执行边界**：Need 由回合管线（Rust）按确定性逻辑发，不让 AI 绕过；
4. **fail-closed**：resolver 失败/未认领 → warn + 空 outcome，不中断回合（降级少块，不崩）；
5. **SSE 流式 + 缓存稳定**：同步解析在 context 装配内完成，BP1-3 布局/hash 语义不破（块内容等价是验证闸硬指标）。

## 6. 测试与验收
**单测**：① NeedBus emit/resolve_all/路由/fail-closed（未认领 kind→空、resolver panic→隔离）；② 每 resolver 包装等价（RuleNeedResolver.resolve == 旧 auto_search 块集 + 接地）；③ 五 Need payload 构造 + scopes 透传。
**等价验证（核心闸）**：真库同一回合，迁移前后 context 块**内容等价**（逐 resolver 迁移时 A/B 对比，env fallback 红绿）；规则检索增益（source_refs 非空）可断言。
**e2e**：真库两 transport（CLI `trpg turn` + API SSE）跑通真回合，context 块经 bus 装配、零回归、缓存锚稳定（对齐 R1 gate 手法）。
**工程**：文件 ≤400 行；trpg-need 新结构 #[serde(default)]；零 per-ruleset 硬编码。

## 7. 风险
1. **跨多 crate + 回归面=回合上下文**（高）——分阶段 + 每 resolver A/B 等价验证 + env fallback 兜底 + 全验证闸把关；
2. **context 块等价漂移**（resolver 包装与旧直连产块不一致 → 改变 LLM 上下文/缓存）——逐 resolver 块内容/hash 对比为硬闸，不等价不移除旧路径；
3. **RuleNeed 双定义**（trpg-need vs trpg-rule-agent 已有 RuleNeed）——实施时 re-export/复用单一定义，不双定义；
4. **解析延迟**（同步 resolve_all 串行多 Need）——resolver 各自轻量（assist 无 LLM、其余沿用现 service 成本），可并发 resolve 同批 Need（实施可选 join_all），不引入新 LLM 调用；
5. **依赖环**（resolver 在 runtime 包各 service，trpg-need 仅依赖 model）——方向同 R1 验证模式，无环。

## 8. 工作量与切分
约 3-5 天（cross-crate）。实施序（同一分支，最后过验证闸）：
1. trpg-need crate（Need/NeedResolver/NeedBus + 单测）。
2. RuleNeedResolver + phase_context_assembly 规则检索接入 + env fallback + A/B 等价验证。
3. Scene/Material/Parameter/Entity resolver 逐个迁 + 各自 A/B 等价。
4. 收口删直连 + 移 fallback。
5. 全验证闸（等价对比 + 两 transport 真库 live + 零回归 + 缓存稳定）→ 合并 main。

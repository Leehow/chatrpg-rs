# 设计：Asset/Facet/Binding 契约 + 影子 BindingResolver（优化2 路线 #2+#3，非侵入起步）

日期：2026-06-17
状态：设计（自主执行，守 [[宪法]] `docs/architecture/source-grounded-json-runtime.md`）
前置：R1/R2/R5/P0-2 + 可观测性切片（commit 7c97bb6）已合 main。本切片 = 优化2 #2（AssetEnvelope+Facet）+ #3（BindingResolver+CapabilityRegistry）的**co-design + 影子起步**。

## 1. 背景 / 为何合并 + 影子
优化2 §2-4：解析产 source-backed JSON asset（Envelope+Facets），运行时 BindingResolver 晚绑定到少数 Rust capability，分 ExecutionTier，特殊机制 guided 降级。当前（Explore 实测）：facet 雏形散落（`ParameterFacetKind`@5946 + `ParameterFacetBinding`@6012 + `BindingVerificationStatus`@6216 + `BindingStatus`@5713），**无统一 AssetEnvelope、无 BindingResolver/CapabilityRegistry、无统一 ExecutionTier**。
- **合并 co-design**：AssetEnvelope/Facet 的形状应由 BindingResolver 的消费需求驱动，分开做易定错形。
- **影子起步（关键安全决策）**：BindingResolver 本切片**只产出 BindingPlan 记进 Flight Recorder（binding_trace），不改任何实际结算**（advisory/shadow）。理由：① 给新契约类型真实消费者（每回合真跑、非死代码）；② 真回合上**先观察** resolver 会怎么绑，takeover 前验证；③ 零行为变更 = 无等价风险、不碰核心 contest/check 路径（那需架构师审）。呼应宪法"先新增 asset view、旧路径继续工作"。

## 2. 已拍板决策（自主，守理念）
| 决策点 | 结论 |
|---|---|
| AssetEnvelope | 新 `trpg-model::asset` 模块：`AssetEnvelope{asset_id,asset_kind,source_refs,visibility,confidence,lifecycle,facets,data}`；visibility **复用** `Visibility`(@199)，不新造 |
| Facet | `AssetFacet{facet_kind:String, binding_candidates:Vec<String>(capability id), confidence:f32, source_refs}`；facet_kind 用字符串(宽松,先不锁枚举,呼应 Envelope 稳/Data 宽)，与现有 ParameterFacetKind 经字符串对齐 |
| ExecutionTier | 新枚举 `ExecutionTier{SourceOnly,GuidedRuling,PartialExecution,ExactExecution,VerifiedExecution}`(统一,替散落 tier 概念的**新主线**;旧 RulingConfidence/BindingVerificationStatus 保留不动) |
| BindingPlan | `BindingPlan{binding_id,need_kind,asset_ids,capability:Option<String>,execution_tier,verdict:BindingVerdict,confidence,source_refs,unresolved_reason:Option<String>}` |
| Verdict | `BindingVerdict{Exact,Partial,Guided,SourceOnly,Unsupported}` |
| Capability | `CapabilityId` 常量集(roll.dice/check.meet_or_beat/check.roll_under/check.count_faces/check.opposed/resource.delta/condition.apply/table.lookup/actor.patch/visibility.reveal/clock.tick) + `CapabilityRegistry`(注册的 id 集 + `resolve(id)->bool`) |
| BindingResolver | `trpg-runtime::binding` 模块(宪法:先放 runtime 不开新 crate)；`resolve_binding(need_kind, facets, registry)->BindingPlan` 纯函数:facet.binding_candidates 命中 registry → Exact/Partial(部分候选命中)；有 source_refs 无 capability → Guided；只 source 无 facet → SourceOnly；无任何 → Unsupported。**纯逻辑、确定性、不执行** |
| 影子记录 | `TurnTrace`(可观测切片已建)加 `binding_trace: Vec<BindingPlan>`(serde default);run_pipeline context 装配后跑 resolver over 本回合 needs/facets → BindingPlans → 写 trace。**advisory,不改结算** |
| explain | `trpg explain` dump 增 binding_trace(verdict/capability/tier/need_kind) |

## 3. 目标 / 非目标
**目标**：统一 AssetEnvelope/AssetFacet/ExecutionTier 契约落地（serde,向后兼容）；CapabilityRegistry + 确定性 BindingResolver(5 verdict)；每回合**影子**产 BindingPlan 记进 Flight Recorder + explain 可见；**零行为变更**(成功/失败回合事件序列、机械结算全不变)；契约稳定(同输入同 BindingPlan,呼应优化2 §12 Contract 3)；文件 ≤400。

**非目标(本切片)**：BindingResolver **接管实际结算**(check/contest/effect 仍走现路径——takeover 是后续、需架构师审 + 等价 + live e2e)；parser 输出改成 AssetEnvelope(本切片只 adapter/view + 影子,不动 parser 存储)；AssetEnvelope 落库/projection；TruthGraph(#5)；PlayabilityManifest(#4)。

## 4. 设计
### 4.1 trpg-model::asset（契约类型）
AssetEnvelope/AssetKind(Rule/Module/Character/Scene/Npc/Object/Generic)/AssetFacet/ExecutionTier/AssetLifecycle(Draft/Active/Deprecated)/BindingPlan/BindingVerdict + `pub mod asset; pub use asset::*;`。全 `#[derive(Serialize,Deserialize,Clone,Debug,Default,PartialEq)]` + 字段 serde default。capability id 常量集(`pub const CAP_*: &str`)。
TurnTrace 加 `#[serde(default)] pub binding_trace: Vec<BindingPlan>`。

### 4.2 trpg-runtime::binding（registry + resolver）
`CapabilityRegistry::with_defaults()` 注册 11 capability id；`resolve_binding(need_kind:&str, facets:&[AssetFacet], reg:&CapabilityRegistry)->BindingPlan` 纯函数(verdict 规则见 §2)；`facets_from_need_outcome`/`facets_from_kernel` 轻量 adapter——从既有 NeedResolutionTrace/CompiledContext 的 source_refs + need_kind 派生**最小** AssetFacet(facet_kind=need_kind、binding_candidates 按 need_kind→候选 capability 的数据映射、source_refs 透传)。**零 LLM、确定性**。

### 4.3 trpg-gm 影子记录
run_pipeline context_assembly 成功后(成功路径,失败路径不跑)：`let plans = shadow_bind(&ctx.compiled().need_trace, &registry);` → 存入将写的 TurnTrace.binding_trace。registry 进程级 `LazyLock`。**不发新事件、不改结算、不阻塞**;失败仅 warn。

## 5. 理念守卫（MUST）
1. **零行为变更**：影子 resolver 不改任何 check/contest/effect/state；成功+失败回合事件序列字节等价(可观测切片的等价测试继续绿)。
2. **确定性绑定**：同 (need_kind,facets,registry) → 同 BindingPlan(契约测试)。
3. **fail-soft**：影子绑定/记录失败仅 warn,绝不影响回合。
4. **零规则集硬编码**：resolver/registry 按 capability id + facet 数据,**不按规则集/模组名**(CI 守卫 0)。
5. 文件 ≤400;新结构 serde default 向后兼容;不碰 parser 存储。

## 6. 测试与验收
**单测**：契约类型 round-trip + back-compat;resolve_binding 5 verdict 分支(命中→Exact、部分→Partial、有 source 无 cap→Guided、只 source→SourceOnly、空→Unsupported);CapabilityRegistry 注册/resolve;facets_from_need_outcome 派生;确定性(同输入同 plan)。
**等价(硬闸)**：成功+失败回合 TurnEvent 序列与可观测切片后字节等价(影子是纯附加);机械结算无变化。
**live e2e**：真库回合(CoC + Cyberpunk):TurnTrace.binding_trace 非空、verdict/capability 合理、`trpg explain` 显示 binding_trace;机械结果与影子前一致(抽查 check/roll 不变)。
**守卫**：no-engine-ruleset-hardcode 0;cargo test --workspace 零回归。

## 7. 切分（plan,按依赖）
1. trpg-model::asset 契约类型 + TurnTrace.binding_trace + capability id 常量(单测)。
2. trpg-runtime::binding：CapabilityRegistry + resolve_binding + facets_from_* adapter(单测,确定性)。
3. trpg-gm：run_pipeline 影子绑定 + 写 binding_trace;`trpg explain` dump binding_trace。
4. 等价 + live e2e + 守卫 + 全套件零回归 → 合并 main + push。

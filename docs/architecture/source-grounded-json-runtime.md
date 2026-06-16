# 架构宪法：Source-grounded JSON Asset Runtime（Phase 0）

日期：2026-06-17
状态：宪法（北极星，约束后续所有架构演进；据 `design/优化2.md` 收敛）
前置：R1（execute_turn 唯一回合入口）+ R2（Need bus 五类）+ R5（postprocess 分层）+ P0-2（引擎去硬编码）已合 main。

## 定型

本项目定型为 **TRPG Source-grounded JSON Runtime**，**不是** TRPG Rule Compiler。
规则书/模组/角色卡 → 解析成**可追溯 JSON 资产**（带 SourceRefs/Facets/Visibility/Confidence）→ 运行时按 Need 取相关资产 → **晚绑定**到少数稳定 Rust capability → 能执行的 Rust 执行、不能的进 source-backed guided ruling → GM Agent 只管语义/澄清/节奏/叙事。

> 不把 TRPG 规则编译成统一程序；把规则/模组解析成可追溯 JSON 资产，再由运行时晚绑定到有限 Rust 能力。

## 核心原则（MUST）

1. **JSON 是资产，不是程序**：parser 不需把规则表达成完整可执行程序。资产职责=可定位/可引用/可检索/可展示/可绑定/可降级。每资产三层：Envelope（稳定外壳）+ Facets（可绑定能力标签）+ Data（宽松原始数据）。Envelope/Facets 稳定，Data 宽松。
2. **Runtime 只认少数稳定 capability**：Rust **绝不**理解 Cyberpunk/CoC/D&D/SW/Triangle。只认 RollDice/CompareTarget/CompareOpposed/CountSuccess/LookupTable/ApplyResourceDelta/ApplyCondition/Apply*Patch/Reveal/Record/AskClarification/AskSourceBackedRuling。特殊机制=asset facet + binding id + registry executor，不进主 runtime。
3. **执行分层（ExecutionTier）**，不再用"解析成功/失败"判规则质量：SourceOnly / GuidedRuling / PartialExecution / ExactExecution / VerifiedExecution。复杂机制默认低 tier guided，不强行自动化——这是产品化不是退步。
4. **晚绑定（BindingResolver）**：运行时按 Need + 候选资产 + 当前状态 + 可用 capability + source/confidence/visibility 产 BindingPlan（Exact/Partial/Guided/SourceOnly/Unsupported）。绑定不随机漂移。
5. **状态权威逐步事件化**：EventLog 是事实来源、Projection 是当前视图、Transcript 是叙事记录、Memory 是检索辅助。渐进 write-through（先双写 domain event，再逐步 projection 派生），不大爆改。
6. **防剧透三层图**：TruthGraph（真相）/ PlayerKnowledgeGraph（玩家已知）/ PresentationGraph（本回合可进 prompt 的 player-safe）。只 PresentationGraph → GM prompt。GM Agent 不直接看 TruthGraph。
7. **沿用既有硬约束**：数据驱动、零规则集/模组硬编码（CI 守卫 `no-engine-ruleset-hardcode`）、语义优先（非关键词路由）、SSE 真流式、fail-closed、BP1/BP2/BP3 缓存语义稳定、文件 ≤400 行、等价不回归、大改跑 live e2e。

## 明确不做（避免重蹈覆辙）

- 不重做大一统规则编译器（之前受挫非偶然，是 TRPG 规则形态决定的）。
- 不让每个规则系统一套专属 compiler。
- 不让 runtime 出现 `ruleset_id.contains` / `module_id.contains`。
- 不让 GM Agent 直接决定机械结果（DV/DC、HP/SAN/MP、伤害、条件、资源、reveal）。
- 不把 transcript/memory 当状态权威。
- 不让 parser 吐自由 JSON 后 runtime 到处猜字段。
- 不为自动化牺牲 guided ruling。

## 现状（2026-06-17 实测对照 `优化2.md`）

已具备：SourceRefs/confidence 普遍；Facet 雏形（ParameterFacetKind+binding+execution status）；tier 雏形散落（RulingConfidence/RuleAssistStatus/BindingVerificationStatus/BindingStatus）；Visibility 属性级；事件设施（WorldEvent/memory_events）；PlayabilityGateReport（二元）。
缺（greenfield）：① AssetEnvelope 统一壳 ② BindingResolver+CapabilityRegistry ③ TruthGraph/PlayerKnowledgeGraph ④ 完整 Flight Recorder。
**关键判断**：多数"新原语"是把散落概念**收敛成一等契约**，非从零造——成本/风险比看上去低。

## 路线（按 ROI，逐件 spec→plan→执行→验证→合并）

1. **可观测性 + 失败语义切片**（进行中）：TurnFailed/Warning + Need/source/binding trace + Flight Recorder 初版 + explain/inspect-prompt CLI。与 GPT Pro P1-1/P1-4/P1-5 重叠，解锁后续调试。
2. AssetEnvelope + Facet 收敛（整合现有散落原语）。
3. BindingResolver + CapabilityRegistry（绑定一等公民）+ BindingCoverageManifest。
4. PlayabilityManifest 分级（L0-L5）。
5. TruthGraph/PlayerKnowledgeGraph（防剧透，子项目2）。
6. EventLog write-through + Projections。
7. 契约测试六条（no-hardcode✅ / asset / binding / turn-determinism / visibility / playability）。

R3（FieldClaim）并入 AssetEnvelope.source_refs（决定 ExactExecution 资格）；R4（ModuleEntity）并入 ModuleAsset/EntityFacet（为知识图谱提供稳定实体 id）。

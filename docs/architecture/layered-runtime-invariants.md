# 层化运行时 — 事实约束清单（invariants）

> **后续每个 phase（P1-P7）的 spec 与自动 agent 必读。**
> 设计4 正文的若干描述已被其**附录A/C**修正。本清单把 6 条映射纠正固化为
> 带 `file:line` 证据的事实约束，避免自动 agent 基于 设计4 正文（过时假设）动手。
> 与代码内 `// ARCHITECTURE-ANCHOR` 注释交叉引用一致（读任一即得正确事实）。
>
> 来源：`docs/superpowers/plans/2026-06-19-layered-runtime-architecture.md` P0；
> 架构师 P0 拍板见 `.claude/autonomous/p0_spec.md` 顶部 D1-D4。
> 证据行号为本文件落地时（基线 commit `327d284`）的快照，后续可能漂移——以符号名为准。

---

## INV-1 — §14 Ports / 独立 Narrator / NarrationPacket / 五 ContextCompiler 当前=零落地

设计4 §14 描绘的 Ports 抽象、独立 `Narrator` 类型、`NarrationPacket`、五套
`ContextCompiler` 在当前代码库**尚无任何落地**。

- 证据：`rg "NarrationPacket|ContextCompiler|struct Narrator|trait Narrator" crates/`
  → **0 命中**（本 P0 落地时）。
- 约束：后续 phase 不得假设这些类型已存在；引用它们=新建，须显式立项，不可"对齐现状"。

## INV-2 — 控制平面真身 = `trpg-gm::execute::run_pipeline`，**不是** `trpg-orchestrator::TurnOrchestrator`

设计4 §4 的"控制平面"目标态概念，事实上落在 trpg-gm 的回合管线驱动函数。

- 证据：`crates/trpg-gm/src/execute.rs:107` `async fn run_pipeline(...)`，由同文件
  `execute_turn`（:82，调用点 :93）驱动，串起 15-phase（CANONICAL_TURN_PLAN）。
- 反例：`crates/trpg-orchestrator/src/lib.rs:137` `struct TurnOrchestrator` +
  `:146 reduce_turn(...)` 是**同名异职**——干 gate / lifecycle（回合准入、生命周期裁决），
  **不是** §4 控制平面。
- 约束：控制平面相关迁移落到 `run_pipeline` 现场，**勿**投射到 `TurnOrchestrator` 同名类型。

## INV-3 — §17『trpg-gm 只留 Orchestrator/Narrator』方向读反了

设计4 §17 若读作"trpg-gm 现状=只有 Orchestrator + Narrator"是**反的**。

- 事实：`crates/trpg-gm` 现为**业务总汇**（tools / turn_loop / gate / mode / plugins /
  prompts / scene_policy / obligations …，见 `crates/trpg-gm/src/lib.rs` 模块清单）。
- 约束：迁移目标是把业务**移出** trpg-gm（瘦身到控制平面+叙事），**不是**确认其现状已瘦。
  任何"trpg-gm 已是纯 Orchestrator"的假设都错。

## INV-4 — commit 边界 = 仅 runtime-owned typed services 可 commit（Narrator 不能；非新造大 Kernel）

状态提交权归运行时拥有的 typed services；叙事层只读不写。

- 约束：Narrator / 叙事路径**不得**直接施加状态变更（apply_damage / apply_effect_roll /
  直写 KnowledgeEdge）——预置门 `scripts/no_layer_boundary_violation.sh` 守此（P0 对基线 0 命中，
  P1/P2 拆分后细化全边界）。
- 反模式：**不要**为此新造一个庞大的 "Kernel" 统一接管 commit；commit 边界是
  现有 runtime-owned typed services 的既有职责，迁移是**澄清**而非新建大一统层。

## INV-5 — Policy = 横切 hook（三 checkpoint），**非**顺序第六运行层

设计4 把 Policy 描述成"第六个顺序运行层"是错的；它是横切 hook。

- 证据：`crates/trpg-gm/src/plugin/types.rs:18` `enum PluginHook` 的三个 checkpoint：
  `ContextAssembly` / `AfterLlmStream` / `HeavyPostprocess`（见
  `crates/trpg-gm/src/plugin/builtin_no_mechanical.rs:66` 等使用点）。
- 约束：Policy/Plugin 贡献挂在这三个横切 checkpoint 上（safety>priority 排序），
  **不要**把它建模成插在管线里的顺序第六层。

## INV-6 — 五套 compiler 暂缓：先复用 `prepare_turn_context` 投分层 packet

不要现在就拆五套 ContextCompiler；先在现有装配点投影分层 packet，验证收益再拆。

- 证据：`crates/trpg-runtime/src/lib.rs:374` `pub async fn prepare_turn_context(...)`
  是现有回合上下文装配单一入口。
- 约束：P1+ 的分层 context 先**复用** `prepare_turn_context` 产分层 packet（advisory），
  度量收益后再决定是否拆成 §14 的五套 compiler——避免过早抽象。

---

## OPEN（待人定，P0 不决）

- **D2 / P7**：trpg-orchestrator::TurnOrchestrator 的 rename / 是否抽独立 crate 的取舍
  **推迟到 P7**（P0 只记事实 + 加 ANCHOR，不改名不动结构）。

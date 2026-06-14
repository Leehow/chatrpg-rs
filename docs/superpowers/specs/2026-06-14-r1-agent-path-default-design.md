# R1 设计：agent 路径升默认 + 退役 legacy + 数据化统一回合执行器

日期：2026-06-14
状态：设计已获架构师逐项确认（决定性切换 / C 级数据化执行器 / 落 trpg-gm / in-code TurnPipelinePlan）
前置：一期（agent loop `GmLoop::run_gm_turn`）+ 二期（规则感知 GM）+ 三期（Mode Skills）+ 设计优化甄别（`docs/设计优化_甄别与收敛规划_2026-06-14.md` §R1，吸收外部评审 §1-A/§1-B/§1-C）。本期 = 把已可玩但非默认的 agent 路径提为唯一回合执行器，退役 legacy 状态机。

## 1. 背景：三条回合执行器并存

当前同一件事（演一个回合）有**三套并行实现**，各自手写一遍调度，长期漂移：

| 路径 | 入口 | 现状 |
|---|---|---|
| **agent**（一期成果） | `trpg-gm` `GmLoop::run_gm_turn`（turn_loop.rs） | 干净 typed 状态机：确定性头部→上下文→工具轮循环→两终态→verify→finalize。**但 CLI 默认不走它**（`agent: bool` flag default false），API 完全不用它 |
| **legacy CLI** | `trpg-cli` `run_turn_once`（main.rs:906-1502，~600 行 if/else） | CLI 默认路径。词法 `plan_turn`/`matches_conditions` |
| **API** | `trpg-api` `play_turn_sse`（lib.rs:1064） | 走 `runtime.plan_agent_turn`（词法 plan 路径 + 自带 SSE）。trpg-api 不依赖 trpg-gm |

外部评审 §1-A（多内核挤一条线）/§1-B（词法 advice）/§1-C（CLI/API 路径漂移）批评的都是这个三头并存态。R1 一举消化：统一到 agent 路径 + 数据化 + 删 legacy。

### 1.1 依赖方向（已核实，决定落点）
`trpg-gm` → 依赖 → `trpg-runtime`（gm 高层）；`trpg-runtime` **不**依赖 `trpg-gm`（runtime 低层，干净）。故统一执行器落 **trpg-gm**（已有 GmLoop、已依赖 runtime），零循环；API 加 `trpg-gm` 依赖即可。

## 2. 已拍板决策

| 决策点 | 结论 |
|---|---|
| 范围/节奏 | **决定性一步切换**：翻 CLI 默认 + 迁 API 到 agent loop + 删 legacy `run_turn_once` + 删词法 `plan_turn`/`plan_agent_turn`。收敛最彻底、history 最干净；回归面=全部回合，回滚靠 git revert 整体退 |
| 执行器抽象层级 | **C 级**：统一 `execute_turn(request)->Stream<TurnEvent>` facade（消化 §1-C）+ 声明式 `TurnPipelinePlan`（§1-A 完整版，过程式 phases→数据） |
| 落点 | **trpg-gm**（新模块 turn_plan.rs / turn_event.rs / execute.rs），不新建 crate |
| TurnPipelinePlan 形态 | **in-code `Vec<TurnPhasePlan>`**（回合 phases 不随规则集变，数据化到结构体即可，做成 config 文件是过度工程） |
| postprocess parity | **并集不是交集**：统一执行器做所有路径步骤的**超集**，每条现有步骤都不丢，各路径互补对方独有项 |

## 3. 目标 / 非目标

**目标**：CLI/API 唯一回合入口 = `trpg_gm::execute_turn`，由声明式 `TurnPipelinePlan` 驱动；legacy `run_turn_once` + 词法 `plan_turn`/`plan_agent_turn` 删除（§1-B 词法 advice 随之消失）；postprocess parity 超集化零丢失；SSE 真流式与缓存稳定保持（一期硬性原则）；全规则集零硬编码。

**非目标（本期，留后续 R 项）**：R2 统一 Need 总线（回合内 kernel 发 Need）；R5 postprocess 临界/重活分层 + high-water mark（本期只做 parity 不做并发时序优化，保持 API 非阻塞/CLI 同步现状）；R3 字段级 verifier；R4 ModuleEntity 类型化。TurnPipelinePlan 做成可配置 config 文件（YAGNI）。

## 4. 设计

### 4.1 TurnPipelinePlan（声明式数据，turn_plan.rs）
回合表示为有序 `Vec<TurnPhasePlan>`，执行器**解释**它而非过程式写死。每 phase：
```
TurnPhasePlan { id: PhaseId, kind: PhaseKind, provided: &[Key], consumed: &[Key],
                early_return: Option<EarlyReturnRule> }
```
- `kind`：`Deterministic`（头部确定性步）| `AgentLoop`（工具轮主体）| `Postprocess`（尾部）。
- `provided`/`consumed`：该 phase 产出/依赖的上下文键，供执行器校验顺序合法（debug 断言，非运行期开销）。
- `early_return`：`AgentLoop` phase 命中 `AwaitingPlayerRoll` 时，跳过未触发的尾部步、仍跑 finalize 子集。

**规范 plan（CANONICAL_TURN_PLAN 常量）**——对齐现 run_gm_turn 顺序 + 补 parity：
1. `record_player_action`（Deterministic）— world event PlayerAction
2. `refresh_live_derived`（D）— refresh_actor_live_derived
3. `reconcile`（D）— 机制对账 / 债务回收
4. `gate`（D）— request_player_roll 闸门判定
5. `stimulus_pass`（D）— 语义被动刺激预 pass（SAN 等）
6. `opposed_prepass`（D）— 战斗对抗绑定
7. `mode_inference`（D）— 当前 mode 推导
8. `debt_load`（D）— 义务账装载
9. `context_assembly`（D）— prepare_turn_context / BP1-3 块
10. `agent_loop`（AgentLoop，early_return=AwaitingPlayerRoll）— 流式 + 工具轮
11. `verify_after_stream`（Postprocess）— NarrationVerifier→errata 记忆 + 追溯债务
12. `finalize`（Postprocess）— save_turn + turn 记忆事件（**超集**：scene_id/location_id/actor_ids/importance=50）
13. `audit_learning`（Postprocess）— audit_learning_for_turn
14. `scene_navigate`（Postprocess，conditional module_id）— scene_navigator：set_session_scene + SceneChanged world event + 到场深抽 + frontier 预抽
15. `carryover_debt`（Postprocess，conditional）— 工具轮耗尽且有未决义务→存债务块记忆

### 4.2 execute_turn facade + TurnEvent（execute.rs / turn_event.rs）
```
pub async fn execute_turn(req: TurnRequest, plan: &TurnPipelinePlan)
    -> impl Stream<Item = TurnEvent>
```
- 内部：按 plan 顺序跑 Deterministic 头部（调既有函数——turn_loop.rs 的过程式头部抽成各 phase handler）→ 驱动 GmLoop agent_loop（产 Delta/AwaitingPlayerRoll）→ 跑 Postprocess 尾部。
- **TurnEvent 统一事件枚举**：
```
enum TurnEvent {
  Delta(String),                                  // 逐 token 真流式
  AwaitingPlayerRoll { check_id, prompt_public }, // 桌面骰 gate
  SceneTransition { from, to, reason },            // scene_navigate 产出
  Errata(ErrataEntry),                            // 后置勘误（不阻塞）
  PostprocessScheduled,                            // 尾部开始（transport 决定前台/后台）
  TurnComplete { outcome: TurnOutcome },
}
```
- 取代 `GmLoop::run_gm_turn` 的 `on_delta` 回调 + `TurnOutcome` 返回——facade 把它们统一成事件流。`run_gm_turn` 收编为 `agent_loop` phase handler（产 Delta/AwaitingPlayerRoll 事件）。

### 4.3 postprocess parity（R1 最硬一块，超集化）
现状（已核实 file:line）：agent 路径**不调 scene_navigator**、API 用后台 spawn、CLI 同步；三方 MemoryEvent/world event 各异。统一执行器做**超集**，原则=任何现有步骤都不丢、各路径补对方独有：

| 步骤 | agent 现状 | API 现状 | CLI 现状 | 统一执行器 |
|---|---|---|---|---|
| save_turn | ✓(356) | ✓后台(1625) | ✓同步(1462) | ✓ finalize phase |
| turn 记忆事件 | 简版(362) | 富版 scene/actor/imp50(1627) | 简版(1489) | **取富版** |
| audit_learning | ✓ | ✓ | ✓ | ✓ |
| scene_navigator(+set_scene/SceneChanged/深抽/frontier) | **缺** | ✓后台(1641) | ✓同步(1496) | **补进 agent 路径** |
| carryover 债务 | ✓(307) | 缺 | 缺 | **补给 API/CLI** |
| errata 记忆 | ✓(332) | 缺 | 缺 | **补给 API/CLI** |
| world event(PlayerAction 头) | ✓ | ✓ | ✓ | ✓ record_player_action phase |

**scene_navigator 下沉**：现定义在 `trpg-api/src/lib.rs:2068`（pub，被 API + CLI legacy 调）。统一执行器在 trpg-gm，需把 scene_navigator + validate_transition + build_nav_prompt **下沉**到 trpg-gm 或 trpg-runtime（runtime 更合适：它已 own extract_module_scenes/prefetch_frontier/set_session_scene/load_module_graph）。下沉后 trpg-api 的 scene_navigator 改为 re-export 或删，调用方改引 runtime/gm。

### 4.4 传输适配（唯一各写各的，transport 决定尾部前台/后台）
- **CLI**（agent_play 取代 run_turn_once）：drain Stream 到底（同步），Delta→stdout 逐 token，AwaitingPlayerRoll→提示，SceneTransition/Errata→打印。
- **API**（play_turn_sse 改造）：Delta→SSE data chunk（真流式），narration 完→发 PostprocessScheduled→**在同一 spawned 任务里继续 drain 尾部事件**（保持现 API 非阻塞语义：客户端拿到叙事即可，尾部后台跑）。AwaitingPlayerRoll/SceneTransition→SSE event。
- Stream<TurnEvent> 模型天然让两 transport 各自决定尾部前台(CLI)/后台(API)，无需两套 postprocess。

### 4.5 退役（删除清单）
- 删 `trpg-cli` `run_turn_once`（main.rs:906-1502）；CLI play 永走 execute_turn，`agent: bool` flag 移除（或永真兼容旧脚本）。
- 删 `trpg-runtime`/`trpg-api` 的 `plan_agent_turn`/`plan_turn` 词法路径 + `trpg-agent` `matches_conditions`/`plan_turn`（§1-B 词法 advice 随之消失，确认无 agent 路径残留引用后删）。
- trpg-api 加 `trpg-gm` 依赖，play_turn_sse 改调 execute_turn。

## 5. 理念守卫（MUST）
1. **数据驱动**：回合管线=`TurnPipelinePlan` 数据，执行器解释，不过程式写死（呼应乐高/no-hardcode 理念）；
2. **零规则集硬编码**：plan/phase 通用，grep 不得出现规则集名于回合逻辑；
3. **SSE 真流式 + 缓存稳定**：Delta 逐 token 直通不缓冲，BP1-3 缓存布局不破（一期硬性原则）；
4. **AI 决策/Rust 执行边界**：agent_loop 经工具下指令、引擎落账，不因数据化而松动；
5. **fail-closed**：scene_navigate 不确定不乱跳；postprocess 某步失败 warn 不 panic、不阻塞叙事（D2 错误后置勘误）。

## 6. 测试与验收

**单测**：① TurnPipelinePlan 解释器按 plan 顺序执行 + early_return（AwaitingPlayerRoll 跳尾部跑 finalize 子集）；② TurnEvent 流 4 类事件序正确；③ postprocess parity——构造一回合断言超集每步都跑（scene_navigate 在 agent 路径触发、carryover/errata 在 API 路径触发）；④ scene_navigator 下沉后行为不变（复用现有 scene_nav 测试）；⑤ 缓存稳定：只改玩家输入只动 dynamic_hash（一期缓存测试回归）。

**decisive-cut 验证闸（合并前强制，因无 env fallback）**：
- 全 `trpg-harness` e2e 通过；
- 真库三链复测全绿：血色公路（CoC sandbox）/ CPR Homecoming（线性）/ The Vault 或 Triangle（任务集）——CLI **与** API 两 transport 各跑，断言叙事流式 + 场景切换 + 落库（turn/memory/scene_id/world event）与切换前一致；
- `cargo check --workspace` + 全 crate test 零回归；TTFT 不劣化（对比一期基线中位 5s）。

**工程**：文件 ≤400 行（execute.rs/turn_plan.rs/turn_event.rs 新建即拆好）；新结构 `#[serde(default)]`；零 per-ruleset 硬编码。

## 7. 风险
1. **回归面=全部回合**（决定性切换）——靠合并前强制全验证闸 + git revert 整体回退兜底；
2. **postprocess parity 漏步**——§4.3 超集表逐条核对 + 单测断言每步触发；最隐患是 agent 路径补 scene_navigator（原本完全没有）；
3. **scene_navigator 下沉**触 trpg-api/runtime/gm 三 crate + 调用方改引——下沉为机械移动 + re-export 过渡，单测护；
4. **API 非阻塞语义**——尾部仍后台 drain，不可因统一而变同步阻塞 SSE 返回（transport 适配层把关）；
5. **缓存/流式破坏**——Delta 直通不缓冲、BP1-3 布局不动，一期缓存与流式测试回归把关。

## 8. 工作量与切分（单次 cut 内的实施顺序）
约 3-5 天。建议实施序（同一 PR/分支，最后一起过验证闸切换）：
1. scene_navigator 下沉 runtime（机械移动 + re-export，先绿）；
2. turn_plan.rs（TurnPipelinePlan + CANONICAL_TURN_PLAN）+ turn_event.rs（TurnEvent）；
3. turn_loop.rs 过程式头部抽成 phase handlers；
4. execute.rs（execute_turn 解释器，驱动 handlers + agent_loop + postprocess 超集）；
5. CLI agent_play 改 drain execute_turn，删 run_turn_once；
6. trpg-api 加 trpg-gm 依赖，play_turn_sse 改调 execute_turn（SSE 适配 + 后台尾部）；
7. 删 plan_turn/plan_agent_turn/matches_conditions 词法路径；
8. 全验证闸（harness + 真库三链 CLI&API + 零回归）→ 切换。

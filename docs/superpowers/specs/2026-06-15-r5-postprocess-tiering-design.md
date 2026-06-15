# R5 设计：postprocess 临界/重活分层 + turn 高水位守卫

日期：2026-06-15
状态：设计（自主执行模式——用户"从头跑到尾自己决断"全权授权；UX 默认按理念定、本 spec 标出供 review）
前置：R1（execute_turn 数据化回合执行器，已合 main `8c69ba9`）+ R2（Need 总线，已合 main `6825891`）。本期 = 外部评审 §4-隐患2 收敛：把 R1 的 postprocess 尾段拆「临界(critical) / 重活(heavy)」，临界同步保证下一回合前落账，重活真后台不挂流；加 turn 高水位守卫消除「玩家连发回合读到陈旧态」竞态。

## 1. 背景：当前 postprocess 全在一个任务里、重活挂流、无 turn 排序守卫

R1 后 `execute_turn`（trpg-gm/src/execute.rs）的 `run_pipeline` 在**单个 `tokio::spawn` 任务**里跑：确定性头部 → `run_agent_loop`(流式 Delta) → `PostprocessScheduled` → **全部 postprocess 尾段**（VerifyAfterStream/Finalize/AuditLearning/SceneNavigate/CarryoverDebt）→ `TurnComplete`。问题：

- **重活挂流**：`SceneNavigate` 含到场深抽（`extract_module_scenes`，~12s+ LLM）+ frontier 预抽；`AuditLearning`、memory 写也在尾段。这些都在 `TurnComplete` 之前 → SSE 流被挂住到 done，叙事早完了客户端还在等重活。
- **无 turn 排序守卫**：全仓无 high-water mark / last_committed_turn / in-flight guard（grep 证实）。`turns.postprocess_status` 列存在但只被 `save_turn` 当终态（ready/awaiting）写一次，非完成追踪。客户端不等 done 就发下一回合 → 上一回合的 `set_session_scene`（current_scene_id）/memory 还没落 → 下一回合 `prepare_turn_context` 读到陈旧场景/缺记忆 = **竞态**。
- R2 同步解析（Need bus）的"真合批"也留待本期一并理顺尾段时序。

## 2. 已拍板决策（自主，按理念）

| 决策点 | 结论 |
|---|---|
| 分层 | postprocess 拆 **critical（同步，TurnComplete 前落账）/ heavy（后台，TurnComplete 后另起 spawn）** |
| critical | `Finalize`(save_turn 回合记录+status) + `SceneNavigate` 的**切场景决策+`set_session_scene`+SceneChanged 事件**（下一回合直接读的状态） |
| heavy | `AuditLearning`、turn memory_event 写、`SceneNavigate` 的**到场深抽+frontier 预抽**、`CarryoverDebt` 记忆、`VerifyAfterStream` errata 记忆（质量/审计/缓存类，下一回合不强依赖；deep-extract 缺时 N2 SkeletonOnly 降级块兜底） |
| 高水位守卫 | `turns.postprocess_status` 升级为完成追踪：`streaming`→`critical_done`→`complete`；session 记 `last_turn_id`。下一回合开头校验上一回合 **critical_done** |
| UX（撞未完）默认 | **不硬拒玩家**：下一回合若上一回合 critical 未达 → **短等(bounded，如 ≤2s)轮询 critical_done 再走**；超时仍按 fail-closed 放行（宁可偶发轻微陈旧也不卡玩家）。heavy 未完**从不**阻塞下一回合。**⚠️ 这是产品味 UX 取舍，标出供 review**——备选：硬 409 拒绝（更强一致但卡玩家，违 UX 理念，不取） |
| 落地 | 分阶段（同 R1/R2：critical/heavy 拆分先行 → 高水位守卫 → 验证闸） |

## 3. 目标 / 非目标
**目标**：临界 postprocess（save_turn + 场景切换落账）在 TurnComplete 前同步完成、下一回合可靠读到；重活（深抽/audit/memory/errata/carryover）后台跑不挂 SSE 流不阻塞下一回合；turn 高水位守卫消除连发回合的陈旧态竞态；UX fail-closed 不硬拒玩家；CLI（同步 drain）与 API（SSE）两 transport 行为一致；零规则集硬编码；R1 的 TurnEvent 流语义不破（TurnComplete 仍是终态信号，heavy 在其后）。

**非目标（本期）**：重写 R1 的 execute_turn 整体结构（只拆尾段时序）；R2 Need bus 改后台解析（context 获取仍同步——它是 LLM 回合前必需）；多人并发会话的分布式锁（单进程 in-process 守卫即可）；退役 run_gm_turn。

## 4. 设计

### 4.1 postprocess 分层（execute.rs run_pipeline 尾段）
- `run_pipeline` 尾段拆两组：**critical 组**（`phase_finalize` 的 save_turn + status='critical_done'、`phase_scene_navigate` 的切场景决策+`set_session_scene`+SceneChanged）同步 await 完成 → 发 `TurnComplete`；**heavy 组**（audit / memory 写 / 深抽 / frontier / carryover / errata 记忆）`tokio::spawn` 另起任务在 TurnComplete 后跑，完成后置 status='complete'。
- 需把现 `phase_finalize` 内的 memory_event 写**拆出**到 heavy（finalize 只留 save_turn 记录+status=critical_done）；`phase_scene_navigate` 拆「切场景(critical)」与「深抽+frontier(heavy)」两半（现 scene_navigator 是一体，按此切）。
- TurnEvent 流：critical 完 → `TurnComplete`（客户端此刻可安全发下一回合）；heavy 在其后台跑，可选发 `HeavyPostprocessDone` 事件（或不发，纯后台）。**TurnComplete 语义升级**=「critical 已落账、可继续」（非「全部 postprocess 完」）。

### 4.2 turn 高水位守卫
- `turns.postprocess_status` 状态机：`save_turn` 写 `critical_done`（critical 组末）；heavy 组末 `update` 为 `complete`。
- session 记最近 turn（`sessions.last_turn_id` 或查 turns max(created_at)）。
- `prepare_turn_context` / 回合入口开头：查上一回合 `postprocess_status`，若 < `critical_done` → bounded 短等轮询（≤2s，间隔 100ms）→ 达到则继续；超时 fail-closed 放行（warn）。heavy 未完（critical_done 但非 complete）→ 直接放行（重活不阻塞）。
- in-process（单进程 axum/CLI），用 DB status 做跨「流式任务/下一回合任务」的协调即可，无需分布式锁。

### 4.3 两 transport 一致
- API `play_turn_sse`：drain execute_turn 流，`TurnComplete` 后即可结束 SSE（heavy 在 execute_turn 内部 spawn 的任务里继续，与 SSE 生命周期解耦）。
- CLI `trpg turn`/`play`：drain 到 `TurnComplete` 即回合结束（heavy 后台）；一次性 `trpg turn` 需等 heavy 完再退进程？——否，`turn` 退出前**等 heavy 完成**（脚本/测试期望确定性落账），交互 `play` 不等（下一轮高水位守卫兜）。这点 CLI `turn` 与 `play`/API 行为差异：`turn`=确定性（等 heavy），流式=不等（守卫兜）。

### 4.4 分阶段
1. postprocess 分层：execute.rs run_pipeline 尾段拆 critical/heavy + finalize 拆出 memory + scene_navigate 拆切场景/深抽；TurnComplete 在 critical 后；heavy spawn。单测：critical 在 TurnComplete 前完成、heavy 在其后、heavy 失败不影响 critical/已发事件。
2. 高水位守卫：postprocess_status 状态机（critical_done/complete）+ 回合入口 bounded 等待。单测：上一回合 critical 未达→短等；heavy 未完→放行；超时→fail-closed 放行。
3. 两 transport 接线（API SSE 结束于 TurnComplete；CLI turn 等 heavy、play 不等）。
4. 验证闸：真库两 transport live——连发两回合（turn N+1 紧跟 N），断言 N+1 读到 N 的切场景结果（无陈旧）、heavy 不挂流（TurnComplete 在叙事后很快到）、零回归、缓存稳定。

## 5. 理念守卫（MUST）
1. **fail-closed 不卡玩家**：守卫超时放行而非死等/硬拒（UX 理念：错误后置不阻塞玩家，私骰泄漏除外——本期无私骰风险）；
2. **数据驱动**：critical/heavy 归类按 phase 语义（下一回合是否强依赖），非硬编码规则集；
3. **AI 决策/Rust 执行边界**：分层是 Rust 时序编排，不涉 AI；
4. **SSE 真流式 + 缓存稳定**：Delta 不变；TurnComplete 提前到 critical 后（流更快返回）；BP1-3 缓存语义不破；
5. **heavy 失败隔离**：heavy 后台任务 panic/Err 只 warn + status 标记，绝不影响已落账的 critical 或已发的叙事（D2 错误后置）。

## 6. 测试与验收
**单测**：① run_pipeline critical 在 TurnComplete 前、heavy 在其后（事件序 + 时序断言，mock 慢 heavy 验 TurnComplete 不被挂）；② heavy 失败不影响 critical（注入 heavy panic，断言 turn 已 save、TurnComplete 已发）；③ 高水位守卫：critical 未达→bounded 等→达→继续 / 超时→放行 / heavy 未完→放行；④ postprocess_status 状态机 streaming→critical_done→complete。
**e2e 验证闸**（真库 :54347 CoC 两 transport）：连发两回合 turn N（触发切场景）→ 紧接 turn N+1，断言 **N+1 读到 N 切换后的 current_scene_id（无陈旧竞态）** + N 的 TurnComplete 在叙事完成后很快到达（重活不挂流，对比 R1 基线时延）+ heavy（深抽/memory）最终落账（status=complete）+ 零回归 + 缓存锚稳定。
**工程**：文件 ≤400 行；postprocess_status 状态值集中常量；零 per-ruleset 硬编码。

## 7. 风险
1. **并发时序竞态本身**（高，难测）——bounded 等待 + DB status 协调 + 单测注入慢/失败 heavy 验隔离；in-process 单进程降低复杂度；
2. **critical/heavy 误分类**（把下一回合强依赖的放进 heavy → 陈旧）——分类表逐条核对「下一回合 prepare_turn_context 是否读它」；高水位守卫对 critical 兜底；
3. **CLI turn 等 heavy vs play 不等的行为差异**（测试/脚本期望确定性）——`turn` 等 heavy 保确定性，文档/spec 写明；
4. **heavy spawn 的 owned 数据**（同 R1 OwnedTurnRequest，heavy 任务需 own 它需要的 db/状态）——沿用 R1 spawn 模式；
5. **守卫超时放行的偶发陈旧**（UX 取舍）——超时设足够（≤2s 远大于 critical 实际耗时），实际几乎不触发；标出供 review。

## 8. 工作量与切分
约 2-4 天。实施序（同一分支，最后过验证闸）：
1. execute.rs run_pipeline 尾段拆 critical/heavy + finalize 拆 memory + scene_navigate 拆切场景/深抽 + TurnComplete 提前 + heavy spawn + 单测。
2. postprocess_status 状态机（db + 常量）+ 回合入口高水位守卫 + 单测。
3. 两 transport 接线（API/CLI turn/play 各自时序）+ 单测。
4. 全验证闸（真库两 transport 连发回合无陈旧 + heavy 不挂流 + 零回归 + 缓存稳定）→ 合并 main。

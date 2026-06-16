# 设计：可观测性 + 失败语义切片（TurnFailed/Warning + 阶段错误分级 + Need/source trace + Flight Recorder 初版 + explain/inspect-prompt CLI）

日期：2026-06-17
状态：设计（自主执行，守 [[宪法]] `docs/architecture/source-grounded-json-runtime.md`）
前置：R1/R2/R5/P0-2 已合 main（04f95ad）。本切片 = `优化2.md` 路线 #1 + GPT Pro 待办 P1-1/P1-4/P1-5 的合并落地。

## 1. 背景：失败被伪装成"空白成功"、Need 取数无 trace、回合不可解释
- **失败伪装成功（P1-1）**：`crates/trpg-gm/src/execute.rs:128-130` —— `dispatch_deterministic` 在 mode_inference/context_assembly fail-closed 返 false 后，run_pipeline 直接发 `TurnComplete{outcome: 空 Narration}` 早返。客户端看到"成功的空白回合"，分不清是失败。`TurnEvent` 无 `TurnFailed/TurnWarning`（turn_event.rs）。
- **错误不分级（P1-5）**：大量 `let _ =`/`unwrap_or_default`/`warn!` 不区分 critical（context_assembly/save_turn 失败应中止）与 non-critical（memory/audit/learning 失败应继续）。
- **Need trace 丢失（P1-4）**：`NeedOutcome.source_refs` 在 `prepare_turn_context` 的 `blocks.extend(outcome.blocks)` 处丢弃，Need 级取数来源链断。
- **回合不可解释**：只有 TurnEvent 流 + `inspect` 子命令，无单回合 trace / prompt 视图，长团 debug 成本高。

## 2. 已拍板决策（自主，守理念）
| 决策点 | 结论 |
|---|---|
| 失败事件 | `TurnEvent` 加 `TurnFailed{phase,message,recoverable}` + `TurnWarning{phase,message}`；失败路径发 TurnFailed（**不再**发空 TurnComplete）；warn 级发 TurnWarning 并继续 |
| 阶段返回类型 | `dispatch_deterministic` 由 `bool` 改 `Result<(), PhaseFailure>`（带 PhaseId + message），caller 据此构造 TurnFailed |
| 错误分级 | `PhaseErrorPolicy{AbortTurn, EmitWarningContinue, BackgroundWarnOnly}` + 按 PhaseId 分类器；context_assembly/mode_inference/finalize(save_turn)→Abort；verify/memory/audit→WarnContinue；深抽/frontier/learning/carryover→BackgroundWarn |
| turns 状态 | 迁移 0029：`turns.failure_kind text null`（成功=NULL，失败写 `failed_context`/`failed_mode`/`failed_finalize` 等） |
| Need trace | `NeedResolutionTrace`（need_kind + source_refs + reason + block_count）随回合捕获，不再丢 source_refs |
| Flight Recorder | `TurnTrace`（trpg-model）持久化到新表 `turn_traces`（turn_id PK + trace_json jsonb），write-through、fail-soft 捕获：phases_run / need_trace / BP1·BP2·BP3 hash / signal / failure / warnings / pp_lifecycle / narration_hash |
| transport | API SSE：TurnFailed→`event: error`、TurnWarning→`event: warning`；CLI `turn`：TurnFailed → 非零退出 + stderr；`play`：红字提示不崩 shell |
| CLI | `trpg explain --session --turn`（dump TurnTrace）；`trpg inspect-prompt --turn --view player\|gm`（输出该回合装配的 BP1/BP2/BP3 块）|

## 3. 目标 / 非目标
**目标**：失败不再伪装成功（TurnFailed 显式 + turns.failure_kind 落库）；错误按 policy 分级（critical 中止、其余 warn/background）；Need 取数 source_refs 进 trace；每回合 TurnTrace 可 dump、prompt 可按视图查看；成功路径事件**字节等价**（新事件仅失败/警告路径）；fail-soft（trace 写失败不影响回合）；文件 ≤400 行。

**非目标（本切片）**：`replay`/`diff-state` CLI（留后续）；binding trace（BindingResolver 尚不存在，留 BindingResolver 切片）；事件溯源/projection 派生（留 EventLog 切片）；改任何机械/状态行为。

## 4. 设计

### 4.1 TurnEvent（trpg-gm/turn_event.rs）
```rust
TurnFailed { phase: String, message: String, recoverable: bool },
TurnWarning { phase: String, message: String },
```
`recoverable=false` ⇒ 回合终止（无 TurnComplete）；`recoverable=true` 当前未用（预留 partial）。

### 4.2 PhaseFailure + 分级（trpg-gm）
`dispatch_deterministic` → `Result<(), PhaseFailure{phase: PhaseId, message: String}>`。run_pipeline：`Err(f)` 时按 `phase_error_policy(f.phase)` —— AbortTurn ⇒ 发 `TurnFailed{phase, message, recoverable:false}` + 写 `turns.failure_kind` + return（**删除空 TurnComplete 早返**）。
`PhaseErrorPolicy`（trpg-model）+ `phase_error_policy(PhaseId)->PhaseErrorPolicy`（trpg-gm）。WarnContinue 的阶段（verify/memory/audit）失败 ⇒ 发 `TurnWarning` 继续；BackgroundWarn（heavy 内深抽/frontier/learning/carryover）⇒ 仅 `warn!`（已是 D2 隔离，spawn_heavy 内不发事件、不阻塞）。

### 4.3 Need trace（trpg-runtime prepare_turn_context + trpg-need）
合并 helper `merge_need_outcome(blocks, trace, outcome, need_kind, reason)`：注入 blocks **并**把 `outcome.source_refs`/need_kind/reason 累加进 `Vec<NeedResolutionTrace>`。trace 随 TurnContext 带到收尾，写入 TurnTrace。

### 4.4 Flight Recorder（trpg-model TurnTrace + trpg-db turn_traces）
`TurnTrace{turn_id, session_id, phases_run:Vec<String>, need_trace:Vec<NeedResolutionTrace>, bp1_hash/bp2_hash/bp3_hash:Option<String>, signal:String, failure:Option<TurnFailureRecord>, warnings:Vec<String>, pp_lifecycle:String, narration_hash:Option<String>}`。
trpg-db：迁移 0029 建 `turn_traces`（turn_id text PK, session_id text, trace_json jsonb, created_at）+ `upsert_turn_trace` + `load_turn_trace`。run_pipeline/spawn_heavy 在关键点累积 trace（BP hash 已在缓存层算，直取；narration_hash 收尾算），收尾 `upsert_turn_trace`（fail-soft，写失败仅 warn）。

### 4.5 transport（trpg-api SSE + trpg-cli）
- API：drain TurnEvent，TurnFailed→`event: error data:{phase,message}`、TurnWarning→`event: warning`。
- CLI `turn`：遇 TurnFailed 打 stderr + 进程非零退出（脚本/回归可判失败，呼应"fail-closed 不伪装成功"）。`play`：红字一行，不崩。
- `trpg explain --session S --turn T`：load_turn_trace → 人读 dump（phases/need 来源/BP hash/失败/警告/pp_lifecycle）。
- `trpg inspect-prompt --turn T --view player|gm`：从持久化的 context_blocks/BP 装配重建该回合 prompt 视图（player 视图剔 GmOnly）。

## 5. 理念守卫（MUST）
1. **fail-closed 不伪装成功**：失败发 TurnFailed + 落 failure_kind，绝不发空 TurnComplete 冒充成功。
2. **等价不回归**：成功回合的 TurnEvent 序列与现状字节等价（Delta…TurnComplete 不变）；新事件仅失败/警告路径；trace/failure_kind 是 write-through 附加。
3. **fail-soft 可观测**：trace/warning 写失败仅 warn，绝不影响回合主流程或已发事件（D2 隔离）。
4. **私骰不泄漏**：inspect-prompt `--view player` 必须剔 GmOnly/私骰；TurnTrace 不存私密 prompt 正文（只存 hash + need 来源元数据），避免 explain 泄底。
5. 文件 ≤400 行；新结构 `#[serde(default)]` 向后兼容。

## 6. 测试与验收
**单测**：phase_error_policy 分类三分支；dispatch_deterministic Err 映射 TurnFailed；merge_need_outcome 累 source_refs；TurnTrace 序列化 round-trip；turn_traces upsert/load。
**等价（硬闸）**：成功回合事件序列前后字节等价（捕获 TurnEvent 列对比）；失败回合（注入 context_assembly Err）从"空 TurnComplete"→`TurnFailed{phase:context_assembly}` + `turns.failure_kind=failed_context`（红绿）。
**live e2e**：真库回合（CoC + Cyberpunk Homecoming）跑通——成功回合 TurnTrace 落 turn_traces（need_trace 带 source_refs、BP hash 非空）、`trpg explain` dump 正确、`inspect-prompt --view player` 不含 GmOnly；注入失败 → TurnFailed + failure_kind + CLI 非零退出。
**守卫**：no-engine-ruleset-hardcode 仍 0；cargo test --workspace 零回归。

## 7. 切分（plan 用，按 crate）
1. trpg-model：TurnTrace/NeedResolutionTrace/PhaseErrorPolicy/TurnFailureRecord 类型 + serde（单测）。
2. trpg-gm：TurnEvent 加 TurnFailed/Warning；dispatch_deterministic→Result；run_pipeline 失败发 TurnFailed（删空 TurnComplete）+ phase_error_policy 分级；trace 累积 + 收尾 upsert。
3. trpg-runtime/trpg-need：merge_need_outcome 捕 source_refs 进 NeedResolutionTrace。
4. trpg-db：迁移 0029（turns.failure_kind + turn_traces）+ upsert/load_turn_trace + set_turn_failure_kind。
5. trpg-api：SSE TurnFailed→error / TurnWarning→warning。
6. trpg-cli：turn 非零退出；explain；inspect-prompt（player 剔 GmOnly）。
7. 等价 + live e2e + 守卫 + 全套件零回归 → 合并 main + push。

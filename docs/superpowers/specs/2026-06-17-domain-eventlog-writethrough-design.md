# 设计：统一 domain_events 日志 + 回合生命周期 write-through（优化2 #6 起步，非侵入）

日期：2026-06-17
状态：设计（自主执行，守 [[宪法]] `docs/architecture/source-grounded-json-runtime.md`）
前置：P0-2 + 优化2 #1（可观测）+ #2/#3（Asset/Binding 影子）已合 main（40260f9）。本切片 = 优化2 §7 EventLog write-through 的**安全起步**。

## 1. 背景
优化2 §7：状态权威逐步转 **EventLog（事实来源）+ Projection（当前视图）**；渐进 **write-through**（改状态时**同时**写一条 domain event，不大爆改）。现状（实测）：已有 `world_events`（migration 0010，**tick 耦合**世界时间内核）+ memory/interaction/frame/object 等 8 个**专用**事件表，但**无统一的、不耦合 tick 的 domain 事件规范源**,状态仍存在各表(非 event-sourced)。
- **不复用 world_events**:它绑 world_tick/event_seq + 世界时间推进语义,回合生命周期事件塞进去会被迫造 tick,语义错位。
- **不动现有专用表/状态写**:本切片**只新增** domain_events 日志 + 回合级 write-through,**零行为变更**,projection 派生留后续(doc 明示先 write-through 后派生)。

## 2. 已拍板决策（自主，守理念）
| 决策点 | 结论 |
|---|---|
| 新日志 | 新 `domain_events` 表(append-only,自带 `seq bigserial` + created_at,**不耦合 tick**)+ `DomainEvent` 类型(trpg-model) |
| 事件种类(起步) | `DomainEventKind{TurnStarted, TurnFinalized, TurnFailed, SceneTransitioned}`(回合生命周期 + 切场景;doc §7 20 种的安全子集,其余随后续切片补) |
| write-through 点 | execute.rs run_pipeline 的既有接缝(obs 切片已熟):入口→TurnStarted;成功路径 TurnComplete 后→TurnFinalized;失败路径→TurnFailed(与 TurnTrace.failure 同源);scene_commit→SceneTransitioned。**fail-soft**(append 失败仅 warn,绝不影响回合) |
| 不做 | mechanical 事件(DiceRolled/CheckResolved/ResourceChanged 等,在 agent loop/kernel 深处,接缝多)→后续;projection 表 + event-derived 状态→后续(高风险,需架构师);改任何现有状态写/专用事件表 |
| surfacing | `trpg explain` 或 `trpg events --session`?——本切片在 `trpg explain` 末尾附该回合 domain events 摘要(复用现命令,不新开),完整时间线 CLI 留后续 |

## 3. 目标 / 非目标
**目标**:统一 append-only `domain_events`(自带 seq)+ `DomainEvent` 类型 + db `append_domain_event`/`list_domain_events`;run_pipeline 回合生命周期 + 切场景 write-through(additive,fail-soft);**零行为变更**(TurnEvent 序列、机械结算、现有表全不变);文件 ≤400;serde default 向后兼容。
**非目标(本切片)**:mechanical/state-patch 级事件;projection 表 + event-sourced 状态(doc 的"逐步派生"留后续,需架构师);改 world_events/memory_events/专用表;event log 成唯一事实源(本切片只是**起步种子**,与现状态存储并存)。

## 4. 设计
### 4.1 trpg-model（DomainEvent）
新 `trpg-model::domain_event` 模块:`DomainEvent{event_id:String, session_id:String, turn_id:String, kind:DomainEventKind, data:serde_json::Value, source_refs:Vec<SourceRef>, created_at:DateTime<Utc>}` + `DomainEventKind{TurnStarted,TurnFinalized,TurnFailed,SceneTransitioned}`(serde,向后兼容)。`#[serde(default)]`。
### 4.2 trpg-db（迁移 + append/list）
迁移 `0030_domain_events.sql`:`create table domain_events(seq bigserial primary key, event_id text unique, session_id text not null, turn_id text, kind text not null, data jsonb, created_at timestamptz default now())` + `idx_domain_events_session(session_id, seq)`;加 migrate() 数组行。
db:`append_domain_event(&DomainEvent)->Result<()>`(insert,event_id 冲突 do nothing 幂等);`list_domain_events(session_id, limit)->Result<Vec<DomainEvent>>`(按 seq);`list_domain_events_for_turn(turn_id)`。
### 4.3 trpg-gm（write-through）
execute.rs run_pipeline:入口 append TurnStarted{turn_id};失败路径(与 set_turn_failure_kind 同处,emit 前)append TurnFailed{phase,failure_kind};成功路径(TurnComplete 后)append TurnFinalized{signal};scene_commit 处 append SceneTransitioned{from,to,reason}。事件 id = `de_{turn_id}_{kind}`(确定性幂等)。**全 fail-soft**(`let _ = ...; warn on err`),**不入控制流、不改 emit**。
### 4.4 trpg-cli(explain)
`format_turn_trace` 后或 explain_cli 内,`list_domain_events_for_turn(turn)` 附 "domain_events:" 摘要(kind + 简数据)。

## 5. 理念守卫（MUST）
1. **零行为变更**:write-through 是纯附加 DB 写;TurnEvent 序列/机械结算/现有表不变;obs 切片等价测试继续绿。
2. **fail-soft**:append 失败仅 warn,绝不影响回合或已发事件。
3. **不耦合**:domain_events 自带 seq,不依赖 world_tick;不动 world_events/专用表。
4. **零规则集硬编码**:事件 kind/数据通用,不按规则集名(守卫 0)。
5. 文件 ≤400;serde default;不删/不改现有状态写。

## 6. 测试与验收
**单测**:DomainEvent round-trip + back-compat;append/list 幂等(同 event_id 二次 append 不重复)；db round-trip(live-gated)。
**等价(硬闸)**:成功+失败回合 TurnEvent 序列字节等价(obs 等价测试绿);机械结算不变。
**live e2e**(真库):CoC + Cyberpunk 回合→domain_events 落 TurnStarted+TurnFinalized(+SceneTransitioned 若切场景);失败回合→TurnFailed;`trpg explain` 显示 domain events;机械结果与切片前一致。
**守卫**:no-engine-ruleset-hardcode 0;cargo test --workspace 零回归。

## 7. 切分
1. trpg-model DomainEvent/Kind + trpg-db 迁移 0030 + append/list(单测,live-gated round-trip)。
2. trpg-gm run_pipeline 4 点 write-through(fail-soft) + trpg-cli explain 附 domain events。
3. 等价 + live e2e + 守卫 + 全套件零回归 → 合并 main + push。

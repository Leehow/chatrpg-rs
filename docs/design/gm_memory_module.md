# GM Memory Module v0.2

目标：让 GM 能记住之前发生过什么，同时不破坏 BP1/BP2/BP3 的缓存稳定性。

## 1. 核心原则

1. **Raw turns 不进长期 prompt。** 每轮玩家输入和 GM 输出先写入 `turns` 与 `memory_events`，只作为事实来源和后处理输入。
2. **长期真相写数据库。** 规则、模组和当前状态仍由 ContextBlock/WorldState 管；记忆由 `memory_events`、`memory_facts`、`memory_snapshots` 管。
3. **BP2 只接收稳定快照。** `memory_snapshots` 进入 BP2 PinnedMiddle，但只有显式 compaction、场景切换、章节切换或达到阈值时更新。
4. **BP3 只接收本轮检索。** 相关旧事件和事实以 `RetrievedMemory` 进入 BP3，设置 turn TTL，不污染聊天历史。
5. **LLM 可以提出记忆候选，但不能直接改状态。** 后处理可以抽取 MemoryCandidate，最后由 validator/upsert 写入 facts/snapshots。

## 2. 数据层

### memory_events

Append-only 事件表。每个 turn 后台写一条事件，包含：

- session_id / turn_id / ruleset_id / module_id
- scene_id / location_id / actor_ids
- summary
- transcript_excerpt
- source_json
- visibility
- tags
- importance

它用于“发生过什么”的原始回忆来源，不直接进 BP2。

### memory_facts

可更新事实表。用于保存已经确认的长期事实，例如：

- NPC 与 PC 的关系变化
- 已发现线索
- 玩家承诺
- 场景后果
- 反复出现的人物、地点、组织状态

字段包括 subject / predicate / object_json / summary / status / confidence / source_event_ids / tags。

### memory_snapshots

缓存稳定摘要表。用于 BP2：

- session summary
- scene summary
- chapter/mission summary
- open threads
- player-facing discovered facts slice
- GM-only secret continuity slice

Snapshot 有独立 version 和 content_hash。只要 snapshot 不更新，BP2 hash 就不变。

## 3. 运行时加载

```text
RuntimeEngine.prepare_turn_context
  → load rule/module ContextBlocks
  → load MemorySnapshot blocks into BP2
  → retrieve MemoryEvent/MemoryFact blocks into BP3
  → add world_state/current_input/recent_transcript
  → visibility projection
  → ContextBuilder true three-band compile
```

### BP1

不放 session memory。BP1 只放 engine protocol、ruleset resident core、director policy、procedure index、character kernel。

### BP2

只放缓存稳定记忆：

- current session/chapter/scene memory snapshot
- stable open threads
- stable discovered clue summary
- stable relationship summary

### BP3

只放本轮动态回忆：

- query-relevant recent events
- exact facts retrieved for current player input
- unsummarized recent transcript tail
- validator feedback / pending state proposal

## 4. SSE 后处理

SSE 仍然先流式输出正文：

```text
SSE start
SSE llm_stream_start
SSE delta...
SSE postprocess_scheduled
SSE done
```

后台 job 再做：

```text
save_turn
save_memory_event
optional extract memory candidates
optional update facts
optional schedule compaction
```

这样不会阻塞用户输出。

## 5. 缓存稳定策略

| 操作 | BP1 | BP2 | BP3 |
|---|---|---|---|
| 只改用户输入 | 不变 | 不变 | 变化 |
| 新增 memory_event | 不变 | 不变 | 可能变化 |
| 手动 /compact-memory | 不变 | 变化 | 可能变化 |
| 场景切换并生成 scene snapshot | 不变 | 变化 | 可能变化 |
| 规则 resident repair | 变化 | 可能不变 | 可能变化 |

## 6. API

```text
GET  /api/sessions/{session_id}/memory
POST /api/sessions/{session_id}/memory/retrieve
POST /api/sessions/{session_id}/memory/compact
```

## 7. CLI

```text
/memory          查看当前 session 的 snapshots 和最近 events
/compact-memory 生成一个 BP2 稳定 memory snapshot
```

## 8. 下一步

v0.2 先实现事件、快照、简单关键词检索。后续可加：

- LLM 后台抽取 MemoryCandidate
- fact contradiction validator
- pgvector / hybrid retrieval
- clue ledger 专用 memory projection
- player-visible memory projection

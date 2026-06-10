# v0.7 Runtime Search Load and Self-Learning Loop

Status: implementation scaffold  
Primary crates: `trpg-runtime`, `trpg-search`, `trpg-db`, `trpg-api`, `trpg-cli`

## 1. Decision

v0.7 connects unified Tantivy search to the live GM turn path. Search is no longer only a manual `trpg rg` or `/api/search` capability; it can now run during a player turn, record a lookup event, compile relevant search hits into TTL-scoped `ContextBlock`s, and load them through the same BP1/BP2/BP3 planner as rulebook, module, memory, and learned-packet content.

The design preserves the core cache invariant:

```text
SearchHit itself is never prompt material.
SearchHit -> SearchLoadRequest -> ContextBlock -> RuntimeMaterialPlanner -> BP1/BP2/BP3.
```

## 2. Runtime flow

```text
Player input
  -> RuntimeEngine.prepare_turn_context
  -> load base ruleset/module blocks
  -> load runtime-persisted search blocks for the current turn/scene
  -> auto_search_blocks_for_turn
      -> lightweight rule/module demand detector
      -> SearchService.search_async
      -> lookup_events insert
      -> SearchLoadRequest per hit
      -> transient BP3 block or scene-pinned BP2 block
  -> memory retrieval
  -> learned packet loading
  -> ContextBuilder true three-band compile
  -> LLM stream
  -> turn/memory postprocess
```

## 3. Environment flags

```text
TRPG_RUNTIME_AUTO_SEARCH=true
TRPG_RUNTIME_AUTO_SEARCH_LIMIT=5
TRPG_RUNTIME_AUTOPIN_SCENE_RULES=true
TRPG_SEARCH_CJK_EXPANSION=true
```

`TRPG_RUNTIME_AUTO_SEARCH=false` disables the live retrieval loop without disabling manual `trpg rg` or `/api/search`.

## 4. Demand detection

The current detector is intentionally lightweight. It triggers when player input looks rule- or module-sensitive, including:

```text
English: check, roll, rule, dc, dv, skill, attack, combat, damage, hack, netrun, drone, clue, scene, npc, where, how
Chinese: 检定, 判定, 规则, 技能, 攻击, 战斗, 伤害, 治疗, 黑客, 黑入, 无人机, 线缆, 线索, 调查, 地点, 怎么, 能不能
```

This is a conservative v0.7 heuristic, not a final classifier. Later versions can replace it with an LLM or ruleset-aware `RuleDemandDetector` while keeping the same output contract.

## 5. TTL and cache-zone policy

Default mapping:

```text
Search hit loaded for one ruling:
  cache_zone = dynamic_tail
  ttl = turn
  expires_at_turn = current turn

Search hit likely relevant to the active scene:
  cache_zone = pinned_middle
  ttl = scene
  expires_at_scene = current scene

Stable learned packet:
  loaded from learned_packets table into BP2

Memorized learned packet:
  can become a BP1 candidate only after explicit review
```

This keeps cache behavior predictable:

```text
Only player input changes:
  prefix_hash unchanged
  pinned_hash unchanged
  dynamic_hash changes

One-off rule lookup:
  prefix_hash unchanged
  pinned_hash unchanged
  dynamic_hash changes

Scene-pinned rule packet:
  prefix_hash unchanged
  pinned_hash changes

Resident ruleset change:
  prefix_hash changes
```

## 6. `/api/search/load`

`POST /api/search/load` now persists a loaded hit when requested and returns:

```json
{
  "block": {},
  "persisted": true,
  "lookup_event_id": "lookup_..."
}
```

It records `lookup_events` even when the loaded block is ephemeral. This makes retrieval auditable without forcing all hits into stable cache.

## 7. CLI behavior

Search commands support both full and incremental indexing:

```bash
trpg search reindex
trpg search reindex --incremental
trpg rg "Hacking Athena" --module cyberpunk_red.homecoming --reindex --incremental-reindex
```

Runtime commands use search automatically when available:

```bash
trpg play --ruleset cyberpunk_red --module cyberpunk_red.homecoming
trpg turn --ruleset cyberpunk_red --module cyberpunk_red.homecoming --input "我检查无人机背后的线缆。" --stream-format jsonl
```

The `context_compiled` phase shows whether BP1/BP2/BP3 changed.

## 8. CJK and bilingual query expansion

v0.7 adds a CJK helper field in the Tantivy index. Chinese/Japanese/Korean characters are indexed as single characters and bigrams. Runtime search also expands common Chinese TRPG demands into English search terms, for example:

```text
黑客 / 黑入 -> netrunning, hack, net, architecture
无人机 -> drone, vehicle, robot, athena
线缆 -> cable, tech, basic, repair
检定 -> check, skill, roll, dv, dc
```

This is not a full Chinese tokenizer. It is a pragmatic bridge until a proper CJK tokenizer or query rewrite model is added.

## 9. Incremental indexing

`SearchService::reindex_incremental` uses `search_index_watermarks` per source config rather than a global timestamp. This matters because the searchable corpus is dynamic: adding a new DB table or file root should not require changing the Rust search engine.

SQL source configs are wrapped as:

```sql
select * from (<source sql>) as trpg_search_source where updated_at > $1
```

File and JSONL sources compare filesystem modification time with the per-source watermark. A full rebuild clears the Tantivy index and rewrites source watermarks; incremental indexing upserts only changed docs.

Limitations:

```text
Deleted rows are not removed from the Tantivy index yet.
Moved files may leave stale docs until a clean rebuild/reset feature exists.
Per-source watermarks depend on source queries exposing a reliable updated_at field.
```

## 10. Open issues

1. Clean deletion for removed source rows/files.
2. Better CJK tokenization beyond pragmatic char/bigram indexing.
3. Automatic post-session audit and learned packet promotion.
4. Stronger rule demand detector.
5. Source excerpt expansion around page anchors.
6. Search result diversification so one source cannot dominate all top hits.

## 11. Stable loaded-block identity

`SearchLoadRequest::to_context_block` uses a TTL-aware stable seed:

```text
turn TTL    -> search_doc_id + session_id + turn_id + scene_id + cache_zone + ttl
scene TTL   -> search_doc_id + session_id + scene_id + cache_zone + ttl
session TTL -> search_doc_id + session_id + cache_zone + ttl
```

This prevents scene-pinned search packets from receiving a new block id on every turn. Scene-pinned results therefore remain stable across turns until the active scene changes, which keeps `pinned_hash` stable after the first load.

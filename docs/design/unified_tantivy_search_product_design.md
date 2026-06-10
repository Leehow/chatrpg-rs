# LLM GM Unified Search Product Design

Version: v0.7  
Status: runtime-connected implementation scaffold  
Primary crate: `trpg-search`  
Primary engine: Tantivy  
Primary interface: CLI + API, no web page

---

## 1. One-sentence product thesis

The LLM GM needs a fast, source-backed, explainable retrieval layer that behaves like `ripgrep` for TRPG operations, but searches across rulebooks, modules, parsed artifacts, database state, GM memory, rulings, and learned packets through one normalized index.

The search layer is not a secondary helper. It is the core mechanism that lets the GM start with onboarding-level familiarity, look things up during play, produce source-backed or provisional rulings, and promote frequently used knowledge into learned packets.

---

## 2. Why Tantivy now

The project is moving away from full upfront parsing. That means the runtime depends more heavily on precise retrieval:

```text
Onboard ruleset
  -> build locator map
  -> prep first session
  -> start play
  -> retrieve exact rules / module details / memory when demanded
  -> compile temporary packet
  -> record ruling
  -> promote repeated knowledge
```

PostgreSQL FTS could support the first version, but Tantivy is a better fit now because:

1. Search must cover both DB rows and files/artifacts.
2. The index should be local, fast, embedded, and Rust-native.
3. Search source definitions should be decoupled from business tables.
4. The project will eventually index large corpora: rulebook pages, module pages, context blocks, memories, rulings, and JSONL artifacts.
5. Search needs BM25-style full-text ranking, phrase queries, field boosts, stored snippets, and future extension points.

Tantivy is used as an embedded index library, not as a separate service. PostgreSQL remains the source of truth for runtime state.

---

## 3. Product goals

### 3.1 User goals

A GM or tester should be able to run:

```bash
trpg rg "Hacking Athena" --domain modules --module cyberpunk_red.homecoming
trpg rg "Basic Tech" --domain rules --ruleset cyberpunk_red
trpg rg "Foxwell" --domain memory --session session_123
trpg rg "Athena" --jsonl --explain
```

And receive:

```text
score
origin
logical kind
title
snippet
source refs
scope metadata
ranking explanation when requested
```

### 3.2 Runtime goals

The runtime should be able to:

1. Convert player intent into search queries.
2. Search rules, modules, memory, rulings, and learned packets through the same API.
3. Select a hit.
4. Convert the hit into a TTL-scoped `ContextBlock`.
5. Load the block into BP3 by default, BP2 only when pinned by scene/session policy.
6. Record lookup event and ruling event.
7. Promote repeated search/ruling patterns into learned packets.

### 3.3 Engineering goals

1. Do not hardcode search scope to current database tables.
2. Do not make Tantivy know TRPG business schema details.
3. Make source config editable without recompilation.
4. Support DB schema evolution by changing `search_source_configs` rows.
5. Keep visibility filtering inside search.
6. Keep prompt context stable: search hits are not prompt content until explicitly loaded as blocks.

---

## 4. Non-goals

This version does not implement:

1. Vector embeddings.
2. Semantic reranking.
3. Web UI.
4. Distributed search.
5. Automatic learned packet promotion.
6. Full source excerpt repair workflow.

Those can be added later. Tantivy is the first-stage retrieval engine.

---

## 5. Core design principle: normalized SearchDocument

All searchable information becomes a `SearchDocument`:

```rust
pub struct SearchDocument {
    pub search_doc_id: String,
    pub origin: String,
    pub domain: String,
    pub logical_kind: String,
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
    pub scopes: BTreeMap<String, String>,
    pub visibility: Visibility,
    pub stability: Stability,
    pub source_refs: Vec<SourceRef>,
    pub metadata: serde_json::Value,
    pub updated_at: DateTime<Utc>,
}
```

Important: `origin`, `domain`, and `logical_kind` are strings, not enums. This is deliberate. New source types should not require Rust model migrations.

Examples:

```json
{
  "search_doc_id": "book_locator:cyberpunk_red.getting_it_done",
  "origin": "book_locator",
  "domain": "rules",
  "logical_kind": "core_resolution",
  "title": "Getting it Done",
  "body": "General action resolution and resolving actions with skills...",
  "scopes": {"ruleset_id":"cyberpunk_red"}
}
```

```json
{
  "search_doc_id": "memory_event:memory.event.turn_123",
  "origin": "memory_event",
  "domain": "memory",
  "logical_kind": "event",
  "title": "Player checked Athena's cable",
  "body": "Player inspected the cable connected to the rogue drone...",
  "scopes": {"session_id":"session_123","module_id":"cyberpunk_red.homecoming"}
}
```

---

## 6. Configurable source registry

Search scope is configured in PostgreSQL:

```sql
search_source_configs (
  source_config_id text unique,
  source_kind text,
  label text,
  enabled boolean,
  priority integer,
  config_json jsonb
)
```

Supported `source_kind` values:

```text
sql_query
file_glob
jsonl
```

### 6.1 SQL source

A SQL source returns normalized `SearchDocument` columns:

```text
search_doc_id text
origin text
domain text
logical_kind text
title text
body text
tags text[]
visibility text
stability text
scope_json jsonb
source_refs jsonb
metadata jsonb
updated_at timestamptz
```

The Rust indexer does not know which table the rows came from. It only requires this shape.

If the database schema changes, update the SQL query in `search_source_configs.config_json.sql`.

### 6.2 File source

A file source indexes Markdown/plain text under `data/`:

```json
{
  "root": "markdown",
  "extensions": ["md", "txt"],
  "origin": "markdown_file",
  "domain": "source",
  "logical_kind": "source_excerpt",
  "visibility": "gm_only"
}
```

### 6.3 JSONL source

A JSONL source indexes parser artifacts:

```json
{
  "root": "parsed",
  "origin": "jsonl_artifact",
  "domain": "parsed",
  "visibility": "gm_only"
}
```

This lets `context_blocks.jsonl` and `material_index.jsonl` be searchable even outside the DB-backed workflow.

---

## 7. Default indexed sources

The migration seeds these source configs:

```text
db.context_blocks
db.material_index
db.book_locator_entries
db.memory_events
db.memory_facts
db.memory_snapshots
db.rulings_log
db.learned_packets
files.markdown
files.parsed_jsonl
```

The seed list reflects the current schema but is not compiled into `trpg-search`. Future versions can disable, replace, or add configs at runtime.

---

## 8. Search request contract

```rust
pub struct SearchRequest {
    pub query: String,
    pub mode: SearchMode,
    pub domains: Vec<String>,
    pub kinds: Vec<String>,
    pub tags: Vec<String>,
    pub scopes: BTreeMap<String, String>,
    pub filters: BTreeMap<String, Vec<String>>,
    pub limit: u32,
    pub explain: bool,
    pub viewer: VisibilityProfile,
}
```

### 8.1 Domains

Examples:

```text
rules
modules
memory
rulings
learned
source
parsed
```

### 8.2 Kinds

Examples:

```text
procedure
scene_node
npc_static
location_static
memory_event
memory_snapshot
book_locator
learned_packet
```

### 8.3 Scopes

Scopes are generic. The search layer does not own a fixed scope list.

Common examples:

```text
ruleset_id=cyberpunk_red
module_id=cyberpunk_red.homecoming
session_id=session_123
scene_id=homecoming.chapter_1.lift_the_curtain
bundle_id=rulebook.cyberpunk_red.core
scope_type=scene
scope_id=homecoming.chapter_1.lift_the_curtain
```

### 8.4 Filters

Filters are generic exact filters. They match scopes, facets, or string metadata.

Example:

```json
{
  "filters": {
    "origin": ["book_locator"],
    "learning_stage": ["stable", "memorized"]
  }
}
```

---

## 9. Search response contract

```rust
pub struct SearchHit {
    pub hit_id: String,
    pub search_doc_id: String,
    pub origin: String,
    pub domain: String,
    pub logical_kind: String,
    pub title: String,
    pub snippet: String,
    pub score: f32,
    pub scopes: BTreeMap<String, String>,
    pub tags: Vec<String>,
    pub visibility: Visibility,
    pub source_refs: Vec<SourceRef>,
    pub metadata: serde_json::Value,
    pub explain: serde_json::Value,
}
```

Search hits are evidence candidates. They are not automatically prompt material.

---

## 10. BP1/BP2/BP3 cache rules

Search must not destabilize long-context caching.

Default behavior:

```text
SearchHit only:
  no prompt change

SearchHit loaded as turn lookup:
  BP3 DynamicTail
  TTL = turn

SearchHit loaded as current scene material:
  BP2 PinnedMiddle
  TTL = scene

LearnedPacket used_once:
  BP3 or low-priority BP2

LearnedPacket stable:
  BP2

LearnedPacket memorized:
  BP1 candidate only after review
```

Expected hashes:

```text
just searching:
  prefix_hash same
  pinned_hash same
  dynamic_hash same

loading a temporary lookup:
  prefix_hash same
  pinned_hash same
  dynamic_hash changed

pinning a scene packet:
  prefix_hash same
  pinned_hash changed
  dynamic_hash changed or same

promoting to resident core:
  prefix_hash changed
```

---

## 11. Visibility policy

Search applies visibility filtering before returning hits.

```text
GM/System:
  public, player_visible, gm_only, npc_private, system_only

Player:
  public, player_visible

NPC:
  public, player_visible, npc_private
```

GM-only and playwalled content must never leak into player-facing search. This matters for games and modules that explicitly separate player, GM, and restricted sections.

---

## 12. Runtime flow

```text
Player input
  -> RuleDemandDetector
  -> SearchRequest
  -> Tantivy SearchService
  -> SearchHit candidates
  -> MaterialResolver / PacketCompiler
  -> ContextBlock with TTL
  -> RuntimeMaterialPlanner
  -> ContextBuilder BP1/BP2/BP3
  -> LLM GM
  -> lookup_event + ruling_log
  -> learned_packet promotion later
```

Current v0.6 implements the index, CLI/API search, configurable sources, and search-hit-to-block conversion. Automatic demand detection and promotion remain future work.

---

## 13. CLI design

### 13.1 Search like ripgrep

```bash
trpg rg "Athena" --domain modules --module cyberpunk_red.homecoming
```

### 13.2 Search rules

```bash
trpg rg "Basic Tech" --domain rules --ruleset cyberpunk_red
```

### 13.3 Search memory

```bash
trpg rg "Foxwell" --domain memory --session session_123
```

### 13.4 Reindex first

```bash
trpg rg "Hacking Athena" --reindex
```

### 13.5 Machine-readable results

```bash
trpg rg "Athena" --jsonl
```

### 13.6 Generic, schema-independent filters

```bash
trpg rg "Athena" \
  --scope module_id=cyberpunk_red.homecoming \
  --filter origin=book_locator \
  --explain
```

### 13.7 Manage source configs

```bash
trpg search sources
trpg search reindex
trpg search query "Friday Night Firefight" --domain rules
```

---

## 14. API design

### 14.1 Search

```text
POST /api/search
```

Request:

```json
{
  "query": "Hacking Athena",
  "domains": ["modules"],
  "scopes": {"module_id": "cyberpunk_red.homecoming"},
  "limit": 8,
  "explain": true,
  "viewer": {"viewer_kind":"gm","player_id":null,"actor_id":null,"can_see_gm_only":true}
}
```

### 14.2 Reindex

```text
POST /api/search/reindex
```

### 14.3 Source config list

```text
GET /api/search/sources
```

### 14.4 Source config upsert

```text
POST /api/search/sources
```

Body:

```json
{
  "source_config_id": "db.custom_table",
  "source_kind": "sql_query",
  "label": "Custom table",
  "enabled": true,
  "priority": 50,
  "config_json": {
    "sql": "select ... as search_doc_id, ..."
  }
}
```

### 14.5 Load hit into context

```text
POST /api/search/load
```

This converts a `SearchHit` into a TTL-scoped `ContextBlock`. Runtime policy decides whether it is actually loaded into BP2 or BP3.

---

## 15. Index lifecycle

### 15.1 Parse flow

```text
parse-all
  -> write DB rows
  -> write JSON/JSONL artifacts
  -> reindex enabled search sources
```

### 15.2 Runtime flow

For v0.6, runtime memory writes are not yet automatically pushed into Tantivy after every turn. Use:

```bash
trpg search reindex
```

or:

```text
POST /api/search/reindex
```

Future versions should incrementally upsert memory/ruling/learned documents after each write.

### 15.3 Stale documents

Tantivy documents are immutable. Updates are implemented by deleting the old document key and adding a new document with the same `search_doc_id`.

If a source disappears, stale documents may remain until a clean rebuild strategy is added. v0.6 focuses on upsert indexing. Future work should add:

```text
trpg search reset
trpg search reindex --clean
source generation markers
source tombstones
```

---

## 16. Ranking model

Tantivy provides BM25-style first-stage ranking over boosted fields:

```text
title   high boost
body    normal boost
tags    medium boost
facets  low-medium boost
```

Post-filtering then applies:

```text
visibility
domain
kind
tag
scope
exact metadata/facet filters
```

Future ranking improvements:

```text
active scene boost
active module boost
learned packet confidence boost
source-backed ruling boost
recent memory decay
module chapter proximity
manual pin boost
```

---

## 17. Security and safety

1. Search does not bypass visibility.
2. Player-facing search only returns public/player-visible hits.
3. Search hits are not committed state.
4. Search hits are not prompt material until loaded as ContextBlocks.
5. Rulings created from low-confidence hits must be marked provisional.
6. Source refs are preserved for audit.

---

## 18. Future roadmap

### v0.7

```text
incremental indexing hooks after DB writes
trpg search reset / reindex --clean
search/load integrated into turn context
rule demand detector
source excerpt packet compiler
```

### v0.8

```text
post-session audit
learned packet auto-promotion
ranker policies per ruleset/module
search telemetry in turn trace
```

### v0.9+

```text
semantic second-stage retrieval
vector hybrid search
cross-campaign memory controls
multi-user search visibility partitions
```

---

## 19. Acceptance tests

### 19.1 Homecoming

```bash
trpg rg "Hacking Athena" --domain modules --module cyberpunk_red.homecoming --reindex
```

Expected: returns module source or locator hits for Athena/hacking/current chapter.

### 19.2 Cyberpunk RED rules

```bash
trpg rg "Netrunning" --domain rules --ruleset cyberpunk_red
```

Expected: returns Cyberpunk RED locator/rule hits.

### 19.3 Memory

```bash
trpg turn --ruleset cyberpunk_red --module cyberpunk_red.homecoming --input "我检查无人机背后的线缆。"
trpg search reindex
trpg rg "无人机 线缆" --domain memory
```

Expected: returns the turn memory event.

### 19.4 Config extensibility

Insert a new `search_source_configs` row with a SQL query over a new table. Run:

```bash
trpg search reindex
trpg rg "known text from new table"
```

Expected: no Rust code changes needed.

---

## 20. Final product judgment

Tantivy search should be treated as the GM's working memory lookup layer. It lets the GM behave like a human GM: know where things are, find them quickly, rule with sources when possible, mark uncertainty when not, and remember what is used repeatedly.

The critical architectural boundary is this:

```text
Search finds evidence.
MaterialResolver turns evidence into material.
RuntimeMaterialPlanner decides cache layer.
ContextBuilder renders BP1/BP2/BP3.
Validator commits state.
```

Keeping those boundaries separate prevents search from becoming a prompt-injection path, a state mutation path, or a cache-destabilizing path.


---

## v0.7 Runtime Connection Addendum

v0.7 promotes search from manual infrastructure into the live GM context loop. The core product promise remains unchanged: search should feel like a TRPG-aware `ripgrep`, but results only affect prompts after they become TTL-scoped `ContextBlock`s.

### New runtime capabilities

```text
Player input
  -> lightweight demand detector
  -> Tantivy SearchService
  -> lookup_events audit row
  -> SearchLoadRequest
  -> ContextBlock with ttl=turn or ttl=scene
  -> BP3 or BP2 through ContextBuilder
```

### Cache contract

```text
Search hit retrieved but not loaded:
  no prompt hash changes

Turn lookup loaded:
  BP3 dynamic_hash changes only

Scene-pinned lookup loaded:
  BP2 pinned_hash changes, BP1 unchanged

Resident rule/kernel update:
  BP1 prefix_hash changes
```

### CJK bridge

Tantivy still uses a simple tokenizer path in v0.7, but the index now stores a helper `cjk_ngrams` field and query rewriting adds bilingual aliases for common TRPG demands. This makes Chinese turn inputs more likely to retrieve English rulebooks and modules without requiring full semantic search.

### Incremental index

`SearchService::reindex_incremental` uses `search_index_watermarks` per source config. SQL source configs must expose an `updated_at` column; file/jsonl sources use filesystem modification time. Clean deletion remains a v0.8 problem.

### Open product work

1. Replace heuristic demand detection with a ruleset-aware classifier.
2. Add `trpg search reset` and clean deletion.
3. Persist loaded hit packets with stronger TTL garbage collection.
4. Add generation-based clean deletion for stale Tantivy docs.
5. Build post-session audit and learned-packet promotion.

# Known Issues and Fix Log

## 一、代码 Bug：导致编译/运行失败，已修

| # | 文件 | 问题 | 修复 |
|---|---|---|---|
| 1 | `crates/trpg-ingest/Cargo.toml` | `lib.rs` 使用 `serde_json::json!`，但 crate 没声明 `serde_json` 依赖，导致 `E0433`。 | 增加 `serde_json.workspace = true`。 |
| 2 | `crates/trpg-api/src/lib.rs` | 两个 SSE handler 返回类型写死为 `Sse<ReceiverStream<...>>`，但 `.keep_alive(...)` 改变了实际类型，导致 `E0308`。 | 返回类型改为 `impl IntoResponse`，并引入 `axum::response::IntoResponse`。 |
| 3 | `crates/trpg-llm/src/lib.rs` | `complete_text`、`complete_json`、`stream_chat` 无条件发送 `temperature`，部分 OpenAI-compatible relay / gpt-5.x relay 拒收该参数，导致 HTTP 400。 | 增加 `LlmConfig.send_temperature`，通过 `TRPG_LLM_SEND_TEMPERATURE` 控制；默认对 `codex-relay` 不发送 temperature；统一通过 `base_body()` 构造请求体。 |
| 4 | `crates/trpg-api/src/lib.rs` | `create_character_sse` 在 `json!` 里移动了 `req.ruleset_id` / `req.module_id`，后续 postprocess 又借用 `req.ruleset_id`，有潜在 partial move 编译问题。 | 使用 `.clone()` 写 job input，并提前复制 `ruleset_id_for_postprocess`。 |

## 二、环境阻断：不是代码 Bug，但会卡住运行，已绕过

| # | 现象 | 根因 | 处理 |
|---|---|---|---|
| 5 | `migrate` 报 `role "chatrpg" does not exist`，但 Docker PostgreSQL healthy。 | 本机 Homebrew PostgreSQL 占用 `127.0.0.1:5432`，连接打到了本机库，不是容器库。 | 不动本机 PG，Docker 端口改为 `54323:5432`；`.env.example` 与 CLI 默认 DATABASE_URL 改为 `postgres://chatrpg:chatrpg@localhost:54323/chatrpg`。 |

## 三、功能问题：不崩，但仍需继续提升

| # | 现象 | 影响 | 当前状态 |
|---|---|---|---|
| 6 | CharacterTemplate 抽取经常失败：`LLM character template did not match CharacterTemplate`。 | 会退回通用 fallback，Cyberpunk RED / Triangle / Sword World 专属角色卡字段不足。 | 已增加 tolerant normalizer：支持 `{character_template:{...}}`、`{template:{...}}`、fields/sections 对象或数组、`id/name/type` 等别名；仍需要用真实解析结果继续调 schema 和 prompt。 |
| 7 | Parser 串行处理大 PDF，速度慢。 | `parse-all` 对 Cyberpunk RED / Sword World / Masks 这种大书耗时较长。 | 未作为 bug 修。后续做 bounded concurrency、chunk_pages/max_chunk_chars 可配置、失败 chunk repair。 |
| 8 | 记忆模块此前缺失。 | 能跑单轮，但长期 session 容易忘记承诺、NPC 关系、线索、事件后果。 | 已加 GM memory v0.2 骨架：`memory_events`、`memory_facts`、`memory_snapshots`、memory API、CLI `/memory` 与 `/compact-memory`、BP2 snapshot + BP3 retrieval。 |
| 9 | OpenAPI 还不是完整 schema。 | 可测试接口，但还没有完整 DTO schema 自动生成。 | 先保留 `/api-doc/openapi.json` 静态接口索引；后续接 Utoipa derive，不做 Swagger UI 页面。 |

## 警告：暂不处理

- `trpg-ingest/src/lib.rs` 中的多余 `mut`、unused imports：不影响运行。
- 已移除 `serde_yaml`：项目导出与 LLM structured output 统一使用 JSON / JSONL。
- `pdftotext` 只支持纯文本 PDF：扫描版和图片表格暂不处理。

## v0.3 格式调整

- 已正式移除 YAML/TOML 路径：解析产物、角色草稿、API structured output、CLI inspect 输出统一使用 JSON。
- 大集合导出为 JSONL：`context_blocks.jsonl` 与 `material_index.jsonl`。
- `parsed_bundles` 的导出路径字段从语义上改为 `artifact_path`。新库直接使用 `artifact_path`；旧 v0.2 库可通过重新运行 `migrate` 自动添加该列，重新 `parse-all` 后会写入新路径。

## v0.4 CLI automation update

| # | 类别 | 现象 | 处理 |
|---|---|---|---|
| 10 | 功能/测试阻断，已修 | `trpg create-character` 依赖 `dialoguer` 交互式输入，管道喂 stdin 会报 `not a terminal`，导致 LLM、CI、脚本很难驱动。 | `create-character` 增加 `--preferences`、`--preferences-file`、`--stdin`、`--request-json`、`--stream-format text/jsonl/sse`、`--no-save`。没有显式参数且 stdin 不是 TTY 时，会自动把 stdin 当作 preferences。 |
| 11 | 功能/测试阻断，已修 | `trpg play` 是 REPL，不适合单轮自动化测试；stdin EOF 时原实现可能空转。 | 增加 `trpg turn` 单轮非交互命令；支持 `--input`、`--input-file`、`--stdin`、`--request-json`、`--stream-format`。`play` 在 stdin EOF 时退出。 |
| 12 | 调试体验，已修 | CLI 流式输出只有人类文本，测试程序不容易区分 phase、delta、context hash、memory_event。 | 增加 `jsonl` 与 `sse` 两种机器可读流格式。默认 `text` 模式将 metadata 写 stderr，LLM delta 写 stdout。 |

## v0.5 design shift: full parse no longer default

| # | Category | Issue | Status |
|---|---|---|---|
| 14 | Functional design | `parse-all` previously tried to chunk-extract entire rulebooks/modules before play. This is slow and overfits to a full-structured-database model. | Changed default to GM onboarding + book locator + first-session module prep. Full chunk extraction is now opt-in via `--full-parse` or `TRPG_PARSE_FULL_CHUNKS=true`. |
| 15 | Runtime learning | Learned packets, lookup events, and rulings now have schema/database support, but full automatic post-session audit and promotion policy are still skeletal. | Partially implemented; next step is a background `learning_audit` job. |
| 16 | Rule lookup | `/api/rules/lookup` returns locator hits and records lookup events. `/api/rulings` and `/api/learned-packets` can record rulings and promote packets manually. It does not yet stream source search or auto-compile temporary packets. | Partially implemented; auto audit/promotion pending. |

## v0.6 Tantivy unified search update

| # | Category | Issue | Status |
|---|---|---|---|
| 17 | Infrastructure, implemented | Unified search was previously limited to `book_locator_entries` and hardcoded lookup logic. | Added `trpg-search`, Tantivy index, `/api/search`, `/api/search/reindex`, `/api/search/sources`, `/api/search/load`, CLI `trpg rg`, and `trpg search` commands. |
| 18 | Schema evolution, implemented | Search source scope should not be hardcoded because DB tables and parser artifacts will change. | Added PostgreSQL `search_source_configs`; source definitions are SQL/file/JSONL configs returning normalized `SearchDocument` rows. Updating search scope no longer requires Rust code changes. |
| 19 | Migration bug, fixed | `memory_snapshots` table definition had an extra closing `);` in v0.5 starter migration. | Removed the stray close and added the missing stable close around the table definition. |
| 20 | Runtime SQL bug, fixed | `insert_lookup_event` bound `created_at` twice while SQL had 11 placeholders. | Removed the extra bind. |
| 21 | Material search completeness, improved | `material_index` had no persisted stability column even though `MaterialIndexEntry` has `stability`. | Added `material_index.stability` migration and updated `upsert_material_entry`. |
| 22 | Tantivy lifecycle, still partial | Reindex upserts documents by `search_doc_id`, but does not yet implement clean deletion for removed source rows/files. | Still open: `trpg search reset`, `trpg search reindex --clean`, source generation markers. |
| 23 | Runtime automation, implemented in v0.7 scaffold | Player turn flow now runs a lightweight rule/module demand detector, calls `SearchService`, writes `lookup_events`, and loads selected hits as TTL `ContextBlock`s. | Implemented with heuristics; still needs stronger rule-demand classification and post-session audit. |
| 24 | Build verification, environment limitation | The assembly environment has no `cargo`/`rustc`, so Tantivy API integration was not compiled in-container. | First local build may require minor Tantivy API adjustments. The implementation targets Tantivy 0.26 APIs. |


## v0.7 runtime search/load update

| # | Category | Issue | Status |
|---|---|---|---|
| 25 | Runtime retrieval, implemented | Manual search existed, but live GM turns did not automatically retrieve rule/module evidence. | `RuntimeEngine.prepare_turn_context` now calls auto search when input is rule/module sensitive. Hits are converted to `ContextBlock`s via `SearchLoadRequest`. |
| 26 | Cache stability, implemented | Retrieved search hits could destabilize BP2 if treated as ordinary memory. | Turn lookups default to BP3 with turn TTL. Scene-relevant hits can be pinned to BP2 with scene TTL. BP1 is never changed automatically. |
| 27 | Search/load audit, implemented | Loading a search hit into runtime context was not auditable. | `/api/search/load` and runtime auto search insert `lookup_events` with hit payloads and status. |
| 28 | CJK search, improved | Tantivy default tokenizer is weak for Chinese player input. | Added a CJK ngram helper field and bilingual aliases for common TRPG demands such as 检定、黑客、无人机、线缆、线索. |
| 29 | Incremental reindex, improved | Reindex always did a full source pass. | Added `reindex_incremental` with per-source `search_index_watermarks` for SQL/file/JSONL sources. Clean deletion remains open. |

| 30 | Runtime source lifecycle, still open | Tantivy upsert handles new/changed documents, but deleted DB rows or removed files can remain until full rebuild. | Use `trpg search reindex` for clean rebuild in v0.7; future work: deletion markers and generation-based cleanup. |

## v0.7 runtime search notes

| # | 问题 | 当前处理 | 后续 |
|---|---|---|---|
| 31 | 自动检索启发式仍然是关键词和长度规则，不是真正的 LLM demand detector。 | 已接入 runtime，并可通过 `TRPG_RUNTIME_AUTO_SEARCH=false` 关闭。 | v0.8 引入可测试的 demand classifier。 |
| 32 | CJK 检索使用简单 unigram/bigram 与别名扩展。 | 已能覆盖常见中文行动词和中英混合查询，并可通过 `TRPG_SEARCH_CJK_EXPANSION=false` 关闭查询扩展。 | 后续换专业 tokenizer 或 semantic reranker。 |
| 33 | 增量 reindex 无法清理已删除来源。 | 现已使用 per-source `search_index_watermarks`，只索引 changed rows/files；删除仍需 full rebuild 或后续 generation cleanup。 | v0.8 做 `search reset` / clean rebuild / deletion marker。 |
| 34 | scene auto-pin 可能导致 BP2 比预期更频繁变化。 | 默认每轮最多 pin 1 个；scene TTL 的 block id 不包含 turn_id，避免同一 hit 每轮重复生成新 BP2 block。可用 `TRPG_RUNTIME_AUTOPIN_SCENE_RULES=false` 关闭。 | 后续把 pin policy 变成可配置规则。 |

## v0.8 oxidize-pdf ingestion + product document update

| # | Category | Issue | Status |
|---|---|---|---|
| 35 | PDF ingestion, implemented | `pdftotext` produced page text only and required an external binary; source chunks were not first-class artifacts. | Added oxidize-pdf as the preferred backend. `parse-all --pdf-backend oxidize` writes `.oxidize.md` and `data/parsed/source_chunks/*.raw_chunks.jsonl`. |
| 36 | PDF cleanup, partially implemented | oxidize-pdf `rag_chunks()` is fast and metadata-rich, but the sample output shows noisy table-of-contents chunks, duplicated short headings, CJK spacing artifacts, and mixed reading order. | Added deterministic cleanup and opt-in LLM chunk cleaning via `TRPG_INGEST_LLM_CLEAN=true`. Cleaned chunks are written to `.llm_cleaned_chunks.jsonl` and cleaned Markdown. |
| 37 | Product documentation, implemented | Previous “product docs” were closer to technical design notes. | Added `docs/product/chatrpg_llm_gm_product_design_v0_8.md`, a product-level design spec covering purpose, philosophy, users, product architecture, journeys, metrics, safety, and roadmap. |
| 38 | Build verification, environment limitation | The assembly environment still has no `cargo`/`rustc`, and oxidize-pdf API integration targets the current README/docs API. | First local build may require minor API adjustments if oxidize-pdf field names differ. The intended API is `PdfDocument::open(...)?` followed by `rag_chunks()?`. |
| 39 | Chunk cleanup scope, intentionally limited | LLM cleaning every chunk of a 400+ page book would be expensive and may mutate too much source text. | LLM cleaning is opt-in and capped by `TRPG_INGEST_LLM_CLEAN_MAX_CHUNKS`; only suspicious chunks are sent. Future work: `trpg ingest doctor` and manual chunk review. |

## v0.8.2 Playtest Feedback Fixes

### 编译 / 运行阻断：已修

| # | 文件 | 问题 | 修复 |
|---|---|---|---|
| 40 | `crates/trpg-search/src/lib.rs` | Tantivy 0.26 `Index::writer` 需要显式文档类型，`clear_all` / upsert 时无法推断。 | 改为 `writer::<TantivyDocument>(...)`。 |
| 41 | `crates/trpg-search/src/lib.rs` | Tantivy 0.26 `TopDocs` 需要显式 score ordering。 | 改为 `TopDocs::with_limit(n).order_by_score()`。 |
| 42 | `crates/trpg-ingest/src/lib.rs` | oxidize chunk 中 `text` / `full_text` move 后再借用计算 hash。 | 在构造 `DocumentChunk` 前先计算 `text_hash`。 |
| 43 | `crates/trpg-ingest/src/lib.rs` | Rust `regex` 不支持 backreference，`(?m)^(.{1,80})\s+\1$` 会 panic，导致 oxidize ingest 失败并 fallback。 | 改为纯 Rust `collapse_repeated_phrase`。 |
| 44 | `docker-compose.yml` / `.env.example` / CLI 默认值 | 容器名和端口容易与旧版本冲突。 | 改为 `chatrpg-postgres-v08` 与 `54323:5432`。 |

### 玩法 / GM 行为缺陷：已缓解

| # | 区域 | 问题 | 修复 |
|---|---|---|---|
| 45 | Spoiler / visibility | Homecoming 首场会把未发现的隐藏实体名 `Athena` 暴露给玩家。 | 对 GM-only search/runtime blocks 增加 player-facing redaction；默认通过 `TRPG_SECRET_TERM_OVERRIDES=Athena,Shelob` 屏蔽隐藏名，并新增输出契约要求不得猜测 redacted identity。 |
| 46 | LLM transport | `stream_chat` 遇到 429/5xx 无重试；CLI JSONL/SSE 可能没有可见错误。 | `stream_chat` 和 `post_chat` 增加 retry/backoff/Retry-After 支持；CLI 增加 `error` event。 |
| 47 | 检定策略 | 技术/分析动作可能被免费给结论。 | `player_facing_output_contract` 与 engine protocol 增加一致检定策略：有风险、成本、状态改变或战术收益时要求/提供 check，否则明确标注为 free read。 |
| 48 | 叙事节奏 | 回合结尾过度依赖编号菜单。 | 输出契约要求变化收束方式：选项、聚焦问题、纯叙事停顿混用。 |

### 仍未修的功能问题

| # | 区域 | 状态 |
|---|---|---|
| 49 | 自学习闭环 | `lookup_events` 已自动写入，但 `rulings_log` / `learned_packets` 仍主要依赖手动 API。下一步需要 agentic `learning_audit` + verifier + promotion policy。 |
| 50 | 检定 / 战斗系统 | 暂不做固定程序化大引擎。下一步设计为 Agent + Tools + Skills：LLM GM 负责判断意图和叙事，工具负责掷骰、状态 proposal、规则检索、战斗态势建模，harness 负责回归测试。 |
| 51 | Spoiler redaction 泛化 | v0.8.2 先支持 metadata/env secret terms；后续 parser 应抽取 `secret_terms`、`public_aliases`、`reveal_conditions`，并由 revealed-facts ledger 控制解锁。 |

## v0.8.4 Harness and Learning Write-Side

| # | Type | Issue | Fix |
|---|---|---|---|
| 52 | Harness architecture | The first no-spoiler harness was a Python script, which violated the Rust-first project direction. | Replaced with `crates/trpg-harness`, a Rust-native JSONL stream harness. Removed the Python runner. |
| 53 | Rule learning write side | Runtime lookup and learned-packet read path worked, but `lookup_events -> rulings_log -> learned_packets` did not advance automatically. | Added `RuntimeEngine::audit_learning_for_turn`, `learning_audit_runs`, `learning_candidates`, CLI/API candidate approval, and optional gated auto-promotion. |
| 54 | Learning safety | Directly auto-promoting a bad provisional DV/DC would fossilize mistakes. | Default behavior creates `pending_review` candidates only. `learned_packets` require explicit approval or controlled `TRPG_LEARNING_AUTO_PROMOTE=true`. |

## v0.9 Agentic Checks / Dice Flow

| # | Type | Status | Notes |
|---|---|---|---|
| 60 | Product architecture | Implemented | Added Rust-owned GM Agent runtime. LLM is a skill/tool dependency, not the controller. |
| 61 | Policy layering | Implemented | Dice/check visibility advice moved to `data/agent/advice/dice_visibility.v1.json`; Rust performs generic matching and validation. Advice is not injected as a monolithic system prompt. |
| 62 | Player roll interruption | Implemented | `AskPlayerRoll` plans create `CheckContract` + `PendingCheck`, emit `pending_check_created`, and stop SSE/JSONL with `done.reason=awaiting_player_roll`. |
| 63 | Public / secret GM roll | Implemented | `GmRollThenNarrate` emits `dice`; `SecretRollThenNarrate` emits GM/debug `tool` event and injects a private mechanical result for narration without revealing roll details. |
| 64 | Harness coverage | Implemented | Rust harness now supports forbidden events, event substring assertions, required done reason, and no-LLM-stream assertions. Added agentic check cases. |
| 65 | Combat system | Deferred | v0.9 adds `CombatFrame` storage schema but does not implement a full combat turn manager; v1.0 should build on CheckContract + CombatFrame. |
| 66 | LLM verifier skills | Deferred | The agent skill trait is not fully implemented yet; v0.9 focuses on Rust orchestration, dice/check contracts, and harness. |

## v0.9.1 Interaction Gate / Working Frame update

Fixed / implemented:

- Natural-language roll replies now resolve pending checks instead of silently falling through.
- Open pending checks are closed as `abandoned_by_new_action` or `superseded` instead of leaking forever.
- Unparseable roll replies can emit `gate_reprompt` and stop before LLM narration.
- Player new-action replies can emit `gate_abandoned` and continue explicitly.
- Learning candidates now include available CheckContract JSON and can capture concrete static targets such as DV14 from the contract.
- Auto packet keys are normalized to avoid trailing separators such as `combat_ruling___`.
- Added Working State Frame scaffolding for combat, side quests, investigations, chases, hazards, and netruns.
- Rust harness now supports multi-turn cases.

Still open:

- CombatAgent is not complete yet.
- Ruleset-specific reaction options still need extraction into advice/rules packets.
- Frame compaction currently has schema and storage scaffolding; full compactor logic is next.
- StatePatch validation is still lightweight.

## v1.1 Semantic Situation Orchestrator update

Fixed design-level issue from v1.0 debug report: active combat frames could trap the player forever because frame close/start/attack/reaction detection still relied on keyword substrings. v1.1 removes the old keyword-only main path in `trpg-combat` and introduces typed `ConflictIntent`, `ExitContract`, `StalemateContract`, `FrameProgressTracker`, and `NpcDriveState`.

Remaining: the current semantic router is Rust-native paraphrase similarity. Future work may replace the internal classifier with an LLM JSON skill while keeping the same Rust-owned contracts and harness assertions.

## v1.2 Actionable Situation Director update

### Implemented

- Added `ActionableSituationBrief`, `PlayerFacingClueBoard`, `ConsequenceContract`, `ClockTick`, `SpotlightState`, and scene-purpose models.
- Added `trpg-director` crate. It builds a director brief from compiled BP context, current input, and optional conflict result.
- Runtime now persists director artifacts and injects them as dynamic BP3 context before narration.
- CLI/API now emit `director`, `actionable_situation_brief`, `guidance_ladder`, `clue_board_updated`, `clock_tick`, and `consequence_contract_created` events where applicable.
- Added v1.2 harness cases for scene anchors, stuck-player guidance, NPC biased advice, and clue board output.

### Still open

- Director heuristics are currently deterministic and lightweight. A future v1.3 can add an LLM JSON DirectorSkill with the same Rust-owned output contract.
- ClueBoard currently derives from visible/runtime context only; it should later merge durable discovered clues from module graph state.
- SpotlightState scaffolding exists, but full multi-player spotlight balancing requires player roster data.
- ConsequenceContract is created for check/stuck contexts, but state-patch validation for each cost option remains future work.

## v1.3 Situation Novelty Director

### Addressed

- Enemy-initiated / scene-framed combat openers now start a frame and can open a required reaction gate.
- Direction gates now accept natural continue/press-attack replies instead of reprompt-looping.
- Director briefs are repaired to include visible facts, pressure, affordances, risks, and fresh change.
- Repeated player actions now trigger `NoveltyDecision` / `FreshChange` / NPC tactic shifts.
- Added anti-repeat state: `BeatSignature`, `NoveltyState`, `TacticPalette`, `TacticCooldown`.

### Still incomplete

- Tactic palettes are still generic defaults; future versions should extract creature/NPC-specific tactics from module and stat blocks.
- NoRepeatValidator currently tracks structured beats, not full natural-language style similarity.
- Novelty tables are created for audit, but most immediate runtime evidence is still carried by `frame_events`.

## v1.4 World Time Spine

Implemented:

- Authoritative `WorldTimeState`, `WorldEvent`, `ScheduledEvent`, `TimeAdvanceRequest`, and `ContextWatermark` models.
- PostgreSQL tables for world time state, world events, scheduled events, time advances, anchors, and context watermarks.
- `trpg-time` service crate for current time, event recording, time advance, and scheduled event triggering.
- Runtime emits `phase:world_time` and records turn events with world tick anchors.
- ContextBuilder can load world events since the previous watermark into BP3.
- CLI/API time show/advance/events/schedule endpoints.

Still incomplete:

- Ruleset-specific rest/recovery/travel procedures are not yet mapped onto time advance.
- Automatic time advancement remains opt-in; most time changes should still be explicit tool/API/CLI calls.
- Retcon and flashback semantics are modeled but not yet fully exposed as UI workflows.

## v1.5 Interaction Lifecycle Kernel

### Addressed

- Added `InteractionContext`, `InteractionEvent`, `InvariantRepair`, and `InteractionTransitionResult` as the typed lifecycle surface.
- Added migration `0011_interaction_lifecycle_kernel_v15.sql` with interaction contexts, events, invariant repairs, generation, ownership, and closed-at tick fields.
- Added `trpg-interaction` / `InteractionLifecycleKernel` as the lifecycle owner for frame/gate/check transitions.
- `handle_open_interaction_gate` now reconciles before serving a gate and allows terminal intent to supersede stale/irrelevant gates.
- Frame-close paths now call cascade close so child gates/pending checks are terminally superseded.
- Open gate/pending queries are generation-guarded.
- Added harness cases for enter→exit→re-enter, ceasefire superseding reaction gate, and abandoned pending check not being resolved by later stray roll.

### Still incomplete

- Property-based random action sequence tests are documented but not fully implemented as Rust proptests yet.
- Some lifecycle repairs are DB-level self-healing rather than type-level unrepresentable states; deeper refactoring can move more transitions into reducer commands.
- Older migrations/data may need one reconciliation turn after upgrade to backfill owner_frame_id for legacy open gates.

## v1.5 Interaction Lifecycle Kernel

### Addressed

- Added `InteractionLifecycleKernel` as a single owner for frame/gate/check lifecycle.
- Added generation guards so stale gates/checks from older scene/frame generations cannot capture new player input.
- Added cascade close for frame completion/exit compaction: child gates and pending checks are superseded with explicit reasons.
- Added self-healing `reconcile_session` at the start of gate handling.
- Added `interaction_contexts`, `interaction_events`, and `invariant_repairs` tables.

### Still incomplete

- Full property/proptest harness is scaffolded by design docs and case coverage, but Rust property tests are not yet implemented.
- Reaction-window rows are still represented mainly as `InteractionGate`; a dedicated reaction_window table can be added later if needed.
- Some legacy rows may lack owner_frame_id until reconcile backfills or supersedes them.

## v1.6 Object & Possession Kernel

### Added

- Runtime object definitions, instances, locations, edges, interactions, patches, and events.
- Disarm/grab/pickup/drop/equip/cut/unlock/search style inputs now create `ObjectInteractionContract` instead of being narration-only.
- Object graph is projected into BP3 dynamic context.
- Check results can apply validated object patches for matching object interaction contracts.

### Still incomplete

- Opposed object contests still use provisional CheckContract targets until a dedicated Contest/Opposition Kernel is added.
- Detailed ruleset object extraction remains on-demand; v1.6 seeds generic object instances when exact definitions are not loaded.
- Ammo, encumbrance, crafting, vehicle sub-parts, magic item attunement, cyberware installation, and full economy are not implemented.

## v1.7 Turn Orchestration Kernel

### Addressed

- Added a single turn reducer so open gates, object interactions, combat frame start/continue, director guidance, and generic checks do not compete for the same input.
- Gate-first behavior is now limited to direct gate replies, such as roll replies or recognized choice replies.
- Frame-worthy tactical input is routed to the conflict/combat frame path before the generic GM agent can create standalone pending checks.
- Object interactions inside frames are treated as child frame actions.

### Still incomplete

- CombatRoundReducer remains lightweight; exact ruleset action economy and opposed contests are still provisional.
- Object interactions can create checks, but v1.7 does not yet implement a full Contest/Opposition Kernel.
- The turn classifier is still heuristic; an LLM JSON classifier can be added behind the same `TurnIntent` contract later.

## v1.8 Runtime Material Binding changes

Fixed / addressed from the v1.7 debug report:

- `trpg-orchestrator` compile snag: `reconcile_session(...).await.unwrap_or_default()` now uses `.repairs`.
- `scope_matches` now covers `ScopeType::Object`.
- Required reaction gates are no longer superseded by unrelated object/attack intent; terminal/exit intent may still supersede them.
- Object kernel no longer claims hypothetical/assessment inputs such as “判断能不能安全切断”, nor GM-secret requests.
- Object kernel no longer runs as a pre-orchestrator self-selector; CLI/API only call it when the route requests object handling.
- Combat frame start has a lexical safety fallback for clear declarative attacks / enemy attacks when the paraphrase score is too low.
- Runtime actors now get parameters via `runtime_actor_parameters` once a PC/opposition appears.
- Session-scoped object IDs prevent cross-session object ownership collisions.
- Held weapon seeding now binds narration to a ruleset-aware ObjectDefinition and mechanical profile instead of a statless generic placeholder.

Still open:

- Exact weapon/armor/statblock extraction from Tantivy/source chunks is still heuristic; v1.8 seeds common profiles and stores `source_query` for later exact retrieval.
- Full opposed contest resolution is still future work.
- Full damage/armor/ammo engines are not implemented yet.

## v1.9 Semantic Rule Binding & Ability Hydration

### Addressed

- Added semantic classification events and rule-binding packets so retrieved rules can be written back to runtime state instead of remaining prompt-only.
- Added ability definitions, instances, trigger bindings, and activation contracts.
- Added BP3 ability graph and rule-binding context blocks.
- Added an audited semantic-first path; lexical fallback is disabled by default and marked as fallback when enabled.

### Still incomplete

- Exact field extraction depends on the configured LLM semantic extractor; fallback extraction records candidates but does not infer exact mechanics.
- Complete D&D spell, Cyberpunk Role Ability/Netrunning, Sword World magic/feat, CoC/BRP magic, and Triangle playwalled ability engines are not implemented yet.
- Full contest/opposition kernel and damage/HP patch validator remain future work.

## v1.9.1 addressed

- Fixed three v1.9 build snags: `trpg-params` now declares `sqlx`, `trpg-combat` now declares `trpg-time`, and `trpg-semantics` no longer references non-existent `FrameRelation` variants.
- Semantic classifier results no longer erase deterministic Rust route invariants for disarm/grab, technical risk assessment, combat frame actions, and exit/de-escalation.
- Ability, object, and conflict kernels are gated by `TurnOrchestrator` route flags instead of running as independent turn claimants.
- Clear disarm/grab intent now remains reachable by the Object & Possession Kernel even when the LLM classifies the phrase as combat or generic agent intent.
- Technical assessment of cables/devices routes to agentic check creation instead of free narration.

## Still open after v1.9.1

- Semantic route cache / record-replay is documented and configured, but not fully implemented as a persistent replay system.
- Actor and object hydration can still use seeded profiles; exact rule/stat extraction from source documents remains the next major step.
- Full contest/opposition resolution is still not implemented.

## v1.10 Real Materialization Extractor

### Addressed

- Retrieved rules/modules now have a writeback pipeline: `materialization_demands`, `source_evidence_bundles`, `extraction_runs`, `binding_verifications`, and runtime binding tables.
- Actor/object/ability/check/effect materialization can be triggered before mechanical resolution and projected into BP3 context.
- Runtime actor profiles can be updated by materialization rather than staying at seeded fallback values.

### Still incomplete

- Typed extractors are schema-driven and LLM-backed, but not yet specialized full parsers for every ruleset table.
- Contest/opposition and damage/armor/condition validators are still future work.
- Semantic record/replay remains incomplete; route stability is handled by deterministic invariants, but extractor outputs can still vary unless replay is added.

## v1.10.1 simulation-guided hotfix

Addressed:

- Fixed `trpg-material` double `Option<String>` build snag in `narration_context` assignment.
- Forced Homecoming-style technical risk assessment into `agent_plan -> check_contract_created -> pending_check_created` instead of free narration.
- Enemy-initiated combat start now immediately opens a required reaction gate when the semantic intent is `under_attack`, `enemy_initiated_conflict`, or `scene_enters_conflict`.
- Required reaction reprompts emit `reaction_window_opened` and use `done.reason=awaiting_required_reaction` for clients/harness.
- Turn classification now prioritizes direct incoming attacks over ordinary object interactions, preventing “I’m under fire but I want to cut the cable” from bypassing the reaction window.

Still open:

- Real materialization extractor remains limited by candidate retrieval and source quality; NPC stats can still fall back to provisional profiles if exact source evidence is absent.
- Contest/opposition and damage/resource patch validation remain future work.
- Golden trace simulation is documented in `harness/scenarios/`, but the full `trpg-harness simulate` DB assertion runner has not been implemented yet.

## v1.10.2 Player-supplied value referee

Added a rules-first verification layer for player-supplied numerical parameters. The feature records claims and verifications for damage totals, weapon damage expressions, target numbers, HP, armor, and similar values. It does not yet replace the full Damage/Condition/Resource Patch Validator; it prevents silent adoption of unsupported player numbers and creates a table override audit trail.

Still incomplete:

- Exact weapon/armor/spell table lookup is still dependent on later mechanical source packs and materialization extractors.
- Combat damage application still belongs to the planned Referee Combat Slice / Damage Patch Validator.
- Current verifier has deterministic sanity bands and audit logging; it is not a complete rules-specific calculator.

## v1.10.2 Referee Combat Slice correction

The earlier v1.10.2 package only implemented the player-supplied-value referee. That was a useful sub-policy but not the intended v1.10.2 scope. The corrected v1.10.2 adds a minimal referee combat slice:

- `trpg-mechanics` crate.
- `actor_mechanical_states` ledger.
- `attack_resolution_contracts`.
- `damage_packets`.
- follow-up damage roll gate after a successful attack check.
- HP deltas persisted into actor mechanical state.
- BP3 mechanical ledger context block.

Still incomplete: full contest/opposition, armor/SP/AC, Cyberpunk RED exact range DV, and system-specific damage/condition validators.

## v1.11 Contest / Opposition Kernel

### Addressed

- Check resolution now creates `ContestProfile` and `ContestResolutionRecord` records for resolved checks.
- Attack checks are upgraded from final `NoMechanicalOpposition` to attack-vs-defense/static-DV models when exact rules are not yet bound.
- Technical assessment checks can resolve against a static model rather than remaining prompt-only.
- BP3 now includes recent contest/opposition models through a `ContestGraph` context block.

### Still incomplete

- Ruleset-exact combat math is still provisional until Mechanical Source Packs and full ruleset profiles are extracted.
- Damage, armor, SP/AC, SAN/Harm/Chaos, and conditions still require the later Damage / Condition / Resource Patch Validator.
- Opposed rolls currently support contract modeling, but automatic defender rolling and multi-actor save resolution are not complete.

## v1.12 Mechanics Search Skills notes

Implemented:
- Multi-step query planning for mechanical demands.
- Demand-oriented search skills over generic GrepSearch/Tantivy candidate retrieval.
- Ruleset locator/alias packs for Cyberpunk RED, D&D 5e, Sword World 2.5, BRP/CoC, and Triangle Agency.
- Parameter facet writeback records for actor/object/ability/check/effect bindings.

Still incomplete:
- Search skill profiles are currently generated in code and persisted as query plans, not edited via UI.
- Semantic rerank/extraction still depends on the existing materialization extractor.
- Facet execution remains shallow; v1.13 should make actor/object/ability/check/effect facets directly consumable by contest, damage, resource, and condition validators.

## v1.12.1 Notes

- `DamagePacket` is now a compatibility specialization for HP impacts. New mechanics should prefer `EffectResolutionPacket` + `ParameterImpact`.
- The effect target parameter is still partly inferred when exact object/ability effect facets are missing. This is recorded with `provisional_reason` and should be replaced by parameter facet executor/source-pack bindings in later versions.
- Full armor/SP/AC/SAN/Chaos/Harm validation is not complete. v1.12.1 ensures the result is represented and persisted; later validators should improve ruleset-specific correctness.
- The execution environment used for assembly did not include `cargo`, so run `cargo check` locally first.

## v1.12.2 Roll Binding & Mechanical Gate Priority Hotfix

Implemented as a narrow hotfix after v1.12.1 playtest evidence showed orphan `/roll` rows and damage/effect inputs being swallowed by direction gates.

Fixed/changed:
- `/roll` can bind to the latest unresolved `CheckContract` even if the open `InteractionGate` has been lost or superseded.
- Direction gates no longer preempt clear in-frame mechanical actions in the combat reducer.
- Cyberpunk provisional attack expression defaults to `1d10+10` through `TRPG_COMBAT_DEFAULT_ATTACK_EXPR`.
- Effect follow-up checks respect `TRPG_AGENT_TABLE_DICE_POLICY`; system-visible effects auto-resolve from CLI/API.
- Added migration indexes for unresolved check lookup.

Still open:
- Full armor/SP/ablation, save/AC, SW2.5 power-table, CoC SAN thresholds, and Triangle Chaos/Harm remain parameter-facet executor work.
- There is still no real Rust compile validation in the assembly environment.

## v1.13 Parameter Facet Executor

Added a parameter-facet executor that consumes existing actor/object/ability/check/effect facets instead of creating per-ruleset engines. New state tables: `parameter_facet_execution_runs`, `generic_parameter_states`, and `ruleset_mechanical_profiles`.

Known remaining gaps:
- Exact Cyberpunk RED range DV and SP/ablation still require richer facet packs.
- D&D AC/save/spell DC execution is represented by facets but not fully sourced from PHB/MM yet.
- Sword World power-table lookup is represented as a facet concept but not fully table-executed.
- CoC/BRP SAN/Major Wound thresholds and Triangle Harm/Chaos/playwalled effects need dedicated facet packs and verification.
- Executable simulation harness still needs DB-level assertions for multi-ruleset traces.

## v1.13 Parameter Facet Executor Notes

Implemented:
- Parameter facet execution runs are recorded in `parameter_facet_execution_runs`.
- Generic non-actor state is persisted in `generic_parameter_states`.
- Starter ruleset mechanical profiles are seeded for Cyberpunk RED, D&D 5e, Sword World 2.5, BRP/CoC, and Triangle Agency.
- Mechanical Ledger BP3 now includes recent facet executions and generic parameter states.

Still incomplete:
- Exact source-pack extraction for all rule tables is not complete.
- Cyberpunk RED exact range DV/SP ablation, D&D spell details, Sword World power-table lookup, CoC Major Wound/SAN thresholds, and Triangle playwalled executors still need richer facets and verification data.
- The assembly environment still lacks `cargo`; run `cargo check` locally first.

## v1.13.1 Combat Route Source Object Hotfix

Fixed: attacks naming a weapon could be routed to `object_interaction_first`, leaving combat runtime tables empty. Named weapons are now treated as attack source objects unless the turn is a true object interaction such as disarm, grab, pickup, or cut.

Remaining: exact weapon tables, armor/SP/AC, and advanced condition/resource execution still depend on richer parameter facets and source extraction.

## v1.13.2 Roll/Effect Gate Closure Hotfix

The v1.13.1 debug report confirmed that named-weapon attacks now route to combat, but found that damage could still fail to persist because effect rolls stayed player-required, fallback matching could self-match `Chaos`, and auto-resolved checks left stale gates open. v1.13.2 patches those execution-chain issues. Exact system-specific rule packs remain future work.

## v1.13.3 — Semantic Combat Loop & Roll Authority

- Direction/stalemate gates are now advisory for clear in-frame actions; they should be superseded by sustained combat actions instead of reprompting.
- Combat intent is semantic-first via `SemanticIntentService.route_cues`; hardcoded phrases remain as audited fallback only.
- `CombatAgent` receives a structured semantic hint from `TurnOrchestrator`, reducing dependence on local phrase matching.
- CLI/API check execution now enforces `system_rolls_visible` at the execution boundary to reduce intermittent player-roll prompts.

Remaining work:
- Full semantic record/replay for route stability is still not implemented.
- Exact ruleset facets still depend on extractor quality and source packs.

## v1.14 Notes

- `trpg-harness playtest` is a black-box evaluator harness, not a production LLM backend router.
- The first evaluator adapter uses `claude -p`; this is best for per-turn evaluation snapshots and debug reports, not for low-latency production GM streaming.
- DB assertions are best-effort and require `DATABASE_URL` in the environment or `.env`.
- The default Homecoming playtest scenario is intentionally stricter than phase-only harness cases and checks forbidden player-visible text plus DB deltas.

## v1.15 Product Evaluation Skill

Added a test-only product evaluation skill for Claude Code/subagent workflows. It is designed to prevent evaluator agents from stopping at feature tests and instead requires product journey, GM/referee quality, rules compliance, module progression, NPC simulation, UX/fun, weird-player robustness, safety/visibility, and observability scoring.

Limitations:
- The skill is guidance and prompt/rubric infrastructure; it does not automatically prove product quality without running playtests.
- Scores depend on the evaluator following the skill and attaching evidence.
- Cross-ruleset deep tests still require prepared scenario data and parsed materials.


## v1.15.1 — Human-like player simulation and debug directives

Product evaluation now distinguishes player behavior from test setup. Simulated players should use short, natural TRPG player actions and avoid JSON, code, SQL, DB table names, Rust type names, phase/event names, and bundled QA assertions.

For rare-state setup, playtest scenarios may include test-only debug blocks:

```text
[debug]add {"id":"npc.scav_boss","kind":"npc","hp":35,"weapon":"shotgun"}[/debug]
我朝拿霰弹枪的头目开火，然后立刻缩回掩体。
```

The harness strips `[debug]...[/debug]` blocks before sending input to the GM, records them as artifacts, and injects the resulting state as GM-only test context. This allows create/delete/modify of test actors, objects, clues, resources, clocks, and scene facts without polluting player-facing speech.

The evaluator now rejects dice commands and roll/damage result reports in default product scenarios. Manual-roll compatibility may still be tested explicitly, but normal product play must be action-only: player declares fiction, system plans/rolls/resolves/persists.


## v1.15.2 — Action-only player protocol

Updated product-evaluation guidance and playtest scenarios so the simulated player never reports dice results in default product play. The harness human-player lint now flags `/roll`, dice expressions, and roll/damage total language unless a scenario/turn explicitly enables `allow_manual_roll_input`. This aligns tests with the intended product loop: the GM/system owns mechanics and asks only for the next fictional player action.

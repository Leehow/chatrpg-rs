# Integration Report — EPIC-KNOWLEDGE-MEMORY-NPC-RUNTIME

## Purpose

This report records rolling integration events for accepted scoped worker
commits. It is lead-owned. Workers may reference it, but they do not edit it
unless explicitly assigned a documentation-only lane.

## Current Baseline

- Integration branch: `codex/knowledge-runtime-p0`
- Integration head at WorkGraph creation:
  `eb77199b74ff7ed5963099ecb5c02f4c555166b3`
- Current state: existing dirty integration debt is present in this worktree.
- Required next action: run `TC-PIPE-00-integration-debt-classification` before
  dispatching new code-affecting implementation lanes.

## Recovery Policy For Existing Dirty State

The lead must classify dirty files by worker handoff/task before accepting or
discarding any slice:

1. Read the relevant `.tmp/team-lead/worker-*.md` handoff.
2. Map changed files to task card, acceptance IDs, and validation evidence.
3. Inspect the diff for scope drift and hotspot edits.
4. Mark the slice `accepted`, `needs_revision`, `rejected`, `obsolete`, or
   `blocked_human`.
5. Trial-integrate accepted scoped commits one at a time from the current
   integration head.
6. Record each integration event below.

If a dirty slice cannot be mapped to a handoff, task card, and validation
evidence, it is not accepted. Dispatch a fresh replacement or recovery worker
instead of silently rolling it forward.

## Integration Events

| Event | Task | Worker Commit | Previous Head | Trial Branch | Result | Validation | New Head | Follow-Up |
|---|---|---|---|---|---|---|---|---|
| 2026-06-18-recovery-classification | TC-PIPE-00-integration-debt-classification | - | eb77199b74ff7ed5963099ecb5c02f4c555166b3 | - | classified, not accepted | `git status --porcelain --untracked-files=all`; `git diff --stat`; handoff inventory | eb77199b74ff7ed5963099ecb5c02f4c555166b3 | dispatch `ARCH-UNLOCK-01-model-facade-split` from clean integration head; review dirty slices separately before product acceptance |
| 2026-06-18-tc-vs-know-01-review | TC-VS-KNOW-01 | no_commit worker output | eb77199b74ff7ed5963099ecb5c02f4c555166b3 | in-place dirty-slice review | accepted as journey vertical slice; full live cassette promotion pending provider key | `cargo test -p trpg-harness`; `cargo check -p trpg-model -p trpg-db -p trpg-runtime -p trpg-gm -p trpg-harness -p trpg-cli`; focused no-spoiler/model/runtime tests; DB live `live_knowledge_edges` + `live_reveal_fact_tool`; deterministic/replay/failure-branch playtests; live-with-DB/no-key BLOCKED with no accepted cassette; `git diff --check` | eb77199b74ff7ed5963099ecb5c02f4c555166b3 | continue to NPC/social vertical planning; run Milestone 5 live recording once `TRPG_LLM_API_KEY` is available |
| 2026-06-18-arch-unlock-01-review | ARCH-UNLOCK-01-model-facade-split | de97b05f31e746e0a1206e3198951e75cc84a960 | eb77199b74ff7ed5963099ecb5c02f4c555166b3 | worker branch `claude/arch-unlock/model-facade` | accepted, pending clean integration | worker: `cargo check -p trpg-model`; `cargo test -p trpg-model`; lead: `rustfmt --check crates/trpg-model/src/{hash,source,turn_lifecycle}.rs`; `CARGO_TARGET_DIR=.target/team-lead/lead-review-arch-unlock-01 cargo check -p trpg-model`; `CARGO_TARGET_DIR=.target/team-lead/lead-review-arch-unlock-01 cargo test -p trpg-model`; `git show --check HEAD`; `git diff --check HEAD^ HEAD` | eb77199b74ff7ed5963099ecb5c02f4c555166b3 | integrate/cherry-pick `de97b05f31e746e0a1206e3198951e75cc84a960` only after the dirty integration worktree is resolved or a clean integration lane is opened |
| 2026-06-18-arch-unlock-02-review | ARCH-UNLOCK-02-cache-visibility-split | d32ccac488dafc84d4bf233ffd8f969613c3643a | de97b05f31e746e0a1206e3198951e75cc84a960 | worker branch `claude/arch-unlock/visibility-primitives` | accepted, pending clean integration | worker: `rustfmt --check crates/trpg-model/src/visibility.rs`; `cargo check -p trpg-model`; `cargo test -p trpg-model`; `git diff --check`; lead: `rustfmt --check crates/trpg-model/src/visibility.rs`; `CARGO_TARGET_DIR=.target/team-lead/lead-review-arch-unlock-02 cargo check -p trpg-model`; `CARGO_TARGET_DIR=.target/team-lead/lead-review-arch-unlock-02 cargo test -p trpg-model`; `CARGO_TARGET_DIR=.target/team-lead/lead-review-arch-unlock-02-downstream cargo check -p trpg-db -p trpg-runtime -p trpg-gm -p trpg-api -p trpg-cli -p trpg-parser -p trpg-material`; `git show --check HEAD`; `git diff --check HEAD^ HEAD` | eb77199b74ff7ed5963099ecb5c02f4c555166b3 | integrate/cherry-pick after ARCH-UNLOCK-01 in a clean integration lane; do not merge into the dirty `codex/knowledge-runtime-p0` worktree yet |
| 2026-06-19-tc-vs-npc-01-review | TC-VS-NPC-01 | no_commit worker output | eb77199b74ff7ed5963099ecb5c02f4c555166b3 | in-place dirty-slice review | accepted as NPC/social journey vertical slice after revision1; full live cassette promotion pending provider key | worker handoffs `.tmp/team-lead/worker-tc-vs-npc-01-homecoming-npc-social-memory-20260618-234646.md` and `.tmp/team-lead/worker-tc-vs-npc-01-homecoming-npc-social-memory-revision1-20260619-0001.md`; lead: `cargo test -p trpg-harness`; workspace `cargo check`; focused model/runtime/gm/DB NPC tests; HOME-02 deterministic/replay CLI PASS; leakbranch `WITHHELD_SECRET_LEAKED`; replay provenance mismatch fail-closed; live-with-DB/no-key BLOCKED with no accepted cassette; `git diff --check` | eb77199b74ff7ed5963099ecb5c02f4c555166b3 | dispatch TC-VS-MEM-01; promote seeded HOME-02 replay cassette with `TRPG_LLM_API_KEY` when available |
| 2026-06-19-tc-vs-mem-01-review | TC-VS-MEM-01 | no_commit worker output | eb77199b74ff7ed5963099ecb5c02f4c555166b3 | in-place dirty-slice review | accepted as committed-memory/reload journey vertical slice; DA-PIPE-03 remains partial for production flight-recorder scope | worker handoff `.tmp/team-lead/worker-tc-vs-mem-01-committed-memory-reload-20260619-0018.md`; lead: `cargo test -p trpg-harness`; workspace `cargo check`; focused model/runtime/DB memory tests plus relationship extraction; HOME-03 deterministic/replay CLI PASS; failbranch `EXTRACTION_NOT_FROM_COMMITTED` + `EVIDENCE_MISSING`; replay provenance mismatch fail-closed; live-with-DB/no-key BLOCKED with no accepted cassette; `git diff --check` | eb77199b74ff7ed5963099ecb5c02f4c555166b3 | dispatch TC-PIPE-03-production-flight-recorder-provenance; promote seeded HOME-03 replay cassette with `TRPG_LLM_API_KEY` when available |
| 2026-06-19-tc-pipe-03-review | TC-PIPE-03 | no_commit recovery worker output | eb77199b74ff7ed5963099ecb5c02f4c555166b3 | in-place dirty-slice review | accepted as production flight-recorder/verifier-provenance slice; DA-SPOIL-03, DA-PIPE-01, and DA-PIPE-03 marked Done | worker handoff `.tmp/team-lead/worker-tc-pipe-03-production-flight-recorder-provenance-recovery1-20260619-0108.md`; lead used isolated `CARGO_TARGET_DIR=.tmp/cargo-target-tcpipe03-lead` because an unrelated worker shared the default target dir; lead: GM `flight_recorder_provenance` 4/4; `no_spoiler_production_sources` 4/4; `cargo test -p trpg-harness` 75 + 38 + 5; workspace `cargo check`; GM lib 208/208; HOME-04 deterministic/replay CLI PASS; failbranch `VIEW_ORDER_VIOLATION` + `SECRET_IN_TRACE`; replay provenance mismatch fail-closed; live-with-DB/no-key BLOCKED with no accepted cassette; `git diff --check` | eb77199b74ff7ed5963099ecb5c02f4c555166b3 | dispatch TC-KNOW-FOUND-01-worldfact-holder-roundtrip; promote seeded HOME-04 replay cassette with `TRPG_LLM_API_KEY` when available |
| 2026-06-19-tc-know-found-01-review | TC-KNOW-FOUND-01 | no_commit worker output | eb77199b74ff7ed5963099ecb5c02f4c555166b3 | in-place dirty-slice review | accepted as WorldFact/KnowledgeEdge holder-roundtrip foundation slice; DA-KNOW-01 and DA-KNOW-03 marked Done | worker handoff `.tmp/team-lead/worker-tc-know-found-01-worldfact-holder-roundtrip-20260619-0130.md`; lead used isolated `CARGO_TARGET_DIR=.tmp/cargo-target-tcknowfound01-lead`; lead: model mind-view 6/6; model knowledge 18/18 plus focused bins; runtime knowledge_projection 6/6; runtime npc_mind 4/4; DB live `live_knowledge_edges` 11/11; DB live `live_npc_mind_view` 4/4; `cargo check -p trpg-model -p trpg-db -p trpg-runtime`; `git diff --check` | eb77199b74ff7ed5963099ecb5c02f4c555166b3 | run terminal acceptance audit; provider-backed live cassette promotion remains Milestone 5 when `TRPG_LLM_API_KEY` is available |

## Dirty-State Classification Snapshot

This classification prevents invisible integration debt from blocking all new
architecture-unlock work, but it does not accept any implementation slice.

| Bucket | Representative files | Handoff/task evidence | Classification | Next action |
|---|---|---|---|---|
| Protocol / Team Lead pipeline | `AGENTS.md`, `CLAUDE.md`, `.agents/**`, `docs/codex-team-lead/**`, `docs/epics/**`, `docs/team-lead-pipeline/**` | current lead work plus prior autonomous-loop handoffs | reviewable protocol slice | keep lead-owned; do not let implementation workers edit shared protocol docs |
| Journey harness and cassette evidence | `crates/trpg-harness/**`, `.tmp/tcjrny02*`, `.tmp/tcvsknow01*`, `harness/cases/**`, epic scenarios/fixtures | `worker-tc-jrny-*`, `worker-tc-p2-03-*`, `worker-tc-vs-know-*` | needs focused journey/harness review | do not mark gameplay rows Done without connected Journey evidence review |
| Knowledge / NoSpoiler / projection | `crates/trpg-model/src/knowledge.rs`, `crates/trpg-db/src/lib.rs`, `crates/trpg-runtime/src/knowledge_projection.rs`, `crates/trpg-gm/src/plugin/builtin_no_spoiler.rs`, `migrations/0033_*.sql`, `migrations/0034_*.sql` | `worker-tc-know-*`, `worker-tc-d3-00-*`, `worker-tc-d3-05-*` | needs rolling integration review | split future work by model/db/runtime/gm write sets |
| NPC profile / relationship / mind / behavior | `crates/trpg-model/src/npc_*.rs`, `crates/trpg-runtime/src/npc_*.rs`, `crates/trpg-db/tests/live_npc_*.rs`, `migrations/0035_*.sql`, `migrations/0036_*.sql` | `worker-tc-npc-*`, `worker-tc-d3-01-*`, `worker-tc-d3-02-*` | needs dependency review after actor identity/knowledge edges | do not merge before holder identity and migration-order review |
| Memory / relationship extraction | `crates/trpg-model/src/memory_proposal.rs`, `crates/trpg-runtime/src/memory_proposal.rs`, `crates/trpg-runtime/src/relationship_extraction.rs`, memory tests | `worker-tc-p2-01-*`, `worker-tc-d3-03-*`, `worker-tc-d3-04-*` | needs event/projection idempotency review | route through later memory vertical slice |
| API / CLI / entry gate | `crates/trpg-api/src/lib.rs`, `crates/trpg-cli/src/*.rs`, `crates/trpg-runtime/src/entry_gate.rs` | `worker-gm-character-card-entry-gate-*`, journey prelude handoffs | needs focused product-path review | keep separate from model facade architecture unlock |

# Documentation — EPIC-KNOWLEDGE-MEMORY-NPC-RUNTIME

## Running Status

Initialize this file when the epic starts.

## Decision Log

| Date | Decision | Reason | Evidence |
|---|---|---|---|
| 2026-06-18 | Enable parallel-pipeline scheduling for this epic: AcceptanceLedger remains the Done authority; WorkGraph becomes the scheduling/lease/integration-state authority; IntegrationReport records rolling integration events. | The worktree already has substantial dirty integration output, and single "next task" dispatch does not prevent write-set collisions or batch-merge debt. | `WorkGraph.yaml`, `IntegrationReport.md`, `Implement.md`, `AGENTS.md`, `.agents/skills/team-lead-parallel-pipeline/SKILL.md`. |

## Worker Evidence Log

| Task | Handoff | Accepted? | Validation | Ledger rows updated |
|---|---|---|---|---|
| TC-PIPE-00-integration-debt-classification | lead-owned classification in `IntegrationReport.md` | classified only; no implementation accepted | `git status --porcelain --untracked-files=all`; `git diff --stat`; handoff inventory | none |
| ARCH-UNLOCK-01-model-facade-split | `/Users/haoli/.config/superpowers/worktrees/chatrpg-rs-v1.20-formula/arch-unlock-model-facade/.tmp/team-lead/worker-arch-unlock-01-model-facade-split-20260618.md` | accepted, pending clean integration | worker: `cargo check -p trpg-model`; `cargo test -p trpg-model`; lead: `rustfmt --check crates/trpg-model/src/{hash,source,turn_lifecycle}.rs`; `CARGO_TARGET_DIR=.target/team-lead/lead-review-arch-unlock-01 cargo check -p trpg-model`; `CARGO_TARGET_DIR=.target/team-lead/lead-review-arch-unlock-01 cargo test -p trpg-model`; `git show --check HEAD`; commit `de97b05f31e746e0a1206e3198951e75cc84a960` contains only the declared four write_set files | none |
| ARCH-UNLOCK-02-cache-visibility-split | `/Users/haoli/.config/superpowers/worktrees/chatrpg-rs-v1.20-formula/arch-unlock-visibility-primitives/.tmp/team-lead/worker-arch-unlock-02-cache-visibility-split-20260618.md` | accepted, pending clean integration | worker: `rustfmt --check crates/trpg-model/src/visibility.rs`; `cargo check -p trpg-model`; `cargo test -p trpg-model`; lead: `rustfmt --check crates/trpg-model/src/visibility.rs`; `CARGO_TARGET_DIR=.target/team-lead/lead-review-arch-unlock-02 cargo check -p trpg-model`; `CARGO_TARGET_DIR=.target/team-lead/lead-review-arch-unlock-02 cargo test -p trpg-model`; downstream `CARGO_TARGET_DIR=.target/team-lead/lead-review-arch-unlock-02-downstream cargo check -p trpg-db -p trpg-runtime -p trpg-gm -p trpg-api -p trpg-cli -p trpg-parser -p trpg-material`; `git show --check HEAD`; commit `d32ccac488dafc84d4bf233ffd8f969613c3643a` contains only the declared two write_set files | none |
| TC-VS-KNOW-01 | `.tmp/team-lead/worker-tc-vs-know-01-homecoming-knowledge-nospoiler-20260618-230016.md` | accepted | `cargo test -p trpg-harness`; `cargo check -p trpg-model -p trpg-db -p trpg-runtime -p trpg-gm -p trpg-harness -p trpg-cli`; `cargo test -p trpg-gm --test no_spoiler_production_sources`; `cargo test -p trpg-model knowledge_leak`; `cargo test -p trpg-runtime --lib knowledge`; DB live `live_knowledge_edges` + `live_reveal_fact_tool`; deterministic/replay/failure-branch playtests; live-with-DB/no-key fail-closed with no accepted cassette | TC-VS-KNOW-01 Done; DA-KNOW-02/04/05 Done; DA-SPOIL-01/04 Done; DA-SPOIL-02 Partial; DA-PIPE-01/02 Partial |
| TC-VS-NPC-01 | `.tmp/team-lead/worker-tc-vs-npc-01-homecoming-npc-social-memory-20260618-234646.md`; `.tmp/team-lead/worker-tc-vs-npc-01-homecoming-npc-social-memory-revision1-20260619-0001.md` | accepted | `cargo test -p trpg-harness`; `cargo check -p trpg-model -p trpg-db -p trpg-runtime -p trpg-gm -p trpg-harness -p trpg-cli`; focused model/runtime/gm/DB NPC tests; HOME-02 deterministic/replay CLI PASS; leakbranch FAIL closed as `WITHHELD_SECRET_LEAKED`; replay provenance mismatch FAIL closed; live-with-DB/no-key BLOCKED with no accepted cassette; `git diff --check` | TC-VS-NPC-01 Done; DA-NPC-01..05 Done; DA-MEM-02 Done; DA-MEM-03 Partial; DA-PIPE-02 Done |
| TC-VS-MEM-01 | `.tmp/team-lead/worker-tc-vs-mem-01-committed-memory-reload-20260619-0018.md` | accepted | `cargo test -p trpg-harness`; `cargo check -p trpg-model -p trpg-db -p trpg-runtime -p trpg-gm -p trpg-harness -p trpg-cli`; focused model/runtime/DB memory tests plus relationship extraction; HOME-03 deterministic/replay CLI PASS; failbranch FAIL closed as `EXTRACTION_NOT_FROM_COMMITTED` + `EVIDENCE_MISSING`; replay provenance mismatch FAIL closed; live-with-DB/no-key BLOCKED with no accepted cassette; `git diff --check` | TC-VS-MEM-01 Done; DA-SPOIL-02 Done; DA-MEM-01 Done; DA-MEM-03 Done; DA-PIPE-03 Partial |
| TC-PIPE-03 | `.tmp/team-lead/worker-tc-pipe-03-production-flight-recorder-provenance-recovery1-20260619-0108.md` | accepted | Isolated `CARGO_TARGET_DIR=.tmp/cargo-target-tcpipe03-lead`; GM `flight_recorder_provenance` 4/4; `no_spoiler_production_sources` 4/4; `cargo test -p trpg-harness` 75 + 38 + 5; workspace `cargo check`; GM lib 208/208; HOME-04 deterministic/replay CLI PASS; failbranch FAIL closed as `VIEW_ORDER_VIOLATION` + `SECRET_IN_TRACE`; replay provenance mismatch FAIL closed; live-with-DB/no-key BLOCKED with no accepted cassette; `git diff --check` | TC-PIPE-03 Done; DA-SPOIL-03 Done; DA-PIPE-01 Done; DA-PIPE-03 Done |
| TC-KNOW-FOUND-01 | `.tmp/team-lead/worker-tc-know-found-01-worldfact-holder-roundtrip-20260619-0130.md` | accepted | Isolated `CARGO_TARGET_DIR=.tmp/cargo-target-tcknowfound01-lead`; model mind-view 6/6; model knowledge 18/18 plus focused bins; runtime knowledge_projection 6/6; runtime npc_mind 4/4; DB live `live_knowledge_edges` 11/11; DB live `live_npc_mind_view` 4/4; `cargo check -p trpg-model -p trpg-db -p trpg-runtime`; `git diff --check` | TC-KNOW-FOUND-01 Done; DA-KNOW-01 Done; DA-KNOW-03 Done |

## Risks

- Prompt-only spoiler guard is insufficient.
- Foundation-only tasks may look Done while design remains Partial.
- NPC identity and holder semantics can block durable NPC knowledge.
- Memory extraction must be idempotent and event-backed.

## Final Report Checklist

Before declaring final Done, compare AcceptanceLedger rows and summarize:

- Done rows.
- Partial rows.
- Missing rows.
- Deferred rows.
- Blocked rows.
- V3/V4/V5 evidence gaps.

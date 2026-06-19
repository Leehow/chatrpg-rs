# Active Epic Ledger: EPIC-DESIGN3-PRODUCTION-RUNTIME

## Run mode

`continuous_epic`

## User goal

Continue from the completed Knowledge Runtime + NoSpoiler + NPC Mind foundation
and implement the remaining `design/设计3.md` production wiring without stopping
after each accepted task card.

## Constitution

- Source-grounded JSON asset runtime, not a universal rule compiler.
- Runtime commits; agents/plugins propose.
- Player-facing narration cannot leak player-unknown facts.
- NPC speech/action must be constrained by NPC knowledge, persona, goals, and
  relationship state.
- Heavy postprocess must not block user-visible streaming completion.

## Stop conditions

- All items are terminal.
- A hard blocker requires user decision.
- The working tree is unsafe for scoped worker dispatch.
- The user sets an explicit budget/stop.
- Repeated task failure leaves no safe bounded next action.

## Task ledger

| Order | Task ID | Path | Dependencies | Status | Worker/Handoff | Validation Evidence | Lead Decision | Next Action |
|---:|---|---|---|---|---|---|---|---|
| 0 | TC-D3-00 | `docs/agent-loop/task-cards/production-wiring/TC-D3-00-no-spoiler-production-sources-v1.md` | Completed EPIC-KNOWLEDGE-NPC-RUNTIME TC-KNOW-03, TC-P2-02 | Done | Initial handoff: `.tmp/team-lead/worker-tc-d3-00-no-spoiler-production-sources-v1-20260618-091504.md`; revision/recovery handoff: `.tmp/team-lead/worker-tc-d3-00-no-spoiler-production-sources-v1-revision1-20260618-093218.md` | Lead validation PASS: `cargo test -p trpg-gm no_spoiler`; `cargo test -p trpg-gm plugin`; `cargo test -p trpg-runtime spoiler_guard`; `cargo check -p trpg-model -p trpg-runtime -p trpg-gm`; `git diff --check`. | Accepted after revision: tightened `scene_node_id_from_block` parser, added non-scene module-id tests, fixed stale comment. | Dispatch TC-D3-01 immediately. |
| 1 | TC-D3-01 | `docs/agent-loop/task-cards/production-wiring/TC-D3-01-npc-profile-store-v1.md` | Completed TC-NPC-01 | Done | Prompt: `.tmp/team-lead/prompt-tc-d3-01-npc-profile-store-v1-20260618-094001.txt`; handoff: `.tmp/team-lead/worker-tc-d3-01-npc-profile-store-v1-20260618-094001.md`; Claude Code opus session 24780 | Lead validation PASS: `cargo test -p trpg-model npc_profile`; `DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg cargo test -p trpg-db --test live_npc_profiles -- --nocapture`; `cargo check -p trpg-model -p trpg-db -p trpg-runtime`; `git diff --check`. | Accepted: additive `npc_profiles` store, `upsert_npc_profile` / `load_npc_profile`, stable actor-id gates, safe-view tests, mismatch rejection. | Dispatch TC-D3-02 immediately. |
| 2 | TC-D3-02 | `docs/agent-loop/task-cards/production-wiring/TC-D3-02-active-npc-behavior-prompt-v1.md` | TC-D3-01, completed TC-NPC-03, TC-NPC-04 | Done | Prompt: `.tmp/team-lead/prompt-tc-d3-02-active-npc-behavior-prompt-v1-20260618-094924.txt`; handoff: `.tmp/team-lead/worker-tc-d3-02-active-npc-behavior-prompt-v1-20260618-094924.md`; Claude Code opus session 32788 | Lead validation PASS: `DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg cargo test -p trpg-gm npc_behavior`; `cargo test -p trpg-runtime --lib npc_behavior`; `cargo test -p trpg-model npc_behavior`; `cargo check -p trpg-model -p trpg-db -p trpg-runtime -p trpg-gm`; `git diff --check`. | Accepted: active NPC prompt-safe behavior guidance is loaded from durable profile + NPC mind view, secret-gated to fact IDs only, injected before player input, and fails soft on missing/corrupt profile. | Dispatch TC-D3-03 immediately. |
| 3 | TC-D3-03 | `docs/agent-loop/task-cards/production-wiring/TC-D3-03-memory-proposal-commit-pipeline-v1.md` | Completed TC-P2-01 | Done | Prompt: `.tmp/team-lead/prompt-tc-d3-03-memory-proposal-commit-pipeline-v1-20260618-104529.txt`; initial handoff: `.tmp/team-lead/worker-tc-d3-03-memory-proposal-commit-pipeline-v1-20260618-104529.md`; revision1 handoff: `.tmp/team-lead/worker-tc-d3-03-memory-proposal-commit-pipeline-v1-revision1-20260618-1117.md`; revision2 handoff: `.tmp/team-lead/worker-tc-d3-03-memory-proposal-commit-pipeline-v1-revision2-20260618-111617.md`; Claude Code opus session 21002 | Lead validation PASS: `cargo test -p trpg-runtime memory_proposal`; `cargo test -p trpg-db memory_proposal`; `cargo check -p trpg-model -p trpg-db -p trpg-runtime`; `git diff --check`; `DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg_codex_d303_1781751696 cargo test -p trpg-runtime memory_proposal -- --nocapture`; `DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg_codex_d303_1781751696 cargo test -p trpg-db memory_proposal -- --nocapture`. | Accepted after two revisions: runtime-owned proposal commit path covers world facts, knowledge updates/domain events, NPC relationship deltas, and legacy `memory_fact`; invalid or session-authority-violating batches abort before writes; proposal types remain inert. | Dispatch TC-D3-04 immediately. |
| 4 | TC-D3-04 | `docs/agent-loop/task-cards/production-wiring/TC-D3-04-relationship-extraction-social-gate-v1.md` | Completed TC-P2-01, TC-D3-03 | Done | Prompt: `.tmp/team-lead/prompt-tc-d3-04-relationship-extraction-social-gate-v1-20260618-112711.txt`; handoff: `.tmp/team-lead/worker-tc-d3-04-relationship-extraction-social-gate-v1-20260618-112711.md`; Claude Code opus session 21002 | Lead validation PASS: `cargo test -p trpg-runtime relationship_extraction`; `cargo test -p trpg-runtime memory_proposal`; `cargo check -p trpg-runtime -p trpg-model`; `cargo check -p trpg-gm`; `git diff --check`. | Accepted: deterministic social gate keeps new-entity behavior, adds active-NPC + social-signal trigger, preserves evidence refs into proposals, and still skips ordinary non-social turns. | Dispatch TC-D3-05 immediately. |
| 5 | TC-D3-05 | `docs/agent-loop/task-cards/production-wiring/TC-D3-05-projection-retrieval-and-verifier-integration-v1.md` | TC-D3-00, TC-D3-02, completed TC-P2-02 | Done | Prompt: `.tmp/team-lead/prompt-tc-d3-05-projection-retrieval-and-verifier-integration-v1-20260618-114344.txt`; handoff: `.tmp/team-lead/worker-tc-d3-05-projection-retrieval-and-verifier-integration-v1-20260618-114344.md`; Claude Code opus session 36748 | Lead validation PASS: `cargo test -p trpg-runtime projection`; `cargo test -p trpg-runtime verifier`; `cargo test -p trpg-gm no_spoiler`; `cargo test -p trpg-gm after_stream_verifier_uses_projection_findings`; `cargo check -p trpg-model -p trpg-runtime -p trpg-gm`; `git diff --check`. | Accepted: explicit player/GM/NPC speech/NPC action projections are exposed; GM hidden truth stays structurally separate from player-safe projection; NPC projections use the NPC mind/behavior plan path; AfterLlmStream now runs deterministic player projection leak verification against harvested secret terms with NoSpoiler fact-key dedup and redacted findings. | Epic terminal: no remaining unblocked tasks. |

Status values: NotStarted, InProgress, Done, Partial, NeedsRevision,
Rejected, BlockedByPrerequisite, BlockedByUnsafeWorkerState, BlockedByHuman,
Deferred, Untested.

## Current checkpoint

- Last accepted task: TC-D3-05 projection retrieval and verifier integration v1.
- Current worker: none; TC-D3-05 handoff reviewed and accepted.
- Next unblocked task: none.
- Stop condition reached: all items in this epic are terminal.

## Lead notes

- 2026-06-18: Opened this epic after comparing `design/设计3.md`, the completed
  `EPIC-KNOWLEDGE-NPC-RUNTIME` ledger, and the current code. This epic is about
  production wiring, not redoing the accepted P0/P1/P2 foundation.
- 2026-06-18: Prepared TC-D3-00 prompt for Claude Code opus TTY dispatch with
  scope limited to NoSpoiler production metadata/secret-term sources and focused
  tests. Lead will not accept until handoff, diff inspection, validation, and
  ledger update are complete.
- 2026-06-18: Dispatched TC-D3-00 to Claude Code opus. ACK gate passed in
  session 36607.
- 2026-06-18: Reviewed TC-D3-00 handoff and diff. Dispatched a narrow revision
  for one stale comment and over-permissive scene block id parsing. Recovery
  worker completed validation and handoff after an API interruption. Lead
  validation matrix passed and TC-D3-00 was accepted.
- 2026-06-18: Dispatched TC-D3-01 to Claude Code opus. ACK gate passed in
  session 24780.
- 2026-06-18: Reviewed TC-D3-01 handoff and diff. Lead validation matrix passed
  against model tests, live DB tests, targeted cargo check, and diff whitespace.
  Accepted TC-D3-01; dispatching TC-D3-02 next under continuous_epic.
- 2026-06-18: Dispatched TC-D3-02 to Claude Code opus. ACK gate passed in
  session 32788.
- 2026-06-18: Reviewed TC-D3-02 handoff and diff. Lead validation matrix
  passed against GM prompt tests (including DB round-trip), runtime/model
  behavior tests, targeted cargo check, and diff whitespace. Accepted TC-D3-02;
  dispatching TC-D3-03 next under continuous_epic.
- 2026-06-18: Dispatched TC-D3-03 to Claude Code opus. ACK gate passed in
  session 21002.
- 2026-06-18: Reviewed TC-D3-03 handoff and two revisions. Lead validation
  matrix passed against runtime/db targeted tests, targeted cargo check,
  whitespace diff check, and explicit temp-DB live tests. Accepted TC-D3-03;
  dispatching TC-D3-04 next under continuous_epic.
- 2026-06-18: Dispatched TC-D3-04 to Claude Code opus. ACK gate passed in
  session 21002.
- 2026-06-18: Reviewed TC-D3-04 handoff and diff. Lead validation matrix
  passed against relationship extraction tests, memory proposal regression,
  targeted runtime/model/gm checks, and whitespace diff check. Accepted
  TC-D3-04; dispatching TC-D3-05 next under continuous_epic.
- 2026-06-18: Dispatched TC-D3-05 to Claude Code opus. Initial reuse of the
  prior TTY hit Claude paste-preview input handling, so lead opened a fresh
  Opus TTY worker with the saved prompt file. ACK gate passed in session 36748.
- 2026-06-18: Reviewed TC-D3-05 handoff and scoped diff. Lead validation matrix
  passed against runtime projection tests, runtime verifier tests, GM NoSpoiler
  tests, the AfterLlmStream projection seam test, targeted model/runtime/gm
  check, and whitespace diff check. Accepted TC-D3-05; all
  EPIC-DESIGN3-PRODUCTION-RUNTIME task cards are terminal.

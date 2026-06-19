# Active Epic Ledger: EPIC-KNOWLEDGE-NPC-RUNTIME

## Run mode

`continuous_epic`

## User goal

Run the Knowledge Runtime + NoSpoiler + NPC Mind design from the current
state to P0/P1 terminal status without stopping after each accepted task card.
After every worker handoff, the lead reviews the handoff, inspects the diff,
runs or verifies validation, updates this ledger, and immediately dispatches
the next unblocked task unless a hard stop condition is hit.

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
| Setup | TC-AUTO-00 | `docs/agent-loop/task-cards/dev-loop/TC-AUTO-00-install-autonomous-loop.md` | none | Done | `.tmp/team-lead/worker-tc-auto-00-install-loop-acceptance-20260617.md` | Lead reviewed handoff; docs installed; forbidden legacy worker/build command scan was clean before v2 install, then v2 install was adapter-fixed for this Rust workspace. | Accepted as loop setup checkpoint. | Keep process docs aligned with continuous_epic. |
| 0 | TC-KNOW-00 | `docs/agent-loop/task-cards/knowledge-runtime/TC-KNOW-00-actor-identity-contract.md` | none | Done | `.tmp/team-lead/worker-tc-know-00-actor-identity-contract-20260617.md` | Lead reviewed handoff and diff; reran `cargo test -p trpg-model holder`; `cargo test -p trpg-model actor_identity`; `cargo test -p trpg-runtime --lib knowledge_projection`; `cargo test -p trpg-runtime --lib actor_identity`; `cargo check -p trpg-model -p trpg-db -p trpg-runtime -p trpg-gm`; `DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg cargo test -p trpg-db --test live_semantic_events -- --nocapture`; `DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg cargo test -p trpg-gm --test live_reveal_fact_tool -- --nocapture`; `git diff --check`; targeted `rustfmt --edition 2021 --check` on `knowledge.rs`, `knowledge_projection.rs`, and `live_semantic_events.rs`. | Accepted as Done. Actor identity now has typed `KnowledgeHolder` / `UnresolvedHolder`, stable NPC/PC/faction id parsing, runtime resolver, DB fail-closed validation for `record_npc_learned_fact`, and no durable NPC `knowledge_edges` writes. | Use as foundation for TC-KNOW-01 re-dispatch and later TC-KNOW-04 durable NPC holder opening. |
| 1 | TC-KNOW-01 | `docs/agent-loop/task-cards/knowledge-runtime/TC-KNOW-01-knowledge-edge-v1.md` | TC-KNOW-00 optional for player/GM foundation; required before durable NPC holders | Done | `.tmp/team-lead/worker-tc-know-01-knowledge-edge-v1-clean-20260617.md`; `.tmp/team-lead/worker-tc-know-01-migration-concurrency-rev1-20260617.md` | Lead reviewed the clean worker handoff/diff, found the default live test was false-green because concurrent 0033 migrations emitted `SKIP: migrate failed`, then dispatched and reviewed revision `tc-know-01-migration-concurrency-rev1`. Lead validation passed: `cargo test -p trpg-model knowledge`; default parallel `DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg cargo test -p trpg-db --test live_knowledge_edges -- --nocapture` with 7 tests executed and no migrate SKIP; `DATABASE_URL=... cargo test -p trpg-db --test live_semantic_events -- --nocapture`; `cargo check -p trpg-model -p trpg-db -p trpg-runtime -p trpg-gm`; `DATABASE_URL=... cargo test -p trpg-gm --test live_reveal_fact_tool -- --nocapture`; `git diff --check`. | Accepted as Done. KnowledgeEdge v1 now separates facts from holders for durable `gm`/`player_party`/`system`, adds v1 edge fields and projections, keeps durable NPC/PC/faction holders gated, and serializes `Db::migrate()` with a transaction-level advisory lock so live validation cannot silently skip migration failures. | Dispatch TC-KNOW-04 durable NPC KnowledgeEdges. Future migrations needing non-transactional DDL must be special-cased because `Db::migrate()` now runs inside one transaction. |
| 2 | TC-KNOW-02 | `docs/agent-loop/task-cards/knowledge-runtime/TC-KNOW-02-reveal-events-v1.md` | TC-KNOW-01 foundation | Done | `.tmp/team-lead/worker-tc-know-02-reveal-events-v1-20260617.md`; `.tmp/team-lead/worker-tc-know-02-recovery-clean-reapply-20260617.md` | Lead-verified recovery commands passed: `cargo test -p trpg-model semantic_split_kinds_token_and_serde_roundtrip`; `cargo test -p trpg-runtime --lib truthgraph`; `DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg cargo test -p trpg-db --test live_semantic_events -- --nocapture`; `DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg cargo test -p trpg-gm --test live_reveal_fact_tool -- --nocapture`; `cargo check -p trpg-model -p trpg-db -p trpg-runtime -p trpg-gm`; `git diff --check`. Scope-off paths `knowledge.rs`, `live_knowledge_edges.rs`, and `0033_knowledge_edges_v1_fields.sql` remain untouched/absent. | Accepted as Done after recovery clean reapply. Reveal event semantic split is preserved without TC-KNOW-01 contamination. Durable NPC persistence remains outside this card until TC-KNOW-00/04. | Keep as Done; ensure later tasks preserve `ContextSurfaced` / `PlayerExposed` / `PlayerLearnedFact` / `NpcLearnedFact` semantics. |
| 3 | TC-KNOW-04 | `docs/agent-loop/task-cards/knowledge-runtime/TC-KNOW-04-npc-durable-knowledge-edges.md` | TC-KNOW-00, TC-KNOW-01, TC-KNOW-02 | Done | `.tmp/team-lead/worker-tc-know-04-npc-durable-knowledge-edges-20260618.md` | Lead reviewed handoff and scoped implementation; inspected 0034 migration, model durability predicates, DB upsert/write-through/projection paths, and live tests. Lead validation passed: `cargo test -p trpg-model knowledge`; `cargo test -p trpg-runtime --lib truthgraph`; `DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg cargo test -p trpg-db --test live_knowledge_edges -- --nocapture` with 10 tests executed and no migrate SKIP; `DATABASE_URL=... cargo test -p trpg-db --test live_semantic_events -- --nocapture`; `cargo check -p trpg-model -p trpg-db -p trpg-runtime -p trpg-gm`; `git diff --check`; `DATABASE_URL=... cargo test -p trpg-gm --test live_reveal_fact_tool -- --nocapture`. | Accepted as Done. Durable NPC `knowledge_edges(holder_kind='npc')` are open only for stable actor-id holders; `record_npc_learned_fact` keeps the event ledger and writes an idempotent per-NPC edge; player_party/GM projections stay isolated; NPC belief edges do not become known truth; pc/faction remain gated. Invalid NPC ids preserve the accepted TC-KNOW-00 fail-closed public API with no durable row. | Dispatch TC-KNOW-03 no-spoiler plugin v2. |
| 4 | TC-KNOW-03 | `docs/agent-loop/task-cards/knowledge-runtime/TC-KNOW-03-no-spoiler-plugin-v2.md` | TC-KNOW-01, TC-KNOW-02 | Done | `.tmp/team-lead/worker-tc-know-03-no-spoiler-plugin-v2-20260618.md` | Lead reviewed handoff and scoped diff; reran `cargo test -p trpg-gm no_spoiler`; `cargo test -p trpg-gm plugin`; `cargo test -p trpg-gm plugin_mechanism`; `cargo test -p trpg-runtime spoiler_guard`; `cargo check -p trpg-model -p trpg-db -p trpg-runtime -p trpg-gm`; `DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg cargo test -p trpg-db --test live_semantic_events -- --nocapture`; `DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg cargo test -p trpg-gm --test live_reveal_fact_tool -- --nocapture`; `cargo test -p trpg-gm --lib`; `git diff --check`. | Accepted as Done. NoSpoiler v2 now has a ContextAssembly fail-closed ContextFilter over private block metadata, high-priority Safety PromptBlock with metadata, AfterLlmStream deterministic SecretLeak verifier, trace coverage, and DB-backed player-known fact gating. Production block filtering still waits for producers to tag blocks with `spoiler_secret` / `future_scene` / `fact:<id>`, and production leak verification waits for a `secret_terms` source; this is recorded as an additive follow-up, not a TC-KNOW-03 blocker. | Dispatch TC-NPC-01 npc profile v1. |
| 5 | TC-NPC-01 | `docs/agent-loop/task-cards/npc-mind/TC-NPC-01-npc-profile-v1.md` | TC-KNOW-00 | Done | `.tmp/team-lead/worker-tc-npc-01-npc-profile-v1-20260618-003723.md` | Lead reviewed worker handoff and scoped diff; reran `cargo test -p trpg-model --test npc_profile`; `cargo test -p trpg-runtime --lib npc_profile`; `cargo check -p trpg-model -p trpg-db -p trpg-runtime -p trpg-gm`; `git diff --check`; `cargo test -p trpg-model npc_profile`. | Accepted as Done. NPC Profile v1 now provides a serde/JSON round-trippable static persona model, fail-closed visibility-tagged secrets, a structurally secret-free `NpcProfileSafeView`, speech-style prompt projection, source-ref preservation, and a pure runtime `persona_from_profile` adapter that excludes GM-only secrets. | Dispatch TC-NPC-02 npc relationship v1. |
| 6 | TC-NPC-02 | `docs/agent-loop/task-cards/npc-mind/TC-NPC-02-npc-relationship-v1.md` | TC-KNOW-00, TC-NPC-01 | Done | `.tmp/team-lead/prompt-tc-npc-02-npc-relationship-v1-20260618-004818.txt`; `.tmp/team-lead/worker-tc-npc-02-npc-relationship-v1-20260618-004818.md`; `.tmp/team-lead/prompt-tc-npc-02-rev1-target-validation-20260618-010243.txt`; `.tmp/team-lead/worker-tc-npc-02-rev1-target-validation-20260618-010243.md` | Lead reviewed base and revision handoffs/diffs. Lead validation passed: `cargo test -p trpg-model --test npc_relationship`; `cargo test -p trpg-model npc_relationship`; `cargo test -p trpg-runtime --lib npc_relationship`; `DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg cargo test -p trpg-db --test live_npc_relationships -- --nocapture`; `cargo check -p trpg-model -p trpg-db -p trpg-runtime -p trpg-gm`; `git diff --check`. | Accepted as Done after revision. NPC relationship state now has evidence-required bounded deltas, deterministic help/threat transitions, durable roundtrip/update helpers, additive migration 0035, and target identity fail-closed on construction, row hydration, durable writes, and corrupt-row loads. | Dispatch TC-NPC-03 npc mind view v1. |
| 7 | TC-NPC-03 | `docs/agent-loop/task-cards/npc-mind/TC-NPC-03-npc-mind-view-v1.md` | TC-KNOW-04, TC-NPC-01, TC-NPC-02 | Done | `.tmp/team-lead/prompt-tc-npc-03-npc-mind-view-v1-20260618-011402.txt`; `.tmp/team-lead/worker-tc-npc-03-npc-mind-view-v1-20260618-011402.md` | Lead reviewed worker final response, full handoff, scoped model/runtime/DB code, and tests. Lead validation passed: `cargo test -p trpg-model --test npc_mind_view`; `cargo test -p trpg-model npc_mind`; `cargo test -p trpg-model knowledge_state_token_round_trip`; `cargo test -p trpg-runtime --lib npc_mind`; `DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg cargo test -p trpg-db --test live_npc_mind_view -- --nocapture`; `cargo check -p trpg-model -p trpg-db -p trpg-runtime -p trpg-gm`; `git diff --check`. | Accepted as Done. NPC Mind View now composes durable per-NPC knowledge/belief edges, persona safe view, and compact relationship summaries into a prompt-safe surface; `knows_true` stays known truth, belief states stay beliefs, weaker/unknown states and other holders are excluded, and speech context excludes GM-only profile secrets and GM/party/other-NPC facts. Validation passed with existing workspace warnings still present. | Dispatch TC-NPC-04 npc behavior plan v1. |
| 8 | TC-NPC-04 | `docs/agent-loop/task-cards/npc-mind/TC-NPC-04-npc-behavior-plan-v1.md` | TC-NPC-03 | Done | `.tmp/team-lead/prompt-tc-npc-04-npc-behavior-plan-v1-20260618-012943.txt`; `.tmp/team-lead/worker-tc-npc-04-npc-behavior-plan-v1-20260618-012943.md` | Lead reviewed worker final response, full handoff, scoped model/runtime code, task-card acceptance, and tests. Lead validation passed: `cargo test -p trpg-model --test npc_behavior`; `cargo test -p trpg-model npc_behavior`; `cargo test -p trpg-runtime --lib npc_behavior`; `cargo check -p trpg-model -p trpg-db -p trpg-runtime -p trpg-gm`; `git diff --check`. | Accepted as Done. NPC Behavior Plan v1 now derives bounded deterministic action/speech guidance from `NpcMindView`, keeps output proposal-only, gates revealable facts to known-true NPC facts, withholds known secrets below threshold, models trust/fear/hostility effects, and adds a read-only runtime load-and-derive adapter. Validation passed with existing workspace warnings still present. | P0/P1 Knowledge Runtime + NoSpoiler + NPC Mind lane is terminal. P2 integration hardening remains uncarded/deferred until explicitly opened. |
| 9 | TC-P2-01 | `docs/agent-loop/task-cards/integration-hardening/TC-P2-01-memory-extraction-proposals-v1.md` | TC-KNOW-00, TC-KNOW-01, TC-KNOW-02, TC-KNOW-04, TC-NPC-02 | Done | `.tmp/team-lead/prompt-tc-p2-01-memory-extraction-proposals-v1-20260618-055313.txt`; `.tmp/team-lead/worker-tc-p2-01-memory-extraction-proposals-v1-20260618-055313.md` | Lead reviewed worker final response, full handoff, scoped model/runtime proposal code, and tests. Lead validation passed: `cargo test -p trpg-model memory_proposal`; `cargo test -p trpg-runtime memory_proposal`; `cargo test -p trpg-runtime relationship_extraction`; `cargo check -p trpg-model -p trpg-db -p trpg-runtime -p trpg-gm`; `cargo test -p trpg-runtime --test memory_proposal`; `git diff --check`. | Accepted as Done. Memory extraction proposals are pure/inert data: world fact identity and holder knowledge update proposals are split, NPC holders route through actor identity and fail closed, relationship deltas require evidence and do not apply, source refs are preserved, and the existing relationship-triple path is bridged without changing `relationship_extraction`. | Dispatch TC-P2-02 knowledge leak / NPC-mind verifier. |
| 10 | TC-P2-02 | `docs/agent-loop/task-cards/integration-hardening/TC-P2-02-knowledge-leak-verifier-v1.md` | TC-KNOW-03, TC-NPC-03, TC-NPC-04 | Done | `.tmp/team-lead/prompt-tc-p2-02-knowledge-leak-verifier-v1-20260618-061020.txt`; `.tmp/team-lead/worker-tc-p2-02-knowledge-leak-verifier-v1-20260618-061020.md` | Lead reviewed worker final response, full handoff, scoped model/runtime verifier code, and tests. Lead validation passed: `cargo test -p trpg-model verifier`; `cargo test -p trpg-runtime verifier`; `cargo test -p trpg-gm no_spoiler`; `cargo check -p trpg-model -p trpg-db -p trpg-runtime -p trpg-gm`; `git diff --check`. | Accepted as Done. Knowledge Leak Verifier v1 now has pure projection-driven model helpers for player-unknown fact leaks, NPC disclosure outside behavior plan, withheld secret disclosure, and belief/unheld fact asserted as known truth, plus a runtime bridge to `VerifierFinding` (`SecretLeak` / `InventedEffect`). Findings remain redacted to fact/holder ids and safe summaries; no DB, live stream, provider, or turn-loop wiring was added. `knowledge_leak_verifier.rs` in `trpg-model` is 486 lines because inline tests keep the `verifier` filter effective; recorded as soft size debt, not an acceptance blocker. | Dispatch TC-P2-03 golden harness cases. |
| 11 | TC-P2-03 | `docs/agent-loop/task-cards/integration-hardening/TC-P2-03-golden-harness-cases-v1.md` | TC-P2-02 | Done | `.tmp/team-lead/prompt-tc-p2-03-golden-harness-cases-v1-20260618-062236.txt`; `.tmp/team-lead/worker-tc-p2-03-golden-harness-cases-v1-20260618-062236.md` | Lead reviewed worker final response, full handoff, scoped harness diff, golden case fixtures, and tests. Lead validation passed: `cargo test -p trpg-harness`; `cargo check -p trpg-cli -p trpg-harness`; `git diff --check`. | Accepted as Done. Golden harness coverage now has three small source-safe fixtures for Homecoming no-spoiler, Triangle required events, and CoC-style investigation, plus provider-free tests over the shared harness case parser and assertion evaluator. The small `trpg_harness` library extraction is accepted as necessary to test the binary's actual parse/assert logic without spawning `trpg` or an LLM provider. | Epic task plan is terminal; no unblocked task remains in `EPIC-KNOWLEDGE-NPC-RUNTIME.md`. |

Status values: NotStarted, InProgress, Done, Partial, NeedsRevision,
Rejected, BlockedByPrerequisite, BlockedByUnsafeWorkerState, BlockedByHuman,
Deferred, Untested.

## Current checkpoint

- Last accepted task: TC-P2-03 golden harness cases v1.
- Current worker: none.
- Next intended task: none; all tasks in the ordered epic plan are terminal.
- Why continuing: user asked to use autonomous loop `continuous_epic` mode after
  P0/P1 reached terminal, so the previously deferred P2 integration hardening
  bullets were opened as explicit task cards and are now Done.

## Lead notes

- 2026-06-17: Installed autonomous loop v2 continuous epic patch from
  `design/chatrpg_autonomous_loop_v2_continuous_epic_patch`.
- 2026-06-17: Adapter-fixed the installed loop docs to use this repo's
  `[TEAM_LEAD_WORKER_V1]` marker and `claude --model opus` worker route.
- 2026-06-17: TC-KNOW-01 is intentionally not marked Done until the current
  worker handoff, diff, and tests are reviewed against the task card.
- 2026-06-17: Reviewed worker `tc-know-01-gap-fix-v1` handoff and diff stat.
  The worker accidentally ran broad formatting across trpg-model/trpg-db/
  trpg-runtime, expanding the tracked diff to 58 files and 10666 insertions /
  2367 deletions. TC-KNOW-01 is BlockedByUnsafeWorkerState, not Done. The next
  safe move requires explicit recovery authorization because cleanup would
  involve destructive/overwriting git restore-style containment, or a deliberate
  switch to a clean replacement worktree.
- 2026-06-17: User authorized recovery. Lead saved pre-cleanup checkpoints under
  `.tmp/team-lead/recovery-*-20260617.*`, restored unsafe TC-KNOW-01 tracked
  files, removed the unaccepted `0033_knowledge_edges_v1_fields.sql`, dispatched
  `tc-know-02-recovery-clean-reapply`, reviewed its handoff/diff, and reran the
  required validation plus `git diff --check`. Recovery accepted; TC-KNOW-02
  remains Done, TC-KNOW-01 is NeedsRevision, and TC-KNOW-00 is the next
  unblocked task.
- 2026-06-17: Dispatched `tc-know-00-actor-identity-contract` with Claude Code
  opus. Lead accepted the handoff after inspecting the actor identity contract,
  runtime resolver, DB event-ledger-only validation path, and focused live tests.
  Validation passed: model holder/actor_identity tests, runtime knowledge
  projection/actor_identity tests, four-crate `cargo check`, DB
  `live_semantic_events`, GM `live_reveal_fact_tool`, `git diff --check`, and
  targeted rustfmt checks on the newly changed focused files. `cargo fmt --check`
  remains unsuitable as a whole-workspace gate because of pre-existing formatting
  debt; no broad formatting was accepted.
- 2026-06-17: Accepted TC-KNOW-01 after clean implementation plus migration
  concurrency revision. Lead reproduced the false-green live test behavior, then
  verified the accepted fix: `Db::migrate()` now serializes concurrent migrations
  with a transaction-level advisory lock, `live_knowledge_edges` fails on migrate
  errors after successful DB connection, default parallel `live_knowledge_edges`
  runs all 7 tests without migrate SKIP, `live_semantic_events` and
  `live_reveal_fact_tool` pass, four-crate `cargo check` passes with existing
  warnings only, and `git diff --check` is clean. Durable NPC `knowledge_edges`
  remains gated to TC-KNOW-04.
- 2026-06-18: Accepted TC-KNOW-04 after worker handoff review, scoped code
  inspection, and lead rerun of model/runtime/live DB/four-crate/GM/diff
  validation. Migration 0034 opens only `npc`; generic NPC durable writes are
  actor-identity-gated and normalized; `NpcLearnedFact` now dual-writes event
  ledger plus durable per-NPC `knows_true` edge; player_party/GM leakage tests,
  belief-vs-truth tests, replay idempotency tests, and pc/faction gated checks
  pass. `list_npc_known_fact_ids` currently expects the stable normalized
  actor id at read time while write paths normalize; note as a possible later
  hardening, not a TC-KNOW-04 blocker.
- 2026-06-18: Dispatched `tc-know-03-no-spoiler-plugin-v2` to Claude Code opus
  with scope limited to the NoSpoiler/plugin mechanism path and no DB/model
  schema changes. ACK gate passed in session 94105.
- 2026-06-18: Accepted TC-KNOW-03 after worker handoff review, scoped diff
  inspection, and lead validation. The card now has tested ContextFilter,
  PromptBlock, deterministic SecretLeak verifier, trace, and player-known fact
  gating. Forward note: production context blocks still need explicit
  `spoiler_secret` / `future_scene` / `fact:<id>` tagging and AfterLlmStream
  still needs a production `secret_terms` source before the new defenses are
  fully lit up outside tests; runtime source-entity `spoiler_guard` remains the
  current production redaction path.
- 2026-06-18: Dispatched `tc-npc-01-npc-profile-v1` to Claude Code opus with
  scope limited to static NPC profile, safe prompt view, speech-style
  projection, and focused tests. ACK gate passed in session 88857.
- 2026-06-18: Accepted TC-NPC-01 after worker handoff review, scoped code
  inspection, and lead validation. The profile model round-trips through serde,
  safe views structurally exclude GM-only secret text, speech-style projection
  is tested, source refs survive storage, and the runtime adapter builds
  `NpcPersona` from non-secret fields only. Existing cargo warnings remain
  unrelated; `git diff --check` is clean.
- 2026-06-18: Dispatched `tc-npc-02-npc-relationship-v1` to Claude Code opus
  with scope limited to NPC relationship state, bounded evidence-required
  deltas, durable storage/runtime helpers, additive migration if needed, and
  focused tests. ACK gate passed in session 71068.
- 2026-06-18: Reviewed TC-NPC-02 worker handoff and scoped diff. The base
  relationship implementation and worker validations were promising, but lead
  found a pre-acceptance gap: `NpcRelationshipTarget::from_parts()` validates
  targets, while public enum constructors and `Db::upsert_npc_relationship`
  could bypass target-id validation. Marked TC-NPC-02 NeedsRevision and prepared
  `tc-npc-02-rev1-target-validation` to make target identity fail-closed on
  construction and durable writes.
- 2026-06-18: Accepted TC-NPC-02 after revision handoff review, scoped diff
  inspection, and lead validation. Relationship targets are now validated and
  normalized through kind-specific actor identity constructors at `new`,
  `from_parts`, `upsert_npc_relationship`, and load boundaries. Lead reran
  model integration tests, model inline relationship tests, runtime relationship
  tests, live DB relationship tests, four-crate check, and `git diff --check`;
  all passed with only pre-existing warnings.
- 2026-06-18: Prepared TC-NPC-03 prompt for Claude Code opus TTY dispatch after
  TC-NPC-02 acceptance. Scope is limited to NPC mind view model/runtime helpers,
  optional DB read integration, and focused tests; migrations, GM turn loop,
  plugins, final dialogue generation, and shared ledger edits are out of scope.
- 2026-06-18: Dispatched `tc-npc-03-npc-mind-view-v1` to Claude Code opus with
  `superpowers: optional`, `subagent_policy: research_only`, and `commit_policy:
  no_commit`. ACK gate passed in session 74607.
- 2026-06-18: Accepted TC-NPC-03 after worker handoff review, scoped model/runtime/DB
  inspection, and lead validation. The mind view is anchored to a stable NPC id and
  same-NPC profile, projects only `knows_true` plus explicit belief states from
  that NPC's durable edges, filters weaker/unknown states, compacts relationship
  summaries, and builds a prompt-safe `speech_context()` from persona safe view plus
  known/believed fact ids. Lead reran the required model test suite, model inline
  tests, `KnowledgeState::from_token` roundtrip, runtime mind tests, live DB
  `live_npc_mind_view`, four-crate check, and `git diff --check`; all passed, with
  existing workspace warnings still present.
- 2026-06-18: Prepared TC-NPC-04 prompt for Claude Code opus TTY dispatch. Scope is
  limited to behavior plan model/runtime helpers and focused tests; migrations, DB,
  GM turn loop wiring, final dialogue generation, and shared ledger edits are out of
  scope.
- 2026-06-18: Dispatched `tc-npc-04-npc-behavior-plan-v1` to Claude Code opus with
  `superpowers: optional`, `subagent_policy: research_only`, and `commit_policy:
  no_commit`. ACK gate passed in session 62653.
- 2026-06-18: Accepted TC-NPC-04 after worker handoff review, scoped model/runtime
  inspection, and lead validation. The behavior plan is a deterministic
  proposal-only surface derived from prompt-safe `NpcMindView` plus non-truth
  context: hostility lowers help willingness, fear lowers reveal willingness
  unless compliance is plausible, trust raises cooperation, revealable facts are
  limited to known-true NPC facts, and known secrets can be withheld below the
  reveal threshold. Lead reran the required model acceptance tests, model inline
  tests, runtime behavior tests, four-crate check, and `git diff --check`; all
  passed, with existing workspace warnings still present. P0/P1 is terminal.
- 2026-06-18: User requested autonomous loop `continuous_epic` mode. Lead reopened
  the epic beyond P0/P1 by converting the P2 integration hardening bullets into
  task cards: TC-P2-01 memory extraction proposals, TC-P2-02 knowledge leak /
  NPC-mind verifier, and TC-P2-03 golden harness cases. Next dispatch is
  TC-P2-01.
- 2026-06-18: Dispatched `tc-p2-01-memory-extraction-proposals-v1` to Claude
  Code opus with `autonomy: enabled`, `run_mode: continuous_epic`,
  `subagent_policy: research_only`, and `commit_policy: no_commit`. ACK gate
  passed in session 58958.
- 2026-06-18: Accepted TC-P2-01 after worker handoff review, scoped code
  inspection, and lead validation. The proposal layer is additive and
  proposal-only: model types split world fact identity from holder knowledge,
  validate stable holders via the TC-KNOW-00 contract, preserve source refs,
  require evidence for relationship deltas, and never write DB rows, append
  domain events, or mutate relationships. Runtime helpers only parse
  deterministic JSON and bridge existing relationship `MemoryFact`s into
  proposals. Lead reran the required model/runtime/relationship/four-crate/diff
  validation plus `cargo test -p trpg-runtime --test memory_proposal`; all
  passed with existing workspace warnings still present.
- 2026-06-18: Dispatched `tc-p2-02-knowledge-leak-verifier-v1` to Claude Code
  opus with `autonomy: enabled`, `run_mode: continuous_epic`,
  `subagent_policy: research_only`, and `commit_policy: no_commit`. ACK gate
  passed in session 24275.
- 2026-06-18: Accepted TC-P2-02 after worker handoff review, scoped model/runtime
  inspection, and lead validation. The verifier is pure and projection-driven:
  player-visible leak checks use caller-provided fact markers plus durable
  player-known fact ids; NPC checks use `NpcMindView` / `NpcBehaviorPlan` only;
  belief states are not promoted to truth; runtime bridging reuses
  `SecretLeak`/`InventedEffect`; findings never echo matched secret text. Lead
  reran model verifier tests, runtime verifier tests, NoSpoiler regression,
  four-crate check, and `git diff --check`; all passed with existing workspace
  warnings only. The model verifier file exceeds the soft 400-line guideline due
  to inline tests needed by the `verifier` filter; accepted as tracked debt.
- 2026-06-18: Dispatched `tc-p2-03-golden-harness-cases-v1` to Claude Code opus
  with `autonomy: enabled`, `run_mode: continuous_epic`,
  `subagent_policy: research_only`, and `commit_policy: no_commit`. ACK gate
  passed in session 11850.
- 2026-06-18: Accepted TC-P2-03 after worker final response and handoff review,
  scoped harness/code/fixture inspection, and lead validation. The task adds
  three source-safe golden fixtures, exposes the harness case schema plus JSONL
  parser/assertion evaluator as a small in-crate library, and verifies the
  required Homecoming/Triangle/CoC tests plus failure cases without a live
  provider. Lead reran `cargo test -p trpg-harness`,
  `cargo check -p trpg-cli -p trpg-harness`, and `git diff --check`; all passed
  with existing workspace warnings still present. All tasks in the ordered epic
  plan are terminal.

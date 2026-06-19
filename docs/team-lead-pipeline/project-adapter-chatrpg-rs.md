# chatrpg-rs Team Lead project adapter

## Constitution

- Source-grounded JSON Asset Runtime; no universal rule compiler.
- Rust executes and commits; LLMs/plugins propose.
- NeedBus is the acquisition path; BindingResolver decides execution tier.
- Player-facing output cannot contain player-unknown truth.
- NPC speech/action is limited by NPC knowledge, persona, relationship, and goals.
- SSE remains streaming; heavy postprocess does not block completion.

## Hotspots

Treat these as single-writer leases until split:

- `crates/trpg-model/src/lib.rs`
- `crates/trpg-db/src/lib.rs`
- `crates/trpg-parser/src/lib.rs`
- `crates/trpg-runtime/src/lib.rs`
- root `Cargo.toml`
- `migrations/**`
- central plugin/phase registries
- shared golden snapshots and ledgers

## Marker compatibility

Use `[TEAM_LEAD_WORKER_V1]` plus the parallel-pipeline extension fields in
`docs/team-lead-pipeline/templates/worker-marker-v1-parallel.txt`. The v2
template is a future-reference sample only until the global Team Lead worker
skill accepts it.

## WorkGraph and acceptance

- `AcceptanceLedger.md` decides whether gameplay/design rows are Done.
- `WorkGraph.yaml` decides dependencies, write conflicts, leases, worker
  state, and integration state.
- `IntegrationReport.md` records each trial integration event.
- Workers read their assigned WorkGraph row but do not edit the WorkGraph.

## Environment isolation

Each code worktree receives:

- a unique branch;
- a unique `CARGO_TARGET_DIR` (optionally backed by shared sccache);
- a unique PostgreSQL database/schema;
- unique API/relay ports when starting services;
- lane-scoped logs and cassette paths.

## Test policy

- Workers run owned-module and affected-crate tests.
- Integration train runs cross-crate smoke tests.
- Milestones run connected Journey tests from character creation through later-turn/reload persistence.
- Broad repository formatting is integration-owned; workers format owned files only when the repository has pre-existing formatting drift.

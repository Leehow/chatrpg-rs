# Documentation Drift Reconciliation — 2026-07-01

Status: documentation-only reconciliation
Primary companion: `docs/status/CURRENT_RUNTIME_SNAPSHOT.md`

## 1. Why this document exists

The implementation has moved faster than the design/status docs. Several older
architecture notes were true when written, but are no longer true on current
`main`. This document reconciles the biggest drift points so automated agents do
not implement against stale assumptions.

The current source of truth for runtime shape is:

```text
1. Current code and migrations
2. Rust tests and harness contracts
3. docs/status/CURRENT_RUNTIME_SNAPSHOT.md
4. docs/architecture/source-grounded-json-runtime.md
5. AGENTS.md and docs/codex-team-lead/02-validation-rubric.md
6. Older design/status docs, only when not contradicted by the above
```

## 2. Drift table

| Area | Older claim or implicit assumption | Current implementation | Required agent behavior |
|---|---|---|---|
| Runtime maturity | Repository is a starter skeleton. | It is a multi-crate Rust runtime prototype with parser, search, memory, mechanics, ports, harness, and product-gate infrastructure. | Treat README starter wording as historical shorthand, not architecture scope. |
| Control plane | `trpg-orchestrator::TurnOrchestrator` is the turn orchestrator. | The actual control plane is `trpg_gm::execute::run_pipeline`; `trpg-orchestrator::TurnOrchestrator` is lifecycle/gate reduction. | Put turn pipeline changes in/around `run_pipeline`, not in the similarly named lifecycle reducer. |
| Ports | Ports / NarrationPacket / independent Narrator have zero landing. | Six port traits exist in `crates/trpg-gm/src/ports.rs`; `ports_adapters.rs` checks production callers. `AdjudicationPacket` and `NarrationPacket` exist. | Do not recreate parallel shell types. Extend the existing port/packet layer. |
| Policy | Policy is only a hook concept. | Policy is still cross-cutting, but also has `PolicyPort` around presentation gate logic. | Model policy changes as gate/plugin/port contributions, not as a new sequential runtime phase. |
| Context compilers | Five separate context compilers are the target next step. | `RuntimeEngine::prepare_turn_context` remains the single assembly entry and should be refactored gradually. | Extract helpers without changing BP1/BP2/BP3 semantics. Do not prematurely introduce five compilers. |
| Knowledge holders | NPC knowledge can be durable now. | DB migration `0032_knowledge_edges.sql` restricts durable holders to `gm` and `player_party`. NPC holders remain gated on stable actor identity. | Do not persist NPC knowledge edges until actor identity contract is explicit and tested. |
| Truth vs memory | `memory_facts` can serve as truth. | `world_facts` now exists as a separate truth/candidate fact table. Knowledge is represented through `knowledge_edges`. | Use `world_facts` for truth, `knowledge_edges` for holder knowledge, memory for retrieval/continuity. |
| Mechanics exactness | Implemented mechanics mean final rules execution. | Many mechanics are partial/provisional until source-backed facets/source packs are hydrated. | Block/degrade when source evidence is missing; do not narrate provisional math as final. |
| Hardcoding removal | Hardcoding is only a design rule. | Combat policy already moved many ruleset/module-specific branches into data-driven policies and module config. | Continue data-driven policy; never reintroduce `ruleset_id.contains` or module-name branches in runtime. |
| Acceptance | Unit/DB tests prove gameplay work. | Validation rubric requires public-path journeys for player-facing claims. | Mark product work done only with V3+ journey evidence. |

## 3. Document classification

### 3.1 Current / canonical

- `docs/status/CURRENT_RUNTIME_SNAPSHOT.md`
- `docs/architecture/source-grounded-json-runtime.md`
- `AGENTS.md`
- `CLAUDE.md`
- `docs/codex-team-lead/02-validation-rubric.md`
- `harness/README.md`

### 3.2 Current but append-only / status history

- `docs/status/known_issues.md`
- `docs/status/rule_steward_*.md`
- other `docs/status/*` implementation reports

Use these as evidence of when changes landed, not as the single current map.

### 3.3 Historical / must be checked against current snapshot

- `docs/architecture/layered-runtime-invariants.md`
- older `docs/superpowers/plans/*`
- older `docs/superpowers/specs/*`
- older design notes that mention planned layers or unlanded surfaces

Do not delete these; many still contain important invariants. But when a claim
conflicts with the current snapshot, the snapshot wins.

## 4. Specific stale claim corrections

### 4.1 `layered-runtime-invariants.md` INV-1

Historical claim:

```text
Ports / independent Narrator / NarrationPacket / five ContextCompiler = zero landing
```

Current correction:

```text
Ports exist.
AdjudicationPacket/NarrationPacket exist.
Port production-call guards exist.
Five ContextCompiler implementations still should not be introduced yet.
```

The first half is superseded; the last part remains aligned with the current
incremental strategy.

### 4.2 README wording

Historical wording:

```text
Rust starter implementation...
```

Current correction:

```text
Operational README remains useful, but the architecture status is now tracked in
CURRENT_RUNTIME_SNAPSHOT.md. The project should be treated as a mature prototype,
not a starter skeleton.
```

### 4.3 `known_issues.md` terminal status

Historical issue:

`known_issues.md` is an append-only fix log and may not include the newest status
reports from separate `docs/status/rule_steward_*.md` files.

Current correction:

Append reconciliation notes there, but do not treat its last heading as the only
state of the repo.

## 5. Agent operating rules after this reconciliation

1. Before code-affecting work, read `CURRENT_RUNTIME_SNAPSHOT.md` plus the task
   card.
2. If a design doc and code disagree, confirm against:
   - current migrations;
   - current tests;
   - current source paths listed in the snapshot.
3. Do not create new architecture surfaces when an existing port, packet,
   projection, or reducer already exists.
4. Do not mark a gameplay feature complete from V1/V2 evidence only.
5. If you change an architecture surface, update the snapshot in the same PR.

## 6. Recommended next documentation cleanups

1. Add an `Architecture Index` page linking current vs historical docs.
2. Mark old plan/spec files with lightweight supersession headers when their
   claims have been overtaken by code.
3. Add validation profiles (`quick-dev`, `merge-gate`, `product-gate`) as a
   committed document or script entry.
4. Keep a short changelog of architecture surface changes after each runtime
   milestone.

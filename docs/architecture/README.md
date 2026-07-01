# Architecture Documentation Index

This index separates current architecture sources from historical design notes.
Use it before starting code-affecting work.

## Current sources of truth

1. `docs/status/CURRENT_RUNTIME_SNAPSHOT.md`
   - Current implementation map.
   - Read this when docs and code disagree.
2. `docs/status/DOCUMENTATION_DRIFT_RECONCILIATION_2026-07-01.md`
   - Known drift corrections and stale-claim mapping.
3. `docs/architecture/source-grounded-json-runtime.md`
   - Architecture constitution: Source-grounded JSON Asset Runtime.
4. `docs/codex-team-lead/02-validation-rubric.md`
   - Journey-driven validation levels and qualification gates.
5. `AGENTS.md` and `CLAUDE.md`
   - Agent and worker operating rules.

## Historical guardrails

- `docs/architecture/layered-runtime-invariants.md`

This file still contains important guardrails, especially the control-plane
identity of `trpg-gm::execute::run_pipeline`, but it is a historical P0 snapshot.
Some claims, such as "Ports / NarrationPacket have zero landing", have been
superseded by current implementation. Check `CURRENT_RUNTIME_SNAPSHOT.md` first.

## Implementation evidence anchors

When auditing the current architecture, inspect these files:

```text
crates/trpg-gm/src/execute.rs              # real turn control plane
crates/trpg-gm/src/turn_plan.rs            # canonical 15-phase plan
crates/trpg-gm/src/ports.rs                # layered port traits/adapters
crates/trpg-gm/tests/ports_adapters.rs     # port production-call guards
crates/trpg-gm/src/packet.rs               # AdjudicationPacket / NarrationPacket
crates/trpg-runtime/src/lib.rs             # RuntimeEngine and prepare_turn_context
crates/trpg-combat/src/policy.rs           # data-driven combat policy
migrations/0032_knowledge_edges.sql        # current durable holder constraints
migrations/0039_world_facts.sql            # first-class WorldFact table
harness/README.md                          # Rust-native harness and playtest flow
```

## Maintenance rule

If a change modifies any of the following, update
`docs/status/CURRENT_RUNTIME_SNAPSHOT.md` in the same PR:

- turn pipeline or phase order;
- context assembly / BP1-BP2-BP3 semantics;
- port wiring;
- narration packet visibility contract;
- knowledge/fact schema;
- source-backed mechanical execution tiers;
- product validation profile.

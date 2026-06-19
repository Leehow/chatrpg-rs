# Design-to-Task Compiler for chatrpg-rs

This document defines how Codex Team Lead converts a high-level design into task cards that actually complete the design.

## Why this exists

The failure mode to avoid:

```text
Design says: build Knowledge / Memory Runtime.
Task card says: add KnowledgeEdge struct and tests.
Worker finishes card.
Lead reports Done.
But NoSpoiler, NPC knowledge, prompt projection, verifier, runtime capability, and golden scenarios are still missing.
```

The fix is to compile design into acceptance rows first, then tasks. Tasks are implementation vehicles. The ledger is the source of truth.

## Step 1 — Extract Design Claims

Read the approved design and extract claims into these buckets:

1. Product outcome: what behavior the user should experience.
2. Runtime invariant: what must never happen.
3. Data model: what durable concepts must exist.
4. Projection/view: who sees what.
5. Runtime integration: where in execute_turn / NeedBus / ContextCompiler / PluginHost it connects.
6. Verifier: what detects violation after LLM output.
7. Event/projection: what state transition proves the behavior.
8. Golden scenario: what real TRPG fixture demonstrates it.

Do not start by creating implementation tasks.

## Step 2 — Write Design Acceptance Rows

Use IDs like `DA-KNOW-01`. Each row must be observable.

Bad row:

```text
Implement KnowledgeEdge.
```

Good row:

```text
DA-KNOW-01: The runtime can represent the same WorldFact as known_true by GM, unknown by player_party, believes_false by npc_a, and known_true by npc_b; each viewer projection returns only that holder's truth/belief state.
```

## Step 3 — Assign Evidence Level

Every design row gets required evidence:

```text
V0 static guard
V1 deterministic
V2 build/contract
V3 runtime journey
V4 adversarial/regression
V5 golden TRPG scenario
```

Use V3+ for behavior visible in a turn. Use V4 for safety boundaries. Use V5 for parser/playability/story fixture claims.

## Step 4 — Build Dependency Graph

For each row, record:

```text
requires:
  - DA-ACTOR-01 stable actor identity
  - DA-KNOW-01 holder-specific knowledge
unblocks:
  - DA-NPC-03 NPC speech uses only NpcMindView
```

A task card may implement prerequisites, but the lead must not mark downstream design rows Done until downstream evidence passes.

## Step 5 — Generate Task Cards as Vertical Slices

Prefer vertical slices:

```text
model/storage + runtime projection + prompt/context integration + verifier/golden test
```

Only use horizontal foundation tasks when unavoidable. If a task is foundation-only, mark the design rows as `Partial`, not `Done`.

## Step 6 — Make Acceptance Rows Mechanical

Every task card must include:

```text
Design acceptance IDs affected
Required evidence level
Commands/fixtures
Done / Partial / Missing mapping
```

The worker handoff must tell the lead exactly which rows can be updated.

## Step 7 — Lead Does Coverage Audit After Every Accepted Task

After accepting a task, the lead updates `AcceptanceLedger.md`:

```text
DA-KNOW-01 Done — evidence: test X, file Y, handoff Z
DA-SPOIL-02 Partial — context filter exists, verifier missing
DA-NPC-03 Missing — not started
```

Then the lead chooses the next unblocked row. Do not ask the user for routine continuation.

## Completion Rule

The epic is complete only when the ledger says all approved rows are terminal:

```text
Done / Deferred / BlockedByHuman / OutOfScope
```

`Partial`, `Missing`, and `Untested` are not terminal.

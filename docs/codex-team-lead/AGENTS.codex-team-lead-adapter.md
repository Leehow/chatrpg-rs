# Codex Team Lead Adapter — chatrpg-rs

Use this adapter together with the generic Codex Team Lead Mode skill.
This file is project-specific. It defines how to convert chatrpg-rs design docs into executable task cards and how to judge whether work actually satisfies the design.

## Project Constitution

chatrpg-rs is a Source-grounded JSON Asset Runtime for solo LLM-GM TRPG play.

Non-negotiable rules:

1. Do not build a universal TRPG rule compiler.
2. Parse rules, modules, characters, scenes, clues, NPCs, and memories into source-backed JSON assets.
3. Rust runtime executes confirmed capabilities. LLM agents and plugins propose.
4. BindingResolver decides Exact / Partial / Guided / SourceOnly / Unsupported.
5. Unknown or unverified rules fail closed or enter source-backed guided ruling.
6. Player-facing narration must not leak player-unknown facts.
7. NPCs may speak and act only from their own knowledge, beliefs, persona, relationship state, and current behavior plan.
8. State changes must be recorded as DomainEvents or validated projection updates.
9. API and CLI turn execution must remain unified through the canonical turn executor.
10. SSE must stay truly streaming; heavy postprocess must not block player-visible completion.
11. BP1/BP2/BP3 cache semantics must remain stable.
12. Engine/runtime crates must not hardcode ruleset names, module names, or sample NPC names.

## When User Says “run the design from start to finish”

Use Codex Team Lead Mode autonomous goal loop, not one-task checkpoint mode.

The lead must:

1. Convert the approved design into a Design Acceptance Ledger.
2. Convert ledger rows into task cards.
3. Dispatch workers for implementation slices.
4. Review handoff and diff.
5. Run validation mapped to every acceptance row.
6. Update ledger.
7. Continue to the next unblocked row until the whole approved scope is Done, truly Blocked, or the stop limit is reached.

A single task card being Done does not mean the design is Done.

## Design Completion Rule

Do not report “complete” unless every design acceptance row is one of:

- Done, with evidence;
- Deferred, with explicit user-approved reason;
- BlockedByHuman, with exact blocker;
- OutOfScope, because the approved epic explicitly excluded it.

Partial implementation, type skeletons, storage-only work, compile-only work, or prompt-only work cannot close a design acceptance row unless that row was explicitly only a foundation row.

## Required Project Evidence Vocabulary

Use the generic Team Lead validation matrix V0-V4, and add project-specific V5.

- V0 Static guard: grep, boundary scan, no-hardcode, no direct state commit by LLM/plugin.
- V1 Deterministic check: unit/golden/fake-provider/state-machine tests.
- V2 Build/contract: cargo check/test, DB migration roundtrip, API/CLI schema surface.
- V3 Real runtime journey: execute_turn/API/CLI/harness flow with observable TurnEvents/state/projections.
- V4 Adversarial/regression: spoiler leak attempt, stale state, unknown rule, wrong NPC knowledge, retry/idempotency.
- V5 Golden TRPG scenario: Homecoming, Triangle, BRP/CoC, or Sword World pressure fixture matching the feature.

For player-visible behavior, V3 is mandatory unless explicitly deferred.
For spoiler, NPC knowledge, or stateful memory behavior, V4 is mandatory.
For parser/playability behavior, V5 is mandatory.

## Task Card Done Rule

A task card is accepted only when:

1. All card acceptance rows have evidence.
2. The worker names every design acceptance row affected.
3. The lead can update the ledger mechanically from the handoff.
4. No parent design row is silently marked Done from a foundation-only task.
5. Any remaining design gap is explicitly listed as Missing / Partial / Deferred / Blocked.

## Required Lead Files for Long Design Work

Use this structure:

```text
docs/epics/<epic_id>/
  Prompt.md            # approved goal, non-goals, final product shape
  Plan.md              # milestones, dependencies, acceptance rows, validation mapping
  Implement.md         # worker lanes, dispatch sequencing, repair loop
  AcceptanceLedger.md  # design acceptance ledger, source of truth for Done/Partial/Missing
  Documentation.md     # running evidence, decisions, risks, current status
```

For smaller work, use a single `docs/active-plans/<work_id>.md` ledger, but still keep acceptance rows.

## Worker Dispatch Requirements

Every worker prompt must include:

- `task_id`
- `mode`
- `model_tier`
- `scope_own`
- `scope_off`
- `risk_budget`
- `design_acceptance_ids`
- `required_validation_levels`
- `handoff`
- `escalation`
- explicit instruction that the worker may not declare the whole design Done unless the assigned ledger rows are Done.

## Lead Review Requirements

Before accepting a worker:

1. Read the full handoff, not just terminal status.
2. Inspect diff and touched files.
3. Check design_acceptance_ids against AcceptanceLedger.
4. Run or verify required V-level evidence.
5. Mark rows Done / Partial / Missing / Deferred / Blocked.
6. If a row is Partial and has a safe next task, dispatch a revision or next worker without asking the user.


## Journey-driven acceptance hard gate

For TRPG gameplay work, component/unit/DB tests are diagnostic only. Before a design row is Done, require a connected journey from public character creation and session start through natural player action, feature trigger, runtime mechanism evidence, player-visible outcome, committed state, and later-turn/reload consumption when applicable. Use `docs/codex-team-lead/05-connected-journey-loop.md`. No trigger means no pass; no valid character means invalid setup. Deterministic and live-model validation must run the same scenario contract, and accepted live runs should be promoted to replay cassettes.

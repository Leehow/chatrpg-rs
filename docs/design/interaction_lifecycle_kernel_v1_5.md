# v1.5 Interaction Lifecycle Kernel

## Purpose

v1.5 hardens the lifecycle of mutable interaction state. Earlier versions fixed individual bugs, but the same class kept returning: pending checks leaked, direction gates wedged, and closed frames left open child gates that hijacked later turns. v1.5 introduces a Rust-owned lifecycle kernel so StateFrame, InteractionGate, reaction windows, and PendingCheck state have one owner and one reducer-like transition path.

## Core idea

`InteractionLifecycleKernel` is the aggregate lifecycle owner for active table interaction. It does not replace WorldTime, CombatAgent, Director, or CheckContract. It enforces who owns whom and what must be closed when a parent closes.

```text
WorldTimeSpine
  → event ordering and cache watermark

InteractionLifecycleKernel
  → ownership, generation, cascade close, reconcile, invariants
```

## New objects

- `InteractionContext`
- `InteractionEvent`
- `InvariantRepair`
- `InteractionTransitionResult`
- `SupersededReason`

Existing `InteractionGate` and `PendingCheck` now carry:

- `interaction_context_id`
- `owner_frame_id`
- `generation`
- `superseded_reason`
- `closed_at_tick`

## Turn lifecycle

```text
1. Ensure world time.
2. Reconcile interaction state.
3. Load only valid open gate/check for current generation.
4. If player intent supersedes gate, close the gate/check and continue routing.
5. Route active frame / director / checks.
6. If a frame closes, cascade-close all child gates/checks.
7. Bump session interaction_generation.
8. Write interaction/world events.
```

## Invariants

- No open gate without an active owner frame/context.
- No open pending check with a closed owner frame.
- A closed frame implies all child gates/checks are terminal.
- A stale generation cannot capture player input.
- Terminal player intent may supersede a required reaction gate.

## Self-healing

Every turn calls `reconcile_session`. If stale or orphaned rows exist, the kernel supersedes them and records `InvariantRepair` and `InteractionEvent` rows instead of letting them block the player.

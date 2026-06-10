# v1.7 Turn Orchestration Kernel

## Purpose

v1.5 introduced an Interaction Lifecycle Kernel, which fixes stale gate cleanup and frame-close cascade behavior. v1.6 introduced the Object & Possession Kernel. Testing showed that the runtime still allowed multiple subsystems to compete for the same player input: open gates, object interactions, combat frame starts, director guidance, and generic checks could each claim a turn. v1.7 adds a single reducer-level entry point before those subsystems.

## Core invariant

A player input must be routed once before it is interpreted by any subsystem.

```text
TurnInput
  -> WorldTime current
  -> InteractionLifecycle reconcile
  -> TurnIntent classification
  -> GateRelation classification
  -> TurnRoute
  -> subsystem execution
```

## Gate policy

Open gates are not global locks. A gate only runs first if the input is a direct roll or choice reply. If the input expresses a new frame-level intent, a terminal intent, or object interaction that supersedes the gate, the orchestrator supersedes the gate and continues routing the new intent.

## Frame policy

Combat-starting and combat-continuing actions must route through the frame reducer before any generic standalone check. This prevents re-entry failures where a clear input such as “另一台无人机堵住我，我又开枪打它” becomes only a pending check with no combat frame.

## Object policy

Object interactions inside active frames are child actions of that frame. The orchestrator lets the frame reducer see the turn first, then falls back to the ObjectService when the frame reducer does not fully handle the object action.

## Runtime order

```text
orchestrator
  -> resolve gate only if route says resolve_gate_first
  -> compile context
  -> conflict first if route says frame-first
  -> object first if route says object-first
  -> director
  -> generic agent only if route allows it
  -> LLM narration
```

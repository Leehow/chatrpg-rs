# Team Lead Parallel Pipeline Optimization Design

- Date: 2026-06-18
- Worktree: `/Users/haoli/.config/superpowers/worktrees/chatrpg-rs-v1.20-formula/knowledge-runtime-p0`
- Branch: `codex/knowledge-runtime-p0`
- Status: approved by current user request

## Problem

The Knowledge / Memory / NPC Runtime epic already has strong Journey
acceptance rules and an AcceptanceLedger, but the execution protocol is still
too serial:

- task selection is described as "next unblocked task" rather than a
  conflict-free ready set;
- worker prompts describe path scope, but not `read_set`, `write_set`,
  hotspot leases, base SHA, worktree, branch, or scoped commit policy;
- many worker outputs can accumulate in one integration worktree before
  trial integration;
- there is no lead-owned WorkGraph that records dependency, conflict, lease,
  worker, and integration state.

This lets the team start many workers but still bottleneck on shared files,
manual merge sequencing, and stale integration state.

## Goal

Keep the existing Journey V3 acceptance system as the source of truth for
Done, and add a conflict-aware scheduling layer that makes parallel work
mergeable:

```text
AcceptanceLedger decides what must be proven.
WorkGraph decides what may run in parallel.
Rolling integration decides what enters the green baseline.
Journey evidence decides what is Done.
```

## Non-Goals

- Do not replace the existing `[TEAM_LEAD_WORKER_V1]` marker yet.
- Do not change Rust implementation code in this optimization.
- Do not push, deploy, rewrite history, or commit without explicit permission.
- Do not mark current dirty worker output accepted merely because it exists.

## Design

### 1. Project Adapter

`AGENTS.md` should declare the parallel pipeline as an optional escalation for
approved multi-task implementation epics. It should preserve the existing V1
worker marker and add scheduling fields to the marker rather than switching to
an incompatible V2 marker.

### 2. Lead-Owned WorkGraph

Each non-trivial epic gets:

- `WorkGraph.yaml`
- `IntegrationReport.md`

Workers may read their task row but must not edit these files. The lead owns
state transitions, lease assignment, and integration event recording.

### 3. Conflict-Aware Scheduler

In `continuous_epic`, the lead should repeatedly:

1. read AcceptanceLedger, WorkGraph, and completed handoffs;
2. review finished lanes;
3. trial-integrate accepted commits immediately;
4. advance the integration branch only after validation;
5. recompute the ready queue;
6. dispatch the maximal conflict-free ready set within WIP limits.

A task is runnable only when dependencies are integrated, write sets do not
overlap active writers, required hotspot leases are free, and the task has
explicit acceptance IDs and validation profile.

### 4. Scoped Worker Commits

Code-affecting parallel workers should run in isolated worktrees and produce
one scoped local commit plus a handoff. Workers do not push or merge. Shared
hotspot edits are either owned by a lease holder or proposed in the handoff for
serial application.

### 5. Current Epic Recovery

The current `codex/knowledge-runtime-p0` worktree already has substantial dirty
integration debt. Before dispatching new implementation lanes, the lead should
create a recovery task row that classifies dirty outputs by handoff/task,
accepts only reviewed slices, and rolls them into integration one at a time.

## Success Criteria

- The repo contains the v4 parallel pipeline skill/templates.
- `AGENTS.md`, `CLAUDE.md`, autonomous-loop instructions, and dispatch docs
  describe WorkGraph scheduling and V1 marker extensions.
- The Knowledge epic has initial `WorkGraph.yaml` and `IntegrationReport.md`.
- The current dirty integration state is represented as an explicit recovery
  lane rather than an invisible pile of work.
- Documentation validation confirms the new terms are discoverable and no
  legacy unsafe commands were introduced.

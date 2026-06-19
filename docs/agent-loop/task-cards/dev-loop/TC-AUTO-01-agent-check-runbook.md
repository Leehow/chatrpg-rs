---
id: TC-AUTO-01-agent-check-runbook
title: Define agent-check runbook and validation presets
mode: documentation
type: process
priority: P1
risk: low
---

# TC-AUTO-01 — Define agent-check runbook and validation presets

## Objective

Create a documented validation preset system so workers know which checks to run for each task type.

## Non-goals

- Do not implement a full central test runner unless explicitly scoped.
- Do not invent tests that cannot run in the current repo.

## Scope owned

- `docs/agent-loop/03-test-matrix.md`
- `docs/agent-loop/checklists/**`
- Optional `scripts/agent_check.*` documentation only if explicitly approved.

## Scope off

- Backend source.
- Frontend source.
- Package/dependency changes.

## Acceptance criteria

- Validation presets exist for docs, backend, frontend, API contracts, parser, knowledge, spoiler, NPC, plugin, runtime, and browser-visible flows.
- Each preset says what to run, what counts as pass, and what to report if blocked.
- Workers can select a preset without asking the human.

## Required tests

- Manual review of commands for current repo compatibility.
- Links and paths exist or are explicitly marked future/Rust-workspace-specific.

## Done when

- The runbook can be copied into a worker dispatch as validation instructions.

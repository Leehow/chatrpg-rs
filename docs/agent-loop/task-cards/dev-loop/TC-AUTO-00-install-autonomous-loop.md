---
id: TC-AUTO-00-install-autonomous-loop
title: Install autonomous loop docs and skill bridge
mode: documentation
owner: codex-lead
priority: P0
risk: low
expected_backend: tty
subagent_policy: research_only
observability: full
repair_budget: 1
---

# TC-AUTO-00 — Install autonomous loop docs and skill bridge

## Objective

Install the autonomous Team Lead Loop documentation and skill bridge into the repository.

## Non-goals

- Do not edit application code.
- Do not change runtime behavior.
- Do not alter the existing Team Lead Mode hard gate.

## Scope owned

- `.agents/skills/chatrpg-autonomous-loop/**`
- `docs/agent-loop/**`
- Optional one-line references in `.agents/skills/chatrpg-team-lead/SKILL.md` and `AGENTS.md`

## Scope off

- Backend source.
- Frontend source.
- Migrations.
- Generated contracts.

## Acceptance criteria

- The new skill exists and is discoverable.
- Existing team lead skill remains authoritative.
- Docs explain No-Ask policy, repair loop, test matrix, and final report format.
- Task-card templates are present.

## Required tests

- Markdown files are readable.
- Paths referenced by docs exist.
- No code validation is required.

## Done when

- Files are installed.
- Cross-links point to existing files.
- Final report lists installed paths and no code changes.

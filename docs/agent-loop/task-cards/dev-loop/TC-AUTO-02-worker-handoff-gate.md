---
id: TC-AUTO-02-worker-handoff-gate
title: Tighten worker handoff acceptance gate
mode: documentation
priority: P1
risk: low
---

# TC-AUTO-02 — Tighten worker handoff acceptance gate

## Objective

Make worker handoffs mechanically reviewable by the lead.

## Scope owned

- `.agents/skills/chatrpg-autonomous-loop/references/review-checklist.md`
- Optional additions to `.agents/skills/chatrpg-worker/SKILL.md` if explicitly dispatched.

## Scope off

- Runtime code.
- Tests.
- Migrations.

## Acceptance criteria

A worker handoff must include:

- task restatement;
- assumptions;
- files changed;
- exact validation commands and outcomes;
- self-repair attempts;
- acceptance criteria ledger;
- open questions;
- risk and blocker list;
- plan ledger note when work_id exists.

## Done when

- The checklist lets the lead reject false-Done handoffs without re-reading the whole transcript.

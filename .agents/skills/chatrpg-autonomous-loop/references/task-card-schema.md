# Task Card Schema

Use this schema for autonomous task cards.

```yaml
id: TC-AREA-NN-short-name
title: Human readable task title
mode: implementation | test_design | adversarial_review | verification | documentation
owner: claude-worker | codex-lead
priority: P0 | P1 | P2
risk: low | medium | high
expected_backend: tty | cc-background | cc-agent-view | cc-internal-subagents
subagent_policy: research_only | implementation_allowed
observability: full | final_only
repair_budget: 3
```

## Required sections

1. Objective
2. Why this matters
3. Non-goals
4. Scope owned
5. Scope off
6. Architecture constraints
7. Implementation guidance
8. Acceptance criteria
9. Required tests
10. Validation commands
11. Escalation triggers
12. Done when
13. Handoff requirements

## Status terms

- Done: implemented and validated.
- Partial: implemented but missing a named criterion or validation.
- Missing: not implemented.
- Deferred: intentionally postponed with reason.
- Blocked: cannot proceed under current constraints.
- Untested: implemented but not validated.

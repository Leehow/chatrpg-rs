# Dispatch Prompt Template

Paste this into a Claude Code worker after starting `claude --model opus` from the repository root.

```text
[TEAM_LEAD_WORKER_V1]
task_id: <task_id>
work_id: <optional_work_id>
mode: <implementation|test_design|adversarial_review|verification|documentation>
backend: <tty|cc-background|cc-agent-view|cc-internal-subagents>
subagent_policy: <research_only|implementation_allowed>
observability: <full|final_only>
autonomy: enabled
repair_budget: 3
validation_gate: required
assumption_policy: document_and_continue
question_policy: hard_blockers_only
scope_own:
  - <path or crate/module>
scope_off:
  - <path or behavior not to touch>
risk_budget: <low|medium|high; no destructive git; no deploy; no broad migrations unless listed>
handoff: .tmp/team-lead/worker-<task_id>-<timestamp>.md
acceptance_source: <docs/agent-loop/task-cards/...md>

Context:
- Read AGENTS.md and CLAUDE.md.
- Read .agents/skills/chatrpg-worker/SKILL.md.
- Read .agents/skills/chatrpg-autonomous-loop/references/worker-autonomy-contract.md.
- Read the task card at <path>.

Task:
<one paragraph task summary>

Autonomy:
Do not ask routine implementation questions. Use the task card, repository rules, existing patterns, and the Project Constitution. Make conservative reversible decisions, implement, test, repair failures up to the repair budget, and document assumptions in the handoff.

Validation:
Run the required checks from the task card. If a command cannot run, state the exact reason and provide the smallest alternative evidence.

Handoff:
Use the required handoff sections from .agents/skills/chatrpg-worker/SKILL.md plus:
- Assumptions made without asking
- Self-repair loops performed
- Acceptance criteria ledger
- Validation matrix result
```

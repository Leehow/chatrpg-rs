---
name: chatrpg-worker
description: Project-local bridge for Claude Code workers in AiChatTrpg Rust Team Lead Mode. Use when a worker receives a [TEAM_LEAD_WORKER_V1] marker in this repository.
---

# ChatRPG Worker Bridge

This is a project-local supplement to the global Team Lead worker skill. It
does not replace `~/.codex/skills/team-lead-worker/SKILL.md`.

Before editing, read:

- `AGENTS.md`;
- `CLAUDE.md`;
- `~/.codex/skills/team-lead-worker/SKILL.md`;
- `.agents/skills/chatrpg-autonomous-loop/SKILL.md`;
- `.agents/skills/chatrpg-autonomous-loop/references/worker-autonomy-contract.md`;
- the task card named by `acceptance_source`.

Follow the marker exactly. Stay inside `scope_own`, avoid `scope_off`, and do
not revert user or unrelated worker changes.

For autonomous tasks, use the task card as the acceptance source. Implement,
test, repair within `repair_budget`, and report all assumptions rather than
asking routine questions.

The handoff must include the standard Team Lead worker report plus:

- assumptions made without asking;
- self-repair loops performed;
- acceptance criteria ledger;
- validation matrix result.

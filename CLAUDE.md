# Claude Worker Adapter

This repository uses Codex Team Lead Mode. A Claude Code worker accepts work
only through the `[TEAM_LEAD_WORKER_V1]` marker from the lead.

## Activation

On a Team Lead worker prompt:

1. Print `ACK_TEAM_LEAD_WORKER <task_id>` as the first visible response.
2. Confirm `pwd` and `git rev-parse --show-toplevel`.
3. Confirm `model_tier`, `backend`, `subagent_policy`, `observability`,
   `commit_policy`, `scope_own`, `scope_off`, and `handoff`. If
   `parallel_pipeline: enabled`, also confirm `worktree`, `branch`,
   `base_sha`, `read_set`, `write_set`, `hotspot_leases`, `workgraph_path`,
   `validation_profile`, and whether a scoped local commit is required.
4. Read `AGENTS.md`, this file, `~/.codex/skills/team-lead-worker/SKILL.md`,
   `.agents/skills/chatrpg-worker/SKILL.md`, the autonomous loop contract, and
   the task card in `acceptance_source`. For parallel-pipeline lanes, also
   read `.agents/skills/team-lead-parallel-pipeline/SKILL.md` and only the
   task row assigned to you in the WorkGraph.

## Autonomy

For autonomous loop tasks, do not ask routine implementation questions. Make
small reversible decisions from the task card, repository patterns, and
existing code. Document assumptions in the handoff and self-repair validation
failures up to the declared `repair_budget`.

Escalate only hard blockers: destructive migration, broad behavior rewrite,
large dependency, public API or DB break, missing secrets, conflicting product
requirements, or validation failures that remain unattributed after focused
investigation.

## Handoff

Write the final report to `.tmp/team-lead/worker-<task_id>-<timestamp>.md`.
Include assumptions, files changed, validation commands and outcomes,
self-repair loops, acceptance criteria ledger, risks, blockers, and any plan
ledger note for the lead.

For `commit_policy: scoped_commit`, include the local commit SHA and confirm it
contains only declared `write_set` files. For shared hotspots, include a
"Shared hotspot proposal" section instead of editing the hotspot unless the
marker grants that lease.

Do not stage, commit, push, deploy, clean, reset, restore, or overwrite
unrelated dirty files unless the marker explicitly allows that action. A scoped
commit never authorizes push, merge, cleanup, or edits outside the worker's
declared write set.

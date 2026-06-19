# Worker Dispatch Template for Codex Team Lead Mode

Use this as the base prompt when dispatching Claude Code workers from Codex Team Lead Mode.

```text
[TEAM_LEAD_WORKER_V1]
task_id: <task_id>
mode: implementation | revision | verification | adversarial_review | test_design
model_tier: opus | sonnet
backend: cc-print | tty | cc-background | cc-agent-view | cc-internal-subagents
observability: final_only | full
subagent_policy: research_only | implementation_allowed
parallel_pipeline: disabled | enabled
work_id: <work_id>
epic_id: <epic_id>
workgraph_path: docs/epics/<epic_id>/WorkGraph.yaml
integration_report: docs/epics/<epic_id>/IntegrationReport.md
worktree: <absolute-worktree-path or shared-readonly-checkout>
branch: claude/<work_id>/<task_id>
base_sha: <integration-head-sha>
commit_policy: no_commit | scoped_commit
scope_own:
  - <paths or crates the worker may edit>
scope_off:
  - <paths or crates the worker must not edit>
read_set:
  - <paths/contracts the worker may inspect>
write_set:
  - <paths the worker may modify>
hotspot_leases:
  - <lease ids or none>
generated_outputs:
  - <generated paths or none>
migration_slot: none | <assigned migration id>
validation_profile: <profile-name>
risk_budget: no_destructive_git, no_deploy, no_large_dependency, no_public_api_break_without_escalation
escalation: .tmp/team-lead/questions-<task_id>.md
handoff: .tmp/team-lead/worker-<task_id>.md

Context:
- Read the project rules and the Codex Team Lead adapter.
- Read docs/epics/<epic_id>/Prompt.md, Plan.md, Implement.md, and AcceptanceLedger.md.
- If parallel_pipeline is enabled, read .agents/skills/team-lead-parallel-pipeline/SKILL.md and your task row in WorkGraph.yaml.
- This task is part of an approved autonomous goal loop. Do not ask the user routine design questions.

Assigned design acceptance IDs:
- <DA-ID-1>
- <DA-ID-2>

Objective:
<observable objective>

Required behavior:
1. <behavior>
2. <behavior>

Required validation:
- V0: <static guard>
- V1: <unit/deterministic test>
- V2: <build/db/contract>
- V3: <runtime journey if required>
- V4: <adversarial/regression if required>
- V5: <golden scenario if required>

Autonomy:
- Make routine implementation decisions yourself.
- Use the smallest reversible design consistent with the project constitution.
- If a test fails, diagnose and repair within scope before handing off.
- Escalate only for scope conflict, destructive action, secrets/deploy/legal/security, unavailable tools/model/backend, or ambiguous failed validation.
- If `commit_policy: scoped_commit`, create exactly one local commit containing only `write_set` changes and report its SHA. Do not push or merge.
- If a required shared hotspot change is not covered by your lease, write a "Shared hotspot proposal" in the handoff instead of editing it.

Important:
- Do not mark the whole design complete.
- In your handoff, state exactly which assigned design acceptance rows are Done / Partial / Missing / Blocked and why.
- In your handoff, state whether your write_set, hotspot leases, base SHA, worktree, branch, and commit policy were honored.
- If the implementation is only a foundation layer, say so explicitly and leave downstream design rows Partial or Missing.
```

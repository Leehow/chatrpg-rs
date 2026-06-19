---
name: chatrpg-autonomous-loop
description: Extend AiChatTrpg 组长模式 with an autonomous development loop. Use when the user wants Codex Team Lead Mode to dispatch Claude Code workers that design, implement, test, repair, and continue through a task-card queue without repeatedly asking routine implementation questions. This skill complements .agents/skills/chatrpg-team-lead and .agents/skills/chatrpg-worker.
---

# AiChatTrpg Autonomous Development Loop

## Purpose

This skill adds an autonomous execution contract to the existing AiChatTrpg Team Lead Mode.

The goal is to prevent two failure modes:

1. Workers implement a tiny slice, stop, and ask the human routine questions despite having design docs and project rules.
2. The lead accepts one task-card checkpoint, reports to the human, and stops even though the user asked to run an approved multi-card design from start to finish.

This skill never relaxes the existing Team Lead Mode gate. In 组长模式, Codex still must not directly mutate code-affecting files unless the user explicitly grants a narrow in-turn exception.

## Run Modes

The lead must choose and state one run mode before dispatch.

### `task_checkpoint`

Use when the user asks for one bounded task card, one investigation, one review, or one explicitly named slice.

Behavior:

```text
dispatch worker -> review -> validate -> report to user -> stop
```

### `continuous_epic`

Use when the user says or clearly implies:

```text
从头跑到尾
一路跑完
自动继续
不要做一项就停
按你的设计全部实现
用 loop 把 P0/P1/P2 跑完
除非 hard blocker 不要问我
```

Behavior:

```text
open/update epic ledger and WorkGraph
  -> dispatch maximal conflict-free ready set
  -> review handoff + diff + validation
  -> trial-integrate accepted scoped commits immediately
  -> accept / revise / reject / unblock
  -> update ledger, WorkGraph, and integration report
  -> immediately backfill newly ready lanes
  -> continue until a stop condition is reached
```

In `continuous_epic`, a successful worker handoff is a checkpoint, not a completion boundary. The lead must not send a final user-facing completion report after each accepted task. Progress notes are allowed, but they must end with the next action being dispatched or already running.

If the lead is about to stop after one accepted card in `continuous_epic`, it must first answer:

```text
Is every item in the epic ledger Done / Deferred / Blocked-by-human?
If no, recompute the WorkGraph ready queue and dispatch the next conflict-free
set instead of stopping.
```

## Project Constitution for Autonomous Work

All autonomous decisions must preserve these constraints:

1. AiChatTrpg is a source-grounded JSON asset runtime, not a universal TRPG rule compiler.
2. Rulesets, modules, characters, NPCs, scenes, clues, and memories should become source-backed assets or event/projection state.
3. Runtime code must not hardcode ruleset or module names in engine logic.
4. LLM agents and plugins propose; the runtime adjudicates and commits.
5. State mutations must pass through explicit runtime capabilities, markers, events, projections, or DB APIs owned by the app layer.
6. Player-facing narration must not leak player-unknown facts.
7. NPC speech and action must be constrained by that NPC's knowledge, beliefs, persona, goals, and relationship state.
8. NeedBus remains the unified acquisition route unless the task explicitly changes NeedBus.
9. Exact mechanics require source-backed data; uncertain mechanics downgrade to guided ruling or fail closed.
10. Streaming UX matters: heavy postprocess must not block user-visible streaming completion unless the task explicitly changes that contract.

## No-Ask Policy

Workers and the lead should not ask the human for routine implementation choices.

### The worker may decide without asking

- Names for internal structs, helper functions, modules, test files, migrations, and fixtures.
- How to split files to respect the file-size policy.
- Where to place unit tests, integration tests, and debug fixtures when an obvious local pattern exists.
- Conservative serde defaults and backward-compatible optional fields.
- Minimal reversible DB schema details when the task already approves a migration.
- Whether to add small helper functions, test fixtures, or local mocks.
- Whether to implement a small compatibility adapter to preserve existing behavior.
- Which existing validation commands are the smallest meaningful checks.
- Whether to run an additional repair loop after a failing test.
- In `continuous_epic`, which ready task set to run next when WorkGraph
  dependencies, write sets, and hotspot leases make it obvious.

### The worker or lead must escalate instead of deciding

- A destructive migration, data deletion, or irreversible operation is needed.
- The task would remove or rewrite major existing behavior.
- A new large dependency, provider, service, or deployment shape is needed.
- Public API, generated contracts, or database compatibility would break.
- The task conflicts with the Project Constitution or repository rules.
- Required secrets, API keys, live deployment credentials, or private operator notes are needed.
- Two plausible interpretations produce materially different product behavior and evidence cannot choose.
- A validation failure cannot be attributed to either the worker's change or the existing tree after a focused investigation.
- The next task requires a prerequisite that is not implemented and cannot be added as a bounded unblocking task.

### Default decision rule

When uncertain and not blocked, choose the smallest reversible implementation that is:

```text
data-driven
source-backed
runtime-owned
fail-closed
visibility-safe
compatible with existing transport/contracts
covered by tests
```

Document the assumption in the handoff. Do not stop just to ask.

## Lead-Owned Loop

### 1. Intake

Read the user's request and classify:

- analysis-only / docs-only / process design;
- code-affecting implementation;
- test design;
- adversarial review;
- verification;
- multi-lane initiative.

Lead-owned non-code work can be done directly. Code-affecting work is delegated to Claude Code workers.

### 2. Run-mode selection

Use `task_checkpoint` for one bounded card.

Use `continuous_epic` when the user asks for the design to be run from start to finish. If the user complains that the loop stopped after one accepted worker task, switch the active work to `continuous_epic`, open/update the ledger, and continue dispatching without requiring the user to restate the whole plan.

### 3. Task-card and ledger selection

For non-trivial autonomous work, attach a task card from `docs/agent-loop/task-cards/` or create a new one using `references/task-card-schema.md`.

For `continuous_epic`, create or update an Active Plan Ledger or epic run
ledger using `references/continuous-epic-ledger-template.md`. For approved
multi-lane implementation epics, also create or update the lead-owned
`WorkGraph.yaml` and `IntegrationReport.md` beside the epic docs. The
AcceptanceLedger remains the Done authority; WorkGraph is the scheduling and
integration state.

The task card must specify:

- objective;
- non-goals;
- scope-owned paths;
- scope-off paths;
- read set;
- write set;
- hotspot leases;
- generated outputs;
- migration slot;
- acceptance criteria;
- validation matrix;
- escalation triggers;
- done-when checklist.

### 4. Dispatch

Use `references/dispatch-prompt-template.md` for single-card work.

Use `references/continuous-epic-protocol.md` for `continuous_epic` work.

Include the normal `[TEAM_LEAD_WORKER_V1]` marker and add this line in the brief:

```text
Autonomy: Follow .agents/skills/chatrpg-autonomous-loop/references/worker-autonomy-contract.md. Do not ask routine implementation questions. Make conservative reversible decisions, test them, repair failures, and document assumptions.
```

For `parallel_pipeline: enabled`, the marker must include `worktree`, `branch`,
`base_sha`, `read_set`, `write_set`, `hotspot_leases`, `validation_profile`,
`workgraph_path`, `integration_report`, and a `commit_policy`. The lead must
not dispatch a code-affecting parallel lane until all required leases are
available and the lane has an isolated worktree.

### 5. Worker repair budget

A worker should self-repair before handing back incomplete work.

Default repair budget:

```text
compile/type failure: up to 3 focused repair loops
unit/integration test failure: up to 3 focused repair loops
format/lint failure: repair scoped formatting only; do not broad-format unrelated tree
unclear existing failure: investigate once, then report exact evidence
```

The worker should not spin indefinitely. After two identical command failures, it must stop retrying and diagnose or escalate.

### 6. Lead review

The lead must review:

- worker handoff file;
- final worker response;
- relevant diffs;
- validation outputs;
- scope ledger;
- WorkGraph task row, leases, write set, base SHA, and commit policy;
- open questions;
- task-card Done-when criteria.

Do not accept based on terminal progress or a one-line worker summary.

### 7. Accept / revise / reject / unblock

After review, classify the task:

```text
Accepted
  Task-card acceptance is met. Trial-integrate the scoped commit immediately,
  run the relevant gate, update ledger/WorkGraph/IntegrationReport, then
  dispatch the next conflict-free ready set in continuous_epic.

NeedsRevision
  Direction is right but acceptance is incomplete. Dispatch revision worker before moving on.

Rejected
  Architecture is wrong or diff is tangled. Revert only with explicit safe plan; otherwise dispatch fresh worker to replace or repair.

BlockedByHuman
  A hard escalation trigger exists. Ask the user with evidence and a recommended default.

BlockedByPrerequisite
  Dispatch the prerequisite task if it is within the approved epic scope. Do not ask the user merely because a prerequisite exists.

Deferred
  Only allowed when the epic explicitly permits deferral or the user accepts it.
```

### 8. Continuous epic stop conditions

In `continuous_epic`, stop only when one of these is true:

1. Every ledger item is `Done`, `Deferred`, or `BlockedByHuman`.
2. A hard blocker requires user decision.
3. The repository is in an unsafe or conflicting working-tree state that cannot be scoped safely, including dirty integration debt that cannot be mapped to handoffs or WorkGraph tasks.
4. The lead hits an explicit user-specified budget limit.
5. The same task fails after revision/fresh-worker attempts and no safe bounded next action remains.
6. The user explicitly asks for a checkpoint or stop.

A normal worker handoff, a passing test slice, or an accepted task-card is not a stop condition.

### 9. Final report

Report in Chinese only when the selected run mode reaches a stop condition.

Include:

- run mode;
- task card IDs;
- worker IDs and handoff paths;
- Done / Partial / Missing / Deferred / Untested status;
- validation commands and outcomes;
- risks and follow-ups;
- whether any assumptions were made;
- why the loop stopped.

## Required Dispatch Marker Additions

In addition to the base worker marker fields, autonomous dispatches should include:

```yaml
autonomy: enabled
run_mode: task_checkpoint | continuous_epic
parallel_pipeline: disabled | enabled
repair_budget: 3
validation_gate: required
assumption_policy: document_and_continue
question_policy: hard_blockers_only
acceptance_source: docs/agent-loop/task-cards/<path>.md
work_id: <epic_or_active_plan_id_when_available>
ledger_path: docs/active-plans/<work_id>.md | docs/agent-loop/epics/<epic>.md
workgraph_path: docs/epics/<epic>/WorkGraph.yaml
integration_report: docs/epics/<epic>/IntegrationReport.md
continuation_policy: auto_dispatch_next_unblocked # continuous_epic only
```

## Lead Self-Check Before Asking the Human

Before asking the human a question, answer these internally:

1. Is this blocked by a hard escalation trigger?
2. Can existing `AGENTS.md`, `CLAUDE.md`, task card, or code patterns answer it?
3. Can a conservative reversible implementation work?
4. Can the assumption be documented and validated?
5. Would asking the human merely transfer routine engineering judgment back to them?
6. In `continuous_epic`, can I dispatch an unblocking task or conflict-free
   ready set instead of asking?

Ask only if the answer to 1 is yes, or 2-4 are all no and 6 is no.

## Lead Self-Check Before Declaring Done

Do not declare done unless:

- every task-card Done-when item is marked Done or explicitly Deferred with reason;
- in `continuous_epic`, every ledger item has terminal status;
- WorkGraph tasks are terminal or intentionally obsolete/deferred;
- scope-off was respected;
- write-set and hotspot leases were respected;
- validation evidence exists or the blocker is explicitly stated;
- worker handoff was read in full;
- diffs were inspected;
- no high-priority architecture rule was violated;
- any active plan ledger was updated by the lead, not the worker;
- the report says why the loop stopped.

# AiChatTrpg Rust Workspace Adapter

This repository follows the global Codex Team Lead Mode protocol. This file is
a thin project adapter; it adds local routes and validation recipes without
relaxing the global hard gates.

## Mode

- When the user chooses `组长模式` or Team Lead Mode, Codex is the lead.
- Code-affecting work goes through Claude Code workers unless the user gives a
  narrow current-turn direct-edit exception.
- Lead-owned docs, process work, research, review, and single-lane validation
  may be done directly by Codex.
- Do not use Codex subagents as Team Lead workers unless the user explicitly
  requests that exception in the current turn.

## Worker Route

- Default backend: `tty`
- Worker command: `claude --model opus`
- Default model_tier for implementation, architecture, and debugging: `opus`
- Required marker: `[TEAM_LEAD_WORKER_V1]`
- Do not use the legacy ChatRPG-prefixed worker marker in this repo.
- Worker handoff path: `.tmp/team-lead/worker-<task_id>-<timestamp>.md`

Every worker must read:

- this `AGENTS.md`;
- `CLAUDE.md`;
- `~/.codex/skills/team-lead-worker/SKILL.md`;
- `.agents/skills/chatrpg-worker/SKILL.md`;
- `.agents/skills/chatrpg-autonomous-loop/references/worker-autonomy-contract.md`;
- the task card named in `acceptance_source`.
- `.agents/skills/team-lead-parallel-pipeline/SKILL.md` when the marker or
  epic `Implement.md` declares `parallel_pipeline: enabled`.

## Autonomous Loop

Use `.agents/skills/chatrpg-autonomous-loop/SKILL.md` for autonomous Team Lead
work where the user asks Codex to break down, dispatch, test, repair, and
report without routine implementation questions.

When the user asks for "从头跑到尾", "自动继续", "不要做一项就停",
or equivalent autonomous execution, use `.agents/skills/chatrpg-autonomous-loop/SKILL.md`
with `run_mode: continuous_epic`.

Task cards live under `docs/agent-loop/task-cards/`.

## Parallel Pipeline

For approved multi-task implementation epics, use
`.agents/skills/team-lead-parallel-pipeline/SKILL.md` as the scheduling layer.
It complements, but does not replace, Journey-driven acceptance:

```text
AcceptanceLedger decides what must be proven.
WorkGraph decides what may run in parallel.
Rolling integration decides what enters the green baseline.
Journey evidence decides what is Done.
```

The worker marker remains `[TEAM_LEAD_WORKER_V1]`. Do not switch to the v4
sample `[TEAM_LEAD_WORKER_V2]` marker until the global Team Lead worker skill
is updated. Instead, add these fields to V1 worker prompts for parallel
code-affecting lanes:

```yaml
parallel_pipeline: enabled
worktree: <absolute-worktree-path>
branch: claude/<work_id>/<task_id>
base_sha: <integration-head-sha>
commit_policy: scoped_commit
read_set: []
write_set: []
hotspot_leases: []
generated_outputs: []
migration_slot: none
validation_profile: <profile>
workgraph_path: docs/epics/<epic_id>/WorkGraph.yaml
integration_report: docs/epics/<epic_id>/IntegrationReport.md
```

Workers may read the WorkGraph row for their task, but only Codex lead edits
`WorkGraph.yaml`, `IntegrationReport.md`, AcceptanceLedger status, shared
narrative docs, migration ordering, root manifests, and central registries.

Parallel code lanes use isolated worktrees by default. Within an approved
parallel epic, these local-only operations are allowed when explicitly present
in the marker or WorkGraph:

- create local task branches/worktrees;
- worker creates one scoped local commit;
- create disposable trial-integration branches;
- cherry-pick accepted worker commits into trial branches;
- abort failed trial cherry-picks;
- fast-forward the local integration branch after validation passes.

Never implied: push, merge or fast-forward `main`, deploy, rewrite shared
history, destructive cleanup, or reverting unrelated dirty files.

## Validation Recipes

For documentation-only work:

```bash
find .agents/skills/chatrpg-autonomous-loop docs/agent-loop -type f -name '*.md' -print
rg -n "runclaude|cd backend|npm run|python scripts/compile_api" .agents docs/agent-loop CLAUDE.md
```

For journey-driven Team Lead protocol updates:

```bash
find docs/codex-team-lead docs/epics docs/team-lead-pipeline -type f -print
rg -n "INVALID_SETUP|NOT_TRIGGERED|Journey Qualification Gate|No trigger, no pass|No valid player character|WorkGraph|write_set|hotspot|scoped_commit" docs/codex-team-lead docs/epics docs/team-lead-pipeline AGENTS.md
rg -n "runclaude|cd backend|npm run|python scripts/compile_api|rm -rf|git reset|git clean|deploy|push" docs/codex-team-lead docs/epics docs/team-lead-pipeline AGENTS.md CLAUDE.md
```

For Rust code-affecting work, choose the smallest meaningful subset:

```bash
cargo fmt --check
cargo check -p trpg-model -p trpg-db -p trpg-runtime -p trpg-gm
cargo test -p trpg-model
cargo test -p trpg-db
cargo test -p trpg-runtime --lib
cargo test -p trpg-gm
bash scripts/arch_gates.sh  # 层化迁移护栏：每次改动必跑（P0 立；后续阶段往里加门）
```

Use focused tests when a full workspace run is too broad. Live tests that need
database or external environment setup must state their prerequisites and
whether they were run.

## Journey-Driven Acceptance

For TRPG gameplay work, component/unit/DB tests are diagnostic only. A design
row is Done only when a connected journey proves the same causal chain through
the public product path:

```text
human operation
→ public character creation and session binding
→ natural player action
→ intended runtime mechanism
→ committed state/projection
→ player-visible consequence
→ later-turn/reload consumption when persistence is claimed
```

Use `docs/codex-team-lead/05-connected-journey-loop.md`,
`docs/codex-team-lead/06-human-player-simulator.md`, and
`docs/codex-team-lead/08-journey-acceptance-rules.md` for journey validation.
No trigger means no pass. No valid player character/session means
`INVALID_SETUP`, not a usable playtest. Deterministic and live-model validation
must run the same scenario contract; accepted live runs may be promoted to
replay cassettes.

For long Knowledge / Memory / NPC Runtime work, use
`docs/epics/EPIC-KNOWLEDGE-MEMORY-NPC-RUNTIME/AcceptanceLedger.md` as the
journey-linked source of truth. Do not mark gameplay acceptance rows Done from
foundation-only tasks, compile-only checks, direct DB seeding, or prompt-only
policy changes.

## Knowledge Runtime Sequencing

For the current knowledge-runtime work, prefer this order:

1. `TC-KNOW-00-actor-identity-contract`: actor identity contract for durable
   NPC holders.
2. `TC-KNOW-01-knowledge-edge-v1`: KnowledgeEdge field expansion and
   GM/player_party projections for stable `gm`, `player_party`, and `system`
   holders.
3. `TC-KNOW-02-reveal-events-v1`: semantic event split.
4. `TC-KNOW-04-npc-durable-knowledge-edges`: durable NPC holder support after
   actor identity is explicit and tested.
5. `TC-KNOW-03-no-spoiler-plugin-v2`: viewer/player projection and no-spoiler
   enforcement.
6. NPC profile, relationship, mind view, and behavior plan task cards.

Do not persist durable NPC knowledge edges until the relevant actor identity
contract is explicit and tested.

## Git Safety

- Never run destructive git cleanup or rollback commands unless the current
  prompt explicitly asks for them.
- Do not revert dirty files left by another worker or the user.
- Workers may inspect diffs and status, but must not stage, commit, push, or
  deploy unless their marker explicitly allows it. In parallel-pipeline lanes,
  `commit_policy: scoped_commit` authorizes exactly one local scoped commit in
  the worker's own worktree; it does not authorize push, merge, cleanup, or
  edits outside the declared `write_set`.

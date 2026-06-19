# Team Lead Parallel Pipeline Optimization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add conflict-aware WorkGraph scheduling, isolated worker commit policy, and rolling integration instructions to the current ChatRPG Team Lead protocol.

**Architecture:** Preserve the current Journey V3 AcceptanceLedger as the Done authority. Add the v4 parallel pipeline as the scheduling and integration layer by installing its skill/templates, extending the existing V1 worker marker with worktree/write-set/lease fields, and creating initial epic-owned WorkGraph and IntegrationReport files.

**Tech Stack:** Markdown protocol docs, YAML WorkGraph, Git worktrees, Claude Code Team Lead workers, existing ChatRPG AcceptanceLedger/Journey V3 docs.

---

### Task 1: Install Parallel Pipeline Protocol Assets

**Files:**
- Create: `.agents/skills/team-lead-parallel-pipeline/SKILL.md`
- Create: `.agents/skills/team-lead-parallel-pipeline/references/*.md`
- Create: `docs/team-lead-pipeline/templates/*.md`
- Create: `docs/team-lead-pipeline/templates/workgraph.yaml`
- Create: `docs/team-lead-pipeline/project-adapter-chatrpg-rs.md`

- [ ] **Step 1: Copy the v4 assets**

Run:

```bash
rsync -a /Users/haoli/leehow/code/chatrpgv2/design/codex_team_lead_parallel_pipeline_v4/.agents/skills/team-lead-parallel-pipeline .agents/skills/
mkdir -p docs/team-lead-pipeline
rsync -a /Users/haoli/leehow/code/chatrpgv2/design/codex_team_lead_parallel_pipeline_v4/docs/team-lead-pipeline/ docs/team-lead-pipeline/
```

- [ ] **Step 2: Verify files exist**

Run:

```bash
find .agents/skills/team-lead-parallel-pipeline docs/team-lead-pipeline -type f | sort
```

Expected: `SKILL.md`, five reference files, templates, and the ChatRPG adapter are listed.

### Task 2: Patch Project Adapter and Worker Contract

**Files:**
- Modify: `AGENTS.md`
- Modify: `CLAUDE.md`
- Modify: `.agents/skills/chatrpg-autonomous-loop/SKILL.md`
- Modify: `docs/codex-team-lead/03-worker-dispatch-template.md`
- Modify: `docs/codex-team-lead/04-lead-evaluation-checklist.md`

- [ ] **Step 1: Add the parallel pipeline section**

Document when to load the new skill, how WorkGraph relates to AcceptanceLedger,
and which local git operations are pre-authorized only for approved epics.

- [ ] **Step 2: Extend the worker marker**

Keep `[TEAM_LEAD_WORKER_V1]`, but require these fields for parallel code lanes:

```yaml
worktree: <absolute path>
branch: claude/<work_id>/<task_id>
base_sha: <integration head>
commit_policy: scoped_commit
read_set: [...]
write_set: [...]
hotspot_leases: [...]
validation_profile: <profile>
```

- [ ] **Step 3: Update review checklist**

The lead must verify the worker used the declared worktree, branch, base SHA,
write set, hotspot leases, and scoped commit policy before accepting.

### Task 3: Add Epic WorkGraph and Integration Report

**Files:**
- Create: `docs/epics/EPIC-KNOWLEDGE-MEMORY-NPC-RUNTIME/WorkGraph.yaml`
- Create: `docs/epics/EPIC-KNOWLEDGE-MEMORY-NPC-RUNTIME/IntegrationReport.md`
- Modify: `docs/epics/EPIC-KNOWLEDGE-MEMORY-NPC-RUNTIME/Implement.md`
- Modify: `docs/epics/EPIC-KNOWLEDGE-MEMORY-NPC-RUNTIME/TaskCards.md`

- [ ] **Step 1: Seed WorkGraph**

Add WIP limits, hotspot definitions, the current integration branch, current
dirty-state recovery lane, and the first vertical-slice tasks.

- [ ] **Step 2: Seed IntegrationReport**

Record that the worktree has existing dirty integration debt and that new code
lanes should wait until recovery classification runs.

- [ ] **Step 3: Update Implement.md**

Replace "select next ledger row/task" with WorkGraph ready-set scheduling and
rolling integration.

- [ ] **Step 4: Update TaskCards.md**

Add ownership metadata for vertical slice cards: dependencies, write sets,
hotspot leases, and validation profile.

### Task 4: Validate Documentation

**Files:**
- Read-only validation over protocol docs.

- [ ] **Step 1: Check new terms are discoverable**

Run:

```bash
rg -n "WorkGraph|write_set|hotspot|scoped_commit|rolling integration|team-lead-parallel-pipeline" AGENTS.md CLAUDE.md .agents docs/codex-team-lead docs/epics docs/team-lead-pipeline
```

Expected: hits in project adapter, worker activation, autonomous loop skill,
dispatch template, epic implementation docs, WorkGraph, and installed v4 assets.

- [ ] **Step 2: Check for known unsafe legacy commands**

Run:

```bash
rg -n "runclaude|cd backend|npm run|python scripts/compile_api|rm -rf|git reset|git clean|deploy|push" AGENTS.md CLAUDE.md .agents docs/codex-team-lead docs/epics docs/team-lead-pipeline
```

Expected: only policy text forbidding push/deploy/destructive cleanup, if any.

- [ ] **Step 3: Review diff**

Run:

```bash
git diff -- AGENTS.md CLAUDE.md .agents docs/codex-team-lead docs/epics docs/team-lead-pipeline docs/superpowers/specs/2026-06-18-team-lead-parallel-pipeline-optimization-design.md docs/superpowers/plans/2026-06-18-team-lead-parallel-pipeline-optimization.md
```

Expected: only protocol/design/plan docs changed.

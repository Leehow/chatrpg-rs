---
id: TC-D3-02-active-npc-behavior-prompt-v1
title: Active NPC Behavior Prompt v1 — mind view to prompt-safe guidance
mode: implementation
owner: claude-worker
priority: P1
risk: high
expected_backend: tty
subagent_policy: research_only
observability: full
repair_budget: 3
---

# TC-D3-02 — Active NPC Behavior Prompt v1

## Objective

Wire active NPCs into GM prompt assembly by loading each NPC's profile, mind
view, and behavior plan, then injecting prompt-safe action/speech guidance.

## Why this matters

TC-NPC-03/04 created mind and behavior foundations, but Design3 requires NPC
speech/action to be constrained before narration renders, not merely available
as a helper.

## Non-goals

- Do not generate final NPC dialogue outside the GM narration path.
- Do not commit relationship or knowledge state from behavior plans.
- Do not make NPCs omniscient by consulting GM truth.
- Do not rewrite the full prompt compiler.

## Scope owned

- GM turn-loop prompt assembly path
- small runtime adapter if needed for active NPC plan loading
- `crates/trpg-gm/tests/**`
- focused runtime/model tests if needed

## Scope off

- profile storage migration (TC-D3-01 owns this)
- NoSpoiler production source work
- memory proposal commit pipeline
- broad context compiler rewrite
- real provider calls
- shared ledger/task-card edits

## Architecture constraints

- Behavior plans must load only the speaking/active NPC's own mind view.
- Prompt block must include only safe profile projection, known/believed fact
  ids or safe summaries, relationship summary, and guidance.
- GM-only secrets and player-unknown facts must not be echoed as prose.
- Missing profile or unstable NPC identity should fail soft/closed: skip guidance
  and trace why, not invent persona.

## Implementation guidance

Prefer a small `ContextBlock` / prompt section with a stable block id and
SystemOnly visibility. Reuse `load_npc_behavior_plan` and profile store helpers.
If "active NPC" detection is not yet explicit, choose the smallest existing
source-backed path and document unsupported producers.

## Acceptance criteria

- A test can create an active NPC with profile, relationship, and knowledge,
  then observe a prompt-safe behavior guidance block.
- Guidance reflects relationship stance and reveal/withhold sets.
- Unknown GM facts do not enter the NPC plan.
- GM-only profile secrets do not enter prompt text.
- Missing or unstable NPC profile does not panic or invent guidance.

## Required tests

- `active_npc_behavior_plan_enters_prompt`
- `active_npc_prompt_excludes_gm_only_profile_secret`
- `active_npc_prompt_excludes_unknown_fact`
- `active_npc_missing_profile_fails_soft`

## Validation commands

```bash
cargo test -p trpg-gm npc_behavior
cargo test -p trpg-runtime --lib npc_behavior
cargo test -p trpg-model npc_behavior
cargo check -p trpg-model -p trpg-db -p trpg-runtime -p trpg-gm
git diff --check
```

## Escalation triggers

- Active NPC identity cannot be determined from any source-backed runtime path.
- The only viable implementation would require passing GM-only secret prose into
  player-visible prompt or output.
- A broad turn-loop rewrite is needed.

## Done when

GM prompt assembly has tested, prompt-safe NPC behavior guidance for active NPCs.

## Handoff requirements

Include active NPC source assumption, changed files, validation output, any
unsupported active-NPC producers, and acceptance-criteria ledger.

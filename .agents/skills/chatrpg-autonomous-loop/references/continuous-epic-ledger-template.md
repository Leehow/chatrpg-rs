# Active Epic Ledger: <work_id>

## Run mode

`continuous_epic`

## User goal

<One paragraph: what the user wants done from start to finish.>

## Constitution

- Source-grounded JSON asset runtime, not rule compiler.
- Runtime commits; agents/plugins propose.
- Player-facing narration cannot leak player-unknown facts.
- NPC speech/action must be constrained by NPC knowledge, persona, goals, and relationship state.
- Heavy postprocess must not block user-visible streaming completion.

## Stop conditions

- All items terminal.
- Hard blocker requiring user decision.
- Unsafe working tree.
- Explicit user budget or stop.
- Repeated task failure with no safe bounded next action.

## Task ledger

| Order | Task ID | Path | Dependencies | Status | Worker/Handoff | Validation Evidence | Lead Decision | Next Action |
|---:|---|---|---|---|---|---|---|---|
| 0 | TC-KNOW-00 | docs/agent-loop/task-cards/knowledge-runtime/TC-KNOW-00-actor-identity-contract.md | none | NotStarted | | | | |
| 1 | TC-KNOW-01 | docs/agent-loop/task-cards/knowledge-runtime/TC-KNOW-01-knowledge-edge-v1.md | TC-KNOW-00 optional | NotStarted | | | | |
| 2 | TC-KNOW-02 | docs/agent-loop/task-cards/knowledge-runtime/TC-KNOW-02-reveal-events-v1.md | TC-KNOW-01 | NotStarted | | | | |
| 3 | TC-KNOW-04 | docs/agent-loop/task-cards/knowledge-runtime/TC-KNOW-04-npc-durable-knowledge-edges.md | TC-KNOW-00, TC-KNOW-01, TC-KNOW-02 | NotStarted | | | | |
| 4 | TC-KNOW-03 | docs/agent-loop/task-cards/knowledge-runtime/TC-KNOW-03-no-spoiler-plugin-v2.md | TC-KNOW-01, TC-KNOW-02 | NotStarted | | | | |
| 5 | TC-NPC-01 | docs/agent-loop/task-cards/npc-mind/TC-NPC-01-npc-profile-v1.md | TC-KNOW-00 | NotStarted | | | | |
| 6 | TC-NPC-02 | docs/agent-loop/task-cards/npc-mind/TC-NPC-02-npc-relationship-v1.md | TC-KNOW-00, TC-NPC-01 | NotStarted | | | | |
| 7 | TC-NPC-03 | docs/agent-loop/task-cards/npc-mind/TC-NPC-03-npc-mind-view-v1.md | TC-KNOW-04, TC-NPC-01, TC-NPC-02 | NotStarted | | | | |
| 8 | TC-NPC-04 | docs/agent-loop/task-cards/npc-mind/TC-NPC-04-npc-behavior-plan-v1.md | TC-NPC-03 | NotStarted | | | | |

Status values: NotStarted, InProgress, Done, Partial, NeedsRevision, Rejected, BlockedByPrerequisite, BlockedByHuman, Deferred, Untested.

## Current checkpoint

- Last accepted task:
- Current worker:
- Next unblocked task:
- Why not stopped:

## Lead notes

Append accepted handoff summaries, validation decisions, and assumptions here. Workers do not edit this section unless explicitly scoped.

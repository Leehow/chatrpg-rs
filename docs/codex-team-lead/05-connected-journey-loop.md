# Connected Journey Development Loop

## Why this replaces separate “functional” and “real” tests

A feature must have one scenario contract. The same scenario runs with different provider adapters:

```text
ScenarioSpec
  ├─ deterministic mode: controlled provider + seeded dice
  ├─ live mode: real GM LLM + isolated player simulator
  └─ replay mode: reviewed live-run cassette
```

All modes use the same public character/session/turn interfaces, player actions, trigger contracts, and evidence schema. Functional regression is therefore a deterministic execution of the real journey, not a separate synthetic test.

## The loop

1. **Select a design acceptance row.**
   State the player-visible claim, not only the type/table to add.

2. **Write the journey checkpoint before implementation.**
   Define:
   - valid starting state;
   - how a human player naturally triggers the feature;
   - expected runtime mechanism;
   - player-visible effect;
   - committed state;
   - later-turn/reload effect.

3. **Implement the smallest vertical slice.**
   A vertical slice may include model, DB, runtime, plugin, tool, prompt projection, and trace work. Do not stop at a foundation layer if the acceptance row claims gameplay behavior.

4. **Run component checks.**
   V0-V2 diagnose local errors.

5. **Run the scenario in deterministic mode.**
   Use the public product path. Seed dice only through the dice adapter. Do not inject the intended domain event or final state.

6. **Repair until the trigger-evidence chain closes.**

```text
human action
  → intent/route
  → need/binding/plugin/tool
  → dice/check/reveal/relationship event
  → committed projection
  → player-visible output
  → later-turn consumption
```

7. **Run the identical scenario in live mode.**
   The GM is the real configured LLM. The player simulator sees only player-visible output, its own character sheet, and public choices.

8. **Review the live run.**
   If accepted, capture provider/tool/dice interactions as a replay cassette.

9. **Replay the accepted cassette in CI.**
   Unexpected extra/missing LLM/tool calls fail replay rather than silently falling back.

10. **Update the acceptance ledger.**
    Mark Done only when the row's required journey checkpoints pass. A task card or worker handoff is not the unit of design completion.

## Trigger contract

Every journey checkpoint must define this tuple:

```text
Precondition
Player action
Reachability signal
Required mechanism evidence
Player-visible assertion
State assertion
Continuity assertion
```

A checkpoint without a reachability signal cannot distinguish “feature passed” from “feature was never used.”

## Natural trigger rule

The player simulator may make at most the approved number of plausible follow-ups. It may clarify intent in ordinary player language, but may not name internal events, tools, fact IDs, database tables, or expected implementation details.

Example trigger ladder for a technical check:

1. “我检查无人机背后的线缆，看它接到哪里。”
2. If the GM only gives surface description: “我实际拆开护盖，判断能不能安全切断，并开始操作。”
3. If no check/roll mechanism is reached after the approved attempts: `NOT_TRIGGERED`; do not inject a pending check.

# Journey Acceptance Rules

## The Trigger-to-Evidence Chain

For each design acceptance row, the lead must be able to point to one connected chain in one journey run:

```text
human operation
→ observed product response
→ intended capability triggered
→ runtime mechanism evidence
→ committed state/projection
→ player-visible consequence
→ later-turn/reload consequence (when claimed)
```

Evidence from unrelated tests cannot be spliced together to claim this chain.

## Feature-specific acceptance

### Character/session readiness

A play journey cannot start until a public character-creation operation has produced a valid actor and session. Missing character is `INVALID_SETUP`.

### Checks and dice

A check-dependent claim requires all applicable evidence:

- natural player action;
- check contract/pending or GM-roll path;
- actual `DiceRolled`;
- actual `CheckResolved`;
- outcome reflected in narration/state;
- later state uses the result if persistent.

No dice means the check feature was not exercised.

### Knowledge and reveal

A reveal claim requires:

- the fact is unknown before the action;
- the player performs a legitimate perception/check/dialogue/handout action;
- the action reaches the reveal capability;
- `PlayerLearnedFact` or equivalent committed event occurs;
- the fact becomes available in a later player projection;
- unrelated NPC holders do not automatically gain it.

### No spoiler

Use a before/trigger/after journey, not separate isolated tests:

1. Before reveal: secret absent from prompt/output/player memory.
2. Trigger: player legitimately discovers the fact.
3. After reveal: the same fact is now eligible for player narration.
4. NPC without knowledge still cannot speak from it.

### NPC persona/relationship

A social journey must include:

- first meeting with stable NPC identity;
- baseline dialogue/behavior;
- a human action such as asking, helping, threatening, lying, paying, or sharing a secret;
- relationship/mind delta with event evidence;
- a subsequent NPC response measurably changed by persona + relationship;
- optional reload/revisit proving persistence.

A DB vector update without changed dialogue/action is not accepted.

### Memory

Memory claims require cross-turn or cross-process consumption:

- event committed in turn N;
- process/session resumed or a later turn executed;
- retrieval/projection contains the fact/relationship;
- GM/NPC behavior uses it appropriately;
- replaying heavy extraction is idempotent.

### Plugins

A plugin claim requires both trace and behavior:

- hook/contribution recorded;
- contribution affects context/prompt/verifier/job as designed;
- player-visible or state effect observed;
- fail policy observed under adversarial input.

## Lead acceptance checklist

Before marking any design row Done, answer yes to all:

1. Did the journey create and bind a real character when play requires one?
2. Did the player action look like something a real player would type?
3. Did the feature actually trigger?
4. Did the expected mechanism execute, including dice/checks where applicable?
5. Is the player-visible result connected to that mechanism?
6. Is the state change committed and inspectable?
7. Is persistence/next-turn behavior proven when the design claims memory?
8. Did deterministic and live modes use the same scenario contract?
9. Was the live run accepted before cassette promotion?
10. Are unit/contract tests used for diagnosis rather than as substitutes for the journey?

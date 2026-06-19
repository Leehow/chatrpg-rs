# Plan — EPIC-KNOWLEDGE-MEMORY-NPC-RUNTIME (Vertical Journey Edition)

## Milestone 0 — Journey Harness Eligibility

Before accepting any gameplay design row, implement/verify a runner that can:

1. create a character through the public CLI/API;
2. bind the character to a session and verify the sheet;
3. start from a playable scene;
4. drive adaptive human-like actions;
5. respond to pending player rolls using the created character and public dice path;
6. capture turn events, traces, read-only DB/projection snapshots, and player-visible output;
7. run one scenario in deterministic, live, and replay modes.

No later milestone can be Done while this milestone is missing.

## Milestone 1 — Fresh Player → Observation → Hidden Truth Stays Hidden

Journey: `JRNY-HOME-01-athena-knowledge-reveal` chapters `setup` and `first_observation`.

The journey must create a real character and session, enter the first playable Homecoming scene, let the player naturally observe, and prove internal scene truth does not become player knowledge or player narration.

Targets: DA-KNOW-02, DA-KNOW-04, DA-SPOIL-01 partial, DA-SPOIL-02 before-reveal half, DA-PIPE-01/02 partial.

## Milestone 2 — Technical Action → Real Check/Dice → Reveal or Legitimate Failure Branch

Continue the same journey. The player naturally inspects/manipulates the cable/device. The system must create an actual check/roll path. The runner must roll with the created character, not invented values.

Success branch proves reveal and later use. Failure branch proves the fact remains hidden and follows a legitimate human alternative without state injection.

Targets: DA-KNOW-05, DA-SPOIL-04, DA-SPOIL-02 after-reveal half, DA-PIPE-02.

## Milestone 3 — NPC First Meeting → Withholding → Relationship Change → Changed Response

Journey: `JRNY-HOME-02-odessa-social-memory`.

The player meets a stable NPC, asks about a secret, observes baseline evasion, then performs a natural social action (help/threat/payment/evidence sharing). The runtime must update relationship/mind state with evidence and the NPC's next response must measurably change while still respecting knowledge and disclosure boundaries.

Targets: DA-NPC-01..05, DA-MEM-02.

## Milestone 4 — Commit → Reload → Remember

Continue the same social/knowledge sessions after process restart or fresh CLI invocation. The player revisits the fact/NPC. The correct knowledge and relationship must be retrieved and consumed in later dialogue/action.

Targets: DA-MEM-01, DA-MEM-03, DA-PIPE-03 plus persistence parts of DA-KNOW/DA-NPC.

## Milestone 5 — Live Runs and Cassette Promotion

Run the exact same scenario specs with the configured real GM LLM and isolated player simulator. Review the evidence bundles. Promote accepted live runs to replay cassettes and require replay in CI.

## Final acceptance

The epic is not complete merely because foundation task cards pass. It is complete only when the journey-linked AcceptanceLedger has no Missing/Partial/Implemented/Untested/BlockedByPrerequisite rows and the required connected journeys pass qualification, trigger, mechanism, outcome, and continuity gates.

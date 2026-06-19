# Codex Team Lead — TRPG Journey-Driven Validation v3

This patch replaces task-card-centric acceptance with connected, human-operation journey acceptance.

Core rule:

> No trigger, no pass. No valid player character/session, no valid playtest. A feature is accepted only when a human-like action reaches the real product path, exercises the intended mechanism, produces player-visible behavior, commits state, and changes a later turn when persistence is part of the claim.

Apply this pack over the previous design-task-evaluation pack. The key replacement files are:

- `docs/codex-team-lead/02-validation-rubric.md`
- `docs/codex-team-lead/05-connected-journey-loop.md`
- `docs/codex-team-lead/06-human-player-simulator.md`
- `docs/codex-team-lead/07-scenario-cassette-contract.md`
- `docs/codex-team-lead/08-journey-acceptance-rules.md`
- `docs/epics/EPIC-KNOWLEDGE-MEMORY-NPC-RUNTIME/Plan.md`
- `docs/epics/EPIC-KNOWLEDGE-MEMORY-NPC-RUNTIME/TaskCards.md`

The same scenario specification must run in deterministic and live-model modes. A passing live run may be promoted to a replay cassette, so functional regression is derived from real play rather than maintained as a disconnected synthetic test.

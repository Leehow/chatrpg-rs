# Validation pipeline

Validation is a concurrent lane, not an end-of-epic phase.

## Per-task gates

- static architecture guard;
- owned-module unit/contract tests;
- affected-crate compile/test;
- DB migration/roundtrip test when applicable;
- no unrelated diff and no shared-hotspot violation.

## Rolling integration gates

- trial cherry-pick succeeds;
- affected dependents compile;
- targeted integration tests pass;
- architecture guards pass;
- integration branch remains green.

## Milestone Journey gate

Use the same ScenarioSpec in:

- deterministic provider + seeded dice;
- live model + adaptive player simulator;
- accepted cassette replay.

The journey must start through the public product path, including character creation, character/session binding, and playable-scene readiness.

Required connected evidence:

```text
HumanOperation
  -> Intent/FeatureTrigger
  -> Need/Binding/Tool/Dice mechanism
  -> DomainEvent/StateCommit
  -> Player-visible or NPC-visible effect
  -> next-turn or reload persistence
```

Statuses:

- `INVALID_SETUP`
- `NOT_TRIGGERED`
- `TRIGGERED_NO_MECHANISM`
- `MECHANISM_NO_USER_EFFECT`
- `PASS`
- `FAIL`
- `FLAKY`
- `BLOCKED`

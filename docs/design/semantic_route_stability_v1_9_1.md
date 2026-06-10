# v1.9.1 Semantic Route Stability Hotfix

## Purpose

v1.9 introduced semantic rule binding and ability hydration, but test feedback showed that LLM-primary routing made the same input route differently across repeated runs. v1.9.1 keeps semantic understanding, but moves route ownership back to a deterministic Rust reducer.

## Principles

- LLM semantic classification is evidence, not the final owner of route selection.
- Rust route invariants decide required reactions, disarm/grab, technical assessments, frame-worthy actions, and direction-gate continuation.
- ObjectService and AbilityService run only when the TurnOrchestrator selected their route.
- Required reaction gates block ordinary object/attack/analysis intents; only terminal/exit/de-escalation intents may supersede them.
- Technical risk assessment creates an agentic check, not free narration and not an object patch.
- Disarm/grab is a compound action: object interaction + possible combat/contest, not a generic attack.

## Fixed classes

1. Build snags: missing `sqlx` in `trpg-params`, missing `trpg-time` in `trpg-combat`, and invalid `FrameRelation` variants in `trpg-semantics`.
2. LLM route instability: Rust invariants now preserve deterministic routes even if semantic classification is low confidence or unavailable.
3. Disarm/grab unreachable: clear disarm/grab phrases route to ObjectInteractionFirst so weapon hydration and ObjectInteractionContract creation are reachable.
4. Homecoming technical analysis: risk-assessment language routes to `StandaloneCheckOrAgent` so the agentic check path can create a pending check.
5. Required reaction priority: non-terminal object/attack/analysis intents cannot supersede required reaction gates.
6. Direction gate continuation: continue/keep firing/press attack remains a direct choice route.

## Non-goal

This hotfix does not implement the full real-materialization extractor. Actor/object/ability parameters are still seeded or provisionally bound when exact table extraction is unavailable.

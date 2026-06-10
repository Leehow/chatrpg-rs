# v1.2 Actionable Situation Director

## Purpose

v1.1 prevents hard frame loops by routing player input semantically. v1.2 addresses the next product issue: the GM must present a playable situation, not merely a vivid paragraph or a list of official-looking options.

The Actionable Situation Director converts the current context into an `ActionableSituationBrief` with visible facts, pressure, affordances, risks, known facts, open questions, and a guidance level. It gives the LLM a bounded director context before narration so the GM behaves as facilitator/referee/scene director/consequence manager rather than a novelist.

## Core Rule

Do not tell players what to do. Tell them what they can perceive, what is under pressure, what is interactable, what risks exist, and ask what they want to change.

## New objects

- `ActionableSituationBrief`
- `PlayerFacingClueBoard`
- `Affordance`
- `PressureItem`
- `RiskItem`
- `GuidanceDecision`
- `CostedExample`
- `NpcBiasedAdvice`
- `ConsequenceContract`
- `ClockTick`
- `SpotlightState`
- `SceneFramePurpose`

## Guidance ladder

1. `observable_facts_only`: describe actionable facts, not options.
2. `summarize_known_info`: recap known public information and leads.
3. `ask_goal`: ask what the players want to change.
4. `offer_action_categories`: provide broad vectors like technical/social/tactical/resource/retreat.
5. `offer_costed_examples`: only when stuck; provide at least three examples, each with a cost.

## Runtime flow

```text
InteractionGate resolution
  → Context compile
  → Semantic Situation Orchestrator / ConflictAgent
  → Actionable Situation Director
  → Agentic checks / LLM narration
  → learning audit / memory / clocks
```

The Director writes dynamic BP3 context blocks and does not modify BP1/BP2 resident material.

## Output constraints

- Never frame one option as the official answer.
- Avoid "你应该..." / "you should..." except safety or rules obligations.
- NPC suggestions must be biased by NPC goals.
- Costed examples require at least three alternatives.
- Failure should be a consequence path, not a dead end.
- The final question should generally be about goal, risk, or approach, not "你们怎么办？".

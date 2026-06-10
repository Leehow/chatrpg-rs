# Product Design: Actionable Situation Director v1.2

## What it is

Actionable Situation Director is the product layer that helps the LLM GM present scenes as playable situations. It turns raw context into player-facing facts, pressure, affordances, risks, known leads, and a decision prompt.

## Why it matters

Players often stall not because the description is weak, but because they do not know the current goal, what objects are interactable, what will happen if they wait, or whether there is a hidden correct route. The Director prevents the GM from acting like a novelist and instead makes it act as host, referee, scene director, and consequence manager.

## Product behavior

Bad behavior:

```text
You are outside the warehouse. A drone is firing. What do you do?
```

v1.2 behavior:

```text
Visible facts: the drone fires in bursts; its cable flashes before each burst; injured police are pinned; server noise comes from the warehouse.
Pressure: if you wait, the police line worsens and more factions may arrive.
Affordances: cable, drone behavior, wounded people, warehouse access.
Question: what do you most want to change first—save people, control the threat, trace the source, gain information, or withdraw?
```

## Architecture

- Rust Director constructs `ActionableSituationBrief`.
- Brief is persisted to PostgreSQL and injected as BP3 dynamic context.
- ClueBoard is player-facing and scene/session-stable.
- ConsequenceContract encodes fail-forward behavior.
- ClockTick records visible pressure changes.
- SpotlightState supports future player spotlight rotation.

## Success metrics

- Scene openings include goal/pressure/affordance/risk anchors.
- Stuck-player inputs produce guidance ladder output rather than a single route.
- NPC advice is biased and never official truth.
- Investigation turns update a clue board without revealing GM-only truth.
- Repeated waiting/searching advances clocks or opens direction gates.
- LLM output stops using a numbered menu every turn.

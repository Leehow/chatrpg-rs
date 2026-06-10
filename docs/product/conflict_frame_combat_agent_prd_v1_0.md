# ChatRPG v1.0 Product Design: Conflict Frame & Combat Agent

## What this feature is

Conflict Frame & Combat Agent is the product layer that lets ChatRPG run combat, chases, anomaly encounters, horror confrontations, hazardous scenes and side-quest pressure without hardcoding one game's combat rules.

It is designed for an LLM GM that behaves like a human GM: it knows how the current conflict is structured, asks for the right player choices, rolls when appropriate, hides secret rolls when needed, records the result, and forgets unimportant tactical details when the conflict ends.

## Design philosophy

1. Combat is not universal; conflict frames are universal.
2. Every required player choice is an InteractionGate.
3. Every roll has a visibility policy.
4. Every mechanical ruling is auditable.
5. Every temporary state has a lifetime.
6. Every ended conflict must compact into durable consequences.
7. Ruleset differences belong in profiles/advice and learned packets, not Rust if/else trees.

## User experience

The player speaks normally. When the system needs a commitment, it interrupts cleanly:

- "Choose your reaction: dodge/take cover, return fire, or take the hit."
- "Roll TECH + Basic Tech + 1d10; success identifies a safe path, failure triggers pressure."
- "You can spend your reaction for an opportunity attack, or let it pass."

If no player choice is needed, the system may roll publicly, roll secretly, use passive resolution, or narrate a free read.

## Product architecture

- Rust Agent owns the state machine.
- LLM skills provide bounded prose/reasoning only when Rust allows them.
- Tantivy search retrieves rules, modules, memory and learned packets.
- StateFrame tracks transient operation state.
- InteractionGate controls current required/optional player input.
- CheckContract frames uncertainty.
- EffectContract frames consequences.
- PatchValidator is the only path to durable state.
- FrameCompactor decides what survives after the fight/mission/scene.

## Supported conflict shapes

### Cyberpunk RED
Firefights, Netrunning hooks, chases, armor/wound/resource state and NPC/vehicle cards.

### D&D 5e
Initiative, movement/action/bonus action/reaction, opportunity attacks, attacks, saves and conditions.

### Call of Cthulhu / BRP
Roll-under checks, Keeper hidden information, fight-back/dodge/maneuver gates, chases, wounds and sanity.

### Triangle Agency
Anomaly encounters, 6d4, Chaos, Harm, mission outcomes, playwalled visibility and aftermath.

## Success metrics

- Required reactions block unrelated actions until resolved.
- Optional reactions can default according to profile.
- Player-required checks stop SSE and resume on the next input.
- Public GM rolls stream as dice events and continue narration.
- Secret rolls are written to audit logs but not revealed to player output.
- Frame state changes only affect BP3.
- Frame compaction preserves aftermath and discards transient round data.
- Learning candidates include CheckContract/EffectContract data, not only prose.

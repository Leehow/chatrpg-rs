# Product Design: Mechanics Search Skills v1.12

## What it is

Mechanics Search Skills make the LLM GM better at finding and binding rules without becoming a rules-lawyer or a brittle keyword bot. The GM starts from a mechanical need, such as combat resolution, weapon damage, armor defense, spell activation, sanity loss, chaos harm, or an NPC stat block, then performs a staged search until it has enough evidence to write parameters back into runtime state.

## Why it matters

The system already has actor, object, ability, check, contest, effect, and materialization models. The missing product layer is a reusable way to ask: "What kind of rule or parameter do I need, and where should I look next?" A single-step search is too brittle. A table may need the combat procedure first, then weapon damage, then armor handling, then the consequence rule.

## Design philosophy

- Use one generic retriever.
- Add reusable search skills over that retriever.
- Avoid one hard-coded search path per ruleset.
- Write results into the existing parameter system.
- Let LLM semantic extraction decide meaning; let Rust persist and validate state.
- Respect visibility, GM-only, and playwalled material.

## Example: combat

When a gunfight begins, the GM does not just search “gun damage.” It plans:

1. Where does this ruleset define combat resolution?
2. What attack/defense model applies?
3. What weapon or ability is the source?
4. What damage or consequence model applies?
5. Does armor, resistance, sanity, chaos, harm, or another resource participate?
6. Which extracted values become actor/object/ability/check/effect facets?

## Success criteria

- The same search skill can plan useful queries for Cyberpunk RED, D&D, Sword World, BRP/CoC, and Triangle Agency.
- Candidate hits are never treated as truth until extracted and written back.
- Search plans are inspectable in DB and BP3 context.
- Parameter facets clearly show which actor/object/ability/check/effect field was filled by which binding.
- Narrative systems without conventional combat are handled by searching consequence/harm/chaos/anomaly procedures rather than forcing weapon/armor assumptions.

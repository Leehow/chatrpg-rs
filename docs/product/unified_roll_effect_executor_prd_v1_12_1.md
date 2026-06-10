# Product Brief: v1.12.1 Unified Roll & Effect Executor

## What it is

A referee layer that lets the LLM GM arrange a roll using rules memory and parameter facets, call a single random dice tool, then apply the result to the correct runtime parameter.

## Why it matters

Players should not be forced to act as the rulebook. The GM should know or look up what to roll, what the target is, what the roll affects, and how the result changes state. Earlier versions could declare that damage happened in narration while leaving the database unchanged. This version makes the mechanical result persistent.

## Experience target

For Homecoming/Cyberpunk RED:

```text
GM: Attack roll is 1d10 + REF + Handgun vs DV 14.
System: rolls or accepts the roll.
GM: 16 vs 14, hit. Damage roll is 3d6.
System: rolls or accepts damage.
GM: 11 damage. HP 28 → 17. This is recorded.
```

For CoC/BRP:

```text
GM: SAN roll is d100 against current SAN.
System: rolls.
GM: failed; SAN loss is rolled and applied to resources.sanity.current.
```

For Triangle Agency:

```text
GM: anomaly ability creates a Chaos impact.
System: rolls if required and increments scene.tracks.chaos.current.
```

## Non-goals

This is not a complete ruleset engine. It does not hardcode Cyberpunk armor ablation, D&D spells, Sword World power tables, or CoC SAN tables. Those remain parameter facets and search-skill bindings consumed by later validators.

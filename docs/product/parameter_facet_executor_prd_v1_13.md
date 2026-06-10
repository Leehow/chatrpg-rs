# Product Design: v1.13 Parameter Facet Executor

## What it does

v1.13 turns rule and parameter fragments into actual adjudication. The GM no longer merely knows that a rule was found; it can apply the rule to an actor, object, ability, scene, resource track, condition, or anomaly state.

## Product promise

When the GM says an effect happened, the system must know:

- what source caused it,
- which target parameter it affects,
- how the value changes,
- whether the source was exact, bound, or provisional,
- and what state was written.

## Why this matters

Earlier versions could create events and even roll dice, but mechanical results could remain narration-only. v1.13 makes the runtime ledger the source of truth. A gunshot, sanity loss, chaos increase, condition application, or object state change is represented as a `ParameterImpact` and a facet execution.

## User-facing behavior

The GM should no longer ask the player for remaining HP, SAN, Chaos, Harm, armor, or table values when the system has a bound facet or a provisional policy. When a value is provisional, the GM should say so briefly and continue play, recording it for audit.

## Scope

Implemented:

- Parameter facet execution.
- Ruleset starter profiles for Cyberpunk RED, D&D 5e, Sword World 2.5, BRP/CoC, and Triangle Agency.
- Generic non-actor parameter states.
- Actor HP/resource/condition impact execution.
- BP3 mechanical ledger projection of facet executions.

Not complete:

- Full extracted source packs for every table.
- Exact Cyberpunk armor ablation.
- Exact D&D spell engine.
- Exact Sword World power-table execution.
- Exact CoC Major Wound thresholds.
- Full Triangle playwalled executor.

These are now data/facet improvements, not new engines.

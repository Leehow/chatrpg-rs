# Product Design: Situation Novelty Director v1.3

## What it is

The Situation Novelty Director prevents ChatRPG from feeling repetitive. It watches recent beats, NPC tactics, player repeated actions, and output closures. When repetition is detected, it forces a tactical or environmental change.

## User-facing goal

If a player says “I keep firing,” the GM should accept that action, but the world should change: the enemy changes cover, threatens an NPC, overheats equipment, calls backup, retreats to an objective anchor, or reveals a new risk.

## Why it matters

Players dislike repetition even when they choose similar actions. Repetition is tolerable only if it has visible cost or progress. The system therefore treats freshness as a state requirement rather than a prose style guideline.

## Product success criteria

- Enemy-initiated combat starts a frame.
- Direction gates do not reprompt-loop on “continue attacking”.
- Repeated player attacks produce fresh change or NPC tactic shift.
- NPCs cannot repeat the same tactic beyond configured limits.
- Director output always includes visible facts, pressure, affordances, risks, and fresh change.
- The new state remains dynamic BP3 context.

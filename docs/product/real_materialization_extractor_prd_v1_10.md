# Product Design: Real Materialization Extractor v1.10

## What it is

The Real Materialization Extractor makes ChatRPG stop treating rules retrieval as prompt text only. When a character appears, a weapon is drawn, an ability is used, or a damage effect is about to apply, the system creates a materialization demand and writes structured parameters back into runtime state.

## Why it matters

A GM cannot run a fight against a “boss in power armor” with the same hidden profile as a generic street mook. A weapon cannot be used for damage with an empty object profile. A named ability cannot create effects without a source ability definition. v1.10 creates the audit trail and writeback path needed before contest, damage, armor, HP, and condition validators can be reliable.

## Success criteria

- Actor profiles include source-backed or explicitly provisional stats, HP, defense, skills, and loadout.
- Object definitions include mechanical profile and rule bindings.
- Ability definitions include activation, cost, target, effect, and triggers.
- Check/effect contracts can be linked to rule binding packets.
- Retrieved exact rule hits are written back, not prompt-only.
- Missing fields produce verifier warnings instead of silent confidence.

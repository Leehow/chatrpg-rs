# Product Design: Referee Combat Slice v1.10.2

## What it is

The Referee Combat Slice turns ChatRPG from a strong narrator into a minimally reliable combat referee for the Homecoming opening encounter.

## Why it exists

Human playtest revealed that the GM could describe Homecoming well, accept player stats, and ask for rolls, but could not consistently answer whether a shot hit, request damage, persist target HP, or remember the target's wounded state next turn. A player should not have to act as the weapon table or HP ledger.

## User promise

After this slice:

- If the GM asks for an attack roll and the roll hits, it asks for damage instead of drifting into free narration.
- If the player reports damage, the GM applies it to the target's tracked HP.
- On the next shot, the GM reads the ledger instead of asking the player what HP remains.
- Player-supplied mechanical values are checked and, if out of band, flagged as table overrides rather than silently accepted.

## Non-goals

- Full Cyberpunk RED combat implementation.
- Full contest/opposition kernel.
- Full armor/SP/AC or wound-state validator.
- Complete source-pack extraction.

Those remain separate milestones.

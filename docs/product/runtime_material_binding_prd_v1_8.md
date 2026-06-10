# Product Design: Runtime Material Binding v1.8

## What this is

Runtime Material Binding makes lazy loading useful at the table. When an NPC, weapon, armor, cable, lock, or other object appears, the system creates a runtime representation with enough mechanical profile to support checks, state changes, and future narration.

## Why it matters

The GM should not narrate an NPC drawing a gun while the system has no gun. The GM should not ask for a disarm check against a statless placeholder. The GM should not forget that the player grabbed the weapon a turn later.

## User experience

When the player sees an enemy draw a sidearm, the runtime creates an opposition actor profile and a held weapon object. When the player disarms that NPC, the weapon transfers in the object graph. When the player fires the grabbed weapon, the combat frame can use that object rather than asking for another grab.

## Non-goals

v1.8 does not implement a full contest engine, full damage engine, full ammo economy, or complete weapon table extraction. It creates the binding layer those systems need.

# v1.6 Object & Possession Kernel

## Purpose

v1.6 adds a runtime object graph for weapons, armor, tools, doors, locks, cables, vehicles, documents, clues, devices, programs, quest items, anomaly objects, and environmental features. The goal is to make disarm, grab, pick up, drop, equip, cut cable, unlock, break, search, loot, and use-object actions into typed contracts and validated object patches instead of narration-only claims.

## Core idea

Object state is not inventory. Inventory is a projection of a richer object graph:

- `ObjectDefinition`: ruleset/module template, usually cold/on-demand.
- `ObjectInstance`: a specific object in the current world.
- `ObjectLocation`: held, equipped, worn, carried, installed, attached, connected, on ground, hidden, destroyed, unknown.
- `ObjectEdge`: contains, loaded_with, installed_in, connected_to, controls, locks, powers, worn_by, held_by, claimed_by.
- `ObjectInteractionContract`: the typed contract for interacting with an object.
- `ObjectPatch`: the validated state change.

## Cache policy

- BP1: protocol only.
- BP2: stable scene objects, loaded object definitions, current NPC loadout cards if stable.
- BP3: who holds what, slots, equipped armor, ammo/quantity, damaged objects, hidden/revealed state, active object interaction contracts.

## Lifecycle policy

Object interactions attach to the v1.5 Interaction Lifecycle Kernel. Closing a frame must supersede open object interactions, just like open gates and pending checks.

## v1.6 boundaries

Implemented:

- object definitions and instances
- object graph context block
- object interaction contracts
- check-backed disarm/grab/cut/unlock/search interactions
- object patch validation and check-result application
- runtime/CLI/API integration

Deferred:

- full opposed contest kernel
- full economy/shop/crafting
- detailed ammo-by-round tracking
- encumbrance
- complete vehicle subsystem
- complete magic item attunement/cyberware surgery

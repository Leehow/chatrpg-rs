# Product Design: Semantic Rule Binding & Ability Hydration v1.9

## What it does

This release turns retrieved rule text into runtime parameters. NPCs, objects, and abilities no longer remain prompt-only or empty. When a character uses a spell, role ability, combat feat, program, requisition, skill-like ability, or triggered reaction, the system creates a typed ability record and activation contract.

## Why it exists

The system already supports lazy loading. The missing piece is writeback: search hits must become actor/object/ability/check/effect data. Otherwise the GM can see a rule in context but the runtime still has no damage, trigger, cost, source, or visibility state.

## User experience

The player can speak naturally. The GM agent should understand the action semantically, request the relevant material, retrieve candidates, extract fields, write a RuleBindingPacket, and continue the turn with auditable mechanics. Keywords can help find candidate text, but they should not decide what the player meant.

## Supported material types

- Actor parameters: stat blocks, HP, skills, defenses.
- Object definitions: weapons, armor, devices, keys, documents, environmental objects.
- Ability definitions: spells, role abilities, combat feats, skills, programs, reactions, passive traits, anomaly effects, requisitions.
- Contracts: checks, effects, contests, object interactions, ability activations.

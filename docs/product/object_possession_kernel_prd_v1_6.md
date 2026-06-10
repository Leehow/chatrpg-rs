# Product Design: Object & Possession Kernel v1.6

## What it is

The Object & Possession Kernel lets the GM treat physical and informational things as real state: weapons can be grabbed, armor can be equipped, cables can be cut, locks can be opened, documents can be hidden, and quest items can be transferred.

## Why it matters

Without object state, a GM can only narrate “you disarm him” or “you cut the cable.” The next turn the system may still treat the NPC as armed or the cable as intact. v1.6 makes these changes auditable and persistent.

## User experience

Player: “我扑过去抢他手里的枪。”

System behavior:

1. Resolve the target object: the weapon held by the opposing NPC.
2. Create an `ObjectInteractionContract`.
3. Ask for a player roll when required.
4. On success, validate and apply `TransferObject` from NPC hand to PC hand.
5. Record an `object_event` and update BP3 object graph.

## Design principles

- Objects are world state, not prose.
- LLMs cannot directly transfer or destroy objects.
- Object interactions are contracts.
- Object changes are patches.
- Ruleset-specific weapon/armor details remain in `mechanical_profile` and `rule_bindings`.
- Visibility matters: hidden/playwalled objects must not leak to players.

## Success metrics

- A disarm action creates an object interaction contract.
- A successful disarm transfers possession.
- A failed disarm does not transfer possession.
- Equipped armor changes runtime object projection.
- Hidden objects remain GM-only until revealed.
- Frame closure supersedes open object interactions.

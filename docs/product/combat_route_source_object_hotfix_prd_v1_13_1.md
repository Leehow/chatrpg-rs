# v1.13.1 Product Note: Named Weapons Must Not Break Combat

Players naturally describe attacks by naming the weapon they use. The GM must interpret that weapon as the source of the attack, not as a separate object manipulation.

This release ensures that phrases such as `用重型手枪开火` and `举枪射击` remain combat actions. Object materialization still happens, but it supports the attack instead of stealing the turn.

Acceptance:

- A named-weapon attack starts or continues combat.
- Disarm/grab/pickup remain object interactions.
- First-turn player attacks commit a mechanical check so `/roll` and auto-roll can bind immediately.

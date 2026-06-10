# PRD — v1.13.3 Semantic Combat Loop & Roll Authority

## Product problem

Players do not always say “开火” or “attack”. They may say “我压低枪口让它别靠近”, “I keep it pinned”, “撃ち続ける”, or similar. A combat GM should understand this semantically. At the same time, once the system knows the player is fighting, a stale direction menu must not block the action.

## User-visible result

- Repeated attacks continue the combat instead of opening/looping a direction menu.
- Named weapons are treated as source objects, not object-interaction targets.
- `/roll` and system-roll policy resolve the current mechanical question instead of creating orphan rolls.
- If the semantic classifier is unavailable, the system can fall back to audited lexical matching, but the final route is still reduced deterministically by Rust.

## Acceptance criteria

- `继续攻击 / keep pressure / another shot` inside an active frame routes through combat, not the direction gate.
- A direction/stalemate gate is closed/superseded by clear in-frame action.
- Multilingual or paraphrased combat declarations work when the semantic classifier is available.
- With `system_rolls_visible`, the system does not ask the player to roll for combat checks unless the table explicitly chooses player-reported dice.

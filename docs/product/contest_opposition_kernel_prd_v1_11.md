# Product Design: Contest / Opposition Kernel v1.11

## What it is

The Contest / Opposition Kernel is the part of ChatRPG that answers: "what is this roll being compared against?" It turns vague checks into explicit contests such as attack vs defense, static DV, opposed check, saving throw, or percentile roll-under.

## Why it matters

A GM that asks for a roll but cannot say whether it succeeded is not a referee. Earlier versions could emit check events, but attacks often stayed at `NoMechanicalOpposition` or `UnknownUntilLookup`. That meant the system could ask for a roll but still fail to resolve the roll mechanically.

## Product goal

When a player rolls to attack, defend, assess risk, resist magic, or use a skill, the system should build a typed resolution model before narrating the result. If exact source data is unavailable, it should mark the model provisional and audit it rather than silently improvising.

## User-facing behavior

- The GM should not ask the player what the target number is.
- The GM may ask for fictional clarifications, such as range or target, when missing.
- If the system uses a provisional target, it should be transparent to the GM/debug layer and should be revisable after rule lookup.
- Damage and state patches should only follow from a resolved contest.

## Success metrics

- Attack rolls create `contest_resolved` events.
- Resolved attacks include target/defense information.
- Technical checks use static target models rather than free narration.
- Percentile systems can produce roll-under models.
- No attack path resolves as final `NoMechanicalOpposition`.

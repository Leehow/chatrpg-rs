# Roll Authority Hotfix v1.15.3

This patch locks the product default back to action-only play:

- Player-facing GM narration must not ask the player for dice totals, damage, DV/DC, HP, SP, resource totals, or other rule-table values.
- System/GM-only runtime context is wrapped in `[gm]...[/gm]`.
- Tool-resolved dice/effect results are wrapped in `[roll]...[/roll]` before narration.
- Operational prompts and non-fictional notices use `[system]...[/system]`.
- Player-confirmed rolls use one input shape: `roll`. Player-reported totals are disabled by default.
- Dice rolling now goes through a small plugin trait with the current pseudo-random implementation as `pseudo_random_v1`.
- CLI/API product-mode turn handlers auto-execute `PlayerRollRequired` checks when `TRPG_AGENT_TABLE_DICE_POLICY=system_rolls_visible`.
- Attack success now immediately consumes system-rollable damage/effect follow-up checks and then falls through to normal LLM narration instead of returning a raw mechanical message.
- Homecoming debug target binding recognizes `scav_boss`/shotgun boss wording and directs damage to `npc.scav_boss` rather than the generic `npc.opposition`.
- NPC defeat through HP impact closes the active frame as `completed` with `frame_outcome=opposition_defeated`.

Compatibility switches:

```text
TRPG_AGENT_TABLE_DICE_POLICY=system_rolls_visible   # default
TRPG_PLAYER_REPORTED_ROLL_TOTALS=false              # default
TRPG_PLAYER_SUPPLIED_ROLL_EXPRESSIONS=false         # default
```

Set `TRPG_AGENT_TABLE_DICE_POLICY=player_rolls` only for legacy tables that deliberately want player-confirmed pending roll prompts.

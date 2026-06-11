# Context Projection Discipline

## Verify the character projection before adjudicating

At the start of a turn, if the provided context contains no player character sheet summary (no pinned character projection, no visible stats, tracks, or resources for the acting character), do not guess or improvise character values. Call `get_actor` with the bound player character id (default `pc.current`) first, then adjudicate with the returned sheet and mechanical profile.

This applies before any `roll_check`, `request_player_roll`, `apply_effect`, or `change_track` that depends on the acting character's parameters. A missing projection in context never means the character has no values; it means the projection was not loaded into this turn.

# v1.13.2 Roll/Effect Gate Closure Hotfix

This hotfix addresses the v1.13.1 debug report's remaining combat blocker: the route hotfix made named-weapon attacks reach combat, but the damage/effect result still failed to persist consistently.

## Fixes

1. Damage/effect follow-up roll requests now carry the same roll visibility as the generated effect check. Under `TRPG_AGENT_TABLE_DICE_POLICY=system_rolls_visible`, damage/effect rolls are `PublicGmRoll` rather than a misleading `PlayerRollRequired` request.
2. The fallback facet selector no longer scans `stakes.before_roll_public`, because that boilerplate enumerates HP/SAN/Chaos/Harm and can self-match `Chaos`, routing ordinary physical damage into the Triangle Chaos track.
3. Attack intent matching now includes common combat continuations such as `开火`, `补枪`, `再来一枪`, `继续打`, and trigger phrases.
4. Semantic result merging cannot override a recognized roll reply. This protects `1d10=9`, `3d6=11`, `/roll`, and similar input from being reclassified as a fresh action.
5. Auto-resolved checks now close their matching `player_roll_required` interaction gate. This prevents a resolved check from leaving a stale open gate that captures every later combat turn.

## Product invariant

If a check is resolved by system roll or parsed roll text, then its pending check and matching interaction gate must be terminal before the next turn is routed.

## Still out of scope

This is not a complete rules pack. Exact Cyberpunk RED range DV, armor/SP ablation, D&D spell/save handling, Sword World power table execution, CoC SAN thresholds, and Triangle playwalled execution still depend on better parameter facets and source packs.

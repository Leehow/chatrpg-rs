# v1.12.2 Roll Binding & Mechanical Gate Priority Hotfix

## Problem
v1.12.1 created DiceTool/RollPlan/EffectResolutionPacket plumbing, but the table loop was still broken:

- `TRPG_AGENT_TABLE_DICE_POLICY=system_rolls_visible` was read but some combat checks still behaved like player-reported checks.
- `/roll` could create a dice row without binding to the open attack/effect check.
- clear attack totals could be deferred for missing distance/cover/weapon detail even when a provisional DV already existed.
- direction/stalemate gates could consume damage/effect follow-ups.
- effect writeback phases could be misleading when no mechanical patch landed.

## Fix
The hotfix treats mechanical gates as higher priority than advisory gates:

1. `/roll` and natural roll replies are routed as `RollReply` even when there is no current `InteractionGate`, then bound to the latest unresolved `CheckContract`.
2. `ConflictDirectionGate` remains advisory and cannot preempt clear attack/effect/check inputs.
3. Cyberpunk attack fallback expressions include provisional actor skill totals via `TRPG_COMBAT_DEFAULT_ATTACK_EXPR=1d10+10`, so system rolls compare total attack values against DV.
4. system-visible effect rolls auto-resolve instead of opening a player-facing damage gate.
5. effect resolution writes `EffectResolutionPacket`, `ParameterImpact`, and HP/resource ledger patches before emitting resolved events.

## Intended loop

```text
Player action / open mechanical gate
  -> RollPlan
  -> DiceTool
  -> ContestResolution or EffectResolution
  -> ParameterImpact
  -> MechanicalLedger
  -> narration
```

This preserves the design principle that the LLM GM may decide what needs rolling, but Rust owns random number generation, state mutation, and auditability.

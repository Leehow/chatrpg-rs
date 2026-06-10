# v1.13.3 Semantic Combat Loop & Roll Authority

## Purpose

This release closes two design-level gaps left after v1.13.2:

1. **Sustained combat must not be interrupted by advisory direction gates.** If the player clearly continues a fight, presses the attack, withdraws, or otherwise performs an in-frame action, the direction/stalemate menu is superseded and the frame reducer continues.
2. **Combat intent must be semantic-first, not keyword-first.** The LLM semantic classifier supplies structured `route_cues`; Rust uses deterministic route invariants to decide final ownership. Lexical phrase lists remain as audited fallback only for semantic outages.
3. **Roll authority must be deterministic.** When `TRPG_AGENT_TABLE_DICE_POLICY=system_rolls_visible`, checks and effects are system-rollable even if a branch accidentally leaves the check as `PlayerRollRequired`.

## Key changes

- `SemanticIntentService` now asks for `route_cues` such as `combat_action`, `sustained_combat_action`, `incoming_harm`, `terminal_or_exit`, and `source_object_use`.
- `TurnOrchestrator` treats semantic combat cues as primary evidence. Lexical matching is retained only as an audited fallback.
- Direction/stalemate gates are advisory. A clear semantic in-frame action routes to `SupersedeGateThenContinueFrame`, not `ResolveGateFirst`.
- `CombatAgent` receives a structured semantic hint from `TurnOrchestrator`, so non-keyword and multilingual combat declarations can still create/continue frames.
- CLI/API roll execution paths force system roll when the configured table policy says so.

## Design invariant

```text
Semantic understanding can be probabilistic;
route ownership and roll authority must be deterministic.
```

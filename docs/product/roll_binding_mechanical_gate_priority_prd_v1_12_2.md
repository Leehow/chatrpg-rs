# Product Note: v1.12.2 Roll Binding & Mechanical Gate Priority

This patch turns the dice UI from decorative into operational. A player typing `/roll` now means "resolve the current mechanical question" whenever there is an unresolved attack, check, or effect roll. If the table is configured for `system_rolls_visible`, the GM can roll visible checks and effects without asking the player to act as the rulebook.

Player-facing goals:

- The GM should not say "there is no locked-in check" when a check contract exists.
- The GM should not ask for weapon damage, target SP, or remaining HP when a provisional/rule-derived parameter is available.
- Direction menus should not interrupt damage rolls or obvious continued attacks.
- A successful hit should lead to an effect roll and a visible ledger change.

The patch is deliberately narrow: it does not implement complete Cyberpunk RED armor ablation, D&D saving throws, Sword World power tables, CoC SAN thresholds, or Triangle Chaos execution. Those remain facet-executor work.

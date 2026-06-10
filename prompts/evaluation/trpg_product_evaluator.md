You are evaluating chatrpg as a PRODUCT, not merely as a Rust repository.

Product promise:
A user uploads rulebooks and modules, creates a character, and plays a single-player TRPG with an LLM GM.

Evaluate the system as:
- a GM/referee,
- a rules assistant,
- a scenario runner,
- an NPC/world simulator,
- and a player-facing product.

Use these rules:
1. Do not stop after build/unit tests.
2. Run or inspect a playtest that reaches at least one meaningful decision and one mechanical resolution.
3. Verify mechanical state using events/DB snapshots, not only narration.
4. Mark hard failure if the GM asks the player for dice results or rule-table values it should own: weapon damage, target DV/DC, SP/AC, previous HP, SAN/Chaos/Harm totals.
5. Mark hard failure if player-facing text leaks GM-only hidden facts.
6. Mark serious failure if a clear player action is blocked by a stale gate/menu.
7. Mark serious failure if NPCs loop the same response instead of changing tactics, morale, negotiation, retreat, or escalation.
8. Reward good GM behavior: clear situation anchors, meaningful choice, source-grounded rulings, interesting consequences, fun pacing.

Default play contract:
The player describes only fictional behavior. The GM/system decides when rules are needed, retrieves/materializes parameters, calls the dice tool, resolves outcomes and effects, writes the ledger, narrates consequences, and stops when the next player decision is needed.

Return strict JSON matching schemas/product_evaluation_report.schema.json when asked for machine output. For human output, use harness/evaluation/evaluation_report_template.md.

Human-player realism rule:
When evaluating a playtest where the player is an LLM/subagent, penalize player inputs that look like QA scripts, code, JSON, SQL, DB/table names, event names, Rust type names, dice commands, reported roll totals, or bundled engineering assertions. Debug directives inside `[debug]...[/debug]` are allowed only as stripped test setup, not as player behavior.

Debug directive rule:
If debug directives were used, judge whether they made the test reproducible and whether the remaining player-facing action was still human-like. Do not give a pass merely because debug state exists; require the GM to act on it through normal fiction/mechanics.

Automatic roll/effect rule:
Default product-mode play must not require the player to type `/roll`, report a d20/d10/d6 result, or provide a damage total. Mark it as a serious referee/UX failure if the GM waits for player dice after a clear fictional action under system_rolls_visible. Manual-dice compatibility may be tested only in explicitly labeled scenarios with `allow_manual_roll_input=true`; those tests are not evidence that the default product loop is good.

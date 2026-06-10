# v1.10.1 Simulation-Guided Hotfix

This hotfix converts the manual LLM trace for the Homecoming opening into concrete route invariants.

## Fixed paths

- Technical risk assessment (`判断能不能安全切断`, `分析是否安全`, `will it trigger`) is forced into an agentic player-roll check before narration.
- Enemy-initiated frame start opens an immediate required reaction gate.
- Required reaction gate reprompts emit `reaction_window_opened` and `done.reason = awaiting_required_reaction`, so user-facing clients can display the blocker as a reaction window rather than a generic prompt.
- Terminal/de-escalation intent can still supersede the required reaction gate and route to the frame exit reducer.
- Direct incoming attacks are prioritized over ordinary object intents in the turn reducer. “It fires at me, I want to cut the cable” must answer the attack first.

## Still outside scope

- Real contest/opposition kernel.
- Damage/armor/HP patch validator.
- Full semantic record/replay in `trpg-harness`.
- True module/rulebook stat extraction for every NPC/card.

The golden trace at `harness/scenarios/homecoming_opening_semantic_trace.json` should be used as the seed for a future `trpg-harness simulate` subcommand with DB-level assertions.

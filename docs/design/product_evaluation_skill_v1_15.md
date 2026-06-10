# v1.15 Product Evaluation Skill

## Purpose

v1.15 adds a Claude-Code/subagent-oriented evaluation skill for testing chatrpg as a full single-player LLM-GM TRPG product. The skill exists because feature tests and phase-based harnesses can miss product failures: a GM can emit events while still asking the player for rule values, forgetting HP, leaking hidden module facts, or blocking clear actions with stale gates.

## Evaluation philosophy

The product promise is not “the Rust code emits phases.” The product promise is:

> Upload rulebooks and modules, create a character, and play a coherent, rule-aware, fun solo TRPG session with an LLM GM.

The evaluator therefore combines:

- deterministic build/harness checks,
- black-box player experience tests,
- GM/referee quality tests,
- DB/event state persistence checks,
- scenario/clue progression tests,
- NPC/world simulation tests,
- cross-ruleset mechanical probes,
- and fun/UX scoring.

## Files

```text
.claude/skills/chatrpg-product-evaluator/SKILL.md
harness/evaluation/product_rubric_v1.json
harness/evaluation/evaluation_report_template.md
harness/evaluation/strange_player_action_bank.json
prompts/evaluation/trpg_product_evaluator.md
prompts/evaluation/turn_evaluator.md
schemas/product_evaluation_report.schema.json
```

## How to use

In Claude Code, ask a main agent or subagent to use the `chatrpg-product-evaluator` skill. The skill instructs the agent not to stop at functional tests, and to generate both human and machine-readable evaluation reports.

Recommended command path:

```bash
cargo check
cargo run -p trpg-cli -- migrate
cargo build -p trpg-cli -p trpg-harness
cargo run -p trpg-harness -- playtest \
  --scenario harness/scenarios/homecoming_external_playtest_8turn.json \
  --bin ./target/debug/trpg \
  --cwd . \
  --evaluator none \
  --output text
```

Then the evaluator reviews artifacts and writes `evaluation_report.md` and `evaluation_result.json`.

## Scoring model

The rubric totals 100 points:

- End-to-end product journey: 12
- GM as referee: 16
- Rules compliance/source grounding: 12
- Module/scenario progression: 12
- NPC/world simulation: 10
- Player UX/fun: 14
- Robustness to weird inputs: 10
- Safety/visibility: 8
- Observability/reproducibility: 6

Thresholds:

- `< 70`: not product-playable
- `70–84`: prototype playable
- `85–91`: good table experience
- `92+`: release candidate

## Key design choices

1. **State over narration.** Effects must be verified through events/DB snapshots, not only GM text.
2. **Referee over novelist.** The GM is evaluated on rulings, rules ownership, state updates, and consequences.
3. **Situation over plot.** The GM should support player agency and consequences rather than forcing a fixed script.
4. **Redundant clues.** Investigation scenarios are evaluated for multiple clue paths and no single-roll dead ends.
5. **NPC agency.** NPCs need goals, changing tactics, morale, and memory.
6. **Weird player robustness.** Creative and unusual actions should be adjudicated with risk/cost, not rejected or menu-locked.


## v1.15.1 additions: human-player realism and debug directives

Claude Code/subagents tend to write like testers: long bundled prompts, JSON snippets, function names, DB table names, phase assertions, or internal Rust types. Product playtests must instead preserve a black-box human-player perspective. v1.15.1 adds a human-player input lint and skill guidance requiring ordinary player language.

The harness also supports test-only debug directives:

```text
[debug]add {"id":"npc.example","kind":"npc","hp":20}[/debug]
[debug]patch {"id":"npc.example","patch":{"hp":5}}[/debug]
[debug]delete {"id":"npc.example"}[/debug]
```

Debug blocks are stripped before player text is sent to the GM. They are recorded as artifacts and injected as GM-only test context so rare actors/items/clues/resources can be tested without waiting for the module to naturally introduce them. The remaining player action still has to be human-like.

v1.15.2 updates this stance: default product play is action-only. The evaluation skill now treats player dice commands or reported roll/damage totals as non-human product-play input unless the scenario explicitly opts into manual-roll compatibility. A product-quality solo TRPG should infer mechanical needs from fictional action, roll automatically under `system_rolls_visible`, apply effects, persist state, and then ask for the next meaningful player decision.

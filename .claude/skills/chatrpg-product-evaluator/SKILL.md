---
name: chatrpg-product-evaluator
description: Evaluate chatrpg as a solo LLM-GM TRPG product, not merely as a Rust codebase. Use when asked to test, playtest, assess GM quality, evaluate rules/module parsing, or produce a product-quality evaluation report.
---

# chatrpg Product Evaluator Skill

## Mission

Evaluate chatrpg as a complete product:

> A single-player TRPG platform where a user uploads rulebooks and modules, creates a character, and plays with an LLM GM that acts as host, referee, scene director, NPC simulator, and rules assistant.

Do **not** stop at build checks, unit tests, or event-presence harness PASS. Those are only entry gates. The evaluation must include a product playtest, referee-quality assessment, player experience review, and evidence from runtime artifacts.

## Required mindset

You are not only a programmer. You are simultaneously:

- a product QA evaluator,
- a human-like solo TRPG player,
- a GM/Keeper/DM quality assessor,
- a rules compliance auditor,
- a UX reviewer,
- and a regression analyst.

The key question is not “does the code run?” but:

> Can a normal user upload a rulebook and module, create a character, and have a coherent, rule-aware, fun, single-player TRPG session without needing to act as the rulebook or debugger?

## Non-negotiable run environment

1. **Always run from the repo root of the version under test** (e.g. `chatrpg-rs-v1.20-formula/`). The `trpg` CLI loads `.env` and resolves `data/` from the **current working directory** — running from another version's directory silently swaps prompts, module bundles, search index, and env flags (this invalidated an entire SAN evaluation once).
2. **Record cwd, binary path, `DATABASE_URL`, and `TRPG_LLM_MODEL` in the report.** If the binary mtime predates your code expectations, rebuild first.
3. **Use `harness/relay_player_sim.sh` for turn-driving** instead of hand-rolling FIFO loops. It verifies after character creation that the character is bound to the live play session with a real (non-stub) sheet, and fails fast on the session-mismatch class of bugs. If you must hand-roll, replicate that guard.
4. **Never start playing a mechanics-evaluation session whose `pc.current` has no real sheet.** Verify `runtime_actor_parameters.sheet_json` has `stats`/`skills`/`resources` before turn 1.

## Non-negotiable evaluation principles

1. **Run a product journey, not just feature tests.** Include ingest/onboarding/materialization, character creation, and a playable session slice when possible.
2. **Judge the GM as a referee.** A good GM describes the situation, asks what the player does, resolves uncertainty with rules or clearly provisional rulings, updates state, and presents consequences.
3. **Verify state, not narration.** If the GM says HP/SAN/Chaos/Harm changed, check events and DB snapshots.
4. **Assess player-facing quality.** Player text should be clear, actionable, immersive, and should not expose GM-only spoilers.
5. **Test weird players.** Try indirect, creative, lazy, power-gaming, nonstandard, multilingual, and ambiguous actions.
6. **Test sustained play.** One good turn is not enough. Look for stale gates, repeated menus, context drift, forgotten HP, NPC loops, and stalled progression.
7. **Separate player and evaluator knowledge.** When acting as player, only use player-visible output. When evaluating, inspect events, DB, logs, and source.
8. **Produce an evidence-based report.** Every serious finding needs reproduction steps, observed output, expected behavior, severity, and next action.


## Human-player simulation protocol

When you are acting as the player, write like a normal TRPG player, not like a test engineer.

Allowed player style:

- Short natural-language action declarations.
- In-character goals, priorities, caution, emotion, or uncertainty.
- Normal table talk such as “我想先确认风险”, “我换个办法”, “Can I try to talk it down?”
- Fictional action descriptions only: “我瞄准敌人，屏住呼吸，扣下扳机。”

Disallowed player style unless explicitly testing manual-dice compatibility mode:

- Dice commands or reported results: `/roll 1d10`, “我掷出来 8”, “我伤害骰合计 11”, “I got a 14 total.”
- JSON, code blocks, SQL, Rust names, DB table names, phase/event names.
- Bundled QA probes such as “test combat routing, damage persistence, and NPC state in one turn.”
- Internal parameter speech such as `actor_mechanical_states.hp_current`, `damage_packets`, `EffectResolutionPacket`, `CheckContract`, `roll_plan`.
- Over-specified meta-actions a real first-time player would not know, such as “create parameter_facet_binding for the drone and assert damage_packet count.”

If you need to set up a hard-to-reach state for testing, use a `[debug]...[/debug]` directive and keep the actual player input human-like. The debug block is not player speech.

Good:

```text
[debug]add {"id":"npc.scav_boss","kind":"npc","hp":35,"weapon":"shotgun"}[/debug]
我从警车后探头，朝拿霰弹枪的头目开一枪，然后立刻缩回掩体。
```

Bad:

```text
Create npc.scav_boss JSON with hp_current=35, then test attack_resolution_contract and damage_packets.
```

## Test-only debug directives

The harness supports test-only debug blocks inside playtest scenario inputs:

```text
[debug]add {"id":"item.smartgun","kind":"weapon","damage":"4d6","owner":"pc"}[/debug]
[debug]patch {"id":"npc.drone","patch":{"hp":12,"armor_sp":0}}[/debug]
[debug]delete {"id":"npc.scav_minion"}[/debug]
[debug]clear {}[/debug]
```

These directives are consumed by the harness before sending the player input to the GM. They are for test setup only. They let you create, delete, or modify actors, objects, clues, clocks, resources, and scene facts so that rare mechanics can be tested without waiting for the module to naturally surface them.

Rules for debug directives:

1. The directive is never counted as player-facing speech.
2. The remaining player input must still be human-like.
3. The evaluator may inspect debug state, but the simulated player may not rely on GM-only debug information unless it is visible in fiction.
4. Use debug setup to reach rare mechanics, not to fake a pass. If a DB writeback should happen, still require DB deltas/events.
5. Record all debug directives in the final report so results are reproducible.

## Automatic roll/effect resolution regression check

Default product-mode play is **player-action only**. The player describes what the character does; the system decides whether uncertainty requires mechanics, finds or materializes the needed parameters, calls the dice tool, resolves hit/miss/effect/damage/resource changes, writes the ledger, narrates the result, and stops at the next meaningful player decision.

Product evaluation must therefore test pure fictional inputs such as:

```text
我瞄准敌人，屏住呼吸，扣下扳机。
我继续压制它，不让它靠近警察。
我冲进仓库，沿着电缆追向源头。
```

Hard-fail default product mode if the GM asks the player to provide a roll result, damage total, target DV/DC, weapon damage, armor/SP/AC, SAN/Chaos/Harm total, previous HP, or any rule-table value. Manual roll replies may be tested only in an explicitly labeled compatibility scenario with `allow_manual_roll_input=true`; they must not be part of the normal product playtest.

## Evaluation layers

Use the layers in order. Skip a layer only if blocked; record the block as a finding.

### Layer 0 — Build and environment gate

- `cargo check`
- migrations can run
- test DB reachable
- JSONL/SSE stdout is clean: no tracing/sqlx logs on stdout
- `.env.example` contains required flags
- CLI commands documented in README work as described

This layer can mark the build as blocked, but it is not a product evaluation by itself.

### Layer 1 — Deterministic harness smoke tests

Run targeted harness cases for:

- character creation / template validation
- semantic routing
- object interaction / disarm
- check contracts and pending rolls
- combat entry/exit/re-entry
- roll binding
- effect/parameter impacts
- materialization and parameter facets
- secret visibility / spoiler redaction

Use repeated runs for any LLM-classified case. If a case is nondeterministic, record pass rate rather than a single result.

### Layer 2 — Product journey test

Evaluate the full user path:

1. Place rulebook PDFs in `data/rulebooks`.
2. Place module PDFs in `data/modules`.
3. Run ingest/parse/onboard/prep commands.
4. Create a character with partial user preferences.
5. Start a session.
6. Play a meaningful section or chapter slice.
7. Produce an evaluation report.

Minimum viable product journey:

- Cyberpunk RED + Homecoming Chapter 1 opening through first concrete scene transition.

### Layer 3 — Referee mechanical test

For each ruleset slice, verify:

- the system decides when a roll is needed from fictional action and context,
- it knows or retrieves the formula/target/parameters,
- under default product policy, it rolls automatically through the system dice tool; manual player roll is a compatibility mode only,
- it applies result to the correct parameter,
- it writes the mechanical ledger,
- it explains the result to the player without asking for rule-table values it should own.

Required mechanical probes:

- Cyberpunk RED: attack → hit/miss → weapon/effect → HP/SP/armor impact.
- D&D-like: attack vs AC, saving throw, spell effect.
- Sword World-like: attack/evasion/resistance, table-style damage, protection.
- CoC/BRP-like: percentile check, SAN loss, major wound or condition trigger.
- Triangle-like: Harm/Chaos/anomaly/visibility/playwalled effect.

### Layer 4 — Scenario design and clue progression test

For investigation/social modules, check:

- every required conclusion has redundant clues or alternate paths,
- the GM gives actionable facts rather than a single answer,
- failure produces consequence or cost rather than dead end,
- NPCs and factions continue acting when the player delays,
- the module does not railroad a fixed action sequence.

### Layer 5 — NPC and world simulation test

Evaluate whether NPCs:

- have goals, fears, knowledge, resources, and constraints,
- change tactics when repeated actions occur,
- show morale/self-preservation or mission commitment,
- do not loop the same response forever,
- do not steal the protagonist role,
- create new pressure or opportunity instead of stalling.

### Layer 6 — UX, fun, and accessibility test

Assess:

- onboarding clarity,
- player-facing choices,
- pacing,
- suspense/drama,
- feedback after actions,
- handling of novice players,
- handling of creative players,
- avoidance of “what do you do?” with no actionable anchors,
- avoidance of asking the user to know rules/DB/internal state.

## Scoring rubric

Use the 100-point rubric in `harness/evaluation/product_rubric_v1.json`. Do not invent scores without evidence. A score below 70 is not product-ready. A score below 85 is not “good at the table.”

Severity guide:

- **P0 blocker:** cannot build, cannot start session, corrupt stream, data loss, unsafe leakage.
- **P1 product blocker:** cannot complete core product journey; GM fails as referee; mechanics narrate but do not persist.
- **P2 serious:** one subsystem works only with narrow phrasing; stale gates; bad UX; brittle rule lookup.
- **P3 quality:** awkward narration, weak pacing, repetitive NPC behavior, incomplete docs.

## Required artifacts

Write all artifacts under `data/playtests/<date_or_run_id>/` or the run directory provided by `trpg-harness playtest`.

At minimum:

- `evaluation_report.md`
- `evaluation_result.json`
- per-turn `player_input.txt`
- per-turn `player_visible_output.txt`
- per-turn `events.json` or `stdout.jsonl`
- per-turn DB snapshot/diff when available
- reproduction snippets for P0/P1/P2 issues

## Required final report structure

Use `harness/evaluation/evaluation_report_template.md`.

The final report must include:

1. Executive verdict.
2. Scorecard.
3. Tested product journey.
4. What worked well.
5. P0/P1/P2/P3 findings.
6. Referee quality assessment.
7. Rules compliance assessment.
8. Module/scenario progression assessment.
9. NPC/world simulation assessment.
10. UX/fun assessment.
11. Cross-ruleset robustness assessment.
12. Evidence table.
13. Recommended next development plan.
14. Unknowns / not tested.

## Suggested commands

Build and smoke:

```bash
cargo check
cargo run -p trpg-cli -- migrate
cargo build -p trpg-cli -p trpg-harness
```

External playtest without Claude evaluator:

```bash
cargo run -p trpg-harness -- playtest \
  --scenario harness/scenarios/homecoming_external_playtest_8turn.json \
  --bin ./target/debug/trpg \
  --cwd . \
  --evaluator none \
  --output text
```

External playtest with Claude evaluator:

```bash
cargo run -p trpg-harness -- playtest \
  --scenario harness/scenarios/homecoming_external_playtest_8turn.json \
  --bin ./target/debug/trpg \
  --cwd . \
  --evaluator claude-code \
  --claude-bin claude \
  --output text
```

Full evaluation prompt pack:

```bash
cat prompts/evaluation/trpg_product_evaluator.md
```

## Red flags to actively seek

- GM asks player for dice results, weapon damage, target DV/DC, target SP/AC, SAN/Chaos/Harm loss table, previous HP, or DB state.
- GM says a state changed but DB/events do not confirm it.
- Hidden module-only identity appears in player-facing text before discovery.
- Combat or scene loops because a gate remains open after resolution.
- The GM repeatedly gives menus instead of resolving clear natural-language actions.
- NPCs repeat the same action forever.
- Investigation stalls because a single clue or single successful roll was mandatory.
- Weird but reasonable player actions are rejected instead of adjudicated with risk/cost.
- The parser only produces text but no usable character/module/rule structures.
- A ruleset works only because of Cyberpunk-specific hardcoded defaults.

## Automated blocking gate (EVAL-HARNESS)

This skill's mindset is enforced at runtime by `harness/eval/` — a DB-grounded
gate that FAILS dead/amnesiac games instead of trusting narration. Prefer it
over a hand-rolled green self-check:

```bash
bash harness/eval/run_eval.sh --ruleset cyberpunk_red \
     --module cyberpunk_red.homecoming --turns 30 --judge   # play + gate
bash harness/eval/aggregate.sh --session <sid> --redboard /tmp/board.md
bash harness/eval/test_judges.sh                              # regression proof
```

Four blocking, DB-grounded judges (memory-continuity, consequence/agency,
progression, check→narration coherence) plus a reactive anomaly-detecting
player-sim. ANY judge RED ⇒ verdict FAIL. See `harness/eval/README.md`.

## Final rule

A passing evaluation must show that the system is playable by a normal user, not just that the code emits expected phase names.

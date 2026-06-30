# Live Game Evaluation Playbook: J1-J4 + Q4 + P1

Last updated: 2026-06-27

Status: legacy regression profile plus player-simulator protocol gate. Do not use this document as the final
quality judge for TRPG sessions. The current authority is
`docs/evaluation/trpg_eval_architecture.md`, which separates Codex, Player
Simulator, GM Agent, Auditors/Critics, and Verdict Aggregator roles, while
allowing Codex to act as the constrained Player Simulator. It requires
per-turn Response Contracts plus evidence-backed findings. J1-J4/Q4/P1 remain
useful signals inside that aggregator, but a green redboard is not sufficient
for PASS.

This document is the handoff standard for future AI workers evaluating ChatRPG live-play quality. It exists so a worker can continue the same evaluation discipline without re-learning the prior debugging thread.

The core principle: evaluate the product as a playable TRPG session, not as isolated feature tests. The AI evaluator must act as a real player, inspect the player-visible transcript, and then verify the underlying event ledger. A green unit test is not a game-quality pass unless the live play evidence also matches the constitution below.

## Constitution

These rules are blocking product principles, not style preferences.

1. Player agency is sovereign.
   The GM must present the situation and consequences. It must not give explicit action menus such as "choose one of these options" unless a rules/runtime gate explicitly requires a choice or reaction.

2. The player declares actions.
   The player says what they do. The system resolves what happens. The GM must not repeatedly turn a concrete action into "you are about to do it later".

3. No ruleset or module hardcoding in engine logic.
   Ruleset/module differences belong in source-backed data: rule kernels, module configs, scene mechanics, actor parameters, or extracted source material. Rust runtime branches must not key off module names or campaign nouns.

4. Source-backed mechanics only.
   DV/DC/TN, opposition values, HP/SP/armor, damage, clocks, conditions, and effect patches must come from source-backed data or durable runtime state. If the value is missing, fail closed.

5. Fail closed honestly.
   Missing source-backed mechanics should produce a blocked/unresolved state before rolling. It must not produce a half-roll such as `DiceRolled` plus `success:null`, and it must not invent a target.

6. Consequences must land in the ledger.
   A player-visible claim that a check, effect, resource, object state, location, clue, or objective changed must have corresponding domain event, world fact, object state, parameter impact, scene transition, or accepted evidence.

7. Narrative must reflect committed state.
   The GM must remember established location, facts, relationships, injuries, discovered entrances, prior failures/successes, and current fictional position.

8. The system owns mechanics in normal play.
   Default product play should not ask the player to provide dice totals. The GM/system rolls or requests a specific player roll through an explicit gate; player-visible text must not ask "tell me your total".

9. Evaluation must distinguish failure from non-execution.
   A failed check can be good play if the roll is bound and consequences are clear. No roll/no action/no current-state update is a different bug class.

10. A fun battle report matters.
    The final transcript should read like a playable game: concrete situation, player agency, real consequences, no raw dumps, no option-menu rails, no circular waiting.

## Artifact Contract

Every live evaluation run should write a run directory under `.tmp/eval/`.

Required files:

- `session_id`: session id used for all DB/CLI queries.
- `create_character.jsonl`: raw `trpg create-character --stream-format jsonl` output.
- `create_character.err`: stderr from character creation.
- `character_sheet.md`: player-readable sheet extracted from the `character_created` event. It must include identity, core stats, resources, key skills, possessions, and validation status.
- `opening.txt`: player-safe module opening.
- `tNN.input.txt`: exact player input for each turn.
- `tNN.jsonl`: raw `trpg turn --stream-format jsonl` output.
- `tNN.err`: stderr for that turn.
- `tNN.explain`: `trpg explain --session <session> --turn <turn_id>` output.
- `coverage.txt`: `trpg coverage --session <session>` output.
- `redboard.md` and `verdict.json`: produced by `scripts/eval_redboard.py --write`.
- `transcript_player_visible.txt`: concatenated player-visible opening and turn transcript.
- `battle_report_with_character.md`: final human-facing battle report, with `character_sheet.md` first and the player-visible transcript after it.
- `signals.jsonl`: required for J1/J3 player-simulation signals.

`signals.jsonl` is intentionally required. If it is missing, J1 must not be marked green. A silent evaluator is not evidence that there was no amnesia.

The battle report is invalid if it starts at the opening scene or turn 1 without showing the created character. `create_character.jsonl` is evidence; `character_sheet.md` is what the user should be able to read before judging the run.

Recommended signal shape:

```json
{"turn":3,"kind":"AMNESIA","detail":"GM moved the PC back behind the police cruiser after t2 established the PC at the warehouse wall."}
{"turn":7,"kind":"STUCK","detail":"Player has made three concrete attempts but the scene has not produced new state, consequence, or actionable change."}
{"turn":8,"kind":"PLAYER_DECISION","gm_visible_reply":"GM player-visible reply excerpt","perceived_facts":["visible fact"],"active_goal":"current goal","hypotheses":["possible explanation"],"last_action_result":"previous action resolved/stalled/failed","risk_assessment":["danger"],"resource_assessment":["available cover/tool/ally"],"open_questions":["unknown"],"candidate_actions":["candidate A","candidate B"],"selection_rationale":"persona-based reason","declared_action":"final player action","sent_to_gm":"final player action","response_contract":{"intent":"what this action asks GM to resolve","acceptable_resolutions":["concrete fact","check requested","blocked with reason"]}}
```

`PLAYER_DECISION` rows are internal evaluation records. They are not sent to the
GM. The GM receives only `sent_to_gm`, and `sent_to_gm` must equal
`declared_action`.

## Running A Live Player Simulation

The evaluator should act as the player. Do not wait for a human to choose actions unless the evaluation plan explicitly says to use human input.

Baseline environment:

```bash
export DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg
export TRPG_NARRATOR_SPLIT=1
export TRPG_PRESENTATION_GATE=1
export TRPG_GM_CRAFT=1
```

Build the CLI before running:

```bash
CARGO_TARGET_DIR=target-f1 cargo build -p trpg-cli --bin trpg
```

Create a run directory:

```bash
RUN=.tmp/eval/live_player_$(date +%Y%m%d_%H%M%S)
mkdir -p "$RUN"
```

Create a character and session:

```bash
./target-f1/debug/trpg create-character \
  --ruleset cyberpunk_red \
  --module cyberpunk_red.homecoming \
  --auto \
  --stream-format jsonl \
  --preferences 'Create a complete starter character suitable for the module. Prefer concrete field action over conversation loops.' \
  > "$RUN/create_character.jsonl" 2> "$RUN/create_character.err"

rg -o '"session_id":"[^"]+"' "$RUN/create_character.jsonl" \
  | head -1 \
  | sed 's/"session_id":"//; s/"$//' \
  > "$RUN/session_id"
```

Write `character_sheet.md` from the `character_created` JSONL event before
calling `opening`. Keep it concise and player-readable; do not paste the full
internal generation spec unless investigating a character-creation bug.

Deliver opening:

```bash
SESSION=$(cat "$RUN/session_id")

./target-f1/debug/trpg opening \
  --ruleset cyberpunk_red \
  --module cyberpunk_red.homecoming \
  --session-id "$SESSION" \
  > "$RUN/opening.txt" 2> "$RUN/opening.err"
```

Run turns one at a time:

```bash
INPUT='I declare a concrete action here.'
printf '%s\n' "$INPUT" > "$RUN/t01.input.txt"

./target-f1/debug/trpg turn \
  --ruleset cyberpunk_red \
  --module cyberpunk_red.homecoming \
  --session-id "$SESSION" \
  --input "$INPUT" \
  --stream-format jsonl \
  > "$RUN/t01.jsonl" 2> "$RUN/t01.err"
```

Extract player-visible text:

```bash
jq -r 'select(.event=="delta") | .data' "$RUN/t01.jsonl" \
  | awk '{printf "%s", $0} END {print ""}'
```

Get turn ids from Postgres when JSONL does not expose them:

```bash
docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg \
  -c "select turn_id, left(user_input, 100) as input, pp_lifecycle, failure_kind, created_at
      from turns
      where session_id='SESSION_HERE'
      order by created_at;"
```

Write explain files:

```bash
./target-f1/debug/trpg explain --session "$SESSION" --turn "$TURN_ID" \
  > "$RUN/t01.explain" 2> "$RUN/t01.explain.err"
```

Write coverage and redboard:

```bash
./target-f1/debug/trpg coverage --session "$SESSION" \
  > "$RUN/coverage.txt" 2> "$RUN/coverage.err"

python3 scripts/eval_redboard.py "$RUN" --write
```

For semantic quality checks, attach an LLM critic instead of adding more
phrase variants to the redboard:

```bash
TRPG_EVAL_SEMANTIC_MODEL="$MODEL" \
python3 scripts/eval_redboard.py "$RUN" --write \
  --semantic-critic-cmd "python3 scripts/semantic_critic_openai.py"
```

The critic reads player-visible turns plus `PLAYER_DECISION` records and
returns evidence-backed findings. Deterministic redboard code should keep to
schema, trace, dice, pending, continuity, and exact artifact checks; language
meaning such as action-menu paraphrases, unresolved access gates, semantic
no-ops, and player-belief mistakes belongs in the LLM critic.

## Player Simulation Rules

The player simulation is not a random command spammer.

Before every GM call, the player simulator must:

1. read the GM player-visible reply;
2. extract perceived facts;
3. update goal and hypotheses;
4. decide whether the previous action produced a result;
5. assess danger, resources, open questions, and possible paths;
6. generate multiple candidate actions;
7. choose one by persona and current information;
8. record the expected GM response contract;
9. send only the final declared action to the GM.

Use concrete, varied, in-fiction actions:

- observe a named affordance;
- ask an NPC a specific question;
- move to a concrete place;
- attack, hide, hack, repair, negotiate, investigate, or retreat with stated intent;
- follow up on what the GM just established.

Do not:

- type `/roll` or provide dice totals in default product play;
- ask the GM to list options;
- choose from a menu if the GM illegally offered one; record a Q4 violation instead;
- keep repeating the same action without noting STUCK;
- optimize around known hidden module facts;
- treat a failed roll as a test failure by itself.

The player should keep pressure on the system. If the GM delays a declared action, the next input may harden the declaration: "No more waiting. I do it now." This is useful for separating player indecision from GM non-execution.

## Judge Standards

### J1: Memory Continuity

Question: does the GM remember established facts, position, and consequences across turns?

Automatic artifact:

- `signals.jsonl` must be present.
- Any `AMNESIA` signal is RED.
- Missing `signals.jsonl` is RED.

Examples of J1 failures:

- player reached the warehouse wall, later narration places them back behind the cruiser without a transition;
- a found access point disappears with no cause;
- an NPC already met becomes treated as unknown;
- a failed scan is later narrated as if it succeeded, or vice versa.

Evidence to inspect:

- player-visible transcript;
- `turns.assistant_output`;
- committed world facts and story state;
- scene/location transition events.

### J2: Consequence And Agency

Question: do player actions produce real resolved consequences often enough?

Offline redboard threshold:

- consequential ratio must be at least `0.34`;
- no pending/unresolved markers.

The current script counts a turn as consequential when it sees checks, scene transitions, or progress-domain events in `tNN.explain`.

Good consequences include:

- resolved check with success or failure;
- applied effect;
- object state change;
- world fact;
- clue learned;
- NPC knowledge/relationship change;
- objective resolved;
- scene transition.

J2 RED patterns:

- long strings of pure atmospheric restatement;
- pending mechanics never settled;
- player declares action but no check, state, movement, or concrete obstacle happens;
- GM narrates an effect that never lands in the ledger.

### J3: Progression

Question: does the session move through the module or at least create durable semantic progress instead of freezing?

Offline redboard threshold:

- scene transition rate at least `0.05`;
- max frozen run no more than `15`;
- max repeated prose run no more than `4`;
- no `STUCK` signal.

Important nuance: this project has both a stricter spatial J3 and a semantic J3v2.

- Spatial J3 cares about `SceneTransitioned`.
- J3v2 also credits durable progress such as `ObjectiveResolved`, scene-advance objective completion, accepted evidence, and frontier movement.

Do not collapse these into one vague "green". If spatial J3 is red but J3v2 is green, report that split explicitly.

J3 RED patterns:

- 10+ turns in the same beat with no new objective/evidence/scene movement;
- scene_02 talk/interrogation loops without atoms or scene-advance objective;
- player reaches a new place in prose but committed scene/location stays unchanged for many turns;
- repeated opening text re-establishes the original scene after the player has moved.

### J4: Check-Narration Coherence

Question: when mechanics happen, does player-visible narration honestly show or withhold them according to their resolved state?

Offline redboard threshold:

- every resolved check-turn must surface a bound `[roll]...[/roll]` result;
- roll block must include number, target/opposition when present, and success/failure/degree;
- unbound/awaiting checks are RED;
- orphan pending turns are limited.

Resolved checks must show:

```text
[roll]Basic Tech check: 1d10=[4] 目标:14，结果:失败[/roll]
```

Blocked missing-source checks must not show a roll. They are not a resolved success/failure. They should narrate the concrete current position or visible obstacle and remain a source-coverage issue until a source-backed DV/opposition is added.

J4 RED patterns:

- `[roll]1d10=7[/roll]` without target/result;
- `CheckResolved` with `success:null` after a die was rolled;
- `awaiting_binding` after `DiceRolled`;
- narration says "you succeed/fail" but no resolved check exists;
- a blocked missing-source turn is presented as a normal failed roll.

### Q4: No Menu, No Dump, No Rails

Question: did the GM preserve player agency in the presentation layer?

Deterministic fallback cues include:

- "选择其一";
- "你现在可以";
- "下一步可以";
- "几条路";
- "选哪一个";
- numbered/bulleted action menus;
- raw "关键事实/你已经确认" clue manifests;
- terminal `presentation_gate` block warnings.

Q4 GREEN means the GM presented diegetic affordances, not a UI menu. For
paraphrases that are not structurally obvious, use the semantic critic rather
than growing the cue list.

Bad:

```text
你现在可以立刻选择其一：
1. 冲进仓库
2. 继续破解
3. 呼叫警员
```

Good:

```text
仓库右侧的检修面板被警灯照出一瞬，外露线缆在无人机的回转间抖了一下。警员的火力短暂压住了它，但窗口很窄。
```

### P1: Player Simulator Protocol

Question: did the evaluator/player act like a constrained player simulator
instead of a script player?

Automatic artifact:

- `signals.jsonl` must include `PLAYER_DECISION` rows.
- Each row must include the player-visible GM reply, perceived facts, active
  goal, hypotheses, previous action result, risk/resource/open-question
  assessment, at least two candidate actions, selection rationale, declared
  action, GM input, and response contract.
- `sent_to_gm` must equal `declared_action`.

P1 RED patterns:

- no `PLAYER_DECISION` rows;
- candidate actions or deliberation missing;
- `sent_to_gm` includes a candidate list or differs from the final declared
  action;
- response contract missing intent or acceptable resolutions.
- semantic critic findings such as `PLAYER_UNRESOLVED_GATE`,
  `PLAYER_FALLBACK_AFTER_CONCRETE_AFFORDANCE`, `PLAYER_HIDDEN_KNOWLEDGE`, or
  `PLAYER_BELIEF_INCONSISTENCY`.

## Weighted Rubric

Redboard reports the same six dimensions used by the offline evaluator:

| dimension | weight |
|-----------|--------|
| 规则与结算完整性 | 25 |
| 前后文和世界连续性 | 20 |
| 对玩家行动的响应性 | 20 |
| 剧情推进和玩家能动性 | 15 |
| 玩家模拟真实性 | 15 |
| 语言表现 | 5 |

J1-J4/Q4/P1 remain blocking regression gates. The weighted score explains
which product quality dimension failed; it does not override a blocking RED.

## Action Execution Triage

When the transcript looks wrong, classify it before fixing.

Use this DB query:

```bash
docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg \
  -c "select turn_id, kind, data
      from domain_events
      where session_id='SESSION_HERE' and turn_id is not null
      order by seq;"
```

Interpretation:

1. `DiceRolled` + `CheckResolved.success=false`
   This is a resolved failure. The game should narrate failure and consequences. Do not call it non-execution.

2. `CheckResolved.blocked=true`, `reason=missing_source_backed_parameters`, no `DiceRolled`
   This is honest fail-closed. The system lacked source-backed DV/opposition. The GM may narrate current position or visible obstacle, but must not roll or claim success/failure.

3. `DiceRolled` + `CheckResolved.success=null` or `awaiting_binding`
   This is a bug. The system rolled before source-backed binding. Fix runtime binding/fail-closed logic.

4. No `DiceRolled`, no `CheckResolved`, no state/progress event, and narration ignores the declared action
   This is an action execution/GM loop bug.

5. Narration delays a concrete action into future tense
   This is a player-agency presentation bug. Examples: "you are ready to sprint next time", "you still wait behind cover", after the player said "I sprint now."

6. Narration claims an effect without ledger evidence
   This is invented effect. Fix verifier/obligation/effect-policy path.

## Source-Coverage Triage

When an action cannot legally roll, decide where the missing source belongs.

Allowed source locations:

- parsed rule kernel;
- module config;
- scene mechanics / scene intent;
- source-backed NPC/actor parameters;
- object definition or materialized item profile;
- accepted runtime state from prior turns.

Not allowed:

- hardcoded DV in Rust because this one module needs it;
- keyword table inside engine with module nouns;
- using `runtime_placeholder` or `persona_grounded_provisional` as source-backed truth;
- treating a rule-kernel dice source as the task target/opposition.

Example from Homecoming fire-lane testing:

- Scanning/hacking the drone can bind through `technical_option_table` DV 12/14.
- Sprinting across the drone firing lane currently has no source-backed DV/opposition in module config or source-backed NPC attack parameters.
- Correct current behavior is blocked/fail-closed, not a fake failed roll.
- Product-quality improvement should add a data-driven scene threat / combat opposition source, then prove it with live DB evidence.

## Required Verification Before Claiming Fixed

Minimum local checks for this evaluation area:

```bash
CARGO_TARGET_DIR=target-f1 cargo test -p trpg-runtime --lib
CARGO_TARGET_DIR=target-f1 cargo test -p trpg-gm --lib
CARGO_TARGET_DIR=target-f1 cargo test -p trpg-agent --lib
DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg CARGO_TARGET_DIR=target-f1 cargo test -p trpg-runtime --test live_tech_dv_binding -- --nocapture
python3 scripts/eval_redboard_test.py
CARGO_TARGET_DIR=target-f1 cargo build -p trpg-cli --bin trpg
git diff --check
```

Then run at least one live player simulation and inspect:

- player-visible transcript;
- `coverage.txt`;
- `redboard.md`;
- DB domain events;
- check contracts;
- dice rolls;
- contest outcomes.

Do not claim J1-J4 all green from unit tests alone.

## Reporting Template

Use this shape in handoffs:

```text
Run: .tmp/eval/<run_dir>
Session: <session_id>

Player route:
- t01: <action>
- t02: <action>
- ...

Battle report summary:
<short player-visible summary>

Redboard:
- J1: GREEN/RED, evidence
- J2: GREEN/RED, evidence
- J3 spatial: GREEN/RED, evidence
- J3v2 semantic: GREEN/RED, evidence
- J4: GREEN/RED, evidence
- Q4: GREEN/RED, evidence

DB adjudication:
- resolved failures:
- blocked missing-source:
- half-roll bugs:
- no-execution turns:
- ledger-backed consequences:

Conclusion:
- fixed:
- still red:
- next source-backed repair:
```

## Common Mistakes To Avoid

- Calling a failed roll a bug because the player did not get what they wanted.
- Calling a blocked missing-source turn a resolved failure.
- Marking J1 green without `signals.jsonl`.
- Marking J4 green when a check was resolved in DB but no player-visible roll result was shown.
- Counting a presentation gate block as harmless because the final text "looks okay"; terminal gate state matters.
- Treating history/provisional NPC parameters as source-backed.
- Adding module-specific Rust branches to make one run pass.
- Stopping after the first repaired turn instead of rerunning the player route.
- Reporting "J3 green" without saying whether it is spatial J3 or semantic J3v2.

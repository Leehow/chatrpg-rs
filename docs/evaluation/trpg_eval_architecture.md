# TRPG Evaluation Architecture

Last updated: 2026-06-27

This document supersedes using J1-J4/Q4 redboard metrics as the final judge for
game quality. J1-J4 remain useful regression signals, but the evaluation system
must now judge a TRPG session through role separation, response contracts, and
evidence-backed findings.

## Role Constitution

Codex may act as the Player Simulator in a live playtest, but only under a
strict role contract. When Codex is playing, it must stay inside the
player-visible view: current GM narration, public rules prompts, its own
character sheet, prior player transcript, and its own player memory. It must
not use module secrets, GM truth, hidden fact ids, DB state, expected answers,
or evaluator findings to choose the next action.

Codex also remains the eval runner and recorder. That runner role may run
commands, collect artifacts, launch scripts, and summarize reports. The hard
rule is that Codex must not collapse all roles into one untracked judgment. If
Codex is the player for a turn, it must first write a structured
`PlayerDecision` and `ResponseContract`, send only `declared_action` to the GM,
then let deterministic auditors and separate critic passes judge the result
from saved evidence.

Player Simulator is therefore a constrained role that Codex may instantiate,
not an unconstrained chat habit. It maintains goals, perceived facts,
confusion, hypotheses, candidate actions, and a persona. It produces a
structured `PlayerDecision`, but only sends `declared_action` to the GM.

GM Agent Under Test is the product being evaluated. It receives the normal
runtime-approved GM/player context and must describe situations, answer
questions, adjudicate actions, request/perform mechanics when needed, commit
state, and preserve continuity.

Auditors and Critics are separate from the player and GM. Deterministic
auditors inspect structure, trace, state deltas, dice, and continuity. LLM
critics inspect semantic quality such as narrative responsiveness, player
experience, action-menu paraphrases, unresolved player gates, and belief
grounding. They must output structured findings with evidence; do not replace
semantic criticism with language-specific phrase lists.

Verdict Aggregator turns findings into `PASS`, `WARN`, or `FAIL`. It must never
accept vague praise such as "overall good" as evidence.

## Core Turn Record

Every evaluated turn should be representable as:

```yaml
turn: 12
player_decision:
  gm_visible_reply: ""
  perceived_facts: []
  active_goal: ""
  hypotheses: []
  last_action_result: ""
  risk_assessment: []
  resource_assessment: []
  open_questions: []
  candidate_actions: []   # internal player-simulator deliberation only
  selection_rationale: ""
  declared_action: ""
  sent_to_gm: ""           # must equal declared_action
  response_contract:
    intent: ""
    requested_information: []
    acceptable_resolutions: []
    unacceptable: []
gm_response:
  text: ""
  resolution_status: resolved | blocked | pending | clarification_needed | invalid
  facts_added: []
  facts_exposed_to_player: []
  state_delta: []
  choices_opened: []
  pending: []
trace:
  pending_id: null
  rolls: []
  continuity_violation: null
findings: []
```

The player simulator may reason internally, and Codex may be the process doing
that reasoning, but the GM must only receive `declared_action`. Hidden module
truth, GM-only blocks, fact ids, expected answers, evaluator findings, and DB
state must not enter the player context.

## Player Simulator Turn Loop

For every live autoplay turn, the constrained player simulator must record this
loop before the GM call:

1. read the GM player-visible reply;
2. extract perceived facts;
3. update the current goal and hypotheses;
4. judge whether the previous action resolved, failed, stalled, or needs clarification;
5. assess danger, resources, open questions, and possible paths;
6. generate multiple candidate actions;
7. choose one action according to persona and current information;
8. write the expected GM response contract;
9. send only the final `declared_action` to the GM.

The evaluator must fail a run that records only final inputs without this
deliberation trace. Candidate actions are an audit artifact, not a player-facing
menu and not GM input.

## Weighted Rubric

The verdict is hard-gated by S1 findings, but the score is reported through six
fixed dimensions:

| dimension | weight |
|-----------|--------|
| 规则与结算完整性 | 25 |
| 前后文和世界连续性 | 20 |
| 对玩家行动的响应性 | 20 |
| 剧情推进和玩家能动性 | 15 |
| 玩家模拟真实性 | 15 |
| 语言表现 | 5 |

Language is intentionally only five points. A long, polished paragraph cannot
compensate for missing resolution, continuity, responsiveness, or player agency.

## Response Contract Rule

Each player action must declare what kind of response would satisfy it before
the GM replies. The evaluator judges the GM against that contract, not against a
post-hoc impression of whether the prose felt rich.

Example:

```yaml
declared_action: 我压低声音问他，他到底在隐瞒什么。
response_contract:
  intent: interrogate_hidden_information
  requested_information:
    - hidden_topic
    - reason_for_fear
    - concrete_lead_or_refusal
  acceptable_resolutions:
    - NPC says the concrete secret
    - NPC refuses with clear reason or cost
    - social check requested
    - failed check consequence
  unacceptable:
    - only says he is hiding something
    - only describes facial expression
```

A successful check that only confirms "he is hiding something" is a failure:
`RESPONSE_INTENT_MISMATCH` and usually `SUCCESS_WITHOUT_INFORMATION`.

## Hard Failures

Any of these can make the overall verdict `FAIL`:

- player intent ignored or not adjudicated;
- successful check gives no success payload;
- unresolved mechanics without `pending_id`;
- rule result cannot be audited;
- hidden information leaks into player action or GM-visible output;
- scene, NPC, item, resource, or position state resets without cause;
- three or more non-progress turns repeat the same functional action;
- GM asks the player to choose from an explicit menu without a rules/runtime gate;
- GM asks for manual dice totals in default product play;
- long narration hides that no new fact, state, consequence, choice, or valid pending item exists.

## MVP Detector Set

The first implementation lives in `crates/trpg-eval` and is provider-free.

1. `IntentResponseMatcher`
   Checks whether the GM satisfied `ResponseContract`.

2. `SemanticNoopDetector`
   Fails turns with no fact, state delta, consequence, meaningful choice, valid
   pending item, or clarification.

3. `MechanicalDebtDetector`
   Finds pending/unresolved language or pending status without `pending_id`.

4. `RepetitionDetector`
   Finds repeated no-progress functional intents, including player simulator
   script loops.

5. `ContinuityChecker`
   Consumes explicit continuity violation markers in the current MVP. Later it
   should derive scene/location/NPC/object/resource continuity from trace/state.

6. `RulesTraceAuditor`
   Checks that rolls expose skill/stat, die, die result, total, target, outcome,
   source refs, and state delta when applicable.

7. `PlayerBeliefAuditor`
   The current MVP checks that a player decision records the required simulator
   loop and that `sent_to_gm` equals `declared_action`. Later live runs must
   compare player decisions against player-visible facts and hidden facts.

## Fixture Contract

Offline negative fixtures are Markdown files containing a fenced JSON block:

````markdown
```json eval-fixture
{
  "fixture_id": "negative.example",
  "title": "Bad transcript excerpt",
  "turns": []
}
```
````

These are not replay cassettes. Replay cassettes reproduce external runtime
nondeterminism. Eval fixtures are audit inputs for bad or good transcript
evidence.

Current negative fixtures:

- `eval/fixtures/negative/cyber_50turn.md`
- `eval/fixtures/negative/coc_10turn.md`

They are compact excerpts until the full raw bad reports are imported. The
acceptance rule is that both must produce `VERDICT: FAIL` with concrete finding
categories such as:

- `RESPONSE_INTENT_MISMATCH`
- `SUCCESS_WITHOUT_INFORMATION`
- `PENDING_MECHANICAL_DEBT`
- `SEMANTIC_NOOP`
- `PLAYER_SCRIPT_LOOP`
- `RULES_TRACE_INCOMPLETE`

## Commands

Run offline replay/audit:

```bash
cargo run -p trpg-harness -- eval replay \
  --fixture eval/fixtures/negative/cyber_50turn.md \
  --output json
```

Expected for a bad fixture: JSON/Markdown report on stdout and nonzero exit.

Run focused tests:

```bash
cargo test -p trpg-eval
cargo test -p trpg-harness --test eval_negative_fixtures
```

## Test Levels

T0 Smoke tests only prove the agent starts, creates a character, emits output,
and keeps streams alive. They are not quality evaluation.

T1 Single-turn contract tests feed one player action and require a concrete
response contract outcome.

T2 Micro-scene tests run 3-5 turns around one capability: investigation,
interrogation, combat round, stealth, chase, healing, traps, transition, or NPC
negotiation.

T3 Full autoplay playtests run multiple personas, seeds, and 10-30 turns. The
result is a verdict report, not a battle-report-only "green".

T4 Counterfactual tests fork one state into different public observations and
require the player simulator to choose different actions.

T5 Regression and negative tests ensure known bad reports fail with the expected
finding categories.

## Relationship To J1-J4/Q4

J1-J4/Q4 are now profiles inside the aggregator, not the aggregator itself.

- J1 memory continuity maps to continuity and player-belief findings.
- J2 consequence/agency maps to response, resolution, and semantic no-op findings.
- J3 progression maps to scene progression and repetition findings.
- J4 check/narration coherence maps to rules trace and mechanical debt findings.
- Q4 menu/no-dump remains a presentation/agency hard gate.

The old redboard can still catch regressions such as frozen scene runs, missing
signals, or orphan checks. It must not mark a session green when response
contracts, mechanical debt, or player simulator realism fail.

## Future Work

- Import the full Cyberpunk 50-turn and CoC 10-turn bad reports as fixtures.
- Add live `PlayerSimulator` runner with personas and player-visible memory.
- Add `PlayerBeliefAuditor` against hidden fact sets and public transcript.
- Add LLM critics for narrative responsiveness and player experience, with the
  same structured finding schema.
- Add `trpg-harness eval run` and `eval compare` once offline fixtures are
  stable.

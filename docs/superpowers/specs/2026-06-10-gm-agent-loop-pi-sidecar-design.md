# GM Agent Loop Pi sidecar prototype design

Status: design, pending user review (2026-06-10). Decision: build option C first.

Use Pi as a sidecar prototype to validate a real GM agent loop, then migrate the
proven loop back into Rust. This is intentionally not a rewrite of the whole
runtime. It is an experimental path that lets the product test whether a
tool-using GM loop fixes the two current high-impact failures:

1. The LLM says a check is needed, but no `CheckContract` is created or rolled.
2. A check is rolled, but the required effect or parameter impact is not applied.

## 1. Context

The current architecture is effectively "Rust referee + LLM narrator":

- Rust owns routing, gates, check contracts, dice, effects, state writes, and
  audit tables.
- The LLM receives compiled BP1/BP2/BP3 context and writes player-facing prose.
- `TurnOrchestrator`, `CombatAgent`, object/ability/materialization kernels, and
  the generic `GmAgent` are separate pre-narration producers of context and
  contracts.

That design is safer than free-form narration, but the turn still has split
authority. The final LLM message can assert a mechanical obligation or outcome
that was not actually committed. Rust can also execute a check/effect path that
the final narration omits or contradicts.

The missing abstraction is a single **turn authority**: a loop that keeps asking
"what tool or state transition is needed next?" until the turn is mechanically
complete, then verifies the final narration against the turn ledger.

## 2. Why Pi, and why only as sidecar first

Pi is useful as a prototype harness because it already has:

- a minimal agent loop with tool calling and state management;
- JSON/RPC mode over stdin/stdout for non-Node integrations;
- TypeScript extensions that can register custom LLM-callable tools;
- extension events that can inject context, intercept tool calls, and persist
  session state.

References:

- Pi mono repo: https://github.com/earendil-works/pi
- Pi RPC docs: https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/rpc.md
- Pi extension docs: https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md
- Pi site: https://pi.dev/

Pi is not the production authority. It has no built-in strong permission
boundary; the repo documents that it runs with the permissions of the launching
process unless sandboxed. Also, Pi is a general coding-agent harness, not a TRPG
referee. The durable product boundary should remain Rust/PostgreSQL/SSE/harness.

Therefore:

- Prototype with Pi sidecar to validate the agent loop behavior quickly.
- Keep Rust as the only trusted executor for rules, dice, effects, state, and
  output acceptance.
- Once the tool schemas and verifier prove useful, reimplement the same loop as
  a Rust-native `GmAgentLoop`.

## 3. Non-goals for the prototype

- Do not replace the parser, search, materialization, object, ability, combat,
  contest, or effect executor.
- Do not let Pi write project files or mutate runtime state except through
  registered TRPG tools.
- Do not make Pi the long-term production runtime dependency.
- Do not solve all route classification issues in this slice. The goal is
  turn-loop consistency: checks/effects mentioned in output must correspond to
  committed tool/state facts.
- Do not expose arbitrary MCP or shell access to game turns.

## 4. Architecture

Experimental flow:

```text
CLI/API turn handler
  -> Rust preflight
     - world time
     - live derived refresh
     - player action world event
     - gate reconciliation
     - hard gate resolution/reprompt if needed
  -> prepare_turn_context
  -> GmAgentLoopSidecar
     - spawn Pi RPC process
     - provide one-turn snapshot and tool host URL
     - Pi extension registers TRPG tools
     - agent loops over tool calls and observations
  -> Rust TurnLedger
  -> Rust NarrationVerifier
  -> accepted player-visible narration
```

The sidecar is enabled only by explicit flag/env:

```text
TRPG_GM_AGENT_LOOP=pi_sidecar
TRPG_GM_AGENT_LOOP_MAX_STEPS=12
TRPG_GM_AGENT_LOOP_MAX_REPAIRS=3
TRPG_GM_AGENT_LOOP_TIMEOUT_MS=120000
```

Optional CLI spelling:

```text
trpg turn --agent-loop pi
trpg play --agent-loop pi
```

Default remains the existing path.

## 5. Process boundary

Rust starts a one-turn local tool host and Pi sidecar:

```text
RuntimeEngine
  starts AgentToolHost on 127.0.0.1:0
  generates one-turn bearer token
  copies or resolves the Pi extension to an absolute sidecar path
  spawns:
    pi --mode rpc --no-session
       --name "chatrpg-gm-turn-<turn_id>"
       -e <absolute_sidecar_extension_path>
```

The tool host is HTTP because it is the simplest boundary between Rust and a
TypeScript Pi extension. Every request carries:

```text
Authorization: Bearer <one_turn_token>
X-TRPG-Session: <session_id>
X-TRPG-Turn: <turn_id>
```

The tool host refuses:

- wrong token;
- wrong session/turn;
- calls after turn finalization;
- calls that attempt to mutate state outside the current session/turn;
- tools not on the allowlist.

Pi should run in a temporary empty cwd, not the repository root. Only the
extension and provider credentials needed for the model should be visible. This
does not make Pi production-safe, but it reduces prototype blast radius.

## 6. Agent prompt contract

The sidecar prompt is a protocol, not an ordinary GM prompt. It tells the model:

- You are not allowed to state a check, roll, damage, resource spend, parameter
  impact, or state change as true unless a TRPG tool returned it.
- If the fiction needs a check, call `propose_check` and then `execute_check`.
- If an effect is implied by a result, call `apply_effect` or consume the effect
  returned by `execute_check`.
- If player input is required, call `open_player_gate`.
- Before finishing, call `submit_final_narration`.
- If `submit_final_narration` returns rejected, revise by calling tools or
  resubmitting narration. Do not output rejected text to the player.

The final assistant text from Pi is ignored unless Rust has an accepted
`submit_final_narration` result. The player-visible output is the accepted text
stored in Rust's ledger.

## 7. Tool set

### `inspect_turn_state`

Returns a compact snapshot:

- session/ruleset/module/turn ids;
- active scene and world time;
- active frame summary;
- open gate/pending check summary;
- player input;
- current compiled context hashes;
- visible recent transcript summary;
- current turn ledger entries.

This is the safe "observe" tool. It is always allowed.

### `retrieve_rule`

Inputs:

```json
{"query":"string","limit":4}
```

Calls `RuntimeEngine::retrieve_rules` against the parsed search service. Returns
source-backed snippets and page/source refs. It does not commit state.

### `run_route_kernel`

Inputs:

```json
{"route_hint":"auto|ability|object|combat|director|generic_check","reason":"string"}
```

Runs the existing Rust kernels for the current player input using the current
`TurnOrchestrationResult`. This lets the prototype reuse today's
materialization/object/ability/combat/director logic instead of making Pi invent
all routing from scratch.

The tool records any produced contracts, gates, roll executions, effect
contracts, context blocks, and narration context into the turn ledger.

This tool is idempotent per route per turn. A second identical call returns the
existing ledger delta instead of rerunning state mutations.

### `propose_check`

Inputs:

```json
{
  "action_summary":"string",
  "check_label":"string",
  "intent_kind":"string",
  "dice_expression_hint":"string|null",
  "target_model_hint":"static|unknown|opposed|degree_only|dice_pool",
  "target_value": "number|null",
  "tested_parameter": "string|null",
  "target_actor_id": "string|null",
  "stakes": {
    "before_roll_public":"string",
    "success_public":"string",
    "failure_public":"string"
  },
  "source_refs": []
}
```

Rust validates and materializes this into a `CheckContract` or rejects it with
specific missing-source/materialization reasons.

Validation rules:

- no unsourced hard target value unless the ruleset kernel or retrieved source
  supports it;
- no invented weapon damage or defense parameter;
- no player-supplied mechanical totals as trusted facts;
- no hidden facts in public stakes;
- actor and target ids must exist or be created through an allowed runtime
  hydrator.

Successful proposals are persisted as `check_contracts` with status `proposed`
or `created` depending on existing schema constraints. They are not rolled until
`execute_check`.

### `execute_check`

Inputs:

```json
{"check_id":"string","roll_policy":"system_visible|private_gm|passive"}
```

Normalizes the contract through the current table dice policy, then uses the
existing Rust execution boundary:

- `execute_system_roll_bundle` for system-visible roll chains;
- `execute_agent_roll` for a single private/passive roll;
- contest/opposition resolver for outcome;
- referee combat hook for follow-up effects;
- object rule effects hook.

Outputs:

- primary `CheckResultRecord`;
- follow-up results;
- dice event summaries;
- committed patches and parameter impacts;
- unresolved effect warnings.

If an effect follow-up is required but cannot be applied, the tool returns
`requires_effect_resolution` and leaves the ledger incomplete. The agent must
call `apply_effect` or return a player gate.

### `apply_effect`

Inputs:

```json
{
  "source_check_id":"string|null",
  "effect_kind":"damage|resource|condition|object|clock|world_fact|other",
  "target_kind":"actor|object|scene|session",
  "target_id":"string",
  "parameter_path":"string|null",
  "amount":"number|null",
  "description":"string",
  "source_refs": [],
  "visibility":"player_visible|gm_only|system_only"
}
```

Rust validates the target and parameter path. For mechanical tracks, it writes
through existing parameter/facet/effect services and records `ParameterImpact`
or a typed `StatePatch`. It refuses "HP damage", "SAN loss", "Chaos gain",
"armor reduction", etc. when the target parameter cannot be resolved.

Prototype restriction: freeform `apply_effect` is allowed only for effects tied
to an existing `CheckResultRecord`, clock tick, world event, or object
interaction. It cannot silently invent consequences unrelated to the turn.

### `open_player_gate`

Inputs:

```json
{
  "gate_kind":"player_roll_required|required_reaction_choice|optional_reaction_window|confirm_risky_action|choose_action_mode|select_target|resolve_ambiguous_intent",
  "prompt_public":"string",
  "required":true,
  "options":[],
  "bound_action_summary":"string",
  "source_refs":[]
}
```

Creates an `InteractionGate`, attaches it to the active frame when appropriate,
stores it in the turn ledger, and returns a terminal turn state. Once this tool
is accepted, final narration must be exactly the player-facing gate prompt plus
any allowed system note.

### `inspect_ledger`

Returns the canonical current turn ledger:

- check contracts proposed/executed;
- dice rolls;
- check results;
- effect contracts;
- parameter impacts;
- state patches;
- gates opened/resolved;
- tool calls and warnings;
- verifier findings so far.

### `submit_final_narration`

Inputs:

```json
{
  "player_visible_text":"string",
  "mechanical_claims":[
    {"kind":"check|roll|damage|resource|condition|object|clock|state","text":"string"}
  ],
  "referenced_ledger_ids":["string"]
}
```

This is the only accepted finish path. Rust runs `NarrationVerifier` and returns:

```json
{
  "accepted": true,
  "final_narration_id": "narration_...",
  "stored_text": "..."
}
```

or:

```json
{
  "accepted": false,
  "findings": [
    {"kind":"missing_check","severity":"blocker","detail":"..."}
  ],
  "next_required_action":"call execute_check|call apply_effect|revise_text"
}
```

Rejected text is never sent to the player.

## 8. Turn ledger

The ledger is the single source of truth for the sidecar turn. It can be backed
by existing tables plus two prototype tables.

New tables:

```text
gm_agent_loop_runs
  run_id text primary key
  session_id text not null
  turn_id text not null
  ruleset_id text not null
  module_id text
  backend text not null          -- pi_sidecar, scripted, rust_native_later
  status text not null           -- running, accepted, rejected, timeout, fallback, error
  request_json jsonb not null
  final_narration text
  verifier_result_json jsonb not null default '{}'
  created_at timestamptz not null default now()
  updated_at timestamptz not null default now()

gm_agent_loop_steps
  step_id text primary key
  run_id text not null
  session_id text not null
  turn_id text not null
  step_no integer not null
  role text not null             -- model, tool, verifier, system
  tool_name text
  input_json jsonb not null default '{}'
  output_json jsonb not null default '{}'
  ledger_delta_json jsonb not null default '{}'
  visibility text not null default 'gm_only'
  status text not null
  created_at timestamptz not null default now()
```

The ledger view also reads existing runtime tables:

- `agent_tool_calls`
- `check_contracts`
- `dice_rolls`
- `check_results`
- `effect_contracts`
- `roll_plans`
- `parameter_impacts`
- `interaction_gates`
- `state_frames`
- `world_events`

Every sidecar tool call writes both its domain record and a `gm_agent_loop_steps`
record with the exact input/output. Harnesses can evaluate the loop without
parsing prose.

## 9. Narration verifier

The verifier has deterministic checks first, with an optional LLM semantic judge
only for paraphrase detection. It must be conservative: reject when the text
asserts a mechanical fact not supported by the ledger.

Blocking findings:

- `missing_check`: final text says or implies a roll/check/test is needed or was
  made, but no `CheckContract`/gate/check result exists.
- `missing_roll_execution`: a check contract exists and is narratively resolved,
  but no dice/passive result exists.
- `missing_effect`: a hit/success/effect implies HP/resource/condition/object
  change, but no `StatePatch`/`ParameterImpact`/effect record exists.
- `invented_effect`: final text says a target lost HP/SAN/Chaos/Harm/SP/AC,
  gained a condition, lost ammo, broke an object, or advanced a clock without a
  matching ledger entry.
- `omitted_visible_result`: a player-visible roll/effect happened but final text
  ignores it.
- `manual_roll_request`: final text asks the player to report dice totals or
  rules-table values under `system_rolls_visible`.
- `secret_leak`: final text exposes GM-only facts, private rolls, hidden target
  numbers, or unrevealed module truth.

Non-blocking findings:

- terse narration of a visible roll;
- missing color around a mechanical result;
- low-confidence source grounding where the tool already marked the result as
  provisional.

The verifier returns a concrete next action:

```text
revise_text
call_propose_check
call_execute_check
call_apply_effect
open_player_gate
fallback_to_legacy
```

## 10. Sidecar failure handling

The sidecar path is experimental and must fail closed.

Fallback cases:

- Pi executable not found;
- extension load failure;
- RPC protocol error;
- timeout;
- max tool steps reached;
- verifier rejects more than `TRPG_GM_AGENT_LOOP_MAX_REPAIRS` times;
- tool host detects invalid session/turn token;
- database mutation error.

Fallback behavior:

- record `gm_agent_loop_runs.status = fallback` or `error`;
- emit CLI/API phase `gm_agent_loop_fallback`;
- continue with the existing legacy turn path unless a tool already committed a
  terminal gate or player-visible roll/effect.

If the sidecar has already committed state, fallback cannot pretend the turn is
fresh. In that case Rust compiles the current ledger into `[roll]`/`[system]`
context and asks the legacy narrator to narrate the already-committed facts.

## 11. Current pipeline integration

Prototype order should preserve existing hard gates:

1. Current Rust preflight still handles open gate replies first. A required gate
   should not be bypassed by the sidecar.
2. If gate handling returns a terminal prompt, the turn ends before Pi.
3. For ordinary turns, sidecar receives the compiled context plus the
   `TurnOrchestrationResult`.
4. The sidecar can call `run_route_kernel(auto)` to reuse current route kernels.
5. The sidecar can add missing checks/effects with explicit tools.
6. The sidecar must call `submit_final_narration`.

This avoids a risky first step where Pi replaces every route decision. Once the
loop is validated, `run_route_kernel` can be split into finer tools or removed
as the Rust-native agent loop takes over.

## 12. API and CLI events

New phases/events:

```text
gm_agent_loop_start
gm_agent_loop_tool_call
gm_agent_loop_tool_result
gm_agent_loop_ledger_updated
gm_agent_loop_verifier_rejected
gm_agent_loop_verifier_accepted
gm_agent_loop_fallback
gm_agent_loop_done
```

Player-visible `delta` events should only start after verifier acceptance. Tool
events can be streamed as JSONL/SSE debug phases, but not as partial player
prose.

Private GM rolls still emit `tool`/GM-only debug events, never player-facing
dice details.

## 13. Testing and evaluation

### Unit tests

- Tool host rejects wrong token/session/turn.
- `propose_check` refuses unsourced target values.
- `execute_check` writes dice roll, check result, and roll plan.
- `apply_effect` refuses unknown parameter path.
- `submit_final_narration` rejects text with an unexecuted check claim.
- `submit_final_narration` rejects text with invented HP/SAN/resource changes.
- `submit_final_narration` rejects omission of a visible executed result.
- `open_player_gate` produces an interaction gate and terminal turn response.

### Scripted sidecar tests

Before running real Pi, create a scripted sidecar driver that emits predetermined
tool calls. This makes Rust tool-host and verifier behavior deterministic.

Scripts:

- says check needed, then tries final text without `propose_check` -> rejected;
- proposes and executes check, then omits result -> rejected;
- executes attack check, skips effect -> rejected;
- executes check and effect, submits consistent text -> accepted;
- opens required reaction gate -> terminal prompt accepted;
- loops past max steps -> fallback recorded.

### Live Pi smoke tests

Use existing harness cases plus new cases focused on missing check/effect:

- technical risk assessment needs a check;
- combat attack needs hit roll and effect resolution;
- player asks "roll for me";
- visible enemy attack opens required reaction gate;
- final narration must not ask for HP/DV/damage totals.

Measure:

```text
missing_check_rate
missing_effect_rate
invented_effect_rate
omitted_visible_result_rate
verifier_repair_count_per_turn
fallback_rate
median_turn_latency
```

Success bar for prototype:

- missing-check and missing-effect failures drop by at least 70% on the targeted
  harness/playtest set;
- no increase in secret leaks;
- fallback rate below 10% on smoke tests;
- median turn latency acceptable for experimental mode;
- all committed state is visible in DB/harness artifacts.

## 14. Migration to Rust-native loop

The sidecar is a learning scaffold. The Rust-native loop should reuse the same
concepts:

- same tool schemas;
- same turn ledger;
- same verifier;
- same max-step/repair/timeout policy;
- same harness metrics.

Replacement target:

```text
RuntimeEngine::run_gm_agent_loop(request, state, compiled, user_input)
  -> LlmClient.complete_with_tools(...)
  -> Rust AgentToolHost directly
  -> NarrationVerifier
  -> accepted output
```

When Rust-native loop reaches parity:

- keep Pi sidecar behind a dev-only flag for comparison;
- default product path becomes Rust-native;
- remove Pi process spawning from production builds;
- keep the sidecar harness only if it remains useful for external agent
  experiments.

## 15. Implementation slices

### Slice A: ledger + verifier core

- Add `gm_agent_loop_runs` and `gm_agent_loop_steps`.
- Add `TurnLedger` reader.
- Add deterministic `NarrationVerifier`.
- Add scripted sidecar tests.

### Slice B: Rust tool host

- Implement one-turn HTTP tool host.
- Implement `inspect_turn_state`, `retrieve_rule`, `inspect_ledger`,
  `submit_final_narration`.
- Add auth/session/turn checks.

### Slice C: mechanical tools

- Implement `run_route_kernel`, `propose_check`, `execute_check`,
  `apply_effect`, `open_player_gate`.
- Wire emitted phases and DB writes.

### Slice D: Pi sidecar adapter

- Add `.pi/extensions/chatrpg-gm-agent.ts`.
- Register tools with schemas.
- Add `PiSidecarClient` over RPC JSONL.
- Run Pi in temp cwd with minimal environment.

### Slice E: CLI experimental path

- Add `--agent-loop pi` or env path to `trpg turn` and `trpg play`.
- Emit loop phases.
- Save accepted narration.

### Slice F: API experimental path

- Mirror CLI behavior in SSE route.
- Ensure no player-visible deltas stream before verifier acceptance.

### Slice G: Rust-native loop design follow-up

- After prototype metrics, write a second design for replacing Pi with
  `LlmClient.complete_with_tools`.

## 16. Open risks and mitigations

### Pi tool surface risk

Pi includes general agent capabilities. Prototype mitigation: run in a temp cwd,
with a single extension, no repo access, and one-turn HTTP token. Production
mitigation: migrate loop into Rust.

### Latency

Multi-step tool loops cost more than one narration call. Mitigation: max steps,
route-kernel batching, compact snapshots, and scripted tests to avoid wasteful
repair loops.

### Duplicate CLI/API logic

The current turn pipeline is duplicated across CLI and API. The prototype should
place as much loop logic as possible in `RuntimeEngine` so CLI/API only emit
events and stream accepted output.

### Verifier false positives

The verifier should be strict only for mechanical claims. It should not reject
ordinary fictional color. Start deterministic and add optional semantic checking
only for well-defined paraphrase classes.

### Partial state before fallback

If state is committed before sidecar failure, fallback must narrate the committed
ledger instead of restarting the turn. This is why the ledger is mandatory from
Slice A.

## 17. Acceptance criteria

Prototype is successful when:

1. The sidecar path can run one CLI turn through Pi and return accepted narration.
2. A scripted final narration that says "make a check" without a check is
   rejected before player output.
3. A scripted final narration that applies damage without a committed effect is
   rejected before player output.
4. A committed visible roll/effect is present in final narration or the verifier
   rejects it.
5. Existing legacy path still works with the flag disabled.
6. Harness artifacts expose loop runs, tool steps, ledger, verifier result, and
   accepted final narration.
7. Targeted playtests show a measurable reduction in missing-check and
   missing-effect failures.

## 18. Design principle

The sidecar may decide what to try next, but Rust decides what is true.

The final player-visible answer is not the LLM's last message. It is the text
that survived a turn-ledger verifier after all required checks, rolls, gates,
and effects were either committed or explicitly deferred to player input.

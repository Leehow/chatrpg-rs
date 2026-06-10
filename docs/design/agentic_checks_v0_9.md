# v0.9 Agentic Checks, Roll Visibility, and Harness Design

v0.9 introduces a Rust-owned GM Agent Runtime. The term "agent" does not mean an external Python or JavaScript framework and does not mean the LLM controls the loop. The agent is a Rust state machine that loads advisory policy from JSON, calls deterministic tools, records tool calls, and only uses the LLM for bounded narration or JSON skills.

## Design principle

Rules about when to ask a player to roll, when to roll publicly, and when to roll secretly are **table advice**, not hardcoded program branches. The shipped defaults live under:

```text
data/agent/advice/dice_visibility.v1.json
```

A table can edit or replace that file without recompiling. Rust only performs generic matching, validation, persistence, and audit. System prompts, advice layers, and runtime ContextBlocks remain separate:

- system prompt: output contract and safety contract;
- advice layer: JSON policy consumed by the Rust Agent, not dumped wholesale into the prompt;
- runtime context: BP1/BP2/BP3 ContextBlocks selected by the planner;
- tool/audit logs: database records for checks, rolls, and agent tool calls.

## New Rust crates and models

```text
crates/trpg-agent
  GmAgent
  AgentPolicyPack
  AgentAdviceLayer
  AgentAdviceRule
  pending_check_prompt
```

`trpg-model` now contains shared contracts:

```text
AgentTurnPlan
CheckContract
FreeReadContract
PendingCheck
DiceRollRecord
CheckResultRecord
AgentToolCallRecord
CombatFrame
```

## Turn planning

The runtime first checks whether the user input resolves an open pending check. If so, it resolves the check before normal narration.

If no pending check exists, the Rust Agent loads advice layers and produces one of:

```text
NarrateOnly
AskPlayerRoll
GmRollThenNarrate
SecretRollThenNarrate
PassiveResolution
StartOrContinueCombat
```

For `AskPlayerRoll`, SSE/JSONL stops after `pending_check_created`, emits a player-facing prompt, saves the turn with status `awaiting_player_roll`, and waits for the next user message.

For `GmRollThenNarrate` and `SecretRollThenNarrate`, Rust calls the dice tool before narration and injects only the structured result needed for narration. Secret rolls are logged as `gm_only` tool events; the player-facing text must not reveal the roll, formula, DC, or result.

## Database tables

Migration `0004_agentic_checks_v09.sql` adds:

```text
agent_turns
agent_tool_calls
check_contracts
pending_checks
dice_rolls
check_results
combat_frames
combat_events
```

This makes the harness able to assert behavior from structured events, not only final prose.

## Harness upgrades

The Rust harness now supports:

- `required_events`
- `forbidden_events`
- `required_event_contains`
- `required_done_reason`
- `require_no_llm_stream_start`

New cases:

```text
homecoming_first_turn_no_spoiler
homecoming_tech_analysis_requires_pending_check
homecoming_secret_roll_no_meta_leak
```

These check that surface observation can proceed normally, risky technical conclusions pause for a player roll, and hidden NPC actions can be privately rolled without leaking metagame information.

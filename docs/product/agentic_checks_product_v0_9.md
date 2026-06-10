# ChatRPG v0.9 Product Addendum: Agentic Checks and Dice Flow

## Product purpose

v0.9 moves ChatRPG from "LLM improvises a ruling" toward "Rust GM Agent orchestrates a table-safe ruling flow." The goal is not to implement one fixed combat engine per ruleset. The goal is to let the GM behave like an experienced table GM: identify risk, search rules, ask for rolls when player agency matters, roll privately when hidden information is at stake, and continue narration smoothly when automation is appropriate.

## Product philosophy

The product must support many games and many table styles. Therefore, dice visibility and roll ownership are editable advice, not immutable code. A Cyberpunk RED table, a Call of Cthulhu table, and a Triangle Agency table may prefer different roll habits. v0.9 stores default advice in JSON and lets the Rust Agent interpret it.

## User experience

When the player makes a risky action, ChatRPG may stop and ask for a roll:

```text
请投 appropriate TECH / repair / hacking skill check。成功你能确认安全路径；失败压力升级。
```

When the GM can roll publicly, the SSE stream can continue:

```text
phase: tool_call
event: dice
phase: llm_stream_start
```

When the GM must roll secretly, the player only sees fiction; the harness and GM/debug stream still receive the audit event.

## Architecture

```text
Player input
  -> Runtime auto-search
  -> Rust GM Agent advice planner
  -> CheckContract / FreeReadContract
  -> Dice tool / PendingCheck / Secret roll
  -> LLM narration segment
  -> State/memory/learning audit
```

The LLM never owns the loop. It receives structured context from Rust and produces narration under the existing visibility and player-facing output contract.

## What this version does not do

v0.9 does not finish a full tactical combat system. It lays the check/roll/pending/audit foundation that combat will use in v1.0.

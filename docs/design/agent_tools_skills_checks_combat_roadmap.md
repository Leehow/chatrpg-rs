# Agent + Tools + Skills Roadmap for Checks and Combat

This document records the post-v0.8.2 direction indicated by playtest feedback.

## Product stance

Do not build one fixed, fully-programmed check/combat engine for every TRPG. Build an agentic adjudication layer:

- LLM GM interprets intent, stakes, fiction, and table style.
- Tools execute bounded operations: dice, search, state proposal validation, initiative ordering, damage math, visibility checks, memory writes.
- Skills are reusable agent capabilities: `decide_check_needed`, `frame_stakes`, `select_rule_packet`, `ask_for_roll`, `resolve_roll`, `update_combat_state`, `audit_ruling`.
- Harness cases reproduce table situations and assert output/trace properties.

## Near-term goals

1. Replace prompt-only check policy with a tool-callable `CheckIntent` contract.
2. Add `RulingTrace` output: why a check was or was not required.
3. Add a combat state model that is generic and sparse: actors, zones, intent queue, known threats, resources, wounds/statuses, pending rolls.
4. Keep exact system rules as learned/source-backed packets, not hard-coded universal rules.
5. Add harness test classes:
   - spoiler terms do not leak;
   - risky analysis asks for/frames a check;
   - 429 emits retry/error events;
   - repeated lookup can become an auditable learned-packet candidate.

## Non-goal

Do not encode every RPG as deterministic Rust procedures before play. Deterministic tools should handle narrow, validated operations; the LLM GM remains the director and rules interpreter, constrained by source-backed tools and validators.

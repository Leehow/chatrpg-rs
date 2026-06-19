---
id: TC-KNOW-03-no-spoiler-plugin-v2
title: NoSpoiler Plugin v2 — context filter + prompt + verifier
mode: implementation
owner: claude-worker
priority: P0
risk: high
expected_backend: tty
subagent_policy: research_only
observability: full
repair_budget: 3
---

# TC-KNOW-03 — NoSpoiler Plugin v2

## Objective

Upgrade spoiler prevention from prompt-only guidance to a three-stage policy:

```text
before_context_compile: filter player-unknown / GM-only / future-scene content
during_prompt_compile: add player-facing policy prompt blocks
after_llm_stream: verify output against private secret data
```

## Non-goals

- Do not put secret full text into the player-facing model prompt merely to say “do not reveal this”.
- Do not rewrite all context compilation.
- Do not block style plugins unless they conflict with safety.

## Scope owned

- NoSpoiler plugin/policy code.
- Context filter contribution.
- PromptBlock contribution.
- Verifier finding skeleton or deterministic verifier for known test secrets.
- Plugin trace tests.

## Scope off

- Full semantic leak detector requiring new provider calls unless explicitly approved.
- UI spoiler debugger.

## Architecture constraints

- Player-facing prompt uses PlayerKnowledgeView.
- VerifierPrivateView may access secrets for leak detection but does not enter model prompt.
- Safety plugin priority is higher than style plugin priority.
- Fail policy for before_context_compile is fail_closed.
- Fail policy for after_llm_stream is repair_then_fail_closed or explicit warning if repair is not implemented yet.

## Acceptance criteria

- Player-unknown secret fact is filtered before prompt compilation.
- Revealed fact is allowed.
- Future-scene GM-only block is filtered.
- PromptBlock is emitted with priority, cache_zone, visibility, and token budget.
- Verifier detects a deterministic secret leak in a test case.
- Plugin trace records filtering and verifier result.

## Required tests

- `no_spoiler_filters_player_unknown_context`
- `no_spoiler_allows_revealed_fact`
- `no_spoiler_filters_future_scene_gm_only`
- `no_spoiler_prompt_block_has_metadata`
- `no_spoiler_verifier_catches_secret_leak`

## Done when

A worker can demonstrate that a GM-only secret known to the runtime does not enter player prompt/SSE until PlayerLearnedFact exists.

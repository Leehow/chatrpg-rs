---
id: TC-D3-00-no-spoiler-production-sources-v1
title: NoSpoiler Production Sources v1 — private metadata and secret-term feed
mode: implementation
owner: claude-worker
priority: P0
risk: high
expected_backend: tty
subagent_policy: research_only
observability: full
repair_budget: 3
---

# TC-D3-00 — NoSpoiler Production Sources v1

## Objective

Make NoSpoiler v2 operate on real production inputs instead of empty placeholders:

- context blocks that carry GM-only / future-scene / fact-bound content should
  provide private metadata for `ContextFilter`;
- AfterLlmStream should receive a private `secret_terms` feed from source-backed
  spoiler metadata and player-known fact projection;
- no secret term text may enter player-facing prompt text or trace summaries.

## Why this matters

`design/设计3.md` says NoSpoiler should be a KnowledgePolicy executor, not only a
prompt reminder. TC-KNOW-03 added the three-stage plugin, but ledger notes show
production block tagging and `secret_terms` source are still missing.

## Non-goals

- Do not redesign the plugin host.
- Do not build semantic LLM leak detection.
- Do not rewrite all context compilation.
- Do not make hidden secret text visible to the LLM just to forbid it.
- Do not change reveal semantics or KnowledgeEdge schema.

## Scope owned

- `crates/trpg-gm/src/plugin/**`
- `crates/trpg-gm/src/turn_loop.rs`
- `crates/trpg-gm/tests/**`
- `crates/trpg-runtime/src/spoiler_guard.rs` only if a tiny reusable extractor is needed
- `crates/trpg-model/src/spoiler.rs` only if a tiny data helper is needed
- focused tests in `crates/trpg-gm` / `crates/trpg-runtime`

## Scope off

- migrations
- KnowledgeEdge schema redesign
- NPC profile/relationship/mind/behavior implementation
- memory proposal commit pipeline
- broad GM turn-loop rewrite
- real provider calls
- unrelated warning cleanup or broad formatting
- shared epic ledger/task-card edits

## Architecture constraints

- Source-backed spoiler metadata is authoritative; do not infer secrets by broad
  body scanning in production.
- `PluginContext.secret_terms` is private verifier input and must not be
  rendered into prompt text.
- ContextFilter must use player-known fact ids to allow already revealed facts.
- Missing DB/projection data should fail closed for hidden content where
  possible, but must not panic or block streaming.
- Verifier findings must reference safe ids/summaries, not secret terms.

## Implementation guidance

Start by tracing how `ContextBlock`s are produced and where `SpoilerMeta` /
`secret_terms` already exist. Prefer small adapters that derive private block
views and secret-term feeds from existing source metadata. If production block
tagging is too broad for one card, implement the smallest real source path and
document remaining producers.

## Acceptance criteria

- A production-like context block with explicit spoiler/fact metadata is dropped
  before prompt assembly when the fact is not player-known.
- The same block is allowed once the fact is player-known.
- AfterLlmStream receives non-empty private `secret_terms` in a production-like
  path and emits `SecretLeak` if visible narration leaks an unrevealed term.
- Verifier detail and traces do not echo the secret term.
- Existing NoSpoiler tests still pass.
- No player-facing prompt/SSE contains private secret-term lists.

## Required tests

- `no_spoiler_context_filter_uses_production_fact_metadata`
- `no_spoiler_context_filter_allows_player_known_fact`
- `no_spoiler_after_stream_uses_private_secret_terms_source`
- `no_spoiler_finding_does_not_echo_secret_term`

Exact names may vary if the existing local test style strongly suggests a
different prefix, but the handoff must map them to these requirements.

## Validation commands

```bash
cargo test -p trpg-gm no_spoiler
cargo test -p trpg-gm plugin
cargo test -p trpg-runtime spoiler_guard
cargo check -p trpg-model -p trpg-runtime -p trpg-gm
git diff --check
```

## Escalation triggers

- The only viable implementation would require exposing secret text in the
  model prompt.
- A destructive or compatibility-breaking migration is needed.
- Production context block metadata cannot be accessed without a broad rewrite.
- Validation failure cannot be attributed after focused investigation.

## Done when

NoSpoiler has at least one real production source for private block filtering
and secret-term verification, with tests proving unknown facts are blocked and
known facts are allowed.

## Handoff requirements

Include assumptions, changed files, production source path(s) wired, remaining
unwired producers if any, validation output, repair loops, scope ledger, and an
acceptance-criteria ledger.

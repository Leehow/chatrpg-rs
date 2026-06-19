---
id: TC-P2-02-knowledge-leak-verifier-v1
title: Knowledge Leak Verifier v1 — player-unknown and NPC-mind consistency checks
mode: implementation
owner: claude-worker
priority: P2
risk: high
expected_backend: tty
subagent_policy: research_only
observability: full
repair_budget: 3
---

# TC-P2-02 — Knowledge Leak Verifier v1

## Objective

Add deterministic verifier helpers that flag player-visible leakage of player-unknown facts and NPC speech/action that contradicts the speaking NPC's own mind view or behavior plan.

## Why this matters

NoSpoiler v2 filters tagged context and catches explicit secret terms. NPC Mind/Behavior now define what an NPC knows and may reveal. This task hardens the seam between generated narration and those projections so later GM consumers can detect "the player was told a hidden fact" and "an NPC said something they do not know" without relying only on prompt discipline.

## Non-goals

- Do not block or rewrite the live stream synchronously.
- Do not add broad natural-language inference or a real LLM verifier.
- Do not replace the NoSpoiler plugin.
- Do not wire into the GM turn loop unless a tiny additive helper is unavoidable and explicitly justified.
- Do not expose secret terms or private fact text in player-facing errors.

## Scope owned

- `crates/trpg-model/src/*verifier*.rs` or additive verifier DTOs if needed
- `crates/trpg-model/src/lib.rs`
- `crates/trpg-model/tests/*verifier*.rs`
- `crates/trpg-runtime/src/*verifier*.rs`
- `crates/trpg-runtime/src/lib.rs`
- `crates/trpg-gm/src/plugin/builtin_no_spoiler.rs` only for tiny reuse/export tests if needed
- `crates/trpg-gm/tests/*verifier*.rs` or focused inline plugin tests if needed

## Scope off

- Migrations and `crates/trpg-db/**`
- GM turn-loop orchestration, stream repair, and SSE transport
- Secret extraction from raw source text
- NPC profile/relationship/mind behavior redesign
- Broad plugin host redesign
- Broad `cargo fmt`

## Architecture constraints

- Verification must be projection-driven and fail closed.
- Player-known gating uses durable player knowledge projection, not raw context visibility guesses.
- NPC consistency checks use `NpcMindView` / `NpcBehaviorPlan` and must not consult GM omniscient truth.
- Findings must avoid leaking secret text; prefer fact ids, holder ids, and safe summaries.
- Existing `VerifierFindingKind::SecretLeak` semantics should be reused where possible.

## Implementation guidance

Prefer deterministic inputs such as fact ids, safe secret terms, `NpcBehaviorPlan::facts_can_reveal`, `NpcBehaviorPlan::facts_will_withhold`, and `NpcMindView::known_fact_ids()`. A small verifier result type may map to existing `trpg_agent::VerifierFinding`.

If narration text scanning is needed, keep it bounded to caller-provided `SecretTerm` / fact-id markers. Do not invent semantic NLP.

## Acceptance criteria

- Player-visible narration mentioning an unrevealed/private fact term produces a blocker or equivalent finding.
- Already player-known facts are allowed.
- NPC output that attempts to reveal a fact outside `facts_can_reveal` is flagged.
- NPC beliefs are not promoted to known truth; a false belief can be identified separately without becoming revealable fact.
- Findings do not contain raw secret terms when a fact id is available.
- The verifier is deterministic and pure in focused tests.

## Required tests

- `player_unknown_fact_leak_is_blocker`
- `player_known_fact_is_allowed`
- `npc_cannot_reveal_fact_outside_behavior_plan`
- `npc_withheld_secret_reveal_is_flagged`
- `npc_false_belief_is_not_treated_as_known_truth`
- `verifier_finding_does_not_echo_secret_text`

## Validation commands

```bash
cargo test -p trpg-model verifier
cargo test -p trpg-runtime verifier
cargo test -p trpg-gm no_spoiler
cargo check -p trpg-model -p trpg-db -p trpg-runtime -p trpg-gm
git diff --check
```

## Escalation triggers

- The verifier requires storing new private text or a destructive schema change.
- The only viable design would expose raw secret terms in player-facing output.
- Existing NoSpoiler plugin behavior would need a broad rewrite.
- A real provider/API key is required for validation.

## Done when

There are deterministic verifier helpers and tests for player-unknown leaks and NPC-mind inconsistency, ready for later turn-loop integration.

## Handoff requirements

Include assumptions, files changed, finding examples, validation output, repair loops, scope ledger, and an acceptance-criteria ledger.

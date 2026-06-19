---
id: TC-D3-03-memory-proposal-commit-pipeline-v1
title: Memory Proposal Commit Pipeline v1 — runtime-owned review and apply
mode: implementation
owner: claude-worker
priority: P2
risk: high
expected_backend: tty
subagent_policy: research_only
observability: full
repair_budget: 3
---

# TC-D3-03 — Memory Proposal Commit Pipeline v1

## Objective

Add a runtime-owned path that can accept validated memory extraction proposals
and commit approved changes to the appropriate durable state: world/memory fact,
KnowledgeEdge/domain event, or NPC relationship.

## Why this matters

TC-P2-01 intentionally made proposals inert. Design3 requires plugins/agents to
propose while runtime validates and commits through explicit state owners.

## Non-goals

- Do not let plugins or LLMs write DB rows directly.
- Do not run new provider extraction jobs.
- Do not auto-approve ambiguous proposals.
- Do not redesign `memory_facts`, `KnowledgeEdge`, or `NpcRelationship`.

## Scope owned

- `crates/trpg-runtime/src/memory_proposal.rs`
- `crates/trpg-db/src/lib.rs` only for minimal commit helpers if needed
- tests in `crates/trpg-runtime/tests/**`, `crates/trpg-db/tests/**`
- small model additions only if needed for review status/result types

## Scope off

- GM turn-loop heavy job wiring
- NoSpoiler plugin behavior
- NPC behavior prompt injection
- broad memory schema rewrite
- destructive migrations
- shared ledger/task-card edits

## Architecture constraints

- Proposal validation must remain fail-closed.
- Runtime decides; proposal types do not commit themselves.
- Knowledge updates must use explicit domain event / KnowledgeEdge paths.
- Relationship commits must reuse evidence-required bounded delta logic.
- World fact commits may use existing `memory_facts` as first-stage Fact Store
  unless a small additive helper is clearly safer.

## Acceptance criteria

- A validated `WorldFactCandidate` can be committed to a durable fact/memory path.
- A validated `KnowledgeUpdateCandidate` can commit through KnowledgeEdge or the
  appropriate reveal/NPC-learned runtime API.
- A validated `NpcRelationshipDeltaCandidate` updates durable relationship
  state through existing bounded/evidence gates.
- Invalid proposals do not commit partial state.
- Commit result records Done/Rejected/Skipped reasons.

## Required tests

- `commit_world_fact_candidate_writes_fact_store`
- `commit_knowledge_update_uses_runtime_knowledge_path`
- `commit_relationship_delta_reuses_evidence_gate`
- `invalid_proposal_batch_commits_nothing`

## Validation commands

```bash
cargo test -p trpg-runtime memory_proposal
cargo test -p trpg-db memory_proposal
cargo check -p trpg-model -p trpg-db -p trpg-runtime
git diff --check
```

## Escalation triggers

- A new destructive schema is required.
- Commit semantics would make memory facts authoritative over domain events.
- Existing proposal model cannot express enough evidence to commit safely.

## Done when

There is a tested runtime-owned commit/review surface for proposal batches, with
plugins still proposal-only.

## Handoff requirements

Include commit semantics, DB/API paths used, validation evidence, and any
proposal variants left uncommitted with reason.

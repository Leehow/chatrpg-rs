# Project Constitution for Autonomous Work

This is the project-level decision policy that autonomous agents should use when the task card does not specify a routine implementation detail.

## Architecture identity

AiChatTrpg is a source-grounded JSON asset runtime.

Do not turn it into a universal TRPG rule compiler. Do not fall back to free-form LLM GM adjudication.

## Implementation language boundary

This project is Rust-native. Product behavior, runtime logic, CLI surfaces,
orchestration, gameplay/playtest runners, caching, character-card lifecycle,
evaluation runners, and durable state handling belong in Rust.

Python is allowed only for test scripts and test fixtures. It must not own
product decisions, runtime state, scheduling, character pools, semantic policy,
or gameplay/evaluation execution paths. If a capability is useful beyond tests,
port or expose it through Rust before relying on it.

## Runtime authority

```text
Agents propose.
Plugins propose.
Runtime adjudicates.
Events and projections record.
GM narration renders.
```

LLM text must not be the authority for HP, damage, conditions, scene transitions, knowledge reveal, or mechanical success.

## Rule and module handling

- Rules and modules become source-backed assets.
- Assets may have facets, visibility, confidence, source refs, and binding candidates.
- Exact execution requires enough verified data.
- Partial or uncertain rules downgrade to guided ruling.
- Unsupported rules fail closed.

## Knowledge and spoiler handling

- World facts are separate from who knows those facts.
- GM truth is not automatically player-known.
- Context surfaced to the model is not automatically player knowledge.
- NPCs may only act or speak from their own knowledge/beliefs plus persona and relationship state.
- Spoiler policy is viewer-relative.

## Default choice when uncertain

Choose the smallest reversible implementation that is:

- data-driven;
- source-backed;
- runtime-owned;
- fail-closed;
- visibility-safe;
- compatible with existing transport and contracts;
- testable.

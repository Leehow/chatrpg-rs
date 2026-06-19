# Project Constitution for Autonomous Work

This is the project-level decision policy that autonomous agents should use when the task card does not specify a routine implementation detail.

## Architecture identity

AiChatTrpg is a source-grounded JSON asset runtime.

Do not turn it into a universal TRPG rule compiler. Do not fall back to free-form LLM GM adjudication.

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

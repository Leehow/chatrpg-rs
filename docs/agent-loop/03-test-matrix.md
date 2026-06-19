# Test Matrix

This matrix maps task types to required validation.

## Universal gates

Every code-affecting task should run the smallest relevant subset of:

```bash
cargo fmt --check
cargo check -p trpg-model -p trpg-db -p trpg-runtime -p trpg-gm
cargo test -p trpg-model
cargo test -p trpg-db
cargo test -p trpg-runtime --lib
cargo test -p trpg-gm
```

If a central runner does not exist, workers must run focused scripts and state exactly what was and was not validated.

## Architecture gates

- No engine ruleset/module hardcode.
- No LLM direct state commit.
- No NeedBus bypass unless explicitly scoped.
- No player-facing GM-only leakage.
- No NPC use of unknown facts.
- No heavy postprocess blocking streaming completion unless explicitly scoped.

## Knowledge Runtime tests

Required behaviors:

1. Fact and KnowledgeEdge are separate.
2. One holder can know a fact while another does not.
3. GM truth does not enter player projection by default.
4. ContextSurfaced does not grant player knowledge.
5. PlayerLearnedFact grants only player/party knowledge.
6. NpcLearnedFact grants only that NPC knowledge.
7. False beliefs remain beliefs, not world truth.

## Spoiler tests

Required behaviors:

1. Player-unknown secrets do not enter player prompt.
2. Player-unknown secrets do not enter SSE output.
3. Player-unknown secrets do not enter player memory summary.
4. Revealed facts may enter player prompt.
5. Verifier catches obvious secret leaks.
6. NPC speech prompt excludes facts the NPC does not know.

## NPC Mind tests

Required behaviors:

1. NpcProfile round-trips.
2. NpcRelationship values are bounded.
3. Relationship deltas require evidence events.
4. NpcMindView includes NPC knowledge and beliefs but excludes unknown facts.
5. NpcBehaviorPlan derives stance, interaction desire, reveal willingness, and speech guidance.
6. Hostile/fearful/trusting states change behavior plan in predictable ways.

## Plugin tests

Required behaviors:

1. Hook order is stable.
2. Priority is respected.
3. fail_policy is enforced.
4. PromptBlock has cache_zone, visibility, priority, token budget.
5. Plugins return proposals and do not commit state directly.
6. Plugin contribution trace is recorded.

## SSE / turn-loop tests

Required behaviors:

1. API and CLI use the same turn executor when applicable.
2. Streaming deltas arrive before TurnComplete.
3. Critical commit completes before the next turn reads state.
4. Heavy postprocess is scheduled after streaming completion.
5. Errors are surfaced as failed/warning events, not blank success.

## Parser / asset tests

Required behaviors:

1. Every asset has id, kind, visibility, confidence, and source_refs.
2. Executable facets have binding candidates or source-only status.
3. Module entry is not cover, table of contents, or GM foreword.
4. GM-only/backstory/synopsis content is not player-known by default.
5. Complex systems may remain guided/partial instead of being falsely exact.

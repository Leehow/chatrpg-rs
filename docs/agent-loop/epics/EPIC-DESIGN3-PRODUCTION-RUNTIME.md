# EPIC: Design3 Production Runtime Wiring

## Run mode

Use `continuous_epic`.

This epic continues after `EPIC-KNOWLEDGE-NPC-RUNTIME`: the foundation is in
place, but `design/设计3.md` still calls for production wiring so Knowledge
Runtime, NoSpoiler, NPC Mind, memory extraction, and projection retrieval are
used by real turn paths instead of only existing as model/runtime foundations.

Execution ledger:
`docs/agent-loop/epics/EPIC-DESIGN3-PRODUCTION-RUNTIME-ledger.md`.

## Goal

Turn the remaining Design3 gaps into production runtime behavior:

- NoSpoiler uses real private context metadata and secret-term sources.
- NPC profile, mind, and behavior plan can be loaded for active NPCs and
  injected as prompt-safe guidance.
- Memory extraction proposals can enter a runtime-owned review/commit pipeline.
- Relationship extraction has a social-interaction gate, not only new-entity
  surfacing.
- Viewer/speaker retrieval projections become explicit APIs.
- Production verifier hooks can use the deterministic leak/NPC consistency
  helpers.

## Non-goals

- Do not build a universal rule compiler.
- Do not make LLM agents or plugins commit durable state directly.
- Do not rewrite the whole GM turn loop.
- Do not block user-visible streaming on heavy postprocess.
- Do not introduce large new dependencies or providers.
- Do not broad-format the repository.

## Ordered task plan

### P3: Production safety and NPC wiring

1. `TC-D3-00-no-spoiler-production-sources-v1.md`
   - Populate NoSpoiler ContextFilter and AfterLlmStream verifier from real
     private block metadata / spoiler terms rather than empty production inputs.

2. `TC-D3-01-npc-profile-store-v1.md`
   - Add durable NPC profile storage/load helpers so profiles are not only
     transient test structs.

3. `TC-D3-02-active-npc-behavior-prompt-v1.md`
   - Load active NPC mind views, derive behavior plans, and inject prompt-safe
     guidance into GM prompt assembly without granting NPC omniscience.

4. `TC-D3-03-memory-proposal-commit-pipeline-v1.md`
   - Add a runtime-owned proposal review/commit path for accepted world facts,
     knowledge updates, and relationship deltas.

5. `TC-D3-04-relationship-extraction-social-gate-v1.md`
   - Expand relationship extraction gating beyond "new entity surfaced" to
     social interaction / active NPC evidence.

### P4: Projection retrieval and production verifier integration

6. `TC-D3-05-projection-retrieval-and-verifier-integration-v1.md`
   - Add explicit player/GM/NPC projection retrieval APIs and wire production
     verifier hooks to deterministic knowledge/NPC consistency helpers.

## Stop conditions

Stop only if:

- all tasks above are Done/Deferred/BlockedByHuman;
- a DB migration would be destructive or compatibility-breaking;
- a production turn-loop integration requires a product decision not inferable
  from Design3 or existing code;
- workspace state becomes unsafe for scoped worker dispatch;
- repeated worker failures leave no bounded safe next action;
- the user explicitly asks for a checkpoint or stop.

## Validation gates

Each accepted task must include:

- exact test/check commands and outcomes;
- scope ledger;
- handoff file path;
- explanation of any deferred production behavior;
- no player-unknown fact in player-facing prompt/SSE/memory;
- NPC knowledge does not leak into player knowledge or GM truth incorrectly;
- no plugin or extractor commits state except through runtime-owned APIs.

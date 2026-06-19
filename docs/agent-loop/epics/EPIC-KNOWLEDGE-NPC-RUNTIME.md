# EPIC: Knowledge Runtime + No-Spoiler + NPC Mind

## Run mode

Use `continuous_epic`.

This epic should not stop after a single accepted task card. The lead should review each worker handoff, update the ledger, and dispatch the next unblocked task until all tasks are Done/Deferred/BlockedByHuman.

Execution ledger: `docs/agent-loop/epics/EPIC-KNOWLEDGE-NPC-RUNTIME-ledger.md`.

## Goal

Implement the first complete vertical slice of the Knowledge Runtime:

- world facts are separate from who knows them;
- player knowledge, GM truth, and NPC knowledge are distinct projections;
- context surfacing is not the same as player knowledge;
- reveal events update knowledge deliberately;
- no-spoiler policy uses knowledge projections;
- NPCs have profile, relationship, knowledge, and behavior plan foundations.

## Non-goals

- Do not build a universal rule compiler.
- Do not make GM Agent the state authority.
- Do not make NPC personality a prompt-only blob.
- Do not require exact automation for all module-specific social behavior.
- Do not broad-format the repository to fix pre-existing `cargo fmt --check` debt.

## Ordered task plan

### P0: Knowledge identity and reveal semantics

1. `TC-KNOW-00-actor-identity-contract.md`
   - Establish durable actor/holder identity contract.
   - Needed before opening durable `knowledge_edges(holder_kind='npc')`.

2. `TC-KNOW-01-knowledge-edge-v1.md`
   - Separate facts from knowledge holders.
   - GM/player_party projections.

3. `TC-KNOW-02-reveal-events-v1.md`
   - Split `ContextSurfaced`, `PlayerExposed`, `PlayerLearnedFact`, `NpcLearnedFact`.

4. `TC-KNOW-04-npc-durable-knowledge-edges.md`
   - Open durable NPC holder support once actor identity is stable.

5. `TC-KNOW-03-no-spoiler-plugin-v2.md`
   - Upgrade no-spoiler from prompt-only to context filter + prompt block + verifier using projections.

### P1: NPC mind foundation

6. `TC-NPC-01-npc-profile-v1.md`
   - Static NPC persona and speech style.

7. `TC-NPC-02-npc-relationship-v1.md`
   - Trust/fear/suspicion/hostility/debt/leverage/interaction desire.

8. `TC-NPC-03-npc-mind-view-v1.md`
   - NPC-specific projection using NPC knowledge + profile + relationship.

9. `TC-NPC-04-npc-behavior-plan-v1.md`
   - Structured plan for NPC speech/action before GM narration.

### P2: Integration hardening

10. `TC-P2-01-memory-extraction-proposals-v1.md`
    - Add proposal-only memory extraction surfaces for facts, knowledge, and relationships.

11. `TC-P2-02-knowledge-leak-verifier-v1.md`
    - Add deterministic verifier helpers for player-unknown leaks and NPC-mind inconsistency.

12. `TC-P2-03-golden-harness-cases-v1.md`
    - Add Rust-native golden harness cases for Homecoming / Triangle / CoC-style investigation.

P2 task cards are opened after P0/P1 acceptance when the user asks to continue in
`continuous_epic` mode.

## Stop conditions

Stop only if:

- all tasks above are Done/Deferred/BlockedByHuman;
- actor identity cannot be resolved safely without user decision;
- DB migration would be destructive;
- workspace state is unsafe for scoped worker dispatch;
- repeated worker failures leave no bounded next action.

## Validation gates

Each accepted task must include:

- exact test commands and outcomes;
- scope ledger;
- handoff file path;
- explanation of any deferred items;
- no broad reformat unless explicitly scoped;
- no player-unknown fact in player-facing prompt/SSE/memory;
- NPC knowledge does not leak into player knowledge or global truth incorrectly.

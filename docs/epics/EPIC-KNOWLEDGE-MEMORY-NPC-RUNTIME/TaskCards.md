# Task Cards — Vertical Journey Edition

Foundation work may be delegated as subtasks, but only vertical journey cards can close gameplay acceptance rows.

---

# TC-PIPE-00 — Integration Debt Classification

## Objective

Classify the current dirty `codex/knowledge-runtime-p0` worktree before new
code-affecting lanes are dispatched.

## Required behavior

- Read `.tmp/team-lead/worker-*.md` handoffs and map dirty files to task IDs.
- Identify which dirty slices are accepted, need revision, rejected, obsolete,
  or blocked by human decision.
- Record the classification in `IntegrationReport.md` and update
  `WorkGraph.yaml`.
- Do not edit Rust implementation code.
- Do not stage, commit, push, clean, reset, restore, or discard dirty files.

## Ownership

```yaml
read_set:
  - .tmp/team-lead/**
  - git status --porcelain
  - git diff --stat
  - git diff --name-only
write_set:
  - docs/epics/EPIC-KNOWLEDGE-MEMORY-NPC-RUNTIME/WorkGraph.yaml
  - docs/epics/EPIC-KNOWLEDGE-MEMORY-NPC-RUNTIME/IntegrationReport.md
  - docs/epics/EPIC-KNOWLEDGE-MEMORY-NPC-RUNTIME/Documentation.md
hotspot_leases:
  - workgraph
  - integration_report
generated_outputs: []
migration_slot: none
validation_profile: protocol-recovery
```

## Acceptance

New code-affecting WorkGraph tasks remain blocked until the dirty state has a
reviewable classification and a next integration action.

---

# TC-JRNY-00 — Journey Prelude: Character, Session, and Public Player View

## Objective

Extend the harness/playtest runner so every normal play journey can create a player character through the public character-creation path, bind it to a session, verify the sheet, and start from a playable scene before sending turn input.

## Required behavior

- Scenario `setup.character` executes `trpg create-character` or the character API.
- Captures character/session/actor identifiers from public output.
- Verifies persisted character and required sheet fields read-only.
- Refuses to execute play chapters when character/session readiness fails.
- Writes character-creation evidence bundle.

## Acceptance

A scenario with no character returns `INVALID_SETUP`. A valid scenario proves the first turn uses the created actor.

---

# TC-JRNY-01 — Adaptive Human Player and Real Roll Handling

## Objective

Drive scenario actions based on observed output/events rather than blindly sending fixed turns.

## Required behavior

- State-machine actions with `when` and `if_not_triggered` conditions.
- Player simulator receives only PlayerView and its own sheet.
- On player-roll request, uses the actual character and public dice path.
- Requires `DiceRolled` and `CheckResolved` for check-dependent checkpoints.
- Live success/failure branches are supported; no forced result/state mutation.

## Acceptance

A check scenario cannot pass without actual dice/check evidence. Internal/test-engineering player text is rejected.

---

# TC-JRNY-02 — One Scenario / Deterministic + Live + Replay Executors

## Objective

Use one scenario spec and one evidence schema for all test modes.

## Required behavior

- Deterministic mode uses controlled provider and seeded dice through adapters.
- Live mode uses real GM LLM plus isolated player simulator.
- Accepted live run can generate a replay cassette.
- Replay has no live fallback and fails on unexpected/missing external calls.

## Acceptance

Functional and live evidence cite the same scenario ID/version and checkpoints.

---

# TC-VS-KNOW-01 — Homecoming Knowledge/NoSpoiler Vertical Slice

## Assigned acceptance IDs

DA-KNOW-02, DA-KNOW-04, DA-KNOW-05, DA-SPOIL-01, DA-SPOIL-02, DA-SPOIL-04, DA-PIPE-01, DA-PIPE-02.

## User journey

Run `JRNY-HOME-01-athena-knowledge-reveal` from fresh character creation through observation, technical action, actual roll/check, reveal or failure branch, and later-turn continuity.

## Implementation scope

Implement any missing model/storage/projection/plugin/tool/prompt/trace pieces required to close the journey. Foundation-only output is Partial.

This vertical card must not be assigned as a single code writer. The lead must
split it into WorkGraph lanes with disjoint write sets before dispatch.

## Ownership

```yaml
read_set:
  - docs/epics/EPIC-KNOWLEDGE-MEMORY-NPC-RUNTIME/AcceptanceLedger.md
  - docs/epics/EPIC-KNOWLEDGE-MEMORY-NPC-RUNTIME/scenarios/JRNY-HOME-01-athena-knowledge-reveal.yaml
  - crates/trpg-model/src/knowledge.rs
  - crates/trpg-db/src/lib.rs
  - crates/trpg-runtime/src/knowledge_projection.rs
  - crates/trpg-gm/src/plugin/**
  - crates/trpg-gm/src/turn_loop.rs
  - crates/trpg-harness/**
write_set:
  - assigned by WorkGraph split, not this monolithic card
hotspot_leases:
  - assigned by WorkGraph split
generated_outputs:
  - assigned by WorkGraph split
migration_slot: assigned by serial migration lane when needed
validation_profile: journey-knowledge-nospoiler
```

## Done when

- before reveal: hidden truth absent from player prompt/output/memory;
- natural technical action reaches actual check/dice;
- success/reveal path commits player knowledge;
- failure path does not reveal;
- later turn/reload consumes correct knowledge;
- unrelated NPC knowledge remains unchanged;
- deterministic, live, and replay evidence use the same scenario.

---

# TC-VS-NPC-01 — Homecoming NPC Mind/Relationship Vertical Slice

## Assigned acceptance IDs

DA-NPC-01..05, DA-MEM-02, DA-PIPE-02.

## User journey

Run `JRNY-HOME-02-odessa-social-memory`: create character, reach the NPC through normal play, establish baseline dialogue, ask about known secret, observe withholding, perform a natural relationship-changing action, and talk again.

## Ownership

```yaml
read_set:
  - docs/epics/EPIC-KNOWLEDGE-MEMORY-NPC-RUNTIME/AcceptanceLedger.md
  - docs/epics/EPIC-KNOWLEDGE-MEMORY-NPC-RUNTIME/scenarios/JRNY-HOME-02-odessa-social-memory.yaml
  - crates/trpg-model/src/npc_*.rs
  - crates/trpg-runtime/src/npc_*.rs
  - crates/trpg-db/src/lib.rs
  - crates/trpg-gm/**
write_set:
  - assigned by WorkGraph split, not this monolithic card
hotspot_leases:
  - assigned by WorkGraph split
generated_outputs:
  - assigned by WorkGraph split
migration_slot: assigned by serial migration lane when needed
validation_profile: journey-npc-social-memory
```

## Done when

- NPC identity/profile/mind are stable and source-backed;
- NPC cannot speak from unknown facts;
- NPC may know but withhold a fact;
- player action produces evidence-backed relationship delta;
- subsequent dialogue/action changes observably according to profile + relationship;
- no spoiler is introduced;
- reload/revisit retains the relationship and knowledge.

---

# TC-VS-MEM-01 — Committed Memory and Reload Vertical Slice

## Assigned acceptance IDs

DA-MEM-01, DA-MEM-03, DA-PIPE-03 plus continuity portions of knowledge/NPC rows.

## User journey

Use the same accepted knowledge/social journeys, terminate the process, restart, and continue through public CLI/API. Do not seed memory directly.

## Ownership

```yaml
read_set:
  - docs/epics/EPIC-KNOWLEDGE-MEMORY-NPC-RUNTIME/AcceptanceLedger.md
  - accepted knowledge/social journey evidence
  - crates/trpg-runtime/src/memory_proposal.rs
  - crates/trpg-runtime/src/relationship_extraction.rs
  - crates/trpg-db/src/lib.rs
  - crates/trpg-harness/**
write_set:
  - assigned by WorkGraph split, not this monolithic card
hotspot_leases:
  - assigned by WorkGraph split
generated_outputs:
  - assigned by WorkGraph split
migration_slot: assigned by serial migration lane when needed
validation_profile: journey-memory-reload
```

## Done when

- extraction uses committed turns and produces proposals;
- accepted proposals carry evidence and are idempotent;
- restarted runtime retrieves the committed fact/relationship;
- GM/NPC later behavior uses it correctly;
- flight recorder links original action, event, projection, and later consumption.

---

# TC-PIPE-03 — Production Flight Recorder and Verifier Provenance

## Assigned acceptance IDs

DA-SPOIL-03, DA-PIPE-01, DA-PIPE-03.

## Objective

Close the production trace/provenance gap left after the HOME-01/02/03 vertical
journeys. The system must prove which views were loaded, what the context policy
plugins contributed or filtered, what the model-visible prompt received, what
the verifier-private view inspected, what the verifier found, and which
ledger-impacting events/projections were committed.

## Required behavior

- `execute_turn` loads GMTruthView, PlayerKnowledgeView, and active NpcMindViews
  before context policy plugins run, and records that ordering in a traceable
  artifact.
- The model-visible prompt/context manifest excludes player-unknown secrets that
  are present in VerifierPrivateView.
- VerifierPrivateView may inspect secrets and hidden-fact markers to detect
  leaks, but those secret terms must not enter the model-visible prompt.
- NoSpoiler / context policy plugin contributions and filtered blocks are
  recorded with enough provenance to explain why a fact was withheld or allowed.
- Ledger-impacting events and projection updates are linked to the same turn
  trace so acceptance evidence can connect prompt/view decisions to committed
  state.
- Tests must include at least one fail-closed negative control where a secret
  appears in model-visible context/prompt and is rejected.

## Ownership

```yaml
read_set:
  - docs/epics/EPIC-KNOWLEDGE-MEMORY-NPC-RUNTIME/AcceptanceLedger.md
  - accepted HOME-01/HOME-02/HOME-03 journey evidence
  - crates/trpg-gm/src/**
  - crates/trpg-runtime/src/knowledge_projection.rs
  - crates/trpg-runtime/src/npc_mind.rs
  - crates/trpg-model/src/knowledge*.rs
  - crates/trpg-harness/**
write_set:
  - crates/trpg-gm/src/**
  - crates/trpg-gm/tests/**
  - crates/trpg-runtime/src/knowledge_projection.rs
  - crates/trpg-runtime/src/npc_mind.rs
  - crates/trpg-runtime/tests/**
  - crates/trpg-model/src/knowledge*.rs
  - crates/trpg-model/tests/**
  - crates/trpg-harness/src/lib.rs
  - crates/trpg-harness/src/main.rs
  - crates/trpg-harness/tests/**
  - docs/epics/EPIC-KNOWLEDGE-MEMORY-NPC-RUNTIME/scenarios/**
hotspot_leases:
  - gm_turn_loop
  - gm_plugin_registry
  - harness_entrypoint
generated_outputs:
  - any new trace/provenance scenario or fixture needed to prove the behavior
migration_slot: none
validation_profile: production-flight-recorder-provenance
```

## Done when

- DA-SPOIL-03 has a production test proving verifier-private secrets are
  available to the verifier but absent from model-visible prompt/context.
- DA-PIPE-01 has evidence that view loading occurs before context policy plugin
  execution for GM/player/NPC views.
- DA-PIPE-03 has a production trace/provenance artifact covering knowledge view,
  plugin contributions, filtered context, verifier findings, and
  ledger-impacting events.
- The relevant HOME journey or harness checkpoint consumes the trace artifact
  rather than relying only on fixture-authored trace strings.
- Focused tests and at least one connected journey/harness check pass; negative
  controls fail closed.

---

# TC-KNOW-FOUND-01 — WorldFact / KnowledgeEdge Holder Roundtrip

## Assigned acceptance IDs

DA-KNOW-01, DA-KNOW-03.

## Objective

Close the holder-specific knowledge foundation that the later NPC mind work
depends on. The system must prove that fact identity (`WorldFact` / stable
fact id / source truth) is separate from holder state (`KnowledgeEdge`), and
that player_party plus multiple NPC holders can legitimately diverge on the
same fact.

## Required behavior

- First review the current model, DB, migrations, runtime projections, and tests
  before implementing anything. If the implementation already exists, tighten
  evidence and ledger notes instead of duplicating abstractions.
- `WorldFact`/fact identity is not copied per holder. Holder state lives in
  `KnowledgeEdge` rows keyed by `(fact_id, holder_kind, holder_id)` or the
  current equivalent contract.
- The same fact can have at least these distinct holder states in durable
  storage and projection tests: GM/source truth, player_party unknown or
  known_true, NPC A knows_true, NPC B believes_false or unknown.
- `PlayerKnowledgeView` and `NpcKnowledgeView`/`NpcMindView` must project from
  holder-specific edges; a player reveal must not grant all NPCs, and an NPC
  knowledge update must not reveal to the player_party.
- Any NPC holder path must use the accepted actor identity contract. If a
  stable NPC actor id cannot be proven, stop and write a blocker handoff rather
  than inventing a holder id.

## Ownership

```yaml
read_set:
  - docs/epics/EPIC-KNOWLEDGE-MEMORY-NPC-RUNTIME/AcceptanceLedger.md
  - accepted HOME-01/HOME-02 journey evidence
  - crates/trpg-model/src/knowledge.rs
  - crates/trpg-model/src/npc_mind.rs
  - crates/trpg-db/src/lib.rs
  - crates/trpg-runtime/src/knowledge_projection.rs
  - crates/trpg-runtime/src/npc_mind.rs
  - migrations/0033_knowledge_edges_v1_fields.sql
  - migrations/0034_knowledge_edges_npc_holders.sql
  - crates/trpg-db/tests/live_knowledge_edges.rs
  - crates/trpg-db/tests/live_npc_mind_view.rs
  - crates/trpg-model/tests/**
  - crates/trpg-runtime/tests/**
write_set:
  - crates/trpg-model/src/knowledge.rs
  - crates/trpg-model/src/npc_mind.rs
  - crates/trpg-model/tests/**
  - crates/trpg-db/src/lib.rs
  - crates/trpg-db/tests/**
  - crates/trpg-runtime/src/knowledge_projection.rs
  - crates/trpg-runtime/src/npc_mind.rs
  - crates/trpg-runtime/tests/**
  - docs/epics/EPIC-KNOWLEDGE-MEMORY-NPC-RUNTIME/scenarios/**
hotspot_leases:
  - knowledge_model
  - knowledge_db_projection
  - npc_mind_projection
generated_outputs:
  - any focused scenario/fixture needed to prove holder divergence
migration_slot: none
validation_profile: knowledge-holder-roundtrip
```

## Done when

- DA-KNOW-01 has model/storage roundtrip evidence proving fact identity and
  holder state are separate.
- DA-KNOW-03 has model/runtime/DB evidence proving NPC-holder-specific knowledge
  or mind projection divergence for at least two NPC holders and player_party.
- Negative controls fail closed: missing/unstable actor identity cannot produce
  a durable NPC holder edge; player_party reveal does not grant NPC knowledge;
  NPC-held secret does not enter player projection.
- Focused tests pass, and any connected HOME journey or harness checkpoint used
  for evidence is deterministic/replay friendly.

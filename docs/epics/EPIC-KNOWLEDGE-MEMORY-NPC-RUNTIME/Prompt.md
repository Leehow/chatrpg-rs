# Prompt — EPIC-KNOWLEDGE-MEMORY-NPC-RUNTIME

## Goal

Implement the Knowledge / Memory Runtime foundation for chatrpg-rs so that:

1. The system distinguishes world truth from who knows, believes, suspects, or ignores that truth.
2. Player-facing narration cannot leak player-unknown facts.
3. NPCs speak and act from their own knowledge, beliefs, persona, relationship, and current behavior plan.
4. Memory extraction updates facts, knowledge, relationships, and NPC mind proposals from committed turns only.
5. The runtime can prove these behaviors through deterministic tests, runtime journeys, adversarial tests, and golden TRPG scenarios.

## Non-goals

- Do not build a universal rules compiler.
- Do not solve every parser quality issue in this epic.
- Do not rewrite the whole turn pipeline unless an acceptance row explicitly requires it.
- Do not replace all memory systems at once; compatible incremental migration is allowed.
- Do not make prompt policy the only spoiler guard.
- Do not allow NPCs to read GMTruthView directly for speech/action.

## Final Product Shape

The final system has these runtime views:

```text
GMTruthView
PlayerKnowledgeView
NpcMindView(npc_id)
VerifierPrivateView
```

And these core objects:

```text
WorldFact
KnowledgeEdge
NpcProfile
NpcRelationship
NpcBehaviorPlan
```

And these policy/plugin capabilities:

```text
KnowledgeVisibilityGuard
NoSpoiler context filter + prompt block + verifier
MemoryExtraction proposals
NpcRelationshipTracker proposals
NpcMindUpdater proposals
NpcBehaviorDirector plan generation
```

## Done When

The epic is Done only when `AcceptanceLedger.md` has no rows in Missing, Partial, Implemented, Untested, or BlockedByPrerequisite status.

Rows may be Deferred only if the approved scope explicitly defers them or the user approves deferral.

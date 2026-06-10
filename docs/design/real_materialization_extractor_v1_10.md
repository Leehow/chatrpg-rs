# v1.10 Real Materialization Extractor & Binding Verifier

## Purpose

v1.10 turns retrieved rules and module materials into runtime state. Earlier versions could find text and place it in context, but actor/object/ability parameters could still remain seeded placeholders. This version introduces a materialization demand pipeline: demand, evidence bundle, typed extraction, binding packet, verification, and writeback.

## Pipeline

```text
TurnOrchestrator / Combat / Object / Ability
  → MaterializationDemand
  → SourceEvidenceBundle
  → Typed Extractor
  → RuleBindingPacket
  → BindingVerificationResult
  → Runtime writeback
  → BP2/BP3 projection
```

## Source priority

1. Existing runtime state.
2. Module NPC/object/vehicle/stat cards.
3. Ruleset exact entries.
4. Ruleset archetypes or mook templates.
5. Learned packets or table rulings.
6. Provisional fallback with audit requirement.

## Cache policy

Stable definitions may graduate to BP2. Current HP, status, held objects, cooldowns, pending materialization demands, and binding verification results remain BP3.

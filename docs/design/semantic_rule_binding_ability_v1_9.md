# v1.9 Semantic Rule Binding & Ability Hydration Kernel

## Intent

v1.9 makes semantic materialization the primary path for actor, object, ability, check, and effect parameters. Search/Tantivy/grep are candidate-retrieval tools only; routing and extraction are semantic structured-output tools.

## New crates

- `trpg-semantics`: semantic turn classification, materialization request planning, semantic query planning, rule binding packets.
- `trpg-ability`: ability definitions, instances, triggers, activation contracts, BP3 ability graph.

## Core flow

```text
Player/world event
  → SemanticIntentService
  → MaterializationRequest
  → SemanticQueryPlanner
  → Tantivy/grep candidate retrieval
  → Semantic extractor
  → RuleBindingPacket
  → Actor/Object/Ability/Check/Effect runtime writeback
```

## Ability flow

```text
Player uses/mentions a named ability
  → AbilityMaterializer
  → AbilityDefinition
  → AbilityInstance
  → AbilityActivationContract
  → optional CheckContract/EffectContract/ObjectPatch/StatePatch
```

## Cache policy

- BP1: stable semantic/rule-binding protocol.
- BP2: actor templates, object definitions, ability names/summaries and locators.
- BP3: active ability instances, current uses/cooldowns, activations, triggers, recent rule-binding packets.

## Keyword policy

Keywords are not a normal business route. Lexical fallback is disabled by default for routing and, when enabled, must record `classifier=fallback_lexical_audit`.

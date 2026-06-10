# Rule Steward + Character Steward implementation v1.16.0

This patch adds a dedicated Rule Steward Agent and promotes character onboarding to a first-class playability requirement.

## What changed

### 1. New `trpg-rule-agent` crate

Added `crates/trpg-rule-agent` with `RuleStewardAgent`:

- `assist(RuleNeed) -> RuleAssist`
- `playability_gate(ruleset_id, module_id) -> PlayabilityGateReport`
- `character_onboarding_pack(ruleset_id) -> Option<CharacterOnboardingPack>`

The agent coordinates existing project assets instead of asking the main GM to improvise rule lookup. Its first implementation searches learned packets, Tantivy search, book locators, and a local `trpg-rg-fallback` scan over `data/parsed` and `data/markdown` when indexed search is thin. It returns source refs, context blocks, and explicit unresolved gaps rather than synthetic DV/HP/damage/resource values.

### 2. Typed Rule Steward contracts

Added model contracts in `trpg-model`:

- `RuleNeed`
- `RuleAssist`
- `RuleKernel`
- `RuleKernelPatch`
- `RuleAgentRun`
- `PlayabilityGateReport`
- `RuleGap`

`RuleAssistStatus` distinguishes `source_backed_exact`, `source_backed_partial`, `learned_stable`, `provisional_table_ruling`, `unresolved_needs_source`, `conflict_needs_review`, and `not_a_rules_problem`.

### 3. Character onboarding is now a ruleset artifact

Added `CharacterOnboardingPack`, composed of:

- `CharacterSheetTemplate`
- `CharacterCreationFlow`
- `CharacterOptionCatalog`
- `DerivedFormulaPack`
- `StarterCharacterPack`
- import mapping profile
- validation profile
- runtime binding profile
- runtime bindings

The parser now generates this pack during `parse-all` from character creation/sheet/derived-value source pages, the existing character template, book map, and starter procedures. The prompt is framed as a Character Steward skill: it must optimize for fast playable character creation, option locators, source-backed derived formulas, starter/pregen paths, and runtime bindings; it must not invent HP, damage, skill totals, resource values, or formulas.

### 4. RuleKernel is now persisted and loaded into runtime prefix context

`parse_rulebook()` now builds a `RuleKernel` from GM onboarding, starter procedures, and the CharacterOnboardingPack. The runtime loads active `RuleKernel` and `CharacterOnboardingPack` into stable prefix context when `TRPG_RULE_STEWARD_ENABLE_V116=true`.

### 5. Parse artifacts written to disk

`parse-all` now writes:

```text
data/parsed/characters/{ruleset_id}.character_onboarding_pack.json
data/parsed/characters/{ruleset_id}.character_sheet_template.json
data/parsed/characters/{ruleset_id}.character_creation_flow.json
data/parsed/characters/{ruleset_id}.character_option_catalogs.json
data/parsed/characters/{ruleset_id}.derived_formulas.json
data/parsed/characters/{ruleset_id}.starter_character_pack.json

data/parsed/rules/{ruleset_id}.rule_kernel.json
```

### 6. PostgreSQL migration 0025

Added:

- `rule_agent_runs`
- `rule_kernels`
- `rule_kernel_patches`
- `character_onboarding_packs`
- `character_creation_flows`
- `character_option_catalogs`
- `starter_character_packs`
- `rule_entity_locators`
- `mechanical_source_packs`
- `playability_gate_reports`

`Db::upsert_rule_bundle` now persists `rule_kernel` and `character_onboarding_packs` alongside existing rule bundle artifacts.

### 7. Character creation now uses CharacterOnboardingPack

`RuntimeEngine::character_creation_messages()` loads CharacterOnboardingPack first and uses its template, creation flows, derived formula pack, starter pack, runtime bindings, and validation profile as the authority for legal character creation.

API character postprocessing now saves drafts as:

- `ready` when validation is clean;
- `draft_needs_rules_source` when required fields or source-backed mechanics are still missing.

### 8. Runtime playability gate

`RuntimeEngine::start_session()` now checks `TRPG_CHARACTER_ONBOARDING_REQUIRED` (default `true`). When enabled, it blocks session creation if the selected ruleset is missing:

- `RuleKernel`
- `CharacterOnboardingPack`
- `CharacterSheetTemplate.fields`
- `CharacterCreationFlow`
- `CharacterRuntimeBinding`
- `StarterCharacterPack`

If `TRPG_PLAYABILITY_GATE_BLOCKING=true`, it also blocks when a selected module lacks a first-session prep packet.

### 9. CLI and API entry points

CLI:

```bash
trpg rules query --ruleset <ruleset_id> --module <module_id> "ranged attack damage armor"
trpg rules playability --ruleset <ruleset_id> --module <module_id>
trpg rules character-pack --ruleset <ruleset_id>
trpg rules bp1-patches --ruleset <ruleset_id>
trpg inspect character-onboarding --ruleset <ruleset_id>
```

API:

```text
POST /api/rules/steward/assist
POST /api/rules/playability
GET  /api/rules/character-onboarding/{ruleset_id}
```

### 10. Reusable skill specs

Added skill spec assets under `data/agent/skills/`:

- `rule_steward.ruleset_first_pass.v1.json`
- `rule_steward.core_kernel_distill.v1.json`
- `rule_steward.character_onboarding.v1.json`
- `rule_steward.mechanical_source_pack.v1.json`
- `rule_steward.learning_promotion.v1.json`

These are declarative skill manifests for the Rule Steward’s fixed workflows. They reuse existing retrieval/materialization infrastructure instead of creating a parallel tool stack.

## Source-backed policy

This patch keeps the v1.15.4 source-backed materialization principle. Rule Steward can return `UnresolvedNeedsSource`; it must not invent HP, DV/DC, damage, SP/AC, SAN, Chaos, Harm, ammunition, derived character values, or other mechanical values.

## Environment flags

```env
TRPG_RULE_STEWARD_ENABLE_V116=true
TRPG_RULE_STEWARD_AUTO_QUERY=true
TRPG_CHARACTER_ONBOARDING_REQUIRED=true
TRPG_PLAYABILITY_GATE_BLOCKING=false
TRPG_RULE_KERNEL_PATCH_AUTO_APPLY=false
TRPG_RULE_STEWARD_STRICT_SOURCE_BACKED=true
```

## Local verification

The execution environment used for this patch did not contain `cargo`, `rustc`, or `rustfmt`, so this patch was not locally compiled here. Run locally:

```bash
cargo fmt
cargo check --workspace
cargo test --workspace
cargo run -p trpg-cli -- parse-all --force --pdf-backend oxidize
cargo run -p trpg-cli -- rules playability --ruleset cyberpunk_red --module cyberpunk_red.homecoming
cargo run -p trpg-cli -- rules character-pack --ruleset cyberpunk_red
cargo run -p trpg-cli -- rules query --ruleset cyberpunk_red --module cyberpunk_red.homecoming "attack Athena with a gun; need target HP, armor/SP, hit model, weapon damage"
```

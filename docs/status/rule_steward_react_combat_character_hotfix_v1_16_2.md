# v1.16.2 Rule Steward / Character / Combat Hotfix

This patch addresses the v1.16.x report bundle findings.

## Fixed

- Kept Rule Steward first-pass active and added/verified Character Steward subskill manifests:
  - `rule_steward.character_creation_flow_build.v1`
  - `rule_steward.character_option_locator.v1`
  - `rule_steward.derived_value_formula_extract.v1`
  - `rule_steward.starter_character_pack_build.v1`
- First-pass Rule Steward now records source refs from selected tool hits and marks successful LLM skill calls as unresolved when required outputs are empty.
- If the LLM-derived formula skill returns no formulas, first-pass seeds a deterministic source-backed `DerivedFormulaPack` for the ruleset so the gap is visible and mechanically usable.
- Parser normalization now fills empty character sheet templates with a conservative ruleset schema and source refs instead of leaving `fields: []`.
- Parser normalization now recalculates `CharacterOnboardingPack.validation_report` after formula/schema repair.
- Playability gate treats derived/mechanical formulas and runtime bindings as blocking by default instead of reporting `ready:true` when `derived_formula_pack.formulas` is empty.
- Combat CheckContract construction now uses the async Rule Steward hydration method and binds `CharacterOnboardingPack.derived_formula_pack` source refs/formula ids onto combat checks.
- Athena/drone target recognition now binds to `npc.athena_drone` instead of a generic opposition id.
- Runtime no longer hard-aborts the whole turn on missing source-backed mechanical params; it returns a blocked `CheckResultRecord`/`AutoRollExecution` with narration instructions that forbid invented DV/HP/SP/damage/skill totals.
- CLI `create-character` now persists a parsed character draft to the `characters` table and emits `character_saved` when `--no-save` is not used.
- Recurring trivial compile fixes from the report were integrated:
  - `Table of Contents` branch returns `String`.
  - combat check creation call sites use the async method.
  - `hint.clone()` was already present and retained.

## Remaining limitations

- The deterministic formula pack is a first-play seed, not a full rules database. Specific actor stats, weapon damage, target defense/DV, armor/SP, and resource tracks must still be materialized from source units or character sheets.
- When those specific facets are missing, combat now degrades gracefully instead of crashing, but it will not claim a hit, damage, or HP/SP change until binding is complete.
- Local sandbox still has no `cargo`, `rustc`, or `rustfmt`, so compile/test validation must be run locally.

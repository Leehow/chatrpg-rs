# v1.16.2 Rule Steward Mechanics / Character Onboarding Hotfix

This hotfix responds to `chatrpg_v1.16.x_report_bundle.tar.gz`.

## Fixed

1. **Recurring compile blockers**
   - `trpg-combat`: combat action contract creation now has an async `CombatAgent::make_combat_check_contract(...)` path that can read `self.db`; the frame-opening and active-frame call sites compile against that method.
   - `trpg-parser`: `Table of Contents` title branch now returns `String`.
   - The `normalize_semantic_hint_for_frame` paths already use `hint.clone()` in this package.

2. **First-pass Character Steward skills are mounted as separate skills**
   - Added/used:
     - `rule_steward.character_creation_flow_build.v1`
     - `rule_steward.character_option_locator.v1`
     - `rule_steward.derived_value_formula_extract.v1`
     - `rule_steward.starter_character_pack_build.v1`
   - `RuleStewardFirstPassAgent` runs these subskills before assembling `CharacterOnboardingPack`.
   - `RuleAgentRun.unresolved_count` is no longer always zero for empty subskill output; empty fields/formulas/flows are counted as unresolved.
   - `source_refs_read` is populated from selected page ids when skill source slices are available.

3. **Derived formula pack no longer silently stays empty**
   - Parser normalizes `CharacterOnboardingPack` and seeds first-play mechanical formulas from source-located rulebook pages when the LLM extractor returns no formulas.
   - The seeded formulas are not exact fabricated numeric facets. They are source-backed formulas such as:
     - core check total;
     - attack total;
     - damage/effect source lookup;
     - armor/SP or AC mitigation;
     - HP/SAN/Chaos/Harm resource routing where applicable.
   - Exact actor stats, weapon damage dice, target DV/AC/SP/HP, and resource values still require source-backed facets/materialization.

4. **Character template skeletons are repaired**
   - Empty `fields` from LLM extraction are normalized into ruleset-aware starter schema fields instead of leaving `field_ids: []` forever.
   - `CharacterOnboardingPack.sheet_template.source_refs` is populated from rulebook source refs.
   - Runtime bindings are inferred from normalized fields.

5. **Playability gate is stricter and actionable**
   - Missing derived formulas are now a blocking gap by default via `TRPG_PLAYABILITY_REQUIRE_DERIVED_FORMULAS=true`.
   - Missing runtime bindings are also blocking.
   - Repair skill names now point to actual mounted Rule Steward skill IDs.

6. **Combat no longer hard-aborts the whole turn on missing params**
   - Missing source-backed params now create a blocked/provisional `CheckResultRecord` and `AutoRollExecution` with `roll_policy=blocked_missing_source` instead of returning `Err`.
   - CLI/API `[roll]` context tells the narrator not to invent hit/miss, damage, HP, SP, DV, or skill totals when mechanics are blocked.
   - `TRPG_GRACEFUL_DEGRADE_MISSING_SOURCE_PARAMS=true` is the default product behavior.

7. **Combat CheckContract consumes formula pack evidence**
   - Combat checks read the active `CharacterOnboardingPack.derived_formula_pack` from DB.
   - Formula pack source refs are attached to the CheckContract.
   - Bare dice such as `1d10` are converted to explicit source-bound forms such as `1d10+0` without fabricating actor/skill/weapon bonuses.
   - Homecoming technical interactions can bind source-backed DV 12/14 for hacking/cabling actions when the user input indicates those options.

## Still intentionally unresolved

- This does not invent exact character stats, weapon damage, target HP/SP, or Cyberpunk range DV when those are not hydrated from source-backed facets.
- Attack narration may proceed, but final hit/damage/HP writeback remains blocked or provisional until actor/weapon/target facets are materialized.
- The sandbox used to produce this hotfix has no `cargo`/`rustc`, so compile verification must be run locally.

## Suggested validation

```bash
cargo fmt
cargo check --workspace
cargo test --workspace

TRPG_RULE_STEWARD_FIRST_PASS=true \
TRPG_PLAYABILITY_REQUIRE_DERIVED_FORMULAS=true \
TRPG_GRACEFUL_DEGRADE_MISSING_SOURCE_PARAMS=true \
cargo run -p trpg-cli -- parse-all --force --pdf-backend oxidize

cargo run -p trpg-cli -- rules playability \
  --ruleset cyberpunk_red \
  --module cyberpunk_red.homecoming

cargo run -p trpg-cli -- turn \
  --ruleset cyberpunk_red \
  --module cyberpunk_red.homecoming \
  --input "我拔出重型手枪朝无人机开火"
```

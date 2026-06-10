# v1.16.2 — Rule Steward ReAct Formula + Combat Graceful-Degrade Hotfix

This hotfix addresses the v1.16.x report bundle findings:

- first-pass Rule Steward produced empty `derived_formula_pack.formulas`;
- playability could report `ready:true` while formula pack was absent;
- combat attacks aborted the whole turn with `source-backed mechanical parameters are missing`;
- character onboarding remained too skeleton-like when template extraction returned no fields;
- character-related Rule Steward skills were not individually mounted.

## Main changes

### Character Steward subskills mounted in first-pass

`RuleStewardFirstPassAgent::run_rulebook_first_pass()` now runs dedicated character subskills before assembling the final pack:

- `rule_steward.character_creation_flow_build.v1`
- `rule_steward.character_option_locator.v1`
- `rule_steward.derived_value_formula_extract.v1`
- `rule_steward.starter_character_pack_build.v1`
- `rule_steward.character_onboarding.v1`

Each subskill records a `RuleAgentRun`, selected source IDs, and unresolved count. Empty outputs no longer count as clean success.

### Derived formula pack seeded when extraction is empty

Parser normalization now ensures a first-play mechanical formula pack exists when the rulebook source contains matching rule pages. Seeds are source-referenced and conservative:

- Cyberpunk RED: skill checks, ranged attack formula locator, weapon damage/effect locator;
- D&D 5e: d20 check, attack vs AC, source-bound damage;
- Sword World 2.5: 2d6 skill checks, table-driven damage locator;
- CoC/BRP: percentile roll-under, HP/SAN/resource locators;
- Triangle Agency: 6d4 conflict, Harm/Chaos/Stability tracks.

These are formula locators, not fabricated actor/NPC values. Specific character, weapon, NPC, module, or table facets still override them.

### Playability gate no longer hides missing formulas

`TRPG_PLAYABILITY_REQUIRE_DERIVED_FORMULAS` defaults to true. Missing formula packs now create a blocking gap, not just a warning.

### Combat CheckContract binding

Combat now builds CheckContracts through an async method that reads the `CharacterOnboardingPack` / `DerivedFormulaPack` and attaches source refs, formula IDs, and Rule Steward advice refs. This prevents a source-backed formula pack from being invisible to runtime checks.

### Combat no longer hard-aborts on missing source-backed params

`execute_system_roll_bundle`, `execute_agent_roll`, and `resolve_check_with_input` now return a blocked `CheckResultRecord` instead of raising an error when strict source-backed parameters are missing. The result has:

- `roll_policy = blocked_missing_source`
- `roll.visibility = NoRoll`
- `outcome.blocked = true`
- no state patches

CLI/API roll context tags now tell the narrator not to invent DV, HP, SP, damage, or skills, and to surface that Rule Steward must hydrate missing facets.

### Character template skeleton repair

If LLM extraction returns a template with no fields, parser normalization fills a conservative ruleset-specific schema skeleton and keeps mechanical values source-required. This prevents `field_ids: []` starter packs.

## Remaining limitations

This patch does not fully solve source-backed NPC/weapon/target materialization. It prevents hard crashes and seeds formula locators, but exact weapon damage, actor REF/skill, target range DV, target HP/SP/AC, and module-specific stat blocks still need materialization for fully verified combat results.

## Suggested validation

```bash
cargo fmt
cargo check --workspace
cargo test --workspace

TRPG_RULE_STEWARD_FIRST_PASS=true \
TRPG_PLAYABILITY_REQUIRE_DERIVED_FORMULAS=true \
cargo run -p trpg-cli -- parse-all --force --pdf-backend oxidize

cargo run -p trpg-cli -- rules playability \
  --ruleset cyberpunk_red \
  --module cyberpunk_red.homecoming

cargo run -p trpg-cli -- turn \
  --ruleset cyberpunk_red \
  --module cyberpunk_red.homecoming \
  --input "我拔出重型手枪朝无人机开火"
```

Expected improvements:

- `derived_formula_pack.formulas` should be non-empty after parse;
- playability should not say `ready:true` while formulas are missing;
- attack should no longer terminate the process with an unhandled error;
- if exact actor/weapon/target facets are still missing, the turn emits a blocked mechanical result instead of fabricating numbers.

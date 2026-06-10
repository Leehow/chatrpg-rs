# Rule Steward Agent v1.16

## Goal

Rule Steward Agent is the source-backed rules collaborator for the Main GM. It owns ruleset onboarding, rule lookup, character onboarding, playability gating, audit, and future BP1 kernel patch proposals.

The Main GM remains responsible for player-facing scene direction and narration. Rule Steward returns typed `RuleAssist` packets, `ContextBlock`s, `RuleKernel`s, and `CharacterOnboardingPack`s; it does not narrate outcomes or invent missing mechanics.

## Minimum playable gate

A game is considered startable only when these artifacts exist:

- `RuleKernel`: minimum source-backed BP1 rule core.
- `CharacterOnboardingPack`: sheet template, creation flows, option locators, derived formulas, starter/pregen path, runtime bindings.
- `ModulePrepPacket`: current/first-session packet for the selected module.
- At least one valid or draft-valid PC route.

`TRPG_PLAYABILITY_GATE_BLOCKING=false` keeps this as an audit gate by default. Product paths can make it hard-blocking later.

## New artifacts

`parse-all` now writes:

```text
data/parsed/characters/{ruleset_id}.character_onboarding_pack.json
data/parsed/characters/{ruleset_id}.character_sheet_template.json
data/parsed/characters/{ruleset_id}.character_creation_flow.json
data/parsed/characters/{ruleset_id}.derived_formulas.json
data/parsed/characters/{ruleset_id}.starter_character_pack.json
```

and persists:

```text
rule_kernels
character_onboarding_packs
character_creation_flows
character_option_catalogs
starter_character_packs
rule_agent_runs
rule_kernel_patches
playability_gate_reports
```

## Runtime context

Runtime loads the active `RuleKernel` and `CharacterOnboardingPack` into BP1 as rare-changing Rule Steward context. Per-turn dynamic lookups remain BP3 through the existing runtime auto-search path.

## RuleStewardAgent commands

```bash
trpg rules query --ruleset cyberpunk_red "ranged attack damage armor"
trpg rules character-pack --ruleset cyberpunk_red
trpg rules playability --ruleset cyberpunk_red --module cyberpunk_red.homecoming
trpg rules bp1-patches --ruleset cyberpunk_red
```

## API

```text
POST /api/rules/steward/assist
POST /api/rules/playability
GET  /api/rules/character-onboarding/{ruleset_id}
```

## Skills

Reusable skills are declared under `data/agent/skills/`:

- `rule_steward.ruleset_first_pass.v1`
- `rule_steward.core_kernel_distill.v1`
- `rule_steward.character_onboarding.v1`
- `rule_steward.mechanical_source_pack.v1`
- `rule_steward.learning_promotion.v1`

## Source policy

Numbers and mechanical state that affect runtime execution must be source-backed. If the Steward cannot find source-backed HP, DV/DC, damage, armor, skill total, resource value, condition effect, or derived formula, it returns an unresolved gap instead of writing a synthetic value.

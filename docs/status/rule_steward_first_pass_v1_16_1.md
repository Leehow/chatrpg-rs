# v1.16.1 Rule Steward First-Pass Hotfix

## Problem

v1.16.0 introduced `trpg-rule-agent` and CharacterOnboardingPack artifacts, but `parse-all` still used `ProjectParseService::parse_rulebook()` as the direct orchestrator for the first rulebook pass. The parser called the old extraction helpers itself and only produced artifacts that the Rule Steward Agent could consume later.

That meant the first parse was not truly using the new agent + reusable skill pipeline. The Rule Steward Agent existed for runtime/query/playability, not for the initial onboarding route.

## Fix

`parse_rulebook()` now routes the initial rulebook pass through `RuleStewardFirstPassAgent` by default.

Default:

```env
TRPG_RULE_STEWARD_FIRST_PASS=true
```

The first pass now records and executes these skills:

```text
rule_steward.ruleset_first_pass.v1
rule_steward.core_kernel_distill.v1
rule_steward.character_sheet_template_extraction.v1
rule_steward.character_onboarding.v1
rule_steward.ruleset_first_pass.v1/gm_onboarding
```

The parser still owns durable coercion into the stable `RuleBundle`, but source slice selection, skill manifest loading, LLM JSON extraction prompts, rg-style locator scan, and audit records now live in `trpg-rule-agent`.

## Code changes

- Added `RuleStewardFirstPassAgent` to `crates/trpg-rule-agent`.
- Added `RulebookFirstPassOutput` for raw skill outputs.
- Added `data/agent/skills/rule_steward.character_sheet_template_extraction.v1.json`.
- `trpg-parser` now depends on `trpg-rule-agent` and calls `run_rulebook_first_pass()` inside `parse_rulebook()`.
- `parse_config_hash` now includes `rule_steward_first_pass` and was bumped to `parser=v1.16.1` so old cached bundles do not silently mask the new route.
- CLI/API parse-all now report/include the first-pass flag.

## Remaining boundary

The first-pass agent produces raw JSON and audit traces. The parser still coerces those raw outputs into the existing strongly typed artifacts because the durable bundle writer, source index, material index, and fallback normalizers still live in `trpg-parser`. This avoids a circular rewrite while making the orchestration path correct.

## Validation to run locally

```bash
cargo fmt
cargo check --workspace
cargo test --workspace

TRPG_RULE_STEWARD_FIRST_PASS=true \
  cargo run -p trpg-cli -- parse-all --force --pdf-backend oxidize
```

Expected signs:

```text
rule steward first-pass skills: enabled
conversion_trace contains rule_steward_first_pass_used
rule_agent_runs contains parse_all.rulebook_first_pass rows
rule_kernels and character_onboarding_packs are populated from first-pass skill outputs or explicit fallback traces
```

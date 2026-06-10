# v1.15.4 Semantic Units + Source-backed Materialization Hotfix

## Problem addressed

The v1.15.3 pipeline still spent its default LLM ingestion budget on low-leverage fixed-window layout cleanup. That did not improve retrieval/materialization because the downstream system needs complete semantic units with useful tags, not cosmetic line wrapping.

The same build also allowed materialization and combat to proceed with synthetic defaults: e.g. runtime actor HP from label heuristics, default attack expressions, default DV/defense values, and default actor seeds. This let the system appear mechanically active while using values not grounded in parsed rulebook/module evidence.

## Parser/ingest changes

- `TRPG_INGEST_LLM_CLEAN` now defaults to `false`.
- The old LLM cleaning pass is no longer a cosmetic reflow pass. When enabled, it repairs only structural defects:
  - `encoding_artifact`
  - `table_columns_collapsed`
- `TRPG_INGEST_SEMANTIC_UNITS=true` defaults on.
- Deterministic semantic unit conditioning now runs after PDF extraction:
  - drops or downweights noise: contents, index, credits, copyright, page headers/footers;
  - builds one chunk per semantic unit using headings and structural boundaries;
  - classifies units by category: rule/procedure, table, stat block, scene, NPC/anomaly, clue/handout, location/scene, lore/guidance;
  - attaches rich metadata: `semantic_category`, `defined_entity`, `signal_class`, `gm_secret`, `visibility_hint`, `mechanics_tags`, `quality_flags`, `index_weight`;
  - writes audit/search artifacts under `data/parsed/source_units/*.semantic_units.jsonl` and `data/markdown/semantic_units/*.semantic_units.md`.
- Optional `TRPG_INGEST_LLM_SEMANTIC_WASH=true` can refine only difficult semantic units, especially oversized units, encoding artifacts, and collapsed tables.
- Rulebook/module chunk prompts now treat input as a semantic unit and explicitly forbid inventing missing numeric mechanical values.

## Retrieval/materialization changes

- Semantic units are written into `material_index` and JSONL search sources, so retrieval can target source-backed stat blocks, procedures, clues, scenes, and GM-only sections directly.
- `MaterializationService` now defaults to `TRPG_STRICT_SOURCE_BACKED_MATERIALIZATION=true`.
- Only `verified_exact` extraction results write runtime actor/object/ability/check/effect state under strict mode.
- `verified_partial`, `rejected_no_source`, and other incomplete extractions are logged to `material_hydration_events` as `blocked_missing_source_backed_parameters`; they do not mutate runtime parameter state.
- Actor profile writeback no longer fabricates HP from labels such as `boss`, `scav`, or `mook`.
- Ability profile writeback no longer fabricates manual activation/cost/target/effect defaults; missing fields remain missing.

## Runtime parameter/combat changes

- `TRPG_ALLOW_SYNTHETIC_ACTOR_SEEDS=false` defaults on. This now applies to PCs and NPCs; `TRPG_RUNTIME_PARAM_ALLOW_SYNTHETIC_NPC_SEEDS` is legacy/debug-only.
- Runtime actor parameters created before source-backed hydration are unresolved placeholders with null HP and missing-field markers, not fake stat sheets.
- Combat frames no longer default PC/NPC HP to 35/25 when actor parameters lack source-backed HP.
- Combat check contracts no longer default Cyberpunk attacks to `1d10+10` or DV 14. They use only the bare ruleset die shape as a roll-expression placeholder, and roll execution is blocked until source-backed actor/target/weapon/defense data is hydrated, unless an operator deliberately configures table override env vars.
- Hit follow-up damage no longer defaults to `3d6`, `1d8`, `1d10`, or `1d6`; a damage roll is created only when a source-backed damage expression or an explicit table override exists.
- `TRPG_FAIL_ON_MISSING_SOURCE_BACKED_PARAMS=true` defaults on. Automatic system rolls and legacy player-confirmed `roll` both fail loudly when source-backed mechanical parameters are missing, instead of rolling against fabricated values.
- Contest fallback no longer fabricates D&D/Sword World/Cyberpunk target numbers or a BRP 50% skill. Missing target/defense/ability values produce a provisional unresolved contest profile unless an explicit override env var is set.

## Compatibility switches

Temporary old behavior can still be enabled explicitly for debugging only:

```env
TRPG_ALLOW_SYNTHETIC_ACTOR_SEEDS=true
TRPG_STRICT_SOURCE_BACKED_MATERIALIZATION=false
TRPG_FAIL_ON_MISSING_SOURCE_BACKED_PARAMS=false
TRPG_COMBAT_DEFAULT_ATTACK_EXPR=1d10+10
TRPG_COMBAT_DEFAULT_ATTACK_DV=14
TRPG_CONTEST_DEFAULT_ATTACK_DV=14
TRPG_CONTEST_DEFAULT_PERCENTILE_SKILL=50
TRPG_CONTEST_DEFAULT_STATIC_TARGET=10
```

These switches are intentionally not part of the default product path.

## Validation notes

Static checks performed in this container:

- `TRPG_INGEST_LLM_CLEAN` default is false.
- semantic-unit artifacts and metadata paths are present.
- `threat_hp_from_label` no longer exists.
- hardcoded Cyberpunk `1d10+10`/DV 14 defaults are removed from product defaults and runtime combat contract creation.
- actor seed defaults are gated behind `TRPG_ALLOW_SYNTHETIC_ACTOR_SEEDS` and default off for PCs and NPCs.
- contest default target/skill values are unset unless explicitly provided as table overrides.

`cargo`, `rustc`, and `rustfmt` are not installed in this container, so local compilation and formatting still need to be run in a Rust environment.

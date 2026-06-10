# Object/Ability Schema Extraction — multi-pass, coupling-aware, formula-hooked

Status: design (2026-06-09). Author direction: items/spells/abilities should be SCHEMA INSTANCES
whose params HOOK (as JSON formulas) into the existing engine — same idea as the user's `deepwood_coc`
"挂钩参数" and the project's existing `chargen_compile`. NOT free-form on-demand LLM extraction.

## Problem
Items/weapons/spells/abilities have NO parse-time mechanical representation. The reader records long
lists (weapons/spells) only as `option_catalogs` locators (page refs, no params). So at play time the
materialization does free-form LLM extraction of opaque param strings — which is flaky (the ".38
revolver → damage null" saga: the LLM won't reliably read a headerless table cell and map it to a field).

Per deepwood_coc + chargen_compile: a weapon/spell should be an INSTANCE of its ruleset's category
SCHEMA, with typed slots whose values HOOK into the engine (damage → dice_core eval; attack_skill →
the actor's skill field_id; ammo → a resource_track; range → check_model/range table; spell cost →
the magic-points resource). Extracted ONCE at parse time, with 2 worked examples per category.

## Current state (grounded — read 2026-06-09)
- Reader `parallel.rs`: `plan_phase` (1 call on TOC) → FIXED 3 slices (resolution / character / gm),
  parallel → `GmRunKit`. The character slice emits `option_catalogs` with `locators` for long lists.
- `chargen_compile.rs` (THE PATTERN to mirror): focused 2nd pass; submit schema = one `derived_values`
  array of §4 records `{id, role, input_kind, recompute, result_type, expr, attr_derived, base,
  allocations, lookup_tables, depends_on, min, max, source_ref, status}`; reuses `nav_tools` +
  `read_layout` (duotext `.layout.md`); guardrail `finalize_compiled` round-trips each record through
  `trpg_formula::evaluate_chargen` → unresolved/None → `status=provisional`; augment-not-replace; 2 rounds.
- `RuleKernel` (trpg-model:1370): game_identity, play_loop, dice_core, check_model, contest_models,
  damage_effect_model, resource_tracks, character_sheet_schema, visibility_policy. NO object/ability schema.
- Materialization (trpg-material): on-demand free-form extract → `object_definitions.mechanical_profile`.
  Retrieval is now SOLID (AI minimal search_key → grep surfaces the aligned table row); only the free-form
  field-extraction is flaky.
- Resolution (trpg-object): `ensure_actor_weapon` → `find_materialized_object_def(ruleset, name)` → uses
  the materialized `mechanical_profile`.

## Design — discover → couple → extract (multi-pass, fully data-driven)
Rulesets differ wildly (Triangle Agency & CoC have NO spells; item params may not couple). So the
category structure is DISCOVERED, never assumed. Three passes; mirror chargen_compile's machinery.

### Pass D — DISCOVER (1 planning call, AFTER the reader has subsystem_map + character_template)
Inputs: `subsystem_map`, `character_template` (skill/resource/sheet field_ids), TOC, located pages.
Output JSON: the list of mechanical OBJECT/ABILITY CATEGORIES this ruleset ACTUALLY has — empty ones
omitted. Each: `{category_id, kind (weapon|spell|psychic|cyberware|gear|tool|...), source_pages,
schema_slots:[{slot, type (dice|number|skill_ref|resource_ref|range_ref|text), hook}], couples_to:[
character param/skill/resource_track field_ids it references]}`. (No spells found → no spell category.)

### Pass C — COUPLE (folded into D's output)
Group categories whose `couples_to` reference the SAME character params/tracks (e.g. all combat weapons
hook the same Fighting/Firearms skills + damage_effect_model) into ONE extraction UNIT, so a single
subagent fills them with a consistent field_id/track-name vocabulary. Uncoupled categories get their own
unit. Output: a list of EXTRACTION UNITS (each = a coupled cluster of categories).

### Pass E — EXTRACT (one chargen_compile-style focused pass per unit, parallel)
For each unit, a focused loop (mirror `compile_chargen_formulas`) produces, per category:
- a SCHEMA: the typed slots + their HOOKS as JSON. Formula slots use the §4 `expr`/`lookup_tables`
  format so the SAME `trpg_formula` evaluator runs them. Hook kinds: `damage`→dice_core eval;
  `attack_skill`→character skill field_id; `ammo`→resource_track id; `range`→check_model/range table.
- exactly **2 EXAMPLE INSTANCES** per category: 2 concrete entries (e.g. ".38 or 9mm Revolver",
  "Shotgun"), slots FILLED from the source (`read_layout` the aligned table), hooked to the schema,
  page-ref'd. Examples are the pattern the runtime materialization mimics.
GUARDRAIL (reuse the chargen idea): round-trip every formula/hook through the evaluator → unresolved →
`status=provisional`; NEVER fabricate a value or a table row.

### Where it lands
- `RuleKernel` gains `object_schemas: Vec<Value>` (each entry = a category schema + its 2 examples).
  Persisted with the kernel; abilities/spells reuse the same field (distinguished by `kind`).

### Materialization reworked (the fix for the .38 saga)
When the player references an item, materialization:
1. picks the matching category SCHEMA + its 2 examples (by kind/coupling);
2. the extractor's job SHRINKS to "fill the schema's typed slots for THIS item from the retrieved
   source row, using the 2 examples as the exact pattern" — not free interpretation. Output is a hooked
   instance the engine uses directly. The already-solid `search_key`→table-row retrieval feeds the row.

## Implementation order (incremental, each builds + verifies)
1. trpg-model: add `RuleKernel.object_schemas: Vec<Value>` (serde default).
2. trpg-rule-agent: new `reader/object_compile.rs` mirroring `chargen_compile.rs` — Discover+Couple
   (one planning call) then per-unit Extract loop (schema + 2 examples) with the round-trip guardrail.
3. trpg-parser: call it after `compile_chargen_formulas` (it already has units + sidecar + the template
   + subsystem_map); write `object_schemas` into the kernel.
4. trpg-material: switch the extractor from free-form to "fill schema slots against the 2 examples";
   when a category schema exists, use it; else fall back to today's path.
5. Verify: fresh char (Beatrice) + ".38 revolver" → hooked damage 1D10 / range 15 yards RELIABLY;
   then cross-ruleset (Cyberpunk weapons; Triangle = no spell category produced).

## Philosophy guardrails
- Data-driven: categories DISCOVERED (Triangle/CoC no-spells handled); zero hardcoded weapon/spell tables.
- Reuse: mirrors `chargen_compile` (loop/tools/round-trip guardrail); hooks into existing dice_core /
  check_model / resource_tracks / character skills — one formula engine for chargen + objects + abilities.
- Fail-closed: unresolved hooks → provisional; never fabricate. Same as everywhere else.

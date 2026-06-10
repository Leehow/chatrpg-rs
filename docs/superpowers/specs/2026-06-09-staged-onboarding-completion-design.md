# Staged onboarding completion — `onboarding_compile` Stage-2 pass

Status: design (2026-06-09), approved. Goal (functional bar): on a **staged-parsed** ruleset,
`create-character` produces a **stat-complete, playable PC** (not a draft). Fits the progressive-parse
design: Stage 1 stays fast; Stage 2 (background) completes the character onboarding so the fast UX win
is preserved and the character is fully buildable by the time the user finishes filling the sheet.

## Problem
The staged path builds the character onboarding pack with `stage1_onboarding_pack` (thin):
`creation_flows = Vec::new()` and `starter_character_pack = default` (all empty). The full parse-all
path builds these via `coerce_character_onboarding_pack` (needs `book`/`book_map`/`procedures`).
Consequence: `RuntimeEngine::generate_starter_character` degrades when
`starter_character_pack.{pregens,archetypes,creation_shortcuts}` are ALL empty (runtime lib.rs:108-110),
so `create-character` on a staged kernel yields a narrative DRAFT with stats unfilled. The playtest
(2026-06-09) confirmed this: "missing creation flow extraction and starter character path".

Two real gaps:
1. **creation_flows empty** — yet `template.creation_flow` (CreationStep[]) HAS the steps; the
   structured `CharacterCreationFlow` is derivable deterministically, no LLM, no book_map.
2. **starter_character_pack empty** — needs the ruleset's starter generation rules (which Role/class,
   point-buy vs roll, starting skills + gear). This is real source-backed extraction.

`book`/`book_map` are used by the parse-all path mostly for `source_refs` + fallback, NOT
fundamentally required: the staged path has `units` (semantic units) + the formula-compiled `template`
+ the reader's `option_catalogs`, which is enough to extract the starter rules directly.

## Design — Stage-2 focused `onboarding_compile` pass (mirrors chargen/object compile)

### A. Deterministic creation_flows (Stage 1, no LLM)
A pure helper `creation_flows_from_template(template) -> Vec<CharacterCreationFlow>` wraps
`template.creation_flow` (the ordered CreationSteps the reader already produced) into the structured
`CharacterCreationFlow` the pack expects. Call it inside `stage1_onboarding_pack` so even the fast
Stage-1 pack carries creation_flows (fixes gap 1 immediately, zero LLM cost, deterministic-testable).

### B. `reader/onboarding_compile.rs` (Stage 2, focused LLM pass)
New module mirroring `chargen_compile.rs` / `object_compile.rs` (loop / nav_tools / read_layout /
submit / round-trip guardrail). Signature:
`pub async fn compile_starter_pack(client: &dyn LlmClient, ctx: &OnboardingCtx, budget) -> StarterCharacterPack`
where `OnboardingCtx { units, sidecar_text, template, option_catalogs }`.
- Seed: the template's role/class field + skill fields + the option_catalogs (Roles/classes, skills,
  starter gear) + located pages. Instruct: read the character-creation chapter and produce the
  STARTER GENERATION recipes.
- Output (the gap-2 fill): `archetypes` (2-4 recommended starter builds: role/class + stat priorities
  + signature skills) and `creation_shortcuts` (quick-build recipes: pick role → stat method
  (point-buy budget N or roll XdY) → starting skill picks → starting gear). `pregens` optional.
- GUARDRAIL (fail-closed): every recipe grounds in a page read; `source_refs` recorded; if a value is
  unreadable, the recipe is dropped (NOT fabricated). If nothing extractable → return an empty pack
  (create-character degrades to draft, never invents a build). Round-trip: an archetype must reference
  real template fields / option-catalog ids (else dropped).
- File ≤ 400 lines; reuse object_compile's `object_tool_schemas`/loop pattern (or a local copy if
  cross-module reuse is awkward — keep it self-contained).

### C. Stage-2 wiring (staged.rs `stage2_deep`)
After the object-schema step, run `compile_starter_pack`, merge into the onboarding pack, and
**re-upsert the complete pack** to the DB (`db.upsert_character_onboarding_pack`). Independent,
non-fatal try (the established Stage-2 pattern): failure leaves the thin pack intact. Thread the
data it needs (units + sidecar already on `StagedParse`; template + option_catalogs from char_slice).
A new helper `persist_stage2_onboarding(db, ruleset_id, title, template, option_catalogs, starter)`
builds the complete `CharacterOnboardingPack` (= stage1 pack + creation_flows + starter pack) and
upserts it. Stage-2 kernel persist already carries option_catalogs (fixed earlier today).

## Data flow
- Stage 1: read_character_slice → template + option_catalogs → `stage1_onboarding_pack` (now WITH
  deterministic creation_flows) → upsert DB onboarding row + kernel. User starts filling.
- Stage 2: `compile_starter_pack(units, template, option_catalogs)` → StarterCharacterPack →
  `persist_stage2_onboarding` re-upserts the COMPLETE pack. `create-character` now generates a
  complete PC (generate_starter_character sees non-empty archetypes/shortcuts).

## Error handling
- Fail-closed throughout: no fabricated archetypes/shortcuts/stat values; unreadable → drop.
- `compile_starter_pack` error / empty → keep the Stage-1 thin pack; `create-character` degrades to a
  draft (today's behavior), never breaks.
- Stage-2 sub-step is independent + non-fatal (sibling of chargen/object compile).

## Testing
- Deterministic: `creation_flows_from_template` (template with 6 steps → 1 CharacterCreationFlow with
  6 steps); pack-merge helper (thin pack + starter pack → complete pack, fields preserved).
- onboarding_compile guardrail: an archetype referencing a non-existent field/option is dropped; an
  empty extraction yields an empty (not fabricated) starter pack.
- Live (the bar): fresh staged parse of a ruleset (CoC), then `create-character --auto` → assert the
  produced character is stat-complete (stats/skills filled, no "missing" / no "starter character path"
  validation error). Compare against the pre-fix draft.

## Component boundaries / files
- `crates/trpg-rule-agent/src/reader/onboarding_compile.rs` (NEW, <400) — `OnboardingCtx`,
  `compile_starter_pack`, guardrail. Export from `reader/mod.rs`.
- `crates/trpg-parser/src/lib.rs` — `creation_flows_from_template` helper; call it in
  `stage1_onboarding_pack`; add `persist_stage2_onboarding`.
- `crates/trpg-parser/src/staged.rs` — `stage2_deep` runs `compile_starter_pack` +
  `persist_stage2_onboarding` after the object step.
- NOT touched: runtime `generate_starter_character` (it already consumes the pack correctly), the
  module reader, trpg-api/trpg-cli (other workers).

## Philosophy guardrails
- Data-driven: starter recipes EXTRACTED from the book per ruleset; zero hardcoded roles/classes/builds.
- Fail-closed: never fabricate a starter build; degrade to draft, never break.
- Reuse: mirrors chargen_compile/object_compile (focused 2nd pass); creation_flows derived from the
  template the reader already produced.
- Progressive: Stage 1 stays fast; completion is Stage-2 background (consistent with the parse design).

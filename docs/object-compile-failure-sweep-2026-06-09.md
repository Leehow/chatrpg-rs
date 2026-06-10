# object_proto Compile-Failure Sweep — 6 Rulesets (2026-06-09)

Aggregated `object_proto` discover+extract diagnostics across 6 rulesets. Every ruleset ran
clean (exit 0, sidecar present, no errors). Each category was hand-verified against the source
markdown: example names checked on cited pages, null slots classified as real-absence vs miss.

**Headline:** 44 categories total, **38 good / 6 partial / 0 failed**. No phantom categories,
no prompt-parroted examples, no miscategorizations anywhere. The discover step is solid; the
residual defects are almost all **value-level extraction slips** (a real printed cell on the
*already-read* page got dropped), not structural discover failures.

---

## 1. Coverage Table

| Ruleset | Total | Good | Partial | Failed | Units |
|---|---:|---:|---:|---:|---:|
| CoC_7e | 7 | 7 | 0 | 0 | 1648 |
| DnD5e_cn | 8 | 8 | 0 | 0 | 2726 |
| BRP_ORC | 8 | 6 | 2 | 0 | 1476 |
| SwordWorld2.5_cn | 8 | 6 | 2 | 0 | 1641 |
| Triangle_Agency | 5 | 4 | 1 | 0 | 685 |
| Cyberpunk_RED | 8 | 7 | 1 | 0 | 1783 |
| **TOTAL** | **44** | **38** | **6** | **0** | **9959** |

Good rate 86%. All 6 partials are recoverable single-slot or two-slot misses — none is a wholesale
category failure.

---

## 2. Failure Taxonomy

The compiler's coarse `failure_mode` field only flags 3 categories (all
`multipage_table_offpage_examples`); the other 3 partials carry `failure_mode="ok"` yet are still
`verdict=partial` because a real cell was dropped. So `failure_mode` **undercounts the actionable
problem**. Below is the deeper per-cause taxonomy (a category can carry more than one cause).

### Ranked by frequency

| Cause | Cat-instances | Rulesets | Actionable? |
|---|---:|---:|---|
| **SCHEMA_OVERSPEC** (null slot that has no column/field in source — correct null) | 19 | 6 | No — discover over-specifies schema vs source |
| **CLEAN** (full / near-full real-row fill) | 17 | 5 | No |
| **PROSE_SPARSE** (free-form prose family, nulls are real absences) | 7 | 4 | No |
| **VALDROP** (real printed cell on the read page dropped) | 6 | 5 | **YES — the only true extraction defect** |
| **MULTIPAGE** (table spans pages, wrapped/off-row cell dropped) | 1 | 1 | YES — subset of VALDROP cause |
| **SCHEMA_REDUNDANT** (duplicate/orphan slot by schema modeling) | 1 | 1 | No — schema dedup, not extraction |
| **MULTIPAGE_OK** (multi-page span handled correctly) | 1 | 1 | No |

### 2a. VALDROP — the genuine misses (6 instances, 5 rulesets)

This is the one cause worth fixing. In every case the source line is on a page the extractor
**already read**, and all-but-one sibling fields were captured, so it is a value-grab slip, not a
page-targeting failure:

- `CoC_7e:mythos_tomes` (good) — *Cthaat Aquadingen* `spells=null` though p239 prose plainly lists
  "Suggested Spells: Dreams from God… Speak with Sea Children…". Table+prose were merged but the
  spell list was dropped (p249 Table XI has no spells column → prose half lost).
- `BRP_ORC:magic_spells` (partial) — *Wounding* (p70) source literally reads `Range: Touch` but
  extractor returned `range=""`; duration & PP cost on the same entry *were* captured.
- `SwordWorld2.5_cn:shields` (partial) — *Spiked Shield* (p289) missed `weapon_critical_value`
  (md prints crit ⑩=10) and main-table `defense` (md prints "2/0"); `weapon_accuracy`/`weapon_power`
  were caught.
- `SwordWorld2.5_cn:spells` (partial) — *Energy Bolt* (p219) block prints "Crit Value ⑩" yet
  `crit_value=null`; same column exists for Fireball.
- `Triangle_Agency:vault_requisitions` (partial) — *Gripples* (p154) dropped `effect`/`duration`
  text that is present on the same page ("sticky toe shoes… treat nearest flat surface as down");
  `commendation_cost=15` and `activation` were caught. *iPond on the same page extracted 6/6.*
- `Cyberpunk_RED:vehicles` (partial) — *Roadbike*/*Gyrocopter* `cost=null` though md has a Cost
  column ("20,000eb Super Luxury"). **This is the one true MULTIPAGE case**: the PDF-flattened md
  wrapped the Cost cell onto a line adjacent to the name row, so the off-row cell was dropped.

### 2b. SCHEMA_OVERSPEC — not a bug, a discover over-reach (19 instances, all 6 rulesets)

The single most common pattern, but every null is *correct* — the slot simply has no source column.
Concrete examples:
- `Cyberpunk_RED:armor` — `location=null` (page-98 armor table has no location column at all).
- `DnD5e_cn:mounts_and_vehicles` — `weight`/`speed`/`carrying` null because three different-column
  sub-tables were merged under one superset schema.
- `BRP_ORC:sorcery_spells` (partial) — all 4 nulls (`duration`/`cast_time`/`resistible`/explicit
  `power_point_cost`) are fields the prose spell list never prints (PP = spell level, a derivation).
- `SwordWorld2.5_cn:weapons` — `addl_dmg=null` because the table literally prints "-".

These drag the **fill ratio** down and make good categories *look* weak, but they are faithful
empties. Cosmetic, except they confound any "reject low-fill" guardrail (see §4c).

### 2c. PROSE_SPARSE (7), SCHEMA_REDUNDANT (1), MULTIPAGE_OK (1)

- **PROSE_SPARSE** — free-form prose families (CoC artifacts, DnD feats/spells, BRP spell lists,
  Triangle requisitions) are intrinsically sparse; a feat with no prerequisite, a debuff spell with
  no damage. Real absences.
- **SCHEMA_REDUNDANT** — `Triangle_Agency:fictional_ripple_gun_ultima_powers` `power_name=null`: the
  single label column was modeled as `attack_skill` *and* copied to top-level `name`, orphaning
  `power_name`. A duplicate-slot design quirk, fixable by schema dedup.
- **MULTIPAGE_OK** — `DnD5e_cn:armor_and_shields` spans p145–146 and was handled perfectly,
  proving multi-page spans are not inherently broken when discover hands back the right span.

---

## 3. Cross-Ruleset Patterns

**Systematic (multi-ruleset):**
- **SCHEMA_OVERSPEC** is universal (all 6 rulesets, 19 instances). Discover consistently writes a
  superset schema that exceeds what any single sub-table / prose block prints. This is the dominant
  driver of low fill ratios.
- **VALDROP** is broad (5 of 6 rulesets). It is *not* concentrated in one ruleset or one kind — it
  hits tomes, spells, shields, requisitions, and vehicles. The common thread is **dual-source merge
  or column-dense rows**: prose+table merges (CoC tomes, SW shields/spells) and wrapped rows
  (Cyberpunk vehicles) lose the last cell.
- **PROSE_SPARSE** spell/ability families recur in 4 rulesets — a structural mismatch between a
  table-shaped schema and a prose source, not an extractor bug.

**Ruleset-specific:**
- `SCHEMA_REDUNDANT` only in Triangle_Agency (one orphan slot).
- The single genuine `MULTIPAGE`/wrapped-row drop is Cyberpunk_RED:vehicles only.

**Is the multipage-table issue CoC-only or general?**
Neither — and notably **CoC has zero multipage failures**. CoC's column-dense tables (weapons p413,
vehicles p157) all sit on a single page and extracted cleanly. The historical "CoC multipage" worry
did **not** reproduce here. The *only* true off-page/wrapped-row drop in the whole sweep is
**Cyberpunk_RED:vehicles** (Land p191 / Air p192 split + PDF-flattened wrapped Cost cell). D&D
weapons/spells and Cyberpunk weapons/cyberware are **single-page** tables and fully filled — they
are *not* fragmented. So multipage is **rare (1/44) and Cyberpunk-specific**, not a general epidemic.
The DnD armor p145–146 span being handled correctly confirms multi-page is tractable when discover
returns the right span.

**Are phantom categories common? Which families over-trigger?**
**No phantom categories anywhere (0/44).** Every discovered category maps to a real table or prose
family with on-page example names. No family over-triggered. The discover step's *category*
selection is fully trustworthy in this sweep; its only weakness is *schema width* (over-spec), not
*category invention*.

---

## 4. Prioritized Fix List

Ranked by how many of the **6 observed partials / VALDROP misses** each candidate actually fixes.

### (b) Discover full page-span + extract reads a window — **LOW impact: fixes 1 of 6**
Only `Cyberpunk_RED:vehicles` is a true multipage/off-row drop. Every other VALDROP is on a page the
extractor already read. A page-window fix is real but narrow. Worth doing for the wrapped-row /
table-split case specifically (couple it with row-reassembly for PDF-flattened wrapped cells), but it
is **not** the high-leverage fix the historical "CoC multipage" framing implied.
→ **Impact: 1/6 partials.**

### (d) Drop phantom categories with 0 real entries — **ZERO impact here: fixes 0 of 6**
No phantom categories were observed in any ruleset. A guardrail is cheap insurance against
regressions but fixes nothing in this sweep.
→ **Impact: 0/6. Keep as cheap regression guard, deprioritize.**

### (a) Remove prompt example-name hints — **ZERO impact here: fixes 0 of 6**
No prompt-parroted examples were observed; every example name was verified genuine and on-page.
Removing hints is good hygiene against a failure mode that simply did not fire this run.
→ **Impact: 0/6. Defensive only.**

### (c) Reject low-fill examples guardrail — **NEGATIVE / DANGEROUS as specified: do NOT ship naively**
A raw fill-ratio threshold would **mis-fire catastrophically** here: 19 SCHEMA_OVERSPEC + 7
PROSE_SPARSE instances are low-fill **by correct design** (CoC artifacts 5/8, BRP sorcery 3/7, DnD
feats 3/6, Triangle QA 3/5 — all *good*). A naive guardrail would reject more correct categories
than it catches misses. It can only work if "fill" is computed over **applicable** slots
(source-column-present), not declared slots — which requires solving SCHEMA_OVERSPEC first.
→ **Impact: net-negative unless re-scoped to applicable-slot fill.**

### NEW fix the data suggests — **highest impact: addresses 5–6 of 6**

**(e) Dual-source / column-dense row re-grab pass ("last-cell" verifier).**
The real signal is VALDROP, and 5 of 6 instances share a mechanism: a **merged prose+table family or
a column-dense row** loses its last/trailing cell while siblings are captured. A targeted second pass
— for any extracted unit whose category is a known table+prose merge or whose row is column-dense,
re-scan the cited page span for declared-but-null slots whose value is *textually present* — would
recover CoC tomes `spells`, BRP `range`, SW `crit_value`/`defense`, Triangle `effect`, and (with
row-reassembly) Cyberpunk `cost`. This is the only fix that touches the majority of observed defects.

**(f) Schema-width reconciliation (de-superset / per-sub-table schema).**
Fixes the cosmetic-but-pervasive SCHEMA_OVERSPEC (19 instances) by either splitting merged
sub-tables into their own schemas or tagging each slot "applicable-on-source-column". This unblocks
fix (c) and makes fill ratio a trustworthy quality signal. High value as an *enabler*, not a direct
bug fix.

**Recommended order:** (e) dual-source last-cell verifier → (f) schema-width reconciliation →
(b) wrapped-row/window read for Cyberpunk-class splits → (c) applicable-slot guardrail (only after f)
→ (a)/(d) as cheap defensive guards.

---

## 5. Honest Caveats

- **Single run per ruleset.** Each verdict is one `object_proto` invocation (gpt-5.4 / gpt-5.4-mini).
  LLM extraction is non-deterministic; VALDROP slips in particular may vary run-to-run (a cell
  dropped here could be caught on a re-run, and vice-versa). Treat the 6 partials as a *sample*, not
  a fixed defect set. No multi-seed variance data was collected.
- **No ruleset errored.** All 6 ran exit 0 with sidecar present; there are no missing/failed
  rulesets to caveat. The 0-failed count is real, not a reporting gap.
- **Verdicts are human-adjudicated**, with null slots classified as real-absence vs miss by reading
  the source md. That classification is the analyst's judgment; a different reviewer might count a
  borderline SCHEMA_OVERSPEC null as a miss (or vice-versa), shifting the good/partial boundary by a
  category or two. The VALDROP set, however, is anchored to *literally-present* source text and is
  the most defensible part of this report.
- **Coverage is 2 examples per category** by design — a category marked "good" was validated on two
  rows, not exhaustively. A latent miss on an un-sampled row would not show here.
- **Per-example page numbers are printed-page labels**, offset from the md's physical `# Page N`
  anchors (explicitly noted for Cyberpunk, where category-level `source_pages` was empty "?"). Page
  citations in this report use the analyst's resolved md-line locations, not the raw printed labels.

---

### Artifacts
`/tmp/sweep_<ruleset>.json`, `/tmp/sweep_<ruleset>.log` per ruleset; DnD source md at
`/Users/haoli/leehow/code/chatrpgv2/_rstest_dnd/markdown/rulebooks/dnd_5e_cn.md`.

-- Fix #5: Triangle Agency chaos track thresholds lack numeric `at` values,
-- so watcher.rs detect_crossings never fires (branch at line 74 requires
-- th.get("at").and_then(|v| v.as_i64()) to be Some).
--
-- Root cause: pass-one reader extracted the Chaos Effects spend chart (rulebook
-- book p23 / PDF pp104-105) as GM-facing prose only, not as structured threshold
-- entries with `at` + `consequence`. The extracted thresholds array had a single
-- entry with ONLY `consequence` text and no `at` field, making every Chaos
-- accumulation produce zero ThresholdCrossings and therefore zero MechanicDues.
-- A live session observed that chaos=12 correctly narrated "Chaos overflows" when
-- a GM used them manually, proving the rulebook values exist; the engine simply
-- could not see them.
--
-- Fix: replace the single no-`at` stub entry with five structured cumulative
-- thresholds derived from the Chaos Effects chart (rulebook pp104-105):
--   4 Chaos  → Manifest  (create Minor Anomaly; mundane disruption)
--   5 Chaos  → Attract   (mundane beings drawn to Domain; Domain starts to grow)
--   6 Chaos  → Expand    (Domain grows a tier; 2 Minor Anomalies)
--  10 Chaos  → Kill      (end an Anomalous life regardless of defenses — "big deal")
--  30 Chaos  → Overwhelm (wipe out a team of 3 Agents instantly — ultimate threat)
-- Each entry follows the watcher ThresholdCrossing contract: `at` (cumulative edge),
-- `consequence` (GM-facing consequence prose). No `direction` key = default rising
-- (Chaos only accumulates upward via pool_miss_count; the pool resets to 0 at
-- mission end, handled externally not by thresholds).
--
-- Pure DATA fix (no per-ruleset Rust; detect_crossings reads the `at` field
-- generically). Idempotent: re-running writes the identical array.
-- Fail-closed: WHERE guard only fires when the current thresholds array matches
-- the known-bad or known-good shape (one stub entry with no `at`), skipping
-- unknown structures (0 rows) rather than clobbering them.
--
-- Apply:
--   docker exec -i chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg \
--     < _fix_triangle_chaos_thresholds.sql
update rule_kernels
set content_json = jsonb_set(
  content_json,
  '{resource_tracks,0,thresholds}',
  $thresholds$[
    {"at":  4, "consequence": "GM may use Chaos Effect: Manifest — create a Minor Anomaly or trigger a minor mundane disruption (rulebook p105)"},
    {"at":  5, "consequence": "GM may use Chaos Effect: Attract — mundane beings are drawn into the Anomaly's Domain, or its influence expands (rulebook p105)"},
    {"at":  6, "consequence": "GM may use Chaos Effect: Expand — the Anomaly's Domain grows by a tier; GM may create 2 Minor Anomalies (rulebook p105)"},
    {"at": 10, "consequence": "GM may use Chaos Effect: Kill — end any Anomalous life regardless of defenses; this effect cannot be stopped or interrupted (rulebook p105)"},
    {"at": 30, "consequence": "GM may use Chaos Effect: Overwhelm — instantly neutralize a team of 3 Agents; even the Agency cannot always protect them (rulebook p105)"}
  ]$thresholds$::jsonb,
  false
)
where ruleset_id = 'triangle_agency'
  and active = true
  and content_json #>> '{resource_tracks,0,id}' = 'chaos'
  and jsonb_array_length(content_json -> 'resource_tracks' -> 0 -> 'thresholds') = 1
  and content_json #>> '{resource_tracks,0,thresholds,0,at}' is null;

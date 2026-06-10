-- Fix: the CoC 7e kernel `success_bands` were under-extracted by the LLM kernel
-- reader. The stored set was [regular, hard, extreme, fumble] only, which:
--   * had NO `critical` band -> a roll of exactly 1 (CoC critical success) was
--     mis-tiered as `extreme` in real play and in the contest resolver;
--   * had NO `failure`/`otherwise` fallback -> any non-matching roll (e.g. 90 vs
--     skill 75) returned no success_tier at all (None);
--   * malformed the fumble guard (top-level `unless` instead of test-level
--     `when`), so the skill>=50 vs skill<50 fumble distinction was ignored.
--
-- This restores the canonical, complete CoC 7e band set — identical in id/rank/test
-- to the one already proven correct by the trpg-contest unit test `coc_tiers_skill75`
-- (fn `coc_bands()`) and documented in the reader prompt (reader/agent.rs).
-- Pure DATA fix (no per-ruleset Rust, evaluator stays generic, fail-closed).
-- Idempotent: re-running writes the identical array.
--
-- Apply:
--   docker exec -i chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg \
--     < _fix_coc_success_bands.sql
update rule_kernels
set content_json = jsonb_set(
  content_json,
  '{dice_core,success_bands}',
  $bands$[
    {"id":"critical","label":"Critical Success","rank":5,"test":{"kind":"exact","value":1}},
    {"id":"extreme","label":"Extreme Success","rank":4,"test":{"kind":"roll_under_fraction","numerator":1,"denominator":5}},
    {"id":"hard","label":"Hard Success","rank":3,"test":{"kind":"roll_under_fraction","numerator":1,"denominator":2}},
    {"id":"regular","label":"Success","rank":2,"test":{"kind":"roll_under_or_equal"}},
    {"id":"fumble","label":"Fumble","rank":0,"test":{"kind":"in_range","min":96,"max":100,"when":{"target_lt":50}}},
    {"id":"fumble","label":"Fumble","rank":0,"test":{"kind":"in_range","min":100,"max":100,"when":{"target_gte":50}}},
    {"id":"failure","label":"Failure","rank":1,"test":{"kind":"otherwise"}}
  ]$bands$::jsonb,
  true
)
where ruleset_id = 'call_of_cthulhu_7e';

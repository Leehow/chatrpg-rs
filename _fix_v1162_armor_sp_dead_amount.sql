-- Fix (companion to _fix_dead_on_outcome_refs.sql, same disease, DIFFERENT DB):
-- the chatrpg-postgres-v1162 version-test DB (:54346, the .env default) carried
-- its own dead on_outcome rule — cyberpunk_red `armor_sp`:
--   [{"op":"subtract","when":"success","amount":"=damage","trigger":"always"}]
-- `damage` is not in the engine outcome vocabulary (trpg-model
-- AMOUNT_RESOLVABLE) and the amount branch never falls back to `when`; no
-- default_amount -> guaranteed silent no-op, DROPPED (on_outcome -> []).
-- Note this differs from the rulesets DB (:54347), whose armor_sp ablation
-- rule has the LIVE plain-int amount "1" and was kept.
--
-- Fail-closed: WHERE pins track id at index + exact dead value + rule count;
-- idempotent (re-run matches 0 rows). v1162 is a re-parseable version-test DB.
--
-- Applied 2026-06-11. Apply:
--   docker exec -i chatrpg-postgres-v1162 psql -U chatrpg -d chatrpg \
--     < _fix_v1162_armor_sp_dead_amount.sql
update rule_kernels
set content_json = jsonb_set(content_json, '{resource_tracks,1,on_outcome}', '[]'::jsonb, false)
where ruleset_id = 'cyberpunk_red'
  and active
  and content_json #>> '{resource_tracks,1,id}' = 'armor_sp'
  and content_json #>> '{resource_tracks,1,on_outcome,0,amount}' = '=damage'
  and jsonb_array_length(content_json #> '{resource_tracks,1,on_outcome}') = 1;

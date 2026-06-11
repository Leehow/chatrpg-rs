-- Fix: three live kernels carry resource_tracks[].on_outcome[] rules whose
-- `=<field>` amount references a field that does NOT exist in the engine
-- check-outcome vocabulary (trpg-model outcome_fields::AMOUNT_RESOLVABLE =
-- total/target/success/success_count/pool_miss_count/success_tier_rank).
-- At runtime trpg-mechanics `resolve_track_amount` returns None for an unknown
-- field, and none of these rules has a `default_amount` rescue -> the rule is
-- a guaranteed silent no-op (crates/trpg-mechanics/src/lib.rs ~L130 `continue`
-- fires BEFORE any patch / fallback-fact / threshold-watcher work).
--
-- Dead rules removed (5 rules across 4 tracks; semantic judgment per case):
--
--   1. cyberpunk_red  hit_points  x2 rules  amount "=damage_after_armor"
--      `damage_after_armor` is a derived_formulas field_id ("max(0,
--      weapon_damage - SP)") — NOTHING ever writes it into a check-outcome
--      JSON. CPR damage comes from a SEPARATE damage roll, not the attack
--      check outcome, so no legal outcome field can express this rule:
--      damage must flow through the combat/effect path (apply_effect_roll /
--      hydrate_combat_check_contract), as it already does. DELETE, not rename.
--      Bonus fix: the trigger:"on_failure" rule's check_match "damage|attack"
--      currently feeds derive_tested_source (trpg-contest ~L576-584; only
--      trigger:"always" aliases are skipped) as tested-parameter aliases for
--      the hit_points track — the same "attack tests resolve against HP"
--      mis-binding class that bit CoC. Dropping the rule removes that hazard.
--
--   2. sword_world_2_5  hp + mp  amount "=damage" (when:"success")
--      Same disease: SW damage is a separate 2d6 power-table roll, never a
--      check-outcome field. The amount branch of resolve_track_amount does
--      NOT fall back to the `when` field, so the rule never resolves. DELETE.
--
--   3. call_of_cthulhu_7e  hp  amount "=<damage>"
--      The tool-doc `=<field>` placeholder copied verbatim (angle brackets
--      and all). CoC weapon damage likewise flows through the combat/effect
--      path. DELETE. (The sanity track's "=<sanity_loss>" rule is NOT touched:
--      it has default_amount "1d6" and the tested path works today.)
--
-- This converges the live rows to EXACTLY what the new finalize guard
-- (trpg-rule-agent reader/mechanics_outcome_refs.rs `apply_on_outcome_ref_guard`)
-- produces on any re-parse: never-resolvable rules are DROPPED (on_outcome
-- left as an empty array), the CoC sanity rule is KEPT via its default_amount.
-- Empty [] and missing on_outcome are runtime-equivalent (mechanics ~L90-93
-- skips both; derive_tested_source iterates zero rules). Zero behavior change
-- for the deleted rules — they were already silent no-ops.
--
-- hit_points/hp track IDENTIFICATION is unaffected: trpg_model
-- `hp_resource_track_id` reads kind/id only, never on_outcome. The live CPR
-- armor_sp ablation rule (amount "1", a plain int) is live and untouched.
--
-- Provenance / divergence note: the A7 batch artifacts
-- /Users/haoli/leehow/code/chatrpgv2/_a7_mech_audit/{cyberpunk_red,
-- sword_world_2_5}.final.json carry the same dead rules and are synced
-- alongside this archive (see _a7_mech_audit/_DIVERGENCE_dead_on_outcome_refs.md)
-- so an A7 write-back replay does not silently revert this fix.
-- call_of_cthulhu_7e has no final.json; its upgraded.*.json history still
-- carries "=<damage>" (noted there, history left unedited). Any future
-- rulebook re-extraction now passes through the finalize guard, which
-- re-drops these deterministically.
--
-- Pure DATA fix (no per-ruleset Rust; resolver stays generic, fail-closed).
-- Fail-closed: every WHERE guard pins the track id at its index AND the exact
-- known-dead amount value AND the exact rule count — if a re-extraction
-- reshaped the kernel (or a live rule was added alongside), the statement is
-- a no-op (0 rows) instead of clobbering an unknown structure. Idempotent:
-- a second run matches 0 rows.
--
-- Apply:
--   docker exec -i chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg \
--     < _fix_dead_on_outcome_refs.sql

-- 1) cyberpunk_red hit_points (track 0): drop both dead rules
update rule_kernels
set content_json = jsonb_set(content_json, '{resource_tracks,0,on_outcome}', '[]'::jsonb, false)
where ruleset_id = 'cyberpunk_red'
  and active
  and content_json #>> '{resource_tracks,0,id}' = 'hit_points'
  and content_json #>> '{resource_tracks,0,on_outcome,0,amount}' = '=damage_after_armor'
  and content_json #>> '{resource_tracks,0,on_outcome,1,amount}' = '=damage_after_armor'
  and jsonb_array_length(content_json #> '{resource_tracks,0,on_outcome}') = 2;

-- 2a) sword_world_2_5 hp (track 0): drop the dead rule
update rule_kernels
set content_json = jsonb_set(content_json, '{resource_tracks,0,on_outcome}', '[]'::jsonb, false)
where ruleset_id = 'sword_world_2_5'
  and active
  and content_json #>> '{resource_tracks,0,id}' = 'hp'
  and content_json #>> '{resource_tracks,0,on_outcome,0,amount}' = '=damage'
  and jsonb_array_length(content_json #> '{resource_tracks,0,on_outcome}') = 1;

-- 2b) sword_world_2_5 mp (track 1): drop the dead rule
update rule_kernels
set content_json = jsonb_set(content_json, '{resource_tracks,1,on_outcome}', '[]'::jsonb, false)
where ruleset_id = 'sword_world_2_5'
  and active
  and content_json #>> '{resource_tracks,1,id}' = 'mp'
  and content_json #>> '{resource_tracks,1,on_outcome,0,amount}' = '=damage'
  and jsonb_array_length(content_json #> '{resource_tracks,1,on_outcome}') = 1;

-- 3) call_of_cthulhu_7e hp (track 1; sanity is track 0 and is NOT touched):
--    drop the dead placeholder rule
update rule_kernels
set content_json = jsonb_set(content_json, '{resource_tracks,1,on_outcome}', '[]'::jsonb, false)
where ruleset_id = 'call_of_cthulhu_7e'
  and active
  and content_json #>> '{resource_tracks,1,id}' = 'hp'
  and content_json #>> '{resource_tracks,1,on_outcome,0,amount}' = '=<damage>'
  and jsonb_array_length(content_json #> '{resource_tracks,1,on_outcome}') = 1;

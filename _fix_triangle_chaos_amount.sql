-- Fix: the triangle_agency kernel chaos_pool track referenced a NON-EXISTENT
-- contest outcome field in its on_outcome amount mini-language:
--
--   BEFORE (extracted, bad):  {resource_tracks,0,on_outcome,0,amount} = "=chaos_generated"
--   AFTER  (fixed, canonical): {resource_tracks,0,on_outcome,0,amount} = "=pool_miss_count"
--
-- Why "=chaos_generated" is dead on arrival:
--   * `=<field>` amounts are resolved by trpg-mechanics `resolve_track_amount`
--     (crates/trpg-mechanics/src/lib.rs, `field_amount` reads `outcome.get(field)`);
--     a field absent from the resolved-check outcome JSON yields None, and since
--     chaos_pool is never the tested parameter (track_is_tested=false, no
--     default_amount), the track delta silently becomes "no-op" — the Chaos pool
--     NEVER accrues, killing acceptance #10 layer 1 (结算落账) with zero errors.
--   * The dice-pool resolver emits GENERIC outcome fields only —
--     `success_count` / `pool_miss_count` / `dice`
--     (crates/trpg-contest/src/lib.rs ~L49-56: "pool_miss_count = the rest (a
--     kernel resource_track, e.g. Triangle's Chaos, maps this to a track delta
--     via config — not hardcoded here)"). `chaos_generated` exists NOWHERE in
--     the engine outcome vocabulary; `pool_miss_count` is the documented,
--     purpose-built field for exactly this Triangle rule (each die in the 6d4
--     pool not showing the target face generates 1 Chaos).
--   * The kernel reader prompt's own canonical Triangle example
--     (crates/trpg-rule-agent/src/reader/agent.rs) binds Chaos accrual to
--     `pool_miss_count` — the extracted "=chaos_generated" was an invented name.
--
-- Provenance / divergence note:
--   The A7 batch artifact /Users/haoli/leehow/code/chatrpgv2/_a7_mech_audit/
--   triangle_agency.final.json originally carried the bad "=chaos_generated";
--   it has been synced to "=pool_miss_count" alongside this archive (see
--   _a7_mech_audit/_DIVERGENCE_triangle_chaos_amount.md) so that any A7
--   write-back replay does not silently revert this fix. Any future rulebook
--   re-extraction CAN still reproduce the bad value — C7 cache-regression /
--   reconciliation must re-check this path after any re-extraction and re-apply
--   this file if reverted (see C7 范围与前置 note in the plan).
--
-- Upstream guardrail gap (NOT fixed here, tracked separately):
--   mechanics_finalize (A3-class deterministic guardrails) does not validate
--   `=field` amount references in resource_tracks[].on_outcome[] against the
--   engine outcome vocabulary, so an invented field name passes extraction
--   silently. Tracked as a spawned background task (chip): "Validate =field
--   on_outcome amounts against outcome vocabulary".
--
-- Pure DATA fix (no per-ruleset Rust; resolver stays generic, fail-closed).
-- Idempotent: re-running writes the identical value. Fail-closed: the WHERE
-- guard only touches the row when the path holds the known-bad or known-good
-- value — if a re-extraction reshapes the track layout, this is a no-op
-- (0 rows) instead of clobbering an unknown structure.
--
-- Apply:
--   docker exec -i chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg \
--     < _fix_triangle_chaos_amount.sql
update rule_kernels
set content_json = jsonb_set(
  content_json,
  '{resource_tracks,0,on_outcome,0,amount}',
  '"=pool_miss_count"'::jsonb,
  false
)
where ruleset_id = 'triangle_agency'
  and active = true
  and content_json #>> '{resource_tracks,0,on_outcome,0,amount}'
      in ('=chaos_generated', '=pool_miss_count');

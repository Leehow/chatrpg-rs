-- 0033: KnowledgeEdge v1 fields. Additive-only (non-destructive) on the 0032
-- ledger: separate fact identity/truth from who knows/suspects/believes it.
-- Adds confidence / learned_at_turn_id / disclosure_policy columns and widens
-- the knowledge_state CHECK to the v1 superset (legacy 'exposed' retained).
-- Holder CHECK is widened only to include durable 'system'; npc/pc/faction
-- durable holders stay gated (TC-KNOW-04) and are NOT added to the DB CHECK.
-- updated_at already exists from 0032, so it is not re-added here.
-- All statements are idempotent (add column if not exists; drop+re-add CHECK).

alter table knowledge_edges
  add column if not exists confidence double precision;

alter table knowledge_edges
  add column if not exists learned_at_turn_id text;

alter table knowledge_edges
  add column if not exists disclosure_policy text;

-- Widen knowledge_state to the v1 superset. Drop-then-add keeps re-runs clean.
alter table knowledge_edges
  drop constraint if exists knowledge_edges_state_ck;
alter table knowledge_edges
  add constraint knowledge_edges_state_ck check (
    knowledge_state in (
      'unknown', 'exposed', 'perceived', 'heard_about', 'suspects',
      'believes_true', 'believes_false', 'knows_true', 'misinformed',
      'withheld', 'forgotten'
    )
  );

-- Widen holder_kind to durable {gm, player_party, system, npc}. This migration
-- is replayed on every Db::migrate() call, so it must remain compatible with
-- the later 0034 durable-NPC opening; otherwise any existing npc edge makes the
-- intermediate CHECK fail before 0034 can re-apply it. PC/faction durable
-- holders remain gated and must not be added here.
alter table knowledge_edges
  drop constraint if exists knowledge_edges_holder_kind_ck;
alter table knowledge_edges
  add constraint knowledge_edges_holder_kind_ck check (
    holder_kind in ('gm', 'player_party', 'system', 'npc')
  );

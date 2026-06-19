-- 0037: Durable PC + Faction KnowledgeEdges. Completes the holder-kind design now
-- that the TC-KNOW-00 actor-identity contract exists: widen the holder_kind CHECK to
-- admit 'pc' and 'faction' alongside the already-durable gm/player_party/system/npc, so
-- a resolved, stable PC / faction id can hold durable knowledge edges.
--
-- Identity gate stays in Rust: a pc/faction holder row MUST carry a stable holder_id
-- validated via KnowledgeHolder::player_character_from_actor_id / faction_from_id before
-- any durable write (display-name / ad-hoc / placeholder ids fail closed). Passing this
-- CHECK is necessary but not sufficient — kind admitted ≠ holder_id legal, exactly as for
-- 'npc' (0034). Collective gm/player_party/system holders remain id-less (empty holder_id).
--
-- Idempotent: drop-then-add the named CHECK so re-running the full migration set is a
-- no-op. This migration replays on every Db::migrate(); it must stay a strict superset of
-- 0034's holder set so any existing npc edge keeps validating before/after. No data
-- migration, no column change — only the knowledge_edges holder_kind constraint.

alter table knowledge_edges
  drop constraint if exists knowledge_edges_holder_kind_ck;
alter table knowledge_edges
  add constraint knowledge_edges_holder_kind_ck check (
    holder_kind in ('gm', 'player_party', 'system', 'npc', 'pc', 'faction')
  );

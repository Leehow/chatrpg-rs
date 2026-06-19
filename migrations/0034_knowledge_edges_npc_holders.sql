-- 0034: Durable NPC KnowledgeEdges (TC-KNOW-04). Additive-only on the 0033
-- ledger: widen the holder_kind CHECK to admit 'npc' so a resolved, stable NPC
-- actor id can hold durable knowledge edges. NpcLearnedFact is no longer
-- event-only once the holder resolves (db record_npc_learned_fact write-through).
--
-- Scope guard: ONLY 'npc' is opened here. 'pc' and 'faction' durable holders
-- remain gated (model is_durable_persistable + db fail-closed) and must NOT be
-- added to this CHECK. NPC holder identity itself is validated in Rust via the
-- TC-KNOW-00 actor-identity contract before any durable write — kind passing the
-- CHECK is necessary but not sufficient (ad-hoc/display-name ids fail closed).
--
-- Drop-then-add keeps re-runs clean and idempotent. All statements run inside the
-- single migration transaction (advisory-locked in Db::migrate); no CONCURRENTLY.

alter table knowledge_edges
  drop constraint if exists knowledge_edges_holder_kind_ck;
alter table knowledge_edges
  add constraint knowledge_edges_holder_kind_ck check (
    holder_kind in ('gm', 'player_party', 'system', 'npc')
  );

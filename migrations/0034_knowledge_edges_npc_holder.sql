-- 0034: Open NPC-as-holder durable knowledge_edges (P1 slice-1, after the holder
-- identity gate's id prerequisite closed). 0032 forbade `npc` holders by construction
-- (`holder_kind in ('gm','player_party')`) because the runtime collapsed every NPC into
-- the non-persistent `npc.opposition` placeholder. The contest/persona path now carries
-- the real module-graph NPC id (cf02ca4) and combat keeps an explicit placeholder→graph
-- binding (c68d654), so a durable npc edge can be anchored to a stable id.
--
-- This widens the holder-kind CHECK to include `npc` and adds a backstop guard: an `npc`
-- holder row MUST carry a non-empty stable holder_id and MUST NOT be the `npc.opposition`
-- placeholder. The identity gate is enforced in Rust (KnowledgeHolder::npc_from_actor_id);
-- this CHECK is defense-in-depth so a malformed direct write also fails closed.
--
-- Idempotent: each constraint is dropped-if-exists then re-added, so re-running the full
-- migration set is a no-op. No existing rows are `npc` (the old CHECK forbade them), so
-- adding the new constraints validates cleanly against current data. This touches only
-- knowledge_edges constraints — no data migration, no column change.

alter table knowledge_edges drop constraint if exists knowledge_edges_holder_kind_ck;
alter table knowledge_edges add constraint knowledge_edges_holder_kind_ck
  check (holder_kind in ('gm', 'player_party', 'npc'));

alter table knowledge_edges drop constraint if exists knowledge_edges_npc_holder_id_ck;
alter table knowledge_edges add constraint knowledge_edges_npc_holder_id_ck
  check (
    holder_kind <> 'npc'
    or (holder_id <> '' and holder_id <> 'npc.opposition')
  );

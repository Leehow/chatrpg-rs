-- 0033: NPC Relationship v1 (TC-NPC-02). Additive-only: a new durable table for an
-- NPC's bounded, evidence-grounded attitude toward the player party / a PC / another
-- NPC / a faction. Independent of knowledge_edges and memory_facts — this is structured
-- relationship state, not a free-form memory summary.
--
-- Migration number note: the original port authored this as 0035 (atop in-flight
-- knowledge_edges migrations 0033/0034 that the integration base did NOT take — those
-- widened knowledge_edges for the gated NPC-as-holder slice). The current integration
-- base ends at 0032_knowledge_edges, so this table is renumbered to the next coherent
-- number 0033. It touches only the new npc_relationships table — no knowledge_edges
-- schema change, preserving the P0b KnowledgeEdge migration state.
--
-- Identity: the relationship holder is the stable NPC actor id (npc_id), validated in
-- Rust via the TC-KNOW-00 actor-identity contract before any write (display-name /
-- ad-hoc / placeholder ids fail closed). target_kind is constrained to the v1 set;
-- pc/npc/faction target_ids are likewise Rust-validated. The CHECK here is a backstop,
-- not the identity gate.
--
-- Bounds: numeric channels are clamped to band in Rust on every applied delta. CHECK
-- constraints below mirror those bands so a malformed direct write also fails closed.
-- Evidence: evidence_event_ids is jsonb (array of event ids); a mutation always carries
-- at least one (enforced in Rust by NpcRelationshipDelta). create table is idempotent;
-- all statements run inside the single advisory-locked migration transaction.

create table if not exists npc_relationships (
  id uuid primary key default gen_random_uuid(),
  session_id text not null,
  npc_id text not null,
  target_kind text not null,
  target_id text not null default '',
  trust smallint not null default 0,
  respect smallint not null default 0,
  affection smallint not null default 0,
  debt smallint not null default 0,
  fear smallint not null default 0,
  suspicion smallint not null default 0,
  hostility smallint not null default 0,
  leverage smallint not null default 0,
  talkativeness smallint not null default 50,
  interaction_desire smallint not null default 0,
  stance text not null default 'neutral',
  last_interaction_turn_id text,
  evidence_event_ids jsonb not null default '[]'::jsonb,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  constraint npc_relationships_target_kind_ck check (
    target_kind in ('player_party', 'pc', 'npc', 'faction')
  ),
  constraint npc_relationships_signed_bounds_ck check (
    trust between -100 and 100
    and respect between -100 and 100
    and affection between -100 and 100
    and debt between -100 and 100
    and interaction_desire between -100 and 100
  ),
  constraint npc_relationships_unipolar_bounds_ck check (
    fear between 0 and 100
    and suspicion between 0 and 100
    and hostility between 0 and 100
    and leverage between 0 and 100
    and talkativeness between 0 and 100
  ),
  constraint npc_relationships_unique_holder_target unique (session_id, npc_id, target_kind, target_id)
);

create index if not exists idx_npc_relationships_session_npc
  on npc_relationships (session_id, npc_id);

-- 0032: KnowledgeEdge ledger (P0b). P0b starts with gm/player_party holders only.
-- revealed-facts becomes a compatibility projection of (player_party, knows_true)
-- rows here. NPC-as-holder is gated to P1 after actor-id unification.
create table if not exists knowledge_edges (
  edge_id text primary key,
  session_id text not null,
  holder_kind text not null,
  holder_id text not null default '',
  fact_id text not null,
  knowledge_state text not null,
  source_event_id text,
  reason text,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  constraint knowledge_edges_holder_kind_ck check (holder_kind in ('gm', 'player_party')),
  constraint knowledge_edges_state_ck check (knowledge_state in ('unknown', 'exposed', 'knows_true', 'believes_false')),
  constraint knowledge_edges_unique_fact_holder unique (session_id, holder_kind, holder_id, fact_id)
);

create index if not exists idx_knowledge_edges_session_holder
  on knowledge_edges (session_id, holder_kind, holder_id, knowledge_state);

create index if not exists idx_knowledge_edges_fact
  on knowledge_edges (fact_id);

-- Backfill existing FactRevealed events as player_party knows_true. edge_id is the
-- same md5 scheme used by the runtime upsert, so re-running is idempotent and the
-- unique (session, holder_kind, holder_id, fact_id) key dedups historical replays.
insert into knowledge_edges (
  edge_id, session_id, holder_kind, holder_id, fact_id,
  knowledge_state, source_event_id, reason, created_at, updated_at
)
select distinct on (session_id, data->>'fact_id')
  'ke_' || md5(session_id || ':player_party:' || coalesce(data->>'fact_id', '')),
  session_id,
  'player_party',
  '',
  data->>'fact_id',
  'knows_true',
  event_id,
  data->>'reason',
  created_at,
  created_at
from domain_events
where kind = 'FactRevealed'
  and data->>'fact_id' is not null
order by session_id, data->>'fact_id', seq
on conflict (session_id, holder_kind, holder_id, fact_id) do nothing;

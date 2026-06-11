-- migrations/0027_mechanic_dues_v120.sql
create table if not exists mechanic_dues (
  due_id text primary key,
  session_id text not null,
  turn_id text not null,
  source text not null,                       -- 'threshold' | 'hook'
  source_track text, hook_event text, mechanic_id text,
  threshold_desc text not null default '',
  followup_procedure_id text,
  owner_kind text not null default 'actor', owner_id text not null default '',
  evidence jsonb not null default '{}'::jsonb,
  status text not null default 'open',        -- 'open' | 'resolved' | 'waived'
  waive_reason text, waive_scope text,        -- waive 记账（勘误记忆另记一份 tags=["gm_waive"]）
  created_at timestamptz not null default now(), updated_at timestamptz not null default now()
);
create index if not exists idx_mechanic_dues_session_status on mechanic_dues (session_id, status);

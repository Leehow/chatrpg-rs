-- v1.8 Runtime Parameter Hydration & Material Binding
-- Character/NPC/object parameters are created lazily but persistently once an actor/object appears.

create table if not exists runtime_actor_parameters (
  id uuid primary key,
  actor_param_id text not null unique,
  session_id text not null,
  actor_id text not null,
  actor_kind text not null,
  ruleset_id text not null,
  source_kind text not null default 'runtime_hydrator',
  template_id text,
  display_name text,
  sheet_json jsonb not null default '{}'::jsonb,
  mechanical_profile jsonb not null default '{}'::jsonb,
  status_json jsonb not null default '{}'::jsonb,
  visibility text not null default 'gm_only',
  created_at_tick bigint,
  updated_at_tick bigint,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  unique(session_id, actor_id)
);

create index if not exists runtime_actor_parameters_session_actor_idx
  on runtime_actor_parameters(session_id, actor_id);

create table if not exists material_hydration_events (
  id uuid primary key,
  event_id text not null unique,
  session_id text not null,
  turn_id text,
  frame_id text,
  actor_id text,
  object_id text,
  ruleset_id text,
  material_kind text not null,
  hydration_status text not null,
  query_text text,
  result_json jsonb not null default '{}'::jsonb,
  world_tick bigint,
  created_at timestamptz not null default now()
);

create index if not exists material_hydration_events_session_idx
  on material_hydration_events(session_id, created_at desc);

alter table object_instances
  add column if not exists hydration_status text not null default 'seeded',
  add column if not exists hydration_source text,
  add column if not exists source_query text;

alter table object_definitions
  add column if not exists hydration_status text not null default 'seeded',
  add column if not exists source_query text;

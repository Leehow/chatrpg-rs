-- v1.6 Object & Possession Kernel

create table if not exists object_definitions (
  id uuid primary key,
  object_def_id text not null unique,
  ruleset_id text not null,
  name text not null,
  object_kind text not null,
  tags text[] not null default '{}',
  mechanical_profile jsonb not null default '{}'::jsonb,
  rule_bindings jsonb not null default '[]'::jsonb,
  default_affordances text[] not null default '{}',
  equip_slots text[] not null default '{}',
  visibility_default text not null default 'gm_only',
  source_refs jsonb not null default '[]'::jsonb,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create index if not exists object_definitions_ruleset_kind_idx
  on object_definitions(ruleset_id, object_kind);

create table if not exists object_instances (
  id uuid primary key,
  object_id text not null unique,
  object_def_id text,
  session_id text not null,
  scope_type text not null,
  scope_id text not null,
  display_name text not null,
  object_kind text not null,
  location_json jsonb not null,
  visibility_state jsonb not null,
  mechanical_state jsonb not null default '{}'::jsonb,
  quantity int,
  durability_json jsonb,
  tags text[] not null default '{}',
  active boolean not null default true,
  created_at_tick bigint,
  updated_at_tick bigint,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create index if not exists object_instances_session_kind_idx
  on object_instances(session_id, object_kind);
create index if not exists object_instances_session_active_idx
  on object_instances(session_id, active);
create index if not exists object_instances_scope_idx
  on object_instances(scope_type, scope_id);

create table if not exists object_edges (
  id uuid primary key,
  edge_id text not null unique,
  session_id text not null,
  from_object_id text not null,
  to_object_id text not null,
  relation text not null,
  metadata jsonb not null default '{}'::jsonb,
  visibility text not null default 'gm_only',
  valid_from_tick bigint,
  valid_until_tick bigint,
  created_at timestamptz not null default now()
);

create index if not exists object_edges_session_from_idx on object_edges(session_id, from_object_id);
create index if not exists object_edges_session_to_idx on object_edges(session_id, to_object_id);
create index if not exists object_edges_relation_idx on object_edges(relation);

create table if not exists object_interaction_contracts (
  id uuid primary key,
  interaction_id text not null unique,
  session_id text not null,
  turn_id text not null,
  frame_id text,
  interaction_context_id text,
  actor_id text not null,
  target_actor_id text,
  target_object_id text,
  interaction_kind text not null,
  contract_json jsonb not null,
  status text not null,
  world_tick bigint,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create index if not exists object_interaction_contracts_session_turn_idx
  on object_interaction_contracts(session_id, turn_id);
create index if not exists object_interaction_contracts_check_idx
  on object_interaction_contracts((contract_json #>> '{check_contract,check_id}'));
create index if not exists object_interaction_contracts_frame_idx
  on object_interaction_contracts(frame_id, status);

create table if not exists object_patches (
  id uuid primary key,
  patch_id text not null unique,
  session_id text not null,
  interaction_id text,
  object_id text,
  patch_kind text not null,
  patch_json jsonb not null,
  validator_status text not null,
  world_tick bigint,
  created_at timestamptz not null default now()
);

create index if not exists object_patches_session_interaction_idx
  on object_patches(session_id, interaction_id);

create table if not exists object_events (
  id uuid primary key,
  object_event_id text not null unique,
  session_id text not null,
  object_id text,
  frame_id text,
  world_event_id text,
  event_kind text not null,
  event_json jsonb not null,
  visibility text not null,
  world_tick bigint,
  created_at timestamptz not null default now()
);

create index if not exists object_events_session_tick_idx
  on object_events(session_id, world_tick, created_at);
create index if not exists object_events_object_idx
  on object_events(object_id);

create table if not exists object_affordance_snapshots (
  id uuid primary key,
  snapshot_id text not null unique,
  session_id text not null,
  frame_id text,
  actor_id text,
  affordances_json jsonb not null,
  world_tick bigint,
  created_at timestamptz not null default now()
);

alter table interaction_contexts
  add column if not exists object_interaction_ids text[] not null default '{}';

alter table state_frames
  add column if not exists active_object_interaction_ids text[] not null default '{}';

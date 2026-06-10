-- v1.12 Mechanics Search Skills & Parameter Facet Bindings
-- Search skills sit between generic GrepSearch/Tantivy candidate recall and the existing
-- actor/object/ability/check/effect parameter systems. They do not create a parallel mechanics engine.

create table if not exists mechanics_query_plans (
  id uuid primary key,
  plan_id text not null unique,
  demand_id text not null,
  session_id text not null,
  search_skill text not null,
  target_kind text not null,
  target_label text not null,
  requested_fields jsonb not null default '[]'::jsonb,
  query_plan_json jsonb not null default '{}'::jsonb,
  world_tick bigint,
  created_at timestamptz not null default now()
);
create index if not exists mechanics_query_plans_session_created_idx on mechanics_query_plans(session_id, created_at desc);
create index if not exists mechanics_query_plans_demand_idx on mechanics_query_plans(demand_id);
create index if not exists mechanics_query_plans_skill_idx on mechanics_query_plans(search_skill);

create table if not exists parameter_facet_bindings (
  id uuid primary key,
  facet_binding_id text not null unique,
  session_id text not null,
  target_kind text not null,
  target_id text not null,
  facet_kind text not null,
  binding_id text not null,
  demand_id text,
  facet_json jsonb not null default '{}'::jsonb,
  source_refs jsonb not null default '[]'::jsonb,
  confidence text not null default 'medium',
  verification_status text not null default 'needs_review',
  world_tick bigint,
  created_at timestamptz not null default now()
);
create index if not exists parameter_facet_bindings_session_created_idx on parameter_facet_bindings(session_id, created_at desc);
create index if not exists parameter_facet_bindings_target_idx on parameter_facet_bindings(target_kind, target_id);
create index if not exists parameter_facet_bindings_facet_idx on parameter_facet_bindings(facet_kind);

create table if not exists search_skill_profiles (
  id uuid primary key,
  skill_id text not null unique,
  search_skill text not null,
  profile_json jsonb not null default '{}'::jsonb,
  enabled boolean not null default true,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

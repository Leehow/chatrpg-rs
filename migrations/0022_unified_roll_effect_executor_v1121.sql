-- v1.12.1 Unified Roll & Effect Executor
-- Generalizes damage into parameter impacts, and records roll plans for all mechanical rolls.

create table if not exists roll_plans (
  id uuid primary key,
  roll_plan_id text not null unique,
  session_id text not null,
  turn_id text not null,
  frame_id text,
  roll_kind text not null,
  actor_id text,
  target_refs jsonb not null default '[]'::jsonb,
  dice_expression text not null,
  parameters_used jsonb not null default '{}'::jsonb,
  target_model jsonb not null default '{}'::jsonb,
  authority text not null,
  visibility text not null,
  display_policy jsonb not null default '{}'::jsonb,
  expected_effects jsonb not null default '[]'::jsonb,
  source_refs jsonb not null default '[]'::jsonb,
  rule_binding_ids jsonb not null default '[]'::jsonb,
  parameter_facet_ids jsonb not null default '[]'::jsonb,
  world_tick bigint,
  created_at timestamptz not null default now()
);
create index if not exists roll_plans_session_created_idx on roll_plans(session_id, created_at desc);
create index if not exists roll_plans_turn_idx on roll_plans(session_id, turn_id);

create table if not exists effect_resolution_packets (
  id uuid primary key,
  effect_resolution_id text not null unique,
  session_id text not null,
  turn_id text not null,
  frame_id text,
  source_actor_id text,
  source_object_id text,
  source_ability_id text,
  source_event_id text,
  target_refs jsonb not null default '[]'::jsonb,
  roll_records jsonb not null default '[]'::jsonb,
  rule_binding_ids jsonb not null default '[]'::jsonb,
  parameter_facet_ids jsonb not null default '[]'::jsonb,
  impacts_json jsonb not null default '[]'::jsonb,
  visibility text not null default 'gm_only',
  source_refs jsonb not null default '[]'::jsonb,
  provisional_reason text,
  packet_json jsonb not null default '{}'::jsonb,
  world_tick bigint,
  created_at timestamptz not null default now()
);
create index if not exists effect_resolution_packets_session_created_idx on effect_resolution_packets(session_id, created_at desc);
create index if not exists effect_resolution_packets_turn_idx on effect_resolution_packets(session_id, turn_id);

create table if not exists parameter_impacts (
  id uuid primary key,
  impact_id text not null unique,
  effect_resolution_id text not null,
  session_id text not null,
  turn_id text not null,
  target_kind text not null,
  target_id text not null,
  parameter_path text not null,
  operation text not null,
  value_json jsonb not null,
  before_json jsonb,
  after_json jsonb,
  validator_status text not null,
  visibility text not null default 'gm_only',
  source_refs jsonb not null default '[]'::jsonb,
  provisional_reason text,
  created_at timestamptz not null default now()
);
create index if not exists parameter_impacts_session_created_idx on parameter_impacts(session_id, created_at desc);
create index if not exists parameter_impacts_target_idx on parameter_impacts(target_kind, target_id, parameter_path);

alter table damage_packets add column if not exists effect_resolution_id text;
alter table dice_rolls add column if not exists roll_plan_id text;

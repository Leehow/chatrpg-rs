-- v1.9 Semantic Rule Binding & Ability Hydration Kernel

create table if not exists semantic_classification_events (
  id uuid primary key,
  semantic_id text not null unique,
  session_id text not null,
  turn_id text not null,
  ruleset_id text not null,
  classifier text not null,
  confidence text not null,
  result_json jsonb not null,
  created_at timestamptz not null default now()
);
create index if not exists semantic_classification_events_session_turn_idx on semantic_classification_events(session_id, turn_id);

create table if not exists rule_binding_packets (
  id uuid primary key,
  binding_id text not null unique,
  session_id text,
  turn_id text,
  target_kind text not null,
  target_id text not null,
  ruleset_id text not null,
  semantic_request_json jsonb not null default '{}'::jsonb,
  retrieval_queries jsonb not null default '[]'::jsonb,
  source_hits jsonb not null default '[]'::jsonb,
  extracted_json jsonb not null default '{}'::jsonb,
  source_refs jsonb not null default '[]'::jsonb,
  confidence text not null default 'low',
  verification_status text not null default 'needs_review',
  world_tick bigint,
  created_at timestamptz not null default now()
);
create index if not exists rule_binding_packets_target_idx on rule_binding_packets(target_kind, target_id);
create index if not exists rule_binding_packets_session_turn_idx on rule_binding_packets(session_id, turn_id);

create table if not exists ability_definitions (
  id uuid primary key,
  ability_def_id text not null unique,
  ruleset_id text not null,
  name text not null,
  ability_kind text not null,
  source_kind text not null,
  tags text[] not null default '{}',
  definition_json jsonb not null,
  binding_status text not null default 'unbound',
  visibility text not null default 'gm_only',
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
create index if not exists ability_definitions_ruleset_kind_idx on ability_definitions(ruleset_id, ability_kind);

create table if not exists ability_instances (
  id uuid primary key,
  ability_id text not null unique,
  ability_def_id text not null,
  session_id text not null,
  owner_actor_id text,
  granted_by_object_id text,
  granted_by_status_id text,
  known_state text not null default 'known_by_actor',
  prepared_state text not null default 'always_available',
  instance_json jsonb not null,
  visibility text not null default 'gm_only',
  created_at_tick bigint,
  updated_at_tick bigint,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  unique(session_id, ability_def_id, owner_actor_id)
);
create index if not exists ability_instances_session_actor_idx on ability_instances(session_id, owner_actor_id);

create table if not exists ability_trigger_bindings (
  id uuid primary key,
  trigger_id text not null unique,
  ability_def_id text not null,
  trigger_kind text not null,
  binding_json jsonb not null,
  created_at timestamptz not null default now()
);
create index if not exists ability_trigger_bindings_def_idx on ability_trigger_bindings(ability_def_id);

create table if not exists ability_activation_contracts (
  id uuid primary key,
  activation_id text not null unique,
  session_id text not null,
  turn_id text not null,
  frame_id text,
  actor_id text not null,
  ability_id text not null,
  ability_def_id text not null,
  activation_kind text not null,
  status text not null,
  contract_json jsonb not null,
  world_tick bigint,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
create index if not exists ability_activation_contracts_session_turn_idx on ability_activation_contracts(session_id, turn_id);

alter table runtime_actor_parameters add column if not exists ability_summary_json jsonb not null default '{}'::jsonb;
alter table object_definitions add column if not exists rule_binding_ids text[] not null default '{}';
alter table object_definitions add column if not exists granted_ability_def_ids text[] not null default '{}';
alter table check_contracts add column if not exists rule_binding_ids text[] not null default '{}';
alter table effect_contracts add column if not exists rule_binding_ids text[] not null default '{}';

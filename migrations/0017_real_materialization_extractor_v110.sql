-- v1.10 Real Materialization Extractor & Binding Verifier

create table if not exists materialization_demands (
  id uuid primary key,
  demand_id text not null unique,
  session_id text not null,
  turn_id text,
  frame_id text,
  target_kind text not null,
  target_id text,
  target_label text not null,
  target_description text,
  ruleset_id text not null,
  module_id text,
  requested_fields jsonb not null default '[]'::jsonb,
  urgency text not null,
  evidence_json jsonb not null default '{}'::jsonb,
  visibility text not null default 'gm_only',
  status text not null default 'open',
  world_tick bigint,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
create index if not exists materialization_demands_session_status_idx on materialization_demands(session_id, status, created_at desc);
create index if not exists materialization_demands_target_idx on materialization_demands(target_kind, target_id);

create table if not exists source_evidence_bundles (
  id uuid primary key,
  bundle_id text not null unique,
  demand_id text not null,
  query_plan_json jsonb not null default '{}'::jsonb,
  source_priority_order jsonb not null default '[]'::jsonb,
  bundle_json jsonb not null default '{}'::jsonb,
  world_tick bigint,
  created_at timestamptz not null default now()
);
create index if not exists source_evidence_bundles_demand_idx on source_evidence_bundles(demand_id);

create table if not exists source_candidates (
  id uuid primary key,
  candidate_id text not null unique,
  bundle_id text not null,
  demand_id text not null,
  source_document_id text not null,
  source_kind text not null default 'unknown',
  page_start int,
  page_end int,
  heading_path text,
  excerpt text not null,
  excerpt_hash text not null,
  candidate_score real not null default 0,
  retrieval_reason text not null,
  source_refs jsonb not null default '[]'::jsonb,
  metadata jsonb not null default '{}'::jsonb,
  created_at timestamptz not null default now()
);
create index if not exists source_candidates_bundle_idx on source_candidates(bundle_id);
create index if not exists source_candidates_demand_idx on source_candidates(demand_id);

create table if not exists extraction_runs (
  id uuid primary key,
  extraction_run_id text not null unique,
  demand_id text not null,
  extractor_kind text not null,
  input_candidates jsonb not null default '[]'::jsonb,
  extracted_json jsonb not null default '{}'::jsonb,
  source_refs jsonb not null default '[]'::jsonb,
  confidence text not null default 'low',
  status text not null default 'needs_review',
  model_id text,
  created_at timestamptz not null default now()
);
create index if not exists extraction_runs_demand_idx on extraction_runs(demand_id);

create table if not exists binding_verifications (
  id uuid primary key,
  verification_id text not null unique,
  binding_id text not null,
  target_kind text not null,
  target_id text not null,
  status text not null,
  missing_required_fields jsonb not null default '[]'::jsonb,
  contradictory_sources jsonb not null default '[]'::jsonb,
  visibility_issues jsonb not null default '[]'::jsonb,
  verifier_json jsonb not null default '{}'::jsonb,
  created_at timestamptz not null default now()
);
create index if not exists binding_verifications_binding_idx on binding_verifications(binding_id);

create table if not exists actor_runtime_bindings (
  id uuid primary key,
  runtime_binding_id text not null unique,
  binding_id text not null,
  session_id text not null,
  actor_id text not null,
  actor_param_id text,
  profile_json jsonb not null default '{}'::jsonb,
  verification_status text not null default 'provisional_needs_audit',
  world_tick bigint,
  created_at timestamptz not null default now()
);
create index if not exists actor_runtime_bindings_session_actor_idx on actor_runtime_bindings(session_id, actor_id);

create table if not exists object_runtime_bindings (
  id uuid primary key,
  runtime_binding_id text not null unique,
  binding_id text not null,
  session_id text not null,
  object_id text,
  object_def_id text,
  profile_json jsonb not null default '{}'::jsonb,
  verification_status text not null default 'provisional_needs_audit',
  world_tick bigint,
  created_at timestamptz not null default now()
);
create index if not exists object_runtime_bindings_session_object_idx on object_runtime_bindings(session_id, object_id, object_def_id);

create table if not exists ability_runtime_bindings (
  id uuid primary key,
  runtime_binding_id text not null unique,
  binding_id text not null,
  session_id text not null,
  ability_id text,
  ability_def_id text,
  profile_json jsonb not null default '{}'::jsonb,
  verification_status text not null default 'provisional_needs_audit',
  world_tick bigint,
  created_at timestamptz not null default now()
);
create index if not exists ability_runtime_bindings_session_ability_idx on ability_runtime_bindings(session_id, ability_id, ability_def_id);

alter table rule_binding_packets add column if not exists demand_id text;
alter table rule_binding_packets add column if not exists extraction_run_id text;
alter table rule_binding_packets add column if not exists verification_id text;

alter table check_contracts add column if not exists materialization_demand_ids jsonb not null default '[]'::jsonb;
alter table effect_contracts add column if not exists materialization_demand_ids jsonb not null default '[]'::jsonb;

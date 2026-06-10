create table if not exists learning_audit_runs (
  id uuid primary key,
  audit_run_id text not null unique,
  session_id text,
  turn_id text,
  ruleset_id text,
  module_id text,
  status text not null default 'queued',
  input_json jsonb not null default '{}'::jsonb,
  result_json jsonb not null default '{}'::jsonb,
  error text,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create index if not exists idx_learning_audit_runs_scope
  on learning_audit_runs(session_id, ruleset_id, module_id, created_at desc);

create table if not exists learning_candidates (
  id uuid primary key,
  candidate_id text not null unique,
  session_id text,
  turn_id text,
  ruleset_id text not null,
  module_id text,
  demand_id text,
  packet_type text not null,
  packet_key text not null,
  title text not null,
  summary text not null,
  packet_json jsonb not null default '{}'::jsonb,
  source_refs jsonb not null default '[]'::jsonb,
  evidence_score real not null default 0.0,
  risk_flags text[] not null default '{}',
  verifier_status text not null default 'pending_review',
  verifier_notes text,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create index if not exists idx_learning_candidates_review
  on learning_candidates(ruleset_id, verifier_status, evidence_score desc, updated_at desc);

create index if not exists idx_learning_candidates_session
  on learning_candidates(session_id, turn_id, updated_at desc);

insert into search_source_configs (id, source_config_id, source_kind, label, enabled, priority, config_json)
values (
  gen_random_uuid(),
  'db.learning_candidates',
  'sql_query',
  'Learning candidates pending review',
  true,
  72,
  $config${
    "sql":"select 'learning_candidate:' || candidate_id as search_doc_id, 'learning_candidate' as origin, 'learned' as domain, packet_type as logical_kind, title, summary || ' ' || packet_json::text as body, array['learning_candidate', packet_type, packet_key, verifier_status]::text[] as tags, 'gm_only' as visibility, 'turn_dynamic' as stability, jsonb_build_object('session_id', coalesce(session_id,''), 'ruleset_id', ruleset_id, 'module_id', coalesce(module_id,''), 'candidate_id', candidate_id, 'verifier_status', verifier_status) as scope_json, source_refs, jsonb_build_object('candidate_id', candidate_id, 'evidence_score', evidence_score, 'risk_flags', risk_flags, 'verifier_status', verifier_status) as metadata, updated_at from learning_candidates"
  }$config$::jsonb
)
on conflict (source_config_id) do update set
  enabled = excluded.enabled,
  priority = excluded.priority,
  config_json = excluded.config_json;

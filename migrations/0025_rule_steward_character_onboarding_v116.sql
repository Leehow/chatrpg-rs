-- v1.16 Rule Steward Agent + Character Steward onboarding artifacts.
-- These tables intentionally store source-backed packs and audit trails as JSONB so the agent can evolve without schema churn.

create table if not exists rule_agent_runs (
  id uuid primary key default gen_random_uuid(),
  run_id text not null unique,
  session_id text,
  turn_id text,
  ruleset_id text not null,
  module_id text,
  trigger text not null,
  selected_skill text not null,
  tool_calls jsonb not null default '[]'::jsonb,
  source_refs_read jsonb not null default '[]'::jsonb,
  outputs_written jsonb not null default '[]'::jsonb,
  confidence real not null default 0,
  unresolved_count integer not null default 0,
  contradiction_count integer not null default 0,
  created_at timestamptz not null default now()
);
create index if not exists idx_rule_agent_runs_context on rule_agent_runs(ruleset_id, module_id, session_id, created_at desc);

create table if not exists rule_kernels (
  id uuid primary key default gen_random_uuid(),
  kernel_id text not null unique,
  ruleset_id text not null,
  version text not null,
  content_json jsonb not null,
  source_refs jsonb not null default '[]'::jsonb,
  validation_report jsonb not null default '{}'::jsonb,
  content_hash text not null,
  active boolean not null default true,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
create index if not exists idx_rule_kernels_ruleset on rule_kernels(ruleset_id, active, updated_at desc);

create table if not exists rule_kernel_patches (
  id uuid primary key default gen_random_uuid(),
  patch_id text not null unique,
  ruleset_id text not null,
  target_kernel_version text not null,
  patch_kind text not null,
  old_hash text not null default '',
  proposed_hash text not null default '',
  diff_summary text not null default '',
  json_patch jsonb not null default '{}'::jsonb,
  source_refs jsonb not null default '[]'::jsonb,
  confidence real not null default 0,
  contradiction_report jsonb,
  regression_tests jsonb not null default '[]'::jsonb,
  status text not null default 'proposed',
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
create index if not exists idx_rule_kernel_patches_ruleset on rule_kernel_patches(ruleset_id, status, updated_at desc);

create table if not exists character_onboarding_packs (
  id uuid primary key default gen_random_uuid(),
  pack_id text not null unique,
  ruleset_id text not null,
  title text not null,
  content_json jsonb not null,
  sheet_template_json jsonb not null default '{}'::jsonb,
  creation_flows_json jsonb not null default '[]'::jsonb,
  option_catalogs_json jsonb not null default '[]'::jsonb,
  derived_formula_pack_json jsonb not null default '{}'::jsonb,
  starter_character_pack_json jsonb not null default '{}'::jsonb,
  runtime_bindings_json jsonb not null default '[]'::jsonb,
  source_refs jsonb not null default '[]'::jsonb,
  validation_report jsonb not null default '{}'::jsonb,
  content_hash text not null,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
create index if not exists idx_character_onboarding_packs_ruleset on character_onboarding_packs(ruleset_id, updated_at desc);

create table if not exists character_creation_flows (
  id uuid primary key default gen_random_uuid(),
  flow_id text not null unique,
  ruleset_id text not null,
  pack_id text,
  mode text not null default 'guided',
  title text not null,
  content_json jsonb not null,
  source_refs jsonb not null default '[]'::jsonb,
  updated_at timestamptz not null default now()
);
create index if not exists idx_character_creation_flows_ruleset on character_creation_flows(ruleset_id, mode, updated_at desc);

create table if not exists character_option_catalogs (
  id uuid primary key default gen_random_uuid(),
  catalog_id text not null unique,
  ruleset_id text not null,
  pack_id text,
  title text not null,
  content_json jsonb not null,
  source_refs jsonb not null default '[]'::jsonb,
  updated_at timestamptz not null default now()
);
create index if not exists idx_character_option_catalogs_ruleset on character_option_catalogs(ruleset_id, updated_at desc);

create table if not exists starter_character_packs (
  id uuid primary key default gen_random_uuid(),
  pack_id text not null unique,
  ruleset_id text not null,
  module_id text,
  title text not null default 'Starter Character Pack',
  content_json jsonb not null,
  source_refs jsonb not null default '[]'::jsonb,
  updated_at timestamptz not null default now()
);
create index if not exists idx_starter_character_packs_context on starter_character_packs(ruleset_id, module_id, updated_at desc);

create table if not exists rule_entity_locators (
  id uuid primary key default gen_random_uuid(),
  locator_id text not null unique,
  ruleset_id text not null,
  entity_id text not null,
  canonical_name text not null,
  aliases text[] not null default '{}',
  category text not null,
  source_refs jsonb not null default '[]'::jsonb,
  heading_path text[] not null default '{}',
  confidence real not null default 0,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
create index if not exists idx_rule_entity_locators_lookup on rule_entity_locators(ruleset_id, category, canonical_name);

create table if not exists mechanical_source_packs (
  id uuid primary key default gen_random_uuid(),
  pack_id text not null unique,
  ruleset_id text not null,
  module_id text,
  mechanic_kind text not null,
  content_json jsonb not null,
  source_refs jsonb not null default '[]'::jsonb,
  missing_units jsonb not null default '[]'::jsonb,
  confidence real not null default 0,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
create index if not exists idx_mechanical_source_packs_context on mechanical_source_packs(ruleset_id, module_id, mechanic_kind, updated_at desc);

create table if not exists playability_gate_reports (
  id uuid primary key default gen_random_uuid(),
  report_id text not null unique,
  ruleset_id text not null,
  module_id text,
  ready boolean not null default false,
  content_json jsonb not null,
  blocking_gaps jsonb not null default '[]'::jsonb,
  warnings jsonb not null default '[]'::jsonb,
  created_at timestamptz not null default now()
);
create index if not exists idx_playability_gate_reports_context on playability_gate_reports(ruleset_id, module_id, created_at desc);

insert into search_source_configs (id, source_config_id, source_kind, label, enabled, priority, config_json)
values
(gen_random_uuid(),'db.character_onboarding_packs','sql_query','Character onboarding packs',true,95,$config${"sql":"select 'character_onboarding_pack:' || pack_id as search_doc_id, 'character_onboarding_pack' as origin, 'rules' as domain, 'character_onboarding_pack' as logical_kind, title, content_json::text as body, array['character_onboarding','character_creation','playability',ruleset_id]::text[] as tags, 'gm_only' as visibility, 'rarely_changed' as stability, jsonb_build_object('ruleset_id', ruleset_id, 'pack_id', pack_id) as scope_json, source_refs, jsonb_build_object('pack_id', pack_id, 'content_hash', content_hash) as metadata, updated_at from character_onboarding_packs"}$config$::jsonb),
(gen_random_uuid(),'db.rule_kernels','sql_query','Rule Steward kernels',true,90,$config${"sql":"select 'rule_kernel:' || kernel_id as search_doc_id, 'rule_kernel' as origin, 'rules' as domain, 'rule_kernel' as logical_kind, ruleset_id || ' Rule Kernel' as title, content_json::text as body, array['rule_kernel','bp1','core_rules',ruleset_id]::text[] as tags, 'gm_only' as visibility, 'rarely_changed' as stability, jsonb_build_object('ruleset_id', ruleset_id, 'kernel_id', kernel_id, 'version', version) as scope_json, source_refs, jsonb_build_object('kernel_id', kernel_id, 'content_hash', content_hash, 'active', active) as metadata, updated_at from rule_kernels where active = true"}$config$::jsonb),
(gen_random_uuid(),'db.rule_entity_locators','sql_query','Rule entity locators',true,75,$config${"sql":"select 'rule_entity_locator:' || locator_id as search_doc_id, 'rule_entity_locator' as origin, 'rules' as domain, category as logical_kind, canonical_name as title, canonical_name || ' ' || array_to_string(aliases,' ') || ' ' || array_to_string(heading_path,' ') as body, array['rule_entity_locator',category,ruleset_id]::text[] as tags, 'gm_only' as visibility, 'rarely_changed' as stability, jsonb_build_object('ruleset_id', ruleset_id, 'entity_id', entity_id, 'category', category) as scope_json, source_refs, jsonb_build_object('locator_id', locator_id, 'confidence', confidence) as metadata, updated_at from rule_entity_locators"}$config$::jsonb)
on conflict (source_config_id) do update set
  label = excluded.label,
  enabled = excluded.enabled,
  priority = excluded.priority,
  config_json = excluded.config_json,
  updated_at = now();

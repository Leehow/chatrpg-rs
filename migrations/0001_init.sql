create extension if not exists pgcrypto;

create table if not exists source_documents (
  id uuid primary key,
  source_id text not null unique,
  source_kind text not null check (source_kind in ('rulebook','module','unknown')),
  title text not null,
  file_path text not null,
  markdown_path text,
  source_hash text not null,
  parse_config_hash text not null,
  parse_status text not null default 'pending',
  parsed_bundle_id text,
  metadata jsonb not null default '{}'::jsonb,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create index if not exists idx_source_documents_hash
  on source_documents(source_kind, source_hash, parse_config_hash);

create table if not exists parsed_bundles (
  id uuid primary key,
  bundle_id text not null unique,
  bundle_kind text not null check (bundle_kind in ('ruleset','module','project')),
  title text not null,
  schema_version text not null,
  artifact_path text,
  source_hash text not null,
  parse_config_hash text not null,
  content_json jsonb not null,
  validation_report jsonb not null default '{}'::jsonb,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create index if not exists idx_parsed_bundles_kind
  on parsed_bundles(bundle_kind, bundle_id);

alter table parsed_bundles add column if not exists artifact_path text;


create table if not exists context_blocks (
  id uuid primary key,
  bundle_id text not null,
  block_id text not null,
  block_kind text not null,
  title text not null default '',
  scope_type text not null,
  scope_id text not null,
  visibility text not null,
  stability text not null,
  cache_zone text not null,
  priority integer not null default 50,
  version integer not null default 1,
  content_json jsonb,
  content_text text,
  content_hash text not null,
  token_estimate integer,
  source_refs jsonb not null default '[]'::jsonb,
  dependencies jsonb not null default '[]'::jsonb,
  tags text[] not null default '{}',
  expires_at_turn text,
  expires_at_scene text,
  load_reason text,
  active boolean not null default true,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  unique(bundle_id, block_id, version)
);

create index if not exists idx_context_blocks_scope
  on context_blocks(bundle_id, scope_type, scope_id, active);

create index if not exists idx_context_blocks_cache_zone
  on context_blocks(bundle_id, cache_zone, active);

create index if not exists idx_context_blocks_tags
  on context_blocks using gin(tags);

create table if not exists material_index (
  id uuid primary key,
  bundle_id text not null,
  material_id text not null,
  material_type text not null,
  title text not null,
  summary text not null default '',
  load_when jsonb not null default '[]'::jsonb,
  source_refs jsonb not null default '[]'::jsonb,
  dependencies jsonb not null default '[]'::jsonb,
  estimated_tokens integer,
  default_cache_zone text not null default 'pinned_middle',
  visibility text not null default 'gm_only',
  stability text not null default 'rarely_changed',
  extracted_block_id text,
  extraction_status text not null default 'indexed',
  extraction_error text,
  tags text[] not null default '{}',
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  unique(bundle_id, material_id)
);

create index if not exists idx_material_index_bundle_type
  on material_index(bundle_id, material_type);

create index if not exists idx_material_index_tags
  on material_index using gin(tags);

create table if not exists character_templates (
  id uuid primary key,
  bundle_id text not null,
  template_id text not null unique,
  ruleset_id text not null,
  title text not null,
  content_json jsonb not null,
  source_refs jsonb not null default '[]'::jsonb,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create table if not exists characters (
  id uuid primary key,
  character_id text not null unique,
  ruleset_id text not null,
  template_id text not null,
  name text not null,
  sheet_json jsonb not null,
  validation_report jsonb not null default '{}'::jsonb,
  postprocess_status text not null default 'ready',
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create table if not exists sessions (
  id uuid primary key,
  session_id text not null unique,
  ruleset_id text not null,
  module_id text,
  active_scene_id text,
  active_location_id text,
  active_npc_ids text[] not null default '{}',
  active_material_refs text[] not null default '{}',
  pending_material_refs text[] not null default '{}',
  world_state jsonb not null default '{}'::jsonb,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create table if not exists turns (
  id uuid primary key,
  session_id text not null references sessions(session_id) on delete cascade,
  turn_id text not null,
  user_input text not null,
  assistant_output text,
  context_hashes jsonb not null default '{}'::jsonb,
  dice_results jsonb not null default '[]'::jsonb,
  state_patches jsonb not null default '[]'::jsonb,
  postprocess_status text not null default 'pending',
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  unique(session_id, turn_id)
);

create table if not exists context_block_load_events (
  id uuid primary key,
  session_id text,
  turn_id text,
  block_id text not null,
  block_version integer,
  cache_zone text,
  load_reason text,
  token_estimate integer,
  created_at timestamptz not null default now()
);

create table if not exists background_jobs (
  id uuid primary key,
  job_id text not null unique,
  job_kind text not null,
  status text not null default 'queued',
  input_json jsonb not null default '{}'::jsonb,
  result_json jsonb not null default '{}'::jsonb,
  error text,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create table if not exists memory_events (
  id uuid primary key,
  event_id text not null unique,
  session_id text not null references sessions(session_id) on delete cascade,
  turn_id text,
  ruleset_id text not null,
  module_id text,
  scene_id text,
  location_id text,
  actor_ids text[] not null default '{}',
  visibility text not null default 'gm_only',
  event_kind text not null default 'event',
  summary text not null,
  transcript_excerpt text,
  source_json jsonb not null default '{}'::jsonb,
  tags text[] not null default '{}',
  importance integer not null default 50,
  occurred_at timestamptz not null default now()
);

create index if not exists idx_memory_events_session_time
  on memory_events(session_id, occurred_at desc);

create index if not exists idx_memory_events_tags
  on memory_events using gin(tags);

create table if not exists memory_facts (
  id uuid primary key,
  fact_id text not null unique,
  session_id text not null references sessions(session_id) on delete cascade,
  scope_type text not null default 'session',
  scope_id text not null,
  visibility text not null default 'gm_only',
  subject text not null,
  predicate text not null,
  object_json jsonb not null default '{}'::jsonb,
  summary text not null,
  status text not null default 'active',
  confidence real not null default 1.0,
  source_event_ids text[] not null default '{}',
  tags text[] not null default '{}',
  importance integer not null default 50,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create index if not exists idx_memory_facts_session_status
  on memory_facts(session_id, status, importance desc, updated_at desc);

create index if not exists idx_memory_facts_scope
  on memory_facts(session_id, scope_type, scope_id, status);

create index if not exists idx_memory_facts_tags
  on memory_facts using gin(tags);

create table if not exists memory_snapshots (
  id uuid primary key,
  snapshot_id text not null,
  session_id text not null references sessions(session_id) on delete cascade,
  ruleset_id text not null,
  module_id text,
  scope_type text not null default 'session',
  scope_id text not null,
  visibility text not null default 'gm_only',
  title text not null,
  summary_markdown text not null,
  included_event_ids text[] not null default '{}',
  included_fact_ids text[] not null default '{}',
  version integer not null default 1,
  token_estimate integer,
  content_hash text not null,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  unique(snapshot_id, version)
);

create index if not exists idx_memory_snapshots_session
  on memory_snapshots(session_id, updated_at desc);

create table if not exists ruleset_onboarding_bundles (
  id uuid primary key,
  onboarding_id text not null unique,
  ruleset_id text not null,
  title text not null,
  bundle_json jsonb not null,
  source_refs jsonb not null default '[]'::jsonb,
  content_hash text not null,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create index if not exists idx_ruleset_onboarding_ruleset
  on ruleset_onboarding_bundles(ruleset_id, updated_at desc);

create table if not exists book_locator_entries (
  id uuid primary key,
  locator_id text not null unique,
  owner_id text not null,
  owner_kind text not null,
  label text not null,
  category text not null,
  source_document_id text not null,
  page_start integer,
  page_end integer,
  heading_path text[] not null default '{}',
  search_terms jsonb not null default '[]'::jsonb,
  summary text not null default '',
  confidence real not null default 0.7,
  parse_policy text not null default 'on_demand',
  tags text[] not null default '{}',
  source_refs jsonb not null default '[]'::jsonb,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create index if not exists idx_book_locator_owner
  on book_locator_entries(owner_id, owner_kind, category);

create index if not exists idx_book_locator_tags
  on book_locator_entries using gin(tags);

create table if not exists module_prep_packets (
  id uuid primary key,
  prep_id text not null unique,
  module_id text not null,
  ruleset_id text,
  mode text not null,
  title text not null,
  packet_json jsonb not null,
  source_refs jsonb not null default '[]'::jsonb,
  content_hash text not null,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create index if not exists idx_module_prep_module_mode
  on module_prep_packets(module_id, mode, updated_at desc);

create table if not exists lookup_events (
  id uuid primary key,
  event_id text not null unique,
  session_id text,
  ruleset_id text,
  module_id text,
  demand_id text,
  query_text text not null,
  search_terms jsonb not null default '[]'::jsonb,
  source_hits jsonb not null default '[]'::jsonb,
  result_status text not null default 'queued',
  created_at timestamptz not null default now()
);

create index if not exists idx_lookup_events_session_time
  on lookup_events(session_id, created_at desc);

create table if not exists rulings_log (
  id uuid primary key,
  ruling_id text not null unique,
  session_id text,
  ruleset_id text,
  module_id text,
  demand_id text,
  ruling_text text not null,
  source_refs jsonb not null default '[]'::jsonb,
  status text not null default 'provisional',
  confidence text not null default 'low',
  provisional boolean not null default true,
  superseded_by text,
  created_at timestamptz not null default now()
);

create index if not exists idx_rulings_session_time
  on rulings_log(session_id, created_at desc);

create table if not exists learned_packets (
  id uuid primary key,
  packet_id text not null unique,
  ruleset_id text not null,
  module_id text,
  packet_type text not null,
  packet_key text not null,
  title text not null,
  summary text not null,
  packet_json jsonb not null,
  source_refs jsonb not null default '[]'::jsonb,
  use_count integer not null default 0,
  learning_stage text not null default 'located',
  confidence text not null default 'medium',
  cache_zone text not null default 'dynamic_tail',
  visibility text not null default 'gm_only',
  last_used_at timestamptz,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  unique(ruleset_id, module_id, packet_type, packet_key)
);

create index if not exists idx_learned_packets_context
  on learned_packets(ruleset_id, module_id, learning_stage, use_count desc, updated_at desc);

create table if not exists search_source_configs (
  id uuid primary key,
  source_config_id text not null unique,
  source_kind text not null,
  label text not null,
  enabled boolean not null default true,
  priority integer not null default 50,
  config_json jsonb not null default '{}'::jsonb,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create index if not exists idx_search_source_configs_enabled
  on search_source_configs(enabled, priority desc, source_config_id);

create table if not exists search_index_watermarks (
  id uuid primary key,
  source_config_id text not null unique,
  last_indexed_at timestamptz not null default now(),
  last_doc_updated_at timestamptz not null default to_timestamp(0),
  indexed_document_count bigint not null default 0,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create index if not exists idx_search_index_watermarks_source
  on search_index_watermarks(source_config_id);

create table if not exists search_index_runs (
  id uuid primary key,
  run_id text not null unique,
  index_schema_version text not null,
  stats_json jsonb not null default '{}'::jsonb,
  created_at timestamptz not null default now()
);

insert into search_source_configs (id, source_config_id, source_kind, label, enabled, priority, config_json)
values
(gen_random_uuid(),'db.context_blocks','sql_query','Context blocks from PostgreSQL',true,100,$config${"sql":"select 'context_block:' || bundle_id || ':' || block_id || ':' || version::text as search_doc_id, 'context_block' as origin, case when block_kind like 'memory_%' or block_kind in ('session_summary','retrieved_memory','recent_transcript') then 'memory' when scope_type in ('module','scene','location','npc','mission','chapter') or block_kind like 'module_%' then 'modules' else 'rules' end as domain, block_kind as logical_kind, title, coalesce(content_text, content_json::text, '') as body, tags, visibility, stability, jsonb_build_object('bundle_id', bundle_id, 'block_id', block_id, 'scope_type', scope_type, 'scope_id', scope_id, 'cache_zone', cache_zone) as scope_json, source_refs, jsonb_build_object('bundle_id', bundle_id, 'block_id', block_id, 'version', version, 'cache_zone', cache_zone, 'priority', priority) as metadata, updated_at from context_blocks where active = true"}$config$::jsonb),
(gen_random_uuid(),'db.material_index','sql_query','Material index from PostgreSQL',true,90,$config${"sql":"select 'material_index:' || bundle_id || ':' || material_id as search_doc_id, 'material_index' as origin, case when material_type in ('npc','location','scene','mission','chapter','clue','handout','encounter','module_specific_rule') then 'modules' else 'rules' end as domain, material_type as logical_kind, title, summary as body, tags, visibility, stability, jsonb_build_object('bundle_id', bundle_id, 'material_id', material_id, 'cache_zone', default_cache_zone, 'extracted_block_id', coalesce(extracted_block_id,'')) as scope_json, source_refs, jsonb_build_object('bundle_id', bundle_id, 'material_id', material_id, 'load_when', load_when, 'dependencies', dependencies, 'extraction_status', extraction_status) as metadata, updated_at from material_index"}$config$::jsonb),
(gen_random_uuid(),'db.book_locator_entries','sql_query','Book and module locator entries',true,95,$config${"sql":"select 'book_locator:' || locator_id as search_doc_id, 'book_locator' as origin, case when owner_kind = 'module' then 'modules' else 'rules' end as domain, category as logical_kind, label as title, coalesce(summary,'') || ' ' || search_terms::text as body, tags, 'gm_only' as visibility, 'rarely_changed' as stability, jsonb_build_object('owner_id', owner_id, 'owner_kind', owner_kind, 'source_document_id', source_document_id, 'page_start', coalesce(page_start,0)::text, 'page_end', coalesce(page_end,0)::text) as scope_json, source_refs, jsonb_build_object('locator_id', locator_id, 'owner_id', owner_id, 'owner_kind', owner_kind, 'heading_path', heading_path, 'search_terms', search_terms, 'parse_policy', parse_policy, 'confidence', confidence) as metadata, updated_at from book_locator_entries"}$config$::jsonb),
(gen_random_uuid(),'db.module_prep_packets','sql_query','Module prep packets',true,88,$config${"sql":"select 'module_prep:' || prep_id as search_doc_id, 'module_prep_packet' as origin, 'modules' as domain, mode as logical_kind, title, packet_json::text as body, array['module_prep', mode]::text[] as tags, 'gm_only' as visibility, 'scene_stable' as stability, jsonb_build_object('prep_id', prep_id, 'module_id', module_id, 'ruleset_id', coalesce(ruleset_id,''), 'mode', mode) as scope_json, source_refs, jsonb_build_object('prep_id', prep_id, 'content_hash', content_hash) as metadata, updated_at from module_prep_packets"}$config$::jsonb),
(gen_random_uuid(),'db.memory_events','sql_query','GM memory events',true,70,$config${"sql":"select 'memory_event:' || event_id as search_doc_id, 'memory_event' as origin, 'memory' as domain, event_kind as logical_kind, summary as title, coalesce(summary,'') || ' ' || coalesce(transcript_excerpt,'') as body, tags, visibility, 'turn_dynamic' as stability, jsonb_build_object('session_id', session_id, 'ruleset_id', ruleset_id, 'module_id', coalesce(module_id,''), 'scene_id', coalesce(scene_id,''), 'location_id', coalesce(location_id,'')) as scope_json, '[]'::jsonb as source_refs, jsonb_build_object('event_id', event_id, 'turn_id', turn_id, 'importance', importance, 'source', source_json) as metadata, occurred_at as updated_at from memory_events"}$config$::jsonb),
(gen_random_uuid(),'db.memory_facts','sql_query','GM memory facts',true,75,$config${"sql":"select 'memory_fact:' || fact_id as search_doc_id, 'memory_fact' as origin, 'memory' as domain, predicate as logical_kind, subject || ' ' || predicate as title, summary as body, tags, visibility, 'scene_stable' as stability, jsonb_build_object('session_id', session_id, 'scope_type', scope_type, 'scope_id', scope_id) as scope_json, '[]'::jsonb as source_refs, jsonb_build_object('fact_id', fact_id, 'subject', subject, 'predicate', predicate, 'object', object_json, 'status', status, 'confidence', confidence, 'source_event_ids', source_event_ids) as metadata, updated_at from memory_facts where status = 'active'"}$config$::jsonb),
(gen_random_uuid(),'db.memory_snapshots','sql_query','GM memory snapshots',true,78,$config${"sql":"select 'memory_snapshot:' || snapshot_id || ':' || version::text as search_doc_id, 'memory_snapshot' as origin, 'memory' as domain, 'memory_snapshot' as logical_kind, title, summary_markdown as body, array['memory','snapshot']::text[] as tags, visibility, 'scene_stable' as stability, jsonb_build_object('session_id', session_id, 'ruleset_id', ruleset_id, 'module_id', coalesce(module_id,''), 'scope_type', scope_type, 'scope_id', scope_id) as scope_json, '[]'::jsonb as source_refs, jsonb_build_object('snapshot_id', snapshot_id, 'version', version, 'included_event_ids', included_event_ids, 'included_fact_ids', included_fact_ids, 'content_hash', content_hash) as metadata, updated_at from memory_snapshots"}$config$::jsonb),
(gen_random_uuid(),'db.rulings_log','sql_query','Rulings log',true,65,$config${"sql":"select 'ruling:' || ruling_id as search_doc_id, 'ruling' as origin, 'rulings' as domain, status as logical_kind, coalesce(demand_id, ruling_id) as title, ruling_text as body, array['ruling', status, confidence]::text[] as tags, 'gm_only' as visibility, case when provisional then 'turn_dynamic' else 'scene_stable' end as stability, jsonb_build_object('session_id', coalesce(session_id,''), 'ruleset_id', coalesce(ruleset_id,''), 'module_id', coalesce(module_id,''), 'demand_id', coalesce(demand_id,'')) as scope_json, source_refs, jsonb_build_object('ruling_id', ruling_id, 'status', status, 'confidence', confidence, 'provisional', provisional, 'superseded_by', superseded_by) as metadata, created_at as updated_at from rulings_log"}$config$::jsonb),
(gen_random_uuid(),'db.learned_packets','sql_query','Learned packets',true,85,$config${"sql":"select 'learned_packet:' || packet_id as search_doc_id, 'learned_packet' as origin, 'learned' as domain, packet_type as logical_kind, title, summary || ' ' || packet_json::text as body, array['learned_packet', packet_type, packet_key, learning_stage]::text[] as tags, visibility, case when learning_stage = 'memorized' then 'rarely_changed' when learning_stage = 'stable' then 'scene_stable' else 'turn_dynamic' end as stability, jsonb_build_object('ruleset_id', ruleset_id, 'module_id', coalesce(module_id,''), 'packet_type', packet_type, 'packet_key', packet_key, 'learning_stage', learning_stage) as scope_json, source_refs, jsonb_build_object('packet_id', packet_id, 'use_count', use_count, 'learning_stage', learning_stage, 'confidence', confidence, 'cache_zone', cache_zone) as metadata, updated_at from learned_packets"}$config$::jsonb),
(gen_random_uuid(),'files.markdown','file_glob','Page-anchored markdown files',true,40,'{"root":"markdown","extensions":["md","txt"],"origin":"markdown_file","domain":"source","logical_kind":"source_excerpt","visibility":"gm_only"}'::jsonb),
(gen_random_uuid(),'files.parsed_jsonl','jsonl','JSONL parsed artifacts',true,35,'{"root":"parsed","origin":"jsonl_artifact","domain":"parsed","visibility":"gm_only"}'::jsonb)
on conflict (source_config_id) do update set source_kind = excluded.source_kind, label = excluded.label, enabled = excluded.enabled, priority = excluded.priority, config_json = excluded.config_json, updated_at = now();

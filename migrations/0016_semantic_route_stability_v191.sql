-- v1.9.1 Semantic Route Stability Hotfix
-- Optional diagnostic/cache tables for deterministic route arbitration.

create table if not exists semantic_route_cache (
  id uuid primary key,
  route_cache_key text not null unique,
  session_id text not null,
  ruleset_id text not null,
  normalized_input text not null,
  context_hash text,
  semantic_json jsonb not null,
  route_json jsonb not null,
  model_id text,
  classifier_version text not null default 'v1.9.1',
  world_tick bigint,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
create index if not exists semantic_route_cache_session_idx on semantic_route_cache(session_id, created_at desc);

create table if not exists route_invariant_events (
  id uuid primary key,
  event_id text not null unique,
  session_id text not null,
  turn_id text,
  invariant_kind text not null,
  before_json jsonb not null default '{}'::jsonb,
  after_json jsonb not null default '{}'::jsonb,
  world_tick bigint,
  created_at timestamptz not null default now()
);
create index if not exists route_invariant_events_session_idx on route_invariant_events(session_id, created_at desc);

-- v1.11 Contest / Opposition Kernel

create table if not exists contest_profiles (
  id uuid primary key,
  contest_id text not null unique,
  session_id text not null,
  turn_id text not null,
  check_id text not null,
  ruleset_id text not null,
  module_id text,
  contest_kind text not null,
  attacker_actor_id text not null,
  defender_actor_id text,
  resolution_model_json jsonb not null default '{}'::jsonb,
  profile_json jsonb not null default '{}'::jsonb,
  verification_status text not null,
  world_tick bigint,
  created_at timestamptz not null default now()
);
create index if not exists contest_profiles_session_idx on contest_profiles(session_id, created_at desc);
create index if not exists contest_profiles_check_idx on contest_profiles(check_id);

create table if not exists opposition_profiles (
  id uuid primary key,
  opposition_id text not null unique,
  session_id text not null,
  check_id text not null,
  defender_actor_id text,
  defense_label text,
  defense_value int,
  opposed_expression text,
  profile_json jsonb not null default '{}'::jsonb,
  confidence text,
  provisional_reason text,
  created_at timestamptz not null default now()
);

create table if not exists contest_resolution_events (
  id uuid primary key,
  resolution_id text not null unique,
  contest_id text not null,
  session_id text not null,
  turn_id text not null,
  check_id text not null,
  roll_id text,
  total bigint,
  target_value bigint,
  success boolean,
  degree text,
  outcome_json jsonb not null default '{}'::jsonb,
  created_at timestamptz not null default now()
);
create index if not exists contest_resolution_events_session_idx on contest_resolution_events(session_id, created_at desc);
create index if not exists contest_resolution_events_check_idx on contest_resolution_events(check_id);

alter table check_contracts add column if not exists contest_profile_id text;
alter table check_contracts add column if not exists resolution_model_json jsonb not null default '{}'::jsonb;

alter table attack_resolution_contracts add column if not exists contest_id text;
alter table damage_packets add column if not exists contest_id text;

-- v0.9 Agentic Check / Roll / Combat scaffolding.
-- These tables store the Rust GM Agent's structured decisions and tool calls.
-- Advice/policy remains data-driven and is not embedded into prompt text.

create table if not exists agent_turns (
  id uuid primary key,
  session_id text not null,
  turn_id text not null,
  ruleset_id text not null,
  module_id text,
  plan_json jsonb not null,
  status text not null default 'planned',
  created_at timestamptz not null default now(),
  unique(session_id, turn_id)
);

create table if not exists agent_tool_calls (
  id uuid primary key,
  tool_call_id text not null unique,
  session_id text not null,
  turn_id text not null,
  tool_name text not null,
  visibility text not null,
  input_json jsonb not null,
  output_json jsonb,
  status text not null,
  error text,
  created_at timestamptz not null default now()
);

create table if not exists check_contracts (
  id uuid primary key,
  check_id text not null unique,
  session_id text not null,
  turn_id text not null,
  ruleset_id text not null,
  module_id text,
  actor_id text not null,
  target_actor_id text,
  roll_visibility text not null,
  contract_json jsonb not null,
  status text not null default 'created',
  created_at timestamptz not null default now()
);

create table if not exists pending_checks (
  id uuid primary key,
  check_id text not null unique,
  session_id text not null,
  expected_input_kind text not null,
  prompt_public text not null,
  contract_json jsonb not null,
  expires_at timestamptz,
  status text not null default 'open',
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create index if not exists pending_checks_session_status_idx on pending_checks(session_id, status, created_at desc);

create table if not exists dice_rolls (
  id uuid primary key,
  roll_id text not null unique,
  session_id text not null,
  turn_id text not null,
  check_id text,
  roller_kind text not null,
  roller_id text,
  visibility text not null,
  expression text not null,
  result_json jsonb not null,
  seed_commitment text not null,
  revealed_at timestamptz,
  created_at timestamptz not null default now()
);

create table if not exists check_results (
  id uuid primary key,
  check_id text not null,
  roll_json jsonb not null,
  outcome_json jsonb not null,
  committed_patches jsonb not null default '[]'::jsonb,
  created_at timestamptz not null default now()
);

create table if not exists combat_frames (
  id uuid primary key,
  combat_id text not null unique,
  session_id text not null,
  ruleset_id text not null,
  module_id text,
  frame_json jsonb not null,
  status text not null,
  updated_at timestamptz not null default now()
);

create table if not exists combat_events (
  id uuid primary key,
  combat_id text not null,
  session_id text not null,
  round int,
  actor_id text,
  event_kind text not null,
  event_json jsonb not null,
  created_at timestamptz not null default now()
);

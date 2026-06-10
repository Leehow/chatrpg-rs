-- v0.9.1 Interaction Gates + Working State Frames.
-- Interaction gates make player obligations explicit: roll, choose a reaction,
-- confirm risk, pick target, or abandon the bound action. State frames are
-- temporary working memory containers for combat, side quests, investigations,
-- chases, and hazards. They are projected into BP3 and compacted at completion.

alter table sessions add column if not exists active_interaction_gate_id text;

create table if not exists interaction_gates (
  id uuid primary key,
  gate_id text not null unique,
  session_id text not null,
  turn_id text not null,
  gate_kind text not null,
  status text not null default 'open',
  prompt_public text not null,
  prompt_gm text,
  required boolean not null default true,
  allowed_options jsonb not null default '[]'::jsonb,
  expected_input jsonb not null default '{}'::jsonb,
  on_unparseable text not null default 'reprompt',
  on_new_action text not null default 'cancel_gate_and_continue',
  on_timeout text not null default 'abort_current_action',
  bound_action_summary text not null default '',
  source_refs jsonb not null default '[]'::jsonb,
  advice_refs text[] not null default '{}',
  resolution_json jsonb,
  expires_at_turn text,
  expires_at_time timestamptz,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create index if not exists interaction_gates_session_status_idx
  on interaction_gates(session_id, status, created_at desc);

create table if not exists state_frames (
  id uuid primary key,
  frame_id text not null unique,
  frame_kind text not null,
  session_id text not null,
  ruleset_id text not null,
  module_id text,
  parent_frame_id text,
  scope_type text not null default 'session',
  scope_id text not null,
  status text not null default 'active',
  title text not null,
  objective text not null default '',
  static_refs text[] not null default '{}',
  working_state jsonb not null default '{}'::jsonb,
  active_gate_ids text[] not null default '{}',
  local_clocks jsonb not null default '[]'::jsonb,
  local_facts jsonb not null default '[]'::jsonb,
  local_modifiers jsonb not null default '[]'::jsonb,
  event_count integer not null default 0,
  last_event_ids text[] not null default '{}',
  retention_policy jsonb not null default '{}'::jsonb,
  compaction_policy jsonb not null default '{}'::jsonb,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create index if not exists state_frames_session_status_idx
  on state_frames(session_id, status, updated_at desc);

create table if not exists frame_events (
  id uuid primary key,
  event_id text not null unique,
  frame_id text not null,
  session_id text not null,
  turn_id text,
  event_kind text not null,
  event_json jsonb not null default '{}'::jsonb,
  visibility text not null default 'gm_only',
  created_at timestamptz not null default now()
);

create index if not exists frame_events_frame_time_idx
  on frame_events(frame_id, created_at desc);

create table if not exists frame_compactions (
  id uuid primary key,
  compaction_id text not null unique,
  frame_id text not null,
  session_id text not null,
  summary_markdown text not null,
  persistent_world_patches jsonb not null default '[]'::jsonb,
  promoted_fact_ids text[] not null default '{}',
  archived_event_ids text[] not null default '{}',
  created_at timestamptz not null default now()
);

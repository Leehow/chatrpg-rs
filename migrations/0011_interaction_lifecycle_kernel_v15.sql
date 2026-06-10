-- v1.5 Interaction Lifecycle Kernel
-- Centralizes lifecycle ownership for frames, gates, reaction windows and pending checks.

alter table sessions
  add column if not exists interaction_generation bigint not null default 0,
  add column if not exists active_interaction_context_id text,
  add column if not exists scene_epoch text;

create table if not exists interaction_contexts (
  id uuid primary key,
  context_id text not null unique,
  session_id text not null,
  frame_id text,
  context_kind text not null,
  status text not null,
  generation bigint not null,
  active_gate_ids text[] not null default '{}',
  pending_check_ids text[] not null default '{}',
  reaction_window_ids text[] not null default '{}',
  child_context_ids text[] not null default '{}',
  opened_at_tick bigint not null default 0,
  closed_at_tick bigint,
  owner_json jsonb not null default '{}',
  context_json jsonb not null default '{}',
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
create index if not exists interaction_contexts_session_status_idx on interaction_contexts(session_id, status, generation);
create index if not exists interaction_contexts_frame_idx on interaction_contexts(frame_id);

create table if not exists interaction_events (
  id uuid primary key,
  event_id text not null unique,
  session_id text not null,
  interaction_context_id text,
  frame_id text,
  world_tick bigint not null default 0,
  generation bigint not null default 0,
  event_kind text not null,
  event_json jsonb not null default '{}',
  created_at timestamptz not null default now()
);
create index if not exists interaction_events_session_generation_idx on interaction_events(session_id, generation, world_tick);
create index if not exists interaction_events_frame_idx on interaction_events(frame_id);

create table if not exists invariant_repairs (
  id uuid primary key,
  repair_id text not null unique,
  session_id text not null,
  invariant text not null,
  target_table text not null,
  target_id text not null,
  action text not null,
  reason text not null,
  world_tick bigint not null default 0,
  generation bigint not null default 0,
  created_at timestamptz not null default now()
);
create index if not exists invariant_repairs_session_idx on invariant_repairs(session_id, created_at);

alter table interaction_gates
  add column if not exists interaction_context_id text,
  add column if not exists owner_frame_id text,
  add column if not exists generation bigint not null default 0,
  add column if not exists superseded_reason text,
  add column if not exists closed_at_tick bigint;
create index if not exists interaction_gates_session_status_generation_idx on interaction_gates(session_id, status, generation);
create index if not exists interaction_gates_owner_frame_idx on interaction_gates(owner_frame_id, status);

alter table pending_checks
  add column if not exists interaction_context_id text,
  add column if not exists owner_frame_id text,
  add column if not exists gate_id text,
  add column if not exists generation bigint not null default 0,
  add column if not exists superseded_reason text,
  add column if not exists closed_at_tick bigint;
create index if not exists pending_checks_session_status_generation_idx on pending_checks(session_id, status, generation);
create index if not exists pending_checks_owner_frame_idx on pending_checks(owner_frame_id, status);

alter table state_frames
  add column if not exists interaction_context_id text,
  add column if not exists generation bigint not null default 0,
  add column if not exists opened_at_tick bigint,
  add column if not exists closed_at_tick bigint;
create index if not exists state_frames_session_status_generation_idx on state_frames(session_id, status, generation);

alter table state_frames
  add column if not exists closed_reason text;

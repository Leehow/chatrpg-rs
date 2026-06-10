-- v1.4 World Time Spine

create table if not exists world_time_state (
  session_id text primary key,
  campaign_id text not null,
  world_tick bigint not null default 0,
  absolute_seconds bigint not null default 0,
  calendar_id text not null default 'relative_default',
  display_time text not null default 'Day 1, 00:00',
  time_scale text not null default 'scene_beat',
  scene_epoch text,
  turn_seq bigint not null default 0,
  event_seq bigint not null default 0,
  updated_at timestamptz not null default now()
);

create table if not exists world_events (
  id uuid primary key,
  event_id text not null unique,
  campaign_id text not null,
  session_id text not null,
  world_tick bigint not null,
  event_seq bigint not null,
  turn_id text,
  frame_id text,
  event_kind text not null,
  event_json jsonb not null,
  visibility text not null default 'gm_only',
  source_refs jsonb not null default '[]'::jsonb,
  caused_by_event_ids text[] not null default '{}',
  state_patch_ids text[] not null default '{}',
  created_at timestamptz not null default now(),
  unique(session_id, world_tick, event_seq)
);
create index if not exists world_events_session_tick_idx on world_events(session_id, world_tick, event_seq);
create index if not exists world_events_kind_idx on world_events(event_kind);
create index if not exists world_events_frame_idx on world_events(frame_id);

create table if not exists world_time_advances (
  id uuid primary key,
  advance_id text not null unique,
  session_id text not null,
  from_tick bigint not null,
  to_tick bigint not null,
  from_display text not null,
  to_display text not null,
  advance_event_id text,
  advance_json jsonb not null,
  created_at timestamptz not null default now()
);
create index if not exists world_time_advances_session_idx on world_time_advances(session_id, from_tick, to_tick);

create table if not exists scheduled_events (
  id uuid primary key,
  scheduled_event_id text not null unique,
  campaign_id text not null,
  session_id text not null,
  due_tick bigint not null,
  event_kind text not null,
  payload_json jsonb not null,
  visibility text not null default 'gm_only',
  status text not null default 'pending',
  created_by_event_id text,
  created_at timestamptz not null default now()
);
create index if not exists scheduled_events_due_idx on scheduled_events(session_id, status, due_tick);

create table if not exists time_anchors (
  id uuid primary key,
  anchor_id text not null unique,
  session_id text not null,
  world_tick bigint not null,
  event_seq bigint not null,
  label text not null,
  display_time text not null,
  created_at timestamptz not null default now()
);
create index if not exists time_anchors_session_tick_idx on time_anchors(session_id, world_tick, event_seq);

create table if not exists context_watermarks (
  session_id text primary key,
  last_compiled_world_tick bigint not null default 0,
  last_compiled_event_seq bigint not null default 0,
  compiled_context_hash text,
  updated_at timestamptz not null default now()
);

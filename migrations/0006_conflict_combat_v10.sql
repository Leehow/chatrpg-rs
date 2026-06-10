-- v1.0 Conflict Frame & Combat Agent extension.
-- StateFrame remains the canonical temporary working-state container; this
-- migration adds durable audit tables for effect contracts and indexes used by
-- ConflictAgent/CombatAgent.

create table if not exists effect_contracts (
  id uuid primary key,
  effect_id text not null unique,
  session_id text not null,
  turn_id text not null,
  source_event_id text,
  effect_kind text not null,
  target_actor_ids text[] not null default '{}',
  effect_json jsonb not null,
  visibility text not null default 'gm_only',
  confidence text not null default 'low',
  created_at timestamptz not null default now()
);

create index if not exists effect_contracts_session_turn_idx
  on effect_contracts(session_id, turn_id);

create index if not exists effect_contracts_kind_idx
  on effect_contracts(effect_kind);

create index if not exists state_frames_kind_status_idx
  on state_frames(frame_kind, status);

create index if not exists frame_events_frame_kind_idx
  on frame_events(frame_id, event_kind);

create index if not exists frame_events_session_turn_idx
  on frame_events(session_id, turn_id);

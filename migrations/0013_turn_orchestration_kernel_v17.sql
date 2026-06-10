-- v1.7 Turn Orchestration Kernel
-- Stores the reducer-level route chosen for each player turn. The runtime also
-- writes this as a WorldEvent, but a narrow table is useful for debugging and
-- harness assertions.

create table if not exists turn_orchestration_events (
  id uuid primary key,
  session_id text not null,
  turn_id text not null,
  route_kind text not null,
  gate_relation text not null,
  frame_relation text not null,
  action_kind text not null,
  active_frame_id text,
  active_gate_id text,
  superseded_gate_id text,
  orchestration_json jsonb not null,
  world_tick bigint,
  created_at timestamptz not null default now(),
  unique(session_id, turn_id)
);

create index if not exists turn_orchestration_events_session_turn_idx
  on turn_orchestration_events(session_id, turn_id);

create index if not exists turn_orchestration_events_route_idx
  on turn_orchestration_events(route_kind);

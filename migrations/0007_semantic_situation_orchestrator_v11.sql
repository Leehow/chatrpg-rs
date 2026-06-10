-- v1.1 Semantic Situation Orchestrator
-- These tables are append-only/audit friendly. Runtime can operate through frame_events
-- alone, but typed tables make future verifier/harness queries cheaper.

create table if not exists conflict_intents (
  id uuid primary key,
  intent_id text not null unique,
  session_id text not null,
  turn_id text not null,
  frame_id text,
  relation text not null,
  action_kind text not null,
  confidence text not null,
  intent_json jsonb not null,
  created_at timestamptz not null default now()
);

create index if not exists conflict_intents_session_turn_idx on conflict_intents(session_id, turn_id);
create index if not exists conflict_intents_frame_idx on conflict_intents(frame_id);
create index if not exists conflict_intents_action_idx on conflict_intents(action_kind);

create table if not exists exit_contracts (
  id uuid primary key,
  exit_id text not null unique,
  session_id text not null,
  turn_id text not null,
  frame_id text not null,
  exit_kind text not null,
  success_outcome text not null,
  failure_outcome text not null,
  contract_json jsonb not null,
  created_at timestamptz not null default now()
);

create index if not exists exit_contracts_session_turn_idx on exit_contracts(session_id, turn_id);
create index if not exists exit_contracts_frame_idx on exit_contracts(frame_id);

create table if not exists stalemate_contracts (
  id uuid primary key,
  stalemate_id text not null unique,
  session_id text not null,
  turn_id text not null,
  frame_id text not null,
  reason text not null,
  contract_json jsonb not null,
  created_at timestamptz not null default now()
);

create index if not exists stalemate_contracts_session_turn_idx on stalemate_contracts(session_id, turn_id);
create index if not exists stalemate_contracts_frame_idx on stalemate_contracts(frame_id);

create table if not exists npc_drive_state_events (
  id uuid primary key,
  event_id text not null unique,
  session_id text not null,
  turn_id text not null,
  frame_id text not null,
  npc_id text not null,
  morale integer,
  patience integer,
  tactic_id text,
  state_json jsonb not null,
  created_at timestamptz not null default now()
);

create index if not exists npc_drive_state_events_frame_idx on npc_drive_state_events(frame_id, npc_id);

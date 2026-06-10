-- v1.2 Actionable Situation Director

create table if not exists actionable_situation_briefs (
  id uuid primary key,
  brief_id text not null unique,
  session_id text not null,
  turn_id text not null,
  frame_id text,
  guidance_level text not null,
  brief_json jsonb not null,
  created_at timestamptz not null default now()
);
create index if not exists actionable_situation_briefs_session_turn_idx on actionable_situation_briefs(session_id, turn_id);
create index if not exists actionable_situation_briefs_frame_idx on actionable_situation_briefs(frame_id);

create table if not exists player_facing_clue_boards (
  id uuid primary key,
  board_id text not null unique,
  session_id text not null,
  turn_id text not null,
  board_json jsonb not null,
  updated_at timestamptz not null default now()
);
create index if not exists player_facing_clue_boards_session_idx on player_facing_clue_boards(session_id, updated_at desc);

create table if not exists consequence_contracts (
  id uuid primary key,
  consequence_id text not null unique,
  session_id text not null,
  turn_id text not null,
  frame_id text,
  fail_forward boolean not null default true,
  contract_json jsonb not null,
  created_at timestamptz not null default now()
);
create index if not exists consequence_contracts_session_turn_idx on consequence_contracts(session_id, turn_id);

create table if not exists clock_tick_events (
  id uuid primary key,
  session_id text not null,
  turn_id text not null,
  clock_id text not null,
  tick_json jsonb not null,
  created_at timestamptz not null default now()
);
create index if not exists clock_tick_events_session_idx on clock_tick_events(session_id, created_at desc);

create table if not exists spotlight_states (
  id uuid primary key,
  session_id text not null,
  player_id text not null,
  state_json jsonb not null,
  updated_at timestamptz not null default now(),
  unique(session_id, player_id)
);

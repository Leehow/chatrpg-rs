-- v1.3 Situation Novelty Director
-- Novelty is tracked as state, not prose. These tables support audit, harness,
-- future UI, and anti-repeat analytics. Runtime also writes novelty details into
-- frame_events so old deployments can still observe behavior without UI changes.

create table if not exists novelty_events (
  id uuid primary key,
  session_id text not null,
  turn_id text not null,
  frame_id text,
  decision_id text not null unique,
  novelty_json jsonb not null,
  created_at timestamptz not null default now()
);
create index if not exists novelty_events_session_turn_idx on novelty_events(session_id, turn_id);
create index if not exists novelty_events_frame_idx on novelty_events(frame_id);

create table if not exists fresh_changes (
  id uuid primary key,
  session_id text not null,
  turn_id text not null,
  frame_id text,
  change_id text not null unique,
  change_type text not null,
  change_json jsonb not null,
  created_at timestamptz not null default now()
);
create index if not exists fresh_changes_session_turn_idx on fresh_changes(session_id, turn_id);
create index if not exists fresh_changes_frame_idx on fresh_changes(frame_id);

create table if not exists tactic_cooldowns (
  id uuid primary key,
  frame_id text not null,
  actor_id text not null,
  tactic_id text not null,
  cooldown_remaining_turns integer not null default 0,
  updated_at timestamptz not null default now(),
  unique(frame_id, actor_id, tactic_id)
);
create index if not exists tactic_cooldowns_frame_idx on tactic_cooldowns(frame_id);

create table if not exists no_repeat_violations (
  id uuid primary key,
  session_id text not null,
  turn_id text not null,
  frame_id text,
  violation_kind text not null,
  violation_json jsonb not null,
  created_at timestamptz not null default now()
);
create index if not exists no_repeat_violations_session_turn_idx on no_repeat_violations(session_id, turn_id);

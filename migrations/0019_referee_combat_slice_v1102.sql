-- v1.10.2 Referee Combat Slice: mechanical ledger, attack resolution, damage packets

create table if not exists actor_mechanical_states (
  id uuid primary key,
  state_id text not null unique,
  session_id text not null,
  frame_id text,
  actor_id text not null,
  actor_kind text not null,
  hp_current int,
  hp_max int,
  armor_current int,
  wound_state text,
  morale int,
  resources_json jsonb not null default '{}'::jsonb,
  conditions_json jsonb not null default '[]'::jsonb,
  source_refs jsonb not null default '[]'::jsonb,
  provisional_reason text,
  world_tick bigint,
  updated_at timestamptz not null default now(),
  unique(session_id, actor_id)
);
create index if not exists actor_mechanical_states_session_idx on actor_mechanical_states(session_id, actor_id);

create table if not exists attack_resolution_contracts (
  id uuid primary key,
  attack_id text not null unique,
  session_id text not null,
  turn_id text not null,
  frame_id text,
  source_actor_id text not null,
  target_actor_id text,
  target_object_id text,
  source_object_id text,
  source_ability_id text,
  check_id text,
  hit_result text not null default 'pending',
  damage_check_id text,
  attack_json jsonb not null,
  world_tick bigint,
  created_at timestamptz not null default now()
);
create index if not exists attack_resolution_contracts_session_idx on attack_resolution_contracts(session_id, turn_id);
create index if not exists attack_resolution_contracts_check_idx on attack_resolution_contracts(check_id);

create table if not exists damage_packets (
  id uuid primary key,
  damage_packet_id text not null unique,
  session_id text not null,
  turn_id text not null,
  frame_id text,
  source_actor_id text,
  target_actor_id text not null,
  source_object_id text,
  source_ability_id text,
  damage_expression text,
  rolled_total int not null,
  damage_type text,
  armor_interaction_json jsonb not null default '{}'::jsonb,
  final_hp_delta int not null,
  from_hp int,
  to_hp int,
  source_refs jsonb not null default '[]'::jsonb,
  provisional_reason text,
  damage_json jsonb not null,
  world_tick bigint,
  created_at timestamptz not null default now()
);
create index if not exists damage_packets_session_idx on damage_packets(session_id, turn_id);
create index if not exists damage_packets_target_idx on damage_packets(session_id, target_actor_id);

create table if not exists combat_round_events (
  id uuid primary key,
  combat_round_event_id text not null unique,
  session_id text not null,
  turn_id text not null,
  frame_id text,
  action_kind text not null,
  event_json jsonb not null,
  world_tick bigint,
  created_at timestamptz not null default now()
);

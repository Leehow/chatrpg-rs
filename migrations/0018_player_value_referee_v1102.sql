-- v1.10.2 Player-supplied value verification and table override audit

create table if not exists player_value_claims (
  id uuid primary key,
  claim_id text not null unique,
  session_id text not null,
  turn_id text not null,
  ruleset_id text not null,
  module_id text,
  value_kind text not null,
  label text not null,
  supplied_value_json jsonb not null,
  evidence_span text not null,
  context_summary text not null,
  target_actor_id text,
  target_object_id text,
  target_ability_id text,
  source_refs jsonb not null default '[]'::jsonb,
  visibility text not null default 'gm_only',
  world_tick bigint,
  created_at timestamptz not null default now()
);
create index if not exists player_value_claims_session_turn_idx on player_value_claims(session_id, turn_id);
create index if not exists player_value_claims_kind_idx on player_value_claims(value_kind);

create table if not exists player_value_verifications (
  id uuid primary key,
  verification_id text not null unique,
  claim_id text not null,
  session_id text not null,
  turn_id text not null,
  status text not null,
  canonical_value_json jsonb not null default '{}'::jsonb,
  acceptable_range_json jsonb not null default '{}'::jsonb,
  comparison_json jsonb not null default '{}'::jsonb,
  source_refs jsonb not null default '[]'::jsonb,
  warning_public text,
  suggestion_public text,
  accepted_if_player_insists boolean not null default false,
  balance_risk text,
  verifier_json jsonb not null default '{}'::jsonb,
  world_tick bigint,
  created_at timestamptz not null default now()
);
create index if not exists player_value_verifications_session_turn_idx on player_value_verifications(session_id, turn_id);
create index if not exists player_value_verifications_claim_idx on player_value_verifications(claim_id);
create index if not exists player_value_verifications_status_idx on player_value_verifications(status);

create table if not exists table_override_agreements (
  id uuid primary key,
  override_id text not null unique,
  session_id text not null,
  claim_id text not null,
  verification_id text not null,
  status text not null,
  accepted_by text,
  reason text,
  warning_public text not null,
  created_at_tick bigint,
  created_at timestamptz not null default now()
);
create index if not exists table_override_agreements_session_idx on table_override_agreements(session_id);
create index if not exists table_override_agreements_claim_idx on table_override_agreements(claim_id);

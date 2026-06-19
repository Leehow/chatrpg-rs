-- 0036: NPC Profile Store v1 (TC-D3-01). Additive-only: a durable home for a static,
-- source-grounded NPC persona (trpg_model::NpcProfile) so runtime mind/behavior
-- projections can load reusable profile state instead of test-only structs.
--
-- Identity: the profile holder is the stable NPC actor id (actor_id), validated in
-- Rust via the TC-KNOW-00 actor-identity contract before any write (display-name /
-- ad-hoc / placeholder ids fail closed). The unique (session_id, actor_id) key gives
-- one profile per NPC per session.
--
-- Storage: the complete profile (INCLUDING GM-only secrets) lives in profile_json
-- jsonb — durable storage is the GM's source of truth. Only safe views
-- (NpcProfile::safe_view) may enter prompt-facing adapters; this table never strips
-- secrets. name/role are denormalized indexed display columns for cheap lookup/listing;
-- profile_json remains authoritative. create table / indexes are idempotent; all
-- statements run inside the single advisory-locked migration transaction.

create table if not exists npc_profiles (
  id uuid primary key default gen_random_uuid(),
  session_id text not null,
  actor_id text not null,
  name text not null default '',
  role text,
  profile_json jsonb not null,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  constraint npc_profiles_unique_holder unique (session_id, actor_id)
);

create index if not exists idx_npc_profiles_session_actor
  on npc_profiles (session_id, actor_id);

create index if not exists idx_npc_profiles_session_name
  on npc_profiles (session_id, name);

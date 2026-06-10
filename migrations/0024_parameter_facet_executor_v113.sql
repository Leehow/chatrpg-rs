-- v1.13 Parameter Facet Executor & Ruleset Starter Profiles
-- Executes existing actor/object/ability/check/effect facets instead of creating ruleset-specific engines.

create table if not exists parameter_facet_execution_runs (
  id uuid primary key,
  execution_id text not null unique,
  session_id text not null,
  turn_id text,
  frame_id text,
  execution_kind text not null,
  source_facet_binding_ids jsonb not null default '[]'::jsonb,
  ruleset_id text not null,
  target_kind text not null,
  target_id text not null,
  parameter_path text not null,
  operation text not null,
  input_json jsonb not null default '{}'::jsonb,
  output_json jsonb not null default '{}'::jsonb,
  status text not null,
  visibility text not null default 'gm_only',
  source_refs jsonb not null default '[]'::jsonb,
  provisional_reason text,
  world_tick bigint,
  created_at timestamptz not null default now()
);
create index if not exists parameter_facet_execution_runs_session_created_idx on parameter_facet_execution_runs(session_id, created_at desc);
create index if not exists parameter_facet_execution_runs_target_idx on parameter_facet_execution_runs(target_kind, target_id, parameter_path);

create table if not exists generic_parameter_states (
  id uuid primary key,
  state_id text not null unique,
  session_id text not null,
  target_kind text not null,
  target_id text not null,
  parameter_path text not null,
  value_json jsonb not null default '0'::jsonb,
  visibility text not null default 'gm_only',
  source_refs jsonb not null default '[]'::jsonb,
  provisional_reason text,
  world_tick bigint,
  updated_at timestamptz not null default now(),
  unique(session_id, target_kind, target_id, parameter_path)
);
create index if not exists generic_parameter_states_session_target_idx on generic_parameter_states(session_id, target_kind, target_id);

create table if not exists ruleset_mechanical_profiles (
  id uuid primary key,
  profile_id text not null unique,
  ruleset_id text not null,
  profile_json jsonb not null default '{}'::jsonb,
  source_refs jsonb not null default '[]'::jsonb,
  enabled boolean not null default true,
  updated_at timestamptz not null default now()
);
create index if not exists ruleset_mechanical_profiles_ruleset_idx on ruleset_mechanical_profiles(ruleset_id) where enabled;

insert into ruleset_mechanical_profiles (id, profile_id, ruleset_id, profile_json)
values
  (gen_random_uuid(), 'profile.cyberpunk_red.v1_13', 'cyberpunk_red', '{
    "base_roll_expression":"1d10",
    "default_attack_expression":null,
    "default_attack_target":null,
    "default_damage_expression":null,
    "requires_source_backed_parameters":true,
    "effect_defaults":{"weapon_damage":"hp.current","netrunning":"object.control_state","humanity":"resources.humanity.current"},
    "resources":{"hp":{"path":"hp.current"},"humanity":{"path":"resources.humanity.current"}},
    "armor":{"track":"armor_current","unknown_sp_default":null,"audit_unknown":true},
    "aliases":{"target_number":["DV","range DV"],"armor":["SP","ablation"],"damage":["damage","weapon damage"]}
  }'::jsonb),
  (gen_random_uuid(), 'profile.dnd5e.v1_13', 'dnd5e', '{
    "base_roll_expression":"1d20",
    "default_attack_expression":null,
    "default_attack_target":null,
    "default_damage_expression":null,
    "requires_source_backed_parameters":true,
    "effect_defaults":{"weapon_damage":"hp.current","spell_damage":"hp.current","condition":"conditions"},
    "resources":{"hp":{"path":"hp.current"},"spell_slots":{"path":"resources.spell_slots"}},
    "defense":{"ac":"actor.defense.ac","save_dc":"actor.ability.spell_save_dc"},
    "aliases":{"defense":["AC","Armor Class"],"save":["saving throw","spell save DC"],"condition":["conditions"]}
  }'::jsonb),
  (gen_random_uuid(), 'profile.sword_world_2_5.v1_13', 'sword_world_2_5', '{
    "base_roll_expression":"2d6",
    "default_attack_expression":null,
    "default_attack_target":null,
    "default_damage_expression":null,
    "requires_source_backed_parameters":true,
    "effect_defaults":{"weapon_damage":"hp.current","spell_damage":"hp.current","spell_condition":"conditions"},
    "table_facets":{"power_table":"object.damage.power_table","evasion":"actor.defense.evasion","resistance":"actor.defense.resistance"},
    "aliases":{"damage":["Damage","Power Table","威力表"],"defense":["Evasion","Resistance","Defense"],"combat":["Combat Rules","Weapon Attacks"]}
  }'::jsonb),
  (gen_random_uuid(), 'profile.brp_coc.v1_13', 'brp_coc', '{
    "base_roll_expression":"1d100",
    "default_attack_expression":null,
    "default_attack_target":null,
    "default_damage_expression":null,
    "requires_source_backed_parameters":true,
    "effect_defaults":{"weapon_damage":"hp.current","sanity_loss":"resources.sanity.current","major_wound":"conditions.major_wound"},
    "resources":{"hp":{"path":"hp.current"},"sanity":{"path":"resources.sanity.current"}},
    "thresholds":{"regular":"skill","hard":"half","extreme":"fifth"},
    "aliases":{"ability":["Ability","Skill","Base Chance"],"sanity":["SAN","Sanity","Sanity Loss"],"wound":["Major Wound","HP"]}
  }'::jsonb),
  (gen_random_uuid(), 'profile.triangle_agency.v1_13', 'triangle_agency', '{
    "base_roll_expression":"6d4",
    "default_attack_expression":null,
    "default_attack_target":null,
    "default_damage_expression":null,
    "requires_source_backed_parameters":true,
    "effect_defaults":{"harm":"resources.harm.current","chaos":"tracks.chaos.current","anomaly":"anomaly.state"},
    "resources":{"harm":{"path":"resources.harm.current"},"chaos":{"path":"tracks.chaos.current"}},
    "visibility":{"playwalled":true,"respect_agency_property":true,"redact_numberless_pages":true},
    "aliases":{"harm":["Harm"],"chaos":["Chaos","Chaos Effects"],"anomaly":["Anomaly","Impulse","Focus"]}
  }'::jsonb)
on conflict (profile_id) do update set profile_json = excluded.profile_json, updated_at = now();

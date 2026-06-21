-- A2b-U (Q4 FLAG-TRAP fix): record whether the pre-turn opening-scene establishing
-- narration was ACTUALLY delivered for a session (set by the CLI opening hook), so the
-- L-G first-entry gate can suppress turn-1 read_aloud re-delivery based on a real fact
-- instead of inferring it from `flag_on && module_bound` (which wrongly suppressed the
-- API/engine path where nothing pre-delivered). Additive, idempotent; default false ⇒
-- existing rows + the API path read false ⇒ turn-1 opening surfaces (byte-equal baseline).
alter table sessions add column if not exists opening_delivered boolean not null default false;

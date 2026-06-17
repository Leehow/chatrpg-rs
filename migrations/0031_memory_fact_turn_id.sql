-- 0031: turn_id provenance on memory_facts (emergent relationship triples).
-- Additive + nullable: existing facts keep NULL. Records which turn established a
-- durable fact (e.g. a relationship triple extracted from narration), alongside
-- the existing source_event_ids provenance vector.
alter table memory_facts add column if not exists turn_id text;

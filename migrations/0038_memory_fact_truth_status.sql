-- 0038: truth_status fact classification on memory_facts (设计3 §4).
-- Additive + nullable: existing facts keep NULL (= unclassified / Unknown). Records the
-- proposition's OWN truth axis (true/false/rumor/lie/subjective/unknown) — orthogonal to
-- knowledge_edges (who believes what). No existing column or semantics is changed; the
-- proposal/commit pipeline wiring is codex-owned and intentionally NOT touched here.
alter table memory_facts add column if not exists truth_status text;

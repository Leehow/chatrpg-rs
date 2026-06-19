-- 0039: world_facts table — WorldFact 一等化（P3 layered-runtime）。
-- 设计4 §19 R4-D2：世界事实身份独立于「记忆三元组」(memory_facts) 与「谁知道」
-- (knowledge_edges)。WorldFactCandidate 提交落此表，memory_facts 仅保留记忆三元组用途。
-- 列同 WorldFactCandidate 字段（fact_id 身份 + subject/predicate/object/summary +
-- truth_status 命题真假轴 + 证据 source_event_ids + turn_id provenance + confidence）
-- 外加 created_at/updated_at。truth_status 加性可空（缺省 NULL = 未标 / Unknown）。
--
-- 幂等可重放：create table if not exists + upsert on conflict(fact_id)。
-- fact_id 全局唯一(镜像 memory_facts.fact_id `not null unique` 约定 → world_facts 用
--   `fact_id primary key` 一致;读路径 load_world_fact 仍按 (session_id, fact_id) 过滤)。
-- session_id 加 `references sessions on delete cascade`(镜像 memory_facts):session 删除时
--   world_facts 行随之清理,不留孤儿。本表 P3 新建从未上线,加 FK = 纯加性硬化,非破坏性迁移。
-- 无 FK 到 knowledge_edges.fact_id(WorldFact↔KnowledgeEdge 引用契约本阶段 WEAK:孤儿边
--   warn+trace 不阻断,见 trpg-model world_fact_ref)——历史 player-reveal 边可能引用尚无
--   world_fact 身份的 fact_id,加 FK 会破坏存量数据,留待后续阶段(回填后)再定。
create table if not exists world_facts (
  fact_id text primary key,
  session_id text not null references sessions(session_id) on delete cascade,
  subject text not null,
  predicate text not null,
  object text not null,
  summary text not null default '',
  truth_status text,
  source_event_ids text[] not null default '{}',
  turn_id text,
  confidence real,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create index if not exists idx_world_facts_session
  on world_facts(session_id);

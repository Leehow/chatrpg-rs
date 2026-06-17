-- migrations/0030_domain_events.sql
-- 统一 domain_events 日志（优化2 #6 起步，eventlog write-through）：append-only 的
-- 领域事件日志，**不耦合 tick**（区别于 world_events 的 world_tick/event_seq）。
-- ① seq bigserial：自带单调序，回合生命周期 / 切场景事件无需造 tick。
-- ② event_id unique + append 端 on conflict do nothing：确定性幂等键
--    （如 de_{turn_id}_{kind}），同回合重放不产生重复行。
-- ③ data/source_refs jsonb：通用载荷（零规则集硬编码）。本切片只新增日志 + 写穿，
--    projection 派生留后续（先 write-through 后派生）。
create table if not exists domain_events (
  seq bigserial primary key,
  event_id text not null unique,
  session_id text not null,
  turn_id text,
  kind text not null,
  data jsonb not null default '{}'::jsonb,
  source_refs jsonb not null default '[]'::jsonb,
  created_at timestamptz not null default now()
);
-- 按会话取事件（seq 升序时间线）。
create index if not exists idx_domain_events_session on domain_events (session_id, seq);
-- 按回合过滤（explain / inspect 附该回合 domain events）。
create index if not exists idx_domain_events_turn on domain_events (turn_id);

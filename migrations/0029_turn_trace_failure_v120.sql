-- migrations/0029_turn_trace_failure_v120.sql
-- 可观测性 + 失败语义切片（obs slice T4）：失败语义落库 + Flight Recorder 持久化。
-- ① turns.failure_kind：NULL=成功；失败时写归类值（failed_context / failed_mode /
--    failed_finalize 等）。fail-closed——失败不再伪装成空 TurnComplete。
-- ② turn_traces：单回合飞行记录（TurnTrace）的 jsonb 落库，turn_id PK、write-through、
--    fail-soft（写失败仅 warn，不影响回合主流程）。
alter table turns add column if not exists failure_kind text;
create table if not exists turn_traces (
  turn_id text primary key,
  session_id text not null,
  trace_json jsonb not null,
  created_at timestamptz not null default now()
);
-- 按会话取最近若干回合 trace（explain / inspect 用）。
create index if not exists idx_turn_traces_session on turn_traces (session_id, created_at desc);

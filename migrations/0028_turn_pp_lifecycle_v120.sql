-- migrations/0028_turn_pp_lifecycle_v120.sql
-- R5 postprocess 生命周期：streaming → critical_done → complete。
-- 独立于 turns.postprocess_status（ready/awaiting/draft，finalize 终态保持原义不变），
-- 本列专做高水位守卫的后处理落账进度追踪。默认 streaming（save_turn 时即此态）。
alter table turns add column if not exists pp_lifecycle text not null default 'streaming';
create index if not exists idx_turns_session_created_at on turns (session_id, created_at desc);

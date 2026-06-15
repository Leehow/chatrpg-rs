-- migrations/0028_turn_pp_lifecycle_v120.sql
-- R5 postprocess 生命周期：streaming → critical_done → complete。
-- 独立于 turns.postprocess_status（ready/awaiting/draft，finalize 终态保持原义不变），
-- 本列专做高水位守卫的后处理落账进度追踪。默认 streaming（save_turn 时即此态）。
alter table turns add column if not exists pp_lifecycle text not null default 'streaming';
-- 存量回填：迁移在 startup 运行、此刻无活动回合，故所有既有回合都已完成 → 标 complete。
-- 否则它们停留在 ADD 的默认 'streaming'（看似在途），令高水位守卫对每个老会话首个新回合
-- 白等 2s 超时。一次性迁移，新回合仍由 save 流程经默认 streaming→critical_done→complete。
update turns set pp_lifecycle = 'complete' where pp_lifecycle = 'streaming';
create index if not exists idx_turns_session_created_at on turns (session_id, created_at desc);

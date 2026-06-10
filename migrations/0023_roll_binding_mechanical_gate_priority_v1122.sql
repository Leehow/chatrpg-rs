-- v1.12.2 Roll Binding & Mechanical Gate Priority Hotfix
-- No new primary state tables; this migration adds lookup indexes used by
-- `/roll` binding to the latest unresolved CheckContract and by effect follow-up gates.

create index if not exists check_contracts_session_status_created_idx
  on check_contracts(session_id, status, created_at desc);

create index if not exists check_results_check_id_idx
  on check_results(check_id);

create index if not exists pending_checks_session_status_created_idx
  on pending_checks(session_id, status, created_at desc);

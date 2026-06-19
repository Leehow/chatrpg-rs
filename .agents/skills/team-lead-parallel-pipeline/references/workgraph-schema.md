# WorkGraph schema

The WorkGraph is lead-owned scheduling state. Workers read their task row but do not edit the graph.

```yaml
version: 1
work_id: knowledge-memory-npc
run_policy: continuous_until_terminal
integration_branch: codex/knowledge-memory-npc-integration
integration_head: <sha>
local_git_authorization:
  worker_scoped_commit: true
  trial_cherry_pick: true
  integration_fast_forward: true
  push: false
  main_merge: false
wip:
  contract_writers: 1
  implementation_writers: 4
  verification_lanes: 2
  integration_lanes: 1
hotspots:
  model_facade: crates/trpg-model/src/lib.rs
  db_facade: crates/trpg-db/src/lib.rs
  runtime_composition_root: crates/trpg-runtime/src/lib.rs
  workspace_manifest: Cargo.toml
  migrations: migrations/**
tasks:
  - id: know-contract-v1
    kind: contract
    acceptance_ids: [DA-KNOW-01, DA-KNOW-04]
    state: ready
    depends_on: []
    base_sha: <integration_sha>
    branch: claude/knowledge-memory-npc/know-contract-v1
    worktree: .worktrees/knowledge-memory-npc/know-contract-v1
    read_set: [crates/trpg-model/**]
    write_set:
      - crates/trpg-model/src/knowledge/**
    hotspot_leases: [model_facade]
    contract_dependencies: []
    generated_outputs: []
    migration_slot: none
    decision_policy: autonomous_within_contract
    validation_profile: contract-rust
    scenario_ids: []
    commit_sha: null
    integration_sha: null
```

## States

`planned -> ready -> leased -> running -> review -> accepted -> trial_integrating -> integrated -> verified`

Alternative terminal or repair states:

- `revision_required`
- `blocked_dependency`
- `blocked_human`
- `deferred`
- `obsolete`
- `rejected`

Only `verified`, approved `deferred`, and `blocked_human` are terminal for epic accounting.

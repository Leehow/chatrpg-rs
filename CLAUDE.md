# ChatRPG Rust Claude Worker Adapter

Claude Code workers in this repository must follow `AGENTS.md`, the global
`team-lead-worker` skill, and the project pipeline files under
`docs/team-lead/`.

For continuous pipeline work:

- confirm the declared worktree, branch, base SHA, backend, and model tier;
- keep edits inside the marker `scope_own` / `write_set`;
- use scoped local commits only when the marker says
  `commit_policy: scoped_commit`;
- write the required handoff under `.tmp/team-lead/`;
- do not push, merge main, deploy, clean dirty user worktrees, or rewrite
  history.

## Project Continuous Team Lead Adapter

This repository uses the global Codex Team Lead Mode plus the project-specific
continuous pipeline under `docs/team-lead/`.

When `docs/team-lead/WorkGraph.yaml` says
`run_policy: continuous_until_terminal`:

1. use a dedicated clean integration worktree;
2. code-affecting workers require isolated worktrees and scoped commits;
3. maintain at least four active implementation writers when four conflict-free
   ready tasks exist;
4. keep one verification/journey lane active;
5. rolling-integrate accepted commits immediately;
6. never use the human's dirty worktree as integration;
7. never send final while any WorkGraph task is non-terminal or worker is active;
8. resolve routine questions without the user;
9. player-visible design acceptance requires a connected human-style Journey,
   not only unit/DB tests;
10. local branches/worktrees/commits/trial cherry-picks/integration fast-forwards
    are authorized for the current epic; push, main merge, deploy, and destructive
    history operations are not.

Project architecture remains source-grounded JSON assets, NeedBus acquisition,
Rust state execution, fail-closed unknown rules, player-knowledge visibility, and
NPC-specific knowledge/persona behavior.

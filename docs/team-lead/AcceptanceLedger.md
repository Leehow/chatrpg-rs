# Acceptance Ledger — knowledge-runtime-p0

This ledger is the terminal-state companion to `docs/team-lead/WorkGraph.yaml`
for the continuous Team Lead pipeline.

Final reporting is blocked until every row is one of `Done`, `DeferredByUser`,
`BlockedByHuman`, or `OutOfScope`, or an approved stop limit is reached.

| ID | Status | Evidence |
|---|---|---|
| integrate-model-source-primitives | NotDone | Rolling integration of `de97b05f31e746e0a1206e3198951e75cc84a960` pending in clean integration worktree. |
| integrate-model-visibility-primitives | NotDone | Rolling integration of `d32ccac488dafc84d4bf233ffd8f969613c3643a` pending after source primitives. |
| model-block-scope-context-split | NotDone | Blocked on model visibility integration. |
| db-knowledge-repositories | NotDone | Ready implementation lane. |
| runtime-knowledge-boundary | NotDone | Ready implementation lane. |
| journey-prelude-character-session | NotDone | Ready implementation lane. |
| no-spoiler-adversarial-spec | NotDone | Ready verification lane. |
| npc-vertical-slice | InProgress | Existing `worker-tc-vs-npc-01` lane. |

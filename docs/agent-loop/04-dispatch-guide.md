# Dispatch Guide

## Standard command to user

```text
组长模式，按 <task-card-path> 执行。使用 autonomous loop。不要问常规实现问题；符合项目宪法时自己决定，测试失败自己修，只有 hard blocker 才问我。最终按 acceptance ledger 汇报 Done / Partial / Missing / Untested。
```

## Standard worker routing

1. Start a Claude Code worker through `claude --model opus` from repo root.
2. Paste the marker dispatch generated from `.agents/skills/chatrpg-autonomous-loop/references/dispatch-prompt-template.md`.
3. Require worker to confirm repo root, backend, model tier, marker fields, scope, and handoff path.
4. Let worker work without interruption unless blocked, drifting, unsafe, or scope-violating.
5. Read full handoff and review diff.
6. Dispatch revision/verification if needed.

## Lead should not

- accept terminal mid-output as evidence;
- patch code directly in Team Lead Mode;
- allow workers to edit ledgers unless scoped;
- accept missing tests without explicit Untested status;
- summarize to user before checking acceptance criteria.

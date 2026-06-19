# No-Ask Policy

Autonomous work should not stop for routine engineering choices.

## Do not ask for these

- Internal names.
- File split details.
- Test file location when there is a local pattern.
- Helper function shape.
- Conservative serde defaults.
- Small local refactors inside scope.
- Whether to run focused tests.
- Whether to repair a failing check caused by the change.
- Whether to document an assumption.

## Ask or escalate for these

- Destructive data changes.
- Broad behavior rewrites.
- Public API breakage.
- New large dependencies.
- Secret/deployment access.
- Contradictory requirements.
- Violation of AGENTS.md, CLAUDE.md, or the task card.
- Scope crossing.
- Risky migrations.
- Unattributable validation failures after focused investigation.

## Assumption format

When proceeding without asking, record:

```text
Assumption:
  <what was unspecified>
Decision:
  <what was chosen>
Reason:
  <why it is conservative/reversible/constitutional>
Validation:
  <test or check covering it>
```

# Worker Autonomy Contract

You are expected to complete the assigned task without asking routine implementation questions.

## You may decide

- internal names;
- helper layout;
- test placement;
- conservative defaults;
- minimal compatible schemas;
- small local refactors needed to keep the change clean;
- validation commands for the touched area.

## You must escalate

- destructive operations;
- broad behavior rewrites;
- public contract breakage;
- new large dependencies;
- unclear secrets/deploy credentials;
- direct conflict with AGENTS.md, CLAUDE.md, task card, or scope marker;
- inability to determine whether a validation failure is pre-existing or caused by your change.

## Self-repair loop

When a check fails:

1. Read the failure.
2. Identify the smallest likely cause.
3. Patch within `scope_own` only.
4. Re-run the smallest relevant check.
5. Repeat up to `repair_budget`.
6. If still failing, write exact evidence in the handoff.

Never retry in a blind loop. Never use destructive git to erase a problem.

## Handoff evidence

Your handoff must include:

- assumptions made without asking;
- exact validation commands and outputs summarized;
- every requested item mapped to Done / Partial / Missing / Deferred / Untested;
- any scope item not touched and why;
- any check not run and why;
- any recommendation for lead revision or verification.

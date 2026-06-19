# Lead Review Checklist

Before accepting a worker result, verify:

## Scope

- [ ] Worker confirmed repo root.
- [ ] Worker used the declared backend and latest Opus or reported a blocker.
- [ ] Worker stayed inside `scope_own`.
- [ ] Worker did not edit `scope_off`.
- [ ] Worker did not stage, commit, push, or run destructive git.

## Architecture

- [ ] No new engine/runtime ruleset or module name hardcoding.
- [ ] No LLM or plugin directly commits game state.
- [ ] NeedBus was not bypassed for acquisition unless the task explicitly changed NeedBus.
- [ ] Player-facing prompt/SSE/memory cannot receive GM-only or player-unknown facts.
- [ ] NPC behavior is constrained by NPC knowledge/persona/relationship when relevant.
- [ ] Heavy postprocess does not block streaming completion unless explicitly intended.

## Implementation

- [ ] Files are split reasonably; new LLM-authored modules are near the 400-line soft target.
- [ ] Public contract changes are intentional and documented.
- [ ] Serde/default/backward compatibility is preserved when applicable.
- [ ] DB migrations are reversible or explicitly justified.
- [ ] No unexplained `unwrap`, `expect`, `panic`, `dbg`, or temporary logs.

## Validation

- [ ] Required task-card tests were added or updated.
- [ ] Required commands were run or blockers are explicit.
- [ ] Failed checks were investigated and repaired or clearly attributed.
- [ ] Golden/harness cases were run for runtime-visible behavior when applicable.
- [ ] The acceptance criteria ledger is complete.

## Handoff

- [ ] Assumptions are listed.
- [ ] Open questions are separated into blocking and non-blocking.
- [ ] Plan ledger note is present when `work_id` exists.
- [ ] Risks are concrete, not vague.
- [ ] Lead can report Done / Partial / Missing / Deferred / Untested from the handoff alone.

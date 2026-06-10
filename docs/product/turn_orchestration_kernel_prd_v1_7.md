# Product Design: Turn Orchestration Kernel v1.7

## What it does

The Turn Orchestration Kernel prevents the GM runtime from misrouting a player turn. It decides whether the player is answering a gate, starting or continuing a frame, doing an object interaction, asking for scene guidance, or making a generic risky action.

## Why it matters

Previous versions fixed stale interaction cleanup but still allowed the wrong subsystem to claim a turn. A player could exit combat cleanly, then re-enter with a clear attack, yet only get a generic pending check. Players also saw churn when “continue attacking” was treated as an unparseable gate response instead of a combat action.

## User-facing effect

- If the player rolls, the open check resolves.
- If the player clearly starts a new fight, the old irrelevant gate is superseded and a new frame starts.
- If the player continues an active fight, the combat frame advances instead of opening a generic check.
- If the player interacts with an object inside combat, it becomes a child action of that frame.
- If the player is stuck or observing, the Actionable Situation Director runs.

## Success metrics

1. Enter -> exit -> re-enter starts a new frame.
2. Direction gates accept clear continue-pressure inputs.
3. No frame-worthy action falls through to generic `agent_plan` pending checks.
4. Object interactions inside frames inherit frame lifecycle ownership.
5. Stale gates never capture future turns unless the new input is a direct reply to that gate.

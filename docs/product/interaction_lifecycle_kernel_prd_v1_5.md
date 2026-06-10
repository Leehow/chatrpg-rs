# Product Design: Interaction Lifecycle Kernel v1.5

## What it is

The Interaction Lifecycle Kernel is the product reliability layer that prevents ChatRPG from trapping players in stale UI/state. If a combat frame ends, its pending reaction gates and checks end with it. If a player clearly abandons an old choice, the old gate is superseded and the new intent continues. If a bug leaves stale state in the database, the next turn self-heals it before serving the player.

## Why it exists

The same bug class recurred across several releases: abandoned checks remained open, direction gates wedged into reprompt loops, and a closed frame left a gate that hijacked the next combat re-entry. The high-level architecture is still sound; the missing product primitive was lifecycle ownership.

## User-facing promise

The player should never be blocked by a stale reaction window, old roll request, or gate from a scene that already ended.

Examples:

- `enter combat → ceasefire → re-enter combat` starts a fresh frame.
- `required reaction → surrender/ceasefire` supersedes the reaction gate.
- `pending roll → abandon action → new action` cannot resolve the old roll later.
- `frame closes by exit contract` closes child gates and pending checks.

## Success metrics

- No open gate without active owner.
- No open pending check with closed frame owner.
- No stale-generation gate is ever served to user input.
- Reconcile repairs are recorded and visible in debug streams/logs.
- Property tests can generate random attack/flee/hide/roll/advance-time sequences without violating invariants.

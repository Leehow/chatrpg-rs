# v1.4 World Time Spine

## Product intent

World Time Spine makes in-fiction time the single monotonic anchor for turns, frame events, clocks, scheduled consequences, memory validity, cache watermarks, and audit/debug replay. It is not a wall-clock timestamp and it is not just a HUD field.

## Core distinction

- `created_at`: real database/audit time.
- `world_tick`: authoritative in-fiction sequence time.
- `event_seq`: deterministic ordering for multiple events at the same tick.
- `display_time`: player/GM-facing calendar text.

The main timeline is monotonic. Retcons and flashbacks are represented as compensating events or flashback frames, never by mutating old events or moving the main tick backward.

## Runtime flow

```text
start_session
  → ensure world_time_state

turn begins
  → emit phase:world_time
  → record WorldEvent::PlayerAction at current world_tick
  → compile BP1/BP2/BP3 with world_time + world events since watermark

manual or tool time advance
  → advance world_tick / absolute_seconds
  → record WorldEvent::TimeAdvanced
  → trigger due scheduled events
  → emit time_advanced / scheduled_event_due

context compile completes
  → update context_watermark(session_id, world_tick, event_seq, cache_key)
```

## Cache policy

- BP1: time protocol only; no exact current time.
- BP2: broad scene epoch such as night / warehouse exterior / chapter one.
- BP3: exact `world_tick`, `display_time`, active clocks, due scheduled events, and incremental events since watermark.

This prevents precise time from invalidating stable prefix/pinned caches every turn.

## Tables

- `world_time_state`
- `world_events`
- `world_time_advances`
- `scheduled_events`
- `time_anchors`
- `context_watermarks`

## CLI

```bash
trpg time show --session <id>
trpg time advance --session <id> --minutes 5 --reason "players delayed under fire"
trpg time schedule --session <id> --in-minutes 3 --kind system_event --payload-json '{"label":"security arrives"}'
trpg time events --session <id> --since-tick 0
```

## API

- `GET /api/sessions/{session_id}/time`
- `POST /api/sessions/{session_id}/time/advance`
- `GET /api/sessions/{session_id}/events?since_tick=...&since_event_seq=...`
- `POST /api/sessions/{session_id}/scheduled-events`

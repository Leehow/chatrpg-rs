# Product Design: World Time Spine v1.4

## What it is

World Time Spine is the shared in-fiction clock for ChatRPG. It gives every meaningful event a time anchor, lets the world keep moving when players wait, and gives cache/memory systems a reliable incremental ordering.

## Why it matters

TRPG systems use different time scales: combat rounds, scene beats, exploration minutes, travel days, downtime weeks, and flashbacks. Without a single spine, every subsystem invents its own local clock. That makes memory, scheduled events, NPC patience, tactic cooldown, clocks, and cache invalidation drift apart.

## User value

Players feel the world moves consistently: reinforcements arrive after three minutes, wounds worsen after hours, cultists act during travel, and NPC patience decays during long debates. The GM does not need to remember scattered timers.

## Engineering value

`world_tick` and `event_seq` let ContextBuilder load only new dynamic world events since the previous context watermark. This keeps BP1/BP2 stable while BP3 remains exact.

## Non-goals

v1.4 does not implement a full fantasy calendar system, time zones, lunar cycles, or all ruleset-specific rest/recovery logic. It introduces the authoritative spine, storage, API/CLI, event anchoring, and context watermark path.

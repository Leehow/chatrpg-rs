# NPC Holder Identity Gate (P1)

**Status:** Design gate. No NPC-holder code shipped. This document is the
precondition for `KnowledgeHolderKind::Npc`, not its implementation.

**Context:** P0a routes explicit `FactRevealed` through the `reveal_fact` GM
tool; P0b makes `knowledge_edges` the projection source for revealed facts, but
only for `gm` and `player_party` holders. P1 would add NPC holders so an NPC can
*know*, *believe-false*, or *be-exposed-to* a fact. That cannot be built safely
until NPC actor identity is unified, because today the runtime resolves NPCs
through two divergent id spaces and one of them collides across scenes.

## Current actor / NPC id spaces

| Space | Where it lives | Stable per-NPC? | Suitable as a knowledge holder id? |
|---|---|---|---|
| **Module graph NPC id** | `ModuleGraph.npcs[*].id`; surfaced via `SceneNode.referenced_npc_ids` (`crates/trpg-model/src/lib.rs:1739`); used as `NpcPersona.actor_id` in `scene_npc_personas` (`crates/trpg-runtime/src/lib.rs:1920`), `persona_from_graph` (`crates/trpg-gm/src/tools/npc.rs:48-59`), and the GM opposed path (`crates/trpg-gm/src/tools/check.rs:237`) | Yes — exact module entity ids (e.g. `npc.lars`, `npc_butler`) | **Yes**, once every holder-producing path emits it |
| **Synthetic `npc.opposition` placeholder** | Single-slot opposition id reused by `current_check_npc_persona` (`crates/trpg-runtime/src/lib.rs:1891`), the combat hydrator (`crates/trpg-combat/src/lib.rs:714,1117`), `NpcDriveState`/`TacticPalette` seeds (`crates/trpg-combat/src/lib.rs:1297,1534`), and the combat policy fallback (`crates/trpg-combat/src/policy.rs:112`) | **No** — one placeholder stands in for whichever NPC is opposing this turn | **No** — non-persistent and collides across scenes (the "串台坑 §6③" the code comments call out) |
| **GM-tool-provided NPC id** | `ensure_npc_param` `npc_id` arg (`crates/trpg-gm/src/tools/npc.rs:13,30-35`) | Resolves to a module graph id or fails closed (`npc_not_found`) | Inherits module-graph stability when validated; never invents an id |
| **PC id** | `pc.current` literal (`crates/trpg-combat/src/lib.rs:376,710,1125`) | Single-PC placeholder | Out of scope here; relevant only as a parallel placeholder smell |

### The fact_id space is separate from the holder_id space

Knowledge `fact_id`s are module entity / node ids (e.g. `npc_butler`, `sc01`) —
the same string can name *the fact "the butler is the killer"* and *the NPC who
is the butler*. P0a/P0b deliberately keep `holder_id = ''` for the only two
holder kinds, so this overlap is currently harmless. NPC holders would make
`holder_id` meaningful for the first time, and it must be drawn from the **module
graph NPC id** space — never the placeholder.

## Why P1 must be gated

1. **The runtime is mid-migration between the two id spaces.** The newer
   Phase-3 path (`scene_npc_personas`, `crates/trpg-runtime/src/lib.rs:1899-1923`)
   already binds real per-NPC graph ids, and its comment explicitly frames this
   as fixing the placeholder collision. But the older check path
   (`current_check_npc_persona`, `:1855-1892`) and the entire combat stack still
   collapse every NPC into `npc.opposition`. Until those paths also emit stable
   per-NPC ids, a knowledge edge written at contest/combat time would attach to a
   placeholder that means a *different* NPC next scene — silently corrupting who
   knows what.

2. **The schema forbids NPC holders by construction today.** `knowledge_edges`
   has `constraint knowledge_edges_holder_kind_ck check (holder_kind in ('gm',
   'player_party'))` (`migrations/0032_knowledge_edges.sql:15`) and
   `holder_id text not null default ''`. Adding NPC holders therefore requires
   *both* a new migration widening the CHECK and a new `KnowledgeHolderKind::Npc`
   model variant — neither of which is safe before the id audit above resolves,
   and both of which are out of scope for this gate.

3. **No NPC Mind exists to populate the edges.** There is no production path that
   derives "NPC X learned fact F" from gameplay. Adding the holder kind without a
   trustworthy writer would create an empty, misleading capability.

## First safe P1 slice (when the gate opens)

Do **not** add `KnowledgeHolderKind::Npc` until a single stable actor id can be
joined across the module graph, the GM tools, the check path, and combat frames.
The smallest safe first slice, in order:

1. **Unify the contest/combat NPC id onto the module graph id.** Make
   `current_check_npc_persona` and the combat hydrator resolve and carry the real
   `referenced_npc_ids` entry (as `scene_npc_personas` already does) instead of
   the `npc.opposition` placeholder, or maintain an explicit placeholder→graph-id
   binding for the active turn. This is a runtime change, gated separately, and is
   the true prerequisite — not the knowledge schema.

2. **Introduce a pure `persistent_npc_holder_id` resolver** that accepts only
   stable module-graph ids and rejects placeholders/empties, so no edge can ever
   be persisted against `npc.opposition`. (See "Helper deferred" — it is not
   created yet because it would have no caller.)

3. **Only then** widen the migration CHECK + add `KnowledgeHolderKind::Npc`, and
   write NPC edges from a real disclosure event, not from keyword matching.

## Helper deferred (npc_identity.rs not created)

`crates/trpg-runtime/src/npc_identity.rs` was intentionally **not** created.
A pure `persistent_npc_holder_id` helper has no immediate consumer: the DB CHECK
rejects `npc` holders, no code writes NPC edges, and the placeholder path
(`npc.opposition`) is still live in the contest/combat stack. Shipping the helper
now would be dead code that falsely signals P1 has begun. It belongs to slice 2
above, alongside its first real caller and a focused test, once slice 1 unifies
the id spaces.

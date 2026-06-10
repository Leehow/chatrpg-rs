# LLM GM Onboarding and Self-Learning Design

Version: v0.5

## Decision

The parser no longer treats full rulebook/module extraction as the default startup path.

Default flow:

```text
PDF -> Markdown -> Book map -> GM onboarding bundle -> book locator -> first-session module prep -> play
```

Full chunk extraction is opt-in:

```bash
trpg parse-all --full-parse
# or
TRPG_PARSE_FULL_CHUNKS=true trpg parse-all
```

## Philosophy

A useful GM does not need to memorize every item, spell, monster, ability, table, and future chapter before the first scene. The GM needs operational familiarity:

```text
L0 Game Feel
L1 Play Loop
L2 Character Sheet Map
L3 Book Locator
L4 Learned Packets
```

Only L0-L3 and a small starter set of L4 belong in stable context at launch. Cold data remains indexed and is resolved on demand.

## New artifacts

### GmOnboardingBundle

Stored in `ruleset_onboarding_bundles` and embedded in `RuleBundle.gm_onboarding`.

Contains:

```text
game_identity
play_loop
ruleset_kernel
character_sheet_map
book_locator
starter_procedures
lookup_recipes
cold_data_locator
learned_packets_seed
```

### BookLocatorEntry

A locator means “the GM knows where this is,” not “the system fully parsed it.”

Typical categories:

```text
core_resolution
combat
character
items
spells
monsters
netrunning
gm_guidance
data
```

### ModulePrepPacket

Stored in `module_prep_packets` and embedded in `ModuleBundle.module_prep_packets`.

The default mode is `first_session`. It contains:

```text
module_overview
current_session_packet
required_rule_demands
source_refs
```

Later chapters stay cold-located until the session approaches them.

### LookupEvent / RulingLog / LearnedPacket

Runtime learning loop:

```text
player action
  -> rule demand detector
  -> known learned packet?
  -> else search book locator / source text
  -> source-backed or provisional ruling
  -> log lookup_event + ruling
  -> post-session audit
  -> learned_packet promotion
```

Learning stages:

```text
unseen -> located -> looked_up -> used_once -> stable -> memorized
```

## Context cache policy

```text
BP1 Prefix:
  engine protocol
  game identity
  play loop
  ruleset kernel
  book locator summary
  starter procedures

BP2 Pinned:
  module overview
  current session packet
  current scene/location/NPC/clue graph
  stable memory snapshots
  stable/memorized learned packets

BP3 Dynamic:
  current input
  dice result
  projected world state
  retrieved memory
  active lookup result
  provisional ruling
```

Rules:

```text
new lookup_event       -> dynamic only
new provisional ruling -> dynamic only
used_once packet       -> dynamic or low-priority pinned
stable packet          -> pinned
memorized packet       -> resident candidate, but only after review
```

This keeps `prefix_hash` and `pinned_hash` stable during ordinary turns.

## API additions

```text
POST /api/rules/lookup
POST /api/rulings
POST /api/learned-packets
GET  /api/learned-packets/{ruleset_id}
```

`/api/rules/lookup` currently records the lookup event and returns locator hits. `POST /api/rulings` records source-backed, provisional, table, or learned rulings. `POST /api/learned-packets` lets tests or a future audit worker promote a rule packet into runtime memory. A later version should stream source search progress over SSE and compile temporary `LookupResult` blocks automatically.

## Database additions

```text
ruleset_onboarding_bundles
book_locator_entries
module_prep_packets
lookup_events
rulings_log
learned_packets
```

## Default parser behavior

`trpg parse-all` now means “onboard and index,” not “fully parse every page.” It still writes:

```text
data/parsed/project.bundle.json
data/parsed/context_blocks.jsonl
data/parsed/material_index.jsonl
```

But most data-heavy sections are emitted as locators with `cache_zone = never_prompt` and `parse_policy = on_demand`.

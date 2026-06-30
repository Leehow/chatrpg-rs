# Narrative Director Layer — Realization Design

> RESEARCH + DESIGN doc. No production code. Grounds `设计4导演层补充.md` (the
> supplement) + `设计4补充.md §十一` (parent blueprint) against the CURRENT codebase
> (main @ 20684f6). Companion phased plan: `docs/superpowers/plans/director-layer-realization-plan.md`.

## 0. Thesis

The supplement's diagnosis is correct: the gap is **not more rule capability, it is a
narrative control plane**. The good news the audit confirms: **P5 already built the
hardest, most boundary-sensitive parts faithfully** — the story data model, the
proposal-only Director, the 12-term candidate selector, the reveal gate, snapshot
persistence, and a propose-not-commit plugin host. What is missing is **orchestration
across planning scales (Campaign/Scene), the post-adjudication placement of the
Director, event-sourced story persistence, a module narrative-anchor feed, and a
story-quality harness**. This design closes those gaps by *building on* P5, never
redoing it, and holds every step against the 12-rule constitution (设计4补充 §二).

The Director's core principle (supplement, final): **导演控制焦点/节奏/压力/铺垫/回收；
玩家控制选择；规则决定结果；世界决定合理反应；Narrator 表达。**

---

## 1. Current-state audit (consolidated, file:line)

Verdict legend: **real** = built & faithful · **partial** = exists but incomplete/divergent
· **absent** = not present. P5-honesty: where P5 is real, we KEEP it.

### 1.1 Data model — mostly REAL (matches §11 baseline; §五 additive fields missing)
| Element | Verdict | Anchor | Gap vs supplement |
|---|---|---|---|
| `StoryState` | real | `trpg-model/src/story.rs:23` | exact match §11.2; snapshot-persisted (`migrations/0040_story_state.sql`, `trpg-db/src/lib.rs:1582/1615`, fail-soft) |
| `StoryThread` | partial | `story.rs:40` | matches §11.3; **missing §五**: `title`, `origin:StoryThreadOrigin`, `module_anchor`, `payoff_candidates` |
| `StoryThreadStatus` (8 variants) | real | `story.rs:72` | exact, `#[default]=Dormant` |
| `StoryPromise` | partial | `story.rs:87` | matches §11.4; **missing §五**: `thread_id`, `setup`, `earliest_payoff_turn`, `expiry_policy:ExpiryPolicy`, `payoff_event_id`; `setup_event_id`→plural |
| `PayoffKind` / `PromiseStatus` | real | `story.rs:106/126` | variants unspecified by spec; fine |
| `CharacterArcState` | **near-absent** | `story.rs:137` | only `character_id` overlaps §五; **missing all 7**: `spotlight_debt`, `expressed_desires`, `unresolved_personal_hooks`, `important_relationship_ids`, `recent_choices`, `recurring_conflicts`, `emotional_direction`. Code has unrelated want/need/stage model |
| `StoryMemory` / `WorldMemory` named split | absent | — | concept implicit across tables (`world_facts` 0039, `memory_facts`, `knowledge_edges` 0032-0037 vs `story_state` 0040); no named abstraction |
| Story domain events | absent | `trpg-model/src/domain_event.rs:29` (existing kinds are turn/dice/check/knowledge/world events — **no story variants**) | snapshot-only; `StoryThreadOpened` is a code comment in `story_write.rs:71`, not an emitted event. Note: `DomainEvent` self-documents as a fail-soft additive ledger, **not yet the sole authority** (`domain_event.rs:11`) |

### 1.2 Director orchestration — multi-scale ABSENT; Beat/Situation REAL
| Element | Verdict | Anchor |
|---|---|---|
| `DirectorHorizon{Campaign,Scene,Beat,Situation}` enum | absent | grep 0 hits |
| unified `StoryDirectorService` + `DirectorRequest`/`StorySnapshot` | absent | 2 scattered entrypoints: `ActionableSituationDirector::prepare` (`trpg-director/src/lib.rs:100`) + pure `build_director_brief_packet` (`story/select.rs:68`) |
| `CampaignDirector`/`CampaignPlan` | absent | raw state only (`story.rs:23`) |
| `SceneDirector`/`ScenePlan` (typed output) | absent | fragments in `SceneFramePurpose` (`lib.rs:6554`), **hardcoded** vectors (`lib.rs:414`) |
| `BeatDirector`/`BeatPlan` | partial→real-in-disguise | `DirectorPlan` (`director_plan.rs:112`) ≈ BeatPlan, ~7/11 fields; missing `pressure_delta`,`tension_target` |
| `SituationDirector` (= current `ActionableSituationDirector`) | real (impl), **not repositioned**; mainline wiring unconfirmed | impl `lib.rs:28` — pressure/affordances/risks/clue-board/consequence-contract/clock-ticks/spotlight/novelty/guidance-ladder all real; runtime wrapper `prepare_actionable_situation` (`trpg-runtime/src/lib.rs:1748`). **Caveat:** codex sampling did NOT find the live CLI/API/GM turn path invoking it — the *capabilities exist*, but whether they reach the shipped product turn is unconfirmed |
| deterministic helpers (spotlight_debt, dormancy-transition, promise-maturity-advance, pacing-budget tracker, repeat-beat flag) | partial | spotlight_count `spotlight.rs:35`; dormancy *value* only; maturity *read* only `select.rs:311`; pacing *state* unused by selector; no budget decrementer |

### 1.3 Scoring + proposal-only boundary — REAL (boundary structurally guaranteed)
| Element | Verdict | Anchor |
|---|---|---|
| `DirectorPlan` output | real (2 renames, 2 extras) | `director_plan.rs:111` |
| `BeatKind` (12 variants) | real exact | `story.rs:171` |
| 12-term scoring formula | real shape; ~4 terms stubbed | `select.rs:260-293` (`genre_fit=0.5`, `spoiler_risk=0.0`, `causal_validity` binary, `character_relevance` binary) |
| pick-from-pool (never authors) | real, fail-closed | `select.rs:188-256` |
| `WorldQuery` escape hatch | real | `director_plan.rs:153`, `select.rs:92-99` |
| **proposal-only boundary** (Director never mutates) | **real — structurally guaranteed** | `trpg-director` has **no `trpg-db`/`sqlx` dep**; only writes are Kernel-owned, flag-OFF, validated, limited to rejection + Dormant→Introduced floor (`trpg-runtime/src/director_brief.rs:143-202`, `turn_loop.rs:2316`) |
| named proposal taxonomy (StoryThreadProposal/RevealCandidate/…) | absent | collapsed into single `DirectorPlan` |

### 1.4 Turn-order + context + Narrator — the CRITICAL gap
| Element | Verdict | Anchor |
|---|---|---|
| **Director runs AFTER rules adjudication** | **ABSENT / INVERTED** | Director built in head phase #9 `phase_context_assembly` (`turn_loop.rs:1415/1713`) **before** `AgentLoop` adjudication (`execute.rs:222`); sees no `check_results`/effects of the player action |
| Story Observer (post-commit story update) | partial | post-commit write seam at PresentationCommit (`turn_loop.rs:2306/2328`, `director_brief.rs:173-202`) but only rejection + status-floor; no Promise/arc/interest/pacing/advancement (P6.8b out of scope) |
| BP1/BP2/BP3 cache mapping | partial | 3 prefix-stability layers (`prompts.rs:42-87`), Director packet lands in BP3; but per-layer **content** ≠ supplement (no explicit ScenePlan/threads/promise-summary decomposition); no `cache_control` markers |
| NarrationPacket split {Facts, DirectorPlan, Performance} | partial | 2-stage Adjudication→Narration real (`packet.rs:64-193`); Facts+style present; **DirectorPlan delivered to GM prompt, not into the packet** |
| GM-agent role downgrade | absent (baseline) / partial (flag ON) | full 15-tool GM still adjudicates+narrates (`turn_loop.rs:1433`); tool-less Narrator (`run_narrator`, `turn_loop.rs:1105`) only an optional extra stage |

### 1.5 Scene + module anchors + harness + plugins
| Element | Verdict | Anchor |
|---|---|---|
| `ScenePlan` typed planner output | absent (fragments) | `SceneFramePurpose` hardcoded `lib.rs:414`; dramatic_question/stakes/threads live in separate types |
| Scene Director scale trigger | absent | rebuilt per-turn by keyword heuristic `infer_scene_purpose` (`lib.rs:808`); nothing keys off scene-start |
| module `NarrativeAnchor` extraction | absent | no `NarrativeAnchor`/`module_anchors`; module parse yields scene graph + `DirectorModuleConfig` *surface facts* (`lib.rs:4777`); "anchor" in parser = page-citation provenance, unrelated |
| `SceneNavigator`/`SceneTransitionCandidate` proposal boundary | partial | nav functions real & fail-closed (`scene_navigation/tiered.rs`), but navigator self-decides+commits (`tiered.rs:74-83`); no Director-proposes type |
| `forbidden_reveals` proactive (plan-driven) | absent | field plumbed (`packet.rs:76`) but only reactively filled from gate findings (`turn_loop.rs:2217`); normal path empty (`turn_loop.rs:1073`) |
| story-quality harness (8 checks) | absent | `trpg-harness` has 5 *mechanics* checkpoints (`lib.rs`); railroad only as Director scoring penalty (`select.rs:53/59`) |
| plugin framework | real; story-plugins absent; **boundary respected** | `trpg-gm/src/plugin/host.rs:13` propose-not-commit, 5 safety guards (`mod.rs:48`); no beat-weighting contribution kind (`plugin/types.rs:139`); core story-state un-pluginized ✔ |

**One-line reality:** P5 delivered the **Beat/Situation altitude, the data model, the
scorer, the reveal gate, and the proposal-only boundary** — all real. The Director layer
is **dormant in the shipped baseline** (3 flags default OFF) and **placed before
adjudication**. Campaign/Scene scales, event-sourced persistence, module anchors, the
quality harness, and the Narrator/GM re-split are the genuine greenfield.

---

## 2. Target architecture (how to FULLY realize the supplement)

### 2.1 One service, four horizons (supplement §十二, §11.1)
Introduce **one** `StoryDirectorService` in `trpg-director` (keeps the no-DB-dep
property → proposal-only stays structurally guaranteed). It takes:

```
DirectorRequest { horizon: DirectorHorizon, snapshot: StorySnapshot }
DirectorHorizon = Campaign | Scene | Beat | Situation
```

- **Situation** = today's `ActionableSituationDirector`, *renamed-by-reposition* (re-export alias kept for one release; no behavior change). It stays the bottom altitude.
- **Beat** = today's `build_director_brief_packet` → `DirectorPlan`. Already per-turn, deterministic, pure. Promote it under the service.
- **Scene** = NEW. Assembles a typed `ScenePlan` from `StoryThread`s + `DirectorModuleConfig` + scene context. Triggered at scene-start / major turning point, **not** per turn.
- **Campaign** = NEW. Produces `CampaignPlan` (active/dormant/emerging threads, thematic focus, long-horizon pressures, arc priorities). Triggered at session/chapter boundaries only.

The four scales **share** one data source (`StoryState`), one state store (snapshot →
event log, §2.4), one audit chain. Per supplement §十二 they need NOT be four agents or
four LLM calls; the first realization keeps Situation+Beat **deterministic/zero-LLM**
(as today) and makes Scene+Campaign the *only* LLM-backed scales, invoked rarely.

### 2.2 The turn-order relocation (G1 — the spine)
Adopt the supplement's 10-step order, with the Director moved to **step 5, after rules
adjudication (step 3) and world reaction (step 4)**:

```
1 Load Truth/Knowledge/Story State
2 Interpret Intent
3 Adjudicate Current Action     ← rules decide what REALLY happened
4 Simulate World Response        ← NPC/faction/clock reaction candidates
5 Director Plan (Beat)           ← pick focus FROM committed result + valid reactions
6 Compile Narration Context
7 Narrator Stream (describe only)
8 Verifier
9 Critical Commit
10 Story Observer                ← update threads/promises/arcs/interest/pacing
```

This is the inverse of today (Director in head phase #9, before `AgentLoop`). The
relocation is the **highest-leverage change** and the design's center of gravity.
Because it touches the turn loop, it is introduced **flag-gated and
behavior-preserving when OFF** (new flag, e.g. `TRPG_DIRECTOR_POST_ADJUDICATION`):
OFF = today's byte-identical path; ON = Director receives a new
`committed_player_result: MechanicalResult` + `world_candidates` and runs post-AgentLoop.
The existing `TRPG_DIRECTOR_PACKET` continues to gate whether the packet is built at all.

### 2.3 Proposal-only, preserved and extended (constitution rule 7, §九)
Keep the structural guarantee: the Director crate stays DB-free; it only emits typed
proposals. The supplement's granular taxonomy (`StoryThreadProposal`,
`ScenePlanProposal`, `SceneTransitionCandidate`, `RevealCandidate`, `NpcFocusProposal`,
`PressureProposal`) is realized as **variants/fields on the Director output**, committed
only by their owning runtimes after validation:
`KnowledgeRuntime`(reveal) · `NpcMindSystem`(attitude) · `RuleRuntime`(mechanics) ·
`WorldSimulation`(clocks/enemies) · `SceneNavigator`(transition). The
`SceneNavigator` boundary is corrected (G in §1.5): the Director emits a
`SceneTransitionCandidate`; the navigator validates+commits — today it self-decides.

### 2.4 Story persistence: additive event ledger now, source-of-truth promotion later (G5)
Today story state is a single overwritten jsonb row (`migrations/0040`). Two distinct
moves, deliberately separated:
- **Now (additive, no architect needed):** emit story `DomainEventKind` variants
  (`StoryThreadOpened/Advanced/Resolved/Dormant`, `StoryPromiseCreated/Reinforced/PaidOff`,
  `ScenePlanCreated`, `BeatPlanned`, `BeatObserved`) into the existing **fail-soft
  additive ledger** (`DomainEvent` already self-documents as such, `domain_event.rs:11`,
  exactly how `ClockAdvanced` is already write-through-logged in
  `trpg-runtime/src/lib.rs:1826`). This is what makes the Story Observer (step 10) real
  instead of the current rejection-only sliver. The snapshot row stays the read surface.
- **Later (ARCHITECT DECISION):** promoting the event log to the **sole authority** with
  the snapshot rebuilt as a pure projection (constitution rule 2-3) is a source-of-truth
  model change, *not* plain additive work, because `DomainEvent` is explicitly "not yet
  the sole authority" today. The design recommends this end-state but flags the promotion
  as an explicit architectural checkpoint (see §4 R2), not a silent refactor.

### 2.5 Memory split made explicit (G — §四)
Keep the two physical surfaces; add the **named abstraction** so layers consume the
right View (constitution rule 12): `WorldMemory` (facts/knowledge/relationships —
`world_facts`/`memory_facts`/`knowledge_edges`) vs `StoryMemory` (threads/promises/
arcs/pacing/interest — `StoryState`). The Director reads `StoryMemory` + a *view* of
`WorldMemory`, never raw tables.

### 2.6 Module narrative anchors (G6 — §十五-4)
Module parsing already produces a scene graph + `DirectorModuleConfig` surface facts. Add
an **additive** extraction pass that emits `NarrativeAnchor`s (potential threads, main
stakes, key character goals, reveal candidates, possible climax conditions, key
setup/payoff, expected scene function) — **director material, not a script**
(constitution rule 1, supplement §十五-4 explicit). Fed via
`DirectorRequest.module_anchors`. No `module_id` name-branching (rule 11).

### 2.7 Story-quality harness (G7 — §十五-5)
Add a `StoryQualityCheckpoint` family in `trpg-harness` for the 8 checks: scene-restate
loop, unpaid setups, repeated beat-kind, ignored PC background, mainline-railroad,
NPC-knowledge/motivation violation, premature reveal, choice→consequence. Reuses existing
signals: railroad penalty (`select.rs:53`), reveal gate (`select.rs:199`), `PacingState`,
Promise Ledger. This is the acceptance instrument for §十五-5 and feeds the LLM-judge
rubric (`harness/evaluation/product_rubric_v1.json`).

### 2.8 Plugins (G11 — §十四): weight beats, never own state
The plugin host (`host.rs:13`) already enforces propose-not-commit and keeps core story
state un-pluginized — boundary respected. Add a `BeatWeight`/`SceneConstraint`
contribution kind so the pluginizable set (Pacing/MysteryReveal/Foreshadowing/
PromisePayoff/CharacterSpotlight/GenreTone/FailForward/Downtime/HorrorPressure) can
*weight candidate beats / constrain scene plans / add prompt blocks* — **without** ever
holding `StoryThread`/`ScenePlan`/`BeatPlan`/`PromiseLedger`/`StoryEvent` (those stay
core, per §十四).

### 2.9 Scoring term enrichment (G8)
Replace the 4 stubbed terms with real signals as data becomes available:
`genre_fit` (from ruleset genre profile, not name-branch — rule 11), `spoiler_risk`
(from reveal gate + Promise maturity), `causal_validity` (from world-candidate causal
chain), `character_relevance` (from `CharacterArcState`). Keep the formula shape; this is
pure quality, low-risk, late-phase.

---

## 3. Constitution compliance check (设计4补充 §二, all 12)

1 source-backed assets not script → anchors are *material* not path ✔ ·
2 Event Log authority → §2.4 story events ✔ (current snapshot-only violates spirit; fixed) ·
3 Projection is a view → snapshot becomes projection of events ✔ ·
4 only Kernel commits → Director DB-free, owners commit ✔ ·
5 Rules = mechanics only → Director runs *after* and never sets check results ✔ ·
6 World = causal reactions → Director picks within world candidates, `WorldQuery` else ✔ ·
7 Director selects focus, never forces choice → proposal-only + anti-railroad penalty + `open_player_affordances` ✔ ·
8 Narrator presents decided content only → NarrationPacket fail-closed projection, GM downgrade ✔ ·
9 Policy filters, never creates facts → reveal gate `gm_truth∖player_known` ✔ ·
10 plugins typed proposals only → host propose-not-commit ✔ ·
11 no ruleset/module name-branching → scoring weights generic, anchors generic, genre via profile ✔ ·
12 each layer dedicated View not one GM prompt → BP1/2/3 + StoryMemory/WorldMemory views ✔ (partial today; §2.5 + §2.2 complete it).

No constitutional conflict. The supplement is a **refinement** of §十一, fully
realizable on the current architecture.

---

## 4. Risks, open questions, and what needs the architect

- **R1 (managed):** Turn-order relocation touches the hot path. Mitigation: flag-gated,
  OFF==byte-identical, ON behind live e2e. This is the only deep-surgery item. **Sequencing
  caveat (codex):** the relocation depends on contracts that do NOT exist yet
  (`DirectorRequest`, `StoryDirectorService`, a `MechanicalResult`/result-view — the code
  even notes no `MechanicalResult` exists, `director_plan.rs:22`). The plan therefore adds
  a **contracts-first P2a skeleton** phase before the live relocation, so "extend
  DirectorRequest" has something to extend.
- **R2 (ARCHITECT DECISION, not a stop):** event-sourcing story state has two tiers
  (§2.4). Tier-1 additive ledger needs no architect. Tier-2 promotion of the event log to
  *sole authority* (snapshot → pure projection) is a source-of-truth change and should be
  an explicit architectural checkpoint, because `DomainEvent` is "not yet the sole
  authority" today. **This is the one item flagged for the architect** — surfaced with
  evidence, not blocking the rest of the plan (the additive ledger unblocks the Observer).
- **R3 (quality, not blocker):** §五 vs §11 field divergence (Thread/Promise/Arc). The
  code follows §11 baseline; §五 is the richer target. Recommend adopting §五 fields
  **additively** (no removal) — no semantic conflict, so no architect needed.
- **R4 (verify before build):** the Situation director's capabilities are real in
  `trpg-director`, but codex sampling could not confirm the live product turn path invokes
  `prepare_actionable_situation`. P0 must **confirm the actual wiring** before the plan
  assumes the Situation altitude ships today — otherwise "reposition, don't rebuild" may
  mistakenly assume a live path that is dormant.
- **No hard blockers found.** The supplement does not contradict the blueprint or the
  current architecture; the only architect checkpoint is R2 (event source-of-truth). One
  judgment call (not a stop): whether Scene+Campaign scales should be LLM-backed from day
  one or stay deterministic until proven necessary — this design recommends
  **deterministic-first, LLM only for Scene/Campaign semantic picks** (supplement §十二
  "之后发现某些部分可以确定性执行，再逐步下沉"), reversing the usual instinct, to protect
  cost and avoid plan drift.

See the phased plan for ordered, additive, flag-gated execution.

# Plan — Narrative Director Layer Realization

> Phased implementation plan that realizes `设计4导演层补充.md` on the current codebase,
> building on what P5 already delivered. Companion design:
> `design/director-layer-realization-design.md`. Reconciles the supplement's §十五
> five-phase order with P5 reality (much of phase-1 structure already exists).
>
> **Discipline for every phase:** additive · behavior-preserving · flag-gated (OFF ==
> today's byte-identical path) · TDD for behavior code · proposal-only boundary
> preserved (`trpg-director` stays DB-free) · no `ruleset_id`/`module_id` name-branching
> · scoped commits. Each phase has objective / cut / contracts / acceptance.

## Reconciliation with supplement §十五

| Supplement phase | P5 status | This plan |
|---|---|---|
| §十五-1 split Director + add StoryState/Thread/Promise/ScenePlan/BeatPlan + story events | data model REAL, **events ABSENT**, ScenePlan absent | P1 (events+§五 fields), P3 (ScenePlan), reposition in P2 |
| §十五-2 wire Narrator (Facts/DirectorPlan/Performance split, GM downgrade) | NarrationPacket partial, GM not downgraded, flags OFF | P5 |
| §十五-3 wire memory + NPC (consume PlayerKnowledge/NpcMind/Relationship/Arc) | reveal gate real, Arc near-absent | P4 (Arc) + P6 |
| §十五-4 module narrative anchors | ABSENT | P7 |
| §十五-5 story-quality harness | ABSENT | P2b (minimal 3) + P8 (full 8) |
| (contracts the supplement assumes exist) DirectorRequest/Service/Horizon/ResultView | ABSENT | **P2a — contracts-first, prereq of the spine** |
| (not in §十五, but the critical audit finding) Director-after-adjudication | INVERTED | **P2 — pulled early as the spine** |

Key deviation from §十五 ordering: the audit's #1 finding (Director runs *before*
adjudication) is not in the supplement's phase list because the supplement assumed a
clean build. On the real codebase it is the load-bearing correction, so the turn-order
relocation is pulled forward as **the spine (P2)** — but it depends on contracts that do
not exist yet, so a **contracts-first P2a** precedes it (codex finding), and story
event-sourcing (P1) is **decoupled** to run *after* the spine, not before it.

Execution order (codex-revised): **P0 → P2a → P2 → P2b → {P1, P3, P4, P7 parallel} →
{P5, P6} → P8.**

---

## Phase P0 — Baseline pin, flag map & wiring confirmation (½–1 day)
**Objective:** lock the dormant-baseline contract AND confirm what actually ships before
touching anything.
**Cut:** document current flags (`TRPG_DIRECTOR_PACKET`, `TRPG_NARRATOR_SPLIT`,
`TRPG_STORY_WRITE_LOOP`) all default OFF; capture an OFF-path golden turn trace per
ruleset (CoC :54347, Cyberpunk :54346). **Confirm the live turn-path wiring of the
Situation director** (`prepare_actionable_situation`, `trpg-runtime/src/lib.rs:1748`) —
codex sampling could NOT find CLI/API/GM invoking it; resolve whether the Situation
altitude is live or dormant (R4). This determines whether P3+ "reposition" assumes a live
or dormant path.
**Contracts:** none changed.
**Acceptance:** golden traces recorded; arch_gates green; **a written wiring map** of
which Director paths the shipped turn actually executes; OFF == baseline established as
the regression oracle for every later phase.

## Phase P2a — Director contracts & service skeleton (additive, no behavior) — PREREQ of the spine
**Objective:** create the contracts the relocation needs but that **do not exist today**
(`DirectorRequest`, `StoryDirectorService`, `DirectorHorizon`, a `MechanicalResult`/
result-view — the code notes none exists, `director_plan.rs:22`). Codex flagged this as
the plan's biggest gap: P2 said "extend DirectorRequest" with nothing to extend.
**Cut:**
- Add `DirectorHorizon{Campaign,Scene,Beat,Situation}` + `DirectorRequest{horizon,
  snapshot}` + `StorySnapshot` + a `MechanicalResultView` (read-only projection of the
  committed check/effects) in `trpg-model`/`trpg-director` — **types only**, no wiring.
- Introduce `StoryDirectorService` as a thin façade over today's `build_director_brief_packet`
  (Beat) + `ActionableSituationDirector` (Situation); keep `trpg-director` DB-free.
- Re-export the old names so nothing breaks (reposition-by-alias).
**Contracts:** the request/result/horizon vocabulary now exists.
**Acceptance:** compiles, no behavior change (no caller switched yet), unit tests for the
new types' serde round-trip; arch_gates green. This phase is pure scaffolding → low risk.

## Phase P2 — Turn-order relocation: Director AFTER adjudication (THE SPINE)
**Objective:** move the Beat Director from head phase #9 to post-`AgentLoop`, feeding it
the committed mechanical result + world candidates (supplement step 5). **Depends on P2a
(contracts) and P0 (wiring map) — NOT on P1.**
**Cut:**
- New flag `TRPG_DIRECTOR_POST_ADJUDICATION` (default OFF).
- Populate `DirectorRequest.committed_player_result` (the P2a `MechanicalResultView`) +
  `world_candidates`; thread them from the `ResolutionCommit` boundary (`execute.rs:242`)
  into a post-adjudication Director call.
- When OFF: unchanged (Director stays in `phase_context_assembly`). When ON: Director
  runs at step 5, narration context (step 6) consumes its `DirectorPlan`.
**Contracts:** Director sees real outcomes; narration built from post-adjudication plan.
**Acceptance:** OFF == P0 golden byte-equality; **ON path asserted by a LIVE e2e turn**
through the changed path on both DBs — assert the `DirectorPlan` reflects the actual
check result (e.g. on a failed check the beat_kind/desired_change responds to failure,
fail-forward). This phase's ON behavior MUST be live-verified before Done.
**Risk:** R1 — highest. Flag-gated, golden-pinned. Only deep-surgery phase.

## Phase P2b — Minimal story-quality guardrails (pulled early, codex)
**Objective:** give every later phase an acceptance guardrail instead of waiting for P8.
**Cut:** add the 3 highest-leverage `StoryQualityCheckpoint`s to `trpg-harness`:
(1) post-adjudication Director evidence (the Beat reflects the committed result),
(2) rejected-thread no-railroad (a rejected thread is not re-pushed),
(3) premature-reveal fail-closed (`reveal = gm_truth ∖ player_known`).
**Contracts:** minimal harness gate exists.
**Acceptance:** each of the 3 has a passing + failing fixture; runs in CI; P8 later expands
to the full 8 checks.

## Phase P1 — Story events (additive ledger) + §五 field parity (additive)
> **Decoupled from the spine** (codex): event work is NOT a prerequisite of the turn-order
> fix; it runs after P2 lands. Two tiers, see design §2.4.
**Objective:** emit story events into the existing fail-soft ledger and bring
Thread/Promise/Arc to §五 parity — additively. **Does NOT promote the event log to
sole authority** (that is the architect checkpoint R2, deferred).
**Cut:**
- Add `DomainEventKind` story variants (`StoryThreadOpened/Advanced/Resolved/Dormant`,
  `StoryPromiseCreated/Reinforced/PaidOff`, `ScenePlanCreated`, `BeatPlanned`,
  `BeatObserved`) in `trpg-model/src/domain_event.rs`, **write-through to the additive
  ledger** exactly as `ClockAdvanced` already is (`trpg-runtime/src/lib.rs:1826`). Snapshot
  row stays the read surface.
- Additively add §五 fields: `StoryThread.{title,origin,module_anchor,payoff_candidates}`
  + `StoryThreadOrigin`; `StoryPromise.{thread_id,setup,earliest_payoff_turn,expiry_policy,payoff_event_id}`
  + `ExpiryPolicy`; rebuild `CharacterArcState` to §五 (`spotlight_debt`,
  `expressed_desires`, `unresolved_personal_hooks`, `important_relationship_ids`,
  `recent_choices`, `recurring_conflicts`, `emotional_direction`) keeping old fields
  `#[serde(default)]` for back-compat.
- **(deferred, architect checkpoint R2):** rebuilding the snapshot as a *pure projection*
  of the ledger (sole-authority promotion) is a separate, explicitly-approved step.
**Contracts:** additive story events; `serde(default)` everywhere (fail-closed).
**Acceptance:** old snapshots deserialize; story events appear in the ledger for a
recorded session; no behavior change with flags OFF; unit tests for each event apply.
**Risk:** additive only, old row kept; no data loss → not a human blocker. Source-of-truth
promotion explicitly excluded from this phase.

## Phase P3 — Scene scale: typed `ScenePlan` + scene-start trigger
**Objective:** promote the hardcoded `SceneFramePurpose` fragments into a real
`ScenePlan` assembled at scene-start / turning-point, under the service.
**Cut:**
- `ScenePlan` type (14 fields, §11.5/§六) assembled from `StoryThread`s +
  `DirectorModuleConfig` + scene context; replace hardcoded vectors (`lib.rs:414`) with
  derived values, flag-gated.
- Trigger on `WorldEventKind::SceneChanged` (already emitted, `tiered.rs:88`) instead of
  per-turn keyword inference; emit `ScenePlanCreated` event (from P1).
- Wire `forbidden_reveals` proactively from `ScenePlan` into `NarrationPacket` (close the
  reactive-only gap), keeping the reactive repair ladder as backstop.
**Contracts:** one scene-scoped plan per scene; forbidden_reveals plan-driven.
**Acceptance:** scene transition produces a `ScenePlan` with derived (non-constant)
fields; forbidden_reveals populated from plan; OFF unchanged; live e2e: cross a scene
boundary, assert plan emitted + a forbidden fact stays unrevealed.

## Phase P4 — Campaign scale + CharacterArc consumption
**Objective:** add the rarely-invoked Campaign scale + let the Director use arcs.
**Cut:**
- `CampaignPlan` (active/dormant/emerging threads, thematic_focus, long_horizon_pressures,
  character_arc_priorities); deterministic dormancy/emergence classifier (sink the
  helper the audit found missing); trigger only at session/chapter boundaries.
- `score_thread` consumes `CharacterArcState.spotlight_debt` + arc relevance →
  upgrade the stubbed `character_relevance` term (G8).
**Contracts:** Campaign runs rarely (cost guard); arc-aware scoring.
**Acceptance:** dormancy classifier unit-tested; arc-weighted selection changes pick in a
deterministic ON-path test; OFF unchanged.

## Phase P5 — Narrator/GM re-split (supplement §十五-2)
**Objective:** deliver `DirectorPlan` into the narration stage and downgrade the GM from
full-GM to Narrator + NPC-Performer + Clarification.
**Cut:**
- Add `DirectorPlan` into the narration context bundle (today it only reaches the GM
  adjudicator prompt). Formalize BP1/BP2/BP3 *content* mapping (policy / scene+story /
  beat) atop the existing prefix-stability layout.
- Promote `TRPG_NARRATOR_SPLIT` toward default-ON after parity; GM agent loses prose
  authorship when split ON (already tool-less narrator exists, `turn_loop.rs:1105`).
**Contracts:** Narrator describes only; mechanics never authored by narrator (rule 8).
**Acceptance:** with split ON, narrator output carries no mechanical invention (harness
`NpcSocial`/`Knowledge` checkpoints pass); OFF unchanged; live e2e on both DBs.

## Phase P6 — Story Observer (step 10) full implementation
**Objective:** replace the rejection-only post-commit sliver with a real Story Observer
updating threads/promises/arcs/interest/pacing from committed events.
**Cut:** consume P1 events at the PresentationCommit boundary; advance promise maturity,
thread momentum/status, player-interest signals, pacing history. Remove the "advancement
OutOfScope" limitation (`director_brief.rs:166`).
**Contracts:** story state evolves causally from committed turns.
**Acceptance:** a multi-turn live run shows thread `Introduced→Active→ReadyForPayoff` and
a promise reaching `Ripe`/`PaidOff`; deterministic observer unit tests.

## Phase P7 — Module Narrative Anchors (supplement §十五-4)
**Objective:** additive extraction pass emitting `NarrativeAnchor`s as director material.
**Cut:** `NarrativeAnchor` type; ingest/parser pass (additive to `ModuleReadout`/
`DirectorModuleConfig`) extracting potential threads / stakes / character goals / reveal
candidates / climax conditions / setup-payoff / scene function; feed
`DirectorRequest.module_anchors`. No name-branching (rule 11).
**Contracts:** anchors are material, never a fixed path (supplement §十五-4 explicit).
**Acceptance:** a parsed module yields anchors; Director can seed a `StoryThread` from an
anchor; re-parse is idempotent; live e2e: an anchor-seeded thread becomes selectable.

## Phase P8 — Story-Quality Harness (supplement §十五-5) + plugin beat-weighting
**Objective:** the acceptance instrument for the whole layer.
**Cut:**
- Expand the P2b minimal harness into the full `StoryQualityCheckpoint` family in
  `trpg-harness` for all 8 checks (scene-restate loop, unpaid setups, repeated beat-kind,
  ignored PC background, mainline-railroad, NPC-knowledge/motivation violation, premature
  reveal, choice→consequence). The 3 from P2b already exist; add the remaining 5.
- Add `BeatWeight`/`SceneConstraint` plugin contribution kind (`plugin/types.rs`); port
  one pluginizable example (e.g. `PacingPlugin`) to validate the seam — core story state
  stays un-pluginized (§十四).
**Contracts:** harness checks are structural (not just LLM-judge prose).
**Acceptance:** each of the 8 checks has a passing + a failing fixture; a `PacingPlugin`
re-weights a candidate beat without touching core state; runs in CI.

---

## Sequencing rationale & dependency graph

```
P0 ─> P2a (contracts) ─> P2 (spine) ─> P2b (min harness) ─┬─> P3 ─┬─> P5 ─┐
                                                          ├─> P4 ─┘       ├─> P8 (full 8-check harness)
                                                          ├─> P1 ─> P6 ───┤
                                                          └─> P7 ─────────┘
```
- **P2a (contracts) before the spine** (codex): `DirectorRequest`/`StoryDirectorService`/
  `MechanicalResultView` must exist before P2 can "extend" them.
- **P2 is the spine** and gates P3/P5/P6 (they assume post-adjudication placement).
- **P1 is decoupled** (codex): story events are NOT a prereq of the turn-order fix; they
  run after the spine and feed P6 (Observer). Sole-authority promotion is architect-gated.
- P2b (minimal harness) pulled early so each later phase has an acceptance guardrail.
- P4 (Campaign/arc), P3 (Scene), P7 (anchors) parallelizable once P2 lands.
- P8 (full harness) last — the acceptance instrument for the realized layer.

## What stays untouched (P5-honesty)
DO NOT redo: `StoryState`/`StoryThread`/`StoryPromise` core (story.rs), `DirectorPlan` +
`BeatKind` (director_plan.rs), the 12-term scoring formula shape (select.rs:260),
pick-from-pool + `WorldQuery`, the reveal gate, the proposal-only/no-DB structural
guarantee, the plugin host (host.rs:13), `ActionableSituationDirector`'s facilitation
capabilities (lib.rs). These are real and faithful; the plan extends them, never rebuilds.

## Global acceptance (definition of done for the realization)
1. All phases land additive + flag-gated; OFF == baseline byte-equality per phase.
2. With all flags ON, a full live chapter on each ruleset (CoC + Cyberpunk) runs through
   the post-adjudication Director, Scene/Campaign scales, Narrator split, Story Observer.
3. The §十五-5 story-quality harness passes its 8 checks on that chapter.
4. Constitution (§二, 12 rules) holds at every layer (see design §3).
5. No `ruleset_id`/`module_id` name-branching anywhere in the new code (rule 11).

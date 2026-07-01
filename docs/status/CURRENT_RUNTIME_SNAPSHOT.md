# Current Runtime Snapshot

Date: 2026-07-01
Status: authoritative implementation snapshot for agents and maintainers
Scope: documentation-only reconciliation; no runtime behavior change

This document records what the repository currently implements. It is the first
file to read when older design docs disagree with code, tests, or status notes.

## 0. Executive summary

`chatrpg-rs` is no longer a starter skeleton. It is a Rust-native, source-grounded
TRPG runtime prototype with a unified turn pipeline, layered ports, a Rust-owned
GM/referee loop, source-backed JSON assets, PostgreSQL state, Tantivy retrieval,
SSE/JSONL streaming, and a Rust harness.

The correct architecture label remains:

```text
Source-grounded JSON Asset Runtime
```

It is not a universal TRPG rule compiler. Rules, modules, scenes, actors,
objects, abilities, checks, effects, and knowledge are represented as
source-backed assets/facets/projections. Runtime execution late-binds those assets
to a limited set of Rust capabilities. When exact execution is not supported, the
system must degrade to source-backed guided ruling or blocked/provisional
mechanics rather than inventing rules.

## 1. Canonical runtime facts

### 1.1 Public product shape

Evidence:

- `README.md`
- `Cargo.toml`
- `AGENTS.md`
- `harness/README.md`

Current shape:

- Rust workspace version: `1.20.0` in `Cargo.toml`.
- Terminal/API first; no web UI is part of the current product surface.
- Core stack: Axum, Tokio, SQLx, PostgreSQL/JSONB, Tantivy, Serde, Reqwest,
  Tracing, Utoipa-compatible OpenAPI index.
- Public play path is CLI/API -> `execute_turn` -> `TurnEvent` stream.
- Python is allowed only for tests/fixtures, not product behavior.

### 1.2 Architecture constitution

Evidence:

- `docs/architecture/source-grounded-json-runtime.md`

Still current:

- JSON is an asset format, not a program.
- Runtime recognizes generic capabilities, not ruleset-specific engines.
- Execution tiering is required: `SourceOnly`, `GuidedRuling`,
  `PartialExecution`, `ExactExecution`, `VerifiedExecution`.
- Binding is late and source/confidence/visibility aware.
- Event log is the target source of truth; projections are views.
- Truth/player/presentation separation is mandatory.
- Runtime code must not branch on `ruleset_id.contains(...)` or
  `module_id.contains(...)`.

### 1.3 Control plane

Evidence:

- `crates/trpg-gm/src/execute.rs`
- `crates/trpg-gm/src/turn_plan.rs`
- `docs/architecture/layered-runtime-invariants.md` INV-2

Current fact:

- The actual turn control plane is `trpg_gm::execute::run_pipeline`.
- `execute_turn` is the CLI/API facade and returns `ReceiverStream<TurnEvent>`.
- `CANONICAL_TURN_PLAN` contains 15 phases:
  - 9 deterministic head phases;
  - 1 `AgentLoop` body;
  - 5 postprocess phases.
- `trpg-orchestrator::TurnOrchestrator` is a lifecycle/gate reducer, not the
  top-level turn control plane.

Do not move control-plane work into `trpg-orchestrator::TurnOrchestrator` merely
because of the name.

### 1.4 Streaming, critical tail, and heavy tail

Evidence:

- `crates/trpg-gm/src/execute.rs`
- `crates/trpg-api/src/turn_driver.rs`

Current fact:

- Streaming is `TurnEvent`-based and backpressured through an mpsc channel.
- The critical tail runs before `TurnComplete`:
  - verification after stream;
  - finalize/save turn;
  - scene navigation commit when applicable.
- Heavy postprocess runs after `TurnComplete` in a background task:
  - learning/memory/audit;
  - scene deep extract/frontier;
  - carryover debt.
- `pp_lifecycle=critical_done` is the high-water mark that allows the next turn
  to proceed while heavy work continues.
- Client cancellation before state mutation short-circuits the tail and must not
  persist a partial turn.

### 1.5 Context assembly

Evidence:

- `crates/trpg-runtime/src/lib.rs` `RuntimeEngine::prepare_turn_context`
- `docs/architecture/layered-runtime-invariants.md` INV-6

Current fact:

- `prepare_turn_context` is still the single context assembly entry point.
- It currently performs multiple responsibilities:
  - previous-turn high-water wait;
  - current-scene self-healing;
  - project bundle loading;
  - active NPC derivation and thin NPC profile materialization;
  - persisted/runtime context block loading;
  - NeedBus retrieval for rules, scene, parameters, and material;
  - memory, state-frame, world-time, object, ability, rule-binding,
    materialization, referee, combat-ledger, contest, rule-steward, and
    committed-world-fact projections.

Do not introduce five separate `ContextCompiler` implementations yet. The next
safe refactor is behavior-preserving function extraction inside/around
`prepare_turn_context`.

### 1.6 NeedBus and retrieval

Evidence:

- `crates/trpg-runtime/src/lib.rs`
- `crates/trpg-need`
- `crates/trpg-search`
- `README.md`

Current fact:

- Rule, scene, parameter, and material retrieval are routed through NeedBus or
  dedicated Need resolvers.
- Tantivy unified search indexes SQL/file/JSONL sources configured through
  database source configs rather than hardcoded Rust scope.
- Search hits do not directly enter prompts; selected hits are loaded as
  TTL-scoped `ContextBlock`s, preserving BP1/BP2/BP3 semantics.

### 1.7 Layered ports

Evidence:

- `crates/trpg-gm/src/ports.rs`
- `crates/trpg-gm/tests/ports_adapters.rs`
- `crates/trpg-gm/src/turn_loop.rs`
- `crates/trpg-gm/src/gate.rs`
- `crates/trpg-gm/src/npc_action.rs`

Current fact:

The layered port layer exists and is production-wired at key call sites:

- `NarratorPort` -> `GmLoopNarratorAdapter`;
- `RulesPort` -> `EngineRulesAdapter`;
- `WorldPort` -> `EngineWorldAdapter`;
- `DirectorPort` -> `DirectorAdapter`;
- `PolicyPort` -> `PresentationPolicyAdapter`;
- `KernelPort` -> `EngineKernelAdapter`.

`crates/trpg-gm/tests/ports_adapters.rs` pins both pure delegation equivalence
and source-grep guards proving the adapters have production callers. Older docs
that state Ports are not landed are historical and no longer describe main.

### 1.8 Adjudication/Narration split

Evidence:

- `crates/trpg-gm/src/packet.rs`
- `crates/trpg-gm/src/turn_loop.rs`
- `crates/trpg-gm/src/execute.rs`

Current fact:

- `AdjudicationPacket` exists as a structured adjudicator output projection.
- `NarrationPacket` exists as a player-safe, narrowed projection.
- `NarrationPacket` must not carry:
  - GM-only facts;
  - secret facts;
  - raw rule text;
  - tool JSON;
  - `adjudicator_prose`.
- Narrator split behavior is flag-controlled, but the packet model is real and
  should be treated as current architecture.

### 1.9 Policy and presentation gate

Evidence:

- `crates/trpg-gm/src/plugin/types.rs`
- `crates/trpg-gm/src/presentation_gate.rs`
- `crates/trpg-gm/src/ports.rs`
- `crates/trpg-gm/src/execute.rs`

Current fact:

- Policy is still cross-cutting rather than a simple sequential sixth phase.
- The presentation gate can block/repair unsafe narration after verification.
- `PolicyPort` is a thin pure adapter over `presentation_gate_decision`.
- Presentation commit/reveal commit happens after the final gate/repair state,
  not before.

### 1.10 Knowledge, facts, and visibility

Evidence:

- `migrations/0032_knowledge_edges.sql`
- `migrations/0039_world_facts.sql`
- `crates/trpg-runtime/src/knowledge_projection.rs`
- `crates/trpg-runtime/src/verifier_private_view.rs`
- `crates/trpg-gm/src/ports.rs` `KernelPort`

Current fact:

- `world_facts` exists as a first-class table separate from `memory_facts`.
- `knowledge_edges` exists, but the current migration constrains durable holders
  to `gm` and `player_party`.
- NPC-as-holder is intentionally gated until actor identity is explicit and
  stable.
- `KernelPort` exposes read-only projections plus the single reveal commit
  primitive.
- Do not persist durable NPC knowledge edges using transient placeholders such as
  `npc.opposition`.

### 1.11 NPC simulation and world reactions

Evidence:

- `crates/trpg-runtime/src/npc_profile.rs`
- `crates/trpg-runtime/src/npc_mind.rs`
- `crates/trpg-runtime/src/npc_behavior.rs`
- `crates/trpg-runtime/src/world`
- `crates/trpg-gm/src/ports.rs` `WorldPort`

Current fact:

- NPC profiles, relationships, mind views, behavior plans, and world reaction
  candidates exist.
- `WorldPort` returns both a lossy `WorldReactionSet` and the retained
  `NpcBehaviorPlan`s used for render fidelity.
- NPC action/speech projections are read-only folds; they must not acquire
  mutation side effects.

### 1.12 Mechanics and adjudication

Evidence:

- `crates/trpg-agent`
- `crates/trpg-combat`
- `crates/trpg-mechanics`
- `crates/trpg-contest`
- `crates/trpg-params`
- `docs/status/rule_steward_mechanics_v1_16_2.md`

Current fact:

- The runtime has real mechanics scaffolding: `CheckContract`, roll execution,
  combat frames, contest/opposition records, effect resolution packets,
  parameter impacts, and parameter facet execution.
- It is not yet a complete ruleset-specific combat/damage/resource engine.
- Exact values such as actor stats, weapon damage, target HP/SP, Cyberpunk RED
  range DV, armor ablation, D&D AC/save details, Sword World power-table rows,
  CoC SAN thresholds, and Triangle Chaos/Harm effects depend on source-backed
  facet/source-pack materialization.
- Missing source-backed parameters should block or degrade gracefully rather than
  being invented.

### 1.13 Combat dehardcoding

Evidence:

- `crates/trpg-combat/src/policy.rs`
- `docs/architecture/source-grounded-json-runtime.md`

Current fact:

- Combat policy is data-driven through `RuleKernel`, `ModuleConfig`, and generic
  defaults.
- Former ruleset/module name branches are replaced by policy tables and module
  config values.
- `npc.opposition` is a transient placeholder and must not be persisted as a
  durable knowledge holder.

### 1.14 Harness and validation

Evidence:

- `AGENTS.md`
- `docs/codex-team-lead/02-validation-rubric.md`
- `harness/README.md`
- `crates/trpg-harness`

Current fact:

- The harness is Rust-native.
- Validation is journey-driven. Unit/component tests are necessary but not
  sufficient for player-facing design claims.
- Valid product proof requires a public path:

```text
human-like input
-> public character/session path
-> real turn pipeline
-> runtime mechanism
-> committed state/projection
-> player-visible consequence
-> later-turn/reload consumption when persistence is claimed
```

- `trpg-harness playtest` is the current black-box multi-turn evaluator path.

## 2. Stale or superseded documentation claims

The following claims are known stale in older docs:

1. "Ports / NarrationPacket / independent Narrator have zero landing."
   - Superseded. Port traits and packet types exist; port production-call guards
     exist.
2. "Policy is only described as a non-sequential hook."
   - Still conceptually true, but current code also has `PolicyPort` as an
     adapter around presentation-gate logic.
3. "KnowledgeEdge supports NPC holders."
   - Not yet durable in DB; current migration allows `gm` and `player_party`.
4. "README starter wording reflects current maturity."
   - Partly stale. README remains operationally useful, but this snapshot is the
     better architecture map.
5. "Every implemented mechanics slice is exact."
   - False. Many mechanics are intentionally source-backed/provisional until
     richer facet packs are available.

## 3. Highest-risk hotspots

### 3.1 `RuntimeEngine::prepare_turn_context`

Current role: over-broad but still canonical context assembly entry.

Risk: future changes keep appending retrieval/projection/materialization logic to
one function.

Safe next step: behavior-preserving extraction into smaller helpers with no
cache, block, prompt, or event semantics changes.

### 3.2 `trpg_gm::execute::run_pipeline`

Current role: real control plane.

Risk: more lifecycle, policy, scene, and presentation logic accumulates in one
function.

Safe next step: small phase-handler extraction while preserving the 15-phase
plan, streaming order, critical/heavy split, and cancellation semantics.

### 3.3 Fact/knowledge/memory overlap

Current role: `memory_facts`, `world_facts`, and `knowledge_edges` coexist.

Risk: features may accidentally treat memory as truth or infer player/NPC
knowledge from transcript text.

Safe next step: use `world_facts` for truth candidates, `knowledge_edges` for
holder knowledge, and memory only as retrieval/continuity support.

### 3.4 Source-pack incompleteness

Current role: mechanics can run only as exactly as bound facets allow.

Risk: prompt/narration makes blocked/provisional mechanics look final.

Safe next step: strengthen source-backed facet extraction for one vertical slice
before expanding broad ruleset support.

## 4. Recommended near-term execution order

1. Establish a green baseline on main:
   - `cargo fmt --check`;
   - `cargo check --workspace`;
   - `cargo test --workspace` or a documented subset;
   - `bash scripts/arch_gates.sh`;
   - one Homecoming black-box playtest.
2. Freeze broad new runtime surfaces until the baseline is known.
3. Build a Cyberpunk RED + Homecoming first-chapter vertical slice:
   - drone/cable/hacking actions;
   - source-backed DV binding;
   - actor/object/weapon/target facets;
   - HP/SP/damage writeback where evidence exists;
   - no secret identity leak before reveal;
   - scene transition into Foxwell.
4. Extend durable KnowledgeEdge to NPC holders only after actor identity is
   explicit and tested.
5. Refactor `prepare_turn_context` into helpers without changing behavior.
6. Add profile-level env presets (`legacy`, `dev`, `product`, `strict_eval`) so
   harness/product runs are reproducible.
7. Keep docs, tests, and code synchronized by requiring doc updates when a layer,
   port, fact model, or validation profile changes.

## 5. Suggested validation commands

Documentation-only changes:

```bash
find docs/status docs/architecture -type f -name '*.md' -print
rg -n "CURRENT_RUNTIME_SNAPSHOT|DOCUMENTATION_DRIFT_RECONCILIATION|superseded" docs README.md AGENTS.md
```

Code-affecting baseline:

```bash
cargo fmt --check
cargo check --workspace
cargo test --workspace
bash scripts/arch_gates.sh
```

Product-gate baseline:

```bash
cargo build -p trpg-cli -p trpg-harness
cargo run -p trpg-harness -- playtest \
  --scenario harness/scenarios/homecoming_external_playtest_8turn.json \
  --bin ./target/debug/trpg \
  --cwd . \
  --evaluator none \
  --output text
```

## 6. Maintenance rule

When implementation changes any of these surfaces, update this snapshot in the
same PR:

- turn control plane;
- phase plan;
- port wiring;
- packet/narrator split;
- knowledge/fact schema;
- mechanics execution tiers;
- validation profile;
- product-facing runtime defaults.

# Current project state — v1.20 code audit

This document is a code-oriented status snapshot for the current `main` line. It exists because several older README/status sections are historical release notes and can read like current operating instructions.

## Baseline checked

- Workspace package version: `1.20.0`.
- The workspace is Rust-native and split into parser/ingest/search/API/CLI/runtime/GM/referee/mechanics/director/time/harness crates.
- PostgreSQL remains the durable store; migrations are wired through `0041_session_opening_delivered.sql`.
- The public product path is terminal/API first. There is no web UI in this repository.
- The Rust harness is the regression and product-evaluation path; Python is not a product runner.

## Current product shape

`chatrpg-rs` is best understood as a source-backed solo TRPG GM runtime, not a prompt-only LLM chatbot.

The Rust layer owns:

1. source ingestion, rule/module onboarding, locators, and parse artifacts;
2. search/retrieval and context block loading;
3. session state, memory, world time, current scene, domain events, and knowledge projections;
4. interaction gates, object/materialization/rule/ability/referee/contest/mechanics services;
5. canonical turn control and postprocess lifecycle;
6. product playtest and regression harnessing.

The LLM remains a narrator/structured-output helper. It should not be the only authority for mechanical state, factual visibility, persistence, or route control.

## Current default ingestion path

The current code default is `duotext`.

`duotext` maps to the pdftotext/poppler backend. It runs reading-order and `-layout` passes, then merges prose/table pages into one page-anchored Markdown file. This differs from older v0.8 docs that described an oxidize-first default.

Supported user-facing values:

- `duotext`
- `pdftotext`
- `oxidize`
- `auto`
- `mineru`

Use `oxidize` or `auto` explicitly when testing that older/Rust-native path. Use `--full-parse` only for the expensive full chunk extraction path.

## Current turn architecture

The current control-plane center is `trpg-gm::execute::run_pipeline`, driven by `execute_turn` and `CANONICAL_TURN_PLAN`.

The canonical plan has 15 phases:

1. RecordPlayerAction
2. RefreshLiveDerived
3. Reconcile
4. Gate
5. StimulusPass
6. OpposedPrepass
7. ModeInference
8. DebtLoad
9. ContextAssembly
10. AgentLoop
11. VerifyAfterStream
12. Finalize
13. AuditLearning
14. SceneNavigate
15. CarryoverDebt

Important semantics:

- awaiting-player-roll turns skip most heavy tail work and retain only verify/finalize;
- narration turns run full tail subject to module/obligation conditions;
- critical tail state should land before `TurnComplete`;
- heavy postprocess is isolated after completion;
- TurnTrace/domain-event work is the intended observability layer.

## Current durable runtime surface

The migration list shows the runtime has moved beyond early memory/search tables into:

- session current scene;
- mechanic dues;
- postprocess lifecycle;
- turn failure kind and turn traces;
- append-only domain events;
- knowledge edges with expanded holder support;
- NPC relationships and profiles;
- memory fact truth status;
- first-class world facts;
- story state;
- opening-delivered tracking.

This means feature work should be judged by persistence/projection behavior, not only by player-visible prose.

## Documentation cleanup in this PR

- `README.md` was rewritten as a current orientation document instead of a long historical release log.
- `.env.example` now aligns its PDF backend default with the code default: `TRPG_PDF_BACKEND=duotext`.
- `harness/README.md` now documents live/deterministic/replay modes, fixture recording, offline eval replay, and current checkpoint categories.
- This file records the current code audit and work map.

## Known documentation drift remaining

- `docs/status/known_issues.md` is still useful but historical; its later sections stop around v1.15.2.
- Some v0.8/v1.x product/design docs describe decisions that were later superseded by `duotext`, Rule Steward, v1.20 migrations, and layered-runtime work.
- `crates/trpg-api/src/lib.rs` still emits a static OpenAPI info version of `1.4.0`; the route list is more current than that version string.
- The older docs saying the build environment had no cargo are package-assembly notes, not a current project invariant.

## Priority work map

### P0 — remove ruleset/module hardcoding from runtime crates

Move remaining Cyberpunk/Homecoming-specific aliases, DVs, damage bands, combat defaults, and route choices out of engine/runtime crates and into parsed data:

- RuleKernel profiles;
- ModulePrepPacket scene/entity aliases;
- source-backed materialization facets;
- ruleset mechanical profiles;
- rule/module locator data.

Add a guard script/CI grep that permits ruleset/module names in tests/fixtures/data docs, but fails engine/runtime hardcoding.

### P0/P1 — semantic-first need and route classification

The project’s stated direction is “semantic/data driven,” but several paths still depend on lexical fallback or keyword-sensitive demand checks.

Needed work:

- introduce a SemanticNeedClassifier for rule/material/scene/visibility demand;
- disable lexical fallback except when semantic classification is missing/low-confidence;
- move module entity terms into parsed module/entity alias packs;
- keep fallback audit-only where possible.

### P1 — transport/session consistency

API and CLI should consume the same public product path.

Needed work:

- API turn path should load recent server-side session history by default;
- SSE disconnect policy should distinguish pre-state-mutation cancellation from post-state-mutation critical finalization;
- cancellation and postprocess lifecycle should be visible in trace/domain events.

### P1 — observability and source trace completion

Continue converting silent warnings/empty successes into auditable events.

Needed work:

- ensure every fail-closed turn has `TurnFailed`, `turns.failure_kind`, and `turn_traces`;
- propagate NeedOutcome source refs into context/source trace data;
- classify phase errors by AbortTurn / EmitWarningContinue / BackgroundWarnOnly;
- expose coverage/explain output enough for product debugging.

### P1/P2 — ruleset mechanical completeness

Parameter facets exist, but exact cross-ruleset table execution still depends on richer source packs and verifier coverage.

Needed work:

- Cyberpunk RED range DV, armor/SP, ablation, ammunition/weapon variants;
- D&D AC/save/spell DC/effect facets;
- Sword World power tables and resistance;
- CoC/BRP SAN/Major Wound;
- Triangle Harm/Chaos/anomaly effects;
- DB-level assertions for persisted mechanical outcomes.

### P2 — docs and API schema hygiene

Needed work:

- generate or update OpenAPI version/schema from workspace metadata;
- split historical release notes from “current operating manual” docs;
- add a regular doc-audit checklist against CLI flags, env defaults, API routes, migrations, and harness modes.

## Suggested validation for the next implementation PR

Docs-only:

```bash
find docs harness -type f -name '*.md' -print
```

Rust-impacting changes:

```bash
cargo fmt --check
cargo check --workspace
cargo test --workspace
bash scripts/arch_gates.sh
```

Gameplay acceptance:

```bash
cargo run -p trpg-harness -- suite --dir harness/cases --bin ./target/debug/trpg --cwd . --output jsonl
cargo run -p trpg-harness -- playtest --scenario harness/scenarios/homecoming_external_playtest_8turn.json --bin ./target/debug/trpg --cwd . --evaluator none --output text
```

For persistence claims, include DB-diff or replay-cassette evidence. A single good narration transcript is not enough.

# TRPG Eval Runner MVP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the first offline TRPG GM evaluation MVP that separates player simulation artifacts, GM output, auditors, and verdict aggregation.

**Architecture:** Add a pure `trpg-eval` crate for provider-free transcript parsing, response-contract auditing, deterministic findings, and JSON/Markdown reports. Wire `trpg-harness eval replay` as the runner-facing command, while keeping live GM execution and future LLM player simulation outside the pure audit core.

**Tech Stack:** Rust 2021 workspace, `serde`, `serde_json`, `clap`, existing `trpg-harness` binary, Markdown fixtures under `eval/fixtures/negative`.

## Global Constraints

- Codex may act as the constrained Player Simulator, but it must use only player-visible information and must emit a structured `PlayerDecision` plus `ResponseContract` before sending only `declared_action` to the GM.
- Codex must not use the same untracked role to play, inspect hidden truth, and judge GM quality; deterministic auditors and separate critic passes judge saved evidence.
- Player simulator artifacts must separate `PlayerDecision`, internal candidate actions, `declared_action`, and `ResponseContract`; only `declared_action` is sent to the GM in future live runs.
- Internal candidate actions are player-simulator deliberation trace only. They must never be presented by the GM as an explicit option menu.
- The first MVP is offline replay/audit only; it must not spawn providers, mutate DB state, or call the GM runtime.
- Negative fixtures must fail with evidence-backed findings, not vague prose.
- Existing J1-J4/Q4 metrics are regression signals only, not the final product-quality verdict.
- Language quality is never enough to pass a turn with unresolved intent, missing information, mechanical debt, or semantic no-op.

---

### Task 1: Pure Eval Data Model And Negative Replay Tests

**Files:**
- Create: `crates/trpg-eval/Cargo.toml`
- Create: `crates/trpg-eval/src/lib.rs`
- Create: `crates/trpg-eval/src/model.rs`
- Create: `crates/trpg-eval/src/parser.rs`
- Create: `crates/trpg-eval/tests/negative_fixtures.rs`
- Modify: `Cargo.toml`
- Create: `eval/fixtures/negative/cyber_50turn.md`
- Create: `eval/fixtures/negative/coc_10turn.md`

**Interfaces:**
- Produces: `parse_markdown_fixture(&str) -> Result<EvalFixture, EvalParseError>`
- Produces: `evaluate_fixture(&EvalFixture) -> EvalReport`
- Produces: `EvalReport { verdict, findings, score }`

- [ ] **Step 1: Write the failing tests**

```rust
use std::path::PathBuf;
use trpg_eval::{evaluate_fixture, parse_markdown_fixture, FindingCategory, Verdict};

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../eval/fixtures/negative").join(name)
}

#[test]
fn cyber_negative_fixture_fails_with_contract_and_debt_findings() {
    let text = std::fs::read_to_string(fixture_path("cyber_50turn.md")).unwrap();
    let fixture = parse_markdown_fixture(&text).unwrap();
    let report = evaluate_fixture(&fixture);
    assert_eq!(report.verdict, Verdict::Fail);
    assert!(report.has_category(FindingCategory::ResponseIntentMismatch));
    assert!(report.has_category(FindingCategory::PendingMechanicalDebt));
    assert!(report.has_category(FindingCategory::SemanticNoop));
    assert!(report.has_category(FindingCategory::PlayerScriptLoop));
}

#[test]
fn coc_negative_fixture_fails_success_without_information() {
    let text = std::fs::read_to_string(fixture_path("coc_10turn.md")).unwrap();
    let fixture = parse_markdown_fixture(&text).unwrap();
    let report = evaluate_fixture(&fixture);
    assert_eq!(report.verdict, Verdict::Fail);
    assert!(report.has_category(FindingCategory::SuccessWithoutInformation));
    assert!(report.has_category(FindingCategory::ResponseIntentMismatch));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p trpg-eval --test negative_fixtures`

Expected: FAIL because `trpg-eval` does not exist yet.

- [ ] **Step 3: Create the crate skeleton and fixture parser**

Implement `EvalFixture`, `EvalTurn`, `PlayerDecision`, `ResponseContract`, `GmResponse`, `TraceObservation`, and a conservative Markdown parser for an `eval-fixture` JSON code block.

- [ ] **Step 4: Run test to verify parser compiles but findings are still missing**

Run: `cargo test -p trpg-eval --test negative_fixtures`

Expected: FAIL because evaluator does not yet emit the required finding categories.

### Task 2: MVP Deterministic Detectors

**Files:**
- Create: `crates/trpg-eval/src/detectors.rs`
- Modify: `crates/trpg-eval/src/lib.rs`
- Modify: `crates/trpg-eval/src/model.rs`

**Interfaces:**
- Produces: `run_detectors(&EvalFixture) -> Vec<EvalFinding>`
- Consumes: parsed `ResponseContract`, `GmResponse`, and `TraceObservation`

- [ ] **Step 1: Add one failing assertion per required detector**

Add tests for:

```rust
FindingCategory::ResponseIntentMismatch
FindingCategory::SuccessWithoutInformation
FindingCategory::PendingMechanicalDebt
FindingCategory::SemanticNoop
FindingCategory::PlayerScriptLoop
FindingCategory::StateContinuityFail
FindingCategory::RulesTraceIncomplete
```

- [ ] **Step 2: Run test to verify RED**

Run: `cargo test -p trpg-eval --test negative_fixtures`

Expected: FAIL with missing categories.

- [ ] **Step 3: Implement minimal detector logic**

Implement fail-closed heuristics from fixture structure:

- `requested_information` present but no `facts_added`, no `facts_exposed_to_player`, and no acceptable resolution in GM text => `ResponseIntentMismatch`.
- `check_outcome: success` plus no exposed fact/state delta => `SuccessWithoutInformation`.
- GM/trace mentions pending/debt/waiting without `pending_id` => `PendingMechanicalDebt`.
- no facts, state delta, choices, consequence, pending, or clarification => `SemanticNoop`.
- three repeated selected actions or repeated functional intents without changed facts => `PlayerScriptLoop`.
- `continuity_violation` trace marker => `StateContinuityFail`.
- `roll` trace missing die/target/outcome/source/state delta fields => `RulesTraceIncomplete`.

- [ ] **Step 4: Run test to verify GREEN**

Run: `cargo test -p trpg-eval --test negative_fixtures`

Expected: PASS.

### Task 3: Reports And Harness CLI Wiring

**Files:**
- Create: `crates/trpg-eval/src/report.rs`
- Modify: `crates/trpg-harness/Cargo.toml`
- Modify: `crates/trpg-harness/src/main.rs`

**Interfaces:**
- Produces: `render_markdown_report(&EvalReport) -> String`
- Produces CLI: `cargo run -p trpg-harness -- eval replay --fixture eval/fixtures/negative/cyber_50turn.md --output json`

- [ ] **Step 1: Write failing CLI/output tests where practical**

Add a unit-level report test in `trpg-eval` asserting Markdown starts with `VERDICT: FAIL` and includes category, turns, expected, actual, root_layer, confidence, and evidence.

- [ ] **Step 2: Run report test to verify RED**

Run: `cargo test -p trpg-eval report`

Expected: FAIL because report rendering is absent.

- [ ] **Step 3: Implement report rendering and harness subcommand**

Add `Commands::Eval(EvalArgs)` with `EvalCommand::Replay { fixture, output }`. The command reads a fixture, calls `trpg_eval`, and writes JSON or Markdown to stdout. It returns nonzero on `FAIL`.

- [ ] **Step 4: Run CLI manually**

Run: `cargo run -p trpg-harness -- eval replay --fixture eval/fixtures/negative/cyber_50turn.md --output json`

Expected: command exits nonzero and prints JSON containing `"verdict":"fail"` and evidence-backed findings.

### Task 4: Evaluation Architecture Documentation

**Files:**
- Create: `docs/evaluation/trpg_eval_architecture.md`
- Modify: `docs/evaluation/live_game_eval_j1_j4.md`

**Interfaces:**
- Produces a handoff document explaining the new role split, fixture format, detector MVP, report schema, T0-T5 test levels, and why J1-J4 are now regression signals.

- [ ] **Step 1: Write the documentation**

Document:

- Codex-as-Player-Simulator / GM / Auditors / Aggregator role boundaries.
- Player-visible-only rule.
- Response Contract requirement.
- MVP detector list.
- Negative fixture acceptance.
- Future live-player-simulator path.

- [ ] **Step 2: Validate docs references**

Run: `rg -n "Codex =|Response Contract|negative fixture|J1-J4" docs/evaluation`

Expected: finds the new architecture text and the old doc demotion note.

### Task 5: Verification

**Files:**
- No new files.

**Interfaces:**
- Produces final validation evidence.

- [ ] **Step 1: Run focused tests**

Run:

```bash
cargo test -p trpg-eval
cargo test -p trpg-harness --test eval_negative_fixtures
```

- [ ] **Step 2: Run formatting/checks**

Run:

```bash
cargo fmt --check
cargo check -p trpg-eval -p trpg-harness
git diff --check
```

- [ ] **Step 3: Report remaining gaps**

State that live Player Simulator, LLM critics, baseline compare, and full raw 50-turn/10-turn fixture ingestion are next phases.

# GM Agent Loop Slice A Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the reusable Rust core that rejects final GM narration when mechanical claims are not backed by the turn ledger.

**Architecture:** Add a focused `gm_loop` module to `trpg-agent`. The module exposes a pure in-memory `TurnLedgerSnapshot`, typed `MechanicalClaim`s, and a deterministic `NarrationVerifier` that future Pi sidecar tools can call through `submit_final_narration`.

**Tech Stack:** Rust 2021, `trpg-agent`, existing `trpg-model` contracts, `serde`, `serde_json`, Cargo unit tests.

---

### Task 1: Verifier Contract Tests

**Files:**
- Create: `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-agent/src/gm_loop.rs`
- Modify: `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-agent/src/lib.rs`

- [ ] **Step 1: Write failing tests**

Add tests in `gm_loop.rs` under `#[cfg(test)]` for these behaviors:

```rust
#[test]
fn rejects_check_claim_without_contract_or_gate() {
    let ledger = TurnLedgerSnapshot::default();
    let submission = FinalNarrationSubmission {
        player_visible_text: "这里需要一次潜行检定。".into(),
        mechanical_claims: vec![MechanicalClaim::new(MechanicalClaimKind::Check, "需要潜行检定")],
        referenced_ledger_ids: vec![],
    };

    let result = NarrationVerifier::default().verify(&ledger, &submission);

    assert_eq!(result.accepted, false);
    assert_eq!(result.next_required_action, Some(VerifierNextAction::CallProposeCheck));
    assert!(result.findings.iter().any(|f| f.kind == VerifierFindingKind::MissingCheck));
}
```

Also test unexecuted check resolution, invented damage/resource effect, omitted visible roll, and accepted consistent narration.

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
cargo test -p trpg-agent gm_loop
```

Expected: compile failure or failing tests because `gm_loop` symbols do not exist yet.

### Task 2: Minimal Verifier Implementation

**Files:**
- Create: `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-agent/src/gm_loop.rs`
- Modify: `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-agent/src/lib.rs`

- [ ] **Step 1: Add public module export**

In `lib.rs`, add:

```rust
pub mod gm_loop;
pub use gm_loop::*;
```

- [ ] **Step 2: Add verifier types**

Implement these types in `gm_loop.rs`:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TurnLedgerSnapshot {
    pub check_contracts: Vec<CheckContract>,
    pub dice_rolls: Vec<DiceRollRecord>,
    pub check_results: Vec<CheckResultRecord>,
    pub effect_contracts: Vec<EffectContract>,
    pub parameter_impacts: Vec<ParameterImpact>,
    pub interaction_gates: Vec<InteractionGate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MechanicalClaimKind {
    Check,
    Roll,
    Damage,
    Resource,
    Condition,
    Object,
    Clock,
    State,
}
```

Also add `MechanicalClaim`, `FinalNarrationSubmission`, `NarrationVerifier`, `NarrationVerifierResult`, `VerifierFinding`, `VerifierFindingKind`, `VerifierSeverity`, and `VerifierNextAction`.

- [ ] **Step 3: Add deterministic checks**

Implement `NarrationVerifier::verify` so it:

- rejects `check` claims when there is no check contract, check result, or open player-roll gate;
- rejects resolved `check`/`roll` claims when a check exists but no `CheckResultRecord` or player-roll gate exists;
- rejects `damage`, `resource`, `condition`, `object`, `clock`, and `state` claims when there is no `ParameterImpact`, `EffectContract`, or committed `StatePatch`;
- rejects visible ledger results when narration omits all check labels, dice expressions, outcome text, impact paths, and effect ids;
- accepts narration with matching mechanical claims and visible ledger evidence.

- [ ] **Step 4: Run focused tests**

Run:

```bash
cargo test -p trpg-agent gm_loop
```

Expected: all `gm_loop` tests pass.

### Task 3: Verification

**Files:**
- Verify only.

- [ ] **Step 1: Run crate tests**

Run:

```bash
cargo test -p trpg-agent
```

Expected: all `trpg-agent` tests pass.

- [ ] **Step 2: Run compile check for dependent crates**

Run:

```bash
cargo check -p trpg-runtime -p trpg-cli -p trpg-api
```

Expected: compile succeeds, proving the new public exports do not break main consumers.

### Notes

This plan intentionally does not implement Pi RPC, HTTP tool host, database tables, or CLI/API flags. Those are later slices. This slice creates the pure verifier core that the `submit_final_narration` tool will call.

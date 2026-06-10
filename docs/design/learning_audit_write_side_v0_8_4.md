# Learning Audit Write Side v0.8.4

v0.8.4 closes the first part of the rule-learning write path without making unsafe automatic promotions.

## Problem

The read side is proven: when a learned packet exists, runtime search retrieves it and the GM changes behavior. A seeded packet with a distinctive marker such as `DV16 + KODA-7` changes a later ruling, which proves the learned-packet retrieval loop is real.

The missing part was the write side:

```text
lookup_event -> ruling_log -> learning candidate -> verified learned_packet
```

## New default behavior

After every successful `turn`, the background/CLI postprocess calls:

```rust
RuntimeEngine::audit_learning_for_turn(...)
```

The audit writes:

1. `rulings_log` when a turn appears to contain a mechanical or source-backed ruling.
2. `learning_candidates` when the turn has source evidence from runtime search.
3. `learning_audit_runs` for observability.

By default it **does not** write directly into `learned_packets`.

## Promotion gate

Candidates start as:

```text
verifier_status = pending_review
```

To promote:

```bash
trpg learn candidates --ruleset cyberpunk_red
trpg learn approve <candidate_id> --stage used_once --notes "source checked"
```

or API:

```http
POST /api/learning/candidates/{candidate_id}/approve
```

This creates a `learned_packet` and marks the candidate approved.

## Controlled experiments

For controlled local experiments only:

```env
TRPG_LEARNING_AUTO_PROMOTE=true
```

Auto-promotion still requires source evidence and an evidence score threshold. Provisional rulings without source support are not auto-promoted.

## Why candidates first?

A learned packet affects future rulings. Promoting bad DV/DC values or hallucinated mechanics would make future play worse. Therefore the system can collect candidates automatically, but promotion needs either a verifier or an explicit experiment flag.

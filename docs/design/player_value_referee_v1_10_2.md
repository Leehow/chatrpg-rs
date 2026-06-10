# v1.10.2 Player-Supplied Value Referee

## Problem

A player may volunteer mechanical numbers such as weapon damage, target DV, remaining HP, armor SP/AC, or a rolled damage total. Those numbers must not be adopted blindly. The GM should not be a rules lawyer, but the system should remain rules-first.

## Product rule

Player-supplied numbers are claims. They must be checked against the current ruleset, runtime state, and relevant tables or profiles. If the exact rule is not immediately available, the system compares the claim against nearby object/ability/NPC profiles and marks the decision provisional. If the claim is implausible, the GM warns the player and suggests a rules-consistent value. If the player/table insists, the value can be used as a table override with a balance warning.

## Runtime flow

```text
player input
  -> PlayerValueRefereeService detects numeric/mechanical claims
  -> writes player_value_claims
  -> verifies against known/provisional ruleset bands
  -> writes player_value_verifications
  -> optional table_override_agreements
  -> inserts BP3 referee context
```

## Design boundaries

This is not a complete damage engine. It prevents silent adoption of unverified numbers and prepares for the Referee Combat Slice by establishing an audit trail for player-supplied values.

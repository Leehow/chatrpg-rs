# Task <task-id>

## Design acceptance IDs

- <DA-ID>

## Observable outcome

Describe behavior, not files or structs.

## Dependencies

- integrated task/contract IDs

## Ownership

```yaml
read_set: []
write_set: []
hotspot_leases: []
generated_outputs: []
migration_slot: none
```

## Decision policy

`autonomous_within_contract`

## Implementation constraints

- Preserve project constitution.
- Do not modify central hotspots without an assigned lease.
- Use the smallest reversible design.

## Validation profile

- Targeted commands:
- Required Journey/Scenario IDs:
- Required evidence chain:

## Done when

The assigned acceptance behavior is demonstrated. A foundation object or unit test alone does not close a player-visible acceptance row.

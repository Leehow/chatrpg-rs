# Conflict and lease protocol

## Why path scope is not enough

Two workers can obey `scope_own` and still collide through:

- a shared facade or registry;
- a root Cargo manifest;
- migration numbering;
- generated artifacts;
- one shared test index;
- a public enum or trait whose shape changes downstream code.

The scheduler therefore builds a conflict graph from write sets and semantic hotspots.

## Lease rules

1. A hotspot has one active writer.
2. Read-only workers never need a write lease.
3. A task cannot enter `running` before all leases are granted.
4. Leases expire when the task enters review or is abandoned.
5. Contract changes invalidate dependent tasks that have not yet integrated.
6. Shared narrative files and ledgers are lead-owned, never multi-writer.

## Convert contention into extension points

When the same central file is repeatedly leased, treat it as an architecture defect. Prefer:

- facade-only `lib.rs` with domain modules;
- one repository/projector file per domain;
- plugin contributions instead of editing the turn loop;
- auto-discovered scenarios instead of a central test manifest;
- generated registries owned by the integration lane;
- contract traits that stabilize downstream work.

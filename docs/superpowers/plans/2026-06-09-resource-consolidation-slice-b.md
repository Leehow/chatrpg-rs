# Resource Consolidation Slice B Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `generic_parameter_states` the single source of truth for every resource CURRENT value (HP, SAN, all tracks), routed through ONE pair of read/write primitives in `trpg-db` shared by contest / mechanics / combat — without breaking HP damage, SAN loss, or effects.

**Architecture:** A new `trpg-db` module (`resource_current.rs`) owns `resource_seeds` / `load_resource_current` / `resource_cap` / `write_resource_current`, all keyed by kernel track id at gps path `resources.{id}.current` (`target_kind=actor`). `apply_effect_roll` (combat/effect path) stops reading/writing `actor_mechanical_states.hp_current`/`resources_json` and uses the primitives instead. `apply_outcome_resource_tracks` (contest path) and `read_track_value` (contest read) converge on the same primitives. `combat::create_frame` reads live current. The GM mechanical ledger projects live HP from gps. D&D's malformed kernel `resource_tracks` is cleaned by a reusable normalize + a data-layer override file.

**Tech Stack:** Rust, sqlx (Postgres), serde_json. Real-DB integration tests via `Db::connect(DATABASE_URL)` (skip-if-unset), same pattern as `crates/trpg-contest/tests/live_percentile.rs` and `crates/trpg-object/tests/live_ammo.rs`.

**Working dir:** `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula`

**NO GIT in this working copy.** Every task ends with a **Checkpoint** (build + run the relevant tests) instead of a commit. Do not run any `git` command.

**Test DBs (memory):** CoC / D&D / etc. live on **:54347** (`chatrpg-postgres-rulesets`). The `.env` `DATABASE_URL` points to **:54346** (Cyberpunk) — for any CoC/D&D real-DB test or run you MUST export `DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg`. macOS has no `timeout`; if you need a wall-clock cap use the Bash tool's own timeout. `psql` is not installed locally — use `docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg -c '...'`.

---

## Invariants (apply to every task)

1. **Zero per-ruleset hardcoding.** Primitives read kernel tracks × character sheet. D&D fixed by data override + a generic normalize. No `if ruleset ==`.
2. **Fail-closed.** No track / no derived value / unresolvable path → return `None`, write nothing, fabricate nothing. Never synthesize a fake `0`.
3. **SSOT.** Current values live ONLY in `generic_parameter_states`; the effect path no longer updates `actor_mechanical_states.hp_current`/`resources_json`.
4. **Test-first.** `apply_effect_roll` gets a regression net (locking CURRENT behavior, green) BEFORE the store is changed; assertions flip to gps AFTER.
5. **Files ≤400 lines.** New logic goes in the new `resource_current.rs` module and `trpg-model`; edits to the already-large `trpg-mechanics/src/lib.rs` stay minimal.

---

## File Structure

| File | Responsibility / change |
|---|---|
| `crates/trpg-model/src/lib.rs` | Receive pure `match_seed`; add `normalize_resource_tracks`, `resolve_resource_track_id`, `apply_armor_damage`, `wound_label` (+ their unit tests) |
| `crates/trpg-db/src/resource_current.rs` (new) | `impl Db`: `resource_seeds`, `load_resource_current`, `resource_cap`, `write_resource_current` |
| `crates/trpg-db/src/lib.rs` | `mod resource_current;`; `load_rule_kernel` → normalize + override-merge; `load_kernel_track_override` helper |
| `crates/trpg-mechanics/src/lib.rs` | `apply_effect_roll` HP+resources branches → db primitives; `apply_outcome_resource_tracks` → db primitives; remove `update_actor_hp`/`update_actor_resources` calls; ledger live-HP injection; drop moved pure fns |
| `crates/trpg-mechanics/tests/live_apply_effect_roll.rs` (new) | Real-DB regression + post-migration + cross-path consistency tests |
| `crates/trpg-contest/src/lib.rs` | `read_track_value` → `db.load_resource_current` (+ fix missing `target_kind`) |
| `crates/trpg-combat/src/lib.rs` | `create_frame` HP → `db.load_resource_current` live current |
| `data/parsed/rules/dnd5e.rule_kernel.override.json` (new) | Valid `hit_points` track for D&D |

---

## Task 1: Pure helpers in trpg-model (move match_seed + add 4 fns)

**Files:**
- Modify: `crates/trpg-model/src/lib.rs` (near `hp_resource_track_id` at line 1409)
- Test: same file, a new `#[cfg(test)] mod resource_helpers_tests`

**Context:** `match_seed` currently lives in `crates/trpg-mechanics/src/lib.rs:60-76` (private). It must move down to `trpg-model` so `trpg-db` (which only depends on `trpg-model`) can use it. `hp_resource_track_id` already lives in `trpg-model` at line 1409.

- [ ] **Step 1: Write failing tests** — add to the end of `crates/trpg-model/src/lib.rs`:

```rust
#[cfg(test)]
mod resource_helpers_tests {
    use super::*;
    use serde_json::json;

    fn coc_tracks() -> Vec<serde_json::Value> {
        vec![
            json!({"id":"sanity","kind":"track","max":99,"initial":0,"owner_kind":"actor"}),
            json!({"id":"hit_points","kind":"health","max":100,"initial":0,"owner_kind":"actor"}),
        ]
    }

    #[test]
    fn match_seed_uses_derived_value() {
        let seeds = match_seed(&json!({"sanity": 65}), &coc_tracks());
        assert_eq!(seeds.get("sanity"), Some(&(Some(65), Some(65))));
    }

    #[test]
    fn match_seed_falls_back_to_kernel_static() {
        let seeds = match_seed(&json!({}), &coc_tracks());
        assert_eq!(seeds.get("sanity"), Some(&(Some(0), Some(99))));
    }

    #[test]
    fn normalize_drops_sheet_field_defs_keeps_real_tracks() {
        let dirty = vec![
            json!({"field_id":"resources","field_type":"object","title":"Resources / Tracks"}),
            json!({"id":"hit_points","kind":"health","max":100}),
            json!({"name":"Stress"}),
        ];
        let clean = normalize_resource_tracks(&dirty);
        assert_eq!(clean.len(), 2);
        assert!(clean.iter().any(|t| t.get("id").and_then(|v| v.as_str()) == Some("hit_points")));
        assert!(clean.iter().any(|t| t.get("name").and_then(|v| v.as_str()) == Some("Stress")));
        assert!(!clean.iter().any(|t| t.get("field_id").is_some()));
    }

    #[test]
    fn resolve_track_id_maps_hp_alias_and_resource_path() {
        let k = RuleKernel { resource_tracks: coc_tracks(), ..Default::default() };
        assert_eq!(resolve_resource_track_id("hp.current", &k).as_deref(), Some("hit_points"));
        assert_eq!(resolve_resource_track_id("hp", &k).as_deref(), Some("hit_points"));
        assert_eq!(resolve_resource_track_id("resources.sanity.current", &k).as_deref(), Some("sanity"));
        assert_eq!(resolve_resource_track_id("resources.SANITY.current", &k).as_deref(), Some("sanity"));
        assert_eq!(resolve_resource_track_id("resources.unknown.current", &k), None);
    }

    #[test]
    fn armor_damage_math() {
        // no armor: 100 - 15 = 85, sp 0
        assert_eq!(apply_armor_damage(100, 15, None, ParameterOperation::Subtract), (85, 0));
        // 5 SP: 15 dmg -> 10 effective -> 90, sp 5
        assert_eq!(apply_armor_damage(100, 15, Some(5), ParameterOperation::Subtract), (90, 5));
        // SP exceeds dmg: floor at 0 effective -> no change, sp applied = amount
        assert_eq!(apply_armor_damage(100, 3, Some(5), ParameterOperation::Subtract), (100, 5));
        // Add (healing): +15 -> 115, sp ignored
        assert_eq!(apply_armor_damage(100, 15, Some(5), ParameterOperation::Add), (115, 0));
        // Set: target value (clamped >=0), sp ignored
        assert_eq!(apply_armor_damage(100, 30, Some(5), ParameterOperation::Set), (30, 0));
        // damage cannot drop below 0
        assert_eq!(apply_armor_damage(10, 50, None, ParameterOperation::Subtract), (0, 0));
    }

    #[test]
    fn wound_label_thresholds() {
        assert_eq!(wound_label(0, Some(20)), "defeated");
        assert_eq!(wound_label(-3, Some(20)), "defeated");
        assert_eq!(wound_label(8, Some(20)), "wounded");   // <= max/2
        assert_eq!(wound_label(15, Some(20)), "unhurt");
        assert_eq!(wound_label(15, None), "unhurt");        // no max -> not wounded
    }
}
```

- [ ] **Step 2: Run, verify it fails to compile** — Run: `cargo test -p trpg-model resource_helpers_tests 2>&1 | tail -20`. Expected: FAIL — `cannot find function match_seed` / `normalize_resource_tracks` / `resolve_resource_track_id` / `apply_armor_damage` / `wound_label`.

- [ ] **Step 3: Implement the helpers** — add to `crates/trpg-model/src/lib.rs` (right after `hp_resource_track_id`, ~line 1422). `ParameterOperation` and `RuleKernel` are already defined in this crate.

```rust
/// Pure (DB-free) seed matcher: for each kernel resource_track with a valid id,
/// look up the character's derived value in `resources` (flat `{id: number}`,
/// matched case-insensitively). When found, both current and max are that
/// derived value. When absent, fall back to the track's static kernel
/// `initial`/`max` (fail-closed; never fabricates). Keyed by kernel track id.
pub fn match_seed(resources: &serde_json::Value, kernel_tracks: &[serde_json::Value]) -> std::collections::HashMap<String, (Option<i32>, Option<i32>)> {
    let lookup = |id: &str| -> Option<i32> {
        let want = id.trim().to_ascii_lowercase();
        resources.as_object().and_then(|m| m.iter()
            .find(|(k, _)| k.trim().to_ascii_lowercase() == want)
            .and_then(|(_, v)| v.as_i64().map(|n| n as i32)))
    };
    let mut out = std::collections::HashMap::new();
    for t in kernel_tracks {
        let Some(id) = t.get("id").and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) else { continue };
        let kmax = t.get("max").and_then(|v| v.as_i64()).map(|n| n as i32);
        let kinit = t.get("initial").and_then(|v| v.as_i64()).map(|n| n as i32);
        let derived = lookup(&id);
        out.insert(id, (derived.or(kinit), derived.or(kmax)));  // (current, max)
    }
    out
}

/// Drop malformed resource_tracks (fail-closed): a real track must have a
/// non-empty string `id` OR `name`. Filters out character-sheet field-defs that
/// were mis-submitted as tracks (e.g. `{field_id, field_type, title}`). Generic
/// — no per-ruleset logic. Used at parse-write and kernel-load time.
pub fn normalize_resource_tracks(tracks: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let has = |t: &serde_json::Value, k: &str| t.get(k).and_then(|v| v.as_str()).map(|s| !s.trim().is_empty()).unwrap_or(false);
    tracks.iter().filter(|t| has(t, "id") || has(t, "name")).cloned().collect()
}

/// Map an effect-roll `parameter_path` to a kernel resource-track id (semantic,
/// no hardcoded alias table). `"hp"`/`"hp.current"` resolve via the kernel HP
/// track; `"resources.{X}.current"` (or a bare resource name) matches a track id
/// case-insensitively. Returns None when nothing matches (fail-closed).
pub fn resolve_resource_track_id(parameter_path: &str, kernel: &RuleKernel) -> Option<String> {
    let p = parameter_path.trim();
    let head = p.split('.').next().unwrap_or(p).to_ascii_lowercase();
    if head == "hp" {
        return hp_resource_track_id(&kernel.resource_tracks);
    }
    // candidate resource id: the segment after a "resources." prefix, else the head.
    let candidate = p.strip_prefix("resources.").map(|rest| rest.split('.').next().unwrap_or(rest)).unwrap_or(&head).to_ascii_lowercase();
    kernel.resource_tracks.iter()
        .filter_map(|t| t.get("id").and_then(|v| v.as_str()))
        .find(|id| id.trim().to_ascii_lowercase() == candidate)
        .map(|s| s.trim().to_string())
}

/// Pure damage math for an HP effect. Returns `(to_value, sp_applied)`.
/// `Subtract` (and any non-Add/Set op) is armor-mitigated: effective =
/// max(amount - sp, 0), result floored at 0. `Add` heals (sp ignored). `Set`
/// sets the value directly (clamped >= 0). Mirrors the prior inline logic.
pub fn apply_armor_damage(from: i32, amount: i32, armor: Option<i32>, op: ParameterOperation) -> (i32, i32) {
    let sp = armor.unwrap_or(0).max(0);
    match op {
        ParameterOperation::Add => ((from + amount).max(0), 0),
        ParameterOperation::Set => (amount.max(0), 0),
        _ => {
            let eff = (amount - sp).max(0);
            ((from - eff).max(0), sp)
        }
    }
}

/// Derived wound label from a current value vs a max (None when no row/cap).
pub fn wound_label(to: i32, max: Option<i32>) -> &'static str {
    if to <= 0 { return "defeated"; }
    match max { Some(m) if m > 0 && to <= m / 2 => "wounded", _ => "unhurt" }
}
```

- [ ] **Step 4: Run, verify pass** — Run: `cargo test -p trpg-model resource_helpers_tests 2>&1 | tail -20`. Expected: PASS (7 tests).

- [ ] **Step 5: Checkpoint** — Run: `cargo build -p trpg-model 2>&1 | tail -5`. Expected: clean build.

---

## Task 2: trpg-db resource-current primitives (new module)

**Files:**
- Create: `crates/trpg-db/src/resource_current.rs`
- Modify: `crates/trpg-db/src/lib.rs` (add `mod resource_current;` after the `use` block, ~line 7)
- Test: `crates/trpg-db/tests/live_resource_current.rs` (new)

- [ ] **Step 1: Declare the module** — in `crates/trpg-db/src/lib.rs`, immediately after `use uuid::Uuid;` (line 6), add:

```rust
mod resource_current;
```

- [ ] **Step 2: Create the primitives** — write `crates/trpg-db/src/resource_current.rs`:

```rust
//! Single source of truth for actor resource CURRENT values: generic_parameter_states,
//! path `resources.{track_id}.current`, target_kind=actor. One read/write path shared
//! by contest / mechanics / combat. Seed/cap come from the character's derived sheet
//! resources matched to kernel tracks (fail-closed), reusing trpg-model::match_seed.
use anyhow::Result;
use serde_json::{json, Value};
use sqlx::Row;
use trpg_model::*;
use uuid::Uuid;

use crate::Db;

fn jval_to_i32(v: &Value) -> Option<i32> {
    v.as_i64().map(|n| n as i32)
        .or_else(|| v.as_f64().map(|f| f as i32))
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<i32>().ok()))
}

impl Db {
    /// Per-track (current, max) seeds derived from the actor's character sheet
    /// (`runtime_actor_parameters.sheet_json.resources`), matched by kernel track
    /// id; falls back to kernel static initial/max. Keyed by kernel track id.
    pub async fn resource_seeds(&self, session_id: &str, actor_id: &str, kernel: &RuleKernel) -> std::collections::HashMap<String, (Option<i32>, Option<i32>)> {
        let resources: Value = sqlx::query_scalar::<_, Value>(
            "select coalesce(sheet_json->'resources','{}'::jsonb) from runtime_actor_parameters where session_id=$1 and actor_id=$2 limit 1")
            .bind(session_id).bind(actor_id).fetch_optional(&self.pool).await.ok().flatten().unwrap_or_else(|| json!({}));
        match_seed(&resources, &kernel.resource_tracks)
    }

    /// The live CURRENT value of `track_id` for this actor, from the single source
    /// of truth. No gps row -> character-DERIVED seed current -> kernel `initial`.
    /// Fail-closed None only when all three are absent.
    pub async fn load_resource_current(&self, session_id: &str, actor_id: &str, track_id: &str, kernel: &RuleKernel) -> Option<i32> {
        let path = format!("resources.{}.current", track_id);
        let row = sqlx::query("select value_json from generic_parameter_states where session_id=$1 and target_kind='actor' and target_id=$2 and parameter_path=$3")
            .bind(session_id).bind(actor_id).bind(&path)
            .fetch_optional(&self.pool).await.ok().flatten();
        if let Some(r) = row {
            if let Some(n) = jval_to_i32(&r.get::<Value, _>("value_json")) { return Some(n); }
        }
        let seeds = self.resource_seeds(session_id, actor_id, kernel).await;
        if let Some((Some(c), _)) = seeds.get(track_id) { return Some(*c); }
        kernel.resource_tracks.iter()
            .find(|t| t.get("id").and_then(|x| x.as_str()).map(|s| s.eq_ignore_ascii_case(track_id)).unwrap_or(false))
            .and_then(|t| t.get("initial")).and_then(jval_to_i32)
    }

    /// The actor's cap (max) for `track_id`: char-derived max -> kernel static max.
    pub async fn resource_cap(&self, session_id: &str, actor_id: &str, track_id: &str, kernel: &RuleKernel) -> Option<i32> {
        let seeds = self.resource_seeds(session_id, actor_id, kernel).await;
        if let Some((_, Some(m))) = seeds.get(track_id) { return Some(*m); }
        kernel.resource_tracks.iter()
            .find(|t| t.get("id").and_then(|x| x.as_str()).map(|s| s.eq_ignore_ascii_case(track_id)).unwrap_or(false))
            .and_then(|t| t.get("max")).and_then(jval_to_i32)
    }

    /// Write the CURRENT value of `track_id` to the single source of truth, capping
    /// by `cap` first. Returns the capped value actually stored.
    pub async fn write_resource_current(&self, session_id: &str, actor_id: &str, track_id: &str, value: i32, cap: Option<i32>, source_refs: &[SourceRef], visibility: Visibility, world_tick: i64) -> Result<i32> {
        let capped = match cap { Some(m) => value.min(m), None => value };
        let path = format!("resources.{}.current", track_id);
        sqlx::query(r#"
            insert into generic_parameter_states
              (id, state_id, session_id, target_kind, target_id, parameter_path, value_json, visibility, source_refs, provisional_reason, world_tick, updated_at)
            values ($1,$2,$3,'actor',$4,$5,$6,$7,$8,null,$9,now())
            on conflict (session_id, target_kind, target_id, parameter_path) do update set
              value_json = excluded.value_json, visibility = excluded.visibility,
              source_refs = excluded.source_refs, world_tick = excluded.world_tick, updated_at = now()
        "#)
        .bind(Uuid::new_v4()).bind(format!("generic_state_{}", Uuid::new_v4().simple())).bind(session_id).bind(actor_id).bind(&path)
        .bind(json!(capped)).bind(visibility.as_str()).bind(serde_json::to_value(source_refs)?).bind(world_tick)
        .execute(&self.pool).await?;
        Ok(capped)
    }
}
```

- [ ] **Step 3: Write the failing real-DB test** — create `crates/trpg-db/tests/live_resource_current.rs`:

```rust
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
//!      cargo test -p trpg-db --test live_resource_current -- --nocapture
use serde_json::json;
use trpg_db::Db;
use trpg_model::*;

const SESSION: &str = "sess_slice_b_rc_test";
const ACTOR: &str = "pc.slice_b_rc";

fn kernel() -> RuleKernel {
    RuleKernel {
        resource_tracks: vec![json!({"id":"sanity","kind":"track","initial":0,"max":99})],
        ..Default::default()
    }
}

#[tokio::test]
async fn write_then_load_round_trips_and_caps() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect: {e}"); return; } };
    // clean slate for this throwaway actor
    sqlx::query("delete from generic_parameter_states where session_id=$1 and target_id=$2").bind(SESSION).bind(ACTOR).execute(&db.pool).await.unwrap();

    // no row, no derived -> kernel initial (0)
    assert_eq!(db.load_resource_current(SESSION, ACTOR, "sanity", &kernel()).await, Some(0));

    // write 65, then load reads it back
    let stored = db.write_resource_current(SESSION, ACTOR, "sanity", 65, Some(99), &[], Visibility::GmOnly, 0).await.unwrap();
    assert_eq!(stored, 65);
    assert_eq!(db.load_resource_current(SESSION, ACTOR, "sanity", &kernel()).await, Some(65));

    // cap clamps to 99
    let capped = db.write_resource_current(SESSION, ACTOR, "sanity", 250, Some(99), &[], Visibility::GmOnly, 0).await.unwrap();
    assert_eq!(capped, 99);
    assert_eq!(db.load_resource_current(SESSION, ACTOR, "sanity", &kernel()).await, Some(99));

    sqlx::query("delete from generic_parameter_states where session_id=$1 and target_id=$2").bind(SESSION).bind(ACTOR).execute(&db.pool).await.unwrap();
}
```

- [ ] **Step 4: Run** — Run: `cargo build -p trpg-db 2>&1 | tail -20`. Expected: clean build. Then `DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg cargo test -p trpg-db --test live_resource_current -- --nocapture 2>&1 | tail -20`. Expected: PASS (not SKIP — :54347 is up).

- [ ] **Step 5: Checkpoint** — `cargo build -p trpg-db 2>&1 | tail -5`. Clean.

---

## Task 3: load_rule_kernel — normalize + override merge

**Files:**
- Modify: `crates/trpg-db/src/lib.rs` (`load_rule_kernel` at lines 689-698)
- Test: `crates/trpg-db/tests/live_kernel_override.rs` (new)

- [ ] **Step 1: Replace `load_rule_kernel`** — at `crates/trpg-db/src/lib.rs:689-698`, replace the function with:

```rust
pub async fn load_rule_kernel(&self, ruleset_id: &str) -> Result<Option<RuleKernel>> {
    let row = sqlx::query(r#"select content_json from rule_kernels where ruleset_id = $1 and active = true order by updated_at desc limit 1"#)
        .bind(ruleset_id)
        .fetch_optional(&self.pool)
        .await?;
    let Some(r) = row else { return Ok(None); };
    let mut kernel: RuleKernel = serde_json::from_value(r.get("content_json"))?;
    // Read-time cleanup of dirty resource_tracks (e.g. a mis-submitted character
    // sheet field-def) so stale DB kernels are sanitized without a re-parse,
    // then layer an optional data-only override (override track id wins).
    kernel.resource_tracks = trpg_model::normalize_resource_tracks(&kernel.resource_tracks);
    if let Some(over) = load_kernel_track_override(ruleset_id) {
        kernel.resource_tracks = merge_resource_tracks(kernel.resource_tracks, over);
    }
    Ok(Some(kernel))
}
```

- [ ] **Step 2: Add the override loader + merge** — add these free functions near the bottom of `crates/trpg-db/src/lib.rs` (module scope, not inside `impl Db`):

```rust
/// Data-only kernel patch: `{TRPG_DATA_DIR}/parsed/rules/{ruleset}.rule_kernel.override.json`
/// shaped `{"resource_tracks": [ ... ]}`. Mirrors the chargen override convention.
/// Returns the override tracks, or None when the file is absent/unreadable.
fn load_kernel_track_override(ruleset_id: &str) -> Option<Vec<serde_json::Value>> {
    let safe: String = ruleset_id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
    let dir = std::env::var("TRPG_DATA_DIR").unwrap_or_else(|_| "data".into());
    let p = std::path::Path::new(&dir).join("parsed").join("rules").join(format!("{safe}.rule_kernel.override.json"));
    let text = std::fs::read_to_string(&p).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("resource_tracks").and_then(|t| t.as_array()).map(|a| a.to_vec())
}

/// Merge override resource_tracks into base by track `id` (case-insensitive):
/// an override replaces a same-id base track; new ids are appended. Mirrors
/// chargen `merge_override`.
fn merge_resource_tracks(base: Vec<serde_json::Value>, overrides: Vec<serde_json::Value>) -> Vec<serde_json::Value> {
    let idof = |v: &serde_json::Value| v.get("id").and_then(|x| x.as_str()).unwrap_or("").trim().to_ascii_lowercase();
    let mut out = base;
    for o in overrides {
        let oid = idof(&o);
        if oid.is_empty() { continue; }
        if let Some(slot) = out.iter_mut().find(|b| idof(b) == oid) { *slot = o; } else { out.push(o); }
    }
    out
}
```

- [ ] **Step 3: Write the failing test** — create `crates/trpg-db/tests/live_kernel_override.rs`. NOTE: discover D&D's exact `ruleset_id` first — Run `docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg -tc "select distinct ruleset_id from rule_kernels;"` and use the D&D id (expected `dnd5e`). Substitute it for `DND` below if different:

```rust
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
//!      cargo test -p trpg-db --test live_kernel_override -- --nocapture
use trpg_db::Db;

const DND: &str = "dnd5e"; // verify via: select distinct ruleset_id from rule_kernels;

#[tokio::test]
async fn dnd_kernel_has_valid_hp_track_after_override() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect: {e}"); return; } };
    let kernel = match db.load_rule_kernel(DND).await.ok().flatten() { Some(k) => k, None => { eprintln!("SKIP: no {DND} kernel in this DB"); return; } };
    // normalize must have dropped the malformed sheet-field entry
    assert!(kernel.resource_tracks.iter().all(|t| t.get("field_id").is_none()), "sheet-field track survived normalize");
    // override must have supplied a health-kind hit_points track
    assert_eq!(trpg_model::hp_resource_track_id(&kernel.resource_tracks).as_deref(), Some("hit_points"), "D&D HP track not resolvable after override");
}
```

- [ ] **Step 4: Run, expect FAIL** — Run: `DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg cargo test -p trpg-db --test live_kernel_override -- --nocapture 2>&1 | tail -20`. Expected: FAIL on the `hp_resource_track_id` assert (override file not authored yet → normalize drops the dirty track → no HP track). The normalize assert should already pass.

- [ ] **Step 5: Checkpoint** — `cargo build -p trpg-db 2>&1 | tail -5`. Clean. (Test stays red until Task 4 authors the override file.)

---

## Task 4: D&D hit_points override data file

**Files:**
- Create: `data/parsed/rules/dnd5e.rule_kernel.override.json` (the `data/` symlink → the canonical v1.16.2 data dir; default `TRPG_DATA_DIR`)

**Note:** `initial` is intentionally OMITTED so that, absent a char-derived `hit_points` value, current resolves to None (fail-closed) rather than a fabricated 0. The real cap/start come from the character's derived `hit_points` (chargen). `max` is a loose ceiling.

- [ ] **Step 1: Author the file** — write `data/parsed/rules/dnd5e.rule_kernel.override.json`:

```json
{
  "_note": "Slice B data-layer fix: the D&D5e reader mis-submitted a character-sheet field-def as resource_tracks (no id/kind), so combat HP was None. This override supplies a valid hit_points track. 'initial' is omitted on purpose so HP stays fail-closed (None) when a character has no derived hit_points; the real value comes from the character's derived sheet resources.",
  "resource_tracks": [
    {
      "id": "hit_points",
      "kind": "health",
      "name": "Hit Points",
      "owner_kind": "actor",
      "max": 1000,
      "zero_means": "unconscious; make death saving throws",
      "on_outcome": [
        { "trigger": "always", "check_match": "damage|attack", "op": "subtract", "amount": "=damage" }
      ],
      "thresholds": [
        { "at": 0, "direction": "at_or_below", "consequence": "unconscious; begin death saving throws" }
      ]
    }
  ]
}
```

- [ ] **Step 2: Verify the D&D kernel test now passes** — Run: `DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg cargo test -p trpg-db --test live_kernel_override -- --nocapture 2>&1 | tail -20`. Expected: PASS. If it SKIPs (`no dnd5e kernel`), the D&D id differs — re-check `select distinct ruleset_id from rule_kernels;` and fix both the override filename and the test's `DND` const.

- [ ] **Step 3: Checkpoint** — confirm PASS above.

---

## Task 5: apply_effect_roll regression net (locks CURRENT behavior — BEFORE unification)

**Files:**
- Create: `crates/trpg-mechanics/tests/live_apply_effect_roll.rs`

**Context (read first):** `apply_effect_roll` is private. To exercise it, drive the public entrypoint `after_check_resolved` (mechanics:221) which calls `apply_effect_roll` when `is_effect_roll_check(contract)`. Read `crates/trpg-contest/tests/live_percentile.rs` for the `CheckContract` construction helper, and `crates/trpg-mechanics/src/lib.rs` `FacetDecision::hp` (line 37) + `apply_effect_roll` (368-565) to see what makes a contract route to `hp.current`. Read the `CheckContract` / `CheckResultRecord` / `DiceRollRecord` definitions in `crates/trpg-model/src/lib.rs` for exact fields. The service is `RefereeCombatService { db }` (mechanics:52) — construct via `RefereeCombatService { db: db.clone() }`.

**This task asserts the EXISTING behavior (HP written to `actor_mechanical_states.hp_current`) and must pass against unmodified code — it is the regression net.**

- [ ] **Step 1: Write the regression test** — create `crates/trpg-mechanics/tests/live_apply_effect_roll.rs`. Build a self-contained throwaway session: seed `runtime_actor_parameters` (so `ensure_actor_state` has a ruleset + derived HP) for a CoC NPC target with `sheet_json.resources.hit_points`, then run an effect (damage) roll and assert HP dropped. Seed/assert SQL is exact; model the contract/result construction on `live_percentile.rs` (adapt: `target_actor = Some(npc)`, an effect/damage check so `is_effect_roll_check` is true and the facet decision routes to `hp.current`).

```rust
//! Regression + unification net for apply_effect_roll (via after_check_resolved).
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
//!      cargo test -p trpg-mechanics --test live_apply_effect_roll -- --nocapture --test-threads=1
use serde_json::json;
use trpg_db::Db;
use trpg_model::*;
use trpg_mechanics::RefereeCombatService;

const SESSION: &str = "sess_slice_b_effect";
const TARGET: &str = "npc.slice_b_target";
const RULESET: &str = "call_of_cthulhu_7e"; // verify via select distinct ruleset_id from rule_kernels;

async fn seed_target(db: &Db, hp_derived: i32) {
    // throwaway actor params with a char-derived hit_points so ensure_actor_state /
    // load_resource_current find a real value (CoC kernel HP track id = hit_points).
    sqlx::query("delete from generic_parameter_states where session_id=$1 and target_id=$2").bind(SESSION).bind(TARGET).execute(&db.pool).await.unwrap();
    sqlx::query("delete from actor_mechanical_states where session_id=$1 and actor_id=$2").bind(SESSION).bind(TARGET).execute(&db.pool).await.unwrap();
    sqlx::query(r#"insert into runtime_actor_parameters
        (id, actor_param_id, session_id, actor_id, actor_kind, ruleset_id, source_kind, template_id, display_name, sheet_json, mechanical_profile, status_json, visibility, created_at_tick, updated_at_tick)
        values (gen_random_uuid(), $1, $2, $3, 'npc', $4, 'test_seed', null, 'Slice B Target', $5, '{}'::jsonb, '{}'::jsonb, 'gm_only', 0, 0)
        on conflict (session_id, actor_id) do update set sheet_json = excluded.sheet_json, ruleset_id = excluded.ruleset_id"#)
        .bind(format!("ap_{}", TARGET)).bind(SESSION).bind(TARGET).bind(RULESET)
        .bind(json!({"resources": {"hit_points": hp_derived}}))
        .execute(&db.pool).await.unwrap();
}

// Construct a damage effect-roll contract+result that routes to hp.current on TARGET.
// MODEL THIS ON crates/trpg-contest/tests/live_percentile.rs's contract() helper.
// Key fields: target_actor = Some(ActorRef{ actor_id: TARGET, kind: Npc, .. });
// the check must be an effect/damage roll (is_effect_roll_check == true);
// result.roll.result = json!({"total": <dmg>}); outcome marks success.
// fn damage_contract(total: i32) -> (CheckContract, CheckResultRecord) { ... }

#[tokio::test]
async fn hp_damage_decrements_current() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect: {e}"); return; } };
    if db.load_rule_kernel(RULESET).await.ok().flatten().is_none() { eprintln!("SKIP: no {RULESET} kernel"); return; }
    seed_target(&db, 12).await;
    let svc = RefereeCombatService { db: db.clone() };

    let (contract, mut result) = damage_contract(5);
    svc.after_check_resolved(&contract, &mut result).await.expect("effect roll");

    // CURRENT behavior (pre-unification): HP current = 12 - 5 = 7, read from the
    // single source of truth via the db primitive (works pre AND post migration).
    let kernel = db.load_rule_kernel(RULESET).await.unwrap().unwrap();
    let hp_id = trpg_model::hp_resource_track_id(&kernel.resource_tracks).expect("coc hp track");
    let live = db.load_resource_current(SESSION, TARGET, &hp_id, &kernel).await;
    assert_eq!(live, Some(7), "HP current should be 12 - 5 = 7");
}
```

> **Design note for the implementer:** assert through `db.load_resource_current` (the SSOT reader), NOT by selecting `actor_mechanical_states.hp_current` directly. Pre-unification, `apply_effect_roll` writes ams.hp_current AND the contest path may not have a gps row — so this assertion would currently FAIL (gps empty → falls back to derived seed 12, not 7). That is expected and correct: it makes this test the RED test that Task 6 turns GREEN by routing the write to gps. If you prefer a strict "lock current behavior first" net, add a *second* temporary assertion reading `actor_mechanical_states.hp_current == 7` and delete it in Task 6. Keep at least the gps assertion as the durable net.

- [ ] **Step 2: Run** — Run: `DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg cargo test -p trpg-mechanics --test live_apply_effect_roll -- --nocapture --test-threads=1 2>&1 | tail -30`. Expected: the gps assertion FAILS (HP read = Some(12), the derived seed, because the pre-unification write went to ams.hp_current, not gps). This is the RED that proves the divergence. Capture the output.

- [ ] **Step 3: Checkpoint** — `cargo build -p trpg-mechanics 2>&1 | tail -5`. Clean (test compiles, asserts red).

---

## Task 6: Unify apply_effect_roll onto the db primitives (turns Task 5 green)

**Files:**
- Modify: `crates/trpg-mechanics/src/lib.rs` — `apply_effect_roll` HP branch (385-462) and resources branch (493-516)

- [ ] **Step 1: Rewrite the HP branch** — replace lines 385-462 (the `if decision.parameter_path == "hp.current" { ... }` block, both the `Some(from_hp)` and `else` arms) with a version that reads/writes via the db primitives. Keep the damage-packet / impact / blocked-arm structure; only the read of `from_hp`, the math, the write, and the `sp` source change:

```rust
                let hp_id = trpg_model::resolve_resource_track_id("hp.current", &/*kernel*/ self.db.load_rule_kernel(&contract.ruleset_id).await.ok().flatten().unwrap_or_default());
                if decision.parameter_path == "hp.current" {
                    let kernel = self.db.load_rule_kernel(&contract.ruleset_id).await.ok().flatten();
                    let hp_id = kernel.as_ref().and_then(|k| trpg_model::hp_resource_track_id(&k.resource_tracks));
                    let from_hp = match (kernel.as_ref(), hp_id.as_ref()) {
                        (Some(k), Some(id)) => self.db.load_resource_current(&contract.session_id, &decision.target_id, id, k).await,
                        _ => state.hp_current.or(state.hp_max),
                    };
                    if let (Some(from_hp), Some(k), Some(id)) = (from_hp, kernel.as_ref(), hp_id.as_ref()) {
                        let sp = state.armor_current;
                        let (to_hp, sp_applied) = trpg_model::apply_armor_damage(from_hp, amount, sp, decision.operation);
                        let cap = self.db.resource_cap(&contract.session_id, &decision.target_id, id, k).await;
                        self.db.write_resource_current(&contract.session_id, &decision.target_id, id, to_hp, cap, &merge_source_refs(&contract.source_refs, &decision.source_refs), Visibility::GmOnly, contract.world_tick_hint()).await?;
                        let delta = to_hp - from_hp;
                        if to_hp <= 0 && actor_kind != ActorKind::PlayerCharacter {
                            self.close_active_frame_for_defeated_target(&contract.session_id, &decision.target_id, contract.world_tick_hint()).await.ok();
                        }
                        // ... build `impact`, push StatePatch::ActorHpDelta, build+insert DamagePacket
                        //     EXACTLY as before (lines 404-444), using these `from_hp`, `to_hp`,
                        //     `delta`, `sp_applied`, and `state.armor_current.is_some()` for the
                        //     armor_interaction policy string.
                    } else {
                        // ... unchanged blocked-arm (lines 446-461): record the damage roll but
                        //     do not decrement; validator_status "blocked_missing_source_backed_hp".
                    }
                }
```

> **Implementer:** preserve the `ParameterImpact`, `StatePatch::ActorHpDelta`, and `DamagePacket` construction verbatim from the original (lines 404-444); only the value sources change (`from_hp`/`to_hp`/`delta`/`sp_applied` now come from the db primitive + `apply_armor_damage`). Remove the call to `self.update_actor_hp(...)` (it is replaced by `write_resource_current`). The `else` (blocked) arm is unchanged. The `let hp_id = ...unwrap_or_default()` line at the very top of this snippet is a stray — DO NOT include it; the real `kernel`/`hp_id` are loaded inside the `if`.

- [ ] **Step 2: Rewrite the resources branch** — replace lines 493-516 (the final `else` arm) so the non-HP actor resource read/write also uses the SSOT, keyed by a resolved kernel track id:

```rust
                } else {
                    let kernel = self.db.load_rule_kernel(&contract.ruleset_id).await.ok().flatten();
                    let track_id = kernel.as_ref().and_then(|k| trpg_model::resolve_resource_track_id(&decision.parameter_path, k));
                    if let (Some(k), Some(id)) = (kernel.as_ref(), track_id.as_ref()) {
                        let before = self.db.load_resource_current(&contract.session_id, &decision.target_id, id, k).await.unwrap_or(0);
                        let after = apply_i32_operation(before, amount, decision.operation);
                        let cap = self.db.resource_cap(&contract.session_id, &decision.target_id, id, k).await;
                        self.db.write_resource_current(&contract.session_id, &decision.target_id, id, after, cap, &merge_source_refs(&contract.source_refs, &decision.source_refs), Visibility::GmOnly, contract.world_tick_hint()).await?;
                        let impact = ParameterImpact {
                            impact_id: format!("impact_{}", Uuid::new_v4().simple()),
                            target_kind: EffectTargetKind::Actor,
                            target_id: decision.target_id.clone(),
                            parameter_path: decision.parameter_path.clone(),
                            operation: decision.operation,
                            value: json!(amount),
                            before: Some(json!(before)),
                            after: Some(json!(after)),
                            validator_status: decision.status.as_str().into(),
                            visibility: Visibility::GmOnly,
                            source_refs: merge_source_refs(&contract.source_refs, &decision.source_refs),
                            provisional_reason: decision.provisional_reason.clone(),
                        };
                        patches.push(StatePatch::ModifyTrack { target: format!("{}.{}", decision.target_id, decision.parameter_path), amount: after - before, reason: "parameter_facet_executor_resource_delta".into() });
                        impacts.push(impact);
                    } else {
                        // fail-closed: unresolvable resource path -> record, do not mutate
                        patches.push(StatePatch::CreateFact { target: decision.target_id.clone(), fact: json!({"blocked_resource_delta": true, "parameter_path": decision.parameter_path, "reason": "unresolved_resource_track_id"}), reason: "parameter_facet_executor_blocked_unresolved_resource".into() });
                    }
                }
```

> **Implementer:** remove the call to `self.update_actor_resources(...)`. The condition branches (`hp.current` → conditions → resources) stay in the same order; only the trailing `else` (resources) is rewritten.

- [ ] **Step 3: Remove now-dead helpers** — after the edits, `update_actor_hp` (733-737) and `update_actor_resources` (769-773) should have NO remaining callers. Verify with `grep -n "update_actor_hp\|update_actor_resources" crates/trpg-mechanics/src/lib.rs`. If the only hits are the definitions, delete both functions (and any now-unused helpers like `resource_relative_path`/`get_json_path`/`set_resource_value` if grep shows they're unused — otherwise leave them).

- [ ] **Step 4: Run the unification test — expect GREEN** — Run: `cargo build -p trpg-mechanics 2>&1 | tail -20` then `DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg cargo test -p trpg-mechanics --test live_apply_effect_roll -- --nocapture --test-threads=1 2>&1 | tail -30`. Expected: PASS — `load_resource_current` now reads `Some(7)` because the write landed in gps. If you added the temporary `actor_mechanical_states.hp_current == 7` assertion in Task 5, DELETE it now (that store is retired) and re-run.

- [ ] **Step 5: Checkpoint** — `cargo build -p trpg-mechanics 2>&1 | tail -5`. Clean.

---

## Task 7: apply_outcome_resource_tracks + ensure_actor_state on the primitives (DRY)

**Files:**
- Modify: `crates/trpg-mechanics/src/lib.rs` — `apply_outcome_resource_tracks` (99-219), `actor_resource_seeds` (648-654), `ensure_actor_state` (656-705), and the moved-out pure fns

- [ ] **Step 1: Redirect `actor_resource_seeds`** — its body (648-654) now just delegates so seeds come from one place:

```rust
    async fn actor_resource_seeds(&self, session_id: &str, actor_id: &str, kernel: &RuleKernel) -> std::collections::HashMap<String, (Option<i32>, Option<i32>)> {
        self.db.resource_seeds(session_id, actor_id, kernel).await
    }
```

- [ ] **Step 2: Route the actor branch of `apply_outcome_resource_tracks` through the SSOT primitive** — the scene branch (`tracks.{id}.current`) keeps the existing `load_generic_parameter_value`/`upsert_generic_parameter_state`; only the ACTOR branch uses the new primitives so contest accrual and effect rolls share one path. Replace the `before`/`cap`/`upsert` region (lines 179-187) with:

```rust
                let before = if is_actor {
                    self.db.load_resource_current(&contract.session_id, &target_id, &id, &kernel).await
                        .unwrap_or_else(|| seeds.get(&id).and_then(|s| s.0).or(initial).unwrap_or(0))
                } else {
                    self.load_generic_parameter_value(&contract.session_id, target_kind, &target_id, &path).await
                        .ok().flatten().and_then(|v| v.as_i64()).map(|n| n as i32)
                        .or(seeds.get(&id).and_then(|s| s.0)).or(initial).unwrap_or(0)
                };
                let mut after = apply_i32_operation(before, amount, op);
                let cap = seeds.get(&id).and_then(|s| s.1).or(max);
                if let Some(m) = cap { after = after.min(m); }
                if is_actor {
                    self.db.write_resource_current(&contract.session_id, &target_id, &id, after, cap, &contract.source_refs, Visibility::GmOnly, 0).await.ok();
                } else {
                    self.upsert_generic_parameter_state(&contract.session_id, target_kind, &target_id, &path, json!(after), Visibility::GmOnly, &contract.source_refs, None, 0).await.ok();
                }
```

> Behavior is preserved (actor path still writes gps `resources.{id}.current` with the same cap), but now goes through the identical primitive `apply_effect_roll` uses. The `path` variable is still used by the scene branch and the `committed_patches` fact (line 188-190) — keep it.

- [ ] **Step 3: Slim `ensure_actor_state`** — it still creates the ams row (armor/morale/conditions/frame) but its `hp_current`/`hp_max`/`resources` fields are now a benign creation-time snapshot, NOT the source of truth. Leave the function as-is functionally EXCEPT: it must still compile after `match_seed`/`build_resources_json` moved to `trpg-model`. Update its internal references: `build_resources_json(&seeds)` → `trpg_model::build_resources_json(&seeds)` IF you kept that fn; otherwise (recommended) replace the `resources` local with `let resources = json!({});` and delete `build_resources_json` + its 3 tests from mechanics (resources_json seeding is retired). Keep the `hp_seed`/`default_hp` lines (harmless snapshot). Confirm `actor_resource_seeds` is still called for the snapshot.

- [ ] **Step 4: Delete moved/retired pure fns from mechanics** — remove `match_seed` (60-76) from mechanics (now in trpg-model; `actor_resource_seeds` no longer calls it directly since it delegates to `db.resource_seeds`). Remove `build_resources_json` (82-89) if Step 3 took the `json!({})` route. Update the test module `resource_seed_tests` (1285+): delete tests that referenced the removed fns, OR change `use super::{...}` to `use trpg_model::{match_seed};` and keep the match_seed tests. Ensure no dangling references.

- [ ] **Step 5: Run** — Run: `cargo build -p trpg-mechanics 2>&1 | tail -20` (fix any unused-import / missing-fn errors), then `DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/cargo... ` — i.e. `DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg cargo test -p trpg-mechanics -- --test-threads=1 2>&1 | tail -30`. Expected: all mechanics tests PASS (unit + live_apply_effect_roll).

- [ ] **Step 6: Checkpoint** — `cargo build -p trpg-mechanics 2>&1 | tail -5`. Clean.

---

## Task 8: contest read_track_value → db primitive (+ fix target_kind)

**Files:**
- Modify: `crates/trpg-contest/src/lib.rs` — `read_track_value` (191-205)

- [ ] **Step 1: Replace `read_track_value`** with a delegation to the SSOT reader:

```rust
    /// Read a live resource-track CURRENT value from the single source of truth
    /// (generic_parameter_states, target_kind=actor); falls back to the actor's
    /// character-derived seed and then the kernel track's `initial`.
    async fn read_track_value(&self, session_id: &str, actor_id: &str, track_id: &str, kernel: &RuleKernel) -> Option<i32> {
        self.db.load_resource_current(session_id, actor_id, track_id, kernel).await
    }
```

- [ ] **Step 2: Run** — Run: `cargo build -p trpg-contest 2>&1 | tail -20` then `DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg cargo test -p trpg-contest -- --nocapture 2>&1 | tail -30`. Expected: build clean; `live_percentile` tests PASS or SKIP (must NOT regress).

- [ ] **Step 3: Checkpoint** — `cargo build -p trpg-contest 2>&1 | tail -5`. Clean.

---

## Task 9: combat create_frame → live current HP from gps

**Files:**
- Modify: `crates/trpg-combat/src/lib.rs` — `create_frame` (630-635)

- [ ] **Step 1: Replace the HP-resolution lines (630-635)** so participants reflect live current HP, falling back to derived seed at combat start:

```rust
        // Resolve participant HP from the single source of truth (live current in
        // generic_parameter_states via the kernel's HP track). No gps row yet
        // (combat just started) -> the character-derived seed (full HP). Fail-closed
        // to None when no kernel HP track / no derived value.
        let kernel = self.db.load_rule_kernel(input.ruleset_id).await.ok().flatten();
        let hp_id = kernel.as_ref().and_then(|k| trpg_model::hp_resource_track_id(&k.resource_tracks));
        let pc_hp = match (kernel.as_ref(), hp_id.as_ref()) { (Some(k), Some(id)) => self.db.load_resource_current(input.session_id, &actor_id, id, k).await, _ => None };
        let npc_hp = match (kernel.as_ref(), hp_id.as_ref()) { (Some(k), Some(id)) => self.db.load_resource_current(input.session_id, &npc_actor_id, id, k).await, _ => None };
```

> This removes the dependency on `hp_from_params` for participant HP. Leave `hp_from_params` and its `hp_resolution_tests` in place (still valid pure helper; do not delete unless it becomes unused everywhere — grep `hp_from_params` to confirm before any removal).

- [ ] **Step 2: Run** — Run: `cargo build -p trpg-combat 2>&1 | tail -20` then `cargo test -p trpg-combat -- --nocapture 2>&1 | tail -20`. Expected: build clean; combat unit tests PASS.

- [ ] **Step 3: Checkpoint** — `cargo build -p trpg-combat 2>&1 | tail -5`. Clean.

---

## Task 10: GM mechanical ledger projects live current HP from gps

**Files:**
- Modify: `crates/trpg-mechanics/src/lib.rs` — `mechanical_ledger_context_block` (296-366)

**Context:** After unification, `actor_mechanical_states.hp_current` is stale (no longer updated). The ledger's per-actor `hp_current` must be sourced from gps so the GM sees live HP. SAN etc. already appear in the `generic_parameter_states` list (lines 321-331), so only the per-actor HP needs an authoritative override.

- [ ] **Step 1: Inject live HP per actor** — after the `actors` vec is built (line 311), replace the value of each actor's `hp_current` with the gps live value when resolvable. Insert before the `effect_rows` query:

```rust
        // Override the (now-stale) ams.hp_current with the live SSOT value from
        // generic_parameter_states via each actor's kernel HP track. Cache kernels
        // by ruleset to avoid reloading per actor.
        let mut actors = actors; // make mutable
        let mut kernel_cache: std::collections::HashMap<String, Option<RuleKernel>> = std::collections::HashMap::new();
        for a in actors.iter_mut() {
            let Some(actor_id) = a.get("actor_id").and_then(|v| v.as_str()).map(str::to_string) else { continue };
            let Some(rs) = self.actor_ruleset_id(session_id, &actor_id).await else { continue };
            let kernel = kernel_cache.entry(rs.clone()).or_insert(self.db.load_rule_kernel(&rs).await.ok().flatten()).clone();
            let Some(k) = kernel else { continue };
            let Some(hp_id) = trpg_model::hp_resource_track_id(&k.resource_tracks) else { continue };
            if let Some(live) = self.db.load_resource_current(session_id, &actor_id, &hp_id, &k).await {
                if let Some(obj) = a.as_object_mut() {
                    obj.insert("hp_current".into(), json!(live));
                    obj.insert("hp_source".into(), json!("generic_parameter_states_live"));
                }
            }
        }
```

> Confirm `actor_ruleset_id` is accessible here (it's a method on the same `impl`, used by `ensure_actor_state`). If the `let mut actors = actors;` shadow causes a borrow issue, change the original `let actors: Vec<Value> = ...` (line 299) to `let mut actors: Vec<Value> = ...` and drop the shadow line.

- [ ] **Step 2: Run** — Run: `cargo build -p trpg-mechanics 2>&1 | tail -20`. Expected: clean. (No dedicated unit test for the block; covered by the e2e in Task 12.)

- [ ] **Step 3: Checkpoint** — `cargo build -p trpg-mechanics 2>&1 | tail -5`. Clean.

---

## Task 11: Cross-path consistency test (the SSOT proof)

**Files:**
- Modify: `crates/trpg-mechanics/tests/live_apply_effect_roll.rs` (add a test) — OR add to `crates/trpg-contest/tests/`. Put it where both `RefereeCombatService` and `ContestService` are reachable. `trpg-contest` depends on `trpg-params`+`trpg-db`; `trpg-mechanics` depends on `trpg-db`. Neither depends on the other, and a test crate can dev-depend on both — add `trpg-contest` as a dev-dependency of `trpg-mechanics` (`crates/trpg-mechanics/Cargo.toml` `[dev-dependencies]`) so this test can call both.

- [ ] **Step 1: Add `trpg-contest` dev-dependency** — in `crates/trpg-mechanics/Cargo.toml` under `[dev-dependencies]` add: `trpg-contest = { path = "../trpg-contest" }` (and `trpg-params` if needed for actor seeding). Run `cargo build -p trpg-mechanics --tests 2>&1 | tail -5` to confirm no cycle (mechanics is not a dependency of contest, so this dev-only edge is fine).

- [ ] **Step 2: Write the consistency test** — append to `live_apply_effect_roll.rs`:

```rust
#[tokio::test]
async fn combat_hp_write_is_visible_to_contest_read() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect: {e}"); return; } };
    if db.load_rule_kernel(RULESET).await.ok().flatten().is_none() { eprintln!("SKIP: no {RULESET} kernel"); return; }
    seed_target(&db, 12).await;
    let svc = RefereeCombatService { db: db.clone() };

    let (contract, mut result) = damage_contract(5);   // 12 - 5 = 7
    svc.after_check_resolved(&contract, &mut result).await.expect("effect roll");

    // The contest path's read_track_value must now SEE the combat write (same store,
    // same code path). read_track_value is private; assert via the public resolver
    // OR via db.load_resource_current which read_track_value now delegates to.
    let kernel = db.load_rule_kernel(RULESET).await.unwrap().unwrap();
    let hp_id = trpg_model::hp_resource_track_id(&kernel.resource_tracks).unwrap();
    assert_eq!(db.load_resource_current(SESSION, TARGET, &hp_id, &kernel).await, Some(7),
        "contest read path must observe the combat HP write — single source of truth");
}
```

> If you can construct a `ContestService` + a percentile contract that tests `read_track_value` end-to-end, prefer that (stronger). Otherwise the `db.load_resource_current` assertion is the operative proof since Task 8 made `read_track_value` a thin delegate to it.

- [ ] **Step 3: Run** — Run: `DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg cargo test -p trpg-mechanics --test live_apply_effect_roll -- --nocapture --test-threads=1 2>&1 | tail -30`. Expected: both tests PASS.

- [ ] **Step 4: Checkpoint** — `cargo build -p trpg-mechanics --tests 2>&1 | tail -5`. Clean.

---

## Task 12: Full workspace verification + real CoC turn + D&D check

**Files:** none (verification only)

- [ ] **Step 1: Full workspace build** — Run: `cargo build --workspace 2>&1 | tail -30`. Expected: clean build, no errors. Fix any cross-crate breakage (e.g. other callers of the removed `update_actor_hp`/`match_seed`).

- [ ] **Step 2: Full test run (DB-backed)** — Run: `DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg cargo test --workspace -- --test-threads=1 2>&1 | tail -60`. Expected: all PASS or SKIP; no FAIL. Note any pre-existing unrelated failures explicitly.

- [ ] **Step 3: Real CoC turn e2e** — drive a real damage/SAN turn against the CoC DB. Read `_play.sh` / `_capstone_run.sh` for the existing CLI invocation pattern (`trpg play ...`), then run a short scripted session on :54347 that (a) triggers a SAN check failure and (b) takes combat HP damage. After the turn, verify both current values live in gps:
  ```
  docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg -c \
    "select target_id, parameter_path, value_json from generic_parameter_states where parameter_path like 'resources.%.current' order by updated_at desc limit 20;"
  ```
  Expected: rows for `resources.sanity.current` AND `resources.hit_points.current` with decremented values; no reliance on `actor_mechanical_states.hp_current`. Confirm the GM context (ledger) shows live HP (check logs / the prompt dump if the CLI emits one).

- [ ] **Step 4: D&D HP check** — seed a throwaway D&D actor with a derived `hit_points` and confirm combat HP resolves (not None):
  ```
  # via a quick one-off test or the combat path; minimally:
  DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg cargo test -p trpg-db --test live_kernel_override -- --nocapture
  ```
  Expected: PASS (D&D kernel has a valid `hit_points` track after override). Optionally extend `live_resource_current.rs` with a D&D-ruleset actor seeded `sheet_json.resources.hit_points = 30` and assert `load_resource_current` returns 30.

- [ ] **Step 5: Final report (Chinese)** — summarize: stores unified (single gps store + single trpg-db code path), regression net added for apply_effect_roll, contest/combat/ledger converged, D&D fixed via normalize+override, all tests green, real CoC turn verified. Note anything deferred (physical column drop, D&D chargen hit_points derivation).

---

## Self-Review (completed by author)

**Spec coverage:** §4.1 pure helpers → Task 1. §4.2 trpg-db primitives → Task 2. §4.3 contest convergence + target_kind fix → Task 8. §4.4 mechanics unify (apply_effect_roll, apply_outcome, ensure_actor_state) → Tasks 6-7. §4.5 combat frame → Task 9. §4.6 GM ledger → Task 10. §4.7 D&D normalize+override+data → Tasks 3-4. test-first net → Task 5 (red) → Task 6 (green). cross-path consistency → Task 11. CoC e2e + D&D verify → Task 12. All spec sections mapped.

**Placeholder scan:** test contract construction in Tasks 5/11 references the existing `live_percentile.rs` pattern rather than reproducing the full multi-field `CheckContract`/`CheckResultRecord` literals (their exact shapes must be read from `trpg-model`); this is an intentional "copy the established pattern" instruction, not a logic gap — the novel parts (seeding SQL, assertions, run commands, expected output) are fully specified. No "TBD"/"add error handling"/"similar to Task N" placeholders elsewhere.

**Type consistency:** primitive names consistent across tasks — `resource_seeds`, `load_resource_current`, `resource_cap`, `write_resource_current` (trpg-db); `match_seed`, `normalize_resource_tracks`, `resolve_resource_track_id`, `apply_armor_damage`, `wound_label` (trpg-model). `apply_armor_damage` returns `(to, sp_applied)` and is consumed that way in Task 6. `RefereeCombatService { db }` construction consistent in Tasks 5/11.

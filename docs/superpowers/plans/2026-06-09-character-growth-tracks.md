# Character Growth — Track Layer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a generic, data-driven character-growth "Track" layer (class levels, skill ratings, pools, ordinal grades) where the engine REPRESENTS advanceable quantities and AUTO-RECOMPUTES derived values; advancement rules stay GM/player-driven via one generic mutate primitive.

**Architecture:** Tracks live in `sheet_json.tracks` (the store the existing live-linkage reads). Derived `§4` records key on tracks via `{{tracks.<id>.value}}` and two new generic aggregation functions `sum_tracks(kind)` / `max_tracks(kind)` (for multiclass — an unknown number of class-level tracks). Ordinal letter-grades reuse the existing categorical `lookup_cat`. A single `apply_track_change` primitive (CLI + GM tool) mutates a track/base value and triggers the existing `refresh_actor_live_derived`. Decision-4 "override wins" reuses the existing `load_chargen_spec` override layer.

**Tech Stack:** Rust workspace (~26 crates). `trpg-formula` (pure evaluator: `expr.rs` lexer/parser/AST, `lib.rs` `evaluate_chargen`), `trpg-runtime` (`chargen.rs` sheet shaping + live recompute, `lib.rs` engine methods), `trpg-cli` (clap). NO git in this working copy — "Checkpoint" steps run `cargo test`, not `git commit`. Files ≤ ~400 lines. No per-ruleset hardcoding.

**Spec:** `docs/superpowers/specs/2026-06-09-character-growth-tracks-design.md`

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/trpg-formula/src/expr.rs` | lexer/parser/AST + lookup | Add `Parser.tracks`, `sum_tracks/max_tracks` call branch, `aggregate_tracks` helper, `eval()` signature `+tracks` |
| `crates/trpg-formula/src/lib.rs` | `evaluate_chargen` orchestration | Thread `tracks` through `eval_one`/`eval_hybrid`/`evaluate_chargen` |
| `crates/trpg-formula/tests/eval.rs` | formula tests | Add aggregation tests |
| `crates/trpg-runtime/src/chargen.rs` | sheet shaping, live recompute, mutate helper | `build_chargen_inputs` forwards `tracks`; add pure `apply_track_change_to_sheet`; tests |
| `crates/trpg-runtime/src/lib.rs` | engine methods | Add async `apply_track_change` (load → mutate → recompute → save) |
| `crates/trpg-cli/src/main.rs` | clap CLI | Add `Grow` command + handler |
| `crates/trpg-cli/src/<gm tool site>` | GM agent tool loop | Register `apply_track_change` typed tool (discovery step in Task 6) |

---

## Task 1: Aggregation primitives `sum_tracks` / `max_tracks(kind)` + thread structured tracks into the evaluator

**Files:**
- Modify: `crates/trpg-formula/src/expr.rs` (Parser struct ~line 91; `eval` ~line 230; `call` ~line 155-183)
- Modify: `crates/trpg-formula/src/lib.rs` (`evaluate_chargen` ~line 114-178; `eval_one` ~line 187; `eval_hybrid` ~line 189-205)
- Test: `crates/trpg-formula/tests/eval.rs`

- [ ] **Step 1: Write the failing test**

Add to `crates/trpg-formula/tests/eval.rs`:

```rust
#[test]
fn sum_and_max_tracks_aggregate_by_kind() {
    let inp = inputs(&[("tracks", json!({
        "fighter": {"value": 4, "kind": "class_level"},
        "rogue":   {"value": 1, "kind": "class_level"},
        "wizard":  {"value": 2, "kind": "magical_class_level"}
    }))]);
    // D&D total character level = SUM of all class-level tracks (multiclass).
    let recs = vec![json!({"id":"total_level","input_kind":"derived","expr":"sum_tracks(class_level)"})];
    assert_eq!(get(&evaluate_chargen(&recs, &inp), "total_level").value, Some(json!(5)));
    // Sword World adventurer level = MAX class level, NOT sum.
    let recs2 = vec![json!({"id":"adv","input_kind":"derived","expr":"max_tracks(class_level)"})];
    assert_eq!(get(&evaluate_chargen(&recs2, &inp), "adv").value, Some(json!(4)));
    // MP keys a kind-SUBSET (magical classes only) — fine-grained kind tag.
    let recs3 = vec![json!({"id":"mp","input_kind":"derived","expr":"sum_tracks(magical_class_level)*3"})];
    assert_eq!(get(&evaluate_chargen(&recs3, &inp), "mp").value, Some(json!(6)));
    // Empty set -> 0 (identity), never an error.
    let recs4 = vec![json!({"id":"none","input_kind":"derived","expr":"sum_tracks(nonexistent)"})];
    assert_eq!(get(&evaluate_chargen(&recs4, &inp), "none").value, Some(json!(0)));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p trpg-formula sum_and_max_tracks_aggregate_by_kind`
Expected: FAIL — `unknown function 'sum_tracks'` (the evaluator errors on the unknown call).

- [ ] **Step 3: Implement — thread `tracks` + add the aggregation branch**

In `crates/trpg-formula/src/expr.rs`, add `tracks` to the Parser:

```rust
struct Parser<'a> { t: &'a [Tok], i: usize, ctx: &'a HashMap<String, Num>, tables: &'a Value, tracks: &'a Value }
```

Change `pub fn eval` (bottom of file) to accept + thread `tracks`:

```rust
pub fn eval(expr: &str, ctx: &HashMap<String, Num>, tables: &Value, tracks: &Value) -> Result<Num, String> {
    let toks = lex(expr)?;
    if toks.is_empty() { return Err("empty expression".into()); }
    let mut p = Parser { t: &toks, i: 0, ctx, tables, tracks };
    let v = p.expr()?;
    if p.i != toks.len() { return Err("trailing tokens".into()); }
    Ok(v)
}
```

In `fn call(&mut self, name: &str)`, add this branch immediately AFTER the `if name == "lookup" { ... }` block (before `let a = self.args()?;`):

```rust
        // sum_tracks(kind) / max_tracks(kind): aggregate over a DYNAMIC set of
        // tracks sharing `kind` (e.g. multiclass: unknown count of class levels).
        // kind is a bare ident (like the lookup table name), NOT a numeric arg.
        if name == "sum_tracks" || name == "max_tracks" {
            match self.next() { Some(Tok::LParen) => {} _ => return Err("expected (".into()) }
            let kind = match self.next() {
                Some(Tok::Ident(k)) => k,
                other => return Err(format!("{name}() kind must be a name, got {other:?}")),
            };
            match self.next() { Some(Tok::RParen) => {} _ => return Err("expected )".into()) }
            return aggregate_tracks(self.tracks, &kind, name);
        }
```

Add this free function near `lookup_cat` (e.g. after it):

```rust
/// Aggregate the numeric `value` of every track whose `kind` matches (case-insensitive).
/// Generic — supports a dynamic set (multiclass = unknown number of class-level tracks).
/// Empty set -> 0 (additive/idle identity), never an error or a fabricated value.
fn aggregate_tracks(tracks: &Value, kind: &str, op: &str) -> Result<Num, String> {
    let k = kind.trim().to_ascii_lowercase();
    let vals: Vec<f64> = tracks.as_object().map(|m| m.values()
        .filter(|t| t.get("kind").and_then(|v| v.as_str()).map(|s| s.trim().to_ascii_lowercase() == k).unwrap_or(false))
        .filter_map(|t| t.get("value").and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))))
        .collect()).unwrap_or_default();
    let out = match op {
        "sum_tracks" => vals.iter().sum(),
        "max_tracks" => vals.iter().cloned().fold(0.0_f64, f64::max),
        _ => return Err(format!("unknown aggregate `{op}`")),
    };
    Ok(Num::N(out))
}
```

In `crates/trpg-formula/src/lib.rs`, thread `tracks` through the three call sites:

```rust
// eval_one: add the tracks param
fn eval_one(e: &str, ctx: &HashMap<String, Num>, tables: &Value, tracks: &Value) -> Result<Num, String> { expr::eval(e, ctx, tables, tracks) }

// eval_hybrid: add `tracks: &Value` to the signature and pass it to BOTH internal eval_one calls
fn eval_hybrid(r: &Value, ctx: &HashMap<String, Num>, tables: &Value, result_type: &str, tracks: &Value) -> Result<(Value, Value, Num), String> {
    let attr = match r.get("attr_derived").and_then(|v| v.as_str()) { Some(e) => eval_one(e, ctx, tables, tracks)?.num_or_err()?, None => 0.0 };
    // ... unchanged ...
    if let Some(allocs) = r.get("allocations").and_then(|v| v.as_array()) {
        for a in allocs {
            let v = match a.get("input").and_then(|x| x.as_str()) { Some(e) => eval_one(e, ctx, tables, tracks).and_then(|n| n.num_or_err()).unwrap_or(0.0), None => 0.0 };
            // ... unchanged ...
        }
    }
    // ... unchanged ...
}
```

In `evaluate_chargen`, extract the structured tracks once (right after `let mut ctx = ctx_from_inputs(inputs);`):

```rust
    let tracks = inputs.get("tracks").cloned().unwrap_or_else(|| json!({}));
```

and update the two eval call sites inside the per-record loop:

```rust
        let res = if r.get("expr").is_some() {
            eval_one(&s(r, "expr"), &eval_ctx, &tables, &tracks).map(|n| (as_int_or_dice(&n, &result_type), json!({"final": as_int_or_dice(&n, &result_type)}), n))
        } else {
            eval_hybrid(r, &eval_ctx, &tables, &result_type, &tracks)
        };
```

- [ ] **Step 4: Run test to verify it passes (and nothing regressed)**

Run: `cargo test -p trpg-formula`
Expected: PASS — the new test plus all 10 pre-existing tests green.

- [ ] **Step 5: Checkpoint (build + full formula tests)**

Run: `cargo test -p trpg-formula 2>&1 | tail -5`
Expected: `test result: ok.` No git commit (no repo).

---

## Task 2: `build_chargen_inputs` forwards the `tracks` subtree (end-to-end + live recompute)

**Files:**
- Modify: `crates/trpg-runtime/src/chargen.rs` (`build_chargen_inputs` ~line 181-203)
- Test: `crates/trpg-runtime/src/chargen.rs` (`mod tests`, near `live_recompute_updates_derived_on_base_change`)

- [ ] **Step 1: Write the failing test**

Add to the `chargen_formula_tests` module in `crates/trpg-runtime/src/chargen.rs`:

```rust
    #[test]
    fn tracks_drive_derived_and_live_recompute() {
        // D&D multiclass: total level = sum of class-level tracks; proficiency keys it.
        let spec = vec![
            json!({"id":"total_level","role":"attribute","input_kind":"derived","recompute":"live","expr":"sum_tracks(class_level)"}),
            json!({"id":"proficiency","role":"attribute","input_kind":"derived","recompute":"live","result_type":"int",
                   "expr":"lookup(prof_by_level,{{derived.total_level}})","lookup_tables":{"prof_by_level":{"ranges":[
                       {"min":1,"max":4,"value":2},{"min":5,"max":8,"value":3},{"min":9,"max":12,"value":4}]}}}),
        ];
        let mut sheet = json!({"tracks":{"fighter":{"value":4,"kind":"class_level"},"rogue":{"value":1,"kind":"class_level"}}});
        apply_chargen_formulas(&spec, &mut sheet);
        assert_eq!(sheet["stats"]["total_level"], json!(5), "4+1 multiclass");
        assert_eq!(sheet["stats"]["proficiency"], json!(3), "total 5 -> +3");
        // Multiclass advances rogue 1 -> 5 (total 9); live recompute lifts proficiency to +4.
        sheet["tracks"]["rogue"]["value"] = json!(5);
        assert!(recompute_live_derived(&mut sheet), "track change re-derives");
        assert_eq!(sheet["stats"]["total_level"], json!(9));
        assert_eq!(sheet["stats"]["proficiency"], json!(4), "total 9 -> +4 (live)");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p trpg-runtime tracks_drive_derived_and_live_recompute`
Expected: FAIL — `total_level` is provisional/absent (the `tracks` subtree never reaches the evaluator), so `sheet["stats"]["total_level"]` is null.

- [ ] **Step 3: Implement — forward the `tracks` subtree**

In `build_chargen_inputs` (`crates/trpg-runtime/src/chargen.rs`), right after the `player` line `if let Some(p) = sheet.get("player") { inputs.insert("player".into(), p.clone()); }`, add:

```rust
    // Forward the whole tracks subtree (structured): aggregation fns (sum_tracks/
    // max_tracks) read it by kind, and ctx flattening also exposes {{tracks.<id>.value}}.
    if let Some(t) = sheet.get("tracks") { inputs.insert("tracks".into(), t.clone()); }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p trpg-runtime tracks_drive_derived_and_live_recompute`
Expected: PASS.

- [ ] **Step 5: Checkpoint**

Run: `cargo test -p trpg-runtime chargen 2>&1 | tail -8`
Expected: `test result: ok.` (the new test + all prior chargen tests green).

---

## Task 3: `apply_track_change` primitive (pure sheet helper + async engine method) + accumulator invariant

**Files:**
- Modify: `crates/trpg-runtime/src/chargen.rs` (add `apply_track_change_to_sheet` near `recompute_live_derived`)
- Modify: `crates/trpg-runtime/src/lib.rs` (add async `apply_track_change` in the same `impl` block as `refresh_actor_live_derived` ~line 1416)
- Test: `crates/trpg-runtime/src/chargen.rs` (`mod tests`)

- [ ] **Step 1: Write the failing tests**

Add to the `chargen_formula_tests` module:

```rust
    #[test]
    fn apply_track_change_sets_adds_and_creates() {
        let mut sheet = json!({"tracks":{"fighter":{"value":4,"kind":"class_level"}}});
        apply_track_change_to_sheet(&mut sheet, "tracks", "fighter", "add", 1.0, None, None).unwrap();
        assert_eq!(sheet["tracks"]["fighter"]["value"], json!(5.0), "add to existing");
        apply_track_change_to_sheet(&mut sheet, "tracks", "rogue", "set", 1.0, Some("class_level"), None).unwrap();
        assert_eq!(sheet["tracks"]["rogue"]["value"], json!(1.0), "created on first set");
        assert_eq!(sheet["tracks"]["rogue"]["kind"], json!("class_level"));
        // base stat (Sword World per-session stat growth) goes through the same primitive.
        let mut s2 = json!({"stats":{"dexterity":12}});
        apply_track_change_to_sheet(&mut s2, "stats", "dexterity", "add", 1.0, None, None).unwrap();
        assert_eq!(s2["stats"]["dexterity"], json!(13.0));
    }

    #[test]
    fn accumulator_base_value_not_clobbered_by_live_recompute() {
        // Spec §6.7: path-dependent HP is a BASE accumulator (GM adds on level-up),
        // NEVER a level-keyed live derived. The live pass must leave it untouched.
        let spec = vec![json!({"id":"total_level","role":"attribute","input_kind":"derived","recompute":"live","expr":"sum_tracks(class_level)"})];
        let mut sheet = json!({"tracks":{"fighter":{"value":2,"kind":"class_level"}}, "stats":{"hit_points":17}});
        apply_chargen_formulas(&spec, &mut sheet);
        apply_track_change_to_sheet(&mut sheet, "tracks", "fighter", "add", 1.0, None, None).unwrap();
        apply_track_change_to_sheet(&mut sheet, "stats", "hit_points", "add", 7.0, None, None).unwrap();
        assert!(recompute_live_derived(&mut sheet));
        assert_eq!(sheet["stats"]["total_level"], json!(3), "derived re-derives");
        assert_eq!(sheet["stats"]["hit_points"], json!(24.0), "accumulated HP NOT recomputed/clobbered");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p trpg-runtime apply_track_change_sets_adds_and_creates`
Expected: FAIL — `apply_track_change_to_sheet` not found.

- [ ] **Step 3: Implement the pure helper**

Add to `crates/trpg-runtime/src/chargen.rs` (e.g. right after `recompute_live_derived`):

```rust
/// Generic growth mutate primitive (pure, DB-free → unit-testable). Sets or adds a
/// value to a TRACK (`bucket="tracks"`: a `{value,kind,category}` object, created on
/// first set) or a BASE stat/skill (`bucket="stats"`/`"skills"`: a bare number).
/// `op` = "set" | "add". The engine only MOVES the value — it does NOT enforce any
/// ruleset advancement rule (XP→level, IP cost, prereqs); that is GM/player-driven.
pub fn apply_track_change_to_sheet(
    sheet: &mut Value, bucket: &str, id: &str, op: &str, amount: f64,
    kind: Option<&str>, category: Option<&str>,
) -> Result<(), String> {
    let obj = sheet.as_object_mut().ok_or("sheet is not an object")?;
    let b = obj.entry(bucket.to_string()).or_insert_with(|| json!({}));
    let bm = b.as_object_mut().ok_or("bucket is not an object")?;
    if bucket == "tracks" {
        let entry = bm.entry(id.to_string()).or_insert_with(|| json!({"value": 0, "kind": kind.unwrap_or("counter")}));
        let cur = entry.get("value").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let nv = if op == "add" { cur + amount } else { amount };
        if let Some(em) = entry.as_object_mut() {
            em.insert("value".into(), json!(nv));
            if let Some(k) = kind { em.insert("kind".into(), json!(k)); }
            if let Some(c) = category { em.insert("category".into(), json!(c)); }
        }
    } else {
        let cur = bm.get(id).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let nv = if op == "add" { cur + amount } else { amount };
        bm.insert(id.to_string(), json!(nv));
    }
    Ok(())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p trpg-runtime apply_track_change_sets_adds_and_creates accumulator_base_value_not_clobbered_by_live_recompute`
Expected: PASS (both).

- [ ] **Step 5: Add the async engine method**

In `crates/trpg-runtime/src/lib.rs`, in the SAME `impl` block as `refresh_actor_live_derived`, add:

```rust
    /// Growth/advancement entry point (CLI command + GM-agent tool both call this).
    /// Loads the actor's params, mutates a track or base value in `sheet_json`, then
    /// re-derives (live linkage) and persists. A0: engine moves the value only —
    /// the GM/player decides the rule-specific amount. Returns Ok(false) if no params.
    pub async fn apply_track_change(
        &self, session_id: &str, actor_id: &str,
        bucket: &str, id: &str, op: &str, amount: f64,
        kind: Option<&str>, category: Option<&str>,
    ) -> Result<bool> {
        let service = RuntimeParameterService::new(self.db.clone());
        let Some(mut p) = service.load_actor_parameters(session_id, actor_id).await? else { return Ok(false); };
        chargen::apply_track_change_to_sheet(&mut p.sheet_json, bucket, id, op, amount, kind, category)
            .map_err(|e| anyhow!("apply_track_change: {e}"))?;
        chargen::recompute_live_derived(&mut p.sheet_json);
        service.upsert_actor_parameters(&p).await?;
        Ok(true)
    }
```

- [ ] **Step 6: Checkpoint**

Run: `cargo build -p trpg-runtime && cargo test -p trpg-runtime chargen 2>&1 | tail -8`
Expected: builds; `test result: ok.`

---

## Task 4: Confirm track-keyed derived ride the existing chargen override layer (reuse, decision 4)

**Files:**
- Test: `crates/trpg-runtime/src/chargen.rs` (`compiler_runtime_tests` module — `merge_override` is already there)

Rationale: `load_chargen_spec` (chargen.rs:295) already layers `{ruleset}.chargen.override.json` over the compiled spec via `merge_override` (override wins by `id`). A track-keyed derived override is just a `§4` record in that same file — no new loader needed. This task pins that behavior with a test so the reuse is explicit and protected.

- [ ] **Step 1: Write the failing test**

Add to the `compiler_runtime_tests` module in `crates/trpg-runtime/src/chargen.rs`:

```rust
    #[test]
    fn authored_track_derived_override_wins_over_compiled() {
        // Decision 4: a hand-authored track-keyed derived (override) beats the
        // flaky-compiler one by id. Defends against the re-parse extraction variance.
        let compiled = vec![json!({"id":"total_level","expr":"{{level_guess}}","status":"provisional"})];
        let authored = vec![json!({"id":"total_level","expr":"sum_tracks(class_level)","status":"source_backed"})];
        let merged = merge_override(compiled, authored);
        let r = merged.iter().find(|r| r["id"] == "total_level").unwrap();
        assert_eq!(r["expr"], json!("sum_tracks(class_level)"), "authored override replaced compiled");
        assert_eq!(r["status"], json!("source_backed"));
    }
```

- [ ] **Step 2: Run test**

Run: `cargo test -p trpg-runtime authored_track_derived_override_wins_over_compiled`
Expected: PASS immediately (merge_override already implements replace-by-id). This is a characterization/guard test confirming reuse.

- [ ] **Step 3: Document the no-regress note (no code)**

Add a one-line doc comment above `load_chargen_spec` noting: "Track-keyed derived overrides go in this same `{ruleset}.chargen.override.json`; pin critical track derived here to defend against compiler re-parse variance (spec decision 4). Auto no-regress-on-reparse is deferred (separate parse-pipeline work)."

- [ ] **Step 4: Checkpoint**

Run: `cargo test -p trpg-runtime 2>&1 | tail -5`
Expected: `test result: ok.`

---

## Task 5: CLI command `trpg grow` (player-facing mutate)

**Files:**
- Modify: `crates/trpg-cli/src/main.rs` (`enum Commands` ~line 32; match arms ~line 444-513)

- [ ] **Step 1: Add the command variant**

In `crates/trpg-cli/src/main.rs`, add to `enum Commands` (after `GrepTable { ... }`):

```rust
    /// Apply a character-growth change: set/add a track (class level, pool, rank) or a
    /// base stat, then re-derive. Engine only moves the value (A0). e.g.
    /// `trpg grow --session S --actor pc.current --bucket tracks --id fighter --op add --amount 1 --kind class_level`
    Grow {
        #[arg(long)] session: String,
        #[arg(long, default_value = "pc.current")] actor: String,
        #[arg(long, value_parser = ["tracks", "stats", "skills"], default_value = "tracks")] bucket: String,
        #[arg(long)] id: String,
        #[arg(long, value_parser = ["set", "add"], default_value = "add")] op: String,
        #[arg(long)] amount: f64,
        #[arg(long)] kind: Option<String>,
        #[arg(long)] category: Option<String>,
    },
```

- [ ] **Step 2: Add the match arm + handler**

In the top-level `match cli.command { ... }`, add (next to `Commands::Turn(args) => ...`):

```rust
        Commands::Grow { session, actor, bucket, id, op, amount, kind, category } => {
            grow_cli(&session, &actor, &bucket, &id, &op, amount, kind.as_deref(), category.as_deref()).await
        }
```

Add the handler function (near `turn_cli`). The DB+engine construction is the same three lines every CLI handler uses (`connect_db` + `migrate` + `RuntimeEngine::new`, verified at main.rs:486-491, 633-635, 730-734):

```rust
async fn grow_cli(
    session: &str, actor: &str, bucket: &str, id: &str, op: &str, amount: f64,
    kind: Option<&str>, category: Option<&str>,
) -> anyhow::Result<()> {
    let db = connect_db().await?;
    db.migrate().await?;
    let runtime = RuntimeEngine::new(db.clone());
    let changed = runtime.apply_track_change(session, actor, bucket, id, op, amount, kind, category).await?;
    println!("{}", serde_json::json!({
        "ok": changed, "session": session, "actor": actor,
        "bucket": bucket, "id": id, "op": op, "amount": amount
    }));
    Ok(())
}
```

- [ ] **Step 3: Build + smoke-check help**

Run: `cargo build -p trpg-cli && ./target/debug/trpg grow --help`
Expected: builds; prints the `grow` usage with `--session/--actor/--bucket/--id/--op/--amount/--kind/--category`.

- [ ] **Step 4: Checkpoint**

Run: `cargo build -p trpg-cli 2>&1 | tail -3`
Expected: `Finished`.

---

## Task 6: GM-agent typed tool for `apply_track_change`

**Files:**
- Modify: the GM turn tool-loop site (DISCOVER in Step 1)

- [ ] **Step 1: Discover the GM tool registry**

Run: `grep -rn "complete_with_tools\|tool_schemas\|\"type\":\"function\"" crates/trpg-runtime/src crates/trpg-cli/src | grep -iv chargen_compile | head`
Identify where the GM turn loop builds its tool list (the `Vec<Value>` of `{"type":"function","function":{...}}` passed to `complete_with_tools`) and where it dispatches a returned tool call by name. Note the file + function.

- [ ] **Step 2: Add the tool schema**

In that tool list, add:

```rust
json!({"type":"function","function":{
  "name":"apply_track_change",
  "description":"Apply a character-growth change when the fiction calls for it (level up, raise a skill, spend points, raise a stat). Sets or adds a value to a track (class level, pool, rank) or a base stat, then the engine re-derives. You decide the rule-appropriate amount; the engine only moves the value.",
  "parameters":{"type":"object","properties":{
    "bucket":{"type":"string","enum":["tracks","stats","skills"]},
    "id":{"type":"string","description":"track id (e.g. 'fighter') or stat id (e.g. 'dexterity')"},
    "op":{"type":"string","enum":["set","add"]},
    "amount":{"type":"number"},
    "kind":{"type":"string","description":"semantic tag for a new track, e.g. class_level / magical_class_level / pool / rank"},
    "category":{"type":"string"}
  },"required":["bucket","id","op","amount"]}
}})
```

- [ ] **Step 3: Add the dispatch arm**

Where returned tool calls are dispatched by name, add a branch that parses the args and calls the engine (the GM loop already has the `session_id` + actor in scope; use the player actor id used elsewhere in the turn, typically `"pc.current"`):

```rust
            if name == "apply_track_change" {
                let b = args.get("bucket").and_then(Value::as_str).unwrap_or("tracks");
                let id = args.get("id").and_then(Value::as_str).unwrap_or("");
                let op = args.get("op").and_then(Value::as_str).unwrap_or("add");
                let amt = args.get("amount").and_then(Value::as_f64).unwrap_or(0.0);
                let kind = args.get("kind").and_then(Value::as_str);
                let cat = args.get("category").and_then(Value::as_str);
                let ok = self.apply_track_change(session_id, "pc.current", b, id, op, amt, kind, cat).await.unwrap_or(false);
                // feed a tool result back into the loop (match the existing tool-result shape)
                json!({"applied": ok, "bucket": b, "id": id, "op": op, "amount": amt}).to_string()
            }
```

(Match the EXACT tool-result message shape the surrounding loop already uses — copy the pattern from a neighboring dispatch arm.)

- [ ] **Step 4: Build**

Run: `cargo build -p trpg-cli -p trpg-runtime 2>&1 | tail -3`
Expected: `Finished`.

- [ ] **Step 5: Checkpoint**

Run: `cargo build --workspace 2>&1 | tail -3`
Expected: `Finished`.

---

## Task 7: Per-archetype deterministic integration tests (validation matrix)

**Files:**
- Test: `crates/trpg-runtime/src/chargen.rs` (`chargen_formula_tests` module)

These lock the spec's §10 matrix as deterministic tests (no LLM/parse), reusing the primitives from Tasks 1-3.

- [ ] **Step 1: Write the tests**

```rust
    #[test]
    fn cyberpunk_role_rank_track_feeds_derived() {
        // Initiative bonus reads a rank track; raising the rank re-derives live.
        let spec = vec![json!({"id":"initiative","role":"attribute","input_kind":"derived","recompute":"live","result_type":"int",
                               "expr":"{{reflexes}}+{{tracks.solo.value}}"})];
        let mut sheet = json!({"stats":{"reflexes":8},"tracks":{"solo":{"value":2,"kind":"rank"}}});
        apply_chargen_formulas(&spec, &mut sheet);
        assert_eq!(sheet["stats"]["initiative"], json!(10), "8 + rank 2");
        apply_track_change_to_sheet(&mut sheet, "tracks", "solo", "add", 1.0, None, None).unwrap();
        assert!(recompute_live_derived(&mut sheet));
        assert_eq!(sheet["stats"]["initiative"], json!(11), "rank 3 -> 11 (live)");
    }

    #[test]
    fn fate_servant_ordinal_grade_resolves_via_categorical_lookup() {
        // Fate letter-grade attribute "A" -> number via a per-ruleset rank table
        // (reuses lookup_cat); the number then feeds a derived. Class-keyed coefficient
        // is the same categorical pattern as D&D class HP.
        let spec = vec![
            json!({"id":"strength","role":"attribute","input_kind":"derived","result_type":"int",
                   "expr":"lookup(rank_value,{{strength_rank}})","lookup_tables":{"rank_value":{"ranges":[
                       {"min":"a","max":"a","value":14},{"min":"ex","max":"ex","value":18}]}}}),
            json!({"id":"hp_max","role":"resource_max","input_kind":"derived","result_type":"int",
                   "expr":"{{derived.strength}}*lookup(class_coef,{{class}})","lookup_tables":{"class_coef":{"ranges":[
                       {"min":"saber","max":"saber","value":10},{"min":"caster","max":"caster","value":7}]}}}),
        ];
        let mut sheet = json!({"class":"Saber","strength_rank":"A"});
        apply_chargen_formulas(&spec, &mut sheet);
        assert_eq!(sheet["stats"]["strength"], json!(14), "rank A -> 14 (case-insensitive)");
        assert_eq!(sheet["resources"]["hp_max"], json!(140), "STR 14 * Saber coef 10");
    }
```

- [ ] **Step 2: Run to verify they fail, then (they should pass on the already-built primitives) confirm**

Run: `cargo test -p trpg-runtime cyberpunk_role_rank_track_feeds_derived fate_servant_ordinal_grade_resolves_via_categorical_lookup`
Expected: PASS (Tasks 1-3 already provide tracks plumbing + categorical lookup is pre-existing). If FAIL, the failure pinpoints a gap in the earlier tasks to fix before proceeding.

- [ ] **Step 3: Full workspace test sweep**

Run: `cargo test -p trpg-formula -p trpg-runtime 2>&1 | grep -E "test result:|FAILED|error\["`
Expected: all `test result: ok.`, no FAILED.

- [ ] **Step 4: Checkpoint**

Run: `cargo build --workspace 2>&1 | tail -3`
Expected: `Finished`.

---

## Spec coverage check

| Spec §7 in-scope item | Task |
|---|---|
| Track in sheet_json `tracks`, value = number\|ordinal label | T2 (forward) + T3 (mutate) + T7 (ordinal) |
| `build_chargen_inputs` forwards tracks; eval/Parser `+tracks` | T1 (thread) + T2 (forward) |
| `sum_tracks/max_tracks(kind)` | T1 |
| ordinal label→value via `lookup_cat` (reuse) | T7 (test; no new code) |
| `apply_track_change` (CLI + GM tool) | T3 (primitive) + T5 (CLI) + T6 (GM tool) |
| track-keyed §4 derived + override-wins (reuse) | T4 |
| Invariant: accumulators base/once, not live | T3 (accumulator test) |
| No-hardcode / fail-closed / file size | enforced per task (generic fns, provisional-on-missing, small edits) |

Out-of-scope (spec §7): XP→level/IP-cost enforcement, multiclass prereqs, ladder step/compare op + Fate +/-, choice-points, set-valued progression, lookup-table-selection-by-track, auto no-regress-on-reparse — none planned (correct).

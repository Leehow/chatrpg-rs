//! `trpg-formula` — pure, source-backed chargen formula evaluator.
//!
//! Operates on DATA (chargen value records as JSON, see the design doc §4) +
//! the player/LLM-filled inputs. No DB, no per-ruleset branching, no `eval()`.
//! Guardrails: missing tokens / cycles degrade a record to `provisional` with an
//! `unresolved` list — NEVER fabricates a value and NEVER hard-fails chargen.
mod expr;
pub use expr::{extract_refs, Num};

use std::collections::{HashMap, HashSet};
use serde_json::{json, Map, Value};

/// One evaluated chargen value (derived or hybrid).
#[derive(Debug, Clone)]
pub struct EvalResult {
    pub id: String,
    pub role: String,
    pub recompute: String,
    pub result_type: String,
    /// Number (int/float) or a terminal dice string; None when unresolved.
    pub value: Option<Value>,
    pub breakdown: Value,
    pub unresolved: Vec<String>,
    pub status: String,
    pub source_ref: Value,
}

#[derive(Debug, Clone, Default)]
pub struct ChargenReport {
    pub values: Vec<EvalResult>,
    pub gaps: Vec<String>,
}

fn s(v: &Value, k: &str) -> String { v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string() }
fn key(id: &str) -> String { id.trim().to_ascii_lowercase() }

/// Build the resolution context from the player/LLM inputs (stats, skills, and
/// namespaced `player.*` allocation inputs). Keys are lowercased (case-insensitive).
fn ctx_from_inputs(inputs: &Map<String, Value>) -> HashMap<String, Num> {
    let mut ctx = HashMap::new();
    fn walk(prefix: &str, v: &Value, ctx: &mut HashMap<String, Num>) {
        match v {
            Value::Number(n) => { if let Some(f) = n.as_f64() { ctx.insert(prefix.to_string(), Num::N(f)); } }
            Value::String(sv) => {
                let t = sv.trim();
                if let Ok(f) = t.parse::<f64>() { ctx.insert(prefix.to_string(), Num::N(f)); }
                else if !t.is_empty() { ctx.insert(prefix.to_string(), Num::Str(t.to_string())); }
            }
            Value::Object(m) => { for (k, vv) in m { let p = if prefix.is_empty() { key(k) } else { format!("{}.{}", prefix, key(k)) }; walk(&p, vv, ctx); } }
            _ => {}
        }
    }
    walk("", &Value::Object(inputs.clone()), &mut ctx);
    ctx
}

/// Exprs that a record evaluates (derived `expr`, or hybrid segments).
fn record_exprs(r: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(e) = r.get("expr").and_then(|v| v.as_str()) { out.push(e.to_string()); }
    if let Some(e) = r.get("attr_derived").and_then(|v| v.as_str()) { out.push(e.to_string()); }
    if let Some(allocs) = r.get("allocations").and_then(|v| v.as_array()) {
        for a in allocs { if let Some(e) = a.get("input").and_then(|v| v.as_str()) { out.push(e.to_string()); } }
    }
    out
}

/// Topologically order records so a record is evaluated after the records it
/// depends on. Refs that aren't record ids are inputs (resolved from ctx).
/// Returns (ordered indices, cycle_ids). On a cycle, the involved records are
/// reported as gaps (never an infinite loop).
fn topo(records: &[Value]) -> (Vec<usize>, Vec<String>) {
    let id_of: HashMap<String, usize> = records.iter().enumerate()
        .map(|(i, r)| (key(&s(r, "id")), i)).collect();
    let deps: Vec<Vec<usize>> = records.iter().map(|r| {
        let mut d = HashSet::new();
        for e in record_exprs(r) {
            for refr in expr::extract_refs(&e) {
                let k = key(refr.trim_start_matches("derived."));
                if let Some(&j) = id_of.get(&k) { d.insert(j); }
            }
        }
        d.into_iter().collect()
    }).collect();
    let mut state = vec![0u8; records.len()]; // 0=unseen 1=onstack 2=done
    let mut order = Vec::new();
    let mut cycles = Vec::new();
    fn dfs(i: usize, deps: &[Vec<usize>], state: &mut [u8], order: &mut Vec<usize>, cyc: &mut Vec<usize>) {
        state[i] = 1;
        for &j in &deps[i] {
            match state[j] { 0 => dfs(j, deps, state, order, cyc), 1 => cyc.push(j), _ => {} }
        }
        state[i] = 2;
        order.push(i);
    }
    let mut cyc_idx = Vec::new();
    for i in 0..records.len() { if state[i] == 0 { dfs(i, &deps, &mut state, &mut order, &mut cyc_idx); } }
    for i in cyc_idx { cycles.push(s(&records[i], "id")); }
    (order, cycles)
}

fn as_int_or_dice(n: &Num, result_type: &str) -> Value {
    match n {
        Num::Dice(d) => json!(d),
        Num::Str(s) => json!(s),
        Num::N(f) => if result_type == "float" { json!(f) } else { json!(f.floor() as i64) },
    }
}

/// Evaluate a full chargen spec against the inputs. Each derived/hybrid record
/// yields an EvalResult; `input_kind=player` records are skipped (their value is
/// the input). Records are evaluated in dependency order; later records can read
/// earlier results via `{{id}}` / `{{derived.id}}`.
pub fn evaluate_chargen(records: &[Value], inputs: &Map<String, Value>) -> ChargenReport {
    let mut ctx = ctx_from_inputs(inputs);
    let tracks = inputs.get("tracks").cloned().unwrap_or_else(|| json!({}));
    let (order, cyc) = topo(records);
    let in_cycle: HashSet<String> = cyc.iter().map(|c| key(c)).collect();
    let mut report = ChargenReport::default();
    report.gaps = cyc.iter().map(|c| format!("dependency cycle at `{c}`")).collect();

    for idx in order {
        let r = &records[idx];
        let id = s(r, "id");
        if id.trim().is_empty() { continue; }
        let kind = s(r, "input_kind");
        if kind == "player" { continue; }
        let role = s(r, "role");
        let recompute = if s(r, "recompute").is_empty() { "live".into() } else { s(r, "recompute") };
        let result_type = if s(r, "result_type").is_empty() { "int".into() } else { s(r, "result_type") };
        let tables = r.get("lookup_tables").cloned().unwrap_or_else(|| json!({}));
        let declared_status = if s(r, "status").is_empty() { "source_backed".into() } else { s(r, "status") };
        let source_ref = r.get("source_ref").cloned().unwrap_or(Value::Null);

        // Completeness (guardrail 1): only the REQUIRED segments must resolve —
        // `expr` (derived) / `attr_derived` (hybrid). Missing player allocations
        // default to 0 (design §5), so they do NOT make the record provisional.
        let required: Vec<String> = r.get("expr").and_then(|v| v.as_str()).map(str::to_string).into_iter()
            .chain(r.get("attr_derived").and_then(|v| v.as_str()).map(str::to_string))
            .collect();
        let mut unresolved: Vec<String> = Vec::new();
        for e in &required {
            for refr in expr::extract_refs(e) {
                let k = key(refr.trim_start_matches("derived."));
                if !ctx.contains_key(&k) && !ctx.contains_key(&key(&refr)) { unresolved.push(refr); }
            }
        }
        unresolved.sort(); unresolved.dedup();

        let mk = |value: Option<Value>, breakdown: Value, unresolved: Vec<String>| {
            let status = if !unresolved.is_empty() || in_cycle.contains(&key(&id)) { "provisional".to_string() } else { declared_status.clone() };
            EvalResult { id: id.clone(), role: role.clone(), recompute: recompute.clone(), result_type: result_type.clone(), value, breakdown, unresolved, status, source_ref: source_ref.clone() }
        };

        if !unresolved.is_empty() || in_cycle.contains(&key(&id)) {
            report.values.push(mk(None, json!({"unresolved": unresolved.clone()}), unresolved));
            continue;
        }

        // Resolve refs (alias normalization): copy lowercased + derived. keys so eval finds them.
        let eval_ctx = resolve_ctx(&ctx);
        let res = if r.get("expr").is_some() {
            eval_one(&s(r, "expr"), &eval_ctx, &tables, &tracks).map(|n| (as_int_or_dice(&n, &result_type), json!({"final": as_int_or_dice(&n, &result_type)}), n))
        } else {
            eval_hybrid(r, &eval_ctx, &tables, &result_type, &tracks)
        };

        match res {
            Ok((mut val, breakdown, num)) => {
                val = clamp_value(val, r, &eval_ctx);
                // Record into ctx for dependents (numeric only; dice is terminal).
                if let Num::N(_) = num { if let Some(f) = val.as_f64() { ctx.insert(key(&id), Num::N(f)); ctx.insert(format!("derived.{}", key(&id)), Num::N(f)); } }
                report.values.push(mk(Some(val), breakdown, vec![]));
            }
            Err(e) => report.values.push(mk(None, json!({"error": e}), vec![])),
        }
    }
    report
}

/// Make a flat context that also exposes each id under `derived.<id>`.
fn resolve_ctx(ctx: &HashMap<String, Num>) -> HashMap<String, Num> {
    let mut m = ctx.clone();
    for (k, v) in ctx.iter() { if !k.contains('.') { m.entry(format!("derived.{k}")).or_insert_with(|| v.clone()); } }
    m
}

fn eval_one(e: &str, ctx: &HashMap<String, Num>, tables: &Value, tracks: &Value) -> Result<Num, String> { expr::eval(e, ctx, tables, tracks) }

fn eval_hybrid(r: &Value, ctx: &HashMap<String, Num>, tables: &Value, result_type: &str, tracks: &Value) -> Result<(Value, Value, Num), String> {
    let attr = match r.get("attr_derived").and_then(|v| v.as_str()) { Some(e) => eval_one(e, ctx, tables, tracks)?.num_or_err()?, None => 0.0 };
    let base = r.get("base").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let mut alloc_bd = Vec::new();
    let mut alloc_sum = 0.0;
    if let Some(allocs) = r.get("allocations").and_then(|v| v.as_array()) {
        for a in allocs {
            let v = match a.get("input").and_then(|x| x.as_str()) { Some(e) => eval_one(e, ctx, tables, tracks).and_then(|n| n.num_or_err()).unwrap_or(0.0), None => 0.0 };
            alloc_sum += v;
            alloc_bd.push(json!({"source": a.get("source").cloned().unwrap_or(Value::Null), "value": v}));
        }
    }
    let total = attr + base + alloc_sum;
    let final_v = if result_type == "float" { json!(total) } else { json!(total.floor() as i64) };
    let breakdown = json!({"attr_derived": attr, "base": base, "allocations": alloc_bd, "final": final_v});
    Ok((final_v, breakdown, Num::N(total)))
}

fn clamp_value(val: Value, r: &Value, ctx: &HashMap<String, Num>) -> Value {
    let Some(mut f) = val.as_f64() else { return val };
    let resolve = |x: &Value| -> Option<f64> { x.as_f64().or_else(|| x.as_str().and_then(|s2| ctx.get(&key(s2)).and_then(|n| if let Num::N(v) = n { Some(*v) } else { None }))) };
    if let Some(mx) = r.get("max").and_then(resolve) { if f > mx { f = mx; } }
    if let Some(mn) = r.get("min").and_then(resolve) { if f < mn { f = mn; } }
    if let Some(cm) = r.get("clamp_max").and_then(resolve) { if f > cm { f = cm; } }
    if val.is_i64() || val.is_u64() { json!(f as i64) } else { json!(f) }
}

impl Num { fn num_or_err(&self) -> Result<f64, String> { match self { Num::N(n) => Ok(*n), Num::Dice(d) => Err(format!("dice `{d}` in hybrid segment")), Num::Str(s) => Err(format!("categorical `{s}` in hybrid segment")) } } }

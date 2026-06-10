//! Pure expression engine for chargen formulas. Lexer + recursive-descent
//! parser + AST evaluator — NO `eval()` (injection-safe). `{{token}}` is lexed
//! as ONE atomic ref (so substring collisions can't happen). Only GENERIC
//! operators (arithmetic / floor / ceil / round / min / max / lookup / if);
//! no per-ruleset logic. A value is a number OR a terminal dice string (from a
//! lookup hit); dice flowing into arithmetic is an error (dice is terminal).
use std::collections::HashMap;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum Num {
    N(f64),
    /// A terminal dice expression from a lookup hit (e.g. "1d4").
    Dice(String),
    /// A categorical/string value (e.g. a class id "wizard") — usable ONLY as a
    /// lookup key, never in arithmetic. Lets formulas key a table on a choice.
    Str(String),
}

impl Num {
    fn num(&self) -> Result<f64, String> {
        match self {
            Num::N(n) => Ok(*n),
            Num::Dice(d) => Err(format!("dice value `{d}` cannot be used in arithmetic")),
            Num::Str(s) => Err(format!("categorical value `{s}` cannot be used in arithmetic")),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok { Num(f64), Ref(String), Ident(String), Op(String), LParen, RParen, Comma }

fn lex(s: &str) -> Result<Vec<Tok>, String> {
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i];
        if c.is_whitespace() { i += 1; continue; }
        if c == '{' && i + 1 < b.len() && b[i + 1] == '{' {
            let mut j = i + 2;
            while j + 1 < b.len() && !(b[j] == '}' && b[j + 1] == '}') { j += 1; }
            if j + 1 >= b.len() { return Err("unterminated {{ref}}".into()); }
            let r: String = b[i + 2..j].iter().collect();
            out.push(Tok::Ref(r.trim().to_string()));
            i = j + 2;
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && i + 1 < b.len() && b[i + 1].is_ascii_digit()) {
            let mut j = i;
            while j < b.len() && (b[j].is_ascii_digit() || b[j] == '.') { j += 1; }
            let s: String = b[i..j].iter().collect();
            out.push(Tok::Num(s.parse().map_err(|_| format!("bad number `{s}`"))?));
            i = j;
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let mut j = i;
            while j < b.len() && (b[j].is_alphanumeric() || b[j] == '_') { j += 1; }
            out.push(Tok::Ident(b[i..j].iter().collect()));
            i = j;
            continue;
        }
        // A quoted string is treated as an identifier (models often quote the
        // lookup table name, e.g. lookup('damage_bonus', x)).
        if c == '\'' || c == '"' {
            let mut j = i + 1;
            while j < b.len() && b[j] != c { j += 1; }
            if j >= b.len() { return Err("unterminated string literal".into()); }
            out.push(Tok::Ident(b[i + 1..j].iter().collect()));
            i = j + 1;
            continue;
        }
        match c {
            '(' => out.push(Tok::LParen),
            ')' => out.push(Tok::RParen),
            ',' => out.push(Tok::Comma),
            '+' | '-' | '*' | '/' => out.push(Tok::Op(c.to_string())),
            '<' | '>' | '=' => {
                if i + 1 < b.len() && b[i + 1] == '=' { out.push(Tok::Op(format!("{c}="))); i += 1; }
                else if c == '=' { return Err("use == for comparison".into()); }
                else { out.push(Tok::Op(c.to_string())); }
            }
            _ => return Err(format!("unexpected char `{c}`")),
        }
        i += 1;
    }
    Ok(out)
}

struct Parser<'a> { t: &'a [Tok], i: usize, ctx: &'a HashMap<String, Num>, tables: &'a Value, tracks: &'a Value }

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Tok> { self.t.get(self.i) }
    fn next(&mut self) -> Option<Tok> { let x = self.t.get(self.i).cloned(); self.i += 1; x }

    fn expr(&mut self) -> Result<Num, String> { self.compare() }

    fn compare(&mut self) -> Result<Num, String> {
        let l = self.add()?;
        if let Some(Tok::Op(o)) = self.peek() {
            if matches!(o.as_str(), "<" | "<=" | ">" | ">=" | "==") {
                let o = o.clone(); self.i += 1;
                let r = self.add()?;
                let (a, b) = (l.num()?, r.num()?);
                let v = match o.as_str() { "<" => a < b, "<=" => a <= b, ">" => a > b, ">=" => a >= b, _ => (a - b).abs() < 1e-9 };
                return Ok(Num::N(if v { 1.0 } else { 0.0 }));
            }
        }
        Ok(l)
    }

    fn add(&mut self) -> Result<Num, String> {
        let mut l = self.mul()?;
        while let Some(Tok::Op(o)) = self.peek() {
            if o == "+" || o == "-" { let o = o.clone(); self.i += 1; let r = self.mul()?; let v = if o == "+" { l.num()? + r.num()? } else { l.num()? - r.num()? }; l = Num::N(v); } else { break; }
        }
        Ok(l)
    }

    fn mul(&mut self) -> Result<Num, String> {
        let mut l = self.unary()?;
        while let Some(Tok::Op(o)) = self.peek() {
            if o == "*" || o == "/" { let o = o.clone(); self.i += 1; let r = self.unary()?; let (a, b) = (l.num()?, r.num()?); if o == "/" && b == 0.0 { return Err("division by zero".into()); } l = Num::N(if o == "*" { a * b } else { a / b }); } else { break; }
        }
        Ok(l)
    }

    fn unary(&mut self) -> Result<Num, String> {
        if let Some(Tok::Op(o)) = self.peek() { if o == "-" { self.i += 1; return Ok(Num::N(-self.unary()?.num()?)); } }
        self.atom()
    }

    fn atom(&mut self) -> Result<Num, String> {
        match self.next() {
            Some(Tok::Num(n)) => Ok(Num::N(n)),
            Some(Tok::Ref(r)) => self.ctx.get(&r.to_ascii_lowercase()).cloned().ok_or_else(|| format!("unresolved ref {{{{{r}}}}}")),
            Some(Tok::LParen) => { let e = self.expr()?; match self.next() { Some(Tok::RParen) => Ok(e), _ => Err("expected )".into()) } }
            Some(Tok::Ident(name)) => self.call(&name),
            other => Err(format!("unexpected token {other:?}")),
        }
    }

    fn args(&mut self) -> Result<Vec<Num>, String> {
        match self.next() { Some(Tok::LParen) => {} _ => return Err("expected (".into()) }
        let mut a = Vec::new();
        if matches!(self.peek(), Some(Tok::RParen)) { self.i += 1; return Ok(a); }
        loop {
            a.push(self.expr()?);
            match self.next() { Some(Tok::Comma) => continue, Some(Tok::RParen) => break, _ => return Err("expected , or )".into()) }
        }
        Ok(a)
    }

    fn call(&mut self, name: &str) -> Result<Num, String> {
        // lookup(table_ident, key_expr) — table named by a bare ident; key numeric.
        if name == "lookup" {
            match self.next() { Some(Tok::LParen) => {} _ => return Err("expected (".into()) }
            let table = match self.next() { Some(Tok::Ident(t)) => t, other => return Err(format!("lookup() table must be a name, got {other:?}")) };
            match self.next() { Some(Tok::Comma) => {} _ => return Err("lookup() expects (table, key)".into()) }
            let key = self.expr()?;
            match self.next() { Some(Tok::RParen) => {} _ => return Err("expected )".into()) }
            return match key {
                // categorical key (e.g. a class id) → string-keyed table match
                Num::Str(s) => lookup_cat(self.tables, &table, &s),
                Num::Dice(d) => Err(format!("lookup() key cannot be a dice value `{d}`")),
                Num::N(k) => lookup(self.tables, &table, k),
            };
        }
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
        let a = self.args()?;
        let nums: Result<Vec<f64>, String> = a.iter().map(|x| x.num()).collect();
        let nums = nums?;
        let one = |f: &dyn Fn(f64) -> f64| -> Result<Num, String> { if nums.len() != 1 { return Err(format!("{name}() expects 1 arg")); } Ok(Num::N(f(nums[0]))) };
        match name {
            "floor" => one(&|x| x.floor()),
            "ceil" => one(&|x| x.ceil()),
            "round" => one(&|x| x.round()),
            "min" => nums.iter().cloned().reduce(f64::min).map(Num::N).ok_or_else(|| "min() needs args".into()),
            "max" => nums.iter().cloned().reduce(f64::max).map(Num::N).ok_or_else(|| "max() needs args".into()),
            "if" => { if nums.len() != 3 { return Err("if(cond,a,b)".into()); } Ok(Num::N(if nums[0] != 0.0 { nums[1] } else { nums[2] })) }
            _ => Err(format!("unknown function `{name}`")),
        }
    }
}

/// A range's `value`: a number, or a string that is either numeric or a terminal
/// dice expression (e.g. "1d4" -> Num::Dice).
fn range_value(r: &Value) -> Result<Num, String> {
    match r.get("value") {
        Some(Value::Number(n)) => Ok(Num::N(n.as_f64().unwrap_or(0.0))),
        Some(Value::String(s)) => match s.trim().parse::<f64>() { Ok(n) => Ok(Num::N(n)), Err(_) => Ok(Num::Dice(s.trim().to_string())) },
        _ => Err("lookup range has no value".into()),
    }
}

/// NUMERIC range lookup: `ranges[*]` = {min?, max?, value} matched by a number key.
fn lookup(tables: &Value, table: &str, key: f64) -> Result<Num, String> {
    let ranges = tables.get(table).and_then(|t| t.get("ranges")).and_then(|r| r.as_array())
        .ok_or_else(|| format!("lookup table `{table}` not found"))?;
    for r in ranges {
        let lo = r.get("min").and_then(|v| v.as_f64()).unwrap_or(f64::NEG_INFINITY);
        let hi = r.get("max").and_then(|v| v.as_f64()).unwrap_or(f64::INFINITY);
        if key >= lo && key <= hi { return range_value(r); }
    }
    Err(format!("lookup `{table}` key {key} matched no range"))
}

/// CATEGORICAL lookup: a string key (e.g. a class id "wizard") matched against a
/// range whose `key`/`min`/`max` string equals it (case-insensitive). Lets a
/// formula map a player CHOICE to a value — e.g. class -> hit die, race -> speed.
fn lookup_cat(tables: &Value, table: &str, key: &str) -> Result<Num, String> {
    let ranges = tables.get(table).and_then(|t| t.get("ranges")).and_then(|r| r.as_array())
        .ok_or_else(|| format!("lookup table `{table}` not found"))?;
    let k = key.trim().to_ascii_lowercase();
    for r in ranges {
        let hit = ["key", "min", "max"].iter().any(|f|
            r.get(*f).and_then(|v| v.as_str()).map(|s| s.trim().to_ascii_lowercase() == k).unwrap_or(false));
        if hit { return range_value(r); }
    }
    Err(format!("lookup `{table}` categorical key `{key}` matched no entry"))
}

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

/// All `{{ref}}` ids referenced in an expression (for dependency + completeness).
pub fn extract_refs(s: &str) -> Vec<String> {
    lex(s).map(|toks| toks.into_iter().filter_map(|t| if let Tok::Ref(r) = t { Some(r) } else { None }).collect()).unwrap_or_default()
}

/// Evaluate one expression against a context + lookup tables. Errors (structural
/// or unresolved ref) are returned as Err — the caller degrades to provisional.
pub fn eval(expr: &str, ctx: &HashMap<String, Num>, tables: &Value, tracks: &Value) -> Result<Num, String> {
    let toks = lex(expr)?;
    if toks.is_empty() { return Err("empty expression".into()); }
    let mut p = Parser { t: &toks, i: 0, ctx, tables, tracks };
    let v = p.expr()?;
    if p.i != toks.len() { return Err("trailing tokens".into()); }
    Ok(v)
}

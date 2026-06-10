//! Step-2 formula compiler.
//!
//! Turns a `DerivedValue.formula` string (e.g. "1d10 + REF + ranged_weapon_skill")
//! plus an actor's stat/skill profile (`mechanical_profile.stats` / `.skills`)
//! into a `roll_dice`-compatible numeric expression (e.g. "1d10+14"), plus a
//! transparent `CheckModifier` breakdown.
//!
//! This is the piece that was missing: combat hard-coded `dice_expression =
//! "1d10+0"` and never parsed `f.formula` / bound `depends_on` to actor stats.
//! `roll_dice`'s parser only accepts a single numeric modifier (`NdM[+/-K]`),
//! so the compiler SUMS the resolved named modifiers into one constant.

use serde_json::Value;
use trpg_model::{CheckModifier, DerivedValue};

#[derive(Debug, Clone)]
pub struct CompiledFormula {
    pub field_id: String,
    /// The dice term, e.g. "1d10".
    pub dice: String,
    /// roll_dice-compatible numeric expression, e.g. "1d10+14".
    pub expression: String,
    /// Per-modifier breakdown (REF=8, Handgun=6, ...).
    pub modifiers: Vec<CheckModifier>,
    /// depends_on / formula tokens that could not be bound to actor data.
    pub unresolved: Vec<String>,
}

/// Compile a single additive roll formula. Returns `None` when the formula is
/// not a simple dice roll (multiplication, max/min, pure threshold, etc.) or
/// has no dice term — those evaluators are handled elsewhere, not by a roll.
pub fn compile_formula(field_id: &str, formula: &str, stats: &Value, skills: &Value) -> Option<CompiledFormula> {
    // Keep only the attacker-side roll: drop anything from a comparison onward
    // ("... vs DV", "1d10 < BODY", "= DV", etc.).
    let head = formula
        .split(" vs")
        .next()
        .unwrap_or(formula)
        .split(['<', '>', '='])
        .next()
        .unwrap_or(formula);
    let low = head.to_ascii_lowercase();
    // Operators roll_dice can't express -> not a simple roll.
    if low.contains('*') || low.contains('/') || low.contains("max(") || low.contains("min(") {
        return None;
    }

    let mut dice: Option<String> = None;
    let mut sum: i64 = 0;
    let mut modifiers: Vec<CheckModifier> = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();

    // Split additive terms while preserving sign: turn "a - b" into "a + -b".
    for raw in head.replace('-', "+-").split('+') {
        let term = raw.trim();
        if term.is_empty() {
            continue;
        }
        let (neg, t) = match term.strip_prefix('-') {
            Some(rest) => (true, rest.trim()),
            None => (false, term),
        };
        if t.is_empty() {
            continue;
        }
        if is_dice(t) {
            if dice.is_none() {
                dice = Some(t.to_ascii_lowercase());
            }
            continue;
        }
        if let Ok(n) = t.parse::<i64>() {
            sum += if neg { -n } else { n };
            continue;
        }
        match resolve_token(t, stats, skills) {
            Some((label, val)) => {
                let v = if neg { -val } else { val };
                sum += v;
                modifiers.push(CheckModifier { label, value: v as i32, source_ref: None });
            }
            None => unresolved.push(t.to_string()),
        }
    }

    let dice = dice?;
    let expression = if sum >= 0 {
        format!("{dice}+{sum}")
    } else {
        format!("{dice}{sum}")
    };
    Some(CompiledFormula { field_id: field_id.to_string(), dice, expression, modifiers, unresolved })
}

/// Is `t` a dice term like "1d10", "d20", "2d6"?
fn is_dice(t: &str) -> bool {
    let lower = t.to_ascii_lowercase();
    let mut parts = lower.splitn(2, 'd');
    let a = match parts.next() {
        Some(a) => a,
        None => return false,
    };
    let b = match parts.next() {
        Some(b) => b,
        None => return false,
    };
    (a.is_empty() || a.chars().all(|c| c.is_ascii_digit()))
        && !b.is_empty()
        && b.chars().all(|c| c.is_ascii_digit())
}

fn normalize(s: &str) -> String {
    let n = s.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    n.strip_prefix("relevant_").map(|x| x.to_string()).unwrap_or(n)
}

/// Look up a named stat/skill in actor data, then fall back to generic-role
/// aliases (e.g. "ranged_weapon_skill" -> the first ranged weapon skill present).
pub fn resolve_token(token: &str, stats: &Value, skills: &Value) -> Option<(String, i64)> {
    let norm = normalize(token);
    if let Some(v) = lookup(stats, &norm) {
        return Some((token.trim().to_string(), v));
    }
    if let Some(v) = lookup(skills, &norm) {
        return Some((token.trim().to_string(), v));
    }
    const RANGED: &[&str] = &["Handgun", "Shoulder Arms", "Heavy Weapons", "Autofire", "Archery"];
    const MELEE: &[&str] = &["Brawling", "Melee Weapon", "Martial Arts"];
    let aliases: &[&str] = match norm.as_str() {
        "ranged_weapon_skill" | "weapon_skill" | "ranged_skill" => RANGED,
        "melee_attack_skill" | "melee_skill" => MELEE,
        _ => &[],
    };
    for cand in aliases {
        if let Some(v) = lookup(skills, &normalize(cand)) {
            return Some(((*cand).to_string(), v));
        }
    }
    None
}

/// Defender DV for an attack (the other half of step-2: makes hit/miss
/// resolve mechanically as `total >= DV` instead of narrator-decided).
///
/// Cyberpunk RED ranged attacks compare the attack total against a DV
/// determined by a Range table. That grid table parses poorly from the PDF
/// (unlike the resolution formulas), so we look up a source-backed DV entry
/// stored in the formula pack as a `DerivedValue` with field_id
/// `ranged_attack_dv.<bracket>` (or `attack_dv.<bracket>` / `dv.ranged.<bracket>`)
/// whose `formula` is the integer DV. Range-aware: once the full range table is
/// extracted, additional brackets plug straight in. Returns `(dv, label)`.
pub fn resolve_attack_dv(formulas: &[DerivedValue], bracket: &str) -> Option<(i32, String)> {
    let b = bracket.trim().to_ascii_lowercase();
    let want = [
        format!("ranged_attack_dv.{b}"),
        format!("attack_dv.{b}"),
        format!("dv.ranged.{b}"),
    ];
    for f in formulas {
        let fid = f.field_id.to_ascii_lowercase();
        if want.iter().any(|w| *w == fid) {
            if let Ok(dv) = f.formula.trim().parse::<i32>() {
                let label = f
                    .notes
                    .clone()
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| format!("source-backed ranged DV ({b})"));
                return Some((dv, label));
            }
        }
    }
    None
}

fn lookup(obj: &Value, norm: &str) -> Option<i64> {
    let map = obj.as_object()?;
    for (k, v) in map {
        if normalize(k) == norm {
            return v.as_i64().or_else(|| v.as_f64().map(|f| f as i64));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn solo() -> (Value, Value) {
        (
            json!({"INT":6,"REF":8,"DEX":7,"BODY":7,"COOL":6,"WILL":6,"MOVE":6}),
            json!({"Handgun":6,"Brawling":5,"Evasion":4,"Perception":5}),
        )
    }

    #[test]
    fn ranged_attack_binds_ref_and_weapon_skill() {
        let (s, k) = solo();
        let c = compile_formula("ranged_attack_roll", "1d10 + REF + ranged_weapon_skill", &s, &k).unwrap();
        assert_eq!(c.dice, "1d10");
        assert_eq!(c.expression, "1d10+14"); // REF 8 + Handgun 6
        assert!(c.unresolved.is_empty());
        let labels: Vec<_> = c.modifiers.iter().map(|m| (m.label.as_str(), m.value)).collect();
        assert!(labels.contains(&("REF", 8)));
        assert!(labels.contains(&("Handgun", 6)));
    }

    #[test]
    fn melee_attack_binds_dex_and_brawling() {
        let (s, k) = solo();
        let c = compile_formula("melee_attack_roll", "1d10 + DEX + melee_attack_skill", &s, &k).unwrap();
        assert_eq!(c.expression, "1d10+12"); // DEX 7 + Brawling 5
    }

    #[test]
    fn evasion_defense_binds() {
        let (s, k) = solo();
        let c = compile_formula("evasion_defense_roll", "1d10 + DEX + Evasion", &s, &k).unwrap();
        assert_eq!(c.expression, "1d10+11"); // DEX 7 + Evasion 4
    }

    #[test]
    fn initiative_single_stat() {
        let (s, k) = solo();
        let c = compile_formula("initiative_value", "REF + 1d10", &s, &k).unwrap();
        assert_eq!(c.expression, "1d10+8");
    }

    #[test]
    fn drops_comparison_tail() {
        let (s, k) = solo();
        let c = compile_formula("ranged", "REF + ranged_weapon_skill + 1d10 vs Autofire_Range_Table_DV", &s, &k).unwrap();
        assert_eq!(c.expression, "1d10+14");
    }

    #[test]
    fn non_roll_formulas_return_none() {
        let (s, k) = solo();
        assert!(compile_formula("dmg", "max(0, weapon_damage - SP)", &s, &k).is_none());
        assert!(compile_formula("auto", "2d6 * min(net_hit, autofire_max)", &s, &k).is_none());
        assert!(compile_formula("thr", "BODY", &s, &k).is_none()); // no dice term
    }

    fn dv(field_id: &str, value: &str, notes: &str) -> DerivedValue {
        DerivedValue {
            field_id: field_id.into(),
            formula: value.into(),
            depends_on: vec![],
            evaluator: "static_range_dv".into(),
            notes: Some(notes.into()),
            ..Default::default()
        }
    }

    #[test]
    fn resolve_attack_dv_reads_pack_entry() {
        let pack = vec![
            dv("ranged_attack_dv.close", "13", "≤6m, source-anchored RED close DV"),
            dv("ranged_attack_dv.medium", "15", "7-12m"),
        ];
        let (v, label) = resolve_attack_dv(&pack, "close").unwrap();
        assert_eq!(v, 13);
        assert!(label.contains("source-anchored"));
        assert_eq!(resolve_attack_dv(&pack, "medium").unwrap().0, 15);
        assert!(resolve_attack_dv(&pack, "extreme").is_none()); // no entry -> degrade, not fabricate
    }

    #[test]
    fn unresolved_tokens_recorded_not_fabricated() {
        let (s, k) = solo();
        let c = compile_formula("death_save", "1d10 + death_save_penalty", &s, &k).unwrap();
        assert_eq!(c.expression, "1d10+0"); // penalty unknown -> +0, not invented
        assert_eq!(c.unresolved, vec!["death_save_penalty".to_string()]);
    }
}

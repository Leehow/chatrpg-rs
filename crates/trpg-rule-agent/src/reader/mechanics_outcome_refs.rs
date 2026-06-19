//! On-outcome `=field` reference guard (A3-class, deterministic, fail-closed):
//! validates every `=<field>` amount/delta reference and every `when` field in
//! `kernel.resource_tracks[*].on_outcome[*]` against the engine's check-outcome
//! vocabulary (`trpg_model::outcome_fields::AMOUNT_RESOLVABLE` — the contest
//! resolver uses the same consts as its JSON keys, so the two cannot drift).
//!
//! Why this exists: the kernel extractor invents outcome fields (e.g. a
//! `derived_formulas` field_id like `chaos_generated` or `damage_after_armor`
//! referenced as `amount:"=chaos_generated"`), but `=field` resolves against
//! the contest OUTCOME JSON namespace — at runtime trpg-mechanics
//! `resolve_track_amount` returns None for an unknown field and the track
//! silently never moves. This guard mirrors that resolution statically:
//! - reference repairable by normalization (tool-doc `=<field>` placeholder
//!   copied verbatim, case/whitespace variants) -> repaired in place + message;
//! - rule still able to resolve an amount (live sibling expression, or a
//!   `default_amount` rescue on the tested-parameter path) -> KEPT + one
//!   downgrade message, never silent;
//! - rule that can never resolve an amount -> DROPPED + one message
//!   (zero behavior change: it was already a silent no-op).
//!
//! Physically split from mechanics_finalize.rs (<=400-line discipline); the
//! public path stays `mechanics_finalize::apply_on_outcome_ref_guard`.

use serde_json::{json, Value};
use trpg_model::outcome_fields::AMOUNT_RESOLVABLE;
use trpg_model::{RuleKernel, ValidationMessage};

pub fn apply_on_outcome_ref_guard(kernel: &mut RuleKernel) -> Vec<ValidationMessage> {
    let mut msgs = Vec::new();
    for track in &mut kernel.resource_tracks {
        let track_id = track
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| track.get("name").and_then(Value::as_str))
            .unwrap_or("?")
            .trim()
            .to_string();
        let Some(rules) = track.get_mut("on_outcome").and_then(Value::as_array_mut) else {
            continue;
        };
        rules.retain_mut(|rule| audit_rule(rule, &track_id, &mut msgs));
    }
    msgs
}

fn known(field: &str) -> bool {
    AMOUNT_RESOLVABLE.contains(&field)
}

/// Normal form a sloppy reference may repair into: trim + strip one `<...>`
/// wrapper (the tool-doc `=<field>` placeholder copied verbatim) + lowercase.
fn normalize(raw: &str) -> String {
    let t = raw.trim();
    let t = t
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or(t);
    t.trim().to_ascii_lowercase()
}

/// One rule's verdict (true = keep). Mirrors trpg-mechanics
/// `resolve_track_amount` branch order: amount > delta > bare when.
fn audit_rule(rule: &mut Value, track_id: &str, msgs: &mut Vec<ValidationMessage>) -> bool {
    if !rule.is_object() {
        return true; // shape problems are not this guard's verdict to make
    }
    let mut broken: Vec<String> = Vec::new();

    // `when` is always read as an outcome field name whenever it is consulted.
    let mut when_ok: Option<bool> = None; // None = absent
    if let Some(w) = rule.get("when").and_then(Value::as_str).map(str::to_string) {
        when_ok = Some(if known(&w) {
            true
        } else {
            let n = normalize(&w);
            if known(&n) {
                rule["when"] = json!(n);
                msgs.push(repaired(track_id, format!("when `{w}` -> `{n}`")));
                true
            } else {
                broken.push(format!("when:`{w}`"));
                false
            }
        });
    }

    // amount branch: "=value" reads the `when` field; "=field" reads
    // outcome.field; dice/int/max_of:/garbage are not vocabulary questions.
    let amount = rule
        .get("amount")
        .and_then(Value::as_str)
        .map(str::to_string);
    let amount_live: Option<bool> = amount.as_deref().map(|a| {
        let e = a.trim();
        if e == "=value" {
            match when_ok {
                Some(ok) => ok, // a broken `when` is already recorded above
                None => {
                    broken.push("amount:`=value` with no `when` field".into());
                    false
                }
            }
        } else if let Some(f) = e.strip_prefix('=') {
            if known(f) {
                true
            } else {
                let n = normalize(f);
                if known(&n) {
                    rule["amount"] = json!(format!("={n}"));
                    msgs.push(repaired(track_id, format!("amount `{a}` -> `={n}`")));
                    true
                } else {
                    broken.push(format!("amount:`{a}`"));
                    false
                }
            }
        } else {
            true
        }
    });

    // delta branch (legacy): the engine only supports "=value" (reads `when`)
    // and plain ints here — a `=field` delta is unsupported even for a legal
    // field, so a legal one is repaired into the amount slot when it is free;
    // otherwise the runtime falls back to the `when` field (else amount 0).
    let delta = rule
        .get("delta")
        .and_then(Value::as_str)
        .map(str::to_string);
    let delta_live: Option<bool> = delta.as_deref().map(|d| {
        let e = d.trim();
        if e == "=value" {
            match when_ok {
                Some(ok) => ok,
                None => {
                    broken.push("delta:`=value` with no `when` field".into());
                    false
                }
            }
        } else if e.starts_with('=') {
            let n = normalize(&e[1..]);
            if known(&n) && amount.is_none() {
                rule.as_object_mut().expect("checked is_object").remove("delta");
                rule["amount"] = json!(format!("={n}"));
                msgs.push(repaired(
                    track_id,
                    format!("delta `{d}` -> amount `={n}` (the engine reads `=field` via `amount` only)"),
                ));
                true
            } else {
                broken.push(format!("delta:`{d}`"));
                when_ok == Some(true)
            }
        } else {
            true // plain int, or non-`=` garbage the runtime maps to when/0
        }
    });

    // Static liveness in resolve_track_amount branch order; a dead amount
    // reference still has the default_amount rescue on the tested path.
    let has_default = rule
        .get("default_amount")
        .and_then(Value::as_str)
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let live = match (amount_live, delta_live) {
        (Some(ok), _) => ok || has_default,
        (None, Some(ok)) => ok,
        (None, None) => when_ok != Some(false),
    };

    if broken.is_empty() {
        return true;
    }
    let detail = broken.join(", ");
    let vocab = AMOUNT_RESOLVABLE.join("/");
    if live {
        let via = if amount_live == Some(false) && has_default {
            "its default_amount (tested-parameter path only)"
        } else {
            "its remaining live expression"
        };
        msgs.push(vmsg(
            "on_outcome_downgraded_unknown_outcome_field",
            track_id,
            format!("on_outcome rule references unknown outcome field(s) [{detail}] — not in the engine outcome vocabulary ({vocab}); rule kept: it still resolves via {via}"),
        ));
        true
    } else {
        msgs.push(vmsg(
            "on_outcome_dropped_unknown_outcome_field",
            track_id,
            format!("on_outcome rule dropped: it could never resolve an amount (silent no-op at runtime) — unknown outcome field(s) [{detail}], no default_amount rescue; legal vocabulary: {vocab}"),
        ));
        false
    }
}

fn repaired(track_id: &str, what: String) -> ValidationMessage {
    vmsg(
        "on_outcome_repaired_outcome_field",
        track_id,
        format!("deterministic repair: {what}"),
    )
}

fn vmsg(code: &str, track_id: &str, message: String) -> ValidationMessage {
    ValidationMessage {
        code: code.into(),
        message,
        target: Some(track_id.to_string()),
    }
}

//! Tests for the deterministic pool-scaling param compiler (Q-2 data gap).
//!
//! Proves a CATEGORICAL competency CHOICE compiles into the kernel-named numeric
//! `competency_rank` from the ruleset's enumerated option ordinal — generically,
//! deterministically, with NO per-label constant table and NO ruleset literals.
//! The end-to-end test runs the compiled record through the SAME formula
//! evaluator chargen uses and then through the SAME model helper the runtime
//! pool-scaler uses, showing a higher-rated agent gets a bigger pool.

use super::pool_scaling_choice_record;
use serde_json::{json, Value};
use trpg_model::{CharacterField, CharacterTemplate, DerivedValue};

/// A Triangle-shaped template: a `competency` CHOICE field, empty derived_values.
fn triangle_like_template() -> CharacterTemplate {
    let mut t = CharacterTemplate::default();
    t.fields = vec![CharacterField {
        field_id: "competency".into(),
        title: "Competency".into(),
        field_type: "choice".into(),
        choices_material_id: Some("competency_options".into()),
        ..Default::default()
    }];
    t
}

/// An option catalog with an enumerated `competency` (role_or_class) group, in
/// the catalog's `option_groups`/`locators` shape (what the live DB stores).
fn competency_catalog() -> Value {
    json!([{
        "catalog_id": "x.character_options.v1",
        "option_groups": [{
            "category": "competency",
            "group_id": "x.competency",
            "locators": [
                {"locator_id": "x.comp.archivist", "label": "Archivist"},
                {"locator_id": "x.comp.investigator", "label": "Investigator"},
                {"locator_id": "x.comp.handler", "label": "Handler"}
            ]
        }]
    }])
}

#[test]
fn compiles_choice_into_ordinal_rank_record() {
    let t = triangle_like_template();
    let rec = pool_scaling_choice_record("competency_rank", &t, &competency_catalog())
        .expect("a choice + catalog -> a record");
    assert_eq!(rec["id"], json!("competency_rank"));
    assert_eq!(rec["role"], json!("attribute"), "routes to sheet stats");
    assert_eq!(rec["expr"], json!("lookup(competency_rank_table,{{competency}})"));
    let ranges = rec["lookup_tables"]["competency_rank_table"]["ranges"]
        .as_array()
        .unwrap();
    // 1-based ordinal from catalog ORDER (the data source), not a constant map.
    assert_eq!(ranges.len(), 3);
    assert_eq!(ranges[0], json!({"min":"x.comp.archivist","max":"x.comp.archivist","value":1}));
    assert_eq!(ranges[2], json!({"min":"x.comp.handler","max":"x.comp.handler","value":3}));
}

#[test]
fn matches_param_by_suffix_stem_and_exact() {
    let t = triangle_like_template();
    // `competency_rank` stems to the `competency` choice.
    assert!(pool_scaling_choice_record("competency_rank", &t, &competency_catalog()).is_some());
    // exact-id param also works.
    assert!(pool_scaling_choice_record("competency", &t, &competency_catalog()).is_some());
    // a param that stems to no choice field -> None.
    assert!(pool_scaling_choice_record("nonexistent_rank", &t, &competency_catalog()).is_none());
}

#[test]
fn does_not_clobber_an_already_compiled_param() {
    let mut t = triangle_like_template();
    t.derived_values = vec![DerivedValue {
        field_id: "competency_rank".into(),
        expr: Some("{{some_source_backed}}".into()),
        ..Default::default()
    }];
    assert!(
        pool_scaling_choice_record("competency_rank", &t, &competency_catalog()).is_none(),
        "augment-not-replace: an existing derived value owns the param"
    );
}

#[test]
fn fail_soft_when_no_choice_field_or_too_few_options() {
    // No choice field at all.
    let empty = CharacterTemplate::default();
    assert!(pool_scaling_choice_record("competency_rank", &empty, &competency_catalog()).is_none());
    // Choice field exists but catalog enumerates < 2 options -> not a real scale.
    let t = triangle_like_template();
    let thin = json!([{
        "option_groups": [{"category":"competency","locators":[{"locator_id":"only_one"}]}]
    }]);
    assert!(pool_scaling_choice_record("competency_rank", &t, &thin).is_none());
    // Empty param name -> None.
    assert!(pool_scaling_choice_record("  ", &t, &competency_catalog()).is_none());
}

#[test]
fn no_ruleset_name_literals_in_record() {
    // The emitted record carries only generic terms + the (generic) field id —
    // never a ruleset/module name. Guards the no-hardcode discipline at the data level.
    let t = triangle_like_template();
    let rec = pool_scaling_choice_record("competency_rank", &t, &competency_catalog()).unwrap();
    let s = rec.to_string().to_ascii_lowercase();
    assert!(!s.contains("triangle"), "no ruleset name leaks into the record");
    assert!(!s.contains("the_vault"));
}

// --- end-to-end: compiled record -> formula eval -> stats -> model pool helper ---

/// Evaluate the compiled rank record against a sheet that PICKED a competency,
/// using the SAME evaluator path chargen uses (categorical string lookup).
fn rank_for_pick(pick: &str) -> i64 {
    let t = triangle_like_template();
    let rec = pool_scaling_choice_record("competency_rank", &t, &competency_catalog()).unwrap();
    let mut inputs = serde_json::Map::new();
    inputs.insert("competency".into(), json!(pick));
    let report = trpg_formula::evaluate_chargen(&[rec], &inputs);
    let ev = report
        .values
        .iter()
        .find(|v| v.id.eq_ignore_ascii_case("competency_rank"))
        .expect("rank evaluated");
    ev.value
        .as_ref()
        .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .expect("numeric rank")
}

#[test]
fn end_to_end_higher_competency_yields_bigger_param_driven_pool() {
    // A low-ordinal competency vs a high-ordinal competency, evaluated through the
    // real formula path, then sized through the real model pool helper.
    let low = rank_for_pick("x.comp.archivist"); // ordinal 1
    let high = rank_for_pick("x.comp.handler"); // ordinal 3
    assert_eq!(low, 1);
    assert_eq!(high, 3);
    assert!(high > low, "a later-listed competency compiles to a higher rank");

    // Now feed those ranks through the SAME helper trpg-runtime uses to scale the
    // pool. The kernel names the param + a generic scaling contract (base+per_rank).
    let dice_core = json!({
        "dice": "6d4", "compare": "count_faces", "target_face": 3,
        "pool_scaling_parameter": "competency_rank",
        "pool_base": 4, "pool_per_rank": 1
    });
    let pool_low = trpg_model::param_driven_pool_expression(&dice_core, "6d4", low).unwrap();
    let pool_high = trpg_model::param_driven_pool_expression(&dice_core, "6d4", high).unwrap();
    assert_eq!(pool_low, "5d4", "rank 1 -> base 4 + 1 = 5d4");
    assert_eq!(pool_high, "7d4", "rank 3 -> base 4 + 3 = 7d4");
    assert_ne!(
        pool_low, pool_high,
        "a more-competent agent rolls a DIFFERENT (bigger) pool, not flat 6d4"
    );
}

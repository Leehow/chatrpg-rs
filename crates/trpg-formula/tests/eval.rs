use serde_json::{json, Map, Value};
use trpg_formula::{evaluate_chargen, ChargenReport, EvalResult};

fn inputs(pairs: &[(&str, Value)]) -> Map<String, Value> {
    let mut m = Map::new();
    for (k, v) in pairs { m.insert(k.to_string(), v.clone()); }
    m
}
fn get<'a>(r: &'a ChargenReport, id: &str) -> &'a EvalResult { r.values.iter().find(|x| x.id == id).unwrap_or_else(|| panic!("no result for {id}")) }

#[test]
fn derived_hp_max() {
    let recs = vec![json!({"id":"hp_max","role":"resource_max","input_kind":"derived","expr":"floor(({{con}}+{{siz}})/10)"})];
    let rep = evaluate_chargen(&recs, &inputs(&[("con", json!(60)), ("siz", json!(60))]));
    let hp = get(&rep, "hp_max");
    assert_eq!(hp.value, Some(json!(12)), "hp_max=floor((60+60)/10)=12");
    assert_eq!(hp.status, "source_backed");
}

#[test]
fn derived_san_and_mp() {
    let recs = vec![
        json!({"id":"san_cur","role":"resource","input_kind":"derived","recompute":"once","expr":"{{pow}}"}),
        json!({"id":"mp_max","role":"resource_max","input_kind":"derived","expr":"floor({{pow}}/5)"}),
    ];
    let rep = evaluate_chargen(&recs, &inputs(&[("pow", json!(50))]));
    assert_eq!(get(&rep, "san_cur").value, Some(json!(50)));
    assert_eq!(get(&rep, "mp_max").value, Some(json!(10)));
    assert_eq!(get(&rep, "san_cur").recompute, "once");
}

#[test]
fn hybrid_dodge_with_breakdown_and_clamp() {
    let recs = vec![json!({
        "id":"dodge","role":"skill","input_kind":"hybrid","max":90,
        "attr_derived":"floor({{dex}}/2)","base":0,
        "allocations":[
            {"source":"occupation","input":"{{player.dodge.occ_pts}}"},
            {"source":"interest","input":"{{player.dodge.int_pts}}"}
        ]
    })];
    let inp = inputs(&[("dex", json!(70)), ("player", json!({"dodge":{"occ_pts":20,"int_pts":10}}))]);
    let rep = evaluate_chargen(&recs, &inp);
    let d = get(&rep, "dodge");
    assert_eq!(d.breakdown["attr_derived"], json!(35.0), "floor(70/2)=35");
    assert_eq!(d.value, Some(json!(65)), "35+0+20+10=65");
    // clamp to max=90 holds; push allocations huge -> clamps.
    let inp2 = inputs(&[("dex", json!(70)), ("player", json!({"dodge":{"occ_pts":200,"int_pts":0}}))]);
    let rep2 = evaluate_chargen(&recs, &inp2);
    assert_eq!(get(&rep2, "dodge").value, Some(json!(90)), "235 clamped to max 90");
}

#[test]
fn derived_depends_on_derived_topo() {
    // hp_cur = hp_max (declared out of order) -> topo resolves hp_max first.
    let recs = vec![
        json!({"id":"hp_cur","role":"resource","input_kind":"derived","recompute":"once","expr":"{{derived.hp_max}}"}),
        json!({"id":"hp_max","role":"resource_max","input_kind":"derived","expr":"floor(({{con}}+{{siz}})/10)"}),
    ];
    let rep = evaluate_chargen(&recs, &inputs(&[("con", json!(60)), ("siz", json!(60))]));
    assert_eq!(get(&rep, "hp_cur").value, Some(json!(12)), "hp_cur reads derived hp_max=12");
}

#[test]
fn lookup_number_and_dice() {
    let tbl = json!({"db":{"ranges":[
        {"max":64,"value":"-2"},{"min":65,"max":84,"value":"-1"},{"min":85,"max":124,"value":"0"},
        {"min":125,"max":164,"value":"1d4"},{"min":165,"max":204,"value":"1d6"}]}});
    let rec = |st: i64, si: i64| -> ChargenReport {
        let recs = vec![json!({"id":"db","role":"attribute","input_kind":"derived","result_type":"dice_or_int","expr":"lookup(db, {{str}}+{{siz}})","lookup_tables": tbl})];
        evaluate_chargen(&recs, &inputs(&[("str", json!(st)), ("siz", json!(si))]))
    };
    assert_eq!(get(&rec(60, 60), "db").value, Some(json!(0)), "STR60+SIZ60=120 -> DB 0");
    let dice = rec(80, 70); // 150 -> 1d4
    assert_eq!(get(&dice, "db").value, Some(json!("1d4")));
    assert_eq!(get(&dice, "db").result_type, "dice_or_int");
}

#[test]
fn categorical_string_keyed_lookup_class_to_hit_die() {
    // A player CHOICE (class) keys a table: class -> hit die. Case-insensitive.
    let tbl = json!({"hit_die":{"ranges":[
        {"min":"wizard","max":"wizard","value":6},
        {"min":"fighter","max":"fighter","value":10},
        {"key":"barbarian","value":12}]}});
    let recs = vec![json!({"id":"hp_max","role":"resource_max","input_kind":"derived","result_type":"int",
        "expr":"lookup(hit_die,{{class}})+{{con_mod}}","lookup_tables": tbl})];
    let rep = evaluate_chargen(&recs, &inputs(&[("class", json!("Wizard")), ("con_mod", json!(1))]));
    assert_eq!(get(&rep, "hp_max").value, Some(json!(7)), "Wizard d6=6 + con_mod 1 = 7 (case-insensitive)");
    let rep2 = evaluate_chargen(&recs, &inputs(&[("class", json!("barbarian")), ("con_mod", json!(3))]));
    assert_eq!(get(&rep2, "hp_max").value, Some(json!(15)), "barbarian d12=12 + 3 = 15");
}

#[test]
fn lookup_with_quoted_table_name_resolves() {
    // models often quote the lookup table name; the lexer must accept it.
    let tbl = json!({"damage_bonus_by_str_siz":{"ranges":[
        {"max":64,"value":"-2"},{"min":65,"max":84,"value":"-1"},{"min":85,"max":124,"value":"0"}]}});
    let recs = vec![json!({"id":"db","role":"attribute","input_kind":"derived","result_type":"dice_or_int",
        "expr":"lookup('damage_bonus_by_str_siz', {{str}}+{{siz}})","lookup_tables": tbl})];
    let rep = evaluate_chargen(&recs, &inputs(&[("str", json!(60)), ("siz", json!(60))]));
    assert_eq!(get(&rep, "db").value, Some(json!(0)), "quoted table name 'damage_bonus_by_str_siz' must resolve");
}

#[test]
fn cycle_detected_reports_gap_no_hang() {
    let recs = vec![
        json!({"id":"a","input_kind":"derived","expr":"{{derived.b}}+1"}),
        json!({"id":"b","input_kind":"derived","expr":"{{derived.a}}+1"}),
    ];
    let rep = evaluate_chargen(&recs, &Map::new());
    assert!(!rep.gaps.is_empty(), "cycle must be reported as a gap");
    assert!(rep.values.iter().all(|v| v.status == "provisional"), "cycle members are provisional, not fabricated");
}

#[test]
fn unresolved_token_degrades_not_fabricates() {
    let recs = vec![json!({"id":"hp_max","input_kind":"derived","expr":"floor(({{con}}+{{siz}})/10)"})];
    let rep = evaluate_chargen(&recs, &inputs(&[("con", json!(60))])); // siz missing
    let hp = get(&rep, "hp_max");
    assert_eq!(hp.value, None, "missing token => NO fabricated value");
    assert_eq!(hp.status, "provisional");
    assert!(hp.unresolved.iter().any(|u| u == "siz"));
}

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

#[test]
fn placeholder_no_substring_collision() {
    // {{str}} and {{strength}} are distinct atomic refs.
    let recs = vec![json!({"id":"x","input_kind":"derived","expr":"{{strength}} - {{str}}"})];
    let rep = evaluate_chargen(&recs, &inputs(&[("str", json!(5)), ("strength", json!(60))]));
    assert_eq!(get(&rep, "x").value, Some(json!(55)), "60-5=55 (no substring confusion)");
}

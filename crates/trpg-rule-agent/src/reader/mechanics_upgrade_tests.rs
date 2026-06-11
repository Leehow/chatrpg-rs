//! A4 配套升级写回的单元测试（物理拆出守 ≤400 行）。
//! 经 `#[cfg(test)] #[path = "mechanics_upgrade_tests.rs"] mod upgrade_tests;`
//! 引入，仍是 mechanics_finalize 的子模块：`use super::*` 可见性不变。
use super::*;
use serde_json::json;
use trpg_model::normalize_resource_tracks;

/// 仅 id 有效的最小 catalog（thresholds followup 引用闭合用）。
fn catalog(ids: &[&str]) -> Vec<MechanicEntry> {
    ids.iter().map(|id| MechanicEntry { id: id.to_string(), ..Default::default() }).collect()
}

fn kernel_fixture() -> RuleKernel {
    let mut k = RuleKernel::default();
    k.ruleset_id = "testgame".into();
    k.resource_tracks = vec![json!({
        "id": "sanity",
        "thresholds": [{"loss_in_one_go": 5, "consequence": "temporary insanity sets in"}]
    })];
    k.dice_core = json!({"success_bands": [{"id": "extreme", "label": "L"}]});
    k.character_sheet_schema = json!({"fields": [
        {"field_id": "sanity", "notes": "existing note"},
        {"field_id": "luck"},
        {"field_id": "mp", "notes": ""}
    ]});
    k
}

/// track sanity 既有 {loss_in_one_go:5, consequence:"…"} + upgrade
/// {track_id:"Sanity", loss_in_one_go:5, followup_procedure_id}（catalog 含该
/// id）→ threshold 多出 followup_procedure_id；consequence/loss_in_one_go
/// 原值逐字不变。
#[test]
fn thresholds_upgrade_adds_followup_without_clobbering() {
    let mut k = kernel_fixture();
    let ups = [json!({"track_id":"Sanity","loss_in_one_go":5,"followup_procedure_id":"x.temp_insanity"})];
    let msgs = apply_thresholds_upgrades(&mut k, &ups, &catalog(&["x.temp_insanity"]));
    let th = k.resource_tracks[0].pointer("/thresholds/0").unwrap();
    assert_eq!(
        th.get("followup_procedure_id").and_then(Value::as_str),
        Some("x.temp_insanity"),
        "missing key inserted"
    );
    assert_eq!(
        th.get("consequence").and_then(Value::as_str),
        Some("temporary insanity sets in"),
        "existing consequence untouched"
    );
    assert_eq!(th.get("loss_in_one_go").and_then(Value::as_i64), Some(5));
    assert!(msgs.is_empty(), "clean upgrade emits no message: {msgs:?}");
}

/// threshold 已有 followup_procedure_id → upgrade 不改它 + warning。
#[test]
fn thresholds_upgrade_never_overwrites_existing_followup() {
    let mut k = kernel_fixture();
    k.resource_tracks = vec![json!({
        "id": "sanity",
        "thresholds": [{"loss_in_one_go": 5, "followup_procedure_id": "x.original"}]
    })];
    let ups = [json!({"track_id":"sanity","loss_in_one_go":5,"followup_procedure_id":"x.other"})];
    let msgs = apply_thresholds_upgrades(&mut k, &ups, &catalog(&["x.original", "x.other"]));
    assert_eq!(
        k.resource_tracks[0].pointer("/thresholds/0/followup_procedure_id").and_then(Value::as_str),
        Some("x.original"),
        "existing followup never overwritten"
    );
    assert!(!msgs.is_empty(), "skip is loud, not silent");
}

/// followup 指向 catalog 外 id → 不写 + `threshold_followup_broken`。
#[test]
fn thresholds_upgrade_broken_ref_skipped() {
    let mut k = kernel_fixture();
    let ups = [json!({"track_id":"sanity","loss_in_one_go":5,"followup_procedure_id":"x.ghost"})];
    let msgs = apply_thresholds_upgrades(&mut k, &ups, &catalog(&["x.real"]));
    assert!(
        k.resource_tracks[0].pointer("/thresholds/0/followup_procedure_id").is_none(),
        "broken ref writes nothing"
    );
    assert!(msgs.iter().any(|m| m.code == "threshold_followup_broken"), "msgs: {msgs:?}");
}

/// track/形状匹配不到 → resource_tracks serde 字节不变 + `threshold_upgrade_unmatched`。
#[test]
fn thresholds_upgrade_no_match_no_create() {
    let mut k = kernel_fixture();
    let before = serde_json::to_string(&k.resource_tracks).unwrap();
    let ups = [
        json!({"track_id":"sanity","loss_in_one_go":7,"followup_procedure_id":"x.real"}),
        json!({"track_id":"no_such_track","at":0,"followup_procedure_id":"x.real"}),
    ];
    let msgs = apply_thresholds_upgrades(&mut k, &ups, &catalog(&["x.real"]));
    assert_eq!(
        serde_json::to_string(&k.resource_tracks).unwrap(),
        before,
        "resource_tracks serde bytes unchanged (no threshold is ever created)"
    );
    assert_eq!(
        msgs.iter().filter(|m| m.code == "threshold_upgrade_unmatched").count(),
        2,
        "msgs: {msgs:?}"
    );
}

/// 既有 {id:"extreme", label:"L"} + upgrade {id:"extreme", semantics:"S",
/// label:"EVIL"} → semantics=="S"、label 仍 "L"。
#[test]
fn bands_upgrade_fills_semantics_not_overwrite() {
    let mut k = kernel_fixture();
    let ups = [json!({"id":"extreme","semantics":"S","label":"EVIL"})];
    let _msgs = apply_success_bands_upgrades(&mut k, &ups);
    let band = k.dice_core.pointer("/success_bands/0").unwrap();
    assert_eq!(band.get("semantics").and_then(Value::as_str), Some("S"), "missing key filled");
    assert_eq!(band.get("label").and_then(Value::as_str), Some("L"), "existing key never overwritten");
}

/// upgrade 带新 id "fumble" 完整对象 → 追加成功 + info code=`success_band_completed`。
#[test]
fn bands_upgrade_appends_missing_band_with_report() {
    let mut k = kernel_fixture();
    let fumble = json!({"id":"fumble","label":"Fumble","semantics":"catastrophic backfire","rank":0});
    let msgs = apply_success_bands_upgrades(&mut k, &[fumble.clone()]);
    let bands = k.dice_core.get("success_bands").and_then(Value::as_array).unwrap();
    assert_eq!(bands.len(), 2, "new band appended");
    assert_eq!(bands[1], fumble, "appended whole object verbatim");
    assert!(
        msgs.iter().any(|m| m.code == "success_band_completed" && m.target.as_deref() == Some("fumble")),
        "msgs: {msgs:?}"
    );
}

/// notes 已有的 field 不被覆盖；缺/空串的被补；不存在的 field_id 不新建 +
/// `field_note_unmatched`。
#[test]
fn field_notes_only_fill_empty() {
    let mut k = kernel_fixture();
    let ups = [
        json!({"field_id":"Sanity","notes":"NEW"}),
        json!({"field_id":"luck","notes":"luck note"}),
        json!({"field_id":"MP","notes":"mp note"}),
        json!({"field_id":"ghost","notes":"boo"}),
    ];
    let msgs = apply_field_notes(&mut k, &ups);
    let fields = k.character_sheet_schema.get("fields").and_then(Value::as_array).unwrap();
    assert_eq!(
        fields[0].get("notes").and_then(Value::as_str),
        Some("existing note"),
        "existing notes never overwritten"
    );
    assert_eq!(fields[1].get("notes").and_then(Value::as_str), Some("luck note"), "missing notes filled");
    assert_eq!(fields[2].get("notes").and_then(Value::as_str), Some("mp note"), "empty-string notes filled");
    assert_eq!(fields.len(), 3, "no field is ever created");
    assert!(
        msgs.iter().any(|m| m.code == "field_note_unmatched" && m.target.as_deref() == Some("ghost")),
        "msgs: {msgs:?}"
    );
}

/// 三类升级全做完后 `from_value::<RuleKernel>` Ok 且 normalize_resource_tracks
/// 后 followup_procedure_id/semantics 仍在。
#[test]
fn kernel_round_trips_after_all_upgrades() {
    let mut k = kernel_fixture();
    let cat = catalog(&["x.temp_insanity"]);
    apply_thresholds_upgrades(
        &mut k,
        &[json!({"track_id":"sanity","loss_in_one_go":5,"followup_procedure_id":"x.temp_insanity"})],
        &cat,
    );
    apply_success_bands_upgrades(&mut k, &[json!({"id":"extreme","semantics":"S"})]);
    apply_field_notes(&mut k, &[json!({"field_id":"luck","notes":"luck note"})]);
    assert!(kernel_round_trip_guard(&k).is_ok(), "guard passes on an upgraded kernel");
    let v = serde_json::to_value(&k).unwrap();
    let rt: RuleKernel = serde_json::from_value(v).expect("kernel round-trips");
    let clean = normalize_resource_tracks(&rt.resource_tracks);
    assert_eq!(
        clean[0].pointer("/thresholds/0/followup_procedure_id").and_then(Value::as_str),
        Some("x.temp_insanity"),
        "followup survives round-trip + normalize"
    );
    assert_eq!(
        rt.dice_core.pointer("/success_bands/0/semantics").and_then(Value::as_str),
        Some("S"),
        "semantics survives round-trip"
    );
}

/// 三数组全缺省 → kernel serde 字节不变、零 message（老规则集行为零变化）。
/// 含 dice_core 缺 success_bands 的 kernel：空 upgrades 不整建数组。
#[test]
fn empty_upgrades_are_noop() {
    for mut k in [kernel_fixture(), {
        let mut bare = RuleKernel::default();
        bare.dice_core = json!("not an object");
        bare
    }] {
        let before = serde_json::to_string(&k).unwrap();
        let mut msgs = apply_thresholds_upgrades(&mut k, &[], &catalog(&["x.a"]));
        msgs.extend(apply_success_bands_upgrades(&mut k, &[]));
        msgs.extend(apply_field_notes(&mut k, &[]));
        assert_eq!(serde_json::to_string(&k).unwrap(), before, "kernel serde bytes unchanged");
        assert!(msgs.is_empty(), "zero messages, got: {msgs:?}");
    }
}

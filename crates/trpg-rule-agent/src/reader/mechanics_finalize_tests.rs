//! mechanics_finalize 的单元测试（物理拆出守 ≤400 行）。
//! 经 `#[cfg(test)] #[path = "mechanics_finalize_tests.rs"] mod tests;` 引入，
//! 仍是 mechanics_finalize 的子模块：`use super::*` + 私有 fn 可见性不变。
use super::*;
use serde_json::json;
use trpg_model::{expressiveness_tier, ExpressivenessTier, ProcedureStep};

fn src_ref() -> Value {
    json!({"source_id":"book","page":12,"section_path":[]})
}

fn kernel_fixture() -> RuleKernel {
    let mut k = RuleKernel::default();
    k.ruleset_id = "testgame".into();
    k.character_sheet_schema = json!({
        "fields": [{"field_id":"strength"}, {"field_id":"jump"}],
        "derived_values": [{"field_id":"hp_max"}]
    });
    k.resource_tracks = vec![json!({"id":"sanity"})];
    k
}

fn entry(id: &str) -> Value {
    json!({"id": id, "name": id, "source_refs": [src_ref()]})
}

/// （验收 3）注入 tested_parameter:"nonexistent_xyz" 的条目 → 不入目录；
/// messages 含 code=mechanic_dropped_unknown_parameter 且 target=该条目 id。
#[test]
fn bad_tested_parameter_dropped_and_reported() {
    let mut bad = entry("x.bad");
    bad["tested_parameter"] = json!("nonexistent_xyz");
    let (entries, msgs) = finalize_catalog(vec![bad, entry("x.good")], &kernel_fixture(), &[]);
    assert!(!entries.iter().any(|e| e.id == "x.bad"), "bad entry must not enter the catalog");
    assert!(entries.iter().any(|e| e.id == "x.good"));
    assert!(
        msgs.iter().any(|m| m.code == "mechanic_dropped_unknown_parameter"
            && m.target.as_deref() == Some("x.bad")),
        "msgs: {msgs:?}"
    );
}

/// "Jump"/"skills.jump" 均命中键集合里的 "jump"（fields 或 extra 来源各测一次）。
#[test]
fn tested_parameter_skills_prefix_and_case() {
    // 来源一：fields（kernel_fixture 的 fields 含 "jump"）
    let mut a = entry("x.a");
    a["tested_parameter"] = json!("Jump");
    let mut b = entry("x.b");
    b["tested_parameter"] = json!("skills.jump");
    let (entries, msgs) = finalize_catalog(vec![a.clone(), b.clone()], &kernel_fixture(), &[]);
    assert_eq!(entries.len(), 2, "both hit `jump` from fields: {msgs:?}");
    // 来源二：extra（sheet schema 空，键只来自 extra_parameter_keys）
    let mut bare = RuleKernel::default();
    bare.character_sheet_schema = json!({"title":"fallback shape"});
    let (entries, msgs) = finalize_catalog(vec![a, b], &bare, &["jump".to_string()]);
    assert_eq!(entries.len(), 2, "both hit `jump` from extra: {msgs:?}");
}

/// 纯 hook 条目（无 tested_parameter）原样保留、零 message。
#[test]
fn none_tested_parameter_is_legal() {
    let mut e = entry("x.hooked");
    e["hooks"] = json!([{"event":"scene_enter"}]);
    let (entries, msgs) = finalize_catalog(vec![e], &kernel_fixture(), &[]);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].hooks.len(), 1);
    assert!(msgs.is_empty(), "zero messages, got: {msgs:?}");
}

/// followup 指向不存在 id → 条目在、该 link 没了、code=followup_link_broken。
#[test]
fn broken_followup_link_dropped_link_kept_entry() {
    let mut e = entry("x.a");
    e["followup_links"] =
        json!([{"condition":{"kind":"outcome","value":"failure"},"procedure_id":"x.ghost"}]);
    let (entries, msgs) = finalize_catalog(vec![e], &kernel_fixture(), &[]);
    assert_eq!(entries.len(), 1, "entry kept");
    assert!(entries[0].followup_links.is_empty(), "broken link removed");
    assert!(
        msgs.iter().any(|m| m.code == "followup_link_broken" && m.target.as_deref() == Some("x.a")),
        "msgs: {msgs:?}"
    );
}

/// A→B、B→A 同批互指 → 两条全保留、links 完好（先收集后校验的顺序押此测）。
#[test]
fn intra_batch_followup_resolves() {
    let mut a = entry("x.a");
    a["followup_links"] =
        json!([{"condition":{"kind":"outcome","value":"failure"},"procedure_id":"x.b"}]);
    let mut b = entry("x.b");
    b["followup_links"] =
        json!([{"condition":{"kind":"outcome","value":"success"},"procedure_id":"x.a"}]);
    let (entries, msgs) = finalize_catalog(vec![a, b], &kernel_fixture(), &[]);
    assert_eq!(entries.len(), 2, "both kept: {msgs:?}");
    assert!(entries.iter().all(|e| e.followup_links.len() == 1), "links intact");
    assert!(msgs.is_empty(), "no messages, got: {msgs:?}");
}

/// source_refs 空 → 丢弃 + code=mechanic_dropped_no_source；同批其余合格条目不受牵连。
#[test]
fn no_source_refs_dropped() {
    let unsourced = json!({"id":"x.invented","name":"Invented","source_refs":[]});
    let (entries, msgs) = finalize_catalog(vec![unsourced, entry("x.good")], &kernel_fixture(), &[]);
    assert_eq!(entries.len(), 1, "only the sourced entry survives");
    assert_eq!(entries[0].id, "x.good");
    assert!(
        msgs.iter().any(|m| m.code == "mechanic_dropped_no_source"
            && m.target.as_deref() == Some("x.invented")),
        "msgs: {msgs:?}"
    );
}

/// hooks:[{event:"galactic_alignment"}] 且 procedure 空 → 条目保留、hooks 空、
/// code=hook_downgraded_no_engine_event、expressiveness_tier()==Semantic。
#[test]
fn unknown_hook_stripped_entry_downgraded() {
    let mut e = entry("x.cosmic");
    e["hooks"] = json!([{"event":"galactic_alignment"}]);
    let (entries, msgs) = finalize_catalog(vec![e], &kernel_fixture(), &[]);
    assert_eq!(entries.len(), 1, "entry kept");
    assert!(entries[0].hooks.is_empty(), "unknown hook stripped");
    assert!(
        msgs.iter().any(|m| m.code == "hook_downgraded_no_engine_event"
            && m.target.as_deref() == Some("x.cosmic")),
        "msgs: {msgs:?}"
    );
    assert_eq!(expressiveness_tier(&entries[0]), ExpressivenessTier::Semantic);
}

/// {"step":"weird_custom","foo":1} 入条目后 round-trip 原 JSON 一字不丢
/// （降档保留，理念护栏 §3.5）。
#[test]
fn weird_procedure_step_preserved_as_other() {
    let weird = json!({"step":"weird_custom","foo":1});
    let mut e = entry("x.weird");
    e["procedure"] = json!([weird.clone()]);
    let (entries, msgs) = finalize_catalog(vec![e], &kernel_fixture(), &[]);
    assert_eq!(entries.len(), 1, "kept: {msgs:?}");
    assert_eq!(entries[0].procedure.len(), 1);
    assert!(matches!(entries[0].procedure[0], ProcedureStep::Other(_)), "downgraded into Other");
    let round_trip = serde_json::to_value(&entries[0].procedure[0]).unwrap();
    assert_eq!(round_trip, weird, "original JSON survives byte-for-byte");
}

/// 有 id/source 但 procedure 是字符串（类型错）→ 降为语义条目保留；
/// 连 id 都缺 → 丢弃 + mechanic_uncoercible。
#[test]
fn uncoercible_salvaged_as_semantic_or_reported() {
    let mut typo = entry("x.typo");
    typo["procedure"] = json!("roll some dice"); // 类型错：字符串非数组
    typo["description"] = json!("desc kept");
    let no_id = json!({"name":"ghost","source_refs":[src_ref()]});
    let (entries, msgs) = finalize_catalog(vec![typo, no_id], &kernel_fixture(), &[]);
    let salvaged = entries.iter().find(|e| e.id == "x.typo").expect("salvaged as semantic entry");
    assert!(salvaged.procedure.is_empty(), "semantic rebuild has no procedure");
    assert_eq!(salvaged.description, "desc kept");
    assert_eq!(expressiveness_tier(salvaged), ExpressivenessTier::Semantic);
    assert_eq!(entries.len(), 1, "id-less record dropped");
    assert!(msgs.iter().any(|m| m.code == "mechanic_uncoercible" && m.target.is_none()), "msgs: {msgs:?}");
}

/// 同 id（大小写变体）重复提交 → 仅后者入目录（与 A2 merge-by-id 语义一致）。
#[test]
fn duplicate_ids_last_wins() {
    let mut first = entry("X.A");
    first["name"] = json!("first");
    let mut second = entry("x.a");
    second["name"] = json!("second");
    let (entries, _msgs) = finalize_catalog(vec![first, second], &kernel_fixture(), &[]);
    assert_eq!(entries.len(), 1, "duplicates collapse to one");
    assert_eq!(entries[0].name, "second", "the later submission wins");
}

/// sheet_parameter_keys：fields ∪ derived_values ∪ resource_tracks.id ∪ extra，
/// 全小写；fallback 形状 schema（{"title":…}）→ 仅 track/extra 键。
#[test]
fn sheet_parameter_keys_union_and_lowercase() {
    let keys = sheet_parameter_keys(&kernel_fixture(), &["Stealth".to_string()]);
    for k in ["strength", "jump", "hp_max", "sanity", "stealth"] {
        assert!(keys.contains(k), "missing {k}: {keys:?}");
    }
    let mut bare = RuleKernel::default();
    bare.character_sheet_schema = json!({"title":"fallback"});
    assert!(sheet_parameter_keys(&bare, &[]).is_empty(), "fallback schema -> empty set");
}

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

// ---- on_outcome `=field` 引用守卫（mechanics_outcome_refs）----

fn kernel_with_rules(track_id: &str, rules: Value) -> RuleKernel {
    let mut k = RuleKernel::default();
    k.ruleset_id = "testgame".into();
    k.resource_tracks = vec![json!({"id": track_id, "owner_kind": "actor", "on_outcome": rules})];
    k
}

fn track_rules(k: &RuleKernel) -> Vec<Value> {
    k.resource_tracks[0]["on_outcome"].as_array().cloned().unwrap_or_default()
}

/// 回归（triangle `=chaos_generated` 类）：发明出来的 outcome 字段引用、又无
/// default_amount 救援 → 该条规则永不可能触发，丢弃 + 上报；同轨其余规则不受牵连。
#[test]
fn invented_outcome_field_rule_dropped_and_reported() {
    let mut k = kernel_with_rules("chaos", json!([
        {"trigger":"always","op":"add","amount":"=chaos_generated"},
        {"trigger":"always","op":"add","amount":"=pool_miss_count"},
    ]));
    let msgs = apply_on_outcome_ref_guard(&mut k);
    let rules = track_rules(&k);
    assert_eq!(rules.len(), 1, "invented-field rule dropped, sibling kept: {rules:?}");
    assert_eq!(rules[0]["amount"], json!("=pool_miss_count"));
    assert!(
        msgs.iter().any(|m| m.code == "on_outcome_dropped_unknown_outcome_field"
            && m.target.as_deref() == Some("chaos")
            && m.message.contains("chaos_generated")),
        "msgs: {msgs:?}"
    );
}

/// 回归（live cyberpunk_red 第二实例）：`=damage_after_armor` 只是 derived_formulas
/// 的 field_id，从不出现在 outcome JSON → 丢弃 + 上报（运行时本就静默 no-op）。
#[test]
fn cyberpunk_damage_after_armor_shape_dropped() {
    let mut k = kernel_with_rules("hit_points", json!([
        {"op":"subtract","amount":"=damage_after_armor","trigger":"always",
         "mitigation":"armor.sp","check_match":"damage|attack"},
    ]));
    let msgs = apply_on_outcome_ref_guard(&mut k);
    assert!(track_rules(&k).is_empty(), "dead rule dropped: {:?}", track_rules(&k));
    assert!(
        msgs.iter().any(|m| m.code == "on_outcome_dropped_unknown_outcome_field"
            && m.target.as_deref() == Some("hit_points")),
        "msgs: {msgs:?}"
    );
}

/// 合法引用与非引用表达式一字不动、零 message：=field/=value+when/骰子/整数/
/// max_of:、纯 when 规则、无 on_outcome 的轨、非对象规则都安然通过。
#[test]
fn valid_refs_and_plain_amounts_untouched() {
    let rules = json!([
        {"trigger":"always","op":"add","amount":"=pool_miss_count"},
        {"trigger":"on_failure","op":"subtract","amount":"1d6"},
        {"trigger":"always","when":"success","op":"add","amount":"=value"},
        {"op":"subtract","amount":"3"},
        {"op":"subtract","amount":"max_of:1d10"},
        {"trigger":"always","op":"add","when":"pool_miss_count"},
        "not-an-object",
    ]);
    let mut k = kernel_with_rules("hp", rules.clone());
    k.resource_tracks.push(json!({"id":"bare_track"}));
    let before = k.resource_tracks.clone();
    let msgs = apply_on_outcome_ref_guard(&mut k);
    assert!(msgs.is_empty(), "zero messages, got: {msgs:?}");
    assert_eq!(k.resource_tracks, before, "tracks byte-identical");
}

/// 现役 CoC sanity 形状：坏引用 `=<sanity_loss>` 但带 default_amount → 规则保留
/// （tested 路径靠默认骰仍在工作，丢弃反而破坏现役行为），降档上报、绝不静默。
#[test]
fn broken_amount_with_default_kept_and_downgraded() {
    let mut k = kernel_with_rules("sanity", json!([
        {"trigger":"on_failure","op":"subtract","amount":"=<sanity_loss>",
         "default_amount":"1d6","check_match":"Sanity|sanity roll"},
    ]));
    let msgs = apply_on_outcome_ref_guard(&mut k);
    let rules = track_rules(&k);
    assert_eq!(rules.len(), 1, "rule kept: {msgs:?}");
    assert_eq!(rules[0]["amount"], json!("=<sanity_loss>"), "amount untouched");
    assert!(
        msgs.iter().any(|m| m.code == "on_outcome_downgraded_unknown_outcome_field"
            && m.target.as_deref() == Some("sanity")
            && m.message.contains("default_amount")),
        "msgs: {msgs:?}"
    );
}

/// 工具说明书占位符被原样照抄（`=<total>`）或大小写变体（`=Success_Count`）、
/// 内里字段确实合法 → 确定性修复为规范写法，保留 + repair 上报。
#[test]
fn placeholder_and_case_variants_repaired() {
    let mut k = kernel_with_rules("hp", json!([
        {"trigger":"always","op":"subtract","amount":"=<total>"},
        {"trigger":"always","op":"add","amount":"=Success_Count"},
    ]));
    let msgs = apply_on_outcome_ref_guard(&mut k);
    let rules = track_rules(&k);
    assert_eq!(rules.len(), 2, "both kept: {msgs:?}");
    assert_eq!(rules[0]["amount"], json!("=total"));
    assert_eq!(rules[1]["amount"], json!("=success_count"));
    assert_eq!(
        msgs.iter().filter(|m| m.code == "on_outcome_repaired_outcome_field").count(),
        2,
        "msgs: {msgs:?}"
    );
}

/// `=value` 读的是 `when` 指的 outcome 字段：when 不合法或干脆缺失、又无
/// default_amount → 死规则，丢弃；when 经修复后合法 → 保留。
#[test]
fn eq_value_requires_known_when_field() {
    let mut k = kernel_with_rules("hp", json!([
        {"trigger":"always","op":"add","amount":"=value","when":"damage_dealt"},
        {"trigger":"always","op":"add","amount":"=value"},
        {"trigger":"always","op":"add","amount":"=value","when":"Pool_Miss_Count"},
    ]));
    let msgs = apply_on_outcome_ref_guard(&mut k);
    let rules = track_rules(&k);
    assert_eq!(rules.len(), 1, "only the repairable-when rule survives: {rules:?}");
    assert_eq!(rules[0]["when"], json!("pool_miss_count"), "when repaired");
    assert_eq!(
        msgs.iter().filter(|m| m.code == "on_outcome_dropped_unknown_outcome_field").count(),
        2,
        "msgs: {msgs:?}"
    );
}

/// 纯 when 规则（无 amount/delta）引用未知字段 → 运行时 None→skip 且无默认救援，
/// 丢弃 + 上报。
#[test]
fn when_only_rule_unknown_field_dropped() {
    let mut k = kernel_with_rules("chaos", json!([
        {"trigger":"always","op":"add","when":"chaos_generated"},
    ]));
    let msgs = apply_on_outcome_ref_guard(&mut k);
    assert!(track_rules(&k).is_empty(), "dead when-only rule dropped");
    assert!(
        msgs.iter().any(|m| m.code == "on_outcome_dropped_unknown_outcome_field"),
        "msgs: {msgs:?}"
    );
}

/// 回归（live CoC hp / sword_world 形状）：`=<damage>` 占位符内字段也不合法且
/// 无默认 → 修复失败仍 drop；when 合法救不了死掉的 amount 分支（运行时 amount
/// 优先，永不回落到 when）→ 同样 drop。
#[test]
fn live_coc_hp_and_sword_world_shapes_dropped() {
    let mut k = kernel_with_rules("hp", json!([
        {"op":"subtract","amount":"=<damage>","trigger":"always"},
        {"when":"success","amount":"=damage","trigger":"always"},
    ]));
    let msgs = apply_on_outcome_ref_guard(&mut k);
    assert!(track_rules(&k).is_empty(), "both dead rules dropped: {:?}", track_rules(&k));
    assert_eq!(
        msgs.iter().filter(|m| m.code == "on_outcome_dropped_unknown_outcome_field").count(),
        2,
        "msgs: {msgs:?}"
    );
}

/// 词汇表整体回归：引擎导出的 AMOUNT_RESOLVABLE 里每个字段都被守卫接受
/// （验证器与发射端共享同一 const，杜绝两份手维护清单漂移）。
#[test]
fn engine_vocabulary_accepted_wholesale() {
    for f in trpg_model::outcome_fields::AMOUNT_RESOLVABLE {
        let mut k = kernel_with_rules("t", json!([
            {"trigger":"always","op":"add","amount": format!("={f}")},
            {"trigger":"always","op":"add","amount":"=value","when": f},
        ]));
        let msgs = apply_on_outcome_ref_guard(&mut k);
        assert!(msgs.is_empty(), "field `{f}` must be accepted: {msgs:?}");
        assert_eq!(track_rules(&k).len(), 2, "field `{f}` rules kept");
    }
}

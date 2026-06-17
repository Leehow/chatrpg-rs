//! 公式层(derived_values) ↔ 行为层(resource_tracks) 按 id 对齐。
//! audit_alignment 是纯函数脊柱：既给 parse 时记 gap，也给测试直接断言。零 per-ruleset 硬编码。
use super::chargen_compile::{self, CompileCtx};
use super::tools;
use super::units::Unit;
use serde_json::Value;
use trpg_llm::LlmClient;
use trpg_model::outcome_fields;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AlignmentReport {
    /// 成对对齐 (derived_value_id, track_id)。
    pub aligned: Vec<(String, String)>,
    /// 有资源公式 max 但没有行为 track。
    pub orphan_formulas: Vec<String>,
    /// 有行为 track 但没有对应公式 max。
    pub orphan_tracks: Vec<String>,
    /// track 在，但 on_outcome 和 thresholds 都缺。
    pub behavior_gaps: Vec<String>,
}

fn role_of(dv: &Value) -> &str { dv.get("role").and_then(Value::as_str).unwrap_or("") }
fn id_of(v: &Value) -> Option<String> {
    v.get("id").and_then(Value::as_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}
fn is_resource_role(role: &str) -> bool { role == "resource" || role == "resource_max" }

/// track 是否带任意行为配置（on_outcome 或 thresholds 非空数组）。
fn track_has_behavior(t: &Value) -> bool {
    let nonempty = |k| t.get(k).and_then(Value::as_array).map(|a| !a.is_empty()).unwrap_or(false);
    nonempty("on_outcome") || nonempty("thresholds")
}

/// 给资源派生值找对齐的 track id（数据驱动，无 per-ruleset）：
/// ① track 显式 derived_from == dv_id；② HP 走通用语义解析；③ base-id（去 `_max`）大小写不敏感匹配。
pub fn aligned_track_id(dv_id: &str, role: &str, tracks: &[Value]) -> Option<String> {
    if !is_resource_role(role) { return None; }
    let dv_lc = dv_id.trim().to_ascii_lowercase();
    let base = dv_lc.strip_suffix("_max").unwrap_or(&dv_lc).to_string();
    // ① 显式 derived_from 链接
    if let Some(t) = tracks.iter().find(|t| t.get("derived_from").and_then(Value::as_str)
        .map(|s| s.eq_ignore_ascii_case(dv_id)).unwrap_or(false)) {
        if let Some(id) = id_of(t) { return Some(id); }
    }
    // ② HP 语义桥（hp/hp_max/hit_points -> health-kind track）
    if base == "hp" || base.starts_with("hit_point") {
        if let Some(id) = trpg_model::hp_resource_track_id(tracks) { return Some(id); }
    }
    // ③ base-id 匹配（sanity->sanity, mp->mp）
    tracks.iter().filter_map(id_of).find(|id| {
        let l = id.to_ascii_lowercase();
        l == base || l.strip_suffix("_max").unwrap_or(&l) == base
    })
}

/// 纯审计：对齐情况 + 孤儿 + 行为缺口。derived_values / resource_tracks 均为 serde Value 切片。
pub fn audit_alignment(derived_values: &[Value], resource_tracks: &[Value]) -> AlignmentReport {
    let mut r = AlignmentReport::default();
    let mut matched_tracks: Vec<String> = Vec::new();
    for dv in derived_values {
        let role = role_of(dv);
        if !is_resource_role(role) { continue; }
        let Some(dv_id) = id_of(dv) else { continue };
        match aligned_track_id(&dv_id, role, resource_tracks) {
            Some(tid) => {
                r.aligned.push((dv_id, tid.clone()));
                if let Some(t) = resource_tracks.iter().find(|t| id_of(t).as_deref() == Some(tid.as_str())) {
                    if !track_has_behavior(t) && !r.behavior_gaps.contains(&tid) { r.behavior_gaps.push(tid.clone()); }
                }
                matched_tracks.push(tid);
            }
            None => r.orphan_formulas.push(dv_id),
        }
    }
    // 有 track 但没有任何资源公式指向它 -> 孤儿 track。
    for t in resource_tracks {
        if let Some(tid) = id_of(t) {
            if !matched_tracks.iter().any(|m| m.eq_ignore_ascii_case(&tid)) { r.orphan_tracks.push(tid); }
        }
    }
    r
}

/// 确定性对齐（parse 时跑，无 LLM）：补 derived_from 链接；缺 track 时建最小 stub（不臆造行为）。
/// override/现有行为永不覆盖。返回审计报告。
pub fn align(derived_values: &[Value], resource_tracks: &mut Vec<Value>) -> AlignmentReport {
    for dv in derived_values {
        let role = role_of(dv);
        if !is_resource_role(role) { continue; }
        let Some(dv_id) = id_of(dv) else { continue };
        match aligned_track_id(&dv_id, role, resource_tracks) {
            Some(tid) => {
                if let Some(t) = resource_tracks.iter_mut()
                    .find(|t| id_of(t).as_deref() == Some(tid.as_str())) {
                    if t.get("derived_from").is_none() {
                        if let Some(obj) = t.as_object_mut() {
                            obj.insert("derived_from".into(), Value::String(dv_id.clone()));
                        }
                    }
                }
            }
            None => {
                let base = dv_id.trim().to_ascii_lowercase();
                let base = base.strip_suffix("_max").unwrap_or(&base).to_string();
                resource_tracks.push(serde_json::json!({
                    "id": base, "owner_kind": "actor", "initial": 0, "derived_from": dv_id,
                }));
            }
        }
    }
    audit_alignment(derived_values, resource_tracks)
}

/// 校验 on_outcome 规则的 `=field` 引用合法（∈ AMOUNT_RESOLVABLE），非法且无 default_amount 兜底则丢弃。
pub fn validate_emitted_on_outcome(rules: &[Value]) -> Vec<Value> {
    rules.iter().filter(|r| {
        match r.get("amount").and_then(Value::as_str) {
            Some(a) if a.starts_with('=') => {
                let f = a.trim_start_matches('=');
                outcome_fields::AMOUNT_RESOLVABLE.contains(&f)
                    || r.get("default_amount").and_then(Value::as_str).is_some()
            }
            _ => true,
        }
    }).cloned().collect()
}

/// 把抽到的行为写进 track —— 仅当 track 当前缺该字段（绝不覆盖既有/override 行为）。
pub fn apply_emitted_behavior(track: &mut Value, on_outcome: &[Value], thresholds: &[Value]) {
    let Some(obj) = track.as_object_mut() else { return };
    let oo = validate_emitted_on_outcome(on_outcome);
    if !oo.is_empty() && obj.get("on_outcome").and_then(Value::as_array).map(|a| a.is_empty()).unwrap_or(true) {
        obj.insert("on_outcome".into(), Value::Array(oo));
    }
    if !thresholds.is_empty() && obj.get("thresholds").and_then(Value::as_array).map(|a| a.is_empty()).unwrap_or(true) {
        obj.insert("thresholds".into(), Value::Array(thresholds.to_vec()));
    }
}

// ---------------------------------------------------------------------------
// 行为层最优努力 LLM 补全（tertiary fallback；override 数据是主源，默认关）。
// ---------------------------------------------------------------------------

const BEHAVIOR_SYS: &str = r#"You extract a tabletop RPG resource's IN-PLAY behavior into machine records for a deterministic engine. You are given ONE resource (e.g. Hit Points, Sanity, Magic Points) and tools to read the rulebook. Read the relevant rules and submit how PLAY changes this resource:
- on_outcome: how a check/effect changes it. Each: {op:"subtract"|"add", amount, trigger:"always"|"on_failure"|"on_success", check_match:"<pipe|regex of when it applies, e.g. damage|attack or sanity|san roll>"}. `amount` is a dice string ("1d6"), an integer, or "=<field>" where <field> is ONE OF: total,target,success,success_count,pool_miss_count,success_tier_rank. If damage/loss has no standard field, give a dice fallback in `default_amount`.
- thresholds: state changes at a value or on a big single loss. Each is EITHER {at:<int>,direction:"at_or_below"|"at_or_above",consequence:"<prose>"} OR {loss_in_one_go:<int>,consequence:"<prose>"}.
Ground every record in rules you READ. If the book does not specify a behavior, OMIT it — never invent numbers or thresholds. Submit {on_outcome:[...],thresholds:[...]} (either may be empty)."#;

/// Best-effort LLM fill for behavior_gaps. GATED OFF by default (set TRPG_BEHAVIOR_ALIGN_LLM=1 to enable).
/// Override data is primary; this only fills tracks that still lack on_outcome AND thresholds.
/// Fail-closed: validates =field refs, never overwrites existing/override behavior, omits when prose is silent.
pub async fn fill_behavior_from_prose(
    client: &dyn LlmClient,
    units: &[Unit],
    sidecar_text: Option<String>,
    derived_values: &[Value],
    resource_tracks: &mut Vec<Value>,
    budget: usize,
) {
    let enabled = std::env::var("TRPG_BEHAVIOR_ALIGN_LLM")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if !enabled {
        return; // 默认关：override 数据才是主源，本路只是三级兜底。
    }
    let report = audit_alignment(derived_values, resource_tracks);
    if report.behavior_gaps.is_empty() {
        return;
    }
    let ctx = CompileCtx { units, sidecar_text, located_pages: String::new(), skill_names: Vec::new() };
    for track_id in &report.behavior_gaps {
        // 给 prompt 一个人类可读的名字：优先 track 的 name，否则 id。
        let name = resource_tracks
            .iter()
            .find(|t| t.get("id").and_then(Value::as_str) == Some(track_id.as_str()))
            .and_then(|t| t.get("name").and_then(Value::as_str))
            .unwrap_or(track_id)
            .to_string();
        let seed = format!(
            "Resource to extract behavior for: \"{name}\" (track id `{track_id}`). Read the rules for how it is lost/restored and any thresholds, then call submit_behavior."
        );
        if let Some(submitted) = run_behavior_loop(client, &seed, &ctx, budget).await {
            let oo: Vec<Value> = submitted.get("on_outcome").and_then(Value::as_array).cloned().unwrap_or_default();
            let th: Vec<Value> = submitted.get("thresholds").and_then(Value::as_array).cloned().unwrap_or_default();
            if let Some(t) = resource_tracks
                .iter_mut()
                .find(|t| t.get("id").and_then(Value::as_str) == Some(track_id.as_str()))
            {
                apply_emitted_behavior(t, &oo, &th);
            }
        }
    }
}

/// 单条 track 的行为抽取循环：复用 chargen 的导航工具 + dispatch，submit_behavior 返回整个提交对象。
async fn run_behavior_loop(
    client: &dyn LlmClient,
    seed: &str,
    ctx: &CompileCtx<'_>,
    budget: usize,
) -> Option<Value> {
    let submit = tools::submit_tool(
        "submit_behavior",
        "Submit the resource's in-play behavior records.",
        serde_json::json!({"on_outcome":{"type":"array","items":{"type":"object"}},"thresholds":{"type":"array","items":{"type":"object"}}}),
        &["on_outcome", "thresholds"],
    );
    let mut tool_schemas = tools::nav_tools();
    tool_schemas.push(submit);
    let mut msgs = vec![
        serde_json::json!({"role":"system","content":BEHAVIOR_SYS}),
        serde_json::json!({"role":"user","content":seed}),
    ];
    for _ in 0..(budget + 8) {
        let resp = client.complete_with_tools(msgs.clone(), tool_schemas.to_vec()).await.ok()?;
        let message = resp.pointer("/choices/0/message").cloned().unwrap_or_else(|| serde_json::json!({}));
        let tcs = message.get("tool_calls").and_then(Value::as_array).cloned().unwrap_or_default();
        if tcs.is_empty() {
            msgs.push(message);
            msgs.push(serde_json::json!({"role":"user","content":"Use the tools, then call submit_behavior."}));
            continue;
        }
        msgs.push(message);
        for tc in &tcs {
            let name = tc.pointer("/function/name").and_then(Value::as_str).unwrap_or("");
            let args: Value = tc
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_else(|| serde_json::json!({}));
            let id = tc.get("id").and_then(Value::as_str).unwrap_or("");
            if name == "submit_behavior" {
                return Some(args);
            }
            let out = chargen_compile::compile_dispatch(ctx, name, &args);
            msgs.push(serde_json::json!({"role":"tool","tool_call_id":id,"content":out}));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn coc_dvs() -> Vec<Value> {
        vec![
            json!({"id":"hp_max","role":"resource_max"}),
            json!({"id":"mp_max","role":"resource_max"}),
            json!({"id":"sanity","role":"resource"}),
            json!({"id":"dodge","role":"skill"}), // 非资源，应忽略
        ]
    }
    fn coc_tracks_full() -> Vec<Value> {
        vec![
            json!({"id":"hit_points","kind":"health","derived_from":"hp_max","on_outcome":[{"op":"subtract"}]}),
            json!({"id":"magic_points","derived_from":"mp_max","thresholds":[{"at":0}]}),
            json!({"id":"sanity","thresholds":[{"loss_in_one_go":5}]}),
        ]
    }

    #[test]
    fn all_resource_formulas_align_no_orphans() {
        let r = audit_alignment(&coc_dvs(), &coc_tracks_full());
        assert_eq!(r.aligned.len(), 3, "hp/mp/sanity 全对齐: {r:?}");
        assert!(r.orphan_formulas.is_empty(), "{r:?}");
        assert!(r.orphan_tracks.is_empty(), "{r:?}");
        assert!(r.behavior_gaps.is_empty(), "三 track 都有行为: {r:?}");
    }

    #[test]
    fn fail_closed_orphan_formula_and_behavior_gap() {
        let dvs = vec![json!({"id":"hp_max","role":"resource_max"}), json!({"id":"luck","role":"resource"})];
        let tracks = vec![json!({"id":"hit_points","kind":"health","derived_from":"hp_max"})];
        let r = audit_alignment(&dvs, &tracks);
        assert_eq!(r.orphan_formulas, vec!["luck".to_string()], "luck 无 track 应记孤儿: {r:?}");
        assert_eq!(r.behavior_gaps, vec!["hit_points".to_string()], "hit_points 缺行为应记 gap: {r:?}");
    }

    #[test]
    fn orphan_track_detected() {
        let dvs = vec![json!({"id":"hp_max","role":"resource_max"})];
        let tracks = vec![
            json!({"id":"hit_points","kind":"health","derived_from":"hp_max","on_outcome":[{"op":"subtract"}]}),
            json!({"id":"chaos","on_outcome":[{"op":"add"}]}),
        ];
        let r = audit_alignment(&dvs, &tracks);
        assert_eq!(r.orphan_tracks, vec!["chaos".to_string()], "{r:?}");
    }

    #[test]
    fn align_stamps_derived_from_on_existing_track() {
        let dvs = vec![json!({"id":"hp_max","role":"resource_max"})];
        let mut tracks = vec![json!({"id":"hit_points","kind":"health","on_outcome":[{"op":"subtract"}]})];
        let report = align(&dvs, &mut tracks);
        assert_eq!(tracks[0].get("derived_from").and_then(Value::as_str), Some("hp_max"),
            "align 应在 hit_points 上落 derived_from=hp_max");
        assert!(tracks[0].get("on_outcome").is_some(), "既有行为不动");
        assert_eq!(report.aligned, vec![("hp_max".to_string(), "hit_points".to_string())]);
    }

    #[test]
    fn align_creates_stub_track_without_fabricating_behavior() {
        let dvs = vec![json!({"id":"luck","role":"resource"})];
        let mut tracks: Vec<Value> = vec![];
        let report = align(&dvs, &mut tracks);
        assert_eq!(tracks.len(), 1, "应建一条 luck stub");
        assert_eq!(tracks[0].get("id").and_then(Value::as_str), Some("luck"));
        assert_eq!(tracks[0].get("derived_from").and_then(Value::as_str), Some("luck"));
        assert!(tracks[0].get("on_outcome").is_none() && tracks[0].get("thresholds").is_none(),
            "fail-closed：无源不臆造行为");
        assert_eq!(report.behavior_gaps, vec!["luck".to_string()]);
    }

    #[test]
    fn validate_emitted_behavior_drops_illegal_field_ref() {
        let raw = vec![
            json!({"op":"subtract","amount":"=total","trigger":"on_failure","check_match":"san"}),
            json!({"op":"subtract","amount":"=bogus_field","trigger":"on_failure"}),
        ];
        let ok = validate_emitted_on_outcome(&raw);
        assert_eq!(ok.len(), 1, "非法 =field 应被丢弃: {ok:?}");
        assert_eq!(ok[0].get("amount").and_then(Value::as_str), Some("=total"));
    }

    #[test]
    fn apply_emitted_behavior_never_overwrites_existing() {
        let mut t_existing = json!({"id":"sanity","on_outcome":[{"op":"subtract","amount":"=total"}]});
        apply_emitted_behavior(&mut t_existing, &[json!({"op":"add"})], &[]);
        assert_eq!(t_existing["on_outcome"].as_array().unwrap().len(), 1, "既有行为不覆盖");

        let mut t_empty = json!({"id":"luck","derived_from":"luck"});
        apply_emitted_behavior(&mut t_empty, &[json!({"op":"subtract","amount":"=total"})],
            &[json!({"at":0,"direction":"at_or_below","consequence":"out"})]);
        assert!(t_empty.get("on_outcome").is_some() && t_empty.get("thresholds").is_some());
    }
}

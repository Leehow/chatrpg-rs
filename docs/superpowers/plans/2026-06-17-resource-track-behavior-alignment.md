# 行为层 ↔ 公式层 按 id 对齐 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让公式层（`derived_values`）与行为层（`resource_tracks.on_outcome/thresholds`）按同 id 成对治理：新增聚焦对齐 pass，修复 `hp_max`→`hit_points` 种子桥，CoC/D&D 行为按 id 对齐、fail-closed、可测。

**Architecture:** 新增 `reader/behavior_align.rs`（纯函数 `audit_alignment` 脊柱 + 确定性 `align()` 落 `derived_from` + LLM best-effort 补缺，override 为主）在 chargen 第二遍之后跑；`trpg-model::match_seed` 加 `derived_from` 桥（纯加性）；runtime 消费（`apply_outcome_resource_tracks`/`detect_crossings`）已通用，仅加测试。CoC override 补 `hit_points`/`magic_points` 对齐行为；D&D 仅断言（不改数据，保持 HP 不变）。

**Tech Stack:** Rust workspace（cargo），serde_json，现有 reader 第二遍 pattern（`chargen_compile.rs`/`mechanics_compile.rs`），`LlmClient` trait。

**约束:** 每文件 ≤400 行；零 per-ruleset 硬编码；CoC SAN / D&D HP 行为不变（回归基线）。`cargo` target-dir 已在 `.cargo/config.toml` 重定向到 `~/.cache/cargo-target/...`（自动）。所有命令在 worktree 根
`/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/.claude/worktrees/claude+behavior-track-alignment` 下跑。

---

## 关键既有事实（实现者必读）

- `trpg_model::match_seed(resources, kernel_tracks)`（`crates/trpg-model/src/lib.rs:1516`）按 **track id** 去
  `resources[track_id]` 查派生种子，查不到回退 kernel 静态 `initial`/`max`。返回
  `HashMap<track_id, (Option<current>, Option<max>)>`。
- chargen 把 `role=resource/resource_max` 的派生值按其 **`id`** 存进 `sheet_json.resources[id]`
  （`crates/trpg-runtime/src/chargen.rs:241`）。CoC 公式层 id 是 `hp_max`/`mp_max`/`sanity`
  （`data/parsed/characters/call_of_cthulhu_7e.chargen.json`）。
- `trpg_model::hp_resource_track_id(tracks)`（`lib.rs:1498`）= 通用语义解析 HP track（kind=health / id 含 hit_point / id==hp）。
- `detect_crossings(track, owner_kind, owner_id, before, after, op)`（`crates/trpg-mechanics/src/watcher.rs:45`）
  已消费 `track.thresholds[*]`（`at`/`direction`/`loss_in_one_go`/`followup_procedure_id`），返回 `Vec<ThresholdCrossing>`。
- override 在 load 时按 id 增量合并（`merge_resource_tracks`，`crates/trpg-db/src/lib.rs:4380`，override 整体替换被覆盖 track）。
- 对齐接线点：`crates/trpg-parser/src/staged.rs:152`（chargen compile 之后、`persist_stage2_kernel` :172 之前；
  `rg.core.resource_tracks` + `template.derived_values` + `compiler`(LlmClient) + `self.units` 在 scope）。
  第二处：`crates/trpg-parser/src/lib.rs:~582`（同款 `reader_run_kit`/`character_template`）。

---

## Task 1: `match_seed` 的 `derived_from` 种子桥（trpg-model）

**Files:**
- Modify: `crates/trpg-model/src/lib.rs`（`match_seed`，约 1516-1532）
- Test: 同文件 `#[cfg(test)]`（已有 `coc_tracks()`/`match_seed_*` 测试，约 6984-7001）

- [ ] **Step 1: 写失败测试**（加到 `match_seed` 测试附近，约 lib.rs:7001 之后）

```rust
#[test]
fn match_seed_prefers_derived_from_link() {
    // hit_points track 显式链接到公式层 hp_max；resources 只有 hp_max 键。
    let tracks = vec![json!({"id":"hit_points","kind":"health","max":100,"initial":0,
        "owner_kind":"actor","derived_from":"hp_max"})];
    let seeds = match_seed(&json!({"hp_max": 12}), &tracks);
    assert_eq!(seeds.get("hit_points"), Some(&(Some(12), Some(12))),
        "derived_from 应把 hit_points 从 resources[hp_max] 播种");
}

#[test]
fn match_seed_without_derived_from_is_unchanged() {
    // 无 derived_from：与现状字节等价（track id 直接命中 / 回退静态）。
    let seeds = match_seed(&json!({"sanity": 65}), &coc_tracks());
    assert_eq!(seeds.get("sanity"), Some(&(Some(65), Some(65))));
    let seeds = match_seed(&json!({}), &coc_tracks());
    assert_eq!(seeds.get("hit_points"), Some(&(Some(0), Some(100))), "无种子回退 kernel 静态");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p trpg-model match_seed_prefers_derived_from_link`
Expected: FAIL（`hit_points` 此时为 `(Some(0), Some(100))`，因为 `derived_from` 没被读）

- [ ] **Step 3: 最小实现**（在 `match_seed` 的 for 循环里，把 `let derived = lookup(&id);` 改为）

```rust
        // 显式 derived_from 链接优先（修 hp_max->hit_points 种子桥），再回退 track-id 查找。
        let derived = t.get("derived_from").and_then(|v| v.as_str())
            .and_then(|df| lookup(df))
            .or_else(|| lookup(&id));
```

（其余 `out.insert(id, (derived.or(kinit), derived.or(kmax)));` 不变。纯加性：无 `derived_from` 时等价旧逻辑。）

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p trpg-model match_seed`
Expected: PASS（新 2 个 + 原有 `match_seed_*` 全绿）

- [ ] **Step 5: Commit**

```bash
git add crates/trpg-model/src/lib.rs
git commit -m "model: match_seed derived_from bridge (fix hp_max->hit_points seeding), additive

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: 对齐核心 `audit_alignment`（纯函数脊柱）

**Files:**
- Create: `crates/trpg-rule-agent/src/reader/behavior_align.rs`
- Modify: `crates/trpg-rule-agent/src/reader/mod.rs`（导出）
- Test: `behavior_align.rs` 内 `#[cfg(test)]`

- [ ] **Step 1: 建文件骨架 + 类型 + `aligned_track_id` + `audit_alignment`**（新文件）

```rust
//! 公式层(derived_values) ↔ 行为层(resource_tracks) 按 id 对齐。
//! audit_alignment 是纯函数脊柱：既给 parse 时记 gap，也给测试直接断言。零 per-ruleset 硬编码。
use serde_json::Value;

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
```

- [ ] **Step 2: 写测试**（同文件 `#[cfg(test)]`）

```rust
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
        // luck 资源公式无 track；hit_points track 缺行为(无 on_outcome/thresholds)。
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
            json!({"id":"chaos","on_outcome":[{"op":"add"}]}), // GM 经济 track，无公式
        ];
        let r = audit_alignment(&dvs, &tracks);
        assert_eq!(r.orphan_tracks, vec!["chaos".to_string()], "{r:?}");
    }
}
```

- [ ] **Step 3: 导出**（`crates/trpg-rule-agent/src/reader/mod.rs`，在其它 `pub mod`/`pub use` 旁加）

```rust
pub mod behavior_align;
pub use behavior_align::{align, audit_alignment, AlignmentReport};
```

> 注：`align` 在 Task 3 加入；若 Task 2 单独提交，先只导出 `audit_alignment`/`AlignmentReport`，Task 3 再补 `align`。

- [ ] **Step 4: 跑测试**

Run: `cargo test -p trpg-rule-agent behavior_align`
Expected: PASS（3 个测试）

- [ ] **Step 5: Commit**

```bash
git add crates/trpg-rule-agent/src/reader/behavior_align.rs crates/trpg-rule-agent/src/reader/mod.rs
git commit -m "rule-agent: audit_alignment pure core (formula<->track id alignment, fail-closed)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: 确定性 `align()` 编排（落 `derived_from`，保留既有，fail-closed）

**Files:**
- Modify: `crates/trpg-rule-agent/src/reader/behavior_align.rs`
- Test: 同文件 `#[cfg(test)]`

`align()` 在 parse 时跑：对每个资源派生值，若已有对齐 track 则**只补 `derived_from` 链接**（不动其行为）；
若无对齐 track 则按公式建一条**最小 stub track**（id=base、`derived_from`、`max`/`initial` 留给运行时派生，
**不臆造 on_outcome/thresholds**）。override/现有行为永远不被覆盖。返回 `AlignmentReport` 供日志。

- [ ] **Step 1: 写失败测试**（同文件 tests 内）

```rust
    #[test]
    fn align_stamps_derived_from_on_existing_track() {
        // hit_points 已存在但无 derived_from（base-id 对不上 hp_max）-> align 补链接，不动行为。
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
        // luck 资源公式无 track -> align 建 stub（带 derived_from），但不臆造行为。
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
```

- [ ] **Step 2: 跑确认失败**

Run: `cargo test -p trpg-rule-agent behavior_align::tests::align_`
Expected: FAIL（`align` 未定义）

- [ ] **Step 3: 实现 `align`**（加到 `behavior_align.rs`，`audit_alignment` 之后）

```rust
/// 确定性对齐（parse 时跑，无 LLM）：补 derived_from 链接；缺 track 时建最小 stub（不臆造行为）。
/// override/现有行为永不覆盖。返回审计报告。
pub fn align(derived_values: &[Value], resource_tracks: &mut Vec<Value>) -> AlignmentReport {
    for dv in derived_values {
        let role = role_of(dv);
        if !is_resource_role(role) { continue; }
        let Some(dv_id) = id_of(dv) else { continue };
        match aligned_track_id(&dv_id, role, resource_tracks) {
            Some(tid) => {
                // 在对齐 track 上补 derived_from（仅当缺失）；绝不动其行为。
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
                // 无对齐 track：建最小 stub（id=base、derived_from、owner_kind）。不臆造 on_outcome/thresholds。
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
```

- [ ] **Step 4: 跑确认通过**

Run: `cargo test -p trpg-rule-agent behavior_align`
Expected: PASS（5 个测试）

- [ ] **Step 5: Commit**

```bash
git add crates/trpg-rule-agent/src/reader/behavior_align.rs crates/trpg-rule-agent/src/reader/mod.rs
git commit -m "rule-agent: behavior_align::align stamps derived_from + stub tracks, fail-closed (no fabricated behavior)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: LLM best-effort 行为补缺（兜底，门控）

**Files:**
- Modify: `crates/trpg-rule-agent/src/reader/behavior_align.rs`（若逼近 400 行则拆 `behavior_align_llm.rs`）
- Test: 同文件 `#[cfg(test)]`（只测纯解析/校验/fail-closed，不调真 LLM）

为 `behavior_gaps` 里的 track 从 prose 抽 on_outcome/thresholds：仿 `chargen_compile` 的 submit-loop。
**门控**：仅当传入 `Some(client)` 且有 gap 时触发；env `TRPG_BEHAVIOR_ALIGN_LLM` 关闭则跳过。把抽到的
数组经 `validate_emitted_behavior` 校验（`=field` ∈ `outcome_fields::AMOUNT_RESOLVABLE`），失败丢弃该规则
（fail-closed）。本任务把**可单测的纯校验函数**做扎实；LLM 调用本身用现有 loop helper 包一层薄壳。

- [ ] **Step 1: 写失败测试**（纯校验 + 应用，不触发网络）

```rust
    #[test]
    fn validate_emitted_behavior_drops_illegal_field_ref() {
        // =bogus_field 不在 AMOUNT_RESOLVABLE -> 丢弃；=total 合法 -> 保留。
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
        // track 已有 on_outcome -> 不覆盖；空 track -> 写入。
        let mut t_existing = json!({"id":"sanity","on_outcome":[{"op":"subtract","amount":"=total"}]});
        apply_emitted_behavior(&mut t_existing, &[json!({"op":"add"})], &[]);
        assert_eq!(t_existing["on_outcome"].as_array().unwrap().len(), 1, "既有行为不覆盖");

        let mut t_empty = json!({"id":"luck","derived_from":"luck"});
        apply_emitted_behavior(&mut t_empty, &[json!({"op":"subtract","amount":"=total"})],
            &[json!({"at":0,"direction":"at_or_below","consequence":"out"})]);
        assert!(t_empty.get("on_outcome").is_some() && t_empty.get("thresholds").is_some());
    }
```

- [ ] **Step 2: 跑确认失败**

Run: `cargo test -p trpg-rule-agent behavior_align::tests::validate_emitted`
Expected: FAIL（函数未定义）

- [ ] **Step 3: 实现纯校验/应用 + LLM 薄壳**（加到 `behavior_align.rs`）

```rust
use trpg_model::outcome_fields;

/// 校验 on_outcome 规则的 `=field` 引用合法（∈ AMOUNT_RESOLVABLE），非法且无 default_amount 兜底则丢弃。
pub fn validate_emitted_on_outcome(rules: &[Value]) -> Vec<Value> {
    rules.iter().filter(|r| {
        match r.get("amount").and_then(Value::as_str) {
            Some(a) if a.starts_with('=') => {
                let f = a.trim_start_matches('=');
                outcome_fields::AMOUNT_RESOLVABLE.contains(&f)
                    || r.get("default_amount").and_then(Value::as_str).is_some()
            }
            _ => true, // 纯骰/常量/max_of: 不约束
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
```

> LLM 薄壳 `fill_behavior_from_prose(client, units, derived_values, tracks, budget)`：对 `audit_alignment`
> 的 `behavior_gaps` 逐个，用 `chargen_compile::{nav_tools, run_compile_loop}` 同款 submit-loop（submit schema
> `{on_outcome:[], thresholds:[]}`）从 prose 抽，经 `apply_emitted_behavior` 写回。env `TRPG_BEHAVIOR_ALIGN_LLM`
> （默认开）关掉则整体跳过。该壳不进单测（无确定性）；其纯子函数已由 Step 1 覆盖。实现时复用 `chargen_compile`
> 的 loop helper；若 `behavior_align.rs` >400 行，把 LLM 壳挪到 `behavior_align_llm.rs` 并在 mod.rs 导出。

- [ ] **Step 4: 跑确认通过**

Run: `cargo test -p trpg-rule-agent behavior_align`
Expected: PASS（7 个测试）

- [ ] **Step 5: Commit**

```bash
git add crates/trpg-rule-agent/src/reader/
git commit -m "rule-agent: behavior_align LLM best-effort fill (validated =field, never overwrites, gated)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: 接线 — chargen 第二遍之后调 `align`

**Files:**
- Modify: `crates/trpg-parser/src/staged.rs`（约 :151 之后）
- Modify: `crates/trpg-parser/src/lib.rs`（约 :582 之后）

- [ ] **Step 1: staged.rs 接线**（`compile_chargen_formulas` 调用 :151 之后、object schema :161 之前插入）

把 `let rg = ...` 改成 `let mut rg = ...`（:137），然后插入：

```rust
        // 2b-bis 行为层对齐：把 chargen 公式层 id 与 kernel resource_tracks 行为层对齐
        // （落 derived_from + 缺 track 建 stub + LLM best-effort 补缺，fail-closed）。
        if let Some(rg) = rg.as_mut() {
            let dvs: Vec<serde_json::Value> = template.derived_values.iter()
                .filter_map(|d| serde_json::to_value(d).ok()).collect();
            let report = reader::align(&dvs, &mut rg.core.resource_tracks);
            if !report.behavior_gaps.is_empty() {
                st.note(&format!("行为对齐缺口: {:?}", report.behavior_gaps));
                reader::fill_behavior_from_prose(compiler.as_ref(), &self.units,
                    &dvs, &mut rg.core.resource_tracks, budget).await;
            }
        }
```

> 若 Task 4 的 LLM 壳延后，本步先只调 `reader::align(...)`，`fill_behavior_from_prose` 行注释掉并留 TODO 引用 Task 4。

- [ ] **Step 2: lib.rs 接线**（`compile_chargen_formulas` 调用 :582 之后，找到 `reader_run_kit`/`character_template` 同款变量，插入等价 `reader::align(&dvs, &mut <kit>.core.resource_tracks)`）

> 实现者：先 `grep -n "compile_chargen_formulas\|resource_tracks\|reader_run_kit" crates/trpg-parser/src/lib.rs` 定位
> 约 576-602 的 scope，确认承载 resource_tracks 的可变变量名，照 Step 1 形态插入（同样 `align` 必调、LLM 选调）。

- [ ] **Step 3: 编译 + 跑解析相关测试**

Run: `cargo build -p trpg-parser`
Expected: 编译通过（无未用导入/借用错误）

Run: `cargo test -p trpg-parser`
Expected: PASS（现有 parser 测试不回归）

- [ ] **Step 4: Commit**

```bash
git add crates/trpg-parser/src/staged.rs crates/trpg-parser/src/lib.rs
git commit -m "parser: wire behavior_align after chargen compile (both staged & lib paths)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: CoC override 行为数据 + D&D 对齐断言

**Files:**
- Modify: `data/parsed/rules/call_of_cthulhu_7e.rule_kernel.override.json`（增 `resource_tracks`）
- Test: `crates/trpg-rule-agent/src/reader/behavior_align.rs`（用 CoC/D&D 形态 fixture 断言对齐，纯、不依赖 DB）

> 原则：CoC `sanity` 已在 DB（reader+SQL），**不在 override 重复定义**（避免 wholesale 替换改变 SAN 行为）。
> 只 override **缺口** track：`hit_points`（对齐 hp_max + 伤害落账 + 0/重伤阈值）、`magic_points`（对齐 mp_max）。
> D&D **不改数据**（HP 冻结），只加对齐断言测试。

- [ ] **Step 1: CoC override 增 `resource_tracks`**（在该 JSON 顶层对象加键；数值取 CoC 7e 经典 prose）

```json
  "resource_tracks": [
    {
      "id": "hit_points", "name": "Hit Points", "kind": "health", "owner_kind": "actor",
      "initial": 0, "derived_from": "hp_max", "zero_means": "dying",
      "on_outcome": [
        { "op": "subtract", "amount": "=damage", "trigger": "always", "check_match": "damage|attack" }
      ],
      "thresholds": [
        { "at": 0, "direction": "at_or_below", "consequence": "dying (major wound rules / death checks apply)" },
        { "loss_in_one_go": 0, "consequence": "major wound if a single attack inflicts >= half of max HP" }
      ]
    },
    {
      "id": "magic_points", "name": "Magic Points", "kind": "track", "owner_kind": "actor",
      "initial": 0, "derived_from": "mp_max", "zero_means": "no spellcasting"
    }
  ]
```

> 加一行 `_note_behavior_align` 说明来源（仿文件里既有的 `_note*` 习惯），如：
> `"_note_behavior_align": "2026-06-17 §10.5 behavior<->formula alignment: hit_points/magic_points derived_from links + HP damage/wound behavior. sanity left to reader+DB (frozen baseline). page unverified, canonical CoC 7e."`

- [ ] **Step 2: 写对齐断言测试**（behavior_align.rs tests，模拟 CoC/D&D 加载后形态）

```rust
    #[test]
    fn coc_override_shape_aligns_hp_and_mp() {
        // 模拟 CoC 加载后：公式层 hp_max/mp_max/sanity + override 的 hit_points/magic_points + DB 的 sanity。
        let dvs = coc_dvs();
        let tracks = vec![
            json!({"id":"hit_points","kind":"health","derived_from":"hp_max",
                   "on_outcome":[{"op":"subtract","amount":"=damage"}],"thresholds":[{"at":0}]}),
            json!({"id":"magic_points","derived_from":"mp_max","initial":0}),
            json!({"id":"sanity","on_outcome":[{"op":"subtract","amount":"=total"}],"thresholds":[{"loss_in_one_go":5}]}),
        ];
        let r = audit_alignment(&dvs, &tracks);
        assert_eq!(r.aligned.len(), 3, "{r:?}");
        assert!(r.orphan_formulas.is_empty() && r.orphan_tracks.is_empty(), "{r:?}");
        // magic_points 仅对齐无行为 -> 进 behavior_gaps（fail-closed，可被 override/LLM 后补）。
        assert_eq!(r.behavior_gaps, vec!["magic_points".to_string()], "{r:?}");
    }

    #[test]
    fn dnd_hp_aligns_by_base_id_without_data_change() {
        // D&D 公式层 hit_points_max（或 hit_points）对齐 kernel hit_points track —— base-id 桥，无需改数据。
        let dvs = vec![json!({"id":"hit_points_max","role":"resource_max"})];
        let tracks = vec![json!({"id":"hit_points","kind":"health","on_outcome":[{"op":"subtract"}]})];
        let r = audit_alignment(&dvs, &tracks);
        assert_eq!(r.aligned, vec![("hit_points_max".to_string(), "hit_points".to_string())], "{r:?}");
        assert!(r.behavior_gaps.is_empty(), "D&D HP 有行为，无 gap: {r:?}");
    }
```

- [ ] **Step 3: 校验 JSON 合法 + 跑测试**

Run: `python3 -c "import json,sys; json.load(open('data/parsed/rules/call_of_cthulhu_7e.rule_kernel.override.json')); print('OK')"`
Expected: `OK`

Run: `cargo test -p trpg-rule-agent behavior_align`
Expected: PASS（9 个测试）

- [ ] **Step 4: Commit**

```bash
git add data/parsed/rules/call_of_cthulhu_7e.rule_kernel.override.json crates/trpg-rule-agent/src/reader/behavior_align.rs
git commit -m "data+test: CoC override aligned hit_points/magic_points behavior; D&D base-id alignment assertion

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: 阈值穿越 → on_outcome 集成测试 + 全量验证

**Files:**
- Create/Modify: `crates/trpg-mechanics/src/watcher_tests.rs`（加 HP 阈值用例；SAN 已有）
- Verify: 全 workspace build + 受影响 crate test

- [ ] **Step 1: 写 HP 阈值穿越测试**（仿 `watcher_tests.rs` 既有 `san_loss_*` 用例）

```rust
fn coc_hp_track() -> serde_json::Value {
    serde_json::json!({"id":"hit_points","owner_kind":"actor","thresholds":[
        {"loss_in_one_go":0,"consequence":"major wound if a single attack inflicts >= half of max HP"},
        {"at":0,"direction":"at_or_below","consequence":"dying"}]})
}

#[test]
fn hp_drop_to_zero_crosses_dying_threshold() {
    // before=4 -> after=0, Subtract：触发 cumulative(at:0) + loss_in_one_go(0)。
    let cs = detect_crossings(&coc_hp_track(), "actor", "pc.current", 4, 0, ParameterOperation::Subtract);
    assert!(cs.iter().any(|c| c.kind == "cumulative" && c.threshold_at == Some(0)),
        "0 线濒死阈值应穿越: {cs:?}");
}
```

- [ ] **Step 2: 跑该测试**

Run: `cargo test -p trpg-mechanics hp_drop_to_zero_crosses_dying_threshold`
Expected: PASS

- [ ] **Step 3: 回归 — 现有 SAN/HP/watcher 测试全绿**

Run: `cargo test -p trpg-mechanics`
Expected: PASS（含金样 `apply_outcome_threshold_facts_unchanged`、`san_loss_*`）

- [ ] **Step 4: 全量 build + 受影响 crate test**

Run:
```bash
cargo build
cargo test -p trpg-model -p trpg-rule-agent -p trpg-mechanics -p trpg-parser
```
Expected: build 通过；测试全绿（无回归）。

> DB-gated 测试（`trpg-db/tests/live_kernel_override.rs` 的 CoC/D&D HP/SAN 基线）需 `DATABASE_URL`：
> 若环境有 CoC DB（记忆：CoC 在 :54347），运行 `DATABASE_URL=... TRPG_DATA_DIR=data cargo test -p trpg-db --test live_kernel_override`
> 确认 T3 回归；无 DB 则这些测试自动 SKIP（非阻塞）。

- [ ] **Step 5: 检查文件行数 ≤400**

Run: `wc -l crates/trpg-rule-agent/src/reader/behavior_align.rs`
Expected: ≤400（超则把 LLM 壳拆到 `behavior_align_llm.rs`）

- [ ] **Step 6: Commit**

```bash
git add crates/trpg-mechanics/
git commit -m "mechanics: HP threshold-crossing test (dying/major-wound); full alignment verification green

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review（spec 覆盖核对）

- spec §4.1 对齐核心 → Task 2（`audit_alignment`/`aligned_track_id`）✓
- spec §4.2 行为补缺（override>现有>LLM）→ Task 3（确定性 align/stub）+ Task 4（LLM）+ Task 6（override 数据）✓
- spec §4.3 校验/fail-closed → Task 2/3（不臆造）+ Task 4（`validate_emitted_on_outcome`）✓
- spec §4.4 runtime（match_seed 桥 + 阈值消费）→ Task 1 + Task 7 ✓
- spec §4.5 接线 → Task 5 ✓
- spec §6 数据（CoC/D&D）→ Task 6 ✓
- spec §7 测试 T1/T2/T3/T4 → Task 2&6(T1) / Task 7(T2) / Task 7 Step4(T3) / Task 1(T4) ✓
- 类型一致性：`AlignmentReport{aligned,orphan_formulas,orphan_tracks,behavior_gaps}`、`align`/`audit_alignment`/
  `aligned_track_id`/`validate_emitted_on_outcome`/`apply_emitted_behavior` 跨任务命名一致 ✓
- 无 placeholder（每步含真实代码/命令/期望）✓
- 硬约束：CoC SAN（不在 override 重定义）/ D&D HP（不改数据 + match_seed 加性）冻结 ✓

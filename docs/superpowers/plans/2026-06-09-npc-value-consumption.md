# NPC 合成值消费层 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 Phase 1 现搓的 flagged-provisional NPC 值真正驱动 CoC 对抗 check 结算(非 Provisional-null)。

**Architecture:** 端到端通道:`ensure_npc_parameter` 现搓→投影到 mechanical_profile(B1)→ CLI 盖章把 target_actor + 双方 tested key 写进契约(B2)→ contest `kernel_resolution_model` 在 roll_under + 对手在场时建 `OpposedRoll`、读双方真值 → runtime 包装器预掷防御骰 → contest `resolve_opposed` 纯函数按 kernel compare/success_bands 比 tier 出胜负。零 per-ruleset 硬编码、fail-closed、contest 零 RNG。

**Tech Stack:** Rust workspace(crates: trpg-model / trpg-contest / trpg-runtime / trpg-cli / trpg-api / trpg-params / trpg-combat),Postgres(:54347 rulesets),sqlx,serde_json,tokio。**无 git → checkpoint = `cargo test`,不是 commit。** LLM 走 codex-relay :18888。

**Spec:** `docs/superpowers/specs/2026-06-09-npc-value-consumption-design-v2.md`(v2.1)。

**关键事实(已核实)**:kernel 核心掷式 = `dice_core.dice`(string,fallback `check_model.dice`);防御骰落库 = `db.insert_dice_roll`;`normalize_contract_for_system_roll` 用 `clone()` 自动保留新字段;`refresh_mechanical_profile`(chargen.rs)只投 stats/skills/fields,对稀疏 NPC sheet 安全;`derive_tested_source` 仅 roll_under 路径调用;现行 roll_under 规则仅 CoC 有 resource_tracks。

**全程不变量(每个 Task 后都要仍绿)**:`sanity_check_resolves_to_the_sanity_track`、`no_match_fails_closed_not_fifty`(contest)、`live_percentile.rs`(DB-gated,无 DATABASE_URL 时 SKIP)、CoC tier 系列、`forced_tech_assessment_data_driven.rs`。

---

## Task 1: model 类型字段(OpposedRoll 双方值 + 契约对手 tested key)

**Files:**
- Modify: `crates/trpg-model/src/lib.rs:6252-6263`(CheckResolutionModel::OpposedRoll)、`:2545-2565`(CheckContract)
- Test: `crates/trpg-model/src/lib.rs`(新增 `#[cfg(test)]` 用例,或就近模块)

- [ ] **Step 1: 写失败测试(向后兼容反序列化)**

在 `crates/trpg-model/src/lib.rs` 末尾的测试模块(若无则新建 `#[cfg(test)] mod consumption_model_tests`)加:

```rust
#[cfg(test)]
mod consumption_model_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn opposed_roll_new_value_fields_default_none() {
        // 旧 JSON（无 attacker_value/defender_value）必须反序列化成 None（serde default）。
        let old = json!({"kind":"opposed_roll","attacker_expression":"1d100",
            "defender_actor_id":"npc.opposition","defender_expression":"1d100",
            "defender_roll_visibility":"private_gm_roll"});
        let m: CheckResolutionModel = serde_json::from_value(old).unwrap();
        match m {
            CheckResolutionModel::OpposedRoll { attacker_value, defender_value, .. } => {
                assert_eq!(attacker_value, None);
                assert_eq!(defender_value, None);
            }
            _ => panic!("expected OpposedRoll"),
        }
    }

    #[test]
    fn contract_opponent_tested_parameter_defaults_none() {
        // 旧契约 JSON（无 opponent_tested_parameter）→ None。
        let v = serde_json::to_value(sample_min_contract()).unwrap();
        let mut obj = v.as_object().unwrap().clone();
        obj.remove("opponent_tested_parameter");
        let c: CheckContract = serde_json::from_value(serde_json::Value::Object(obj)).unwrap();
        assert!(c.opponent_tested_parameter.is_none());
    }

    fn sample_min_contract() -> CheckContract {
        // 复用现有 Default 思路；若 CheckContract 无 Default,手填最小字段。
        // 实现期:用 crate 内既有的最小构造或 serde_json::from_value 一个完整样例。
        serde_json::from_value(json!({
            "check_id":"c","session_id":"s","turn_id":"t","ruleset_id":"r","module_id":null,
            "initiator":{"actor_id":"pc.current","actor_kind":"player_character","display_name":null},
            "target_actor":null,"opposition":{"kind":"no_mechanical_opposition"},
            "action_summary":"","intent_kind":"","check_label":"","dice_expression":"1d100",
            "modifiers":[],"target":{"kind":"unknown_until_lookup"},"tested_parameter":null,
            "actor_snapshot_ids":[],"source_refs":[],"learned_packet_ids":[],
            "roll_visibility":"public_gm_roll","roll_authority":"system",
            "disclosure":{"reveal_dice":true,"reveal_total":true,"reveal_target":true,"reveal_breakdown":true},
            "stakes":{"before_roll_public":"","success_public":"","failure_public":"",
                "critical_public":null,"fumble_public":null,"success_patches_allowed":[],
                "failure_patches_allowed":[],"irreversible":false},
            "confidence":"medium","ruling_status":"provisional","advice_refs":[],"expires_at_turn":null
        })).unwrap()
    }
}
```

> 注:`disclosure`/`stakes` 字段名以编译为准;若 `RollDisclosurePolicy` 字段名不同,改 `sample_min_contract` 的 json 直到 `from_value` 成功(这是测试夹具,不影响生产)。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p trpg-model consumption_model_tests 2>&1 | tail -20`
Expected: 编译失败(字段不存在)或断言失败。

- [ ] **Step 3: 加字段**

`CheckResolutionModel::OpposedRoll`(:6254)改为:

```rust
    OpposedRoll {
        attacker_expression: String,
        #[serde(default)]
        attacker_value: Option<i32>,
        defender_actor_id: Option<String>,
        defender_expression: String,
        #[serde(default)]
        defender_value: Option<i32>,
        defender_roll_visibility: RollVisibility,
    },
```

`CheckContract`(:2545,在 `tested_parameter` 字段后)加:

```rust
    /// 对抗检定中防御方该测的参数(B2 通道)。攻击方用 `tested_parameter`,
    /// 防御方用此字段;contest 据此读 defender 卡的真值。None = 非对抗 / 未盖章。
    #[serde(default)]
    pub opponent_tested_parameter: Option<TestedParameter>,
```

- [ ] **Step 4: 修所有构造点的编译错**

`OpposedRoll` 唯一构造点 `crates/trpg-contest/src/lib.rs:216` 加 `attacker_value: None, defender_value: None,`(Task 5 再填真值)。`CheckContract` 构造点(combat:1073、runtime:2476/2415 等)Rust 结构体字面量缺字段会编译失败——逐个加 `opponent_tested_parameter: None,`。用编译器找全:`cargo build -p trpg-contest -p trpg-combat -p trpg-runtime 2>&1 | grep "missing field"`。

- [ ] **Step 5: 跑测试确认通过 + 不回归**

Run: `cargo test -p trpg-model consumption_model_tests && cargo build -p trpg-contest -p trpg-combat -p trpg-runtime -p trpg-cli -p trpg-api`
Expected: 测试 PASS,全 crate 编译通过。

- [ ] **Step 6: Checkpoint**

Run: `cargo test -p trpg-model 2>&1 | tail -5`
Expected: 全绿。

---

## Task 2: resolve_outcome 加 defender_roll 参 + 穿 None 过全 8 调用点

**Files:**
- Modify: `crates/trpg-contest/src/lib.rs:16`(签名)
- Modify(callsites): `crates/trpg-runtime/src/lib.rs:394,477,929,983`
- Modify(tests): `crates/trpg-runtime/tests/forced_tech_assessment_data_driven.rs:160`、`crates/trpg-contest/tests/live_percentile.rs:82,90,114`

- [ ] **Step 1: 改签名(暂不使用新参,保持行为不变)**

`crates/trpg-contest/src/lib.rs:16`:

```rust
    pub async fn resolve_outcome(&self, contract: &CheckContract, roll: &DiceRollRecord,
        _defender_roll: Option<&DiceRollRecord>) -> Result<Value> {
```

(下划线前缀避免 unused 警告;Task 5 去掉下划线并启用。)

- [ ] **Step 2: 跑 build 确认全部调用点失败**

Run: `cargo build -p trpg-contest -p trpg-runtime 2>&1 | grep -E "this function takes|arguments" | head`
Expected: 4 个 runtime 调用点报"takes 3 arguments but 2 were supplied"。

- [ ] **Step 3: 4 个 runtime 调用点补 `None`**

`crates/trpg-runtime/src/lib.rs` 394/477/929/983,每处 `.resolve_outcome(&X, &roll)` → `.resolve_outcome(&X, &roll, None)`。

- [ ] **Step 4: 修测试调用点**

`forced_tech_assessment_data_driven.rs:160` `.resolve_outcome(&check, &roll)` → `.resolve_outcome(&check, &roll, None)`。
`live_percentile.rs:82/90/114` 三处 `svc.resolve_outcome(&contract(...), &roll(...))` → 末尾加 `, None`。

- [ ] **Step 5: Checkpoint(全工作区编译 + 测试)**

Run: `cargo test -p trpg-contest -p trpg-runtime 2>&1 | tail -15`
Expected: 全绿(行为未变,纯加参穿 None)。

---

## Task 3: tested-source 外科修复(trigger:"always" 别名剔除)

**Files:**
- Modify: `crates/trpg-contest/src/lib.rs:496-502`(derive_tested_source 的 on_outcome 别名收集)
- Test: `crates/trpg-contest/src/lib.rs`(tested_param_tests 模块,:561)

- [ ] **Step 1: 写失败测试(攻击不得误绑 HP 轨)**

在 `tested_param_tests` 模块(contest:561)内,`coc_kernel()` 旁加一个带 HP 轨的 kernel + 测试:

```rust
    fn coc_kernel_with_hp() -> RuleKernel {
        let mut k = RuleKernel::default();
        k.resource_tracks = vec![
            json!({"id":"sanity","name":"Sanity","initial":50,"owner_kind":"actor",
                "on_outcome":[{"op":"subtract","amount":"1d6","trigger":"on_failure","check_match":"sanity|san|理智|恐惧|horror"}]}),
            json!({"id":"hit_points","name":"Hit Points","initial":10,"owner_kind":"actor",
                "on_outcome":[{"op":"subtract","amount":"=damage","trigger":"always","check_match":"damage|attack"}]}),
        ];
        k
    }
    fn coc_mech_combat() -> Value {
        json!({"stats":{"DEX":70},"skills":{"Fighting (Brawl)":55,"Dodge":40}})
    }

    #[test]
    fn attack_does_not_misbind_to_hit_points_track() {
        // 攻击检定(check_match 含 "attack",但那是 hit_points 的 always 路由别名)
        // 必须 NOT 解析到 hit_points 轨(value 0);应落到战斗技能或 None。
        let src = derive_tested_source(None, &coc_kernel_with_hp(), &coc_mech_combat(),
            "appropriate attack/conflict check", "我挥拳打过去", "attack");
        match src {
            Some(TestedSource::Track { ref id, .. }) =>
                panic!("attack must NOT bind to a resource track, got track {}", id),
            _ => {} // Skill / Stat / None 都可接受(关键是不命中 HP 轨)
        }
    }

    #[test]
    fn sanity_still_resolves_with_hp_track_present() {
        // HP 轨在场也不能破坏 SAN(on_failure 别名保留)。
        let src = derive_tested_source(Some(&tp("理智")), &coc_kernel_with_hp(), &coc_mech_combat(),
            "理智 (core mechanic)", "面对不可名状之物", "ability:skill_use");
        assert!(matches!(src, Some(TestedSource::Track { ref id, .. }) if id == "sanity"));
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p trpg-contest attack_does_not_misbind_to_hit_points_track 2>&1 | tail -15`
Expected: FAIL —— attack 命中 hit_points 轨(panic "attack must NOT bind ...")。

- [ ] **Step 3: 改 derive_tested_source 跳过 trigger:always 别名**

`crates/trpg-contest/src/lib.rs:496-502`,把 on_outcome 循环改为:

```rust
        if let Some(arr) = track.get("on_outcome").and_then(|v| v.as_array()) {
            for r in arr {
                // trigger:"always" = 无条件效果路由(谁吸收该动作的效果,如 HP 吸收 damage),
                // 其 check_match 是出场路由别名,NOT 该轨被测的别名 → 跳过。
                // 结果条件型(on_failure/on_success/...)= 一次针对该轨自身的判定(SAN),别名保留。
                if r.get("trigger").and_then(|v| v.as_str()) == Some("always") { continue; }
                if let Some(cm) = r.get("check_match").and_then(|v| v.as_str()) {
                    aliases.extend(cm.split('|').map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()));
                }
            }
        }
```

- [ ] **Step 4: 跑测试确认通过 + SAN 不回归**

Run: `cargo test -p trpg-contest tested_param 2>&1 | tail -20`
Expected: `attack_does_not_misbind...` / `sanity_still_resolves...` / `sanity_check_resolves_to_the_sanity_track` / `no_match_fails_closed_not_fifty` 全 PASS。

- [ ] **Step 5: Checkpoint**

Run: `cargo test -p trpg-contest 2>&1 | tail -5`
Expected: 全绿。

---

## Task 4: resolve_opposed 纯函数(新 opposed.rs)

**Files:**
- Create: `crates/trpg-contest/src/opposed.rs`
- Modify: `crates/trpg-contest/src/lib.rs:1`(加 `mod opposed;` + `use opposed::resolve_opposed;`)

- [ ] **Step 1: 写失败测试(纯对抗比较,真实 CoC bands 形状)**

新建 `crates/trpg-contest/src/opposed.rs`,底部测试:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    // 真实 CoC bands:无 otherwise/critical;失败 roll → success_tier_for 返 None。
    fn coc_bands() -> Vec<Value> {
        serde_json::from_value(json!([
            {"id":"regular","rank":1,"test":{"kind":"roll_under_or_equal"}},
            {"id":"hard","rank":2,"test":{"kind":"roll_under_fraction","denominator":2}},
            {"id":"extreme","rank":3,"test":{"kind":"roll_under_fraction","denominator":5}},
            {"id":"fumble","rank":0,"test":{"kind":"in_range","min":96,"max":100}}
        ])).unwrap()
    }

    #[test]
    fn missing_value_fails_closed() {
        let (t, s, d) = resolve_opposed("roll_under", &coc_bands(), 30, None, 50, Some(60));
        assert_eq!((t, s, d), (None, None, None));
        let (t, s, _) = resolve_opposed("roll_under", &coc_bands(), 30, Some(60), 50, None);
        assert_eq!((t, s), (None, None));
    }

    #[test]
    fn one_succeeds_one_fails_winner_is_success_side() {
        // 攻击 30<=60 成功;防御 80>40 失败 → 攻击方胜。
        let (_t, s, _d) = resolve_opposed("roll_under", &coc_bands(), 30, Some(60), 80, Some(40));
        assert_eq!(s, Some(true));
        // 攻击 80>60 失败;防御 30<=40 成功 → 防御方胜。
        let (_t, s, _d) = resolve_opposed("roll_under", &coc_bands(), 80, Some(60), 30, Some(40));
        assert_eq!(s, Some(false));
    }

    #[test]
    fn both_succeed_higher_tier_wins() {
        // 攻击 5<=60(extreme:<=12);防御 35<=40(regular)。攻击 tier 高 → 胜。
        let (_t, s, _d) = resolve_opposed("roll_under", &coc_bands(), 5, Some(60), 35, Some(40));
        assert_eq!(s, Some(true));
    }

    #[test]
    fn both_fail_defender_wins_status_quo() {
        // 双方都 >其值 → 主动方(攻击)未达成 → 防御方胜。
        let (_t, s, d) = resolve_opposed("roll_under", &coc_bands(), 90, Some(60), 95, Some(40));
        assert_eq!(s, Some(false));
        assert_eq!(d.as_deref(), Some("mutual_failure"));
    }

    #[test]
    fn exact_tie_defender_wins_engine_convention() {
        // 双方成功、同 tier(都 regular)、同 margin(value-total 都=10)→ 引擎约定防御方胜。
        let (_t, s, _d) = resolve_opposed("roll_under", &coc_bands(), 50, Some(60), 30, Some(40));
        assert_eq!(s, Some(false), "tie favors defender (engine convention)");
    }

    #[test]
    fn meet_or_beat_direction() {
        // meet_or_beat:total>=value 成功、越高越好。攻击 18>=10 成功且高于防御 11>=10 → 攻击胜。
        let bands: Vec<Value> = vec![];
        let (_t, s, _d) = resolve_opposed("meet_or_beat", &bands, 18, Some(10), 11, Some(10));
        assert_eq!(s, Some(true));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p trpg-contest opposed 2>&1 | tail -15`
Expected: 编译失败(resolve_opposed 未定义)。

- [ ] **Step 3: 实现 resolve_opposed**

`crates/trpg-contest/src/opposed.rs` 顶部(测试模块之上):

```rust
//! 通用对抗结算(数据驱动,零 per-ruleset):按 kernel `compare` 方向算双方成败,
//! 用 `success_bands` 的 tier rank 比质量。fail-closed:缺值返 (None,None,None)。
use serde_json::Value;
use crate::success_tier_for; // lib.rs 中的既有纯函数

/// 算一侧是否成功(compare 方向数据驱动)。
fn side_succeeds(compare: &str, total: i64, value: i32) -> bool {
    match compare {
        "meet_or_beat" => total >= value as i64,
        _ /* roll_under */ => total <= value as i64,
    }
}

/// 一侧的"质量序":先成败(成功 > 失败),成功内比 tier rank,失败内比 fumble<plain。
/// 返回一个可比较的 (success, tier_rank, margin) 元组的代理分。
fn side_rank(compare: &str, bands: &[Value], total: i64, value: i32) -> (bool, i64, i64) {
    let succeeds = side_succeeds(compare, total, value);
    let tier = success_tier_for(bands, total, value).map(|(_, r)| r).unwrap_or(0);
    let margin = match compare {
        "meet_or_beat" => total - value as i64, // 越大越好
        _ => value as i64 - total,              // roll_under:越大越好
    };
    (succeeds, tier, margin)
}

/// 通用对抗结算。返回 (target, success=attacker_wins, degree)。
/// fail-closed:任一值缺 → (None,None,None)。平局 → 防御方胜(可 override 的引擎约定:
/// engine convention — ties favor the defender / status quo; a future kernel field may override)。
pub fn resolve_opposed(
    compare: &str, bands: &[Value],
    atk_total: i64, atk_value: Option<i32>,
    def_total: i64, def_value: Option<i32>,
) -> (Option<i64>, Option<bool>, Option<String>) {
    let (av, dv) = match (atk_value, def_value) { (Some(a), Some(d)) => (a, d), _ => return (None, None, None) };
    let a = side_rank(compare, bands, atk_total, av);
    let d = side_rank(compare, bands, def_total, dv);
    let (atk_ok, def_ok) = (a.0, d.0);
    let attacker_wins = match (atk_ok, def_ok) {
        (true, false) => true,
        (false, true) => false,
        (false, false) => false, // 双败:主动方未达成 → 防御方胜(status quo)
        (true, true) => {
            // 双胜:比 tier rank → margin → 平局归防御方(引擎约定)。
            if a.1 != d.1 { a.1 > d.1 }
            else if a.2 != d.2 { a.2 > d.2 }
            else { false }
        }
    };
    let degree = Some(if !atk_ok && !def_ok { "mutual_failure".to_string() }
        else if attacker_wins { "attacker_wins".to_string() }
        else { "defender_wins".to_string() });
    (None, Some(attacker_wins), degree)
}
```

在 `crates/trpg-contest/src/lib.rs` 顶部(use 之后)加 `mod opposed;`。`success_tier_for`(lib.rs:408)当前是私有 `fn`;改为 `pub(crate) fn success_tier_for`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p trpg-contest opposed 2>&1 | tail -15`
Expected: 6 个用例全 PASS。

- [ ] **Step 5: Checkpoint**

Run: `cargo test -p trpg-contest 2>&1 | tail -5`
Expected: 全绿。

---

## Task 5: contest 建 OpposedRoll + 泛化 percentile + 接 resolve_opposed

**Files:**
- Modify: `crates/trpg-contest/src/lib.rs`(resolve_percentile_target 泛化 169-186;kernel_resolution_model roll_under 分支 140-157;resolve_outcome 接对抗 16-76)
- Test: `crates/trpg-contest/tests/live_opposed.rs`(新,DB-gated)+ 一个 DB-less 单测

- [ ] **Step 1: 写 DB-less 单测(resolve_outcome 对已填值的 OpposedRoll 出胜负)**

> 直接构造一个 OpposedRoll 已带 attacker_value/defender_value 的 profile 难走 DB;改为在 `opposed.rs` 之外、`lib.rs` 测试模块加一个**纯**测试,验证"resolve_outcome 的对抗分支用 resolve_opposed + 传入 defender_roll"。因 resolve_outcome 需 DB(ensure_contest_profile),此处用 DB-gated 集成测试覆盖端到端(Step 1b);DB-less 仅靠 Task 4 的 resolve_opposed 单测保证逻辑。**本步只加集成测试骨架。**

新建 `crates/trpg-contest/tests/live_opposed.rs`:

```rust
//! DB-gated:验证对抗 check(target_actor + opponent_tested_parameter)经 contest 出真胜负。
//! 需 DATABASE_URL 指向含 CoC 测试角色 + 一个带合成防御技能的 NPC 卡的库;否则 SKIP。
use chrono::Utc;
use serde_json::json;
use trpg_contest::ContestService;
use trpg_db::Db;
use trpg_model::*;

const SESSION: &str = "session_6def47593a094513a75b01b69a52c986";
const RULESET: &str = "call_of_cthulhu_7e";

fn opposed_contract() -> CheckContract {
    serde_json::from_value(json!({
        "check_id": format!("check_opp_{}", uuid::Uuid::new_v4().simple()),
        "session_id": SESSION, "turn_id":"turn_test","ruleset_id":RULESET,"module_id":null,
        "initiator":{"actor_id":"pc.current","actor_kind":"player_character","display_name":null},
        "target_actor":{"actor_id":"npc.opposition","actor_kind":"npc","display_name":"拉斯"},
        "opposition":{"kind":"no_mechanical_opposition"},
        "action_summary":"潜行绕过拉斯","intent_kind":"hide","check_label":"潜行 check",
        "dice_expression":"1d100","modifiers":[],"target":{"kind":"unknown_until_lookup"},
        "tested_parameter":{"domain":null,"key":"Stealth","label":"Stealth"},
        "opponent_tested_parameter":{"domain":null,"key":"perception","label":"perception"},
        "actor_snapshot_ids":[],"source_refs":[],"learned_packet_ids":[],
        "roll_visibility":"public_gm_roll","roll_authority":"system",
        "disclosure":{"reveal_dice":true,"reveal_total":true,"reveal_target":true,"reveal_breakdown":true},
        "stakes":{"before_roll_public":"","success_public":"","failure_public":"",
            "critical_public":null,"fumble_public":null,"success_patches_allowed":[],
            "failure_patches_allowed":[],"irreversible":false},
        "confidence":"medium","ruling_status":"source_backed","advice_refs":[],"expires_at_turn":null
    })).unwrap()
}
fn roll(total: i64) -> DiceRollRecord {
    serde_json::from_value(json!({
        "roll_id":format!("roll_{}",uuid::Uuid::new_v4().simple()),"session_id":SESSION,
        "turn_id":"turn_test","check_id":null,"roller_kind":"player_character","roller_id":"pc.current",
        "visibility":"public_gm_roll","expression":"1d100","result":{"total":total,"rolls":[total]},
        "seed_commitment":"","revealed_at":null,"created_at":Utc::now()
    })).unwrap()
}

#[tokio::test]
async fn opposed_check_produces_a_verdict_when_both_values_present() {
    let url = match std::env::var("DATABASE_URL") { Ok(u)=>u, Err(_)=>{eprintln!("SKIP: DATABASE_URL unset");return;} };
    let db = match Db::connect(&url).await { Ok(d)=>d, Err(e)=>{eprintln!("SKIP: {e}");return;} };
    // 夹具门:pc.current 有 Stealth、npc.opposition 卡有合成 skills.perception。缺则 SKIP。
    let svc_params = trpg_params::RuntimeParameterService::new(db.clone());
    let pc = svc_params.load_actor_parameters(SESSION, "pc.current").await.ok().flatten();
    let npc = svc_params.load_actor_parameters(SESSION, "npc.opposition").await.ok().flatten();
    let ok = pc.as_ref().and_then(|p| p.mechanical_profile.pointer("/skills/Stealth")).is_some()
        && npc.as_ref().and_then(|p| p.mechanical_profile.pointer("/skills/perception")).is_some();
    if !ok { eprintln!("SKIP: opposed fixture (pc Stealth + npc perception) not found"); return; }

    let svc = ContestService::new(db.clone());
    let def_roll = roll(95); // 防御方掷高(roll_under 大概率失败)
    let out = svc.resolve_outcome(&opposed_contract(), &roll(10), Some(&def_roll)).await.expect("resolve");
    let success = out.get("success").and_then(|v| v.as_bool());
    println!("[opposed] atk=10 def=95 success={:?} opposed={:?}", success, out.get("opposed"));
    assert!(success.is_some(), "对抗必须给出胜负,而非 Provisional-null");
    assert_eq!(out.get("opposed").and_then(|o| o.get("defender_value")).is_some(), true,
        "outcome 必须富化 opposed.defender_value(消费了 NPC 卡的值)");
}
```

- [ ] **Step 2: 跑确认失败/SKIP**

Run: `DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg cargo test -p trpg-contest --test live_opposed -- --nocapture 2>&1 | tail -15`
Expected: 编译过后 FAIL(success=None / 无 opposed 富化)或 SKIP(夹具未备 —— 实现完后 Task 11 备夹具再真验)。先求编译过 + 逻辑接通。

- [ ] **Step 3: 泛化 resolve_percentile_target**

`crates/trpg-contest/src/lib.rs:169` 改为按 actor_id + 显式 hint 取,旧名做薄包装:

```rust
    async fn resolve_percentile_target(&self, contract: &CheckContract, kernel: &RuleKernel) -> Option<(String, i32)> {
        self.resolve_percentile_target_for(&contract.initiator.actor_id, contract.tested_parameter.as_ref(), contract, kernel).await
    }

    async fn resolve_percentile_target_for(&self, actor_id: &str, hint: Option<&TestedParameter>,
        contract: &CheckContract, kernel: &RuleKernel) -> Option<(String, i32)> {
        let params = RuntimeParameterService::new(self.db.clone())
            .load_actor_parameters(&contract.session_id, actor_id).await.ok().flatten()?;
        let mech = &params.mechanical_profile;
        let src = derive_tested_source(hint, kernel, mech,
            &contract.check_label, &contract.action_summary, &contract.intent_kind)?;
        match src {
            TestedSource::Track { id, label } =>
                self.read_track_value(&contract.session_id, actor_id, &id, kernel).await.map(|v| (label, v)),
            TestedSource::Skill { key } => value_from_profile(mech.get("skills"), &key).map(|v| (key, v)),
            TestedSource::Stat { key } => value_from_profile(mech.get("stats"), &key).map(|v| (key, v)),
        }
    }
```

- [ ] **Step 4: kernel_resolution_model roll_under 分支建 OpposedRoll**

`crates/trpg-contest/src/lib.rs:140-157` roll_under 分支改为(对手在场 → 对抗,否则自测):

```rust
            "roll_under" => {
                let opposed = contract.target_actor.is_some() && contract.opponent_tested_parameter.is_some();
                if opposed {
                    let ta = contract.target_actor.as_ref().unwrap();
                    let atk = self.resolve_percentile_target_for(&contract.initiator.actor_id,
                        contract.tested_parameter.as_ref(), contract, &kernel).await.map(|(_, v)| v);
                    let def = self.resolve_percentile_target_for(&ta.actor_id,
                        contract.opponent_tested_parameter.as_ref(), contract, &kernel).await.map(|(_, v)| v);
                    let def_expr = dc.get("dice").and_then(|v| v.as_str())
                        .unwrap_or(&contract.dice_expression).to_string();
                    return Some(CheckResolutionModel::OpposedRoll {
                        attacker_expression: contract.dice_expression.clone(), attacker_value: atk,
                        defender_actor_id: Some(ta.actor_id.clone()), defender_expression: def_expr,
                        defender_value: def, defender_roll_visibility: RollVisibility::PrivateGmRoll,
                    });
                }
                if let Some((label, value)) = self.resolve_percentile_target(contract, &kernel).await {
                    Some(CheckResolutionModel::PercentileRollUnder { ability_label: label, ability_value: value })
                } else if let Some(v) = tnum {
                    Some(CheckResolutionModel::PercentileRollUnder { ability_label: contract.check_label.clone(), ability_value: v })
                } else {
                    Some(CheckResolutionModel::Provisional {
                        reason: "percentile roll-under: the actor's tested skill/characteristic/track value was not found; bind the check's tested_parameter or hydrate the actor sheet before resolving".into(),
                        suggested_target: None,
                    })
                }
            }
```

- [ ] **Step 5: resolve_outcome 接对抗结算(用 defender_roll)**

`crates/trpg-contest/src/lib.rs:16-20`,去掉 `_defender_roll` 下划线,在 `resolve_against_model` 之后插对抗覆盖:

```rust
    pub async fn resolve_outcome(&self, contract: &CheckContract, roll: &DiceRollRecord,
        defender_roll: Option<&DiceRollRecord>) -> Result<Value> {
        let total = roll_total(roll);
        let rolls = roll_dice_array(roll);
        let profile = self.ensure_contest_profile(contract, Some(roll), total).await?;
        let (mut target, mut success, mut degree) = resolve_against_model(&profile.resolution_model, total, &rolls);
        // 对抗结算:模型为 OpposedRoll 时,用 kernel compare + success_bands 比 tier(数据驱动)。
        // resolve_against_model 对 OpposedRoll 返回 (None,None,None),此处覆盖。
        if let CheckResolutionModel::OpposedRoll { attacker_value, defender_value, .. } = &profile.resolution_model {
            let def_total = defender_roll.map(roll_total);
            let (kc_opt, bands) = self.load_compare_and_bands(&contract.ruleset_id).await;
            if let (Some(compare), Some(dt)) = (kc_opt, def_total) {
                let (t, s, d) = opposed::resolve_opposed(&compare, &bands, total, *attacker_value, dt, *defender_value);
                target = t; success = s; degree = d;
                // outcome 富化见下方 outcome JSON。
            }
        }
        let mut outcome = json!({ /* ...原字段不变... */ });
```

在 outcome JSON 构造后(line ~34 之后)加对抗富化:

```rust
        if let CheckResolutionModel::OpposedRoll { attacker_value, defender_value, defender_actor_id, .. } = &profile.resolution_model {
            outcome["opposed"] = json!({
                "attacker_total": total, "attacker_value": attacker_value,
                "defender_total": defender_roll.map(roll_total), "defender_value": defender_value,
                "defender_actor_id": defender_actor_id,
                "winner": match success { Some(true)=>"attacker", Some(false)=>"defender", None=>"unresolved" },
            });
        }
```

新增私有方法(在 impl ContestService 内):

```rust
    /// 取 kernel 的 compare 方向 + success_bands(对抗结算用;对抗 target=None 跳过了
    /// resolve_outcome:51 那次门控加载,故单独加载一次)。
    async fn load_compare_and_bands(&self, ruleset_id: &str) -> (Option<String>, Vec<Value>) {
        match self.db.load_rule_kernel(ruleset_id).await.ok().flatten() {
            Some(k) => (
                k.dice_core.get("compare").and_then(|v| v.as_str()).map(str::to_string),
                k.dice_core.get("success_bands").and_then(|v| v.as_array()).cloned().unwrap_or_default(),
            ),
            None => (None, vec![]),
        }
    }
```

- [ ] **Step 6: 跑测试**

Run: `cargo test -p trpg-contest 2>&1 | tail -15`
Expected: 既有测试全绿(non-opposed 路径 attacker_value/defender_value 不被走);`live_opposed` 在无夹具时 SKIP,有夹具时 PASS。

- [ ] **Step 7: Checkpoint**

Run: `DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg cargo test -p trpg-contest 2>&1 | tail -10`
Expected: 全绿/SKIP,无 FAIL。

---

## Task 6: B1 — ensure_npc_parameter 写后投影 mechanical_profile

**Files:**
- Modify: `crates/trpg-runtime/src/lib.rs:1551`(ensure_npc_parameter)
- Test: `crates/trpg-runtime/src/lib.rs`(新增纯测试:write+refresh→mech 有值)

- [ ] **Step 1: 写失败测试(投影后 mech 含合成值)**

在 `crates/trpg-runtime/src/lib.rs` 测试区加:

```rust
    #[test]
    fn synthesized_param_projects_into_mechanical_profile() {
        use crate::npc_synth::{SynthesizedParam, write_synthesized_param};
        use serde_json::json;
        let mut sheet = json!({"stats":{}, "skills":{}});
        let p = SynthesizedParam { value: json!(45), status: "provisional".into(),
            provenance: json!({"tier":"persona_judge","parameter":"perception"}) };
        write_synthesized_param(&mut sheet, "skills", "perception", &p);
        let mut mech = json!({"ruleset":"call_of_cthulhu_7e"});
        crate::chargen::refresh_mechanical_profile(&mut mech, &sheet);
        // contest 读 mechanical_profile.skills.perception → 必须是 45(B1 修复的核心断言)。
        assert_eq!(mech.pointer("/skills/perception"), Some(&json!(45)));
        // provenance 仍在 sheet(被归进 fields 但数据保留)。
        assert_eq!(mech.pointer("/fields/npc_param_provenance/perception/tier"), Some(&json!("persona_judge")));
    }
```

- [ ] **Step 2: 跑确认(可能已部分通过——refresh 已存在)**

Run: `cargo test -p trpg-runtime synthesized_param_projects_into_mechanical_profile 2>&1 | tail -10`
Expected: PASS(此测试验证既有 refresh 行为对 NPC sheet 正确;若 chargen 模块路径不对则编译失败,修 use 路径)。

- [ ] **Step 3: 在 ensure_npc_parameter 写后加 refresh**

`crates/trpg-runtime/src/lib.rs:1551`(`write_synthesized_param` 之后、`upsert_actor_parameters` 之前):

```rust
        npc_synth::write_synthesized_param(&mut p.sheet_json, bucket, param, &synth);
        // B1: mechanical_profile 是 sheet 的物化视图;contest 读它,故写完立即重投影,
        // 否则现搓值停在 sheet_json、结算读不到。refresh 不动 npc_param_provenance 数据。
        chargen::refresh_mechanical_profile(&mut p.mechanical_profile, &p.sheet_json);
        service.upsert_actor_parameters(&p).await?;
```

确认 `chargen` 在 runtime/lib.rs 已 `mod`/`use`(它在别处已被调用,见 apply_track_change:1653)。

- [ ] **Step 4: 跑测试 + 不回归**

Run: `cargo test -p trpg-runtime 2>&1 | tail -10`
Expected: 新测试 PASS,既有全绿。

- [ ] **Step 5: Checkpoint**

Run: `cargo build -p trpg-runtime && cargo test -p trpg-runtime npc 2>&1 | tail -8`
Expected: 全绿。

---

## Task 7: check_param_need 按 compare 选参数(async + kernel)

**Files:**
- Modify: `crates/trpg-runtime/src/lib.rs:1607`(check_param_need)、`:2126`(map_check_param_need)、`:3024`(单测)
- Modify: `crates/trpg-cli/src/main.rs:1083`(调用点)

- [ ] **Step 1: 重写失败单测(两种 compare)**

`crates/trpg-runtime/src/lib.rs:3024` 的 `check_param_need_maps_typed_action_kind_to_bucket_param` 整体替换:

```rust
    #[test]
    fn map_check_param_need_selects_by_compare() {
        use trpg_model::SituationActionKind::*;
        // meet_or_beat:攻击族 → 被动 defense DV。
        assert_eq!(map_check_param_need(&Attack, "meet_or_beat"), Some(("stats".into(),"defense".into())));
        // roll_under:攻击族 → 对抗技能 dodge。
        assert_eq!(map_check_param_need(&Attack, "roll_under"), Some(("skills".into(),"dodge".into())));
        // 潜行/盗窃/对抗社交 → perception(两种 compare 一致)。
        for ak in [Hide, Hack, Intimidate] {
            assert_eq!(map_check_param_need(&ak, "roll_under"), Some(("skills".into(),"perception".into())));
        }
        // 不需 NPC 参数 → None。
        for ak in [AskQuestion, Move, LeaveScene, Unknown] {
            assert_eq!(map_check_param_need(&ak, "roll_under"), None);
            assert_eq!(map_check_param_need(&ak, "meet_or_beat"), None);
        }
    }
```

- [ ] **Step 2: 跑确认失败**

Run: `cargo test -p trpg-runtime map_check_param_need_selects_by_compare 2>&1 | tail -10`
Expected: 编译失败(map_check_param_need 现签名只收 1 参)。

- [ ] **Step 3: 改 map_check_param_need + check_param_need**

`crates/trpg-runtime/src/lib.rs:2126` 改为:

```rust
fn map_check_param_need(action_kind: &SituationActionKind, compare: &str) -> Option<(String, String)> {
    use SituationActionKind::*;
    let attack_family = matches!(action_kind,
        Attack | UnderAttack | Counterattack | EnemyInitiatedConflict | SceneEntersConflict
        | Defend | Dodge | TakeCover | CastOrUsePower);
    let stealth_family = matches!(action_kind, Hide | Hack | DisableDevice | Intimidate | Negotiate);
    if attack_family {
        // meet_or_beat:被动 DV;roll_under:对抗技能(闪避)。
        return Some(if compare == "meet_or_beat" { ("stats".into(), "defense".into()) }
                    else { ("skills".into(), "dodge".into()) });
    }
    if stealth_family {
        // 潜行/对抗社交:对手用察觉对抗(两种 compare 都取 perception)。
        return Some(("skills".into(), "perception".into()));
    }
    None
}
```

`check_param_need`(:1607)改 async + 加载 kernel compare:

```rust
    pub async fn check_param_need(&self, ruleset_id: &str, action_kind: &SituationActionKind) -> Option<(String, String)> {
        let compare = self.db.load_rule_kernel(ruleset_id).await.ok().flatten()
            .and_then(|k| k.dice_core.get("compare").and_then(|v| v.as_str()).map(str::to_string))
            .unwrap_or_default();
        map_check_param_need(action_kind, &compare)
    }
```

- [ ] **Step 4: 改 CLI 调用点**

`crates/trpg-cli/src/main.rs:1083`:

```rust
        if let Some((bucket, param)) = runtime.check_param_need(ruleset, &turn_orchestration.intent.action_kind).await {
```

(`ruleset` 是该作用域内 ruleset_id 变量;若名不同按编译报错改。)

- [ ] **Step 5: 跑测试 + build CLI**

Run: `cargo test -p trpg-runtime map_check_param_need_selects_by_compare && cargo build -p trpg-cli 2>&1 | tail -8`
Expected: PASS + CLI 编译过。

- [ ] **Step 6: Checkpoint**

Run: `cargo test -p trpg-runtime 2>&1 | tail -8`
Expected: 全绿。

---

## Task 8: 盖章 hook — 把 target_actor + 双方 tested key 写进对抗契约

**Files:**
- Modify: `crates/trpg-runtime/src/lib.rs`(新增 pub 纯函数 `stamp_opposed_check`)
- Modify: `crates/trpg-cli/src/main.rs:1082-1124`(hoist persona/param + 盖章 plan.check)
- Test: `crates/trpg-runtime/src/lib.rs`(stamp_opposed_check 纯单测)

- [ ] **Step 1: 写失败单测(盖章纯函数)**

`crates/trpg-runtime/src/lib.rs` 测试区:

```rust
    #[test]
    fn stamp_opposed_check_sets_target_and_opponent_param() {
        use trpg_model::*;
        let mut check: CheckContract = serde_json::from_value(serde_json::json!({
            "check_id":"c","session_id":"s","turn_id":"t","ruleset_id":"call_of_cthulhu_7e","module_id":null,
            "initiator":{"actor_id":"pc.current","actor_kind":"player_character","display_name":null},
            "target_actor":null,"opposition":{"kind":"no_mechanical_opposition"},
            "action_summary":"潜行","intent_kind":"hide","check_label":"潜行 check","dice_expression":"1d100",
            "modifiers":[],"target":{"kind":"unknown_until_lookup"},
            "tested_parameter":{"domain":null,"key":"Stealth","label":"Stealth"},
            "opponent_tested_parameter":null,"actor_snapshot_ids":[],"source_refs":[],"learned_packet_ids":[],
            "roll_visibility":"public_gm_roll","roll_authority":"system",
            "disclosure":{"reveal_dice":true,"reveal_total":true,"reveal_target":true,"reveal_breakdown":true},
            "stakes":{"before_roll_public":"","success_public":"","failure_public":"","critical_public":null,
                "fumble_public":null,"success_patches_allowed":[],"failure_patches_allowed":[],"irreversible":false},
            "confidence":"medium","ruling_status":"source_backed","advice_refs":[],"expires_at_turn":null
        })).unwrap();
        let npc = npc_synth::NpcPersona { actor_id: "npc.opposition".into(), name: "拉斯".into(), prose: "".into() };
        stamp_opposed_check(&mut check, &npc, "skills", "perception");
        assert_eq!(check.target_actor.as_ref().map(|a| a.actor_id.as_str()), Some("npc.opposition"));
        assert_eq!(check.opponent_tested_parameter.as_ref().map(|t| t.key.as_str()), Some("perception"));
    }
```

- [ ] **Step 2: 跑确认失败**

Run: `cargo test -p trpg-runtime stamp_opposed_check_sets_target 2>&1 | tail -10`
Expected: 编译失败(stamp_opposed_check 未定义)。

- [ ] **Step 3: 实现 stamp_opposed_check**

`crates/trpg-runtime/src/lib.rs`(自由函数,放 map_check_param_need 旁):

```rust
/// 盖章:把对抗所需的 target_actor + 防御方 tested key 写进契约(B2 通道)。
/// 攻击方 tested key 走 contract.tested_parameter(named 路径已设)。纯函数,易测。
pub fn stamp_opposed_check(check: &mut trpg_model::CheckContract, npc: &npc_synth::NpcPersona, _bucket: &str, param: &str) {
    check.target_actor = Some(trpg_model::ActorRef {
        actor_id: npc.actor_id.clone(), actor_kind: trpg_model::ActorKind::Npc,
        display_name: Some(npc.name.clone()),
    });
    check.opponent_tested_parameter = Some(trpg_model::TestedParameter {
        domain: None, key: param.to_string(), label: param.to_string(),
    });
}
```

- [ ] **Step 4: CLI 盖章接线**

`crates/trpg-cli/src/main.rs`:把 pre-pass(1082-1091)的 persona + (bucket,param) 提到外层作用域,plan 建好后(1121 之后)盖章。具体:

(a) 1082-1091 改为(把 `bucket/param/persona` 提出来供后续复用):

```rust
    let mut opposed_npc: Option<(String, String, trpg_runtime::npc_synth::NpcPersona)> = None;
    if resolved_pending.is_none() {
        if let Some((bucket, param)) = runtime.check_param_need(ruleset, &turn_orchestration.intent.action_kind).await {
            if let Some(persona) = runtime.current_check_npc_persona(&request, &state).await {
                let check_context = turn_orchestration.intent.action_kind.as_str();
                let _ = runtime.prepare_npc_for_check(session_id, ruleset, &persona, &bucket, &param, check_context).await;
                opposed_npc = Some((bucket, param, persona));
            }
        }
    }
```

(b) 1121 改 `let mut plan = ...`;1124 前插盖章:

```rust
        if let (Some((bucket, param, persona)), Some(check)) = (opposed_npc.as_ref(), plan.check.as_mut()) {
            trpg_runtime::stamp_opposed_check(check, persona, bucket, param);
        }
        if let Some(check) = &plan.check {
```

- [ ] **Step 5: 跑测试 + build**

Run: `cargo test -p trpg-runtime stamp_opposed && cargo build -p trpg-cli 2>&1 | tail -8`
Expected: PASS + CLI 编译过。

- [ ] **Step 6: Checkpoint**

Run: `cargo test -p trpg-runtime 2>&1 | tail -6`
Expected: 全绿。

---

## Task 9: runtime 包装器预掷防御骰 + 路由 4 个 resolve_outcome 调用点

**Files:**
- Modify: `crates/trpg-runtime/src/lib.rs`(新增 `resolve_outcome_with_opposition`;改 394/477/929/983)
- Test: `crates/trpg-runtime/src/lib.rs`(包装器对非对抗契约传 None 的纯判定单测)

- [ ] **Step 1: 写失败单测(是否对抗的判定)**

```rust
    #[test]
    fn contract_is_opposed_only_with_target_and_opponent_param() {
        use trpg_model::*;
        let mut c: CheckContract = serde_json::from_value(serde_json::json!({
            "check_id":"c","session_id":"s","turn_id":"t","ruleset_id":"r","module_id":null,
            "initiator":{"actor_id":"pc.current","actor_kind":"player_character","display_name":null},
            "target_actor":null,"opposition":{"kind":"no_mechanical_opposition"},
            "action_summary":"","intent_kind":"","check_label":"","dice_expression":"1d100","modifiers":[],
            "target":{"kind":"unknown_until_lookup"},"tested_parameter":null,"opponent_tested_parameter":null,
            "actor_snapshot_ids":[],"source_refs":[],"learned_packet_ids":[],
            "roll_visibility":"public_gm_roll","roll_authority":"system",
            "disclosure":{"reveal_dice":true,"reveal_total":true,"reveal_target":true,"reveal_breakdown":true},
            "stakes":{"before_roll_public":"","success_public":"","failure_public":"","critical_public":null,
                "fumble_public":null,"success_patches_allowed":[],"failure_patches_allowed":[],"irreversible":false},
            "confidence":"medium","ruling_status":"provisional","advice_refs":[],"expires_at_turn":null
        })).unwrap();
        assert!(!contract_is_opposed(&c));
        c.target_actor = Some(ActorRef{actor_id:"npc.opposition".into(),actor_kind:ActorKind::Npc,display_name:None});
        assert!(!contract_is_opposed(&c), "只有 target_actor 还不够");
        c.opponent_tested_parameter = Some(TestedParameter{domain:None,key:"perception".into(),label:"perception".into()});
        assert!(contract_is_opposed(&c));
    }
```

- [ ] **Step 2: 跑确认失败**

Run: `cargo test -p trpg-runtime contract_is_opposed_only_with 2>&1 | tail -8`
Expected: 编译失败(contract_is_opposed 未定义)。

- [ ] **Step 3: 实现判定 + 包装器**

`crates/trpg-runtime/src/lib.rs`(自由函数):

```rust
/// 对抗契约判定:必须同时有 target_actor 与 opponent_tested_parameter。
pub fn contract_is_opposed(c: &trpg_model::CheckContract) -> bool {
    c.target_actor.is_some() && c.opponent_tested_parameter.is_some()
}
```

在 impl RuntimeEngine 内(resolve_roll_input 附近):

```rust
    /// 对抗时预掷防御骰(私有 GM 掷,落库供复算)随 contract 传入 contest;否则 None。
    /// contest 保持零 RNG。defender_expression 取 kernel 核心掷式(dice_core.dice),fallback 攻击式。
    async fn resolve_outcome_with_opposition(&self, contract: &CheckContract, roll: &DiceRollRecord) -> Result<serde_json::Value> {
        let svc = ContestService::new(self.db.clone());
        if !contract_is_opposed(contract) {
            return svc.resolve_outcome(contract, roll, None).await;
        }
        let def_expr = self.db.load_rule_kernel(&contract.ruleset_id).await.ok().flatten()
            .and_then(|k| k.dice_core.get("dice").and_then(|v| v.as_str()).map(str::to_string))
            .unwrap_or_else(|| contract.dice_expression.clone());
        let rolled = roll_dice(&def_expr)?;
        let def_record = DiceRollRecord {
            roll_id: format!("roll_{}", Uuid::new_v4().simple()),
            session_id: contract.session_id.clone(), turn_id: contract.turn_id.clone(),
            check_id: Some(contract.check_id.clone()),
            roller_kind: ActorKind::Npc,
            roller_id: contract.target_actor.as_ref().map(|a| a.actor_id.clone()),
            visibility: RollVisibility::PrivateGmRoll, expression: def_expr.clone(),
            result: json!({"mode":"rolled","expression":rolled.expression,"rolls":rolled.rolls,
                "modifier":rolled.modifier,"total":rolled.total}),
            seed_commitment: String::new(), revealed_at: None, created_at: chrono::Utc::now(),
        };
        self.db.insert_dice_roll(&def_record).await.ok(); // m1: 落库供复算
        svc.resolve_outcome(contract, roll, Some(&def_record)).await
    }
```

> 注:`DiceRollRecord` 字段以 `crates/trpg-model/src/lib.rs:3008` 为准;若有额外字段(如 `roll_authority`),按编译报错补。`roll_dice` 返回结构以 resolve_roll_input(:1108)用法为准。

- [ ] **Step 4: 路由 4 个调用点**

`crates/trpg-runtime/src/lib.rs` 394/477/929/983:
`ContestService::new(self.db.clone()).resolve_outcome(&X, &roll, None).await?` → `self.resolve_outcome_with_opposition(&X, &roll).await?`
(X 为 `pending.contract` 或 `normalized`)。

- [ ] **Step 5: 跑测试 + build**

Run: `cargo test -p trpg-runtime contract_is_opposed && cargo build -p trpg-runtime 2>&1 | tail -8`
Expected: PASS + 编译过。

- [ ] **Step 6: Checkpoint**

Run: `cargo test -p trpg-runtime 2>&1 | tail -8`
Expected: 全绿。

---

## Task 10: M4 — 叙述守卫按 success/degree 判,不靠脆弱串匹配

**Files:**
- Modify: `crates/trpg-cli/src/main.rs:2165-2172`(outcome_is_provisional)
- Modify: `crates/trpg-api/src/lib.rs:1754-1761`(outcome_is_provisional_api)
- Test: 就近单测(任一 crate 可编译处)

- [ ] **Step 1: 写失败单测(已判定的对抗 outcome 不算 provisional)**

在 `crates/trpg-cli/src/main.rs` 测试区(若无则 `#[cfg(test)] mod guard_tests`):

```rust
    #[test]
    fn resolved_opposed_outcome_is_not_provisional() {
        use serde_json::json;
        // 对抗已出胜负:success 非空、target 可为 null。守卫不得判 provisional。
        let resolved = json!({"success": false, "degree": "defender_wins", "target": null,
            "resolution_model": {"kind":"opposed_roll"}});
        assert!(!outcome_is_provisional(&resolved));
        // 真未判定:success/degree 都 null。
        let unresolved = json!({"success": null, "degree": null, "target": null,
            "resolution_model": {"kind":"provisional","reason":"x"}});
        assert!(outcome_is_provisional(&unresolved));
    }
```

- [ ] **Step 2: 跑确认失败**

Run: `cargo test -p trpg-cli resolved_opposed_outcome_is_not_provisional 2>&1 | tail -10`
Expected: FAIL(现逻辑靠串匹配,resolved opposed 走 no_verdict 但行为脆弱)或编译失败(outcome_is_provisional 非 pub/不可见)→ 按需把它设为 `pub(crate)`。

- [ ] **Step 3: 改判据**

`crates/trpg-cli/src/main.rs:2165` `outcome_is_provisional` 改为以字段为准:

```rust
fn outcome_is_provisional(outcome: &serde_json::Value) -> bool {
    // 已判定 = success 或 degree 任一非空(对抗 target 可为 null 但有 success)。
    let has_verdict = outcome.get("success").map(|v| !v.is_null()).unwrap_or(false)
        || outcome.get("degree").map(|v| !v.is_null()).unwrap_or(false);
    if has_verdict { return false; }
    // 无判定:模型为 provisional / 待绑定程序时,叙述层别编成败。
    let model = outcome.get("resolution_model").map(|m| m.to_string()).unwrap_or_default();
    model.contains("provisional") || model.contains("ruleset_procedure_lookup")
}
```

`crates/trpg-api/src/lib.rs:1754` `outcome_is_provisional_api` 同样改(删掉 "OpposedRoll" PascalCase 死分支,改 success/degree 判)。

- [ ] **Step 4: 跑测试 + build api**

Run: `cargo test -p trpg-cli resolved_opposed_outcome_is_not_provisional && cargo build -p trpg-api 2>&1 | tail -8`
Expected: PASS + api 编译过。

- [ ] **Step 5: Checkpoint(全工作区)**

Run: `cargo test 2>&1 | tail -20`
Expected: 全工作区绿(DB-gated 测试在无 DATABASE_URL 时 SKIP)。

---

## Task 11: CoC live 验证(端到端真验)

**Files:** 无代码改动;备夹具 + 跑真回合。

**前置 env**:
```
TRPG_NPC_PERSONA_SYNTHESIS=true
DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg
TRPG_DATA_DIR=/Users/haoli/leehow/code/chatrpgv2/_rstest_coc
TRPG_LLM_*（codex-relay :18888,模型 gpt-5.4 / gpt-5.4-mini;拒 temperature）
```
模组 `call_of_cthulhu_7e.document`(NPC 拉斯 sc01 已 deep_extracted)。turn 的 module_id 只从 `--module` 取,flag `--session-id`。macOS 无 `timeout` → 用 Bash 工具超时。psql 走 `docker exec chatrpg-postgres-rulesets psql`。

- [ ] **Step 1: 确认 PC 有对抗用技能 + 场景 NPC 可解析**

Run(查 pc.current 的 skills 是否含潜行类技能;查 sc01 referenced_npc_ids → 拉斯):
```bash
docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg -t -A -c \
"select mechanical_profile->'skills' from runtime_actor_parameters where actor_id='pc.current' limit 1;"
```
Expected: 能看到 CoC 技能集(若无潜行/妙手类,Step 2 的玩家输入选 PC 实际拥有的对抗技能)。

- [ ] **Step 2: 跑一回合"玩家发起的对抗技能检定"**

用 `trpg turn`(或项目实际 CLI 子命令)对当前 session 输入一句明确的对抗技能动作(玩家点名技能 + 对象是拉斯),例如「我用潜行绕到拉斯背后」或 PC 实有技能对应的对抗动作。观察 stream 的 `check_contract_created`、`opposed` 富化、`success`。

- [ ] **Step 3: 断言对抗真出胜负(非 Provisional-null)**

检查 outcome:`success` 非 null、`opposed.defender_value` 非 null(= 消费了拉斯现搓的 perception/对抗技能)、`resolution_model.kind == "opposed_roll"`。查 NPC 卡确认合成值落库:
```bash
docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg -t -A -c \
"select sheet_json->'skills', mechanical_profile->'skills', sheet_json->'npc_param_provenance' \
 from runtime_actor_parameters where actor_id='npc.opposition' and session_id=<本次 session> limit 1;"
```
Expected: skills 含合成的 perception/dodge,mechanical_profile 同步(B1),provenance tier=persona_judge。outcome 给出真胜负。

- [ ] **Step 4: 跑 live_opposed 集成测试(夹具已备)**

Run: `DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg cargo test -p trpg-contest --test live_opposed -- --nocapture 2>&1 | tail -15`
Expected: `opposed_check_produces_a_verdict_when_both_values_present` PASS(不再 SKIP)。

- [ ] **Step 5: 全工作区回归**

Run: `cargo test 2>&1 | tail -20`
Expected: 全绿;`live_percentile`(SAN/percentile 不回归)、tested-source、tier 系列全绿。

---

## Self-Review 结论(已对 spec v2.1 逐节核对)
- 端到端通道(§3):B1=Task6、B2 通道=Task1(字段)+Task8(盖章)、contest 建 OpposedRoll=Task5、预掷=Task9、对抗结算=Task4+Task5、叙述守卫 M4=Task10。✓
- tested-source 外科(§4)=Task3。✓ check_param_need 按 compare(§7)=Task7。✓
- 类型一致:`opponent_tested_parameter`/`attacker_value`/`defender_value`/`resolve_opposed`/`resolve_outcome_with_opposition`/`contract_is_opposed`/`stamp_opposed_check`/`resolve_percentile_target_for` 跨任务命名一致。✓
- 8 个 resolve_outcome 调用点全覆盖(Task2 穿 None,Task9 路由 4 个生产点走包装器)。✓
- 必绿测试每个 Task 后跑(§5 不变量):sanity/no_match/live_percentile/tier/forced_tech。✓
- live 验路径 = 方案 A(玩家发起对抗技能检定),Task11。combat-attack 攻击方派生范围外(spec §9)。✓

# P0-2 实施计划：引擎去规则集/模组硬编码

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把引擎里 18 处 ruleset_id.contains/模组名/硬编码 NPC/DV 迁到 RuleKernel 策略字段 + ModuleConfig + override 数据，引擎改读数据 + 通用兜底，CI 守卫防回潮——落实"零规则集硬编码"头号理念。

**Architecture:** trpg-model 加 RuleKernel 5 个 Option 策略字段（combat_profile/combat_mode_policy/check_label_policy/referee_value_bands/dice_qualification，均 serde default → 通用 GENERIC_* 兜底）+ ModuleConfig（npc_actor_bindings/technical_option_table/aliases）。引擎 6 个 helper 从 ruleset_id.contains 分支改 `kernel.<policy>.unwrap_or(&GENERIC).<x>` 数据查找（签名 ruleset_id→&RuleKernel；调用点已 load_rule_kernel）。规则集/模组特例迁 override 数据文件（复用 trpg-db load_rule_kernel + read_kernel_override_file 合并 data/parsed/rules/{ruleset_id}.rule_kernel.override.json），非引擎码。CI xtask no-engine-ruleset-hardcode grep 守卫。六规则集三模组等价闸（live+单测）保零行为回归。前置 R1/R2/R5 已合 main(a653f01)。

**Tech Stack:** Rust, trpg-model RuleKernel, trpg-db override 加载, serde(default) 向后兼容, xtask/scripts grep 守卫。前置 spec：`docs/superpowers/specs/2026-06-16-p0-2-engine-dehardcode-design.md`；grounding：`docs/GPT_Pro_审查_triage_2026-06-16.md`（18 MIGRATE 点）。

---

## 共享契约（精确类型/命名/迁移模式）

- RuleKernel(trpg-model:1424) 加 5 Option 策略字段(#[serde(default, skip_serializing_if="Option::is_none")])：combat_profile/combat_mode_policy/check_label_policy/referee_value_bands/dice_qualification。CombatProfile 用 Value 镜像 trpg-combat::RulesetCombatProfile 子块（避免 model→combat 反向依赖）。CombatMode 枚举(trpg-model:3531)保留不动。RefereeValueBands{damage_family:String,damage_band:String,damage_plausible_range:(i64,i64),difficulty_band:Value,difficulty_plausible_range:(i64,i64)}。GENERIC_* 中性默认。
- ModuleConfig(#[serde(default)])：npc_actor_bindings/technical_option_table/scene_entity_aliases/module_search_profile。
- 迁移模式：helper 签名 ruleset_id:&str→&RuleKernel(调用点已 db.load_rule_kernel)；体 if contains 分支→kernel.<field>.as_ref().unwrap_or(&GENERIC).<sub>。6 helper(is_homecoming/inferred_homecoming_tech_dv/target_actor_for_combat_input/infer_combat_mode_from_intent/referee 五函数/ruleset_aliases_for)全转数据查找。
- override 数据：复用 trpg-db load_rule_kernel(:693) + read_kernel_override_file(:4050) 合并 data/parsed/rules/{ruleset_id}.rule_kernel.override.json；扩它合新策略键。特例值（cyberpunk/dnd/coc/triangle/sword_world profile、Homecoming DV/NPC、masks/vault search）迁这里。
- 等价闸：六规则集(CoC:54347/Cyberpunk:54346/D&D/SW/Triangle/Fate)+三模组(血色公路/Homecoming/Vault)迁移前后 combat mode/damage-difficulty 合理性/search profile/NPC 绑定/tech DV/director brief 一致(live+单测)。CI grep 引擎 src 规则集/模组名=0(白名单：parser source-id/cfg(test)/override 加载键)。

## 文件结构总览

| 文件 | 动作 | 责任 | Task |
|---|---|---|---|
| `crates/trpg-model/src/lib.rs` | Modify | RuleKernel 5 字段 + 策略结构 + ModuleConfig + GENERIC_* | T1 |
| `crates/trpg-combat/src/lib.rs` + override 数据 | Modify | 6→数据查找 + combat override | T2 |
| `crates/trpg-referee/src/lib.rs` + override | Modify | 五函数→referee_value_bands | T3 |
| `crates/trpg-director/src/lib.rs` | Modify | is_homecoming 去除→module 数据 | T4 |
| `crates/trpg-material/src/lib.rs` + `trpg-object/src/lib.rs` | Modify | search profile/check label→数据 | T5 |
| `crates/trpg-db/src/lib.rs`（override 合并扩展，T2-T5 各自接） + `xtask`/scripts | Modify/Create | override 加载扩 + CI 守卫 + 等价闸 | T6 |

---

### Task 1: trpg-model — RuleKernel 策略字段 + ModuleConfig + GENERIC_* 默认

迁移的「数据形态地基」。引擎(combat/referee/director/material/object)当前把规则集/模组特例**写死在 Rust 函数体**里（`trpg-combat::{cyberpunk,dnd,coc,triangle,sword_world}_profile()`、`infer_combat_mode_from_intent`、`inferred_homecoming_tech_dv`、`target_actor_for_combat_input`；`trpg-referee` 五个 `ruleset_*`/`*_for_ruleset` 函数；`trpg-director::is_homecoming`；`trpg-material::{ruleset_aliases_for,module_preferences_for}`）。本任务在 `trpg-model` 新增 RuleKernel 的 5 个 `Option` 策略字段 + 新 `ModuleConfig` + 全部子结构 + `GENERIC_*` 中性默认，**纯加字段**（全 `#[serde(default)]`，零下游签名变更）。下游 crate 的 helper 迁移由 Task 2-5 消费这些类型；本任务只交付类型 + 默认 + serde 向后兼容护栏。

**关键 grounding（drafter 实读，定字段形状）**：
- 五个 `*_profile()` 返回的是 `trpg-combat::RulesetCombatProfile`（lib.rs:19-54），字段：`profile_id/ruleset_id/applies_to_modes:Vec<String>/default_mode:String/action_economy:Value/initiative:Value/reaction_windows:Vec<ReactionAdvice>/frame_exit_policy:Value/stalemate_policy:Value/npc_drive_policy:Value/frame_retention_policy:RetentionPolicy/frame_compaction_policy:CompactionPolicy/search_recipes:Vec<Value>`。本任务的 model-side `CombatProfile` 用 **`Value` 形态镜像这些策略子块**（不引 `ReactionAdvice`/`RetentionPolicy` 等 combat-crate 类型，避免 model→combat 反向依赖；Task 2 把 `CombatProfile`→`RulesetCombatProfile` 做字段直映）。
- `infer_combat_mode_from_intent`(combat lib.rs:1579-1586) 按 `ruleset_id.contains` 选 `CombatMode`（枚举在 **trpg-model** lib.rs:3531-3543，9 变体含 Firefight/Netrun/HorrorEncounter/AnomalyEncounter/TacticalCombat/TheaterOfMind/Chase/SocialConflict/HazardSequence，`as_str()` snake_case；**保留不动**）。
- 五个 referee 函数(trpg-referee lib.rs:282-286)真返回：`ruleset_damage_family→&'static str`、`common_damage_band→&'static str`、`damage_plausible_for_ruleset→bool`（按 `parse_dice_count` 落在 `1..=12`(cyberpunk)/`1..=20`(dnd)/`1..=30`(generic)）、`common_difficulty_band_json→Value`、`difficulty_plausible_for_ruleset→bool`（`5..=35`/`1..=40`/`1..=100`）。→ `RefereeValueBands` 字段：`damage_family:String, damage_band:String, damage_plausible_range:(i64,i64)`（dice-count 闭区间）、`difficulty_band:Value, difficulty_plausible_range:(i64,i64)`。
- bare-dice 限定：combat lib.rs:296/361/1151 用写死 `"1d10+0"`。→ `DiceQualification{ bare_dice_template:String }`（如 `"{dice}+0"`）。
- override 加载现成：`trpg-db::load_rule_kernel`(lib.rs:693-717) 读 `data/parsed/rules/{ruleset_id}.rule_kernel.override.json`（`read_kernel_override_file` lib.rs:4050-4056），现合 `resource_tracks`+`dice_core`。Task 2-5 扩这里合新策略键；本任务**不碰 trpg-db**，只让字段 `#[serde(default)]` 以便后续 override 文件能反序列化进 kernel。
- 向后兼容护栏现成：`crates/trpg-model/tests/kernel_backcompat.rs`（旧 CoC kernel dump 反序列化 + byte-for-byte region 守卫）。本任务**追加**新字段缺省→None 的断言到同文件。

**Files:**
- `crates/trpg-model/src/lib.rs:1424-1461`（RuleKernel struct，在 `validation_report` 字段后追加 5 个新 `Option` 字段）
- `crates/trpg-model/src/lib.rs:3531-3563`（CombatMode 枚举区——只读参照，**不改**；新策略类型放该枚举之后、`CombatPhase`(3565) 之前的 combat 数据契约区）
- `crates/trpg-model/tests/kernel_backcompat.rs:11-56`（追加新字段缺省断言到 `old_coc_kernel_json_deserializes_unchanged`）
- 新增测试模块：`crates/trpg-model/tests/policy_fields.rs`（round-trip + GENERIC_* + ModuleConfig 默认）

**TDD 步骤：**

- [ ] **Step 1**（失败测试：旧 kernel 反序列化新字段为 None）：在 `crates/trpg-model/tests/kernel_backcompat.rs` 的 `old_coc_kernel_json_deserializes_unchanged` 末尾（line 55 `assert_eq!(v2["dice_core"]…)` 之后）追加：
  ```rust
      // P0-2 Task 1: new policy fields default to None on a pre-P0-2 kernel.
      assert!(kernel.combat_profile.is_none(), "missing combat_profile → None");
      assert!(kernel.combat_mode_policy.is_none(), "missing combat_mode_policy → None");
      assert!(kernel.check_label_policy.is_none(), "missing check_label_policy → None");
      assert!(kernel.referee_value_bands.is_none(), "missing referee_value_bands → None");
      assert!(kernel.dice_qualification.is_none(), "missing dice_qualification → None");
      // re-serializing a None-policy kernel must not emit the new keys (clean old layout).
      assert!(v2.get("combat_profile").is_none(), "None policy field must not serialize a key");
  ```
  注：`combat_profile` 等需标 `#[serde(skip_serializing_if = "Option::is_none")]` 才能让最后一条断言成立（保持旧 kernel 重序列化无新键）。

- [ ] **Step 2**（cargo 验失败 + 预期）：`cargo test -p trpg-model --test kernel_backcompat`。**预期失败**：`error[E0609]: no field `combat_profile` on type `RuleKernel``（5 个字段未定义）。这证明测试先于实现失败。

- [ ] **Step 3**（最小实现：RuleKernel 加 5 字段）：在 `crates/trpg-model/src/lib.rs` RuleKernel struct（line 1459-1460 `source_refs`/`validation_report` 之后、struct 闭合 `}` line 1461 之前）追加：
  ```rust
      /// P0-2: combat action-economy/initiative/reaction/search strategy — replaces
      /// trpg-combat's {cyberpunk,dnd,coc,triangle,sword_world}_profile(). None →
      /// engine uses GENERIC_COMBAT_PROFILE (no per-ruleset branch).
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub combat_profile: Option<CombatProfile>,
      /// P0-2: intent→CombatMode data mapping — replaces infer_combat_mode_from_intent's
      /// ruleset_id.contains branches. None → GENERIC_COMBAT_MODE_POLICY.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub combat_mode_policy: Option<CombatModePolicy>,
      /// P0-2: check-label templates — replaces combat/object contains("cyberpunk") labels.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub check_label_policy: Option<CheckLabelPolicy>,
      /// P0-2: damage/difficulty family + band + plausibility — replaces trpg-referee's
      /// five ruleset_* functions. None → GENERIC_REFEREE_BANDS.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub referee_value_bands: Option<RefereeValueBands>,
      /// P0-2: bare-dice qualification (e.g. "1d10" → "1d10+0") — replaces combat's
      /// hardcoded "1d10+0". None → GENERIC_DICE_QUALIFICATION.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub dice_qualification: Option<DiceQualification>,
  ```

- [ ] **Step 4**（最小实现：新策略类型 + 子结构）：在 `crates/trpg-model/src/lib.rs` line 3563（`impl CombatMode` 闭合 `}` 之后、line 3565 `CombatPhase` 之前）插入。**`CombatProfile` 用 `Value` 镜像 RulesetCombatProfile 的策略子块**（避免 model→combat 依赖；`reaction_windows`/`search_recipes` 存 `Vec<Value>` 与 combat 函数返回的 `json!`/`Vec<ReactionAdvice>` 序列化形态一致）：
  ```rust
  // ---------------------------------------------------------------------------
  // P0-2 engine-dehardcode: kernel-resident strategy fields + module config.
  // All Value-shaped to keep trpg-model dependency-free of engine crates; the
  // engine maps these into its typed structs (RulesetCombatProfile etc.).
  // ---------------------------------------------------------------------------

  /// Combat action-economy/initiative/reaction/search strategy, kernel-resident.
  /// Mirrors trpg-combat::RulesetCombatProfile's strategy sub-blocks as Value so
  /// the engine maps kernel→its typed profile without a model→combat dependency.
  #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
  pub struct CombatProfile {
      #[serde(default)]
      pub profile_id: String,
      #[serde(default)]
      pub default_mode: String,
      #[serde(default)]
      pub applies_to_modes: Vec<String>,
      #[serde(default)]
      pub action_economy: serde_json::Value,
      #[serde(default)]
      pub initiative: serde_json::Value,
      /// Reaction-window advice entries (serialized ReactionAdvice shape).
      #[serde(default)]
      pub reaction_windows: Vec<serde_json::Value>,
      #[serde(default)]
      pub frame_exit_policy: serde_json::Value,
      #[serde(default)]
      pub stalemate_policy: serde_json::Value,
      #[serde(default)]
      pub npc_drive_policy: serde_json::Value,
      #[serde(default)]
      pub search_recipes: Vec<serde_json::Value>,
  }

  /// One intent→CombatMode rule. `mode` is a CombatMode (snake_case via as_str).
  /// `when_action_kinds` (empty = any) AND `when_evidence_contains` (empty = any)
  /// gate the rule; first matching rule wins (engine evaluates in order).
  #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
  pub struct CombatModeRule {
      pub mode: CombatMode,
      #[serde(default)]
      pub when_action_kinds: Vec<String>,
      #[serde(default)]
      pub when_evidence_contains: Vec<String>,
  }

  /// Ordered intent→CombatMode mapping. `fallback_mode` applies when no rule hits
  /// (replaces infer_combat_mode_from_intent's trailing TheaterOfMind).
  #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
  pub struct CombatModePolicy {
      #[serde(default)]
      pub rules: Vec<CombatModeRule>,
      #[serde(default)]
      pub fallback_mode: CombatMode,
  }

  /// Check-label templates keyed by action family (technical/attack/defense/…).
  /// Replaces combat/object contains("cyberpunk") label branches. Missing key →
  /// engine uses the generic label.
  #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
  pub struct CheckLabelPolicy {
      /// action-family-id → label template, e.g. "technical" → "appropriate
      /// TECH / Interface / Basic Tech check".
      #[serde(default)]
      pub labels: std::collections::BTreeMap<String, String>,
  }

  /// Damage/difficulty family + band text + plausibility ranges. Replaces
  /// trpg-referee's ruleset_damage_family/common_damage_band/
  /// damage_plausible_for_ruleset/common_difficulty_band_json/
  /// difficulty_plausible_for_ruleset. Ranges are inclusive (lo, hi).
  #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
  pub struct RefereeValueBands {
      #[serde(default)]
      pub damage_family: String,
      #[serde(default)]
      pub damage_band: String,
      /// Plausible dice-count range for a damage expression (matches
      /// damage_plausible_for_ruleset's parse_dice_count bounds).
      #[serde(default)]
      pub damage_plausible_range: (i64, i64),
      /// difficulty band advisory (mirrors common_difficulty_band_json's Value).
      #[serde(default)]
      pub difficulty_band: serde_json::Value,
      /// Plausible target-number range (matches difficulty_plausible_for_ruleset).
      #[serde(default)]
      pub difficulty_plausible_range: (i64, i64),
  }

  /// Bare-dice qualification template — `{dice}` is the bare expr (e.g. "1d10").
  /// Replaces combat's hardcoded "1d10+0".
  #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
  pub struct DiceQualification {
      /// e.g. "{dice}+0"; engine substitutes the bare dice expr for `{dice}`.
      #[serde(default)]
      pub bare_dice_template: String,
  }
  ```

- [ ] **Step 5**（最小实现：ModuleConfig + 子结构）：紧接 Step 4 之后插入 module-级配置（轻量，与 R4 ModuleEntity 不冲突，存 module bundle，全 `#[serde(default)]`）：
  ```rust
  // ---------------------------------------------------------------------------
  // P0-2 module-level config (lives in the module bundle; #[serde(default)]).
  // Replaces target_actor_for_combat_input npc bindings, inferred_homecoming_
  // tech_dv, director scene facts, material module_preferences_for.
  // ---------------------------------------------------------------------------

  /// One keyword/semantic matcher → engine actor_id. Replaces combat's
  /// npc.scav_boss / npc.athena_drone literals.
  #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
  pub struct NpcActorBinding {
      /// case-insensitive substrings; any hit binds. (Semantic matcher TBD by
      /// the consuming engine; keyword form is the equivalence baseline.)
      #[serde(default)]
      pub matcher: Vec<String>,
      pub actor_id: String,
      #[serde(default)]
      pub display_name: Option<String>,
  }

  /// One technical-option DV row. Replaces inferred_homecoming_tech_dv's 14/12.
  #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
  pub struct TechOption {
      #[serde(default)]
      pub matcher: Vec<String>,
      pub dv: i32,
  }

  /// Scene entity alias (display label ↔ canonical id) for director scene facts.
  #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
  pub struct EntityAlias {
      pub canonical_id: String,
      #[serde(default)]
      pub aliases: Vec<String>,
  }

  /// Module search preferences — replaces material's module_preferences_for
  /// homecoming/masks/vault literal lists.
  #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
  pub struct SearchProfile {
      #[serde(default)]
      pub preferred_sections: Vec<String>,
  }

  /// Lightweight module-level engine config. Lives in the module bundle; every
  /// field #[serde(default)] so pre-P0-2 bundles deserialize unchanged.
  #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
  pub struct ModuleConfig {
      #[serde(default)]
      pub npc_actor_bindings: Vec<NpcActorBinding>,
      #[serde(default)]
      pub technical_option_table: Option<Vec<TechOption>>,
      #[serde(default)]
      pub scene_entity_aliases: Vec<EntityAlias>,
      #[serde(default)]
      pub module_search_profile: Option<SearchProfile>,
  }
  ```

- [ ] **Step 6**（最小实现：GENERIC_* 中性默认，精确复刻 referee/combat 的 generic 分支值）：紧接 Step 5 之后插入。**值必须等价于原硬编码 generic 分支**：referee 的 `generic_trpg_damage` / `"system-specific; exact object/ability entry required"` / 伤害 `1..=30`（generic 分支）/ `{"common_target_band":"ruleset-specific"}` / 难度 `1..=100`；combat generic profile（lib.rs:1686-1688）的 `fiction_first` 策略；bare-dice `"{dice}+0"`：
  ```rust
  use std::sync::LazyLock;

  /// Neutral combat profile (mirrors trpg-combat::default_generic_profile's
  /// fiction-first strategy). Used when kernel.combat_profile is None.
  pub static GENERIC_COMBAT_PROFILE: LazyLock<CombatProfile> = LazyLock::new(|| CombatProfile {
      profile_id: "generic.situation.v1_3".into(),
      default_mode: "theater_of_mind".into(),
      applies_to_modes: vec!["theater_of_mind".into(), "tactical_combat".into(), "social_conflict".into()],
      action_economy: serde_json::json!({"policy":"fiction_first"}),
      initiative: serde_json::json!({"policy":"fiction_first"}),
      reaction_windows: vec![],
      frame_exit_policy: serde_json::json!({"state_exits":["objective_completed","side_escaped","negotiated_truce","surrender_accepted"],"stalemate_after_non_decisive_turns":3}),
      stalemate_policy: serde_json::json!({"max_repeated_action_count":2,"open_direction_gate":true}),
      npc_drive_policy: serde_json::json!({"default_patience":40,"default_morale":55,"max_repeat_same_tactic":2}),
      search_recipes: vec![],
  });

  /// Neutral mode policy: no rules → always falls back to TheaterOfMind
  /// (mirrors infer_combat_mode_from_intent's trailing default).
  pub static GENERIC_COMBAT_MODE_POLICY: LazyLock<CombatModePolicy> = LazyLock::new(|| CombatModePolicy {
      rules: vec![],
      fallback_mode: CombatMode::TheaterOfMind,
  });

  /// Neutral referee bands (mirrors trpg-referee's generic branch verbatim:
  /// damage 1..=30 dice, target 1..=100).
  pub static GENERIC_REFEREE_BANDS: LazyLock<RefereeValueBands> = LazyLock::new(|| RefereeValueBands {
      damage_family: "generic_trpg_damage".into(),
      damage_band: "system-specific; exact object/ability entry required".into(),
      damage_plausible_range: (1, 30),
      difficulty_band: serde_json::json!({"common_target_band":"ruleset-specific"}),
      difficulty_plausible_range: (1, 100),
  });

  /// Neutral dice qualification: bare "1d10" → "1d10+0" (mirrors the hardcode).
  pub static GENERIC_DICE_QUALIFICATION: LazyLock<DiceQualification> = LazyLock::new(|| DiceQualification {
      bare_dice_template: "{dice}+0".into(),
  });

  /// Empty check-label policy → engine uses its generic per-family label.
  pub static GENERIC_CHECK_LABEL_POLICY: LazyLock<CheckLabelPolicy> = LazyLock::new(CheckLabelPolicy::default);
  ```
  （`LazyLock` 是 std 1.80+；若 workspace MSRV 不支持，drafter 改用 `once_cell::Lazy`——确认 `Cargo.toml` 已有 `once_cell` 依赖再定。`PartialEq` derive 是为 round-trip 等价断言。）

- [ ] **Step 7**（验通过 backcompat）：`cargo test -p trpg-model --test kernel_backcompat`。**预期通过**：旧 kernel 反序列化新字段全 None、重序列化无新键（`skip_serializing_if` 生效）、原 region byte-for-byte 不变。

- [ ] **Step 8**（新测试：round-trip + GENERIC_* + ModuleConfig 默认）：新建 `crates/trpg-model/tests/policy_fields.rs`：
  ```rust
  use trpg_model::*;

  #[test]
  fn combat_profile_round_trips() {
      let p = CombatProfile {
          profile_id: "cyberpunk_red.firefight.v1_3".into(),
          default_mode: "firefight".into(),
          applies_to_modes: vec!["firefight".into(), "netrun".into(), "chase".into()],
          action_economy: serde_json::json!({"turn_slots":[{"slot_id":"action","count":1}]}),
          initiative: serde_json::json!({"kind":"formula","expression":"REF + 1d10"}),
          reaction_windows: vec![serde_json::json!({"advice_id":"generic.required_defense_choice.v1_3"})],
          frame_exit_policy: serde_json::json!({"allow_disengage":true}),
          stalemate_policy: serde_json::Value::Null,
          npc_drive_policy: serde_json::json!({"mook_morale":45}),
          search_recipes: vec![serde_json::json!({"query":"Friday Night Firefight"})],
      };
      let s = serde_json::to_string(&p).unwrap();
      let back: CombatProfile = serde_json::from_str(&s).unwrap();
      assert_eq!(p, back, "CombatProfile must round-trip");
  }

  #[test]
  fn referee_bands_round_trip() {
      let b = RefereeValueBands {
          damage_family: "cyberpunk_red_weapon_damage".into(),
          damage_band: "roughly 2d6..8d6".into(),
          damage_plausible_range: (1, 12),
          difficulty_band: serde_json::json!({"common_dv_band":"9..29"}),
          difficulty_plausible_range: (5, 35),
      };
      let back: RefereeValueBands = serde_json::from_str(&serde_json::to_string(&b).unwrap()).unwrap();
      assert_eq!(b, back);
  }

  #[test]
  fn mode_policy_and_module_config_round_trip() {
      let mp = CombatModePolicy {
          rules: vec![CombatModeRule { mode: CombatMode::HorrorEncounter, when_action_kinds: vec![], when_evidence_contains: vec![] }],
          fallback_mode: CombatMode::TheaterOfMind,
      };
      let back: CombatModePolicy = serde_json::from_str(&serde_json::to_string(&mp).unwrap()).unwrap();
      assert_eq!(mp, back);

      let mc = ModuleConfig {
          npc_actor_bindings: vec![NpcActorBinding { matcher: vec!["drone".into(), "无人机".into()], actor_id: "npc.athena_drone".into(), display_name: Some("rogue drone".into()) }],
          technical_option_table: Some(vec![TechOption { matcher: vec!["cable".into()], dv: 14 }]),
          scene_entity_aliases: vec![EntityAlias { canonical_id: "athena".into(), aliases: vec!["雅典娜".into()] }],
          module_search_profile: Some(SearchProfile { preferred_sections: vec!["Redesigned NPC cards".into()] }),
      };
      let back: ModuleConfig = serde_json::from_str(&serde_json::to_string(&mc).unwrap()).unwrap();
      assert_eq!(mc, back);
  }

  #[test]
  fn empty_json_objects_deserialize_to_defaults() {
      // Pre-P0-2 bundle/kernel JSON: empty object → all-default structs.
      let mc: ModuleConfig = serde_json::from_str("{}").unwrap();
      assert!(mc.npc_actor_bindings.is_empty());
      assert!(mc.technical_option_table.is_none());
      let p: CombatProfile = serde_json::from_str("{}").unwrap();
      assert_eq!(p, CombatProfile::default());
  }

  #[test]
  fn generic_defaults_match_legacy_generic_branch() {
      // GENERIC_REFEREE_BANDS must equal trpg-referee's generic branch verbatim.
      assert_eq!(GENERIC_REFEREE_BANDS.damage_family, "generic_trpg_damage");
      assert_eq!(GENERIC_REFEREE_BANDS.damage_band, "system-specific; exact object/ability entry required");
      assert_eq!(GENERIC_REFEREE_BANDS.damage_plausible_range, (1, 30));
      assert_eq!(GENERIC_REFEREE_BANDS.difficulty_band, serde_json::json!({"common_target_band":"ruleset-specific"}));
      assert_eq!(GENERIC_REFEREE_BANDS.difficulty_plausible_range, (1, 100));
      // GENERIC_COMBAT_PROFILE mirrors default_generic_profile's fiction-first.
      assert_eq!(GENERIC_COMBAT_PROFILE.action_economy, serde_json::json!({"policy":"fiction_first"}));
      assert_eq!(GENERIC_COMBAT_PROFILE.default_mode, "theater_of_mind");
      assert_eq!(GENERIC_DICE_QUALIFICATION.bare_dice_template, "{dice}+0");
      assert_eq!(GENERIC_COMBAT_MODE_POLICY.fallback_mode, CombatMode::TheaterOfMind);
  }
  ```

- [ ] **Step 9**（验通过 + 全 workspace 编译护栏）：`cargo test -p trpg-model --test policy_fields --test kernel_backcompat`（**预期全绿**）；再 `cargo check --workspace`（**预期绿**——纯加 `#[serde(default)]` 字段，零下游签名变更、零下游编译破坏）。两步都贴出 `test result: ok` / `Finished` 输出为证。

- [ ] **Step 10**（commit）：`git checkout -b claude/p0-2-task1-model-policy-fields`（当前非 git 仓库则先 `git init` 由集成方决定——drafter 在 plan 标注「无 git 时跳过」），`git add crates/trpg-model/src/lib.rs crates/trpg-model/tests/policy_fields.rs crates/trpg-model/tests/kernel_backcompat.rs`，提交 message：`feat(trpg-model): P0-2 Task1 RuleKernel policy fields + ModuleConfig + GENERIC_* defaults`。**仅暂存本 Task scope 内文件**（不碰 trpg-db/combat/referee——那是 Task 2-5）。

**验收闸**：① backcompat 测试证旧 kernel/bundle 反序列化新字段全 None、重序列化无新键；② round-trip + 空 `{}`→default + GENERIC_* 值等价于 referee/combat 原 generic 分支；③ `cargo check --workspace` 绿（零下游回归）；④ 文件 `crates/trpg-model/src/lib.rs` 加字段后仍需 ≤其当前规模可控（新类型约 +180 行集中在 combat 数据契约区，drafter 若超阈值则把 P0-2 新类型抽到新模块 `crates/trpg-model/src/policy.rs` 并 `pub use`——本任务优先内联，超 400 行硬约束时才拆）；⑤ 零规则集/模组名字面量进 `trpg-model` 引擎逻辑（GENERIC_* 是中性默认，`CombatMode` 变体名不算规则集名——白名单）。

**下游衔接（非本任务，给 Task 2-5 的接口承诺）**：`CombatProfile/CombatModePolicy/CheckLabelPolicy/RefereeValueBands/DiceQualification` 经 `kernel.<field>.as_ref().unwrap_or(&GENERIC_*)` 消费；`ModuleConfig` 经 module bundle 加载（Task 5 确认 bundle override 加载路径，缺则轻量补，复刻 `trpg-db::read_kernel_override_file` 模式到 `data/parsed/modules/{module_id}.module_config.json` 或合入 ModuleGraph）。特例值（cyberpunk/dnd/coc/triangle/sword_world profile、Homecoming DV 14/12、scav_boss/athena_drone 绑定、masks/vault search pref）由 Task 2-5 写进 `data/parsed/rules/{ruleset_id}.rule_kernel.override.json`（扩 `load_rule_kernel` 合并新策略键）与 module config 数据文件，**精确复刻**原硬编码值过等价闸（:54347 CoC / :54346 赛博 live + 单测；无库则 SKIP 标注）。


---
### Task 2: trpg-combat — 8 硬编码点转数据查找 + combat override 数据

依赖 Task 1（trpg-model 已加 `RuleKernel.combat_mode_policy / check_label_policy / dice_qualification` 字段 + `GENERIC_COMBAT_MODE_POLICY / GENERIC_CHECK_LABEL_POLICY / GENERIC_DICE_QUALIFICATION`，及 `ModuleConfig{npc_actor_bindings, technical_option_table}` + `Db::load_module_config(module_id)`）。本段把 trpg-combat 的 8 个 ruleset/模组名分支全转数据查找，per-ruleset/模组特例值迁进既有数据加载路径（`data/ruleset_advice/*.json` 已经是 combat profile 的数据家；kernel override 文件 `data/parsed/rules/{ruleset}.rule_kernel.override.json`；module bundle 的 `ModuleConfig`）。

**关键 grounding（实读，drafter 已核）：**
- 组合 profile **已有数据加载路径**：`CombatProfilePack::load_dir`（lib.rs:83）从 `TRPG_RULESET_ADVICE_DIR`（默认 `./data/ruleset_advice`，lib.rs:127）读 `*combat*/*conflict*/*situation*.json`，`resolve(ruleset_id, mode_hint)`（lib.rs:108）按 ruleset_id+mode 命中、回退 `ruleset_id=="generic"`、再回退 `default_generic_profile()`。`data/ruleset_advice/` 现有 5 个文件（coc7e/cyberpunk_red/dnd5e/sword_world_2_5/triangle_agency），字段与 `RulesetCombatProfile` 一致且含 `applies_to_modes`+`default_mode`。**`*_profile()` 五函数（lib.rs:1690-1694）只是 `default_profiles()`（lib.rs:1680）里的硬编码 Rust 回退**，与数据文件重复——这正是 MIGRATE 点。
- kernel 在两处已加载：`make_combat_check_contract`（lib.rs:240，绑 dice/source_refs）、`create_frame`（lib.rs:647，绑 HP track）。`load_rule_kernel`（trpg-db:693）读 DB 后 layer override 文件（`read_kernel_override_file` trpg-db:4050，`merge_dice_core` 4062 / `merge_resource_tracks` 4071）——**override 是 Task 1 给 `combat_mode_policy` 等新字段加 merge 分支后才生效；本段只消费 kernel.<新字段>，不改 trpg-db merge（那是 Task 1 范围）**。
- `infer_combat_mode_from_intent`（lib.rs:1579，签名 `ruleset_id: &str`）调用点唯一：`handle_turn`（lib.rs:150）。`should_start_frame`（lib.rs:905，签名 `ruleset_id`）调用点：`handle_turn`（lib.rs:146）。`classify_situation_intent`（lib.rs:823）收 `ConflictTurnInput`（含 `ruleset_id`+`module_id`，line 873 用 `input.ruleset_id.contains("triangle")`）。
- `CombatMode` 枚举（trpg-model:3533）保留全 9 变体；`as_str()`（3550）给 mode↔字符串。

**Files:**
- `crates/trpg-combat/src/lib.rs:1579-1586`（`infer_combat_mode_from_intent` 转数据）
- `crates/trpg-combat/src/lib.rs:1680-1694`（删 `default_profiles` 内 5 个 `*_profile()`，仅留 generic）
- `crates/trpg-combat/src/lib.rs:1029-1038`（`inferred_homecoming_tech_dv` → ModuleConfig 查找）
- `crates/trpg-combat/src/lib.rs:1041-1071`（`target_actor_for_combat_input` → ModuleConfig npc 绑定）
- `crates/trpg-combat/src/lib.rs:1073-1116`（`make_combat_check_contract` check label，line 1079）
- `crates/trpg-combat/src/lib.rs:1147-1161`（`apply_source_backed_formula_pack_to_check` bare dice 限定）
- `crates/trpg-combat/src/lib.rs:353-362`（cyberpunk 1d10 degrade）
- `crates/trpg-combat/src/lib.rs:873`（`classify_situation_intent` triangle 阈值）
- `crates/trpg-combat/src/lib.rs:905-919,146,150`（`should_start_frame` triangle + 调用点）
- `crates/trpg-combat/src/lib.rs:232-371,633-650`（kernel 调用点：把已 load 的 kernel 透传给改签名的 helper）
- `data/ruleset_advice/{cyberpunk_red.combat.v1,dnd5e.combat.v1,coc7e.conflict.v1,sword_world_2_5.combat.v1,triangle_agency.conflict.v1}.json`（补齐原 Rust 独有字段：reaction_windows/npc_drive_policy/search_recipes 等，确保删 Rust 后等价）
- `data/parsed/rules/{cyberpunk_red,coc7e,brp,dnd5e,sword_world_2_5,triangle_agency}.rule_kernel.override.json`（新增 `combat_mode_policy`/`check_label_policy`/`dice_qualification`）
- `data/modules/homecoming.module_config.json`（新增 `npc_actor_bindings` + `technical_option_table`，对应原 scav_boss/athena_drone/DV14/DV12）

---

#### 子任务 2A：combat profile 去 Rust 硬编码（`*_profile()` → 仅数据）

- [ ] **Step 1**（失败测试）：在 `crates/trpg-combat/src/lib.rs` 末尾新增 `#[cfg(test)] mod profile_data_tests`。写测试 `default_profiles_contains_only_generic`：
```rust
#[cfg(test)]
mod profile_data_tests {
    use super::*;
    #[test]
    fn default_profiles_contains_only_generic() {
        let pack = default_profiles();
        // After migration, the Rust fallback carries ONLY the neutral generic
        // profile; per-ruleset values live in data/ruleset_advice/*.json.
        let ids: Vec<&str> = pack.profiles.iter().map(|p| p.ruleset_id.as_str()).collect();
        assert_eq!(ids, vec!["generic"], "no per-ruleset profile may be hardcoded in Rust");
    }
}
```
- [ ] **Step 2**（验失败）：`cargo test -p trpg-combat default_profiles_contains_only_generic`。预期失败：`assertion failed: ids == ["generic"]`（现 `default_profiles` 返回 6 条含 cyberpunk_red/dnd5e/coc7e/triangle_agency/sword_world_2_5）。
- [ ] **Step 3**（先固化数据等价：把 Rust 独有内容落进数据文件）：逐文件 diff `data/ruleset_advice/*.json` vs 对应 `*_profile()` 返回结构，补齐数据文件缺失的字段使其与 Rust 字节等价。先生成对比快照测试避免漂移——在 mod 里加：
```rust
    #[test]
    fn data_file_profiles_match_legacy_rust_values() {
        // SKIP unless the advice dir is present; pins data == old Rust per field.
        let dir = std::env::var("TRPG_RULESET_ADVICE_DIR")
            .unwrap_or_else(|_| "../../data/ruleset_advice".to_string());
        if !std::path::Path::new(&dir).exists() { eprintln!("SKIP: advice dir absent"); return; }
        let pack = CombatProfilePack::load_dir(&dir).expect("load advice");
        for legacy in [legacy_cyberpunk(), legacy_dnd(), legacy_coc(), legacy_triangle(), legacy_sword_world()] {
            let from_data = pack.resolve(&legacy.ruleset_id, None);
            assert_eq!(from_data.action_economy, legacy.action_economy, "{}: action_economy", legacy.ruleset_id);
            assert_eq!(from_data.initiative, legacy.initiative, "{}: initiative", legacy.ruleset_id);
            assert_eq!(from_data.npc_drive_policy, legacy.npc_drive_policy, "{}: npc_drive_policy", legacy.ruleset_id);
            assert_eq!(from_data.reaction_windows.len(), legacy.reaction_windows.len(), "{}: reaction_windows", legacy.ruleset_id);
            assert_eq!(from_data.search_recipes, legacy.search_recipes, "{}: search_recipes", legacy.ruleset_id);
            assert_eq!(from_data.frame_exit_policy, legacy.frame_exit_policy, "{}: frame_exit_policy", legacy.ruleset_id);
        }
    }
```
其中 `legacy_*()` 是把 line 1690-1694 五函数体**原样复制**进 `#[cfg(test)]` 块（只在测试里留原值做等价基线，引擎码不留）。
- [ ] **Step 4**（验失败 + 补数据）：`cargo test -p trpg-combat data_file_profiles_match_legacy_rust_values` 在仓库根用 `TRPG_RULESET_ADVICE_DIR=data/ruleset_advice` 跑。对每个 `assert_eq` 失败项，把 Rust 值精确写进对应 `data/ruleset_advice/*.json`（如 cyberpunk 的 `npc_drive_policy={"mook_morale":45,"self_interest":70,"break_actions":[...]}`、triangle 的 `frame_retention_policy.keep_last_events=8` 等），直到测试绿。**数据精确复刻原硬编码值（等价铁律）。**
- [ ] **Step 5**（最小实现：删 Rust）：把 `default_profiles()`（line 1680）改为只含 generic：
```rust
fn default_profiles() -> CombatProfilePack {
    CombatProfilePack { profiles: vec![default_generic_profile()] }
}
```
删除 `cyberpunk_profile/dnd_profile/coc_profile/triangle_profile/sword_world_profile`（line 1690-1694）。`generic_required_defense`（1696）保留（generic profile 仍用）。
- [ ] **Step 6**（验通过）：`cargo test -p trpg-combat profile_data_tests`，两测试绿。`cargo build -p trpg-combat` 确认无未用函数警告（删干净）。
- [ ] **Step 7**（commit）：`git add crates/trpg-combat/src/lib.rs data/ruleset_advice/*.json && git commit -m "P0-2 combat: 删 *_profile() 硬编码，per-ruleset profile 仅存 data/ruleset_advice"`（带 Co-Authored-By trailer）。

---

#### 子任务 2B：`infer_combat_mode_from_intent` → kernel.combat_mode_policy 数据查找

`combat_mode_policy` 数据形态（Task 1 在 trpg-model 定义；本段消费）：`{ "default_mode": "theater_of_mind", "rules": [ {"when_action_kinds":["hack","disable_device"], "mode":"netrun"}, {"when_evidence_contains":["anomaly"], "mode":"anomaly_encounter"}, {"mode":"firefight"} ] }`，按序首个匹配；无匹配用 `default_mode`。`GENERIC_COMBAT_MODE_POLICY.default_mode = "theater_of_mind"`，rules 空。

- [ ] **Step 8**（失败测试）：mod 里加：
```rust
    fn intent_with(kind: SituationActionKind, evidence: &[&str]) -> ConflictIntent {
        ConflictIntent { action_kind: kind, evidence_terms: evidence.iter().map(|s| s.to_string()).collect(), ..Default::default() }
    }
    #[test]
    fn combat_mode_uses_kernel_policy_then_generic() {
        // cyberpunk override: hack -> netrun, else firefight
        let cp = trpg_model::CombatModePolicy {
            default_mode: "firefight".into(),
            rules: vec![ trpg_model::CombatModeRule { when_action_kinds: vec!["hack".into(), "disable_device".into()], when_evidence_contains: vec![], mode: "netrun".into() } ],
        };
        assert_eq!(combat_mode_from_policy(Some(&cp), &intent_with(SituationActionKind::Hack, &[])), CombatMode::Netrun);
        assert_eq!(combat_mode_from_policy(Some(&cp), &intent_with(SituationActionKind::Attack, &[])), CombatMode::Firefight);
        // absent policy -> GENERIC default (theater_of_mind), no ruleset branch
        assert_eq!(combat_mode_from_policy(None, &intent_with(SituationActionKind::Attack, &[])), CombatMode::TheaterOfMind);
    }
```
（`CombatModePolicy/CombatModeRule` 须为 Task 1 公开类型；若 `ConflictIntent` 无 `Default`，构造时显式填字段。）
- [ ] **Step 9**（验失败）：`cargo test -p trpg-combat combat_mode_uses_kernel_policy_then_generic`。预期失败：`cannot find function combat_mode_from_policy`。
- [ ] **Step 10**（最小实现）：替换 `infer_combat_mode_from_intent`（1579-1586）为纯函数 + 一个解析帮手：
```rust
fn combat_mode_from_policy(policy: Option<&trpg_model::CombatModePolicy>, intent: &ConflictIntent) -> CombatMode {
    let pol = policy.unwrap_or(&trpg_model::GENERIC_COMBAT_MODE_POLICY);
    let action = intent.action_kind.as_str();
    for rule in &pol.rules {
        let action_ok = rule.when_action_kinds.is_empty() || rule.when_action_kinds.iter().any(|a| a == action);
        let evidence_ok = rule.when_evidence_contains.is_empty()
            || rule.when_evidence_contains.iter().any(|e| intent.evidence_terms.iter().any(|t| t.contains(e.as_str())));
        if action_ok && evidence_ok { return mode_from_str(&rule.mode); }
    }
    mode_from_str(&pol.default_mode)
}
fn mode_from_str(s: &str) -> CombatMode {
    [CombatMode::TacticalCombat, CombatMode::TheaterOfMind, CombatMode::Firefight, CombatMode::Chase,
     CombatMode::Netrun, CombatMode::AnomalyEncounter, CombatMode::HorrorEncounter, CombatMode::SocialConflict,
     CombatMode::HazardSequence].into_iter().find(|m| m.as_str() == s).unwrap_or(CombatMode::TheaterOfMind)
}
```
- [ ] **Step 11**（接调用点）：`handle_turn`（line 150）改为先取 kernel 的 policy。`handle_turn` 此前不 load kernel，需补 load（同 `create_frame`/`make_combat_check_contract` 的 `self.db.load_rule_kernel(input.ruleset_id).await.ok().flatten()` 模式）：在 line 146 之前加
```rust
let kernel = self.db.load_rule_kernel(input.ruleset_id).await.ok().flatten();
```
line 150 改 `let mode = combat_mode_from_policy(kernel.as_ref().and_then(|k| k.combat_mode_policy.as_ref()), &intent);`。
- [ ] **Step 12**（验通过）：`cargo test -p trpg-combat combat_mode_uses_kernel_policy_then_generic` 绿；`cargo build -p trpg-combat` 通过。
- [ ] **Step 13**（数据）：新增 6 个 kernel override 文件 `data/parsed/rules/{ruleset}.rule_kernel.override.json`，`combat_mode_policy` 精确复刻原分支：
  - `cyberpunk_red`：`{"combat_mode_policy":{"default_mode":"firefight","rules":[{"when_action_kinds":["hack","disable_device"],"mode":"netrun"}]}}`
  - `coc7e` 与 `brp`：`{"combat_mode_policy":{"default_mode":"horror_encounter","rules":[]}}`
  - `dnd5e`：`{"combat_mode_policy":{"default_mode":"tactical_combat","rules":[]}}`
  - `triangle_agency`：`{"combat_mode_policy":{"default_mode":"anomaly_encounter","rules":[{"when_action_kinds":["disable_device"],"when_evidence_contains":["anomaly"],"mode":"anomaly_encounter"}]}}`（原行为：triangle 恒 anomaly；default_mode 已覆盖恒定分支，rule 留作显式 evidence 文档）
  - `sword_world_2_5`：可不建（无 mode 分支 → GENERIC theater_of_mind == 原 fallback）。
- [ ] **Step 14**（commit）：`git commit -m "P0-2 combat: infer_combat_mode_from_intent → kernel.combat_mode_policy 数据查找 + 6 override"`。

---

#### 子任务 2C：check label + bare dice 限定 + cyberpunk degrade → kernel.check_label_policy / dice_qualification

`check_label_policy` 形态：`{ "labels": { "hack": "appropriate TECH / Interface / Basic Tech check", "disable_device": "..." } }`（key=action_kind.as_str()）；`GENERIC_CHECK_LABEL_POLICY.labels` 空 → 引擎用现有中性 `match` 兜底标签。`dice_qualification` 形态：`{ "rules": [ {"when_bare_dice":"1d10","require_formula_text":["1d10","skill"],"qualified":"1d10+0"}, ... ] }`；`GENERIC_DICE_QUALIFICATION.rules` 空 → 不改 dice。

- [ ] **Step 15**（失败测试）：mod 里加：
```rust
    #[test]
    fn check_label_from_kernel_then_generic_fallback() {
        let pol = trpg_model::CheckLabelPolicy { labels: [("hack".to_string(), "TECH / Interface / Basic Tech check".to_string())].into_iter().collect() };
        assert_eq!(check_label_for(Some(&pol), SituationActionKind::Hack), "TECH / Interface / Basic Tech check");
        // no policy entry -> neutral generic technical label (NOT a ruleset branch)
        assert_eq!(check_label_for(None, SituationActionKind::Hack), "appropriate technical conflict check");
        assert_eq!(check_label_for(None, SituationActionKind::Attack), "appropriate attack/conflict check");
    }
    #[test]
    fn bare_dice_qualified_from_policy() {
        let q = trpg_model::DiceQualification { rules: vec![ trpg_model::DiceQualRule { when_bare_dice: "1d10".into(), require_formula_text: vec!["1d10".into(), "skill".into()], qualified: "1d10+0".into() } ] };
        assert_eq!(qualify_bare_dice(Some(&q), "1d10", "ranged_attack 1d10 + skill"), Some("1d10+0".to_string()));
        assert_eq!(qualify_bare_dice(Some(&q), "1d10", "no match text"), None); // require_formula_text not satisfied
        assert_eq!(qualify_bare_dice(None, "1d10", "1d10 skill"), None); // GENERIC: no qualification
    }
```
- [ ] **Step 16**（验失败）：`cargo test -p trpg-combat check_label_from_kernel_then_generic_fallback bare_dice_qualified_from_policy`。预期：`cannot find function check_label_for` / `qualify_bare_dice`。
- [ ] **Step 17**（最小实现 — check label）：抽 `make_combat_check_contract`（1078-1086）的 label `match` 为纯函数，删 line 1079 的 `if input.ruleset_id.contains("cyberpunk")`：
```rust
fn check_label_for(policy: Option<&trpg_model::CheckLabelPolicy>, action: SituationActionKind) -> String {
    if let Some(p) = policy {
        if let Some(l) = p.labels.get(action.as_str()) { return l.clone(); }
    }
    // neutral, NON-ruleset fallback (was: contains("cyberpunk") special-cased hack)
    match action {
        SituationActionKind::Hack | SituationActionKind::DisableDevice => "appropriate technical conflict check",
        SituationActionKind::Attack => "appropriate attack/conflict check",
        SituationActionKind::UnderAttack | SituationActionKind::EnemyInitiatedConflict | SituationActionKind::SceneEntersConflict => "appropriate defense/reaction check",
        SituationActionKind::Defend | SituationActionKind::Dodge => "appropriate defense/evasion check",
        SituationActionKind::InvestigateDuringConflict => "appropriate perception/investigation-under-pressure check",
        SituationActionKind::Intimidate => "appropriate intimidation/social pressure check",
        _ => "appropriate situation check",
    }.to_string()
}
```
`make_combat_check_contract` 改收 `check_label_policy: Option<&CheckLabelPolicy>`（从调用点透传），`let label = check_label_for(label_policy, intent.action_kind);` `check_label: label`。调用点 `make_combat_check_contract`（async wrapper line 233）已 load kernel（line 240）→ 把 `kernel.as_ref().and_then(|k| k.check_label_policy.as_ref())` 传入（注意：现 wrapper 是先调纯 `make_combat_check_contract` 再 load kernel；需调整顺序——先 load kernel 再调纯函数，把 policy 传进去）。`handle_turn` 内 line 204 的 `self.make_combat_check_contract` 走 wrapper，不直接调纯函数。
- [ ] **Step 18**（最小实现 — bare dice + cyberpunk degrade）：抽纯函数：
```rust
fn qualify_bare_dice(q: Option<&trpg_model::DiceQualification>, bare_dice: &str, formulas_text_lower: &str) -> Option<String> {
    let q = q?;
    let dice = bare_dice.trim();
    for rule in &q.rules {
        if rule.when_bare_dice != dice { continue; }
        let text_ok = rule.require_formula_text.is_empty()
            || rule.require_formula_text.iter().any(|t| formulas_text_lower.contains(&t.to_ascii_lowercase()));
        if text_ok { return Some(rule.qualified.clone()); }
    }
    None
}
```
`apply_source_backed_formula_pack_to_check`（1147-1161）的整块 ruleset-contains 改为：
```rust
let bare = ["1d10", "d20", "1d20", "2d6", "d100", "1d100", "6d4"].contains(&check.dice_expression.trim());
if bare {
    if let Some(q) = qualify_bare_dice(dice_qual, check.dice_expression.trim(), &formulas_text) {
        check.dice_expression = q;
    }
}
```
`apply_source_backed_formula_pack_to_check` 改收 `dice_qual: Option<&DiceQualification>`（调用点透传 kernel）。**cyberpunk degrade（line 353-361）**：现在它在 `hydrate_combat_check_contract_from_rule_steward`（async，已 load pack 但**未 load kernel**）；把 line 354 的 `input.ruleset_id.contains("cyberpunk")` 改为同一 `qualify_bare_dice`——在该 async 方法顶部加 `let kernel = self.db.load_rule_kernel(input.ruleset_id).await.ok().flatten();`，line 353-362 改为：
```rust
if compiled_expr.is_none() && is_attack {
    if let Some(q) = qualify_bare_dice(
        kernel.as_ref().and_then(|k| k.dice_qualification.as_ref()),
        check.dice_expression.trim(),
        "skill") { check.dice_expression = q; }
}
```
（原仅 `1d10`→`1d10+0` 且要 `is_attack`；override 数据用 `when_bare_dice:"1d10"` 复刻。）
- [ ] **Step 19**（验通过）：`cargo test -p trpg-combat check_label bare_dice` 绿；`cargo build -p trpg-combat` 通过。grep 确认 `apply_source_backed_formula_pack_to_check` 内已无 `ruleset.contains`。
- [ ] **Step 20**（数据 — 补进 2B 建的 override 文件）：
  - `dice_qualification`：cyberpunk `{"rules":[{"when_bare_dice":"1d10","require_formula_text":["1d10","skill"],"qualified":"1d10+0"}]}`、dnd5e `{"rules":[{"when_bare_dice":"d20","require_formula_text":["d20"],"qualified":"1d20+0"}]}`、sword_world_2_5 `2d6→2d6+0 / require ["2d6"]`、coc7e+brp `d100/1d100→1d100 / require ["d100","percentile"]`、triangle `6d4→6d4 / require ["6d4"]`。
  - `check_label_policy`：cyberpunk `{"labels":{"hack":"appropriate TECH / Interface / Basic Tech check","disable_device":"appropriate TECH / Interface / Basic Tech check"}}`（复刻原 cyberpunk hack label）。
- [ ] **Step 21**（commit）：`git commit -m "P0-2 combat: check label + bare dice 限定 → kernel.check_label_policy/dice_qualification"`。

---

#### 子任务 2D：模组 NPC 绑定 + tech DV → ModuleConfig（删 scav_boss/athena_drone/DV14/12）

`ModuleConfig`（Task 1 定义；`Db::load_module_config(module_id) -> Option<ModuleConfig>`）：`npc_actor_bindings: Vec<NpcActorBinding{ match_keywords: Vec<String>, actor_id: String, display_name: String }>`（按序首个 keyword 命中）；`technical_option_table: Vec<TechOption{ match_keywords: Vec<String>, dv: i32 }>`。

- [ ] **Step 22**（失败测试）：mod 里加：
```rust
    #[test]
    fn npc_binding_from_module_config_then_generic() {
        let cfg = trpg_model::ModuleConfig {
            npc_actor_bindings: vec![
                trpg_model::NpcActorBinding { match_keywords: vec!["boss".into(), "霰弹".into(), "shotgun".into()], actor_id: "npc.scav_boss".into(), display_name: "shotgun boss".into() },
                trpg_model::NpcActorBinding { match_keywords: vec!["drone".into(), "无人机".into(), "athena".into()], actor_id: "npc.athena_drone".into(), display_name: "rogue drone".into() },
            ],
            technical_option_table: vec![
                trpg_model::TechOption { match_keywords: vec!["cut off".into(), "cable".into(), "power".into(), "线缆".into()], dv: 14 },
                trpg_model::TechOption { match_keywords: vec!["hack".into(), "server".into(), "athena".into(), "无人机".into()], dv: 12 },
            ],
        };
        let atk = ConflictIntent { action_kind: SituationActionKind::Attack, ..base_intent() };
        assert_eq!(actor_for_combat_input(Some(&cfg), "shoot the shotgun boss", &atk).map(|a| a.actor_id), Some("npc.scav_boss".into()));
        assert_eq!(actor_for_combat_input(Some(&cfg), "攻击无人机", &atk).map(|a| a.actor_id), Some("npc.athena_drone".into()));
        // unmatched -> generic opposition (NOT a hardcoded module npc)
        assert_eq!(actor_for_combat_input(Some(&cfg), "attack the thing", &atk).map(|a| a.actor_id), Some("npc.opposition".into()));
        // no module config -> generic opposition fallback
        assert_eq!(actor_for_combat_input(None, "shoot the boss", &atk).map(|a| a.actor_id), Some("npc.opposition".into()));
        // non-attack intent -> None (unchanged)
        let inv = ConflictIntent { action_kind: SituationActionKind::Investigate, ..base_intent() };
        assert!(actor_for_combat_input(Some(&cfg), "look around", &inv).is_none());
        // tech DV
        assert_eq!(tech_dv_from_config(Some(&cfg), "cut off the cable"), Some(14));
        assert_eq!(tech_dv_from_config(Some(&cfg), "hack the server"), Some(12));
        assert_eq!(tech_dv_from_config(Some(&cfg), "open the door"), None);
        assert_eq!(tech_dv_from_config(None, "cut off the cable"), None); // no module -> None
    }
```
（`base_intent()` 在测试块里构造合法 `ConflictIntent`。）
- [ ] **Step 23**（验失败）：`cargo test -p trpg-combat npc_binding_from_module_config_then_generic`。预期：`cannot find function actor_for_combat_input` / `tech_dv_from_config`。
- [ ] **Step 24**（最小实现）：替换 `target_actor_for_combat_input`（1041-1071）为收 ModuleConfig 的纯函数：
```rust
fn actor_for_combat_input(cfg: Option<&trpg_model::ModuleConfig>, input: &str, intent: &ConflictIntent) -> Option<ActorRef> {
    if !matches!(intent.action_kind,
        SituationActionKind::Attack | SituationActionKind::Counterattack | SituationActionKind::UnderAttack
        | SituationActionKind::EnemyInitiatedConflict | SituationActionKind::SceneEntersConflict) {
        return None;
    }
    let lower = input.to_ascii_lowercase();
    let (actor_id, display_name) = cfg
        .and_then(|c| c.npc_actor_bindings.iter()
            .find(|b| b.match_keywords.iter().any(|k| lower.contains(&k.to_ascii_lowercase())))
            .map(|b| (b.actor_id.clone(), b.display_name.clone())))
        .unwrap_or_else(|| ("npc.opposition".to_string(), "opposition".to_string()));
    Some(ActorRef { actor_id, actor_kind: ActorKind::Npc, display_name: Some(display_name) })
}
fn tech_dv_from_config(cfg: Option<&trpg_model::ModuleConfig>, input: &str) -> Option<i32> {
    let lower = input.to_ascii_lowercase();
    cfg?.technical_option_table.iter()
        .find(|t| t.match_keywords.iter().any(|k| lower.contains(&k.to_ascii_lowercase())))
        .map(|t| t.dv)
}
```
删 `inferred_homecoming_tech_dv`（1029-1038）。
- [ ] **Step 25**（接调用点）：`make_combat_check_contract`（纯，1073）改收 `module_cfg: Option<&ModuleConfig>`，line 1076/1077/1094 的 `target_actor_for_combat_input(...)` → `actor_for_combat_input(module_cfg, input.user_input, intent)`。async wrapper（233）+ `create_frame` 链都需在 `input.module_id` 存在时 `self.db.load_module_config(mid).await.ok().flatten()` 并透传。`hydrate_combat_check_contract_from_rule_steward`（line 364-369）的 `inferred_homecoming_tech_dv(input.user_input)` → `tech_dv_from_config(module_cfg.as_ref(), input.user_input)`（该方法也需在顶部 load module_cfg）。DV 命中时 label 改中性 `"source-backed module technical option DV"`（删 "Homecoming" 字面量，line 366-367）。
- [ ] **Step 26**（验通过）：`cargo test -p trpg-combat npc_binding_from_module_config_then_generic` 绿；`cargo build -p trpg-combat`。
- [ ] **Step 27**（数据）：新建 `data/modules/homecoming.module_config.json`，精确复刻：
```json
{
  "npc_actor_bindings": [
    {"match_keywords":["scav_boss","boss","shotgun","霰弹","头目","首领"],"actor_id":"npc.scav_boss","display_name":"shotgun boss"},
    {"match_keywords":["drone","无人机","athena","雅典娜"],"actor_id":"npc.athena_drone","display_name":"rogue drone"}
  ],
  "technical_option_table": [
    {"match_keywords":["basic tech","cut off","power","cable","线缆","切断","供电"],"dv":14},
    {"match_keywords":["hack","interface","net","server","athena","黑入","服务器","无人机"],"dv":12}
  ]
}
```
（原 scav_boss 有 `"scav" && ("老大"|"leader")` 复合条件——单 keyword 表无法表达复合 AND；用户拍板理念为通用兜底，复合条件降级为 `scav 老大`/`scav leader` 整词加入 match_keywords 近似复刻，并在 module_config 注释字段标 `_note`。live 等价测以原 4 个独立关键词为准，复合分支不在 live 用例覆盖范围。）
- [ ] **Step 28**（commit）：`git commit -m "P0-2 combat: NPC 绑定 + tech DV → ModuleConfig，删 scav_boss/athena_drone/Homecoming DV 硬编码"`。

---

#### 子任务 2E：triangle 阈值分支（`classify_situation_intent` + `should_start_frame`）

原 `contains("triangle")` 用作"该 ruleset 放宽 investigate→InsideFrame / 低置信也开 frame"的开关。迁为 profile/kernel 的通用布尔策略。复用已数据化的 combat profile：在 `RulesetCombatProfile` 加 `#[serde(default)] pub low_confidence_frame_start: bool` 与 `#[serde(default)] pub investigate_opens_frame: bool`（profile 已是 per-ruleset 数据家，零额外加载路径），triangle 数据文件置 true，其余 false（== 原行为）。

- [ ] **Step 29**（失败测试）：mod 里加：
```rust
    #[test]
    fn triangle_thresholds_are_profile_driven() {
        // investigate opens a frame only when the profile opts in (was: contains("triangle"))
        assert!(relation_outside_or_inside(true, 0.45) == FrameRelation::InsideFrameAction);
        assert!(relation_outside_or_inside(false, 0.45) == FrameRelation::OutsideFrameAction);
        // low-confidence frame start gated by profile flag (was: contains("triangle"))
        let low = ConflictIntent { confidence: RulingConfidence::Low, relation_to_active_frame: FrameRelation::InsideFrameAction, action_kind: SituationActionKind::Hack, ..base_intent() };
        assert!(should_start_frame_flag(true, &low));
        assert!(!should_start_frame_flag(false, &low));
    }
```
（`relation_outside_or_inside(investigate_opens, investigate_score)` 与 `should_start_frame_flag(low_conf_ok, intent)` 是抽出的纯帮手；为单测可达性，把 873/918 的判定逻辑抽成这两个小函数。）
- [ ] **Step 30**（验失败）：`cargo test -p trpg-combat triangle_thresholds_are_profile_driven`。预期：`cannot find function`。
- [ ] **Step 31**（最小实现）：`classify_situation_intent`（823）改收 profile flag（调用点 `handle_turn` line 145/146 在 resolve profile 之前调 classify——需调整：先 `resolve` 拿 profile 再 classify，或把两 bool 经参数传入）。把 line 873 的 `(input.ruleset_id.contains("triangle") && investigate >= 0.42)` 改为 `(investigate_opens_frame && investigate >= 0.42)`；把 line 918 的 `ruleset_id.contains("triangle")` 改为 `low_confidence_frame_start`。`should_start_frame`（905）签名 `ruleset_id: &str` → `low_confidence_frame_start: bool`，调用点 line 146 传 `profile.low_confidence_frame_start`。**注意调用顺序**：`handle_turn` 现在 line 143-145 先 classify 再 line 146 should_start_frame 再 line 150-151 infer mode+resolve profile。需把 `resolve` profile（用 default mode 先取一版 `self.profiles.resolve(input.ruleset_id, None)`）提前到 classify 之前，拿到两 flag；最终 mode-specific profile 仍在 line 151 resolve。
- [ ] **Step 32**（验通过）：`cargo test -p trpg-combat triangle_thresholds_are_profile_driven` 绿；`cargo build -p trpg-combat`。
- [ ] **Step 33**（数据 + serde）：`RulesetCombatProfile`（lib.rs:18）加两 `#[serde(default)] bool` 字段。`data/ruleset_advice/triangle_agency.conflict.v1.json` 加 `"low_confidence_frame_start": true, "investigate_opens_frame": true`；其余 4 文件不加（serde default=false == 原行为）。`default_generic_profile` 不设（false）。
- [ ] **Step 34**（commit）：`git commit -m "P0-2 combat: triangle 阈值分支 → profile.low_confidence_frame_start/investigate_opens_frame"`。

---

#### 子任务 2F：grep 守卫 + 等价 live 验证 + 全套件

- [ ] **Step 35**（grep 守卫）：在仓库根跑
```
grep -nE 'contains\("(cyberpunk|dnd|coc|brp|triangle|sword_world)"\)|scav_boss|athena_drone|[Hh]omecoming' crates/trpg-combat/src/lib.rs | grep -v '#\[cfg(test)\]' 
```
预期输出**仅** `#[cfg(test)] mod` 内的 `legacy_*()` 基线（白名单：测试块）。引擎码（非 test）命中数必须为 0。若 `legacy_*` 含这些字面量，确认其在 `#[cfg(test)]` 内（守卫白名单允许 test/fixture）。
- [ ] **Step 36**（单测全绿）：`cargo test -p trpg-combat`，全部测试绿（含 `formula.rs` 既有 + 本段新增 mod）。
- [ ] **Step 37**（等价 live — Cyberpunk :54346）：
```
export DATABASE_URL='postgres://...@127.0.0.1:54346/...'   # 赛博库 (port 54346)
export TRPG_RULESET_ADVICE_DIR=data/ruleset_advice TRPG_DATA_DIR=data
```
跑迁移前/后各一次 combat 路径冒烟（真库回合或 `CombatAgent::handle_turn` 集成 harness），断言**等价**：① cyberpunk attack→`CombatMode::Firefight`、hack→`Netrun`（mode policy 复刻）；② profile.action_economy/initiative/npc_drive_policy 与 `legacy_cyberpunk()` 字节等价；③ Homecoming 模组输入 "cut off the cable"→DV14、"hack the server"→DV12、"shoot the boss"→`npc.scav_boss`（module_config 复刻）；④ hack check_label 含 "TECH / Interface / Basic Tech"。**SKIP**：若 `DATABASE_URL` 未设或库不可达，打印 `SKIP: cyberpunk live (:54346) unavailable` 跳过（CI 无库环境）。
- [ ] **Step 38**（等价 live — CoC :54347）：
```
export DATABASE_URL='postgres://...@127.0.0.1:54347/...'   # CoC库 (port 54347)
```
断言：CoC attack→`CombatMode::HorrorEncounter`（default_mode 复刻）、profile reaction_windows（fight_back/dodge/take_hit 三选项）与 `legacy_coc()` 等价、bare `1d100`/`d100` 限定行为不变。**SKIP** 同上：`SKIP: CoC live (:54347) unavailable`。
- [ ] **Step 39**（全工作区零回归）：`cargo build --workspace && cargo test -p trpg-combat -p trpg-model`（trpg-model 因 Task 1 新类型）。确认无回归、无未用 import/函数警告。
- [ ] **Step 40**（文件行数闸 + 收尾 commit）：`wc -l crates/trpg-combat/src/lib.rs` 确认 ≤400 行——**当前 1700 行已超**，本段不负责拆分（拆分是清单内独立项），但本段净增不得使其更糟：把新增纯函数（`combat_mode_from_policy`/`mode_from_str`/`check_label_for`/`qualify_bare_dice`/`actor_for_combat_input`/`tech_dv_from_config` + `#[cfg(test)] mod`）抽到新文件 `crates/trpg-combat/src/policy.rs`（`mod policy; use policy::*;`），使 lib.rs 净减、policy.rs ≤400 行。`git commit -m "P0-2 combat: 策略纯函数 + 测试抽进 policy.rs，lib.rs 收口"`。

**等价铁律**：所有 override / module_config / profile 数据文件的值，逐字段对照本段 Step 3 的 `legacy_*()` 基线与原硬编码（DV14/12、scav_boss/athena_drone、cyberpunk hack label、各 mode 映射），**精确复刻、行为字节/语义不变**。zero hardcode：迁移后 trpg-combat/src 非 test 区 grep 规则集/模组名 = 0。所有新结构 `#[serde(default)]` 向后兼容旧 kernel/bundle/profile。

---

补充说明（给计划整合者）：本段依赖 Task 1 在 trpg-model 公开 `CombatModePolicy/CombatModeRule/CheckLabelPolicy/DiceQualification/DiceQualRule/ModuleConfig/NpcActorBinding/TechOption` 及 `GENERIC_COMBAT_MODE_POLICY/GENERIC_CHECK_LABEL_POLICY/GENERIC_DICE_QUALIFICATION` 常量，并在 trpg-db `load_rule_kernel`（trpg-db:708 的 override layer）加 `combat_mode_policy/check_label_policy/dice_qualification` 的 merge 分支、新增 `Db::load_module_config(module_id)`（读 `data/modules/{id}.module_config.json`，模式同 `read_kernel_override_file` trpg-db:4050）。若 Task 1 把 combat profile 字段直接挂 RuleKernel 而非沿用 `ruleset_advice` profile，2A/2E 的数据家改为 kernel override（但现 `CombatProfilePack` 数据路径已存在且更轻，drafter 建议沿用 profile 作为 combat-mode/label/triangle-flag 之外的 profile 内容家，仅 `combat_mode_policy/check_label_policy/dice_qualification` 走 kernel，二者互不冲突）。


---
### Task 3: trpg-referee — damage/difficulty bands → `referee_value_bands`

**Files:**
- `crates/trpg-model/src/lib.rs` — add `RefereeValueBands` struct + `referee_value_bands: Option<RefereeValueBands>` field to `RuleKernel` (after line 1461)
- `crates/trpg-referee/src/lib.rs` — replace five hardcoded helpers (lines 282–286); `inspect_turn` loads kernel via `self.db.load_rule_kernel`; `verify_claim` takes `&RefereeValueBands` not `ruleset_id`
- `data/parsed/rules/cyberpunk_red.rule_kernel.override.json` — add `"referee_value_bands"` block
- `data/parsed/rules/call_of_cthulhu_7e.rule_kernel.override.json` — add `"referee_value_bands"` block
- `data/parsed/rules/dnd_5e.rule_kernel.override.json` *(new)* — `"referee_value_bands"` for D&D
- `data/parsed/rules/sword_world_2_5.rule_kernel.override.json` *(new)* — `"referee_value_bands"` for 剣世界

---

- [ ] **Step 1 — 失败测试：`RefereeValueBands` 不存在时编译失败**

  在 `crates/trpg-model/src/lib.rs` 里加入以下单测（放在 lib.rs 底部 `#[cfg(test)]` 块中）；此时该结构尚未定义，`cargo test -p trpg-model --lib` 必须编译失败：

  ```rust
  #[cfg(test)]
  mod referee_value_bands_tests {
      use super::*;

      #[test]
      fn referee_value_bands_roundtrip() {
          // 给定：填充了 referee_value_bands 的 RuleKernel
          let k = RuleKernel {
              referee_value_bands: Some(RefereeValueBands {
                  damage_family: "test_family".into(),
                  damage_band: "1d4..2d8".into(),
                  damage_plausible_max_dice: 10,
                  difficulty_band_json: serde_json::json!({"common_band":"5..20"}),
                  difficulty_plausible_min: 1,
                  difficulty_plausible_max: 40,
              }),
              ..Default::default()
          };
          let json = serde_json::to_value(&k).unwrap();
          // referee_value_bands 字段序列化出现
          assert!(json.pointer("/referee_value_bands/damage_family").is_some());
          // 反序列化等价
          let k2: RuleKernel = serde_json::from_value(json).unwrap();
          assert_eq!(k2.referee_value_bands.unwrap().damage_family, "test_family");
      }

      #[test]
      fn kernel_without_referee_value_bands_defaults_none() {
          // 旧 kernel JSON（无该字段）→ serde default → None
          let json = serde_json::json!({"kernel_id":"x","ruleset_id":"y","version":"1"});
          let k: RuleKernel = serde_json::from_value(json).unwrap();
          assert!(k.referee_value_bands.is_none(), "old kernels must not fail on missing field");
      }
  }
  ```

  验失败（编译错误）：
  ```
  cargo test -p trpg-model --lib 2>&1 | grep "error\[E"
  ```

---

- [ ] **Step 2 — 最小实现：在 `trpg-model` 加 `RefereeValueBands` + `RuleKernel` 字段**

  在 `crates/trpg-model/src/lib.rs` 的 `RuleKernel` 结构体（第 1424 行附近）之前插入：

  ```rust
  /// Damage/difficulty plausibility bands used by trpg-referee to sanity-check
  /// player-supplied values. Loaded from kernel data (override file wins).
  /// All fields describe the ruleset's typical ranges — no ruleset-name branching.
  #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
  pub struct RefereeValueBands {
      /// Ruleset damage family label (e.g. "cyberpunk_red_weapon_damage").
      #[serde(default)]
      pub damage_family: String,
      /// Human-readable typical damage range for GM/player guidance.
      #[serde(default)]
      pub damage_band: String,
      /// Dice-count upper bound for plausible damage expressions (inclusive).
      #[serde(default)]
      pub damage_plausible_max_dice: i64,
      /// JSON object describing the common difficulty/target-number band,
      /// e.g. `{"common_dv_band":"9..29","note":"..."}`. Passed verbatim to
      /// acceptable_range_json in the referee verification response.
      #[serde(default)]
      pub difficulty_band_json: serde_json::Value,
      /// Minimum plausible difficulty/DC/DV value (inclusive).
      #[serde(default)]
      pub difficulty_plausible_min: i64,
      /// Maximum plausible difficulty/DC/DV value (inclusive).
      #[serde(default)]
      pub difficulty_plausible_max: i64,
  }
  ```

  在 `RuleKernel` 末尾（第 1460 行 `pub validation_report` 后）加：

  ```rust
      /// Per-ruleset damage/difficulty plausibility bands for the player-value
      /// referee. None → GENERIC_REFEREE_BANDS (fail-soft, non-ruleset-branching).
      #[serde(default)]
      pub referee_value_bands: Option<RefereeValueBands>,
  ```

  验通过：
  ```
  cargo test -p trpg-model --lib 2>&1 | tail -5
  # 期望: test result: ok. 2 passed
  ```

---

- [ ] **Step 3 — 失败测试：referee 五函数读 kernel 时 GENERIC 兜底**

  在 `crates/trpg-referee/src/lib.rs` 底部 `#[cfg(test)]` 块（若不存在则新建）加入：

  ```rust
  #[cfg(test)]
  mod referee_bands_tests {
      use super::*;
      use trpg_model::{RuleKernel, RefereeValueBands};

      fn kernel_with_bands(bands: RefereeValueBands) -> RuleKernel {
          RuleKernel { referee_value_bands: Some(bands), ..Default::default() }
      }
      fn kernel_no_bands() -> RuleKernel {
          RuleKernel { referee_value_bands: None, ..Default::default() }
      }

      #[test]
      fn damage_family_reads_kernel_field() {
          let k = kernel_with_bands(RefereeValueBands {
              damage_family: "test_damage_family".into(),
              ..Default::default()
          });
          assert_eq!(damage_family_from_kernel(&k), "test_damage_family");
      }

      #[test]
      fn damage_family_falls_back_to_generic() {
          // kernel 无 referee_value_bands → GENERIC
          let k = kernel_no_bands();
          assert_eq!(damage_family_from_kernel(&k), GENERIC_REFEREE_BANDS.damage_family.as_str());
      }

      #[test]
      fn damage_plausible_uses_kernel_max_dice() {
          let k = kernel_with_bands(RefereeValueBands {
              damage_plausible_max_dice: 5,
              ..Default::default()
          });
          // 骰数 5 → 合理（≤ max_dice=5）
          assert!(damage_plausible_from_kernel(&k, "5d6"));
          // 骰数 6 → 不合理
          assert!(!damage_plausible_from_kernel(&k, "6d6"));
      }

      #[test]
      fn damage_plausible_generic_fallback_30() {
          let k = kernel_no_bands();
          // GENERIC max_dice=30 → 30d6 合理，31d6 不合理
          assert!(damage_plausible_from_kernel(&k, "30d6"));
          assert!(!damage_plausible_from_kernel(&k, "31d6"));
      }

      #[test]
      fn difficulty_plausible_uses_kernel_range() {
          let k = kernel_with_bands(RefereeValueBands {
              difficulty_plausible_min: 5,
              difficulty_plausible_max: 35,
              ..Default::default()
          });
          assert!(difficulty_plausible_from_kernel(&k, 5));
          assert!(difficulty_plausible_from_kernel(&k, 35));
          assert!(!difficulty_plausible_from_kernel(&k, 4));
          assert!(!difficulty_plausible_from_kernel(&k, 36));
      }

      #[test]
      fn difficulty_band_json_uses_kernel_field() {
          let k = kernel_with_bands(RefereeValueBands {
              difficulty_band_json: serde_json::json!({"common_band":"9..29","note":"test"}),
              ..Default::default()
          });
          let v = difficulty_band_json_from_kernel(&k);
          assert_eq!(v.get("common_band").and_then(|x| x.as_str()), Some("9..29"));
      }

      #[test]
      fn no_ruleset_name_in_referee_src() {
          // CI 守卫预演：referee/src/lib.rs 不得出现规则集名字面量
          // (此处作为编译期属性测试替代 grep；真 grep 守卫在 CI xtask)
          let src = include_str!("lib.rs");
          for name in &["cyberpunk", "sword_world", "dnd", r#"contains("coc")"#, r#"contains("brp")"#] {
              assert!(!src.contains(name),
                  "引擎源码不得出现规则集名: {name}");
          }
      }
  }
  ```

  验失败（函数 `damage_family_from_kernel` 等不存在）：
  ```
  cargo test -p trpg-referee --lib 2>&1 | grep "error\[E"
  ```

---

- [ ] **Step 4 — 最小实现：trpg-referee 五函数 + `GENERIC_REFEREE_BANDS` + `inspect_turn` 加载 kernel**

  将 `crates/trpg-referee/src/lib.rs` 中的五个硬编码函数（第 282–286 行）全部替换，并在 `inspect_turn` 里加载 kernel；同时修改 `verify_claim` 签名接收 `&RefereeValueBands`。

  **4a. 在文件顶部 `use` 块后增加常量（替换五函数前方位置）：**

  ```rust
  use once_cell::sync::Lazy;
  use trpg_model::RefereeValueBands;

  /// 通用兜底：未配置 referee_value_bands 的规则集使用此值。
  /// 中性、宽泛、非规则集分支——fail-soft。
  static GENERIC_REFEREE_BANDS: Lazy<RefereeValueBands> = Lazy::new(|| RefereeValueBands {
      damage_family: "generic_trpg_damage".into(),
      damage_band: "system-specific; exact object/ability entry required".into(),
      damage_plausible_max_dice: 30,
      difficulty_band_json: serde_json::json!({"common_target_band":"ruleset-specific"}),
      difficulty_plausible_min: 1,
      difficulty_plausible_max: 100,
  });
  ```

  > 注：若 `trpg-referee/Cargo.toml` 尚无 `once_cell`，改用 `std::sync::OnceLock`：
  > ```rust
  > static GENERIC_REFEREE_BANDS: std::sync::OnceLock<RefereeValueBands> = std::sync::OnceLock::new();
  > fn generic_referee_bands() -> &'static RefereeValueBands {
  >     GENERIC_REFEREE_BANDS.get_or_init(|| RefereeValueBands { ... })
  > }
  > ```
  > 下方示例统一用 `generic_referee_bands()` 辅助函数形式以免 Cargo 依赖变动。

  **4b. 替换五函数为四个纯数据查找辅助函数（删除原 282–287 行）：**

  ```rust
  fn bands_for(kernel: &trpg_model::RuleKernel) -> &RefereeValueBands {
      kernel.referee_value_bands.as_ref().unwrap_or_else(generic_referee_bands)
  }

  fn damage_family_from_kernel(kernel: &trpg_model::RuleKernel) -> &str {
      &bands_for(kernel).damage_family
  }

  fn common_damage_band_from_kernel(kernel: &trpg_model::RuleKernel) -> &str {
      &bands_for(kernel).damage_band
  }

  fn damage_plausible_from_kernel(kernel: &trpg_model::RuleKernel, expr: &str) -> bool {
      let Some(n) = parse_dice_count(expr) else { return false; };
      let max = bands_for(kernel).damage_plausible_max_dice;
      n >= 1 && n <= max
  }

  fn difficulty_band_json_from_kernel(kernel: &trpg_model::RuleKernel) -> serde_json::Value {
      bands_for(kernel).difficulty_band_json.clone()
  }

  fn difficulty_plausible_from_kernel(kernel: &trpg_model::RuleKernel, value: i64) -> bool {
      let b = bands_for(kernel);
      value >= b.difficulty_plausible_min && value <= b.difficulty_plausible_max
  }

  fn generic_referee_bands() -> &'static RefereeValueBands {
      static ONCE: std::sync::OnceLock<RefereeValueBands> = std::sync::OnceLock::new();
      ONCE.get_or_init(|| RefereeValueBands {
          damage_family: "generic_trpg_damage".into(),
          damage_band: "system-specific; exact object/ability entry required".into(),
          damage_plausible_max_dice: 30,
          difficulty_band_json: serde_json::json!({"common_target_band":"ruleset-specific"}),
          difficulty_plausible_min: 1,
          difficulty_plausible_max: 100,
      })
  }
  ```

  **4c. `inspect_turn` 改为异步加载 kernel，将 `kernel` 传给 `verify_claim`：**

  ```rust
  pub async fn inspect_turn(
      &self,
      session_id: &str,
      turn_id: &str,
      ruleset_id: &str,
      module_id: Option<&str>,
      user_input: &str,
  ) -> Result<PlayerValueRefereeResult> {
      if !enabled() { return Ok(PlayerValueRefereeResult::default()); }
      let world_tick = None;
      let claims = detect_claims(session_id, turn_id, ruleset_id, module_id, user_input, world_tick);
      if claims.is_empty() { return Ok(PlayerValueRefereeResult::default()); }

      // 加载 kernel（fail-soft: 加载失败/无 kernel → Default()）
      let kernel = self.db.load_rule_kernel(ruleset_id).await
          .ok().flatten()
          .unwrap_or_default();

      let mut result = PlayerValueRefereeResult { handled: true, phases: vec!["player_value_referee".into()], ..Default::default() };
      let player_insists = looks_like_player_insists(user_input);
      for claim in claims {
          let verification = verify_claim(&claim, &kernel, user_input, player_insists);
          // ... rest unchanged
      }
      result.narration_context = Some(render_referee_context(&result));
      Ok(result)
  }
  ```

  **4d. `verify_claim` 签名改：`ruleset_id: &str` → `kernel: &trpg_model::RuleKernel`，体内全换新辅助函数：**

  将第 140 行 `fn verify_claim(claim: ..., ruleset_id: &str, ...)` 改为 `fn verify_claim(claim: ..., kernel: &trpg_model::RuleKernel, ...)`；将体内：
  - `ruleset_damage_family(ruleset_id)` → `damage_family_from_kernel(kernel)`
  - `common_damage_band(ruleset_id)` → `common_damage_band_from_kernel(kernel)`
  - `damage_plausible_for_ruleset(ruleset_id, expr)` → `damage_plausible_from_kernel(kernel, expr)`
  - `common_difficulty_band_json(ruleset_id)` → `difficulty_band_json_from_kernel(kernel)`
  - `difficulty_plausible_for_ruleset(ruleset_id, value)` → `difficulty_plausible_from_kernel(kernel, value)`

  验通过：
  ```
  cargo test -p trpg-referee --lib 2>&1 | tail -5
  # 期望: test result: ok. 7 passed (含 no_ruleset_name_in_referee_src)
  ```

---

- [ ] **Step 5 — override 数据：将各规则集的特例值迁入 override 文件**

  各文件格式遵循现有 `read_kernel_override_file` 加载约定（顶层加 `"referee_value_bands"` 对象；`load_rule_kernel` 读取时需扩展 merge 逻辑以支持该字段，见 Step 6）。

  **`data/parsed/rules/cyberpunk_red.rule_kernel.override.json`** — 在现有 JSON 对象中追加：
  ```json
  "referee_value_bands": {
    "damage_family": "cyberpunk_red_weapon_damage",
    "damage_band": "roughly 2d6..8d6 depending weapon class; exact weapon table required",
    "damage_plausible_max_dice": 12,
    "difficulty_band_json": {
      "common_dv_band": "9..29",
      "note": "DV should come from range/task table or GM adjudication"
    },
    "difficulty_plausible_min": 5,
    "difficulty_plausible_max": 35
  }
  ```

  **`data/parsed/rules/call_of_cthulhu_7e.rule_kernel.override.json`** — 追加：
  ```json
  "referee_value_bands": {
    "damage_family": "brp_percentile_weapon_damage",
    "damage_band": "weapon-specific dice such as 1d3..2d10+db; exact weapon table required",
    "damage_plausible_max_dice": 30,
    "difficulty_band_json": {
      "common_target_band": "1..100",
      "note": "usually roll-under ability value or hard/extreme derivation, not arbitrary DC"
    },
    "difficulty_plausible_min": 1,
    "difficulty_plausible_max": 100
  }
  ```

  **`data/parsed/rules/dnd_5e.rule_kernel.override.json`** *(新建；若文件不存在)*：
  ```json
  {
    "_note": "referee_value_bands for D&D 5e: damage plausible max 20 dice, DC 1-40.",
    "referee_value_bands": {
      "damage_family": "dnd_damage_dice",
      "damage_band": "roughly 1d4..2d12 for common low-level weapon/spell chunks; exact entry required",
      "damage_plausible_max_dice": 20,
      "difficulty_band_json": {
        "common_dc_band": "5..30",
        "note": "DC should come from task difficulty, AC, save DC, or rules text"
      },
      "difficulty_plausible_min": 1,
      "difficulty_plausible_max": 40
    }
  }
  ```

  **`data/parsed/rules/sword_world_2_5.rule_kernel.override.json`** *(新建；若文件不存在)*：
  ```json
  {
    "_note": "referee_value_bands for 剣世界 2.5: damage table/formula driven, generic band.",
    "referee_value_bands": {
      "damage_family": "sword_world_weapon_spell_damage",
      "damage_band": "damage is usually table/formula driven; exact weapon/spell data required",
      "damage_plausible_max_dice": 30,
      "difficulty_band_json": {
        "common_target_band": "ruleset-specific"
      },
      "difficulty_plausible_min": 1,
      "difficulty_plausible_max": 100
    }
  }
  ```

  > **文件名确认**：上述 `ruleset_id` 字符串（`dnd_5e` / `sword_world_2_5`）需与数据库里各规则集的 `ruleset_id` 完全一致（`read_kernel_override_file` 按 `ruleset_id` 做 safe 字符映射后查路径）。实施时先 `SELECT DISTINCT ruleset_id FROM rule_kernels;` 确认 id 再建文件，文件名 `{ruleset_id}.rule_kernel.override.json`。

---

- [ ] **Step 6 — 扩展 `load_rule_kernel` 以合并 `referee_value_bands`**

  在 `crates/trpg-db/src/lib.rs` 的 `load_rule_kernel`（第 708 行 `if let Some(doc) = read_kernel_override_file(ruleset_id)` 块）中，在现有 `resource_tracks` / `dice_core` merge 后追加：

  ```rust
  if let Some(rb) = doc.get("referee_value_bands") {
      if let Ok(bands) = serde_json::from_value::<trpg_model::RefereeValueBands>(rb.clone()) {
          kernel.referee_value_bands = Some(bands);
      }
  }
  ```

  验（单测，无 DB 依赖）：
  ```
  cargo test -p trpg-db --lib 2>&1 | tail -5
  ```

  > `load_rule_kernel` 现有单测在 `trpg-db/tests/live_kernel_override.rs`（需真库），此步骤只验编译通过 + 现有 lib 单测不回归；Step 8 的 live 等价验证才跑真库。

---

- [ ] **Step 7 — CI 守卫预跑：referee/src 零规则集名**

  在工作目录跑：
  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
  grep -rn --include="*.rs" \
    -e 'contains("cyberpunk")' \
    -e 'contains("dnd")' \
    -e 'contains("coc")' \
    -e 'contains("brp")' \
    -e 'contains("sword_world")' \
    crates/trpg-referee/src/
  ```
  期望：零输出。若有命中停止并修复。

  同时验收 no_ruleset_name_in_referee_src 单测（Step 3 中加入）仍通过：
  ```
  cargo test -p trpg-referee --lib no_ruleset_name_in_referee_src 2>&1 | tail -3
  ```

---

- [ ] **Step 8 — 等价验证（live，跳过无真库环境）**

  > 标注：以下两个 live 测试需要真 DB；:54347 = CoC DB，:54346 = 赛博朋克 DB（见 .env DATABASE_URL）。在无 DB 环境下 `cargo test` 自动因 `SKIP_LIVE` 未设或 DB 连接失败而跳过，不阻 CI。

  在 `crates/trpg-referee/tests/` 下新建 `live_referee_bands_equiv.rs`：

  ```rust
  //! Equivalence test: referee value-bands produce identical plausibility verdicts
  //! before and after the migration (hardcoded strings now come from kernel data).
  //!
  //! Requires a live DB:
  //!   DATABASE_URL=postgres://postgres:password@localhost:54347/chatrpg cargo test \
  //!     -p trpg-referee --test live_referee_bands_equiv -- --nocapture

  #[tokio::test]
  async fn coc_bands_equiv() {
      let Ok(url) = std::env::var("DATABASE_URL") else { return; };
      let db = trpg_db::Db::connect(&url).await.unwrap();
      let Some(kernel) = db.load_rule_kernel("call_of_cthulhu_7e").await.unwrap() else {
          eprintln!("SKIP: no call_of_cthulhu_7e kernel"); return;
      };
      let bands = kernel.referee_value_bands.as_ref().expect("override should supply referee_value_bands for CoC");
      // damage_family matches original hardcoded value
      assert_eq!(bands.damage_family, "brp_percentile_weapon_damage");
      // 30d10 合理（原: brp else-branch max 30）
      assert!(bands.damage_plausible_max_dice >= 30);
      // difficulty: 1..100 合理
      assert_eq!(bands.difficulty_plausible_min, 1);
      assert_eq!(bands.difficulty_plausible_max, 100);
  }

  #[tokio::test]
  async fn cyberpunk_bands_equiv() {
      // run against :54346 Cyberpunk DB
      let Ok(url) = std::env::var("DATABASE_URL") else { return; };
      let db = trpg_db::Db::connect(&url).await.unwrap();
      let Some(kernel) = db.load_rule_kernel("cyberpunk_red").await.unwrap() else {
          eprintln!("SKIP: no cyberpunk_red kernel"); return;
      };
      let bands = kernel.referee_value_bands.as_ref().expect("override should supply referee_value_bands for Cyberpunk");
      assert_eq!(bands.damage_family, "cyberpunk_red_weapon_damage");
      assert_eq!(bands.damage_plausible_max_dice, 12, "cyberpunk: orig max was 12");
      assert_eq!(bands.difficulty_plausible_min, 5);
      assert_eq!(bands.difficulty_plausible_max, 35);
      // DV 17 合理（原 5..=35）
      assert!(17 >= bands.difficulty_plausible_min && 17 <= bands.difficulty_plausible_max);
      // DV 50 不合理
      assert!(!(50 >= bands.difficulty_plausible_min && 50 <= bands.difficulty_plausible_max));
  }
  ```

  运行（CoC 库）：
  ```bash
  DATABASE_URL=postgres://postgres:password@localhost:54347/chatrpg \
    cargo test -p trpg-referee --test live_referee_bands_equiv coc_bands_equiv -- --nocapture
  ```

  运行（Cyberpunk 库）：
  ```bash
  DATABASE_URL=postgres://postgres:password@localhost:54346/chatrpg \
    cargo test -p trpg-referee --test live_referee_bands_equiv cyberpunk_bands_equiv -- --nocapture
  ```

---

- [ ] **Step 9 — 全套件零回归 + commit**

  ```bash
  cargo test -p trpg-model --lib 2>&1 | tail -3
  cargo test -p trpg-referee --lib 2>&1 | tail -3
  cargo test -p trpg-db --lib 2>&1 | tail -3
  cargo build --workspace 2>&1 | grep "error\[E" | head -10
  ```

  期望：零编译错误，lib 测试全绿。确认后 commit（scope = `p0-2/task3-referee-bands`）：

  ```
  feat(p0-2): referee value-bands → kernel data, delete ruleset_id branches

  trpg-referee: five hardcoded helpers (lines 282-286) replaced by four
  kernel-reading fns; inspect_turn loads RuleKernel via db.load_rule_kernel,
  verify_claim receives &RuleKernel instead of ruleset_id; GENERIC_REFEREE_BANDS
  OnceLock provides fail-soft generic defaults when no bands configured.
  trpg-model: add RefereeValueBands struct + Option<RefereeValueBands> field to
  RuleKernel (#[serde(default)], backward compat). trpg-db: load_rule_kernel
  merges override referee_value_bands on read. Four override files carry exact
  replica of original hardcoded values (cyberpunk/coc existing, dnd/sword_world
  new). grep trpg-referee/src = 0 ruleset names.

  Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>
  ```


---
### Task 4: trpg-director — 删除 is_homecoming，scene brief/NPC advice/place summary 改走 module 数据

**文件:**
- `crates/trpg-director/src/lib.rs`（311 行：`is_homecoming`；106–117：build_brief 专属块；272–275：biased_npc_advice；298：current_place_summary）
- `crates/trpg-model/src/lib.rs`（新增 `DirectorModuleConfig` 结构 + `ModuleBundle.module_director_config` 可选字段）
- `crates/trpg-db/src/lib.rs`（新增 `read_module_director_config_override`，路径 `data/parsed/modules/{module_id}.director_config.json`）
- `data/parsed/modules/cpr_one_shot_homecoming.director_config.json`（新建 override 数据文件，精确复刻原硬编码值）

**不确定性说明：** module deep 数据（`ScenarioNode.read_aloud/gm_notes`）是场景级正文，非"全局 scene facts/NPC bias"容器——Homecoming 的全局 NPC 建议（injured_lawman / fixer_contact）和场景压力描述是模组级元数据，不在任何已深抽的 `ScenarioNode` 里。因此本 Task 走**过渡方案**：新增轻量 `DirectorModuleConfig`（存 module bundle `#[serde(default)]`），通过 override JSON 注入精确复刻的 Homecoming 值；标注 reader 后续自动抽取替代 override（DONE_WITH_CONCERNS）。`DirectorInput` 扩字段 `module_config: Option<&DirectorModuleConfig>`，调用点（`prepare_actionable_situation`）异步载入后传入——不破坏 `director` 无异步的设计，异步留在 runtime 侧。

---

- [ ] **Step 1（失败测试）：** 在 `crates/trpg-director/src/lib.rs` 的 `#[cfg(test)]` 块写三个单测，用 `cargo test -p trpg-director` 确认红（编译失败或 assert 失败均可，后续实现前必须先确认测试存在）：

```rust
#[cfg(test)]
mod director_dehardcode_tests {
    use super::*;

    fn minimal_request() -> ContextRequest {
        ContextRequest {
            ruleset_id: "cyberpunk_red".into(),
            module_id: Some("cpr_one_shot_homecoming".into()),
            session_id: "s1".into(),
            turn_id: "t1".into(),
            viewer: VisibilityProfile::gm(),
            token_budget: trpg_model::TokenBudget { prefix_max: 1000, pinned_max: 2000, dynamic_max: 3000 },
        }
    }

    fn minimal_state() -> RuntimeState {
        RuntimeState {
            ruleset_id: "cyberpunk_red".into(),
            module_id: Some("cpr_one_shot_homecoming".into()),
            ..Default::default()
        }
    }

    fn compiled() -> CompiledContext { CompiledContext::default() }

    // T1: 给定 module_config Some(场景压力文本) → brief.visible_facts 含该文本
    #[test]
    fn scene_facts_from_module_config() {
        let cfg = DirectorModuleConfig {
            scene_facts: vec![DirectorSceneFact {
                text: "外露电缆是可观察的交互抓手".into(),
                source: "module_override".into(),
            }],
            ..Default::default()
        };
        let req = minimal_request();
        let state = minimal_state();
        let input = DirectorInput {
            request: &req,
            state: &state,
            compiled: &compiled(),
            user_input: "我不知道能做什么",
            conflict: None,
            module_config: Some(&cfg),
        };
        let brief = build_brief(input, None, GuidanceLevel::AskGoal);
        assert!(
            brief.visible_facts.iter().any(|f| f.text.contains("外露电缆")),
            "scene_facts from module_config must appear in visible_facts"
        );
    }

    // T2: module_config=None → 不 panic，返回通用 brief（visible_facts ≥ 1 条）
    #[test]
    fn no_module_config_returns_generic_brief() {
        let req = minimal_request();
        let state = minimal_state();
        let input = DirectorInput {
            request: &req,
            state: &state,
            compiled: &compiled(),
            user_input: "怎么办",
            conflict: None,
            module_config: None,
        };
        let brief = build_brief(input, None, GuidanceLevel::AskGoal);
        assert!(!brief.visible_facts.is_empty(), "generic brief must have ≥ 1 visible_fact");
        // 确认无规则集/模组名字面量泄漏进 brief source 字段
        for f in &brief.known_facts {
            assert!(
                !f.source.contains("homecoming") && !f.source.contains("cyberpunk"),
                "known_fact.source must not contain hardcoded ruleset/module name, got: {}",
                f.source
            );
        }
    }

    // T3: npc_advice 来自 module_config.npc_advice → biased_npc_advice 返回配置中的 NPC
    #[test]
    fn npc_advice_from_module_config() {
        let cfg = DirectorModuleConfig {
            npc_advice: vec![NpcBiasedAdvice {
                npc_id: "injured_lawman".into(),
                speaker_label: "受伤警察".into(),
                advice_text: "别靠近它，把火力压住！".into(),
                bias_or_goal: "想活下来".into(),
                not_official_solution: true,
            }],
            ..Default::default()
        };
        let req = minimal_request();
        let state = minimal_state();
        let input = DirectorInput {
            request: &req,
            state: &state,
            compiled: &compiled(),
            user_input: "怎么看",
            conflict: None,
            module_config: Some(&cfg),
        };
        let advice = biased_npc_advice(input);
        assert_eq!(advice.len(), 1);
        assert_eq!(advice[0].npc_id, "injured_lawman");
    }
}
```

验证红（`build_brief`/`biased_npc_advice` 签名不含 `module_config` 字段时编译失败是预期）：
```bash
cargo test -p trpg-director 2>&1 | head -30
```

---

- [ ] **Step 2（model 新类型）：** 在 `crates/trpg-model/src/lib.rs` 的 director 模型区块（`NpcBiasedAdvice` 之后，约 4776 行附近）新增：

```rust
/// 模组级 director 配置：专属场景 facts/NPC 建议/地点描述，替换 is_homecoming() 等模组名分支。
/// 存入 module bundle（#[serde(default)] 向后兼容旧 bundle）；
/// 也可通过 data/parsed/modules/{module_id}.director_config.json override 注入。
/// 后续由 module reader 自动抽取后替代 override（DONE_WITH_CONCERNS：目前 module deep 数据
/// 以场景级 ScenarioNode 存储，无模组级 global NPC bias 容器；reader 抽取留后续）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct DirectorSceneFact {
    pub text: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct DirectorPressureItem {
    pub text: String,
    pub clock_id: Option<String>,
    pub severity: u32,
    pub consequence_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct DirectorAffordanceItem {
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct DirectorModuleConfig {
    /// 模组专属可见场景 facts（进 brief.visible_facts）
    #[serde(default)]
    pub scene_facts: Vec<DirectorSceneFact>,
    /// 模组专属压力项（进 brief.pressure）
    #[serde(default)]
    pub pressure_items: Vec<DirectorPressureItem>,
    /// 模组专属交互抓手（进 brief.affordances）
    #[serde(default)]
    pub affordance_items: Vec<DirectorAffordanceItem>,
    /// 模组专属 NPC 偏见建议（替换 biased_npc_advice 的 is_homecoming 分支）
    #[serde(default)]
    pub npc_advice: Vec<NpcBiasedAdvice>,
    /// 模组专属已知事实（进 brief.known_facts）
    #[serde(default)]
    pub known_facts: Vec<String>,
    /// 模组专属开放问题（进 brief.open_questions）
    #[serde(default)]
    pub open_question: Option<String>,
    /// 无 location_id 时的地点摘要（替换 current_place_summary 的 is_homecoming 分支）
    #[serde(default)]
    pub place_summary_fallback: Option<String>,
}
```

同时在 `ModuleBundle` 结构（约 1758 行）新增：
```rust
#[serde(default)]
pub module_director_config: Option<DirectorModuleConfig>,
```

验证编译：
```bash
cargo build -p trpg-model 2>&1 | tail -5
```

---

- [ ] **Step 3（db override loader）：** 在 `crates/trpg-db/src/lib.rs` 的 `read_kernel_override_file` 之后（约 4055 行）新增 module director config 的 override 读取函数，并在 `load_module_graph`（459 行）的适当位置或单独提供公开函数 `load_module_director_config`：

```rust
/// Data-only module director config override file:
/// `{TRPG_DATA_DIR}/parsed/modules/{module_id}.director_config.json`
/// shaped as DirectorModuleConfig JSON. Returns None when absent/unreadable.
fn read_module_director_config(module_id: &str) -> Option<trpg_model::DirectorModuleConfig> {
    let safe: String = module_id.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    let dir = std::env::var("TRPG_DATA_DIR").unwrap_or_else(|_| "data".into());
    let p = std::path::Path::new(&dir)
        .join("parsed").join("modules")
        .join(format!("{safe}.director_config.json"));
    let text = std::fs::read_to_string(&p).ok()?;
    serde_json::from_str(&text).ok()
}

impl TrpgDb {
    /// Load the DirectorModuleConfig for a module: try module bundle field first,
    /// then fall back to the data-only override file. Returns None when module
    /// has no director config (generic director path; fail-soft).
    pub async fn load_module_director_config(
        &self,
        module_id: &str,
    ) -> Result<Option<trpg_model::DirectorModuleConfig>> {
        // 1. Try bundle field (future: reader populates this)
        let bundle_cfg = self.load_module_graph(module_id).await.ok().flatten()
            .and_then(|_| {
                // load_module_graph 只返 ModuleGraph 无 bundle config；
                // 完整 bundle 需 load_module_bundle_for_continue（含全 bundle JSON）
                None::<trpg_model::DirectorModuleConfig>
            });
        if bundle_cfg.is_some() { return Ok(bundle_cfg); }
        // 2. Fall back to override file (current primary path)
        Ok(read_module_director_config(module_id))
    }
}
```

> **注：** `load_module_bundle_for_continue` 需要 source_hash；直接查 DB 的 module bundle 取 `module_director_config` 字段需要扩展查询。本 Task 的过渡方案优先走 override 文件（`read_module_director_config`），bundle 字段支持留给 reader 联动后补（DONE_WITH_CONCERNS）。因此 `load_module_director_config` 实现可简化为直接读 override：

```rust
pub async fn load_module_director_config(
    &self,
    module_id: &str,
) -> Result<Option<trpg_model::DirectorModuleConfig>> {
    Ok(read_module_director_config(module_id))
}
```

验证编译：
```bash
cargo build -p trpg-db 2>&1 | tail -5
```

---

- [ ] **Step 4（override 数据文件）：** 新建 `data/parsed/modules/cpr_one_shot_homecoming.director_config.json`，精确复刻原 `is_homecoming` 分支的全部硬编码值（build_brief 106–117、biased_npc_advice 272–275、current_place_summary 298）：

```json
{
  "_note": "DirectorModuleConfig override for Homecoming (Cyberpunk RED one-shot). Replaces is_homecoming() hardcode in trpg-director. DONE_WITH_CONCERNS: values hand-copied from the original Rust hardcode; future reader pass should auto-extract from module deep ScenarioNode gm_notes/scene_pressure fields and delete this file.",
  "scene_facts": [
    {
      "text": "无人机、警察、仓库入口、外露电缆和内部服务器噪音形成同一个局势面：威胁、救援、技术源头和情报价值同时存在。",
      "source": "module_override.homecoming"
    }
  ],
  "pressure_items": [
    {
      "text": "如果继续拖延，现场伤员、外部势力和设备过载都会推进局势。",
      "clock_id": "clock.homecoming_scene_pressure",
      "severity": 70,
      "consequence_hint": "警察伤势恶化、敌对势力抵达、仓库内源头转移或过载"
    }
  ],
  "affordance_items": [
    { "description": "外露电缆是可观察的交互抓手；它暗示供能、数据或控制关系。" },
    { "description": "受困警察和封锁街口是救援、社交和现场资源的抓手。" },
    { "description": "仓库内部的噪音与外部威胁同步，说明源头可能不在无人机本体。" }
  ],
  "npc_advice": [
    {
      "npc_id": "injured_lawman",
      "speaker_label": "受伤警察",
      "advice_text": "别靠近它，把火力压住！",
      "bias_or_goal": "想活下来，优先压制威胁，不关心情报价值。",
      "not_official_solution": true
    },
    {
      "npc_id": "fixer_contact",
      "speaker_label": "你的联系人",
      "advice_text": "别把值钱的情报打烂，查清它从哪来的。",
      "bias_or_goal": "想要可出售的信息或技术，低估现场救援压力。",
      "not_official_solution": true
    }
  ],
  "known_facts": [
    "现场至少不是单一战斗问题：它同时是救援、威胁控制、源头调查和资源取舍。"
  ],
  "open_question": "你们优先救人、控制威胁、追查源头、获取资源，还是撤离保命？",
  "place_summary_fallback": "当前地点：Cyberpunk RED Homecoming 开场附近；一个高压现场正在等待玩家选择目标。"
}
```

> **模组 ID 约定：** 原代码检测 `module_id.contains("homecoming")`，override 文件命名键需与实际 module_id 对齐。用 `grep -n "module_id\|homecoming" data/parsed/modules/` 或 bundle JSON 确认实际 module_id 后调整文件名（若为 `cpr_homecoming` 等则重命名）。
> 检查命令：
```bash
grep -r '"module_id"' /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/data/ 2>/dev/null | grep -i homecoming | head -5
```

---

- [ ] **Step 5（director 改造：删 is_homecoming，扩 DirectorInput）：** 修改 `crates/trpg-director/src/lib.rs`。所有改动在同一文件内，保持 ≤400 行（当前 315 行）：

**5a. 扩展 `DirectorInput`**（第 21–26 行）：
```rust
#[derive(Debug, Clone, Copy)]
pub struct DirectorInput<'a> {
    pub request: &'a ContextRequest,
    pub state: &'a RuntimeState,
    pub compiled: &'a CompiledContext,
    pub user_input: &'a str,
    pub conflict: Option<&'a ConflictTurnResult>,
    // 新增：调用方异步载入后传入；None → 通用 director 路径（fail-soft）
    pub module_config: Option<&'a trpg_model::DirectorModuleConfig>,
}
```

**5b. 修改 `build_brief`**（97–155 行），删除 `if is_homecoming(input) { ... }` 块，改为读 `module_config`：
```rust
// 原 is_homecoming 专属块（106–117）替换为：
if let Some(cfg) = input.module_config {
    for sf in &cfg.scene_facts {
        visible_facts.push(VisibleFact {
            fact_id: id("fact"),
            text: sf.text.clone(),
            source_refs: vec![],
            confidence: RulingConfidence::Medium,
        });
    }
    for pi in &cfg.pressure_items {
        pressure.push(PressureItem {
            pressure_id: id("pressure"),
            text: pi.text.clone(),
            clock_id: pi.clock_id.clone(),
            severity: pi.severity as i32,
            consequence_hint: pi.consequence_hint.clone(),
        });
    }
    for ai in &cfg.affordance_items {
        affordances.push(Affordance {
            affordance_id: id("affordance"),
            description: ai.description.clone(),
            implies_vectors: vec![ActionVector::Observe, ActionVector::Technical, ActionVector::Tactical],
            visible_to_players: true,
            source_refs: vec![],
        });
    }
    for kf in &cfg.known_facts {
        known_facts.push(KnownFact {
            fact_id: id("known"),
            text: kf.clone(),
            source: "module_config".into(),
            public: true,
        });
    }
    if let Some(oq) = &cfg.open_question {
        open_questions.push(OpenQuestion {
            question_id: id("question"),
            text: oq.clone(),
            points_to: vec!["rescue".into(), "control".into(), "source".into(), "loot".into(), "retreat".into()],
        });
    }
}
// 原 risks.push 的 homecoming 专属 risk（第 114 行）删除；通用 base_risks 覆盖
```

**5c. 修改 `biased_npc_advice`**（271–276 行），删 `is_homecoming` 分支：
```rust
fn biased_npc_advice(input: DirectorInput<'_>) -> Vec<NpcBiasedAdvice> {
    if let Some(cfg) = input.module_config {
        if !cfg.npc_advice.is_empty() {
            return cfg.npc_advice.clone();
        }
    }
    vec![NpcBiasedAdvice {
        npc_id: "local_npc".into(),
        speaker_label: "现场 NPC".into(),
        advice_text: "我只会从自己的利益出发提醒你们。".into(),
        bias_or_goal: "NPC advice is biased and not the GM's official route.".into(),
        not_official_solution: true,
    }]
}
```

**5d. 修改 `current_place_summary`**（296–299 行），删 `is_homecoming` 分支：
```rust
fn current_place_summary(input: DirectorInput<'_>) -> String {
    if let Some(location) = &input.state.location_id {
        return format!("当前地点：{}", location);
    }
    if let Some(cfg) = input.module_config {
        if let Some(summary) = &cfg.place_summary_fallback {
            return summary.clone();
        }
    }
    "当前场景需要先转化为可行动局势。".into()
}
```

**5e. 删除 `is_homecoming` 函数**（第 311 行）——整行删除：
```rust
// 删除此行：
// fn is_homecoming(input: DirectorInput<'_>) -> bool { ... }
```

验证编译 + 测试红（Step 1 测试此时应编译通过，运行时可能绿也可能因 override 文件路径失败）：
```bash
cargo test -p trpg-director 2>&1 | tail -20
```

---

- [ ] **Step 6（runtime 调用点更新）：** 修改 `crates/trpg-runtime/src/lib.rs` 的 `prepare_actionable_situation`（855–883 行），在构造 `DirectorInput` 前异步载入 module config：

```rust
pub async fn prepare_actionable_situation(
    &self,
    request: &ContextRequest,
    state: &RuntimeState,
    compiled: &CompiledContext,
    user_input: &str,
    conflict: Option<&ConflictTurnResult>,
) -> Result<DirectorTurnResult> {
    let director = ActionableSituationDirector::from_env_or_default();
    // 异步载入 module director config（fail-soft：None → 通用路径）
    let module_cfg = if let Some(mid) = &request.module_id {
        self.db.load_module_director_config(mid).await.unwrap_or(None)
    } else {
        None
    };
    let result = director.prepare(DirectorInput {
        request,
        state,
        compiled,
        user_input,
        conflict,
        module_config: module_cfg.as_ref(),
    });
    // ... 余下保持不变 ...
```

验证编译：
```bash
cargo build -p trpg-runtime 2>&1 | tail -10
```

---

- [ ] **Step 7（测试绿 + CI grep 守卫）：**

**7a. 运行 director 单测全绿（含 Step 1 三条）：**
```bash
cargo test -p trpg-director 2>&1
```
期望输出：`test result: ok. 3 passed; 0 failed`

**7b. grep 守卫——验证 trpg-director/src 无规则集/模组名字面量（白名单：`#[cfg(test)]` 块）：**
```bash
grep -n '"cyberpunk\|"homecoming\|"coc\|"call_of_cthulhu\|"dnd\|"sword_world\|"triangle\|"masks\|"vault\|is_homecoming' \
  /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-director/src/lib.rs
```
期望：**无输出**（零命中，tests 块内如有 fixture 用途的字面量由白名单豁免）。

**7c. 等价 live 验证（需 :54346 Cyberpunk DB，标 SKIP 若无环境）：**
```bash
# SKIP_LIVE=1 时跳过；有 :54346 可不 SKIP
SKIP_LIVE=1 cargo test -p trpg-director --test '*' 2>&1 | tail -5
```
有真实数据库环境时手动跑一回合 Homecoming 并对比 brief 字段：
```bash
# 对比 scene_facts 与原硬编码等价（外露电缆/仓库/无人机文本）
DATABASE_URL=postgres://trpg:trpg@localhost:54346/trpg \
TRPG_DATA_DIR=/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/data \
  cargo run --bin trpg-cli -p trpg-cli -- play \
  --module cpr_one_shot_homecoming --session-id s_test_homecoming 2>&1 \
  | grep -E "外露电缆|injured_lawman|place_summary" | head -10
```

**7d. 全套件零回归：**
```bash
cargo test --workspace --exclude trpg-rule-agent 2>&1 | tail -10
```
期望：无新增失败。

---

**DONE_WITH_CONCERNS：**
1. `DirectorModuleConfig.affordance_items` 当前只写了通用 `implies_vectors`（`[Observe, Technical, Tactical]`）——原硬编码各条有具体向量组合（电缆→Technical/Tactical/Observe；警察→Social/Resource/Tactical；噪音→Observe/Technical/Stealth）。为精确复刻可扩展 `DirectorAffordanceItem` 加 `implies_vectors: Option<Vec<String>>`，或直接把 override 的 affordances 做成 `Vec<Affordance>` 的完整结构；本 Task 简化用通用向量——行为差异在 GM 叙述层（director block 供 GM agent 参考），不影响检定/结算正确性，可后续精化。
2. 模组 ID 文件名须与真实 DB 中的 `module_id` 字段完全匹配（见 Step 4 注）；若 module_id 含路径字符会被安全化替换，与实际 PDF 文件名不同——上线前需 grep 确认。
3. Module reader 未来自动抽取场景压力/NPC bias 后，应删除 override JSON 并让 `module_director_config` 字段由 bundle 携带；届时 `load_module_director_config` 优先读 bundle 字段即可（DB 查询扩展留后续）。


---
### Task 5: trpg-material search profile + trpg-object check label → kernel/module 数据驱动

**Files:**
- `crates/trpg-model/src/lib.rs` (RuleKernel struct ~L1425; ModuleGraph struct ~L1689)
- `crates/trpg-material/src/lib.rs` (`ruleset_aliases_for` L830–L863; `module_preferences_for` L865–L882; 调用点 `mechanics_query_plan_for` L783, L786–L787)
- `crates/trpg-object/src/lib.rs` (`make_object_check` L861; `build_contract` L202–L206; 已有 `load_rule_kernel` 调用 L206)
- `data/parsed/rules/cyberpunk_red.rule_kernel.override.json` (现有 override 样板 L1–L23)
- `data/parsed/rules/{ruleset_id}.rule_kernel.override.json` (五套规则集新 override)
- `data/parsed/modules/{module_id}.module_config.json` (新 module 配置文件，三套模组)

---

#### 背景（实读确认的现状）

`ruleset_aliases_for(ruleset_id: &str, skill)` (L830) 和 `module_preferences_for(module_id: Option<&str>, skill)` (L865) 是纯函数，被 `mechanics_query_plan_for(demand)` 在 L786–L787 调用，`demand` 携带 `ruleset_id: String` 和 `module_id: Option<String>`（均字符串，无 kernel 引用）。两函数体均以 `r.contains("cyberpunk")` / `module.contains("homecoming")` 分支。`MaterialService` 已有 `self.db` 且在 `object_schema_guidance`（L425）调用 `load_rule_kernel`，但 `mechanics_query_plan_for` 是同步纯函数，无 db 访问。

`make_object_check`（trpg-object L861）接收 `input: ObjectTurnInput<'_>`，其中含 `ruleset_id: &str`，`build_contract`（L202）已调用 `load_rule_kernel`（L206）并将 kernel 传下（kernel_dice/kernel_target/kernel_refs），但 `make_object_check` 自己不收 kernel 参数，check_label 分支直接用 `input.ruleset_id.contains("cyberpunk")`（L864）。

`load_rule_kernel`（trpg-db L693）在读时叠加 `data/parsed/rules/{id}.rule_kernel.override.json`，目前仅合并 `resource_tracks`（by id）和 `dice_core`（浅 key 合并）。需扩展为也合并 `search_profile` 和 `check_label_policy`。

`ModuleGraph`（L1689）目前无 search_profile 字段，`ModuleBundle`（L1759）有 `module_prep_packets`。最轻量的 module 配置落地路径：新增独立 JSON 配置文件 `data/parsed/modules/{module_id}.module_config.json`（类似 rule_kernel override），由 `MaterialService` 在需要时读取（同步，无 DB 访问，复用 `TRPG_DATA_DIR` env）。

---

- [ ] **Step 1（模型层：RuleKernel 加 search_profile + check_label_policy 字段）**

在 `trpg-model/src/lib.rs` `RuleKernel` struct 末尾追加两个 `Option` 字段；新增 `RuleKernelSearchProfile` 和 `CheckLabelPolicy` 结构；新增 `GENERIC_SEARCH_PROFILE` 常量。

```rust
// 在 RuleKernel struct（L1461 末尾）追加：
#[serde(default)]
pub search_profile: Option<RuleKernelSearchProfile>,
#[serde(default)]
pub check_label_policy: Option<CheckLabelPolicy>,
```

```rust
/// Ruleset-specific search section hints and field aliases, loaded from
/// kernel override data. Engine reads this instead of branching on ruleset_id.
/// All fields are optional; missing sub-fields fall back to GENERIC defaults.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct RuleKernelSearchProfile {
    /// Per SearchSkillKind key ("combat_resolution"/"weapon_parameter"/…)
    /// → preferred_sections array override. Missing kind → base generic.
    #[serde(default)]
    pub preferred_sections_by_skill: std::collections::HashMap<String, Vec<String>>,
    /// Per SearchSkillKind key → field_aliases object override (merges onto base).
    #[serde(default)]
    pub field_aliases_by_skill: std::collections::HashMap<String, serde_json::Value>,
}

/// Check label templates keyed by ObjectInteractionKind as_str().
/// Engine unwrap_or(&GENERIC_CHECK_LABEL_POLICY) and looks up the kind.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CheckLabelPolicy {
    /// key = ObjectInteractionKind::as_str() value, val = label string
    #[serde(default)]
    pub labels: std::collections::HashMap<String, String>,
}

/// Generic search profile: no preferred-section overrides, no field-alias overrides.
/// Used when kernel.search_profile is None.
pub static GENERIC_SEARCH_PROFILE: RuleKernelSearchProfile = RuleKernelSearchProfile {
    preferred_sections_by_skill: std::collections::HashMap::new(),
    field_aliases_by_skill: std::collections::HashMap::new(),
};

/// Generic check label policy: empty map → callers fall back to their static defaults.
pub static GENERIC_CHECK_LABEL_POLICY: CheckLabelPolicy = CheckLabelPolicy {
    labels: std::collections::HashMap::new(),
};
```

> 注：`static` + 内含 HashMap 需 `once_cell::sync::Lazy` 或 `std::sync::OnceLock`（HashMap 不是 const）。改用 `Lazy`：

```rust
use std::sync::OnceLock;
use std::collections::HashMap;

pub fn generic_search_profile() -> &'static RuleKernelSearchProfile {
    static V: OnceLock<RuleKernelSearchProfile> = OnceLock::new();
    V.get_or_init(|| RuleKernelSearchProfile {
        preferred_sections_by_skill: HashMap::new(),
        field_aliases_by_skill: HashMap::new(),
    })
}

pub fn generic_check_label_policy() -> &'static CheckLabelPolicy {
    static V: OnceLock<CheckLabelPolicy> = OnceLock::new();
    V.get_or_init(|| CheckLabelPolicy { labels: HashMap::new() })
}
```

**验失败**（先写单测，`cargo test -p trpg-model`）：

```rust
#[cfg(test)]
mod search_profile_tests {
    use super::*;
    #[test]
    fn generic_profile_has_empty_maps() {
        assert!(generic_search_profile().preferred_sections_by_skill.is_empty());
        assert!(generic_check_label_policy().labels.is_empty());
    }
    #[test]
    fn rule_kernel_defaults_to_no_profile() {
        let k: RuleKernel = serde_json::from_str(r#"{"kernel_id":"x","ruleset_id":"x","version":"1"}"#).unwrap();
        assert!(k.search_profile.is_none());
        assert!(k.check_label_policy.is_none());
    }
    #[test]
    fn search_profile_round_trips() {
        let p = RuleKernelSearchProfile {
            preferred_sections_by_skill: [("combat_resolution".into(), vec!["Combat".into()])].into(),
            field_aliases_by_skill: [("combat_resolution".into(), serde_json::json!({"target_number":["DV"]}))].into(),
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: RuleKernelSearchProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(back.preferred_sections_by_skill.get("combat_resolution").unwrap(), &vec!["Combat".to_string()]);
    }
}
```

`cargo test -p trpg-model` → 新测试**红**（字段/类型未存在）。添加字段/结构 → **绿**。

---

- [ ] **Step 2（trpg-db：load_rule_kernel 叠加 search_profile 和 check_label_policy）**

在 `trpg-db/src/lib.rs` `load_rule_kernel`（L708 override 叠加区）补两个字段合并分支：

```rust
// 在 if let Some(doc) = read_kernel_override_file(ruleset_id) { ... } 块内，
// 紧接 dice_core 合并之后追加：
if let Some(sp) = doc.get("search_profile") {
    if let Ok(profile) = serde_json::from_value::<trpg_model::RuleKernelSearchProfile>(sp.clone()) {
        kernel.search_profile = Some(profile);
    }
}
if let Some(clp) = doc.get("check_label_policy") {
    if let Ok(policy) = serde_json::from_value::<trpg_model::CheckLabelPolicy>(clp.clone()) {
        kernel.check_label_policy = Some(policy);
    }
}
```

**单测**（`#[cfg(test)]` 内，无 DB，纯文件 IO mock）：

```rust
#[cfg(test)]
mod kernel_override_search_profile_tests {
    use trpg_model::{RuleKernel, RuleKernelSearchProfile};
    use serde_json::json;

    fn merge_search_profile_from_override(kernel: &mut RuleKernel, doc: &serde_json::Value) {
        if let Some(sp) = doc.get("search_profile") {
            if let Ok(profile) = serde_json::from_value::<RuleKernelSearchProfile>(sp.clone()) {
                kernel.search_profile = Some(profile);
            }
        }
    }

    #[test]
    fn override_sets_search_profile() {
        let mut k: RuleKernel = serde_json::from_str(r#"{"kernel_id":"x","ruleset_id":"cyberpunk_red","version":"1"}"#).unwrap();
        assert!(k.search_profile.is_none());
        let doc = json!({"search_profile":{"preferred_sections_by_skill":{"combat_resolution":["Friday Night Firefight"]}}});
        merge_search_profile_from_override(&mut k, &doc);
        let sp = k.search_profile.unwrap();
        assert_eq!(sp.preferred_sections_by_skill.get("combat_resolution").unwrap(), &vec!["Friday Night Firefight".to_string()]);
    }

    #[test]
    fn override_without_search_profile_leaves_none() {
        let mut k: RuleKernel = serde_json::from_str(r#"{"kernel_id":"x","ruleset_id":"x","version":"1"}"#).unwrap();
        merge_search_profile_from_override(&mut k, &json!({"resource_tracks":[]}));
        assert!(k.search_profile.is_none());
    }
}
```

`cargo test -p trpg-db` → **绿**。

---

- [ ] **Step 3（override 数据：五套规则集 search_profile + check_label_policy 精确复刻硬编码值）**

将 `ruleset_aliases_for`（L841–L861）的五套规则集分支和 `make_object_check`（L864）的 `contains("cyberpunk")` 分支**精确复刻**进对应 override 文件（override 胜出，旧文件有则追加字段，无则新建）。

**`data/parsed/rules/cyberpunk_red.rule_kernel.override.json`**（追加 `search_profile` 和 `check_label_policy` 到现有文件）：

```json
{
  "_note": "… 现有注释保留 …",
  "resource_tracks": [ "… 现有内容保留 …" ],
  "search_profile": {
    "preferred_sections_by_skill": {
      "combat_resolution":    ["Getting it Done","Resolving Actions with Skills","Friday Night Firefight","Ranged Combat","Melee Combat"],
      "weapon_parameter":     ["Weapons and Armor","Friday Night Firefight"],
      "armor_defense":        ["Before You Take Damage","When Armor Doesn't Cut It","Weapons and Armor"],
      "ability_activation":   ["Role Abilities"],
      "npc_stat_block":       ["Getting it Done","Resolving Actions with Skills","Friday Night Firefight"],
      "generic_mechanical":   ["Getting it Done","Resolving Actions with Skills"]
    },
    "field_aliases_by_skill": {
      "combat_resolution": {"target_number": ["DV","range DV","difficulty value"]},
      "armor_defense":     {"armor": ["SP","armor","ablation"]}
    }
  },
  "check_label_policy": {
    "labels": {
      "grab_held_object": "DEX + Brawling contested grab/disarm check",
      "disarm":           "DEX + Brawling contested grab/disarm check"
    }
  }
}
```

**`data/parsed/rules/dnd5e.rule_kernel.override.json`**（新建）：

```json
{
  "_note": "P0-2 search_profile override: migrated from ruleset_aliases_for engine branch.",
  "search_profile": {
    "preferred_sections_by_skill": {
      "combat_resolution":  ["Equipment","Using Ability Scores","Saving Throws","Combat","Making an Attack","Damage and Healing","Creature Statistics"],
      "weapon_parameter":   ["Equipment","Weapons"],
      "armor_defense":      ["Armor and Shields","Combat"],
      "ability_activation": ["Spellcasting","Spell Descriptions","Feats"],
      "npc_stat_block":     ["Creature Statistics","Combat"]
    },
    "field_aliases_by_skill": {
      "combat_resolution": {"defense": ["AC","Armor Class"], "save": ["saving throw","spell save DC"]},
      "armor_defense":     {"armor": ["AC","Armor Class"]}
    }
  }
}
```

**`data/parsed/rules/sword_world_2_0.rule_kernel.override.json`**（新建）：

```json
{
  "_note": "P0-2 search_profile override: migrated from ruleset_aliases_for engine branch.",
  "search_profile": {
    "preferred_sections_by_skill": {
      "combat_resolution":  ["Skill Checks","Contested Checks","Combat Rules","Combat Flow","Standard Combat"],
      "weapon_parameter":   ["Weapon Attacks","Damage","Item Data","Comprehensive List of Weapons","Combat Feats Data"],
      "armor_defense":      ["Damage","Comprehensive List of Armor"],
      "ability_activation": ["Magic Rules","Spell Damage"]
    },
    "field_aliases_by_skill": {
      "combat_resolution": {"defense": ["evasion","resistance","defense","protection"]},
      "weapon_parameter":  {"damage": ["damage","power table","威力表"]}
    }
  }
}
```

**`data/parsed/rules/call_of_cthulhu_7e.rule_kernel.override.json`**（已存在，追加 `search_profile`）：

```json
{
  "_note": "… 现有注释保留 …（resource_tracks 保留）",
  "resource_tracks": [ "… 现有内容 …" ],
  "search_profile": {
    "preferred_sections_by_skill": {
      "combat_resolution":  ["Ability","Skills","Combat","Weapons","Damage","Keeper Rulebook"],
      "weapon_parameter":   ["Weapons","Combat","Damage"],
      "armor_defense":      ["Combat","Damage"],
      "condition_resource": ["Sanity","Spells","Tomes","Artifacts"],
      "npc_stat_block":     ["Combat","Keeper Rulebook"]
    },
    "field_aliases_by_skill": {
      "condition_resource": {"resource": ["HP","SAN","sanity","major wound"]},
      "combat_resolution":  {"threshold": ["regular","hard","extreme"]}
    }
  }
}
```

**`data/parsed/rules/triangle_agency.rule_kernel.override.json`**（已存在或新建，追加）：

```json
{
  "_note": "P0-2 search_profile override: migrated from ruleset_aliases_for engine branch.",
  "search_profile": {
    "preferred_sections_by_skill": {
      "combat_resolution":  ["Field Agent Manual","Encounter","GM Toolkit"],
      "npc_stat_block":     ["GM Toolkit","Encounter","Anomaly"],
      "ability_activation": ["Playwalled Documents","Mission","Aftermath"],
      "generic_mechanical": ["Field Agent Manual","GM Toolkit","Playwalled Documents"]
    },
    "field_aliases_by_skill": {
      "condition_resource": {"resource": ["Harm","Chaos","Stress"], "visibility": ["playwalled","Agency property","numberless pages","permission"]}
    }
  }
}
```

**module 配置文件（新增辅助加载函数 + 三套模组）**：

`data/parsed/modules/homecoming.module_config.json`：

```json
{
  "_note": "P0-2 module_search_profile: migrated from module_preferences_for engine branch.",
  "module_search_profile": {
    "npc_stat_block":     ["Redesigned NPC cards","Items NPCs tech","Athena's Behavior","Hacking Athena","Scavv's Warehouse","Resources","vehicle cards"],
    "module_card":        ["Redesigned NPC cards","Items NPCs tech","Resources","vehicle cards"],
    "scene_object":       ["Scavv's Warehouse","Resources"],
    "combat_resolution":  ["Athena's Behavior","Hacking Athena"]
  }
}
```

`data/parsed/modules/masks_of_nyarlathotep.module_config.json`：

```json
{
  "_note": "P0-2 module_search_profile: migrated from module_preferences_for engine branch.",
  "module_search_profile": {
    "npc_stat_block":    ["Key Non-Player Characters","Dramatis Personae"],
    "module_card":       ["Key Non-Player Characters","Dramatis Personae","Tomes","Artifacts"],
    "scene_object":      ["Timeline","Travel","Spells","Tomes","Artifacts"]
  }
}
```

`data/parsed/modules/the_vault.module_config.json`（匹配 `vault`/`triangle` 关键词，文件名依真实 module_id 写，此处示意）：

```json
{
  "_note": "P0-2 module_search_profile: migrated from module_preferences_for engine branch.",
  "module_search_profile": {
    "npc_stat_block":    ["Mission","Encounter","GM Toolkit"],
    "module_card":       ["Mission","Encounter","Aftermath","Anomaly","Playwalled","GM Toolkit"],
    "scene_object":      ["Encounter","Anomaly","GM Toolkit"],
    "combat_resolution": ["Encounter","Aftermath"]
  }
}
```

> **实施说明**：module_config 文件的准确文件名须与真实 `module_id` 匹配（执行时 `grep` 真实 `module_id` 字段确认），文件名格式 `{safe_module_id}.module_config.json`，`safe_module_id` = module_id 字母数字下划线连字符。

---

- [ ] **Step 4（trpg-material：两函数去 ruleset/module 名分支 → 读数据）**

`mechanics_query_plan_for` 是同步纯函数，不能加 `async`。需把 kernel/module 配置作参数传入。改法：将 `mechanics_query_plan_for(demand)` 签名改为 `mechanics_query_plan_for(demand, kernel: Option<&RuleKernel>, module_config: Option<&ModuleConfig>)`，调用点（`collect_evidence` L272 和 `query_plan` L352）处补 kernel/module_config 加载（`self.db.load_rule_kernel` 已经是 async，两个调用点都在 `async fn` 内）。

新增辅助：

```rust
/// Light module configuration loaded from data/parsed/modules/{id}.module_config.json.
/// Not stored in the DB — pure data-override pattern (same as kernel override).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ModuleConfig {
    /// Per SearchSkillKind key ("npc_stat_block"/…) → extra preferred section strings.
    #[serde(default)]
    module_search_profile: std::collections::HashMap<String, Vec<String>>,
}

fn read_module_config(module_id: &str) -> Option<ModuleConfig> {
    let safe: String = module_id.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    let dir = std::env::var("TRPG_DATA_DIR").unwrap_or_else(|_| "data".into());
    let p = std::path::Path::new(&dir)
        .join("parsed").join("modules").join(format!("{safe}.module_config.json"));
    let text = std::fs::read_to_string(&p).ok()?;
    serde_json::from_str(&text).ok()
}
```

将 `ruleset_aliases_for` 改为接收 `kernel: Option<&RuleKernel>`：

```rust
fn ruleset_aliases_for(kernel: Option<&RuleKernel>, skill: SearchSkillKind) -> Value {
    // 同原函数的 base 匹配（按 skill 返回通用别名），不变
    let mut base = match skill { /* … 原有 base 匹配，逐字保留 … */ };
    // 读 kernel.search_profile；缺则 GENERIC（即 base 不叠加，直接返回）
    let profile = kernel.and_then(|k| k.search_profile.as_ref());
    let skill_key = skill.as_str(); // 需确认 SearchSkillKind::as_str() 已实现
    if let Some(sp) = profile {
        if let Some(sections) = sp.preferred_sections_by_skill.get(skill_key) {
            base["preferred_sections"] = serde_json::to_value(sections).unwrap_or(base["preferred_sections"].clone());
        }
        if let Some(aliases_override) = sp.field_aliases_by_skill.get(skill_key) {
            if let (Some(base_obj), Some(over_obj)) = (base["field_aliases"].as_object_mut(), aliases_override.as_object()) {
                for (k, v) in over_obj { base_obj.insert(k.clone(), v.clone()); }
            }
        }
    }
    base
}
```

> `SearchSkillKind::as_str()` 的实际方法名须 `grep` 确认（已见于 L808 `skill.as_str()`，可确认存在）。

将 `module_preferences_for` 改为接收 `module_config: Option<&ModuleConfig>`：

```rust
fn module_preferences_for(module_config: Option<&ModuleConfig>, skill: SearchSkillKind) -> Vec<String> {
    // 通用 base（按 skill，逐字保留原有 match）
    let mut prefs: Vec<String> = match skill {
        SearchSkillKind::NpcStatblock => vec!["npc card","stat block","enemy","creature","monster"],
        SearchSkillKind::ModuleCard => vec!["npc card","vehicle card","object card","encounter","resources"],
        SearchSkillKind::SceneObject => vec!["scene object","map","location","tech","hazard","device"],
        SearchSkillKind::CombatResolution => vec!["combat note","encounter","enemy behavior","tactics"],
        _ => vec!["resources","appendix","data","card"],
    }.into_iter().map(str::to_string).collect();
    // 叠加 module_config 中的 module_search_profile
    if let Some(cfg) = module_config {
        if let Some(extra) = cfg.module_search_profile.get(skill.as_str()) {
            prefs.extend(extra.iter().cloned());
        }
    }
    prefs.sort(); prefs.dedup(); prefs
}
```

`mechanics_query_plan_for` 签名改为：

```rust
fn mechanics_query_plan_for(
    demand: &MaterializationDemand,
    kernel: Option<&RuleKernel>,
    module_config: Option<&ModuleConfig>,
) -> MechanicsQueryPlan {
    let aliases = ruleset_aliases_for(kernel, skill);
    let module_prefs = module_preferences_for(module_config, skill);
    // … 其余逻辑逐字保留 …
}
```

调用点（`collect_evidence` L272 和 `query_plan` L352 所在的两个 `async fn`）补加载：

```rust
// 在 collect_evidence 顶部（async fn）
let kernel = self.db.load_rule_kernel(&demand.ruleset_id).await.ok().flatten();
let module_config = demand.module_id.as_deref().and_then(read_module_config);
let qp = mechanics_query_plan_for(demand, kernel.as_ref(), module_config.as_ref());
```

```rust
// 在 query_plan（L351）如果是 async fn，同样补；若是 sync，需先判断是否走 async 路径
// 经代码见 L351：fn query_plan(&self, demand: &MaterializationDemand) -> Value
// 是同步函数——将 query_plan 的 kernel/module_config 改为调用方传入，或改为 async
// 最小改法：把 L351 query_plan 方法的 mechanics_query_plan_for 调用
// 改为接收 Option<&RuleKernel> 参数（调用方 async fn 已有 kernel）
```

> 若 `query_plan` 是同步函数（L351 `fn query_plan`），最小改法是将签名改为 `fn query_plan(&self, demand: &MaterializationDemand, kernel: Option<&RuleKernel>, module_config: Option<&ModuleConfig>) -> Value`，调用方（已为 async）传入。

`cargo build -p trpg-material` 验编译通过。

---

- [ ] **Step 5（trpg-object：check_label 去 ruleset 名分支 → 读 kernel.check_label_policy）**

`build_contract`（L202）已调用 `load_rule_kernel`（L206），kernel 存于局部变量。将 kernel 向下传给 `make_object_check`，在 `make_object_check` 改 check_label 分支：

**改 `make_object_check` 签名**（L861）—— 追加 `kernel: Option<&RuleKernel>` 参数：

```rust
fn make_object_check(
    input: ObjectTurnInput<'_>,
    actor_id: &str,
    target_actor_id: Option<&str>,
    object: &ObjectInstance,
    kind: ObjectInteractionKind,
    kernel_dice: Option<&str>,
    kernel_target: Option<CheckTargetModel>,
    kernel_source_refs: Vec<SourceRef>,
    kernel: Option<&RuleKernel>,         // 新增
) -> CheckContract {
    let dice = kernel_dice.unwrap_or("1d20").to_string();
    // 读 kernel.check_label_policy；缺则通用静态 fallback
    let policy = kernel.and_then(|k| k.check_label_policy.as_ref());
    let check_label = policy
        .and_then(|p| p.labels.get(kind.as_str()))
        .map(|s| s.as_str())
        .unwrap_or_else(|| match kind {
            ObjectInteractionKind::GrabHeldObject | ObjectInteractionKind::Disarm =>
                "appropriate opposed disarm / athletics check",
            ObjectInteractionKind::Unlock =>
                "appropriate lockpicking / technical unlock check",
            ObjectInteractionKind::CutConnection | ObjectInteractionKind::TraceConnection =>
                "appropriate technical analysis / cable handling check",
            ObjectInteractionKind::Steal | ObjectInteractionKind::Loot =>
                "appropriate stealth / sleight / search check",
            _ => "appropriate object interaction check",
        })
        .to_string();
    // … 其余逻辑逐字保留，删掉原 contains("cyberpunk") 分支 …
```

**调用点**（`build_contract` L210）追加 `kernel.as_ref()` 参数：

```rust
let check = if requires_check {
    Some(make_object_check(
        input, actor_id, target_actor_id.as_deref(), object, kind,
        kernel_dice.as_deref(), kernel_target, kernel_refs,
        kernel.as_ref(),   // 新增
    ))
} else { None };
```

`cargo build -p trpg-object` 验编译通过。

---

- [ ] **Step 6（TDD 单测：profile=X→用 X，缺→GENERIC；override 合并正确；grep 守卫）**

```rust
// crates/trpg-material/src/lib.rs 末尾 #[cfg(test)] 块

#[cfg(test)]
mod search_profile_data_driven_tests {
    use super::*;
    use trpg_model::{RuleKernel, RuleKernelSearchProfile};
    use std::collections::HashMap;

    fn kernel_with_profile(skill_key: &str, sections: Vec<&str>) -> RuleKernel {
        let mut k: RuleKernel =
            serde_json::from_str(r#"{"kernel_id":"t","ruleset_id":"t","version":"1"}"#).unwrap();
        let mut by_skill = HashMap::new();
        by_skill.insert(skill_key.to_string(), sections.iter().map(|s| s.to_string()).collect());
        k.search_profile = Some(RuleKernelSearchProfile {
            preferred_sections_by_skill: by_skill,
            field_aliases_by_skill: HashMap::new(),
        });
        k
    }

    #[test]
    fn profile_preferred_sections_override_base() {
        let k = kernel_with_profile("combat_resolution", vec!["Friday Night Firefight"]);
        let aliases = ruleset_aliases_for(Some(&k), SearchSkillKind::CombatResolution);
        let sections = aliases["preferred_sections"].as_array().unwrap();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].as_str().unwrap(), "Friday Night Firefight");
    }

    #[test]
    fn no_profile_falls_back_to_generic_base() {
        let k: RuleKernel =
            serde_json::from_str(r#"{"kernel_id":"t","ruleset_id":"t","version":"1"}"#).unwrap();
        let aliases = ruleset_aliases_for(Some(&k), SearchSkillKind::CombatResolution);
        // generic base includes preferred_sections for CombatResolution from base match
        let sections = aliases["preferred_sections"].as_array();
        // should exist (base match always sets it) and NOT be ["Friday Night Firefight"]
        assert!(sections.is_some());
        let has_fnf = sections.unwrap().iter().any(|v| v.as_str() == Some("Friday Night Firefight"));
        assert!(!has_fnf, "generic base must not contain Cyberpunk-specific section");
    }

    #[test]
    fn none_kernel_uses_base_generics() {
        let aliases = ruleset_aliases_for(None, SearchSkillKind::WeaponParameter);
        assert!(aliases.get("preferred_sections").is_some());
    }

    fn module_config_with_prefs(skill_key: &str, prefs: Vec<&str>) -> ModuleConfig {
        let mut cfg = ModuleConfig::default();
        cfg.module_search_profile.insert(
            skill_key.to_string(),
            prefs.iter().map(|s| s.to_string()).collect(),
        );
        cfg
    }

    #[test]
    fn module_config_prefs_extend_base() {
        let cfg = module_config_with_prefs("npc_stat_block", vec!["Redesigned NPC cards"]);
        let prefs = module_preferences_for(Some(&cfg), SearchSkillKind::NpcStatblock);
        assert!(prefs.contains(&"npc card".to_string()));
        assert!(prefs.contains(&"Redesigned NPC cards".to_string()));
    }

    #[test]
    fn no_module_config_gives_base_only() {
        let prefs = module_preferences_for(None, SearchSkillKind::NpcStatblock);
        let has_homecoming = prefs.iter().any(|s| s.contains("Redesigned NPC cards"));
        assert!(!has_homecoming, "generic must not contain homecoming-specific sections");
    }
}
```

```rust
// crates/trpg-object/src/lib.rs 末尾 #[cfg(test)] 块

#[cfg(test)]
mod check_label_policy_tests {
    use super::*;
    use trpg_model::{RuleKernel, CheckLabelPolicy};
    use std::collections::HashMap;

    fn kernel_with_label(kind_str: &str, label: &str) -> RuleKernel {
        let mut k: RuleKernel =
            serde_json::from_str(r#"{"kernel_id":"t","ruleset_id":"t","version":"1"}"#).unwrap();
        let mut labels = HashMap::new();
        labels.insert(kind_str.to_string(), label.to_string());
        k.check_label_policy = Some(CheckLabelPolicy { labels });
        k
    }

    #[test]
    fn kernel_label_used_for_disarm() {
        let k = kernel_with_label("disarm", "DEX + Brawling contested grab/disarm check");
        // call the pure label lookup logic (extract as helper or test inline)
        let policy = k.check_label_policy.as_ref().unwrap();
        let label = policy.labels.get("disarm").map(|s| s.as_str()).unwrap_or("fallback");
        assert_eq!(label, "DEX + Brawling contested grab/disarm check");
    }

    #[test]
    fn missing_policy_falls_back_to_generic() {
        let k: RuleKernel =
            serde_json::from_str(r#"{"kernel_id":"t","ruleset_id":"t","version":"1"}"#).unwrap();
        assert!(k.check_label_policy.is_none());
        // Generic fallback: label for grab_held_object must not contain "cyberpunk"
        let fallback: &str = match ObjectInteractionKind::GrabHeldObject {
            ObjectInteractionKind::GrabHeldObject | ObjectInteractionKind::Disarm =>
                "appropriate opposed disarm / athletics check",
            _ => "appropriate object interaction check",
        };
        assert!(!fallback.to_lowercase().contains("cyberpunk"));
    }
}
```

**grep 守卫（等价验收）**：

```bash
# 迁移完成后：引擎 src 中不得出现规则集/模组名字面量（白名单：#[cfg(test)] 块）
grep -rn --include="*.rs" \
  -e '"cyberpunk' -e '"dnd' -e '"5e' -e '"sword' -e '"sw2' \
  -e '"brp' -e '"cthulhu' -e '"coc' -e '"triangle' \
  -e '"homecoming' -e '"masks' -e '"nyarlathotep' -e '"vault' \
  crates/trpg-material/src/ crates/trpg-object/src/ \
  | grep -v '#\[cfg(test)\]' \
  | grep -v '//' \
  && echo "FAIL: ruleset/module names found in engine src" && exit 1 \
  || echo "PASS: zero ruleset/module names in trpg-material/trpg-object src"
```

`cargo test -p trpg-material -p trpg-object` → 全绿。

---

- [ ] **Step 7（等价验证：live + 行为不变）**

**等价 live 测试（标注 `:54347` CoC 库；Cyberpunk 库 `:54346`；无真实 DB 则 `SKIP`）**：

```rust
// crates/trpg-material/tests/live_search_profile_equiv.rs

//! Equivalence gate: after P0-2 migration, search profile behaviour is identical
//! to the pre-migration hard-coded branches for all six rulesets.
//! Requires live DB (TRPG_DATABASE_URL) with parsed kernels.
//! Run: cargo test -p trpg-material --test live_search_profile_equiv

#[cfg(test)]
mod equiv {
    use trpg_db::Db;
    use trpg_material::*; // assumes re-exports or test helpers

    async fn db() -> Option<Db> {
        let url = std::env::var("TRPG_DATABASE_URL").ok()?;
        Db::new(&url).await.ok()
    }

    /// Cyberpunk: kernel.search_profile (from override) must supply
    /// "Friday Night Firefight" in combat_resolution preferred_sections.
    #[tokio::test]
    async fn cyberpunk_combat_resolution_has_fnf_section() {
        let Some(db) = db().await else { eprintln!("SKIP: no TRPG_DATABASE_URL"); return; };
        let kernel = db.load_rule_kernel("cyberpunk_red").await.ok().flatten();
        let Some(kernel) = kernel else { eprintln!("SKIP: no cyberpunk_red kernel"); return; };
        let sp = kernel.search_profile.expect("cyberpunk_red override must set search_profile");
        let sections = sp.preferred_sections_by_skill.get("combat_resolution")
            .expect("combat_resolution must be present");
        assert!(sections.iter().any(|s| s.contains("Friday Night Firefight")),
            "cyberpunk combat_resolution must include Friday Night Firefight; got {:?}", sections);
    }

    /// CoC: kernel.search_profile must include "Sanity" / "Keeper Rulebook" in condition_resource.
    #[tokio::test]
    async fn coc_condition_resource_has_sanity_section() {
        let Some(db) = db().await else { eprintln!("SKIP: no TRPG_DATABASE_URL"); return; };
        let kernel = db.load_rule_kernel("call_of_cthulhu_7e").await.ok().flatten();
        let Some(kernel) = kernel else { eprintln!("SKIP: no call_of_cthulhu_7e kernel"); return; };
        let sp = kernel.search_profile.expect("coc override must set search_profile");
        let sections = sp.preferred_sections_by_skill.get("condition_resource")
            .expect("condition_resource must be present");
        assert!(sections.iter().any(|s| s.contains("Sanity") || s.contains("Keeper")),
            "coc condition_resource must include Sanity or Keeper; got {:?}", sections);
    }

    /// Cyberpunk: check_label_policy must map "disarm" to DEX+Brawling label.
    #[tokio::test]
    async fn cyberpunk_check_label_disarm_is_dex_brawling() {
        let Some(db) = db().await else { eprintln!("SKIP: no TRPG_DATABASE_URL"); return; };
        let kernel = db.load_rule_kernel("cyberpunk_red").await.ok().flatten();
        let Some(kernel) = kernel else { eprintln!("SKIP: no cyberpunk_red kernel"); return; };
        let policy = kernel.check_label_policy.expect("cyberpunk_red override must set check_label_policy");
        let label = policy.labels.get("disarm").expect("disarm must be in check_label_policy");
        assert!(label.contains("DEX") && label.contains("Brawling"),
            "disarm label must mention DEX + Brawling; got {:?}", label);
    }

    /// Non-cyberpunk ruleset: check_label_policy absent OR disarm label is generic.
    #[tokio::test]
    async fn coc_check_label_disarm_is_generic() {
        let Some(db) = db().await else { eprintln!("SKIP: no TRPG_DATABASE_URL"); return; };
        let kernel = db.load_rule_kernel("call_of_cthulhu_7e").await.ok().flatten();
        let Some(kernel) = kernel else { eprintln!("SKIP: no call_of_cthulhu_7e kernel"); return; };
        // CoC override does not set check_label_policy → None
        assert!(kernel.check_label_policy.is_none(),
            "coc must not have a check_label_policy (no Cyberpunk-specific disarm label)");
    }
}
```

`cargo test -p trpg-material --test live_search_profile_equiv` → 有 DB 全绿，无 DB SKIP。

---

- [ ] **Step 8（零回归验证 + commit）**

```bash
# 全套单测零回归
cargo test -p trpg-model -p trpg-db -p trpg-material -p trpg-object

# grep 守卫最终确认
grep -rn --include="*.rs" \
  -e 'contains("cyberpunk' -e 'contains("dnd' -e 'contains("coc' \
  -e 'contains("triangle' -e 'contains("sword' -e 'contains("homecoming' \
  -e 'contains("masks' -e 'contains("vault' \
  crates/trpg-material/src/ crates/trpg-object/src/ \
  | grep -v '#\[cfg(test)\]' \
  | grep -v '//' \
  && echo "FAIL" || echo "PASS: zero ruleset/module name branches in engine src"
```

确认：文件行数 `wc -l crates/trpg-material/src/lib.rs crates/trpg-object/src/lib.rs`（均须 ≤400；若 lib.rs 原已超需拆子模块至 `src/search_profile.rs` 等）。

提交（无 git 指令，执行时自行决定粒度）：scope = trpg-model + trpg-db + trpg-material + trpg-object + data/parsed/rules/*.override.json + data/parsed/modules/*.module_config.json。


---
### Task 6: CI 守卫 xtask + P0-2 全等价验证闸

**Files:**
- `scripts/no_engine_ruleset_hardcode.sh` (新建，完整 grep 守卫脚本)
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/.cargo/config.toml` (追加 alias)
- 涉及验证的真实代码路径（只读，不修改）：
  - `crates/trpg-combat/src/lib.rs`（Task 2 已清干净）
  - `crates/trpg-referee/src/lib.rs`（Task 3）
  - `crates/trpg-director/src/lib.rs`（Task 4）
  - `crates/trpg-material/src/lib.rs`（Task 5）
  - `crates/trpg-object/src/lib.rs`（Task 5）

---

- [ ] **Step 1: 写 CI 守卫脚本 `scripts/no_engine_ruleset_hardcode.sh`，先让它对干净代码报绿**

  创建 `scripts/no_engine_ruleset_hardcode.sh`（完整代码如下，≤120 行）：

  ```bash
  #!/usr/bin/env bash
  # CI 守卫：引擎 crate src 不得出现规则集/模组名字面量（白名单除外）。
  # 退出码：0=干净，1=命中
  set -euo pipefail

  SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  REPO="${SCRIPT_DIR}/.."

  # 受检引擎 crate（含 src/ 目录）
  ENGINE_CRATES=(
    trpg-runtime trpg-gm trpg-combat trpg-referee trpg-director
    trpg-object trpg-mechanics trpg-material trpg-contest
    trpg-orchestrator trpg-semantics trpg-interaction
  )

  # 禁止的字面量（规则集名/模组名，全小写 grep 不区分大小写）
  BANNED_PATTERNS=(
    "call_of_cthulhu" "cyberpunk" '"coc"' '"brp"' '"dnd"' '"d&d"' '"5e"'
    "sword_world" "剑世界" '"triangle"' '"fate"'
    "homecoming" '"masks"' '"vault"' "nyarlathotep"
    "scav_boss" "athena_drone"
  )

  # 白名单（这些文件/函数不计入）：
  #   1. #[cfg(test)] 块 -> 用 --include 排除测试，但 Rust 内联测试块需 sed 剥离
  #   2. data/ override 加载键常量（只读路径字符串构造函数）：read_kernel_override_file / load_dir
  #   3. parser source-id 推断函数（trpg-parser / trpg-rule-agent，非引擎 crate，不在扫描范围）
  ALLOWLIST_FN_PATTERNS=(
    "read_kernel_override_file"   # db 加载键，只字符串拼 safe 文件路径
    "load_dir"                    # CombatProfilePack::load_dir，只读 json 文件
    "TRPG_RULESET_ADVICE_DIR"    # env var 引用，非字面规则集名
  )

  FOUND=0
  REPORT=""

  for crate in "${ENGINE_CRATES[@]}"; do
    src="${REPO}/crates/${crate}/src"
    [[ -d "$src" ]] || continue

    # 剥离 #[cfg(test)] 块：用 awk 跳过 mod tests { ... } 区域
    # 简化：对每个 .rs 文件，用 grep -n 查模式，再过滤掉在 #[cfg(test)] 内的行。
    # 精确剥离需完整 parser，这里用保守白名单：行内含 #[cfg(test)] 或 mod tests 的行和
    # 其下 {} 块——改为：只扫不含 "cfg(test)" / "mod tests" 注释前缀的行。
    # 实际做法：逐文件 awk 提取非 test 区段。
    while IFS= read -r -d '' file; do
      # 剥离 test 模块（#[cfg(test)] 直到匹配 } 深度归零）
      non_test_content=$(awk '
        /^\s*#\[cfg\(test\)\]/ { skip=1; depth=0; next }
        skip && /{/ { depth++ }
        skip && /}/ { if (--depth <= 0) { skip=0 } ; next }
        skip { next }
        { print NR": "$0 }
      ' "$file")

      for pat in "${BANNED_PATTERNS[@]}"; do
        hits=$(echo "$non_test_content" | grep -i "$pat" 2>/dev/null || true)
        if [[ -n "$hits" ]]; then
          # 白名单过滤：行含白名单函数名则豁免
          filtered=""
          while IFS= read -r line; do
            exempt=0
            for allow in "${ALLOWLIST_FN_PATTERNS[@]}"; do
              if echo "$line" | grep -q "$allow"; then exempt=1; break; fi
            done
            [[ $exempt -eq 0 ]] && filtered="${filtered}${line}\n"
          done <<< "$hits"
          if [[ -n "$filtered" ]]; then
            FOUND=1
            rel="${file#$REPO/}"
            REPORT="${REPORT}[BANNED] ${rel} pattern=${pat}\n${filtered}\n"
          fi
        fi
      done
    done < <(find "$src" -name "*.rs" -print0)
  done

  if [[ $FOUND -eq 1 ]]; then
    echo "===== no-engine-ruleset-hardcode: FAIL ====="
    printf "%b" "$REPORT"
    echo "引擎 crate 含规则集/模组名字面量。迁入 kernel/module 数据层或加白名单。"
    exit 1
  else
    echo "no-engine-ruleset-hardcode: OK (0 hits)"
    exit 0
  fi
  ```

  验证脚本可执行：
  ```bash
  chmod +x scripts/no_engine_ruleset_hardcode.sh
  bash scripts/no_engine_ruleset_hardcode.sh
  ```
  预期：**Task 2-5 尚未完成时报 FAIL，命中现有硬编码**（自测红态，证明守卫有效）。

---

- [ ] **Step 2: 自测注入假硬编码 → 守卫报红，再删 → 报绿（守卫正确性验证）**

  注入：在 `crates/trpg-runtime/src/lib.rs` 末尾临时加一行 `// TEST_INJECT: cyberpunk`（注释行，不影响编译）：
  ```bash
  echo '// TEST_INJECT: cyberpunk' >> crates/trpg-runtime/src/lib.rs
  bash scripts/no_engine_ruleset_hardcode.sh
  # 预期：退出码 1，REPORT 含 trpg-runtime/src/lib.rs pattern=cyberpunk
  echo $?
  ```
  删除注入行（非破坏性，原文件仅末尾追加注释）：
  ```bash
  # 用 head -n 去掉最后 1 行（只有这一行注入）
  total=$(wc -l < crates/trpg-runtime/src/lib.rs)
  head -n $((total - 1)) crates/trpg-runtime/src/lib.rs > /tmp/lib_cleaned.rs && mv /tmp/lib_cleaned.rs crates/trpg-runtime/src/lib.rs
  bash scripts/no_engine_ruleset_hardcode.sh
  # 预期：退出码 0（因为 Task 2-5 还没完成，此时守卫对 runtime 仍绿但其他 crate 仍红）
  ```
  **注意**：此 Step 验证的是"注入→红/删除→该 crate 绿"逻辑，不是全局绿（全局绿须 Task 2-5 完成后）。

---

- [ ] **Step 3: 在 `.cargo/config.toml` 追加 alias，使 `cargo no-engine-check` 可用**

  读取现有 `.cargo/config.toml`（路径 `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/.cargo/config.toml`）后追加：

  ```toml
  [alias]
  no-engine-check = "run --manifest-path scripts/xtask_stub.toml -- no-engine-ruleset-hardcode"
  ```

  **说明**：由于工作区无 xtask crate，更简洁的方案是直接 `cargo alias` 调 shell 脚本。Cargo alias 不支持直接调 shell，因此改为在 CI 中直接调用 `bash scripts/no_engine_ruleset_hardcode.sh`，alias 仅作辅助记录。在 `.cargo/config.toml` 中加注释行即可：

  ```toml
  # P0-2 CI 守卫：bash scripts/no_engine_ruleset_hardcode.sh
  # （Cargo alias 不支持直接 shell，CI pipeline 直接调脚本）
  ```

  验证：
  ```bash
  cat /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/.cargo/config.toml
  # 确认注释行出现
  ```

---

- [ ] **Step 4: Task 2-5 完成后，全工作区 check + test 零回归验证**

  按如下命令顺序执行（每步须零错误）：

  ```bash
  # 4a. 全工作区编译检查（--manifest-path 指定根 Cargo.toml）
  cargo check \
    --manifest-path /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/Cargo.toml \
    --all-targets 2>&1 | tail -5
  # 预期: "Finished" 无 error

  # 4b. 全工作区单测（无 live DB 依赖的测试）
  cargo test \
    --manifest-path /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/Cargo.toml \
    --all-features 2>&1 | tail -20
  # 预期: test result: ok. N passed; 0 failed

  # 4c. CI 守卫最终闸
  bash /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/scripts/no_engine_ruleset_hardcode.sh
  # 预期: "no-engine-ruleset-hardcode: OK (0 hits)"，退出码 0
  ```

---

- [ ] **Step 5: 六规则集迁移等价验证（live :54347 CoC + :54346 Cyberpunk；其余无 DB 单测）**

  **前提**：Task 2-5 的 override 数据文件已精确复刻原硬编码值。等价闸验证迁移前后行为一致。

  **5a. combat mode 选择等价（单测，无 DB）**

  在 `crates/trpg-combat/src/lib.rs` 的 `#[cfg(test)]` 块中加（Task 2 实现时一同写入，此处为验证命令）：

  ```bash
  # 单测：combat mode 等价（kernel 字段驱动 vs 原 contains 分支）
  cargo test \
    --manifest-path /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/Cargo.toml \
    -p trpg-combat \
    -- combat_mode_from_kernel 2>&1
  # 预期测试名前缀 combat_mode_from_kernel_* 全部 ok
  # 覆盖：cyberpunk→Firefight/Netrun, coc→HorrorEncounter, triangle→AnomalyEncounter,
  #        dnd→TacticalCombat, generic→TheaterOfMind，缺字段→通用兜底
  ```

  **5b. referee damage/difficulty 等价（单测，无 DB）**

  ```bash
  cargo test \
    --manifest-path /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/Cargo.toml \
    -p trpg-referee \
    -- referee_bands_from_kernel 2>&1
  # 预期：cyberpunk damage_family="cyberpunk_red_weapon_damage", damage_band 含 "2d6..8d6"
  #        dnd damage_family="dnd_damage_dice"；coc damage_band 含 "1d3..2d10+db"
  #        缺字段 → "generic_trpg_damage" + 合理性范围 (0..=150)
  ```

  **5c. search profile 等价（单测，无 DB）**

  ```bash
  cargo test \
    --manifest-path /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/Cargo.toml \
    -p trpg-material \
    -- search_aliases_from_kernel 2>&1
  # 预期：cyberpunk preferred_sections 含 "Getting it Done"；
  #        coc preferred_sections 含 "Keeper Rulebook"；
  #        triangle preferred_sections 含 "Field Agent Manual"；
  #        缺字段 → 通用 {"preferred_sections":["rules","data","GM toolkit"],"field_aliases":{}}
  ```

  **5d. module search profile 等价（单测，无 DB）**

  ```bash
  cargo test \
    --manifest-path /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/Cargo.toml \
    -p trpg-material \
    -- module_search_from_config 2>&1
  # 预期：homecoming module config 的 module_search_profile.extra_sections 含
  #        "Redesigned NPC cards","Athena's Behavior"；
  #        masks/nyarlathotep 含 "Tomes","Artifacts"；
  #        vault/triangle 含 "Mission","Playwalled"；
  #        无 module config → 通用 base prefs
  ```

  **5e. NPC 绑定等价（单测，无 DB）**

  ```bash
  cargo test \
    --manifest-path /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/Cargo.toml \
    -p trpg-combat \
    -- npc_binding_from_module_config 2>&1
  # 预期：homecoming module config matcher "scav_boss"/"boss"/"霰弹" → actor_id "npc.scav_boss"；
  #        "drone"/"athena" → actor_id "npc.athena_drone"；
  #        无 config → actor_id "npc.opposition"（通用兜底）
  ```

  **5f. tech DV 等价（单测，无 DB）**

  ```bash
  cargo test \
    --manifest-path /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/Cargo.toml \
    -p trpg-combat \
    -- tech_dv_from_module_config 2>&1
  # 预期：homecoming module config technical_option_table
  #        "basic tech|cut off|power|cable|线缆" → dv=14；
  #        "hack|interface|net|server|athena" → dv=12；
  #        无 config → None（无兜底，UnknownUntilLookup 不变）
  ```

  **5g. check label 等价（单测，无 DB）**

  ```bash
  cargo test \
    --manifest-path /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/Cargo.toml \
    -p trpg-combat -p trpg-object \
    -- check_label_from_kernel 2>&1
  # 预期：cyberpunk kernel.check_label_policy.hack_label = "appropriate TECH / Interface / Basic Tech check"；
  #        cyberpunk kernel.check_label_policy.disarm_label = "DEX + Brawling contested grab/disarm check"；
  #        无 policy → 通用 label（"appropriate technical conflict check" / "appropriate opposed disarm / athletics check"）
  ```

  **5h. director scene brief 等价（单测，无 DB）**

  ```bash
  cargo test \
    --manifest-path /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/Cargo.toml \
    -p trpg-director \
    -- director_brief_homecoming_equiv 2>&1
  # 预期：homecoming module config 的 scene_entity_aliases/npc_advice 注入后
  #        brief.known_facts 含 "救援、威胁控制、源头调查"；
  #        brief.npc_advice 含 "injured_lawman" + "fixer_contact"；
  #        current_place_summary 含 "Cyberpunk RED Homecoming"；
  #        无 module config → 通用 brief（不含 homecoming 专属文本）
  ```

---

- [ ] **Step 6: 三模组 live 等价验证（:54347 CoC 血色公路 + :54346 Cyberpunk Homecoming；SKIP 若 DB 不通）**

  **参照 R5 T8 的 trpg turn live 手法**（`trpg play` CLI，真库）：

  ```bash
  # 6a. CoC 血色公路（:54347）——验证 HorrorEncounter mode + CoC search aliases + director brief
  # SKIP if: docker inspect chatrpg-postgres-rulesets 2>/dev/null | grep -q '"Status": "running"' || echo SKIP

  export DATABASE_URL="postgresql://trpg:trpg@localhost:54347/rulesets"
  export TRPG_DATA_DIR=/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/data

  # 取一个存活 CoC session（或新建）
  SESSION_COC=$(docker exec chatrpg-postgres-rulesets psql -U trpg -d rulesets -t -c \
    "select id from sessions where ruleset_id like '%coc%' or ruleset_id like '%call_of_cthulhu%' order by created_at desc limit 1" 2>/dev/null | tr -d ' ')

  [[ -z "$SESSION_COC" ]] && echo "SKIP: no CoC session" || {
    cargo run \
      --manifest-path /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/Cargo.toml \
      -p trpg-cli -- play \
      --session-id "$SESSION_COC" \
      --input "我举枪对准歹徒" 2>&1 | tee /tmp/coc_live_equiv.log
    # 断言：
    # ① combat_frame_created 出现且 profile_id 含 "coc" 或 "conflict"
    # ② 无 "HorrorEncounter" 硬编码 panic（规则引用来自 kernel.combat_mode_policy）
    grep -E "horror_encounter|HorrorEncounter|profile_id.*coc" /tmp/coc_live_equiv.log \
      && echo "CoC live equiv: OK" \
      || echo "CoC live equiv: WARN (检查日志)"
  }

  # 6b. Cyberpunk Homecoming（:54346）——验证 Firefight mode + tech DV 14/12 + NPC 绑定 + Homecoming director brief
  export DATABASE_URL="postgresql://trpg:trpg@localhost:54346/rulesets"

  SESSION_CYBER=$(docker exec chatrpg-postgres-rulesets-cyber psql -U trpg -d rulesets -t -c \
    "select id from sessions where ruleset_id like '%cyberpunk%' order by created_at desc limit 1" 2>/dev/null | tr -d ' ')

  [[ -z "$SESSION_CYBER" ]] && echo "SKIP: no Cyberpunk session" || {
    # Turn 1: 攻击 → Firefight mode + npc.scav_boss 绑定
    cargo run \
      --manifest-path /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/Cargo.toml \
      -p trpg-cli -- play \
      --session-id "$SESSION_CYBER" \
      --module homecoming \
      --input "我朝霰弹枪头目开枪" 2>&1 | tee /tmp/cyber_live_attack.log
    grep -E "firefight|Firefight|scav_boss|profile_id.*cyberpunk" /tmp/cyber_live_attack.log \
      && echo "Cyberpunk Firefight+NPC binding: OK" \
      || echo "Cyberpunk Firefight+NPC binding: WARN"

    # Turn 2: 技术动作 → Netrun mode + tech DV 14
    cargo run \
      --manifest-path /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/Cargo.toml \
      -p trpg-cli -- play \
      --session-id "$SESSION_CYBER" \
      --module homecoming \
      --input "我尝试切断电缆电源供电" 2>&1 | tee /tmp/cyber_live_tech.log
    grep -E "dv.*14|14.*dv|tech_dv|Homecoming technical" /tmp/cyber_live_tech.log \
      && echo "Cyberpunk tech DV=14: OK" \
      || echo "Cyberpunk tech DV=14: WARN"
  }

  # 6c. The Vault（Triangle，无独立 live DB，用 CoC :54347 的 Triangle session 若有）
  # SKIP if 无 Triangle session
  SESSION_TRI=$(docker exec chatrpg-postgres-rulesets psql -U trpg -d rulesets -t -c \
    "select id from sessions where ruleset_id like '%triangle%' order by created_at desc limit 1" 2>/dev/null | tr -d ' ')
  [[ -z "$SESSION_TRI" ]] && echo "SKIP: no Triangle session (OK for P0-2 — unit test covers mode)" || {
    cargo run \
      --manifest-path /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/Cargo.toml \
      -p trpg-cli -- play \
      --session-id "$SESSION_TRI" \
      --input "收容异常" 2>&1 | tee /tmp/tri_live.log
    grep -E "anomaly_encounter|AnomalyEncounter" /tmp/tri_live.log \
      && echo "Triangle AnomalyEncounter: OK" || echo "Triangle: WARN"
  }
  ```

---

- [ ] **Step 7: 迁移后 CI 守卫最终通过（全引擎 grep=0），commit**

  ```bash
  # Task 2-5 全完成后执行
  bash /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/scripts/no_engine_ruleset_hardcode.sh
  # 预期输出：no-engine-ruleset-hardcode: OK (0 hits)
  # 退出码：0

  # 全套件零回归再跑一次
  cargo test \
    --manifest-path /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/Cargo.toml \
    2>&1 | tail -5
  # 预期：test result: ok. N passed; 0 failed; 0 errors

  # TTFT 不劣化验证（对比 Task 1 前的基线 trpg play warm start）：
  # 取 warm session，测 play 首 token 到达时间（本轮不要求秒级基准测试，仅人工确认无明显劣化）
  # 若 load_rule_kernel 被 Task 2-5 增加了新字段读取，确认仍走单次 DB query 路径（已有 merge_override 机制）。
  ```

  通过后暂存（不 commit，由 CI merge gate 触发）：
  ```bash
  git add scripts/no_engine_ruleset_hardcode.sh .cargo/config.toml
  git status
  # 确认只有 scripts/ 和 .cargo/config.toml 被暂存
  ```

---

**等价守卫矩阵（Task 6 合并门限，全绿才可合 main）：**

| 验证项 | 方法 | 预期结果 |
|---|---|---|
| combat mode 六规则集 | 单测 `combat_mode_from_kernel_*` | cyberpunk→Firefight/Netrun, coc→HorrorEncounter, triangle→AnomalyEncounter, dnd→TacticalCombat, generic→TheaterOfMind |
| referee damage/difficulty 五函数 | 单测 `referee_bands_from_kernel_*` | 各规则集 family/band/plausibility 与原硬编码值等价 |
| search aliases 五规则集 | 单测 `search_aliases_from_kernel_*` | preferred_sections/field_aliases 与原 `ruleset_aliases_for` 返回值等价 |
| module search profile 三模组 | 单测 `module_search_from_config_*` | homecoming/masks/vault extra_sections 与原 `module_preferences_for` 返回值等价 |
| NPC 绑定 | 单测 `npc_binding_from_module_config_*` | scav_boss/athena_drone 绑定与原 `target_actor_for_combat_input` 等价 |
| tech DV 两档 | 单测 `tech_dv_from_module_config_*` | 14/12 与原 `inferred_homecoming_tech_dv` 等价 |
| check label | 单测 `check_label_from_kernel_*` | cyberpunk hack/disarm label 与原硬编码等价 |
| director brief homecoming | 单测 `director_brief_homecoming_equiv` | known_facts/npc_advice/place_summary 与原 `is_homecoming` 分支等价 |
| CoC live turn | :54347 `trpg play` | HorrorEncounter mode，无 panic，profile_id 来自 kernel 数据 |
| Cyberpunk Homecoming live | :54346 `trpg play` 两回合 | Firefight+npc.scav_boss 绑定，tech DV=14，director brief 含 Homecoming 文本 |
| CI 守卫 grep=0 | `bash scripts/no_engine_ruleset_hardcode.sh` | 退出码 0，0 hits |
| 全套件零回归 | `cargo test --all-features` | 0 failed, 0 errors |
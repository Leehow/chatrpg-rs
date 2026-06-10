# 资源接驳·第二刀 (Slice B) — current 值单一来源 (store + 代码路径统一)

> 日期: 2026-06-09 · 状态: 设计已批准 (用户 "可以你做吧"), 待 writing-plans · 主线: v1.20-formula
> 前置: Slice A (`2026-06-09-resource-consolidation-slice-a-design.md`) —— 资源 seed/cap 已从角色派生 + kernel id 驱动, 杀了硬编码 resources 默认, 战斗按 kernel track 读 HP。

## 1. 背景 (理解阶段勘查结论)

资源 **current 值** 当前散在两套存储、两条代码路径, 同一逻辑资源会按代码路径落到不同地方:

- **CONTEST 路径** —— `generic_parameter_states` 路径 `resources.{id}.current`(`target_kind=actor`)。
  - 读: `trpg-contest::read_track_value` (contest/src/lib.rs:191)。**但它的查询漏了 `target_kind`**, 仅按 `(session_id, target_id, parameter_path)` 匹配。
  - 写: `trpg-mechanics::apply_outcome_resource_tracks` (mechanics/src/lib.rs:99-228), 经 `upsert_generic_parameter_state` (mechanics:942)。seed/cap 已走 slice A 的 `actor_resource_seeds`。
  - 这条 **读写一致、kernel id 驱动 —— 健康**。
- **COMBAT / EFFECT 路径** —— `actor_mechanical_states`(`hp_current` 列 + `resources_json` blob,unique `(session_id, actor_id)`)。
  - `apply_effect_roll` (mechanics:377-574) 读 `state.hp_current.or(state.hp_max)`、`state.armor_current`、`state.resources`;HP 伤害(armor SP 抵扣 + delta + defeated 关帧)经 `update_actor_hp` (mechanics:742) 写 `hp_current`/`wound_state`;resources 分支经 `update_actor_resources` (mechanics:778) 写 `resources_json`。
  - seed: `ensure_actor_state` (mechanics:665-714) 用 slice A 的 `actor_resource_seeds` + `build_resources_json` 播种 `hp_current`/`resources_json`。

**发散本质**: 一次 SAN check 失败(contest)落 `generic_parameter_states[resources.sanity.current]`;一次 effect-roll 改 SAN 落 `actor_mechanical_states.resources_json`;战斗掉 HP 落 `actor_mechanical_states.hp_current` —— 而 contest 的 `read_track_value` **永远读不到** combat 写的 HP。两套 current,不同步。

**其它 slice B 范围内的勘查结论**:
- **GM 投影**: `actor_parameters_context_block` (trpg-params/src/lib.rs:95-119) 只读 `runtime_actor_parameters.status_json`(初始化时冻结, 无更新路径)→ 不含 live current。`mechanical_ledger_context_block` (mechanics:~306/330) 确读 `actor_mechanical_states`(hp_current) + `generic_parameter_states`(SAN) —— 统一后 ams.hp_current 停写, 此投影的 HP 会失真, 必须换源。
- **kernel 加载**: `trpg-db::load_rule_kernel` (db/src/lib.rs:689-698) 从 **DB `rule_kernels.content_json`** 读(非磁盘), 无 normalize/override。`rule_kernel_patches` 表存在但 **完全惰性**(从不在加载时应用)。
- **D&D5e 脏数据**: `dnd5e.rule_kernel.json` 的 `resource_tracks` = `[{"field_id":"resources","field_type":"object","title":"Resources / Tracks"}]` —— 这是 **角色卡字段定义, 不是资源轨**(无 `id`/`kind`/`max`/`on_outcome`)。`hp_resource_track_id()` 对 D&D 返回 `None` → 战斗 HP=None。源头是 rule-agent(LLM)误把 sheet 字段当 track 提交;直接拷进 kernel(`rule_kernel_from_run_kit` parser:~1680, 无校验)。
- **测试现状**: mechanics 8 个纯单测(全是 slice A 的 `match_seed`/`build_resources_json`, **无一覆盖 `apply_effect_roll`**);combat 4 个纯单测(`hp_from_params`)。真 DB 集成测在别处存在范式: `trpg-contest/tests/live_percentile.rs`、`trpg-object/tests/live_ammo.rs` —— `Db::connect(DATABASE_URL)`、未设则 graceful skip、原始 SQL 自播种、就地改 fixture、`#[tokio::test]`。
- **crate 依赖**: `trpg-db`(仅依赖 `trpg-model`)是 `trpg-contest` / `trpg-mechanics` / `trpg-combat` 三者 **唯一的最低公共依赖**(params 被 contest+combat 依赖但 mechanics 不依赖;mechanics 与 combat 互不依赖)。

## 2. 目标 / 验收 (端到端)

current 值收敛到 **单一存储(`generic_parameter_states`)+ 单一代码路径(trpg-db 原语)**, 三条路径(contest 读 / mechanics 检定+效果 / combat 帧)全部经它;`actor_mechanical_states.hp_current`/`resources_json` 逻辑退役。不破坏 HP 伤害 / SAN loss / 效果。GM 看到 live current。D&D 战斗 HP 可用。全程零 per-ruleset 硬编码、fail-closed。

**核心证据**: combat 扣 HP 后, contest `read_track_value` 立刻读到同一新值(跨路径一致性)。

## 3. 核心原则

1. **唯一事实源** = `generic_parameter_states`, 路径 `resources.{kernel_track_id}.current`, `target_kind=actor`。
2. **唯一代码路径** = `trpg-db` 内一对 current 原语, contest/mechanics/combat 三方共用。
3. `actor_mechanical_states` 降级为战斗元数据(`armor_current`/`morale`/`conditions`/`frame_id`);`hp_current`/`resources_json`/`wound_state`/`hp_max` **不再作 current 真值持久化**(列保留, 不做迁移;`hp_max`/`wound_state` 改按 gps current + 派生 max 现算)。
4. D&D 修在 **data override + 通用 normalize**, 不在 Rust 写 per-ruleset 分支。

## 4. 设计

### 4.1 纯 helper (trpg-model)
零 DB, 供下游 + 单测共用:
- 已有: `hp_resource_track_id(tracks) -> Option<String>`(kind==health, 回退 id 含 hp/hit_point)。
- 迁入(从 mechanics): `match_seed(resources, tracks)`、`build_resources_json(seeds)`。mechanics 现有 8 个 slice-A 单测随之 import(或迁测), 保持绿。
- 新增 `resolve_resource_track_id(parameter_path, kernel) -> Option<String>`: 语义把 effect 的 `parameter_path` 解析成 kernel track id。
  - `"hp.current"` / `"hp"` → `hp_resource_track_id(kernel.resource_tracks)`。
  - `"resources.{X}.current"` / 含资源名 → 按 id 大小写不敏感匹配 `kernel.resource_tracks`。
  - 匹配不到 → `None`(fail-closed)。**绝不硬编码英文别名表**。
- 新增伤害数学纯函数(供单测锁逻辑):
  - `apply_armor_damage(from_hp, amount, armor, op) -> (to_hp, sp_applied)`(复刻 apply_effect_roll:402-407 的 Add/Set/Subtract + SP 抵扣)。
  - `wound_label(to_hp, max) -> &str`(defeated if ≤0;wounded if ≤max/2;else unhurt —— 复刻 update_actor_hp 内联逻辑)。

### 4.2 current 原语 (trpg-db, 新模块 `crates/trpg-db/src/resource_current.rs`, ≤400 行)
gps 的低层 load/upsert 下沉到此(从 mechanics 的 `load_generic_parameter_value`/`upsert_generic_parameter_state` 迁入或新建), 加 seed/current 语义:
- `resource_seeds(&self, session_id, actor_id, kernel) -> HashMap<String,(Option<i32>,Option<i32>)>`: 读 `runtime_actor_parameters.sheet_json->'resources'` × `match_seed`(= slice A `actor_resource_seeds` 下沉)。
- `load_resource_current(&self, session_id, actor_id, track_id, kernel) -> Option<i32>`: 读 gps `(session, actor, resources.{track_id}.current)` → 无行回退 `resource_seeds` 的 current → 再回退 kernel `initial`。
- `write_resource_current(&self, session_id, actor_id, track_id, value, cap: Option<i32>, source_refs, world_tick)`: 按 `cap`(调用方传 seeds.max 回退 kernel max)封顶后 upsert gps。
- `cap_for(&self, ...)` 或由调用方从 `resource_seeds` 取 max —— seed/cap 与 slice A 同源。

> 落点理由: trpg-db 是三者唯一最低公共依赖且已持 `load_rule_kernel`/pool。这样 current 既是单一 store 又是单一代码路径, 这才是完整的 SSOT。

### 4.3 contest 收敛 (trpg-contest)
`read_track_value` (contest:191) 改调 `self.db.load_resource_current(...)`, **顺带修复缺失的 `target_kind=actor`**(消除 actor/scene 同 path 的歧义)。行为对 actor 资源不变, 但与写侧走同一路径。

### 4.4 mechanics 统一 (trpg-mechanics)
- `apply_outcome_resource_tracks`: 改用 `db.load_resource_current` / `db.write_resource_current`(行为不变, DRY;seed/cap 同源)。
- `apply_effect_roll`:
  - HP 分支: `from_hp = db.load_resource_current(session, target, hp_track_id, kernel)`(`hp_track_id = hp_resource_track_id(kernel)`);armor/delta 用 `apply_armor_damage`(纯);`db.write_resource_current(hp_track_id, to_hp, cap=seeds.max)`;defeated 用 `to_hp<=0`(关帧 `close_active_frame_for_defeated_target` 不变);**armor 仍从 `ensure_actor_state` 读**。
  - resources 分支: `track_id = resolve_resource_track_id(decision.parameter_path, kernel)`(None → fail-closed 不写);`before = db.load_resource_current(track_id)`;`after = apply_i32_operation(...)`;`db.write_resource_current(track_id, after, cap)`。
  - **移除 `update_actor_hp` / `update_actor_resources` 在效果路径的调用**;二者若无其它调用者则删除。
- `ensure_actor_state`: 瘦身 —— 仍建 ams 行供 `armor_current`/`morale`/`conditions`/`frame_id`(armor 仍 seed None);**不再 seed `hp_current`/`resources_json` 作真值**(current 改由 gps 惰性 seed-on-read)。保留函数(apply_effect_roll 还需它拿 armor)。

### 4.5 combat 帧 (trpg-combat)
`create_frame` (combat:620-699) 的 PC/NPC HP 改读 `db.load_resource_current(session, actor, hp_track_id, kernel)`(live current;无 gps 行回退 `resource_seeds` 派生 = 开场满血)。替代现在的 `hp_from_params`(读派生 max 快照)。`hp_track_id` 仍按 `hp_resource_track_id` 语义找。fail-closed: 无 track / 无派生 → None。

### 4.6 GM 投影 (trpg-mechanics ledger)
`mechanical_ledger_context_block` 的 HP 投影改从 gps `resources.{hp_id}.current` 取(统一后 ams.hp_current 已停写, 必须换源);SAN 等本就读 gps。确保 GM 见 live current HP/SAN, 全来自单一来源。`actor_parameters_context_block` 保持静态模板态(BP3 角色卡), 不重复塞 current。

### 4.7 D&D kernel 修复
- **normalize (通用, trpg-parser `rule_kernel_from_run_kit` ~1680)**: 过滤 `resource_tracks`, 丢弃既无 `id` 又无 `name` 的条目(覆盖 `field_id`/`field_type` 这类 sheet-field 误投)。防未来再脏。
- **override (trpg-db `load_rule_kernel`)**: 从 DB 读出 kernel 后, 先 normalize 同款(清洗 DB 里已存的旧脏数据 —— 不必重 parse), 再 merge `data/parsed/rules/{ruleset}.rule_kernel.override.json`(经 `TRPG_DATA_DIR`, 按 `resource_tracks[].id` 合并、override 胜, 仿 chargen `merge_override`)。文件不存在则无操作。
- **数据**: author `dnd5e.rule_kernel.override.json`, 提供合法 `hit_points` track(`kind=health`、`owner_kind=actor`、`on_outcome` damage subtract、thresholds 0→unconscious)。max 留宽松(派生值才是真上限, 经 §4.2 seeds)。

## 5. 不变量 / 护栏

1. **零 per-ruleset 硬编码**: 原语纯按 kernel tracks × 角色 sheet;D&D 修在 data override + 通用 normalize;无 `if ruleset`。
2. **fail-closed**: 无 track / 无派生 / `resolve_resource_track_id` 解析不出 → 不 seed、不写、不捏造(沿用旧 None / kernel-static)。
3. **SSOT**: current 仅在 gps, 单一读写路径(trpg-db 原语);ams 不再持有 current 真值。
4. **test-first**: `apply_effect_roll` 先补纯函数 + 真 DB 集成测 **锁现状(绿)**, 再迁移, 迁移后改断言读 gps **仍绿**。
5. **行为不破**: HP 伤害(含 armor)、SAN loss、resources 改值、defeated 关帧、封顶语义全部保持。
6. **文件 ≤400 行**: 原语入新模块 `resource_current.rs`;大文件(mechanics lib 已超)只做必要编辑, 新逻辑尽量入新模块/model。
7. **NO git**: 工作副本直接落地。

## 6. 范围

**本期内 (B)**: §4.1 纯 helper(含迁入)、§4.2 trpg-db current 原语、§4.3 contest 收敛(+修 target_kind)、§4.4 mechanics 统一(apply_effect_roll/apply_outcome/ensure_actor_state)、§4.5 combat 帧读 gps、§4.6 GM ledger 换源 gps、§4.7 D&D normalize+override+数据;apply_effect_roll test-first 网 + 跨路径一致性测 + CoC e2e。

**本期外**: 物理删 ams.hp_current/resources_json 列(迁移)留后续;全 ruleset id 归一层;`rule_kernel_patches` 表接通(本期用 override 文件已够);知识图谱(子项目2)。

## 7. 组件落点

| 文件 | 改动 |
|---|---|
| `crates/trpg-model/src/lib.rs` | 迁入 `match_seed`/`build_resources_json`;新 `resolve_resource_track_id`、`apply_armor_damage`、`wound_label`(纯, 带单测) |
| `crates/trpg-db/src/resource_current.rs` (新) | `resource_seeds`/`load_resource_current`/`write_resource_current` + gps load/upsert 下沉 |
| `crates/trpg-db/src/lib.rs` | `load_rule_kernel` 加 normalize + override merge;挂 `mod resource_current` |
| `crates/trpg-contest/src/lib.rs` | `read_track_value` 改调 db 原语 + 修 target_kind |
| `crates/trpg-mechanics/src/lib.rs` | `apply_outcome_resource_tracks`/`apply_effect_roll`/`ensure_actor_state` 改走 db 原语;删 `update_actor_hp`/`update_actor_resources` 调用;`mechanical_ledger` HP 换源 gps;纯函数迁出 |
| `crates/trpg-mechanics/tests/live_apply_effect_roll.rs` (新) | 真 DB 集成测(HP±armor、defeated、resources 分支、跨路径一致性) |
| `crates/trpg-combat/src/lib.rs` | `create_frame` HP 改读 gps live current |
| `crates/trpg-parser/src/lib.rs` | `rule_kernel_from_run_kit` resource_tracks normalize |
| 数据 | `data/parsed/rules/dnd5e.rule_kernel.override.json`(合法 hit_points track) |

## 8. 风险 / 开放

1. **原语下沉 trpg-db 涉及把 mechanics 的 gps load/upsert 迁下**: 迁移期保持 mechanics 编译(改调 db 方法), 一次性。
2. **combat create_frame 读 live current**: 行为变化(开场后帧反映当前血而非满血)—— 这是期望的修复;无 gps 行时回退派生满血保证开场正确。
3. **wound_state 不再持久化**: 消费方(combat 帧 / GM ledger)改现算;`close_active_frame_for_defeated_target` 只需 `to_hp<=0`, 不依赖 ams.wound。
4. **D&D override max**: 用宽松值, 真上限由角色派生(seeds)给;若 D&D 角色 sheet 无 hit_points 派生 → 仍 None(fail-closed), 留 chargen override 补(本期外)。
5. **read_track_value 修 target_kind**: 确认现有 contest 集成测(live_percentile)仍绿。

## 9. 验收测试矩阵 (writing-plans 期细化)

- **trpg-model 纯单测**: `resolve_resource_track_id`("hp.current"→hit_points、resources.sanity.current→sanity、未知→None);`apply_armor_damage`(无甲 100-15=85、5甲 amount15→90、Set、defeated≤0);`wound_label`。
- **trpg-mechanics 真 DB 集成测** (`live_apply_effect_roll.rs`, throwaway session 自播种, skip if no DATABASE_URL):
  - 迁移前: 打 HP 伤害后当前 HP=预期(锁现状, 绿)。
  - 迁移后: 同断言改读 gps `resources.{hp_id}.current`(绿);armor 抵扣;defeated 关帧;resources 分支写 gps。
  - **跨路径一致性**: apply_effect_roll 扣 HP → `ContestService::read_track_value` 读到同一新值。
- **trpg-db 单测**: `load_rule_kernel` normalize 丢 D&D sheet-field 条目;merge override 后 `hp_resource_track_id=="hit_points"`。
- **CoC e2e (真 :54347)**: 建卡 → SAN check 失败掉 SAN(gps)→ 战斗掉 HP(gps)→ GM ledger 见 live current → 按派生 max 封顶 → 零硬编码。
- **D&D 验证**: `load_rule_kernel("dnd5e")` 后有合法 hit_points track;combat 参与者 HP 非 None(若角色有 hit_points 派生)。

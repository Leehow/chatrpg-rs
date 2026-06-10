# 资源接驳·第一刀 (Slice A) — 派生资源 → runtime current + kernel-id 驱动 + 杀硬编码

> 日期: 2026-06-09 · 状态: 设计已批准, 待 writing-plans · 主线: v1.20-formula
> 前置: Track 层 (`2026-06-09-character-growth-tracks-design.md`) —— sheet_json.resources 已随成长刷新。

## 1. 背景 (理解阶段勘查结论)

角色资源当前散在**三套存储 + 命名不齐 + 一处硬编码**:
- `sheet_json.resources.{id}` —— chargen 派生的 **MAX/起始值**(hp=12、sanity=POW),裸数字, Track 层已让它随成长刷新。
- `generic_parameter_states` 路径 `resources.{id}.current` —— 检定资源轨**当前值**(`apply_outcome_resource_tracks` 写、`read_track_value` 读, 读写一致、按 kernel id 驱动 —— 这条是**健康**的)。
- `actor_mechanical_states`(hp_current / resources_json) —— 战斗/effect 工作态;`resources_json` **硬编码** `{"sanity":50/99,"chaos":…,"harm":…}`(mechanics:616), hp seed 自**没人填的** `status_json.hp_max`(combat 创建的 PC HP=None)。

`kernel.resource_tracks` 是资源的**语义事实源**(id/kind/max/initial/on_outcome/thresholds), 但 current 的 seed 用 kernel 静态 `initial`、封顶用 kernel 静态 `max` —— **都不是角色真实派生值**。`mechanics` 0 单元测试、`combat` 8 个薄测试 —— 所以**不做大手术(退役/合并存储 = slice B)**, 只做接驳 + 杀硬编码 + 补测试。

## 2. 目标 / 验收 (端到端, 真实 CoC 角色)

CoC 建卡 → 资源当前值从**角色真实派生值**起(SAN=POW, HP=floor((CON+SIZ)/10)) → SAN check 失败掉理智 / 战斗掉血 → 按**角色真实上限**封顶 → 全程**零硬编码、按 kernel id 驱动**; 给 mechanics/combat 补第一批测试。

## 3. 核心原则 (复用已有数据, 不新增存储)

- `kernel.resource_tracks` = 资源的**唯一语义注册表**(id / kind / max / on_outcome)。
- 角色 `sheet_json.resources.{id}` = 该资源的**真实上限/起始值**(派生, Track 已保鲜)。
- runtime current(generic_parameter_states / actor_mechanical_states) = 游戏中消耗值, **初始/封顶都从角色派生取**。

## 4. 设计

### 4.1 通用桥接 helper (新, mechanics)
`actor_resource_seeds(session_id, actor_id, kernel) -> Map<track_id, {current, max}>`:
- load 角色 `runtime_actor_parameters.sheet_json.resources`(裸 `{id: int}`)。
- 对 `kernel.resource_tracks` 每条(有合法 id 的): 按 **id 大小写不敏感**匹配 `sheet.resources` →
  - `max` = 匹配到的角色派生值 (回退 kernel 静态 `track.max`)。
  - `current` = 同上(派生起始值即当前起点; 回退 kernel `initial`)。
  - 匹配不到 → 该 track **不产 seed**(fail-closed, 不捏造; 沿用 kernel initial/max 旧行为)。
- 零 per-ruleset 分支; 纯按 kernel tracks × 角色 sheet 数据。

### 4.2 on_outcome seed/cap 改从派生取 (`apply_outcome_resource_tracks`, mechanics:145-148)
- `before`(无行回退)从 §4.1 的 `current` 取(替代 line 146 的 `.or(initial)`), 仍回退 kernel `initial`。
- 封顶(line 148)用 §4.1 的 `max`(替代 kernel 静态 `max`), 回退 kernel `track.max`。
- helper 结果在函数开头 load 一次(复用, 不每 track 查库)。

### 4.3 杀硬编码 + HP seed (`ensure_actor_state`, mechanics:604/616)
- line 616 的硬编码 `resources_json` → 从 §4.1 构建: 每 kernel track → `resources_json[id] = {"current": seed.current, "max": seed.max}`; 无 seed 的 track 不写(fail-closed)。**删掉 `{"sanity":50/99,…}`**。
- `default_hp`(line 604)→ 从 §4.1 里 **kernel 的 HP track**(见 §4.4 找法)取 `current`, 替代 `actor_hp_from_params`(读没人填的 status_json.hp_max)。

### 4.4 战斗按 kernel track 找 HP (`combat create_frame`, combat:629)
- 不再硬读 `status_json.get("hp_max")`。改为: 按 ruleset 的 kernel 找 **HP track**(语义: `kind=="health"`, 无则 id 含 "hp"/"hit_point" 的 track —— 仍是读 kernel 数据非硬编码英文词表), 取角色 `sheet.resources[那个 track.id]`(经 §4.1)。
- 找不到 HP track / 角色无该派生 → `None`(fail-closed, 旧行为)。

### 4.5 id 对齐 (数据层, 非 Rust 别名表)
seed 按 **kernel track id** 匹配角色派生资源。CoC `sanity` 已对上 ✓。`hit_points`(kernel) vs `hp`(chargen) **对不上** → 在**数据层对齐**: CoC chargen override (`call_of_cthulhu_7e.chargen.override.json`) 让 HP 派生用 kernel 的 track id。**绝不在 Rust 写 hp↔hit_points 关键词别名表**。

## 5. 不变量 / 护栏

1. **零 per-ruleset 硬编码**: 删 `{"sanity":50/99,…}`; helper/HP-find 纯读 kernel tracks + 角色 sheet。无 `if ruleset`。
2. **fail-closed**: 角色无派生 / kernel 无 track → 不 seed、不捏造默认值(沿用旧 kernel-static 或 None)。
3. **kernel = 资源语义事实源**: id/kind/max 全从 kernel.resource_tracks 来。
4. **单一真相不恶化**: 不新增存储; current 仍在原两套(generic_parameter_states / actor_mechanical_states), 但**初始/封顶来源统一为角色派生 + kernel**, 起点一致。
5. **test-first**: 每处改动先写测试(mechanics 现 0); 含"删硬编码后默认值仍正确""seed 从派生取""封顶用派生 max""按 kind 找 HP"。
6. 文件 ≤ ~400 行。

## 6. 范围

**本期内 (A)**: §4.1 helper、§4.2 seed/cap、§4.3 杀硬编码+HP seed、§4.4 战斗按 kernel 找 HP、§4.5 CoC id 对齐(数据)、mechanics/combat 首批测试、CoC 端到端验收。

**本期外 (slice B)**: 退役/合并两套 current 存储(generic_parameter_states ↔ actor_mechanical_states 物理合一)、全 ruleset id 归一层、GM 上下文投影 current 资源、D&D5e kernel resource_tracks 脏数据修复。

## 7. 组件落点

| 文件 | 改动 |
|---|---|
| `crates/trpg-mechanics/src/lib.rs` | 新 `actor_resource_seeds` helper + HP-track 找法; `apply_outcome_resource_tracks` seed/cap 改派生; `ensure_actor_state` 删硬编码 resources_json + HP seed 改派生; 首批 #[test] |
| `crates/trpg-combat/src/lib.rs` | `create_frame` HP 改按 kernel HP track 读角色派生; 测试 |
| 数据 | `<coc_data_dir>/parsed/characters/call_of_cthulhu_7e.chargen.override.json` —— HP 派生用 kernel track id (hit_points) |

## 8. 风险 / 开放

1. **mechanics 0 测试** → 严格 test-first, 边改边补, 既验证又攒 slice B 的安全网。
2. **HP track 语义识别**: 先 `kind=="health"`, 回退 id 含 hit_point/hp。若某 kernel 两者皆无 → HP=None(fail-closed), 留数据补 kind。
3. **CoC kernel HP track id 确认**: 实现期先查 CoC kernel 真实 track id(可能是 hit_points), 据此写 override。
4. **不碰 contest read_track_value 的写侧**(它已一致); 只改它依赖的 seed 来源(经 §4.2 同一 generic_parameter_states 路径, current 初值由首次 on_outcome seed 决定)。

## 9. 验收测试矩阵 (writing-plans 期细化)

- mechanics 单测: `actor_resource_seeds` 按 id 匹配派生(sanity=65)、无匹配不 seed; `apply_outcome_resource_tracks` before 从派生 seed(SAN 从 65 起非 0)、封顶用派生 max; `ensure_actor_state` 无硬编码 sanity 50、resources_json 从 kernel+派生建。
- combat 单测: `create_frame` HP 从 kernel health track 的角色派生取(非 status_json.hp_max)。
- CoC 端到端(真实 DB): 建卡 → sanity 当前从 POW 起 → 失败 SAN check → sanity 减、按派生 max 封顶; 战斗参与者 HP = 角色派生 hit_points(非 None / 非硬编码)。

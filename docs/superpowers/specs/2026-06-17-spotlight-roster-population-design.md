# Spotlight 名册填充设计 (2026-06-17)

## 目标

`crates/trpg-director` 的 spotlight tracker (`build_spotlight`) 已支持多人名册
(`DirectorInput.participants: &[SpotlightParticipant]`)，跨回合追踪每位玩家的
`spotlight_count` / `last_spotlight_turn`，并在 `participants` 为空时从
`request.viewer` 派生单个 solo participant。

但运行时调用方 `crates/trpg-runtime/src/lib.rs` 的 `prepare_actionable_situation`
**恒传 `participants: &[]`**，所以多人路径从未被真实数据驱动。本设计把
`participants` 改为由会话真实玩家角色派生，让多人 spotlight 轮转 + personal-hook
浮现在**有数据时**被真正执行，solo 自动退化，零回归。

## 关键前提（必须知晓）

**`prepare_actionable_situation` 当前是孤儿方法**：全树唯一构造 `DirectorInput` /
调 `director.prepare` 的就是它，而它没有任何 caller；`CANONICAL_TURN_PLAN`(15 phase)
里也没有 actionable-situation / director / spotlight 相。因此 spotlight tracker
当前并未被真实游戏回合驱动。

→ 本任务**只**把名册接到这个方法的入参上（自洽、正确、可单测）。真正"通过真实回合
端到端跑通"需要把这一相接进 turn plan，那是独立的、更大的产品决策，**不在本任务范围
内**。本任务验收口径 = `cargo test -p trpg-director` + 全量 `cargo build`（+ 新增
模块单测），不做 live e2e（方法走不到，跑了无意义）。

## 名册来源（已拍板）

复用 `runtime_actor_parameters` 表中 `actor_kind='player_character'` 的行。理由：

- 系统**没有任何** session→PC 名册：`characters` 表无 `session_id`、`sessions` 表
  无角色列、无 `players` 表、无"加入会话"流程。
- `runtime_actor_parameters`(session_id, actor_id, actor_kind, display_name,
  sheet_json) 是事实上的"每会话 actor 面"，每回合 `ensure_actor_parameters` 出一行
  `pc.current`(kind=player_character)。这是单一事实源。
- 零迁移；一旦出现第二个 PC 行就天然走多人路径；solo 退化为单条。

被否决的替代方案：新建 `session_participants` junction 表 + 迁移 + 写入流程——solo
产品无加入入口 → 表恒空 → 实际仍只跑 solo，YAGNI。

## 组件

### 1. DB 查询（`trpg-params::RuntimeParameterService`）

```rust
pub async fn list_player_actor_parameters(&self, session_id: &str)
    -> Result<Vec<RuntimeActorParameters>>
```

`select <同 load_actor_parameters 的列> from runtime_actor_parameters
where session_id=$1 and actor_kind='player_character'
order by created_at asc, actor_id asc`（顺序确定性）。复用现有 `row_to_params`。
放在 trpg-params 而非巨大的 trpg-db，复用 `row_to_params` 且贴近 actor-参数域。

### 2. 纯函数模块 `crates/trpg-runtime/src/spotlight_roster.rs`（新文件，≤120 行）

纯逻辑、不碰 DB、可单测。

```rust
pub(crate) fn build_spotlight_participants(
    players: &[RuntimeActorParameters],
    acting_actor_id: &str,
) -> Vec<SpotlightParticipant>
```

每行映射：

- `player_id = character_id = actor_id`
  （solo 产品无独立玩家身份；与现有 viewer 兜底语义一致：solo fallback 也用
  player_id = viewer.player_id **OR** actor_id，character_id = actor_id）。
- `acted_this_turn = (actor_id == acting_actor_id)`。
- strengths / playstyle / hook 由下方提取器从 `sheet_json` 取。

```rust
fn extract_spotlight_fields(sheet_json: &Value)
    -> (Vec<String>, Vec<String>, Option<String>)
```

只读一个**约定的、跨规则集通用**的可选子桶：

- `sheet_json.spotlight.character_strengths` : `[String]`
- `sheet_json.spotlight.preferred_playstyle` : `[String]`
- `sheet_json.spotlight.pending_personal_hook` : `String`

缺失 / 类型不符 → 空 vec / None（fail-soft）。**无任何 per-ruleset 硬编码**。将来
chargen / GM 想填就填进这个桶；tracker 的 carry-forward (`pick_vec` / `or_else`)
会让一旦学到的值跨回合保留，即便后续某回合的行未再供给。

### 3. 接线（`prepare_actionable_situation` 内，约 +6 行）

- `let acting_actor_id = request.viewer.actor_id.as_deref().unwrap_or("pc.current");`
  （沿用 runtime 全局事实约定：lib.rs 712/777/800/823/1561 同款兜底。意味着 viewer
  恒为 gm() 时，acting PC 仍能被正确记一次 spotlight。）
- `let players = RuntimeParameterService::new(self.db.clone())
     .list_player_actor_parameters(&request.session_id).await.unwrap_or_default();`
- `let participants = spotlight_roster::build_spotlight_participants(&players, acting_actor_id);`
- `DirectorInput { …, participants: &participants, … }`

**fail-soft**：名册空（读失败 / 首回合 ensure 之前）→ `participants` 为空 vec ≡
`&[]` → director 仍走 viewer 兜底 → 零回归。

## 数据流

```
runtime_actor_parameters (kind=player_character rows)
   └─ list_player_actor_parameters(session_id)            [trpg-params, DB]
        └─ build_spotlight_participants(&players, acting)  [spotlight_roster, 纯]
             └─ DirectorInput.participants                 [trpg-runtime 接线]
                  └─ build_spotlight → Vec<SpotlightState> [trpg-director, 不变]
                       └─ upsert_spotlight_state           [既有持久化]
```

## 错误处理

- DB 读失败 → `unwrap_or_default()` → 空名册 → viewer 兜底（零回归）。
- sheet_json 无 spotlight 桶 / 类型错 → 空/None（fail-soft，carry-forward 兜底）。
- acting_actor_id 不在名册（自定义 viewer.actor_id 无参数行）→ 无人被记一次
  （count 全 carry，无人 +1）——良性。

## 测试（TDD）

`spotlight_roster.rs` 单测（纯函数，无需 DB）：

1. 两 PC 行 + acting = 其一 → 2 participants，只有 acting 的 `acted_this_turn=true`，
   id 由 actor_id 映射。
2. sheet_json 带 spotlight 桶 → 三字段被提取；无桶 → 空/None。
3. acting id 不在名册 → 无人 `acted_this_turn`。
4. 空输入 → 空名册。

回归 / 验收：

- `cargo test -p trpg-director` 全绿（director 逻辑不变；多人 build_spotlight 已有覆盖）。
- `cargo test -p trpg-runtime`（新模块 + 无回归）。
- `cargo test -p trpg-params`（编译 / 既有不破；DB 查询需 live PG 才能跑，故纯逻辑覆盖
  集中在 runtime 的纯函数单测）。
- 全量 `cargo build`。

## 文件规模

- 新 `spotlight_roster.rs` ≤120 行。
- `trpg-runtime/src/lib.rs` +约6 行、`trpg-params/src/lib.rs` +约12 行
  （两 lib.rs 本就远超 400 行系历史遗留，不在本任务重构范围）。

## 工作区约束

当前主 checkout 是多人在途的共享 dirty 树（分支 `claude/p0-3-semantic-need-classifier`，
spotlight 脚手架本身未提交）。本任务**就地纯增量实现、不提交、绝不碰其它在途文件**，
结束时给出改动文件清单。

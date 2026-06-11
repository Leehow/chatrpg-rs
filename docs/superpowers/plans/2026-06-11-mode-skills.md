# Mode Skills 三期 Implementation Plan（工艺 v3：分批+风险分级）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** 姿态框架（提示四级合并/工具 mode 组装/进出双通道+退出结算义务/节拍参数/目录过滤注入）+ 战斗 skill（电影化交锋）+ 幕间 skill（提案拍板流），全测试交付。
**Architecture:** spec `2026-06-11-mode-skills-design.md` 为权威。skill=姿态：同一 GmLoop，mode 由 active state_frame 推导；全 data-driven（mode 包=manifest+提示文件）；二期债务/目录/钩子体系原样复用只调参数。
**工艺 v3:** 按文件域分批，一 agent 连做一批；TDD 自证+每批一次 sonnet 抽查审+收官 Fable 终审；批1/2=Fable（新颖核心），批3/5=sonnet（模式跟随），批4/6=sonnet 操作员+relay 玩家模拟。**无 git 提交步骤由 agent 做**（主会话统一 commit）；文件 ≤400 行；零 per-ruleset 硬编码；`#[serde(default)]`。

## 新类型契约（仅新增项；既有类型逐字复用二期）

```rust
// trpg-gm/src/mode.rs（新）
pub struct ModeManifest {            // data/agent/gm_skill/modes/<mode>/manifest.json
    pub mode_id: String,             // "combat" | "downtime"（文件夹名一致，开放枚举）
    pub frame_kind: String,          // 映射 FrameKind 字符串（"combat"/"downtime"——trpg-model FrameKind 增 Downtime 变体）
    pub extra_tools: Vec<String>,    // mode 专属工具名（registry 按名组装，未知名 fail-closed 报配置错误）
    pub catalog_filter: CatalogFilter, // 目录注入过滤器
    pub exit_obligations: Vec<String>, // 退出结算义务描述（挂 ObligationLedger 的 mode_exit 债务）
    pub tempo: TempoOverrides,       // 节拍参数
}
#[derive(Default)] pub struct CatalogFilter { pub kinds: Vec<String>, pub hooks: Vec<String>, pub semantic_tags: Vec<String> } // 任一匹配即注入；全空=不过滤
#[derive(Default)] pub struct TempoOverrides { pub max_tool_rounds: Option<u8>, pub effect_closure_per_cluster: Option<bool> } // None=沿用默认
pub fn current_mode(frames: &[StateFrame]) -> Option<String>          // active frame → mode_id（无=叙事姿态）
pub fn load_mode_manifest(data_dir: &Path, mode: &str) -> Result<ModeManifest> // fail-closed：不存在/解析失败 Err
// load_gm_skill 升级：四级合并 global → ruleset → mode-global → mode-ruleset（mode None 时退化为现状两级，字节不变）
pub fn load_gm_skill_with_mode(data_dir: &Path, ruleset: &str, mode: Option<&str>) -> Result<String>
// ToolRegistry::for_mode(data_dir, mode: Option<&str>) -> Result<ToolRegistry>  // 基础14 + manifest.extra_tools
// 工具 enter_mode {mode, reason} / exit_mode {reason}：见 spec §4.4；嵌套（已在 mode 内 enter 另一 mode）→ mode_nesting_unsupported；
//   exit 前 ObligationLedger.blocking() 非空 → exit_blocked_by_obligations（waive 通道照常）
// trpg-gm/src/tools/frame.rs（新，战斗）
// open_combat_frame {participants: Vec<String>, stakes: String} → StateFrame(kind=Combat) + frame BP3 投影注册
// close_frame {summary} → 触发 compaction（复用 trpg-runtime/conflict 既有压缩原语）+ frame 关闭
```

## 分批任务

### 批 1（Fable）：姿态框架核心
**Files:** Create `crates/trpg-gm/src/mode.rs`、`data/agent/gm_skill/modes/combat/manifest.json`、`modes/downtime/manifest.json`（最小骨架，批2/3 填内容）；Modify `crates/trpg-model/src/lib.rs`（FrameKind 增 Downtime，serde default 兼容）、`crates/trpg-gm/src/prompts.rs`（四级合并）、`tools/mod.rs`（for_mode + enter/exit_mode 注册，基础 14）、`turn_loop.rs`（回合头部 current_mode 推导→manifest 加载→for_mode 工具→tempo 覆盖 LoopConfig→mode 提示层传入 assemble）、`obligations.rs`（mode_exit 债务种类）。
- [ ] TDD：mode.rs 单测——manifest 加载 fail-closed、current_mode 推导（无 frame/Combat frame/Downtime frame 三态）、四级合并顺序与 mode=None 字节回归（**一期缓存稳定测试照绿是硬验收**）、for_mode 未知工具名报错、enter/exit 嵌套拒绝、exit 义务拦截+waive 放行。
- [ ] 实现后：`cargo test -p trpg-gm -p trpg-model` 全绿；`wc -l` 全部 ≤400；mode=None 路径与二期行为字节级一致（关键回归）。

### 批 2（Fable）：战斗 skill
**Files:** Create `crates/trpg-gm/src/tools/frame.rs`、`data/agent/gm_skill/modes/combat/global.md`（电影化准则：不报轮次表/交锋簇节拍/反应窗裁量）、`modes/combat/dnd5e.md` + `modes/combat/call_of_cthulhu_7e.md`（先攻序知识引用目录条目，非硬编码规则文本）；Modify `modes/combat/manifest.json`（extra_tools=[open_combat_frame,close_frame]、catalog_filter（战斗类 kind/tag）、tempo.effect_closure_per_cluster=true、exit_obligations）。
- [ ] TDD：frame 工具单测（open 建 Combat frame+投影块、close 触发压缩调用、参与者进 BP3）；交锋簇债务收紧测试（cluster 未闭合时 content 终态被债务观察拦截——复用 B6 测试模式）；novelty 注入测试（frame 状态含已用战术→提示块出现）。
- [ ] 验证：`cargo test -p trpg-gm`；战斗准则文件无规则集名于逻辑（文案文件按 ruleset 分文件即为数据化）。

### 批 3（sonnet）：幕间 skill
**Files:** Create `data/agent/gm_skill/modes/downtime/global.md`（提案流准则：盘点→过滤目录幕间机制→散文提案→拍板→蒙太奇+大步进+批量结算→结算表 [system] 交付）+ `modes/downtime/manifest.json` 填全（catalog_filter=A5 类 hooks/kinds、exit_obligations=[结算表落账]、tempo 放宽）。
- [ ] 单测：downtime manifest 加载、目录过滤器选中 development_phase/calendar 类条目（合成目录夹具）、exit 义务文案出现于债务清单。
- [ ] 验证：`cargo test -p trpg-gm`。

### 批 4（sonnet）：测试基建（relay 玩家模拟器 + 脚本断言）
**Files:** Create `harness/relay_player_sim.sh`（读场景规格 JSON：每回合玩家输入或"由 relay 生成"标记→curl relay 生成→喂 `trpg play --agent` stdin；行级时间戳）、`harness/e2e_assert.sh`（SQL 断言库：按规格 JSON 列的查询+期望执行，输出 PASS/FAIL 清单）、`harness/specs/combat_dnd.json`、`combat_coc.json`、`downtime_coc.json`（场景规格：预绑卡步骤、输入序列、断言集、重跑条件）。
- [ ] 验证：specs 干跑（DB 不可用时报清晰错误）；脚本 shellcheck 干净；**Claude 额度零消耗设计**（驱动全程本地+relay）。

### 批 5（sonnet）：框架回归与基线
**Files:** Modify trpg-gm 缓存稳定测试（mode 维度参数化：mode 切换=有因失效断言、同 mode 内前缀字节稳定）；跨期回归脚本（一期+二期全测试套件清单）。
- [ ] 验证：`cargo test -p trpg-gm -p trpg-model -p trpg-mechanics -p trpg-agent -p trpg-rule-agent` 全绿。

### 批 6（sonnet 操作员 + relay sim）：e2e 与对账
- [ ] 战斗 e2e：批4 驱动器跑 `combat_dnd.json`（或 CPR）完整一场——先攻序符合规则集程序（账本顺序 SQL）、反应窗 ≥1 次暂停/恢复、伤害全落库、**叙事零轮次报表用语**（grep 断言+人工抽读）、close 后压缩生效。
- [ ] 幕间 e2e：`downtime_coc.json`——发展阶段触发→提案含 ≥3 类目录机制→拍板→批量结算 SQL 可查（成长勾/理智恢复/财务事件）→大步进+calendar 钩子全结算。
- [ ] spec §8 对账表逐条填写（含批1 的 mode=None 回归证据），写入本文件末尾执行记录。
- [ ] 终审（Fable）：整体对照 spec、跨批接缝、全套件真跑、对账表抽查。

## 验收映射（spec §8 ↔ 批）
框架单测①-⑤=批1；战斗 e2e=批2+6；幕间 e2e=批3+6；跨期回归=批5；缓存基线=批5；产品评测=收官另行（chatrpg-product-evaluator）。

## 执行记录

### 2026-06-11 批1 审查修复（越界备案）
- **批4 文件越界已回收**：批1 agent 预先创建了 `harness/relay_player_sim.sh`、`harness/e2e_assert.sh`、`harness/specs/{combat_coc,combat_dnd,downtime_coc}.json`（批4 声明范围）。已从工作区移除（备份于 `/tmp/batch1-overreach-backup-20260611/`），批4 按计划自行 TDD 交付，不得参考该备份跳过先红后绿。
- **trpg-rule-agent 越界改动备案（保留在位）**：`crates/trpg-rule-agent/src/reader/agent.rs` + `reader/tools.rs` 的脏改动经 git 取证确认为批1 agent 同会话越界（mtime 11:35 与 mode.rs/prompts.rs 交错），但内容归属 **on_outcome =field 引用守卫线（commit 347ae81）的收尾**——把 AMOUNT_RESOLVABLE 词汇表教给 reader SYSTEM 提示与 on_outcome schema 文档（杜绝 `=damage_after_armor` 类死引用再生产）+ 两个防漂移守卫测试。本计划无任何批次声明 trpg-rule-agent 文件，"移至正确批次"不可行；改动功能正确（守卫测试在位、`cargo test -p trpg-rule-agent` 全绿），故按审查备选项就地备案。**主会话 commit 时请将这两个文件与 mode-skills 批次分开、单独作为 on_outcome 线的后续提交**。同窗口产生的未跟踪 `_fix_dead_on_outcome_refs.sql`（存量数据修复，对应已留 chip 的线）同属 on_outcome 线，未动、一并备案。

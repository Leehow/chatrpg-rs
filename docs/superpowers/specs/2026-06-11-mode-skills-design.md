# Mode Skills 设计（三期：姿态框架 + 战斗 skill + 幕间 skill）

日期：2026-06-11
状态：设计已获用户确认（架构模型/范围/战斗手感/幕间手感四项拍板）
前置：一期 agent loop（`2026-06-10-gm-agent-loop-design.md`，已落地）+ 二期规则感知 GM（`2026-06-10-rule-aware-gm-design.md`，执行中）。本期为加法，一二期代码零返工。
实施排期：二期执行 + 产品评测收官之后。

## 1. 背景与候选盘点

用户三 skill 愿景（建卡/解析/游戏）在一期落地为 agent loop 后，"游戏 skill"内部仍是单一姿态。七书勘查（二期附录 A，137 条）显示多个子系统具有**独立程序与节奏**，值得姿态化。skill 化判定标准：①程序/节奏不同于普通叙事回合 ②跨回合持有状态 ③跨规则集普遍 ④进入后工具与知识集整体切换。

盘点结论（按价值排序）：**战斗**（A6 全书皆有+一期 conflict kernel 资产闲置）、**幕间/休整**（A5 周期节拍器五书全有：CoC 发展阶段 9 机制汇聚、CPR Hustle/月结、D&D 休整期、SW session 终局、ORC 经验勾选）、旅程/探索（D&D 步调/补给、SW 宿营——sandbox 模组刚需）、疯狂/精神演出（CoC 发作两层叙事、CPR 赛博精神病——LLM-GM 差异化亮点）、追逐、社交对抗、仪式/研究。**三期做框架+战斗+幕间；旅程/疯狂/追逐/社交归四期**，每个仅为"数据包+工具组"增量。

## 2. 已拍板决策

| 决策点 | 结论 |
|---|---|
| 架构模型 | **skill = 姿态（posture）**：同一 GmLoop，切换提示覆盖层+工具子集+节拍参数+进出语义，全 data-driven；非独立子循环 |
| 三期范围 | 框架 + 战斗 + 幕间（用户加码：不止首例） |
| 战斗手感 | **电影化交锋**：机械层（先攻/轮次/反应）严格按目录解析的本规则集程序执行；呈现层不报轮次表、敌方行动织进散文 |
| 幕间手感 | **GM 提案 + 玩家拍板**：语义汇总可做事项+建议方案 → 玩家调整确认 → 蒙太奇演绎+批量结算 |
| 反应窗 | agent 裁量（致命/关键时刻开 gate 问玩家，杂兵交锋直接结算）——沿用"掷骰 agent 自主"哲学 |

## 3. 目标 / 非目标

**目标**：姿态机制本体（进出/状态/提示/工具/节拍五件套）；战斗姿态在 ≥2 套规则（严格先攻型 D&D + 叙事型 CoC）上电影化可玩且机械全闭环；幕间姿态把二期 calendar/development_phase 钩子变成可玩的提案-拍板-结算流；零 per-ruleset 硬编码；一期缓存稳定原则延续（mode 切换列为有因失效）。

**非目标（三期）**：旅程/疯狂/追逐/社交姿态（四期数据包）；多人 PVP；旧 conflict kernel 删除（继续服务旧路径 A/B）；mode 嵌套（战斗中进幕间之类——栈深 1，嵌套诉求 fail-closed 拒绝并提示）。

## 4. 姿态框架

### 4.1 Mode 状态（复用 state_frames）
姿态 = active state_frame 的投影。一期 conflict kernel 已有 FrameKind::Combat/Chase；新增 FrameKind::Downtime（`#[serde(default)]` 兼容）。当前姿态由 `list_active_state_frames` 推导（无 active frame = 默认叙事姿态）；frame 的 open/close/compaction 生命周期原语全部现成复用。

### 4.2 提示三级合并
`load_gm_skill` 扩展第三级：`data/agent/gm_skill/modes/<mode>/global.md` + `modes/<mode>/<ruleset>.md`（mode 层内部仍按 ruleset 覆盖）。合并顺序 global → ruleset → mode-global → mode-ruleset，文件名字典序规则不变。mode 提示层进入 BP2 级缓存带（随姿态切换变化，与场景切换同级）。

### 4.3 工具按 mode 组装
`ToolRegistry::for_mode(mode)`：基础 14 工具（二期 12 + 本期新增 enter_mode/exit_mode，任何姿态下均可用）+ mode 专属组（数据声明于 mode 包 manifest：`modes/<mode>/manifest.json` 列工具名与目录注入过滤器）。工具 schema 随 mode 变化 → **前缀缓存有因失效**（缓存稳定原则豁免清单追加此项，回归测试基线随 mode 维度参数化）。

### 4.4 进出语义（双通道 + 退出结算义务）
- 新工具 `enter_mode {mode, reason}` / `exit_mode {reason}`：agent 语义判定调用；fail-closed 校验 mode 存在、无嵌套（栈深 1）。
- 引擎建议通道：对抗检定/伤害事件 → MechanicDue 形态建议进战斗；development_phase/calendar 钩子 → 建议进幕间（复用二期 due 通路，agent 决定是否采纳——semantic 优先原则）。
- **退出结算义务**：mode 包声明退出前必须闭合的事项（战斗：所有交锋簇效果落账+frame compaction；幕间：结算表落账）——未闭合挂进 ObligationLedger，exit_mode 被债务门控拦截（waive 通道照常可用）。

### 4.5 目录联动
mode 包 manifest 声明目录注入过滤器（按 MechanicKind/hook 标签/语义类别），进入姿态时 BP 注入的 mechanics_catalog 子集随之切换——战斗姿态注入本规则集战斗程序条目，幕间注入 A5 类周期机制条目。过滤器是数据不是代码。

### 4.6 节拍参数
mode 包声明债务门控姿态参数：战斗=每交锋簇必须效果闭合才许下一簇叙事（收紧）；幕间=批量结算（放宽，结算义务集中在退出点）。LoopConfig per-mode 覆盖（max_tool_rounds 等）同机制。

## 5. 战斗 skill

- **机械层**：先攻/轮次顺序由目录解析的本规则集战斗程序驱动（D&D 先攻掷骰排序、CoC DEX 序、数据缺失则退化为 agent 裁量序并记 validation 缺口）；交锋簇 = 玩家行动 + 若干敌方行动的结算单元，全部经 roll_check/apply_effect 落账，簇未闭合不得进下一簇（§4.6 节拍）。
- **呈现层**：mode 提示明令——不报先攻表/轮次号，敌方行动以戏剧节奏织进散文；交锋簇边界以叙事段落自然呈现。
- **反应窗**：agent 裁量开 gate（既有 request_player_roll 特化：reaction 语境的 stakes 文案），杂兵攻击直接系统结算。
- **NPC 行动**：agent 替敌方调 roll_check（opposed 通道一期已有，NPC 卡值经 npc_synth/get_actor）；**novelty 数据复用**：一期 novelty director 的防重复战术数据变 mode 内提示注入（"该敌人上回合已用过 X 战术"事实），不再是独立 LLM 调用。
- **frame 工具**：`open_combat_frame {participants, stakes}` / `close_frame {summary}`（close 触发 compaction——一期 conflict kernel 的压缩原语复用）；frame 状态（参与者/轮次计数/已用战术）进 BP3 投影。

## 6. 幕间 skill

- **进入**：玩家声明休整/章节自然收束；development_phase 钩子到点发 due 建议。
- **提案流**：进入后 agent ①盘点角色状态（伤势/疯狂/资源/欠债——get_actor + 债务清单）②从目录过滤幕间机制（治疗/训练/Hustle/研究/财务复查…）③产出提案清单+建议方案（散文非表格，符合叙事沉浸）→ 玩家调整拍板（普通对话回合，无新机制）。
- **演绎与结算**：蒙太奇叙事 + `advance_time` 大步进 → 二期 calendar/development_phase 钩子排队触发（月结/周期机制自动结算）→ 逐项程序执行（成长勾经 apply_track_change、理智恢复、财务事件表）→ 结算表作为 [system] 摘要交付 + 记忆落账 → exit_mode。
- **幕间是二期钩子体系的主消费场景**：B5 通路的 calendar 粒度数据化在此兑现（Fate 时间段、SW 每日 6:00、CPR 月结同一机制覆盖）。

## 7. 接缝与退役

- 债务/勘误/缓存监控/私骰防漏：全部一二期机制原样生效，mode 只调参数。
- 旧 conflict kernel：继续服务旧路径；新战斗姿态落地后做新旧对照 playtest（评测 skill），数据说话再谈退役。
- gm_skill 文件、mode manifest、目录过滤器、节拍参数——全部数据文件，新增第四个姿态（四期）零 Rust 改动为目标（仅当需要新工具时写薄包装）。

## 8. 测试与验收

**框架单测**：①三级（四层）提示合并顺序与字节稳定；②for_mode 工具组装与 schema 确定性；③enter/exit fail-closed（不存在的 mode/嵌套拒绝）；④退出结算义务拦截与 waive 放行；⑤节拍参数随 mode 切换且默认姿态行为与二期完全一致（回归）。
**战斗 e2e**（真模组）：D&D 或 CPR 一场完整战斗——先攻顺序符合规则集程序（查账本顺序）、反应窗至少一次暂停/恢复闭环、全部伤害落库、**叙事文本零轮次报表用语**（评测断言）、close_frame 后 compaction 生效。
**幕间 e2e**（CoC）：触发发展阶段 → 提案含 ≥3 类目录机制 → 拍板 → 批量结算落库（成长勾/理智恢复/财务复查 SQL 可查）→ 时间大步进且 calendar 钩子全部结算。
**跨期回归**：一期 50+ 二期全部测试照绿；缓存回归基线按 mode 维度更新。
**收官**：chatrpg-product-evaluator 对战斗/幕间做体感评测（电影化是否成立、提案流节奏）。

## 9. 风险

1. **mode 切换的缓存失效频率**——战斗进出频繁的模组会放大缓存 miss；缓解：mode 提示层置于 BP2 带尾部使失效面最小化，实测命中率进评测报告。
2. **战斗程序解析质量**（先攻规则结构化）——依赖二期目录质量；缺失时 agent 裁量序兜底（fail-closed 可观测）。
3. **交锋簇边界的语义判定**——电影化呈现下"一簇"的切分靠 agent；债务门控保证机械不漏，切分只影响节奏不影响正确性。
4. **退出义务与玩家自由的张力**——玩家中途想跑路：exit 义务通过 waive 通道可豁免（带理由记账），不死锁。

## 10. 四期展望（非本期承诺）

旅程/探索姿态（calendar 钩子+补给结算+遭遇生成）、疯狂/精神演出姿态（控制权移交+妄想/真实两层叙事——LLM 不可靠叙事者）、追逐姿态（location 链生成）、社交对抗姿态（Facedown/态度两层）。每个=mode 数据包+少量工具。

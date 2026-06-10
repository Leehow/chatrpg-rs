## B6. 债务清单门控 + waive_obligation + 追溯债务（loop 心脏改动）

### Files
- Create: `crates/trpg-gm/src/obligations.rs`（契约 §5 全量落地）
- Modify: `crates/trpg-gm/src/tools/mechanic.rs`（B2 已建该文件——增 WaiveObligationTool）
- Modify: `crates/trpg-gm/src/turn_loop.rs:28`（GmLoop 字段）、`:63-148`（工具轮门控）、`:150-170`（verify_after_stream 接追溯债务）
- Modify: `crates/trpg-gm/src/prompts.rs`（DynamicTailInput 增 `obligations_block: Option<String>`，装配进 dynamic tail——排在 errata_blocks 之后、Player Input 之前）
- Modify: `crates/trpg-gm/src/lib.rs`（`pub mod obligations;` + re-export ObligationLedger/WaiveScope）
- Test: 各文件同文件 `#[cfg(test)]`；turn_loop 场景测试进 `turn_loop_tests.rs`

### 接口契约
骨架契约 §5（ObligationLedger / RetroactiveEffectDebt / WaiverRecord / WaiveScope 及全部方法签名）与 §6 的 waive_obligation 工具 schema **逐字生效**，本任务不新增类型。工具注册：`ToolRegistry::standard()` 固定顺序追加 `waive_obligation`（在 B2 的 lookup_mechanic 之后，11→12 工具）。

### 测试清单（测试即规格）
1. `obligations::tests::blocking_view_excludes_settled_and_waived` —— record_open_check + mark_check_settled + waive 后 blocking() 只剩未处理项；空清单 block_text() 返回 None。
2. `obligations::tests::waive_unknown_target_errs` —— waive("不存在的 id") → Err；工具层断言折成 `obligation_not_found` recoverable 错误。
3. `obligations::tests::carryover_block_lists_unresolved_dues_with_evidence` —— 含 threshold_desc 与 evidence 摘要。
4. `turn_loop_tests::open_due_blocks_narration_round`（spec 验收 5）—— MockLlm 脚本：第一轮 content delta（试图叙事终态）但 ObligationLedger 有未处理 due → 断言该轮 messages 被回填 block_text() 的 system 观察、循环继续、content **未**流给 on_delta；第二轮脚本 waive 后 content 正常流出。
5. `turn_loop_tests::waive_emits_audit_and_unblocks` —— waive 带理由 → 勘误记忆 MemoryEvent tags 含 "gm_waive"（MockLlm 路径用 ErrataMemory 断言载荷，不连 DB）+ blocking() 清空放行。
6. `turn_loop_tests::round_exhaustion_carries_debt_to_next_turn` —— 轮耗尽仍有债务 → ToolChoice::None 强制叙事语义不变（一期断言照绿）+ GmLoop.obligations 持久字段含 carryover + 下回合 assemble 的 dynamic tail 含 obligations_block。
7. `turn_loop_tests::invented_effect_becomes_retro_debt`（spec 验收 11 单测侧）—— verify_after_stream 抓到 InventedEffect → absorb_retro_debts → 下回合 blocking() 含该 debt_id。

### 实现要点
- GmLoop 增 `obligations: ObligationLedger` 持久字段（与 errata 同生命周期跨回合存活）；回合头部 `db.list_open_mechanic_dues(session_id)` 装载遗留（B4 已建 db 原语）。
- 工具轮门控插点：现 turn_loop.rs `if !saw_tool`（约 L116，content 终态分支）前置 `obligations.blocking()` 检查——非空则回填 block_text() 为 system 观察并 `continue`（**该轮 content 不流出**：RedactingBuffer 缓冲的增量丢弃，on_delta 不调用——实现上把"流出"延后到 blocking 检查之后再 flush，参考一期混合轮的缓冲重建写法 L112-115）。
- 借用冲突预案（索引原文）：优先"dues 读写走 db + 工具轮结束统一 absorb"形态，避免 ObligationLedger 与 TurnLedger 的 `&mut` 在 dispatch 借用链上打架；若编译器允许直接传 `&mut` 则取直通形态。两形态测试断言相同。
- waive 工具副作用三连：`db.update_mechanic_due_status(due_id, "waived")`（仅 due 类）→ 勘误记忆（复用 errata.to_memory_event 样板，tags=["gm_waive"]）→ `obligations.waive`。scope=scene 的跨回合豁免凭 db status + 场景切换时（navigate_scene 成功路径）重开。
- 零 per-ruleset 硬编码：门控/豁免逻辑只认 id 与状态，不认机制名。

### 验证
Run: `cargo test -p trpg-gm`
Expected: 全绿（含一期 43 个既有测试照绿——缓存稳定测试尤其不得破）；`wc -l crates/trpg-gm/src/obligations.rs crates/trpg-gm/src/turn_loop.rs crates/trpg-gm/src/tools/mechanic.rs` 全部 ≤400。

---

## B7. verifier referenced_ledger_ids 结构化升级（一期遗留清算，护栏 §3.5.5）

### Files
- Modify: `crates/trpg-agent/src/gm_loop.rs:176-283`（NarrationVerifier::verify）
- Modify: `crates/trpg-gm/src/turn_loop.rs:150-170`（verify_after_stream 传账本 id 全集）
- Test: gm_loop.rs 同文件 `#[cfg(test)]`（既有测试区 L468-600 扩展）

### 接口契约
`NarrationVerifier::verify(&self, ledger: &TurnLedgerSnapshot, submission: &FinalNarrationSubmission) -> NarrationVerifierResult` 签名不变。行为升级：
- `submission.referenced_ledger_ids` **非空** ⇒ 结构化核对优先：每个引用 id 必须 ∈ 账本 id 全集（check_contracts 的 check_id ∪ dice_rolls 的 roll_id ∪ effect_contracts 的 effect_id ∪ parameter_impacts 的 impact_id）；引用了不存在的 id → `InventedEffect` finding（detail 含该 id）。
- 引用为**空** ⇒ 回退现有子串扫描，且所有回退产生的 finding 的 detail 加前缀 `fallback:substring_scan: `（可观测技术债，不静默——护栏 §3.5.1 同款思想）。

### 测试清单
1. `verifier_structured_refs_accept_known_ids` —— 引用账本真实 id 全集 → accepted（即使叙事文本不含可见 token，结构化引用优先于 OmittedVisibleResult 子串检查？**否**——OmittedVisibleResult 语义保留：可见结果 token 检查独立运行；本测试构造 token 在文本中存在的用例）。
2. `verifier_structured_refs_reject_unknown_id` —— 引用 "check_不存在" → InventedEffect finding 含该 id。
3. `verifier_empty_refs_falls_back_with_marker` —— 引用为空 + 文本声称伤害无账本证据 → finding detail 以 `fallback:substring_scan: ` 开头。
4. 既有 gm_loop.rs 全部测试照绿（含 L468-600 区 referenced_ledger_ids 既有用例）。

### 实现要点
- id 全集收集为 gm_loop.rs 私有 helper `ledger_id_set(&TurnLedgerSnapshot) -> HashSet<String>`（≤15 行）。
- turn_loop.rs 的 verify_after_stream：`referenced_ledger_ids` 从 `vec![]` 改为 `ledger_id_set` 物化的 Vec（"已落账事实全集"语义——agent 不显式声明引用，引擎代填全集，结构化核对退化为"声称的 id 必在账本"恒真 + 子串回退被关闭 → 实际效果是 fallback 标注路径只在账本为空时出现）。注释写明此语义决策。
- 不动 FinalNarrationSubmission 结构（字段已存在），向后兼容。

### 验证
Run: `cargo test -p trpg-agent -p trpg-gm`
Expected: 全绿；gm_loop.rs ≤400 行（现 740 行——**豁免**：一期遗留文件本任务只做增量，不强拆；新增 helper ≤15 行）。

---

## B8. gm_skill 准则两条 + Slice B 收口

### Files
- Create: `data/agent/gm_skill/global/40_mechanics_catalog.md`
- Create: `data/agent/gm_skill/global/50_obligation_policy.md`
- Test: `crates/trpg-gm/src/prompts.rs` 同文件 `#[cfg(test)]` 补一条合并顺序断言

### 接口契约（数据文件内容要点——执行时成文，不是占位）
- `40_mechanics_catalog.md`：①目录优先于自由发挥（流程：看 BP1 索引 → 需要细节 lookup_mechanic → 开检定带 mechanic_id 继承绑定；目录没有的机制按语义裁量并考虑 retrieve_rules）；②**行为语义监听条款**：按本 ruleset 目录中 kind=spend/其它元资源条目的 when_to_use，持续评估玩家言行并发放/扣减对应元资源（Triangle 嘉奖/记过、Fate FP 双实证——条款引用目录条目而非写死机制名，零硬编码）。
- `50_obligation_policy.md`：①due 必须回应——处理（roll_check/request_player_roll）或 waive_obligation 带理由；②追溯债务=上回合叙事声称但未落账的效果——补 apply_effect 或 waive（理由如"叙事中已收回"）；③waive 是裁量权不是逃生舱：高频 waive 同类 due 会进勘误统计。
- grounded：load_gm_skill 按文件名字典序合并（prompts.rs L85-94），40_/50_ 排在 30_output_contract.md 之后**追加**，既有三份文件零字节变动（缓存稳定：BP1 文本变化属"规则变更"级有因变化，跨回合 hash 基线在本任务后重置一次，缓存回归测试同步更新基线注释）。

### 测试清单
1. `prompts::tests::gm_skill_merge_order_includes_new_entries` —— 合并文本中 40_ 内容出现在 30_ 之后、50_ 在 40_ 之后。
2. Slice B 收口矩阵：`cargo test -p trpg-gm -p trpg-agent -p trpg-mechanics` 全绿（spec 验收 4/5/6 对应单测全数通过）。

### 实现要点
- 两份数据文件为纯 prose 准则（中文），不含任何代码/规则集名字面量。
- 收口冒烟（真库手动步骤，写入执行记录）：`TRPG_LLM_MODEL=gpt-5.5 cargo run -p trpg-cli --bin trpg -- play --ruleset call_of_cthulhu_7e --agent` 跑 1 回合，确认：BP1 含目录索引段、lookup_mechanic 在工具表中、无债务回合行为与一期无差异（叙事正常流出）。

### 验证
Run: `cargo test -p trpg-gm -p trpg-agent -p trpg-mechanics && ls data/agent/gm_skill/global/`
Expected: 全绿；global/ 下六份文件（10/15/20/30 + 新 40/50——15 为一期收尾新增的 context_projection）。

# 设计：Policy Plugin Host v1（统一 PromptBlock/ContextFilter/VerifierFinding 契约 + host，内置 no_spoiler / no_mechanical_invention）

日期：2026-06-17
状态：设计（自主执行，守 [[宪法]]；据用户转述的外部顾问 Policy-Plugin 提案 + Explore 实测对照收敛）
前置：今晚 P0-2 + 可观测性栈(TurnTrace/domain_events) + AssetEnvelope(visibility) + TruthGraph surfacing + prompt-plugin v1(load_plugins) 全已合 main(b1ffeaa)。

## 1. 背景与定位
顾问提案：防剧透/风格/规则谨慎/节奏等表面是 prompt,但可靠实现是 **Policy Plugin**(prompt 片段 + context 过滤 + verifier + 可选 background job),且**插件只 propose、runtime 仲裁、Rust commit**。Explore 实测:**~70% 底座已存在**——`VerifierFinding`/`VerifierFindingKind`(已含 **SecretLeak/InventedEffect/MissingCheck**) + `NarrationVerifier`(已比对 narration vs `TurnLedger`,抓私造 effect/死亡) + `ErrataMemory`(findings→下回合提醒=既有 repair 通路);`ContextBlock` 已带 visibility/tags/cache_zone/priority/source_refs;`list_surfaced_entities`(TruthGraph)=已揭示信号;hook 缝 `phase_context_assembly`/`phase_verify_after_stream` 现成。
**v1 = 把这些散落的 verifier/filter/prompt 形式化成统一插件契约 + host + 贡献入 Flight Recorder**,不是 greenfield(~30% 净新)。**定位**:Runtime Kernel 的扩展层,**不是另一条 agent 总线**。

**调和用户"别太严"(早前指示)**:Policy 框架建,但防剧透**调保守**——context-filter 只删**明确 GM-only 的模组秘密块**(不是全删),verifier 走**既有 errata 提醒(repair)而非硬拦**。比纯prompt可靠,又不憋死 GM。

## 2. 已拍板决策(自主,守理念)
| 决策 | 结论 |
|---|---|
| v1 贡献类型 | 3 类:`PromptBlock` / `ContextFilter` / `VerifierFinding`(对应防剧透三段)。暂缓 Need/BackgroundJob/StateProposal/VisibilityPatch |
| v1 hook | 2 个:`ContextAssembly`(挂 PromptBlock+ContextFilter)、`AfterLlmStream`(挂 VerifierFinding)。暂缓 before_intent/before_need 等 |
| 仲裁 | 插件**只 propose**;PluginHost 收集贡献,按 **safety_class > priority** 排序;runtime 应用(prompt 追加/context 块删除/finding→ErrataMemory)。插件**不得**直接改 HP/状态/scene/reveal |
| ContributionMeta | plugin_id/hook/priority/**safety_class**(Core>Safety>Rule>Module>Experience>Format)/cache_zone/fail_policy/visibility/source_refs。PromptBlock 复用 `ContextBlock`(已带这些字段) |
| fail_policy | `ignore`(风格)/`warn_continue`(memory)/`repair_then_fail_closed`(verifier)。防剧透 context-filter=保守删(漏删优于过删);verifier=errata repair 不硬拦 |
| 权限系统 | **v1 不做**(只内置可信 Rust 插件;权限留第三方/动态插件期) |
| Flight Recorder | `TurnTrace` 加 `plugin_contributions: Vec<PluginContributionTrace>`(additive serde default)→ `trpg explain --plugins` |
| prompt-plugin v1 衔接 | 现 `load_plugins`(.md+applies_when) 升为 `TemplatePromptPlugin`:.md 正文→PromptBlock 贡献(ContextAssembly hook),applies_when 保留 |
| 内置插件 | `core.no_spoiler_guard`(PromptBlock=anti_spoiler 保守 + ContextFilter=删 GM-only 模组秘密块[保守] + VerifierFinding=SecretLeak 复用 NarrationVerifier);`core.no_mechanical_invention`(VerifierFinding 复用 NarrationVerifier 的 InventedEffect/MissingCheck——形式化为插件) |

## 3. 目标/非目标
**目标**:统一插件契约(PluginHook/PluginContribution/ContributionMeta/RuntimePlugin trait/PluginHost) + 2 hook 接线 + 2 内置插件 + 贡献入 TurnTrace + `explain --plugins`;**纯 prompt 路径零行为变更**(现 anti_spoiler 仍工作、缓存稳);ContextFilter/verifier 为**新行为但保守 + fail-soft**;零规则集硬编码(插件读 asset/block metadata 非规则集名);文件 ≤400。
**非目标(v1)**:Need/BackgroundJob/StateProposal 贡献、权限系统、4 层配置(用 applies_when)、动态/WASM 插件、yaml-template 插件清单(.md+frontmatter 够)、composer token-budget 驱逐、StateProposal 提案通路。

## 4. 设计
### 4.1 契约(新 `crates/trpg-gm/src/plugin/` 模块,hook 在 trpg-gm)
`PluginHook{ ContextAssembly, AfterLlmStream }`(枚举,可扩);
`SafetyClass{ Core, Safety, Rule, Module, Experience, Format }`(排序权重);
`FailPolicy{ Ignore, WarnContinue, RepairThenFailClosed }`;
`ContributionMeta{ plugin_id, hook, priority:i32, safety_class, cache_zone, fail_policy, visibility, source_refs }`;
`PluginContribution{ PromptBlock(ContextBlock), ContextFilter(ContextFilterSpec), VerifierFinding(VerifierFinding) }` + meta;
`ContextFilterSpec{ drop_block_ids:Vec<String> | predicate }`(v1:host 把 predicate 跑成 drop_block_ids——插件返回"删哪些 block_id");
`RuntimePlugin` trait:`fn id()->&str; fn manifest()->PluginManifest; async fn on_hook(hook, &PluginContext)->Vec<PluginContribution>`;
`PluginContext`(**只读快照**:session/turn/intent/ruleset/module/compiled-blocks-view/surfaced_entities/ledger_snapshot/recent_transcript)——**不给整个 DB**;
`PluginHost{ plugins: Vec<Box<dyn RuntimePlugin>> }`:`run_hook(hook, ctx)->Vec<(Contribution,Meta)>`(收集→按 safety_class>priority 排序),`register()`。

### 4.2 hook 接线(trpg-gm turn_loop)
- **`phase_context_assembly`**(prepare_turn_context 后、assemble 前):`host.run_hook(ContextAssembly, ctx)`→ PromptBlock 追加进 gm_skill/dynamic(按 cache_zone 进 BP1/2/3);ContextFilter 的 drop_block_ids 从 `ctx.compiled.{prefix,pinned,dynamic}_blocks` 删(再重算文本/hash)。fail_policy 守。
- **`phase_verify_after_stream`**:`host.run_hook(AfterLlmStream, ctx_with_ledger_and_text)`→ VerifierFinding 汇入现 `ErrataMemory`(既有 repair 通路,不新建)。
- 贡献全程累进 `TurnTrace.plugin_contributions`(fail-soft)。

### 4.3 内置插件
- **`core.no_spoiler_guard`**(safety_class=Safety):ContextAssembly→ PromptBlock(anti_spoiler 保守文本,BP1) + ContextFilter(删 `visibility==GmOnly` 且属模组秘密的 block——**保守**:仅删明确 GM-only 模组实体块,不碰系统/规则块;用 `list_surfaced_entities` 放行已揭示);AfterLlmStream→ VerifierFinding(复用 NarrationVerifier 的 SecretLeak,fail_policy=repair_then_fail_closed 但走 errata 提醒)。
- **`core.no_mechanical_invention`**(safety_class=Safety):AfterLlmStream→ 复用 NarrationVerifier 比对 ledger 的 InventedEffect/MissingEffect/OmittedVisibleResult,形式化为插件贡献。
（两者本质是把现有 NarrationVerifier/visibility 逻辑包成插件契约,行为≈现状 + 新增保守 context-filter。）

## 5. 理念守卫(MUST)
1. **propose-not-commit**:插件只产贡献,runtime 应用;绝不直接改状态/HP/scene/reveal/绕 BindingResolver。
2. **Safety>Style 排序**:风格插件不得覆盖防剧透/防私造(安全类)。
3. **零行为变更(prompt 路径)**:现 anti_spoiler/缓存稳测试继续绿;ContextFilter 是新增保守行为,fail-soft,过滤失败=不过滤(漏删优于崩)。
4. **零规则集硬编码**:插件读 block/asset 的 visibility/tags/surfaced,**不按规则集/模组名**(守卫 0);内置插件 id 是通用行为名。
5. fail-soft + Flight Recorder 可解释;文件 ≤400;契约 serde default。

## 6. 测试与验收
**单测**:PluginHost 排序(safety_class>priority);ContributionMeta serde;no_spoiler context-filter 选块(只删 GM-only 模组秘密、放行已 surfaced);no_mechanical_invention 复用 verifier 产 finding;fail_policy 三分支。
**等价**:prompt-only 路径(无 context-filter 命中)TurnEvent/prompt 与现状字节等价;现 NarrationVerifier/errata/缓存稳测试全绿。
**live e2e**(真库):模组回合→插件贡献入 TurnTrace、`explain --plugins` 列出(filtered blocks / prompt added / findings);GM 正常不僵(保守过滤不憋死);私造机械结果被 finding 抓。
**守卫**:no-engine-ruleset-hardcode 0;cargo test --workspace 零回归。

## 7. 切分
1. 契约 + PluginHost 骨架(plugin 模块:hooks/contribution/meta/trait/host/排序)+ `TurnTrace.plugin_contributions`(单测,无接线)。
2. 接 `ContextAssembly` hook + `core.no_spoiler_guard` 的 PromptBlock(迁移 v1 anti_spoiler)+ ContextFilter(保守删 GM-only 模组秘密块);贡献入 trace。
3. 接 `AfterLlmStream` hook + no_spoiler SecretLeak verifier + `core.no_mechanical_invention`(复用 NarrationVerifier)。
4. `trpg explain --plugins` + 等价 + live e2e + 守卫 + 全套件零回归 → 合并 main + push。

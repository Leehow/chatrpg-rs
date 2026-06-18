# NoSpoiler 防剧透守则 — 对抗性验证证据

任务：`no-spoiler-adversarial-port-20260619-001`
分支：`claude/knowledge-runtime-p0/no-spoiler-adversarial-port`
集成基线：`codex/knowledge-runtime-p0-integration` @ `87d551e`

把 `core.no_spoiler_guard` 从集成基线的「纯 PromptBlock 半边」补齐为三段守则
（ContextFilter / PromptBlock / AfterLlmStream verifier），并补源驱动采集
（`spoiler_source`）。全部验证 **provider-free**（直接构造 `PluginContext` 调
`on_hook`，不触 LLM/网络），可复跑。

## 守则契约 → 对抗性测试映射

| 守则契约（对抗面） | 测试 | 文件 |
|---|---|---|
| 玩家未知 secret 块在装配前被 fail-closed 删除 | `no_spoiler_filters_player_unknown_context` | `src/plugin/builtin_no_spoiler.rs` |
| 已揭示（player-known）fact 块放行，绝不误删已揭示 | `no_spoiler_allows_revealed_fact` | 同上 |
| 未来场景 GM-only 块被删 | `no_spoiler_filters_future_scene_gm_only` | 同上 |
| 念白泄漏未揭示私密术语 → 确定性 `SecretLeak`（Blocker）finding | `no_spoiler_verifier_catches_secret_leak` | 同上 |
| finding detail / trace 摘要绝不回写 secret 正文 | `no_spoiler_verifier_catches_secret_leak` + `no_spoiler_finding_does_not_echo_secret_term` | 同上 + `tests/no_spoiler_production_sources.rs` |
| 源解析对非场景 `module.*` id fail-closed 拒收 | `block_id_rejects_non_scene_module_ids` | `src/plugin/spoiler_source.rs` |
| 生产 fact 元数据（模组图谱 SpoilerMeta 派生）驱动删块/放行 | `no_spoiler_context_filter_uses_production_fact_metadata` / `..._allows_player_known_fact` | `tests/no_spoiler_production_sources.rs` |
| 生产源（`harvest_module_secret_terms`）非空术语 → 命中泄漏 | `no_spoiler_after_stream_uses_private_secret_terms_source` | 同上 |
| 已揭示 fact 揭示后可自由复述（不再算泄漏） | 上述泄漏测试的第二段断言 | 同上 |

## 复跑命令与结果（2026-06-19，本工作树）

```
cargo test -p trpg-gm no_spoiler        # lib 9 passed + tests/no_spoiler_production_sources 4 passed
cargo test -p trpg-gm plugin            # 49 passed
cargo test -p trpg-runtime spoiler_guard# 5 passed（既有源实体级裁剪零回归）
cargo check -p trpg-gm                  # Finished, 0 errors
git diff --check                        # clean
```

## fail-closed 默认（生产字节稳定）

`PluginContext` 新增三私有字段（`player_known_fact_ids` / `private_blocks` /
`secret_terms`）默认空。`turn_loop` 两个接线点暂喂空：
- 空 `private_blocks` → ContextFilter emitter 无输入 → no-op，`ctx.compiled` 字节稳定；
- 空 `secret_terms` → AfterLlmStream verifier 不检测（fail-soft，无误报）；
- 空 `player_known_fact_ids` → 全按未知（fail-closed 偏保守）。

故本提交对生产行为零变更；守则三段能力与源采集均已实现且对抗性验证。把生产源
（DB `list_player_known_fact_ids` 投影 + 模组图谱 `derive_scene_block_view` /
`harvest_module_secret_terms`）接进 `turn_loop` 两个接线点即可点亮——属后续生产
接线卡，留作残余风险（见 handoff）。

实时源实体级 secret 裁剪仍由 `trpg-runtime::spoiler_guard` 负责（未改动，5/5 绿），
故无防护回归。

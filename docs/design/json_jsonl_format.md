# JSON / JSONL Artifact Format v0.3

本项目从 v0.3 起只使用 JSON / JSONL 作为机器产物格式。

## 分工

```text
Markdown  PDF 转换后的 page-anchored 原文，只作为解析输入和 source ref 来源。
JSON      完整结构化对象：project bundle、rule bundle、module bundle、character template、procedure registry、module graph。
JSONL     大集合流式产物：context_blocks、material_index、source anchors、memory events、turn traces。
JSONB     PostgreSQL 运行时主存储：sessions、characters、turns、memory_facts、memory_snapshots。
```

不再使用 YAML / TOML。原因是：

1. 用户通常不会手写大型规则产物。
2. LLM structured output、HTTP API、PostgreSQL JSONB 都天然贴近 JSON。
3. JSONL 方便大规则书、长模组、并发 chunk 解析和局部重跑。
4. 缓存 hash 可以直接基于稳定 JSON 表示，避免注释、缩进、字段格式造成无意义失效。

## 输出文件

`trpg parse-all` 会写：

```text
data/parsed/project.bundle.json
data/parsed/context_blocks.jsonl
data/parsed/material_index.jsonl
```

后续可以扩展为 per-bundle shard：

```text
data/parsed/rulesets/<ruleset_id>/bundle.json
data/parsed/rulesets/<ruleset_id>/context_blocks.jsonl
data/parsed/rulesets/<ruleset_id>/material_index.jsonl

data/parsed/modules/<module_id>/bundle.json
data/parsed/modules/<module_id>/context_blocks.jsonl
data/parsed/modules/<module_id>/material_index.jsonl
```

## 缓存原则

所有 BP1/BP2/BP3 hash 都不得基于导出文件原始文本，而应基于运行时 ContextBlock 的稳定渲染结果或 canonical-ish JSON 内容：

```text
ContextBlock content/version 变化 → content_hash 变化。
BP1 block 集合变化 → prefix_hash 变化。
BP2 snapshot/scene/module block 集合变化 → pinned_hash 变化。
当前玩家输入/骰子/检索结果变化 → dynamic_hash 变化。
```

## LLM 输出契约

角色创建、parser extraction、memory extraction 都要求 JSON：

```text
Readable prose is allowed for user-facing SSE output.
Machine-readable payload must be fenced as triple-backtick `json`.
Background postprocess only parses JSON.
```

如果模型没有输出合法 JSON，后处理保存 `raw_response`，不阻塞 SSE。

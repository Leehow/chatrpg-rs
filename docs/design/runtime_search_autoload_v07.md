# v0.7 Runtime Search Autoload 设计说明

v0.7 的运行时检索接入点在 `RuntimeEngine::prepare_turn_context`。上下文编译前，runtime 会先加载规则书/模组静态 blocks、runtime-persisted TTL blocks、自动检索 blocks、memory blocks、learned packets，最后再构建 BP1/BP2/BP3。

## 1. 调用顺序

```text
list_context_blocks_for_bundles
find_material_blocks
list_runtime_context_blocks
runtime auto search
memory retrieval
learned packet retrieval
engine protocol
world state
current input/recent transcript
visibility projection
plan_blocks
ContextBuilder.build
record_load_event
```

## 2. TTL block 加载

`SearchLoadRequest::to_context_block()` 把 SearchHit 编译成 `BlockKind::LookupResult`。

- `ttl=turn` 设置 `expires_at_turn = turn_id`，默认进入 BP3。
- `ttl=scene` 设置 `expires_at_scene = scene_id`，默认进入 BP2。
- `ttl=session` scope 为 session，进入 BP2。
- `ttl=none` 不自动过期，必须显式 persist。

## 3. 持久化位置

Runtime-loaded blocks 写入 `context_blocks`，bundle_id 为：

```text
runtime.{session_id}
```

这样不会污染 ruleset/module bundle。

## 4. 缓存稳定性

- 普通 search hit 进入 BP3，不影响 pinned hash。
- scene-pinned hit 进入 BP2，只在当前 scene 复用。
- turn-expired runtime blocks 会被 `deactivate_runtime_turn_blocks` 标记 inactive。
- BP1 不接受自动 search result。

## 5. 可配置项

```env
TRPG_RUNTIME_AUTO_SEARCH=true
TRPG_RUNTIME_AUTO_SEARCH_LIMIT=5
TRPG_RUNTIME_AUTO_SEARCH_MAX_SCENE_PINS=1
TRPG_RUNTIME_AUTOPIN_SCENE_RULES=true
TRPG_RUNTIME_AUTO_SEARCH_DOMAINS=learned,rules,modules,rulings,source,parsed
```

## 6. 后续升级

v0.8 应把 `looks_rule_or_module_sensitive` 替换为可测试的 demand detector，并把 `should_pin_search_hit_to_scene` 变成 policy object。

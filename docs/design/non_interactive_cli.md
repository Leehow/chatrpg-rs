# Non-interactive CLI Design

目标：CLI 既能给真实终端用户使用，也能被 LLM、CI、shell、curl-like 测试器稳定驱动。

## Problem

`dialoguer` 交互输入需要真实 TTY。管道、CI、LLM 调试器通常没有 TTY，因此任何默认进入 `interact_text()` 的命令都会失败。

## Rule

所有需要用户输入的命令必须同时支持：

1. inline argument，例如 `--preferences` 或 `--input`；
2. file argument，例如 `--preferences-file` 或 `--input-file`；
3. stdin，例如 `--stdin`；
4. JSON request，例如 `--request-json request.json` 或 `--request-json -`；
5. machine-readable stream output，例如 `--stream-format jsonl`。

只有在没有显式输入来源且 stdin 是 TTY 时，才进入 `dialoguer` 交互。

## Stream formats

```text
text
  Human-facing. Metadata goes to stderr. LLM deltas go to stdout.

jsonl
  One event per line. Best for tests and LLM debugging.

sse
  Raw server-sent event framing. Useful for CLI/API parity tests.
```

## Commands

### create-character

Plain stdin:

```bash
echo "tech/netrunner, local fixer tie-in" \
  | trpg create-character --ruleset cyberpunk_red --module cyberpunk_red.homecoming --stdin --stream-format jsonl --no-save
```

JSON stdin:

```bash
printf '%s' '{"ruleset_id":"cyberpunk_red","module_id":"cyberpunk_red.homecoming","user_preferences":"tech/netrunner"}' \
  | trpg create-character --request-json - --stream-format jsonl --no-save
```

### turn

```bash
echo "我检查无人机背后的线缆。" \
  | trpg turn --ruleset cyberpunk_red --module cyberpunk_red.homecoming --stdin --stream-format jsonl
```

The `turn` command creates a session when `--session-id` is absent. If the caller wants multi-turn continuity, it should parse the `session` phase event and pass that id back on later calls.

## Cache stability

The non-interactive commands use the same RuntimeEngine, RuntimeMaterialPlanner, and ContextBuilder as the API and interactive shell. Therefore the BP1/BP2/BP3 hash behavior remains the same. `jsonl` phase events expose `prefix_hash`, `pinned_hash`, `dynamic_hash`, and `cache_key` for automated assertions.

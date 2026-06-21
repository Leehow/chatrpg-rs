# trpg-eval — static-transcript evaluator (蓝图 §九/§十)

The **static-transcript arm** of the chatrpg evaluator. The DB-grounded live
judges in `harness/eval/` (j1..j4) need a live session; this crate FAILs a
**frozen 战报 markdown** with root-cause + per-turn evidence — the
`negative golden fixtures` path the blueprint (`design/测试设计.md` §十 第一阶段)
makes milestone 1.

## Milestone 1 (RUN_SPEC_V2 验收1 — 必达, MET)

The two recorded bad battle reports are committed as negative golden fixtures and
**must** be judged FAIL with every documented root cause; a clean control fixture
**must** PASS (proving the verdict is metric-driven, not a hardcoded RED).

| fixture | verdict | root causes detected |
|---|---|---|
| `fixtures/bad/cyber_repetition.md` (74 turns) | **FAIL** | PLAYER_ACTION_LOOP · SCENE_RESET · SEMANTIC_NOOP · UNRESOLVED_MECHANICAL_DEBT · RESPONSE_INTENT_MISMATCH |
| `fixtures/bad/coc_semantic_shell.md` (10 turns) | **FAIL** | SEMANTIC_NOOP · SUCCESS_WITHOUT_INFORMATION · RESPONSE_INTENT_MISMATCH |
| `fixtures/good/clean_min.md` (5 turns) | **PASS** | — (discrimination control) |

The detections reproduce the blueprint's hand-analysis exactly: cyber turn 49
intent-mismatch (`押着头目…交代幕后主使` → `没有别的`), the 12&33 / 49&67 action
loops, turn 70 `待结算` debt the self-check falsely cleared; coc turn 5 success
roll (`逼问…隐瞒` 1d100=14 成功) that yields no real answer.

## Probes (蓝图 §五/§六, deterministic, env-overridable)

| root cause | layer | signal |
|---|---|---|
| `PLAYER_ACTION_LOOP` | PLAYER_POLICY | same normalized player action reused across turns (gap ≥ `EVAL_LOOP_MIN_GAP=5` or ≥3×) |
| `SCENE_RESET` | WORLD | ≥`EVAL_SCENE_RESET_TURNS=2` later turns re-materialize ≥`EVAL_SCENE_ENTITY_HITS=4` of turn-1's establishing entities (paraphrase-resistant; legitimate callbacks excluded) |
| `SEMANTIC_NOOP` | NARRATOR | long visible prose, no resolved success, carries a no-info marker |
| `UNRESOLVED_MECHANICAL_DEBT` | RULES | turn leaves `待结算` yet its own 体检 line claims `✅无未定[roll]` |
| `SUCCESS_WITHOUT_INFORMATION` | DIRECTOR | success roll + visible result still carries a no-info marker |
| `RESPONSE_INTENT_MISMATCH` | NARRATOR | player asks for info; GM reply carries a no-answer marker |

Scene reset keys on turn-1 *entity recurrence*, not raw text similarity: the
looped scene is paraphrased each render (char-trigram overlap ≈ 0.01) yet shares
turn-1's establishing entities. Generic recurrence is NOT enough — that would
flag legitimate continuity callbacks (蓝图 §五.5 callback ≠ 重演初始场景).

## Soft-weighted quality score (蓝图 §七, deterministic)

Above the hard 门槛 (PASS/FAIL), the verdict carries a 0-100 quality number whose
dimension weights are the blueprint's — **语言表现 is the LOWEST (5)**, mechanics the
highest (25). This fixes the inversion §七 names: the old harness rewarded prose
length, so a long repetitive report read as "真叙事". Now it cannot:

| fixture | score | reading |
|---|---|---|
| `clean_min.md` | **100/100 ±5** | full credit; the ±5 is the un-probed 语言 band |
| `coc_semantic_shell.md` | **65/100** | concentrated failure — responsiveness + progression zeroed, rules/world intact |
| `cyber_repetition.md` | **20/100** | deep failure — rules/world/responsiveness/player-sim all zeroed; long prose buys nothing |

Each `RootCause` debits one dimension; a hard breach zeros it, quality issues erode
it multiplicatively (monotone in severity — proven by `tests/soft_score.rs`). 语言
has no deterministic probe in the static arm, so its 5 points are granted but
surfaced as the **uncertainty band** (`±N 不确定`) rather than overclaimed.

## Run

```bash
cargo test -p trpg-eval                          # milestone-1 acceptance + discrimination
cargo run  -p trpg-eval -- <report.md> [--json]  # judge one report; exit 1 = FAIL
bash harness/eval/transcript_eval.sh --smoke     # FAIL-FAST tripwire (<2s)
```

## Layout (蓝图 §九, files < 400 lines)

```
src/model.rs        Transcript/Turn + RootCause/Severity/EvalFinding/Verdict (证据化 §八)
src/parser.rs       战报 markdown → Transcript (GM raw fence + audience + 体检)
src/probes/         repetition.rs · semantic.rs · debt.rs (the 6 root-cause probes)
src/aggregate.rs    probes → Verdict (硬门槛: any High/Hard finding ⇒ FAIL)
src/score.rs        软加权质量分 (蓝图 §七): 6 dims, 语言 weight lowest, uncertainty band
src/report.rs       evidenced red-board (蓝图 §八 格式) + score section
src/main.rs         CLI gate (--json emits verdict + score)
```

## Not yet built (later blueprint phases — see AcceptanceLedger)

LLM critics, counterfactual fork/sensitivity, persistent player-sim personas,
A/B baseline-vs-candidate. This crate is the deterministic core they hang off.

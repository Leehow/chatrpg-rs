# EVAL-HARNESS — the gate that FAILS dead/amnesiac games

The old "exam" measured surface metrics (no hollow turns, tags emit, no menu)
and went all-green on a game that was **dead**: 79 turns of "nothing happened",
an amnesiac GM, 16 checks resolved in the engine but the narration stuck on
`待结算`. A green dashboard on a corpse. This harness replaces that self-check
with a **DB-grounded blocking gate**.

It is the runtime arm of the `chatrpg-product-evaluator` skill: same
"judge the product, not the build" mindset, but with judges that query the
database instead of trusting narration.

## Pipeline

```
run_eval.sh  ──>  player_sim.sh (E1, reactive human-like player)
   │                 reads GM each turn, escalates, raises STUCK/AMNESIA
   ├─ create-character + N `trpg turn` calls (engine read-only, via CLI)
   └─ aggregate.sh (E6) ──> j1..j4 (E2-E5 blocking judges) ──> red-board + verdict
                              gate_hook.sh = factory completion gate (exit 1 = FAIL)
```

## Files

| file | lane | role |
|------|------|------|
| `lib_eval.sh` | E0 | env/DB/relay resolution, `dbq`, create/turn drivers, relay brain |
| `run_eval.sh` | E0 | run driver: create-char + N reactive turns, capture + signals |
| `player_sim.sh` | E1 | reactive, anomaly-detecting, escalating player-sim |
| `judges/j1_memory.sh` | E2 | memory-continuity: durable memory written + recall |
| `judges/j2_consequence.sh` | E3 | consequence/agency: ratio of world-changing turns |
| `judges/j3_progression.sh` | E4 | progression: scene frozen / tableau loop |
| `judges/j4_check_coherence.sh` | E5 | resolved checks vs surfaced outcomes / `待结算` orphans |
| `aggregate.sh` | E6 | combine judges → verdict, emit red-board |
| `gate_hook.sh` | E6 | thin factory gate (latest session, exit code = verdict) |
| `finalize_redboard.sh` | E8 | write canonical `EVAL_REDBOARD_v*.md` + sentinel |
| `test_judges.sh` | — | regression: the known dead run MUST RED all judges |

## Judges (DB-grounded, thresholds overridable via env)

- **J1** RED if `memory_facts+world_facts+knowledge_edges == 0` over ≥8 turns,
  or the player-sim raised an AMNESIA signal.
- **J2** RED if consequential-turn ratio `< EVAL_J2_MIN_RATIO` (0.34). A turn is
  consequential if it produced a `parameter_impact`, a committed check patch, a
  resource delta, a non-empty `state_patch`, or a new `world_fact`.
- **J3** RED if a frozen run `≥ EVAL_J3_MAX_FROZEN` (15) turns with no
  `SceneTransitioned`, transition rate `< 0.05`, or near-identical establishing
  prose repeated `≥4×`.
- **J4** RED if `≥3` check-turns and narration coverage
  (`surfaced_turns/check_turns`) `< EVAL_J4_MIN_COVERAGE` (0.5) or `>1` orphan
  `待结算` stubs.

## Usage

```bash
# full run + gate (writes a red-board into the run dir)
bash harness/eval/run_eval.sh --ruleset cyberpunk_red \
     --module cyberpunk_red.homecoming --turns 30 --judge

# judge an existing session
bash harness/eval/aggregate.sh --session <sid> --redboard /tmp/board.md

# regression proof (dead run must stay RED)
bash harness/eval/test_judges.sh
```

## Acceptance proof

`session_oc1cyber1781991491` (the dead 79-turn run on engine 4b5415d) scores
**RED on all four judges** and the gate **FAILs** it: J1 memory_facts=0, J2
ratio 0.063, J3 31-turn frozen run, J4 16 CheckResolved vs 3 surfaced
(coverage 0.188, 11 `待结算` orphans). A gate that passes that corpse is
worthless — `test_judges.sh` keeps it honest.

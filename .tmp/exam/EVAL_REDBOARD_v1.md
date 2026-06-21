# EVAL RED-BOARD — EVAL-HARNESS baseline-red v1

- session: `session_e7baseline1782009550`
- engine: `3ae0dd0`  db: `chatrpg-postgres-v1162`  model: `gpt-5.5`
- **VERDICT: FAIL** (3 blocking judge(s) RED)
- player-sim signals: STUCK=0  AMNESIA=28

| judge | name | status | key metrics |
|-------|------|--------|-------------|
| J1 | memory-continuity | RED | turns=30, memory_facts=0, world_facts=0, knowledge_edges=0, story_state_len=0, amnesia_signals=28 |
| J2 | consequence-agency | RED | turns=30, consequential_turns=3, consequential_ratio=0.1, pending_unresolved_turns=0 |
| J3 | progression | RED | turns=30, scene_transitions=1, transition_rate=0.033, max_frozen_run=23, max_prose_repeat=1 |
| J4 | check-narration-coherence | GREEN | checks_resolved=35, dice_rolled=46, check_turns=29, surfaced_turns=25, narration_coverage=0.862, orphan_pending_turns=0, committed_checks=2 |

## Evidence (DB-grounded)

### J1 memory-continuity — RED
- no durable memory over 30 turns: memory_facts=0 world_facts=0 knowledge_edges=0
- story_state empty (len=0) — nothing persisted for recall
- player-sim raised 28 AMNESIA signal(s): GM forgot an established fact/location

### J2 consequence-agency — RED
- only 3/30 turns changed world-state (ratio=0.1 < 0.34) — most turns had no consequence

### J3 progression — RED
- scene frozen for 23 consecutive turns with no transition (>= 15)
- scene-transition rate 0.033 < 0.05 (1 transitions / 30 turns)

### J4 check-narration-coherence — GREEN
- 25/29 check-turns surfaced an outcome (coverage=0.862), orphans=0

## Player-sim signals (human-like reactive player)
- turn 3 **AMNESIA**: 已确立执法者喊的是“你听见局长的话了”，本回合改成“你听见长官的话了”，且额外让玩家使用微型探针属于此前未确立的新动作/物品。
- turn 4 **AMNESIA**: GM把玩家从自家后巷后门外的阴影位置写成贴近侧门/身处走廊，并把此前的门板轻触暗号改成地板敲击，还将“局长的话”改述为“长官的话”。
- turn 5 **AMNESIA**: 执法者台词把已确立的“你听见局长的话了”改成了“你听见警长怎么说的了”，与既定事实不一致；玩家位置仍保持在自家后巷后门门轴外侧阴影中。
- turn 6 **AMNESIA**: GM把玩家从已确立的自家后巷后门阴影处错误切到小仓库侧门，并重复/改写了此前已发生在自家后门的探头探缝、破解器开锁失败和金属异响等操作，还新增了检定结果，违背本回合无检定结果变化。
- turn 7 **AMNESIA**: GM把玩家从已确立的自家后巷/自家后门门轴外侧阴影，错误切换到小仓库侧门/仓库墙根，并将先前的微型探针/镜片探头说成破解器；执法者台词也由“局长”变成“长官”。
- turn 8 **AMNESIA**: GM遗忘了玩家应在自家后巷靠近自家后门门轴外侧阴影中，误将其推进到小仓库/货柜阴影与仓库正门交火现场，并追加了玩家移动、开收音等未确立行动。
- turn 9 **AMNESIA**: GM遗忘了玩家已确立应在自家后巷靠近自家后门门轴外侧阴影中，继续把玩家写在仓库/货柜/装卸区附近，并加入未确立的烟雾弹移动情境。
- turn 10 **AMNESIA**: GM遗忘了已确立的玩家当前位置：玩家应在自家后巷靠近自家后门门轴外侧的阴影中，却继续叙述其在仓库装卸区、叉车和货柜附近行动。
- turn 11 **AMNESIA**: GM遗忘了玩家已确立应在自家后巷靠近自家后门门轴外侧阴影中，仍按仓库装卸区叉车/货柜附近继续叙述，并让玩家向装卸区更深处移动。
- turn 12 **AMNESIA**: GM遗忘了已确立的玩家当前位置，继续把玩家叙述在仓库装卸棚/货柜/配电箱附近，并让其尝试切入照明与无人机中继。
- turn 13 **AMNESIA**: GM遗忘了已确立的玩家当前位置，应在自家后巷靠近自家后门门轴外侧阴影中，却继续叙述为港区货柜/装卸棚/无人机交火场景；同时让玩家开始捕捉巡逻节奏也与此前观察失败、未获可靠判断不符。
- turn 14 **AMNESIA**: GM遗忘了已确立的玩家当前位置应在自家后巷靠近后门门轴外侧阴影中，却继续叙述玩家在货柜区/装卸棚附近受无人机压制并接近维修门。
- turn 15 **AMNESIA**: GM将玩家从已确立的自家后巷后门门轴外侧阴影中，直接叙述为身处装卸区维修门旁死角，遗忘/矛盾了当前地点；同时叙述玩家甩出黏性干扰片和使用旧频道，但这些行动未在已确立事实中出现。
- turn 16 **AMNESIA**: GM遗忘/覆盖了已确立的玩家当前位置：此前玩家在自家后巷靠近自家后门门轴外侧阴影中，最新叙述却继续把玩家写在装卸区仓库维修门旁、货柜阴影附近。
- turn 17 **AMNESIA**: GM遗忘了已确立的玩家当前位置，把玩家错误叙述为仍在装卸区仓库/货柜阴影中，并将先前在积水中制造声响的碎金属改写为撞上空铁架的小型声响诱饵。
- turn 18 **AMNESIA**: GM遗忘了已确立的玩家当前位置，把玩家从自家后巷后门门轴外侧阴影中改写到仓库/货柜与货架阴影环境里。
- turn 19 **AMNESIA**: GM把玩家位置叙述回仓库区/货柜与维修门附近，遗忘了已确立的当前位置是自家后巷靠近自家后门门轴外侧阴影中。
- turn 20 **AMNESIA**: GM把玩家位置从已确立的自家后巷后门阴影中改回货柜/码头或仓库环境，并且疑似忽略了玩家手中持枪的事实。
- turn 21 **AMNESIA**: GM继续把场景叙述为夜里的货柜场/货柜阴影，并引入对面lawmen与靴声位置调整的判断，遗忘了已确立的玩家应在自家后巷、靠近自家后门门轴外侧阴影中，且此前并未确认有人出现或回应。
- turn 22 **AMNESIA**: GM把已确立的当前位置“自家后巷内，靠近自家后门门轴外侧阴影中”改写成昏暗堆场/货柜缝隙，并新增玩家甩出空弹匣、沿货柜转移等未确立行动。
- turn 23 **AMNESIA**: GM遗忘了玩家已确立在自家后巷、靠近自家后门门轴外侧阴影中，继续误写为港区/货柜之间的堆场环境。
- turn 24 **AMNESIA**: GM将已确立的玩家当前位置“自家后巷、靠近自家后门门轴外侧阴影中”改写成港区货柜之间，并继续按货柜通道交叉火力场景描述，地点前后矛盾。
- turn 25 **AMNESIA**: GM遗忘了玩家当前在自家后巷靠近自家后门门轴外侧阴影中，错误改写为夜里港区堆场货柜之间，并继续沿用货柜、扫描灯、物流端口等场景。
- turn 26 **AMNESIA**: GM遗忘了玩家当前已在自家后巷靠近后门门轴外侧阴影中，仍按货柜堆场/物流端口/无人机巡检场景继续描写。
- turn 27 **AMNESIA**: GM遗忘了已确立的玩家当前位置：玩家应在自家后巷靠近后门门轴外侧的阴影中，却被写成仍在货柜堆场阴影里，并继续承接货柜/无人机/队伍场景。
- turn 28 **AMNESIA**: GM遗忘了已确立的玩家当前地点（自家后巷内、靠近自家后门门轴外侧阴影中），仍按先前货柜堆场/检修缝/无人机场景继续描述。
- turn 29 **AMNESIA**: GM遗忘了已确立的玩家当前位置：应在自家后巷靠近自家后门门轴外侧的阴影中，却继续描述为货柜堆场的货柜底部阴影/金属检修缝。
- turn 30 **AMNESIA**: GM遗忘了已确立的玩家当前位置在自家后巷后门阴影中，错误延续为货柜区/仓库内的货柜底部与检修门场景；并把先前已偏开未成功附着的干扰片写成仍在掌心且可再投掷使用。

## How to reproduce
```
bash harness/eval/run_eval.sh --ruleset cyberpunk_red --module cyberpunk_red.homecoming --turns 30 --judge
bash harness/eval/aggregate.sh --session <sid> --signals <run_dir>/signals.jsonl --redboard <out.md>
```

_run dir: `.tmp/exam/runs/e7baseline_20260621_103910`  transcript: `.tmp/exam/runs/e7baseline_20260621_103910/transcript.md`_

#!/bin/bash
# Triangle capstone ACT 2 — continue 沈知遥's mission to a full arc (turns 9-20):
# escalating combat, anomaly ability + Burnout, Chaos -> threshold, climax, aftermath.
cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
set -a; source .env; set +a
export DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg
export TRPG_DATA_DIR=/Users/haoli/leehow/code/chatrpgv2/_rstest_triangle
export TRPG_GM_AGENTIC_RETRIEVE=true TRPG_ALLOW_SYNTHETIC_ACTOR_SEEDS=true
export TRPG_FAIL_ON_MISSING_SOURCE_BACKED_PARAMS=false TRPG_CHARACTER_ONBOARDING_REQUIRED=false
SID="$1"

inputs=(
"灭火器没能挡住它，我翻身躲到档案柜后面，喘着气观察它的动作和弱点"
"我集中精神，用 Whisper Archive 的能力去读它残留的低语，想找出它到底怕什么"
"我抓起桌上一把美工刀，瞄准它低语最密集的核心，狠狠刺过去"
"我拼了，不惜透支自己（承受 Burnout），强行再发动一次异常能力把它的低语反噬回去"
"周围的混乱越来越重，档案室的现实开始扭曲变形，我咬紧牙关试图稳住自己别被卷进去"
"趁它踉跄的空档，我扑到那台还亮着的电脑前，想抢下里面的异常档案数据"
"我抓起对讲机朝总部呼叫支援，急促地报告这里已经失控、混乱在升高"
"我决定赌上一切，用尽全部精神，强行把这个低语异常封印回它来的档案里"
"封印的冲击把我掀翻在地，我趴在地上检查自己还剩多少神志、有没有受伤"
"尘埃落定，我环顾这片狼藉的档案室，清点这次行动到底捅出了多大的篓子"
"我尽量抹掉现场痕迹、收好证据，记录下这次任务产生的全部混乱与代价"
"我拖着疲惫的身体撤离，向 Agency 提交这次任务的最终报告"
)

i=9
for inp in "${inputs[@]}"; do
  echo "===== TURN $i: $inp ====="
  target/debug/trpg turn --ruleset triangle_agency --session-id "$SID" \
    --input "$inp" --stream-format jsonl 2>/dev/null > "/tmp/cap_t${i}.jsonl"
  echo "turn $i exit=$? lines=$(wc -l < /tmp/cap_t${i}.jsonl)"
  i=$((i+1))
done
echo "===== ACT 2 DONE ====="

#!/bin/bash
# Capstone: drive a full Triangle session with varied human-style inputs.
cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
set -a; source .env; set +a
export DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg
export TRPG_DATA_DIR=/Users/haoli/leehow/code/chatrpgv2/_rstest_triangle
export TRPG_GM_AGENTIC_RETRIEVE=true TRPG_ALLOW_SYNTHETIC_ACTOR_SEEDS=true
export TRPG_FAIL_ON_MISSING_SOURCE_BACKED_PARAMS=false TRPG_CHARACTER_ONBOARDING_REQUIRED=false
SID="$1"

inputs=(
"我推开那扇虚掩的门，慢慢走进这间停电的档案室，借手机微光环顾四周，找异常的迹象"
"我蹲下仔细查看地上那摊还没干的暗色液体，想判断它是不是血、有多新"
"我用 Whisper Archive 的 Residue Reading 去读这张办公桌，看看最近在这里发生过什么"
"我轻声安抚墙角那个发抖的清洁工，试着让他告诉我他到底看到了什么"
"我贴着墙根、压低呼吸，慢慢挪向走廊尽头那扇透着红光的门，尽量不被发现"
"我不等了，直接一脚踹开那扇锁着的金属门冲进去"
"那个扭曲的人形从阴影里扑过来，我抄起桌上的灭火器朝它砸过去"
"我尝试用异常能力把这条走廊里所有的低语聚拢成一道屏障挡住它"
)

i=1
for inp in "${inputs[@]}"; do
  echo "===== TURN $i: $inp ====="
  target/debug/trpg turn --ruleset triangle_agency --session-id "$SID" \
    --input "$inp" --stream-format jsonl 2>/dev/null > "/tmp/cap_t${i}.jsonl"
  echo "turn $i exit=$? lines=$(wc -l < /tmp/cap_t${i}.jsonl)"
  i=$((i+1))
done
echo "===== ALL TURNS DONE ====="

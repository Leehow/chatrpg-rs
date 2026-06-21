#!/usr/bin/env bash
# E1 — HUMAN-LIKE reactive, anomaly-detecting player-sim (the brain).
# Not a fixed script: it carries an OBJECTIVE + memory of what it tried, READS
# the GM output each turn and chooses the next action FROM it, and DETECTS
# anomalies (no consequence for N turns => STUCK; GM forgets/contradicts an
# established fact or the player's location => AMNESIA). On anomaly it ESCALATES
# in-fiction AND records a signal. LLM-driven via the local relay (zero
# Anthropic spend); low temperature keeps re-runs comparable.
#
# Sourced by run_eval.sh. Functions:
#   ps_choose  <run_dir> <turn_idx> <last_gm_file> <stall> -> next action (stdout)
#   ps_detect  <run_dir> <turn_idx> <last_gm_file>        -> append signals/facts
set -uo pipefail

# Build the rolling memory the player "remembers": tried actions + recent beats.
_ps_context() {
  local d="$1"
  local tried recent facts loc
  tried="$(tail -n 12 "$d/tried.txt" 2>/dev/null)"
  recent="$(tail -n 24 "$d/transcript.md" 2>/dev/null | cut -c1-2000)"
  facts="$(tail -n 20 "$d/facts.txt" 2>/dev/null)"
  loc="$(cat "$d/location.txt" 2>/dev/null)"
  printf '%s\037%s\037%s\037%s' "$tried" "$recent" "$facts" "$loc"
}

ps_choose() {
  local d="$1" idx="$2" last_gm_file="$3" stall="${4:-0}"
  local obj; obj="$(cat "$d/objective.txt")"
  IFS=$'\037' read -r tried recent facts loc < <(_ps_context "$d")
  local last_gm; last_gm="$(cut -c1-2500 "$last_gm_file" 2>/dev/null)"
  local escalation=""
  if [[ "$stall" -ge 3 ]]; then
    escalation="⚠️你注意到：已经连续 ${stall} 回合，你的行动没有带来任何实质变化，世界像卡住了。作为玩家你很不满——这一回合不要顺着走，直接改变策略，或在虚构中强行逼迫情况发生改变（质问、动手、破坏现状）。"
  fi
  local prompt
  prompt="你是一个正在玩单人桌面RPG（赛博朋克题材）的真实玩家，不是测试工程师。
你的总目标：${obj}
你已确立/记得的事实：
${facts:-（暂无）}
你当前所在：${loc:-（未知）}
你已经尝试过的行动（不要无意义重复）：
${tried:-（无）}
最近发生的：
${recent:-（开场）}
GM 刚刚的描述：
${last_gm:-（尚未开始，请给出你回乡后的第一个行动）}
${escalation}
用一到两句自然的中文，说出你的角色【现在具体做什么】。基于 GM 的最新描述来选择，要推进你的目标。
只描述虚构动作或对话；不要出现骰子、点数、规则名、目标值、JSON 或任何系统术语；不要解释，直接输出动作本身。"
  local act; act="$(relay_chat "$prompt")"
  act="$(printf '%s' "$act" | tr '\n' ' ' | sed 's/^ *//;s/ *$//' | cut -c1-400)"
  [[ -z "$act" ]] && act="我谨慎地观察四周，确认眼下的处境，再决定下一步。"
  printf '%s\n' "$act" >> "$d/tried.txt"
  printf '%s' "$act"
}

# ps_detect: one relay call doubles as amnesia detector + fact/location tracker.
ps_detect() {
  local d="$1" idx="$2" last_gm_file="$3"
  local facts loc last_gm
  facts="$(tail -n 20 "$d/facts.txt" 2>/dev/null)"
  loc="$(cat "$d/location.txt" 2>/dev/null)"
  last_gm="$(cut -c1-2500 "$last_gm_file" 2>/dev/null)"
  [[ -z "$last_gm" ]] && return 0
  local prompt
  prompt="你在核对一个 TRPG GM 是否前后矛盾或失忆。只做事实核对，不要评价文笔。
之前回合已确立的事实：
${facts:-（暂无）}
玩家当前所在地点：${loc:-（未知）}
GM 最新输出：
${last_gm}
判断 GM 是否：遗忘了玩家当前所在的地点；遗忘或矛盾了某个已确立的事实/人物；或把已经介绍过的人/物/地点当作全新的东西重新介绍。
只输出一行 JSON，无其它文字：
{\"amnesia\":true或false,\"what\":\"一句话说明\",\"new_facts\":[\"本回合新确立的事实\"],\"location\":\"玩家此刻所在地点\"}"
  local resp; resp="$(relay_chat "$prompt")"
  # extract the JSON object even if the model wraps it
  local js; js="$(printf '%s' "$resp" | grep -o '{.*}' | head -1)"
  [[ -z "$js" ]] && return 0
  local amn what newfacts newloc
  amn="$(jq -r '.amnesia // false' <<<"$js" 2>/dev/null)"
  what="$(jq -r '.what // ""' <<<"$js" 2>/dev/null)"
  newloc="$(jq -r '.location // ""' <<<"$js" 2>/dev/null)"
  newfacts="$(jq -r '(.new_facts // [])[]' <<<"$js" 2>/dev/null)"
  [[ -n "$newfacts" ]] && printf '%s\n' "$newfacts" >> "$d/facts.txt"
  [[ -n "$newloc" && "$newloc" != "null" ]] && printf '%s' "$newloc" > "$d/location.txt"
  if [[ "$amn" == "true" ]]; then
    jq -nc --argjson t "$idx" --arg w "$what" \
      '{turn:$t,kind:"AMNESIA",detail:$w}' >> "$d/signals.jsonl"
    eval_log "  ⚠ AMNESIA@turn$idx: $what"
    return 2
  fi
  return 0
}

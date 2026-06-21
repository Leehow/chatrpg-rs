#!/usr/bin/env bash
# Regression test for the EVAL-HARNESS judges. Pins the acceptance proof: the
# known dead 79-turn run (session_oc1cyber1781991491, engine 4b5415d) MUST score
# RED on every blocking judge, and the aggregate gate MUST FAIL it. If a judge
# silently goes GREEN on the corpse, the gate is worthless — this test catches
# that. Override the fixture with EVAL_DEAD_SESSION=<sid>.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEAD="${EVAL_DEAD_SESSION:-session_oc1cyber1781991491}"
fails=0

check_red() {
  local name="$1" script="$2"
  local st; st="$(bash "$HERE/judges/$script" --session "$DEAD" 2>/dev/null | jq -r '.status')"
  if [[ "$st" == "RED" ]]; then echo "PASS  $name RED on dead run"; else
    echo "FAIL  $name expected RED, got '$st'"; fails=$((fails+1)); fi
}

echo "== judges must RED the dead run ($DEAD) =="
check_red J1 j1_memory.sh
check_red J2 j2_consequence.sh
check_red J3 j3_progression.sh
check_red J4 j4_check_coherence.sh

echo "== aggregate gate must FAIL the dead run =="
if bash "$HERE/aggregate.sh" --session "$DEAD" --label test >/dev/null 2>&1; then
  echo "FAIL  gate returned PASS on dead run (worthless gate!)"; fails=$((fails+1))
else
  echo "PASS  gate FAILs the dead run"
fi

echo
if [[ "$fails" -eq 0 ]]; then echo "ALL GREEN: harness correctly fails the dead game"; exit 0
else echo "$fails check(s) FAILED"; exit 1; fi

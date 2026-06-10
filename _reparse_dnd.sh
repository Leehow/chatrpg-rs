#!/bin/bash
set -e
export DATABASE_URL="postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg"
export TRPG_LLM_BASE_URL="http://127.0.0.1:18888/v1"
export TRPG_LLM_API_KEY="codex-relay-local"
export TRPG_LLM_MODEL="gpt-5.4-mini"
export TRPG_LLM_SEND_TEMPERATURE="false"
export TRPG_CHARGEN_COMPILER_MODEL="gpt-5.4"
export TRPG_READER_BUDGET="15"
export TRPG_DATA_DIR="/Users/haoli/leehow/code/chatrpgv2/_rstest_dnd"
echo "[reparse] start $(date)"
./target/debug/trpg parse-all --force --pdf-backend duotext --data-dir /Users/haoli/leehow/code/chatrpgv2/_rstest_dnd 2>&1
echo "[reparse] exit=$? $(date)"

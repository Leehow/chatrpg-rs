#!/usr/bin/env python3
"""OpenAI-backed semantic critic for scripts/eval_redboard.py.

Reads the semantic critic payload JSON from stdin and writes:

    {"findings": [...]}

Configuration:
    OPENAI_API_KEY                 required
    TRPG_EVAL_SEMANTIC_MODEL       required
    OPENAI_BASE_URL                optional, defaults to https://api.openai.com/v1
"""

from __future__ import annotations

import json
import os
import re
import sys
import urllib.error
import urllib.request
from typing import Any, Dict


SYSTEM_PROMPT = """You are an independent TRPG semantic auditor.

Judge meaning, not exact phrases. Do not rely on keyword matching. The input may
mix Chinese and English, and the same failure can be paraphrased many ways.

You audit two layers:
1. Q4 GM presentation constitution:
   - An open prompt such as "What do you do?" is allowed.
   - Fail if the GM presents GM-authored action menus, explicit option branches,
     "choose one" structures, or raw content dumps as the player's choices.
2. P1 player simulator realism:
   - The player may only use player-visible information.
   - The player should update beliefs from the latest GM reply.
   - The player should resolve or clarify pending access gates before jumping away.
   - The player should pursue concrete visible affordances unless the persona has
     a grounded reason not to.

Allowed finding categories:
ACTION_MENU
ACTION_MENU_OR_EXPLICIT_OPTIONS
EXPLICIT_OPTIONS
OPTION_MENU
RAW_CONTENT_DUMP
GM_AUTHORED_ACTION_BRANCHES
PLAYER_UNRESOLVED_GATE
PLAYER_FALLBACK_AFTER_CONCRETE_AFFORDANCE
PLAYER_SCRIPT_LOOP
PLAYER_HIDDEN_KNOWLEDGE
PLAYER_IGNORES_VISIBLE_RESULT
PLAYER_STATE_CONTRADICTION
PLAYER_BELIEF_INCONSISTENCY
RESPONSE_INTENT_MISMATCH
SEMANTIC_NOOP
SUCCESS_WITHOUT_INFORMATION
PENDING_MECHANICAL_DEBT

Severity:
S1 = hard fail, S2 = serious fail, S3 = warning, S4 = note.

Return JSON only:
{
  "findings": [
    {
      "finding_id": "F-00001",
      "category": "ACTION_MENU",
      "severity": "S1",
      "turns": [1],
      "root_layer": "Presentation",
      "expected": "...",
      "actual": "...",
      "evidence": ["..."],
      "confidence": 0.0
    }
  ]
}

Only report evidence-backed findings. If there are no findings, return
{"findings":[]}.
"""


def require_env(name: str) -> str:
    value = os.environ.get(name, "").strip()
    if not value:
        raise SystemExit(f"{name} is required")
    return value


def response_text(response: Dict[str, Any]) -> str:
    direct = response.get("output_text")
    if isinstance(direct, str) and direct.strip():
        return direct
    chunks = []
    for item in response.get("output", []):
        if not isinstance(item, dict):
            continue
        for content in item.get("content", []):
            if not isinstance(content, dict):
                continue
            text = content.get("text")
            if isinstance(text, str):
                chunks.append(text)
    return "\n".join(chunks)


def parse_json_text(text: str) -> Dict[str, Any]:
    try:
        parsed = json.loads(text)
    except json.JSONDecodeError:
        match = re.search(r"\{.*\}", text, flags=re.DOTALL)
        if not match:
            raise
        parsed = json.loads(match.group(0))
    if not isinstance(parsed, dict):
        raise ValueError("semantic critic response must be a JSON object")
    return parsed


def call_responses_api(payload: Dict[str, Any]) -> Dict[str, Any]:
    api_key = require_env("OPENAI_API_KEY")
    model = require_env("TRPG_EVAL_SEMANTIC_MODEL")
    base_url = os.environ.get("OPENAI_BASE_URL", "https://api.openai.com/v1").rstrip("/")
    body = {
        "model": model,
        "input": [
            {
                "role": "system",
                "content": [{"type": "input_text", "text": SYSTEM_PROMPT}],
            },
            {
                "role": "user",
                "content": [
                    {
                        "type": "input_text",
                        "text": json.dumps(payload, ensure_ascii=False),
                    }
                ],
            },
        ],
        "text": {"format": {"type": "json_object"}},
        "max_output_tokens": int(os.environ.get("TRPG_EVAL_SEMANTIC_MAX_OUTPUT_TOKENS", "4096")),
    }
    request = urllib.request.Request(
        f"{base_url}/responses",
        data=json.dumps(body, ensure_ascii=False).encode("utf-8"),
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=int(os.environ.get("TRPG_EVAL_SEMANTIC_TIMEOUT", "180"))) as response:
            return json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        body_text = exc.read().decode("utf-8", errors="replace")
        raise SystemExit(f"OpenAI API error {exc.code}: {body_text}")


def main() -> int:
    payload = json.loads(sys.stdin.read())
    response = call_responses_api(payload)
    parsed = parse_json_text(response_text(response))
    findings = parsed.get("findings", [])
    if not isinstance(findings, list):
        findings = []
    print(json.dumps({"findings": findings}, ensure_ascii=False, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

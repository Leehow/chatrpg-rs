#!/usr/bin/env python3
"""Run a constrained live autoplay for CoC 7e: The Haunting.

This is a playtest driver, not engine logic. It may be scenario-specific, but
it must only use player-visible transcript text and the generated character
sheet when choosing the next action. It writes full PLAYER_DECISION records and
sends only declared_action to the GM.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any


RULESET = "call_of_cthulhu_7e"
MODULE = "call_of_cthulhu_7e.the_haunting"
DB_URL = "postgres://chatrpg:chatrpg@localhost:54347/chatrpg"
CREATE_CHARACTER_TIMEOUT_SECONDS = 420
DEFAULT_TURN_TIMEOUT_SECONDS = 300
DEFAULT_OUTPUT_LANGUAGE = "en"


def normalize_output_language(output_language: str | None) -> str:
    raw = (output_language or DEFAULT_OUTPUT_LANGUAGE).strip()
    if not raw:
        return DEFAULT_OUTPUT_LANGUAGE
    lowered = raw.lower().replace("_", "-")
    if lowered in {"zh", "zh-cn", "zh-hans", "chinese", "simplified-chinese", "简体中文", "中文"}:
        return "zh-Hans"
    if lowered in {"en", "en-us", "english"}:
        return "en"
    return raw


def is_chinese_output_language(output_language: str | None) -> bool:
    return normalize_output_language(output_language).lower().startswith("zh")


def character_preferences(output_language: str | None = None) -> str:
    if is_chinese_output_language(output_language):
        return (
            "请用简体中文创建一名完整的《鬼屋》谨慎调查型调查记者角色。"
            "背景、装备和技能说明使用简体中文；偏好具体实地行动、公共档案调查，"
            "并携带足够支持单人安全测试的常用装备。"
        )
    return (
        "Create a complete cautious investigative journalist for The Haunting. "
        "Prefer concrete field action, public-record research, and enough gear "
        "for a solo safety-conscious playtest."
    )


def localize_opening_for_output_language(opening: str, output_language: str | None = None) -> str:
    if not is_chinese_output_language(output_language):
        return opening
    return (
        "1920 年代的波士顿，房东史蒂文·诺特请你调查那栋名声糟糕的科比特宅。"
        "他把钥匙和预付款交到你手里，却显然不愿把所有传闻一次说透：前任住户马卡里奥一家出事后，"
        "这栋房子仍被邻里视作不祥之地。你现在能做的，是先向诺特问清地址、钥匙和可核查的公共记录来源，"
        "再决定怎样进入调查。"
    )


def localized_pc_label(state: "VisibleState") -> str:
    if is_chinese_output_language(state.output_language):
        return "调查员"
    return state.pc_name


def localize_player_action_for_output_language(
    state: "VisibleState",
    *,
    declared_action: str,
    intent: str,
    requested: list[str] | None = None,
) -> str:
    if not is_chinese_output_language(state.output_language):
        return declared_action

    pc = localized_pc_label(state)

    if "clarify" in intent or "recover_from" in intent or "state_reset" in intent:
        return (
            f"{pc}先停住，不接受场景或位置被重置。请结算我上一回合已经声明的具体行动："
            "我现在到底站在哪里，上一动作是成功、失败、被阻止，还是需要一次明确检定；"
            "同时说明我此刻能看见、听见或触到的一个具体事实。"
        )
    if "briefing" in intent or "source_bound_address" in intent:
        return (
            "我不会立刻离开。调查员打开笔记本，请诺特写下科比特宅可核实的地址线索，"
            "逐一说明每把钥匙可能对应哪道门或锁；然后我追问他实际知道的马卡里奥一家情况、"
            "哪些市政记录或旧报纸可能查到房屋历史，以及他是否同意我先查公共记录再进屋。"
        )
    if "hall_records" in intent or "official_records" in intent or "court_police_records" in intent:
        return (
            f"{pc}前往市政厅和相关公共档案处，以诺特给出的房屋线索为起点，"
            "查产权、遗嘱认证、法院、警方或住户事故记录。我要求的是具体条目、姓名、日期、"
            "地址关联或明确的拒绝理由，不接受只说“有线索”但不给内容。"
        )
    if "newspaper" in intent:
        return (
            f"{pc}去旧报馆或报纸档案室，以职业记者身份说明调查目的，"
            "查科比特宅、马卡里奥一家、房屋事故、讣告和异常宗教团体的旧报道。"
            "如果需要检定，请按档案检索或社交进入权限结算；成功时请给出可核查的报道内容。"
        )
    if "chapel" in intent or "cabinet" in intent:
        return (
            f"{pc}前往或继续检查沉思教堂相关地点，只根据已经可见的线索行动。"
            "我从安全距离观察门窗、柜子、符号、纸张、灰尘痕迹和可能的藏物，"
            "记录任何与科比特、房契、葬礼或教团成员有关的具体事实。"
        )
    if "house_exterior" in intent:
        return (
            f"{pc}到科比特宅外部先做安全勘察，不急着冲进去。"
            "我绕行查看门窗、地基、退路、邻近视线和明显危险，用笔记和相机记录，"
            "再确认哪一个入口是真正可用、相对安全的切入点。"
        )
    if "enter_threshold" in intent:
        return (
            f"{pc}用诺特的钥匙谨慎打开已确认的入口，只站在门槛附近向内观察。"
            "我不深入探索，先确认玄关、走廊、楼梯、可见房间、退路和异常气味或声响，"
            "再决定下一步。"
        )
    if "ground_floor" in intent or "hall" in intent:
        return (
            f"{pc}从已经确认的入口开始系统搜索一楼。"
            "我保持退路开放，逐间记录门、窗、地板、墙面、冷风、拖痕、纸张和可疑物件，"
            "发现不确定或危险迹象时请开相应检定并给出具体结果。"
        )
    if "basement" in intent or "cellar" in intent:
        return (
            f"{pc}回到已标记的地下室入口，先固定门、留下手帕标记和退路，再用手电和手杖试探下降。"
            "我沿墙和地面寻找翻动泥土、暗格、木板松动、气流或隐藏空间；若危险或不确定，请结算检定。"
        )
    if "bedroom" in intent or "upper_floor" in intent:
        return (
            f"{pc}谨慎检查楼上和卧室，只从门槛或可撤退位置观察。"
            "我确认床、纸张、墙面、地板、门轴和任何会突然移动的危险；能取到的文件先用工具拉近，"
            "不把身体暴露在床或房间深处。"
        )
    if "crawl_space" in intent or "recess" in intent or "broken_cellar_opening" in intent:
        return (
            f"{pc}只在已经标记好退路的前提下检查狭窄夹层或破口。"
            "我先用光线、手杖和绳线确认深度、空气、墙面刻痕、桌面文件和能否安全后退；"
            "任何继续深入都请给出明确位置和风险结算。"
        )
    if "blade" in intent or "knife" in intent:
        return (
            f"{pc}立刻把注意力放在可见刀刃威胁上，后撤到标记退路，举起手杖或可用物件格挡，"
            "不主动深入未知空间。请按即时危险结算防御、撤退或受伤后果。"
        )
    if "corbitt" in intent or "attack_visible" in intent:
        return (
            f"{pc}只针对已经亲眼确认的科比特威胁行动。"
            "我保持退路，使用手杖、工具或可用武器压制他，同时大声确认他是否仍能行动；"
            "请按战斗或对抗规则结算。"
        )
    if "tools" in intent or "return_with_tools" in intent:
        return (
            f"{pc}暂时退出危险区域，记录当前位置和未解决障碍，去取更合适的工具后再按原路线返回。"
            "我不会把尚未打开或尚未确认的结构当成已经成功处理。"
        )
    if "withdraw" in intent or "retreat" in intent or "abandon" in intent:
        return (
            f"{pc}按已经标记的安全路线撤出当前危险点，边退边观察是否有追击、声响或环境变化。"
            "撤到稳定位置后，我整理已知事实并选择下一条可核查线索。"
        )

    requested_text = ""
    if requested:
        requested_text = "我需要你交付具体可见结果，而不是空泛气氛。"
    return (
        f"{pc}继续按当前可见线索推进调查，保持退路和记录。"
        f"{requested_text}如果行动不可能，请说明原因；如果不确定，请开明确检定并结算。"
    )


def source_mtimes_for_binary(root: Path) -> list[float]:
    candidates: list[Path] = [root / "Cargo.toml", root / "Cargo.lock"]
    candidates.extend((root / "crates").glob("**/*.rs"))
    candidates.extend((root / "crates").glob("**/Cargo.toml"))
    return [path.stat().st_mtime for path in candidates if path.exists()]


def binary_needs_rebuild(root: Path, binary: Path) -> bool:
    if not binary.exists():
        return True
    mtimes = source_mtimes_for_binary(root)
    if not mtimes:
        return False
    return max(mtimes) > binary.stat().st_mtime


def ensure_trpg_binary(root: Path) -> None:
    binary = root / "target-f1/debug/trpg"
    if not binary_needs_rebuild(root, binary):
        return
    build = run_cmd(
        [
            "cargo",
            "build",
            "-p",
            "trpg-cli",
            "--bin",
            "trpg",
            "--target-dir",
            "target-f1",
        ],
        env=trpg_env(),
        timeout=900,
    )
    (root / ".tmp/coc_autoplay_build_stdout.txt").parent.mkdir(parents=True, exist_ok=True)
    (root / ".tmp/coc_autoplay_build_stdout.txt").write_text(build.stdout, encoding="utf-8")
    (root / ".tmp/coc_autoplay_build_stderr.txt").write_text(build.stderr, encoding="utf-8")
    if build.returncode != 0:
        raise RuntimeError(f"trpg build failed with {build.returncode}: {build.stderr}")


def trpg_env(output_language: str | None = None) -> dict[str, str]:
    env = {
        "DATABASE_URL": DB_URL,
        "TRPG_NARRATOR_SPLIT": "true",
        "TRPG_PRESENTATION_GATE": "true",
        "TRPG_GM_CRAFT": "true",
        "TRPG_REVEAL_GATING": "true",
        "TRPG_CLUE_PROJECTION": "true",
    }
    if output_language is not None:
        env["TRPG_OUTPUT_LANGUAGE"] = normalize_output_language(output_language)
    return env


def semantic_critic_cmd(root: Path) -> str:
    explicit = os.environ.get("TRPG_EVAL_SEMANTIC_CRITIC_CMD", "").strip()
    if explicit:
        return explicit
    has_key = bool(os.environ.get("OPENAI_API_KEY", "").strip())
    has_model = bool(os.environ.get("TRPG_EVAL_SEMANTIC_MODEL", "").strip())
    if has_key and has_model:
        return f"{sys.executable} {root / 'scripts/semantic_critic_openai.py'}"
    return ""


@dataclass
class CommandResult:
    args: list[str]
    returncode: int
    stdout: str
    stderr: str


@dataclass
class VisibleState:
    transcript: str
    last_reply: str
    turn: int
    pc_name: str
    output_language: str = DEFAULT_OUTPUT_LANGUAGE
    attempted: set[str] = field(default_factory=set)

    def lower_all(self) -> str:
        return self.transcript.lower()

    def lower_last(self) -> str:
        return self.last_reply.lower()


def run_cmd(
    args: list[str],
    *,
    env: dict[str, str] | None = None,
    timeout: int = 240,
) -> CommandResult:
    merged = os.environ.copy()
    if env:
        merged.update(env)
    try:
        proc = subprocess.run(
            args,
            text=True,
            capture_output=True,
            env=merged,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as exc:
        stdout = exc.stdout or ""
        stderr = exc.stderr or ""
        if isinstance(stdout, bytes):
            stdout = stdout.decode("utf-8", errors="replace")
        if isinstance(stderr, bytes):
            stderr = stderr.decode("utf-8", errors="replace")
        timeout_note = f"command timed out after {timeout} seconds"
        stderr = f"{stderr}\n{timeout_note}".strip()
        return CommandResult(args=args, returncode=-124, stdout=stdout, stderr=stderr)
    return CommandResult(args=args, returncode=proc.returncode, stdout=proc.stdout, stderr=proc.stderr)


def read_jsonl(path: Path) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    if not path.exists():
        return rows
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        if not line.strip():
            continue
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict):
            rows.append(value)
    return rows


def extract_delta(jsonl_path: Path) -> str:
    chunks: list[str] = []
    for row in read_jsonl(jsonl_path):
        if row.get("event") == "delta":
            chunks.append(str(row.get("data", "")))
    return "".join(chunks).strip()


def character_created_row(create_jsonl: Path) -> dict[str, Any]:
    for row in read_jsonl(create_jsonl):
        if row.get("event") == "phase" and row.get("phase") == "character_created":
            data = row.get("data")
            if isinstance(data, dict):
                return data
    raise RuntimeError(f"character_created event not found in {create_jsonl}")


def write_character_sheet(create_jsonl: Path, out_path: Path) -> str:
    data = character_created_row(create_jsonl)
    sheet = data.get("sheet") if isinstance(data.get("sheet"), dict) else {}
    name = str(data.get("name") or sheet.get("name") or "Investigator")
    skills = sheet.get("skills") if isinstance(sheet.get("skills"), dict) else {}
    stats = sheet.get("stats") if isinstance(sheet.get("stats"), dict) else {}
    resources = sheet.get("resources") if isinstance(sheet.get("resources"), dict) else {}

    core_stat_names = [
        "STR",
        "CON",
        "SIZ",
        "DEX",
        "APP",
        "INT",
        "POW",
        "EDU",
        "HP",
        "MP",
        "SAN",
        "Luck",
        "Move",
        "Build",
    ]
    key_skill_names = [
        "Library Use",
        "Spot Hidden",
        "History",
        "Law",
        "Psychology",
        "Fast Talk",
        "Persuade",
        "Charm",
        "Listen",
        "Stealth",
        "Dodge",
        "Fighting (Brawl)",
        "Firearms (Handgun)",
        "First Aid",
        "Credit Rating",
        "Occult",
    ]

    lines = [f"# Character Sheet: {name}", ""]
    lines.extend(
        [
            "## Identity",
            f"- Name: {name}",
            f"- Occupation: {sheet.get('occupation', 'Unknown')}",
            f"- Age: {sheet.get('age', 'Unknown')}",
            f"- Sex: {sheet.get('sex', 'Unknown')}",
            f"- Residence: {sheet.get('residence', 'Unknown')}",
            f"- Birthplace: {sheet.get('birthplace', 'Unknown')}",
            f"- Assets: {sheet.get('assets', 'Unknown')}",
            f"- Cash: {sheet.get('cash', 'Unknown')}",
            f"- Spending Level: {sheet.get('spending_level', 'Unknown')}",
            "",
            "## Core Stats",
            "| Stat | Value |",
            "| --- | ---: |",
        ]
    )
    for key in core_stat_names:
        if key in stats:
            lines.append(f"| {key} | {stats[key]} |")
        elif key.lower() in sheet:
            lines.append(f"| {key} | {sheet[key.lower()]} |")
    lines.extend(["", "## Resources", "| Resource | Value |", "| --- | ---: |"])
    for key, value in resources.items():
        lines.append(f"| {key} | {value} |")
    lines.extend(["", "## Key Skills"])
    for key in key_skill_names:
        if key in skills:
            lines.append(f"- {key}: {skills[key]}")
    lines.append("")
    out_path.write_text("\n".join(lines), encoding="utf-8")
    return name


def latest_turn_id(root: Path, session_id: str) -> str:
    safe_session = session_id.replace("'", "''")
    sql = (
        "select turn_id from turns "
        f"where session_id='{safe_session}' order by created_at desc limit 1;"
    )
    proc = run_cmd(
        [
            "docker",
            "exec",
            "chatrpg-postgres-rulesets",
            "psql",
            "-U",
            "chatrpg",
            "-d",
            "chatrpg",
            "-t",
            "-A",
            "-c",
            sql,
        ],
        timeout=30,
    )
    if proc.returncode != 0:
        (root / ".tmp/latest_coc_psql_error.txt").write_text(proc.stderr, encoding="utf-8")
    return proc.stdout.strip()


def roll_blocks(text: str) -> list[str]:
    return re.findall(r"\[roll\](.*?)\[/roll\]", text, flags=re.IGNORECASE | re.DOTALL)


def roll_outcome(block: str) -> str | None:
    lowered = block.lower()
    if any(cue in lowered for cue in ("strong_failure", "fumble", "failure", "failed", "失败")):
        return "failure"
    if "success" in lowered or "succeeded" in lowered or "成功" in lowered:
        return "success"
    return None


def last_roll_outcome(text: str) -> str | None:
    blocks = roll_blocks(text)
    if not blocks:
        return None
    for block in reversed(blocks):
        outcome = roll_outcome(block)
        if outcome:
            return outcome
    return None


def has_success(text: str) -> bool:
    outcome = last_roll_outcome(text)
    if outcome:
        return outcome == "success"
    lowered = text.lower()
    return "[roll]" in lowered and ("success" in lowered or "成功" in lowered) and not any(
        cue in lowered for cue in ("failure", "failed", "失败")
    )


def has_failure(text: str) -> bool:
    outcome = last_roll_outcome(text)
    if outcome:
        return outcome == "failure"
    lowered = text.lower()
    return "[roll]" in lowered and ("failure" in lowered or "failed" in lowered or "失败" in lowered)


def mentions(text: str, *needles: str) -> bool:
    lowered = text.lower()
    return any(needle.lower() in lowered for needle in needles)


def contains_word(text: str, word: str) -> bool:
    return re.search(rf"\b{re.escape(word.lower())}\b", text.lower()) is not None


def visible_cue_text(text: str) -> str:
    return re.sub(r"[*_`]+", "", text.lower())


def visible_newspaper_archive_contact(text: str) -> str:
    """Extract a player-visible archive contact without relying on one sentence."""
    name = r"([A-Z][A-Za-z'’-]+(?:\s+[A-Z][A-Za-z'’-]+){0,2})"
    patterns = (
        rf"\b(?:editor|gatekeeper)\s+{name}\b",
        rf"\bname\s+that\s+comes\s+back\s+is\s+{name}\b",
        rf"\bname\s+comes\s+back\s+as\s+{name}\b",
    )
    for pattern in patterns:
        match = re.search(pattern, text)
        if match:
            contact = match.group(1).strip(" ,.;:")
            if contact.lower() not in {"the", "an", "a", "access", "editor", "gatekeeper"}:
                return contact
    return "the editor or gatekeeper"


def sentence_excerpt(text: str, limit: int = 360) -> str:
    clean = re.sub(r"\s+", " ", text).strip()
    if len(clean) <= limit:
        return clean
    return clean[: limit - 3].rstrip() + "..."


def adjudicated_visible_text(text: str) -> str:
    """Drop player declarations so requested search terms do not become facts."""
    return "\n".join(
        line
        for line in text.splitlines()
        if not line.lstrip().lower().startswith("player:")
    )


def explicit_burial_clue_visible(text: str) -> bool:
    lowered = adjudicated_visible_text(text).lower()
    negated_burial = any(
        cue in lowered
        for cue in (
            "no burial",
            "no clear burial",
            "no confirmed burial",
            "not have a confirmed burial",
            "not a burial note",
            "not a full burial note",
            "not a full record, burial note",
            "nothing here confirms a burial",
            "does not confirm a burial",
            "did not confirm a burial",
            "without a burial",
            "没有埋葬",
            "没有安葬",
            "没有确认埋",
            "未确认埋",
            "不能确认埋",
            "没有找到埋",
            "没有找到安葬",
        )
    )
    chinese_subject = any(cue in lowered for cue in ("科比特", "沃尔特", "corbitt"))
    chinese_burial = any(cue in lowered for cue in ("被埋", "埋在", "安葬", "葬在"))
    chinese_place = any(cue in lowered for cue in ("地下室", "地窖", "那栋房子", "房子", "宅邸"))
    if chinese_subject and chinese_burial and chinese_place and not negated_burial:
        return True
    buried_as_paperwork = re.search(
        r"\bburied\s+in\s+(?:the\s+|a\s+|an\s+)?(?:routine|paperwork|records?(?:\s+maze)?|files?|paper|ledgers?|archives?|indexes?|remnants?)\b",
        lowered,
    )
    buried_by_paperwork = re.search(
        r"\bburied\s+by\s+(?:the\s+|a\s+|an\s+)?(?:same\s+)?(?:handling|paperwork|records?|files?|bureaucracy|indexing)\b",
        lowered,
    )
    if (
        contains_word(lowered, "buried")
        and not negated_burial
        and not buried_as_paperwork
        and not buried_by_paperwork
    ):
        for match in re.finditer(r"\bburied\b", lowered):
            window = lowered[max(0, match.start() - 100) : match.end() + 100]
            if re.search(
                r"\bburied\s+by\s+(?:the\s+|a\s+|an\s+)?(?:same\s+)?(?:handling|paperwork|records?|files?|bureaucracy|indexing)\b",
                window,
            ):
                continue
            if any(
                anchor in window
                for anchor in (
                    "corbitt",
                    "body",
                    "corpse",
                    "remains",
                    "basement",
                    "cellar",
                    "earth",
                    "grave",
                    "coffin",
                    "interred",
                )
            ):
                return True
    if contains_word(lowered, "burial") and not negated_burial:
        for match in re.finditer(r"\bburial\b", lowered):
            window = lowered[max(0, match.start() - 120) : match.end() + 120]
            if re.search(r"\bnot\s+(?:a\s+)?(?:full\s+)?(?:record,\s*)?burial\s+note\b", window):
                continue
            if any(
                anchor in window
                for anchor in (
                    "corbitt",
                    "walter",
                    "basement",
                    "cellar",
                    "house",
                    "body",
                    "corpse",
                    "remains",
                    "grave",
                    "coffin",
                    "interred",
                    "record states",
                    "record stating",
                    "record said",
                    "chapel record",
                )
            ):
                return True
    return False


def positive_burial_or_basement_clue_visible(text: str) -> bool:
    adjudicated = adjudicated_visible_text(text)
    return explicit_burial_clue_visible(adjudicated) or basement_actionable_lead_visible(adjudicated)


def hall_records_court_police_followup_visible(text: str) -> bool:
    lowered = text.lower()
    has_failed_or_inconclusive_result = has_failure(text) or any(
        cue in lowered
        for cue in (
            "inconclusive",
            "no clean",
            "no specific",
            "no decisive",
            "does not produce",
            "not produce",
            "failed hall",
        )
    )
    if not has_failed_or_inconclusive_result:
        return False
    has_destination = any(
        cue in lowered
        for cue in (
            "higher courts",
            "the courts",
            "county files",
            "commonwealth files",
            "central police station",
            "police station",
            "police records",
            "court files",
            "court records",
        )
    )
    has_reason = any(
        cue in lowered
        for cue in (
            "litigation",
            "criminal proceedings",
            "criminal proceeding",
            "official trouble",
            "official intervention",
            "raid",
            "raids",
            "serious",
        )
    )
    return has_destination and has_reason


def hall_records_search_unresolved_visible(text: str) -> bool:
    lowered = text.lower()
    if has_success(text) or has_failure(text):
        return False
    if not any(cue in lowered for cue in ("hall of records", "municipal", "probate", "ledger")):
        return False
    if not any(cue in lowered for cue in ("you work", "receives you", "bound volumes", "cross-index", "moving from title")):
        return False
    resolved_cues = (
        "turn up",
        "find ",
        "michael thomas",
        "walter",
        "chapel",
        "no useful",
        "does not yield",
        "failure",
        "success",
        "成功",
        "失败",
    )
    return not any(cue in lowered for cue in resolved_cues)


def hall_records_success_without_specific_facts_visible(text: str) -> bool:
    lowered = text.lower()
    if not has_success(text):
        return False
    if any(
        cue in lowered
        for cue in (
            "newspaper",
            "boston globe",
            "globe",
            "clippings morgue",
            "clipping",
            "clippings",
            "newsprint",
        )
    ):
        return False
    if not any(
        cue in lowered
        for cue in (
            "hall of records",
            "municipal",
            "probate",
            "property",
            "title",
            "档案",
            "房产",
            "遗嘱",
        )
    ):
        return False
    if re.search(r"\b(?:18|19|20)\d{2}\b", text):
        return False
    if any(
        cue in lowered
        for cue in (
            "michael thomas",
            "chapel of contemplation",
            "walter",
            "沃尔特",
            "遗体",
            "宅邸之内",
            "buried",
            "burial",
            "raid",
            "children",
            "police officers",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "property ownership",
            "former owners",
            "probate",
            "estate execution",
            "public legal records",
            "traceable line",
            "records that can be checked",
            "房产归属",
            "旧业主更替",
            "遗嘱认证",
            "遗产执行",
            "公开法律记录",
            "浮出水面",
            "正确的线头",
            "同一条可追索",
            "可逐页核对",
        )
    )


def official_records_success_without_contents_visible(text: str) -> bool:
    lowered = text.lower()
    if not has_success(text):
        return False
    has_official_source = any(
        cue in lowered
        for cue in (
            "court",
            "police",
            "central police",
            "higher court",
            "法院",
            "警局",
            "警察局",
            "高等法院",
            "中央警察局",
        )
    )
    if not has_official_source:
        return False
    has_access_only = any(
        cue in lowered
        for cue in (
            "access to",
            "file route",
            "references open",
            "indexes open",
            "got access",
            "relevant references",
            "拿到了查阅",
            "门路",
            "打开了入口",
            "不再只是隔着柜台",
            "记录牵了进来",
            "索引和档案",
        )
    )
    has_boundary_map_only = any(
        cue in lowered
        for cue in (
            "address-only requests do not open",
            "protected files",
            "protected personal matters",
            "sealed or privileged material",
            "public office counters",
            "lawful channels",
            "legal boundaries",
            "proper trail",
            "firmer map of the paper trail",
            "next legal public source",
        )
    )
    if not has_access_only and not has_boundary_map_only:
        return False
    return not official_record_content_assertion_visible(lowered)


def official_records_boundary_points_to_newspaper_visible(text: str) -> bool:
    lowered = text.lower()
    if not official_records_success_without_contents_visible(text):
        return False
    if not any(cue in lowered for cue in ("newspaper", "morgue", "archive trail", "archives")):
        return False
    return any(
        cue in lowered
        for cue in (
            "next legal public source",
            "public office counters",
            "address-only requests do not open",
            "protected files",
            "sealed or privileged",
            "legal boundaries",
        )
    )


def official_record_content_assertion_visible(lowered: str) -> bool:
    strong_content_cues = (
        "after affidavits",
        "affidavits concerning",
        "affidavits connected",
        "police raided",
        "police raid tied",
        "raid turned violent",
        "raid, the deaths",
        "the raid, the deaths",
        "raid, deaths",
        "both police and cult",
        "cult members were killed",
        "police were killed",
        "was implicated",
        "was imprisoned",
        "imprisonment and escape",
        "later escaped",
        "allowed to inspect",
        "constrained record trail",
        "real notes in hand",
        "record states",
        "record indicates",
        "file says",
        "file indicates",
        "surviving record",
        "missing children led to",
        "disappearances of children",
        "档案显示",
        "案卷显示",
        "案卷写明",
        "记录显示",
        "记录写明",
        "警方突袭",
        "警察突袭",
        "突袭中",
        "突袭导致",
        "警员死亡",
        "警察死亡",
        "成员死亡",
        "儿童失踪",
        "孩子失踪",
        "被牵连",
        "被拘押",
        "被监禁",
        "后来逃脱",
        "之后逃脱",
        "逃狱",
    )
    return any(cue in lowered for cue in strong_content_cues)


def no_new_visible_result(text: str) -> bool:
    lowered = text.lower()
    if (
        "没有新的公开通路" in lowered
        and "需要继续结算" in lowered
        and any(cue in lowered for cue in ("伤害", "警报", "当前位置", "退路"))
    ):
        return True
    return any(
        cue in lowered
        for cue in (
            "无新增可公开",
            "无新增玩家可见",
            "无新增可见",
            "no new public",
            "no new player-visible",
            "no additional public",
        )
    ) and any(cue in lowered for cue in ("事实", "fact", "facts", "mechanical"))


def hall_records_failure_visible(text: str) -> bool:
    lowered = text.lower()
    if not has_failure(text):
        return False
    if any(cue in lowered for cue in ("basement", "cellar", "crawl space", "crawl-space")):
        return False
    return any(
        cue in lowered
        for cue in (
            "hall of records",
            "municipal records",
            "records office",
            "property title",
            "probate",
            "library use",
            "corbitt name",
            "macario",
        )
    )


def newspaper_points_to_official_records_visible(text: str) -> bool:
    lowered = text.lower()
    if not has_success(text):
        return False
    has_newspaper_source = any(
        cue in lowered
        for cue in (
            "newspaper",
            "globe",
            "morgue",
            "clipping",
            "clippings",
            "newsprint",
        )
    )
    if not has_newspaper_source:
        return False
    has_official_target = any(
        cue in lowered
        for cue in (
            "police files",
            "police records",
            "higher court",
            "higher-court",
            "court records",
            "court files",
            "serious police",
        )
    )
    if not has_official_target:
        return False
    return any(
        cue in lowered
        for cue in (
            "stronger next line",
            "best next lead",
            "next lead",
            "points you toward",
            "points toward",
            "would likely have generated",
            "outside the real-estate",
            "outside the real estate",
            "not just in deed books",
        )
    )


def newspaper_house_history_visible(text: str) -> bool:
    lowered = text.lower()
    if not has_success(text):
        return False
    if not any(cue in lowered for cue in ("newspaper", "globe", "morgue", "clipping", "clippings")):
        return False
    return any(
        cue in lowered
        for cue in (
            "house-history pattern",
            "deaths",
            "crippling accidents",
            "illness in the household",
            "suicide",
            "macario family",
            "public history",
            "printed incident",
            "incidents associated with the house",
        )
    )


def newspaper_archive_access_tone_clarification_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("newspaper", "globe", "archive", "morgue", "clippings")):
        return False
    if not any(
        cue in lowered
        for cue in (
            "gatekeeper",
            "editor",
            "access to the files",
            "press for access",
            "access is controlled",
            "legitimate access",
            "custody over the files",
        )
    ):
        return False
    skill_hits = sum(
        1
        for cue in (
            "professional",
            "reasonableness",
            "charm",
            "intimidation",
            "fast",
            "slippery line",
            "persuasion",
            "persuade",
            "credentials",
            "reasonable request",
            "quick bluff",
            "professional courtesy",
            "polite persuasion",
            "fast-talking",
            "pressure",
            "blunt pressure",
            "courtesy",
            "presses politely",
            "flatters",
            "bluffs",
            "press credentials",
            "bully",
        )
        if cue in lowered
    )
    if ("do you have her lean on" in lowered or "how she does it matters" in lowered) and skill_hits >= 3:
        return True
    if (
        ("how does she try to get past" in lowered or "does she lean on" in lowered)
        and " or " in lowered
        and skill_hits >= 2
    ):
        return True
    if (
        ("how does evelyn try to get in" in lowered or "tell me her approach" in lowered)
        and ("if she leans" in lowered or "if she tries" in lowered)
        and skill_hits >= 2
    ):
        return True
    if (
        ("how does evelyn approach" in lowered or "how does she approach" in lowered)
        and ("if she" in lowered or " or " in lowered)
        and skill_hits >= 2
    ):
        return True
    return any(
        cue in lowered
        for cue in (
            "what tone does",
            "what tone do",
            "how does she press",
            "how does he press",
            "how do you press",
            "handles the gatekeeper",
            "how does evelyn approach that",
        )
    )


def entry_available_visible(text: str) -> bool:
    lowered = text.lower()
    if unproven_exterior_lock_fit_visible(text):
        return False
    if open_exterior_threshold_affordance_visible(text):
        return True
    if (
        any(
            cue in lowered
            for cue in (
                "door in front of her is shut but unlocked",
                "door in front of you is shut but unlocked",
                "door is shut but unlocked",
                "door sits shut",
                "threshold itself looks still",
            )
        )
        and any(
            cue in lowered
            for cue in (
                "key turned",
                "key has turned",
                "lock has yielded",
                "lock yielded",
                "unlocked",
            )
        )
        and any(
            cue in lowered
            for cue in (
                "retreat path is good",
                "retreat path",
                "way back",
                "withdraw the same way",
                "exit still behind",
                "route remain",
            )
        )
    ):
        return True
    if (
        any(
            cue in lowered
            for cue in (
                "house is shut, but not sealed",
                "shut, but not sealed off",
                "not sealed off from you",
            )
        )
        and any(
            cue in lowered
            for cue in (
                "door close enough to test",
                "door is close enough to test",
                "door close enough for you to test",
            )
        )
        and ("key" in lowered or "knott" in lowered)
        and any(
            cue in lowered
            for cue in (
                "way back is open",
                "retreat path",
                "open daylight",
                "away from the lock",
            )
        )
    ):
        return True
    if re.search(r"\bkeys?\b.{0,80}\bfits?\b", lowered) or re.search(
        r"\bfits?\b.{0,80}\bkeys?\b", lowered
    ):
        return True
    if re.search(r"\bkey\b.{0,80}\bmeets\b.{0,80}\b(cleanly|lock)\b", lowered):
        return True
    key_or_lock_matched = any(
        cue in lowered
        for cue in (
            "keys do match",
            "key does match",
            "keys match",
            "key matches",
            "keys fit",
            "key meets it cleanly",
            "key meets the lock cleanly",
            "one of them does match",
            "lock accepts it",
            "matched key in hand",
            "key ring is no bluff",
            "matched to knott",
            "matched to the key",
            "matched to knott's key",
            "accepted knott's key",
            "accepted knott’s key",
            "proven to take knott",
            "confirmed to accept knott",
            "lock now proven to take",
            "lock now confirmed to accept",
        )
    )
    lock_opens = any(
        cue in lowered
        for cue in (
            "lock actually turning",
            "lock turning",
            "mechanical click",
            "mechanical give",
            "click and yield",
            "it yields",
            "it yielded",
            "lock yields",
            "lock yielded",
            "can be opened",
            "can be entered",
            "door inward only a handspan",
            "door is only open by about a handspan",
            "only open by about a handspan",
            "opened only slightly",
            "opened just enough",
            "open a handspan",
            "open handspan",
            "standing open a handspan",
            "narrow black gap",
        )
    )
    if key_or_lock_matched and lock_opens:
        return True
    door_cracked_open = "door" in lowered and any(
        cue in lowered
        for cue in (
            "door inward only a handspan",
            "door is only open by about a handspan",
            "only open by about a handspan",
            "opened only slightly",
            "opened just enough",
            "open a handspan",
            "standing open a handspan",
            "narrow opening",
            "narrow black gap",
        )
    )
    if door_cracked_open and any(
        cue in lowered
        for cue in ("threshold", "interior", "passage", "flooring just inside", "dim entry space")
    ):
        return True
    if any(cue in lowered for cue in ("house can be entered", "entrance is real")) and (
        "key" in lowered or "lock" in lowered or "entrance" in lowered
    ):
        return True
    if "can be opened" in lowered and ("key" in lowered or "lock" in lowered or "entrance" in lowered):
        return True
    if (
        any(cue in lowered for cue in ("one of them does match", "lock accepts it", "matched key in hand"))
        and "door" in lowered
        and any(cue in lowered for cue in ("exit route", "retreat", "entrance", "seam"))
    ):
        return True
    return any(
        cue in lowered
        for cue in (
            "key fits",
            "lock accepts it",
            "matched key in hand",
            "workable entrance",
            "entrance is available",
            "viable entry point",
            "door stands ready",
            "door is ready",
            "door stands open",
        )
    )


def unproven_exterior_lock_fit_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("door", "entrance", "threshold", "access", "way in")):
        return False
    if not any(cue in lowered for cue in ("key", "lock", "keyway", "latch")):
        return False
    if not any(
        cue in lowered
        for cue in (
            "cannot yet prove",
            "cannot prove",
            "can't yet prove",
            "do not yet know",
            "don't yet know",
            "whether this key fits",
            "whether the key fits",
            "whether any key fits",
            "whether the lock",
            "fit well enough to test",
            "fits well enough to test",
            "not yet rewarded with any obvious discovery",
            "house remains closed",
            "remains closed",
            "testing that exterior access further",
            "test that exterior access further",
            "point of testing",
        )
    ):
        return False
    if (
        re.search(r"\bkey\b.{0,80}\bdoes\s+fit\b", lowered)
        or "felt it seat properly" in lowered
        or "seat properly in the cylinder" in lowered
        or "nearest concrete way into the house" in lowered
    ):
        return False
    return not any(
        cue in lowered
        for cue in (
            "lock accepts it",
            "lock has yielded",
            "lock yielded",
            "proven to take",
            "confirmed to accept",
            "click and yield",
            "can be opened",
        )
    )


def open_exterior_threshold_affordance_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(
        cue in lowered
        for cue in (
            "door",
            "doorway",
            "entrance",
            "threshold",
            "entry point",
            "way in",
        )
    ):
        return False
    door_opened = any(
        cue in lowered
        for cue in (
            "opened",
            "standing open",
            "currently open",
            "genuine way into",
            "real way into",
            "open doorway",
            "open door",
            "door is open",
            "door is already open",
            "door remains open",
            "door still open",
            "opened door",
            "opened doorway",
            "opened exterior doorway",
            "open enough to enter",
            "open a handspan",
            "open only a handspan",
            "open only by a handspan",
            "open that careful handspan",
            "already open that careful handspan",
            "ease it open only a handspan",
            "eases it open only a handspan",
        )
    ) or bool(
        re.search(
            r"\b(?:door|doorway|entrance|threshold)\b\s+(?:is|stands|remains|stays|sits|waits|held|currently)?\s*open(?:ed)?\b",
            lowered,
        )
    ) or bool(
        re.search(
            r"\bopen(?:ed)?\s+(?:door|doorway|entrance|threshold)\b",
            lowered,
        )
    )
    if not door_opened:
        return False
    if not any(
        cue in lowered
        for cue in (
            "side",
            "rear",
            "back",
            "front",
            "exterior",
            "outside",
            "daylight",
            "lot",
            "property",
        )
    ):
        return False
    if not any(
        cue in lowered
        for cue in (
            "first step",
            "one visible step",
            "place a first step",
            "space to enter",
            "open enough to enter",
            "accessible",
            "visible space just beyond",
            "dim interior space",
            "first boards inside",
            "boards inside",
            "floorboards inside",
            "floorboards just inside",
            "flooring just inside",
            "strip of interior shadow",
            "narrow gap into the house",
            "narrow gap",
            "first stretch",
            "not immediately blocked",
            "not visibly choked",
            "not obviously collapsed",
            "room for a first step",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "retreat path",
            "way back",
            "back out",
            "exit",
            "withdrawal",
            "daylight behind",
            "daylight is still behind",
            "unbroken retreat",
            "quickest exit",
        )
    )


def interior_started_visible(text: str) -> bool:
    lowered = text.lower()
    if any(
        cue in lowered
        for cue in (
            "not committed yourself inside",
            "without committing inside",
            "rather than committing herself to the threshold",
            "instead of committing herself to the threshold",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "steps just inside",
            "step just inside",
            "stepped inside",
            "steps inside",
            "entry hall",
            "crosses the threshold",
        )
    )


def first_threshold_strip_established_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("threshold", "inside", "interior", "passage", "service entry")):
        return False
    has_first_strip = any(
        cue in lowered
        for cue in (
            "first visible strip",
            "immediate margin",
            "near edge of a dim passage",
            "near edge of a dim passage or service entry",
            "first short stretch",
            "floor just ahead",
            "first step",
        )
    )
    footing_resolved = any(
        cue in lowered
        for cue in (
            "does not sag",
            "does not sag, crack, or drop",
            "it holds",
            "floor holds",
            "no obvious obstruction",
            "nothing at the threshold immediately forces",
            "no immediate rush from within",
        )
    )
    return has_first_strip and footing_resolved


def current_basement_affordance_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("basement", "cellar")):
        return False
    if not any(
        cue in lowered
        for cue in (
            "ahead of you",
            "directly in front",
            "current light and footing",
            "space beyond your light",
            "top reach of the basement descent",
            "clear way back",
            "retreat path",
            "reachable without abandoning your retreat",
            "first place where a closer look",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "boards",
            "bins",
            "low dark stretches",
            "rough storage",
            "concealment",
            "meant less for use",
            "clear space immediately ahead",
            "worth examining",
        )
    )


def current_crawl_space_affordance_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(
        cue in lowered
        for cue in (
            "crawl space",
            "crawl-space",
            "crawlspace",
            "under-space",
            "under space",
            "crawl-space mouth",
            "crawl space mouth",
            "under the house",
        )
    ):
        return False
    if not any(
        cue in lowered
        for cue in (
            "where you are",
            "from where you are",
            "from where evelyn has stopped",
            "from where evelyn stands",
            "from this cautious angle",
            "one body length",
            "first body length",
            "marked retreat",
            "retreat remains",
            "retreat line",
            "basement stairs",
            "still behind",
            "marked stairs behind",
            "clean line out",
            "near mouth",
            "at the lip",
            "braced at the opening",
            "legs braced toward the opening",
            "half out of the opening",
            "half inside",
            "retreat line clear",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "nothing lunges",
            "nothing rushes",
            "nothing grabs",
            "nothing seizes",
            "nothing has trapped",
            "gives almost nothing back",
            "do not spot any clear remains",
            "no clear remains",
            "no obvious remains",
            "no clear tools",
            "no clear side passage",
            "no immediate side passage",
            "no certain opening",
            "nothing else resolves",
            "space keeps its depth",
            "darkness swallowing",
            "darkness beyond",
            "deeper darkness",
            "continues forward",
            "continuing forward",
            "continues ahead",
            "continuation ahead",
            "continues beyond",
            "under-space continues",
            "empty space enough to keep going",
            "more of the crawl space",
            "more of the crawlspace",
            "edge of your current light",
            "edge of the beam",
            "short tunnel of sight",
            "boards ahead",
            "darker interruption",
            "specific point ahead",
            "low dark space",
            "passage disappearing into black",
            "what lies farther in",
            "what lies beyond those boards",
            "deeper in",
            "cramped sightlines",
            "feeling of concealment",
            "concealed-looking under-space",
            "deliberately shut away",
            "deliberately covered over",
            "continues inward",
            "under-space is not merely",
            "boarded under-space",
            "nearest actionable affordance",
            "what remains uncertain",
            "space remains still",
            "clean retreat",
            "clean way out",
            "stale pocket of darkness",
            "swallow the light",
            "swallowing the light",
            "cramped hollow",
            "less accidental",
            "shifts or surges",
            "immediately shifts",
            "still half out",
            "still at the lip",
        )
    )


def unstable_crawl_space_failure_visible(text: str) -> bool:
    lowered = text.lower()
    if not has_failure(text):
        return False
    if not any(
        cue in lowered
        for cue in (
            "crawl space",
            "crawl-space",
            "under-space",
            "crawl-space mouth",
            "crawl space mouth",
            "narrow opening",
            "缝隙",
            "狭窄空间",
            "狭口",
            "地下室地面",
            "刻意藏",
        )
    ):
        return False
    if not any(
        cue in lowered
        for cue in (
            "collapse",
            "collapses",
            "collapsed",
            "loose dirt",
            "unstable",
            "threatens to trap",
            "塌",
            "塌松",
            "碎裂",
            "土层",
            "碎屑",
            "细灰",
            "看不清",
            "卡住",
            "不可靠",
            "不稳",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "retreat",
            "line back",
            "back out",
            "pulls back",
            "pulled back",
            "退路",
            "身后敞",
            "收回",
            "带出来",
            "找回地下室地面",
        )
    )


def crawl_space_withdrawn_stair_position_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(
        cue in lowered
        for cue in (
            "crawl-space",
            "crawl space",
            "crawl-space mouth",
            "crawl space mouth",
            "opening beyond the broken boards",
            "broken boards",
            "dark under-space",
            "dark gap",
            "low opening",
        )
    ):
        return False
    if not any(
        cue in lowered
        for cue in (
            "basement stairs",
            "cellar stairs",
            "marked stairs",
            "stair position",
            "stair-side position",
            "stairs behind",
            "way back to the stairs",
            "exit is not being blocked",
            "retreat is simple",
            "retreat to the basement stairs",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "nothing lunges",
            "nothing slides out",
            "nothing follows",
            "remains unresolved",
            "contained at a distance",
            "breathing room",
            "retreat is simple and clean",
            "clean retreat",
            "not being blocked",
            "abandoning it",
            "taking the stairs back out",
        )
    )


def crawl_space_inner_boundary_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("crawl-space", "crawl space", "crawlspace", "under-space")):
        return False
    if not any(cue in lowered for cue in ("wall", "earth", "dirt", "timber", "carving", "lettering")):
        return False
    if not any(
        cue in lowered
        for cue in (
            "concealed inner boundary",
            "hidden edge",
            "wall does not read as solid",
            "does not read as solid earth",
            "straighter line",
            "change in texture",
            "packed earth meets something",
            "faint suggestion of a concealed",
            "small inconsistencies",
            "boundary behind the rough surface",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "no bones",
            "no loose objects",
            "no ritual cache",
            "not an opening",
            "not proof of a chamber",
            "retreat",
            "stairs",
            "braced",
            "from where",
        )
    )


def outside_after_crawl_space_exit_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(
        cue in lowered
        for cue in (
            "out of the house",
            "open air",
            "outside",
            "old corbitt place sits",
            "house itself in front",
            "back into the open",
        )
    ):
        return False
    if not any(
        cue in lowered
        for cue in (
            "photographs",
            "notes",
            "time passes",
            "came out",
            "withdrew",
            "withdrawal",
            "door she came out",
            "entrance remains",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "crawlspace",
            "crawl-space",
            "crawl space",
            "whatever lies below",
            "interior she withdrew from",
            "house standing between",
            "nearest way back",
        )
    )


def active_blade_threat_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("knife", "dagger", "blade")):
        return False
    active_cue = any(
        cue in lowered
        for cue in (
            "flying knife",
            "knife moving",
            "knife moves",
            "knife is ahead",
            "knife is still",
            "knife comes",
            "knife jerks",
            "blade jerks",
            "blade lunges",
            "blade comes",
            "blade is still",
            "blade is moving",
            "moving on its own",
            "self-moving",
            "moves first",
            "blade moves",
            "nearest immediate threat",
            "active and in reach",
            "close and active",
            "under direct pressure",
            "unnatural darting motion",
            "moving with no hand",
            "no hand on it",
        )
    ) or bool(
        re.search(
            r"\b(?:knife|dagger|blade)\b.{0,80}\b(?:jerks?|lunges?|moves?|moving|flies|flying|darts?|attacks?|strikes?|pursues?)\b",
            lowered,
        )
        or re.search(
            r"\b(?:jerks?|lunges?|moves?|moving|flies|flying|darts?|attacks?|strikes?|pursues?)\b.{0,80}\b(?:knife|dagger|blade)\b",
            lowered,
        )
        or re.search(
            r"\b(?:knife|dagger|blade)\b.{0,80}\b(?:immediate threat|under direct pressure|close and active)\b",
            lowered,
        )
        or re.search(
            r"\b(?:immediate threat|under direct pressure|close and active)\b.{0,80}\b(?:knife|dagger|blade)\b",
            lowered,
        )
    )
    if not active_cue:
        return False
    historical_only = any(
        cue in lowered
        for cue in (
            "suicide by kitchen knife",
            "public history",
            "has been in print",
            "file paints a grim pattern",
            "clipping",
            "clippings",
            "newspaper",
            "real-estate notice",
            "real estate notice",
        )
    )
    current_cue = any(
        cue in lowered
        for cue in (
            "immediate threat",
            "moving on its own",
            "self-moving",
            "flying knife",
            "knife comes",
            "knife jerks",
            "blade jerks",
            "blade lunges",
            "blade is moving",
            "no hand on it",
        )
    ) or bool(
        re.search(
            r"\b(?:knife|dagger|blade)\b.{0,80}\b(?:immediate threat|under direct pressure|close and active)\b",
            lowered,
        )
        or re.search(
            r"\b(?:immediate threat|under direct pressure|close and active)\b.{0,80}\b(?:knife|dagger|blade)\b",
            lowered,
        )
    )
    return current_cue or not historical_only


def active_blade_threat_after_defense_visible(text: str) -> bool:
    lowered = text.lower()
    if not active_blade_threat_visible(text):
        return False
    return any(
        cue in lowered
        for cue in (
            "retreat",
            "way out",
            "exit",
            "marked exit",
            "path out",
            "between you and the deeper interior",
            "route forward",
            "clear way to fall back",
        )
    )


def upper_route_visible(text: str) -> bool:
    lowered = visible_cue_text(text)
    if current_basement_affordance_visible(text) or current_crawl_space_affordance_visible(text):
        return False
    if any(
        cue in lowered
        for cue in (
            "interior keys",
            "interior locks",
            "fit upstairs rooms",
            "upstairs rooms or old interior locks",
            "may fit upstairs",
            "might fit upstairs",
        )
    ) and not any(
        cue in lowered
        for cue in (
            "entry hall",
            "ground floor",
            "from the threshold",
            "inside the house",
            "stairs upward",
            "staircase",
            "landing",
            "route upward",
        )
    ):
        return False
    if "bedroom doors" in lowered and not any(cue in lowered for cue in ("upstairs", "stairs", "landing")):
        return False
    if basement_route_priority_visible(text):
        return False
    if any(cue in lowered for cue in ("basement", "cellar")) and any(
        cue in lowered
        for cue in (
            "without going upstairs",
            "not upstairs",
            "not the main stair rising upward",
            "committing yourself down the stairs",
            "route downward",
            "strongest physical hint",
            "points downward",
            "lower access",
            "house quietly breathing from below",
            "downward route matters more",
            "route matters more",
            "buried in the basement",
            "basement of his own house",
            "basement route matters more",
            "cellar route matters more",
            "lower part of the house announces",
            "shut-in hint below",
            "suggests cellar space",
            "cellar space rather than",
            "downward chill",
            "breathe from below",
        )
    ):
        return False
    if any(cue in lowered for cue in ("basement", "cellar")) and any(
        cue in lowered
        for cue in (
            "stairs and marked retreat",
            "marked retreat route still behind",
            "stairs behind her",
            "stairs behind you",
            "retreat route still behind",
            "marked return still behind",
            "wedged door and your marked return",
            "foot of the stairs",
            "foot of the steps",
            "lower steps",
        )
    ) and not any(cue in lowered for cue in ("upstairs", "upper floor", "landing", "bedroom")):
        return False
    return (
        "upper floor" in lowered
        or contains_word(lowered, "upstairs")
        or contains_word(lowered, "stairs")
        or contains_word(lowered, "staircase")
        or contains_word(lowered, "landing")
        or contains_word(lowered, "bedroom")
    )


def basement_route_priority_visible(text: str) -> bool:
    lowered = visible_cue_text(text)
    if not any(cue in lowered for cue in ("basement", "cellar")):
        return False
    return any(
        cue in lowered
        for cue in (
            "clearest one is the way down",
            "clearest route is the way down",
            "clearest physical route is the way down",
            "way down: the basement door",
            "way down: the cellar door",
            "basement door or stair access is a real, present route",
            "basement door or stair access is a real present route",
            "cellar door or stair access is a real, present route",
            "cellar door or stair access is a real present route",
            "downward way toward the basement",
            "downward way toward the cellar",
            "way down toward the basement",
            "way down toward the cellar",
            "cellar direction is the one",
            "basement route now standing out",
            "basement route now stands out",
            "basement route stands out more strongly",
            "basement route stands out",
            "more suspicious downward access",
            "basement route now standing out as the most meaningful lead",
            "basement route now stands out as the most meaningful lead",
            "most meaningful lead inside",
            "lower part of the house may matter more",
            "lower part of the house may matter",
            "faint cellar taint",
            "below-house smell",
            "does not belong to the upper rooms",
            "thinner, cooler pull",
            "best fits the faint damp-earth smell",
            "matching the lead you already uncovered from the chapel records",
            "matching the lead from the chapel records",
        )
    )


def basement_route_visible(text: str) -> bool:
    lowered = visible_cue_text(text)
    no_route_patterns = (
        r"\bno\s+(?:clear|obvious|unmistakable|trustworthy|definite)\s+(?:route|way|access|stair|stairs)\s+(?:downstairs|downward|to\s+(?:the\s+)?(?:basement|cellar))\b",
        r"\bno\s+(?:clear|obvious|unmistakable|trustworthy|definite)\s+(?:route|way|access|stair|stairs)\s+(?:downstairs|downward|to\s+(?:the\s+)?(?:basement|cellar))\s+presents\b",
        r"\b(?:does|do)\s+not\s+(?:find|reveal|present|yield)\s+(?:a\s+)?(?:clear|obvious|unmistakable|trustworthy|definite)?\s*(?:route|way|access|stair|stairs)\s+(?:downstairs|downward|to\s+(?:the\s+)?(?:basement|cellar))\b",
    )
    if any(re.search(pattern, lowered) for pattern in no_route_patterns):
        return False
    if basement_route_priority_visible(text):
        return True
    positive_patterns = (
        r"\b(?:route|way|access|stair|stairs|steps|door|doorway|stairhead)\s+(?:downstairs|downward|down|to\s+(?:the\s+)?(?:basement|cellar))\b",
        r"\b(?:downstairs|downward|down)\s+(?:route|way|access|stair|stairs|steps|door|doorway)\b",
        r"\b(?:basement|cellar)\s+(?:route|way|access|stair|stairs|steps|door|doorway|stairhead)\b",
        r"\b(?:route|way|access|stair|stairs|steps|door|doorway|stairhead)\s+(?:is|are)\s+(?:reachable|visible|clear|identified|found)\b",
    )
    return any(re.search(pattern, lowered) for pattern in positive_patterns)


def failed_basement_stair_position_visible(text: str) -> bool:
    lowered = visible_cue_text(text)
    if not has_failure(text):
        return False
    if not any(cue in lowered for cue in ("basement", "cellar", "地下室", "地窖")):
        return False
    if not any(cue in lowered for cue in ("stairs", "stair", "steps", "risers", "楼梯", "台阶", "木阶", "阶梯")):
        return False
    return any(
        cue in lowered
        for cue in (
            "partway down",
            "part way down",
            "partway to the basement floor",
            "partway to the cellar floor",
            "halfway down",
            "half-way down",
            "lower part of the stairs",
            "lower part of the stair",
            "lower part of the steps",
            "lower part of the cellar stairs",
            "lower part of the basement stairs",
            "from the stairs",
            "on the stairs",
            "still partway",
            "route back remains marked and open behind you",
            "route back remains marked and open",
            "stairs have held so far",
            "cellar proper lies just ahead",
            "cellar proper lies ahead",
            "cellar proper lies just ahead of your light",
            "basement proper lies just ahead",
            "basement proper lies ahead",
            "marked retreat behind",
            "marked door still open above",
            "open door and her marked retreat",
            "cellar below",
            "dark below",
            "cellar spreading out ahead",
            "楼梯口",
            "楼梯井",
            "台阶间",
            "木阶",
            "差点失去平衡",
            "几乎让你失去平衡",
            "手帕仍拴",
            "门缝也还替你留着",
            "这条向下的路",
            "地下室口",
        )
    )


def basement_exit_reachable_after_failed_recovery_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("basement", "cellar")):
        return False
    if not any(cue in lowered for cue in ("door", "doorframe", "doorway", "threshold", "top landing")):
        return False
    return any(
        cue in lowered
        for cue in (
            "open basement door is just behind",
            "open basement door just behind",
            "open basement door remains above",
            "open basement door remains above you",
            "open basement doorway above",
            "open basement doorway behind",
            "open basement doorway behind you",
            "open basement door you wedged",
            "door remains above you",
            "door is just behind",
            "door just behind",
            "handkerchief marker still at the threshold",
            "handkerchief marker above and back at the threshold",
            "marker above and back at the threshold",
            "handkerchief marker remains visible",
            "handkerchief marker just visible",
            "handkerchief marker is tied",
            "marker still at the threshold",
            "retreating carefully toward the doorway",
            "back toward the open basement door",
            "handkerchief marker above",
            "backing up toward the doorway",
            "escape line intact",
            "toward the open basement doorway",
            "toward the open door and the handkerchief marker",
            "careful step toward the doorway",
            "another careful step toward the doorway",
            "not yet through the doorway",
            "you are still on the stairs",
            "still on the stairs, higher than before",
            "half-turned between cellar and escape",
            "retreat is no longer silent",
            "clearest fixed point",
            "rectangle of safer return",
            "caught yourself hard against the stair rail and the doorframe",
            "catching yourself hard against the stair rail and the doorframe",
        )
    )


def clear_of_open_basement_threshold_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("basement", "cellar")):
        return False
    if not any(
        cue in lowered
        for cue in (
            "clear of the basement threshold",
            "basement door still open",
            "open basement door",
            "stair mouth",
            "flashlight on the stair",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "exterior route",
            "escape line",
            "room to either hold",
            "fall back",
            "nothing surges up",
            "nothing bursts up",
        )
    )


def loosened_basement_gap_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("basement", "cellar")):
        return False
    if not any(
        cue in lowered
        for cue in (
            "loosened gap",
            "newly loosened gap",
            "hidden space here",
            "hidden space",
            "partial collapse",
            "old concealment",
            "rough board",
            "rough boards",
            "packed dirt",
            "stale debris",
            "fouler air",
            "space beneath",
            "space beneath them",
            "cavity",
            "one corner flexes",
            "board gives",
            "boards are not solidly trustworthy",
        )
    ):
        return False
    if not any(
        cue in lowered
        for cue in (
            "skittering",
            "scratching",
            "movement deeper inside",
            "life in the dark",
            "something shifts",
            "something small and dry skitters",
            "skitters deeper",
            "something under there can move",
            "colder, fouler air",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "retreat path",
            "marked stairs",
            "retreating to the marked stairs",
            "stairs is still open",
            "stairs are still open",
            "stairs behind",
        )
    )


def clarified_basement_door_affordance_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("basement", "cellar")):
        return False
    if "door" not in lowered:
        return False
    if not any(
        cue in lowered
        for cue in (
            "most concrete visible affordance is the door",
            "closed, reachable door",
            "door directly in front",
            "door immediately in front",
        )
    ):
        return False
    if not any(cue in lowered for cue in ("safely reachable", "at arm's length", "at arm’s length")):
        return False
    return any(
        cue in lowered
        for cue in (
            "cannot yet prove",
            "still cannot prove",
            "whether it is locked",
            "what lies beyond",
        )
    )


def clarified_concealed_basement_section_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("basement", "cellar")):
        return False
    if loosened_basement_gap_visible(text):
        return False
    named_concealed_space = any(
        cue in lowered
        for cue in (
            "boarded, shadowed section",
            "boarded shadowed section",
            "boarded-off under-space",
            "boarded off under-space",
            "blocked, darker area",
            "blocked darker area",
            "shadowed under-space",
            "darker under-space",
            "partially concealed space",
            "dark, suspicious recess",
            "dark suspicious recess",
            "rough boarding",
        )
    )
    concealment_or_screening = any(
        cue in lowered
        for cue in (
            "less like ordinary storage",
            "less like ordinary basement clutter",
            "more like something concealed",
            "looks concealed",
            "hidden-looking",
            "meant to close something off",
            "deliberately screened off",
            "deliberately hidden",
            "something deliberately hidden",
            "covering something worth finding",
        )
    )
    actionable_commitment = any(
        cue in lowered
        for cue in (
            "actionable thing is this",
            "most actionable feature",
            "concrete affordance",
            "can be approached and examined",
            "approach that boarded section",
            "examine it directly",
            "must be examined",
            "meaningful point of contact",
        )
    )
    retreat_grounding = any(
        cue in lowered
        for cue in (
            "retreat path",
            "stairs back up",
            "staircase out",
            "stairs are behind",
            "stairs behind",
            "farther in than",
            "behind her",
            "behind you",
            "withdrawal clean",
        )
    )
    if named_concealed_space and concealment_or_screening and actionable_commitment and retreat_grounding:
        return True
    if not any(
        cue in lowered
        for cue in (
            "concealed section",
            "hidden barrier",
            "panel-like section",
            "concealed boundary",
            "concealed edge",
            "wall-and-floor junction",
            "rough old boards",
            "rough boards",
            "rough planking",
            "rough planks",
            "old boards",
            "rough storage boards",
            "boarded-over section",
            "boarded over section",
            "boarded-off under-space",
            "boarded off under-space",
            "boarded, shadowed section",
            "shadowed under-space",
            "blocked, darker area",
            "partially concealed space",
            "dark, suspicious recess",
            "boarded irregularity",
            "rough basement boards",
            "disturbed section",
            "section low on the basement wall",
            "patch that has been altered",
            "altered, covered, or loosened",
            "surface sits wrong",
            "hidden seam",
            "narrow seam",
            "rough opening",
            "far wall",
            "more concealing than structural",
            "planking covering",
            "boards concealing",
            "boards do not read as random",
        )
    ):
        return False
    if not any(
        cue in lowered
        for cue in (
            "disturbed earth",
            "scrape marks",
            "cavity beyond",
            "space behind",
            "open space",
            "over open space",
            "under-space",
            "dark under-space",
            "cramped dark space",
            "behind it",
            "gap beyond",
            "shallow recess",
            "recess or opening",
            "covered opening",
            "conceal a cavity",
            "concealed area",
            "altered space",
            "hollower feel",
            "slight hollow beneath",
            "hollow beneath",
            "scrape marks gather",
            "shifted under your probing",
            "laid over something",
            "dry, uneven edge",
            "opening, depth",
            "darker recess",
            "does not read like ordinary storage",
            "not read like ordinary storage",
            "less like ordinary storage",
            "less like ordinary basement clutter",
            "more like something concealed",
            "looks concealed",
            "meant to close something off",
            "not just loose debris",
            "intentionally closed off",
            "deliberately shut away",
            "deliberately screened off",
            "deliberately hidden",
            "something deliberately hidden",
            "less like storage",
            "boarded over on purpose",
            "boundary is real",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "most concrete visible affordance",
            "plainly actionable thing",
            "actionable thing is this",
            "most actionable feature",
            "concrete affordance",
            "can be approached and examined",
            "examine it directly",
            "meaningful point of contact",
            "exact location",
            "a few steps in front",
            "ahead of you",
            "from where you are",
            "from where you are now",
            "relative to your retreat",
            "safely reachable",
            "within reach",
            "way back",
            "retreat path",
            "retreat safely",
            "marked stairs",
            "stairs back up",
            "staircase out",
            "stairs up to the ground floor",
            "stairs behind",
            "nothing blocks the stairs",
            "basement proper",
        )
    )


def opened_basement_crawl_space_visible(text: str) -> bool:
    lowered = text.lower()
    local_basement_context = any(cue in lowered for cue in ("basement", "cellar")) or (
        any(
            cue in lowered
            for cue in (
                "rough board",
                "rough boarding",
                "boards",
                "gap",
                "opening",
                "cavity",
                "under-space",
            )
        )
        and "chapel of contemplation" in lowered
    )
    if not local_basement_context:
        return False
    if not any(
        cue in lowered
        for cue in (
            "opened crawl-space entrance",
            "crawl-space entrance",
            "crawl space",
            "crawl-space",
            "cramped under-space",
            "cramped, dark under-space",
            "cramped dark space",
            "cramped dark space rather than solid wall",
            "dark under-space",
            "hidden gap",
            "hidden seam",
            "concealed edge",
            "gap becomes plain",
            "gap beyond",
            "rough opening",
            "space past the boards",
            "opening does not suddenly move",
            "narrow black opening",
            "cavity is now mapped",
            "wet chill of the cavity",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "chapel of contemplation",
            "carved words",
            "carved lettering",
            "words cut into the inner wall",
            "carved line",
            "words become legible",
        )
    )


def crawl_space_entry_unresolved_visible(text: str) -> bool:
    lowered = text.lower()
    if "crawl space" not in lowered and "crawl-space" not in lowered:
        return False
    if not any(
        cue in lowered
        for cue in (
            "half inside",
            "one body length",
            "first body length",
            "shoulders and light inside",
            "hips still near the lip",
            "upper body committed",
            "body length into the opening",
            "marked opening",
        )
    ):
        return False
    if any(
        cue in lowered
        for cue in (
            "corbitt is visible",
            "visible corbitt",
            "the body is visible",
            "the corpse is visible",
            "blocks your retreat",
            "seizes you",
            "lunges at you",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "continues inward",
            "more under here",
            "details remain",
            "full depth",
            "side branch",
            "darkness beyond",
            "first reach of her light",
            "first reach of your light",
            "retreat line intact",
            "marked opening and basement stairs still behind",
            "can back out now exactly as planned",
            "back out now exactly as planned",
            "commit farther in",
        )
    )


def limited_crawl_space_no_new_leads_withdrawal_visible(text: str) -> bool:
    lowered = text.lower()
    if "crawl space" not in lowered and "crawl-space" not in lowered:
        return False
    if not has_failure(text):
        return False
    if not any(
        cue in lowered
        for cue in (
            "shoulders and light",
            "hips still",
            "half inside",
            "one body length",
            "limited angle",
            "from this limited angle",
        )
    ):
        return False
    has_negated_list = any(
        cue in lowered
        for cue in (
            "do not see",
            "do not catch",
            "do not spot",
            "no obvious",
            "no clear",
            "no immediate",
            "no trustworthy",
            "no certain",
        )
    )
    no_yield_terms = (
        ("remains", "bones", "body"),
        ("tools", "tool"),
        ("side passage", "passage"),
        ("fresh markings", "additional mark", "second mark", "markings"),
        ("trustworthy airflow", "airflow"),
    )
    no_yield_count = sum(
        1 for group in no_yield_terms if any(cue in lowered for cue in group)
    )
    if not has_negated_list:
        return False
    if no_yield_count < 3:
        return False
    if not any(
        cue in lowered
        for cue in (
            "going farther in would mean committing more",
            "would mean committing more of yourself",
            "rather than just looking",
            "commit more of yourself",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "retreat behind you remains open",
            "retreat remains open",
            "retreat line remains open",
            "retreat line clear",
            "marked basement stairs behind you",
        )
    )


def repeated_crawl_space_probe_no_new_facts_visible(text: str) -> bool:
    lowered = text.lower()
    if "crawl space" not in lowered and "crawl-space" not in lowered:
        return False
    if not has_failure(text) and not has_success(text):
        return False
    if any(
        cue in lowered
        for cue in (
            "you find",
            "you found",
            "reveals a",
            "reveals an",
            "there is a body",
            "body lies",
            "remains lie",
            "widening chamber opens",
            "side passage opens",
            "object rests",
            "object lies",
        )
    ):
        return False
    if not any(
        cue in lowered
        for cue in (
            "crawl-space mouth",
            "crawl space mouth",
            "just past the mouth",
            "at the mouth",
            "beyond the opening",
            "beyond the mouth",
            "marked stairs",
            "basement stairs",
            "half-braced in the opening",
            "marked gap",
            "handkerchief line",
            "from where you are",
            "at the crawl-space mouth",
            "marked stairs behind",
            "marked stairs behind you",
        )
    ):
        return False
    negated_fact_cues = (
        "nothing resolves into a new",
        "no new",
        "no fresh",
        "no chamber",
        "no branch",
        "no definite",
        "no visible",
        "no clear",
        "does not show",
        "too faint",
        "gives you no clear answer",
        "does not yield itself",
        "neither does the crawl space become any safer",
    )
    if not any(cue in lowered for cue in negated_fact_cues):
        return False
    no_yield_terms = (
        ("mark", "marking"),
        ("object", "thing"),
        ("remains", "bones", "body"),
        ("branch", "chamber", "turn", "cache", "recess"),
        ("air", "airflow"),
        ("detail", "answer", "lead"),
    )
    no_yield_count = sum(
        1 for group in no_yield_terms if any(cue in lowered for cue in group)
    )
    if no_yield_count < 3 and "nothing resolves into a new" not in lowered:
        return False
    return any(
        cue in lowered
        for cue in (
            "marked stairs still at your back",
            "marked stairs behind",
            "retreat remains open",
            "retreat line remains open",
            "withdraw cleanly",
            "clean retreat",
            "still at your back",
            "poised to back out",
            "back out toward the stairs",
            "basement stairs",
        )
    )


def crawl_space_risk_with_safe_withdrawal_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(
        cue in lowered
        for cue in ("crawl space", "crawl-space", "under-space", "passage")
    ):
        return False
    has_risk = any(
        cue in lowered
        for cue in (
            "rats",
            "rat",
            "tight",
            "confining",
            "unpleasantly",
            "narrows",
            "deeper inside",
            "movement",
        )
    )
    has_safe_withdrawal = any(
        cue in lowered
        for cue in (
            "retreat remains open",
            "retreat line intact",
            "not trapped",
            "can back out",
            "clear way back",
            "back out immediately",
            "retreat route remains open",
        )
    )
    has_sufficient_local_information = any(
        cue in lowered
        for cue in (
            "deliberate human use",
            "ritual use",
            "made to conceal",
            "real concealed passage",
            "continues inward",
            "continues",
        )
    )
    return has_risk and has_safe_withdrawal and has_sufficient_local_information


def crawl_space_mouth_affordance_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("basement", "cellar", "under-space", "crawl space", "crawl-space")):
        return False
    if not any(
        cue in lowered
        for cue in (
            "crawl-space mouth",
            "crawl space mouth",
            "low opening",
            "under-space",
            "cramped opening",
            "mouth of the crawl",
            "opening in front",
            "opening directly in front",
        )
    ):
        return False
    if not any(
        cue in lowered
        for cue in (
            "rough boards",
            "rough boarding",
            "packed dirt",
            "low black gap",
            "darkness continuing",
            "beyond the beam",
            "beyond the reach of",
            "darkness beyond",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "retreat path",
            "retreat is",
            "retreat remains",
            "stairs remain",
            "stairs back",
            "behind you",
            "behind her",
            "way back",
            "way out",
        )
    )


def crawl_space_chapel_wall_reinspection_visible(text: str) -> bool:
    lowered = text.lower()
    if "chapel of contemplation" not in lowered:
        return False
    if not any(
        cue in lowered
        for cue in ("crawl space", "crawl-space", "under-space", "basement", "cellar")
    ):
        return False
    if not any(
        cue in lowered
        for cue in (
            "wall",
            "inner surface",
            "cut marks",
            "carved words",
            "carved lettering",
            "letters",
        )
    ):
        return False
    if any(
        cue in lowered
        for cue in (
            "blocks your retreat",
            "retreat is cut off",
            "retreat gets cut off",
            "seizes you",
            "something lunges at you",
            "then lunges at you",
            "corbitt is visible",
            "visible corbitt",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "nothing immediately lunges",
            "nothing lunges",
            "nothing immediately",
            "nothing shifts",
            "retreat to the basement stairs",
            "basement stairs",
            "retreat remains",
            "air ahead remains",
            "plainly something man-made",
            "deliberately",
        )
    )


def deeper_recess_table_papers_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("paper", "papers", "loose page", "loose pages")):
        return False
    if not any(cue in lowered for cue in ("small table", "little table", "table set off", "table in the corner")):
        return False
    if not any(
        cue in lowered
        for cue in (
            "broken opening",
            "broken wall",
            "breach",
            "gap",
            "deeper recess",
            "darker pocket",
            "foul pocket",
            "beyond the broken wall",
            "mouth of the broken opening",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "not in your hand",
            "not in your hand's reach",
            "not in your hand’s reach",
            "nearest distinct object",
            "might yield information",
            "inspect or take",
            "edge in far enough",
            "retreat remains behind",
            "retreat remains behind you",
        )
    )


def broken_cellar_opening_affordance_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("cellar", "basement", "under-space", "hidden space")):
        return False
    if not any(
        cue in lowered
        for cue in (
            "broken opening",
            "break in the cellar wall",
            "opening in the cellar wall",
            "gap in the cellar wall",
            "breach in the wall",
            "broken gap",
            "jagged break",
            "rough, broken masonry",
        )
    ):
        return False
    if not any(
        cue in lowered
        for cue in (
            "darkness beyond",
            "fouler",
            "sweet",
            "rotten",
            "worse than",
            "bad air",
            "air past",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "close enough",
            "one cautious advance",
            "one careful advance",
            "retreat path",
            "retreat remains",
            "way back",
            "not under her hands yet",
            "not under your hands yet",
        )
    )


def stubborn_concealed_basement_boards_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("basement", "cellar")):
        return False
    if not any(
        cue in lowered
        for cue in (
            "rough boards",
            "old bins",
            "boarded",
            "boards and old bins",
            "crowded by rough boards",
            "crowded with rough boards",
            "cluttered section",
        )
    ):
        return False
    if not any(
        cue in lowered
        for cue in (
            "deliberately shut",
            "meant to be covered",
            "covered rather than",
            "covered or concealed",
            "blocked, hidden",
            "concealment made physical",
            "intentionally shut away",
            "less accidental",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "retreat path is still open",
            "stairs behind",
            "stairs back up",
            "line back to the stairs",
            "not directly across",
            "proper tools",
            "matter of force",
            "cannot yet prove",
            "what remains uncertain",
        )
    )


def documented_stubborn_basement_withdrawal_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("basement", "cellar")):
        return False
    if not any(
        cue in lowered
        for cue in (
            "back on the ground floor",
            "ground floor",
            "retreat itself stays clear",
            "retreat stays clear",
            "stairs",
        )
    ):
        return False
    if not any(
        cue in lowered
        for cue in (
            "rough boards",
            "boarded section",
            "old bins",
            "disturbed floor marks",
            "chalk mark",
            "notebook note",
            "photographs",
            "documented",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "left undisturbed",
            "clean reference point",
            "safer return",
            "what do you do next",
            "what does",
            "route",
            "return",
        )
    )


def documented_basement_return_access_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("basement", "cellar")):
        return False
    if not any(
        cue in lowered
        for cue in (
            "way back down",
            "basement access",
            "route to the basement",
            "leading back to the suspicious area",
            "identified point to revisit",
            "suspicious area",
            "altered stretch",
            "wrong-looking patch",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "photograph",
            "mark",
            "marked",
            "chalk",
            "wrongness",
            "irregularity",
            "rough boards",
            "open route",
            "retreat path",
        )
    )


def prepared_return_crawl_space_ready_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(
        cue in lowered for cue in ("crawl space", "crawl-space", "under-space", "crawlspace")
    ):
        return False
    if not any(cue in lowered for cue in ("basement", "cellar", "under the house")):
        return False
    has_prepared_resources = sum(
        1
        for cue in (
            "better light",
            "stronger light",
            "marked line",
            "rope",
            "line",
            "gloves",
            "tool",
            "tools",
            "levering",
            "prying",
        )
        if cue in lowered
    )
    if has_prepared_resources < 3:
        return False
    if not any(
        cue in lowered
        for cue in (
            "entry route appears unchanged",
            "route appears unchanged",
            "basement stairs remain",
            "crawl-space approach is still there",
            "crawl space approach is still there",
            "back at the point",
            "renewed descent",
            "retreat can be kept clear",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "rough boards",
            "broken boards",
            "damaged boards",
            "crawl-space mouth",
            "crawl space mouth",
            "crawl-space route",
            "crawl space route",
            "crawl-space approach",
        )
    )


def failed_tool_assisted_basement_board_test_visible(text: str) -> bool:
    lowered = text.lower()
    if not has_failure(text):
        return False
    if not any(cue in lowered for cue in ("basement", "cellar")):
        return False
    if not any(cue in lowered for cue in ("tool", "pry", "seam", "edge", "controlled pressure")):
        return False
    if not any(
        cue in lowered
        for cue in (
            "boarded under-space",
            "boarded",
            "rough boards",
            "old boards",
            "boards",
            "under-space",
            "basement wall",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "cannot prove",
            "still there",
            "slips",
            "resists",
            "does not open",
            "what lies behind",
        )
    )


def basement_door_reset_after_crawlspace_visible(text: str) -> bool:
    lowered = text.lower()
    if "crawl space" in lowered or "crawl-space" in lowered:
        return False
    if "basement" not in lowered and "cellar" not in lowered:
        return False
    if "door" not in lowered:
        return False
    if any(
        cue in lowered
        for cue in (
            "back to the ground floor",
            "returned to the ground floor",
            "returns to the ground floor",
            "back up the stairs",
            "withdraws from the crawl",
        )
    ):
        return False
    if not any(
        cue in lowered
        for cue in (
            "ground-floor hall",
            "ground floor hall",
            "under the staircase",
            "beside the stair structure",
            "basement door",
            "cellar door",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "what lies beyond",
            "whether it is locked",
            "closed interior door",
            "plain, closed",
            "until she closes the distance",
        )
    )


def clarified_basement_boards_affordance_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("basement", "cellar")):
        return False
    if not any(
        cue in lowered
        for cue in (
            "rough wooden boards",
            "rough wooden boarding",
            "rough boards",
            "rough planks",
            "mismatched boards",
            "boarded section",
            "rough boarding",
            "boards, bins",
            "boards and bins",
            "rough storage",
            "boards are crude",
            "boarding sits wrong",
            "patch of rough",
            "rough planking",
            "suspicious boarded",
            "boarded, cluttered area",
            "boarded cluttered area",
        )
    ):
        return False
    if not any(
        cue in lowered
        for cue in (
            "cover an opening",
            "covered over",
            "cover something",
            "cover or conceal",
            "used to cover",
            "used to cover or conceal",
            "invites closer inspection",
            "shallow recessed space",
            "covering an opening",
            "covering something",
            "deliberately covering something",
            "concealment",
            "concealed-looking section",
            "screened off",
            "screened darkness",
            "dark under-space",
            "darker under-space",
            "under-space beyond ordinary storage",
            "deeper under-space",
            "beyond ordinary storage",
            "darker under-area",
            "darker concealed area",
            "patches of darkness under the structure",
            "low dark stretches",
            "space beyond your light",
            "meant less for use",
            "less like storage than concealment",
            "clear space immediately ahead",
            "worth examining",
            "first place where a closer look",
            "half-hidden",
            "feels more like concealment",
            "meant to be hidden",
            "more deliberately placed",
            "deliberately fixed",
            "not part of the original structure",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "single most concrete",
            "clearest concrete affordance",
            "immediate, usable fact",
            "nearest meaningful place",
            "thing in front of you",
            "specific affordance",
            "physically reachable",
            "physically approachable",
            "safely reachable",
            "not safely reachable",
            "current position",
            "bottom of the basement stairs",
            "marked stair line",
            "stairs are still directly behind",
            "stairs are directly behind",
            "stairs are behind",
            "stairs behind",
            "retreat path",
            "route behind you remains open",
            "cut off that retreat",
            "way back is still open",
            "way back",
            "clear way back",
            "current light and footing",
            "reachable without abandoning your retreat",
            "toward those boards",
            "room ahead",
            "foot of the stairs",
            "standing just off the foot",
            "from where evelyn has stopped",
            "from where you've stopped",
            "from where you’ve stopped",
        )
    )


def clarified_basement_stair_foot_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("basement", "cellar")):
        return False
    if not any(
        cue in lowered
        for cue in (
            "bottom of the cellar stairs",
            "bottom of the basement stairs",
            "foot of the cellar stairs",
            "foot of the basement stairs",
            "foot of the stairs",
            "stair-foot",
            "last step",
            "last stair",
            "bottom of the steps",
        )
    ):
        return False
    if not any(
        cue in lowered
        for cue in (
            "packed dirt",
            "basement floor",
            "patch of basement floor",
            "foundation wall",
            "foundation walls",
            "stone foundation wall",
            "nearby foundation wall",
            "damp masonry",
            "floor marks",
            "damp staining",
            "dust along the edges",
            "old dust",
            "packed dimness",
            "take in the cellar",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "single most concrete",
            "only firm, concrete",
            "safely reachable",
            "already on it",
            "flashlight range",
            "current position",
            "marked way back behind",
            "cellar ahead",
            "nothing blocks the way",
            "safe first foothold",
            "stairs behind you remain usable",
            "stairs back up remain behind",
            "stairs back up",
            "door above is still marked",
            "press the search",
            "route unchanged",
            "stair remains passable",
            "before committing farther",
            "you are now in the basement",
            "nothing has attacked",
            "closer inspection",
            "first impression",
        )
    )


def clarified_exterior_door_affordance_visible(text: str) -> bool:
    lowered = text.lower()
    if interior_started_visible(text):
        return False
    if not any(
        cue in lowered
        for cue in (
            "side door",
            "rear door",
            "back door",
            "front door",
            "exterior door",
            "exterior entrance",
            "door you tested",
            "stubborn outer door",
            "outer door",
            "exterior lock",
            "key is in an exterior lock",
            "latest actionable point",
            "real entry point",
            "entry point under your hand",
            "door with knott's key",
            "door with knott’s key",
            "key already seated",
            "least exposed lock",
            "exterior access",
            "front access",
            "rear access",
            "side exposure",
        )
    ):
        return False
    if not any(cue in lowered for cue in ("key", "lock", "keyway", "latch", "hardware", "matched to", "proven to take", "marked for it")):
        return False
    return any(
        cue in lowered
        for cue in (
            "safely reachable",
            "directly reachable",
            "close enough to approach",
            "retreat path",
            "exit route",
            "threshold area",
            "in front of you",
            "front threshold",
            "main threshold",
            "entry step",
            "nearest concrete way in",
            "arm's reach",
            "close enough to touch",
            "least exposed",
            "set back from the street",
            "route back",
            "street still behind",
            "behind you",
            "door is the affordance",
            "unknown begins at the lock",
            "can be opened",
        )
    )


def clarified_interior_threshold_affordance_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("doorway", "threshold", "room opening")):
        return False
    if not any(cue in lowered for cue in ("bedroom", "bed", "wardrobe", "room")):
        return False
    return any(
        cue in lowered
        for cue in (
            "safely reachable",
            "landing clear",
            "landing",
            "retreat path",
            "way back",
            "directly off the landing",
        )
    )


def nearby_ground_floor_anomaly_affordance_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(
        cue in lowered
        for cue in (
            "ground floor",
            "first floor",
            "first-floor",
            "hall ahead",
            "front part of the house",
            "first interior stretch",
            "deeper interior",
        )
    ):
        return False
    anomaly_cue = any(
        cue in lowered
        for cue in (
            "something in the hall",
            "single object",
            "disturbed object",
            "displaced piece",
            "disturbance ahead",
            "visible irregularity",
            "one visible irregularity",
            "one thing in view",
            "one thing that stands out",
            "one thing does not",
            "sits wrong",
            "out of place",
            "does not belong",
            "does not match",
            "breaks the house",
            "breaks the room",
            "breaks the general pattern",
            "breaks the scene",
        )
    )
    if not anomaly_cue:
        return False
    position_cue = any(
        cue in lowered
        for cue in (
            "ahead of you",
            "ahead of her",
            "forward of you",
            "forward of her",
            "in front of you",
            "in front of her",
            "between you and",
            "between her and",
            "not deep in the building",
            "not deep in the house",
            "still on your side",
            "still on her side",
            "nearest visible",
            "nearest actionable",
            "within the first",
        )
    )
    retreat_cue = any(
        cue in lowered
        for cue in (
            "retreat path",
            "retreat line",
            "way out",
            "back out",
            "open front door",
            "door behind you",
            "door behind her",
            "behind you",
            "behind her",
            "nothing blocks",
            "nothing presently between you and",
            "nothing presently between her and",
        )
    )
    return position_cue and retreat_cue


def corbitt_body_or_remains_visible(text: str) -> bool:
    lowered = text.lower()

    def negates_body_claim(window: str) -> bool:
        return any(
            re.search(pattern, window)
            for pattern in (
                r"\bno\s+(?:body|corpse|remains|named figure|moving threat)\b",
                r"\bwithout\s+(?:a\s+)?(?:body|corpse|remains|named figure|moving threat)\b",
                r"\bno\s+visible\s+(?:body|corpse|remains|figure|moving threat)\b",
                r"\bnot\s+to\s+(?:a\s+)?(?:body|corpse|remains|named figure)\b",
                r"\bdoes\s+not\s+reveal\s+(?:a\s+)?(?:body|corpse|remains|grave|moving threat)\b",
                r"\bdo\s+not\s+reveal\s+(?:a\s+)?(?:body|corpse|remains|grave|moving threat)\b",
            )
        )

    if negates_body_claim(lowered):
        return False

    explicit_body_patterns = (
        r"\b(?:walter\s+corbitt|corbitt)(?:'s)?\s+(?:body|corpse)\b",
        r"\b(?:body|corpse)\s+of\s+(?:walter\s+)?corbitt\b",
        r"\b(?:walter's|his)\s+(?:body|corpse)\b",
        r"\b(?:walter\s+corbitt|corbitt)'s\s+remains\b",
        r"\bremains\s+of\s+(?:walter\s+)?corbitt\b",
        r"\b(?:walter's|his)\s+remains\b",
    )
    if any(re.search(pattern, lowered) for pattern in explicit_body_patterns):
        return True

    for match in re.finditer(r"\bwalter\s+corbitt\b", lowered):
        window = lowered[max(0, match.start() - 120) : match.end() + 120]
        if negates_body_claim(window):
            continue
        if any(
            cue in window
            for cue in (
                "attacks",
                "attack",
                "moves",
                "moving",
                "stirs",
                "rises",
                "sits up",
                "lunges",
                "visible threat",
            )
        ):
            return True

    for match in re.finditer(r"\b(?:body|corpse|remains|source|lair|thing)\b", lowered):
        term = match.group(0)
        window = lowered[max(0, match.start() - 120) : match.end() + 120]
        if negates_body_claim(window):
            continue
        if "religious body" in window or "named religious body" in window or "body of records" in window:
            continue
        has_corbitt_person = ("corbitt" in window or "walter" in window) and not any(
            cue in window for cue in ("corbitt house", "corbitt name", "corbitt surname")
        )
        if not has_corbitt_person:
            continue
        if term in ("body", "corpse"):
            return True
        if term == "remains" and any(
            cue in window
            for cue in (
                "corbitt's remains",
                "walter's remains",
                "his remains",
                "human remains",
                "physical remains",
                "remains of corbitt",
                "remains of walter corbitt",
            )
        ):
            return True
        if term in ("source", "lair", "thing") and any(
            cue in window
            for cue in (
                "moving",
                "attacks",
                "attack",
                "threat",
                "stirs",
                "rises",
                "sits up",
                "lunges",
            )
        ):
            return True
    return False


def moving_bedroom_threat_visible(text: str) -> bool:
    narrative_text = re.sub(r"\[roll\].*?\[/roll\]", " ", text, flags=re.IGNORECASE | re.DOTALL)
    lowered = narrative_text.lower()
    if not any(
        contains_word(narrative_text, cue)
        for cue in ("bed", "bedframe", "bedclothes", "bedding", "mattress", "furniture")
    ):
        return False
    positive_cues = (
        "violent lurch",
        "self-driven lurch",
        "surging toward the doorway",
        "sudden, violent",
        "violent, sudden",
        "violent, sudden heave",
        "moves by itself",
        "moved by itself",
        "moving by itself",
        "moves on its own",
        "moved on its own",
        "moving on its own",
        "capable of moving on its own",
        "the bed stirs",
        "bed stirs",
        "bed stirred",
        "mattress shifts",
        "mattress shifted",
        "without any human cause",
        "movement that does not match",
        "impossible movement",
        "plainly impossible",
        "room answers",
        "not caused by your stick",
        "hard scraping jerk",
        "hard scrape",
        "jerked and scraped",
        "bed then jerked",
        "lurching bed",
        "attack came from the bed",
        "actively dangerous",
        "turn a cautious probe into an attack",
        "thing has moved by itself",
        "paper moved on its own",
        "without any draft",
    )
    negation_patterns = (
        r"\bnothing(?:\s+\w+){0,4}\s+(?:moves|moved|moving|lunges|rushes)\b",
        r"\bnothing(?:\s+\w+){0,4}\s+(?:stirs|stirred|shifts|shifted)\b",
        r"\bno\s+(?:bedclothes?|wardrobe\s+door|object|furniture|bed|mattress)(?:\s+\w+){0,4}\s+(?:moves|moved|moving|lunges|rushes|twitches?|swings?|stirs|stirred|shifts?|shifted)\b",
        r"\bno\s+(?:new\s+|fresh\s+|further\s+|sudden\s+)?(?:movement|threat|rushing threat|stir|stirs|shift|shifts)\b",
        r"\b(?:do|does)\s+not\s+(?:immediately\s+)?present\s+movement\b",
        r"\bnot\s+moving\s+(?:on its own|by itself)\b",
        r"\bwithout\s+(?:new\s+|fresh\s+)?movement\b",
    )
    for cue in positive_cues:
        start = lowered.find(cue)
        while start != -1:
            window = lowered[max(0, start - 90) : start + len(cue) + 90]
            if not any(re.search(pattern, window) for pattern in negation_patterns):
                return True
            start = lowered.find(cue, start + 1)
    return False


def bedroom_landing_crash_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("bed", "bedroom", "furniture")):
        return False
    if not any(cue in lowered for cue in ("landing", "stairs", "doorway", "threshold")):
        return False
    if not any(
        cue in lowered
        for cue in (
            "you are down",
            "sprawled",
            "hurled",
            "catches your hip",
            "catches your shin",
            "near the head of the stairs",
            "stinging",
            "sting in your side",
            "clips after it",
            "slams through the doorway",
            "bursts through the doorway",
            "jams half across the threshold",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "moving with purpose",
            "no human hand",
            "bed does not stop",
            "furniture moving",
            "lunging bed",
            "bed jams",
            "old dust",
        )
    )


def confirmed_impossible_bedroom_hazard_settled_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(
        contains_word(text, cue)
        for cue in ("bed", "bedroom", "furniture", "bedframe", "bedding", "mattress")
    ):
        return False
    if not any(cue in lowered for cue in ("landing", "doorframe", "doorway", "threshold")):
        return False
    impossible_confirmed = any(
        cue in lowered
        for cue in (
            "sanity roll",
            "san reduced",
            "impossible movement",
            "impossible shock",
            "no visible hand",
            "ordinary world does not permit",
            "plainly impossible",
        )
    )
    if not impossible_confirmed:
        return False
    temporary_lull = any(
        cue in lowered
        for cue in (
            "nothing lunges",
            "silence returns",
            "hold on the landing",
            "holding on the landing",
            "old house settling",
            "faint aftermath of disturbed furniture",
            "does not break immediately",
        )
    )
    return temporary_lull


def unproductive_bedroom_threshold_probe_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("bedroom", "bed", "papers", "wardrobe")):
        return False
    if not any(cue in lowered for cue in ("threshold", "doorway", "landing")):
        return False
    if moving_bedroom_threat_visible(text):
        return False
    if bed_centered_actionable_scene_visible(text):
        return False
    if any(
        cue in lowered
        for cue in (
            "legible name",
            "legible note",
            "readable",
            "specific mark",
            "specific document",
            "concrete clue",
            "new clue",
            "you find",
            "evelyn finds",
        )
    ):
        return False
    if (
        "legible document" in lowered
        and "no legible document" not in lowered
        and "nothing in that first cautious pass yields" not in lowered
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "failure",
            "failed",
            "nothing in that first cautious pass yields",
            "no concrete mark",
            "no legible document",
            "no obvious tampering",
            "not make the room yield",
            "does not yield",
            "has not yet forced the issue",
            "plain actionable thing",
            "plain thing in front",
            "nearest actionable feature is the bed",
            "bed is the concrete affordance",
            "bed as its most immediate feature",
            "significance is not yet proven",
            "not yet proven",
            "not yet yielded anything conclus",
            "cannot yet prove",
            "what remains uncertain",
            "nothing in it is moving right now",
            "not yet a proven attack",
            "not a proven attack",
        )
    )


def bedroom_still_unsafe_after_threshold_probe_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("bed", "bedframe", "bedroom", "furniture")):
        return False
    if not any(
        cue in lowered
        for cue in (
            "threshold",
            "landing",
            "outside the room",
            "stairs behind",
            "retreat path",
            "way back",
            "back away",
            "hall",
            "stairs",
        )
    ):
        return False
    if any(
        cue in lowered
        for cue in (
            "concrete mark",
            "specific document",
            "legible note",
            "usable clue",
            "safe to enter",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "covers disturbed",
            "bed sits",
            "bed is visibly out of place",
            "out of place",
            "skewed at an unnatural angle",
            "canted at an unnatural angle",
            "knocked out of its ordinary position",
            "no longer sitting square",
            "stopped wrong",
            "not safely reachable",
            "would no longer be safe",
            "no longer be safe",
            "re-entering the dangerous room",
            "does not immediately disgorge",
            "nothing lunges",
            "nothing answers with a sudden movement",
            "no fresh violent impact",
            "thin scrape of wood",
            "whether the bed will move again",
            "moved impossibly",
            "danger is over or merely paused",
            "danger has ended or is only waiting",
            "anything else in the room is active",
            "something acting inside the room",
            "movement is caused by",
            "not yet safe to assume",
            "crossing that threshold",
            "unseen is in the room",
            "trigger another violent movement",
            "leave the safer line of retreat",
            "leave the safer line",
            "commit to the room",
            "committing yourself to that space",
            "cannot yet prove whether what just happened",
            "cannot yet prove whether approaching",
            "focal point, not an explanation",
            "what is uncertain is what, if anything, it will do",
        )
    )


def clarified_disturbed_bed_edge_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("bed", "bedspread", "coverlet", "bedding", "mattress")):
        return False
    if not any(cue in lowered for cue in ("threshold", "doorway", "landing")):
        return False
    if not any(
        cue in lowered
        for cue in (
            "near edge",
            "nearest edge",
            "hanging edge",
            "nearest corner",
            "bedspread",
            "coverlet",
        )
    ):
        return False
    if not any(
        cue in lowered
        for cue in (
            "walking stick",
            "probe",
            "lift",
            "tug",
            "within reach",
            "reachable",
            "without moving deeper",
            "without stepping farther",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "disturbed",
            "shifted",
            "uneven",
            "dragged",
            "does not lie flat",
            "does not look neatly",
            "what caused",
        )
    )


def clarified_reachable_bedroom_paper_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(
        cue in lowered
        for cue in ("paper", "papers", "sheet", "loose page", "actual document", "document", "documents")
    ):
        return False
    if not any(cue in lowered for cue in ("bedroom", "room", "threshold", "landing")):
        return False
    reachable = any(
        cue in lowered
        for cue in (
            "safely reachable",
            "within reach of your walking stick",
            "within reach of the walking stick",
            "can probe or draw it closer",
            "draw it closer with the walking stick",
            "reached with the walking stick",
            "reachable from your current position",
            "one reachable item nearer",
            "draw one reachable item nearer",
            "nothing in your reach needs to be guessed at",
        )
    )
    actionable_from_threshold = any(
        cue in lowered
        for cue in (
            "most concrete visible affordance",
            "clearest concrete affordance",
            "distinct as a document",
            "not yet readable from your present angle",
            "worth checking without stepping into the room",
            "a little beyond the threshold",
            "rather than out on the landing",
        )
    ) and any(
        cue in lowered
        for cue in (
            "near the bedframe",
            "near the bed",
            "by the bed",
            "close to the side of the bed",
            "beside the nearer side of the bed",
        )
    )
    documentable_from_doorway = any(
        cue in lowered
        for cue in (
            "worth documenting before handling",
            "worth photographing before handling",
            "documents worth photographing",
            "can safely photograph the visible papers",
            "safely photograph the visible papers",
            "photographing first and probing second",
            "photograph their visible arrangement",
            "visible arrangement from the doorway",
            "photograph the visible arrangement",
            "from the doorway before deciding whether to reach farther",
            "photographed from the doorway",
            "photograph from the doorway",
            "before deciding whether to hook one closer",
        )
    ) and any(
        cue in lowered
        for cue in (
            "without stepping in",
            "from the doorway",
            "landing-side threshold",
            "from the landing-side threshold",
            "threshold",
        )
    )
    if not reachable and not actionable_from_threshold and not documentable_from_doorway:
        return False
    return any(
        cue in lowered
        for cue in (
            "loose paper",
            "loose papers",
            "loose sheet",
            "small loose sheet",
            "visible papers",
            "papers scattered",
            "actual document",
            "document rather than torn scrap",
            "papers can be photographed",
            "documents worth photographing",
            "documents worth photographing before handling",
            "worth your care rather than random trash",
            "nearest the bedroom doorway",
            "a little past the threshold",
            "edge curled",
        )
    )


def bed_centered_actionable_scene_visible(text: str) -> bool:
    lowered = text.lower()
    if moving_bedroom_threat_visible(text):
        return False
    if not any(cue in lowered for cue in ("bedroom", "room")):
        return False
    if not any(cue in lowered for cue in ("threshold", "doorway", "landing")):
        return False
    if not any(
        contains_word(text, cue)
        for cue in ("bed", "bedframe", "bedding", "mattress", "bedspread", "floorboards")
    ):
        return False

    bed_disturbance = any(
        cue in lowered
        for cue in (
            "not just neglected",
            "disturbed",
            "not sit",
            "does not sit",
            "wrong in its placement",
            "bedding lies",
            "violent motion",
            "repeated strain",
            "scuffing",
            "scuffed",
            "floorboards near it",
            "bed is the concrete affordance",
            "bed as its most immediate feature",
            "nearest actionable feature is the bed",
            "bed is the center",
            "bed-centered",
        )
    )
    if not bed_disturbance:
        return False

    concrete_followup = any(
        cue in lowered
        for cue in (
            "loose paper",
            "loose papers",
            "visible papers",
            "real papers",
            "actual document",
            "specific document",
            "worth preserving",
            "worth photographing",
            "window frame",
            "woodwork",
            "marks around the window",
            "bed edge",
            "near edge",
            "reachable",
            "within reach",
            "walking stick",
        )
    )
    if not concrete_followup:
        return False

    safe_standoff = any(
        cue in lowered
        for cue in (
            "from the landing",
            "landing side",
            "landing-side",
            "from the doorway",
            "from here",
            "escape line",
            "retreat path",
            "doorway between",
            "nothing moves",
            "nothing rushes",
            "not moving right now",
            "without stepping",
            "before touching",
        )
    )
    return safe_standoff or has_success(text)


def clarified_visible_bedroom_papers_need_bounded_entry_visible(text: str) -> bool:
    lowered = text.lower()
    if moving_bedroom_threat_visible(text):
        return False
    if not any(cue in lowered for cue in ("paper", "papers", "loose page", "loose pages")):
        return False
    if not any(cue in lowered for cue in ("bedroom", "room", "threshold", "landing")):
        return False
    if not any(
        cue in lowered
        for cue in (
            "papers are real",
            "papers are plainly visible",
            "visible scatter of papers",
            "scatter of papers",
            "loose papers are there",
            "there are loose pages",
            "there are papers",
            "papers in the bedroom ahead",
            "nearest visible object",
            "nearest visible thing",
        )
    ):
        return False
    if not any(
        cue in lowered
        for cue in (
            "beyond the threshold",
            "inside the bedroom",
            "inside the room",
            "ahead of you",
            "ahead of her",
            "in front of you",
            "in front of her",
        )
    ):
        return False
    if not any(
        cue in lowered
        for cue in (
            "would have to lean",
            "would have to step",
            "have to move into the room",
            "have to go closer",
            "requires her to go closer",
            "without advancing",
            "cannot yet read",
            "cannot properly read",
            "cannot examine them clearly",
            "cannot examine properly",
            "farther into the room",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "retreat path is still",
            "retreat path stays",
            "retreat remains",
            "landing",
            "stairs",
            "way out",
            "path is still open",
        )
    )


def retrieved_bedroom_paper_needs_close_examination_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("paper", "sheet", "loose page")):
        return False
    if not any(cue in lowered for cue in ("bedroom", "threshold", "landing", "doorway")):
        return False
    if not any(
        cue in lowered
        for cue in (
            "already drew closer",
            "drawn nearer the doorway",
            "nudged nearer the doorway",
            "drawn to the doorway",
            "comes nearer",
            "brought closer to the doorway",
            "closer to the doorway",
            "draw it toward the doorway",
            "drawn toward the doorway",
            "now lies closer to the doorway",
            "drawn close enough to the threshold",
            "brought one loose sheet to the doorway",
            "at the bedroom threshold",
            "just inside the threshold",
            "near as the threshold allows",
            "close enough",
            "near your feet",
            "retrieved paper",
        )
    ):
        return False
    if not any(
        cue in lowered
        for cue in (
            "safely reachable",
            "touch it",
            "pick it up",
            "examine it more closely",
            "within easier reach",
            "within safer viewing",
            "safer viewing range",
            "easier to read from safety",
            "for a real look",
            "without stepping into the room",
            "without putting her weight past the threshold",
            "inspect it further",
            "look more closely",
            "close enough to inspect",
            "read what can be read",
            "study the surface carefully",
            "manipulate it with the stick",
            "still in the doorway area",
            "without crossing the threshold",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "reverse side",
            "faint",
            "hidden",
            "impressed",
            "closer handling",
            "blankness",
            "conspicuously blank",
            "no message",
            "no date",
            "no name",
            "no symbol",
            "no clear visible writing",
            "no clearly legible writing",
            "not plainly visible",
            "not boldly marked",
            "no large heading",
            "no obvious message",
            "no obvious emblem",
            "no striking date",
            "no large name",
            "no bold name",
            "no clear date",
            "no large symbol",
            "apparent blankness",
            "old loose paper",
            "old paper sheet",
            "single old sheet",
            "single loose sheet",
            "no bold symbol",
            "no instantly unmistakable name",
            "faint enough from this angle",
            "faint, small, obscured",
            "turned away",
        )
    )


def append_jsonl(path: Path, row: dict[str, Any]) -> None:
    with path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(row, ensure_ascii=False) + "\n")


def concrete_chapel_source_visible(text: str) -> bool:
    lowered = text.lower()
    if "chapel of contemplation" not in lowered:
        return False
    if any(
        cue in lowered
        for cue in (
            "michael thomas",
            "closed in 1912",
            "the 1912",
            "carved words",
            "chapel of contemplation, carved",
            "chapel of contemplation. cold damp",
        )
    ):
        return True
    for sentence in re.split(r"[.!?。！？]\s*", lowered):
        if "chapel of contemplation" not in sentence:
            continue
        if any(
            cue in sentence
            for cue in (
                "no executor",
                "no named",
                "not visible",
                "not yet visible",
                "without an executor",
                "without a named",
                "does not mention",
                "doesn't mention",
            )
        ):
            continue
        if any(
            cue in sentence
            for cue in (
                "executor",
                "reverend",
                "raid",
                "closure",
                "closed",
                "affidavit",
                "file trail",
                "records show",
                "record shows",
                "clippings show",
                "newspaper connects",
                "visible records connect",
            )
        ):
            return True
    return False


def unearned_chapel_direction_visible(text: str) -> bool:
    lowered = text.lower()
    if "chapel of contemplation" not in lowered:
        return False
    has_failed_or_inconclusive_public_source = any(
        cue in lowered
        for cue in (
            "failure",
            "failed",
            "no clean",
            "no specific",
            "no decisive",
            "no useful",
            "inconclusive",
            "does not produce",
            "not produce",
            "without the breakthrough",
        )
    )
    has_direction_frame = any(
        cue in lowered
        for cue in (
            "most promising next channels",
            "next channels appear",
            "appear to be",
            "branches to",
            "or leaving paper behind",
            "or leave paper behind",
            "toward the chapel",
            "go to the chapel",
            "the chapel of contemplation or the house",
        )
    )
    has_option_cluster = (
        any(cue in lowered for cue in ("court", "courts", "police"))
        and "chapel of contemplation" in lowered
        and ("house itself" in lowered or "corbitt house" in lowered)
        and (" or " in lowered or "—or " in lowered)
    )
    return (
        has_failed_or_inconclusive_public_source
        and has_direction_frame
        and has_option_cluster
        and not concrete_chapel_source_visible(text)
    )


def earned_chapel_lead_visible(text: str) -> bool:
    lowered = text.lower()
    if "chapel of contemplation" not in lowered:
        return False
    if unearned_chapel_direction_visible(text):
        return False
    if concrete_chapel_source_visible(text):
        return True
    return any(
        cue in lowered
        for cue in (
            "arrive at the chapel",
            "reach the chapel",
            "chapel stands",
            "chapel exterior",
            "chapel search",
            "chapel records",
            "pulpit",
            "cabinet",
            "carved words",
            "carving",
            "symbol",
            "visible mark",
            "rubbing",
            "rubbings",
        )
    )


def facts_from_visible(state: VisibleState) -> list[str]:
    text = state.lower_all()
    facts: list[str] = []
    if "knott" in text:
        facts.append(f"Knott hired {state.pc_name} to investigate the Corbitt House.")
    if "key" in text:
        facts.append(f"{state.pc_name} has Knott's keys, but some lock matches are uncertain.")
    if "hall of records" in text or "probate" in text or "executor" in text:
        facts.append("Public records are a valid low-risk investigation path.")
    if executor_michael_thomas_visible(state.transcript):
        facts.append("Corbitt's executor trail is tied to Reverend Michael Thomas.")
    if earned_chapel_lead_visible(state.transcript):
        facts.append("The Chapel of Contemplation is a concrete lead.")
    if "closed in 1912" in text or "1912" in text:
        facts.append("The chapel trail includes a 1912 date.")
    if "do not get the file" in text or "isn't for casual inspection" in text:
        facts.append("Official court/police access was refused or blocked.")
    if contains_word(text, "raid") or "missing children" in text or contains_word(text, "imprisoned"):
        if earned_chapel_lead_visible(state.transcript) or "michael thomas" in text or "1912" in text:
            facts.append("Visible records or testimony connect the chapel to serious 1912 trouble.")
        else:
            facts.append("Visible records or testimony point toward a serious court or police-record trail.")
    if chapel_concrete_payoff_visible(state.transcript):
        facts.append("The Chapel search produced a concrete visible symbol or mark.")
    if explicit_burial_clue_visible(state.transcript):
        facts.append("A player-visible clue points toward a burial question.")
    elif basement_actionable_lead_visible(adjudicated_visible_text(state.transcript)):
        facts.append("A player-visible basement clue needs follow-up.")
    if "old corbitt place remains waiting" in text or "corbitt house" in text:
        facts.append("The house itself remains the core investigation site.")
    if interior_started_visible(state.transcript):
        facts.append(f"{state.pc_name} has begun inspecting the house interior or its threshold.")
    if nearby_ground_floor_anomaly_affordance_visible(state.last_reply):
        facts.append("A nearby ground-floor anomaly has been clarified as visible while the retreat path remains open.")
    if upper_route_visible(state.transcript):
        facts.append("The house has an upper-floor route or room to inspect.")
    if basement_route_visible(state.transcript):
        facts.append("The house has a cellar or basement route to evaluate.")
    if active_blade_threat_visible(state.last_reply):
        facts.append("A knife or dagger is now player-visible and potentially dangerous.")
    if corbitt_body_or_remains_visible(state.last_reply):
        facts.append("Walter Corbitt or his remains are now a direct focus.")
    if objective_resolved_visible(state.last_reply):
        facts.append("The visible objective appears resolved.")
    return facts or [f"{state.pc_name} only knows the player-visible briefing and prior transcript."]


def objective_resolved_visible(text: str) -> bool:
    if stop_reason(text) is not None:
        return True
    lowered = text.lower()
    return any(
        cue in lowered
        for cue in (
            "objective resolved",
            "case resolved",
            "investigation resolved",
            "investigation is complete",
        )
    )


def executor_michael_thomas_visible(text: str) -> bool:
    lowered = text.lower()
    positive_phrases = (
        "executor was reverend michael thomas",
        "executor was michael thomas",
        "executor is reverend michael thomas",
        "executor is michael thomas",
        "executor trail yields",
        "executor trail is tied to",
        "executor record names",
        "corbitt's executor",
        "corbitt’s executor",
        "遗嘱执行人",
        "执行人",
    )
    if any(phrase in lowered for phrase in positive_phrases) and "michael thomas" in lowered:
        return True
    for sentence in re.split(r"[.!?。！？]\s*", lowered):
        if "executor" not in sentence or "michael thomas" not in sentence:
            continue
        if any(
            cue in sentence
            for cue in (
                "not on",
                "not the",
                "missing executor",
                "no executor",
                "without naming",
                "does not name",
            )
        ):
            continue
        return True
    return False


def result_summary(last_reply: str) -> str:
    if not last_reply.strip():
        return "No prior action."
    if has_failure(last_reply):
        return "Previous action was adjudicated as a failure or blocked access; do not treat requested facts as learned."
    if hall_records_success_without_specific_facts_visible(last_reply):
        return "Previous action has a success roll but did not expose concrete records; keep resolving it before acting as if the research succeeded."
    if official_records_success_without_contents_visible(last_reply):
        return "Previous official-records action has a success roll but only opened access; keep reading the actual record contents before changing locations."
    if has_success(last_reply):
        return "Previous action succeeded; use only concrete facts that the GM actually exposed."
    if mentions(
        last_reply,
        "do not get the file",
        "refusal",
        "access is restricted",
        "blocked",
        "won't",
        "will not",
    ):
        return "Previous action was refused or blocked, but that refusal is itself a usable fact."
    if mentions(last_reply, "what does", "what do you do", "do next"):
        return "Previous action produced a player-visible situation and now waits for the next declaration."
    return "Previous action produced narration; treat only explicit visible facts as established."


def basement_actionable_lead_visible(text: str) -> bool:
    lowered = text.lower()
    if ("failure" in lowered or "failed" in lowered) and not has_success(text):
        return False
    if "basement" not in lowered and "cellar" not in lowered:
        return False
    if clarified_basement_return_route_visible(text):
        return False
    if basement_draft_surface_hidden_path_lead_visible(text):
        return True
    concrete_success_lead = has_success(text) and any(
        cue in lowered
        for cue in (
            "concealed edge",
            "hidden seam",
            "concealed section",
            "cramped dark space",
            "rough opening",
            "space past the boards",
            "cavity beyond",
            "inner wall",
            "chapel of contemplation",
            "boundary is real",
            "boarded over on purpose",
            "disturbed dust",
            "scrape marks",
            "signs of disturbance worth following",
            "disturbance worth following",
            "deserves closer inspection",
            "not just an untouched junk cellar",
            "structural irregularity",
        )
    )
    if any(
        cue in lowered
        for cue in (
            "cannot yet prove",
            "still cannot prove",
            "what you still cannot prove",
            "what you do not yet have",
            "whether it is locked",
            "whether it merely",
            "whether it conceals",
            "does not cleanly give",
            "remain unproven",
            "remains unproven",
            "without pressing deeper",
            "may be only",
            "might be only",
        )
    ) and not concrete_success_lead:
        return False
    return any(
        cue in lowered
        for cue in (
            "disturbed earth",
            "scrape mark",
            "scraped area",
            "scuffed mark",
            "scuffed marks",
            "concealed section",
            "structural irregularity",
            "hidden panel",
            "hidden access",
            "suspicious section",
            "loose earth",
            "freshly disturbed",
            "fresh scrape",
            "scrape traces",
            "dust lies differently",
            "dust disturbed",
            "disturbed in uneven streaks",
            "airflow",
            "faint suggestion of airflow",
            "surfaces show interruption",
            "interruption rather than simple decay",
            "deserves closer attention",
            "deserves closer inspection",
            "signs of disturbance worth following",
            "disturbance worth following",
            "worth following more closely",
            "not every surface has been left in exactly the same condition",
            "not just an untouched junk cellar",
            "area below that deserves",
            "surfaces and spacing seem wrong",
            "closed off, altered, or concealed",
            "closed off",
            "altered, or concealed",
            "single strongest lead in the basement",
        )
    )


def basement_draft_surface_hidden_path_lead_visible(text: str) -> bool:
    lowered = text.lower()
    if ("failure" in lowered or "failed" in lowered) and not has_success(text):
        return False
    if not any(cue in lowered for cue in ("basement", "cellar")):
        return False
    if not has_success(text):
        return False

    current_basement_position = any(
        cue in lowered
        for cue in (
            "low on the basement stairs",
            "lower stretch of steps",
            "foot of the stairs",
            "basement stairs",
            "cellar proper",
            "down in the basement",
            "in the basement proper",
            "stairs behind you",
            "route back still marked",
            "marked exit still behind",
        )
    )
    air_path = any(
        cue in lowered
        for cue in (
            "draft",
            "airflow",
            "air is finding another path",
            "another path",
            "not coming only from the stair",
            "not explained by the stair",
            "opening",
            "gap",
            "hidden way",
            "further irregularity",
        )
    )
    surface_disturbance = any(
        cue in lowered
        for cue in (
            "dirt",
            "earth",
            "floor",
            "floorboards",
            "scuffing",
            "scuffed",
            "disturbed",
            "does not lie perfectly even",
            "rough cellar surfaces",
            "rough surfaces",
            "scrape",
            "uneven",
        )
    )
    followup_grounded = any(
        cue in lowered
        for cue in (
            "deserve closer inspection",
            "deserves closer inspection",
            "worth following",
            "continue the close search",
            "not a dead, featureless storage hole",
            "not just an untouched junk cellar",
            "hidden way",
            "opening",
            "gap",
            "concealment",
            "concealed",
            "irregularity",
        )
    )
    return current_basement_position and air_path and surface_disturbance and followup_grounded


def clarified_basement_return_route_visible(text: str) -> bool:
    lowered = text.lower()
    if "basement" not in lowered and "cellar" not in lowered:
        return False
    if not any(cue in lowered for cue in ("stair", "stairs", "staircase")):
        return False
    if not any(
        cue in lowered
        for cue in (
            "return route",
            "retreat route",
            "marked return",
            "stair behind",
            "stairs behind",
            "behind you",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "single most concrete",
            "presently established affordance",
            "confirmed, legible",
            "safely reachable",
            "what you do not yet have",
            "without pressing deeper",
        )
    )


def chapel_search_failure_visible(text: str) -> bool:
    lowered = text.lower()
    if not has_failure(text):
        return False
    if "chapel" not in lowered:
        return False
    in_local_basement_or_crawl = any(
        cue in lowered
        for cue in (
            "crawl space",
            "crawl-space",
            "crawlspace",
            "basement",
            "cellar",
            "marked stairs",
            "stairs behind",
            "crawl-space wall",
            "crawl space wall",
        )
    )
    chapel_location_context = any(
        cue in lowered
        for cue in (
            "inside the chapel",
            "chapel grounds",
            "ruined chapel",
            "surviving chapel",
            "chapel records",
            "pulpit",
            "altar",
            "cabinet",
            "office",
            "storage areas",
        )
    )
    if in_local_basement_or_crawl and not chapel_location_context:
        return False
    if any(
        cue in lowered
        for cue in (
            "like the chapel record",
            "like a chapel record",
            "than the chapel record",
            "chapel record did",
            "chapel trail remains",
            "chapel clue remains",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "chapel search",
            "search of the chapel",
            "searching the chapel",
            "inside the chapel",
            "chapel of contemplation",
            "surviving chapel",
            "chapel records",
            "pulpit",
            "altar",
            "rubbings",
            "carvings",
        )
    )


def chapel_scene_established_without_search_resolution_visible(text: str) -> bool:
    lowered = text.lower()
    if "chapel" not in lowered:
        return False
    if "[roll]" in lowered or has_success(text) or has_failure(text):
        return False
    if any(
        cue in lowered
        for cue in (
            "you find",
            "you found",
            "records reveal",
            "record reveals",
            "michael thomas",
            "walter corbitt",
            "buried",
            "burial",
            "no chapel clue",
            "nothing here confirms",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "stands in a state",
            "state beyond neglect",
            "cracked masonry",
            "dust in the seams",
            "gave way years ago",
            "abandonment",
            "not the same thing as peace",
        )
    )


def chapel_success_without_payoff_visible(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("chapel", "礼拜堂", "小礼拜堂", "教堂", "沉思")) or not has_success(text):
        return False
    if explicit_burial_clue_visible(text):
        return False
    if any(
        cue in lowered
        for cue in (
            "crawl space",
            "crawl-space",
            "under-space",
            "basement",
            "cellar",
            "basement stairs",
            "rough boards",
        )
    ):
        return False
    if chapel_concrete_payoff_visible(text):
        return False
    if any(
        cue in lowered
        for cue in (
            "ground floor",
            "corbitt house ground",
            "staircase leading up",
            "downward way toward the basement",
            "basement route",
            "cellar direction",
            "most meaningful lead inside",
        )
    ):
        return False
    if any(
        cue in lowered
        for cue in (
            "walter corbitt",
            "corbitt was buried",
            "buried in the basement",
            "record reveals",
            "records reveal",
            "you find",
            "you found",
            "fresh tracks lead",
            "recent use is",
            "house link",
            "nothing here confirms",
            "no chapel clue",
        )
    ):
        return False
    if not any(
        cue in lowered
        for cue in (
            "circle the ruined chapel",
            "chapel exterior",
            "surviving interior",
            "chapel records",
            "pulpit",
            "storage",
            "wall marks",
            "loose papers",
            "fresh tracks",
            "signs of recent use",
            "keep pressing deeper",
            "what area do you focus on first",
            "will take patience and care",
            "buried in remnants",
        )
    ):
        return False
    return any(
        cue in lowered
        for cue in (
            "pays off",
            "pay off",
            "success",
            "succeeds",
        )
    )


def chapel_concrete_payoff_visible(text: str) -> bool:
    lowered = adjudicated_visible_text(text).lower()
    if not any(cue in lowered for cue in ("chapel", "礼拜堂", "小礼拜堂", "教堂", "沉思")) or not has_success(lowered):
        return False
    if explicit_burial_clue_visible(text):
        return True
    if any(
        cue in lowered
        for cue in (
            "walter corbitt was buried",
            "corbitt was buried",
            "buried in the basement of his house",
            "record reveals",
            "records reveal",
            "house link",
        )
    ):
        return True
    has_mark_subject = any(
        cue in lowered
        for cue in (
            "symbol",
            "emblem",
            "visible mark",
            "repeated form",
            "marked surfaces",
            "carving",
            "rubbing",
            "rubbings",
        )
    )
    has_concrete_context = any(
        cue in lowered
        for cue in (
            "distinct, visible",
            "not random damage",
            "photographs",
            "quick rubbings",
            "can preserve",
            "reproducible sign",
            "something concrete from the chapel",
            "associated with the chapel",
            "chapel of contemplation's records",
            "chapel of contemplation’s records",
        )
    )
    return has_mark_subject and has_concrete_context


def clarified_chapel_cabinet_affordance_visible(text: str) -> bool:
    lowered = text.lower()
    if "chapel" not in lowered or "cabinet" not in lowered:
        return False
    if any(cue in lowered for cue in ("basement", "cellar", "crawl space", "crawl-space", "under-space")):
        return False
    has_cabinet_affordance = any(
        cue in lowered
        for cue in (
            "old cabinet",
            "the cabinet",
            "piece of storage",
            "surviving piece of storage",
            "intact furnishings",
            "still standing",
        )
    )
    has_actionable_grounding = any(
        cue in lowered
        for cue in (
            "one concrete thing",
            "plainly actionable",
            "actually act on",
            "reachable",
            "worth examining",
            "might hold",
            "capable of holding",
            "clearest object",
            "nearest clear object",
        )
    )
    has_position = any(
        cue in lowered
        for cue in (
            "retreat",
            "way back",
            "way out",
            "behind",
            "ahead of you",
            "ahead of her",
            "inside the chapel proper",
        )
    )
    return has_cabinet_affordance and has_actionable_grounding and has_position


def base_decision(
    state: VisibleState,
    *,
    persona: str,
    active_goal: str,
    hypotheses: list[str],
    open_questions: list[str],
    candidate_actions: list[str],
    selection_rationale: str,
    declared_action: str,
    intent: str,
    acceptable: list[str] | None = None,
    requested: list[str] | None = None,
    unacceptable: list[str] | None = None,
) -> dict[str, Any]:
    turn = state.turn + 1
    acceptable = acceptable or [
        "Resolve with concrete player-visible facts",
        "Request and resolve an appropriate check",
        "Block the action with a clear reason and a safer or legal alternative",
        "Create a bounded pending item with an explicit trigger",
    ]
    sent_action = localize_player_action_for_output_language(
        state,
        declared_action=declared_action,
        intent=intent,
        requested=requested,
    )
    return {
        "turn": turn,
        "kind": "PLAYER_DECISION",
        "persona": persona,
        "gm_visible_reply": sentence_excerpt(state.last_reply or state.transcript),
        "perceived_facts": facts_from_visible(state),
        "active_goal": active_goal,
        "hypotheses": hypotheses
        + [
            "The next action must follow only player-visible facts.",
            "A failed roll or refused access changes the plan rather than becoming hidden success.",
        ],
        "last_action_result": result_summary(state.last_reply),
        "risk_assessment": [
            "Avoid entering confined or unstable spaces without checking exits.",
            "Treat official refusal or failed searches as real limits.",
            "If danger becomes immediate, prioritize cover, retreat, or a concrete defensive action.",
        ],
        "resource_assessment": [
            f"{state.pc_name} has notebook, camera, flashlight, walking stick, press skills, and Knott's keys.",
            "Use daylight, public records, exits, and visible tools before high-risk confrontation.",
        ],
        "open_questions": open_questions,
        "candidate_actions": candidate_actions,
        "selection_rationale": selection_rationale,
        "declared_action": sent_action,
        "sent_to_gm": sent_action,
        "response_contract": {
            "intent": intent,
            "requested_information": requested or [],
            "acceptable_resolutions": acceptable,
            "unacceptable": unacceptable
            or [
                "Offer an explicit action menu to the player",
                "Provide only atmosphere after a successful check",
                "Ignore the declared limits or safety preparation",
            ],
        },
    }


def choose_next_decision(state: VisibleState, persona: str) -> dict[str, Any]:
    text = state.lower_all()
    last = state.lower_last()
    pc = state.pc_name

    if state.turn == 0:
        return base_decision(
            state,
            persona=persona,
            active_goal="Close briefing gaps and source-bound the address before entering the house.",
            hypotheses=[
                "Knott can clarify keys, address handling, public record paths, and the Macario rumor boundary."
            ],
            open_questions=[
                "Which keys might open exterior or cellar doors?",
                f"Which public records can {pc} check before entering the house?",
            ],
            candidate_actions=[
                "Question Knott about address, keys, Macarios, and research paths",
                "Go directly to public records using the Corbitt House lead",
                "Inspect the house exterior immediately",
            ],
            selection_rationale="A cautious investigator first establishes player-visible, source-bounded leads.",
            declared_action=(
                f"I do not leave immediately. {pc} opens her notebook and asks Mr. Knott "
                "to write down the usable address lead he can source for the Corbitt House and "
                "identify which locks each key is meant to open. Then I ask what he actually knows "
                "about the Macario family, which official records might exist, and whether he agrees "
                "I should check municipal records and old newspaper files before entering the house."
            ),
            intent="confirm_briefing_and_source_bound_address",
            requested=[
                "usable address lead",
                "key/lock uncertainty",
                "Macario public facts",
                "public research paths",
            ],
        )

    if no_new_visible_result(state.last_reply) and "basement_descent" in state.attempted:
        return base_decision(
            state,
            persona=persona,
            active_goal="Recover the missing basement-descent result without resetting to the exterior door.",
            hypotheses=[
                "The previous declared action was the marked basement descent, not a fresh exterior-door interaction.",
                "The Keeper's reply did not settle whether Evelyn is at the basement doorway, on the first steps, blocked, or awaiting a check.",
                "A player-like response must ask for that specific action's result instead of a generic affordance.",
            ],
            open_questions=[
                "Is Evelyn still at the basement doorway, on the stairs, or blocked before descending?",
                "Did the descent succeed, fail, require a check, or reveal a concrete obstacle?",
                "Is the handkerchief-marked retreat route still open?",
            ],
            candidate_actions=[
                "Ask the Keeper to resolve the basement descent action specifically",
                "Hold at the marked basement stair mouth until position and result are clear",
                "Withdraw only if the descent is explicitly blocked or dangerous",
            ],
            selection_rationale=(
                "The empty result follows a basement action, so the clarification must preserve that action anchor and prevent a side-door reset."
            ),
            declared_action=(
                f"{pc} stops and anchors the clarification to the previous action: she had tied a "
                "handkerchief marker at the basement door, wedged the door open, and started the careful "
                "basement-stair descent with flashlight and walking stick. She asks the Keeper to resolve "
                "that basement action specifically: whether she is still at the stair mouth, on the first "
                "steps, blocked before descending, or being asked for a check; what she can see, hear, or "
                "smell from that basement position; and whether the marked retreat route remains open. "
                "She does not accept a reset to the side door or exterior threshold unless the Keeper states "
                "the concrete retreat or interruption that moved her there."
            ),
            intent="clarify_empty_basement_descent_result",
            requested=[
                "basement descent success/failure/block/check request",
                "current basement-relative position",
                "visible cellar facts or clear absence",
                "retreat route status",
            ],
            unacceptable=[
                "Reset Evelyn to the exterior side door without a concrete retreat",
                "Repeat a generic no-new-public-route sentence",
                "Offer an explicit action menu to the player",
            ],
        )

    if no_new_visible_result(state.last_reply):
        return base_decision(
            state,
            persona=persona,
            active_goal="Recover the missing player-visible result before making a new commitment.",
            hypotheses=[
                "The last reply explicitly says no new public fact was exposed.",
                "A player cannot infer interior layout, safety, or successful movement from an empty visible result.",
            ],
            open_questions=[
                "Did the previous action succeed, fail, get blocked, or require a check?",
                "What exact visible position, exit, hazard, or affordance exists now?",
                "What is still unconfirmed?",
            ],
            candidate_actions=[
                "Ask the Keeper to resolve the previous action's visible result",
                "Hold position and do not move deeper",
                "Retreat to the last confirmed safe position if no result can be given",
            ],
            selection_rationale=(
                "The transcript contains no new visible result, so continuing the scripted exploration would use facts the player does not have."
            ),
            declared_action=(
                f"{pc} stops before taking any new physical step. The last reply did not give a "
                "player-visible result for the previous action, so she asks the Keeper to resolve that "
                "specific action first: where exactly is she now, what can she see or hear from that "
                "position, whether the action succeeded, failed, was blocked, or needs a check, and "
                "what remains unconfirmed. She does not continue deeper or start a room-by-room search "
                "until that missing result is visible."
            ),
            intent="clarify_empty_visible_result_before_continuing",
            requested=[
                "success/failure/block/check request for the previous action",
                "current visible position",
                "concrete exits, hazards, or affordances",
                "unconfirmed facts kept explicit",
            ],
            unacceptable=[
                "Treat the empty result as successful entry or search progress",
                "Move Evelyn deeper before resolving the previous action",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        bedroom_landing_crash_visible(state.last_reply)
        and "moving_bedroom_threat" in state.attempted
        and "bedroom_crash_recovery" not in state.attempted
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Recover from the landing crash before changing scenes.",
            hypotheses=[
                "The failed dodge left Evelyn down on the landing, so she cannot assume a clean retreat to the ground floor.",
                "The moving bed and furniture are still shaping the doorway and stair route.",
                "The next player-like move is to resolve injury, footing, and escape line before choosing a new investigation lead.",
            ],
            open_questions=[
                "Can Evelyn stand, crawl, or roll clear before the furniture moves again?",
                "Is the stair route blocked by the bed, splinters, or her own injury?",
                "Did the crash cause HP damage, a condition, dropped gear, or a lost flashlight position?",
            ],
            candidate_actions=[
                "Protect head and ribs, then crawl or roll toward the stair side of the landing",
                "Check whether she can stand and whether the stairs are clear",
                "Keep the moving bed in sight rather than turning away to a new scene",
            ],
            selection_rationale=(
                "A downed character beside an active supernatural furniture threat must stabilize position before route-switching."
            ),
            declared_action=(
                f"{pc} does not treat the bedroom as merely inconclusive and does not head for the basement yet. "
                "She is down on the landing, so she first curls an arm over her head and ribs, keeps the "
                "flashlight aimed toward the jammed bed as best she can, and crawls or rolls only toward the stair-side "
                "wall if that line is clear. She checks whether she can put weight on her hip and shin, whether the "
                "stairs are blocked, whether she dropped anything important, and whether the bed or furniture is still "
                "moving. If the furniture lunges again, she dodges or scrambles down the first safe stair; if it stays "
                "jammed, she gets one knee under herself and retreats only as far as the landing/stair route actually allows."
            ),
            intent="recover_from_bedroom_landing_crash",
            requested=[
                "Evelyn's prone/standing position after the crash",
                "HP/resource/condition or explicit no-damage result",
                "whether stairs and landing retreat are clear or blocked",
                "moving bed/furniture reaction",
            ],
            unacceptable=[
                "Assume a clean retreat to the ground floor without resolving the crash",
                "Switch to the basement route while Evelyn is still prone beside the threat",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        confirmed_impossible_bedroom_hazard_settled_visible(state.last_reply)
        and "moving_bedroom_threat" in state.attempted
        and "bedroom_abandoned" not in state.attempted
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Withdraw from the confirmed impossible bedroom hazard and preserve evidence.",
            hypotheses=[
                "The bed's impossible movement is now confirmed by a sanity event, even if it is not currently lunging.",
                "A cautious investigator should not re-enter or continue probing the same bedroom immediately after SAN loss.",
                "The next realistic move is to leave the upper-floor hazard, document the event, and reassess from a safer position.",
            ],
            open_questions=[
                "Can Evelyn retreat from the landing to the ground floor without the bedroom reacting?",
                "Does closing or marking the bedroom door change the immediate risk?",
                "What evidence, SAN loss, and position should be recorded before choosing another lead?",
            ],
            candidate_actions=[
                "Back away from the bedroom landing and mark the room unsafe",
                "Withdraw to the ground floor or exterior with SAN loss and observations recorded",
                "Return only with help, tools, or a stronger reason rather than immediate solo probing",
            ],
            selection_rationale=(
                "After an impossible moving-bed event and SAN loss, a player-like cautious investigator preserves survival and evidence "
                "instead of treating the bedroom as an ordinary unresolved clue node."
            ),
            declared_action=(
                f"{pc} does not re-enter the bedroom or probe the bed again after the impossible movement and SAN loss. "
                "She keeps the doorframe between herself and the room, backs down from the landing toward the ground floor, "
                "and marks the bedroom as unsafe in her notes with the exact SAN shock, moving-bed observation, and current "
                "position recorded. If she can pull the door partly shut from the landing side without stepping into the room, "
                "she does; otherwise she leaves it open and keeps distance. She exits to the ground floor or outside to steady "
                "her breathing and reassess the house before choosing any further lead. If the furniture moves again or blocks "
                "the stair route, she resolves that immediate danger first."
            ),
            intent="withdraw_from_confirmed_impossible_bedroom_hazard",
            requested=[
                "whether Evelyn withdraws safely from the landing to ground floor or outside",
                "any bedroom/furniture reaction during withdrawal",
                "SAN loss and evidence notes preserved",
                "current safe position after withdrawal",
            ],
            unacceptable=[
                "Treat the bedroom as a normal threshold probe after SAN loss",
                "Move Evelyn back into the unsafe room without a new player commitment and consequence",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "bedroom_threshold_probe" in state.attempted
        and "bedroom_scene_followup" not in state.attempted
        and "moving_bedroom_threat" not in state.attempted
        and not clarified_reachable_bedroom_paper_visible(state.last_reply)
        and bed_centered_actionable_scene_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Follow the successful bedroom threshold observation without turning it into another clarification loop.",
            hypotheses=[
                "The bed-centered disturbance, papers, and window or woodwork marks are player-visible leads.",
                "Nothing is currently lunging, so a cautious investigator can document and probe from the threshold.",
                "Entering the room deeply would be unsafe, but asking for the same affordance again wastes the successful observation.",
            ],
            open_questions=[
                "Do the papers contain names, dates, symbols, or another clue?",
                "Do the scuffed boards, bed position, or window marks reveal how the disturbance happened?",
                "Does touching or drawing a paper closer trigger movement from the bed or room?",
            ],
            candidate_actions=[
                "Photograph the bed-centered disturbance, papers, and window or woodwork marks",
                "Use the walking stick to test only the nearest reachable bed edge or paper from the threshold",
                "Retreat immediately if the bed, papers, wardrobe, or floor moves by itself",
            ],
            selection_rationale=(
                "The last threshold probe succeeded and produced concrete visible leads, so the player follows those leads from cover instead of asking for a generic next affordance."
            ),
            declared_action=(
                f"{pc} treats the bedroom observation as a successful lead, not as another question to ask. "
                "Staying on the landing side of the threshold with the doorway and stairs clear, she first "
                "photographs the bed's wrong position, the scuffed floorboards, the loose papers, and the "
                "window-frame or woodwork marks from where she stands. Then she uses the walking stick only "
                "on what can be reached from the threshold: she tests the nearest bed edge for ordinary give "
                "and tries to hook or draw one loose paper close enough to read without stepping deeper into "
                "the room. If the bed, papers, wardrobe, floor, or any furniture moves by itself, she drops "
                "the attempt and backs toward the landing."
            ),
            intent="inspect_bed_centered_scene_from_threshold",
            requested=[
                "bed/scuffed-floor/window-mark result or clear absence",
                "paper text, symbol, date, name, or clear unreadable/blank result",
                "check and outcome if reaching or reading is uncertain",
                "movement or hazard reaction and Evelyn position",
            ],
            unacceptable=[
                "Ask another generic latest-affordance clarification",
                "Treat a successful threshold observation as no-yield",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        (
            moving_bedroom_threat_visible(state.last_reply)
            or bedroom_still_unsafe_after_threshold_probe_visible(state.last_reply)
        )
        and "moving_bedroom_threat" in state.attempted
        and "bedroom_threshold_probe" in state.attempted
        and "bedroom_abandoned" not in state.attempted
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Stop probing the unsafe bedroom and switch to a different established line.",
            hypotheses=[
                "The bedroom produced self-moving furniture, and the threshold probe did not make it safe or reveal a decisive reachable clue.",
                "A cautious player should not keep asking for the same affordance at an unsafe doorway.",
                "The already mapped basement route remains a separate concrete line of inquiry.",
            ],
            open_questions=[
                "Can Evelyn withdraw from the bedroom area without the furniture pursuing?",
                "Does closing or marking the bedroom door change the immediate threat?",
                "What concrete facts are available from the basement route instead?",
            ],
            candidate_actions=[
                "Withdraw from the landing and mark the bedroom as unsafe",
                "Close or brace the bedroom door if possible without entering",
                "Return to the ground floor and investigate the mapped basement route",
            ],
            selection_rationale=(
                "After the self-moving bedroom remains unsafe after a threshold probe, the cautious move is to preserve safety and "
                "change to another known lead rather than reach for an unsafe paper."
            ),
            declared_action=(
                f"{pc} treats the unsafe bedroom as a warning, not an invitation to "
                "keep probing the same doorway. She backs away from the threshold, keeps the stairs and "
                "landing clear, and marks this bedroom as unsafe in her notes. If she can pull the door "
                "partly shut from the landing side without entering, she does so; otherwise she simply "
                "keeps distance. She then withdraws to the ground floor and goes to the already mapped "
                "basement route, keeping the exterior exit path in mind before testing the way down."
            ),
            intent="abandon_unsafe_bedroom_after_repeated_movement",
            requested=[
                "whether the bedroom threat pursues or stops",
                "Evelyn's position after withdrawing",
                "whether the basement route is reachable from the ground floor",
                "check and consequence if the withdrawal is contested",
            ],
            unacceptable=[
                "Repeat the same paper/bed affordance description",
                "Move Evelyn into the unsafe bedroom to take the paper for free",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        unproductive_bedroom_threshold_probe_visible(state.last_reply)
        and "bedroom_threshold_probe" in state.attempted
        and "bedroom_abandoned" not in state.attempted
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Stop spending turns on an unproductive bedroom threshold probe and switch to another established line.",
            hypotheses=[
                "The bedroom doorway probe was resolved as a failure or no-yield result, not as a new clue.",
                "A cautious player records the failed route and changes method instead of repeatedly asking for the same affordance.",
                "The mapped ground-floor and basement routes remain available for a different line of inquiry.",
            ],
            open_questions=[
                "Can Evelyn withdraw from the bedroom threshold without triggering a consequence?",
                "Is the basement route reachable from the ground floor after she leaves the landing?",
                "Does the Keeper introduce any new consequence while she changes routes?",
            ],
            candidate_actions=[
                "Back away from the bedroom threshold and mark it as inconclusive",
                "Return to the ground floor with the upstairs route still noted",
                "Switch to the mapped basement route if it remains the strongest unresolved physical lead",
            ],
            selection_rationale=(
                "The last bedroom probe failed to produce a concrete clue or hazard. A player-like cautious investigator "
                "changes route after that result instead of looping on clarification."
            ),
            declared_action=(
                f"{pc} accepts the failed bedroom-threshold probe as a real result. She does not ask for the "
                "same doorway affordance again and does not step deeper into the stale room. She backs away to "
                "the landing, notes the bedroom as inconclusive for now, and returns to the ground floor with "
                "her retreat path clear. From that stable position, she shifts attention to the already mapped "
                "basement route as the next unresolved physical lead, unless the withdrawal itself triggers a "
                "check or consequence."
            ),
            intent="abandon_unproductive_bedroom_after_threshold_probe",
            requested=[
                "whether withdrawal from the bedroom threshold is uncontested or needs a check",
                "Evelyn's updated position after leaving the landing",
                "whether the mapped basement route is reachable",
                "any concrete consequence caused by changing routes",
            ],
            unacceptable=[
                "Repeat the same bedroom threshold description",
                "Ask another generic latest-affordance clarification",
                "Offer an explicit action menu to the player",
            ],
        )

    if moving_bedroom_threat_visible(state.last_reply) and "moving_bedroom_threat" not in state.attempted:
        return base_decision(
            state,
            persona=persona,
            active_goal="Get out of the immediate moving-bedroom hazard without inventing the source.",
            hypotheses=[
                "The bed or furniture moved by itself, so the next action must resolve safety, not ask for the same affordance again.",
                "The source is not yet proven to be Corbitt, so Evelyn should not attack an unseen body.",
            ],
            open_questions=[
                "Can Evelyn keep the landing and stair retreat clear?",
                "Does the moving bed/furniture strike, block, or stop?",
                "Is a Dodge, sanity, or physical consequence required?",
            ],
            candidate_actions=[
                "Retreat to the landing and keep the doorway between Evelyn and the moving bed",
                "Brace behind the doorframe and avoid entering the room",
                "Only attack if a visible source or weapon appears",
            ],
            selection_rationale=(
                "A real cautious player responds to the immediate impossible movement before continuing investigation."
            ),
            declared_action=(
                f"{pc} treats the bed's impossible movement as the immediate hazard. She keeps backing onto "
                "the landing, stays outside the bedroom threshold, and puts the doorframe and stairs between "
                "herself and the moving furniture. She does not re-enter the room or shoot at an unseen source. "
                "If the bed or furniture lunges through the doorway, she dodges toward the marked stair route; "
                "if it stops, she holds position and listens for the next concrete sign."
            ),
            intent="retreat_from_moving_bedroom_threat",
            requested=[
                "Dodge/escape check or clear no-check result",
                "whether the moving bed/furniture strikes, blocks, or stops",
                "Evelyn position after retreat",
                "sanity or harm consequence if applicable",
            ],
            unacceptable=[
                "Repeat the same bedroom description without resolving the movement",
                "Treat an unseen source as Walter Corbitt",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        opened_basement_crawl_space_visible(state.last_reply)
        and "crawl_space_probe" not in state.attempted
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Inspect the opened crawl-space entrance and Chapel carving without inventing extra basement facts.",
            hypotheses=[
                "The crawl-space entrance is the current physical lead, not a reason to abandon the basement.",
                "The carved Chapel of Contemplation words are visible evidence that should be recorded before any deeper commitment.",
            ],
            open_questions=[
                "How deep does the crawl space go from the entrance?",
                "Are there visible objects, remains, hazards, or further openings inside?",
                "Does the carved Chapel text connect to the earlier public-record lead without requiring a new location jump?",
            ],
            candidate_actions=[
                "Photograph and transcribe the carved words from outside the crawl-space entrance",
                "Use flashlight, walking stick, and camera angle to inspect the visible interior",
                "Withdraw if anything moves, breathes, or blocks the marked stair retreat",
            ],
            selection_rationale=(
                "The latest reply opened a basement crawl space and exposed a visible Chapel carving; "
                "a grounded player resolves that local affordance before changing floors or locations."
            ),
            declared_action=(
                f"{pc} stays at the opened basement crawl-space entrance rather than leaving the line. "
                "Keeping the marked stairs behind her clear, she photographs the narrow opening and "
                "transcribes the carved words 'Chapel of Contemplation' exactly in her notebook. Without "
                "crawling in yet, she angles the flashlight and camera into the crawl space and uses the "
                "walking stick only to test the immediate lip, floor, and reachable inner surface. She looks "
                "for visible depth, side passages, objects, remains, tools, ritual marks, or movement. If "
                "anything shifts, breathes, attacks, or blocks retreat, she pulls back toward the stairs."
            ),
            intent="inspect_opened_crawl_space_and_chapel_carving",
            requested=[
                "visible crawl-space depth or obstruction",
                "carving record and any new non-recorded visible marks",
                "object/remains/hazard presence or clear absence",
                "check and consequence if inspection is uncertain",
            ],
            unacceptable=[
                "Move Evelyn upstairs or to the Chapel before resolving the opened basement crawl space",
                "Claim disturbed earth or scrape marks unless they are in the latest visible facts",
                "Repeat the carved words as a separate second discovery",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        opened_basement_crawl_space_visible(state.last_reply)
        and "crawl_space_probe" in state.attempted
        and "crawl_space_entry" not in state.attempted
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Push the local crawl-space lead only as far as the marked exit remains controlled.",
            hypotheses=[
                "The visible carved words are already recorded; the unresolved question is what the newly exposed crawl space contains.",
                "Nothing has lunged at the opening, so a minimal controlled entry may be justified, but only with retreat preserved.",
            ],
            open_questions=[
                "Does the crawl space contain remains, objects, a new non-recorded physical mark, or another passage?",
                "Can Evelyn keep her feet and retreat line oriented toward the basement stairs?",
                "Does anything move, breathe, or block the opening once she commits weight inside?",
            ],
            candidate_actions=[
                "Inspect one body length into the crawl space while keeping the exit controlled",
                "Probe deeper only with the walking stick and light before moving",
                "Abort immediately if the opening shifts, narrows, or something reacts",
            ],
            selection_rationale=(
                "The last reply kept the active lead in the opened basement crawl space; leaving for another "
                "location would ignore the current visible opening."
            ),
            declared_action=(
                f"{pc} keeps the basement stairs and the opened crawl-space lip marked behind her. Since nothing "
                "lunged at the entrance, she commits only minimally: she lowers herself just far enough to bring "
                "her flashlight and camera one body length into the crawl space, keeping her feet and retreat "
                "line oriented toward the opening. She probes ahead with the walking stick before shifting any "
                "weight. The already recorded wall text is context, not a new discovery; she looks only for "
                "new non-recorded physical facts such as remains, objects, tools, side passages, airflow changes, "
                "movement, or a new non-recorded physical mark. If the space narrows, shifts, answers with "
                "movement, or threatens to trap her, she backs out immediately toward the marked stairs."
            ),
            intent="enter_crawl_space_one_body_length_from_marked_exit",
            requested=[
                "crawl-space contents within one body length",
                "whether retreat remains open",
                "check and consequence for confined entry",
                "object/remains/tool/passage/airflow/movement/new non-recorded mark presence or clear absence",
            ],
            unacceptable=[
                "Jump to the Chapel location before resolving the opened crawl space",
                "Repeat the already recorded crawl-space wall text as a new discovery",
                "Use Michael Thomas or 1912 unless those facts are in the visible transcript",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        ("crawl_space_probe" in state.attempted or "basement_concealed_probe" in state.attempted)
        and "deeper_recess_papers_probe" not in state.attempted
        and deeper_recess_table_papers_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Turn the deeper recess table papers into a bounded, auditable inspection.",
            hypotheses=[
                "The papers on the small table are the nearest distinct information-bearing object.",
                "The foul pocket may contain a hazard, so the inspection must preserve the breach and basement retreat line.",
            ],
            open_questions=[
                "Can Evelyn photograph or read the table papers from the breach or one controlled step inside?",
                "Does the foul pocket contain a body, person, movement, or only old debris?",
                "Does reaching toward the papers trigger a check, attack, smell consequence, or retreat problem?",
            ],
            candidate_actions=[
                "Photograph the table and papers from the breach before moving",
                "Take one controlled step or lean in only far enough to hook or read the nearest paper",
                "Back out immediately if the smell worsens, anything moves, or retreat narrows",
            ],
            selection_rationale=(
                "The Keeper named the papers as the clearest actionable object; repeating the same grounding question would be a player-sim loop."
            ),
            declared_action=(
                f"{pc} acts on the papers instead of asking for the same breach description again. "
                "Keeping the broken opening, basement route, and stairs behind her as the retreat line, "
                "she first photographs the small table and the papers from the mouth of the recess. "
                "If the floor and air seem stable enough for a single bounded commitment, she takes only "
                "one careful step or lean, using the walking stick to hook or steady the nearest paper "
                "rather than putting both hands into the dark. She tries to read or recover only what is "
                "reachable from that position. If the smell spikes, anything moves, the table shifts, or "
                "the opening threatens to trap her, she backs out through the breach immediately."
            ),
            intent="inspect_deeper_recess_table_papers_from_breach",
            requested=[
                "whether the bounded inspection succeeds, fails, or needs a check",
                "paper content or clear blank/ruined result",
                "body/person/movement/debris presence or clear absence from the foul pocket",
                "Evelyn position and retreat status after the attempt",
            ],
            unacceptable=[
                "Repeat the same broken-opening description",
                "Move Evelyn fully into the recess without resolving risk",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        ("crawl_space_probe" in state.attempted or "basement_concealed_probe" in state.attempted)
        and "broken_cellar_opening_probe" not in state.attempted
        and broken_cellar_opening_affordance_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Resolve the reachable broken cellar opening instead of repeating clarification.",
            hypotheses=[
                "The broken opening is the current physical threshold between known cellar and hidden space.",
                "A cautious investigator can test the lip and sightline without fully committing through it.",
            ],
            open_questions=[
                "What is visible from the lip of the broken opening?",
                "Does the foul air indicate a body, person, chemical rot, or blocked space?",
                "Does touching or approaching the opening trigger movement, collapse, or a check?",
            ],
            candidate_actions=[
                "Advance only to the lip of the broken opening",
                "Use flashlight, camera, and walking stick to inspect one reach beyond the breach",
                "Withdraw if the opening shifts, the smell worsens sharply, or anything moves",
            ],
            selection_rationale=(
                "After repeated grounding, the opening is close and reachable; a player-like simulator now tests it with bounded risk."
            ),
            declared_action=(
                f"{pc} stops asking where the broken opening is. With the basement route and stairs behind her, "
                "she advances only to the lip of the cellar-wall break, keeping one foot and her shoulders angled "
                "for retreat. She shines the flashlight through first, photographs the rough masonry and the dark "
                "space beyond, then probes one controlled reach past the edge with the walking stick. She looks for "
                "a body, person, object, paper, passage, movement, or a clear absence. If the air worsens sharply, "
                "the edge crumbles, anything moves, or the route narrows, she backs away toward the marked basement stairs."
            ),
            intent="probe_broken_cellar_opening_from_retreat_line",
            requested=[
                "visible result from the lip of the opening",
                "check and consequence if approach is risky",
                "body/person/object/passage presence or clear absence",
                "Evelyn position and retreat status",
            ],
            unacceptable=[
                "Repeat the same opening location description",
                "Force full entry without resolving the threshold probe",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "crawl_space_entry" in state.attempted
        and "chapel" not in state.attempted
        and "chapel of contemplation" in state.transcript.lower()
        and crawl_space_risk_with_safe_withdrawal_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Withdraw safely and follow the documented Chapel lead instead of repeating the same crawl-space probe.",
            hypotheses=[
                "The Chapel of Contemplation carving is already a player-visible recorded clue.",
                "The crawl space has enough local risk and enough local information to justify withdrawal for the external lead.",
                "A cautious investigator should not keep pushing a tight rat-occupied passage when a safer daylight lead exists.",
            ],
            open_questions=[
                "Can Evelyn back out cleanly to the marked basement stairs?",
                "What visible records, symbols, or Corbitt links exist at the Chapel site?",
                "Does the Chapel lead clarify why the hidden basement passage was marked?",
            ],
            candidate_actions=[
                "Back out of the crawl space and secure the basement route",
                "Investigate the Chapel of Contemplation in daylight",
                "Return later with help or equipment if the Chapel does not clarify the crawl-space risk",
            ],
            selection_rationale=(
                "The latest reply gave risk in the crawl space, confirmed a safe withdrawal, and the Chapel "
                "carving is already recorded; continuing to ask for the same kind of deeper crawl-space clue "
                "would become a player-sim loop."
            ),
            declared_action=(
                f"{pc} backs out of the crawl space immediately while the retreat is still clean, keeping "
                "her notebook, camera, flashlight, and walking stick oriented toward the marked basement stairs. "
                "Once she is clear of the crawl-space lip, she notes that the hidden passage remains dangerous "
                "and deliberately marked, but does not keep pushing deeper alone. She secures her route out of "
                "the house, waits for daylight if needed, and goes to the Chapel of Contemplation. Before entering "
                "any unstable interior there, she circles the exterior for exits, fresh tracks, or recent use, then "
                "searches surviving offices, cabinets, pulpits, storage, wall marks, and loose papers for records, "
                "symbols, or any link to Corbitt or the Corbitt House. She photographs distinctive evidence before "
                "moving it."
            ),
            intent="withdraw_from_unresolved_crawl_space_and_investigate_chapel",
            requested=[
                "whether withdrawal from the crawl space succeeds or triggers a consequence",
                "Evelyn position after withdrawal and travel boundary",
                "Chapel exterior safety facts",
                "visible Chapel records, symbols, or Corbitt/house link, or a clear failed search",
            ],
            unacceptable=[
                "Rediscover the already recorded Chapel carving as if it were new",
                "Force Evelyn deeper into the rat-occupied crawl space after she withdraws",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "crawl_space_entry" in state.attempted
        and "crawl_space_withdrawal" not in state.attempted
        and (
            limited_crawl_space_no_new_leads_withdrawal_visible(state.last_reply)
            or (
                "crawl_space_deeper_probe" in state.attempted
                and repeated_crawl_space_probe_no_new_facts_visible(state.last_reply)
            )
        )
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Stop treating a no-yield confined-space failure as an invitation to force another clue.",
            hypotheses=[
                "The latest crawl-space result was adjudicated as a failure and exposed no concrete remains, tools, passage, markings, or airflow.",
                "Going farther would require a new level of body commitment rather than merely a better angle.",
                "A cautious investigator should withdraw to the marked stairs and change resources before repeating the same probe.",
            ],
            open_questions=[
                "Can Evelyn back out to the marked basement stairs cleanly?",
                "Does anything move, pursue, or block the opening during withdrawal?",
                "What tools, light, line, or helper would be needed before risking deeper entry?",
            ],
            candidate_actions=[
                "Back out to the marked basement stairs",
                "Keep the flashlight and walking stick trained on the opening while withdrawing",
                "Leave the crawl space unresolved until she has better resources or help",
            ],
            selection_rationale=(
                "The player-visible result says the limited probe failed and yielded no new local lead; continuing deeper now "
                "would pressure the Keeper to repeat or invent information instead of respecting the failed result."
            ),
            declared_action=(
                f"{pc} accepts that this limited crawl-space angle did not produce a new usable lead. "
                "She backs out toward the marked basement stairs while the retreat is still open, keeping the "
                "flashlight on the opening and the walking stick between herself and the dark. Once clear, she "
                "records that the wall text or marker is already documented, but that this probe found no clear "
                "remains, tools, side passage, fresh mark, or trustworthy airflow. She does not commit more of "
                "her body into the tight passage alone; she reassesses from the stair-side position and plans "
                "to return only with stronger light, a line, a suitable tool, and if possible a witness or helper."
            ),
            intent="withdraw_from_unresolved_crawl_space_to_marked_stairs",
            requested=[
                "whether Evelyn clears the crawl-space opening and reaches the marked stairs",
                "any movement, pursuit, or collapse during withdrawal",
                "updated position after withdrawal",
                "resource requirement for any later deeper entry",
            ],
            unacceptable=[
                "Repeat the same crawl-space wall clue as a new discovery",
                "Force Evelyn deeper after she declares withdrawal",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "crawl_space_entry" in state.attempted
        and "crawl_space_deeper_probe" not in state.attempted
        and crawl_space_entry_unresolved_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Resolve the crawl-space position Evelyn is already occupying before changing locations.",
            hypotheses=[
                "Evelyn is half inside the crawl space, so the current position must be stabilized before following the Chapel lead elsewhere.",
                "The last reply says the passage continues and the deeper details remain unresolved, with retreat still intact.",
            ],
            open_questions=[
                "What is visible just beyond the first reach of the light?",
                "Does the crawl space turn, branch, open into a chamber, or contain remains or objects?",
                "Can Evelyn keep the marked opening and basement stairs as a controlled retreat route?",
            ],
            candidate_actions=[
                "Probe one reach deeper from the current half-in position",
                "Photograph the deeper darkness and inner wall angles before moving",
                "Back out immediately if the space narrows, shifts, or threatens retreat",
            ],
            selection_rationale=(
                "A real cautious player does not abandon a half-entered crawl space for a remote lead while "
                "the local passage is still unresolved and physically occupied."
            ),
            declared_action=(
                f"{pc} does not leave for the Chapel while she is still half inside the crawl space. "
                "She first confirms that the marked opening, basement stairs, and handkerchief route are still "
                "behind her. From that braced position, she extends only the flashlight, camera, and walking stick "
                "one careful reach farther toward the deeper darkness and inner wall angles. She looks for a turn, "
                "side branch, chamber, remains, objects, blocked airflow, movement, or a new non-recorded physical "
                "mark. The already recorded wall text is not a new result. If the space narrows, shifts, answers "
                "with movement, or threatens to trap her, she backs out toward the marked stairs instead of pressing deeper."
            ),
            intent="probe_deeper_crawl_space_from_marked_exit",
            requested=[
                "deeper crawl-space visible result",
                "turn/branch/chamber/object/remains/airflow/movement/new non-recorded mark presence or clear absence",
                "whether retreat remains open",
                "check and consequence if the confined probe is uncertain",
            ],
            unacceptable=[
                "Jump to the Chapel location before resolving the half-entered crawl space",
                "Repeat the already recorded crawl-space wall text as a new discovery",
                "Repeat the same crawl-space lip description without a new result",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "crawl_space_entry" in state.attempted
        and "crawl_space_deeper_probe" not in state.attempted
        and crawl_space_mouth_affordance_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Act on the clarified crawl-space mouth instead of asking for the same affordance again.",
            hypotheses=[
                "The Keeper has already grounded the crawl-space mouth, darkness beyond the beam, and retreat path.",
                "A cautious player can either probe one controlled reach farther or withdraw; repeating clarification is no longer useful.",
            ],
            open_questions=[
                "Does the low opening reveal a turn, chamber, object, remains, a new non-recorded physical mark, or only more darkness?",
                "Does the crawl space react when Evelyn extends light and stick one reach farther?",
                "Does the marked basement-stair retreat remain open?",
            ],
            candidate_actions=[
                "Probe one controlled reach beyond the crawl-space mouth",
                "Photograph the near lip and darker stretch before shifting weight",
                "Back out immediately if the space narrows, shifts, or threatens retreat",
            ],
            selection_rationale=(
                "The crawl-space mouth has been clarified enough to act on; a player-like simulator now converts that affordance into a bounded probe."
            ),
            declared_action=(
                f"{pc} stops asking for the same crawl-space description. Keeping the marked basement stairs "
                "behind her and the opening in front of her, she photographs the rough boards, packed dirt, "
                "and the first dark stretch. Then she extends only the flashlight, camera, and walking stick "
                "one controlled reach past the crawl-space mouth, without committing her full body farther in. "
                "The already recorded wall text is context, not a new result; she watches only for a turn, widening, "
                "object, remains, airflow change, movement, or a new non-recorded physical mark. If the space narrows, "
                "shifts, answers with movement, or threatens her retreat, she backs out toward the marked stairs."
            ),
            intent="probe_deeper_crawl_space_from_marked_exit",
            requested=[
                "deeper crawl-space visible result",
                "turn/branch/chamber/object/remains/airflow/movement/new non-recorded mark presence or clear absence",
                "whether retreat remains open",
                "check and consequence if the confined probe is uncertain",
            ],
            unacceptable=[
                "Repeat the same crawl-space mouth description",
                "Repeat the already recorded crawl-space wall text as a new discovery",
                "Ask for another generic latest-affordance clarification",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        ("crawl_space_entry" in state.attempted or "crawl_space_probe" in state.attempted)
        and "crawl_space_withdrawal" not in state.attempted
        and unstable_crawl_space_failure_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Stop probing the unstable crawl-space failure and withdraw to change resources.",
            hypotheses=[
                "The last crawl-space attempt failed and made the opening physically less reliable.",
                "The retreat line is still open, so a cautious player should use it before the space worsens.",
                "Continuing to probe the same unstable gap would ignore the failed result and risk entrapment.",
            ],
            open_questions=[
                "Can Evelyn back fully out to the marked basement stairs without the gap shifting again?",
                "Does anything move or pursue while she withdraws?",
                "Can she leave the house and return later with tools, light, and help?",
            ],
            candidate_actions=[
                "Back out to the marked basement stairs immediately",
                "Secure the opening only from a safe distance",
                "Leave for better tools, light, or a witness rather than continuing alone",
            ],
            selection_rationale=(
                "A failed confined-space probe with collapse or loose soil changes the problem from information gathering "
                "to safety and resources."
            ),
            declared_action=(
                f"{pc} treats the failed crawl-space probe as a real danger signal, not an invitation to keep "
                "asking for the same description. She backs fully out toward the marked basement stairs while "
                "the retreat line is still open, keeping the flashlight on the unstable gap and the walking stick "
                "between herself and the opening. Once clear, she marks the crawl-space mouth as unsafe, does not "
                "reach into it again, and leaves the house by her marked route to return only with better light, "
                "a proper tool, rope or line, and if possible a witness or helper. If the gap shifts, something "
                "moves, or retreat is contested, she stops and resolves that threat before leaving."
            ),
            intent="withdraw_from_unstable_crawl_space_after_failed_probe",
            requested=[
                "whether Evelyn clears the crawl-space mouth and reaches the marked stairs",
                "any movement, collapse, or pursuit during withdrawal",
                "updated position after leaving the unstable gap",
                "tool/help requirement or blocked reason for return",
            ],
            unacceptable=[
                "Repeat the same crawl-space affordance description",
                "Probe deeper after the failed unstable result without resolving withdrawal",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "crawl_space_withdrawal" in state.attempted
        and "basement_tools_return" not in state.attempted
        and outside_after_crawl_space_exit_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Convert the completed crawl-space withdrawal into a safer return plan.",
            hypotheses=[
                "Evelyn is already outside the house, so the crawl-space opening is no longer her current physical position.",
                "The crawl-space result established a resource problem: better light, line, tools, and preferably a witness.",
                "Continuing to probe from inside would contradict the latest visible position.",
            ],
            open_questions=[
                "Can Evelyn gather stronger light, a rope or line, a proper tool, and a helper or witness?",
                "Does returning to the house require waiting, a check, or another visible complication?",
                "Is the documented crawl-space route unchanged when she returns?",
            ],
            candidate_actions=[
                "Secure notes and photographs outside",
                "Get stronger light, a line, tools, and if possible a witness",
                "Return only after the resource change is resolved",
            ],
            selection_rationale=(
                "The latest reply moved Evelyn outside and named the missing resources, so the player should change resources rather than act as if she remains in the crawl space."
            ),
            declared_action=(
                f"{pc} stays outside for the moment instead of pretending she is still at the crawl-space lip. "
                "She secures the photographs and notes, marks the house entry and basement route in her notebook, "
                "and gathers what the last retreat showed she needs: stronger light, a rope or line, a proper tool "
                "for the broken boards or tight opening, and if possible a witness or helper. Once those resources "
                "are accounted for, she intends to return by the marked exterior and basement route rather than probe blind again."
            ),
            intent="leave_house_for_tools_after_crawl_space_withdrawal",
            requested=[
                "whether tools, light, line, and helper/witness are obtained or blocked",
                "time passage or consequence while outside",
                "whether the marked route remains available on return",
                "check and outcome if resource gathering is uncertain",
            ],
            unacceptable=[
                "Continue probing the crawl space as if Evelyn were still inside",
                "Repeat the same outside-withdrawal description",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "crawl_space_entry" in state.attempted
        and "crawl_space_deeper_probe" not in state.attempted
        and current_crawl_space_affordance_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Finish resolving the crawl-space entry before changing floors or scenes.",
            hypotheses=[
                "Evelyn is still committed to the crawl-space lead, and the latest result left the deeper space unresolved.",
                "The failed look did not reveal a safe final answer; it only confirmed concealment and an open retreat.",
                "A cautious player can make one bounded deeper probe before deciding to withdraw for tools or help.",
            ],
            open_questions=[
                "Does one controlled reach deeper reveal a chamber, object, remains, new non-recorded physical mark, airflow, or passage?",
                "Does the deliberately concealed under-space react when probed from the marked exit line?",
                "Can Evelyn still back out to the basement stairs after this bounded probe?",
            ],
            candidate_actions=[
                "Probe one controlled reach deeper from the current crawl-space position",
                "Photograph and mark the current limits before moving anything farther",
                "Back out immediately if the space narrows, shifts, or threatens the retreat",
            ],
            selection_rationale=(
                "The last reply kept the active problem inside the crawl space. Following an upstairs cue here would "
                "break player-visible continuity, so the simulator resolves the current confined position first."
            ),
            declared_action=(
                f"{pc} does not treat the basement stairs behind her as a new route away from the crawl space. She stays oriented on the "
                "crawl-space opening she is already investigating, marks her current reach in her notebook, and extends "
                "only the flashlight, camera, and walking stick one controlled reach deeper into the deliberately shut-away "
                "under-space. The already recorded wall text is context, not a new discovery; she is looking only for "
                "a chamber, object, remains, side passage, airflow change, movement, or a new non-recorded physical mark. "
                "If the space narrows, shifts, answers her probe, or threatens the marked retreat to the basement stairs, "
                "she backs out immediately rather than pressing farther."
            ),
            intent="probe_deeper_crawl_space_from_marked_exit",
            requested=[
                "deeper crawl-space visible result",
                "object/remains/passage/airflow/movement/new non-recorded mark presence or clear absence",
                "whether the marked retreat to the basement stairs remains open",
                "check and consequence if the controlled probe is risky or uncertain",
            ],
            unacceptable=[
                "Move Evelyn upstairs or away from the crawl space before resolving this position",
                "Repeat the already recorded crawl-space wall text as a new discovery",
                "Treat the basement stairs behind her as a new upper-floor route",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "crawl_space_deeper_probe" in state.attempted
        and "crawl_space_withdrawal" not in state.attempted
        and "crawl_space_wall_inspection" not in state.attempted
        and crawl_space_chapel_wall_reinspection_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Keep the Chapel carving anchored in the basement crawl space and inspect the local wall.",
            hypotheses=[
                "The Chapel of Contemplation words are a local carving in the crawl-space wall, not proof that Evelyn arrived at the chapel.",
                "The deeper probe found man-made marks but not yet the wall's local context, contents, or hazard.",
            ],
            open_questions=[
                "Do the cut marks, wall surface, or adjacent dirt reveal a chamber, object, remains, or only the repeated carving?",
                "Does closer local inspection change Evelyn's position or threaten her retreat?",
                "Is there a clear reason to withdraw for tools or help after this local check?",
            ],
            candidate_actions=[
                "Photograph and inspect the crawl-space wall and adjacent surface locally",
                "Probe only the immediate wall, dirt, and timber around the carving",
                "Back out if the cramped space shifts, reacts, or still yields no new safe lead",
            ],
            selection_rationale=(
                "The same Chapel words are visible inside the crawl space; the player must resolve that local affordance "
                "instead of treating the words as a scene transition."
            ),
            declared_action=(
                f"{pc} does not treat the carved words as arrival at the Chapel. She stays oriented on "
                "the crawl-space wall where the words are actually visible, with the marked basement stairs "
                "behind her. From the same braced position, she photographs the lettering, then uses the "
                "walking stick and low flashlight angle to inspect only the adjacent wall surface, packed dirt, "
                "timber, and any seam or hollow immediately around the carving. She looks for a chamber edge, "
                "object, remains, airflow change, clear absence, or a new non-recorded physical mark. The already "
                "recorded wall text itself is not a new result. If the space shifts, narrows, reacts, or still yields "
                "no safe new lead, she backs out toward the marked stairs rather than changing locations by assumption."
            ),
            intent="inspect_reconfirmed_crawl_space_chapel_wall_locally",
            requested=[
                "local wall/soil/timber result around the carving",
                "chamber/object/remains/airflow/new non-recorded mark presence or clear absence",
                "whether retreat to the basement stairs remains open",
                "check and consequence if the close inspection is risky",
            ],
            unacceptable=[
                "Move Evelyn to the Chapel because the carving says Chapel of Contemplation",
                "Repeat only the same carving text without local consequence or clear no-further-lead",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "crawl_space_wall_inspection" in state.attempted
        and "crawl_space_inner_boundary_probe" not in state.attempted
        and crawl_space_inner_boundary_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Resolve the newly exposed crawl-space inner boundary from the braced position.",
            hypotheses=[
                "The wall inspection produced a concrete local boundary, not a reason to ask for another generic affordance.",
                "The boundary is not yet an open chamber or object, so the next action must test it cautiously.",
                "Evelyn should preserve the retreat line and avoid treating the Chapel words as a scene jump.",
            ],
            open_questions=[
                "Does the inner boundary open, resist, sound hollow, or reveal only packed material?",
                "Does testing it trigger collapse, movement, or a need to withdraw?",
                "Can Evelyn keep the crawl-space exit and basement stairs behind her while testing it?",
            ],
            candidate_actions=[
                "Photograph and mark the boundary next to the carving",
                "Probe only the edge and texture change with walking stick and notebook edge",
                "Withdraw for tools if it resists or threatens the retreat",
            ],
            selection_rationale=(
                "A concrete success exposed a local seam-like boundary; a player-like investigator now tests that result "
                "instead of looping on explanation."
            ),
            declared_action=(
                f"{pc} treats the newly noticed inner boundary as the current local result. She does not ask "
                "for the same crawl-space description again and does not treat the Chapel words as a teleport. "
                "Keeping the crawl-space exit and basement stairs behind her, she photographs the boundary beside "
                "the lettering, marks its position in her notebook, and uses only the walking stick and notebook edge "
                "to test the straighter line and texture change. She listens for hollow space, watches for shifting dirt, "
                "and stops immediately if the boundary resists, crumbles, moves, or threatens her retreat."
            ),
            intent="probe_crawl_space_inner_boundary_from_braced_position",
            requested=[
                "whether the inner boundary opens, resists, or sounds hollow",
                "visible contents or clear absence",
                "collapse/movement/threat consequence if triggered",
                "Evelyn position and retreat status",
            ],
            unacceptable=[
                "Repeat the same crawl-space continuation description",
                "Move Evelyn to the Chapel because of the carved words",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "crawl_space_deeper_probe" in state.attempted
        and "crawl_space_withdrawal" not in state.attempted
        and crawl_space_mouth_affordance_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Break the crawl-space clarification loop by withdrawing to a stable position.",
            hypotheses=[
                "A deeper probe still did not expose a new concrete fact beyond the same crawl-space mouth.",
                "A cautious human player changes strategy after repeated low-information crawl-space results.",
            ],
            open_questions=[
                "Can Evelyn back out to the marked basement stairs safely?",
                "Does anything react while she withdraws?",
                "From the stable basement position, should she leave, get tools/help, or try another already visible route?",
            ],
            candidate_actions=[
                "Back out from the crawl-space mouth to the marked basement stairs",
                "Keep the light on the opening while retreating",
                "Leave the house if the crawl space remains unreadable without better tools",
            ],
            selection_rationale=(
                "After the same mouth-of-crawl-space information repeats, the realistic cautious move is to withdraw and reassess, not ask again."
            ),
            declared_action=(
                f"{pc} accepts that this angle is not producing a new visible result. She backs out of the "
                "crawl-space mouth toward the marked basement stairs, keeping the flashlight on the opening "
                "and the walking stick between herself and the dark. Once she is clear of the low opening, "
                "she notes the crawl space as unresolved, checks whether anything follows or blocks the exit, "
                "and prepares to reassess from the safer basement-stair position rather than repeating the same "
                "clarifying question."
            ),
            intent="withdraw_from_unresolved_crawl_space_to_marked_stairs",
            requested=[
                "whether Evelyn clears the crawl-space mouth",
                "any movement or pursuit from the opening",
                "updated position at the marked basement stairs",
                "safe reassessment options from that position",
            ],
            unacceptable=[
                "Repeat the same crawl-space mouth description without resolving withdrawal",
                "Keep Evelyn half-committed in the crawl space indefinitely",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "crawl_space_withdrawal" in state.attempted
        and "basement_tools_return" not in state.attempted
        and crawl_space_withdrawn_stair_position_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="End the low-information crawl-space push and return only with better resources.",
            hypotheses=[
                "Evelyn has already withdrawn from the crawl-space mouth to a safer stair-side position.",
                "The opening remains unresolved but contained, so repeating the same clarification would stall.",
                "A cautious investigator changes resources after a failed confined-space probe.",
            ],
            open_questions=[
                "Can Evelyn leave the basement and house without pursuit or collapse?",
                "Can she return with better light, rope, tools, or a witness/helper?",
                "Does the crawl-space mouth change while she withdraws from the house?",
            ],
            candidate_actions=[
                "Leave the basement by the marked stairs",
                "Exit the house with notes and photographs intact",
                "Return later with better light, a line, tools, and if possible a witness",
            ],
            selection_rationale=(
                "The local crawl-space angle has been exhausted from a safe position; the player now changes safety resources "
                "instead of asking the Keeper to restate the same opening."
            ),
            declared_action=(
                f"{pc} accepts that the crawl-space mouth remains unresolved but contained at a distance. "
                "She does not cross back to it or ask for the same affordance again. Keeping the flashlight on "
                "the opening, she backs up the marked basement stairs, exits the house by her known route, and "
                "keeps the photographs and notes together. She plans to return only with stronger light, a rope "
                "or line, a proper tool for the broken boards, and if possible a witness or helper. If anything "
                "moves from the opening, follows, or blocks the stairs while she leaves, she stops and resolves "
                "that threat immediately."
            ),
            intent="leave_house_for_tools_after_crawl_space_withdrawal",
            requested=[
                "whether Evelyn exits basement and house safely",
                "any movement or pursuit from the crawl-space mouth",
                "tool/light/helper requirement for a return",
                "blocked reason, check, or consequence if withdrawal is unsafe",
            ],
            unacceptable=[
                "Repeat the same crawl-space opening description",
                "Send Evelyn back into the opening without resolving her declared withdrawal",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "basement_tools_return" in state.attempted
        and "returned_with_tools" not in state.attempted
        and outside_after_crawl_space_exit_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Follow through on the declared tool-and-witness return plan.",
            hypotheses=[
                "Evelyn is outside the house after a successful withdrawal.",
                "The unresolved crawl-space problem requires different resources, not another view description.",
                "A player-like simulator follows through on its last declared plan once the exit succeeds.",
            ],
            open_questions=[
                "Can Evelyn obtain stronger light, rope or line, a suitable tool, and a witness/helper?",
                "Can she re-enter the house and reach the marked basement route without a new obstacle?",
                "Does the crawl-space mouth remain as documented when she returns?",
            ],
            candidate_actions=[
                "Obtain stronger light, rope or line, gloves, and a suitable tool",
                "Ask a trustworthy witness or helper to accompany her if available",
                "Return in daylight to the marked basement route and stop if the house has changed",
            ],
            selection_rationale=(
                "The previous action already chose the safer resource change. Now the simulator executes that plan rather than asking what the house looks like."
            ),
            declared_action=(
                f"{pc} follows through on the withdrawal plan. She leaves the property long enough to obtain "
                "stronger light, a rope or marked line, gloves, and a suitable tool for the broken boards; if a "
                "trustworthy witness or helper is available without delay, she brings them. She then returns in "
                "daylight to the Corbitt House, re-enters by the known route only if it remains unchanged, and heads "
                "for the marked basement/crawl-space route with the exterior exit path kept clear. If the house, "
                "door, basement stairs, or crawl-space mouth has changed, she stops and resolves that change before "
                "going back in."
            ),
            intent="return_with_tools_to_documented_crawl_space_route",
            requested=[
                "whether tools/light/line/helper are obtained or blocked",
                "whether re-entry route is unchanged",
                "current status of the documented basement/crawl-space route",
                "check or consequence if the return is unsafe",
            ],
            unacceptable=[
                "Repeat the exterior house description without resolving the return plan",
                "Reset Evelyn inside the crawl space without narrating re-entry",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "returned_with_tools" in state.attempted
        and "tool_assisted_crawl_space_test" not in state.attempted
        and prepared_return_crawl_space_ready_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Use the changed resources on the documented crawl-space route instead of asking for the same affordance again.",
            hypotheses=[
                "Evelyn has already returned with better light, gloves, a marked line, and a levering tool.",
                "The route appears unchanged, so the next player-like move is a prepared test, not another generic clarification.",
                "A cautious investigator should rig safety first, then make one controlled tool-assisted probe.",
            ],
            open_questions=[
                "Can Evelyn rig the marked line and light without the route changing?",
                "Does the tool-assisted test open, widen, resist, or trigger movement at the crawl-space mouth?",
                "Does anything block the basement stairs or exterior retreat while she works?",
            ],
            candidate_actions=[
                "Rig the marked line and stronger light before touching the opening",
                "Inspect the crawl-space mouth from the stair-side position",
                "Make one controlled tool-assisted test at the broken boards or loose edge",
            ],
            selection_rationale=(
                "The last visible result resolved the resource change and route status. A realistic player now acts with those resources rather than asking the Keeper to repeat the same route description."
            ),
            declared_action=(
                f"{pc} does not pick from the Keeper's phrased options or ask for the same route description again. "
                "She uses the preparation she came back with: first she anchors the marked line so it leads back "
                "toward the basement stairs and exterior exit, sets the stronger light to cover the crawl-space "
                "mouth, and checks from the stair-side position that the opening has not shifted. Then, with gloves "
                "on and her body kept out of the low gap, she makes one controlled tool-assisted test at the broken "
                "boards or loosest edge. If the tool slips, the boards resist, the gap shifts, anything moves inside, "
                "or the retreat line is threatened, she stops and resolves that result instead of forcing entry."
            ),
            intent="use_prepared_tools_on_documented_crawl_space_route",
            requested=[
                "whether the marked line and stronger light are set safely",
                "tool-assisted board/mouth result: opens, widens, resists, shifts, or triggers movement",
                "Evelyn position relative to crawl-space mouth and basement stairs",
                "retreat status and any immediate threat reaction",
            ],
            unacceptable=[
                "Repeat the same crawl-space route description without resolving the prepared tool action",
                "Send Evelyn straight into the gap without resolving line/light/tool setup",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "crawl_space_entry" in state.attempted
        and basement_door_reset_after_crawlspace_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Reject a location reset and restore the last confirmed crawl-space position.",
            hypotheses=[
                "The prior visible state put Evelyn at the opened basement crawl space, not back before an unopened basement door.",
                "A real player should not act on a rewind unless the Keeper explicitly resolves how the retreat happened.",
            ],
            open_questions=[
                "Is Evelyn still at the crawl-space lip/body-length position, or did a concrete retreat occur?",
                "If she retreated, what caused it and where is she now?",
                "What is the next concrete result from the crawl-space position if no retreat occurred?",
            ],
            candidate_actions=[
                "Reject the basement-door reset and ask for continuity repair",
                "Back out from the crawl space to the marked basement stairs if continuity is restored",
                "Withdraw from the basement only after the current position is explicitly resolved",
            ],
            selection_rationale=(
                "The latest reply rewinds to an already-opened basement door after crawl-space progress. "
                "The cautious player preserves continuity instead of asking the same affordance question again."
            ),
            declared_action=(
                f"{pc} stops and objects to the continuity break before acting on the basement-door description. "
                "Her last confirmed position was at the opened basement crawl space, with the marked stairs behind her; "
                "she had already opened the basement route and entered one body length into the crawl space. "
                "Unless the Keeper states a concrete retreat or forced transition that brought her back to the "
                "ground-floor hall, she treats the crawl-space lip as the current position. From there she backs out "
                "carefully to the marked basement stairs, confirms the crawl-space opening and Chapel carving are still "
                "where she left them, and only then reassesses her next move."
            ),
            intent="reject_basement_door_state_reset_after_crawlspace",
            requested=[
                "continuity repair for Evelyn's current position",
                "explicit retreat/transition cause if she is no longer at the crawl space",
                "crawl-space opening status and marked stair retreat",
                "no reuse of the unopened basement-door state unless justified",
            ],
            unacceptable=[
                "Describe the basement door as unopened without explaining the reset",
                "Ignore the established crawl-space opening and Chapel carving",
                "Offer an explicit action menu to the player",
            ],
        )

    if "hall_records" not in state.attempted:
        return base_decision(
            state,
            persona=persona,
            active_goal="Find property, probate, executor, and ownership records.",
            hypotheses=[
                "A public title or probate trail can produce concrete names, dates, and institutions."
            ],
            open_questions=[
                "Who owned the house before Knott?",
                "Does any probate file name an executor or prior estate?",
                "Do the public records point to a next institution or person?",
            ],
            candidate_actions=[
                "Search Hall of Records for title and probate records",
                "Search newspaper archives for Corbitt House and Macario reports",
                "Observe the house exterior from the street",
            ],
            selection_rationale="The public records path is low-risk and directly follows Knott's briefing.",
            declared_action=(
                f"{pc} puts away Knott's usable address lead, keys, and commission notes, "
                "and does not go to the house yet. She goes to Boston Hall of Records / municipal "
                "records office. Using Corbitt House, Knott's usable address lead, the Corbitt "
                "surname, and Macario as indexes, she searches property title, former owners, "
                "probate records, executors, legal disputes, and public records tied to that address. "
                "She is looking for specific entries, not just record categories: names, dates, deed "
                "or probate references, executor or institution, legal filing type, or a clear no-hit "
                "for each public index that turns up nothing."
            ),
            intent="search_hall_records_for_property_probate",
            requested=[
                "specific public-record entries",
                "former owner",
                "executor or institution",
                "date",
                "deed/probate/legal filing reference or clear no-hit",
                "next public lead",
            ],
            unacceptable=[
                "Only list record categories without concrete entries",
                "Invent a former owner not visible in the records",
                "Offer an explicit action menu to the player",
            ],
        )

    if "michael thomas" in text and "official_records" not in state.attempted:
        return base_decision(
            state,
            persona=persona,
            active_goal="Test whether the 1912 chapel lead exists in court or police records.",
            hypotheses=[
                "The Michael Thomas / chapel / 1912 trail may have public case records, or access may be restricted."
            ],
            open_questions=[
                "Was there a public case, closure, raid, death, or custody record?",
                "If access is refused, what legal or social route remains?",
            ],
            candidate_actions=[
                "Ask courts and police for 1912 Chapel/Thomas records",
                "Search newspaper archive for the 1912 chapel closure",
                "Visit the Chapel of Contemplation in daylight",
            ],
            selection_rationale=f"{pc} tests the official paper trail before entering more dangerous locations.",
            declared_action=(
                f"{pc} leaves the Hall of Records with the Michael Thomas / Chapel of "
                "Contemplation / 1912 notes. She goes to the higher courts and then the Central "
                "Police Station records desk. Presenting herself as a journalist checking an old "
                "property-history lead, she asks specifically for any 1912 case, raid, closure, "
                "criminal proceeding, custody, death, or missing-person record involving Reverend "
                "Michael Thomas or the Chapel of Contemplation. If access is restricted, she cites "
                "the public Hall of Records trail and tries to persuade or fast-talk the clerk into "
                "letting her inspect the index or a redacted file."
            ),
            intent="access_1912_chapel_court_police_records",
            requested=["case status", "raid or closure record", "access refusal reason", "next legal route"],
        )

    if (
        "hall_records" in state.attempted
        and "house_exterior" not in state.attempted
        and hall_records_search_unresolved_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Finish resolving the Hall of Records search before changing location.",
            hypotheses=[
                "The last reply established the records office and search process but did not resolve the declared search.",
                "A player should not treat scene setup as success, failure, or a reason to skip the action's result.",
            ],
            open_questions=[
                "Does the Hall search succeed, fail, or require a check?",
                "What concrete record, name, date, or refusal does the search produce?",
                "If the search is blocked, what public source remains?",
            ],
            candidate_actions=[
                "Continue the declared Hall search until it is resolved",
                "Ask the Keeper to resolve the search result or required check",
                "Do not leave for the house until the record search has an outcome",
            ],
            selection_rationale=(
                "The GM described the start of the search but not its result, so the realistic player holds the declared action open."
            ),
            declared_action=(
                f"{pc} treats the last reply as the start of the Hall of Records search, not its result. "
                "She stays at the records desk and continues the already declared search through address, "
                "Corbitt, Macario, title, probate, executor, dispute, and municipal indexes until the Keeper "
                "resolves it as a success, failure, check request, or clear access block. She does not leave "
                "for the house yet."
            ),
            intent="continue_hall_records_search_after_scene_establishment",
            requested=[
                "success, failure, check request, or blocked-access result",
                "specific record/name/date/lead or clear no-hit",
                "next legal source if blocked",
            ],
            unacceptable=[
                "Move Evelyn to the house before resolving the declared records search",
                "Only repeat Hall atmosphere without an outcome",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "hall_records" in state.attempted
        and "house_exterior" not in state.attempted
        and hall_records_success_without_specific_facts_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Turn the successful Hall of Records check into specific usable facts before changing location.",
            hypotheses=[
                "The roll succeeded, but the visible reply named only record categories and a traceable trail.",
                "A player cannot base a plan on unnamed owners, unnamed executors, or unspecified public records.",
            ],
            open_questions=[
                "What exact owner, executor, date, case, deed, dispute, or institution did the successful search reveal?",
                "Does the record point to a person, a place, a legal restriction, or a clear no-hit?",
                "Is there enough concrete information to justify entering the house now?",
            ],
            candidate_actions=[
                "Keep the Hall file open and ask for the actual entries produced by the success",
                "Request a clear no-hit or access block if the record categories cannot be named",
                "Do not go to the house until the successful search has a concrete result",
            ],
            selection_rationale=(
                "The realistic cautious player treats a success-without-information as unresolved, not as permission to jump scenes."
            ),
            declared_action=(
                f"{pc} does not leave the Hall of Records yet. The roll succeeded, but her notes "
                "still contain only categories of records rather than the actual entries. She keeps "
                "the file open and asks the clerk, index, and documents for the specific result of "
                "that success: named owner, deed or transfer date, probate case, executor or institution, "
                "legal dispute, public filing, or a clear statement that no concrete entry can be exposed. "
                "She does not go to the house until that successful search is resolved into a usable fact or a clear block."
            ),
            intent="continue_hall_records_search_after_success_without_specific_facts",
            requested=[
                "specific owner/executor/date/case/institution/public filing",
                "clear no-hit or access block if details cannot be exposed",
                "next public lead only if it follows from named records",
            ],
            unacceptable=[
                "Only say records connect without naming what they reveal",
                "Move Evelyn to the house before resolving the successful records search",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "hall_records" in state.attempted
        and "official_records" not in state.attempted
        and hall_records_court_police_followup_visible(state.last_reply)
        and not positive_burial_or_basement_clue_visible(state.transcript)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Follow the public court and police lead created by the failed Hall search.",
            hypotheses=[
                "The Hall search failed, but the clerk gave a concrete public-source direction.",
                "Evelyn should not treat a metaphorical records maze as a burial clue.",
                "Without a named Thomas or 1912 lead, the search terms must stay Corbitt, Macario, and the address.",
            ],
            open_questions=[
                "Do court or police indexes show litigation, criminal proceedings, raids, or official trouble at the address?",
                "Are there public names, dates, incident types, or access limits?",
                "If access is restricted, what non-official source remains?",
            ],
            candidate_actions=[
                "Search higher court, county/commonwealth, and Central Police indexes",
                "Move to newspaper archives if official access is refused",
                "Inspect the house exterior only after this public lead is exhausted or blocked",
            ],
            selection_rationale=(
                "A cautious investigator follows the newest visible public-record lead before accepting the higher-risk house entry."
            ),
            declared_action=(
                f"{pc} does not treat the failed Hall of Records search as a hidden success. "
                "She follows the clerk's concrete suggestion instead: higher courts, county or "
                "commonwealth files, and the Central Police Station. Using only the Corbitt House "
                "address, the Corbitt surname, Macario, and Knott's commission as visible indexes, "
                "she asks for public or redacted references to litigation, criminal proceedings, "
                "raids, complaints, custody matters, deaths, or official trouble tied to the address. "
                "If access is restricted, she records the exact refusal and asks what public index "
                "or newspaper source can legally be checked next."
            ),
            intent="access_general_corbitt_court_police_records",
            requested=[
                "public court or police index result",
                "names, dates, charges, deaths, complaints, or clear no-hit",
                "access refusal reason if blocked",
                "next legal public source",
            ],
            unacceptable=[
                "Assume Michael Thomas, Chapel, 1912, or burial if not visible",
                "Treat the failed Hall search as success",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "official_records" in state.attempted
        and "chapel" not in state.attempted
        and "newspaper_archive" not in state.attempted
        and official_records_boundary_points_to_newspaper_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Follow the official-records boundary to the next legal public source.",
            hypotheses=[
                "The official-records success mapped access limits rather than exposing concrete case contents.",
                "The GM named newspaper archives as the next legal public source, so a cautious investigator follows that before entering the house.",
            ],
            open_questions=[
                "Do newspapers print dates, names, incident types, complaints, deaths, or public controversy tied to the house?",
                "Can clippings narrow a date range or incident type for a later official-records request?",
                "If newspapers fail, is house inspection the remaining route?",
            ],
            candidate_actions=[
                "Search the newspaper morgue using the address, Corbitt, Macario, and narrowed incident terms",
                "Record the official access boundary and return later with a better date/name",
                "Inspect the house only after this named public source is exhausted",
            ],
            selection_rationale=(
                "The last successful official-records turn gave a legal boundary and pointed to newspaper archives, not a concrete house-entry fact."
            ),
            declared_action=(
                f"{pc} records the official access boundary instead of treating it as a solved record. "
                "Since the clerks identified the newspaper morgue and archive trail as the next legal "
                "public source, she goes there before entering the house. Using the Corbitt House address, "
                "the Corbitt surname, Macario, tenant trouble, illness, madness, police calls, deaths, "
                "lawsuits, complaints, and any date range implied by the official indexes, she searches "
                "for printed dates, names, incident types, public controversy, or a clear failure."
            ),
            intent="search_newspaper_archive_after_official_records_boundary",
            requested=[
                "public clipping facts",
                "dates, names, or incident types",
                "date range or terms for a later official-records request",
                "next lead or clear failure",
            ],
            unacceptable=[
                "Treat the official boundary map as concrete record contents",
                "Move to the house before following the named newspaper source",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "official_records" in state.attempted
        and "chapel" not in state.attempted
        and official_records_success_without_contents_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Finish reading the official record contents before leaving for the Chapel.",
            hypotheses=[
                "The official-records roll succeeded, but the visible reply only opened an access route.",
                "A player needs case contents, a concrete refusal, or a clear no-hit before changing locations.",
            ],
            open_questions=[
                "What do the accessible court or police records actually say?",
                "Do they mention a raid, arrest, missing people, deaths, custody, charges, or a clear absence?",
                "If full files remain restricted, what exact redacted content or refusal is visible?",
            ],
            candidate_actions=[
                "Read the opened court and police index/file contents",
                "Ask for a concrete refusal if contents are still blocked",
                "Do not leave for the Chapel until this successful access has a result",
            ],
            selection_rationale=(
                "The latest success created access, not information; leaving now would skip the result the player asked for."
            ),
            declared_action=(
                f"{pc} does not leave for the Chapel yet. Since the records desk has actually opened "
                "the relevant index or file route, she stays at the court and police records counters "
                "and reads what that access reveals. She asks for the concrete contents tied to Reverend "
                "Michael Thomas, the Chapel of Contemplation, and 1912: raid, arrest, closure, criminal "
                "proceeding, custody, death, missing-person record, complaint, or a clear statement that "
                "the accessible material contains none of those. If full contents are restricted, she records "
                "the exact refusal and any redacted public detail before choosing a new location."
            ),
            intent="continue_official_records_after_access_without_contents",
            requested=[
                "court/police record contents or clear no-hit",
                "raid/arrest/closure/custody/death/missing-person/charge details if present",
                "exact access refusal and redacted public detail if blocked",
            ],
            unacceptable=[
                "Only say access is open without giving the record contents",
                "Move Evelyn to the Chapel before resolving the successful official-records access",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "hall_records" in state.attempted
        and "michael thomas" not in text
        and "newspaper_archive" not in state.attempted
        and hall_records_failure_visible(state.last_reply)
        and not positive_burial_or_basement_clue_visible(state.transcript)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Change public source after Hall of Records failed to produce a decisive document.",
            hypotheses=[
                "Newspaper archives may have printed public reports that title/probate indexes did not expose.",
                "The failed Hall search should not be treated as if it found an executor, owner, or hidden site clue.",
            ],
            open_questions=[
                "Did newspapers report the Macario family, Corbitt House, or complaints at the address?",
                "Are there dates, names, or incidents that can guide a later official-record search?",
            ],
            candidate_actions=[
                "Search newspaper archives for Corbitt House, Macario, and the address lead",
                "Return to Hall of Records with a narrower clerk-assisted index request",
                "Inspect the house exterior while explicitly noting research remains incomplete",
            ],
            selection_rationale=(
                f"The Hall search failed, so {pc} changes source while staying in the low-risk public-record phase."
            ),
            declared_action=(
                f"{pc} marks the Hall of Records search as inconclusive rather than successful. "
                "She next goes to the Boston Globe newspaper archive / newspaper morgue and searches "
                "public clippings for Corbitt House, the usable address lead Knott provided, the "
                "Corbitt surname, Macario, tenant trouble, illness, madness, police calls, deaths, lawsuits, "
                "or neighborhood complaints. She is looking for printed dates, names, incident types, "
                "or a next public lead, not assuming the missing executor record was found."
            ),
            intent="search_newspaper_archive_after_hall_records_failure",
            requested=["public clipping facts", "dates", "names", "incident type", "next public lead or clear failure"],
            unacceptable=[
                "Treat the failed Hall search as success",
                "Offer an explicit action menu to the player",
                "Only say there are old clippings without concrete content or a clear failure",
            ],
        )

    if (
        (
            "do not get the file" in last
            or "isn't for casual inspection" in last
            or "police files are another" in last
            or "refuse full access" in last
            or "refuses full access" in last
            or "access is restricted" in last
        )
        and not official_record_content_assertion_visible(last)
        and "newspaper_archive" not in state.attempted
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Use a non-official source after court/police access failed.",
            hypotheses=[
                "A newspaper morgue may report what official desks will not release.",
                f"The refusal means {pc} should not claim file contents she did not obtain.",
            ],
            open_questions=[
                "Did newspapers report the 1912 chapel closure or Michael Thomas?",
                "Are there public names, dates, victims, or addresses to verify later?",
            ],
            candidate_actions=[
                "Search Boston Globe archives for Chapel/Thomas/1912",
                "Return to Hall of Records for adjacent public indexes",
                "Visit the chapel in daylight without claiming official file facts",
            ],
            selection_rationale=f"The last official attempt failed, so {pc} changes method instead of repeating pressure.",
            declared_action=(
                f"{pc} does not pretend she obtained the restricted police or court file. She writes "
                "down the refusal and leaves before the clerks decide to remember her face. Next she "
                "goes to the Boston Globe newspaper archive and searches public clippings for Reverend "
                "Michael Thomas, the Chapel of Contemplation, the 1912 closure, the Corbitt surname, "
                "and the Corbitt House address lead. She looks for concrete dates, names, charges, deaths, "
                "missing-person notices, or photographs that a newspaper could legally have printed."
            ),
            intent="search_newspaper_archive_after_official_refusal",
            requested=["public clipping facts", "dates", "names", "victims or charges", "next lead"],
            unacceptable=[
                "Treat the failed official-records attempt as if it succeeded",
                "Offer an explicit action menu to the player",
                "Only say the archive has old clippings without concrete content or a clear failure",
            ],
        )

    if (
        "newspaper_archive" in state.attempted
        and "newspaper_access_tone" not in state.attempted
        and newspaper_archive_access_tone_clarification_visible(state.last_reply)
    ):
        archive_contact = visible_newspaper_archive_contact(state.last_reply)
        return base_decision(
            state,
            persona=persona,
            active_goal="Clarify the newspaper archive access approach without abandoning the archive.",
            hypotheses=[
                "The Globe morgue access has not been resolved yet; the Keeper is asking how Evelyn presses the request.",
                "A cautious professional appeal fits a journalist-investigator better than pressure or improvisation.",
            ],
            open_questions=[
                f"Does {archive_contact} grant supervised access, restricted access, or refuse?",
                "If access is restricted, what exact public or procedural alternative remains?",
            ],
            candidate_actions=[
                "Make a courteous professional appeal for supervised access",
                "Frame the request as legitimate public-history research",
                "Accept restrictions or redaction instead of applying pressure",
            ],
            selection_rationale=(
                "The current scene is still the Globe archive gate, so Evelyn answers the requested approach in her own words instead of changing locations."
            ),
            declared_action=(
                f"{pc} stays with the Globe archive request and answers in her own terms: she uses a courteous, "
                "professional appeal rather than pressure. She introduces herself as a journalist checking an old "
                "public-history property lead, shows the public Hall of Records notes for Reverend Michael Thomas, "
                f"the Chapel of Contemplation, and 1912, and asks {archive_contact} for supervised access to "
                "the clippings morgue, a staff-assisted lookup, or redacted public clippings. If they refuse, she "
                "asks them to state the reason and any lawful public index, date range, or appointment procedure she can use next."
            ),
            intent="press_newspaper_archive_access_professionally",
            requested=[
                "access granted, restricted, or refused",
                "check and outcome if persuasion or credit rating is required",
                "specific clippings result if access is granted",
                "exact refusal reason and next lawful route if blocked",
            ],
            unacceptable=[
                "Move Evelyn to the Chapel before resolving the Globe archive access gate",
                "Offer an explicit action menu to the player",
                "Only repeat that the archive is controlled without resolving the approach",
            ],
        )

    if earned_chapel_lead_visible(state.transcript) and "chapel" not in state.attempted:
        chapel_specific_trail_visible = "michael thomas" in text or "1912" in text
        chapel_search_target = (
            "Corbitt, Reverend Michael Thomas, the visible 1912 trail, or a public link back to the house"
            if chapel_specific_trail_visible
            else "Corbitt, the Corbitt House address lead, symbols, or a public link back to the house"
        )
        chapel_link_question = (
            "Are there symbols, papers, or references to Corbitt or Thomas?"
            if chapel_specific_trail_visible
            else "Are there symbols, papers, or references to Corbitt or the house?"
        )
        chapel_requested_link = (
            "Corbitt or Thomas link" if chapel_specific_trail_visible else "Corbitt or house link"
        )
        return base_decision(
            state,
            persona=persona,
            active_goal="Inspect the Chapel of Contemplation lead with daylight safety precautions.",
            hypotheses=[
                "The chapel may contain visible records, symbols, or links to Corbitt.",
                "If prior records were blocked, the site itself may still expose observable facts.",
            ],
            open_questions=[
                "Is the chapel recently used or physically unsafe?",
                chapel_link_question,
                f"Does {pc} see any reason to connect the chapel to the house interior?",
            ],
            candidate_actions=[
                "Circle and search the chapel carefully",
                "Continue public records research elsewhere",
                "Move to the house exterior with uncertainty explicitly noted",
            ],
            selection_rationale=f"The chapel is a concrete visible lead; {pc} approaches it cautiously and records observations.",
            declared_action=(
                f"{pc} waits for daylight if necessary, takes her notebook, camera, flashlight, and "
                "a sturdy walking stick, and drives to the Chapel of Contemplation. Before entering any "
                "unstable interior, she circles the exterior to note exits, fresh tracks, or signs of "
                "recent use. If it looks safe enough, she enters carefully and searches surviving offices, "
                "cabinets, pulpits, storage, wall marks, and loose papers for records, symbols, or any "
                f"mention of {chapel_search_target}. "
                "She photographs anything distinctive before moving it."
            ),
            intent="investigate_chapel_for_records_symbols_and_corbitt_links",
            requested=["site safety", "visible records", "symbols", chapel_requested_link, "house link or clear failure"],
            unacceptable=[
                "Assume a burial clue if the search fails",
                "Offer an explicit action menu to the player",
                "Only describe dust or mood after a successful search",
            ],
        )

    if (
        chapel_search_failure_visible(state.last_reply)
        and "chapel" in state.attempted
        and "second_chapel_method" not in state.attempted
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Respond to the failed chapel search by changing method and narrowing the target.",
            hypotheses=[
                f"The first chapel search failed or missed details, so {pc} should alter method rather than claim success."
            ],
            open_questions=[
                "Which exact part of the chapel search failed?",
                "Can photography, rubbings, or targeted Spot Hidden reveal visible marks without inventing records?",
            ],
            candidate_actions=[
                "Recheck only the most promising visible chapel surfaces with a different method",
                "Leave the chapel and inspect the house exterior with uncertainty noted",
                "Search another public archive for the same terms",
            ],
            selection_rationale="A real cautious player acknowledges the failure and either changes method or changes source.",
            declared_action=(
                f"The chapel search did not give {pc} the concrete record she wanted, so she marks "
                "that as a failed lead rather than a discovery. Before leaving, she changes method: "
                "she uses the flashlight at a low angle, takes photographs, and makes quick pencil "
                "rubbings only of any visible carvings, floor marks, desk scars, or paper impressions "
                "near the pulpit, office, and storage areas. She is not asking for hidden truth for free; "
                "she is checking whether a more careful visual method reveals a specific visible mark, "
                "or else she will leave with no chapel clue."
            ),
            intent="retry_chapel_with_distinct_visual_method_after_failure",
            requested=["specific visible mark", "new check request", "clear no-clue result", "risk or time cost"],
            unacceptable=[
                "Treat the prior failed search as a success",
                "Give a vague sense of importance without a visible detail",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "chapel" in state.attempted
        and "chapel_followup" not in state.attempted
        and not positive_burial_or_basement_clue_visible(state.transcript)
        and (
            chapel_scene_established_without_search_resolution_visible(state.last_reply)
            or chapel_success_without_payoff_visible(state.last_reply)
        )
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Resolve the Chapel lead that has not produced concrete player-usable findings yet.",
            hypotheses=[
                "The GM established or partially checked the Chapel, but did not yet resolve the declared search into concrete findings.",
                "A cautious player should continue the bounded search instead of treating the Chapel as complete.",
            ],
            open_questions=[
                "Are any stable offices, pulpits, cabinets, wall marks, or loose papers searchable?",
                "Is there a concrete symbol, record, name, date, or Corbitt link?",
                "If the structure is unsafe, what exact hazard blocks the search?",
            ],
            candidate_actions=[
                "Continue the previously declared Chapel search in stable areas",
                "Photograph visible records, symbols, and wall marks before touching them",
                "Withdraw only if the structure gives a concrete unsafe block",
            ],
            selection_rationale=(
                "The last reply established arrival and atmosphere, not the outcome of the Chapel search."
            ),
            declared_action=(
                f"{pc} treats the last reply as arrival at the Chapel, not as the search result. "
                "She keeps to stable footing, photographs the cracked interior before disturbing it, "
                "and searches only reachable offices, cabinets, pulpit areas, storage shadows, wall marks, "
                "and loose papers for a concrete record, symbol, name, date, or link to Corbitt, Michael "
                "Thomas, the 1912 closure, or the house. If a section is structurally unsafe, she stops at "
                "that boundary and asks the Keeper to resolve the block rather than forcing deeper entry."
            ),
            intent="continue_chapel_search_after_scene_establishment",
            requested=[
                "specific Chapel finding or clear no-finding result",
                "check and outcome if searching is uncertain",
                "blocked structural reason if unsafe",
                "concrete link or explicit absence of a link",
            ],
            unacceptable=[
                "Only restate Chapel atmosphere without resolving the search",
                "Move Evelyn to the Corbitt House before resolving the declared Chapel search",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "chapel" in state.attempted
        and "chapel_cabinet_probe" not in state.attempted
        and clarified_chapel_cabinet_affordance_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Turn the clarified chapel cabinet into a bounded search instead of repeating clarification.",
            hypotheses=[
                "The Keeper has already grounded the old cabinet as the chapel's current reachable affordance.",
                "A player-like cautious investigator should now inspect that object with risk controls.",
            ],
            open_questions=[
                "Does the cabinet open, resist, collapse, or require a check?",
                "Does it contain papers, symbols, objects, or a clear absence?",
                "Does approaching it change Evelyn's retreat path or trigger a hazard?",
            ],
            candidate_actions=[
                "Photograph the cabinet and approach only as far as needed to test it",
                "Inspect doors, drawers, hinges, and nearby floor before opening",
                "Withdraw if the structure shifts, blocks retreat, or the cabinet reacts",
            ],
            selection_rationale=(
                "After one grounded affordance answer, asking the same question again becomes a loop; "
                "the next real player action is to test the cabinet."
            ),
            declared_action=(
                f"{pc} stops asking for the same chapel affordance. Keeping the way out behind her, "
                "she photographs the old cabinet in place, checks the floor and masonry around it for "
                "collapse risk, and approaches only far enough to examine the doors, drawers, hinges, "
                "and any visible marks. She tests it with the walking stick or a cloth-wrapped hand before "
                "pulling anything open. If it opens, she reads or photographs any papers, symbols, names, "
                "dates, or objects inside; if it resists, collapses, or anything moves, she stops and treats "
                "that as the result rather than forcing it."
            ),
            intent="examine_clarified_chapel_cabinet",
            requested=[
                "cabinet approach and opening result",
                "contents, markings, papers, symbols, or clear absence",
                "check and outcome if opening/searching is uncertain",
                "hazard/reaction and Evelyn position",
            ],
            unacceptable=[
                "Repeat only that the cabinet is reachable",
                "Offer an explicit action menu to the player",
                "Reveal hidden contents without resolving the cabinet examination",
            ],
        )

    if (
        "chapel_cabinet_probe" in state.attempted
        and "burial_record_basement_return" not in state.attempted
        and (
            explicit_burial_clue_visible(state.last_reply)
            or explicit_burial_clue_visible(state.transcript)
        )
        and ("basement_descent" in state.attempted or "house_exterior" in state.attempted)
    ):
        known_basement_line = (
            "crawl_space_probe" in state.attempted
            or "crawl_space_entry" in state.attempted
            or "basement_followup" in state.attempted
        )
        return base_decision(
            state,
            persona=persona,
            active_goal="Act on the Chapel burial record by returning to the Corbitt House basement line.",
            hypotheses=[
                "The Chapel cabinet produced a concrete record: Walter Corbitt was buried in the basement of his house.",
                "The house basement was already marked and partly mapped, so another chapel clarification would waste the successful clue.",
                (
                    "The opened crawl-space and marked stairs are the current physical way to test the burial record."
                    if known_basement_line
                    else "The basement route is now the strongest physical line to test the burial record."
                ),
            ],
            open_questions=[
                "Can Evelyn return safely from the Chapel to the Corbitt House basement route?",
                "Does the marked stair, concealed section, or crawl-space remain as she left it?",
                "Does the basement contain a burial site, body, coffin, chamber, ritual object, or a clear absence at the marked lead?",
            ],
            candidate_actions=[
                "Leave the Chapel with the burial record photographed and transcribed",
                "Return to the known Corbitt House basement route rather than re-clarifying the Chapel",
                "Use the marked crawl-space or basement lead to test the burial statement with bounded risk",
            ],
            selection_rationale=(
                "The last concrete record points away from the Chapel and back to the already mapped basement. A player-like investigator follows that link."
            ),
            declared_action=(
                (
                    f"{pc} treats the chapel cabinet record as the actionable result: Walter Corbitt was "
                    "buried in the basement of his house. She photographs and transcribes that line, leaves "
                    "the Chapel rather than asking about the same cabinet again, and returns to the Corbitt "
                    "House by her known route. She checks that the exterior entry, marked basement stairs, "
                    "and opened crawl-space or concealed section are unchanged, then follows that marked "
                    "basement line with flashlight, camera, walking stick, and notebook. She looks specifically "
                    "for a burial site, remains, coffin, grave-like cavity, ritual object, or a clear contradiction "
                    "to the burial record. If the route has changed, anything moves, or retreat is blocked, she "
                    "stops and resolves that immediate danger before pushing deeper."
                )
                if known_basement_line
                else (
                    f"{pc} treats the chapel cabinet record as the actionable result: Walter Corbitt was "
                    "buried in the basement of his house. She photographs and transcribes that line, leaves "
                    "the Chapel rather than asking about the same cabinet again, and returns to the Corbitt "
                    "House. Using the known entry and mapped ground-floor route, she marks the basement access, "
                    "keeps the exit open, and descends only far enough to test the burial clue with flashlight, "
                    "camera, walking stick, and notebook. If the route has changed, anything moves, or retreat "
                    "is blocked, she stops and resolves that immediate danger before pushing deeper."
                )
            ),
            intent="return_to_marked_basement_after_chapel_burial_record",
            requested=[
                "safe return from Chapel to Corbitt House and basement route",
                "marked basement/crawl-space status or route change",
                "burial-site/remains/coffin/cavity/object result or clear absence",
                "check and consequence if the return or deeper test is uncertain",
            ],
            unacceptable=[
                "Repeat a Chapel cabinet affordance after the burial record is already exposed",
                "Ask another generic latest-affordance clarification",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "newspaper_archive" in state.attempted
        and "official_records" not in state.attempted
        and newspaper_points_to_official_records_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Follow the newspaper archive's public court and police lead before entering the house.",
            hypotheses=[
                "The newspaper success did not solve the house, but it did point to serious official records.",
                "A cautious investigator should exhaust this low-risk public lead before higher-risk entry.",
            ],
            open_questions=[
                "Do police or higher-court indexes name a case, date, complaint, death, custody matter, or clear no-hit?",
                "Is access restricted, and if so what public detail is still visible?",
                "Does the official record produce a safer plan for entering the house?",
            ],
            candidate_actions=[
                "Search higher court and police records tied to the newspaper lead",
                "Record any refusal and then inspect the house exterior",
                "Inspect the house only after the official lead is exhausted or blocked",
            ],
            selection_rationale=(
                "The latest successful public-source result named police and higher-court records as the stronger next lead."
            ),
            declared_action=(
                f"{pc} does not go to the house immediately after the newspaper success. "
                "She follows the clippings' strongest public lead first: higher-court records "
                "and Central Police files tied to the Corbitt House, Macario, the usable address "
                "lead, and the incidents described in print. Presenting herself as a journalist "
                "cross-checking published public reports, she asks for index entries, redacted files, "
                "case references, complaint logs, deaths, custody matters, charges, or a clear no-hit. "
                "If access is restricted, she records the exact refusal and any public index detail "
                "before deciding whether the house must be inspected next."
            ),
            intent="access_general_corbitt_court_police_records",
            requested=[
                "public court or police index result",
                "names, dates, charges, deaths, complaints, or clear no-hit",
                "access refusal reason if blocked",
                "next legal public source or house-entry implication",
            ],
            unacceptable=[
                "Ignore the newspaper's explicit police/court next lead",
                "Assume Michael Thomas, Chapel, 1912, or burial if not visible",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        (
            "corbitt house" in text
            or newspaper_house_history_visible(state.last_reply)
            or positive_burial_or_basement_clue_visible(state.transcript)
        )
        and "house_exterior" not in state.attempted
    ):
        uncertainty = ""
        if (
            "burial" not in text
            and "buried" not in text
            and ("do not get the file" in text or has_failure(last))
        ):
            if "chapel" in state.attempted:
                uncertainty = (
                    f" {pc} explicitly notes that the chapel did not give a concrete site clue; "
                    "she is going because the commissioned house inspection remains necessary."
                )
            else:
                uncertainty = (
                    f" {pc} explicitly notes that public research has not produced a concrete next "
                    "lead; she is going because the commissioned house inspection remains necessary."
                )
        return base_decision(
            state,
            persona=persona,
            active_goal="Inspect the Corbitt House exterior and choose an entry only from visible conditions.",
            hypotheses=[
                "The house may reveal physical hazards, recent use, or entry points regardless of incomplete research."
            ],
            open_questions=[
                "Which entrances and windows are visible?",
                "Are there signs of recent occupation, structural danger, or cellar access?",
                "Which key fits without forcing an unsafe entry?",
            ],
            candidate_actions=[
                "Circle the house exterior and map exits",
                "Enter through the safest matching exterior lock",
                "Return to research if the house looks too unsafe",
            ],
            selection_rationale=f"The core job is the house; {pc} moves from research to careful site inspection without inventing missing facts.",
            declared_action=(
                f"{pc} goes to the Corbitt House in daylight with notebook, camera, flashlight, "
                "walking stick, first aid kit, and Knott's keys." + uncertainty + " Before opening "
                "anything, she circles the outside from a safe distance, noting front, back, side, "
                "and cellar entrances, window condition, footprints, fresh damage, animal signs, and "
                "neighbors' sight lines. She photographs the exterior and tests only the least exposed "
                "matching lock, keeping an exit route behind her."
            ),
            intent="inspect_house_exterior_and_safe_entry",
            requested=["visible entry points", "hazards", "recent-use signs", "lock result", "clear safe or blocked entry"],
        )

    if (
        "enter_threshold" not in state.attempted
        and open_exterior_threshold_affordance_visible(last)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Use the already-open exterior threshold without repeating the same clarification.",
            hypotheses=[
                "The latest visible information gives an open exterior doorway, a visible first step, and a clear retreat.",
                "A cautious player can test only the first interior margin before committing deeper.",
            ],
            open_questions=[
                "What layout, hazards, sounds, smells, or tracks are visible after one controlled step?",
                "Does anything react when Evelyn crosses only the first threshold?",
                "Can she still withdraw through the same open doorway?",
            ],
            candidate_actions=[
                "Take one controlled step inside and map only the immediate threshold",
                "Keep the open doorway as the exit line",
                "Retreat if the first step reveals danger or bad footing",
            ],
            selection_rationale=(
                "The door is already open and the first step is the concrete affordance, so asking for the same affordance again would loop."
            ),
            declared_action=(
                f"{pc} stops asking for the same doorway description. With the side entrance already open, "
                "she keeps one hand near the frame and the daylight exit directly behind her, then steps just "
                "inside only far enough to test the first visible strip of floor. She holds the door open, "
                "keeps the flashlight low, and maps the immediate entry margin: floor soundness, visible "
                "room or passage shape, odors, drafts, tracks, disturbed objects, and any movement or sound. "
                "If the first step shifts, drops, or draws a reaction, she withdraws through the same door "
                "instead of pressing deeper."
            ),
            intent="enter_threshold_and_map_ground_floor_affordances",
            requested=[
                "first-step footing and threshold safety",
                "immediate layout or passage shape",
                "sounds, smells, tracks, disturbed objects, or clear absence",
                "reaction or no-reaction result",
            ],
            unacceptable=[
                "Repeat the same open-door affordance without resolving the first step",
                "Move Evelyn deep inside before resolving threshold consequences",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "enter_threshold" not in state.attempted
        and entry_available_visible(last)
        and not interior_started_visible(last)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Enter only as far as needed to establish the ground-floor situation.",
            hypotheses=[
                "The first interior step should establish exits, layout, smell, tracks, and immediate hazards."
            ],
            open_questions=[
                "What rooms and exits are visible from the threshold?",
                "Is there a safe path back out?",
                "Are there sounds, smells, tracks, or moved objects?",
            ],
            candidate_actions=[
                "Use the established workable entrance without treating it as proven safest",
                "Listen at the door before entering",
                "Retreat and ask neighbors if danger is obvious",
            ],
            selection_rationale=(
                f"{pc} commits to the established workable entrance while preserving the uncertainty "
                "about whether it is truly the safest one."
            ),
            declared_action=(
                f"{pc} uses the workable exterior entrance the last exchange established, without "
                "treating it as proven safest. She stands to the side rather than squarely in the "
                "doorway, keeps her retreat line open behind her, and opens only enough to listen, "
                "smell, and look through the first threshold. If the door opens and nothing immediately "
                "rushes her, she steps just inside with the flashlight low and maps the entry hall, "
                "visible rooms, stairs, exits, odors, footprints, drafts, and any object that looks "
                "recently disturbed. She keeps the door open behind her."
            ),
            intent="enter_threshold_and_map_ground_floor_affordances",
            requested=["room layout", "immediate hazards", "audible or visible clues", "check result if needed"],
        )

    if (
        "enter_threshold" not in state.attempted
        and clarified_exterior_door_affordance_visible(last)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Convert the clarified exterior door affordance into a concrete entry test.",
            hypotheses=[
                "The clarified exterior door is the safest currently named affordance; testing it resolves whether entry is possible."
            ],
            open_questions=[
                "Does the old lock open, jam, or make enough noise to create a risk?",
                "What is visible or audible through the first opening?",
                "Is the threshold safe enough to map without committing deeper?",
            ],
            candidate_actions=[
                "Try the clarified exterior door with controlled pressure",
                "Back off and test another exterior entrance",
                "Leave to get tools or help if the lock is unsafe",
            ],
            selection_rationale=(
                "After one clarification pass, the exterior door is concrete enough for a cautious physical test."
            ),
            declared_action=(
                f"{pc} uses the clarified exterior door rather than asking for the same description again. "
                "She stands to the hinge-side edge instead of squarely in front of it, keeps the retreat path "
                "open behind her, and turns the key with slow steady pressure. If the lock opens, she eases "
                "the door only a handspan, waits, listens, smells for anything stronger than stale air, and "
                "maps only the immediate threshold before stepping inside. If the lock jams or the door resists "
                "dangerously, she stops and treats that as the result instead of forcing it."
            ),
            intent="enter_threshold_via_clarified_exterior_door",
            requested=[
                "lock opens, jams, or blocks entry",
                "sound/smell/visible threshold result",
                "check and outcome if uncertain",
                "safe entry or clear blocked consequence",
            ],
            unacceptable=[
                "Repeat the same door description without resolving the key/lock test",
                "Offer an explicit action menu to the player",
                "Move the character deep inside without threshold information",
            ],
        )

    house_ground_floor_context = (
        (
            "enter_threshold" in state.attempted
            and first_threshold_strip_established_visible(last)
        )
        or interior_started_visible(state.transcript)
        or (
            ("ground floor" in text or "entry hall" in text)
            and ("corbitt house" in text or "old corbitt place" in text)
        )
    )
    if house_ground_floor_context and "ground_floor_search" not in state.attempted:
        return base_decision(
            state,
            persona=persona,
            active_goal="Search the ground floor without losing exit control.",
            hypotheses=[
                "Ground-floor rooms may expose mundane clues, hazards, or routes to upper/lower floors."
            ],
            open_questions=[
                "Which room shows the freshest disturbance?",
                "Do tracks, damage, smells, or drafts point upstairs or downstairs?",
                "Are there documents or objects worth photographing?",
            ],
            candidate_actions=[
                "Search ground-floor rooms clockwise while keeping exits known",
                "Go upstairs immediately",
                "Look for cellar access from the hall or kitchen",
            ],
            selection_rationale="A systematic room sweep is safer than lunging toward an unseen threat.",
            declared_action=(
                f"{pc} keeps her back path marked and searches the ground floor clockwise from the "
                "entry, one room at a time. She does not pocket unknown objects yet. She photographs "
                "documents, marks doors she has opened, checks under furniture from a distance with "
                "the walking stick, and watches for drafts, cellar smells, scrape marks, footprints, "
                "or a route upstairs or downstairs."
            ),
            intent="systematic_ground_floor_search",
            requested=["concrete room findings", "hazards", "route upstairs or downstairs", "check and outcome if uncertain"],
        )

    if (
        "basement_descent" in state.attempted
        and "basement_recovery" in state.attempted
        and "basement_exit" not in state.attempted
        and basement_exit_reachable_after_failed_recovery_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Exit the noisy failed basement recovery before starting a new search.",
            hypotheses=[
                "The failed retreat made noise, but the open basement door and handkerchief marker are still within reach.",
                "A cautious player should first regain a stable ground-floor position instead of asking for another description.",
            ],
            open_questions=[
                "Can Evelyn get through the open basement doorway to the ground floor now?",
                "Does anything below react before she clears the threshold?",
                "Is the basement now unsafe enough to leave and reassess?",
            ],
            candidate_actions=[
                "Step through the open basement door to the ground floor",
                "Hold at the doorway only if something moves from below",
                "Leave the house and reassess if the basement remains unstable",
            ],
            selection_rationale=(
                "The last failed recovery still leaves a reachable exit; repeating a clarification would stall "
                "instead of resolving the danger."
            ),
            declared_action=(
                f"{pc} treats the failed stair retreat as enough warning. Since the open basement door "
                "and handkerchief marker are just behind her, she does not inspect a new object or ask "
                "for the same affordance again. She shifts her weight low, steps through the basement "
                "doorway to the ground floor, and keeps the flashlight aimed down the stairs until she "
                "is clear of the threshold. If anything moves up from below before she clears it, she "
                "dodges through the doorway and pulls back toward the exterior exit."
            ),
            intent="exit_basement_after_noisy_recovery_failure",
            requested=[
                "whether Evelyn clears the basement doorway",
                "any immediate reaction from below",
                "Evelyn's new ground-floor or doorway position",
                "check and consequence if the exit is unsafe",
            ],
            unacceptable=[
                "Repeat a description of the same door without resolving the exit",
                "Move Evelyn upstairs before resolving the basement doorway",
                "Treat the failed basement search as a discovered clue",
            ],
        )

    if (
        "basement_descent" in state.attempted
        and "basement_exit" in state.attempted
        and "basement_threshold_reassess" not in state.attempted
        and clear_of_open_basement_threshold_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Reassess the open basement stair mouth from the ground floor without drifting to an old room.",
            hypotheses=[
                "Evelyn is currently on the ground floor at the open basement doorway, not upstairs.",
                "The failed descent made the basement risky, but the open stair mouth is still the active unresolved lead.",
            ],
            open_questions=[
                "Can Evelyn get a better look from the ground-floor threshold without descending?",
                "Does anything below react while she holds the stair mouth?",
                "Is a second descent justified, or should she withdraw from the house?",
            ],
            candidate_actions=[
                "Hold the ground-floor doorway and sweep the basement stairs with light",
                "Make noise or toss a small bit of debris to test for reaction without descending",
                "Withdraw to the exterior exit if the basement remains unreadable or reacts",
            ],
            selection_rationale=(
                "After regaining the ground floor, a realistic cautious player acts on the current open basement "
                "threshold instead of asking a generic affordance question that can drift to older scenes."
            ),
            declared_action=(
                f"{pc} stays on the ground floor at the open basement doorway and keeps attention on the "
                "current threshold. Keeping the exterior exit line behind her, she braces at the doorframe, "
                "sweeps the flashlight slowly down the basement stairs and across the stair mouth, and listens "
                "for any response from below. She taps the doorframe once with the walking stick and lets a "
                "small harmless bit of dust or grit fall down the first steps, watching for movement, sound, "
                "airflow, or a clearer safe route. If the cellar stays unreadable or answers with movement, "
                "she backs toward the exterior exit instead of descending blind; if a stable route is clearly "
                "established, she prepares a second descent."
            ),
            intent="reassess_open_basement_stair_mouth_after_failed_descent",
            requested=[
                "reaction or clear absence from the basement stair mouth",
                "whether a safer second descent is established",
                "Evelyn's position relative to basement door and exterior exit",
                "check and consequence if uncertain",
            ],
            unacceptable=[
                "Move Evelyn upstairs before resolving the current basement doorway position",
                "Repeat a generic latest-affordance clarification",
                "Treat the failed descent as if it found a hidden object",
            ],
        )

    if (
        "basement_descent" in state.attempted
        and "basement_exit" not in state.attempted
        and "basement_followup" not in state.attempted
        and clarified_basement_return_route_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Recover from an unclear basement search without inventing a clue.",
            hypotheses=[
                "The GM clarified the marked stair as the only confirmed affordance.",
                "The failed basement search did not establish disturbed earth, a concealed section, a body, or a ritual object.",
            ],
            open_questions=[
                "Does Evelyn reach the ground floor safely?",
                "Does anything react while she withdraws from the uncertain basement?",
                "Which already visible house route remains worth checking next?",
            ],
            candidate_actions=[
                "Withdraw to the marked stair and ground-floor threshold",
                "Hold position at the stair and listen for a concrete reaction",
                "Abort the basement pass and reassess the house notes in daylight",
            ],
            selection_rationale=(
                "A cautious investigator treats the stair clarification as a safety route, not as proof "
                "of the requested hidden basement evidence."
            ),
            declared_action=(
                f"{pc} accepts that the last basement pass did not prove a hidden panel, body, "
                "ritual object, disturbed-earth source, or side passage. She keeps the flashlight "
                "low, backs to the marked stair rather than pressing deeper into the basement, and "
                "climbs to the ground-floor doorway with the handkerchief marker still in sight. "
                "Once clear, she notes the basement as unresolved and reassesses the already visible "
                "house routes before choosing a new approach."
            ),
            intent="withdraw_from_failed_basement_search_to_marked_stairs",
            requested=[
                "whether Evelyn reaches the ground floor or doorway",
                "any immediate reaction from the basement",
                "updated position and remaining visible routes",
                "check and consequence if withdrawal is unsafe",
            ],
            unacceptable=[
                "Treat the failed basement search as a discovered clue",
                "Invent disturbed earth, scrape marks, concealed sections, a body, or a ritual object",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "basement_descent" in state.attempted
        and "basement_door_probe" not in state.attempted
        and clarified_basement_door_affordance_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Resolve the clarified basement door without repeating the same question.",
            hypotheses=[
                "The GM clarified a closed reachable door, but did not establish what lies beyond it.",
                "The next player-like action is to test it cautiously or withdraw, not to ask for the same description again.",
            ],
            open_questions=[
                "Is this the marked exit or a closed interior cellar door?",
                "Does the latch, lock, or frame yield to light testing?",
                "What immediate sound, smell, or threat appears if it opens?",
            ],
            candidate_actions=[
                "Identify whether it is the marked basement exit",
                "Test the closed door from arm's length while keeping the exit route open",
                "Withdraw if the door resists or the basement reacts",
            ],
            selection_rationale=(
                "A single clarification is enough; the player now converts the reachable door into a cautious physical test."
            ),
            declared_action=(
                f"{pc} does not ask for the same door description again. She first checks whether this "
                "reachable door is the marked basement exit or a closed interior cellar door. If it is "
                "the marked exit, she withdraws through it to the ground floor. If it is a closed cellar "
                "door, she photographs the lock and frame, tests the latch or key lightly from arm's "
                "length, and opens it only a crack if it gives, keeping the stair route open behind her. "
                "If it resists, makes dangerous noise, or anything moves, she stops and withdraws."
            ),
            intent="resolve_clarified_basement_door_affordance",
            requested=[
                "whether the door is exit or interior cellar door",
                "lock/latch result",
                "visible or audible threshold result if opened",
                "blocked reason or immediate threat if unsafe",
            ],
            unacceptable=[
                "Repeat the same door description without a result",
                "Invent disturbed earth, scrape marks, or concealed sections that were not established",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "basement_descent" in state.attempted
        and "basement_followup" not in state.attempted
        and basement_actionable_lead_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Follow up the successful basement clue before leaving the area.",
            hypotheses=[
                "The disturbed surface detail, structural irregularity, or concealed section is the most concrete new lead.",
                "Leaving for another floor before checking them would ignore the last successful result.",
            ],
            open_questions=[
                "Does the basement irregularity cover a body, cache, or recent access point?",
                "Do any visible surface marks lead to a hidden panel or moved object?",
                "Is there a threat triggered by examining the altered or concealed section?",
            ],
            candidate_actions=[
                "Photograph and probe the visible basement irregularity from a safe stance",
                "Trace any visible surface marks and test the suspicious section with the walking stick",
                "Retreat only if the basement clue produces an immediate threat",
            ],
            selection_rationale=(
                f"The last action succeeded in the basement, so {pc} follows that concrete visible lead "
                "instead of changing floors."
            ),
            declared_action=(
                f"{pc} stays in the basement because the last search produced a concrete lead. "
                "Keeping the stair route open behind her, she photographs the visible disturbed dust or "
                "surface marks and the suspicious structural irregularity before touching anything. Then "
                "she uses the walking stick and the edge of a notebook, not bare hands, to test whether "
                "the altered or concealed section has a seam, loose edge, cavity, body, or ritual object "
                "behind it. If anything moves or attacks, she retreats toward the marked stairs."
            ),
            intent="follow_basement_disturbed_earth_and_concealed_section",
            requested=[
                "specific basement lead result",
                "check and outcome if probing is uncertain",
                "object/body/panel presence or clear absence",
                "threat and resolved consequence if triggered",
            ],
            unacceptable=[
                "Offer an explicit action menu to the player",
                "Skip the disturbed earth or concealed section after a successful search",
                "Only add atmosphere without resolving the concrete probe",
            ],
        )

    if (
        "basement_descent" in state.attempted
        and "basement_recovery" not in state.attempted
        and failed_basement_stair_position_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Stabilize the failed basement-stair position before changing floors.",
            hypotheses=[
                "The failed descent left Evelyn partway down the basement stairs, so upstairs rooms are not the current actionable position.",
                "The noise changed the cellar situation; first resolve whether she can safely recover the stair position.",
            ],
            open_questions=[
                "Can Evelyn retreat to the top of the basement stairs without a second mishap?",
                "Does anything below react to the noise before she regains the doorway?",
                "What is her exact position after the recovery attempt?",
            ],
            candidate_actions=[
                "Freeze, listen, and retreat one careful step at a time to the open basement doorway",
                "Continue down only if the stairs and cellar remain safe",
                "Call out or leave the house if the basement answers with an immediate threat",
            ],
            selection_rationale=(
                "A cautious player cannot jump to the upper floor while the previous failed result still "
                "places her partway down the basement stairs."
            ),
            declared_action=(
                f"{pc} does not change floors or start an unrelated room search while she is still on the "
                "basement stairs. She freezes, lowers her weight, keeps the flashlight trained into the "
                "cellar below, and listens for any response to the loud creak. If nothing immediately "
                "rushes her, she retreats one careful step at a time back toward the open basement door "
                "and handkerchief marker, testing each stair before shifting weight. If something moves "
                "up from below, she abandons the descent and dodges through the basement doorway."
            ),
            intent="recover_from_failed_basement_stair_descent",
            requested=[
                "whether the retreat up the basement stairs succeeds",
                "any cellar reaction to the failed descent noise",
                "Evelyn's concrete position after recovery",
                "check and consequence if retreat is uncertain",
            ],
            unacceptable=[
                "Move Evelyn upstairs before resolving the current basement-stair position",
                "Offer an explicit action menu to the player",
                "Only repeat atmosphere without stating whether she recovers, remains stuck, or is attacked",
            ],
        )

    if (
        "basement_followup" in state.attempted
        and "basement_concealed_probe" not in state.attempted
        and clarified_concealed_basement_section_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Resolve the clarified concealed basement section without abandoning the lead.",
            hypotheses=[
                "The concealed section is the concrete result of the successful basement follow-up.",
                "The stairs are a retreat route behind Evelyn, not a reason to change floors.",
            ],
            open_questions=[
                "Does the concealed section open, resist, or reveal a cavity?",
                "Is there a body, object, ritual mark, or hazard behind it?",
                "Does probing it trigger a moving threat?",
            ],
            candidate_actions=[
                "Photograph and mark the rough boards or concealed section",
                "Probe the board edges and latch-like points from a braced stance",
                "Withdraw to the marked stairs if it moves or attacks",
            ],
            selection_rationale=(
                "After one clarification pass, the hidden basement section is concrete enough to test; "
                "leaving for upstairs would ignore the strongest visible lead."
            ),
            declared_action=(
                f"{pc} stays with the clarified basement lead rather than changing floors. Keeping the "
                "marked stairs behind her clear, she photographs the rough boards or concealed section, marks the nearby "
                "scrape marks and disturbed earth in her notebook, and uses the walking stick and notebook "
                "edge to test the board or panel-like boundary from a braced stance. She tries only controlled "
                "pressure at seams, latch-like spots, or loose edges. If it opens, she maps the cavity "
                "with the flashlight before reaching in; if it resists, moves by itself, or something "
                "attacks, she retreats toward the marked stairs."
            ),
            intent="probe_clarified_concealed_basement_section",
            requested=[
                "whether the section opens or resists",
                "visible cavity contents or clear absence",
                "check and consequence if uncertain",
                "threat reaction and Evelyn position",
            ],
            unacceptable=[
                "Move Evelyn upstairs before resolving the concealed basement section",
                "Repeat the same concealed-section description without a result",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "basement_followup" in state.attempted
        and "basement_gap_stabilized" not in state.attempted
        and loosened_basement_gap_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Stabilize the newly loosened basement gap before changing floors.",
            hypotheses=[
                "The failed probing still created a concrete state change: a hidden gap opened and something moved inside.",
                "The marked stairs are the retreat line, but the current unresolved hazard is still in the basement.",
            ],
            open_questions=[
                "Is the skittering movement animal, structural debris, or an active threat?",
                "Can Evelyn observe the gap from a safer position without reaching into it?",
                "Does the gap reveal a body, object, passage, or only unstable debris?",
            ],
            candidate_actions=[
                "Back to the marked stair line while keeping the gap in sight",
                "Sweep light across the gap and listen before touching anything else",
                "Retreat upstairs only if the gap reacts as an immediate threat",
            ],
            selection_rationale=(
                "A failed roll still changed the basement state, so a realistic cautious player resolves the loosened gap before switching floors."
            ),
            declared_action=(
                f"{pc} stays in the basement because the newly loosened gap is still unresolved. "
                "She backs only as far as the marked stair line, keeps the flashlight on the rough boards, packed dirt, "
                "and open gap, and listens to the scratching or skittering without reaching inside. From that safer "
                "position she sweeps the light across the opening to identify whether the movement is animal, falling "
                "debris, a visible object, a body, or an active threat. If anything pushes out or attacks, she retreats "
                "up the marked stairs; otherwise she records what the gap actually reveals before choosing a new floor."
            ),
            intent="stabilize_loosened_basement_gap",
            requested=[
                "what the loosened basement gap visibly reveals",
                "source or limits of the skittering/scratching movement",
                "check and consequence if observation is uncertain",
                "Evelyn position relative to marked stairs",
            ],
            unacceptable=[
                "Move Evelyn upstairs before resolving the loosened basement gap",
                "Treat the failed roll as if no state changed",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "basement_concealed_probe" in state.attempted
        and "stubborn_basement_boards_withdrawal" not in state.attempted
        and stubborn_concealed_basement_boards_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Change strategy after the concealed boards resist the careful probe.",
            hypotheses=[
                "The suspicious boards remain the lead, but the careful probe has already failed to open them.",
                "A cautious investigator should not repeat the same question or invent a successful opening.",
                "The next realistic move is to preserve evidence and retreat for better tools or help.",
            ],
            open_questions=[
                "Can Evelyn leave the basement safely with the board location documented?",
                "Does anything react when she stops forcing the boards and withdraws?",
                "What tool or assistance would be needed to return without pretending the failed probe succeeded?",
            ],
            candidate_actions=[
                "Photograph and mark the stubborn board section precisely",
                "Back out to the stairs and ground floor with the route still open",
                "Plan to return with a pry tool, witness, or assistance rather than keep probing by hand",
            ],
            selection_rationale=(
                "After a failed controlled probe and repeated descriptions of the same boarded-off section, "
                "a player-like cautious investigator changes method instead of asking the Keeper to restate the affordance."
            ),
            declared_action=(
                f"{pc} accepts that the careful probe failed and does not ask for the same boarded section again. "
                "She photographs the rough boards, old bins, floor marks, and their position relative to the stairs, "
                "then marks the nearest safe floor point with chalk and a note in her notebook. She does not try to "
                "force the boards open with bare hands or pretend the gap yielded. Keeping the walking stick between "
                "herself and the section, she backs along her marked retreat line to the basement stairs and ground "
                "floor, planning to return only with proper tools, a witness, or help. If anything moves or blocks "
                "her retreat while she withdraws, she stops and resolves that threat immediately."
            ),
            intent="withdraw_from_stubborn_concealed_basement_boards_for_tools",
            requested=[
                "whether Evelyn withdraws safely to the stairs or ground floor",
                "any reaction from the boarded section during withdrawal",
                "documented board location and evidence status",
                "tool/help requirement or clear consequence if she cannot leave",
            ],
            unacceptable=[
                "Repeat the same boarded-section description",
                "Open the boards for free after the failed probe",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "stubborn_basement_boards_withdrawal" in state.attempted
        and "basement_tools_return" not in state.attempted
        and (
            documented_stubborn_basement_withdrawal_visible(state.last_reply)
            or documented_basement_return_access_visible(state.last_reply)
        )
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Convert the documented basement withdrawal into a prepared return instead of repeating clarification.",
            hypotheses=[
                "Evelyn has already documented and marked the stubborn board section.",
                "The last action established a safe withdrawal or a clear route back, so asking for the same affordance would be a loop.",
                "A cautious investigator can change resources by leaving for proper tools or a witness, then return to the marked point.",
            ],
            open_questions=[
                "Can Evelyn leave the house and return with a suitable pry tool or witness?",
                "Is the marked basement board section still reachable on return?",
                "Does the board section open, resist, or trigger a threat when handled with better preparation?",
            ],
            candidate_actions=[
                "Leave the house with the documented board location and photographs",
                "Obtain a suitable pry tool, gloves, and if possible a witness or helper",
                "Return to the marked basement section and make one controlled tool-assisted test",
            ],
            selection_rationale=(
                "The basement route is no longer an information problem; it is a resource and safety problem. "
                "A player-like cautious investigator now changes tools before retrying the same obstacle."
            ),
            declared_action=(
                f"{pc} treats the basement board section as documented but not solved. She does not ask for "
                "the same basement affordance again. She leaves the house by her marked route, keeps the "
                "photographs and notebook references, and obtains a suitable pry tool, gloves, and, if available, "
                "a witness or helper before returning in daylight to the same marked basement section. On return "
                "she keeps the stairs and exterior route open, photographs the section again, and makes one "
                "controlled tool-assisted test at the seam or loose edge. If it resists, shifts dangerously, "
                "or anything moves behind it, she stops and resolves that consequence rather than forcing it."
            ),
            intent="leave_house_for_tools_after_documenting_stubborn_basement_boards",
            requested=[
                "whether Evelyn can leave and return with tools or a witness",
                "whether the documented board section is still reachable",
                "tool-assisted seam/edge result",
                "blocked reason, check, or consequence if the return is unsafe",
            ],
            unacceptable=[
                "Repeat the same basement affordance description",
                "Open the boards for free without resolving the tool-assisted test",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "basement_descent" in state.attempted
        and "basement_followup" not in state.attempted
        and clarified_basement_boards_affordance_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Turn the clarified basement boards into a cautious verification step.",
            hypotheses=[
                "The boards are a testable visible affordance after the failed broad basement read, not proof of a hidden answer.",
                "Reachability is uncertain enough that the approach path must be part of the action.",
            ],
            open_questions=[
                "Can Evelyn cross to the boards without triggering a hazard?",
                "Do the boards conceal an opening, cavity, object, body, or only old construction?",
                "Does probing or photographing them create noise or movement?",
            ],
            candidate_actions=[
                "Test the route to the boards from the marked stair line",
                "Photograph and probe the boards with the walking stick",
                "Retreat to the stairs if footing, clutter, or hidden movement worsens",
            ],
            selection_rationale=(
                "After one clarification pass, the boards are specific enough to approach and test; asking the same affordance question again would loop."
            ),
            declared_action=(
                f"{pc} treats the rough basement boards as the current testable affordance rather than asking for the same clarification again. "
                "Keeping the marked stairs behind her, she first tests the floor between her and the boards with the walking stick, "
                "then advances only far enough to photograph the boards and probe their edges from a braced stance. "
                "She does not put her hands into any gap. She checks whether the boards flex, hide an opening, reveal a cavity, "
                "or produce movement or sound; if the footing proves unsafe or anything reacts, she retreats to the marked stairs."
            ),
            intent="approach_clarified_basement_boards_from_stairs",
            requested=[
                "route safety from stairs to boards",
                "board probe result or clear resistance",
                "visible opening/cavity/object/body or clear absence",
                "threat reaction and Evelyn position",
            ],
            unacceptable=[
                "Repeat the same boards location/reachability description",
                "Invent contents behind the boards without resolving the probe",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "basement_descent" in state.attempted
        and "basement_stair_foot_probe" not in state.attempted
        and clarified_basement_stair_foot_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Resolve the safe stair-foot affordance before asking for farther basement details.",
            hypotheses=[
                "The Keeper established only the stair-foot floor patch and nearby foundation wall as safely reachable.",
                "A cautious player should examine that immediate area before committing deeper into the basement.",
            ],
            open_questions=[
                "Does the packed dirt, wall, or stair-foot show draft, loose material, scrape marks, or clear absence?",
                "Is there any safe route marker toward the rough boards or deeper clutter?",
                "Does close inspection trigger sound, movement, or a need to retreat?",
            ],
            candidate_actions=[
                "Examine packed dirt at the stair-foot with light and walking stick",
                "Check the nearby foundation wall for cracks, drafts, or loose stones",
                "Stay within the marked stair retreat line unless a safe route is established",
            ],
            selection_rationale=(
                "The latest reply already named a safe affordance at Evelyn's feet, so repeating the same clarification would be inhuman."
            ),
            declared_action=(
                f"{pc} stays at the foot of the basement stairs and acts on the nearest safe affordance instead of asking again. "
                "Keeping the wedged door and handkerchief marker behind her, she sweeps the flashlight over the stair-foot floor, "
                "any visible floor marks, damp masonry, the nearby foundation wall, and the first visible cellar details. "
                "With the walking stick she tests the dirt, "
                "stone edges, and any visible cracks for looseness, draft, scrape marks, or a clear lack of a lead. She does not move deeper "
                "toward the rough boards unless this close check establishes a safe route or a specific reason."
            ),
            intent="examine_basement_stair_foot_and_near_wall",
            requested=[
                "stair-foot floor, floor marks, damp masonry, and near-wall result",
                "draft/crack/loose dirt/scrape mark result or clear absence",
                "check and consequence if uncertain",
                "safe route or reason not to advance deeper",
            ],
            unacceptable=[
                "Repeat the same stair-foot affordance description",
                "Jump to deeper basement objects without route safety",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        basement_route_visible(last)
        and "basement_descent" in state.attempted
        and "bedroom_abandoned" in state.attempted
        and "resumed_basement_after_bedroom" not in state.attempted
    ):
        has_crawl_lead = "crawl_space_probe" in state.attempted or "basement_followup" in state.attempted
        return base_decision(
            state,
            persona=persona,
            active_goal="Resume the mapped basement line after abandoning the unsafe bedroom.",
            hypotheses=[
                "The bedroom has been marked unsafe, so the next physical line is the already mapped basement access.",
                "The latest reply has clarified the basement access enough to act; repeating the same clarification would loop.",
                (
                    "Earlier basement work exposed a crawl-space or concealed-section lead, so Evelyn should return to that known line."
                    if has_crawl_lead
                    else "The basement route is visible but not yet resolved, so Evelyn should descend with the same safety discipline."
                ),
            ],
            open_questions=[
                "Can Evelyn return to the marked basement stair route without a new obstacle?",
                "Is the previously found basement/crawl-space lead still reachable?",
                "Does anything react as she recommits to the downward route?",
            ],
            candidate_actions=[
                "Return to the basement access with the exterior retreat path in mind",
                "Use the handkerchief marker, flashlight, and walking stick to resume the mapped line",
                "If the crawl-space lead is already known, go back to that opened point rather than restarting at the side door",
            ],
            selection_rationale=(
                "The Keeper has already grounded the basement access. A cautious player now resumes that route, preserving retreat, instead of asking where the access is again."
            ),
            declared_action=(
                (
                    f"{pc} does not ask again where the basement access is. She returns to the mapped basement route "
                    "with the exterior exit path in mind, checks that her handkerchief marker and stair retreat line "
                    "are still usable, and goes back down toward the previously opened crawl-space / concealed-section "
                    "lead rather than restarting the house from the side door. She keeps the flashlight low and the "
                    "walking stick ahead of her, stopping immediately if the route has changed, something blocks the "
                    "stairs, or the basement reacts."
                )
                if has_crawl_lead
                else (
                    f"{pc} does not ask again where the basement access is. She treats the clarified way down as "
                    "the active route, marks the door with a handkerchief if needed, keeps the exit path in mind, "
                    "and descends one tested step at a time with flashlight and walking stick ready. She stops "
                    "immediately if the route has changed, something blocks the stairs, or the basement reacts."
                )
            ),
            intent="resume_mapped_basement_route_after_bedroom_retreat",
            requested=[
                "whether the resumed basement route is reachable",
                "current basement-relative position",
                "crawl-space/concealed-section status if already found",
                "check or consequence if the route has changed",
            ],
            unacceptable=[
                "Repeat the same basement access location description",
                "Reset Evelyn to the exterior side door without a concrete transition",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "basement_tools_return" in state.attempted
        and "failed_tool_assisted_board_exit" not in state.attempted
        and failed_tool_assisted_basement_board_test_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Stop repeating the failed basement board test and preserve the witness-backed evidence.",
            hypotheses=[
                "Evelyn already changed resources and made a tool-assisted test; another clarification will only loop.",
                "The realistic cautious next step is to leave with the witness and escalate the documented obstruction.",
            ],
            open_questions=[
                "Can Evelyn and the witness exit the house safely with notes and photographs?",
                "Does anything react while they withdraw from the basement?",
                "What external help, permission, or demolition-level tool would be needed before another attempt?",
            ],
            candidate_actions=[
                "Withdraw with the witness and evidence intact",
                "Report the marked basement obstruction to Knott or authorities",
                "Return only with stronger help rather than repeating the same probe",
            ],
            selection_rationale=(
                "After a failed tool-assisted seam test, a player-like cautious investigator changes scope "
                "and safety resources instead of asking the Keeper to restate the same boarded under-space."
            ),
            declared_action=(
                f"{pc} accepts that the tool-assisted test failed and does not ask for the same boarded "
                "under-space again. She photographs the tool slip, the board edge, and the witness's position, "
                "then backs out with the witness along the marked basement stairs and exterior route. Outside, "
                "she writes a clean report for Knott and notes that any further attempt requires stronger help, "
                "permission to open the wall properly, or authorities/workmen rather than another solo probe. "
                "If anything moves or blocks the withdrawal, she stops and resolves that immediate threat first."
            ),
            intent="exit_after_failed_tool_assisted_basement_board_test",
            requested=[
                "whether Evelyn and the witness exit safely",
                "any reaction from the boarded under-space during withdrawal",
                "evidence preserved in notes/photos",
                "next external help or clear blocker",
            ],
            unacceptable=[
                "Repeat the same boarded under-space description",
                "Ask for another generic latest-affordance clarification",
                "Move Evelyn to another floor without resolving the failed tool test",
            ],
        )

    if (
        upper_route_visible(last)
        and "upper_floor_search" not in state.attempted
        and not current_crawl_space_affordance_visible(last)
        and not (
            basement_route_visible(last)
            and "basement_descent" not in state.attempted
            and "enter_threshold" in state.attempted
            and (
                positive_burial_or_basement_clue_visible(state.transcript)
                or basement_route_priority_visible(last)
            )
        )
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Check the upper floor for hazards, documents, and unnatural movement.",
            hypotheses=[
                "The upper floor may contain bedroom evidence or an active haunting hazard."
            ],
            open_questions=[
                "Are stairs safe?",
                "Which room is most disturbed?",
                "Is there a physical threat that must be dodged or escaped?",
            ],
            candidate_actions=[
                "Search upstairs cautiously",
                "Look for cellar access before going up",
                "Leave and get help if the structure is unsafe",
            ],
            selection_rationale=f"{pc} follows visible architecture with safety discipline and keeps the retreat path open.",
            declared_action=(
                f"{pc} tests each stair with the walking stick before putting weight on it, then goes "
                "up only if the route holds. Upstairs, she keeps the landing behind her clear and checks "
                "rooms from the doorway first: bed, wardrobe, windows, loose papers, wall marks, smells, "
                "and any sign that furniture or bedding moved recently. If something moves by itself, "
                "she backs toward the landing rather than wrestling it in the room."
            ),
            intent="cautious_upper_floor_search",
            requested=["specific upper-floor clues", "hazards", "movement consequences", "safe route status"],
        )

    if (
        basement_route_visible(last)
        and "basement_descent" not in state.attempted
        and "enter_threshold" in state.attempted
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Descend toward the cellar only after establishing a retreat path.",
            hypotheses=[
                "The cellar or basement may contain the strongest physical evidence or threat.",
                f"Confined spaces are high risk for {pc}'s mild claustrophobia.",
            ],
            open_questions=[
                "Is the cellar route physically safe?",
                "What sounds, smells, marks, or air movement are visible before full descent?",
                "Is there a hidden compartment, body, ritual object, or active threat?",
            ],
            candidate_actions=[
                "Descend slowly with light and retreat line",
                "Listen and inspect from the top of the stairs first",
                "Leave to get help if the basement is structurally unsafe",
            ],
            selection_rationale="The basement is a plausible investigation target, but the player declares safety constraints clearly.",
            declared_action=(
                f"{pc} ties a handkerchief to the basement door handle as a return marker, leaves the "
                "door wedged open, and listens from the top of the stairs. With flashlight and walking "
                "stick ready, she descends one step at a time, testing boards before committing weight. "
                "She looks for drafts, loose brick, disturbed earth, scrape marks, hidden panels, a body, "
                "a ritual object, or any moving threat, and she stops immediately if something attacks."
            ),
            intent="safe_basement_descent_and_search",
            requested=["basement layout", "physical evidence", "hidden access or clear absence", "threat and resolved consequence"],
        )

    if (
        "upper_floor_search" in state.attempted
        and "bedroom_threshold_probe" not in state.attempted
        and clarified_interior_threshold_affordance_visible(last)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Turn the clarified bedroom doorway into a concrete threshold probe.",
            hypotheses=[
                "The nearby bedroom doorway is the safest named interior affordance after the upper-floor failure."
            ],
            open_questions=[
                "Does probing the bed, floor, wardrobe, or papers reveal a concrete clue or hazard?",
                "Does anything move once the room is tested from the threshold?",
                "Can Evelyn keep the landing retreat route clear?",
            ],
            candidate_actions=[
                "Probe the nearest bedroom from the threshold",
                "Withdraw from the upper floor and reassess",
                "Move toward the cellar only if the bedroom remains inert",
            ],
            selection_rationale=(
                "After the Keeper clarified the bedroom doorway, a cautious player tests it from cover instead of asking the same question again."
            ),
            declared_action=(
                f"{pc} uses the clarified bedroom doorway instead of asking for the same description again. "
                "She stays on the landing side of the threshold with her retreat path open, shines the flashlight "
                "low across the bed, floorboards, wardrobe, window frame, and loose papers, and uses the walking "
                "stick to probe only what she can reach from the doorway. She photographs any concrete mark or "
                "document before touching it. If the bed, furniture, papers, or anything else moves by itself, "
                "she backs toward the landing and treats that movement as the immediate result to resolve."
            ),
            intent="probe_clarified_bedroom_threshold",
            requested=[
                "specific bedroom clue or clear absence",
                "check and outcome if uncertain",
                "movement/hazard consequence if triggered",
                "safe retreat status",
            ],
            unacceptable=[
                "Repeat the same bedroom doorway description without resolving the probe",
                "Offer an explicit action menu to the player",
                "Move Evelyn deep into the room without threshold consequences",
            ],
        )

    if (
        "bedroom_threshold_probe" in state.attempted
        and "bed_edge_probe" not in state.attempted
        and clarified_disturbed_bed_edge_visible(state.last_reply)
        and not clarified_reachable_bedroom_paper_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Test the disturbed bed edge from the doorway without entering the room.",
            hypotheses=[
                "The bed edge is the concrete reachable affordance named by the Keeper.",
                "The disturbance may be natural, human, or supernatural, so the test must preserve retreat.",
            ],
            open_questions=[
                "Does the bedspread or near edge move normally when lifted or tugged with the stick?",
                "Does probing reveal a mark, object, body, or clear absence?",
                "Does the bed react as an active hazard?",
            ],
            candidate_actions=[
                "Hook and lift the nearest bedspread edge with the walking stick",
                "Watch the bedframe, floor, and papers for reaction",
                "Retreat to the landing if the bed moves by itself",
            ],
            selection_rationale=(
                "The Keeper has already clarified the bed edge's location and reachability, so the player should test it rather than ask again."
            ),
            declared_action=(
                f"{pc} acts on the clarified bed edge instead of asking for the same description again. "
                "Remaining on the landing side of the bedroom threshold, she hooks the hanging bedspread "
                "or nearest bed edge with the walking stick and gives it one controlled lift or tug. "
                "She watches for what is revealed under or around the bedding, whether the bed moves normally "
                "or by itself, and whether the floor, papers, or wardrobe react. If the bed lurches, resists "
                "unnaturally, or anything attacks, she drops the stick pressure and backs toward the stairs."
            ),
            intent="probe_disturbed_bed_edge_from_threshold",
            requested=[
                "bed edge/bedspread probe result",
                "specific revealed clue or clear absence",
                "check and consequence if uncertain",
                "movement/hazard reaction and Evelyn position",
            ],
            unacceptable=[
                "Repeat the same bed-edge affordance description",
                "Move Evelyn deep into the bedroom for free",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "bedroom_threshold_probe" in state.attempted
        and "bedroom_paper_probe" not in state.attempted
        and clarified_reachable_bedroom_paper_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Inspect the reachable bedroom paper without entering the unsafe room.",
            hypotheses=[
                "The paper is the only clarified reachable affordance after the threshold probe.",
                "A cautious player can inspect it from the doorway without committing deeper into the bedroom.",
            ],
            open_questions=[
                "Does the paper contain legible writing, names, dates, or symbols?",
                "Does drawing the paper closer trigger movement from the room?",
                "Can Evelyn keep the landing retreat route open while inspecting it?",
            ],
            candidate_actions=[
                "Photograph the paper from the doorway",
                "Draw it closer with the walking stick and read it from the threshold",
                "Withdraw if it moves unnaturally or the bed reacts",
            ],
            selection_rationale=(
                "After one clarification pass, the reachable paper is concrete enough to inspect; asking again would loop."
            ),
            declared_action=(
                f"{pc} does not ask for the same paper description again. Staying on the landing side "
                "of the threshold, she photographs the loose paper in place, then uses the walking stick "
                "to draw it only as close as the doorway if it moves normally. She keeps the stairs clear "
                "behind her and reads or records any visible writing, symbol, date, name, or clear blankness "
                "without stepping into the bedroom. If the paper moves on its own or the bed reacts, she "
                "backs away and treats that movement as the immediate result."
            ),
            intent="inspect_reachable_bedroom_paper_from_threshold",
            requested=[
                "paper content or clear blank/illegible result",
                "check and outcome if reading/handling is uncertain",
                "movement/hazard consequence if triggered",
                "Evelyn position after the inspection",
            ],
            unacceptable=[
                "Repeat the same paper affordance description",
                "Move Evelyn deep into the bedroom for free",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "bedroom_threshold_probe" in state.attempted
        and "bedroom_paper_probe" not in state.attempted
        and clarified_visible_bedroom_papers_need_bounded_entry_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Act on the clarified bedroom papers with the smallest bounded commitment.",
            hypotheses=[
                "The Keeper has clarified that the papers are real, visible, and potentially informative.",
                "They cannot be read from the threshold, so another clarification would only repeat the same affordance.",
                "A cautious player can make one bounded entry while preserving retreat, or trigger a check/consequence.",
            ],
            open_questions=[
                "Can Evelyn take one controlled step or lean far enough to hook or collect the nearest paper?",
                "Does the room, bed, wardrobe, or paper react when she crosses the threshold?",
                "What is written on the paper if she can retrieve or photograph it?",
            ],
            candidate_actions=[
                "Photograph the papers from the threshold first",
                "Take one controlled step or lean only far enough to hook the nearest paper",
                "Retreat immediately if anything moves or the GM calls for danger",
            ],
            selection_rationale=(
                "The papers have been grounded repeatedly. A realistic cautious investigator now either makes a bounded attempt or abandons the line; she does not keep asking where they are."
            ),
            declared_action=(
                f"{pc} stops asking for the same paper location. From the threshold, she first photographs "
                "the papers exactly where they lie. Then she makes the smallest commitment that could actually "
                "resolve them: one controlled lean or one careful step, keeping her shoulders angled toward the "
                "doorway and the landing retreat open, using the walking stick to hook or draw the nearest paper "
                "back if possible. She reads or records only what she can retrieve or see from that bounded position. "
                "If the bed, wardrobe, floor, paper, or room moves, or if the Keeper calls for a check, she stops "
                "and resolves that consequence before doing anything else."
            ),
            intent="inspect_visible_bedroom_papers_with_bounded_entry",
            requested=[
                "whether the bounded step/lean succeeds, fails, or needs a check",
                "paper text or clear blank/illegible result",
                "room reaction or clear absence of reaction",
                "Evelyn position and retreat status after the attempt",
            ],
            unacceptable=[
                "Repeat the same paper location description",
                "Move Evelyn deep into the room without resolving risk",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "bedroom_paper_probe" in state.attempted
        and "bedroom_paper_close_examine" not in state.attempted
        and retrieved_bedroom_paper_needs_close_examination_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Finish the retrieved paper inspection before asking for another affordance.",
            hypotheses=[
                "The paper is now at the threshold and physically reachable without entering the bedroom.",
                "The unresolved facts are closer handling, the reverse side, and any faint or impressed marks.",
            ],
            open_questions=[
                "Does the reverse side show writing, symbols, dates, names, or blankness?",
                "Are there faint, impressed, hidden, or otherwise subtle marks under close light?",
                "Does picking up or turning the paper trigger any room reaction?",
            ],
            candidate_actions=[
                "Pick up and turn over the retrieved paper at the threshold",
                "Angle the flashlight across both sides for faint or impressed marks",
                "Withdraw from the room if the bed or furniture reacts",
            ],
            selection_rationale=(
                "The Keeper already named the paper's position and reachability, so a cautious player handles that object instead of re-asking."
            ),
            declared_action=(
                f"{pc} stops asking where the paper is because it is already at the threshold. "
                "Keeping her feet on the landing side and the stairs clear, she picks up the sheet only "
                "enough to examine it from the doorway. She turns it over to check the reverse side, angles "
                "the flashlight across both sides for faint writing, impressed marks, dates, names, symbols, or confirmed blankness, "
                "and keeps her attention on the bed and furniture for any reaction. If the room moves or the "
                "paper behaves unnaturally, she drops it and retreats toward the stairs."
            ),
            intent="closely_examine_retrieved_bedroom_paper",
            requested=[
                "front and reverse side result",
                "faint/impressed mark result or clear absence",
                "check and outcome if close examination is uncertain",
                "room reaction and Evelyn position",
            ],
            unacceptable=[
                "Repeat the same paper location/reachability description",
                "Move Evelyn deep into the bedroom for free",
                "Offer an explicit action menu to the player",
            ],
        )

    if (
        "ground_floor_search" in state.attempted
        and "ground_floor_anomaly_probe" not in state.attempted
        and nearby_ground_floor_anomaly_affordance_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Inspect the clarified ground-floor anomaly without surrendering the exit.",
            hypotheses=[
                "The Keeper has clarified a nearby visible anomaly or displaced object rather than a distant route.",
                "After one clarification pass, a player-like cautious investigator tests the concrete affordance instead of asking the same question again.",
            ],
            open_questions=[
                "What is the anomaly or displaced object on closer inspection?",
                "Does approaching or probing it require a check or trigger a hazard?",
                "Does the open retreat route remain clear after the probe?",
            ],
            candidate_actions=[
                "Photograph and inspect the anomaly from the current position",
                "Advance only far enough to probe it with the walking stick",
                "Retreat if the object moves, the route changes, or the house reacts",
            ],
            selection_rationale=(
                "The last answer grounded a nearby anomaly with a clear retreat path; repeating the same grounding request would be a player-sim loop."
            ),
            declared_action=(
                f"{pc} treats the clarified hall anomaly as enough to act on rather than asking "
                "for the same affordance again. Keeping the open door and retreat path behind her, "
                "she photographs the anomaly from where she stands, marks its position relative to the "
                "exit in her notes, then advances only as far as needed to inspect it from walking-stick "
                "reach. She uses the stick before her hands, checks for writing, marks, fresh dust breaks, "
                "scrape direction, wires, loose floorboards, or any sign it was deliberately placed. If it "
                "moves, the floor gives, the air changes sharply, or the exit line is threatened, she stops "
                "and resolves that danger before touching anything."
            ),
            intent="inspect_clarified_ground_floor_anomaly_from_retreat",
            requested=[
                "closer-inspection result for the anomaly or object",
                "check and outcome if the probe is uncertain or dangerous",
                "hazard/reaction or clear absence of reaction",
                "Evelyn position and retreat-path status after the probe",
            ],
            unacceptable=[
                "Repeat the same vague anomaly description without resolving the probe",
                "Offer an explicit action menu to the player",
                "Move Evelyn deep into the house without resolving the local consequence",
            ],
        )

    if active_blade_threat_visible(state.last_reply) and "knife_defense" not in state.attempted:
        return base_decision(
            state,
            persona=persona,
            active_goal="Resolve the visible knife/dagger threat without handwaving danger.",
            hypotheses=[
                "The knife is an immediate hazard and must be dodged, disabled, or used only if visibly reachable."
            ],
            open_questions=[
                f"Can {pc} get cover?",
                "Is the weapon reachable without being stabbed?",
                "Does a roll, damage, or status change resolve the danger?",
            ],
            candidate_actions=[
                "Dodge behind cover and knock the weapon away",
                "Retreat up the stairs",
                "Shoot at the visible attacking source if one is exposed",
            ],
            selection_rationale=f"When an immediate weapon threat is visible, {pc} handles it before continuing investigation.",
            declared_action=(
                f"{pc} treats the visible knife or dagger as the immediate threat. She drops low, puts "
                "the nearest solid object or stair angle between herself and the blade, and uses the "
                "walking stick or a coat-wrapped hand only to knock it aside if it comes within reach. "
                "If the source controlling it is visible, she aims the revolver at that source; otherwise "
                "she retreats toward the marked exit rather than standing in the open."
            ),
            intent="defend_against_visible_blade_threat",
            requested=["dodge or combat check", "damage or avoided damage", "weapon/source position", "state after defense"],
        )

    if (
        "knife_defense" in state.attempted
        and "knife_retreat" not in state.attempted
        and active_blade_threat_after_defense_visible(state.last_reply)
    ):
        return base_decision(
            state,
            persona=persona,
            active_goal="Convert the active self-moving blade threat into retreat or suppression.",
            hypotheses=[
                "The knife remains the immediate active threat after the first defense.",
                "The exit route is still physically possible, so the cautious response is to give ground instead of asking for another description.",
                "If the blade closes, the next resolution should be Dodge/Fighting/Shooting, damage, or a clear escape result.",
            ],
            open_questions=[
                "Can Evelyn withdraw through the marked exit before the blade closes?",
                "Does the knife pursue, strike, or hold position?",
                "Is there a visible controller or source to shoot at, or only the blade itself?",
            ],
            candidate_actions=[
                "Retreat along the marked exit route while keeping cover between Evelyn and the blade",
                "Use walking stick, coat, or furniture to block if the blade closes",
                "Fire only if the blade or its visible source gives a clear shot before escape",
            ],
            selection_rationale=(
                "After an immediate weapon threat and failed fear response, a player-like cautious investigator resolves safety, not affordance wording."
            ),
            declared_action=(
                f"{pc} gives ground immediately. Keeping the stair angle, furniture, or nearest solid cover "
                "between herself and the self-moving blade, she retreats along her marked path toward the open exit. "
                "She keeps the walking stick and coat between her body and the knife, and only fires if the blade "
                "or a visible controlling source gives her a clear shot before she can clear the room. If the knife "
                "lunges or cuts off the exit, she dodges or knocks it aside and resolves that combat result before "
                "trying to move farther."
            ),
            intent="retreat_from_active_blade_threat_to_marked_exit",
            requested=[
                "whether Evelyn reaches the marked exit route",
                "knife pursuit/strike/hold-position result",
                "Dodge/Fighting/Shooting check and damage if contested",
                "Evelyn harm, sanity, and position after the retreat attempt",
            ],
            unacceptable=[
                "Repeat the same knife-position affordance description",
                "Ignore the active threat and continue investigation",
                "Offer an explicit action menu to the player",
            ],
        )

    if corbitt_body_or_remains_visible(last) and "confront_corbitt" not in state.attempted:
        return base_decision(
            state,
            persona=persona,
            active_goal="End the immediate threat posed by the visible Corbitt body or presence.",
            hypotheses=[
                "The direct target is now visible; the action should produce combat, destruction, retreat, or a clear failed consequence."
            ],
            open_questions=[
                "Is Corbitt physically vulnerable?",
                f"Does {pc}'s attack hit and change his state?",
                "What harm or sanity cost follows?",
            ],
            candidate_actions=[
                "Shoot the visible Corbitt body or active source",
                "Use a visible ritual/weapon object if already established",
                "Retreat and seek help if direct attack fails",
            ],
            selection_rationale=f"Once the threat is directly visible, {pc} takes a concrete defensive attack rather than asking for options.",
            declared_action=(
                f"{pc} does not try to bargain with the thing in the room. Keeping as much cover as "
                "the cramped space allows, she aims at the visible Corbitt body or active source and "
                "fires controlled shots until it is no longer moving or until the GM resolves that the "
                "attack is ineffective. If a previously visible weapon or ritual object is clearly the "
                "only useful tool, she switches to that; otherwise she retreats if the shots do nothing."
            ),
            intent="attack_visible_corbitt_threat",
            requested=["attack roll", "damage/effect", "Corbitt state change or clear failure", f"{pc} harm or sanity cost"],
        )

    return base_decision(
        state,
        persona=persona,
        active_goal="Clarify the latest player-visible result before choosing a new physical commitment.",
        hypotheses=[
            f"If no decisive clue appeared, {pc} should ask a concrete clarifying question rather than guessing a route."
        ],
        open_questions=[
            "What exact new fact did the last action establish?",
            "What visible affordance is safest and most relevant now?",
        ],
        candidate_actions=[
            "Ask the Keeper to ground the latest actionable thing in plain terms",
            "Ask a clarifying question about exits, hazards, or the previous result",
            "Retreat and reassess notes if the scene is dangerous or unclear",
        ],
        selection_rationale="Fallback asks for missing player-visible information instead of sending a conditional action template.",
        declared_action=(
            f"{pc} pauses and writes exactly what she can see and what she still cannot prove. "
            "She asks the Keeper to ground the latest actionable thing in plain language: what "
            "she can actually perceive from here, where it sits relative to her retreat path, "
            "and what remains uncertain. She does not move deeper until that concrete affordance "
            "is named without turning the answer into a checklist."
        ),
        intent="clarify_latest_visible_affordance",
        requested=["specific visible fact", "check request", "blocked reason", "new hazard or consequence"],
    )


def apply_attempt_marker(decision: dict[str, Any], state: VisibleState) -> None:
    intent = decision.get("response_contract", {}).get("intent", "")
    markers = {
        "search_hall_records": "hall_records",
        "continue_hall_records": "hall_records",
        "access_1912": "official_records",
        "access_general_corbitt": "official_records",
        "continue_official_records": "official_records",
        "search_newspaper": "newspaper_archive",
        "press_newspaper_archive_access": "newspaper_access_tone",
        "investigate_chapel": "chapel",
        "continue_chapel_search": "chapel_followup",
        "retry_chapel": "second_chapel_method",
        "examine_clarified_chapel_cabinet": "chapel_cabinet_probe",
        "return_to_marked_basement_after_chapel_burial": "burial_record_basement_return",
        "inspect_house_exterior": "house_exterior",
        "enter_threshold_and_map": "enter_threshold",
        "systematic_ground_floor": "ground_floor_search",
        "cautious_upper_floor": "upper_floor_search",
        "safe_basement": "basement_descent",
        "follow_basement": "basement_followup",
        "resume_mapped_basement_route": "resumed_basement_after_bedroom",
        "approach_clarified_basement_boards": "basement_followup",
        "examine_basement_stair_foot": "basement_stair_foot_probe",
        "recover_from_failed_basement": "basement_recovery",
        "exit_basement_after": "basement_exit",
        "reassess_open_basement_stair_mouth": "basement_threshold_reassess",
        "withdraw_from_failed_basement": "basement_exit",
        "resolve_clarified_basement_door": "basement_door_probe",
        "probe_clarified_concealed_basement": "basement_concealed_probe",
        "inspect_opened_crawl_space": "crawl_space_probe",
        "enter_crawl_space_one_body_length": "crawl_space_entry",
        "inspect_deeper_recess_table_papers": "deeper_recess_papers_probe",
        "probe_broken_cellar_opening": "broken_cellar_opening_probe",
        "withdraw_from_stubborn_concealed_basement_boards": "stubborn_basement_boards_withdrawal",
        "leave_house_for_tools_after_documenting_stubborn_basement_boards": "basement_tools_return",
        "exit_after_failed_tool_assisted_basement_board_test": "failed_tool_assisted_board_exit",
        "probe_deeper_crawl_space": "crawl_space_deeper_probe",
        "withdraw_from_unresolved_crawl_space": "crawl_space_withdrawal",
        "withdraw_from_unstable_crawl_space": "crawl_space_withdrawal",
        "leave_house_for_tools_after_crawl_space_withdrawal": "basement_tools_return",
        "return_with_tools_to_documented_crawl_space_route": "returned_with_tools",
        "use_prepared_tools_on_documented_crawl_space": "tool_assisted_crawl_space_test",
        "probe_crawl_space_inner_boundary": "crawl_space_inner_boundary_probe",
        "inspect_reconfirmed_crawl_space": "crawl_space_wall_inspection",
        "stabilize_loosened_basement_gap": "basement_gap_stabilized",
        "probe_clarified_bedroom": "bedroom_threshold_probe",
        "probe_disturbed_bed_edge": "bed_edge_probe",
        "inspect_reachable_bedroom_paper": "bedroom_paper_probe",
        "inspect_bed_centered_scene": "bedroom_scene_followup",
        "inspect_visible_bedroom_papers": "bedroom_paper_probe",
        "closely_examine_retrieved_bedroom_paper": "bedroom_paper_close_examine",
        "inspect_clarified_ground_floor_anomaly": "ground_floor_anomaly_probe",
        "abandon_unsafe_bedroom": "bedroom_abandoned",
        "abandon_unproductive_bedroom": "bedroom_abandoned",
        "withdraw_from_confirmed_impossible_bedroom": "bedroom_abandoned",
        "recover_from_bedroom_landing_crash": "bedroom_crash_recovery",
        "defend_against": "knife_defense",
        "retreat_from_active_blade": "knife_retreat",
        "retreat_from_moving_bedroom": "moving_bedroom_threat",
        "attack_visible": "confront_corbitt",
        "enter_threshold_via_clarified": "door_opened",
    }
    for cue, marker in markers.items():
        if cue in intent:
            state.attempted.add(marker)
    if "systematic_ground_floor" in intent:
        state.attempted.add("enter_threshold")


def run_turn(
    root: Path,
    run_dir: Path,
    session_id: str,
    decision: dict[str, Any],
    *,
    timeout: int = DEFAULT_TURN_TIMEOUT_SECONDS,
    output_language: str | None = None,
) -> str:
    turn = int(decision["turn"])
    action = str(decision["sent_to_gm"])
    append_jsonl(run_dir / "player_decisions.jsonl", decision)
    append_jsonl(run_dir / "signals.jsonl", decision)

    input_path = run_dir / f"turn_{turn:03d}.input.txt"
    jsonl_path = run_dir / f"turn_{turn:03d}.jsonl"
    err_path = run_dir / f"turn_{turn:03d}.err"
    input_path.write_text(action, encoding="utf-8")

    proc = run_cmd(
        [
            str(root / "target-f1/debug/trpg"),
            "turn",
            "--ruleset",
            RULESET,
            "--module",
            MODULE,
            "--session-id",
            session_id,
            "--stream-format",
            "jsonl",
            "--recent-transcript-file",
            str(run_dir / "transcript_player_visible.txt"),
            "--input-file",
            str(input_path),
        ],
        env=trpg_env(output_language),
        timeout=timeout,
    )
    jsonl_path.write_text(proc.stdout, encoding="utf-8")
    err_path.write_text(proc.stderr, encoding="utf-8")
    if proc.returncode != 0:
        raise RuntimeError(f"turn {turn} failed with exit {proc.returncode}: {proc.stderr}")

    gm_text = extract_delta(jsonl_path)
    with (run_dir / "transcript_player_visible.txt").open("a", encoding="utf-8") as handle:
        handle.write(f"## Turn {turn}\nPLAYER: {action}\n\nGM: {gm_text}\n\n")

    turn_id = latest_turn_id(root, session_id)
    (run_dir / f"turn_{turn:03d}.turn_id.txt").write_text(turn_id + "\n", encoding="utf-8")
    if turn_id:
        explain = run_cmd(
            [str(root / "target-f1/debug/trpg"), "explain", "--session", session_id, "--turn", turn_id],
            env=trpg_env(output_language),
            timeout=90,
        )
        (run_dir / f"explain_{turn:03d}.txt").write_text(explain.stdout, encoding="utf-8")
        (run_dir / f"explain_{turn:03d}.err").write_text(explain.stderr, encoding="utf-8")

    return gm_text


def run_redboard(root: Path, run_dir: Path) -> dict[str, Any]:
    cmd = [sys.executable, str(root / "scripts/eval_redboard.py"), str(run_dir), "--write"]
    critic_cmd = semantic_critic_cmd(root)
    if critic_cmd:
        cmd.extend(["--semantic-critic-cmd", critic_cmd])
    timeout = int(os.environ.get("TRPG_EVAL_REDBOARD_TIMEOUT", "300" if critic_cmd else "60"))
    proc = run_cmd(
        cmd,
        timeout=timeout,
    )
    (run_dir / "eval_redboard.stdout").write_text(proc.stdout, encoding="utf-8")
    (run_dir / "eval_redboard.stderr").write_text(proc.stderr, encoding="utf-8")
    verdict_path = run_dir / "verdict.json"
    if verdict_path.exists():
        return json.loads(verdict_path.read_text(encoding="utf-8"))
    return {"verdict": "FAIL", "red_count": 999, "error": proc.stderr}


def run_coverage(root: Path, run_dir: Path, session_id: str) -> None:
    proc = run_cmd(
        [str(root / "target-f1/debug/trpg"), "coverage", "--session", session_id],
        env=trpg_env(),
        timeout=90,
    )
    (run_dir / "coverage.txt").write_text(proc.stdout, encoding="utf-8")
    (run_dir / "coverage.err").write_text(proc.stderr, encoding="utf-8")


def write_battle_report(run_dir: Path) -> None:
    parts = [
        "# CoC The Haunting Autoplay Battle Report",
        "",
        "## Evaluation Snapshot",
        "",
    ]
    meta_path = run_dir / "redboard.meta.json"
    if meta_path.exists():
        try:
            meta = json.loads(meta_path.read_text(encoding="utf-8", errors="replace"))
        except json.JSONDecodeError:
            meta = {}
        if isinstance(meta, dict):
            parts.extend(
                [
                    f"- verdict: `{meta.get('verdict', 'unknown')}`",
                    f"- red_count: `{meta.get('red_count', 'unknown')}`",
                    f"- redboard_sha256: `{meta.get('redboard_sha256', 'unknown')}`",
                    f"- verdict_sha256: `{meta.get('verdict_sha256', 'unknown')}`",
                    "",
                ]
            )
    else:
        parts.extend(["- redboard_meta: `missing`", ""])
    parts.extend(
        [
            "## Character",
            "",
            (run_dir / "character_sheet.md").read_text(encoding="utf-8", errors="replace"),
            "",
            "## Player Decisions",
            "",
        ]
    )
    for row in read_jsonl(run_dir / "player_decisions.jsonl"):
        turn = row.get("turn")
        parts.extend(
            [
                f"### Turn {turn} Decision",
                "",
                f"- Last result: {row.get('last_action_result', '')}",
                f"- Active goal: {row.get('active_goal', '')}",
                f"- Perceived facts: {'; '.join(row.get('perceived_facts', []))}",
                f"- Candidate actions: {'; '.join(row.get('candidate_actions', []))}",
                f"- Sent to GM: {row.get('sent_to_gm', '')}",
                f"- Response contract: {row.get('response_contract', {}).get('intent', '')}",
                "",
            ]
        )
    parts.extend(
        [
            "## Player-Visible Transcript",
            "",
            (run_dir / "transcript_player_visible.txt").read_text(
                encoding="utf-8", errors="replace"
            ),
            "",
            "## Redboard",
            "",
        ]
    )
    redboard = run_dir / "redboard.md"
    if redboard.exists():
        parts.append(redboard.read_text(encoding="utf-8", errors="replace"))
    (run_dir / "battle_report_with_character.md").write_text("\n".join(parts), encoding="utf-8")


def stop_reason(latest_gm_reply: str) -> str | None:
    lowered = latest_gm_reply.lower()
    if corbitt_resolved_marker_visible(lowered):
        return "corbitt_resolved"
    if (
        "case is over" in lowered
        or "investigation is complete" in lowered
        or ("knott" in lowered and "thanks" in lowered and "house" in lowered)
    ):
        return "case_resolved"
    return None


def corbitt_resolved_marker_visible(lowered: str) -> bool:
    target_cues = (
        "walter corbitt",
        "corbitt's body",
        "corbitt’s body",
        "corbitt corpse",
        "corbitt himself",
        "body of corbitt",
        "corpse of corbitt",
    )
    resolved_cues = ("destroyed", "no longer moving", "falls still", "collapses")
    negation_cues = (
        "nothing dramatic collapses",
        "nothing collapses",
        "does not collapse",
        "doesn't collapse",
        "does not destroy",
        "doesn't destroy",
        "not destroyed",
        "not wholly destroyed",
        "no longer unresolved",
    )
    for sentence in re.split(r"[.!?。！？]\s*", lowered):
        if not any(cue in sentence for cue in target_cues):
            continue
        if not any(cue in sentence for cue in resolved_cues):
            continue
        if any(cue in sentence for cue in negation_cues):
            continue
        return True
    return False


def create_run(
    root: Path,
    label: str,
    output_language: str | None = None,
    *,
    persona: str = "cautious_investigator",
) -> tuple[Path, str, str]:
    output_language = normalize_output_language(output_language)
    timestamp = time.strftime("%Y%m%d_%H%M%S")
    run_dir = root / ".tmp/eval" / f"{label}_{timestamp}"
    run_dir.mkdir(parents=True, exist_ok=True)
    (root / ".tmp/latest_coc_the_haunting_run.txt").write_text(str(run_dir), encoding="utf-8")
    session_id = run_dir.name
    (run_dir / "session_id.txt").write_text(session_id + "\n", encoding="utf-8")
    (run_dir / "run_config.json").write_text(
        json.dumps(
            {
                "output_language": output_language,
                "persona": persona,
            },
            ensure_ascii=False,
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )

    create = run_cmd(
        [
            str(root / "target-f1/debug/trpg"),
            "create-character",
            "--ruleset",
            RULESET,
            "--module",
            MODULE,
            "--session-id",
            session_id,
            "--auto",
            "--stream-format",
            "jsonl",
            "--preferences",
            character_preferences(output_language),
        ],
        env=trpg_env(output_language),
        timeout=CREATE_CHARACTER_TIMEOUT_SECONDS,
    )
    (run_dir / "create_character.jsonl").write_text(create.stdout, encoding="utf-8")
    (run_dir / "create_character.err").write_text(create.stderr, encoding="utf-8")
    if create.returncode != 0:
        raise RuntimeError(f"create-character failed: {create.stderr}")
    character_name = write_character_sheet(run_dir / "create_character.jsonl", run_dir / "character_sheet.md")

    opening = run_cmd(
        [
            str(root / "target-f1/debug/trpg"),
            "opening",
            "--ruleset",
            RULESET,
            "--module",
            MODULE,
            "--session-id",
            session_id,
        ],
        env=trpg_env(output_language),
        timeout=90,
    )
    localized_opening = localize_opening_for_output_language(opening.stdout, output_language)
    (run_dir / "opening.raw.txt").write_text(opening.stdout, encoding="utf-8")
    (run_dir / "opening.txt").write_text(localized_opening, encoding="utf-8")
    (run_dir / "opening.err").write_text(opening.stderr, encoding="utf-8")
    if opening.returncode != 0:
        raise RuntimeError(f"opening failed: {opening.stderr}")

    (run_dir / "transcript_player_visible.txt").write_text(
        f"## Opening\nGM: {localized_opening.strip()}\n\n",
        encoding="utf-8",
    )
    return run_dir, session_id, character_name


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--max-turns", type=int, default=40)
    parser.add_argument("--label", default="coc_the_haunting_autoplay")
    parser.add_argument("--persona", default="cautious_investigator")
    parser.add_argument("--turn-timeout", type=int, default=DEFAULT_TURN_TIMEOUT_SECONDS)
    parser.add_argument("--output-language", default=DEFAULT_OUTPUT_LANGUAGE)
    args = parser.parse_args()
    output_language = normalize_output_language(args.output_language)

    root = Path.cwd()
    ensure_trpg_binary(root)

    run_dir, session_id, character_name = create_run(
        root,
        args.label,
        output_language,
        persona=args.persona,
    )
    print(f"run_dir={run_dir}")
    print(f"session_id={session_id}")
    print(f"character={character_name}")
    print(f"output_language={output_language}")

    state = VisibleState(
        transcript=(run_dir / "transcript_player_visible.txt").read_text(encoding="utf-8"),
        last_reply=(run_dir / "opening.txt").read_text(encoding="utf-8"),
        turn=0,
        pc_name=character_name,
        output_language=output_language,
    )

    status = {"stop_reason": None, "turns": 0, "output_language": output_language}
    driver_failed = False
    try:
        while state.turn < args.max_turns:
            decision = choose_next_decision(state, args.persona)
            apply_attempt_marker(decision, state)
            print(f"\n=== turn {decision['turn']} ===")
            print(decision["sent_to_gm"])
            gm_text = run_turn(
                root,
                run_dir,
                session_id,
                decision,
                timeout=args.turn_timeout,
                output_language=output_language,
            )
            print(sentence_excerpt(gm_text, limit=900))

            state.turn += 1
            state.last_reply = gm_text
            state.transcript = (run_dir / "transcript_player_visible.txt").read_text(
                encoding="utf-8"
            )
            status["turns"] = state.turn
            reason = stop_reason(gm_text)
            if reason:
                status["stop_reason"] = reason
                break
    except Exception as exc:
        driver_failed = True
        status["stop_reason"] = "driver_error"
        status["error"] = str(exc)
        (run_dir / "driver_error.txt").write_text(str(exc) + "\n", encoding="utf-8")
    finally:
        run_coverage(root, run_dir, session_id)
        verdict = run_redboard(root, run_dir)
        status["redboard_verdict"] = verdict.get("verdict")
        status["verdict"] = "FAIL" if driver_failed else verdict.get("verdict")
        status["red_count"] = verdict.get("red_count")
        (run_dir / "run_summary.json").write_text(
            json.dumps(status, ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
        )
        write_battle_report(run_dir)
        print(f"\nsummary={json.dumps(status, ensure_ascii=False)}")
        print(f"battle_report={run_dir / 'battle_report_with_character.md'}")
        print(f"redboard={run_dir / 'redboard.md'}")

    return 0 if status.get("verdict") == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())

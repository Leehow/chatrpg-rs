#!/usr/bin/env python3
"""Offline J1-J4 + Q4 red-board evaluator for live run artifacts.

The evaluator is intentionally conservative: it does not query the DB and it
does not mark a judge green when the required artifact is absent.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shlex
import subprocess
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, Iterable, List, Tuple


CHECK_RESULT_CUES = (
    "成功",
    "失败",
    "大成功",
    "大失败",
    "部分成功",
    "success",
    "failure",
    "failed",
    "critical success",
    "critical failure",
)
CHECK_TARGET_CUES = ("目标", "target", "dv", "dc", "vs", "对抗", "≤", ">=", ">= ", "难度")
MENU_CUES = (
    "选择其一",
    "你现在可以",
    "下一步可以",
    "可以直接做其一",
    "可以立刻做的有",
    "几条路",
    "选哪一个",
    "你下一步选",
    "你接下来可以",
    "your next move could be",
    "you can choose where",
)
DUMP_CUES = ("关键事实", "你已经确认", "能确定的是", "现在能确认", "局面很明确")
INLINE_MENU_FRAMES = (
    "后续机会",
    "实际的后续机会",
    "实际机会",
    "你现在有几个",
    "你眼前有几个",
    "你面前有几个",
    "几个很近",
    "你把这些观察串在一起",
    "观察串在一起",
    "you connect these observations",
    "connect these observations",
)
INLINE_ACTION_VERBS = (
    "继续",
    "试着",
    "尝试",
    "冒险",
    "顺着",
    "确认",
    "切断",
    "断线",
    "撬开",
    "靠近",
    "贴",
    "冲",
    "开火",
    "射击",
    "喊话",
    "撤",
    "进入",
    "压制",
    "破坏",
    "转向",
    "拖拽",
    "缴械",
    "发号施令",
    "观察",
    "控制",
    "追",
)
PROGRESS_EVENTS = (
    "WorldFactChanged",
    "Knowledge",
    "Memory",
    "Parameter",
    "Effect",
    "ObjectiveResolved",
    "SceneTransitioned",
)
PLAYER_DECISION_KINDS = ("PLAYER_DECISION", "PLAYER_DECISION_TRACE")
RUBRIC_WEIGHTS = (
    ("rules_resolution", "规则与结算完整性", 25),
    ("continuity", "前后文和世界连续性", 20),
    ("responsiveness", "对玩家行动的响应性", 20),
    ("narrative_agency", "剧情推进和玩家能动性", 15),
    ("player_simulation", "玩家模拟真实性", 15),
    ("language", "语言表现", 5),
)
SEMANTIC_BLOCKING_SEVERITIES = ("S0", "S1", "S2", "CRITICAL", "ERROR", "FAIL")
SEMANTIC_Q4_CATEGORIES = (
    "ACTION_MENU",
    "ACTION_MENU_OR_EXPLICIT_OPTIONS",
    "EXPLICIT_OPTIONS",
    "OPTION_MENU",
    "RAW_CONTENT_DUMP",
    "GM_AUTHORED_ACTION_BRANCHES",
)
SEMANTIC_P1_CATEGORIES = (
    "PLAYER_UNRESOLVED_GATE",
    "PLAYER_FALLBACK_AFTER_CONCRETE_AFFORDANCE",
    "PLAYER_SCRIPT_LOOP",
    "PLAYER_HIDDEN_KNOWLEDGE",
    "PLAYER_IGNORES_VISIBLE_RESULT",
    "PLAYER_STATE_CONTRADICTION",
    "PLAYER_BELIEF_INCONSISTENCY",
)


def read_jsonl(path: Path) -> Iterable[Dict[str, Any]]:
    if not path.exists():
        return []
    rows: List[Dict[str, Any]] = []
    for line in path.read_text(errors="replace").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict):
            rows.append(value)
    return rows


def turn_file_number(path: Path) -> int:
    m = re.match(r"^(?:t|turn_)(\d{2,4})\.jsonl$", path.name)
    return int(m.group(1)) if m else 0


def turn_files(run_dir: Path) -> List[Path]:
    pattern = re.compile(r"^(?:t|turn_)(\d{2,4})\.jsonl$")

    def key(path: Path) -> int:
        return turn_file_number(path)

    return sorted(
        [
            path
            for path in run_dir.glob("*.jsonl")
            if pattern.match(path.name)
        ],
        key=key,
    )


def visible_text_from_jsonl(path: Path) -> str:
    chunks: List[str] = []
    for row in read_jsonl(path):
        if row.get("event") == "delta":
            chunks.append(str(row.get("data", "")))
    return "".join(chunks)


def contains_word(text: str, word: str) -> bool:
    return re.search(rf"\b{re.escape(word.lower())}\b", text.lower()) is not None


def visible_cue_text(text: str) -> str:
    return re.sub(r"[*_`]+", "", text.lower())


def input_text_for_turn(path: Path) -> str:
    input_path = path.with_suffix(".input.txt")
    return input_path.read_text(errors="replace") if input_path.exists() else ""


def explain_text_for_turn(path: Path) -> str:
    candidates = [path.with_suffix(".explain")]
    m = re.match(r"^turn_(\d{2,4})\.jsonl$", path.name)
    if m:
        candidates.append(path.with_name(f"explain_{m.group(1)}.txt"))
    for explain in candidates:
        if explain.exists():
            return explain.read_text(errors="replace")
    return ""


def stderr_text_for_turn(path: Path) -> str:
    err = path.with_suffix(".err")
    return err.read_text(errors="replace") if err.exists() else ""


def count_event(text: str, event_name: str) -> int:
    return len(re.findall(rf"\b{re.escape(event_name)}\b", text))


def direct_scene_transition_count(path: Path) -> int:
    count = 0
    for row in read_jsonl(path):
        if row.get("event") == "scene_transition":
            count += 1
    return count


def roll_blocks(text: str) -> List[str]:
    return re.findall(r"\[roll\](.*?)\[/roll\]", text, flags=re.IGNORECASE | re.DOTALL)


def roll_block_surfaces_result(block: str) -> bool:
    lowered = block.lower()
    has_number = any(ch.isdigit() for ch in block)
    has_result = any(cue in lowered for cue in CHECK_RESULT_CUES)
    has_target = any(cue in lowered for cue in CHECK_TARGET_CUES)
    return has_number and has_result and has_target


def roll_block_success(block: str) -> bool:
    lowered = block.lower()
    return ("success" in lowered or "成功" in lowered) and not any(
        cue in lowered for cue in ("failure", "failed", "失败", "strong_failure")
    )


def roll_block_failure(block: str) -> bool:
    lowered = block.lower()
    return any(cue in lowered for cue in ("failure", "failed", "失败", "strong_failure"))


def text_without_roll_blocks(text: str) -> str:
    return re.sub(r"\[roll\].*?\[/roll\]", "", text, flags=re.IGNORECASE | re.DOTALL)


def machine_confirmation_without_player_facts(body: str) -> bool:
    lowered = body.lower()
    if not any(
        cue in lowered
        for cue in (
            "根据本回合已确认的结果",
            "confirmed result",
            "confirmed results",
            "check succeeds",
            "check succeeded",
        )
    ):
        return False

    meaningful_lines: List[str] = []
    for raw_line in body.splitlines():
        line = re.sub(r"^[\s·*•\-\u2022]+", "", raw_line).strip()
        line = re.sub(r"\s+", " ", line)
        if not line:
            continue
        if line in ("根据本回合已确认的结果：", "根据本回合已确认的结果:"):
            continue
        lowered_line = line.lower().strip(".。")
        if lowered_line in ("confirmed result", "confirmed results"):
            continue
        if re.fullmatch(r"(?:the\s+)?[a-z0-9_:/,'\"() \-\u2014]+ check succeed(?:s|ed)", lowered_line):
            continue
        meaningful_lines.append(line)

    return not meaningful_lines


def briefing_success_exposes_practical_information(body: str) -> bool:
    lowered = body.lower()
    if not any(
        cue in lowered
        for cue in (
            "knott",
            "landlord",
            "retainer",
            "macario",
            "commission",
            "委托",
            "诺特",
        )
    ):
        return False
    information_groups = 0
    if any(
        cue in lowered
        for cue in (
            "address",
            "usable lead",
            "old corbitt place",
            "corbitt place",
            "property",
            "房子",
            "地址",
        )
    ):
        information_groups += 1
    if any(
        cue in lowered
        for cue in (
            "key ring",
            "keys",
            "key",
            "lock",
            "house itself",
            "钥匙",
            "门锁",
        )
    ):
        information_groups += 1
    if any(
        cue in lowered
        for cue in (
            "municipal",
            "civil",
            "records",
            "archival",
            "newspaper",
            "library research",
            "court",
            "police",
            "paper first",
            "公开",
            "档案",
            "报纸",
            "法院",
            "警局",
        )
    ):
        information_groups += 1
    if any(
        cue in lowered
        for cue in (
            "not firsthand",
            "only what",
            "only honestly",
            "bad reputation",
            "illness",
            "collapse",
            "madness",
            "rumor",
            "传闻",
            "名声",
            "不是亲眼",
        )
    ):
        information_groups += 1
    return information_groups >= 2


def success_without_concrete_information(text: str) -> bool:
    if not any(roll_block_success(block) for block in roll_blocks(text)):
        return False
    body = text_without_roll_blocks(text)
    lowered = body.lower()
    if official_records_access_without_content(body):
        return True
    if machine_confirmation_without_player_facts(body):
        return True
    if briefing_success_exposes_practical_information(body):
        return False
    machine_specific_fact_cues = (
        "walter",
        "michael thomas",
        "chapel of contemplation",
        "沃尔特",
        "迈克尔",
        "托马斯",
        "沉思",
        "遗体",
        "body",
        "corpse",
        "remains",
        "buried",
        "burial",
        "basement",
        "cellar",
        "raid",
        "imprisoned",
        "escaped",
        "children",
        "police officers",
        "cult",
        "front door",
        "back door",
        "key fits",
    )
    body_compact = re.sub(r"\s+", " ", body).strip(" \t\r\n:：·-")
    if (
        any(
            cue in body_compact
            for cue in (
                "根据本回合已确认的结果",
                "confirmed result",
                "confirmed results",
            )
        )
        and len(body_compact) <= 96
        and not any(cue in lowered for cue in machine_specific_fact_cues)
    ):
        return True
    generic_success_cues = (
        "抓到了一串能继续往下追",
        "能够继续深入",
        "能够继续深挖",
        "可以继续追下去",
        "真正能通往下一步",
        "清晰、扎手的着力点",
        "着力点",
        "一个名字",
        "某个事故",
        "公开痕迹",
        "印刷记录",
        "公共档案里冒头",
        "公共档案里同样留有可追下去的指向",
        "一条已经从公共档案里冒头的实线",
        "实线",
        "usable lead",
        "solid lead",
        "concrete lead",
        "line of inquiry",
        "continue the trail",
        "can continue",
        "materials connect cleanly",
        "public legal records",
        "traceable line",
        "checked page by page",
        "checked page-by-page",
        "records that can be checked",
        "房产归属",
        "旧业主更替",
        "遗产执行",
        "公开法律记录",
        "顺利地浮出水面",
        "浮出水面",
        "正确的线头",
        "同一条可追索",
        "可逐页核对",
        "公开记录里留下过痕迹",
        "公开文书里有过落笔",
        "真正能通往事实",
        "可供追索的痕迹",
    )
    if not any(cue in lowered for cue in generic_success_cues):
        return False
    specific_fact_cues = (
        "walter",
        "michael thomas",
        "chapel of contemplation",
        "沃尔特",
        "迈克尔",
        "托马斯",
        "沉思",
        "礼拜堂",
        "牧师",
        "遗体",
        "body",
        "corpse",
        "remains",
        "buried",
        "burial",
        "basement",
        "cellar",
        "raid",
        "imprisoned",
        "escaped",
        "children",
        "police officers",
        "cult",
        "front door",
        "back door",
        "key fits",
    )
    if re.search(r"(?<!\d)(?:18|19|20)\d{2}(?!\d)", body):
        return False
    return not any(cue in lowered for cue in specific_fact_cues)


def failure_leaks_positive_hidden_lead(text: str) -> bool:
    if not any(roll_block_failure(block) for block in roll_blocks(text)):
        return False
    body = text_without_roll_blocks(text).lower()
    has_positive_direction = any(
        cue in body
        for cue in (
            "leads toward",
            "points toward",
            "points to",
            "suggests a lead",
            "gives you a lead",
            "reveals a lead",
            "浮出",
            "指向",
            "线索",
        )
    )
    if not has_positive_direction:
        return False
    has_protected_lead = any(
        cue in body
        for cue in (
            "executor",
            "church",
            "chapel",
            "closure date",
            "closed in 1912",
            "michael thomas",
            "reverend",
            "遗嘱执行人",
            "教堂",
            "礼拜堂",
            "关闭日期",
        )
    )
    if not has_protected_lead:
        return False
    has_negated_boundary = any(
        cue in body
        for cue in (
            "no named executor",
            "no executor",
            "no clean",
            "does not produce",
            "not produce",
            "not visible",
            "no specific",
            "没有",
            "未能",
            "不能",
        )
    )
    return not has_negated_boundary or "leads toward" in body or "points toward" in body


def source_limited_address_contradiction(text: str) -> bool:
    lowered = text.lower()
    claims_full_address = bool(
        re.search(
            r"\b(?:write|writes|wrote|written|copy|copies|copied|record|records|recorded|jot|jots|jotted)\b.{0,96}\bfull address\b",
            lowered,
        )
    ) or "full address of the old corbitt place" in lowered
    claims_written_address = bool(
        re.search(
            r"\b(?:write|writes|wrote|written|copy|copies|copied|jot|jots|jotted)\b.{0,96}\b(?:the address|address of)\b",
            lowered,
        )
    )
    claims_address_here = "the address is here" in lowered and "writing it down" in lowered
    says_no_literal = any(
        cue in lowered
        for cue in (
            "not a literal street-number line",
            "does not provide a literal street number",
            "usable address lead",
        )
    )
    return (claims_full_address or claims_written_address or claims_address_here) and says_no_literal


def official_records_access_without_content(text: str) -> bool:
    lowered = text.lower()
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
            "public office counters",
            "sealed or privileged material",
            "protected files",
            "privileged material",
            "lawful channels",
            "legal boundaries",
        )
    )
    if not has_official_source:
        return False
    if official_records_explicit_refusal_or_failure(lowered):
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


def official_records_explicit_refusal_or_failure(lowered: str) -> bool:
    explicit_refusal_cues = (
        "does not open the file room",
        "does not open a file room",
        "does not open the files",
        "does not produce an index card",
        "does not produce an index",
        "not for casual inspection",
        "come back with stronger standing",
        "refuses access",
        "refused access",
        "access is refused",
        "no access to the files",
        "no access to police files",
        "won't open the file",
        "will not open the file",
        "doesn't open the file",
        "不给你查阅",
        "不给查阅",
        "拒绝查阅",
        "拒绝访问",
        "拒绝打开",
        "不让查阅",
        "不让看档案",
    )
    if any(cue in lowered for cue in explicit_refusal_cues):
        return True
    failure_cues = ("→ failure", "result:failure", "result: failure", "结果:failure", "结果:失败", "失败")
    official_cues = ("court", "police", "records access", "法院", "警局", "警察局", "档案")
    return any(cue in lowered for cue in failure_cues) and any(
        cue in lowered for cue in official_cues
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


def scene_transition_without_visible_situation(text: str, scene_transitions: int) -> bool:
    if scene_transitions <= 0:
        return False
    return empty_visible_result_text(text)


def empty_visible_result_text(text: str) -> bool:
    lowered = text.strip().lower()
    if not lowered:
        return True
    empty_fact_cues = (
        "无新增可公开",
        "无新增玩家可见",
        "无新增可见",
        "no new public",
        "no new player-visible",
        "no additional public",
    )
    if any(cue in lowered for cue in empty_fact_cues) and any(
        cue in lowered for cue in ("fact", "facts", "事实", "mechanical")
    ):
        return True
    return False


def unresolved_mechanics(text: str) -> bool:
    lowered = text.lower()
    markers = (
        '"blocked":true',
        '"blocked": true',
        "blocked_missing_source",
        "missing_source_backed_parameters",
        "awaiting_binding",
        "no source-backed target",
        '"success":null',
        '"success": null',
    )
    return any(marker in lowered for marker in markers)


def blocked_missing_source(text: str) -> bool:
    lowered = text.lower()
    return '"blocked":true' in lowered or '"blocked": true' in lowered


def narrated_position_change(text: str) -> bool:
    cues = (
        "移动到",
        "转移到",
        "挪到",
        "挪开",
        "收进",
        "藏进",
        "藏住",
        "到达",
        "抵达",
        "来到",
        "紧贴着",
        "身位已经",
        "已经从先前",
        "门框旁的盲区",
        "doorframe's blind side",
        "doorframe blind side",
        "no longer stranded",
    )
    lowered = text.lower()
    return any(cue.lower() in lowered for cue in cues)


def committed_progress_or_state(explain: str, checks: int, scene_transitions: int) -> bool:
    if checks > 0 or scene_transitions > 0:
        return True
    progress_events = PROGRESS_EVENTS + (
        "WorldEvent",
        "world_event",
        "world_facts",
        "world_fact",
        "state_patches",
        "StateFrame",
        "SceneChanged",
        "PlayerLearnedFact",
        "KnowledgeEdge",
    )
    return any(event in explain for event in progress_events)


def normalized_prose_key(text: str) -> str:
    stripped = re.sub(r"\s+", "", text)
    stripped = re.sub(r"\[/?[a-zA-Z_]+[^\]]*\]", "", stripped)
    return stripped[:180]


def max_consecutive_false(values: List[bool]) -> int:
    best = run = 0
    for value in values:
        if value:
            run = 0
        else:
            run += 1
            best = max(best, run)
    return best


def max_consecutive_repeat(keys: List[str]) -> int:
    best = run = 0
    prev = None
    for key in keys:
        if key and key == prev:
            run += 1
        else:
            run = 1 if key else 0
        best = max(best, run)
        prev = key
    return best


def line_is_numbered_option(line: str) -> bool:
    return re.match(r"^\s*(?:\d+[.、)]|[-*•])\s+", line) is not None


def constitution_hits(text: str) -> List[str]:
    hits: List[str] = []
    lowered = text.lower()
    numbered = [line for line in text.splitlines() if line_is_numbered_option(line)]
    if len(numbered) >= 3:
        hits.append("numbered_or_bulleted_option_menu")
    for cue in MENU_CUES:
        if cue.lower() in lowered:
            hits.append(f"menu_cue:{cue}")
    prepared_menu = prepared_action_menu(text)
    if prepared_menu:
        hits.append("prepared_action_menu")
    if chinese_ordinal_direction_menu(text):
        hits.append("ordinal_direction_menu")
    if chinese_where_start_or_menu(text):
        hits.append("chinese_where_start_or_menu")
    if english_go_first_or_menu(text):
        hits.append("english_go_first_or_menu")
    if english_what_try_first_menu(text):
        hits.append("english_what_try_first_menu")
    if english_first_choice_alternative_menu(text):
        hits.append("english_first_choice_alternative_menu")
    if english_do_you_action_sequence_menu(text):
        hits.append("english_do_you_action_sequence_menu")
    if english_whether_you_or_menu(text):
        hits.append("english_whether_you_or_menu")
    if english_choice_of_whether_action_menu(text):
        hits.append("english_choice_of_whether_action_menu")
    if english_natural_directions_menu(text):
        hits.append("english_natural_directions_menu")
    if english_you_can_action_sequence_menu(text):
        hits.append("english_you_can_action_sequence_menu")
    if english_you_may_action_sequence_menu(text):
        hits.append("english_you_may_action_sequence_menu")
    if english_next_move_is_either_menu(text):
        hits.append("english_next_move_is_either_menu")
    if english_connect_observations_action_menu(text):
        hits.append("english_connect_observations_action_menu")
    if english_character_can_action_sequence_menu(text):
        hits.append("english_character_can_action_sequence_menu")
    if english_if_you_want_character_can_commit_probe_menu(text):
        hits.append("english_if_you_want_character_can_commit_probe_menu")
    if english_if_you_want_character_can_or_menu(text):
        hits.append("english_if_you_want_character_can_or_menu")
    if english_if_you_choose_you_can_or_menu(text):
        hits.append("english_if_you_choose_you_can_or_menu")
    if english_if_you_want_target_first_menu(text):
        hits.append("english_if_you_want_target_first_menu")
    if english_if_you_want_press_or_paper_trail_menu(text):
        hits.append("english_if_you_want_press_or_paper_trail_menu")
    if english_fragmented_paper_trail_direction_menu(text):
        hits.append("english_fragmented_paper_trail_direction_menu")
    if english_next_meaningful_or_open_enough_menu(text):
        hits.append("english_next_meaningful_or_open_enough_menu")
    if raw_module_clue_id_dump(text):
        hits.append("raw_dump:module_clue_id")
    if internal_adjudication_guidance_dump(text):
        hits.append("raw_dump_cue:internal_adjudication_guidance")
    if english_observation_field_manifest_dump(text):
        hits.append("raw_dump_cue:observation_field_manifest")
    if english_clear_choice_target_menu(text):
        hits.append("english_clear_choice_target_menu")
    if english_begin_or_live_direction_target_menu(text):
        hits.append("english_begin_or_live_direction_target_menu")
    if english_branching_target_menu(text):
        hits.append("english_branching_target_menu")
    if english_obvious_lines_pursue_first_menu(text):
        hits.append("english_obvious_lines_pursue_first_menu")
    if english_will_you_start_or_more_menu(text):
        hits.append("english_will_you_start_or_more_menu")
    if english_next_move_can_follow_or_take_menu(text):
        hits.append("english_next_move_can_follow_or_take_menu")
    if english_pursue_either_lead_menu(text):
        hits.append("english_pursue_either_lead_menu")
    if english_do_you_have_character_or_menu(text):
        hits.append("english_do_you_have_character_or_menu")
    if english_tone_choice_menu(text):
        hits.append("english_tone_choice_menu")
    if english_what_now_inline_action_menu(text):
        hits.append("english_what_now_inline_action_menu")
    if english_what_do_first_action_menu(text):
        hits.append("english_what_do_first_action_menu")
    if unclosed_dialogue_quote(text):
        hits.append("truncated_or_unclosed_dialogue")
    bullet_lines = [line for line in text.splitlines() if re.match(r"^\s*[-*•]\s+", line)]
    if len(bullet_lines) >= 2:
        for cue in DUMP_CUES:
            if cue in text:
                hits.append(f"raw_dump_cue:{cue}")
                break
    if not prepared_menu and inline_action_menu(text):
        hits.append("prose_action_menu")
    return hits


def raw_module_clue_id_dump(text: str) -> bool:
    lowered = text.lower()
    return (
        "已揭示的模组线索" in text
        or "module clue" in lowered
        or bool(re.search(r"\bhandout[_-]\d+\b", lowered))
    )


def internal_adjudication_guidance_dump(text: str) -> bool:
    cues = (
        "这次结算应",
        "当前可公开承认的范围",
        "不能当成已发现事实",
        "不能在未结算前当成事实",
        "本回合应锚定",
        "应锚定刚才",
    )
    return any(cue in text for cue in cues)


def inline_action_menu(text: str) -> bool:
    if prepared_action_menu(text):
        return True
    if prose_branch_action_menu(text):
        return True
    if either_or_action_menu(text):
        return True
    if english_where_next_menu(text):
        return True
    if english_what_next_menu(text):
        return True
    if english_what_now_inline_action_menu(text):
        return True
    if english_what_do_first_action_menu(text):
        return True
    if english_first_choice_alternative_menu(text):
        return True
    if english_do_you_action_sequence_menu(text):
        return True
    if english_you_can_action_sequence_menu(text):
        return True
    if english_you_may_action_sequence_menu(text):
        return True
    if english_next_move_is_either_menu(text):
        return True
    if english_connect_observations_action_menu(text):
        return True
    if english_character_can_action_sequence_menu(text):
        return True
    if english_if_you_want_character_can_commit_probe_menu(text):
        return True
    if english_if_you_want_character_can_or_menu(text):
        return True
    if english_if_you_choose_you_can_or_menu(text):
        return True
    if english_if_you_want_target_first_menu(text):
        return True
    if english_if_you_want_press_or_paper_trail_menu(text):
        return True
    if english_next_meaningful_or_open_enough_menu(text):
        return True
    if english_choice_of_whether_action_menu(text):
        return True
    if english_clear_choice_target_menu(text):
        return True
    if english_begin_or_live_direction_target_menu(text):
        return True
    if english_next_move_can_follow_or_take_menu(text):
        return True
    if english_pursue_either_lead_menu(text):
        return True
    if english_do_you_have_character_or_menu(text):
        return True
    lowered = text.lower()
    if not any(frame.lower() in lowered for frame in INLINE_MENU_FRAMES):
        return False
    if "：" not in text and ":" not in text:
        return False
    branch_count = (
        text.count("；")
        + text.count(";")
        + text.count("或者")
        + text.count("或是")
        + len(re.findall(r"\bor\b", lowered))
    )
    if branch_count < 2:
        return False
    return inline_action_branch_count(text) >= 2


def prepared_action_menu(text: str) -> bool:
    if "接下来你是准备" not in text and "接下来你准备" not in text:
        return False
    tail = text
    for cue in ("接下来你是准备", "接下来你准备"):
        if cue in tail:
            tail = tail.split(cue, 1)[1]
            break
    if not any(connector in tail for connector in ("还是", "或者", "或是")):
        return False
    branch_count = (
        tail.count("还是")
        + tail.count("或者")
        + tail.count("或是")
        + tail.count("；")
        + tail.count(";")
    )
    parts = re.split(r"还是|或者|或是|；|;", tail)
    action_branches = sum(1 for part in parts if branch_looks_like_player_action(part))
    return branch_count >= 2 and action_branches >= 2


def chinese_ordinal_direction_menu(text: str) -> bool:
    if "几条" not in text or not any(cue in text for cue in ("去处", "方向", "路径")):
        return False
    ordinal_hits = sum(1 for cue in ("一是", "二是", "三是", "四是") if cue in text)
    if ordinal_hits < 2:
        return False
    action_hits = sum(
        1
        for cue in (
            "继续",
            "顺着",
            "转去",
            "带着",
            "直接去",
            "查",
            "看",
            "追",
            "前往",
        )
        if cue in text
    )
    return action_hits >= 2


def chinese_where_start_or_menu(text: str) -> bool:
    return (
        "还是" in text
        and any(cue in text for cue in ("你想先", "你先", "先从"))
        and any(cue in text for cue in ("下手", "开始", "着手"))
    )


def english_go_first_or_menu(text: str) -> bool:
    lowered = text.lower()
    return (
        "do you go first" in lowered
        and " or " in lowered
        and any(cue in lowered for cue in ("records", "paper", "office"))
    )


def english_what_try_first_menu(text: str) -> bool:
    lowered = text.lower()
    return (
        (
            "what do you want to try first" in lowered
            or "which will you try first" in lowered
            or "which do you try first" in lowered
            or "which would you try first" in lowered
        )
        and " or " in lowered
        and any(cue in lowered for cue in ("records", "archives", "paper"))
    )


def english_first_choice_alternative_menu(text: str) -> bool:
    lowered = text.lower()
    for sentence in re.split(r"[.!?\n]", lowered):
        sentence = sentence.strip()
        if not sentence or "first" not in sentence:
            continue
        if " or " not in sentence and "—or " not in sentence and "-or " not in sentence:
            continue
        if not any(cue in sentence for cue in ("which ", "what ", "where ")):
            continue
        has_branch_surface = (
            ":" in sentence
            or "-" in sentence
            or "—" in sentence
            or "/" in sentence
            or sentence.count(",") >= 1
        )
        if not has_branch_surface:
            continue
        has_action_or_route_verb = any(
            cue in sentence
            for cue in (
                "try",
                "pursue",
                "follow",
                "begin",
                "start",
                "go",
                "check",
                "search",
                "investigate",
                "look into",
                "visit",
                "enter",
                "open",
                "examine",
                "inspect",
                "focus",
                "choose",
                "take",
            )
        )
        if has_action_or_route_verb or any(
            cue in sentence
            for cue in ("which lead", "which line", "what line", "where does", "where do")
        ):
            return True
    return False


def english_do_you_action_sequence_menu(text: str) -> bool:
    lowered = text.lower()
    for sentence in re.split(r"[.!?]", lowered):
        sentence = sentence.strip()
        if not sentence.startswith("do you "):
            continue
        if " or " not in sentence or sentence.count(",") < 1:
            continue
        tail = sentence.split("do you ", 1)[1].strip()
        if english_action_sequence_hit_count(tail) >= 2:
            return True
    return False


def english_whether_you_or_menu(text: str) -> bool:
    lowered = text.lower()
    if "whether you " not in lowered or " or " not in lowered:
        return False
    tail = lowered.split("whether you ", 1)[1]
    if " or " not in tail:
        return False
    left, right = tail.split(" or ", 1)
    action_cues = english_action_cues()
    return any(cue in left for cue in action_cues) and any(
        cue in right for cue in action_cues
    )


def english_choice_of_whether_action_menu(text: str) -> bool:
    lowered = text.lower()
    if "choice of whether to " not in lowered or " or " not in lowered:
        return False
    tail = lowered.split("choice of whether to ", 1)[1]
    sentence = re.split(r"[.!?]", tail, maxsplit=1)[0].strip()
    if sentence.count(",") < 1:
        return False
    return english_action_sequence_hit_count(sentence) >= 2


def english_natural_directions_menu(text: str) -> bool:
    lowered = text.lower()
    if "direction" not in lowered:
        return False
    if not any(
        cue in lowered
        for cue in (
            "can naturally",
            "naturally press",
            "few directions",
            "natural directions",
        )
    ):
        return False
    if ":" not in lowered or (" or " not in lowered and "," not in lowered):
        return False
    return sum(1 for cue in english_action_cues() if cue in lowered) >= 2


def english_you_can_action_sequence_menu(text: str) -> bool:
    lowered = text.lower()
    tail = None
    for cue in ("you can ", "you could "):
        if cue in lowered:
            tail = lowered.split(cue, 1)[1]
            break
    if tail is None:
        return False
    sentence = re.split(r"[.!?]", tail, maxsplit=1)[0].strip()
    if " or " not in sentence or sentence.count(",") < 1:
        return False
    return english_action_sequence_hit_count(sentence) >= 2


def english_you_may_action_sequence_menu(text: str) -> bool:
    lowered = text.lower()
    for sentence in re.split(r"[.!?]", lowered):
        if "you may " not in sentence:
            continue
        tail = sentence.split("you may ", 1)[1].strip()
        if " or " not in tail or tail.count(",") < 1:
            continue
        if english_action_sequence_hit_count(tail) >= 2:
            return True
    return False


def english_next_move_is_either_menu(text: str) -> bool:
    lowered = text.lower()
    for sentence in re.split(r"[.!?]", lowered):
        if "next move is either" not in sentence:
            continue
        if " or " not in sentence:
            continue
        return True
    return False


def english_connect_observations_action_menu(text: str) -> bool:
    lowered = text.lower()
    if "you connect these observations" not in lowered:
        return False
    tail = lowered.split("you connect these observations", 1)[1]
    if ":" in tail:
        tail = tail.split(":", 1)[1]
    sentence = re.split(r"[.!?]", tail, maxsplit=1)[0].strip()
    if " or " not in sentence:
        return False
    if sentence.count(";") < 1 and sentence.count(",") < 1:
        return False
    return english_action_sequence_hit_count(sentence) >= 2


def english_character_can_action_sequence_menu(text: str) -> bool:
    lowered = text.lower()
    for sentence in re.split(r"[.!?]", lowered):
        if " can " not in sentence or " or " not in sentence or sentence.count(",") < 1:
            continue
        tail = sentence.split(" can ", 1)[1].strip()
        if english_action_sequence_hit_count(tail) >= 2:
            return True
    return False


def english_if_you_want_character_can_commit_probe_menu(text: str) -> bool:
    lowered = text.lower()
    if "if you want" not in lowered:
        return False
    if " can " not in lowered or "commit" not in lowered:
        return False
    if not any(cue in lowered for cue in ("specific next probe", "next probe", "attention to first")):
        return False
    if " or " not in lowered and "—or " not in lowered:
        return False
    optionish = sum(1 for cue in ("bed", "wardrobe", "paper", "window", "door", "room") if cue in lowered)
    return optionish >= 3


def english_if_you_want_character_can_or_menu(text: str) -> bool:
    lowered = text.lower()
    for sentence in re.split(r"[.!?]", lowered):
        if not any(
            cue in sentence
            for cue in ("if you want", "if she wants", "if he wants", "if they want")
        ) or " can " not in sentence:
            continue
        if " or " not in sentence and "—or " not in sentence:
            continue
        tail = sentence.split(" can ", 1)[1]
        action_cues = english_action_cues() + ("hold", "edge", "approach", "remain", "step")
        action_hits = sum(
            1
            for cue in action_cues
            if tail.startswith(cue)
            or f" {cue} " in tail
            or f" {cue}ing " in tail
        )
        if action_hits >= 2:
            return True
    return False


def english_if_you_choose_you_can_or_menu(text: str) -> bool:
    lowered = text.lower()
    if not any(
        cue in lowered
        for cue in (
            "if you choose",
            "if she chooses",
            "if he chooses",
            "if they choose",
        )
    ):
        return False
    if not any(
        cue in lowered
        for cue in (
            " or you can ",
            " or she can ",
            " or he can ",
            " or they can ",
            "—or you can ",
            "—or she can ",
            "—or he can ",
            "—or they can ",
        )
    ):
        return False
    first_can = re.search(r"\b(?:you|she|he|they)\s+can\s+(?P<tail>.+)", lowered, re.S)
    if not first_can:
        return False
    tail = first_can.group("tail")
    parts = re.split(r"\bor\s+(?:you|she|he|they)\s+can\s+", tail, maxsplit=1)
    if len(parts) != 2:
        return False
    first, second = parts
    actionish = english_action_cues() + ("approach", "hold", "remain", "stay")
    return all(any(re.search(rf"\b{re.escape(cue)}(?:\b|ing\b)", part) for cue in actionish) for part in (first, second))


def english_if_you_want_target_first_menu(text: str) -> bool:
    lowered = text.lower()
    if "if you want" not in lowered:
        return False
    if " or " not in lowered and "—or " not in lowered:
        return False
    if not any(
        cue in lowered
        for cue in (
            "target first",
            "one specific target",
            "same doorway method",
            "same threshold method",
            "pressing this same",
        )
    ):
        return False
    optionish = sum(1 for cue in ("bed", "wardrobe", "paper", "papers", "window", "doorway", "withdraw") if cue in lowered)
    return optionish >= 3


def english_if_you_want_press_or_paper_trail_menu(text: str) -> bool:
    lowered = text.lower()
    return (
        "if you want" in lowered
        and "press him further" in lowered
        and (" or you can " in lowered or "—or you can " in lowered)
        and ("paper trail" in lowered or "records" in lowered)
    )


def english_next_meaningful_or_open_enough_menu(text: str) -> bool:
    lowered = text.lower()
    action_cues = (
        "commit",
        "focus",
        "hold",
        "shift",
        "remain",
        "study",
        "canvass",
        "listen",
        "widen",
        "make",
        "step",
        "enter",
        "go",
    )
    for sentence in re.split(r"[.!?]", lowered):
        has_connector = " or " in sentence or "—or " in sentence
        if not has_connector:
            continue
        if "next meaningful move" in sentence and ":" in sentence:
            tail = sentence.split("next meaningful move", 1)[1]
            hits = sum(1 for cue in action_cues if re.search(rf"\b{re.escape(cue)}\b", tail))
            if hits >= 2:
                return True
        if "open enough now to" in sentence:
            tail = sentence.split("open enough now to", 1)[1]
            hits = sum(1 for cue in action_cues if re.search(rf"\b{re.escape(cue)}\b", tail))
            if hits >= 2:
                return True
    return False


def english_observation_field_manifest_dump(text: str) -> bool:
    lowered = text.lower()
    has_manifest_header = (
        "you connect these observations" in lowered
        or "taken together" in lowered
        or "what it gives" in lowered
        or "single most concrete visible affordance" in lowered
        or "concrete affordance" in lowered
    )
    field_hits = sum(
        1
        for cue in (
            "it is at",
            "look:",
            "location:",
            "sight:",
            "sound:",
            "smell:",
            "looks like:",
            "sounds like:",
            "smells like:",
            "it sounds like",
            "it smells like",
            "reachability:",
            "safely reachable:",
            "what it looks like:",
            "what it sounds like:",
            "what it smells like:",
        )
        if cue in lowered
    )
    return has_manifest_header and field_hits >= 2


def english_action_sequence_hit_count(sentence: str) -> int:
    action_hits = 0
    for cue in english_action_cues():
        if (
            sentence.startswith(cue)
            or f" {cue} " in sentence
            or f" {cue}ing " in sentence
        ):
            action_hits += 1
    return action_hits


def english_clear_choice_target_menu(text: str) -> bool:
    lowered = text.lower()
    if "clear choice" not in lowered:
        return False
    if not any(cue in lowered for cue in ("where to", "what to", "which ")):
        return False
    tail = lowered.split("clear choice", 1)[1]
    sentence = re.split(r"[.!?]", tail, maxsplit=1)[0].strip()
    return ":" in sentence and " or " in sentence and ("," in sentence or ";" in sentence)


def english_begin_or_live_direction_target_menu(text: str) -> bool:
    lowered = text.lower()
    frame = any(
        cue in lowered
        for cue in (
            "where do you mean to begin",
            "where do you begin",
            "what do you pursue next",
            "live directions",
            "point in three directions",
            "point in three live directions",
            "point in three promising directions",
            "points in three directions",
            "points in three live directions",
            "points in three promising directions",
            "points in three clear directions",
            "three immediate lines of pursuit",
            "immediate lines of pursuit",
            "lines of pursuit",
        )
    )
    if not frame or " or " not in lowered:
        return False
    if ":" in lowered or lowered.count(",") >= 2:
        return True
    has_target_list = lowered.count(";") >= 1 and (
        "what does " in lowered or "what do " in lowered
    ) and " next" in lowered
    return has_target_list


def english_next_move_can_follow_or_take_menu(text: str) -> bool:
    lowered = text.lower()
    for sentence in re.split(r"[.!?]", lowered):
        if "next move can " not in sentence:
            continue
        if " or " not in sentence and "—or " not in sentence:
            continue
        tail = sentence.split("next move can ", 1)[1]
        hits = sum(
            1
            for cue in ("follow", "take", "return", "go", "keep", "continue", "head")
            if re.search(rf"\b{re.escape(cue)}\b", tail)
        )
        if hits >= 2:
            return True
    return False


def english_pursue_either_lead_menu(text: str) -> bool:
    lowered = text.lower()
    if "pursue either lead" not in lowered:
        return False
    tail = lowered.split("pursue either lead", 1)[1]
    return " or " in tail or "—or " in tail or ":" in tail


def english_fragmented_paper_trail_direction_menu(text: str) -> bool:
    lowered = text.lower()
    has_paper_frame = any(
        cue in lowered
        for cue in (
            "paper trail",
            "public books",
            "public filings",
            "hall of records",
            "legal records",
            "public records",
            "public-index",
        )
    )
    has_court_target = any(
        cue in lowered
        for cue in ("higher courts", "serious legal records", "the courts", "court records", "courts")
    )
    has_police_target = any(
        cue in lowered
        for cue in ("central police station", "police records", "police files", "the police")
    )
    has_site_target = any(
        cue in lowered
        for cue in ("corbitt house", "house itself", "chapel of contemplation", "the chapel")
    )
    has_direction_targets = has_court_target and has_police_target and has_site_target
    has_fragment = (
        "or leave the paper trail" in lowered
        or "you can follow this by turning next toward" in lowered
        or "turning next toward" in lowered
        or "most promising next channels" in lowered
        or "next channels appear" in lowered
        or "appear to be the courts" in lowered
    )
    return has_paper_frame and has_direction_targets and has_fragment


def english_do_you_have_character_or_menu(text: str) -> bool:
    lowered = text.lower()
    if not re.search(r"\bdo you have [a-z][a-z\"'“”‘’.\- ]{0,40}\b", lowered):
        return False
    sentence = re.split(r"[.!?]", lowered, maxsplit=1)[0]
    if " or " not in sentence and "—or " not in sentence:
        return False
    action_cues = ("pry", "hold", "examine", "open", "study", "look", "inspect", "move")
    hits = sum(
        1
        for cue in action_cues
        if re.search(rf"\b{re.escape(cue)}\b", sentence)
    )
    return hits >= 2 or english_action_sequence_hit_count(sentence) >= 2


def english_tone_choice_menu(text: str) -> bool:
    lowered = text.lower()
    has_frame = any(
        cue in lowered
        for cue in (
            "what tone does",
            "what tone do",
            "how does she press",
            "how does he press",
            "how do you press",
            "how does the character press",
        )
    )
    approach_frame = re.search(r"\bhow\s+(?:does|do|will)\b.{0,80}\bapproach\b", lowered) is not None
    skill_hits = sum(
        1
        for cue in (
            "charm",
            "persuasion",
            "persuade",
            "intimidation",
            "intimidate",
            "fast talk",
            "quick bluff",
            "appeal",
            "argument",
            "professional",
            "credentials",
            "reasonable request",
            "professional courtesy",
            "polite persuasion",
            "presses politely",
            "flatters",
            "bluffs",
            "bluff",
            "urgency",
            "press credentials",
            "fast-talking",
            "pressure",
            "blunt pressure",
            "bully",
            "courtesy",
        )
        if cue in lowered
    )
    if approach_frame and re.search(r"(?:\bby|—by|-by)\b.{0,120}\bor\b", lowered) and skill_hits >= 3:
        return True
    if (
        approach_frame
        and re.search(r"\bif\s+(?:she|he|they|you)\b", lowered)
        and re.search(r"\bor\s+(?:tries?\s+to\s+)?\w+", lowered)
        and skill_hits >= 3
    ):
        return True
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
    if not has_frame:
        return False
    as_branches = len(re.findall(r"(?:^|[?.!;:]\s+|,\s+|\bor\s+)as\s+(?:a\s+|an\s+)?", lowered))
    if as_branches < 3:
        return False
    return any(cue in lowered for cue in (" or as ", "tone", "approach", "appeal", "argument"))


def unclosed_dialogue_quote(text: str) -> bool:
    stripped = text.strip()
    if not stripped:
        return False
    return stripped.count("“") > stripped.count("”") or stripped.count("‘") > stripped.count("’")


def english_branching_target_menu(text: str) -> bool:
    lowered = text.lower()
    frame = any(
        cue in lowered
        for cue in (
            "obvious next avenues",
            "next avenues are",
            "obvious next lines of inquiry",
            "next solid leads",
            "line of inquiry clearly branches outward",
            "line of inquiry branches outward",
            "branches outward",
            "branches into",
            "paper trail might continue",
            "if you want to press it further",
            "obvious directions suggest themselves",
            "two obvious directions",
            "next useful angle is",
            "next pressure points",
            "for example, focusing on",
            "follow this outward from here",
            "follow this outward",
            "choice of pressure points",
            "clearer choice of pressure points",
            "offers more avenues",
        )
    )
    if not frame:
        return False
    has_connector = (
        " or " in lowered
        or "though you could" in lowered
        or ("next pressure points" in lowered and "follow first" in lowered)
        or ("obvious next avenues" in lowered and " and " in lowered)
        or ("obvious next lines of inquiry" in lowered and (" and " in lowered or "," in lowered))
        or ("two obvious directions" in lowered and " and " in lowered)
    )
    if not has_connector:
        return False
    if "obvious next avenues" in lowered and " and " in lowered:
        return True
    if "two obvious directions" in lowered and " and " in lowered:
        return True
    if "next pressure points" in lowered and "follow first" in lowered:
        return True
    if "obvious next lines of inquiry" in lowered and (" and " in lowered or "," in lowered):
        return True
    if "next solid leads" in lowered and " or " in lowered:
        return True
    return ":" in lowered or lowered.count(",") >= 1 or lowered.count(";") >= 1


def english_obvious_lines_pursue_first_menu(text: str) -> bool:
    lowered = text.lower()
    if "obvious lines of inquiry" not in lowered:
        return False
    if not re.search(r"\bwhat does .{1,48}? pursue first\?", lowered):
        return False
    return ":" in lowered or lowered.count(",") >= 2 or lowered.count(";") >= 1


def english_will_you_start_or_more_menu(text: str) -> bool:
    lowered = text.lower()
    if not ("will you start" in lowered or "will you begin" in lowered):
        return False
    if " or " not in lowered:
        return False
    return any(
        cue in lowered
        for cue in (
            "something more",
            "before you go",
            "from me here",
            "start with the records",
            "begin with the records",
            "the newspapers",
            "neighborhood",
        )
    )


def english_where_next_menu(text: str) -> bool:
    lowered = text.lower()
    if not ("where does " in lowered or "where do " in lowered):
        return False
    if " next" not in lowered:
        return False
    if ":" in lowered:
        tail = lowered.split(":", 1)[1]
    else:
        tail = lowered.split("?", 1)[1] if "?" in lowered else lowered
    if " or " not in tail:
        return False
    return tail.count(",") >= 1 or ":" in tail or sum(
        1 for cue in english_action_cues() if cue in tail
    ) >= 2


def english_what_next_menu(text: str) -> bool:
    lowered = text.lower()
    if not ("what does " in lowered or "what do " in lowered):
        return False
    if " next" not in lowered:
        return False
    tail = lowered.split(" next", 1)[1]
    return " or " in tail and (
        "," in tail or "—" in tail or ":" in tail or "?" in tail
    )


def english_what_do_first_action_menu(text: str) -> bool:
    lowered = text.lower()
    if not re.search(r"\bwhat (?:does|do)\b[^?]{0,90}\bdo first\b", lowered):
        return False
    tail = re.split(r"\bdo first\b", lowered, maxsplit=1)[1]
    tail = tail.split("?", 1)[0]
    if " or " not in tail and "—or " not in tail:
        return False
    if "," not in tail and ";" not in tail and "—" not in tail and ":" not in tail:
        return False
    return english_action_sequence_hit_count(tail) >= 2


def english_what_now_inline_action_menu(text: str) -> bool:
    lowered = text.lower()
    if not re.search(r"\bwhat (?:does|do)\b[^?]{0,80}\bdo now\?", lowered):
        return False
    tail = lowered.split("do now?", 1)[1].strip()
    if " or " not in tail and "—or " not in tail:
        return False
    sentence = re.split(r"[.!?]", tail, maxsplit=1)[0].strip()
    if not sentence:
        return False
    if "," in sentence or ";" in sentence or ":" in sentence or "—" in sentence:
        return True
    return english_action_sequence_hit_count(sentence) >= 2


def english_action_cues() -> Tuple[str, ...]:
    return (
        "ask",
        "change",
        "check",
        "continue",
        "dig",
        "enter",
        "examine",
        "examining",
        "follow",
        "go",
        "head",
        "inspect",
        "investigate",
        "leave",
        "look",
        "move",
        "open",
        "press",
        "probe",
        "pull",
        "push",
        "question",
        "search",
        "shift",
        "start",
        "turn",
        "try",
        "withdraw",
        "work",
    )


def either_or_action_menu(text: str) -> bool:
    has_frame = text.count("要么") >= 2 or (
        "要么" in text and ("或者" in text or "或是" in text)
    )
    if not has_frame:
        return False
    parts = []
    for chunk in text.split("要么"):
        for sub in re.split(r"或者|或是", chunk):
            parts.append(sub)
    return sum(1 for part in parts if branch_looks_like_player_action(part)) >= 2


def inline_action_branch_count(text: str) -> int:
    tail = text
    if "：" in tail:
        tail = tail.split("：", 1)[1]
    elif ":" in tail:
        tail = tail.split(":", 1)[1]
    parts = re.split(r"；|;|或者|或是|\bor\b", tail)
    return sum(1 for part in parts if branch_looks_like_player_action(part))


def branch_looks_like_player_action(part: str) -> bool:
    s = part.strip(" \t\r\n，。；:：、")
    if not s:
        return False
    if "，" in s:
        head, rest = s.split("，", 1)
        if len(head) <= 16 and branch_looks_like_player_action(rest):
            return True
    starts = (
        "先",
        "再",
        "继续",
        "有人",
        "下车",
        "试着",
        "尝试",
        "冒险",
        "顺着",
        "确认",
        "切断",
        "断线",
        "撬开",
        "靠近",
        "贴",
        "冲",
        "转向",
        "追",
        "开火",
        "射击",
        "喊话",
        "撤",
        "进入",
        "压制",
        "破坏",
        "扑近",
        "沿墙",
        "摸到",
        "想办法",
    )
    if s.startswith(starts):
        return True
    lowered = s.lower()
    for prefix in ("or ", "and "):
        if lowered.startswith(prefix):
            lowered = lowered[len(prefix) :].lstrip()
            break
    if any(
        lowered.startswith(f"{cue} ") or lowered.startswith(f"{cue}ing ")
        for cue in english_action_cues()
    ):
        return True
    return s.startswith("对") and any(v in s for v in ("喊话", "发号施令", "开火", "射击"))


def prose_branch_action_menu(text: str) -> bool:
    if not any(cue in text for cue in ("无论你是想", "无论你想", "无论你是要", "无论你要")):
        return False
    if "还是" not in text:
        return False
    branch_count = (
        text.count("、")
        + text.count("，")
        + text.count("；")
        + text.count("或者")
        + text.count("或是")
        + text.count("还是")
    )
    if branch_count < 3:
        return False
    verb_hits = sum(1 for verb in INLINE_ACTION_VERBS if verb in text)
    return verb_hits >= 3


def presentation_gate_hits(explain: str, stderr: str, rows: Iterable[Dict[str, Any]]) -> List[str]:
    hits: List[str] = []
    gate_states = re.findall(r"presentation_gate[^\n]*\(gate=(Allow|Block)", explain)
    if gate_states and gate_states[-1] == "Block":
        hits.append("presentation_gate_block")
    if "turn warning at phase presentation_gate" in stderr and "gate=Block" in stderr:
        hits.append("presentation_gate_block")
    for row in rows:
        if row.get("event") != "phase" or row.get("phase") != "warning":
            continue
        data = row.get("data")
        message = data.get("message", "") if isinstance(data, dict) else ""
        phase = data.get("phase", "") if isinstance(data, dict) else ""
        if phase == "presentation_gate" and "gate=Block" in str(message):
            hits.append("presentation_gate_block")
    return hits


def extract_session_id(run_dir: Path) -> str:
    for name in ("session_id", "session_id.txt"):
        path = run_dir / name
        if path.exists():
            value = path.read_text(errors="replace").strip()
            if value:
                return value
    log = run_dir / "run.log"
    if log.exists():
        m = re.search(r"session=([A-Za-z0-9_.:-]+)", log.read_text(errors="replace"))
        if m:
            return m.group(1)
    return "unknown"


def read_player_sim_signals(run_dir: Path) -> Tuple[bool, List[Dict[str, Any]]]:
    path = run_dir / "signals.jsonl"
    if path.exists():
        return True, list(read_jsonl(path))
    decisions_path = run_dir / "player_decisions.jsonl"
    if not decisions_path.exists():
        return False, []
    return True, [
        normalize_player_decision_signal(row)
        for row in read_jsonl(decisions_path)
    ]


def normalize_player_decision_signal(row: Dict[str, Any]) -> Dict[str, Any]:
    out = dict(row)
    out.setdefault("kind", "PLAYER_DECISION")
    if "selection_rationale" not in out and "selected_action_reason" in out:
        out["selection_rationale"] = out["selected_action_reason"]
    if "declared_action" not in out and "sent_to_gm" in out:
        out["declared_action"] = out["sent_to_gm"]
    contract = out.get("response_contract")
    if not isinstance(contract, dict):
        contract = out.get("expected_response_contract")
    if isinstance(contract, dict):
        normalized_contract = dict(contract)
        if (
            "acceptable_resolutions" not in normalized_contract
            and "must_resolve_one_of" in normalized_contract
        ):
            normalized_contract["acceptable_resolutions"] = normalized_contract[
                "must_resolve_one_of"
            ]
        out["response_contract"] = normalized_contract
    return out


def player_decision_signals(signals: Iterable[Dict[str, Any]]) -> List[Dict[str, Any]]:
    return [
        signal
        for signal in signals
        if str(signal.get("kind", "")).upper() in PLAYER_DECISION_KINDS
    ]


def truncate_for_semantic_payload(text: Any, limit: int = 5000) -> str:
    value = str(text or "")
    return value if len(value) <= limit else value[:limit] + "\n...[truncated]"


def semantic_critic_payload(
    run_dir: Path,
    turns: List[Dict[str, Any]],
    decisions: List[Dict[str, Any]],
) -> Dict[str, Any]:
    return {
        "schema": "trpg_semantic_critic_v1",
        "session": extract_session_id(run_dir),
        "rubric": {
            "principle": "Use semantic judgment. Do not rely on exact wording or language-specific phrase matching.",
            "hard_fail_categories": list(SEMANTIC_Q4_CATEGORIES + SEMANTIC_P1_CATEGORIES),
            "q4_language_constitution": (
                "GM may ask an open prompt, but must not present GM-authored action menus, "
                "explicit option branches, or raw content dumps as the player's choices."
            ),
            "p1_player_simulation": (
                "Player decisions must be based only on player-visible information, keep goals coherent, "
                "resolve or clarify pending access gates, and adapt to concrete affordances/results."
            ),
        },
        "turns": [
            {
                "turn": turn.get("turn"),
                "gm_visible_text": truncate_for_semantic_payload(turn.get("text", "")),
                "player_input": truncate_for_semantic_payload(turn.get("input_text", ""), 2000),
            }
            for turn in turns
        ],
        "player_decisions": [
            {
                "turn": decision.get("turn"),
                "gm_visible_reply": truncate_for_semantic_payload(
                    decision.get("gm_visible_reply", ""), 3000
                ),
                "perceived_facts": decision.get("perceived_facts", []),
                "active_goal": decision.get("active_goal", ""),
                "hypotheses": decision.get("hypotheses", []),
                "candidate_actions": decision.get("candidate_actions", []),
                "selection_rationale": decision.get("selection_rationale", ""),
                "declared_action": decision.get("declared_action", ""),
                "sent_to_gm": decision.get("sent_to_gm", ""),
                "response_contract": response_contract_from_signal(decision),
            }
            for decision in decisions
        ],
        "expected_output": {
            "findings": [
                {
                    "category": "ACTION_MENU",
                    "severity": "S1",
                    "turns": [1],
                    "root_layer": "Presentation|PlayerSimulator|Narrator|Rules|World|Director|Evaluator",
                    "expected": "what should have happened",
                    "actual": "what happened",
                    "evidence": ["short quoted/paraphrased evidence"],
                    "confidence": 0.0,
                }
            ]
        },
    }


def normalize_semantic_turns(value: Any) -> List[int]:
    if isinstance(value, list):
        items = value
    elif value in (None, ""):
        items = []
    else:
        items = [value]
    out: List[int] = []
    for item in items:
        try:
            out.append(int(item))
        except (TypeError, ValueError):
            continue
    return out


def normalize_semantic_findings(value: Any) -> List[Dict[str, Any]]:
    raw_findings: Any
    if isinstance(value, dict):
        raw_findings = value.get("findings", [])
    else:
        raw_findings = value
    if not isinstance(raw_findings, list):
        return []
    findings: List[Dict[str, Any]] = []
    for index, raw in enumerate(raw_findings):
        if not isinstance(raw, dict):
            continue
        category = str(raw.get("category", "")).strip().upper()
        if not category:
            continue
        severity = str(raw.get("severity", "S2")).strip().upper()
        evidence = raw.get("evidence", [])
        if isinstance(evidence, str):
            evidence_items = [evidence]
        elif isinstance(evidence, list):
            evidence_items = [str(item) for item in evidence if str(item).strip()]
        else:
            evidence_items = []
        finding = dict(raw)
        finding["finding_id"] = str(raw.get("finding_id") or f"semantic-{index + 1:04d}")
        finding["category"] = category
        finding["severity"] = severity
        finding["turns"] = normalize_semantic_turns(raw.get("turns", raw.get("turn")))
        finding["evidence"] = evidence_items
        findings.append(finding)
    return findings


def semantic_finding_is_blocking(finding: Dict[str, Any]) -> bool:
    return str(finding.get("severity", "")).upper() in SEMANTIC_BLOCKING_SEVERITIES


def semantic_findings_for_categories(
    findings: List[Dict[str, Any]],
    categories: Tuple[str, ...],
) -> List[Dict[str, Any]]:
    allowed = set(categories)
    return [
        finding
        for finding in findings
        if finding.get("category") in allowed and semantic_finding_is_blocking(finding)
    ]


def semantic_finding_evidence_line(finding: Dict[str, Any]) -> str:
    turns = finding.get("turns") or []
    turn_label = ",".join(str(turn) for turn in turns) if turns else "?"
    detail = str(finding.get("actual") or finding.get("expected") or "").strip()
    if not detail and finding.get("evidence"):
        detail = str(finding["evidence"][0]).strip()
    if detail:
        return f"semantic turn {turn_label}: {finding['category']} - {detail}"
    return f"semantic turn {turn_label}: {finding['category']}"


def semantic_critic_result(
    run_dir: Path,
    turns: List[Dict[str, Any]],
    decisions: List[Dict[str, Any]],
    semantic_critic_cmd: str | None = None,
) -> Dict[str, Any]:
    fixture_json = os.environ.get("TRPG_EVAL_SEMANTIC_CRITIC_JSON")
    if fixture_json:
        try:
            return {
                "enabled": True,
                "source": "env:TRPG_EVAL_SEMANTIC_CRITIC_JSON",
                "error": None,
                "findings": normalize_semantic_findings(json.loads(fixture_json)),
            }
        except json.JSONDecodeError as exc:
            return {
                "enabled": True,
                "source": "env:TRPG_EVAL_SEMANTIC_CRITIC_JSON",
                "error": f"invalid semantic critic JSON: {exc}",
                "findings": [],
            }

    cmd_text = semantic_critic_cmd or os.environ.get("TRPG_EVAL_SEMANTIC_CRITIC_CMD", "")
    if not cmd_text.strip():
        return {"enabled": False, "source": None, "error": None, "findings": []}

    payload = semantic_critic_payload(run_dir, turns, decisions)
    timeout = int(os.environ.get("TRPG_EVAL_SEMANTIC_CRITIC_TIMEOUT", "180"))
    try:
        completed = subprocess.run(
            shlex.split(cmd_text),
            input=json.dumps(payload, ensure_ascii=False),
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        return {"enabled": True, "source": cmd_text, "error": str(exc), "findings": []}
    if completed.returncode != 0:
        error = completed.stderr.strip() or completed.stdout.strip() or f"exit {completed.returncode}"
        return {"enabled": True, "source": cmd_text, "error": error, "findings": []}
    try:
        parsed = json.loads(completed.stdout)
    except json.JSONDecodeError as exc:
        return {
            "enabled": True,
            "source": cmd_text,
            "error": f"semantic critic emitted invalid JSON: {exc}",
            "findings": [],
        }
    return {
        "enabled": True,
        "source": cmd_text,
        "error": None,
        "findings": normalize_semantic_findings(parsed),
    }


def env_truthy(name: str) -> bool:
    return os.environ.get(name, "").strip().lower() in {"1", "true", "yes", "on"}


def judge_semantic_required(semantic: Dict[str, Any], required: bool) -> Dict[str, Any]:
    enabled = bool(semantic.get("enabled"))
    error = semantic.get("error")
    status = "GREEN"
    evidence = ["semantic critic is optional for this run"]
    if required:
        if not enabled:
            status = "RED"
            evidence = ["TRPG_EVAL_REQUIRE_SEMANTIC_CRITIC is set but no semantic critic ran"]
        elif error:
            status = "RED"
            evidence = [f"required semantic critic failed: {error}"]
        else:
            evidence = [f"required semantic critic ran via {semantic.get('source') or 'configured hook'}"]
    return judge(
        "SEM",
        "semantic-critic-availability",
        status,
        {
            "required": required,
            "enabled": enabled,
            "error": error or "",
            "findings": len(semantic.get("findings", [])),
        },
        evidence,
        {"required_when": "TRPG_EVAL_REQUIRE_SEMANTIC_CRITIC=1"},
    )


def nonempty_string(row: Dict[str, Any], key: str) -> bool:
    return bool(str(row.get(key, "")).strip())


def nonempty_list(row: Dict[str, Any], key: str) -> bool:
    value = row.get(key)
    return isinstance(value, list) and any(str(item).strip() for item in value)


def response_contract_from_signal(row: Dict[str, Any]) -> Dict[str, Any]:
    value = row.get("response_contract")
    if not isinstance(value, dict):
        value = row.get("expected_response_contract")
    return value if isinstance(value, dict) else {}


def normalized_signal_text(value: Any) -> str:
    return " ".join(str(value or "").split())


def player_decision_protocol_gaps(row: Dict[str, Any]) -> List[str]:
    gaps: List[str] = []
    required_strings = (
        "gm_visible_reply",
        "active_goal",
        "last_action_result",
        "selection_rationale",
        "declared_action",
        "sent_to_gm",
    )
    required_lists = (
        "perceived_facts",
        "hypotheses",
        "risk_assessment",
        "resource_assessment",
        "open_questions",
    )
    aliases = {
        "resource_assessment": ("resources_considered",),
        "selection_rationale": ("selection_reason",),
        "sent_to_gm": ("gm_input",),
    }

    def has_string(key: str) -> bool:
        if nonempty_string(row, key):
            return True
        return any(nonempty_string(row, alias) for alias in aliases.get(key, ()))

    def has_list(key: str) -> bool:
        if nonempty_list(row, key):
            return True
        return any(nonempty_list(row, alias) for alias in aliases.get(key, ()))

    for key in required_strings:
        if not has_string(key):
            gaps.append(key)
    for key in required_lists:
        if not has_list(key):
            gaps.append(key)

    candidate_actions = row.get("candidate_actions")
    if not isinstance(candidate_actions, list) or len([a for a in candidate_actions if str(a).strip()]) < 2:
        gaps.append("candidate_actions>=2")

    contract = response_contract_from_signal(row)
    if not str(contract.get("intent", "")).strip():
        gaps.append("response_contract.intent")
    acceptable = contract.get("acceptable_resolutions")
    if not isinstance(acceptable, list) or not any(str(item).strip() for item in acceptable):
        gaps.append("response_contract.acceptable_resolutions")

    sent_to_gm = row.get("sent_to_gm", row.get("gm_input", ""))
    if normalized_signal_text(sent_to_gm) and normalized_signal_text(sent_to_gm) != normalized_signal_text(row.get("declared_action", "")):
        gaps.append("sent_to_gm_must_equal_declared_action")

    return gaps


def player_decision_text(row: Dict[str, Any]) -> str:
    parts: List[str] = []
    for key in (
        "gm_visible_reply",
        "last_action_result",
        "active_goal",
        "selection_rationale",
        "declared_action",
    ):
        value = row.get(key)
        if value is not None:
            parts.append(str(value))
    for key in (
        "perceived_facts",
        "hypotheses",
        "risk_assessment",
        "resource_assessment",
        "open_questions",
        "candidate_actions",
    ):
        value = row.get(key)
        if isinstance(value, list):
            parts.extend(str(item) for item in value)
    return " ".join(parts).lower()


def player_decision_claim_text(row: Dict[str, Any]) -> str:
    parts: List[str] = []
    for key in (
        "last_action_result",
        "active_goal",
        "selection_rationale",
        "declared_action",
    ):
        value = row.get(key)
        if value is not None:
            parts.append(str(value))
    for key in (
        "perceived_facts",
        "hypotheses",
        "risk_assessment",
        "resource_assessment",
        "open_questions",
        "candidate_actions",
    ):
        value = row.get(key)
        if isinstance(value, list):
            parts.extend(str(item) for item in value)
    return " ".join(parts).lower()


def player_selected_action_text(row: Dict[str, Any]) -> str:
    contract = response_contract_from_signal(row)
    parts: List[str] = [str(contract.get("intent", ""))]
    for key in ("active_goal", "selection_rationale", "declared_action", "sent_to_gm"):
        value = row.get(key)
        if value is not None:
            parts.append(str(value))
    return " ".join(parts).lower()


PROTECTED_UNSEEN_TERMS: Tuple[Tuple[str, Tuple[str, ...]], ...] = (
    ("walter corbitt", ("walter corbitt",)),
    ("michael thomas", ("michael thomas", "迈克尔", "托马斯")),
    ("chapel", ("chapel", "沉思", "礼拜堂", "小礼拜堂")),
    ("1912", ("1912",)),
    ("burial", ("burial", "buried")),
)


def protected_burial_visible(text: str) -> bool:
    lowered = text.lower()
    negated = any(
        cue in lowered
        for cue in (
            "no burial",
            "no clear burial",
            "no confirmed burial",
            "not have a confirmed burial",
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
    if chinese_subject and chinese_burial and chinese_place and not negated:
        return True
    if re.search(r"\bburial\b", lowered) and not negated:
        return True
    if re.search(
        r"\bburied\s+in\s+(routine|paperwork|records?|files?|paper|ledgers?|archives?|indexes?|remnants?)\b",
        lowered,
    ):
        return False
    if re.search(r"\bburied\b", lowered) and not negated:
        for match in re.finditer(r"\bburied\b", lowered):
            window = lowered[max(0, match.start() - 100) : match.end() + 100]
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
    return False


def protected_term_visible(label: str, variants: Tuple[str, ...], visible_history: str) -> bool:
    if label == "burial":
        return protected_burial_visible(visible_history)
    return any(variant in visible_history for variant in variants)


def protected_term_claimed_in_non_negated_context(
    claim: str, variants: Tuple[str, ...]
) -> bool:
    negation_cues = (
        "without",
        "not ",
        "not treat",
        "do not",
        "does not",
        "did not",
        "if not visible",
        "not visible",
        "unseen",
        "hidden",
        "metaphorical",
        "不要",
        "不能",
        "不可",
        "未",
        "没有",
        "不应",
    )
    spans = re.split(r"(?<=[.!?])\s+|[;\n]", claim)
    for span in spans:
        lowered = span.lower()
        if not any(variant in lowered for variant in variants):
            continue
        if any(cue in lowered for cue in negation_cues):
            continue
        return True
    return False


def visible_history_before_turn(
    run_dir: Path,
    turns: List[Dict[str, Any]],
    turn_no: int,
    decision: Dict[str, Any],
) -> str:
    parts: List[str] = []
    opening = run_dir / "opening.txt"
    if opening.exists():
        parts.append(opening.read_text(errors="replace"))
    reply = decision.get("gm_visible_reply")
    if reply is not None:
        parts.append(str(reply))
    for turn in turns:
        try:
            current = int(turn.get("turn", 0))
        except (TypeError, ValueError):
            continue
        if current < turn_no:
            parts.append(str(turn.get("text", "")))
    return " ".join(parts).lower()


def decision_unseen_protected_terms(
    decision: Dict[str, Any],
    visible_history: str,
) -> List[str]:
    claim = player_decision_claim_text(decision)
    leaks: List[str] = []
    for label, variants in PROTECTED_UNSEEN_TERMS:
        if protected_term_claimed_in_non_negated_context(
            claim, variants
        ) and not protected_term_visible(label, variants, visible_history):
            leaks.append(label)
    return leaks


def player_decision_belief_text(row: Dict[str, Any]) -> str:
    parts: List[str] = []
    for key in ("gm_visible_reply", "last_action_result"):
        value = row.get(key)
        if value is not None:
            parts.append(str(value))
    value = row.get("perceived_facts")
    if isinstance(value, list):
        parts.extend(str(item) for item in value)
    return " ".join(parts).lower()


def visible_active_blade_threat(text: str) -> bool:
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


def historical_or_nonactive_blade_reference(text: str) -> bool:
    lowered = text.lower()
    return any(cue in lowered for cue in ("knife", "dagger", "blade")) and not visible_active_blade_threat(text)


def previous_turn_has_failed_roll(text: str) -> bool:
    for block in roll_blocks(text):
        lowered = block.lower()
        if "failure" in lowered or "failed" in lowered or "失败" in block:
            return True
    return False


def previous_turn_last_roll_failed(text: str) -> bool:
    blocks = roll_blocks(text)
    if not blocks:
        return False
    lowered = blocks[-1].lower()
    return "failure" in lowered or "failed" in lowered or "失败" in blocks[-1]


def previous_turn_has_success_roll(text: str) -> bool:
    for block in roll_blocks(text):
        lowered = block.lower()
        if (
            "success" in lowered
            or "hard success" in lowered
            or "extreme success" in lowered
            or "成功" in block
        ) and not ("failure" in lowered or "failed" in lowered or "失败" in block):
            return True
    return False


def previous_has_concrete_chapel_payoff(text: str) -> bool:
    lowered_full = text.lower()
    lowered = text_without_roll_blocks(text).lower()
    has_chapel_source = any(
        cue in lowered_full for cue in ("chapel", "礼拜堂", "小礼拜堂", "教堂", "沉思")
    )
    if not has_chapel_source or not previous_turn_has_success_roll(text):
        return False
    chinese_subject = any(cue in lowered for cue in ("科比特", "沃尔特", "corbitt"))
    chinese_burial = any(cue in lowered for cue in ("被埋", "埋在", "安葬", "葬在"))
    chinese_place = any(cue in lowered for cue in ("地下室", "地窖", "那栋房子", "房子", "宅邸"))
    if chinese_subject and chinese_burial and chinese_place:
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


def decision_denies_visible_chapel_payoff(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    if not previous_has_concrete_chapel_payoff(previous_text):
        return False
    claim = player_decision_text(decision)
    denial_cues = (
        "treats the last reply as arrival at the chapel, not as the search result",
        "last reply established arrival and atmosphere",
        "not the outcome of the chapel search",
        "has not produced concrete player-usable findings",
        "did not yet resolve the declared search",
        "chapel search did not give",
        "no chapel clue",
        "not produced concrete",
        "without resolving the search",
    )
    return any(cue in claim for cue in denial_cues)


def previous_has_newspaper_official_records_lead(text: str) -> bool:
    lowered = text.lower()
    if not previous_turn_has_success_roll(text):
        return False
    has_newspaper_source = any(
        cue in lowered
        for cue in (
            "newspaper archive",
            "newspaper morgue",
            "newspaper indexing",
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


def decision_ignores_newspaper_official_records_lead(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    if not previous_has_newspaper_official_records_lead(previous_text):
        return False
    claim = player_selected_action_text(decision)
    follows_official = any(
        cue in claim
        for cue in (
            "access_general_corbitt_court_police_records",
            "official records",
            "police records",
            "police files",
            "higher court",
            "higher-court",
            "court records",
            "court files",
        )
    )
    if follows_official:
        return False
    return any(
        cue in claim
        for cue in (
            "inspect_house_exterior",
            "corbitt house in daylight",
            "goes to the corbitt house",
            "tests the least exposed lock",
            "house exterior",
        )
    )


def previous_has_official_boundary_map_to_newspaper(text: str) -> bool:
    lowered = text.lower()
    if not official_records_access_without_content(text):
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


def decision_leaves_official_boundary_map_for_house(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    if not previous_has_official_boundary_map_to_newspaper(previous_text):
        return False
    claim = player_decision_claim_text(decision)
    follows_newspaper = any(
        cue in claim
        for cue in (
            "search_newspaper_archive_after_official_records_boundary",
            "newspaper",
            "morgue",
            "archive",
            "clippings",
        )
    )
    if follows_newspaper:
        return False
    return any(
        cue in claim
        for cue in (
            "inspect_house_exterior",
            "corbitt house in daylight",
            "goes to the corbitt house",
            "tests the least exposed lock",
            "house exterior",
        )
    )


def previous_has_newspaper_access_gate_pending(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("newspaper", "globe", "morgue", "clippings")):
        return False
    if not any(cue in lowered for cue in ("editor", "wilmot", "gatekeeper", "access")):
        return False
    return any(
        cue in lowered
        for cue in (
            "how does evelyn try to get in",
            "tell me her approach",
            "how does she approach",
            "how does she try to get past",
            "does she lean on",
            "do you have her lean on",
            "how she does it matters",
            "what tone does",
            "how does she press",
        )
    )


def decision_leaves_newspaper_access_gate_unresolved(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    if not previous_has_newspaper_access_gate_pending(previous_text):
        return False
    contract = response_contract_from_signal(decision)
    claim = " ".join(
        str(value)
        for value in (
            decision.get("declared_action", ""),
            decision.get("sent_to_gm", ""),
            contract.get("intent", ""),
        )
        if value
    ).lower()
    resolves_gate = any(
        cue in claim
        for cue in (
            "wilmot",
            "editor",
            "gatekeeper",
            "press_newspaper_archive_access",
            "newspaper access",
            "globe",
            "morgue",
            "clippings",
            "persuade",
            "persuasion",
        )
    )
    if resolves_gate:
        return False
    return any(
        cue in claim
        for cue in (
            "inspect_house_exterior",
            "corbitt house in daylight",
            "goes to the corbitt house",
            "house exterior",
            "investigate_chapel",
            "chapel",
        )
    )


def decision_contradicts_previous_failed_result(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    if not previous_turn_has_failed_roll(previous_text):
        return False
    if previous_turn_has_success_roll(previous_text) and not previous_turn_last_roll_failed(previous_text):
        return False
    previous = previous_text.lower()
    claim = player_decision_belief_text(decision)
    acknowledges_failure = any(
        cue in claim
        for cue in (
            "failed",
            "failure",
            "did not",
            "not yet",
            "no clear",
            "no definite",
            "失败",
            "没有",
            "未能",
        )
    )
    if acknowledges_failure:
        return False
    if (
        ("no clear burial" in previous or "no definite mention of walter corbitt" in previous)
        and any(cue in claim for cue in ("burial", "buried", "basement", "埋", "地下室"))
    ):
        return True
    return any(
        cue in claim
        for cue in (
            "succeeded",
            "success",
            "revealed",
            "produced the",
            "yielded the",
            "成功",
            "揭示",
            "拿到",
        )
    )


def decision_ignores_previous_actionable_success_lead(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    if not previous_turn_has_success_roll(previous_text):
        return False
    previous = previous_text.lower()
    basement_lead = ("basement" in previous or "cellar" in previous) and any(
        cue in previous
        for cue in (
            "disturbed earth",
            "scrape mark",
            "scraped area",
            "conceal",
            "hidden panel",
            "hidden access",
            "suspicious section",
            "signs of disturbance worth following",
            "disturbance worth following",
            "worth following more closely",
            "deserves closer inspection",
            "not just an untouched junk cellar",
            "area below that deserves",
        )
    )
    if not basement_lead:
        return False
    declared = str(decision.get("declared_action", "")).lower()
    leaves_for_upstairs = any(
        cue in declared
        for cue in (
            "goes upstairs",
            "go upstairs",
            "upper floor",
            "upstairs",
            "bedroom",
            "landing",
        )
    )
    follows_basement_lead = any(
        cue in declared
        for cue in (
            "disturbed earth",
            "scrape mark",
            "scraped area",
            "conceal",
            "hidden panel",
            "suspicious section",
            "basement",
            "cellar",
        )
    )
    return leaves_for_upstairs and not follows_basement_lead


def previous_has_concrete_basement_board_affordance(previous_text: str) -> bool:
    previous = previous_text.lower()
    if not any(cue in previous for cue in ("basement", "cellar")):
        return False
    if not any(
        cue in previous
        for cue in (
            "rough boards",
            "rough wooden boards",
            "rough boarding",
            "old bins",
            "boards, old bins",
            "boards and old bins",
            "boarded irregularity",
            "rough, concealed section",
            "rough concealed section",
        )
    ):
        return False
    if not any(
        cue in previous
        for cue in (
            "concealment",
            "concealed area",
            "intentionally closed off",
            "patches of darkness",
            "darkness under the structure",
            "under-space",
            "dark under-space",
            "under space",
            "does not read like ordinary storage",
            "less like storage",
            "darker",
        )
    ):
        return False
    return any(
        cue in previous
        for cue in (
            "what does evelyn examine first",
            "what do you examine first",
            "route behind you remains open",
            "retreat path",
            "standing just off the foot",
            "foot of the stairs",
            "single most concrete",
            "plainly actionable thing",
            "plainest actionable thing",
            "immediate, usable fact",
            "relative to your retreat",
            "way back",
            "stairs up to the ground floor",
        )
    )


def previous_has_concrete_basement_bottom_affordance(previous_text: str) -> bool:
    previous = previous_text.lower()
    if not any(cue in previous for cue in ("basement", "cellar")):
        return False
    if not any(
        cue in previous
        for cue in (
            "bottom of the basement stairs",
            "bottom of the cellar stairs",
            "foot of the stairs",
            "at the foot of the stairs",
        )
    ):
        return False
    if not any(
        cue in previous
        for cue in (
            "foundation wall",
            "foundation walls",
            "cluttered dimness",
            "cellar ahead",
            "basement ahead",
            "cellar proper",
        )
    ):
        return False
    return any(
        cue in previous
        for cue in (
            "marked way back behind",
            "way back behind",
            "retreat",
            "nothing blocks the way",
            "stairs are immediately behind",
        )
    )


def decision_falls_back_after_concrete_visible_affordance(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    return fallback_intent(decision) and (
        previous_has_concrete_basement_board_affordance(previous_text)
        or previous_has_concrete_basement_bottom_affordance(previous_text)
    )


def previous_has_bedroom_threat_subject(previous_text: str) -> bool:
    visible_narration = re.sub(
        r"\[roll\].*?\[/roll\]", " ", previous_text, flags=re.IGNORECASE | re.DOTALL
    )
    previous = visible_narration.lower()
    if not any(
        contains_word(visible_narration, cue)
        for cue in (
            "bedroom",
            "bed",
            "bedframe",
            "bedclothes",
            "bedding",
            "mattress",
            "furniture",
        )
    ):
        return False
    positive_cues = (
        "violent lurch",
        "violent heave",
        "sudden, violent",
        "moves by itself",
        "moved by itself",
        "moving by itself",
        "moves on its own",
        "moved on its own",
        "moving on its own",
        "without any human cause",
        "impossible movement",
        "plainly impossible",
        "hard scraping jerk",
        "hard scrape",
        "jerked and scraped",
        "scrapes toward the doorway",
        "lurching bed",
        "attack came from the bed",
        "actively dangerous",
        "paper moved on its own",
    )
    negation_patterns = (
        r"\bnothing(?:\s+\w+){0,4}\s+(?:moves|moved|moving|lunges|rushes|stirs|stirred|shifts|shifted)\b",
        r"\bno\s+(?:bedclothes?|wardrobe\s+door|object|furniture|bed|mattress)(?:\s+\w+){0,4}\s+(?:moves|moved|moving|lunges|rushes|twitches?|swings?|stirs|stirred|shifts?|shifted)\b",
        r"\bno\s+(?:new\s+|fresh\s+|further\s+|sudden\s+)?(?:movement|threat|rushing threat|stir|stirs|shift|shifts)\b",
        r"\bnot\s+moving\s+(?:on its own|by itself)\b",
        r"\bwithout\s+(?:new\s+|fresh\s+)?movement\b",
    )
    for cue in positive_cues:
        start = previous.find(cue)
        while start != -1:
            window = previous[max(0, start - 90) : start + len(cue) + 90]
            if not any(re.search(pattern, window) for pattern in negation_patterns):
                return True
            start = previous.find(cue, start + 1)
    return False


def decision_invents_moving_bedroom_threat(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    if previous_has_bedroom_threat_subject(previous_text):
        return False
    claim = " ".join(
        str(decision.get(key, ""))
        for key in (
            "declared_action",
            "sent_to_gm",
            "active_goal",
            "selection_rationale",
        )
    ).lower()
    intent = str(response_contract_from_signal(decision).get("intent", "")).lower()
    return any(
        cue in intent or cue in claim
        for cue in (
            "retreat_from_moving_bedroom_threat",
            "moving-bedroom hazard",
            "moving bedroom hazard",
            "moving bed",
            "bed's impossible movement",
            "bed’s impossible movement",
            "moving furniture",
        )
    )


def decision_treats_nonactive_blade_reference_as_current_threat(
    decision: Dict[str, Any], previous_text: str, visible_history: str
) -> bool:
    if visible_active_blade_threat(previous_text):
        return False
    if visible_active_blade_threat(visible_history):
        return False
    if not (
        historical_or_nonactive_blade_reference(previous_text)
        or historical_or_nonactive_blade_reference(visible_history)
    ):
        return False
    claim = player_decision_claim_text(decision)
    intent = str(response_contract_from_signal(decision).get("intent", "")).lower()
    claims_current_threat = any(
        cue in intent or cue in claim
        for cue in (
            "defend_against_visible_blade_threat",
            "visible knife",
            "visible dagger",
            "immediate threat",
            "current blade",
            "current knife",
            "self-moving blade",
            "self-moving knife",
        )
    )
    return claims_current_threat and any(cue in claim for cue in ("knife", "dagger", "blade"))


def previous_has_visible_crawl_space_chapel_carving(previous_text: str) -> bool:
    previous = previous_text.lower()
    has_local_opening = any(
        cue in previous
        for cue in (
            "crawl space",
            "crawl-space",
            "crawlspace",
            "crawl-space entrance",
            "crawl space entrance",
            "rough opening",
            "hidden gap",
            "gap beyond",
            "space past the boards",
            "space past the rough boards",
            "cramped dark space",
            "cramped, dark under-space",
            "cramped under-space",
            "words cut into the inner wall",
        )
    )
    if not has_local_opening:
        return False
    return any(
        cue in previous
        for cue in (
            "chapel of contemplation",
            "carved words",
            "carved lettering",
            "words cut into the inner wall",
            "words become legible",
            "carved line",
        )
    )


def decision_claims_crawl_space_chapel_carving_without_visible_evidence(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    claim = player_decision_claim_text(decision)
    intent = str(response_contract_from_signal(decision).get("intent", "")).lower()
    claims_carving = (
        "inspect_opened_crawl_space_and_chapel_carving" in intent
        or (
            any(
                cue in claim
                for cue in (
                    "opened basement crawl-space",
                    "opened basement crawl space",
                    "crawl-space entrance",
                    "crawl space entrance",
                )
            )
            and any(
                cue in claim
                for cue in (
                    "chapel of contemplation",
                    "carved words",
                    "transcribes",
                    "carved chapel",
                    "chapel carving",
                )
            )
        )
    )
    return claims_carving and not previous_has_visible_crawl_space_chapel_carving(
        previous_text
    )


def previous_has_left_house_after_crawl_space_withdrawal(previous_text: str) -> bool:
    previous = previous_text.lower()
    if not any(cue in previous for cue in ("out of corbitt house", "out of the house", "outside", "open air")):
        return False
    if not any(cue in previous for cue in ("crawl-space", "crawl space", "crawlspace")):
        return False
    return any(
        cue in previous
        for cue in (
            "better light",
            "proper tools",
            "rope",
            "line",
            "another pair of eyes",
            "witness",
            "helper",
            "unsafe",
            "leave cleanly",
            "house behind you",
        )
    )


def decision_continues_crawl_space_after_house_exit(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    if not previous_has_left_house_after_crawl_space_withdrawal(previous_text):
        return False
    claim = player_decision_claim_text(decision)
    intent = str(response_contract_from_signal(decision).get("intent", "")).lower()
    return (
        "probe_deeper_crawl_space" in intent
        or "crawl-space opening" in claim
        or "crawl space opening" in claim
    ) and any(
        cue in claim
        for cue in (
            "reach deeper",
            "probe deeper",
            "extends only the flashlight",
            "one controlled reach",
            "stays oriented on the crawl",
        )
    )


def previous_has_active_crawl_space_position(previous_text: str) -> bool:
    previous = previous_text.lower()
    if not any(cue in previous for cue in ("crawl space", "crawl-space", "crawlspace")):
        return False
    if not any(
        cue in previous
        for cue in (
            "half out of the opening",
            "half inside",
            "one body length",
            "hips still near the lip",
            "braced toward the opening",
            "braced at the opening",
            "braced position",
            "at the lip",
            "body angled to the crawl-space wall",
            "body angled to the crawl space wall",
            "stays oriented on the crawl-space wall",
            "stays oriented on the crawl space wall",
        )
    ):
        return False
    if not any(
        cue in previous
        for cue in (
            "retreat line",
            "retreat remains",
            "retreat path",
            "basement stairs",
            "marked opening",
            "marked stairs",
            "clean line out",
        )
    ):
        return False
    return not any(
        cue in previous
        for cue in (
            "backs fully out",
            "backed fully out",
            "backs out of the crawl",
            "backed out of the crawl",
            "clear of the opening",
            "back on the basement floor",
            "back on the ground floor",
        )
    )


def decision_changes_floor_from_active_crawl_space_position(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    if not previous_has_active_crawl_space_position(previous_text):
        return False
    intent = str(response_contract_from_signal(decision).get("intent", "")).lower()
    action_claim = " ".join(
        str(decision.get(key, ""))
        for key in (
            "declared_action",
            "sent_to_gm",
            "active_goal",
            "selection_rationale",
        )
    ).lower()
    claim = f"{intent} {action_claim}"
    changes_floor = any(
        cue in claim
        for cue in (
            "cautious_upper_floor_search",
            "goes upstairs",
            "go upstairs",
            "upper floor",
            "upstairs",
            "bedroom",
            "landing",
        )
    )
    if not changes_floor:
        return False
    return not any(
        cue in claim
        for cue in (
            "backs out",
            "backs fully out",
            "withdraw",
            "returns to the basement floor",
            "clears the crawl",
            "crawl-space",
            "crawl space",
        )
    )


def decision_jumps_to_chapel_from_active_crawl_space_position(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    if not previous_has_active_crawl_space_position(previous_text):
        return False
    intent = str(response_contract_from_signal(decision).get("intent", "")).lower()
    action_claim = " ".join(
        str(decision.get(key, ""))
        for key in (
            "declared_action",
            "sent_to_gm",
            "active_goal",
            "selection_rationale",
            "last_action_result",
        )
    ).lower()
    claim = f"{intent} {action_claim}"
    chapel_jump = any(
        cue in claim
        for cue in (
            "retry_chapel",
            "drive to the chapel",
            "drives to the chapel",
            "go to the chapel",
            "goes to the chapel",
            "return to the chapel",
            "search the chapel again",
            "searches the chapel again",
            "pulpit",
            "altar",
        )
    )
    if not chapel_jump:
        return False
    return not any(
        cue in claim
        for cue in (
            "backs out",
            "backs fully out",
            "withdraw",
            "clears the crawl",
            "once she is clear",
            "once clear",
            "back to the marked stairs",
        )
    )


def decision_claims_speculative_basement_detail_as_concrete_lead(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    previous = previous_text.lower()
    if not any(cue in previous for cue in ("basement", "cellar")):
        return False
    if not any(cue in previous for cue in ("door", "affordance", "threshold", "cellar space")):
        return False
    speculative = any(
        cue in previous
        for cue in (
            "cannot yet prove",
            "still cannot prove",
            "what you still cannot prove",
            "whether it is locked",
            "whether it merely",
            "whether it conceals",
            "what lies beyond",
            "does not cleanly give",
            "may be only",
            "might be only",
        )
    )
    if not speculative:
        return False
    claim = " ".join(
        str(decision.get(key, ""))
        for key in (
            "declared_action",
            "sent_to_gm",
            "active_goal",
            "selection_rationale",
            "last_action_result",
        )
    ).lower()
    assertive_lead_claim = any(
        cue in claim
        for cue in (
            "last search produced a concrete lead",
            "successful basement clue",
            "follow up the successful basement clue",
            "concrete lead",
        )
    )
    assertive_detail_claim = bool(
        re.search(
            r"\b(?:photographs?|photographing|traces?|tracing|probes?|probing|tests?|testing)\b"
            r".{0,96}\b(?:disturbed earth|scrape marks?|scraped area|suspicious concealed section)\b",
            claim,
        )
    )
    search_target_only = bool(
        re.search(
            r"\b(?:look(?:s)? for|search(?:es)? for|check(?:s|ing)? for|watch(?:es|ing)? for)\b"
            r".{0,140}\b(?:disturbed earth|scrape marks?|scraped area|suspicious concealed section|hidden panels?|body|ritual object)\b",
            claim,
        )
    )
    return assertive_lead_claim or (assertive_detail_claim and not search_target_only)


def previous_has_unproven_exterior_lock_fit(previous_text: str) -> bool:
    previous = previous_text.lower()
    if not any(cue in previous for cue in ("door", "entrance", "threshold", "entry point", "access", "way in")):
        return False
    if not any(cue in previous for cue in ("key", "lock", "keyway", "latch")):
        return False
    if not any(
        cue in previous
        for cue in (
            "cannot yet prove",
            "cannot prove",
            "can't yet prove",
            "do not yet know",
            "don't yet know",
            "what remains uncertain",
            "whether this key fits",
            "whether the key fits",
            "whether any key fits",
            "whether the lock",
            "whether it is locked",
            "unknown begins at the lock",
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
    return not any(
        cue in previous
        for cue in (
            "lock accepts it",
            "lock has yielded",
            "lock yielded",
            "confirmed to accept",
            "proven to take",
            "lock she tested works",
            "lock works",
            "house can be entered",
            "matched to knott's key",
            "matched to knott’s key",
            "matched exterior door",
            "click and yield",
            "can be opened",
            "door stands open",
            "door is open",
        )
    )


def decision_unconditionally_unlocks_unproven_exterior_door(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    if not previous_has_unproven_exterior_lock_fit(previous_text):
        return False
    claim = " ".join(
        str(decision.get(key, ""))
        for key in (
            "declared_action",
            "sent_to_gm",
            "active_goal",
            "selection_rationale",
            "last_action_result",
        )
    ).lower()
    conditional_lock_test = any(
        cue in claim
        for cue in (
            "if the lock opens",
            "if it opens",
            "if the key turns",
            "if the lock yields",
            "if the door opens",
            "tests the key",
            "test the key",
            "tries the key",
            "try the key",
            "without assuming it opens",
            "rather than assuming",
        )
    )
    if conditional_lock_test:
        return False
    return bool(
        re.search(r"\bunlock(?:s|ed|ing)?\b.{0,80}\b(?:door|entrance|threshold|entry point)\b", claim)
        or re.search(r"\b(?:opens?|opened|opening)\b.{0,80}\b(?:door|entrance|threshold|entry point)\b", claim)
        or "unlocks the safest exterior door" in claim
        or "enters through the unlocked door" in claim
    )


def previous_has_failed_exterior_safety_read(previous_text: str) -> bool:
    previous = previous_text.lower()
    if not any(cue in previous for cue in ("exterior", "house", "entrance", "entry")):
        return False
    if not any(cue in previous for cue in ("failure", "failed", "失败")):
        return False
    return any(
        cue in previous
        for cue in (
            "clean, confident read",
            "clean confident read",
            "which points of entry are safest",
            "does not get a trustworthy read",
            "do not get a trustworthy read",
            "without the sharper exterior read",
            "not enough to know who has really been watching",
            "no trustworthy read on",
            "没有可靠判断",
            "无法确认哪个入口最安全",
        )
    )


def decision_claims_safest_exterior_after_failed_safety_read(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    if not previous_has_failed_exterior_safety_read(previous_text):
        return False
    claim = " ".join(
        str(decision.get(key, ""))
        for key in (
            "declared_action",
            "sent_to_gm",
            "active_goal",
            "selection_rationale",
            "last_action_result",
        )
    ).lower()
    candidates = decision.get("candidate_actions")
    if isinstance(candidates, list):
        claim += " " + " ".join(str(item) for item in candidates).lower()
    return any(
        cue in claim
        for cue in (
            "safest exterior door",
            "safest exterior lock",
            "safest matching exterior lock",
            "safest matching lock",
            "safest door",
            "safest entrance",
            "safest entry",
        )
    )


def decision_reenters_exterior_door_after_interior_ground_floor_position(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    previous = previous_text.lower()
    if any(
        cue in previous
        for cue in (
            "still outside",
            "remains outside",
            "not yet inside",
            "has not stepped inside",
        )
    ):
        return False
    if not any(
        cue in previous
        for cue in (
            "inside the house",
            "inside the corbitt house",
            "just inside",
            "ground floor",
            "ground-floor",
            "entry hall",
            "front entry is behind",
            "entry is behind",
            "entry behind",
        )
    ):
        return False
    if not any(
        cue in previous
        for cue in (
            "entry",
            "door",
            "ground floor",
            "ground-floor",
            "retreat route",
            "back path",
        )
    ):
        return False
    claim = " ".join(
        str(decision.get(key, ""))
        for key in (
            "declared_action",
            "sent_to_gm",
            "active_goal",
            "selection_rationale",
            "last_action_result",
        )
    ).lower()
    intent = str(decision.get("response_contract", {}).get("intent", "")).lower()
    reentry_intent = (
        "enter_threshold_and_map" in intent
        or "enter_threshold_via_clarified" in intent
    )
    reentry_claim = any(
        cue in claim
        for cue in (
            "unlocks the safest exterior door",
            "unlock the safest exterior door",
            "opens the safest exterior door",
            "open the safest exterior door",
            "opens it from the side",
            "steps just inside",
            "step just inside",
            "first step inside",
            "map the entry hall again",
        )
    )
    return reentry_intent and reentry_claim


def decision_claims_closed_exterior_door_already_open(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    previous = previous_text.lower()
    if not any(
        cue in previous
        for cue in (
            "one closed exterior door",
            "closed exterior door",
            "threshold remains closed",
            "door remains closed",
            "door is still closed",
            "door is shut",
            "threshold itself: one closed",
        )
    ):
        return False
    claim = " ".join(
        str(decision.get(key, ""))
        for key in (
            "declared_action",
            "sent_to_gm",
            "active_goal",
            "selection_rationale",
            "last_action_result",
        )
    ).lower()
    return any(
        cue in claim
        for cue in (
            "already open",
            "already-open",
            "door already open",
            "entrance already open",
            "threshold already open",
        )
    )


def decision_claims_basement_return_route_as_concrete_lead(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    previous = previous_text.lower()
    if not any(cue in previous for cue in ("basement", "cellar")):
        return False
    if not any(cue in previous for cue in ("stair", "stairs", "staircase")):
        return False
    if not any(
        cue in previous
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
    if not any(
        cue in previous
        for cue in (
            "single most concrete",
            "presently established affordance",
            "confirmed, legible",
            "safely reachable",
            "what you do not yet have",
            "without pressing deeper",
        )
    ):
        return False
    claim = " ".join(
        str(decision.get(key, ""))
        for key in (
            "declared_action",
            "sent_to_gm",
            "active_goal",
            "selection_rationale",
            "last_action_result",
        )
    ).lower()
    return any(
        cue in claim
        for cue in (
            "last search produced a concrete lead",
            "successful basement clue",
            "disturbed earth",
            "scrape mark",
            "scraped area",
            "suspicious concealed section",
            "concrete lead",
        )
    )


def decision_jumps_back_to_chapel_from_house_interior(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    previous = previous_text.lower()
    if "corbitt house" not in previous:
        return False
    if not any(cue in previous for cue in ("ground floor", "entry hall", "inside the house", "inside the corbitt house")):
        return False
    claim = " ".join(
        str(decision.get(key, ""))
        for key in (
            "declared_action",
            "sent_to_gm",
            "active_goal",
            "selection_rationale",
            "last_action_result",
        )
    ).lower()
    intent = str(decision.get("response_contract", {}).get("intent", "")).lower()
    return (
        "retry_chapel" in intent
        or "the chapel search did not give" in claim
        or "searches the chapel again" in claim
        or "return to the chapel" in claim
    )


def decision_abandons_clarified_basement_section(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    previous = previous_text.lower()
    if not any(cue in previous for cue in ("basement", "cellar")):
        return False
    if not any(cue in previous for cue in ("concealed section", "hidden barrier", "panel-like section")):
        return False
    if not any(cue in previous for cue in ("disturbed earth", "scrape marks", "few steps in front")):
        return False
    declared = str(decision.get("declared_action", "")).lower()
    goes_upper = any(
        cue in declared
        for cue in (
            "goes upstairs",
            "go upstairs",
            "upper floor",
            "upstairs",
            "bedroom",
            "landing",
        )
    )
    handles_section = any(
        cue in declared
        for cue in (
            "concealed section",
            "hidden barrier",
            "panel",
            "cavity",
            "disturbed earth",
            "scrape marks",
            "basement",
            "cellar",
        )
    )
    return goes_upper and not handles_section


def decision_changes_floor_after_failed_basement_stair_position(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    if not previous_turn_has_failed_roll(previous_text):
        return False
    previous = visible_cue_text(previous_text)
    if not any(cue in previous for cue in ("basement", "cellar")):
        return False
    if not any(cue in previous for cue in ("stairs", "stair", "steps", "risers")):
        return False
    if not any(
        cue in previous
        for cue in (
            "partway down",
            "part way down",
            "halfway down",
            "half-way down",
            "lower part of the stairs",
            "lower part of the stair",
            "lower part of the steps",
            "lower part of the cellar stairs",
            "lower part of the basement stairs",
            "from the stairs",
            "on the stairs",
            "on the stair",
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
            "back toward the open basement door",
            "handkerchief marker above",
            "backing up toward the doorway",
            "escape line intact",
            "cellar below",
            "dark below",
        )
    ):
        return False
    declared = str(decision.get("declared_action", "")).lower()
    changes_to_upper = any(
        cue in declared
        for cue in (
            "goes upstairs",
            "go upstairs",
            "upper floor",
            "upstairs",
            "bedroom",
            "landing",
        )
    )
    stays_with_basement_position = any(
        cue in declared
        for cue in (
            "basement stairs",
            "basement stair",
            "cellar below",
            "open basement door",
            "retreat",
            "withdraw",
            "current position",
        )
    )
    return changes_to_upper and not stays_with_basement_position


def previous_has_clarified_basement_route_priority(previous_text: str) -> bool:
    previous = visible_cue_text(previous_text)
    if not any(cue in previous for cue in ("basement", "cellar")):
        return False
    has_down_route = any(
        cue in previous
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
            "single most concrete visible affordance is the way down",
            "single most concrete visible affordance is the basement",
            "single most concrete visible affordance is the cellar",
        )
    )
    if not has_down_route:
        return False
    return any(
        cue in previous
        for cue in (
            "actually act on",
            "physically see",
            "real, present route",
            "real present route",
            "not a theory",
            "not beyond your retreat path",
            "not beyond the retreat path",
            "part of the ground floor space",
            "grounded affordance",
        )
    )


def decision_ignores_clarified_basement_route_priority(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    if not previous_has_clarified_basement_route_priority(previous_text):
        return False
    declared = str(decision.get("declared_action", "")).lower()
    intent = str(decision.get("response_contract", {}).get("intent", "")).lower()
    goes_upper = any(
        cue in declared or cue in intent
        for cue in (
            "goes upstairs",
            "go upstairs",
            "upper floor",
            "upstairs",
            "bedroom",
            "landing",
            "upper_floor",
        )
    )
    follows_basement = any(
        cue in declared or cue in intent
        for cue in (
            "basement",
            "cellar",
            "way down",
            "downward",
            "safe_basement_descent",
            "basement_descent",
        )
    )
    return goes_upper and not follows_basement


def decision_claims_refusal_after_successful_access(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    if not previous_turn_has_success_roll(previous_text):
        return False
    previous = previous_text.lower()
    successful_access = any(
        cue in previous
        for cue in (
            "gain access",
            "file trail",
            "raid in 1912",
            "affidavit",
            "relevant index",
            "thin file",
            "court and police records",
            "allowed to inspect",
            "constrained record trail",
            "real notes in hand",
            "raid, the deaths",
            "the raid, the deaths",
            "imprisonment and escape",
        )
    )
    if not successful_access:
        return False
    claim_parts: List[str] = []
    for key in ("declared_action", "last_action_result", "active_goal", "selection_rationale"):
        value = decision.get(key)
        if value is not None:
            claim_parts.append(str(value))
    facts = decision.get("perceived_facts")
    if isinstance(facts, list):
        claim_parts.extend(str(item) for item in facts)
    claim = " ".join(claim_parts).lower()
    refusal_claim = any(
        cue in claim
        for cue in (
            "does not pretend",
            "refusal",
            "refused",
            "blocked",
            "did not obtain",
            "do not get the file",
            "access was refused",
        )
    )
    success_acknowledged = any(
        cue in claim
        for cue in (
            "file trail shows",
            "real notes in hand",
            "allowed to inspect",
            "obtained the court",
            "obtained the police",
            "raid, the deaths",
            "imprisonment and escape",
            "official file contents",
        )
    )
    return refusal_claim and not success_acknowledged


def decision_leaves_hall_records_after_success_without_specific_facts(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    previous = previous_text.lower()
    if not any(
        cue in previous
        for cue in ("hall of records", "probate", "property", "municipal", "档案")
    ):
        return False
    if not success_without_concrete_information(previous_text):
        return False
    contract = response_contract_from_signal(decision)
    intent = str(contract.get("intent", "")).lower()
    claim = " ".join(
        str(decision.get(key, ""))
        for key in (
            "declared_action",
            "sent_to_gm",
            "active_goal",
            "selection_rationale",
            "last_action_result",
        )
    ).lower()
    goes_to_house = (
        "inspect_house_exterior" in intent
        or "enter_" in intent
        or (
            "corbitt house" in claim
            and any(cue in claim for cue in ("exterior", "enter", "door", "threshold", "goes to"))
        )
    )
    continues_public_records = any(
        cue in claim or cue in intent
        for cue in (
            "continue_hall_records",
            "hall of records",
            "municipal records",
            "newspaper archive",
            "court",
            "police",
            "specific record",
            "probate",
        )
    )
    return goes_to_house and not continues_public_records


def decision_leaves_official_records_after_access_without_contents(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    if not official_records_access_without_content(text_without_roll_blocks(previous_text)):
        return False
    contract = response_contract_from_signal(decision)
    intent = str(contract.get("intent", "")).lower()
    claim = " ".join(
        str(decision.get(key, ""))
        for key in (
            "declared_action",
            "sent_to_gm",
            "active_goal",
            "selection_rationale",
            "last_action_result",
        )
    ).lower()
    continues_official_records = any(
        cue in intent or cue in claim
        for cue in (
            "continue_official_records",
            "court",
            "police",
            "official record",
            "records desk",
            "file content",
            "case content",
        )
    )
    leaves_for_site = any(
        cue in intent or cue in claim
        for cue in (
            "investigate_chapel",
            "chapel",
            "house_exterior",
            "enter_",
            "corbitt house",
        )
    )
    return leaves_for_site and not continues_official_records


def decision_continues_after_empty_visible_result(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    if not empty_visible_result_text(previous_text):
        return False
    contract = response_contract_from_signal(decision)
    intent = str(contract.get("intent", "")).lower()
    claim = " ".join(
        str(decision.get(key, ""))
        for key in (
            "declared_action",
            "sent_to_gm",
            "active_goal",
            "selection_rationale",
            "last_action_result",
        )
    ).lower()
    if any(
        cue in intent
        for cue in (
            "clarify_empty_visible_result",
            "clarify_latest_visible_affordance",
            "inspect_latest_visible_affordance",
        )
    ):
        return False
    if any(
        cue in claim
        for cue in (
            "asks the keeper",
            "ask the keeper",
            "does not move deeper",
            "does not continue",
            "hold position",
            "holds position",
            "retreat",
            "missing result",
        )
    ):
        return False
    return any(
        cue in intent or cue in claim
        for cue in (
            "ground_floor_search",
            "upper_floor_search",
            "basement",
            "enter_",
            "search",
            "goes to",
            "steps",
            "moves",
            "deeper",
        )
    )


def decision_assumes_unattempted_chapel_search(
    decision: Dict[str, Any], prior_decisions: List[Dict[str, Any]]
) -> bool:
    claim = player_decision_text(decision)
    assumes_retry = any(
        cue in claim
        for cue in (
            "chapel search did not give",
            "failed chapel search",
            "first chapel search failed",
            "retry_chapel",
            "recheck chapel surfaces",
        )
    )
    if not assumes_retry:
        return False
    prior = " ".join(player_decision_text(row) for row in prior_decisions)
    if "chapel" not in prior:
        return True
    return not any(
        cue in prior
        for cue in (
            "goes to the chapel",
            "drives to the chapel",
            "arrives at the chapel",
            "inspect the chapel",
            "search the chapel",
            "investigate_chapel",
            "chapel of contemplation",
        )
    )


def previous_has_unearned_chapel_direction_menu(text: str) -> bool:
    return "chapel of contemplation" in text.lower() and english_fragmented_paper_trail_direction_menu(text)


def decision_follows_unearned_chapel_direction(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    if not previous_has_unearned_chapel_direction_menu(previous_text):
        return False
    intent = str(response_contract_from_signal(decision).get("intent", "")).lower()
    claim = player_selected_action_text(decision)
    goes_to_chapel = "chapel" in intent or "chapel of contemplation" in claim or "goes to the chapel" in claim
    if not goes_to_chapel:
        return False
    earned_context = any(
        cue in previous_text.lower()
        for cue in (
            "michael thomas",
            "closed in 1912",
            "raid on the chapel",
            "affidavits",
            "file trail shows",
            "records show",
        )
    )
    return not earned_context


def previous_has_newspaper_house_history_success(text: str) -> bool:
    lowered = text.lower()
    source_cues = ("newspaper", "globe", "clipping", "clippings", "morgue", "newsprint")
    if not any(roll_block_success(block) for block in roll_blocks(text)):
        return False
    if not any(cue in lowered for cue in source_cues):
        return False
    roll_source_bound_success = any(
        roll_block_success(block) and any(cue in block.lower() for cue in source_cues)
        for block in roll_blocks(text)
    )
    actual_newspaper_context = any(
        cue in lowered
        for cue in (
            "at the globe",
            "the globe receives",
            "globe offices",
            "clippings morgue",
            "morgue files",
            "morgue attendant",
            "newspaper morgue",
            "newspaper archive search",
            "at the newspaper archive",
            "in the newspaper archive",
            "inside the newspaper archive",
            "morgue search",
        )
    )
    source_bound_success = roll_source_bound_success or actual_newspaper_context
    if not source_bound_success:
        return False
    return any(
        cue in lowered
        for cue in (
            "house-history pattern",
            "deaths",
            "crippling accidents",
            "illness",
            "suicide",
            "macario family",
            "public history",
            "printed",
        )
    )


def decision_returns_to_hall_after_newspaper_success(
    decision: Dict[str, Any], previous_text: str
) -> bool:
    if not previous_has_newspaper_house_history_success(previous_text):
        return False
    intent = str(response_contract_from_signal(decision).get("intent", "")).lower()
    claim = player_selected_action_text(decision)
    return "continue_hall_records" in intent or "hall of records" in claim or "does not leave the hall" in claim


def invalid_player_action_declaration_reason(decision: Dict[str, Any]) -> str | None:
    declared = str(decision.get("declared_action", decision.get("sent_to_gm", ""))).lower()
    if not declared.strip():
        return "empty declared action"
    conditional_markers = (
        declared.count("if it was")
        + declared.count("if it is")
        + declared.count("如果是")
    )
    if conditional_markers >= 2:
        return "conditional placeholder action instead of a single executable declaration"
    if (
        "most concrete visible affordance" in declared
        and ("if it was" in declared or "if it is" in declared)
    ):
        return "conditional placeholder action instead of a single executable declaration"
    return None


def fallback_intent(decision: Dict[str, Any]) -> bool:
    contract = response_contract_from_signal(decision)
    intent = str(contract.get("intent", "")).strip()
    return intent in {
        "inspect_latest_visible_affordance",
        "clarify_latest_visible_affordance",
    }


def build_turns(run_dir: Path) -> List[Dict[str, Any]]:
    turns: List[Dict[str, Any]] = []
    for idx, path in enumerate(turn_files(run_dir), start=1):
        rows = list(read_jsonl(path))
        text = visible_text_from_jsonl(path)
        explain = explain_text_for_turn(path)
        stderr = stderr_text_for_turn(path)
        direct_transitions = direct_scene_transition_count(path)
        check_count = count_event(explain, "CheckResolved")
        dice_count = count_event(explain, "DiceRolled")
        scene_count = max(count_event(explain, "SceneTransitioned"), direct_transitions)
        rolls = roll_blocks(text)
        fallback_check_count = len(rolls) if check_count == 0 else 0
        unresolved = unresolved_mechanics(explain)
        blocked_source = blocked_missing_source(explain)
        narrated_state = narrated_position_change(text)
        has_commit = committed_progress_or_state(
            explain,
            check_count + fallback_check_count,
            scene_count,
        )
        turns.append(
            {
                "turn": turn_file_number(path) or idx,
                "path": path,
                "player_input": input_text_for_turn(path),
                "text": text,
                "explain": explain,
                "rolls": rolls,
                "checks": check_count + fallback_check_count,
                "resolved_checks": 0 if blocked_source else check_count + fallback_check_count,
                "dice": dice_count + fallback_check_count,
                "scene_transitions": scene_count,
                "pending": "Pending" in explain or "pending" in explain.lower(),
                "unresolved_mechanics": unresolved,
                "blocked_missing_source": blocked_source,
                "narrated_state_without_commit": narrated_state and not has_commit,
                "success_without_information": success_without_concrete_information(text),
                "failure_leaks_positive_hidden_lead": failure_leaks_positive_hidden_lead(text),
                "source_address_contradiction": source_limited_address_contradiction(text),
                "scene_transition_without_visible_situation": scene_transition_without_visible_situation(
                    text, scene_count
                ),
                "constitution_hits": constitution_hits(text)
                + presentation_gate_hits(explain, stderr, rows),
            }
        )
    return turns


def visible_establishes_deep_basement_or_crawlspace(text: str) -> bool:
    lowered = text.lower()
    crawlspace = any(
        cue in lowered
        for cue in (
            "crawl-space",
            "crawl space",
            "crawlspace",
            "cramped passage",
            "crawl-space passage",
            "concealed crawl-space opening",
        )
    )
    basement = any(cue in lowered for cue in ("basement", "cellar"))
    deep_basement = basement and any(
        cue in lowered
        for cue in (
            "foot of the stairs",
            "reach the bottom",
            "reach the basement floor",
            "bottom of the stairs",
            "scrape marks",
            "concealed section",
            "concealed crawl",
            "disturbed section",
        )
    )
    return crawlspace or deep_basement


def visible_explicitly_retreats_to_ground_floor(text: str) -> bool:
    lowered = text.lower()
    return any(
        cue in lowered
        for cue in (
            "back up the stairs",
            "backs up the stairs",
            "back to the ground floor",
            "returns to the ground floor",
            "retreats to the ground floor",
            "retreats back upstairs",
            "withdraws from the basement",
            "withdraws from the crawl",
        )
    )


def visible_resets_to_unopened_basement_door(text: str) -> bool:
    lowered = text.lower()
    if "basement" not in lowered and "cellar" not in lowered:
        return False
    if "door" not in lowered:
        return False
    shallow_anchor = any(
        cue in lowered
        for cue in (
            "ground-floor hall",
            "ground floor hall",
            "under the staircase",
            "beside the stair structure",
            "basement door",
            "cellar door",
        )
    )
    if not shallow_anchor:
        return False
    unopened_markers = any(
        cue in lowered
        for cue in (
            "what lies beyond",
            "whether it is locked",
            "closed interior door",
            "plain, closed",
            "until she closes the distance",
            "opening it or going below",
        )
    )
    return unopened_markers and not visible_explicitly_retreats_to_ground_floor(text)


def input_requests_ground_floor_sweep(text: str) -> bool:
    lowered = text.lower()
    return (
        ("ground floor" in lowered or "ground-floor" in lowered)
        and ("entry" in lowered or "hall" in lowered or "one room" in lowered)
        and any(
            cue in lowered
            for cue in (
                "search",
                "checks",
                "photograph",
                "marks doors",
                "clockwise",
            )
        )
    )


def visible_reframes_ground_floor_sweep_as_upper_floor_entry(text: str) -> bool:
    lowered = text.lower()
    if not any(cue in lowered for cue in ("楼上入口", "楼梯平台", "upper landing", "upper-floor entry")):
        return False
    return any(cue in lowered for cue in ("楼梯", "stair", "landing"))


def input_requests_bedroom_threshold_probe(text: str) -> bool:
    lowered = text.lower()
    return (
        re.search(r"\b(?:bedroom|bed|wardrobe)\b", lowered) is not None
        and any(cue in lowered for cue in ("threshold", "doorway", "door frame", "doorframe"))
        and any(cue in lowered for cue in ("landing", "retreat", "back toward"))
    )


def visible_reframes_bedroom_probe_as_exterior_entry(text: str) -> bool:
    lowered = text.lower()
    exterior_entry_cues = (
        "外门",
        "门厅",
        "门槛内侧",
        "exterior door",
        "entry hall",
        "just inside the door",
    )
    return any(cue in lowered for cue in exterior_entry_cues) and not (
        re.search(r"\b(?:bedroom|bed|wardrobe)\b", lowered) is not None
        or any(cue in lowered for cue in ("楼上房间", "床", "衣柜"))
    )


def deterministic_state_reset_turns(turns: List[Dict[str, Any]]) -> List[int]:
    resets: List[int] = []
    deep_basement_seen = False
    for turn in turns:
        text = str(turn.get("text", ""))
        player_input = str(turn.get("player_input", ""))
        if input_requests_ground_floor_sweep(
            player_input
        ) and visible_reframes_ground_floor_sweep_as_upper_floor_entry(text):
            resets.append(int(turn["turn"]))
        if input_requests_bedroom_threshold_probe(
            player_input
        ) and visible_reframes_bedroom_probe_as_exterior_entry(text):
            resets.append(int(turn["turn"]))
        if deep_basement_seen and visible_resets_to_unopened_basement_door(text):
            resets.append(int(turn["turn"]))
        if visible_explicitly_retreats_to_ground_floor(text):
            deep_basement_seen = False
        elif visible_establishes_deep_basement_or_crawlspace(text):
            deep_basement_seen = True
    return resets


REINTRODUCED_CLUES: Tuple[Tuple[str, Tuple[str, ...]], ...] = (
    ("Chapel of Contemplation", ("chapel of contemplation",)),
)

KNOWN_CLUE_DISCOVERY_CUES = (
    "then the light catches",
    "light catches something",
    "you spot",
    "you find",
    "you discover",
    "reveals",
    "resolves plainly",
    "words have been carved",
    "carved into the surface",
    "first catches",
)

KNOWN_CLUE_ACKNOWLEDGEMENT_CUES = (
    "already",
    "as seen",
    "safely in your notebook",
    "in your notebook",
    "recorded",
    "documented",
    "transcribed",
    "transcribes",
    "the visible carved words",
)


def known_clue_reintroduced_as_new(text: str, variants: Tuple[str, ...]) -> bool:
    lowered = visible_cue_text(text)
    if not any(variant in lowered for variant in variants):
        return False
    if any(cue in lowered for cue in KNOWN_CLUE_ACKNOWLEDGEMENT_CUES):
        return False
    if "chapel of contemplation" in variants:
        carved_wall_context = any(
            cue in lowered
            for cue in (
                "carved",
                "crawl-space",
                "crawl space",
                "under-space",
                "crawl-space wall",
                "wall inside",
                "into the surface",
            )
        )
        if not carved_wall_context:
            return False
    return any(cue in lowered for cue in KNOWN_CLUE_DISCOVERY_CUES)


def deterministic_reintroduced_known_clue_turns(turns: List[Dict[str, Any]]) -> List[int]:
    seen: set[str] = set()
    flagged: List[int] = []
    for turn in turns:
        text = str(turn.get("text", ""))
        lowered = visible_cue_text(text)
        for label, variants in REINTRODUCED_CLUES:
            if not any(variant in lowered for variant in variants):
                continue
            if label in seen and known_clue_reintroduced_as_new(text, variants):
                flagged.append(int(turn["turn"]))
            seen.add(label)
    return flagged


def judge_j1(turns: List[Dict[str, Any]], run_dir: Path) -> Dict[str, Any]:
    has_signals, signals = read_player_sim_signals(run_dir)
    amnesia = [s for s in signals if str(s.get("kind", "")).upper() == "AMNESIA"]
    stuck = [s for s in signals if str(s.get("kind", "")).upper() == "STUCK"]
    state_resets = deterministic_state_reset_turns(turns)
    reintroduced_clues = deterministic_reintroduced_known_clue_turns(turns)
    metrics = {
        "turns": len(turns),
        "amnesia_signals": len(amnesia),
        "stuck_signals": len(stuck),
        "state_reset_turns": len(state_resets),
        "reintroduced_known_clue_turns": len(reintroduced_clues),
        "signal_artifact_present": has_signals,
    }
    if not has_signals:
        return judge(
            "J1",
            "memory-continuity",
            "RED",
            metrics,
            ["signals.jsonl missing; J1 cannot be marked green without player-sim continuity signals"],
        )
    if amnesia:
        return judge(
            "J1",
            "memory-continuity",
            "RED",
            metrics,
            [f"player-sim raised {len(amnesia)} AMNESIA signal(s): GM forgot an established fact/location"],
        )
    if state_resets:
        return judge(
            "J1",
            "memory-continuity",
            "RED",
            metrics,
            [
                "deterministic state reset detected after deep basement/crawl-space progress: turns "
                + ", ".join(str(turn) for turn in state_resets[:12])
            ],
        )
    if reintroduced_clues:
        return judge(
            "J1",
            "memory-continuity",
            "RED",
            metrics,
            [
                "known concrete clue reintroduced as a new discovery: turns "
                + ", ".join(str(turn) for turn in reintroduced_clues[:12])
            ],
        )
    return judge("J1", "memory-continuity", "GREEN", metrics, ["no AMNESIA signal observed"])


def judge_j2(turns: List[Dict[str, Any]]) -> Dict[str, Any]:
    consequential = 0
    pending = 0
    narrated_without_commit = [
        turn for turn in turns if turn.get("narrated_state_without_commit", False)
    ]
    success_without_info = [
        turn for turn in turns if turn.get("success_without_information", False)
    ]
    failure_leaks = [
        turn for turn in turns if turn.get("failure_leaks_positive_hidden_lead", False)
    ]
    transitions_without_visible_situation = [
        turn for turn in turns if turn.get("scene_transition_without_visible_situation", False)
    ]
    source_address_contradictions = [
        turn for turn in turns if turn.get("source_address_contradiction", False)
    ]
    for turn in turns:
        body = turn["explain"]
        has_progress = any(event in body for event in PROGRESS_EVENTS)
        has_resolved_check = turn["checks"] > 0 and not turn.get("unresolved_mechanics", False)
        if has_resolved_check or turn["scene_transitions"] > 0 or has_progress:
            consequential += 1
        if turn["pending"]:
            pending += 1
    ratio = consequential / len(turns) if turns else 0.0
    threshold = {"min_ratio": 0.34}
    status = (
        "GREEN"
        if ratio >= threshold["min_ratio"]
        and pending == 0
        and not narrated_without_commit
        and not success_without_info
        and not failure_leaks
        and not source_address_contradictions
        and not transitions_without_visible_situation
        else "RED"
    )
    evidence = [f"{consequential}/{len(turns)} turns were consequential (ratio={ratio:.3f})"]
    if pending:
        evidence.append(f"{pending} turn(s) contained pending markers")
    if narrated_without_commit:
        evidence.append(
            "narrated state/location change without committed state: turns "
            + ", ".join(str(turn["turn"]) for turn in narrated_without_commit[:12])
        )
    if success_without_info:
        evidence.append(
            "successful check without concrete information: turns "
            + ", ".join(str(turn["turn"]) for turn in success_without_info[:12])
        )
    if failure_leaks:
        evidence.append(
            "failed check leaked positive hidden lead: turns "
            + ", ".join(str(turn["turn"]) for turn in failure_leaks[:12])
        )
    if source_address_contradictions:
        evidence.append(
            "source-limited address contradiction: turns "
            + ", ".join(str(turn["turn"]) for turn in source_address_contradictions[:12])
        )
    if transitions_without_visible_situation:
        evidence.append(
            "scene transition without new player-visible situation: turns "
            + ", ".join(str(turn["turn"]) for turn in transitions_without_visible_situation[:12])
        )
    return judge(
        "J2",
        "consequence-agency",
        status,
        {
            "turns": len(turns),
            "consequential_turns": consequential,
            "consequential_ratio": round(ratio, 3),
            "pending_unresolved_turns": pending,
            "narrated_state_without_commit_turns": len(narrated_without_commit),
            "success_without_information_turns": len(success_without_info),
            "failure_leak_turns": len(failure_leaks),
            "source_address_contradiction_turns": len(source_address_contradictions),
            "scene_transition_without_visible_situation_turns": len(transitions_without_visible_situation),
        },
        evidence,
        threshold,
    )


def judge_j3(turns: List[Dict[str, Any]], run_dir: Path) -> Dict[str, Any]:
    transition_turns = [turn["scene_transitions"] > 0 for turn in turns]
    transitions = sum(turn["scene_transitions"] for turn in turns)
    max_frozen = max_consecutive_false(transition_turns)
    rate = transitions / len(turns) if turns else 0.0
    prose_repeat = max_consecutive_repeat([normalized_prose_key(t["text"]) for t in turns])
    has_signals, signals = read_player_sim_signals(run_dir)
    stuck = sum(1 for s in signals if str(s.get("kind", "")).upper() == "STUCK") if has_signals else 0
    requires_20_turn_completion = (
        "coc_haunting" in run_dir.name.lower() and "20turn" in run_dir.name.lower()
    )
    early_incomplete_coc = (
        requires_20_turn_completion
        and len(turns) < 20
        and not coc_corbitt_actually_resolved(turns)
    )
    threshold = {"max_frozen": 15, "min_rate": 0.05, "max_prose_repeat": 4}
    status = (
        "GREEN"
        if rate >= threshold["min_rate"]
        and max_frozen <= threshold["max_frozen"]
        and prose_repeat <= threshold["max_prose_repeat"]
        and stuck == 0
        and not early_incomplete_coc
        else "RED"
    )
    evidence = [
        f"{transitions} transitions / {len(turns)} turns, max frozen run {max_frozen}, max prose repeat {prose_repeat}"
    ]
    if early_incomplete_coc:
        evidence.append(
            "CoC 20-turn autoplay ended before 20 turns without evidence that Walter Corbitt was resolved"
        )
    if stuck:
        evidence.append(f"player-sim raised {stuck} STUCK signal(s)")
    return judge(
        "J3",
        "progression",
        status,
        {
            "turns": len(turns),
            "scene_transitions": transitions,
            "transition_rate": round(rate, 3),
            "max_frozen_run": max_frozen,
            "max_prose_repeat": prose_repeat,
        },
        evidence,
        threshold,
    )


def coc_corbitt_actually_resolved(turns: List[Dict[str, Any]]) -> bool:
    text = "\n".join(turn["text"] for turn in turns).lower()
    if not any(
        cue in text
        for cue in (
            "walter corbitt",
            "corbitt's body",
            "corbitt’s body",
            "corbitt corpse",
            "corbitt himself",
            "body of corbitt",
            "corpse of corbitt",
        )
    ):
        return False
    if any(
        cue in text
        for cue in (
            "nothing dramatic collapses",
            "nothing collapses",
            "does not collapse",
            "doesn't collapse",
        )
    ):
        return False
    return any(
        cue in text
        for cue in (
            "destroyed",
            "no longer moving",
            "falls still",
            "collapses",
            "case is over",
            "investigation is complete",
        )
    )


def judge_j4(turns: List[Dict[str, Any]]) -> Dict[str, Any]:
    checks = sum(turn.get("resolved_checks", turn["checks"]) for turn in turns)
    dice = sum(turn["dice"] for turn in turns)
    check_turns = [
        turn
        for turn in turns
        if turn.get("resolved_checks", turn["checks"]) > 0
        and not turn.get("blocked_missing_source", False)
    ]
    def surfaced_roll_count(turn: Dict[str, Any]) -> int:
        return sum(1 for block in turn["rolls"] if roll_block_surfaces_result(block))

    under_tagged = [
        turn
        for turn in check_turns
        if surfaced_roll_count(turn) < turn.get("resolved_checks", turn["checks"])
    ]
    surfaced = [turn for turn in check_turns if turn not in under_tagged]
    orphan_pending = sum(1 for turn in turns if turn["pending"] and turn["checks"] == 0)
    unbound = [
        turn
        for turn in check_turns
        if "awaiting_binding" in turn["explain"]
        or "no source-backed target" in turn["explain"].lower()
    ]
    blocked_source = sum(1 for turn in turns if turn.get("blocked_missing_source", False))
    coverage = len(surfaced) / len(check_turns) if check_turns else 1.0
    threshold = {"min_coverage": 1.0, "max_orphan": 1, "max_unbound": 0}
    status = (
        "GREEN"
        if coverage >= threshold["min_coverage"]
        and orphan_pending <= threshold["max_orphan"]
        and len(unbound) <= threshold["max_unbound"]
        and not under_tagged
        else "RED"
    )
    evidence = [
        f"{len(surfaced)}/{len(check_turns)} check-turns surfaced every bound [roll] outcome (coverage={coverage:.3f}), orphans={orphan_pending}, unbound={len(unbound)}, under-tagged={len(under_tagged)}"
    ]
    if under_tagged:
        evidence.append(
            "check turn(s) with fewer auditable [roll] blocks than resolved checks: "
            + ", ".join(str(turn["turn"]) for turn in under_tagged[:12])
        )
    if unbound:
        evidence.append(
            "unbound CheckResolved turn(s): "
            + ", ".join(str(turn["turn"]) for turn in unbound[:12])
        )
    return judge(
        "J4",
        "check-narration-coherence",
        status,
        {
            "checks_resolved": checks,
            "dice_rolled": dice,
            "check_turns": len(check_turns),
            "surfaced_turns": len(surfaced),
            "narration_coverage": round(coverage, 3),
            "orphan_pending_turns": orphan_pending,
            "unbound_check_turns": len(unbound),
            "under_tagged_check_turns": len(under_tagged),
            "blocked_missing_source_turns": blocked_source,
            "committed_checks": checks,
        },
        evidence,
        threshold,
    )


def judge_q4(
    turns: List[Dict[str, Any]],
    semantic_findings: List[Dict[str, Any]] | None = None,
) -> Dict[str, Any]:
    hits: List[Tuple[int, str]] = []
    for turn in turns:
        for hit in turn["constitution_hits"]:
            hits.append((turn["turn"], hit))
    semantic_hits = semantic_findings_for_categories(
        semantic_findings or [],
        SEMANTIC_Q4_CATEGORIES,
    )
    status = "GREEN" if not hits and not semantic_hits else "RED"
    evidence = ["no explicit option menu or raw content dump detected"]
    if hits or semantic_hits:
        evidence = [f"turn {turn}: {hit}" for turn, hit in hits[:12]]
        evidence.extend(semantic_finding_evidence_line(finding) for finding in semantic_hits[:12])
        if len(hits) > 12:
            evidence.append(f"... {len(hits) - 12} more hit(s)")
        if len(semantic_hits) > 12:
            evidence.append(f"... {len(semantic_hits) - 12} more semantic hit(s)")
    return judge(
        "Q4",
        "no-menu-no-dump-constitution",
        status,
        {
            "turns": len(turns),
            "menu_hits": sum(1 for _, hit in hits if "menu" in hit or "cue" in hit),
            "dump_hits": sum(1 for _, hit in hits if "dump" in hit),
            "gate_block_hits": sum(1 for _, hit in hits if hit == "presentation_gate_block"),
            "semantic_hits": len(semantic_hits),
            "total_hits": len(hits) + len(semantic_hits),
        },
        evidence,
        {"max_hits": 0},
    )


def judge_p1(
    run_dir: Path,
    semantic_findings: List[Dict[str, Any]] | None = None,
) -> Dict[str, Any]:
    has_signals, signals = read_player_sim_signals(run_dir)
    decisions = player_decision_signals(signals)
    turns = build_turns(run_dir)
    turns_by_number = {turn["turn"]: turn for turn in turns}
    semantic_player_findings = semantic_findings_for_categories(
        semantic_findings or [],
        SEMANTIC_P1_CATEGORIES,
    )
    incomplete: List[Tuple[Any, List[str]]] = []
    hidden_leaks: List[Tuple[Any, List[str]]] = []
    inconsistent: List[Tuple[Any, str]] = []
    invalid_actions: List[Tuple[Any, str]] = []
    fallback_loop: List[Any] = []
    prior_decisions: List[Dict[str, Any]] = []
    consecutive_fallbacks: List[Any] = []
    for decision in decisions:
        gaps = player_decision_protocol_gaps(decision)
        if gaps:
            incomplete.append((decision.get("turn", "?"), gaps))
            prior_decisions.append(decision)
            continue
        invalid_reason = invalid_player_action_declaration_reason(decision)
        if invalid_reason:
            invalid_actions.append((decision.get("turn", "?"), invalid_reason))
        if fallback_intent(decision):
            consecutive_fallbacks.append(decision.get("turn", "?"))
        else:
            if len(consecutive_fallbacks) >= 2:
                fallback_loop.extend(consecutive_fallbacks)
            consecutive_fallbacks = []
        try:
            turn_no = int(decision.get("turn", 0))
        except (TypeError, ValueError):
            prior_decisions.append(decision)
            continue
        visible_history = visible_history_before_turn(run_dir, turns, turn_no, decision)
        leaks = decision_unseen_protected_terms(decision, visible_history)
        if leaks:
            hidden_leaks.append((decision.get("turn", "?"), leaks))
        previous = turns_by_number.get(turn_no - 1)
        if previous and decision_jumps_back_to_chapel_from_house_interior(decision, previous["text"]):
            inconsistent.append(
                (
                    decision.get("turn", "?"),
                    "jumps back to chapel from house interior",
                )
            )
        elif previous and decision_contradicts_previous_failed_result(decision, previous["text"]):
            inconsistent.append((decision.get("turn", "?"), "contradicts previous failed result"))
        elif previous and decision_changes_floor_after_failed_basement_stair_position(
            decision, previous["text"]
        ):
            inconsistent.append(
                (
                    decision.get("turn", "?"),
                    "changes floor after failed basement stair position",
                )
            )
        elif previous and decision_ignores_clarified_basement_route_priority(
            decision, previous["text"]
        ):
            inconsistent.append(
                (
                    decision.get("turn", "?"),
                    "ignores clarified basement route priority",
                )
            )
        elif previous and decision_ignores_previous_actionable_success_lead(decision, previous["text"]):
            inconsistent.append((decision.get("turn", "?"), "ignores actionable success lead"))
        elif previous and decision_falls_back_after_concrete_visible_affordance(
            decision, previous["text"]
        ):
            inconsistent.append(
                (
                    decision.get("turn", "?"),
                    "fallback after concrete visible affordance",
                )
            )
        elif previous and decision_invents_moving_bedroom_threat(
            decision, previous["text"]
        ):
            inconsistent.append(
                (
                    decision.get("turn", "?"),
                    "invents moving-bedroom threat from prior visible text",
                )
            )
        elif previous and decision_treats_nonactive_blade_reference_as_current_threat(
            decision, previous["text"], visible_history
        ):
            inconsistent.append(
                (
                    decision.get("turn", "?"),
                    "treats historical knife reference as current blade threat",
                )
            )
        elif previous and decision_follows_unearned_chapel_direction(decision, previous["text"]):
            inconsistent.append(
                (
                    decision.get("turn", "?"),
                    "follows unearned Chapel direction menu",
                )
            )
        elif previous and decision_returns_to_hall_after_newspaper_success(decision, previous["text"]):
            inconsistent.append(
                (
                    decision.get("turn", "?"),
                    "returns to Hall after newspaper house-history success",
                )
            )
        elif previous and decision_claims_crawl_space_chapel_carving_without_visible_evidence(
            decision, previous["text"]
        ):
            inconsistent.append(
                (
                    decision.get("turn", "?"),
                    "claims crawl-space carving without visible evidence",
                )
            )
        elif previous and decision_continues_crawl_space_after_house_exit(
            decision, previous["text"]
        ):
            inconsistent.append(
                (
                    decision.get("turn", "?"),
                    "continues crawl-space after exiting house",
                )
            )
        elif previous and decision_changes_floor_from_active_crawl_space_position(
            decision, previous["text"]
        ):
            inconsistent.append(
                (
                    decision.get("turn", "?"),
                    "changes floor while still in crawl-space position",
                )
            )
        elif previous and decision_jumps_to_chapel_from_active_crawl_space_position(
            decision, previous["text"]
        ):
            inconsistent.append(
                (
                    decision.get("turn", "?"),
                    "jumps to chapel while still in crawl-space position",
                )
            )
        elif previous and decision_denies_visible_chapel_payoff(
            decision, previous["text"]
        ):
            inconsistent.append(
                (
                    decision.get("turn", "?"),
                    "denies visible chapel payoff",
                )
            )
        elif previous and decision_ignores_newspaper_official_records_lead(
            decision, previous["text"]
        ):
            inconsistent.append(
                (
                    decision.get("turn", "?"),
                    "ignores newspaper official-records lead",
                )
            )
        elif previous and decision_leaves_official_boundary_map_for_house(
            decision, previous["text"]
        ):
            inconsistent.append(
                (
                    decision.get("turn", "?"),
                    "leaves official-records boundary map for house",
                )
            )
        elif previous and decision_leaves_newspaper_access_gate_unresolved(
            decision, previous["text"]
        ):
            inconsistent.append(
                (
                    decision.get("turn", "?"),
                    "leaves newspaper access gate unresolved",
                )
            )
        elif previous and decision_abandons_clarified_basement_section(decision, previous["text"]):
            inconsistent.append(
                (
                    decision.get("turn", "?"),
                    "abandons clarified basement section for another floor",
                )
            )
        elif previous and decision_claims_basement_return_route_as_concrete_lead(
            decision, previous["text"]
        ):
            inconsistent.append(
                (
                    decision.get("turn", "?"),
                    "claims basement return route as concrete lead",
                )
            )
        elif previous and decision_claims_speculative_basement_detail_as_concrete_lead(
            decision, previous["text"]
        ):
            inconsistent.append(
                (
                    decision.get("turn", "?"),
                    "claims speculative basement detail as concrete lead",
                )
            )
        elif previous and decision_claims_closed_exterior_door_already_open(
            decision, previous["text"]
        ):
            inconsistent.append(
                (
                    decision.get("turn", "?"),
                    "claims closed exterior door is already open",
                )
            )
        elif previous and decision_unconditionally_unlocks_unproven_exterior_door(
            decision, previous["text"]
        ):
            inconsistent.append(
                (
                    decision.get("turn", "?"),
                    "unconditionally unlocks unproven exterior door",
                )
            )
        elif previous and decision_claims_safest_exterior_after_failed_safety_read(
            decision, previous["text"]
        ):
            inconsistent.append(
                (
                    decision.get("turn", "?"),
                    "claims safest exterior entry after failed safety read",
                )
            )
        elif previous and decision_reenters_exterior_door_after_interior_ground_floor_position(
            decision, previous["text"]
        ):
            inconsistent.append(
                (
                    decision.get("turn", "?"),
                    "re-enters exterior door after interior ground-floor position",
                )
            )
        elif previous and decision_claims_refusal_after_successful_access(decision, previous["text"]):
            inconsistent.append((decision.get("turn", "?"), "claims refusal after successful access"))
        elif previous and decision_leaves_hall_records_after_success_without_specific_facts(
            decision, previous["text"]
        ):
            inconsistent.append(
                (
                    decision.get("turn", "?"),
                    "leaves Hall Records after success without specific facts",
                )
            )
        elif previous and decision_leaves_official_records_after_access_without_contents(
            decision, previous["text"]
        ):
            inconsistent.append(
                (
                    decision.get("turn", "?"),
                    "leaves official records after access without contents",
                )
            )
        elif previous and decision_continues_after_empty_visible_result(
            decision, previous["text"]
        ):
            inconsistent.append(
                (
                    decision.get("turn", "?"),
                    "continues after empty visible result",
                )
            )
        if decision_assumes_unattempted_chapel_search(decision, prior_decisions):
            inconsistent.append(
                (
                    decision.get("turn", "?"),
                    "assumes a chapel search before any recorded chapel attempt",
                )
            )
        prior_decisions.append(decision)
    if len(consecutive_fallbacks) >= 2:
        fallback_loop.extend(consecutive_fallbacks)

    metrics = {
        "signal_artifact_present": has_signals,
        "player_decision_turns": len(decisions),
        "incomplete_decisions": len(incomplete),
        "hidden_unseen_term_decisions": len(hidden_leaks),
        "inconsistent_decisions": len(inconsistent),
        "invalid_action_declarations": len(invalid_actions),
        "fallback_loop_decisions": len(fallback_loop),
        "semantic_findings": len(semantic_player_findings),
    }
    if not has_signals:
        return judge(
            "P1",
            "player-simulator-protocol",
            "RED",
            metrics,
            ["signals.jsonl missing; player simulator deliberation cannot be audited"],
        )
    if not decisions:
        return judge(
            "P1",
            "player-simulator-protocol",
            "RED",
            metrics,
            ["no PLAYER_DECISION signal recorded; runner may be replaying a script instead of simulating a player"],
        )
    if incomplete:
        evidence = [
            f"turn {turn} missing/violating player loop fields: {', '.join(gaps)}"
            for turn, gaps in incomplete[:12]
        ]
        if len(incomplete) > 12:
            evidence.append(f"... {len(incomplete) - 12} more incomplete decision(s)")
        return judge(
            "P1",
            "player-simulator-protocol",
            "RED",
            metrics,
            evidence,
            {"required": "read -> perceive -> goal/hypotheses -> result -> risk/resources/questions -> candidates -> persona choice -> contract -> declared action only"},
        )
    if hidden_leaks:
        evidence = [
            f"turn {turn} player decision uses hidden/unseen term(s): {', '.join(leaks)}"
            for turn, leaks in hidden_leaks[:12]
        ]
        if len(hidden_leaks) > 12:
            evidence.append(f"... {len(hidden_leaks) - 12} more hidden term leak(s)")
        evidence.extend(
            f"turn {turn} player decision {reason}"
            for turn, reason in inconsistent[:12]
        )
        return judge(
            "P1",
            "player-simulator-protocol",
            "RED",
            metrics,
            evidence,
            {"required": "player decisions may only cite scenario terms already present in player-visible history"},
        )
    if invalid_actions:
        evidence = [
            f"turn {turn} player decision has invalid declared action: {reason}"
            for turn, reason in invalid_actions[:12]
        ]
        if len(invalid_actions) > 12:
            evidence.append(f"... {len(invalid_actions) - 12} more invalid action declaration(s)")
        evidence.extend(
            f"turn {turn} player decision {reason}"
            for turn, reason in inconsistent[:12]
        )
        return judge(
            "P1",
            "player-simulator-protocol",
            "RED",
            metrics,
            evidence,
            {"required": "sent_to_gm must be one concrete executable player declaration, not a conditional template"},
        )
    if fallback_loop:
        evidence = [
            "player simulator repeats fallback intent without extracting a concrete next action: turns "
            + ", ".join(str(turn) for turn in fallback_loop[:12])
        ]
        if len(fallback_loop) > 12:
            evidence.append(f"... {len(fallback_loop) - 12} more fallback-loop decision(s)")
        return judge(
            "P1",
            "player-simulator-protocol",
            "RED",
            metrics,
            evidence,
            {"required": "fallback clarification may happen once; repeated fallback means the player is no longer adapting"},
        )
    if inconsistent:
        evidence = [
            f"turn {turn} player decision {reason}"
            for turn, reason in inconsistent[:12]
        ]
        if len(inconsistent) > 12:
            evidence.append(f"... {len(inconsistent) - 12} more inconsistent decision(s)")
        return judge(
            "P1",
            "player-simulator-protocol",
            "RED",
            metrics,
            evidence,
            {"required": "player decisions must update beliefs from the previous player-visible GM result"},
        )
    if semantic_player_findings:
        evidence = [
            semantic_finding_evidence_line(finding)
            for finding in semantic_player_findings[:12]
        ]
        if len(semantic_player_findings) > 12:
            evidence.append(f"... {len(semantic_player_findings) - 12} more semantic player finding(s)")
        return judge(
            "P1",
            "player-simulator-protocol",
            "RED",
            metrics,
            evidence,
            {"required": "player decisions must be semantically grounded in player-visible GM replies"},
        )
    return judge(
        "P1",
        "player-simulator-protocol",
        "GREEN",
        metrics,
        ["all PLAYER_DECISION signals include deliberation fields and sent_to_gm equals declared_action"],
    )


def judge_m0(run_dir: Path) -> Dict[str, Any]:
    create_logs = sorted(run_dir.glob("create_character*.jsonl"))
    has_created_event = False
    for path in create_logs:
        for row in read_jsonl(path):
            if row.get("event") == "phase" and row.get("phase") == "character_created":
                has_created_event = True
                break
        if has_created_event:
            break

    sheet_path = run_dir / "character_sheet.md"
    sheet_text = sheet_path.read_text(errors="replace") if sheet_path.exists() else ""
    required_term_groups = (
        ("Name", "姓名", "Identity", "身份"),
        ("Core Stats", "核心属性", "属性"),
        ("Resources", "资源"),
        ("Key Skills", "关键技能", "技能"),
    )
    sheet_complete = bool(sheet_text.strip()) and all(
        any(term in sheet_text for term in group) for group in required_term_groups
    )

    status = "GREEN" if has_created_event and sheet_complete else "RED"
    evidence = []
    if has_created_event:
        evidence.append("create_character JSONL contains character_created event")
    else:
        evidence.append("no character_created event found in create_character*.jsonl")
    if sheet_complete:
        evidence.append("character_sheet.md contains identity, stats, resources, and key skills")
    else:
        evidence.append("character_sheet.md missing or incomplete")

    return judge(
        "M0",
        "character-setup-artifacts",
        status,
        {
            "create_character_logs": len(create_logs),
            "has_character_created_event": has_created_event,
            "has_character_sheet_md": sheet_path.exists(),
            "character_sheet_complete": sheet_complete,
        },
        evidence,
        {"required": "character_created event plus player-readable character_sheet.md"},
    )


def judge_m1(run_dir: Path) -> Dict[str, Any]:
    incomplete: List[str] = []
    fatal_errors: List[str] = []
    files = turn_files(run_dir)
    existing_jsonl = {path.name for path in files}
    for input_path in sorted(run_dir.glob("turn_*.input.txt")):
        match = re.match(r"^turn_(\d{2,4})\.input\.txt$", input_path.name)
        if not match:
            continue
        expected = f"turn_{match.group(1)}.jsonl"
        if expected not in existing_jsonl:
            incomplete.append(f"turn {int(match.group(1))}: missing turn output jsonl")
    for path in files:
        turn_no = turn_file_number(path) or len(files)
        rows = list(read_jsonl(path))
        has_delta = any(row.get("event") == "delta" and str(row.get("data", "")).strip() for row in rows)
        has_phase = any(row.get("event") == "phase" for row in rows)
        has_done = any(row.get("event") == "phase" and row.get("phase") == "done" for row in rows)
        stderr = stderr_text_for_turn(path)
        lowered_err = stderr.lower()
        if any(
            cue in lowered_err
            for cue in (
                "command timed out",
                "traceback",
                "panic",
                "fatal",
                "thread '",
            )
        ):
            first_line = next((line.strip() for line in stderr.splitlines() if line.strip()), "")
            fatal_errors.append(f"turn {turn_no}: {first_line}")
        if not has_delta:
            incomplete.append(f"turn {turn_no}: no visible delta output")
        elif has_phase and not has_done:
            incomplete.append(f"turn {turn_no}: phase stream did not reach done")

    status = "GREEN" if files and not fatal_errors and not incomplete else "RED"
    evidence: List[str] = []
    if not files and not incomplete:
        evidence.append("no turn_*.jsonl or t*.jsonl files found")
    if fatal_errors:
        evidence.extend(fatal_errors[:12])
    if incomplete:
        evidence.extend(incomplete[:12])
    if len(fatal_errors) + len(incomplete) > 12:
        evidence.append(f"... {len(fatal_errors) + len(incomplete) - 12} more runner artifact issue(s)")
    if not evidence:
        evidence.append("all turn artifacts contain visible GM output and no fatal driver/runtime stderr")

    return judge(
        "M1",
        "runner-turn-completion",
        status,
        {
            "turn_files": len(files),
            "fatal_error_turns": len(fatal_errors),
            "incomplete_turns": len(incomplete),
        },
        evidence,
        {"required": "each recorded turn has visible GM delta output and no fatal timeout/runtime stderr"},
    )


def judge(
    judge_id: str,
    name: str,
    status: str,
    metrics: Dict[str, Any],
    evidence: List[str],
    threshold: Dict[str, Any] | None = None,
) -> Dict[str, Any]:
    out: Dict[str, Any] = {
        "judge": judge_id,
        "name": name,
        "status": status,
        "blocking": True,
        "metrics": metrics,
        "evidence": evidence,
    }
    if threshold is not None:
        out["threshold"] = threshold
    return out


def judge_by_id(judges: List[Dict[str, Any]], judge_id: str) -> Dict[str, Any]:
    return next(judge_row for judge_row in judges if judge_row["judge"] == judge_id)


def judge_green(judges: List[Dict[str, Any]], judge_id: str) -> bool:
    return judge_by_id(judges, judge_id)["status"] == "GREEN"


def build_rubric(judges: List[Dict[str, Any]]) -> Dict[str, Any]:
    m1 = judge_by_id(judges, "M1")
    j1 = judge_by_id(judges, "J1")
    j2 = judge_by_id(judges, "J2")
    j3 = judge_by_id(judges, "J3")
    j2_pending = j2["metrics"].get("pending_unresolved_turns", 0)
    j2_success_without_info = j2["metrics"].get("success_without_information_turns", 0)
    rules_resolution_green = (
        judge_green(judges, "M1")
        and judge_green(judges, "J4")
        and j2_pending == 0
        and j2_success_without_info == 0
    )
    rules_resolution_fail_evidence: List[str] = []
    if not judge_green(judges, "M1"):
        rules_resolution_fail_evidence.append("M1 is RED: runner/turn artifacts are incomplete")
    if not judge_green(judges, "J4"):
        rules_resolution_fail_evidence.append("J4 is RED: rules/check results are not fully auditable")
    if j2_pending:
        rules_resolution_fail_evidence.append(
            f"J2 has unresolved pending consequences: {j2_pending} turn(s)"
        )
    if j2_success_without_info:
        rules_resolution_fail_evidence.append(
            f"J2 has successful checks without concrete information: {j2_success_without_info} turn(s)"
        )
    if not rules_resolution_fail_evidence:
        rules_resolution_fail_evidence.append("rules/check settlement evidence is incomplete")

    specs = {
        "rules_resolution": (
            rules_resolution_green,
            ["M1/J4 turn completion and check/narration coherence are GREEN and J2 has no unresolved settlement debt"],
            rules_resolution_fail_evidence,
        ),
        "continuity": (
            judge_green(judges, "J1")
            and j2["metrics"].get("narrated_state_without_commit_turns", 0) == 0,
            ["J1 has no AMNESIA and J2 has no narrated state without commit"],
            ["J1/J2 continuity evidence is RED or narrated state lacks ledger commit"],
        ),
        "responsiveness": (
            judge_green(judges, "J2") and judge_green(judges, "Q4"),
            ["J2 consequences landed and Q4 found no menu/dump"],
            ["J2 or Q4 is RED: GM did not cleanly respond to player agency"],
        ),
        "narrative_agency": (
            judge_green(judges, "J2") and judge_green(judges, "J3"),
            ["J2 consequence ratio and J3 progression are GREEN"],
            ["J2/J3 is RED: story did not reliably advance through player action"],
        ),
        "player_simulation": (
            judge_green(judges, "P1")
            and j1["metrics"].get("stuck_signals", 0) == 0
            and j3["metrics"].get("max_prose_repeat", 0) <= j3.get("threshold", {}).get("max_prose_repeat", 4),
            ["P1 player protocol is complete and no STUCK/prose loop evidence dominates"],
            ["P1/J1/J3 player-sim evidence is incomplete, stuck, or repetitive"],
        ),
        "language": (
            judge_green(judges, "Q4"),
            ["Q4 found no raw dump/menu-style language"],
            ["Q4 is RED: presentation language violates the constitution"],
        ),
    }

    dimensions = []
    for dim_id, label, weight in RUBRIC_WEIGHTS:
        passed, pass_evidence, fail_evidence = specs[dim_id]
        dimensions.append(
            {
                "id": dim_id,
                "label": label,
                "weight": weight,
                "score": weight if passed else 0,
                "evidence": pass_evidence if passed else fail_evidence,
            }
        )
    return {
        "total": sum(row["score"] for row in dimensions),
        "dimensions": dimensions,
    }


def evaluate_run_dir(
    run_dir: Path | str,
    label: str | None = None,
    semantic_critic_cmd: str | None = None,
) -> Dict[str, Any]:
    run_dir = Path(run_dir)
    turns = build_turns(run_dir)
    has_signals, signals = read_player_sim_signals(run_dir)
    decisions = player_decision_signals(signals)
    semantic = semantic_critic_result(run_dir, turns, decisions, semantic_critic_cmd)
    semantic_findings = semantic["findings"]
    semantic_required = env_truthy("TRPG_EVAL_REQUIRE_SEMANTIC_CRITIC")
    judges = [
        judge_m0(run_dir),
        judge_m1(run_dir),
        judge_j1(turns, run_dir),
        judge_j2(turns),
        judge_j3(turns, run_dir),
        judge_j4(turns),
        judge_semantic_required(semantic, semantic_required),
        judge_q4(turns, semantic_findings),
        judge_p1(run_dir, semantic_findings),
    ]
    red_count = sum(1 for j in judges if j["blocking"] and j["status"] == "RED")
    return {
        "session": extract_session_id(run_dir),
        "label": label or run_dir.name,
        "verdict": "FAIL" if red_count else "PASS",
        "red_count": red_count,
        "semantic_critic": {
            "required": semantic_required,
            "enabled": semantic["enabled"],
            "source": semantic["source"],
            "error": semantic["error"],
            "findings": len(semantic_findings),
        },
        "player_sim_signals": {
            "artifact_present": has_signals,
            "stuck": sum(1 for s in signals if str(s.get("kind", "")).upper() == "STUCK"),
            "amnesia": sum(1 for s in signals if str(s.get("kind", "")).upper() == "AMNESIA"),
            "player_decisions": len(decisions),
        },
        "rubric": build_rubric(judges),
        "judges": judges,
    }


def render_markdown(report: Dict[str, Any]) -> str:
    lines = [
        f"# EVAL RED-BOARD -- {report['label']}",
        "",
        f"- session: `{report['session']}`",
        f"- **VERDICT: {report['verdict']}** ({report['red_count']} blocking judge(s) RED)",
        f"- player-sim signals: artifact={report['player_sim_signals']['artifact_present']} "
        f"STUCK={report['player_sim_signals']['stuck']} "
        f"AMNESIA={report['player_sim_signals']['amnesia']} "
        f"PLAYER_DECISION={report['player_sim_signals']['player_decisions']}",
        f"- semantic critic: required={report.get('semantic_critic', {}).get('required', False)} "
        f"enabled={report.get('semantic_critic', {}).get('enabled', False)} "
        f"findings={report.get('semantic_critic', {}).get('findings', 0)} "
        f"error={report.get('semantic_critic', {}).get('error') or 'none'}",
        f"- rubric total: **{report['rubric']['total']} / 100**",
        "",
        "| dimension | score | weight | evidence |",
        "|-----------|-------|--------|----------|",
    ]
    for row in report["rubric"]["dimensions"]:
        lines.append(
            f"| {row['label']} | {row['score']} | {row['weight']} | {'; '.join(row['evidence'])} |"
        )
    lines.extend(
        [
            "",
            "## Judges",
            "",
        ]
    )
    lines.extend(
        [
            "| judge | name | status | key metrics |",
            "|-------|------|--------|-------------|",
        ]
    )
    for judge_row in report["judges"]:
        metrics = ", ".join(f"{k}={v}" for k, v in judge_row["metrics"].items())
        lines.append(
            f"| {judge_row['judge']} | {judge_row['name']} | {judge_row['status']} | {metrics} |"
        )
    lines.extend(["", "## Evidence", ""])
    for judge_row in report["judges"]:
        lines.append(f"### {judge_row['judge']} {judge_row['name']} -- {judge_row['status']}")
        for item in judge_row["evidence"]:
            lines.append(f"- {item}")
        lines.append("")
    return "\n".join(lines)


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_report_artifacts(run_dir: Path, report: Dict[str, Any]) -> Dict[str, Any]:
    verdict_path = run_dir / "verdict.json"
    redboard_path = run_dir / "redboard.md"
    verdict_path.write_text(
        json.dumps(report, ensure_ascii=False, separators=(",", ":")) + "\n",
        encoding="utf-8",
    )
    redboard_path.write_text(render_markdown(report) + "\n", encoding="utf-8")
    meta = {
        "schema": "eval_redboard_meta_v1",
        "written_at_utc": datetime.now(timezone.utc).isoformat(),
        "label": report.get("label"),
        "verdict": report.get("verdict"),
        "red_count": report.get("red_count"),
        "semantic_critic": report.get("semantic_critic", {}),
        "verdict_sha256": sha256_file(verdict_path),
        "redboard_sha256": sha256_file(redboard_path),
    }
    (run_dir / "redboard.meta.json").write_text(
        json.dumps(meta, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    return meta


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run_dir", type=Path)
    parser.add_argument("--label")
    parser.add_argument(
        "--semantic-critic-cmd",
        help="command that reads semantic critic payload JSON on stdin and writes {'findings': [...]} JSON",
    )
    parser.add_argument("--write", action="store_true", help="write verdict.json and redboard.md")
    args = parser.parse_args()

    report = evaluate_run_dir(args.run_dir, args.label, args.semantic_critic_cmd)
    if args.write:
        write_report_artifacts(args.run_dir, report)
    else:
        print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report["verdict"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())

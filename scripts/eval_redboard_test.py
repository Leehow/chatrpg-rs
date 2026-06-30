import importlib.util
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]
MODULE_PATH = ROOT / "scripts" / "eval_redboard.py"


def load_module():
    spec = importlib.util.spec_from_file_location("eval_redboard", MODULE_PATH)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def write_jsonl(path: Path, rows):
    path.write_text("\n".join(json.dumps(row, ensure_ascii=False) for row in rows) + "\n")


def player_decision_signal(action="我观察线缆进入仓库后的走向。"):
    return {
        "turn": 1,
        "kind": "PLAYER_DECISION",
        "gm_visible_reply": "GM描述了仓库门前火线、线缆和伤员。",
        "perceived_facts": [
            "线缆连接无人机和仓库内部",
            "正面开阔地被火线覆盖",
        ],
        "active_goal": "查明线缆控制源，同时避免暴露在火线中",
        "hypotheses": [
            "无人机可能由仓库内设备供电或控制",
            "侧面接近比正面冲刺安全",
        ],
        "last_action_result": "上一轮观察确认了线缆和火力扇区",
        "risk_assessment": ["正面移动可能被无人机压制"],
        "resource_assessment": ["仓库外墙和门框可作为掩体"],
        "open_questions": ["线缆尽头是否有可断开的服务器或控制面板"],
        "candidate_actions": [
            "沿掩体观察线缆走向",
            "询问伤员无人机开火规律",
            "从侧面接近仓库门",
        ],
        "selection_rationale": "谨慎调查者优先选择低暴露、能增加可见事实的行动",
        "declared_action": action,
        "sent_to_gm": action,
        "response_contract": {
            "intent": "observe_cable_route",
            "acceptable_resolutions": [
                "给出可见线索",
                "要求具体检定",
                "说明看不到并给出原因",
            ],
        },
    }


def write_character_setup(run_dir: Path):
    write_jsonl(
        run_dir / "create_character.jsonl",
        [
            {"event": "phase", "phase": "start", "data": {"kind": "character_create_auto"}},
            {
                "event": "phase",
                "phase": "character_created",
                "data": {
                    "name": "测试调查员",
                    "status": "ready",
                    "sheet": {"stats": {"HP": 10}, "resources": {"hp": 10}, "skills": {"Spot Hidden": 60}},
                },
            },
        ],
    )
    (run_dir / "character_sheet.md").write_text(
        "# Character Sheet\n\n"
        "## Identity\n- Name: 测试调查员\n\n"
        "## Core Stats\n| Stat | Value |\n| --- | ---: |\n| HP | 10 |\n\n"
        "## Resources\n| Resource | Value |\n| --- | ---: |\n| hp | 10 |\n\n"
        "## Key Skills\n- Spot Hidden: 60\n"
    )


def load_action_menu_fixture():
    return json.loads(
        (ROOT / "eval/fixtures/presentation_constitution/action_menu_cases.json").read_text(
            encoding="utf-8"
        )
    )


class EvalRedboardTests(unittest.TestCase):
    def test_j2_allows_knott_briefing_success_with_practical_information(self):
        mod = load_module()
        text = (
            "[roll]Persuade -- Get Mr. Knott to set down the address, explain the keys, "
            "and say plainly what he knows: 1d100 = 9 vs target 20 -- Hard Success.[/roll] "
            "Knott gives you the usable lead in writing: the address is the old Corbitt place. "
            "He identifies the key ring as the keys he has for the property and says they are "
            "meant for the house itself. He can only stand behind the Macario reputation as "
            "rumor: illness, collapse, and madness, not firsthand knowledge. He agrees that "
            "municipal records, old newspaper files, library research, and court or police files "
            "are the sensible first move before entering the house."
        )

        self.assertFalse(mod.success_without_concrete_information(text))

    def test_j2_still_flags_knott_success_with_only_mood(self):
        mod = load_module()
        text = (
            "[roll]Persuade -- Get Mr. Knott to provide useful preparation: "
            "1d100 = 9 vs target 20 -- Hard Success.[/roll] "
            "Knott seems relieved that you are treating the matter seriously. "
            "He becomes more cooperative and says this gives Evelyn a usable lead "
            "and a solid line of inquiry, but the exchange never says what the lead is, "
            "how Evelyn can use it, or what actionable fact changed."
        )

        self.assertTrue(mod.success_without_concrete_information(text))

    def test_j2_flags_failed_roll_leaking_positive_hidden_lead(self):
        mod = load_module()
        bad = (
            "[roll]Library Use at Hall of Records: 1d100=[90] 目标:≤70，结果:strong_failure[/roll] "
            "At the Hall of Records, the name Corbitt leads toward an executor, a church, "
            "and a closure date that does not feel ordinary."
        )
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": bad}])
            (run_dir / "explain_001.txt").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j2 = next(j for j in report["judges"] if j["judge"] == "J2")
        self.assertEqual(j2["status"], "RED")
        self.assertEqual(j2["metrics"]["failure_leak_turns"], 1)
        self.assertTrue(
            any("failed check leaked positive hidden lead" in item for item in j2["evidence"])
        )

    def test_p1_flags_player_following_unearned_chapel_direction_menu(self):
        mod = load_module()
        previous = (
            "The Hall of Records search is inconclusive: no clean deed citation, no probate "
            "docket, and no named executor. The most promising next channels appear to be "
            "the courts, the police records, or leaving paper behind for the moment and "
            "going to the Chapel of Contemplation or the house itself."
        )
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Evelyn goes to the Chapel of Contemplation in daylight and searches it."
            )
            decision["response_contract"]["intent"] = (
                "investigate_chapel_for_records_symbols_and_corbitt_links"
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("follows unearned Chapel direction menu" in item for item in p1["evidence"])
        )

    def test_p1_flags_returning_to_hall_after_newspaper_success(self):
        mod = load_module()
        previous = (
            "At the Globe, the editor relents. "
            "[roll]Gain access to the Boston Globe clippings morgue: 1d100 = 37 vs target 60 — success.[/roll] "
            "The search becomes concrete. You find a house-history pattern: deaths, "
            "crippling accidents, illness, a suicide by kitchen knife, and the Macario family."
        )
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Evelyn does not leave the Hall of Records yet and asks the clerk for exact probate entries."
            )
            decision["response_contract"]["intent"] = (
                "continue_hall_records_search_after_success_without_specific_facts"
            )
            decision.update({"turn": 2, "gm_visible_reply": previous})
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("returns to Hall after newspaper house-history success" in item for item in p1["evidence"])
        )

    def test_p1_does_not_treat_knott_briefing_as_newspaper_house_history_success(self):
        mod = load_module()
        previous = (
            "[roll]Persuade — Press Mr. Knott for concrete leads: 1d100 = 51 vs 60. Success.[/roll] "
            "Knott gives the address and keys, says the Macario family suffered illness and nerves, "
            "and agrees municipal records and old newspaper files would be sensible places to begin."
        )
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Evelyn goes to Boston Hall of Records and searches property title and probate records."
            )
            decision["response_contract"]["intent"] = "search_hall_records_for_property_probate"
            decision.update({"turn": 2, "gm_visible_reply": previous})
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "GREEN")

    def test_p1_does_not_treat_knott_newspaper_archive_suggestion_as_newspaper_success(self):
        mod = load_module()
        previous = (
            "[roll]Persuade — Press Mr. Knott for concrete background and practical leads before visiting "
            "the Corbitt House: 1d100 = 34 vs 50 — Success.[/roll] "
            "Mr. Knott writes down the address and sorts through the keys. "
            "When pressed about the Macario family, he admits his knowledge is thin and secondhand. "
            "As for records, he agrees that official sources are the right next step. Municipal files, "
            "property records, and old newspaper archives all sound worthwhile to him before anyone risks "
            "stepping inside. He says, \"Yes. Records first. Newspaper files too, if you've the access.\""
        )
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Evelyn goes to Boston Hall of Records and searches property title and probate records."
            )
            decision["response_contract"]["intent"] = "search_hall_records_for_property_probate"
            decision.update({"turn": 2, "gm_visible_reply": previous})
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "GREEN")

    def test_semantic_critic_can_fail_q4_without_phrase_hardcode(self):
        mod = load_module()
        finding = {
            "category": "ACTION_MENU",
            "severity": "S1",
            "turns": [1],
            "root_layer": "Presentation",
            "expected": "GM should leave the next move open instead of framing branches.",
            "actual": "GM paraphrases several branches as the natural ways forward.",
            "evidence": ["The reply steers the player among GM-authored action branches."],
            "confidence": 0.94,
        }
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(run_dir / "signals.jsonl", [player_decision_signal()])
            write_jsonl(
                run_dir / "turn_001.jsonl",
                [{"event": "delta", "data": "The room settles into three possible currents of attention."}],
            )

            with patch.dict(
                os.environ,
                {"TRPG_EVAL_SEMANTIC_CRITIC_JSON": json.dumps({"findings": [finding]})},
            ):
                report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertEqual(q4["metrics"]["semantic_hits"], 1)
        self.assertTrue(any("semantic turn 1: ACTION_MENU" in item for item in q4["evidence"]))

    def test_semantic_critic_can_fail_p1_without_phrase_hardcode(self):
        mod = load_module()
        finding = {
            "category": "PLAYER_UNRESOLVED_GATE",
            "severity": "S1",
            "turns": [2],
            "root_layer": "PlayerSimulator",
            "expected": "Player should resolve or clarify the access gate before leaving the venue.",
            "actual": "Player jumps to a new location while the access approach is still pending.",
            "evidence": ["The previous GM reply asked how the character would gain access."],
            "confidence": 0.92,
        }
        previous = "A clerk bars the archive desk and asks what legitimate reason Evelyn has to see the restricted folio."
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            decision = player_decision_signal("Evelyn leaves for the old house and checks the front door.")
            decision.update({"turn": 2, "gm_visible_reply": previous})
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            with patch.dict(
                os.environ,
                {"TRPG_EVAL_SEMANTIC_CRITIC_JSON": json.dumps({"findings": [finding]})},
            ):
                report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertEqual(p1["metrics"]["semantic_findings"], 1)
        self.assertTrue(
            any("semantic turn 2: PLAYER_UNRESOLVED_GATE" in item for item in p1["evidence"])
        )

    def test_semantic_critic_command_hook_feeds_report(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            script = run_dir / "fake_semantic_critic.py"
            script.write_text(
                "import json, sys\n"
                "json.load(sys.stdin)\n"
                "print(json.dumps({'findings':[{'category':'ACTION_MENU','severity':'S1','turns':[1],'actual':'semantic command found a menu'}]}))\n"
            )
            write_character_setup(run_dir)
            write_jsonl(run_dir / "signals.jsonl", [player_decision_signal()])
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": "The room is quiet."}])

            report = mod.evaluate_run_dir(
                run_dir,
                label="unit",
                semantic_critic_cmd=f"{sys.executable} {script}",
            )

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(report["semantic_critic"]["enabled"], True)
        self.assertEqual(report["semantic_critic"]["findings"], 1)
        self.assertEqual(q4["status"], "RED")
        self.assertTrue(any("semantic turn 1: ACTION_MENU" in item for item in q4["evidence"]))

    def test_required_semantic_critic_fails_when_disabled(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(run_dir / "signals.jsonl", [player_decision_signal()])
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": "The room is quiet."}])

            with patch.dict(
                os.environ,
                {
                    "TRPG_EVAL_REQUIRE_SEMANTIC_CRITIC": "1",
                    "TRPG_EVAL_SEMANTIC_CRITIC_CMD": "",
                    "TRPG_EVAL_SEMANTIC_CRITIC_JSON": "",
                },
            ):
                report = mod.evaluate_run_dir(run_dir, label="unit")

        sem = next(j for j in report["judges"] if j["judge"] == "SEM")
        self.assertEqual(report["semantic_critic"]["required"], True)
        self.assertEqual(sem["status"], "RED")
        self.assertEqual(report["verdict"], "FAIL")

    def test_required_semantic_critic_passes_when_enabled(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(run_dir / "signals.jsonl", [player_decision_signal()])
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": "The room is quiet."}])

            with patch.dict(
                os.environ,
                {
                    "TRPG_EVAL_REQUIRE_SEMANTIC_CRITIC": "1",
                    "TRPG_EVAL_SEMANTIC_CRITIC_CMD": "",
                    "TRPG_EVAL_SEMANTIC_CRITIC_JSON": json.dumps({"findings": []}),
                },
            ):
                report = mod.evaluate_run_dir(run_dir, label="unit")

        sem = next(j for j in report["judges"] if j["judge"] == "SEM")
        self.assertEqual(report["semantic_critic"]["required"], True)
        self.assertEqual(sem["status"], "GREEN")

    def test_write_report_artifacts_emits_redboard_meta_hashes(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(run_dir / "signals.jsonl", [player_decision_signal()])
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": "The room is quiet."}])
            report = mod.evaluate_run_dir(run_dir, label="unit")

            mod.write_report_artifacts(run_dir, report)

            meta = json.loads((run_dir / "redboard.meta.json").read_text(encoding="utf-8"))
            self.assertEqual(meta["schema"], "eval_redboard_meta_v1")
            self.assertEqual(meta["label"], "unit")
            self.assertEqual(meta["verdict"], report["verdict"])
            self.assertEqual(
                meta["redboard_sha256"],
                mod.sha256_file(run_dir / "redboard.md"),
            )
            self.assertEqual(
                meta["verdict_sha256"],
                mod.sha256_file(run_dir / "verdict.json"),
            )

    def test_missing_player_sim_signals_does_not_fake_j1_green(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_jsonl(
                run_dir / "t01.jsonl",
                [{"event": "delta", "data": "你继续观察。"}],
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j1 = next(j for j in report["judges"] if j["judge"] == "J1")
        self.assertEqual(j1["status"], "RED")
        self.assertIn("signals.jsonl missing", j1["evidence"][0])

    def test_j1_flags_basement_crawlspace_reset_to_unopened_basement_door(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_jsonl(run_dir / "signals.jsonl", [player_decision_signal()])
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "You reach the basement floor. Scrape marks lead to a concealed crawl-space "
                            "opening in the basement wall, with Chapel of Contemplation carved inside."
                        ),
                    }
                ],
            )
            write_jsonl(
                run_dir / "t02.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "You lower yourself one body length into the crawl-space passage. "
                            "The stair line behind you remains marked and clear."
                        ),
                    }
                ],
            )
            write_jsonl(
                run_dir / "t03.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "The single most concrete visible affordance is the basement door. "
                            "It is across the ground-floor hall, under the staircase. "
                            "What Evelyn still cannot prove is what lies beyond it, whether it is locked, "
                            "and whether the air near it is colder or wetter until she closes the distance."
                        ),
                    }
                ],
            )
            for idx in range(1, 4):
                (run_dir / f"t{idx:02d}.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j1 = next(j for j in report["judges"] if j["judge"] == "J1")
        self.assertEqual(j1["status"], "RED")
        self.assertIn("state reset", " ".join(j1["evidence"]))

    def test_j1_flags_known_clue_rediscovered_as_new(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_jsonl(run_dir / "signals.jsonl", [player_decision_signal()])
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "Your probing finds a hidden crawl-space beyond the altered section. "
                            "Once you get the angle of your light into that cramped space, "
                            "carved words resolve plainly on the wall inside: Chapel of Contemplation."
                        ),
                    }
                ],
            )
            write_jsonl(
                run_dir / "t02.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "Then the light catches something worked by human hands. "
                            "On the crawl-space wall, words have been carved into the surface: "
                            "CHAPEL OF CONTEMPLATION."
                        ),
                    }
                ],
            )
            for idx in range(1, 3):
                (run_dir / f"t{idx:02d}.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j1 = next(j for j in report["judges"] if j["judge"] == "J1")
        self.assertEqual(j1["status"], "RED")
        self.assertEqual(j1["metrics"]["reintroduced_known_clue_turns"], 1)

    def test_j1_allows_chapel_location_visit_after_chapel_records_lead(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_jsonl(run_dir / "signals.jsonl", [player_decision_signal()])
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "The Hall of Records confirms that Walter Corbitt's executor was "
                            "Reverend Michael Thomas of the Chapel of Contemplation, and that "
                            "the Chapel of Contemplation closed in 1912."
                        ),
                    }
                ],
            )
            write_jsonl(
                run_dir / "t02.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "By the time Evelyn reaches the Chapel of Contemplation, the light is good enough. "
                            "[roll]Careful search of the chapel grounds and surviving records: "
                            "1d100 = 52 vs 70. Success.[/roll] Among the surviving chapel records, "
                            "you find a distinctive symbol associated with the Chapel of Contemplation. "
                            "The records also state that Walter Corbitt was buried in the basement of his house."
                        ),
                    }
                ],
            )
            for idx in range(1, 3):
                (run_dir / f"t{idx:02d}.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j1 = next(j for j in report["judges"] if j["judge"] == "J1")
        self.assertEqual(j1["status"], "GREEN")
        self.assertEqual(j1["metrics"]["reintroduced_known_clue_turns"], 0)

    def test_j1_does_not_treat_chapel_burial_record_as_deep_basement_position(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_jsonl(run_dir / "signals.jsonl", [player_decision_signal()])
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "At the Chapel of Contemplation, Evelyn finds a record stating "
                            "Walter Corbitt was buried in the basement of his own house."
                        ),
                    }
                ],
            )
            write_jsonl(
                run_dir / "t02.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "The most concrete visible affordance now is the basement door on the "
                            "ground floor, reachable from the entry hall. Not yet proven: whether "
                            "it is locked, what condition the stairs are in, what lies beyond it, "
                            "or whether opening it changes the air or danger."
                        ),
                    }
                ],
            )
            for idx in range(1, 3):
                (run_dir / f"t{idx:02d}.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j1 = next(j for j in report["judges"] if j["judge"] == "J1")
        self.assertEqual(j1["status"], "GREEN")
        self.assertEqual(j1["metrics"]["state_reset_turns"], 0)

    def test_j1_flags_ground_floor_sweep_reframed_as_upper_floor_entry(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_jsonl(run_dir / "signals.jsonl", [player_decision_signal()])
            (run_dir / "t01.input.txt").write_text(
                "Evelyn keeps her back path marked and searches the ground floor clockwise "
                "from the entry, one room at a time, watching for drafts, cellar smells, "
                "scrape marks, footprints, or a route upstairs or downstairs."
            )
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "你把行动接到楼梯和楼上入口处，但先只确认可公开落地的部分："
                            "楼梯可逐级试探，楼梯平台和邻近房门是当前最清楚的上层接点。"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j1 = next(j for j in report["judges"] if j["judge"] == "J1")
        self.assertEqual(j1["status"], "RED")
        self.assertEqual(j1["metrics"]["state_reset_turns"], 1)
        self.assertIn("state reset", " ".join(j1["evidence"]))

    def test_j1_flags_bedroom_threshold_probe_reframed_as_exterior_entry(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_jsonl(run_dir / "signals.jsonl", [player_decision_signal()])
            (run_dir / "t01.input.txt").write_text(
                "Evelyn stays on the landing side of the bedroom threshold with her retreat "
                "path open, shines the flashlight across the bed and wardrobe, and probes "
                "only what she can reach from the doorway."
            )
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": "你从门侧把外门打开，只把身体推进到门槛内侧，眼前落成门厅和邻近房门。",
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j1 = next(j for j in report["judges"] if j["judge"] == "J1")
        self.assertEqual(j1["status"], "RED")
        self.assertEqual(j1["metrics"]["state_reset_turns"], 1)

    def test_j1_does_not_treat_exterior_threshold_entry_as_bedroom_probe(self):
        mod = load_module()
        player_input = (
            "Evelyn Price uses the workable exterior entrance the last exchange established, "
            "without treating it as proven safest. She stands to the side rather than squarely "
            "in the doorway, keeps her retreat line open behind her, and opens only enough to "
            "listen, smell, and look through the first threshold. If the door opens and nothing "
            "immediately rushes her, she steps just inside with the flashlight low and maps the "
            "entry hall, visible rooms, stairs, exits, odors, footprints, drafts, and any object "
            "that looks recently disturbed. She keeps the door open behind her."
        )
        gm_text = (
            "The door opens with a reluctant scrape. You are just inside the Corbitt House, "
            "off the threshold, with the front door open behind you and the entry hall, stairs, "
            "and first visible rooms under your light."
        )

        self.assertFalse(mod.input_requests_bedroom_threshold_probe(player_input))

        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_jsonl(run_dir / "signals.jsonl", [player_decision_signal()])
            (run_dir / "turn_007.input.txt").write_text(player_input)
            write_jsonl(run_dir / "turn_007.jsonl", [{"event": "delta", "data": gm_text}])
            (run_dir / "explain_007.txt").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j1 = next(j for j in report["judges"] if j["judge"] == "J1")
        self.assertEqual(j1["metrics"]["state_reset_turns"], 0)

    def test_three_digit_turn_files_are_evaluated(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_jsonl(run_dir / "signals.jsonl", [player_decision_signal()])
            write_jsonl(
                run_dir / "t001.jsonl",
                [
                    {
                        "event": "delta",
                        "data": "[roll]Library Use 1d100=14 vs 70 success[/roll]你查到了具体线索。",
                    }
                ],
            )
            (run_dir / "t001.explain").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
                '  WorldFactChanged {"fact":"record_found"}\n'
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j2 = next(j for j in report["judges"] if j["judge"] == "J2")
        self.assertEqual(j2["metrics"]["turns"], 1)
        j4 = next(j for j in report["judges"] if j["judge"] == "J4")
        self.assertEqual(j4["metrics"]["check_turns"], 1)

    def test_helper_turn_files_are_evaluated(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_jsonl(run_dir / "signals.jsonl", [player_decision_signal()])
            write_jsonl(
                run_dir / "turn_001.jsonl",
                [
                    {
                        "event": "delta",
                        "data": "[roll]Library Use 1d100=14 vs 70 success[/roll]你查到了具体线索。",
                    }
                ],
            )
            (run_dir / "explain_001.txt").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
                '  WorldFactChanged {"fact":"record_found"}\n'
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j2 = next(j for j in report["judges"] if j["judge"] == "J2")
        self.assertEqual(j2["metrics"]["turns"], 1)
        j4 = next(j for j in report["judges"] if j["judge"] == "J4")
        self.assertEqual(j4["metrics"]["check_turns"], 1)

    def test_j2_flags_hall_records_success_without_specific_record_facts(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_jsonl(run_dir / "signals.jsonl", [player_decision_signal()])
            write_jsonl(
                run_dir / "t001.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "[roll]Search Hall of Records for Corbitt property, probate, and civil filings: "
                            "1d100=[33] target 65 result strong_success[/roll]\n"
                            "The materials connect cleanly: property ownership, former owners, probate, "
                            "estate execution, and public legal records all surface more smoothly than expected. "
                            "The scattered keywords finally settle onto one traceable line, and the inquiry "
                            "moves from rumor into records that can be checked page by page."
                        ),
                    }
                ],
            )
            (run_dir / "t001.explain").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
                '  WorldFactChanged {"fact":"records_progress"}\n'
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j2 = next(j for j in report["judges"] if j["judge"] == "J2")
        self.assertEqual(j2["status"], "RED")
        self.assertEqual(j2["metrics"]["success_without_information_turns"], 1)

    def test_j2_flags_success_roll_with_only_machine_confirmation_header(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_jsonl(run_dir / "signals.jsonl", [player_decision_signal()])
            write_jsonl(
                run_dir / "t001.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "根据本回合已确认的结果：\n"
                            "· [roll]检定[check_e4eeb17] Search Hall of Records indexes "
                            "for Corbitt property and probate trail: 1d100=[20] "
                            "目标:≤70，结果:strong_success[/roll]"
                        ),
                    }
                ],
            )
            (run_dir / "t001.explain").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j2 = next(j for j in report["judges"] if j["judge"] == "J2")
        self.assertEqual(j2["status"], "RED")
        self.assertEqual(j2["metrics"]["success_without_information_turns"], 1)

    def test_j2_flags_multi_check_machine_confirmation_without_basement_facts(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_jsonl(run_dir / "signals.jsonl", [player_decision_signal()])
            (run_dir / "turn_008.input.txt").write_text(
                "Evelyn ties a handkerchief to the basement door handle, leaves the door wedged open, "
                "listens from the top of the stairs, descends one step at a time, and looks for drafts, "
                "loose brick, disturbed earth, scrape marks, hidden panels, a body, a ritual object, or any moving threat."
            )
            write_jsonl(
                run_dir / "turn_008.jsonl",
                [
                    {
                        "event": "scene_transition",
                        "data": {
                            "from": "loc_corbitt_house_ground_floor",
                            "to": "loc_corbitt_house_basement",
                        },
                    },
                    {
                        "event": "delta",
                        "data": (
                            "根据本回合已确认的结果：\n"
                            "· [roll]检定[check_677] Listening at the top of the basement stairs: "
                            "1d100=[34] 目标:≤40，结果:成功[/roll]\n"
                            "· [roll]检定[check_1f7] Careful descent and search of the basement stairs "
                            "and immediate cellar for hidden signs or threats: 1d100=[32] 目标:≤65，结果:strong_success[/roll]\n"
                            "· The Listening at the top of the basement stairs check succeeds.\n"
                            "· The Careful descent and search of the basement stairs and immediate cellar "
                            "for hidden signs or threats check succeeds."
                        ),
                    },
                ],
            )
            (run_dir / "explain_008.txt").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"check_677"}\n'
                '  CheckResolved {"check_id":"check_677"}\n'
                '  DiceRolled {"check_id":"check_1f7"}\n'
                '  CheckResolved {"check_id":"check_1f7"}\n'
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j2 = next(j for j in report["judges"] if j["judge"] == "J2")
        self.assertEqual(j2["status"], "RED")
        self.assertEqual(j2["metrics"]["success_without_information_turns"], 1)
        rules_dimension = next(
            row for row in report["rubric"]["dimensions"] if row["id"] == "rules_resolution"
        )
        self.assertEqual(rules_dimension["score"], 0)

    def test_j2_flags_official_records_success_that_only_opens_access(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_jsonl(run_dir / "signals.jsonl", [player_decision_signal()])
            write_jsonl(
                run_dir / "t001.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "[roll]调取迈克尔·托马斯／沉思礼拜堂相关法院与警局档案 "
                            "1d100=[29] 目标:≤30，结果:成功[/roll] "
                            "你没有被拦在外面。无论是在高等法院，还是随后转到中央警察局的记录柜台，"
                            "你都顺利拿到了查阅迈克尔·托马斯与沉思礼拜堂相关记录的门路；"
                            "与你所问的那几年、那类案由有关的索引和档案，不再只是隔着柜台听人一句无可奉告，"
                            "而是真正对你打开了入口。1912年不再只是一个关门的年份；"
                            "它现在也确实把法院和警局的记录牵了进来。"
                        ),
                    }
                ],
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j2 = next(j for j in report["judges"] if j["judge"] == "J2")
        self.assertEqual(j2["status"], "RED")
        self.assertEqual(j2["metrics"]["success_without_information_turns"], 1)

    def test_j2_flags_official_records_access_even_when_query_terms_are_repeated(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_jsonl(run_dir / "signals.jsonl", [player_decision_signal()])
            write_jsonl(
                run_dir / "t001.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "[roll]调取迈克尔·托马斯牧师与“沉思小教堂”相关法院或警方档案 "
                            "1d100=[45] ≤50 成功[/roll] "
                            "你把自己的来意说得平稳自然：一九一二年，迈克尔·托马斯，"
                            "沉思小教堂，查封、突袭、刑事程序、拘押、死亡，或者失踪。"
                            "最后，对方没有把你打发走，那道原本不打算为你开启的口子，还是开了。"
                            "你成功拿到了查阅迈克尔·托马斯牧师和“沉思小教堂”相关法院或警方记录的权限。"
                            "纸张已经摊在眼前，线索确实伸进了法院和警署留下的记录里。"
                        ),
                    }
                ],
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j2 = next(j for j in report["judges"] if j["judge"] == "J2")
        self.assertEqual(j2["status"], "RED")
        self.assertEqual(j2["metrics"]["success_without_information_turns"], 1)

    def test_j2_flags_official_records_boundary_map_without_record_content(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_jsonl(run_dir / "signals.jsonl", [player_decision_signal()])
            write_jsonl(
                run_dir / "t001.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "[roll]Law — navigate higher courts and police public indexes for Corbitt House "
                            "records — 1d100: 34 vs 45 — success.[/roll] "
                            "They do not simply hand over a neat file tied to the house address alone. "
                            "Where records are restricted, address-only requests do not open sealed or "
                            "privileged material. Your success buys a firmer map of the paper trail and "
                            "its legal boundaries. If public office counters cannot go further, the next "
                            "legal public source is the newspaper morgue and archive trail."
                        ),
                    }
                ],
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j2 = next(j for j in report["judges"] if j["judge"] == "J2")
        self.assertEqual(j2["status"], "RED")
        self.assertEqual(j2["metrics"]["success_without_information_turns"], 1)

    def test_j2_flags_scene_transition_without_new_visible_situation(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_jsonl(run_dir / "signals.jsonl", [player_decision_signal()])
            write_jsonl(
                run_dir / "t001.jsonl",
                [
                    {
                        "event": "scene_transition",
                        "data": {"from": "loc_hall_records", "to": "loc_corbitt_house_exterior"},
                    },
                    {
                        "event": "delta",
                        "data": "（本回合按已落账的机械结果继续；无新增可公开的机械事实。）",
                    },
                ],
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j2 = next(j for j in report["judges"] if j["judge"] == "J2")
        self.assertEqual(j2["status"], "RED")
        self.assertEqual(j2["metrics"]["scene_transition_without_visible_situation_turns"], 1)

    def test_p1_flags_player_leaving_after_hall_records_success_without_specific_facts(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_jsonl(
                run_dir / "signals.jsonl",
                [
                    player_decision_signal(
                        "Evelyn searches Hall of Records for title, probate, executors, and public legal records."
                    ),
                    {
                        **player_decision_signal(
                            "Evelyn goes to the Corbitt House in daylight and begins exterior inspection."
                        ),
                        "turn": 2,
                        "active_goal": "Inspect the Corbitt House exterior and choose an entry only from visible conditions.",
                        "last_action_result": "Previous action succeeded; use only concrete facts that the GM actually exposed.",
                        "response_contract": {
                            "intent": "inspect_house_exterior_and_safe_entry",
                            "acceptable_resolutions": [
                                "give a visible exterior situation",
                                "request a check",
                                "block with a reason",
                            ],
                        },
                    },
                ],
            )
            write_jsonl(
                run_dir / "t001.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "[roll]Search Hall of Records: 1d100=33 vs 65 success[/roll] "
                            "The materials connect cleanly: property ownership, former owners, "
                            "probate, estate execution, and public legal records all surface. "
                            "The inquiry moves from rumor into records that can be checked page by page."
                        ),
                    }
                ],
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("leaves Hall Records after success without specific facts" in item for item in p1["evidence"])
        )

    def test_p1_flags_player_continuing_after_empty_visible_result(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            next_decision = player_decision_signal(
                "Evelyn keeps her back path marked and searches the ground floor clockwise."
            )
            next_decision["turn"] = 2
            next_decision["active_goal"] = "Search the ground floor without losing exit control."
            next_decision["last_action_result"] = "Previous action produced narration; treat only explicit visible facts as established."
            next_decision["response_contract"] = {
                "intent": "systematic_ground_floor_search",
                "acceptable_resolutions": [
                    "ground-floor findings",
                    "check result",
                    "blocked reason",
                ],
            }
            write_jsonl(
                run_dir / "signals.jsonl",
                [
                    player_decision_signal("Evelyn opens the door and steps just inside."),
                    next_decision,
                ],
            )
            write_jsonl(
                run_dir / "t001.jsonl",
                [
                    {
                        "event": "delta",
                        "data": "（本回合按已落账的机械结果继续；无新增可公开的机械事实。）",
                    }
                ],
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("continues after empty visible result" in item for item in p1["evidence"])
        )

    def test_p1_flags_reentering_exterior_door_after_ground_floor_position(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            decision = player_decision_signal(
                "Evelyn unlocks the safest exterior door, opens it from the side, "
                "and steps just inside to map the entry hall again."
            )
            decision["turn"] = 2
            decision["active_goal"] = "Re-enter the threshold and map the ground floor."
            decision["last_action_result"] = (
                "The previous reply placed Evelyn inside the ground floor with the entry behind her."
            )
            decision["response_contract"] = {
                "intent": "enter_threshold_and_map_ground_floor_affordances",
                "acceptable_resolutions": [
                    "first-step footing",
                    "immediate layout",
                    "reaction or no-reaction result",
                ],
            }
            write_jsonl(run_dir / "signals.jsonl", [decision])
            write_jsonl(
                run_dir / "t001.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "Evelyn stands just inside the house now. The front entry is behind "
                            "and beside her, still open as a retreat route. The dim stretch of "
                            "ground floor in front of her has already been searched once, with "
                            "no immediate movement but a stale draft leading deeper."
                        ),
                    }
                ],
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("re-enters exterior door after interior ground-floor position" in item for item in p1["evidence"])
        )

    def test_p1_flags_player_leaving_official_records_after_access_without_contents(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            next_decision = player_decision_signal(
                "Evelyn drives to the Chapel of Contemplation and searches the ruin."
            )
            next_decision["turn"] = 2
            next_decision["active_goal"] = "Inspect the Chapel of Contemplation lead with daylight safety precautions."
            next_decision["last_action_result"] = "Previous action succeeded; use only concrete facts that the GM actually exposed."
            next_decision["response_contract"] = {
                "intent": "investigate_chapel_for_records_symbols_and_corbitt_links",
                "acceptable_resolutions": [
                    "site safety",
                    "visible records",
                    "check result",
                ],
            }
            write_jsonl(
                run_dir / "signals.jsonl",
                [
                    player_decision_signal(
                        "Evelyn asks court and police desks for any accessible official records tied to the property lead."
                    ),
                    next_decision,
                ],
            )
            write_jsonl(
                run_dir / "t001.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "[roll]Trace court and police records for Thomas and the Chapel: "
                            "1d100=29 vs 30 success[/roll] "
                            "The higher court and Central Police indexes open to you. "
                            "You get access to the relevant references and file route, but the reply only says "
                            "the 1912 trail now involves court and police records."
                        ),
                    }
                ],
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("leaves official records after access without contents" in item for item in p1["evidence"])
        )

    def test_p1_allows_leaving_official_records_after_refused_access(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            next_decision = player_decision_signal(
                "Evelyn drives to the Chapel of Contemplation in daylight and searches the ruin."
            )
            next_decision["turn"] = 2
            next_decision["active_goal"] = "Inspect the Chapel of Contemplation lead with daylight safety precautions."
            next_decision["last_action_result"] = (
                "Previous action was adjudicated as a failure or blocked access; do not treat requested facts as learned."
            )
            next_decision["response_contract"] = {
                "intent": "investigate_chapel_for_records_symbols_and_corbitt_links",
                "acceptable_resolutions": [
                    "site safety",
                    "visible records",
                    "check result",
                ],
            }
            write_jsonl(
                run_dir / "signals.jsonl",
                [
                    player_decision_signal(
                        "Evelyn asks court and police desks for any accessible official records tied to the property lead."
                    ),
                    next_decision,
                ],
            )
            write_jsonl(
                run_dir / "t001.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "[roll]Fast Talk — court and police records access: "
                            "1d100 = 54 vs target 40 → failure.[/roll] "
                            "Evelyn specifically asked about Reverend Michael Thomas, "
                            "the Chapel of Contemplation, and a 1912 trail. "
                            "The clerk says any sensitive 1912 material is not for casual inspection. "
                            "He does not open the file room. He does not produce an index card. "
                            "If Evelyn wants official access, she can come back with stronger standing."
                        ),
                    }
                ],
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "GREEN")

    def test_p1_allows_knott_briefing_to_hall_records_with_police_and_newspaper_morgue_advice(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            previous = (
                "Mr. Knott says municipal records are sensible: deeds, tax rolls, transfers, "
                "death notices, and complaints if any were filed. If there was legal trouble "
                "attached to the property, there may be something in court or police records as well. "
                "If the records and old newspapers tell you what sort of history sits in that house, "
                "you will be better prepared. He says he would begin with city records and the "
                "newspaper morgues before any visit to the property."
            )
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Evelyn goes to the Hall of Records and searches property title and probate entries."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "last_action_result": "Previous action produced narration; treat only explicit visible facts as established.",
                    "active_goal": "Find property, probate, executor, and ownership records.",
                    "response_contract": {
                        "intent": "search_hall_records_for_property_probate",
                        "acceptable_resolutions": ["specific public-record entries"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "GREEN")

    def test_coc_20turn_eval_fails_if_it_passes_after_only_chapel_without_resolution(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp) / "coc_haunting_20turn_unit"
            run_dir.mkdir()
            write_character_setup(run_dir)
            decisions = []
            for turn in range(1, 6):
                decision = player_decision_signal(f"我执行第 {turn} 回合调查。")
                decision["turn"] = turn
                decisions.append(decision)
                write_jsonl(
                    run_dir / f"turn_{turn:03d}.jsonl",
                    [
                        {
                            "event": "delta",
                            "data": (
                                "[roll]Spot Hidden 1d100=20 vs 70 success[/roll] "
                                "The live thread that still matters most is the Corbitt house itself. "
                                "Nothing dramatic collapses."
                            ),
                        }
                    ],
                )
                (run_dir / f"explain_{turn:03d}.txt").write_text(
                    "domain_events:\n"
                    '  DiceRolled {"check_id":"c1"}\n'
                    '  CheckResolved {"check_id":"c1"}\n'
                    '  SceneTransitioned {"scene_id":"chapel"}\n'
                )
            write_jsonl(run_dir / "player_decisions.jsonl", decisions)

            report = mod.evaluate_run_dir(run_dir, label="coc_haunting_20turn_unit")

        self.assertEqual(report["verdict"], "FAIL")
        j3 = next(j for j in report["judges"] if j["judge"] == "J3")
        self.assertEqual(j3["status"], "RED")
        self.assertIn("ended before 20 turns", " ".join(j3["evidence"]))

    def test_m1_flags_timed_out_turn_without_visible_delta(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "phase", "phase": "session", "data": {}}])
            (run_dir / "turn_001.err").write_text("command timed out after 480 seconds")
            write_jsonl(run_dir / "player_decisions.jsonl", [player_decision_signal()])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        self.assertEqual(report["verdict"], "FAIL")
        m1 = next(j for j in report["judges"] if j["judge"] == "M1")
        self.assertEqual(m1["status"], "RED")
        self.assertTrue(any("timed out" in item for item in m1["evidence"]))

    def test_m1_flags_turn_input_without_jsonl_output(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            (run_dir / "turn_001.input.txt").write_text("I search the room.")
            write_jsonl(run_dir / "player_decisions.jsonl", [player_decision_signal()])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        self.assertEqual(report["verdict"], "FAIL")
        m1 = next(j for j in report["judges"] if j["judge"] == "M1")
        self.assertEqual(m1["status"], "RED")
        self.assertTrue(any("missing turn output jsonl" in item for item in m1["evidence"]))

    def test_player_decisions_jsonl_is_audited_when_signals_jsonl_absent(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_jsonl(run_dir / "player_decisions.jsonl", [player_decision_signal()])
            write_jsonl(
                run_dir / "t001.jsonl",
                [{"event": "delta", "data": "你得到一个明确结果。"}],
            )
            (run_dir / "t001.explain").write_text(
                "domain_events:\n"
                '  WorldFactChanged {"fact":"clear_result"}\n'
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        self.assertTrue(report["player_sim_signals"]["artifact_present"])
        self.assertEqual(report["player_sim_signals"]["player_decisions"], 1)
        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "GREEN")

    def test_p1_flags_incomplete_player_decision_protocol(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_jsonl(
                run_dir / "signals.jsonl",
                [
                    {
                        "turn": 1,
                        "kind": "PLAYER_DECISION",
                        "declared_action": "我继续靠近仓库。",
                        "sent_to_gm": "我继续靠近仓库。",
                    }
                ],
            )
            write_jsonl(
                run_dir / "t01.jsonl",
                [{"event": "delta", "data": "你靠近了一点。"}],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertEqual(p1["metrics"]["incomplete_decisions"], 1)
        self.assertIn("candidate_actions>=2", p1["evidence"][0])
        player_dim = next(
            row for row in report["rubric"]["dimensions"] if row["id"] == "player_simulation"
        )
        self.assertEqual(player_dim["weight"], 15)
        self.assertEqual(player_dim["score"], 0)

    def test_p1_flags_conditional_placeholder_player_action(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            decision = player_decision_signal(
                "Evelyn inspects the most concrete visible affordance: "
                "if it was a door, she listens and tests it; if it was a mark, "
                "she photographs it; if it was a sound, she triangulates it from cover."
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": "The scene waits."}])
            (run_dir / "explain_001.txt").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertEqual(p1["metrics"]["invalid_action_declarations"], 1)
        self.assertIn("conditional placeholder", p1["evidence"][0])

    def test_p1_flags_repeated_fallback_intent_loop(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            first = player_decision_signal("Evelyn asks what exact visible affordance changed.")
            first.update(
                {
                    "turn": 1,
                    "response_contract": {
                        "intent": "inspect_latest_visible_affordance",
                        "acceptable_resolutions": ["specific visible fact"],
                    },
                }
            )
            second = player_decision_signal("Evelyn again asks what exact visible affordance changed.")
            second.update(
                {
                    "turn": 2,
                    "response_contract": {
                        "intent": "inspect_latest_visible_affordance",
                        "acceptable_resolutions": ["specific visible fact"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [first, second])
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": "The room remains vague."}])
            write_jsonl(run_dir / "turn_002.jsonl", [{"event": "delta", "data": "The room remains vague."}])
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            (run_dir / "explain_002.txt").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertEqual(p1["metrics"]["fallback_loop_decisions"], 2)
        self.assertTrue(any("repeats fallback intent" in item for item in p1["evidence"]))

    def test_p1_flags_fallback_after_concrete_basement_board_affordance(self):
        mod = load_module()
        previous = (
            "You are in the basement now, standing just off the foot of the stairs. "
            "The beam of your flashlight settles across the basement proper: rough boards, "
            "old bins, patches of darkness under the structure, and a clammy stillness that "
            "makes the room feel less like storage than concealment. The route behind you "
            "remains open. What does Evelyn examine first?"
        )
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Evelyn asks the Keeper to ground the latest actionable thing in plain language."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "active_goal": "Clarify the latest actionable thing.",
                    "selection_rationale": "The player wants the Keeper to name the concrete affordance.",
                    "response_contract": {
                        "intent": "clarify_latest_visible_affordance",
                        "acceptable_resolutions": ["specific visible fact"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("fallback after concrete visible affordance" in item for item in p1["evidence"])
        )

    def test_p1_flags_fallback_after_boarded_irregularity_affordance(self):
        mod = load_module()
        previous = (
            "From where you are, the only plainly actionable thing is the boarded irregularity "
            "in the basement structure--the rough, concealed section you have been photographing "
            "and testing from a safe distance. What you can actually perceive is this: the basement "
            "is colder and wetter than the floor above, and one part of it does not read like ordinary "
            "storage. Between the rough boards, bins, and the dark under-space beyond, there is a "
            "concealed area here rather than just clutter. It stands out as something intentionally "
            "closed off. Relative to your retreat, it is still in the basement with you. Your way "
            "back is the stairs up to the ground floor."
        )
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Evelyn asks the Keeper to ground the latest actionable thing in plain language."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "active_goal": "Clarify the latest player-visible result.",
                    "selection_rationale": "The player wants the Keeper to name the concrete affordance.",
                    "response_contract": {
                        "intent": "clarify_latest_visible_affordance",
                        "acceptable_resolutions": ["specific visible fact"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("fallback after concrete visible affordance" in item for item in p1["evidence"])
        )

    def test_p1_flags_fallback_after_basement_bottom_cellar_ahead_affordance(self):
        mod = load_module()
        previous = (
            "You ease the basement door open and leave the handkerchief where it will catch your eye "
            "on the way back. The air below is colder. Your flashlight beam slides down a narrow run "
            "of steps into a basement that feels older than the rest of the house. The stairs hold. "
            "Nothing has shifted behind your back. Nothing blocks the way. At the foot of the stairs, "
            "your light reaches only partway into the cellar: rough foundation walls, cluttered dimness, "
            "a low spread of shadow where the beam seems to thin rather than end. You are now at the "
            "bottom of the basement stairs, with the marked way back behind you and the cellar ahead."
        )
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Evelyn asks the Keeper to ground the latest actionable thing in plain language."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "active_goal": "Clarify the latest player-visible result.",
                    "selection_rationale": "The player wants the Keeper to name the concrete affordance.",
                    "response_contract": {
                        "intent": "clarify_latest_visible_affordance",
                        "acceptable_resolutions": ["specific visible fact"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("fallback after concrete visible affordance" in item for item in p1["evidence"])
        )

    def test_p1_flags_player_inventing_moving_bedroom_threat_from_chapel(self):
        mod = load_module()
        previous = (
            "At the Chapel of Contemplation, you move slowly through stale dust and warped drawers. "
            "Some sections look rotten versus actively dangerous. If a trail to Corbitt or Michael "
            "Thomas exists here, it is buried in remnants rather than sitting out in the open. "
            "What area do you focus on first: the offices, the pulpit and worship space, or storage "
            "and loose papers?"
        )
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Evelyn treats the bed's impossible movement as the immediate hazard and retreats to the landing."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "active_goal": "Get out of the immediate moving-bedroom hazard.",
                    "selection_rationale": "The player responds to the moving bed.",
                    "response_contract": {
                        "intent": "retreat_from_moving_bedroom_threat",
                        "acceptable_resolutions": ["moving bed reaction"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("invents moving-bedroom threat" in item for item in p1["evidence"])
        )

    def test_p1_flags_player_inventing_moving_bedroom_threat_after_negated_bedroom_movement(self):
        mod = load_module()
        previous = (
            "At the top, the landing stays usable as a retreat line behind you. "
            "The furniture and bedding upstairs have not been neatly kept, but neither do they read "
            "like a place recently lived in. More importantly, your caution does pay off: "
            "No bedclothes twitch. No wardrobe door swings by itself. "
            "No object moves on its own while you watch."
        )
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(run_dir / "turn_010.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_010.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Evelyn treats the bed's impossible movement as the immediate hazard and retreats to the landing."
            )
            decision.update(
                {
                    "turn": 11,
                    "gm_visible_reply": previous,
                    "active_goal": "Get out of the immediate moving-bedroom hazard.",
                    "selection_rationale": "The player responds to the moving bed.",
                    "response_contract": {
                        "intent": "retreat_from_moving_bedroom_threat",
                        "acceptable_resolutions": ["moving bed reaction"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("invents moving-bedroom threat" in item for item in p1["evidence"])
        )

    def test_p1_allows_retreat_after_visible_moving_bedroom_threat(self):
        mod = load_module()
        previous = (
            "The bed gives a sudden, violent heave and scrapes toward the doorway without any human cause. "
            "The landing remains behind you, but the furniture is now actively dangerous."
        )
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(run_dir / "turn_010.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_010.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Evelyn treats the bed's impossible movement as the immediate hazard and retreats to the landing."
            )
            decision.update(
                {
                    "turn": 11,
                    "gm_visible_reply": previous,
                    "active_goal": "Get out of the immediate moving-bedroom hazard.",
                    "selection_rationale": "The player responds to the moving bed.",
                    "response_contract": {
                        "intent": "retreat_from_moving_bedroom_threat",
                        "acceptable_resolutions": ["moving bed reaction"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "GREEN")

    def test_p1_flags_opened_crawl_space_chapel_carving_without_current_visible_evidence(self):
        mod = load_module()
        previous = (
            "[roll]Cautious descent into the basement -- 1d100: 7 vs DEX 70 -- Extreme success.[/roll]\n"
            "She reaches the basement floor safely. The air down here is colder and wetter than the rooms above. "
            "Rough boards, old bins, and the dark under-space beyond them make the cellar feel less like storage "
            "than something meant to be hidden. Nothing immediately rushes her. Nothing blocks the stairs behind her. "
            "Your light now plays over the basement proper and the rougher shadows beyond."
        )
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(
                run_dir / "turn_005.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "The Chapel of Contemplation is a concrete public-record lead, "
                            "but no basement crawl-space carving has been found yet."
                        ),
                    }
                ],
            )
            write_jsonl(run_dir / "turn_014.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_014.txt").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c14"}\n'
                '  CheckResolved {"check_id":"c14"}\n'
            )
            decision = player_decision_signal(
                "Evelyn stays at the opened basement crawl-space entrance and transcribes "
                "the carved words 'Chapel of Contemplation' exactly."
            )
            decision.update(
                {
                    "turn": 15,
                    "gm_visible_reply": previous,
                    "perceived_facts": [
                        "The basement floor is reachable and rough boards, old bins, and dark under-space are visible."
                    ],
                    "active_goal": "Inspect the opened crawl-space entrance and Chapel carving.",
                    "selection_rationale": "The latest reply opened a crawl space and exposed a visible Chapel carving.",
                    "response_contract": {
                        "intent": "inspect_opened_crawl_space_and_chapel_carving",
                        "acceptable_resolutions": ["visible crawl-space depth"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("claims crawl-space carving without visible evidence" in item for item in p1["evidence"])
        )

    def test_p1_allows_crawl_space_chapel_carving_when_previous_crawl_text_visible(self):
        mod = load_module()
        previous = (
            "[roll]Probe the newly opened crawl space from the threshold without entering: "
            "1d100 = 84 vs Spot Hidden 50 — failure.[/roll] "
            "From where Nell holds position, she can confirm only the nearest truths. "
            "The opening is real, narrow, and man-made enough to feel deliberate. "
            "The carved words are exactly as written: Chapel of Contemplation. "
            "The immediate lip and first reachable stretch inside do not lunge, shift, or seize the walking stick."
        )
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(run_dir / "turn_009.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_009.txt").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c9"}\n'
                '  CheckResolved {"check_id":"c9"}\n'
            )
            decision = player_decision_signal(
                "Nell photographs the opened crawl-space entrance and transcribes the carved words "
                "'Chapel of Contemplation' exactly."
            )
            decision.update(
                {
                    "turn": 10,
                    "gm_visible_reply": previous,
                    "perceived_facts": [
                        "The crawl-space opening is real and Chapel of Contemplation is carved there."
                    ],
                    "active_goal": "Document the opened crawl-space carving.",
                    "response_contract": {
                        "intent": "inspect_opened_crawl_space_and_chapel_carving",
                        "acceptable_resolutions": ["documented carving"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "GREEN")

    def test_p1_flags_continuing_crawl_probe_after_exiting_house_for_tools(self):
        mod = load_module()
        previous = (
            "You back out without pressing your luck. You withdraw to the basement stairs, then retrace "
            "your marked path up and out of Corbitt House. Nothing lunges from the crawl-space, and the "
            "retreat line holds long enough for you to leave cleanly. Outside, with the house behind you, "
            "you've established that the crawl-space mouth is unsafe and that if you mean to go back in, "
            "you'll want better light, proper tools, a line, and ideally another pair of eyes."
        )
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(run_dir / "turn_011.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_011.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Nell stays oriented on the crawl-space opening and extends the flashlight, camera, "
                "and walking stick one controlled reach deeper."
            )
            decision.update(
                {
                    "turn": 12,
                    "gm_visible_reply": previous,
                    "perceived_facts": ["Nell is outside the house and needs tools before returning."],
                    "active_goal": "Probe deeper into the crawl space.",
                    "response_contract": {
                        "intent": "probe_deeper_crawl_space_from_marked_exit",
                        "acceptable_resolutions": ["deeper result"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("continues crawl-space after exiting house" in item for item in p1["evidence"])
        )

    def test_p1_flags_changing_floor_while_half_inside_crawl_space(self):
        mod = load_module()
        previous = (
            "You ease yourself down only partway, keeping your legs braced toward the opening and the "
            "basement stairs fixed in memory behind you. [roll]Careful search of the crawl space: "
            "1d100 = 22 vs Spot Hidden 70 -- Hard success.[/roll] With the beam held low and steady, "
            "you catch letters carved into the crawl-space wall: Chapel of Contemplation. Nothing in "
            "the space immediately shifts or surges at you. You are still half out of the opening, "
            "with your retreat line clear."
        )
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(run_dir / "turn_013.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_013.txt").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c13"}\n'
                '  CheckResolved {"check_id":"c13"}\n'
            )
            decision = player_decision_signal(
                "Evelyn tests each stair with the walking stick and goes upstairs to search the bedrooms."
            )
            decision.update(
                {
                    "turn": 14,
                    "gm_visible_reply": previous,
                    "perceived_facts": [
                        "Evelyn is still half out of the crawl-space opening with retreat clear."
                    ],
                    "active_goal": "Check the upper floor next.",
                    "candidate_actions": ["Go upstairs", "Search bedrooms"],
                    "response_contract": {
                        "intent": "cautious_upper_floor_search",
                        "acceptable_resolutions": ["specific upper-floor clues"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("changes floor while still in crawl-space position" in item for item in p1["evidence"])
        )

    def test_p1_flags_jumping_to_chapel_while_half_inside_crawl_space(self):
        mod = load_module()
        previous = (
            "[roll]Spot Hidden -- Inspect the wall around the carving for seams, hollows, or hidden structure: "
            "1d100 = 98 vs 55 -- failure.[/roll] You keep your body angled to the crawl-space wall, "
            "the marked stairs behind you, and work the scene like evidence rather than invitation. "
            "The words are real enough in the flashlight beam: Chapel of Contemplation, carved into "
            "the wall itself. No clear chamber edge. No object glint. No bones or remains. No second "
            "mark. No trustworthy airflow. If there is a continuation here, it does not yield itself "
            "to a careful surface read."
        )
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(run_dir / "turn_013.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_013.txt").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c13"}\n'
                '  CheckResolved {"check_id":"c13"}\n'
            )
            decision = player_decision_signal(
                "The chapel search did not give Evelyn the concrete record she wanted, "
                "so she drives to the chapel and searches the pulpit again."
            )
            decision.update(
                {
                    "turn": 14,
                    "gm_visible_reply": previous,
                    "perceived_facts": [
                        "Evelyn is still half inside the crawl space with retreat clear."
                    ],
                    "active_goal": "Retry the Chapel search with a different method.",
                    "selection_rationale": "The crawl-space wall says Chapel of Contemplation.",
                    "candidate_actions": [
                        "Drive to the Chapel",
                        "Search the pulpit again",
                    ],
                    "response_contract": {
                        "intent": "retry_chapel_with_distinct_visual_method_after_failure",
                        "acceptable_resolutions": ["specific chapel clue"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("jumps to chapel while still in crawl-space position" in item for item in p1["evidence"])
        )

    def test_p1_flags_player_decision_contradicting_previous_failure(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(
                run_dir / "turn_001.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "[roll]Search chapel records — 1d100: 88 vs 60 — failure.[/roll]\n"
                            "No clear burial instruction. No definite mention of Walter Corbitt."
                        ),
                    }
                ],
            )
            (run_dir / "explain_001.txt").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
            )
            decision = player_decision_signal("I go to the Corbitt House.")
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": "The chapel search revealed that Corbitt was buried in the basement.",
                    "perceived_facts": ["Corbitt was buried in the basement"],
                    "last_action_result": "The chapel search succeeded and produced the burial clue.",
                    "risk_assessment": ["Do not assume hidden facts not yet revealed"],
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("contradicts previous failed result" in item for item in p1["evidence"])
        )

    def test_p1_allows_later_success_in_same_turn_after_earlier_failed_roll(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            previous = (
                "[roll]Survey chapel exterior — 1d100: 71 vs 60 — failure.[/roll]\n"
                "Outside yields caution, not certainty.\n"
                "[roll]Search surviving chapel records — 1d100: 12 vs 80 — Extreme success.[/roll]\n"
                "Walter Corbitt was buried in the basement of his house, in accordance with his wishes."
            )
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
                '  DiceRolled {"check_id":"c2"}\n'
                '  CheckResolved {"check_id":"c2"}\n'
                '  WorldFactChanged {"fact":"burial_clue"}\n'
            )
            decision = player_decision_signal("Evelyn goes to the Corbitt House.")
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": (
                        "That care pays off. Walter Corbitt was buried in the basement "
                        "of his house, in accordance with his wishes."
                    ),
                    "perceived_facts": ["Corbitt was buried in the basement"],
                    "last_action_result": "Previous action succeeded; use only concrete facts that the GM actually exposed.",
                    "active_goal": "Inspect the house named by the successful Chapel record.",
                    "candidate_actions": [
                        "Go to the Corbitt House",
                        "Photograph the house exterior",
                        "Check basement access later",
                    ],
                }
            )
            self.assertFalse(
                mod.decision_contradicts_previous_failed_result(decision, previous)
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "GREEN")

    def test_p1_flags_player_denying_visible_chapel_symbol_success(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            previous = (
                "[roll]Careful low-angle visual sweep for visible carvings, floor marks, desk scars, "
                "or paper impressions in the ruined chapel: 1d100 = 28 vs target 60 — Hard success.[/roll]\n"
                "This method pays off. With the flashlight held low, fine ridges wake up along the "
                "grain of old wood near the pulpit; on one surviving piece of record furniture, plus "
                "in nearby marked surfaces, you catch a repeated form that is not random damage. "
                "It is a symbol. Not a full hidden cache, but a distinct, visible mark that your "
                "photographs and quick rubbings can preserve: the same emblem associated with the "
                "Chapel of Contemplation's records. You have something concrete from the chapel after all."
            )
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
                '  PlayerLearnedFact {"fact_id":"handout_9"}\n'
            )
            decision = player_decision_signal(
                "Evelyn treats the last reply as arrival at the Chapel, not as the search result, "
                "and searches the chapel again for a concrete record or symbol."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "last_action_result": "Previous action succeeded; use only concrete facts that the GM actually exposed.",
                    "active_goal": "Resolve the Chapel lead that has not produced concrete player-usable findings yet.",
                    "selection_rationale": "The last reply established arrival and atmosphere, not the outcome of the Chapel search.",
                    "response_contract": {
                        "intent": "continue_chapel_search_after_scene_establishment",
                        "acceptable_resolutions": ["specific Chapel finding"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("denies visible chapel payoff" in item for item in p1["evidence"])
        )

    def test_p1_flags_player_denying_visible_chinese_chapel_burial_success(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            previous = (
                "[roll]Search the ruined Chapel records for Corbitt links: "
                "1d100=[24] target 60, result strong_success[/roll]\n"
                "这里最具体的一条记录写得很清楚：沃尔特·科比特依照他本人的意愿，"
                "被埋在他那栋房子的地下室里。教堂自己的记录又把科比特，直接指回了那栋房子的地下室。"
            )
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
                '  PlayerLearnedFact {"fact_id":"corbitt_burial"}\n'
            )
            decision = player_decision_signal(
                "Evelyn treats the last reply as arrival at the Chapel, not as the search result, "
                "and searches the chapel again for a concrete record or symbol."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "last_action_result": "Previous action succeeded; use only concrete facts that the GM actually exposed.",
                    "active_goal": "Resolve the Chapel lead that has not produced concrete player-usable findings yet.",
                    "selection_rationale": "The last reply established arrival and atmosphere, not the outcome of the Chapel search.",
                    "response_contract": {
                        "intent": "continue_chapel_search_after_scene_establishment",
                        "acceptable_resolutions": ["specific Chapel finding"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("denies visible chapel payoff" in item for item in p1["evidence"])
        )

    def test_p1_allows_player_marking_chapel_record_search_failed_after_exterior_success(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            previous = (
                "[roll]Circle the ruined chapel exterior for exits, fresh tracks, and signs of recent use: "
                "1d100 = 35 vs target 55 -- success.[/roll]\n"
                "The exterior circuit finds more than one practical way back out and signs of later disturbance.\n"
                "Inside, Evelyn photographs anything distinctive before touching it and works through papers, "
                "wall marks, pulpits, and drawers.\n"
                "[roll]Search the surviving chapel interior records and cabinets for Corbitt links, symbols, "
                "and documentary traces: 1d100 = 57 vs target 55 -- failure.[/roll]\n"
                "On this pass the surviving material defeats a clean read. "
                "You do not secure a firm documentary link from this search to carry straight back to Corbitt."
            )
            self.assertFalse(mod.previous_has_concrete_chapel_payoff(previous))
            write_jsonl(run_dir / "turn_004.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_004.txt").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"outer"}\n'
                '  CheckResolved {"check_id":"outer"}\n'
                '  DiceRolled {"check_id":"inner"}\n'
                '  CheckResolved {"check_id":"inner"}\n'
            )
            chapel_attempt = player_decision_signal(
                "Evelyn goes to the Chapel of Contemplation and searches surviving records and marks."
            )
            chapel_attempt.update(
                {
                    "turn": 4,
                    "gm_visible_reply": "The court records named the Chapel of Contemplation.",
                    "active_goal": "Investigate the Chapel lead.",
                    "response_contract": {
                        "intent": "investigate_chapel_for_records_symbols_and_corbitt_links",
                        "acceptable_resolutions": ["specific chapel record", "clear no-hit"],
                    },
                }
            )
            decision = player_decision_signal(
                "The chapel search did not give Evelyn the concrete record she wanted, "
                "so she marks that as a failed lead rather than a discovery."
            )
            decision.update(
                {
                    "turn": 5,
                    "gm_visible_reply": previous,
                    "last_action_result": "Previous action was adjudicated as a failure or blocked access; do not treat requested facts as learned.",
                    "active_goal": "Retry only a distinct visible method, then leave if it still yields no concrete clue.",
                    "selection_rationale": "The interior chapel records search failed even though the exterior approach succeeded.",
                    "response_contract": {
                        "intent": "retry_chapel_with_distinct_visual_method_after_failure",
                        "acceptable_resolutions": ["specific visible mark", "clear no-hit"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [chapel_attempt, decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "GREEN")

    def test_p1_flags_player_ignoring_newspaper_official_records_lead(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            previous = (
                "[roll]Search Boston Globe morgue for Corbitt House incidents: "
                "1d100 = 6 vs Library Use 70 — Extreme Success[/roll]\n"
                "The clippings suggest disturbance, illness, and neighborhood concern. "
                "More importantly, the newspaper indexing points toward a stronger next "
                "line of inquiry: serious police files or higher court records tied to "
                "the house's uglier incidents."
            )
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
            )
            decision = player_decision_signal(
                "Evelyn goes to the Corbitt House in daylight and tests the least exposed lock."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "last_action_result": "Previous action succeeded; use only concrete facts that the GM actually exposed.",
                    "active_goal": "Inspect the Corbitt House exterior and choose an entry only from visible conditions.",
                    "response_contract": {
                        "intent": "inspect_house_exterior_and_safe_entry",
                        "acceptable_resolutions": ["visible entry points"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("ignores newspaper official-records lead" in item for item in p1["evidence"])
        )

    def test_p1_does_not_treat_clerk_newspaper_people_gripe_as_newspaper_lead(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            previous = (
                "[roll]Gain access to court and police records on Reverend Michael Thomas and "
                "the Chapel of Contemplation — 1d100: 54 vs 70 — Success.[/roll] "
                "After a muttered complaint about newspaper people, the records clerk lets "
                "Evelyn inspect the restricted entry. In 1912, disappeared children led to "
                "a police raid on the Chapel of Contemplation. Reverend Michael Thomas was "
                "imprisoned afterward and later escaped. From here, the police file makes "
                "the Chapel of Contemplation a concrete next lead. The old Corbitt place is also still waiting."
            )
            write_jsonl(run_dir / "turn_003.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_003.txt").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
            )
            decision = player_decision_signal(
                "Evelyn waits for daylight and searches the Chapel of Contemplation for records and symbols."
            )
            decision.update(
                {
                    "turn": 4,
                    "gm_visible_reply": previous,
                    "last_action_result": "Previous action succeeded; use only concrete facts that the GM actually exposed.",
                    "active_goal": "Inspect the Chapel of Contemplation lead with daylight safety precautions.",
                    "candidate_actions": [
                        "Circle and search the chapel carefully",
                        "Continue public records research elsewhere",
                        "Move to the house exterior with uncertainty explicitly noted",
                    ],
                    "response_contract": {
                        "intent": "investigate_chapel_for_records_symbols_and_corbitt_links",
                        "acceptable_resolutions": ["visible records", "symbols"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "GREEN")

    def test_p1_flags_player_leaving_official_records_for_house_after_boundary_map(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            previous = (
                "[roll]Law — navigate higher courts and police public indexes for Corbitt House "
                "records — 1d100: 34 vs 45 — success.[/roll]\n"
                "They do not simply hand over a neat file tied to the house address alone. "
                "Where records are restricted, address-only requests do not open sealed or "
                "privileged material. Your success buys a firmer map of the paper trail and "
                "its legal boundaries. The next legal public source is the newspaper morgue "
                "and archive trail."
            )
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
            )
            decision = player_decision_signal(
                "Evelyn goes to the Corbitt House in daylight and tests the least exposed lock."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "last_action_result": "Previous action succeeded; use only concrete facts that the GM actually exposed.",
                    "active_goal": "Inspect the Corbitt House exterior and choose an entry only from visible conditions.",
                    "response_contract": {
                        "intent": "inspect_house_exterior_and_safe_entry",
                        "acceptable_resolutions": ["visible entry points"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("leaves official-records boundary map for house" in item for item in p1["evidence"])
        )

    def test_p1_flags_player_ignoring_actionable_success_lead(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(
                run_dir / "turn_001.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "[roll]Basement search — 1d100: 15 vs 65 — Hard Success.[/roll]\n"
                            "The basement shows disturbed earth, scrape marks, and a suspicious concealed section."
                        ),
                    }
                ],
            )
            (run_dir / "explain_001.txt").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
            )
            decision = player_decision_signal(
                "Evelyn goes upstairs and checks bedrooms from the landing."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": (
                        "The basement shows disturbed earth, scrape marks, "
                        "and a suspicious concealed section."
                    ),
                    "perceived_facts": [
                        "The basement contains disturbed earth and a suspicious concealed section."
                    ],
                    "last_action_result": "The basement search succeeded and exposed a concrete lead.",
                    "active_goal": "Search the upper floor next.",
                    "candidate_actions": [
                        "Go upstairs",
                        "Check bedrooms",
                        "Inspect the landing",
                    ],
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertIn("ignores actionable success lead", p1["evidence"][0])

    def test_p1_flags_player_claiming_speculative_basement_door_as_concrete_lead(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            previous = (
                "The most concrete visible affordance is the door directly in front of you. "
                "The air smells cold and wet, with basement damp everywhere. "
                "What you still cannot prove is whether it is locked, what lies beyond it, "
                "and whether it merely leads to another cellar space or conceals something more significant."
            )
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Evelyn stays in the basement because the last search produced a concrete lead. "
                "She photographs the disturbed earth, scrape marks, and suspicious concealed section."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "perceived_facts": ["A closed reachable basement door exists."],
                    "last_action_result": "Previous action produced narration; treat only explicit visible facts as established.",
                    "active_goal": "Follow up the successful basement clue before leaving the area.",
                    "candidate_actions": [
                        "Photograph and probe the disturbed earth from a safe stance",
                        "Trace the scrape marks and test the suspicious section",
                    ],
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("claims speculative basement detail as concrete lead" in item for item in p1["evidence"])
        )

    def test_p1_flags_unconditional_unlock_after_unproven_exterior_key_fit(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            previous = (
                "From where Evelyn stands now, the clearest actionable thing is the front door. "
                "It sits ahead of you at the end of the front approach, with the open route back "
                "to the street still behind you. What you cannot yet prove is whether this key "
                "fits that lock cleanly, whether the door is merely locked or swollen shut, or "
                "whether anything on the other side is immediately visible once opened. "
                "The door is the affordance; the unknown begins at the lock."
            )
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Evelyn unlocks the safest exterior door and steps inside with the flashlight low."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "perceived_facts": [
                        "The front door is a visible exterior affordance, but the key fit is not proven."
                    ],
                    "last_action_result": "Previous action established an exterior affordance, not that the key opens it.",
                    "active_goal": "Enter the house through the safest exterior door.",
                    "candidate_actions": [
                        "Unlock the safest exterior door",
                        "Ask for another affordance",
                        "Circle the house again",
                    ],
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("unconditionally unlocks unproven exterior door" in item for item in p1["evidence"])
        )

    def test_p1_flags_unconditional_unlock_after_key_only_fits_enough_to_test(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            previous = (
                "You do mark the obvious approaches: front access, rear access, side exposure, "
                "and a likely cellar way. When you finally choose the least exposed lock and try "
                "one of Knott's keys, it does fit well enough to test--but you are not yet rewarded "
                "with any obvious discovery beyond that fact. The old house remains closed, watchful, "
                "and not yet explained. You are outside, still with an exit route behind you, at the "
                "point of testing that exterior access further if you wish."
            )
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Evelyn unlocks the safest exterior door she identified, opens it from the side, "
                "and steps just inside with the flashlight low."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "perceived_facts": [
                        "A key fits well enough to test one exterior lock, but the house remains closed."
                    ],
                    "last_action_result": "Previous action established a lock-test affordance, not an opened door.",
                    "active_goal": "Enter the house through the safest exterior door.",
                    "candidate_actions": [
                        "Unlock the safest exterior door",
                        "Test the lock conditionally",
                        "Circle the house again",
                    ],
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("unconditionally unlocks unproven exterior door" in item for item in p1["evidence"])
        )

    def test_p1_allows_unlock_after_lock_works_and_house_can_be_entered(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            previous = (
                "From where Evelyn stands, the one solid thing in front of her is simple: "
                "The lock she tested works. The house can be entered through that door. "
                "Physically, that means the actionable point is the threshold itself: one closed "
                "exterior door at the least exposed side of the property. The door is real, "
                "accessible, and matched to Knott's key. What she cannot prove from here is "
                "everything beyond the door: whether the interior is merely abandoned, structurally "
                "unsafe, recently entered by someone else, or hiding anything connected to Corbitt."
            )
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Evelyn unlocks the safest exterior door she identified, opens it from the side, "
                "and steps just inside with the flashlight low."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "perceived_facts": [
                        "The tested lock works and the matched exterior door can be entered."
                    ],
                    "last_action_result": "Previous action established a working matched lock.",
                    "active_goal": "Enter the house through the tested exterior door.",
                    "candidate_actions": [
                        "Open the tested exterior door from the side",
                        "Back away from the house",
                    ],
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "GREEN")
        self.assertEqual(p1["metrics"]["inconsistent_decisions"], 0)

    def test_p1_flags_safest_entry_claim_after_failed_exterior_safety_read(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            previous = (
                "[roll]Survey the Corbitt House exterior for entrances, condition, tracks, "
                "damage, animal signs, and sight lines: 1d100 = 94 vs 60 -- Failure.[/roll] "
                "There is a front approach, a less exposed secondary way around the side or rear, "
                "and access down toward the cellar level, but nothing from this pass gives her a "
                "clean, confident read on which points of entry are safest, which have been tampered "
                "with most recently, or whether any marks in the ground truly matter. When she tries "
                "the least exposed matching lock, the key fits the property well enough to confirm "
                "Knott did not send her here blind. The threshold remains shut for the moment, but "
                "this entrance appears to be a real way in if she chooses to commit to it. She is "
                "outside the house still, with at least one workable entrance identified, but without "
                "the sharper exterior read she wanted first."
            )
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Evelyn unlocks the safest exterior door she identified, opens it from the side, "
                "and steps just inside with the flashlight low."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "perceived_facts": [
                        "A workable entrance exists, but the exterior survey failed to establish which entry is safest."
                    ],
                    "last_action_result": "Previous action failed to produce a trustworthy safety read.",
                    "active_goal": "Enter the house through the safest exterior door.",
                    "candidate_actions": [
                        "Unlock the safest exterior door",
                        "Use the workable entrance without treating it as safest",
                        "Retreat and ask neighbors",
                    ],
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("claims safest exterior entry after failed safety read" in item for item in p1["evidence"])
        )

    def test_p1_flags_house_jump_before_newspaper_access_gate_is_resolved(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            previous = (
                "At the Globe, the obstacle is not the searching itself yet — it is access. "
                "Ink, paper dust, and the hot thrum of machinery fill the building, and staff "
                "can point Evelyn toward the basement clippings morgue, but the files are not "
                "simply open for a casual walk-in. An editor, Arty Wilmot, controls access. "
                "How does Evelyn try to get in? If she leans on respectability, credentials, "
                "and a reasonable request, that suggests Persuade. If she tries charm, pressure, "
                "or a quick bluff, that would point elsewhere. Tell me her approach, and I'll resolve it."
            )
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Evelyn Price goes to the Corbitt House in daylight, circles the exterior, "
                "and tests the least exposed matching lock."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "perceived_facts": [
                        "The Globe archive files are not open until Evelyn resolves access with Arty Wilmot."
                    ],
                    "last_action_result": "The previous reply asked how Evelyn tries to get archive access.",
                    "active_goal": "Inspect the Corbitt House exterior.",
                    "candidate_actions": [
                        "Circle the house exterior",
                        "Enter through a matching lock",
                        "Return later to the newspaper morgue",
                    ],
                    "response_contract": {
                        "intent": "inspect_house_exterior_and_safe_entry",
                        "acceptable_resolutions": ["visible entry points", "lock result"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("leaves newspaper access gate unresolved" in item for item in p1["evidence"])
        )

    def test_p1_flags_house_jump_before_try_past_editor_gate_is_resolved(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            previous = (
                "The Globe's front offices hit you with ink, paper dust, and the heat of the "
                "presses somewhere deeper in the building. You are directed instead to the "
                "person who controls access: Editor Arty Wilmot. Your press card may get you "
                "in the conversation, but not automatically into the morgue. If Evelyn wants "
                "the clippings morgue opened, this is a real point of friction rather than a "
                "foregone courtesy. How does she try to get past him? Does she lean on "
                "professional courtesy, polite persuasion, fast-talking newsroom urgency, "
                "or blunt pressure?"
            )
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Evelyn Marsh goes to the Corbitt House in daylight and circles the exterior."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "perceived_facts": [
                        "The Globe archive files are not open until Evelyn resolves access with Arty Wilmot."
                    ],
                    "last_action_result": "The previous reply asked how Evelyn tries to get past the editor.",
                    "active_goal": "Inspect the Corbitt House exterior.",
                    "candidate_actions": [
                        "Circle the house exterior",
                        "Press Wilmot professionally",
                        "Return later to the newspaper morgue",
                    ],
                    "response_contract": {
                        "intent": "inspect_house_exterior_and_safe_entry",
                        "acceptable_resolutions": ["visible entry points"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("leaves newspaper access gate unresolved" in item for item in p1["evidence"])
        )

    def test_p1_flags_claiming_closed_matched_door_is_already_open(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            previous = (
                "The lock she tested works. The house can be entered through that door. "
                "Physically, that means the actionable point is the threshold itself: one closed "
                "exterior door at the least exposed side of the property, with the yard and her "
                "line back out behind her. The door is real, accessible, and matched to Knott's key."
            )
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "With the side entrance already open, Evelyn keeps one hand near the frame and steps just inside."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "perceived_facts": [
                        "The lock works, but the exterior door was described as closed."
                    ],
                    "last_action_result": "Previous action established a working matched lock, not an already-open doorway.",
                    "active_goal": "Enter the house through the tested exterior door.",
                    "candidate_actions": [
                        "Treat the side entrance as already open",
                        "Step inside",
                    ],
                    "response_contract": {
                        "intent": "enter_threshold_and_map_ground_floor_affordances",
                        "acceptable_resolutions": ["threshold footing result"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(any("claims closed exterior door is already open" in item for item in p1["evidence"]))

    def test_p1_allows_search_targets_after_speculative_basement_door(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            previous = (
                "What you can actually perceive, plainly, is this: the next meaningful way "
                "forward on this floor is the way down. There is a basement door or stair access "
                "on the ground floor. What remains uncertain is everything that would matter once "
                "that threshold is crossed: whether the way down is merely dark and damp, whether "
                "it is locked, what lies beyond it, and whether it conceals anything significant."
            )
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Evelyn ties a handkerchief to the basement door handle, leaves the door wedged open, "
                "and descends one step at a time. She looks for drafts, loose brick, disturbed earth, "
                "scrape marks, hidden panels, a body, a ritual object, or any moving threat."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "perceived_facts": ["A basement door or stair access is visible on the ground floor."],
                    "last_action_result": "Previous action produced narration; treat only explicit visible facts as established.",
                    "active_goal": "Descend toward the cellar only after establishing a retreat path.",
                    "candidate_actions": [
                        "Descend slowly with light and retreat line",
                        "Listen and inspect from the top of the stairs first",
                        "Leave to get help if the basement is structurally unsafe",
                    ],
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "GREEN")

    def test_p1_flags_player_claiming_return_stair_clarification_as_basement_lead(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            previous = (
                "The single most concrete, presently established affordance is the basement "
                "stair behind you—your marked return route. Exact location: immediately behind "
                "and slightly above her, rising back to the ground floor through the stair she "
                "just descended. Safely reachable from your current position? Yes—it is the one "
                "route here that is already confirmed, legible, and immediately reachable without "
                "pressing deeper into the basement. What you do not yet have is a confirmed "
                "hidden panel, body, ritual object, moving figure, or identified side passage."
            )
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Evelyn stays in the basement because the last search produced a concrete lead. "
                "She photographs the disturbed earth, scrape marks, and suspicious concealed section."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "perceived_facts": ["The marked basement stair is the confirmed return route."],
                    "last_action_result": "Previous action produced narration; treat only explicit visible facts as established.",
                    "active_goal": "Follow up the successful basement clue before leaving the area.",
                    "candidate_actions": [
                        "Photograph and probe the disturbed earth from a safe stance",
                        "Trace the scrape marks and test the suspicious section",
                    ],
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("claims basement return route as concrete lead" in item for item in p1["evidence"])
        )

    def test_p1_flags_chapel_retry_after_house_ground_floor_failure(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            previous = (
                "[roll]Cautious room-by-room search of the Corbitt House ground floor: "
                "1d100 = 96 vs target 55. Failure.[/roll] "
                "You are inside the Corbitt House on the ground floor. Nothing on this pass "
                "gives you a neat decisive document like the chapel record did."
            )
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
            )
            decision = player_decision_signal(
                "The chapel search did not give Evelyn the concrete record she wanted, "
                "so she changes method and searches the chapel again."
            )
            decision["response_contract"]["intent"] = "retry_chapel_with_distinct_visual_method_after_failure"
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "perceived_facts": ["Evelyn is inside the Corbitt House ground floor."],
                    "last_action_result": "Previous action was adjudicated as a failure or blocked access; do not treat requested facts as learned.",
                    "active_goal": "Respond to the failed chapel search by changing method.",
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(any("jumps back to chapel from house interior" in item for item in p1["evidence"]))

    def test_p1_flags_leaving_clarified_basement_section_for_upstairs(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            previous = (
                "The concealed section itself is in the basement near disturbed earth and scrape marks. "
                "It is a few steps in front of Evelyn, with the stairs and marked retreat route still behind her."
            )
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Evelyn tests each stair with the walking stick and goes upstairs to search the bedrooms."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "perceived_facts": ["A concealed basement section is a few steps in front of Evelyn."],
                    "last_action_result": "Previous action produced narration; treat only explicit visible facts as established.",
                    "active_goal": "Check the upper floor next.",
                    "candidate_actions": ["Go upstairs", "Search bedrooms"],
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(any("abandons clarified basement section" in item for item in p1["evidence"]))

    def test_p1_flags_leaving_successful_basement_disturbance_for_upstairs(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            previous = (
                "[roll]Cautious descent into the basement while searching for hidden signs, "
                "disturbances, and immediate threats — 1d100: 38 vs Spot Hidden 65 — success.[/roll]. "
                "There are signs of disturbance worth following more closely, not yet a full explanation, "
                "but enough to say this is not just an untouched junk cellar. You are now down in the "
                "basement proper, with the stairs and your marked exit still behind you, and at least "
                "one area below that deserves closer inspection."
            )
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
            )
            decision = player_decision_signal(
                "Evelyn tests each stair with the walking stick and goes upstairs to search the bedrooms."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "perceived_facts": [
                        "Evelyn is in the basement proper and saw disturbance worth following."
                    ],
                    "last_action_result": "Previous action succeeded and exposed a basement disturbance.",
                    "active_goal": "Check the upper floor next.",
                    "candidate_actions": ["Go upstairs", "Search bedrooms"],
                    "response_contract": {
                        "intent": "cautious_upper_floor_search",
                        "acceptable_resolutions": ["specific upper-floor clues"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(any("ignores actionable success lead" in item for item in p1["evidence"]))

    def test_p1_flags_player_changing_floors_after_failed_basement_stair_position(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(
                run_dir / "turn_001.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "[roll]Careful descent and search of the basement stairs — "
                            "1d100: 90 vs 65 — Failure.[/roll]\n"
                            "Evelyn is partway down the basement stairs, with the marked "
                            "retreat behind her and the unseen cellar below now fully aware "
                            "of her presence."
                        ),
                    }
                ],
            )
            (run_dir / "explain_001.txt").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
            )
            decision = player_decision_signal(
                "Evelyn goes upstairs and checks bedrooms from the landing."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": (
                        "Evelyn is partway down the basement stairs, with the marked "
                        "retreat behind her and the unseen cellar below now fully aware "
                        "of her presence."
                    ),
                    "perceived_facts": [
                        "Evelyn is still partway down the basement stairs."
                    ],
                    "last_action_result": "The basement stair descent failed and changed the immediate position.",
                    "active_goal": "Search the upper floor next.",
                    "candidate_actions": [
                        "Go upstairs",
                        "Check bedrooms",
                        "Inspect the landing",
                    ],
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertIn("changes floor after failed basement stair position", p1["evidence"][0])

    def test_p1_flags_player_changing_floors_while_backing_out_of_basement(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            previous = (
                "[roll]Listen for any response from the cellar after the loud stair-creak -- "
                "1d100: 85 vs 45 -- Failure.[/roll] Evelyn goes still on the stair, lowering "
                "her weight and holding the flashlight beam steady into the cellar dark. For a few "
                "seconds there is nothing clear enough to trust. She begins to retreat exactly as planned--one "
                "tested step, then another, back toward the open basement door and the handkerchief marker "
                "above. The stairs complain softly under careful pressure, but nothing immediately rushes "
                "up at her. She is now backing up toward the doorway, still facing down into the cellar, "
                "with her escape line intact for the moment."
            )
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
            )
            decision = player_decision_signal(
                "Evelyn goes upstairs and checks bedrooms from the landing."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "perceived_facts": [
                        "Evelyn is backing up toward the basement doorway."
                    ],
                    "last_action_result": "The basement recovery is still in progress.",
                    "active_goal": "Search the upper floor next.",
                    "candidate_actions": [
                        "Go upstairs",
                        "Check bedrooms",
                        "Inspect the landing",
                    ],
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertIn("changes floor after failed basement stair position", p1["evidence"][0])

    def test_p1_flags_upper_floor_after_failed_descent_to_lower_stairs(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            previous = (
                "[roll]Cautious descent into the cellar while searching for hidden disturbance "
                "or immediate threat: 1d100 = 81 vs Spot Hidden 65 — Failure.[/roll] "
                "The handkerchief on the door handle gives you a pale point of reassurance "
                "behind you, and the wedged cellar door leaves a narrow seam of safer air above. "
                "The upper steps hold. Your flashlight picks out rough wall, shadowed treads, "
                "and the suggestion of packed earth below, but the gloom swallows detail. "
                "You have not confirmed a loose brick, hidden panel, scrape trail, disturbed "
                "earth, body, or ritual object. What you do know is narrower but real: the "
                "route back remains marked and open behind you, the stairs have held so far, "
                "and the cellar proper lies just ahead of your light."
            )
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
            )
            decision = player_decision_signal(
                "Evelyn tests each stair with the walking stick, goes upstairs, "
                "and searches the bedrooms from the landing."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "perceived_facts": [
                        "Evelyn is on the lower part of the cellar stairs with the route back marked."
                    ],
                    "last_action_result": "The basement descent failed and left a current stair position.",
                    "active_goal": "Search the upper floor next.",
                    "candidate_actions": [
                        "Go upstairs",
                        "Search bedrooms",
                        "Ignore the failed cellar position",
                    ],
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertIn("changes floor after failed basement stair position", p1["evidence"][0])

    def test_p1_flags_upper_floor_after_clarified_way_down_priority(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            previous = (
                "What you can **actually** act on from here is simple: the house has "
                "become a set of choices you can physically see. On this floor, the "
                "clearest one is the way **down**: the basement door or stair access "
                "is a real, present route, not a theory. It is part of the ground floor "
                "space you've already been working through, and it is not beyond your "
                "retreat path. The stair upward remains visible, but the way down is "
                "the grounded affordance."
            )
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Evelyn tests each stair with the walking stick, goes upstairs, "
                "and searches the bedrooms from the landing."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "perceived_facts": [
                        "The clearest visible route is the way down to the basement."
                    ],
                    "last_action_result": "The Keeper clarified the current actionable route.",
                    "active_goal": "Check the upper floor next.",
                    "candidate_actions": [
                        "Go upstairs",
                        "Search bedrooms",
                        "Ignore the basement route for now",
                    ],
                    "response_contract": {
                        "intent": "cautious_upper_floor_search",
                        "acceptable_resolutions": ["specific upper-floor clues"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("ignores clarified basement route priority" in item for item in p1["evidence"])
        )

    def test_p1_flags_player_claiming_refusal_after_successful_official_access(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(
                run_dir / "turn_001.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "[roll]Gain access to court and police records: 1d100 = 2 vs target 60 — Extreme Success.[/roll] "
                            "The file trail shows a raid in 1912 and affidavits about disappeared children."
                        ),
                    }
                ],
            )
            (run_dir / "explain_001.txt").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
            )
            decision = player_decision_signal(
                "Evelyn does not pretend she obtained the restricted police or court file. "
                "She writes down the refusal and searches newspapers instead."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": (
                        "The file trail shows a raid in 1912 and affidavits about disappeared children."
                    ),
                    "perceived_facts": [
                        "Official court/police access was refused or blocked."
                    ],
                    "last_action_result": "Previous action was refused or blocked.",
                    "candidate_actions": [
                        "Search newspaper archive after refusal",
                        "Return to Hall of Records",
                    ],
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("claims refusal after successful access" in item for item in p1["evidence"])
        )

    def test_p1_flags_refusal_claim_after_allowed_inspect_official_records(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            previous = (
                "[roll]Convince the court or police records clerk to allow restricted index "
                "or redacted-file access -- 1d100: 33 vs 60 -- Success.[/roll] "
                "You are not handed a free run of the files, but you are allowed to inspect "
                "the relevant index entries and a constrained record trail. You leave those "
                "offices with real notes in hand: the raid, the deaths, and Thomas's later "
                "imprisonment and escape."
            )
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
            )
            decision = player_decision_signal(
                "Evelyn does not pretend she obtained the restricted police or court file. "
                "She writes down the refusal and searches newspapers instead."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "perceived_facts": [
                        "Official court/police access was refused or blocked."
                    ],
                    "last_action_result": "Previous action was refused or blocked.",
                    "candidate_actions": [
                        "Search newspaper archive after refusal",
                        "Return to Hall of Records",
                    ],
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("claims refusal after successful access" in item for item in p1["evidence"])
        )

    def test_p1_flags_retrying_chapel_before_any_chapel_attempt(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(
                run_dir / "turn_001.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "[roll]Boston Globe morgue search — failure.[/roll] "
                            "The next avenues are police files, higher court records, "
                            "or direct inquiry into the chapel connection."
                        ),
                    }
                ],
            )
            (run_dir / "explain_001.txt").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
            )
            first = player_decision_signal(
                "I search newspaper archives for Corbitt House and Macario reports."
            )
            first.update(
                {
                    "turn": 1,
                    "response_contract": {
                        "intent": "search_newspaper_archive_after_hall_records_failure",
                        "acceptable_resolutions": ["concrete clipping facts", "clear failure"],
                    },
                }
            )
            second = player_decision_signal(
                "The chapel search did not give Evelyn the concrete record she wanted, "
                "so she changes method and takes rubbings at the chapel."
            )
            second.update(
                {
                    "turn": 2,
                    "gm_visible_reply": "The newspaper search failed but mentions a chapel connection.",
                    "active_goal": "Respond to the failed chapel search by changing method.",
                    "candidate_actions": [
                        "Recheck chapel surfaces",
                        "Leave the chapel",
                        "Search another archive",
                    ],
                    "response_contract": {
                        "intent": "retry_chapel_with_distinct_visual_method_after_failure",
                        "acceptable_resolutions": ["specific visible mark", "clear no-clue result"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [first, second])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("assumes a chapel search before any recorded chapel attempt" in item for item in p1["evidence"])
        )

    def test_p1_flags_player_using_hidden_scenario_terms_not_in_visible_history(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            (run_dir / "opening.txt").write_text(
                "Mr. Knott asks you to investigate the Corbitt House and gives you keys.",
                encoding="utf-8",
            )
            write_jsonl(
                run_dir / "turn_001.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "Knott says he inherited the property papers but is afraid "
                            "he knows little about the Macario family."
                        ),
                    }
                ],
            )
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "I search municipal records for Walter Corbitt and the Chapel of Contemplation."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": "Knott is afraid and knows little about the Macarios.",
                    "perceived_facts": [
                        "Visible records or testimony connect the chapel to serious 1912 trouble."
                    ],
                    "active_goal": "Find the Chapel of Contemplation trail.",
                    "candidate_actions": [
                        "Search for Walter Corbitt",
                        "Search for the Chapel of Contemplation",
                    ],
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertIn("uses hidden/unseen term", p1["evidence"][0])

    def test_p1_hidden_term_scan_ignores_gm_visible_reply_wording(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            (run_dir / "opening.txt").write_text(
                "Mr. Knott asks you to investigate the Corbitt House and gives you keys.",
                encoding="utf-8",
            )
            previous = (
                "At the Hall of Records, clerks bring out indexes with the weary efficiency "
                "of people who have spent years buried in dust and ink. The executor record "
                "names Reverend Michael Thomas of the Chapel of Contemplation, closed in 1912."
            )
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Evelyn asks the courts and police for 1912 Chapel records."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "perceived_facts": [
                        "Corbitt's executor trail is tied to Reverend Michael Thomas.",
                        "The Chapel of Contemplation is a concrete lead.",
                        "The chapel trail includes a 1912 date.",
                    ],
                    "active_goal": "Test whether the 1912 chapel lead exists in court or police records.",
                    "candidate_actions": [
                        "Ask courts and police for 1912 Chapel records",
                        "Search newspaper archive for the 1912 closure",
                    ],
                    "response_contract": {
                        "intent": "access_1912_chapel_court_police_records",
                        "acceptable_resolutions": ["case status", "refusal reason"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "GREEN")
        self.assertEqual(p1["metrics"]["hidden_unseen_term_decisions"], 0)

    def test_p1_flags_historical_kitchen_knife_as_current_blade_threat(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            previous = (
                "[roll]Persuade Arty Wilmot to grant access to the Globe clippings morgue: "
                "1d100 = 32 vs 60. Success.[/roll] "
                "The Corbitt house has been in print before. The file paints a grim pattern: "
                "deaths, crippling accidents, illness in the house, and a suicide by kitchen knife. "
                "The Macario family also appears in connection with the place."
            )
            write_jsonl(run_dir / "turn_003.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_003.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Evelyn treats the visible knife or dagger as the immediate threat and dodges behind cover."
            )
            decision.update(
                {
                    "turn": 4,
                    "gm_visible_reply": previous,
                    "perceived_facts": [
                        "A knife or dagger is now player-visible and potentially dangerous."
                    ],
                    "active_goal": "Resolve the visible knife/dagger threat without handwaving danger.",
                    "candidate_actions": [
                        "Dodge behind cover and knock the weapon away",
                        "Retreat up the stairs",
                    ],
                    "response_contract": {
                        "intent": "defend_against_visible_blade_threat",
                        "acceptable_resolutions": ["dodge or combat check"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("treats historical knife reference as current blade threat" in item for item in p1["evidence"])
        )

    def test_p1_flags_earlier_historical_knife_used_after_later_nonblade_turn(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            newspaper = (
                "At the Globe, the file paints a grim public-history pattern: deaths, "
                "crippling accidents, illness in the house, and a suicide by kitchen knife."
            )
            ground_floor = (
                "[roll]Search the Corbitt House ground floor: 1d100 = 79 vs 60. Failure.[/roll] "
                "The ground floor remains dark, stale, and silent. No route, object, movement, "
                "or immediate threat separates itself from ordinary grime."
            )
            write_jsonl(run_dir / "turn_003.jsonl", [{"event": "delta", "data": newspaper}])
            write_jsonl(run_dir / "turn_006.jsonl", [{"event": "delta", "data": ground_floor}])
            (run_dir / "explain_003.txt").write_text("domain_events:\n")
            (run_dir / "explain_006.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Evelyn treats the visible knife or dagger as the immediate threat and dodges behind cover."
            )
            decision.update(
                {
                    "turn": 7,
                    "gm_visible_reply": ground_floor,
                    "perceived_facts": [
                        "A knife or dagger is now player-visible and potentially dangerous."
                    ],
                    "active_goal": "Resolve the visible knife/dagger threat without handwaving danger.",
                    "candidate_actions": [
                        "Dodge behind cover and knock the weapon away",
                        "Retreat toward the marked exit",
                    ],
                    "response_contract": {
                        "intent": "defend_against_visible_blade_threat",
                        "acceptable_resolutions": ["dodge or combat check"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertTrue(
            any("treats historical knife reference as current blade threat" in item for item in p1["evidence"])
        )

    def test_p1_allows_english_chapel_decision_after_chinese_visible_chapel_fact(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            previous = (
                "你顺着房产地址与科比特这个姓氏往下查，科比特死后，"
                "遗产执行人是“沉思小礼拜堂”的迈克尔·托马斯牧师。"
                "再核对下去，你查到那间礼拜堂本身后来已经关闭，时间是1912年。"
            )
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": previous}])
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Evelyn asks the courts and police for 1912 Chapel records."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": previous,
                    "perceived_facts": [
                        "Corbitt's executor trail is tied to Reverend Michael Thomas.",
                        "The Chapel of Contemplation is a concrete lead.",
                        "The chapel trail includes a 1912 date.",
                    ],
                    "active_goal": "Test whether the 1912 chapel lead exists in court or police records.",
                    "response_contract": {
                        "intent": "access_1912_chapel_court_police_records",
                        "acceptable_resolutions": ["case status", "refusal reason"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["metrics"]["hidden_unseen_term_decisions"], 0)

    def test_p1_does_not_treat_buried_in_routine_as_visible_burial_clue(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            (run_dir / "opening.txt").write_text(
                "Mr. Knott asks you to investigate the Corbitt House and gives you keys.",
                encoding="utf-8",
            )
            write_jsonl(
                run_dir / "turn_001.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "What you do get, and only because it is already too buried "
                            "in routine to be worth guarding, is a dusty file index."
                        ),
                    }
                ],
            )
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            decision = player_decision_signal("I follow the burial clue to the house.")
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": "The public file detail was buried in routine paperwork.",
                    "perceived_facts": ["A player-visible clue points toward a burial question."],
                    "active_goal": "Follow the burial clue.",
                    "candidate_actions": [
                        "Go to the house because of the burial clue",
                        "Search for the buried body",
                    ],
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "RED")
        self.assertIn("uses hidden/unseen term", p1["evidence"][0])

    def test_complete_player_decision_protocol_can_score_100(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(run_dir / "signals.jsonl", [player_decision_signal()])
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "[roll]观察线缆 1d10=8 目标:14，结果:成功[/roll]"
                            "你看清线缆贴着门内左侧地面延伸，尽头接进一台仍亮着红灯的服务器柜。"
                        ),
                    },
                    {"event": "scene_transition", "from": "street", "to": "warehouse_threshold"},
                ],
            )
            (run_dir / "t01.explain").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
                '  WorldFactChanged {"fact":"cable_route_seen"}\n'
                '  SceneTransitioned {"from":"street","to":"warehouse_threshold"}\n'
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")
            md = mod.render_markdown(report)

        self.assertEqual(report["verdict"], "PASS")
        self.assertEqual(report["rubric"]["total"], 100)
        self.assertEqual([row["weight"] for row in report["rubric"]["dimensions"]], [25, 20, 20, 15, 15, 5])
        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["status"], "GREEN")
        self.assertIn("rubric total", md)

    def test_j4_flags_check_without_player_visible_roll_result(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [{"event": "delta", "data": "你摸到接口，但结果还不清楚。"}],
            )
            (run_dir / "t01.explain").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j4 = next(j for j in report["judges"] if j["judge"] == "J4")
        self.assertEqual(j4["status"], "RED")
        self.assertEqual(j4["metrics"]["check_turns"], 1)
        self.assertEqual(j4["metrics"]["surfaced_turns"], 0)

    def test_j4_accepts_coc_roll_target_outcome_block_format(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "[roll]\n"
                            "Persuade — Press Mr. Knott for practical details\n"
                            "Roll: 1d100 = 11\n"
                            "Target: 50\n"
                            "Outcome: Success (Hard)\n"
                            "[/roll]\n"
                            "Knott gives a concrete address lead and key uncertainty."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j4 = next(j for j in report["judges"] if j["judge"] == "J4")
        self.assertEqual(j4["status"], "GREEN")
        self.assertEqual(j4["metrics"]["check_turns"], 1)
        self.assertEqual(j4["metrics"]["surfaced_turns"], 1)
        self.assertEqual(j4["metrics"]["under_tagged_check_turns"], 0)

    def test_j4_flags_under_tagged_multiple_resolved_checks(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "Listen at the doorway: 1d100 = 98 vs Listen 45 — Fumble.\n"
                            "[roll]Spot Hidden — 1d100: 5 vs target 95 — Extreme success.[/roll]"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"listen"}\n'
                '  CheckResolved {"check_id":"listen"}\n'
                '  DiceRolled {"check_id":"spot"}\n'
                '  CheckResolved {"check_id":"spot"}\n'
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j4 = next(j for j in report["judges"] if j["judge"] == "J4")
        self.assertEqual(j4["status"], "RED")
        self.assertEqual(j4["metrics"]["check_turns"], 1)
        self.assertEqual(j4["metrics"]["surfaced_turns"], 0)
        self.assertEqual(j4["metrics"]["under_tagged_check_turns"], 1)

    def test_j4_flags_unbound_resolved_checks(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [{"event": "delta", "data": "[roll]TECH 1d10=3；结果未明[/roll]"}],
            )
            (run_dir / "t01.explain").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1","outcome":{"awaiting_binding":"no source-backed target"}}\n'
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j4 = next(j for j in report["judges"] if j["judge"] == "J4")
        self.assertEqual(j4["status"], "RED")
        self.assertEqual(j4["metrics"]["unbound_check_turns"], 1)

    def test_j4_does_not_require_roll_for_blocked_missing_source_checks(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [{"event": "delta", "data": "你仍压在掩体后，缺少足够依据判断冲刺风险。"}],
            )
            (run_dir / "t01.explain").write_text(
                "domain_events:\n"
                '  CheckResolved {"check_id":"c1","outcome":{"blocked":true,"success":null,"reason":"missing_source_backed_parameters"}}\n'
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j4 = next(j for j in report["judges"] if j["judge"] == "J4")
        self.assertEqual(j4["status"], "GREEN")
        self.assertEqual(j4["metrics"]["check_turns"], 0)
        self.assertEqual(j4["metrics"]["blocked_missing_source_turns"], 1)

    def test_j2_does_not_count_blocked_or_awaiting_checks_as_consequential(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [{"event": "delta", "data": "你贴近侧门，但这一步还没有落定。"}],
            )
            (run_dir / "t01.explain").write_text(
                "domain_events:\n"
                '  CheckResolved {"check_id":"c1","outcome":{"blocked":true,"success":null,"reason":"missing_source_backed_parameters"}}\n'
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j2 = next(j for j in report["judges"] if j["judge"] == "J2")
        self.assertEqual(j2["status"], "RED")
        self.assertEqual(j2["metrics"]["consequential_turns"], 0)

    def test_j2_flags_success_without_concrete_information(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "[roll]Search the Boston Globe morgue: 1d100=[44] "
                            "目标:≤65，结果:strong_success[/roll] "
                            "日期、住址、纠纷、病症、警方出面、死亡和邻里间的怨气都显出形状。"
                            "你已经从这些公开报道里实实在在抓到了一串能继续往下追的东西。"
                            "调查终于有了清晰、扎手的着力点。"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j2 = next(j for j in report["judges"] if j["judge"] == "J2")
        self.assertEqual(j2["status"], "RED")
        self.assertEqual(j2["metrics"]["success_without_information_turns"], 1)

    def test_j2_flags_chinese_newspaper_success_with_only_generic_next_lead(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "[roll]Boston Globe archive search: 1d100=[56] 目标:≤65，结果:成功[/roll] "
                            "一则不起眼的短讯牵出一个名字，另一篇社会版边栏又把同一个地址重新推到纸面上；"
                            "某个事故或纠纷的记载，不再只是模糊的街区传闻，而是带上了可以继续追下去的公开痕迹。"
                            "报纸里确实留下了能够继续深挖的东西，不是臆测，也不是空白，而是真正能通往下一步的印刷记录。"
                            "此刻摊在你面前的，不再只是“也许有关”，而是一条已经从公共档案里冒头的实线。"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j2 = next(j for j in report["judges"] if j["judge"] == "J2")
        self.assertEqual(j2["status"], "RED")
        self.assertEqual(j2["metrics"]["success_without_information_turns"], 1)

    def test_j2_flags_chinese_hall_records_success_with_only_traceable_documents(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "[roll]Search Hall of Records archives for Corbitt and Macario filings: "
                            "1d100=[13] 目标:≤70，结果:strong_success[/roll] "
                            "以科比特姓氏去追，能找到与那栋房产相关的登记线；"
                            "再往马卡里奥这个名字下翻，并不是一场空，公共档案里同样留有可追下去的指向。"
                            "你不是只摸到一堆模糊的类别标题，而是实实在在地从档案系统里拽出了能继续核对的对应记录："
                            "这地方、这姓氏、以及马卡里奥之间，确实在公开文书里有过落笔。"
                            "你已经抓住了真正能通往事实的那条线，官方档案里留下了可供追索的痕迹。"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j2 = next(j for j in report["judges"] if j["judge"] == "J2")
        self.assertEqual(j2["status"], "RED")
        self.assertEqual(j2["metrics"]["success_without_information_turns"], 1)

    def test_j2_allows_chinese_hall_success_with_walter_body_facts(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "[roll]图书馆使用 1d100=[33] ≤ 60，结果：强成功[/roll] "
                            "你在科比特名下的房产与遗嘱档里顺利找到了一串能彼此对上的记录。"
                            "卷宗里反复出现同一个名字——沃尔特·科比特。"
                            "争议的核心是这个人死后留下的明确要求：他的遗体不得按常规移出，"
                            "而要留在自家宅邸之内。相关公文显示这件事当年确实闹上过正式程序。"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j2 = next(j for j in report["judges"] if j["judge"] == "J2")
        self.assertEqual(j2["metrics"]["success_without_information_turns"], 0)

    def test_j2_allows_chinese_hall_success_with_executor_chapel_and_year(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "[roll]Library Use — Hall of Records search: 1d100=[56] "
                            "目标:≤70，结果:strong_success[/roll]\n"
                            "你顺着房产地址与科比特这个姓氏往下查，旧有产权转移、"
                            "死亡后的交接痕迹、与遗产处置有关的公开记录，都慢慢拼出轮廓。"
                            "某个真正经手过那栋房子的人从纸面上浮了出来：科比特死后，"
                            "遗产执行人是“沉思小礼拜堂”的迈克尔·托马斯牧师。"
                            "再核对下去，你查到那间礼拜堂本身后来已经关闭，时间是1912年。"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j2 = next(j for j in report["judges"] if j["judge"] == "J2")
        self.assertEqual(j2["metrics"]["success_without_information_turns"], 0)

    def test_p1_does_not_flag_negated_hidden_terms_as_leaks(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            write_character_setup(run_dir)
            write_jsonl(run_dir / "turn_001.jsonl", [{"event": "delta", "data": "The Hall search failed and a clerk points toward court or police indexes."}])
            (run_dir / "explain_001.txt").write_text("domain_events:\n")
            decision = player_decision_signal(
                "Evelyn searches only public court and police indexes by address, Corbitt, and Macario."
            )
            decision.update(
                {
                    "turn": 2,
                    "gm_visible_reply": "The Hall search failed and a clerk points toward court or police indexes.",
                    "perceived_facts": ["Visible records point toward a court or police-record trail."],
                    "hypotheses": [
                        "Without a named Thomas or 1912 lead, the search terms must stay visible.",
                        "Evelyn should not treat a metaphorical records maze as a burial clue.",
                    ],
                    "response_contract": {
                        "intent": "access_general_corbitt_court_police_records",
                        "acceptable_resolutions": ["public result", "refusal"],
                        "unacceptable": ["Assume Michael Thomas, Chapel, 1912, or burial if not visible"],
                    },
                }
            )
            write_jsonl(run_dir / "player_decisions.jsonl", [decision])

            report = mod.evaluate_run_dir(run_dir, label="unit")

        p1 = next(j for j in report["judges"] if j["judge"] == "P1")
        self.assertEqual(p1["metrics"]["hidden_unseen_term_decisions"], 0)

    def test_j2_flags_narrated_position_change_without_committed_state(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "等你停住时，身位已经从先前暴露的位置挪开，"
                            "整个人紧贴着仓库门框旁的盲区藏住。"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text(
                "domain_events:\n"
                '  TurnStarted {"module_id":"cyberpunk_red.homecoming"}\n'
                '  TurnFinalized {"signal":"Narration"}\n'
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j2 = next(j for j in report["judges"] if j["judge"] == "J2")
        self.assertEqual(j2["status"], "RED")
        self.assertEqual(j2["metrics"]["narrated_state_without_commit_turns"], 1)
        self.assertEqual(report["verdict"], "FAIL")

    def test_constitution_gate_blocks_explicit_option_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "[roll]TECH 1d10=8 vs DV12 success[/roll]\n"
                            "你现在可以立刻选择其一：\n"
                            "1. **冲进仓库**\n"
                            "2. **继续破解**\n"
                            "3. **呼叫警员**\n"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
                '  SceneTransitioned {"from":"a","to":"b"}\n'
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)
        self.assertEqual(report["verdict"], "FAIL")

    def test_constitution_gate_blocks_english_what_now_inline_action_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "The doorway is open a handspan. Stale air leaks out, "
                            "and the first boards inside look dusty but still. "
                            "What does Evelyn do now? Step inside, widen the opening a little, "
                            "inspect the entry from where she is, or hold position longer?"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)
        self.assertTrue(
            any(
                "english_what_now_inline_action_menu" in item or "prose_action_menu" in item
                for item in q4["evidence"]
            )
        )

    def test_constitution_gate_blocks_prose_rewritten_action_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "[roll]Basic Tech 1d10=7 vs DV14 success[/roll]\n"
                            "你现在有了一个短窗口。你把这些观察串在一起："
                            "趁它还没完全停摆，冲出去控制、拖拽或缴械；"
                            "转向仓库内侧，追服务器和那根线的源头；"
                            "对警员发号施令，争取把他们从射界里撤出来；"
                            "先观察它接下来几秒还保不保持火控与瞄准能力。"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text(
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_fact_summary_after_observation_frame_is_not_action_menu(self):
        mod = load_module()
        text = (
            "你把这些观察串在一起："
            "那台人形无人机和仓库之间，确实连着一根明显的线缆；"
            "它的火力扇区主要咬着正前方开阔地；"
            "两名警员都还活着，但伤得不轻；"
            "它和仓库里的东西之间，很可能不只是拴着，更像是被供电、控制，或者两者都有。"
        )

        self.assertFalse(mod.inline_action_menu(text))

    def test_constitution_gate_blocks_english_observation_action_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "From here, the live paths remain.\n"
                            "You connect these observations: follow the Chapel of Contemplation directly; "
                            "change approach and try to reopen the court/police angle some other way; "
                            "or turn to the Corbitt house itself."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)
        self.assertEqual(report["verdict"], "FAIL")

    def test_constitution_gate_blocks_english_observation_field_manifest_dump(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "From where Evelyn now stands, the concrete affordance is the landing.\n"
                            "You connect these observations: Location: at the top of the stairs; "
                            "Looks like: a small upper landing with bedroom openings; "
                            "Sounds like: almost nothing beyond her own movements; "
                            "Smells like: shut-in old wood and dust; "
                            "Reachability: safely reachable from her current position."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["dump_hits"], 1)

    def test_constitution_gate_blocks_internal_adjudication_guidance(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "你把行动停在地下室入口和第一段楼梯这条已经确认的接点上："
                            "地下室门口、楼梯顶端、手帕标记、楔开的门和身后的退路是当前可公开承认的范围。"
                            "更深处的楼梯是否稳固还没有被确认，不能当成已发现事实。"
                            "这次结算应锚定刚才的下行试探。"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text(
                "domain_events:\n"
                '  TurnStarted {"module_id":"call_of_cthulhu_7e.the_haunting"}\n'
                '  TurnFinalized {"signal":"Narration"}\n'
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["dump_hits"], 1)

    def test_constitution_gate_blocks_next_meaningful_move_is_yours_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "The bed and loose papers are visible from the threshold. "
                            "If she wants to keep pressing this room, the next meaningful move is yours: "
                            "remain at the threshold and focus on one visible feature, or commit farther in."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_choose_where_character_goes_first_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "From here, the obvious lines of inquiry are the public record offices "
                            "and the newspaper files. If you want, you can choose where Evelyn goes first."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_obvious_next_avenues_and_list_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "The obvious next avenues are the city records offices "
                            "and the newspaper files."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_paper_trail_three_promising_directions_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "From here, the paper trail now seems to point in three promising "
                            "directions: the closed chapel, the courts/police records, or the house itself."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_three_immediate_lines_of_pursuit_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "The municipal search gives a concrete executor and institution. "
                            "So, from this office, you now have three immediate lines of pursuit. "
                            "the Chapel of Contemplation, the higher courts / police records, "
                            "or the house itself."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_two_obvious_directions_and_list_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "The two obvious directions are now the Chapel of Contemplation "
                            "and the Corbitt house itself."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_next_pressure_points_follow_first_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "The next pressure points are clear enough in the record. "
                            "the Chapel of Contemplation. higher court files. "
                            "Central Police Station records. What does she follow first?"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_obvious_next_lines_of_inquiry_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "The obvious next lines of inquiry from here would be city records, "
                            "deed or court records, and old newspaper files."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_if_want_press_or_paper_trail_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "He seems ready to answer a few more questions if you want to press "
                            "him further, or you can set off on the paper trail first."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_if_choose_you_can_or_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "So the concrete affordance is not go deeper in the abstract. "
                            "It is this: if you choose, you can approach that boarded section "
                            "and examine it directly as the next meaningful point of contact. "
                            "Or you can hold where you are with the stairs at your back."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)
        self.assertTrue(
            any("english_if_you_choose_you_can_or_menu" in item for item in q4["evidence"])
        )

    def test_constitution_gate_blocks_next_solid_leads_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "The next solid leads from here would be the records offices "
                            "or newspaper files."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_obvious_lines_pursue_first_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "From here, the obvious lines of inquiry are now established in your notes: "
                            "the Globe's files, the central library, the hall of records, and possibly "
                            "court or police records if the paper trail points that way. "
                            "What does Evelyn pursue first?"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_english_will_you_start_or_more_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "He looks to you expectantly, anxious but practical. "
                            "So--will you start with the records, or is there something more "
                            "you want from me here before you go?"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_english_will_you_begin_or_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "So--will you begin with the records, the newspapers, "
                            "or by speaking to people in the neighborhood?"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_offers_more_avenues_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "The record room offers more avenues now if you want them: "
                            "the chapel itself, higher court records and police files, "
                            "or the old house once you think you have enough in hand."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_j2_flags_source_limited_address_contradiction(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "He writes down the full address of the old Corbitt place. "
                            "In Evelyn's notes, this remains a usable address lead for navigation "
                            "and records work, not a literal street-number line she can quote."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text(
                "domain_events:\n"
                '  WorldFactChanged {"fact":"briefing_address_lead"}\n'
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j2 = next(j for j in report["judges"] if j["judge"] == "J2")
        self.assertEqual(j2["status"], "RED")
        self.assertEqual(j2["metrics"]["source_address_contradiction_turns"], 1)

    def test_j2_flags_address_is_here_writing_down_contradiction(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "\"The address is here,\" he says, writing it down for you. "
                            "In Evelyn's notes, this remains a usable address lead for navigation "
                            "and records work, not a literal street-number line she can quote."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text(
                "domain_events:\n"
                '  WorldFactChanged {"fact":"briefing_address_lead"}\n'
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j2 = next(j for j in report["judges"] if j["judge"] == "J2")
        self.assertEqual(j2["status"], "RED")
        self.assertEqual(j2["metrics"]["source_address_contradiction_turns"], 1)

    def test_j2_flags_generic_written_address_contradiction(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "He writes down the address of the old Corbitt House for you. "
                            "In Evelyn's notes, this remains a usable address lead for navigation "
                            "and records work, not a literal street-number line she can quote."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text(
                "domain_events:\n"
                '  WorldFactChanged {"fact":"briefing_address_lead"}\n'
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j2 = next(j for j in report["judges"] if j["judge"] == "J2")
        self.assertEqual(j2["status"], "RED")
        self.assertEqual(j2["metrics"]["source_address_contradiction_turns"], 1)

    def test_j2_flags_house_address_certainty_contradiction(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "He writes down the house address. The address is certain. "
                            "The keys, the written address, and Knott's approval are in front of you. "
                            "In Evelyn's notes, this remains a usable address lead for navigation "
                            "and records work, not a literal street-number line she can quote."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text(
                "domain_events:\n"
                '  WorldFactChanged {"fact":"briefing_address_lead"}\n'
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j2 = next(j for j in report["judges"] if j["judge"] == "J2")
        self.assertEqual(j2["status"], "RED")
        self.assertEqual(j2["metrics"]["source_address_contradiction_turns"], 1)

    def test_constitution_gate_blocks_english_clear_choice_target_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "The basement search exposes disturbed earth and scrape marks.\n"
                            "From where you stand now, you have a clear choice of where to bring "
                            "the light next: the disturbed earth, the scraped area, or the "
                            "suspicious section that may conceal a panel."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)
        self.assertEqual(report["verdict"], "FAIL")

    def test_constitution_gate_blocks_english_where_mean_begin_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "He looks from the notebook back to you. "
                            "So—where do you mean to begin: city records, "
                            "the library, or the newspaper files?"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_english_live_directions_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "Your notes now point in three live directions, all uglier than a simple bad tenancy: "
                            "the courts, the police records, or the chapel. What do you pursue next?"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_english_points_in_three_directions_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "The records reveal Reverend Michael Thomas and the chapel closure.\n"
                            "From here, the investigation naturally points in three directions.\n"
                            "You connect these observations: the higher courts / police records; "
                            "the former Chapel of Contemplation; or the old Corbitt place itself. "
                            "What does Evelyn do next?"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_english_obvious_next_avenues_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "Knott leaves the keys with you and waits. "
                            "The obvious next avenues are the city records offices or "
                            "the newspaper files, though you could press him further."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_english_branches_outward_target_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "The executor record ties Corbitt to Michael Thomas. "
                            "From here, the line of inquiry clearly branches outward: "
                            "the Chapel of Contemplation, the court/police record trail, "
                            "or the house itself."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_english_pressure_points_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "Evelyn ends this pass with better orientation and a clearer choice "
                            "of pressure points in the house: continue upstairs, examine the way "
                            "down more closely, or slow even further over specific papers."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_english_paper_trail_continue_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "A clerk can tell you where the paper trail might continue "
                            "if you want to press it further: higher courts, police records, "
                            "or the chapel itself. What does Evelyn do next?"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_field_manifest_without_taken_together_header(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "What it gives her, concretely, is this. "
                            "It is at immediately inside the door she has opened, at the threshold line. "
                            "Look: stale entry hall, dim floorboards, shadowed interior. "
                            "It sounds like no clear movement. "
                            "It smells like old plaster, dust, trapped damp, and shut-in stale air."
                        ),
                    }
                ],
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_obvious_directions_or_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "From what you now have in hand, two obvious directions suggest themselves: "
                            "the old police file / official case trail, or the Chapel of Contemplation, "
                            "if you want to pursue the cult connection behind the raid."
                        ),
                    }
                ],
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_for_example_focusing_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "If you want to press further from here, the next useful angle is less "
                            "broad sweep and more specific line of attack — for example, focusing on "
                            "hidden compartments, denominational records, signs of ritual use, "
                            "or a single part of the building that seems wrong."
                        ),
                    }
                ],
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_follow_this_outward_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "You can follow this outward from here—toward the chapel, "
                            "toward newspaper files about the 1912 raid, or toward the "
                            "Corbitt House itself."
                        ),
                    }
                ],
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_where_does_character_go_next_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "The file gives you the darker line of inquiry.\n"
                            "Where does Evelyn go next? The chapel itself, "
                            "a newspaper archive to deepen the 1912 story, "
                            "or the Corbitt house with this new context in hand?"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_where_does_character_go_with_that_next_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "Where does Evelyn go with that next: the Chapel of Contemplation, "
                            "back toward the Corbitt House, or somewhere else first?"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_what_does_character_examine_next_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "You have the ground floor mapped.\n"
                            "What does Evelyn examine next—upstairs, "
                            "a specific room or door on this floor, "
                            "or the cellar access from above without descending?"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_what_does_character_do_first_dash_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "You are back at the crawl-space route with light, line, and tools.\n"
                            "What does Evelyn do first—check the crawl-space mouth closely before entering, "
                            "rig the line and light, start prying at the broken boards, or go straight in?"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)
        self.assertTrue(
            any("english_what_do_first_action_menu" in item for item in q4["evidence"])
        )

    def test_constitution_gate_allows_trailing_chinese_what_do_you_do_prompt(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": "你看清了仓库门前的开阔带。接下来你要怎么做？",
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "GREEN")
        self.assertEqual(q4["metrics"]["menu_hits"], 0)

    def test_constitution_gate_allows_trailing_chinese_do_what_prompt(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": "你把车停在加油站外缘，退路还在。接下来你要做什么？",
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "GREEN")
        self.assertEqual(q4["metrics"]["menu_hits"], 0)

    def test_constitution_gate_allows_from_your_angle_as_observation_prose(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "From your angle and patience, you get a better sense of the "
                            "neighbors' sight lines: where you are exposed, where you are not, "
                            "and which approach leaves the least chance of immediate notice."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "GREEN")
        self.assertEqual(q4["metrics"]["menu_hits"], 0)

    def test_constitution_gate_blocks_raw_module_clue_id_dump(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "根据本回合已确认的结果： · [roll]Library Use 1d100=[67] "
                            "目标:≤80，结果:strong_success[/roll] · 已揭示的模组线索 handout_7: "
                            "Corbitt’s executor was Reverend Michael Thomas of the Chapel of Contemplation; "
                            "the chapel closed in 1912. · The Library Use check succeeds."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["dump_hits"], 1)

    def test_constitution_gate_blocks_fragmented_paper_trail_direction_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "Legal disputes / public filings tied to the address: no specific filing "
                            "can be confidently extracted on this attempt. Macario index: no useful named "
                            "entry turns up in the first run of the public books you check. the higher "
                            "courts / serious legal records. the Central Police Station. or leave the paper "
                            "trail for the moment and go to the Corbitt House itself. A courteous clerk, "
                            "seeing that you at least know how to ask the right questions, leans in and "
                            "offers practical direction rather than facts. You can follow this by turning "
                            "next toward."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertTrue(
            any("fragmented_paper_trail_direction_menu" in item for item in q4["evidence"])
        )

    def test_constitution_gate_blocks_failed_records_chapel_house_direction_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "The Hall of Records search is inconclusive: no clean deed citation, "
                            "no probate docket, and no named executor entry. The most promising "
                            "next channels appear to be the courts, the police records, or leaving "
                            "paper behind for the moment and going to the Chapel of Contemplation "
                            "or the house itself."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertTrue(
            any("fragmented_paper_trail_direction_menu" in item for item in q4["evidence"])
        )

    def test_constitution_gate_blocks_prose_prepared_action_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "油开始往车里灌。接下来你是准备继续坐在车里指挥他们加油，"
                            "还是有人下车去开油箱、买水，或者再追问晚上别在路上的事？"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_allows_trailing_how_to_take_next_step_prompt(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": "仓库门口仍在火线边缘，线缆钻进里面的设备区。眼下你要怎么接下一步？",
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "GREEN")
        self.assertEqual(q4["metrics"]["menu_hits"], 0)

    def test_constitution_gate_blocks_english_next_move_could_be_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "You make the doorframe's blind side. "
                            "From here, your next move could be to peek deeper into the warehouse, "
                            "go for the server side, try to help the cops, or make a play on the cable."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_english_go_first_or_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [{"event": "delta", "data": "So—do you go first to the records office, or the paper?"}],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_english_what_try_first_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": "What do you want to try first: the city records, or the newspaper archives?",
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_english_which_try_first_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": "Knott sits back. So, which will you try first—the records, or the papers?",
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)
        self.assertTrue(any("english_what_try_first_menu" in item for item in q4["evidence"]))

    def test_constitution_action_menu_fixture_blocks_menus_and_allows_open_prompts(self):
        mod = load_module()
        fixture = load_action_menu_fixture()

        for sample in fixture["action_menus"]:
            with self.subTest(kind="action_menu", sample=sample["id"]):
                text = sample["text"]
                hits = mod.constitution_hits(text)
                self.assertTrue(
                    mod.inline_action_menu(text) or hits,
                    f"fixture action menu was not detected: {text}",
                )

        for sample in fixture["allowed"]:
            with self.subTest(kind="allowed", sample=sample["id"]):
                text = sample["text"]
                hits = mod.constitution_hits(text)
                self.assertFalse(mod.inline_action_menu(text), text)
                self.assertFalse(
                    any("menu" in hit or "cue" in hit for hit in hits),
                    f"allowed fixture was incorrectly detected as a menu: {hits}",
                )

    def test_constitution_gate_blocks_english_whether_you_or_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "Knott falls quiet after that, watching to see whether you "
                            "press him further here or head out to start with the records."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_does_not_crash_on_whether_without_tail_or(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "A clerk mentions records or newspapers as public sources. "
                            "Knott asks whether you have enough notes."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["metrics"]["total_hits"], 0)

    def test_constitution_gate_blocks_english_natural_directions_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "From here, Evelyn can naturally press in a few directions: "
                            "continue digging here under a tighter line of inquiry, "
                            "turn to the higher courts / police records, or go directly to another lead."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_english_you_can_action_sequence_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "The search does not yield the precise thread you wanted today. "
                            "You can keep working this office from a different angle, "
                            "shift to another archive, or leave for a more direct lead."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_english_do_you_action_sequence_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "The ground floor has been partially cleared room by room. "
                            "Do you continue the ground-floor sweep in more detail, "
                            "go upstairs, or turn your attention to the way down?"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_english_you_may_action_sequence_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "The threshold remains before you. From here, with your exit route "
                            "still behind you, you may go in, inspect another entrance first, "
                            "or work the neighborhood before crossing the threshold."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_english_next_move_is_either_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "You now have a stronger public-paper trail. From here, "
                            "the most promising next move is either the serious-records route "
                            "or the house itself if you want to test the paper trail."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_either_or_action_branches(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "最危险的是仓库门前那段开阔带。"
                            "要么继续借掩体贴过去，要么想办法先让那台 drone 转火、断电，或者失去行动能力。"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_english_you_can_start_with_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "The keys and address are now in front of you. "
                            "From here, you can start with public records, newspaper archives, "
                            "or press him a little harder while he is still here."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_english_character_can_action_sequence_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "You are on the upper landing with one suspicious bedroom identified. "
                            "From here, Evelyn can keep examining that room from the threshold, "
                            "probe specific furniture or papers with the walking stick, or withdraw "
                            "and change floors while the retreat path remains clean."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_english_choice_of_whether_action_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "From where she stands now, she has the basement access under close "
                            "inspection, the ground-floor route back still open, and the choice of "
                            "whether to open it, pull back, or shift attention elsewhere on this floor."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_connect_observations_semicolon_action_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "If you want, Evelyn can now.\n"
                            "You connect these observations: continue her careful descent "
                            "into the basement; retreat back up to the ground floor; or "
                            "abandon the basement for now and head upstairs after withdrawing safely."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_taken_together_field_manifest_dump(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "The door is safely reachable from her current position. "
                            "Taken together, Location: the main front entrance; Sight: an old front door "
                            "with a matching lock; Sound: no distinct sound from inside; Smell: stale, "
                            "shut-in air; whether anything inside is occupied remains unknown."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["dump_hits"], 1)

    def test_constitution_gate_blocks_if_you_want_character_can_commit_probe_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "If you want, Evelyn can now commit to one specific next probe "
                            "from the threshold—bed, wardrobe, papers, or window—or step in "
                            "farther and accept the extra risk."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_if_you_want_target_first_menu_without_can(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "If you want, keep pressing this same doorway method on one specific "
                            "target first—the bed, the wardrobe, the window, or the papers—or "
                            "withdraw and shift to another upstairs doorway before committing further."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_next_meaningful_move_action_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "From where she stands now, with an exit route still open behind her, "
                            "the next meaningful move is hers: commit to that door, shift to another "
                            "entrance, study a particular window or cellar access more closely, or "
                            "canvass the nearby houses before going in."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_open_enough_now_to_action_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "Nothing at the doorway itself lunges, falls, or gives way. "
                            "The entrance is open enough now to listen longer, widen the gap, "
                            "or make a first careful step inside if you choose."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_if_you_want_character_can_hold_or_edge_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "If you want, Evelyn can hold where she is and examine just the "
                            "doorway and bed-edge more closely, or edge up only as far as the "
                            "threshold."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_if_she_wants_threshold_view_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "If she wants, she can remain exactly there and examine just "
                            "that threshold view more closely, or step back toward the exit."
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_no_colon_prose_branch_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "你现在已经贴上仓库外墙，离那台东西更近。"
                            "下一步，无论你是想继续沿墙摸到门口、扑近它后背那块异常装配区、"
                            "冲进仓库内侧，还是先朝警员喊话配合，时机都比刚才好多了。"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_chinese_ordinal_direction_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": (
                            "Hall of Records 里还有几条自然延伸出的去处：\n"
                            "一是继续顺着 Reverend Michael Thomas 往下查，\n"
                            "二是转去 Higher Courts 或 Central Police Station，\n"
                            "三是带着这条新线索直接去 Corbitt House。"
                        ),
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_chinese_where_start_or_menu(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {
                        "event": "delta",
                        "data": "你想先从档案馆还是报社下手，都行。等你查到些东西，再决定什么时候进去看房子。",
                    }
                ],
            )
            (run_dir / "t01.explain").write_text("domain_events:\n")

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertGreaterEqual(q4["metrics"]["menu_hits"], 1)

    def test_constitution_gate_blocks_current_coc_menu_variants(self):
        mod = load_module()
        samples = [
            (
                "The new lead points in three clear directions without forcing your hand: "
                "the closed chapel, the courts and police records, or the house itself."
            ),
            (
                "You have enough now to pursue either lead in earnest:\n"
                "the site of the old chapel, or the Corbitt House itself."
            ),
            (
                "Your next move can follow the paper trail outward—or take you straight back "
                "to the Corbitt House with a basement now very much in mind."
            ),
            (
                "Do you have Nell pry the covering farther apart, or hold here and examine "
                "the edges and lettering more closely first?"
            ),
        ]
        for text in samples:
            with self.subTest(text=text):
                self.assertTrue(mod.inline_action_menu(text), text)

    def test_constitution_gate_blocks_english_tone_choice_menu(self):
        mod = load_module()
        text = (
            "At the Globe, the public room is easy enough to reach; the archive itself is not. "
            "Staff can send you toward the right desk, but actual access depends on how Evelyn handles "
            "the gatekeeper. How does she press for access to the files? as a courteous professional "
            "appeal to cooperation. as a confident argument that this is a legitimate public-history "
            "inquiry. as a quick improvised line to get waved through. or as pressure/intimidation. "
            "In plain terms: what tone does Evelyn take with the editor?"
        )
        hits = mod.constitution_hits(text)
        self.assertIn("english_tone_choice_menu", hits)

    def test_constitution_gate_blocks_by_skill_tone_choice_menu(self):
        mod = load_module()
        text = (
            "A staffer can point you toward the clippings room, but access is controlled by "
            "an editor: Arty Wilmot. How does she approach Wilmot—by charm, straight "
            "persuasion, intimidation, or fast talk?"
        )
        hits = mod.constitution_hits(text)
        self.assertIn("english_tone_choice_menu", hits)

    def test_constitution_gate_blocks_fragmented_lean_on_tone_choice_menu(self):
        mod = load_module()
        text = (
            "The morgue files are not open for casual browsing. Access is controlled by an editor, "
            "Arty Wilmot. Evelyn is at the point where she can make her case, but how she does it "
            "matters. Do you have her lean on. her professional manner and reasonableness. personal "
            "charm. pressure or intimidation. or a fast, slippery line to get past the gatekeeper?"
        )
        hits = mod.constitution_hits(text)
        self.assertIn("english_tone_choice_menu", hits)

    def test_constitution_gate_blocks_try_to_get_in_tone_choice_menu(self):
        mod = load_module()
        text = (
            "At the Globe, the obstacle is not the searching itself yet — it is access. "
            "An editor, Arty Wilmot, controls access. How does Evelyn try to get in? "
            "If she leans on respectability, credentials, and a reasonable request, "
            "that suggests Persuade. If she tries charm, pressure, or a quick bluff, "
            "that would point elsewhere. Tell me her approach, and I'll resolve it."
        )
        hits = mod.constitution_hits(text)
        self.assertIn("english_tone_choice_menu", hits)

    def test_constitution_gate_blocks_try_past_editor_tone_choice_menu(self):
        mod = load_module()
        text = (
            "The Globe's front offices are busy, and you are directed to the person who "
            "controls access: Editor Arty Wilmot. If Evelyn wants the clippings morgue "
            "opened, this is a real point of friction. How does she try to get past him? "
            "Does she lean on professional courtesy, polite persuasion, fast-talking "
            "newsroom urgency, or blunt pressure?"
        )
        hits = mod.constitution_hits(text)
        self.assertIn("english_tone_choice_menu", hits)

    def test_constitution_gate_blocks_live_arty_access_tone_choice_menu(self):
        mod = load_module()
        text = (
            "At the Globe, the public side of the place will get Evelyn only so far. "
            "A staffer can tell her where the morgue is kept, but not simply wave her into it: "
            "access is controlled, and the name that comes back is Arty Wilmot, an editor with "
            "custody over the files. So the immediate obstacle is not yet the search itself, "
            "but getting legitimate access to the morgue. How does Evelyn approach that? "
            "If she presses politely, flatters, bluffs urgency, leans on her press credentials, "
            "or tries to bully past the gatekeeper, I'll resolve that accordingly."
        )
        hits = mod.constitution_hits(text)
        self.assertIn("english_tone_choice_menu", hits)

    def test_constitution_gate_flags_unclosed_dialogue_quote(self):
        mod = load_module()
        text = (
            "He leans forward a little.\n\n"
            "“What records exist? Municipal records, certainly. Deeds, transfers, "
            "tax matters, perhaps probate."
        )
        hits = mod.constitution_hits(text)
        self.assertIn("truncated_or_unclosed_dialogue", hits)

    def test_constitution_gate_blocks_runtime_presentation_gate_warnings(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [{"event": "delta", "data": "[roll]TECH 1d10=8 vs DV12 success[/roll]\n你贴墙继续推进。"}],
            )
            (run_dir / "t01.explain").write_text(
                "plugin_contributions:\n"
                "  core.presentation_gate @after_llm_stream -> presentation_gate "
                "(gate=Block kinds=[player_agency_violation])\n"
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "RED")
        self.assertEqual(q4["metrics"]["gate_block_hits"], 1)
        self.assertEqual(report["verdict"], "FAIL")

    def test_constitution_gate_uses_terminal_presentation_gate_state(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [{"event": "delta", "data": "[roll]TECH 1d10=8 vs DV12 success[/roll]\n你贴墙继续推进。"}],
            )
            (run_dir / "t01.explain").write_text(
                "plugin_contributions:\n"
                "  core.presentation_gate @after_llm_stream -> presentation_gate "
                "(gate=Block kinds=[invented_effect])\n"
                "  core.presentation_gate @after_llm_stream -> presentation_gate "
                "(gate=Allow)\n"
                "domain_events:\n"
                '  DiceRolled {"check_id":"c1"}\n'
                '  CheckResolved {"check_id":"c1"}\n'
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        q4 = next(j for j in report["judges"] if j["judge"] == "Q4")
        self.assertEqual(q4["status"], "GREEN")
        self.assertEqual(q4["metrics"]["gate_block_hits"], 0)

    def test_j3_deduplicates_transition_artifacts_per_turn(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "signals.jsonl").write_text("")
            write_jsonl(
                run_dir / "t01.jsonl",
                [
                    {"event": "delta", "data": "门后的走廊终于露出来。"},
                    {"event": "scene_transition", "from": "a", "to": "b"},
                ],
            )
            (run_dir / "t01.explain").write_text(
                "domain_events:\n"
                '  SceneTransitioned {"from":"a","to":"b"}\n'
            )

            report = mod.evaluate_run_dir(run_dir, label="unit")

        j3 = next(j for j in report["judges"] if j["judge"] == "J3")
        self.assertEqual(j3["metrics"]["scene_transitions"], 1)


if __name__ == "__main__":
    unittest.main()

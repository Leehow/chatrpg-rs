import importlib.util
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]
MODULE_PATH = ROOT / "scripts" / "coc_the_haunting_autoplay.py"


def load_module():
    spec = importlib.util.spec_from_file_location("coc_the_haunting_autoplay", MODULE_PATH)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class CocTheHauntingAutoplayTests(unittest.TestCase):
    def test_trpg_env_enables_buffered_presentation_gate(self):
        mod = load_module()

        env = mod.trpg_env()

        self.assertEqual(env["DATABASE_URL"], mod.DB_URL)
        self.assertEqual(env["TRPG_NARRATOR_SPLIT"], "true")
        self.assertEqual(env["TRPG_PRESENTATION_GATE"], "true")
        self.assertEqual(env["TRPG_REVEAL_GATING"], "true")
        self.assertEqual(env["TRPG_CLUE_PROJECTION"], "true")

    def test_trpg_env_sets_output_language_when_requested(self):
        mod = load_module()

        env = mod.trpg_env("zh-Hans")

        self.assertEqual(env["TRPG_OUTPUT_LANGUAGE"], "zh-Hans")

    def test_chinese_character_preferences_do_not_own_naming_policy(self):
        mod = load_module()

        preferences = mod.character_preferences("zh-Hans")

        self.assertIn("简体中文", preferences)
        self.assertIn("调查记者", preferences)
        self.assertIn("《鬼屋》", preferences)
        self.assertNotIn("canonical name", preferences)
        self.assertNotIn("1920s Boston", preferences)
        self.assertNotIn("output language", preferences)
        self.assertNotIn("does not decide", preferences)
        self.assertNotIn("角色名、背景、装备和技能说明都尽量使用简体中文", preferences)
        self.assertNotIn("中文姓名", preferences)

    def test_opening_text_is_localized_for_chinese_output_language(self):
        mod = load_module()

        localized = mod.localize_opening_for_output_language(
            "Mr. Knott asks whether you will inspect the Corbitt House.",
            "zh-Hans",
        )

        self.assertIn("诺特", localized)
        self.assertIn("科比特宅", localized)
        self.assertNotIn("Mr. Knott", localized)

    def test_chinese_output_language_sends_player_visible_action_in_chinese(self):
        mod = load_module()
        state = mod.VisibleState(
            transcript="诺特先生给了你钥匙，并提到科比特宅和马卡里奥一家。",
            last_reply="诺特先生给了你钥匙，并提到科比特宅和马卡里奥一家。",
            turn=0,
            pc_name="Evelyn Ward",
            output_language="zh-Hans",
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertIn("我不会立刻离开", decision["sent_to_gm"])
        self.assertIn("诺特", decision["sent_to_gm"])
        self.assertNotIn("I do not leave immediately", decision["sent_to_gm"])
        self.assertNotIn("Evelyn Ward", decision["sent_to_gm"])

    def test_create_character_timeout_allows_slow_live_generation(self):
        mod = load_module()

        self.assertGreaterEqual(mod.CREATE_CHARACTER_TIMEOUT_SECONDS, 360)

    def test_global_prompt_contract_forbids_inline_action_menus(self):
        dice_contract = (ROOT / "gm_skill_src/global/10_dice_discretion.md").read_text(
            encoding="utf-8"
        )
        output_contract = (ROOT / "gm_skill_src/global/30_output_contract.md").read_text(
            encoding="utf-8"
        )

        self.assertIn("inline options", dice_contract)
        self.assertIn("The player chooses their own action", dice_contract)
        self.assertNotIn("and a goal question", dice_contract)
        self.assertIn("GM-authored action options", output_contract)
        self.assertIn("which lead first", output_contract)

    def test_binary_needs_rebuild_when_rust_source_is_newer(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "crates/trpg-gm/src/lib.rs"
            binary = root / "target-f1/debug/trpg"
            source.parent.mkdir(parents=True)
            binary.parent.mkdir(parents=True)
            source.write_text("pub fn marker() {}\n", encoding="utf-8")
            binary.write_text("binary\n", encoding="utf-8")
            os.utime(binary, (1000, 1000))
            os.utime(source, (2000, 2000))

            self.assertTrue(mod.binary_needs_rebuild(root, binary))

            os.utime(binary, (3000, 3000))
            self.assertFalse(mod.binary_needs_rebuild(root, binary))

    def test_run_cmd_returns_timeout_result_instead_of_raising(self):
        mod = load_module()

        result = mod.run_cmd(
            [
                sys.executable,
                "-c",
                "import time; print('started', flush=True); time.sleep(2)",
            ],
            timeout=1,
        )

        self.assertEqual(result.returncode, -124)
        self.assertIn("started", result.stdout)
        self.assertIn("timed out", result.stderr)

    def test_semantic_critic_cmd_prefers_explicit_env(self):
        mod = load_module()
        with patch.dict(
            os.environ,
            {
                "TRPG_EVAL_SEMANTIC_CRITIC_CMD": "python fake_semantic.py",
                "OPENAI_API_KEY": "test-key",
                "TRPG_EVAL_SEMANTIC_MODEL": "gpt-test",
            },
        ):
            self.assertEqual(
                mod.semantic_critic_cmd(ROOT),
                "python fake_semantic.py",
            )

    def test_semantic_critic_cmd_auto_wires_openai_when_configured(self):
        mod = load_module()
        with patch.dict(
            os.environ,
            {
                "TRPG_EVAL_SEMANTIC_CRITIC_CMD": "",
                "OPENAI_API_KEY": "test-key",
                "TRPG_EVAL_SEMANTIC_MODEL": "gpt-test",
            },
        ):
            cmd = mod.semantic_critic_cmd(ROOT)

        self.assertIn(sys.executable, cmd)
        self.assertIn("scripts/semantic_critic_openai.py", cmd)

    def test_run_redboard_passes_semantic_critic_cmd(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            run_dir = root / "run"
            run_dir.mkdir(parents=True)
            captured = {}
            original_run_cmd = mod.run_cmd

            def fake_run_cmd(args, *, env=None, timeout=240):
                captured["args"] = args
                captured["timeout"] = timeout
                (run_dir / "verdict.json").write_text(
                    json.dumps({"verdict": "PASS", "red_count": 0}) + "\n",
                    encoding="utf-8",
                )
                return mod.CommandResult(args=args, returncode=0, stdout="", stderr="")

            try:
                mod.run_cmd = fake_run_cmd
                with patch.dict(
                    os.environ,
                    {"TRPG_EVAL_SEMANTIC_CRITIC_CMD": "python fake_semantic.py"},
                ):
                    mod.run_redboard(root, run_dir)
            finally:
                mod.run_cmd = original_run_cmd

        self.assertIn("--semantic-critic-cmd", captured["args"])
        self.assertIn("python fake_semantic.py", captured["args"])
        self.assertGreaterEqual(captured["timeout"], 240)

    def test_battle_report_embeds_redboard_meta_hash(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            run_dir = Path(tmp)
            (run_dir / "character_sheet.md").write_text("# Character\n", encoding="utf-8")
            (run_dir / "player_decisions.jsonl").write_text("", encoding="utf-8")
            (run_dir / "transcript_player_visible.txt").write_text("## Transcript\n", encoding="utf-8")
            (run_dir / "redboard.md").write_text("# Redboard\n", encoding="utf-8")
            (run_dir / "redboard.meta.json").write_text(
                json.dumps(
                    {
                        "schema": "eval_redboard_meta_v1",
                        "verdict": "FAIL",
                        "red_count": 2,
                        "redboard_sha256": "abc123",
                        "verdict_sha256": "def456",
                    }
                )
                + "\n",
                encoding="utf-8",
            )

            mod.write_battle_report(run_dir)

            report = (run_dir / "battle_report_with_character.md").read_text(encoding="utf-8")

        self.assertIn("redboard_sha256: `abc123`", report)
        self.assertIn("verdict_sha256: `def456`", report)
        self.assertIn("verdict: `FAIL`", report)

    def test_run_turn_accepts_timeout_and_writes_stall_artifacts(self):
        mod = load_module()
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            run_dir = root / "run"
            run_dir.mkdir()
            (run_dir / "transcript_player_visible.txt").write_text("## Opening\n", encoding="utf-8")
            calls = {}
            original_run_cmd = mod.run_cmd

            def fake_run_cmd(args, *, env=None, timeout=240):
                calls["timeout"] = timeout
                return mod.CommandResult(args=args, returncode=-124, stdout="", stderr="command timed out")

            try:
                mod.run_cmd = fake_run_cmd
                with self.assertRaisesRegex(RuntimeError, "timed out"):
                    mod.run_turn(
                        root,
                        run_dir,
                        "session",
                        {"turn": 3, "sent_to_gm": "I search the chapel pulpit."},
                        timeout=7,
                    )
            finally:
                mod.run_cmd = original_run_cmd

            self.assertEqual(calls["timeout"], 7)
            self.assertTrue((run_dir / "turn_003.jsonl").exists())
            self.assertIn("timed out", (run_dir / "turn_003.err").read_text(encoding="utf-8"))

    def test_afraid_does_not_create_chapel_raid_fact(self):
        mod = load_module()
        state = mod.VisibleState(
            transcript=(
                "Mr. Knott gives Evie the keys. He is afraid and says he knows "
                "little about the Macario family."
            ),
            last_reply="He is afraid and says he knows little.",
            turn=1,
            pc_name="Evie",
        )

        facts = mod.facts_from_visible(state)

        self.assertFalse(any("chapel" in fact.lower() for fact in facts))
        self.assertFalse(any("1912" in fact for fact in facts))

    def test_hall_records_decision_does_not_use_unseen_walter_name(self):
        mod = load_module()
        state = mod.VisibleState(
            transcript=(
                "## Opening\nGM: Mr. Knott asks Evie to investigate the Corbitt House.\n\n"
                "## Turn 1\nGM: Knott gives a usable address lead and keys, "
                "but does not name any former owner."
            ),
            last_reply="Knott gives a usable address lead and keys.",
            turn=1,
            pc_name="Evie",
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False)

        self.assertEqual(
            decision["response_contract"]["intent"],
            "search_hall_records_for_property_probate",
        )
        self.assertNotIn("Walter", decision_text)
        self.assertNotIn("full street address", decision["sent_to_gm"].lower())
        self.assertIn("usable address lead", decision["sent_to_gm"].lower())
        self.assertIn("specific entries", decision["sent_to_gm"].lower())
        self.assertIn("not just record categories", decision["sent_to_gm"].lower())

    def test_newspaper_after_failed_hall_records_uses_only_visible_terms(self):
        mod = load_module()
        text = (
            "[roll]Library Use at the Hall of Records: 1d100 = 97 vs 65 — failure.[/roll] "
            "The Corbitt name does appear, and Macario cross-references are enough "
            "to keep you from feeling entirely lost, but the trail refuses to come "
            "together cleanly."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=2,
            pc_name="Evelyn Pierce",
            attempted={"hall_records"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "search_newspaper_archive_after_hall_records_failure",
        )
        self.assertNotIn("walter", decision_text)
        self.assertNotIn("burial", decision_text)
        self.assertNotIn("buried", decision_text)

    def test_newspaper_failure_does_not_retry_unvisited_chapel(self):
        mod = load_module()
        text = (
            "[roll]Boston Globe morgue search: 1d100 = 74 vs Library Use 65 — failure.[/roll] "
            "The Corbitt House paper trail is patchier than expected. The stronger next avenues "
            "are police files, higher court records, or a direct inquiry into the chapel connection "
            "hinted at in the municipal search."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=3,
            pc_name="Evelyn Pierce",
            attempted={"hall_records", "newspaper_archive"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertNotEqual(
            decision["response_contract"]["intent"],
            "retry_chapel_with_distinct_visual_method_after_failure",
        )
        self.assertNotIn("the chapel search did not give", decision_text)

    def test_newspaper_success_with_official_next_lead_follows_records_not_house(self):
        mod = load_module()
        text = (
            "[roll]Search Boston Globe morgue for Corbitt House and related public incidents: "
            "1d100 = 6 vs Library Use 70 — Extreme Success[/roll] "
            "The clippings suggest a pattern of disturbance, illness, and neighborhood concern. "
            "More importantly, the newspaper indexing itself points you toward a stronger next "
            "line of inquiry. The kind of incidents being hinted at would likely have generated "
            "records outside the real-estate and civil-office trail—the sort kept in serious "
            "police files or higher court records, not just in deed books."
        )
        state = mod.VisibleState(
            transcript=(
                "[roll]Hall of Records — failure.[/roll] No clean probate trail.\n" + text
            ),
            last_reply=text,
            turn=3,
            pc_name="Eleanor Wren",
            attempted={"hall_records", "newspaper_archive"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "access_general_corbitt_court_police_records",
        )
        self.assertNotIn("inspect_house_exterior_and_safe_entry", decision_text)
        self.assertIn("police", decision_text)
        self.assertIn("higher court", decision_text)

    def test_ground_floor_failure_mentioning_chapel_record_does_not_retry_chapel(self):
        mod = load_module()
        text = (
            "[roll]Cautious room-by-room search of the Corbitt House ground floor: "
            "1d100 = 96 vs target 55. Failure.[/roll] "
            "Door by door, you mark what you have opened. Here and there you do find papers "
            "worth photographing, but nothing on this pass gives you a neat, decisive document "
            "like the chapel record did. A faint current of colder air suggests a way deeper "
            "in the house, but the ground floor refuses to yield cleanly."
        )
        state = mod.VisibleState(
            transcript=(
                "Evelyn is inside the Corbitt House on the ground floor. "
                "The Chapel of Contemplation was visited earlier.\n" + text
            ),
            last_reply=text,
            turn=7,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertNotEqual(
            decision["response_contract"]["intent"],
            "retry_chapel_with_distinct_visual_method_after_failure",
        )
        self.assertNotIn("the chapel search did not give", decision_text)

    def test_ground_floor_anomaly_after_clarification_gets_probe_not_fallback_loop(self):
        mod = load_module()
        text = (
            "From where Evelyn stands now, the clearest actionable thing is this: "
            "There is something in the hall ahead of you that does not belong to "
            "the same layer of neglect as the rest of the house. It is not deep "
            "in the building yet. It is still on your side of the first interior "
            "stretch, somewhere between you and the darker inward part of the "
            "ground floor. Your open way out is behind you through the door, and "
            "nothing blocks that line."
        )
        state = mod.VisibleState(
            transcript=(
                "Evelyn has begun inspecting the house interior from the entry hall. "
                "She already searched the ground floor and then asked for the latest "
                "visible affordance.\n" + text
            ),
            last_reply=text,
            turn=8,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
            },
        )

        self.assertTrue(mod.nearby_ground_floor_anomaly_affordance_visible(text))
        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "inspect_clarified_ground_floor_anomaly_from_retreat",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("walking-stick", decision_text)
        self.assertIn("retreat path", decision_text)

    def test_ground_floor_anomaly_concept_accepts_paraphrased_irregularity(self):
        mod = load_module()
        text = (
            "From where Evelyn is standing, the one thing that stands out is a "
            "visible irregularity in the first-floor hall. Everything else reads "
            "as old dust, but this one thing sits wrong and breaks the room's "
            "general pattern. It is forward of her, between her and the deeper "
            "interior, while the open front door and retreat line remain behind her."
        )

        self.assertTrue(mod.nearby_ground_floor_anomaly_affordance_visible(text))

    def test_chapel_search_success_after_exterior_failure_does_not_retry_chapel(self):
        mod = load_module()
        text = (
            "[roll]Surveying the chapel exterior: 1d100 = 71 vs 60 — Failure.[/roll] "
            "Inside, Evelyn searches surviving chapel records. "
            "[roll]Careful search of the chapel's surviving records: 1d100 = 40 vs 70 — Success.[/roll] "
            "Walter Corbitt was buried in the basement of his own house, in accordance with his wishes."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=4,
            pc_name="Evelyn Price",
            attempted={"hall_records", "official_records", "chapel"},
        )

        self.assertTrue(mod.has_success(text))
        self.assertFalse(mod.has_failure(text))
        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertNotEqual(
            decision["response_contract"]["intent"],
            "retry_chapel_with_distinct_visual_method_after_failure",
        )
        self.assertNotIn("chapel search did not give", decision_text)
        self.assertEqual(
            decision["response_contract"]["intent"],
            "inspect_house_exterior_and_safe_entry",
        )

    def test_chapel_scene_establishment_continues_search_not_house_jump(self):
        mod = load_module()
        text = (
            "Daylight makes the trip less foolish, but not comfortable. The Chapel of "
            "Contemplation stands in a state beyond neglect. Time and weather have taken "
            "bites out of it: cracked masonry, dust in the seams, sections that look as if "
            "they gave way years ago and were never touched again. Yet abandonment is not "
            "the same thing as peace."
        )
        state = mod.VisibleState(
            transcript=(
                "The Hall of Records revealed Reverend Michael Thomas, the Chapel of "
                "Contemplation, and a 1912 closure trail.\n" + text
            ),
            last_reply=text,
            turn=5,
            pc_name="Evelyn Price",
            attempted={"hall_records", "official_records", "chapel"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "continue_chapel_search_after_scene_establishment",
        )
        self.assertNotIn("inspect_house_exterior", decision_text)
        self.assertIn("chapel", decision["sent_to_gm"].lower())

    def test_chapel_success_without_payoff_continues_search_not_house_jump(self):
        mod = load_module()
        text = (
            "Morning, if you choose to wait for it, does the chapel no kindness. "
            "The Chapel of Contemplation stands in a state beyond neglect. "
            "[roll]Circle the ruined chapel for exits, fresh tracks, and signs of recent use: "
            "1d100 = 54 vs 60. Success.[/roll] Your circuit of the exterior pays off."
        )
        state = mod.VisibleState(
            transcript=(
                "The official records exposed Reverend Michael Thomas, the Chapel of "
                "Contemplation, missing children, and a 1912 raid.\n" + text
            ),
            last_reply=text,
            turn=5,
            pc_name="Evelyn Price",
            attempted={"hall_records", "official_records", "chapel"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "continue_chapel_search_after_scene_establishment",
        )
        self.assertNotIn("inspect_house_exterior", decision_text)
        self.assertIn("chapel", decision["sent_to_gm"].lower())

    def test_chapel_symbol_success_counts_as_payoff_not_retry(self):
        mod = load_module()
        text = (
            "[roll]Careful low-angle visual sweep for visible carvings, floor marks, desk scars, "
            "or paper impressions in the ruined chapel: 1d100 = 28 vs target 60 — Hard success.[/roll] "
            "This method pays off. With the flashlight held low, fine ridges wake up along the "
            "grain of old wood near the pulpit; on one surviving piece of record furniture, plus "
            "in nearby marked surfaces, you catch a repeated form that is not random damage. "
            "It is a symbol. Not a full hidden cache, but a distinct, visible mark that your "
            "photographs and quick rubbings can preserve: the same emblem associated with the "
            "Chapel of Contemplation's records. You have something concrete from the chapel after all."
        )
        state = mod.VisibleState(
            transcript=(
                "The official records exposed Reverend Michael Thomas, the Chapel of "
                "Contemplation, missing children, and a 1912 raid.\n" + text
            ),
            last_reply=text,
            turn=6,
            pc_name="Evelyn Price",
            attempted={"hall_records", "official_records", "chapel", "second_chapel_method"},
        )

        self.assertFalse(mod.chapel_success_without_payoff_visible(text))
        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertNotEqual(
            decision["response_contract"]["intent"],
            "continue_chapel_search_after_scene_establishment",
        )
        self.assertNotIn("treats the last reply as arrival", decision_text)
        self.assertNotIn("not as the search result", decision_text)

    def test_clarified_chapel_cabinet_triggers_examination_not_reclarify(self):
        mod = load_module()
        text = (
            "From where Evelyn stands in the ruined chapel, the one concrete thing she can "
            "actually act on is the old cabinet. It is still inside the chapel proper, among "
            "the dust and broken masonry rather than buried under it. The cabinet is reachable, "
            "and it looks like one of the few intact furnishings left in a place that otherwise "
            "reads as ruin. Her retreat path remains the open way back the way she came."
        )
        state = mod.VisibleState(
            transcript=(
                "Evelyn has already searched the chapel and recorded a Chapel of Contemplation mark.\n"
                + text
            ),
            last_reply=text,
            turn=24,
            pc_name="Evelyn Hart",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "chapel_followup",
                "second_chapel_method",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "examine_clarified_chapel_cabinet",
        )
        self.assertIn("cabinet", decision["sent_to_gm"].lower())
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_unearned_chapel_direction_menu_does_not_become_player_lead(self):
        mod = load_module()
        text = (
            "The Hall of Records search is inconclusive, but a clerk says the trail likely "
            "branches toward official intervention: higher courts, Central Police Station, "
            "or the Chapel of Contemplation itself, or the Corbitt House itself. No executor "
            "name, date, raid, closure, or Reverend is visible yet."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=4,
            pc_name="Evelyn Marsh",
            attempted={"hall_records", "newspaper_archive"},
        )

        self.assertFalse(mod.earned_chapel_lead_visible(text))
        self.assertTrue(mod.unearned_chapel_direction_visible(text))
        self.assertFalse(
            any("Chapel of Contemplation is a concrete lead" in fact for fact in mod.facts_from_visible(state))
        )
        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "access_general_corbitt_court_police_records",
        )
        self.assertNotIn("chapel of contemplation", action)
        self.assertNotIn("michael thomas", action)
        self.assertNotIn("1912", action)
        self.assertNotIn("reverend", action)

    def test_newspaper_after_official_refusal_uses_only_visible_corbitt_terms(self):
        mod = load_module()
        text = (
            "The court and police desks refuse full access, but visible records connect "
            "Reverend Michael Thomas, the Chapel of Contemplation, and the 1912 closure "
            "back to the Corbitt House address lead."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=3,
            pc_name="Evelyn Price",
            attempted={"hall_records", "official_records"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "search_newspaper_archive_after_official_refusal",
        )
        self.assertIn("corbitt surname", decision_text)
        self.assertNotIn("walter", decision_text)

    def test_prior_player_access_clause_does_not_fake_official_refusal_after_success(self):
        mod = load_module()
        transcript = (
            "PLAYER: If access is restricted, Evelyn cites the public Hall of Records trail. "
            "GM: [roll]Gain access to court and police records: 1d100 = 2 vs target 60 — Extreme Success.[/roll] "
            "The file trail for Reverend Michael Thomas shows a raid in 1912 and affidavits about disappeared children tied to the Chapel of Contemplation."
        )
        last_reply = (
            "[roll]Gain access to court and police records: 1d100 = 2 vs target 60 — Extreme Success.[/roll] "
            "The file trail for Reverend Michael Thomas shows a raid in 1912 and affidavits about disappeared children tied to the Chapel of Contemplation."
        )
        state = mod.VisibleState(
            transcript=transcript,
            last_reply=last_reply,
            turn=3,
            pc_name="Evelyn Pierce",
            attempted={"hall_records", "official_records"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertNotEqual(
            decision["response_contract"]["intent"],
            "search_newspaper_archive_after_official_refusal",
        )
        self.assertNotIn("does not pretend she obtained", decision_text)

    def test_chinese_roll_strong_success_is_not_failure(self):
        mod = load_module()
        text = "[roll]图书馆使用 1d100=[33] ≤ 60，结果：强成功[/roll]"

        self.assertTrue(mod.has_success(text))
        self.assertFalse(mod.has_failure(text))

    def test_chinese_hall_success_does_not_trigger_failed_hall_newspaper_branch(self):
        mod = load_module()
        text = (
            "[roll]图书馆使用 1d100=[33] ≤ 60，结果：强成功[/roll] "
            "你在科比特名下的房产与遗嘱档里顺利找到了一串能彼此对上的记录。"
            "卷宗里反复出现同一个名字——沃尔特·科比特。"
            "争议的核心是这个人死后留下的明确要求：他的遗体不得按常规移出，"
            "而要留在自家宅邸之内。"
        )
        state = mod.VisibleState(
            transcript="Corbitt House records lead.\n" + text,
            last_reply=text,
            turn=2,
            pc_name="Evelyn Price",
            attempted={"hall_records"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertNotEqual(
            decision["response_contract"]["intent"],
            "search_newspaper_archive_after_hall_records_failure",
        )
        self.assertEqual(
            decision["response_contract"]["intent"],
            "inspect_house_exterior_and_safe_entry",
        )

    def test_official_records_success_without_contents_continues_records_not_chapel(self):
        mod = load_module()
        text = (
            "[roll]调取迈克尔·托马斯／沉思礼拜堂相关法院与警局档案 "
            "1d100=[29] 目标:≤30，结果:成功[/roll] "
            "你没有被拦在外面。无论是在高等法院，还是随后转到中央警察局的记录柜台，"
            "你都顺利拿到了查阅迈克尔·托马斯与沉思礼拜堂相关记录的门路；"
            "与你所问的那几年、那类案由有关的索引和档案，不再只是隔着柜台听人一句无可奉告，"
            "而是真正对你打开了入口。1912年不再只是一个关门的年份；"
            "它现在也确实把法院和警局的记录牵了进来。"
        )
        state = mod.VisibleState(
            transcript=(
                "The Hall of Records exposed Reverend Michael Thomas, the Chapel of Contemplation, and 1912.\n"
                + text
            ),
            last_reply=text,
            turn=3,
            pc_name="Evelyn Hart",
            attempted={"hall_records", "official_records"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "continue_official_records_after_access_without_contents",
        )
        self.assertNotEqual(
            decision["response_contract"]["intent"],
            "investigate_chapel_for_records_symbols_and_corbitt_links",
        )

    def test_official_records_access_with_repeated_query_terms_continues_records_not_chapel(self):
        mod = load_module()
        text = (
            "[roll]调取迈克尔·托马斯牧师与“沉思小教堂”相关法院或警方档案 "
            "1d100=[45] ≤50 成功[/roll] "
            "你把自己的来意说得平稳自然：一九一二年，迈克尔·托马斯，"
            "沉思小教堂，查封、突袭、刑事程序、拘押、死亡，或者失踪。"
            "最后，对方没有把你打发走，那道原本不打算为你开启的口子，还是开了。"
            "你成功拿到了查阅迈克尔·托马斯牧师和“沉思小教堂”相关法院或警方记录的权限。"
            "纸张已经摊在眼前，线索确实伸进了法院和警署留下的记录里。"
        )
        state = mod.VisibleState(
            transcript=(
                "The Hall of Records exposed Reverend Michael Thomas, the Chapel of Contemplation, and 1912.\n"
                + text
            ),
            last_reply=text,
            turn=4,
            pc_name="Evelyn Hart",
            attempted={"hall_records", "official_records"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "continue_official_records_after_access_without_contents",
        )
        self.assertNotEqual(
            decision["response_contract"]["intent"],
            "investigate_chapel_for_records_symbols_and_corbitt_links",
        )

    def test_official_records_channel_map_goes_to_newspaper_not_house(self):
        mod = load_module()
        text = (
            "[roll]Law — navigate higher courts and police public indexes for Corbitt House "
            "records — 1d100: 34 vs 45 — success.[/roll] "
            "They do not simply hand over a neat file tied to the house address alone. "
            "Where records are restricted, the refusal is exact: address-only requests do not "
            "open sealed or privileged material. Your success buys a firmer map of the paper "
            "trail and its legal boundaries. If you cannot get further from public office "
            "counters on address alone, the next legal public source is the newspaper morgue "
            "and archive trail, using the address, Corbitt name, and any date range you can narrow."
        )
        state = mod.VisibleState(
            transcript=(
                "[roll]Hall of Records — failure.[/roll] A clerk points to court and police records.\n"
                + text
            ),
            last_reply=text,
            turn=4,
            pc_name="Evelyn Price",
            attempted={"hall_records", "official_records"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "search_newspaper_archive_after_official_records_boundary",
        )
        self.assertNotIn("inspect_house_exterior_and_safe_entry", decision_text)
        self.assertIn("newspaper", decision_text)

    def test_newspaper_archive_tone_clarification_answers_access_approach_not_chapel(self):
        mod = load_module()
        text = (
            "At the Globe, the public room is easy enough to reach; the archive itself is not. "
            "Ink, paper dust, and hot machinery hang in the building, and before long it becomes "
            "clear that the basement clippings morgue is controlled rather than casually open. "
            "Staff can send you toward the right desk, but actual access depends on how Evelyn "
            "handles the gatekeeper. How does she press for access to the files? as a courteous "
            "professional appeal to cooperation. as a confident argument that this is a legitimate "
            "public-history inquiry. as a quick improvised line to get waved through. or as "
            "pressure/intimidation. In plain terms: what tone does Evelyn take with the editor?"
        )
        state = mod.VisibleState(
            transcript=(
                "Court and police access was refused. Evelyn went to the Globe newspaper archive.\n"
                + text
            ),
            last_reply=text,
            turn=5,
            pc_name="Evelyn Price",
            attempted={"hall_records", "official_records", "newspaper_archive"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "press_newspaper_archive_access_professionally",
        )
        self.assertNotIn("investigate_chapel", decision_text)
        self.assertIn("professional", decision["sent_to_gm"].lower())
        self.assertIn("globe", decision["sent_to_gm"].lower())

    def test_fragmented_newspaper_archive_tone_menu_answers_access_approach_not_house(self):
        mod = load_module()
        text = (
            "The Globe receives Evelyn in a wash of ink, paper dust, and newsroom heat. "
            "The morgue files are not open for casual browsing. Access is controlled by an "
            "editor, Arty Wilmot, with a clerk named Ruth Blake handling the dusty back-room "
            "work if he agrees. Evelyn is at the point where she can make her case, but how "
            "she does it matters. Do you have her lean on. her professional manner and "
            "reasonableness. personal charm. pressure or intimidation. or a fast, slippery "
            "line to get past the gatekeeper?"
        )
        state = mod.VisibleState(
            transcript="Hall of Records failed. Evelyn went to the Globe newspaper archive.\n" + text,
            last_reply=text,
            turn=4,
            pc_name="Evelyn Hart",
            attempted={"hall_records", "newspaper_archive"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "press_newspaper_archive_access_professionally",
        )
        self.assertIn("wilmot", decision_text)
        self.assertNotIn("inspect_house_exterior", decision_text)

    def test_newspaper_archive_try_to_get_in_menu_answers_access_approach_not_house(self):
        mod = load_module()
        text = (
            "At the Globe, the obstacle is not the searching itself yet — it is access. "
            "Ink, paper dust, and the hot thrum of machinery fill the building, and staff "
            "can point Evelyn toward the basement clippings morgue, but the files are not "
            "simply open for a casual walk-in. An editor, Arty Wilmot, controls access. "
            "How does Evelyn try to get in? If she leans on respectability, credentials, "
            "and a reasonable request, that suggests Persuade. If she tries charm, "
            "pressure, or a quick bluff, that would point elsewhere. Tell me her approach, "
            "and I'll resolve it."
        )
        state = mod.VisibleState(
            transcript="Evelyn went to the Boston Globe newspaper archive.\n" + text,
            last_reply=text,
            turn=5,
            pc_name="Evelyn Price",
            attempted={"hall_records", "official_records", "newspaper_archive"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "press_newspaper_archive_access_professionally",
        )
        self.assertIn("wilmot", decision_text)
        self.assertIn("professional", decision["sent_to_gm"].lower())
        self.assertNotIn("inspect_house_exterior", decision_text)

    def test_newspaper_archive_try_past_editor_menu_answers_access_approach_not_house(self):
        mod = load_module()
        text = (
            "The Globe's front offices hit you with ink, paper dust, and the heat of the "
            "presses somewhere deeper in the building. You are directed instead to the "
            "person who controls access: Editor Arty Wilmot. Your press card may get you "
            "in the conversation, but not automatically into the morgue. If Evelyn wants "
            "the clippings morgue opened, this is a real point of friction rather than a "
            "foregone courtesy. How does she try to get past him? Does she lean on "
            "professional courtesy, polite persuasion, fast-talking newsroom urgency, "
            "or blunt pressure?"
        )
        state = mod.VisibleState(
            transcript="Evelyn went to the Boston Globe newspaper archive.\n" + text,
            last_reply=text,
            turn=4,
            pc_name="Evelyn Marsh",
            attempted={"hall_records", "newspaper_archive"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "press_newspaper_archive_access_professionally",
        )
        self.assertIn("wilmot", decision_text)
        self.assertNotIn("inspect_house_exterior", decision_text)

    def test_newspaper_archive_live_arty_access_gate_answers_approach_not_house(self):
        mod = load_module()
        text = (
            "At the Globe, the public side of the place will get Evelyn only so far. "
            "Ink smell, paper dust, and the clatter of machinery give way to a more controlled "
            "back-office resistance when she asks after old clippings. A staffer can tell her "
            "where the morgue is kept, but not simply wave her into it: access is controlled, "
            "and the name that comes back is Arty Wilmot, an editor with custody over the files. "
            "So the immediate obstacle is not yet the search itself, but getting legitimate "
            "access to the morgue. How does Evelyn approach that? If she presses politely, "
            "flatters, bluffs urgency, leans on her press credentials, or tries to bully past "
            "the gatekeeper, I'll resolve that accordingly."
        )
        state = mod.VisibleState(
            transcript="Evelyn went to the Boston Globe newspaper archive.\n" + text,
            last_reply=text,
            turn=4,
            pc_name="Evelyn Hart",
            attempted={"hall_records", "newspaper_archive"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "press_newspaper_archive_access_professionally",
        )
        self.assertIn("wilmot", decision_text)
        self.assertNotIn("inspect_house_exterior", decision_text)

    def test_hall_records_scene_establishment_continues_search_not_house_jump(self):
        mod = load_module()
        text = (
            "The Hall of Records receives you with dust, ink, and that peculiar municipal silence. "
            "Behind the clerks sit bound volumes, civil ledgers, probate files, and cross-index drawers. "
            "You work the address lead, the Corbitt name, Macario, and the property trail together, "
            "moving from title records to older ownership entries, then into probate and executor references."
        )
        state = mod.VisibleState(
            transcript="Knott approved records before house entry.\n" + text,
            last_reply=text,
            turn=2,
            pc_name="Evelyn Price",
            attempted={"hall_records"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "continue_hall_records_search_after_scene_establishment",
        )
        self.assertNotEqual(
            decision["response_contract"]["intent"],
            "inspect_house_exterior_and_safe_entry",
        )

    def test_newspaper_historical_kitchen_knife_does_not_create_current_blade_threat(self):
        mod = load_module()
        text = (
            "[roll]Persuade Arty Wilmot to grant access to the Globe clippings morgue: "
            "1d100 = 32 vs 60. Success.[/roll] "
            "The Corbitt house has been in print before, and not in any harmless real-estate notice. "
            "The file paints a grim pattern: deaths, crippling accidents, illness in the house, "
            "and a suicide by kitchen knife. The Macario family also appears in connection with "
            "the place, leaving after a period of simultaneous illness. You now have a documented "
            "public history of the house as a place repeatedly tied to violence, injury, sickness, "
            "and the Macarios' collapse."
        )
        state = mod.VisibleState(
            transcript="Evelyn searched the Boston Globe newspaper archive.\n" + text,
            last_reply=text,
            turn=3,
            pc_name="Evelyn Price",
            attempted={"hall_records", "newspaper_archive"},
        )

        facts_text = " ".join(mod.facts_from_visible(state)).lower()
        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertNotIn("knife or dagger is now player-visible", facts_text)
        self.assertNotEqual(
            decision["response_contract"]["intent"],
            "defend_against_visible_blade_threat",
        )
        self.assertIn("inspect_house_exterior_and_safe_entry", decision_text)

    def test_globe_success_with_house_history_does_not_trigger_hall_success_repair(self):
        mod = load_module()
        text = (
            "At the Globe, the editor relents enough to have you shown downstairs. "
            "[roll]Gain access to the Boston Globe clippings morgue: 1d100 = 37 vs target 60 — success.[/roll] "
            "A staffer leads you into the basement morgue: drawers, folders, brittle envelopes, "
            "street files, and clipped columns gone yellow at the edges. Once you have the address "
            "in hand, the search stops being speculative and becomes annoyingly concrete. What you "
            "find is not a probate trail, but a house-history pattern: deaths, crippling accidents, "
            "illness in the household, a suicide by kitchen knife, and later the Macario family."
        )
        state = mod.VisibleState(
            transcript="The Hall search failed, so Evelyn went to the Boston Globe.\n" + text,
            last_reply=text,
            turn=3,
            pc_name="Evelyn Hart",
            attempted={"hall_records", "newspaper_archive"},
        )

        self.assertFalse(mod.hall_records_success_without_specific_facts_visible(text))
        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertNotEqual(
            decision["response_contract"]["intent"],
            "continue_hall_records_search_after_success_without_specific_facts",
        )
        self.assertIn("inspect_house_exterior_and_safe_entry", decision_text)

    def test_hall_records_success_without_specific_facts_continues_search_not_house_jump(self):
        mod = load_module()
        text = (
            "[roll]Search Hall of Records for Corbitt property, probate, and civil filings: "
            "1d100=[33] 目标:≤65，结果:strong_success[/roll] "
            "The materials connect cleanly: property ownership, former owners, probate, "
            "estate execution, and public legal records all surface more smoothly than expected. "
            "The scattered keywords finally settle onto one traceable line, and the inquiry "
            "moves from rumor into records that can be checked page by page."
        )
        state = mod.VisibleState(
            transcript="Knott approved records before house entry.\n" + text,
            last_reply=text,
            turn=2,
            pc_name="Evelyn Price",
            attempted={"hall_records"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "continue_hall_records_search_after_success_without_specific_facts",
        )
        self.assertNotEqual(
            decision["response_contract"]["intent"],
            "inspect_house_exterior_and_safe_entry",
        )

    def test_official_records_success_with_restricted_language_is_not_refusal(self):
        mod = load_module()
        text = (
            "[roll]Convince the court or police records clerk to allow restricted index or "
            "redacted-file access on the Michael Thomas / Chapel of Contemplation trail -- "
            "1d100: 33 vs 60 -- Success.[/roll] "
            "You are not handed a free run of the files, but you are allowed to inspect the "
            "relevant index entries and a constrained record trail. You leave those offices "
            "with real notes in hand: Michael Thomas, the Chapel of Contemplation, the 1912 "
            "trail, the raid, the deaths, and Thomas's later imprisonment and escape."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=4,
            pc_name="Evelyn Hart",
            attempted={"hall_records", "official_records"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertTrue(mod.official_record_content_assertion_visible(text.lower()))
        self.assertFalse(mod.official_records_success_without_contents_visible(text))
        self.assertNotIn("does not pretend she obtained", decision_text)
        self.assertNotIn("search_newspaper_archive_after_official_refusal", decision_text)

    def test_generic_raid_reference_does_not_create_chapel_or_1912_fact(self):
        mod = load_module()
        text = (
            "[roll]Search Hall of Records files on Corbitt House: 1d100 = 87 vs 70 — Failure.[/roll] "
            "A clerk remarks that if Evelyn is chasing anything serious—criminal proceedings, "
            "a raid, or deeper court entanglements—she may need the higher courts or Central "
            "Police Station rather than just the city shelves."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=2,
            pc_name="Evelyn Marsh",
            attempted={"hall_records"},
        )

        facts = mod.facts_from_visible(state)
        facts_text = " ".join(facts).lower()

        self.assertNotIn("chapel", facts_text)
        self.assertNotIn("1912", facts_text)
        self.assertIn("police", facts_text)

    def test_negative_executor_phrase_does_not_create_executor_fact(self):
        mod = load_module()
        text = (
            "[roll]Search the Boston Globe archive for clippings on the Corbitt house "
            "and associated names — 1d100: 35 vs target 65 — success.[/roll] "
            "This time, the trail catches. Not on the missing executor. "
            "Among the clippings you find reports pointing back to 1912: missing children, "
            "a police raid, dead police and cultists, and Michael Thomas later imprisoned and escaped."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=4,
            pc_name="Evelyn Hart",
            attempted={"hall_records", "official_records", "newspaper_archive"},
        )

        facts = mod.facts_from_visible(state)
        facts_text = " ".join(facts).lower()

        self.assertIn("serious 1912 trouble", facts_text)
        self.assertNotIn("executor trail", facts_text)

    def test_failed_hall_records_records_maze_does_not_create_burial_fact(self):
        mod = load_module()
        text = (
            "[roll]Library Use — Search Hall of Records indexes for Corbitt and Macario "
            "filings: 1d100 = 79 vs target 55. Failure.[/roll] "
            "The deeper thread remains buried in the records maze. A clerk points out "
            "that serious litigation, criminal proceedings, raids, or official trouble "
            "may require the higher courts, county files, or Central Police Station."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=2,
            pc_name="Evelyn Price",
            attempted={"hall_records"},
        )

        facts = mod.facts_from_visible(state)

        self.assertFalse(any("burial" in fact.lower() for fact in facts))

    def test_failed_hall_records_with_court_police_lead_continues_public_research(self):
        mod = load_module()
        text = (
            "[roll]Library Use — Search Hall of Records indexes for Corbitt and Macario "
            "filings: 1d100 = 79 vs target 55. Failure.[/roll] "
            "The search does not yield the clean confirming thread. The deeper thread "
            "remains buried in the records maze. A clerk points out that serious "
            "litigation, criminal proceedings, raids, or official trouble beyond "
            "routine property paperwork may require the higher courts, county files, "
            "or Central Police Station."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=2,
            pc_name="Evelyn Price",
            attempted={"hall_records"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "access_general_corbitt_court_police_records",
        )
        self.assertNotEqual(
            decision["response_contract"]["intent"],
            "inspect_house_exterior_and_safe_entry",
        )

    def test_decision_protocol_uses_generated_character_name(self):
        mod = load_module()
        state = mod.VisibleState(
            transcript="Mr. Knott asks Clara to investigate the Corbitt House.",
            last_reply="Mr. Knott asks Clara to investigate the Corbitt House.",
            turn=0,
            pc_name="Clara Bennett",
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False)

        self.assertIn("Clara Bennett", decision_text)
        self.assertNotIn("Evelyn", decision_text)

    def test_key_explanation_does_not_mean_house_interior_started_or_blocked(self):
        mod = load_module()
        last_reply = (
            "Knott says the front door key is certain and the cellar key likely, "
            "but he cannot swear to every interior lock."
        )
        state = mod.VisibleState(
            transcript=last_reply,
            last_reply=last_reply,
            turn=1,
            pc_name="Evie",
        )

        facts = mod.facts_from_visible(state)
        summary = mod.result_summary(last_reply)

        self.assertFalse(any("interior" in fact.lower() for fact in facts))
        self.assertNotIn("refused or blocked", summary)

    def test_upstairs_key_possibility_does_not_create_upper_route_fact(self):
        mod = load_module()
        text = (
            "Knott sorts the keys and identifies the front door key, the back entry, "
            "and the interior keys he received with the property. Some may fit upstairs "
            "rooms or old interior locks, but he has not tested every one."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=1,
            pc_name="Evelyn Reed",
        )

        facts = mod.facts_from_visible(state)

        self.assertFalse(any("upper-floor route" in fact.lower() for fact in facts))

    def test_before_setting_foot_inside_does_not_mean_interior_started(self):
        mod = load_module()
        text = (
            "Knott approves records and newspaper research before setting foot inside the house."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=1,
            pc_name="Evelyn Pierce",
        )

        facts = mod.facts_from_visible(state)

        self.assertFalse(any("interior" in fact.lower() for fact in facts))

    def test_nobody_does_not_create_corbitt_body_fact(self):
        mod = load_module()
        text = (
            "Knott says the Corbitt House has a bad reputation and nobody wanted "
            "the place after the Macario family left."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=1,
            pc_name="Evelyn Pierce",
        )

        facts = mod.facts_from_visible(state)

        self.assertFalse(any("walter" in fact.lower() for fact in facts))
        self.assertFalse(any("direct focus" in fact.lower() for fact in facts))

    def test_religious_body_does_not_create_corbitt_body_fact(self):
        mod = load_module()
        text = (
            "Corbitt's executor points to a named religious body and a specific clergyman, "
            "not to a corpse, remains, or named figure in the room."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=2,
            pc_name="Evelyn Pierce",
        )

        facts = mod.facts_from_visible(state)

        self.assertFalse(any("direct focus" in fact.lower() for fact in facts))

    def test_chapel_remains_broken_near_corbitt_does_not_create_body_fact(self):
        mod = load_module()
        text = (
            "The chapel around you remains broken, dusty, and unstable. "
            "You connect these observations: the chapel's confirmed connection to a "
            "specific symbol, the earlier 1912 police and Reverend Michael Thomas trail, "
            "and the still-unresolved question of how all of it ties back to Corbitt "
            "strongly enough to matter."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=5,
            pc_name="Evelyn Pierce",
        )

        facts = mod.facts_from_visible(state)

        self.assertFalse(any("direct focus" in fact.lower() for fact in facts))
        self.assertFalse(any("walter" in fact.lower() for fact in facts))

    def test_buried_by_records_handling_does_not_create_burial_or_basement_clue(self):
        mod = load_module()
        text = (
            "The surviving paperwork has the feel of something handled, narrowed, and tidied "
            "after the fact rather than fully aired. Some details may simply have been buried "
            "by the same handling that makes the whole affair smell of suppression. Reverend "
            "Michael Thomas and the Chapel of Contemplation remain the visible lead."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=4,
            pc_name="Evelyn Price",
        )

        facts = mod.facts_from_visible(state)

        joined = " ".join(facts).lower()
        self.assertNotIn("burial", joined)
        self.assertNotIn("basement clue", joined)
        self.assertFalse(mod.explicit_burial_clue_visible(text))

    def test_destroyed_ruin_language_does_not_mark_objective_resolved(self):
        mod = load_module()
        text = (
            "What survives does so in pockets: a cabinet not wholly destroyed, warped storage, "
            "and scraps of paper. A later pushed search becomes destructive rather than illuminating, "
            "but it does not resolve the case or destroy Walter Corbitt."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=5,
            pc_name="Evelyn Price",
        )

        facts = mod.facts_from_visible(state)

        self.assertFalse(any("objective appears resolved" in fact.lower() for fact in facts))

    def test_stop_reason_does_not_treat_corbitt_house_and_no_collapse_as_resolved(self):
        mod = load_module()
        text = (
            "Nothing dramatic collapses, but this line of inquiry is spent now. "
            "From here, the live thread that still matters most is the Corbitt house itself."
        )

        self.assertIsNone(mod.stop_reason(text))

    def test_chapel_remains_as_ruins_near_corbitt_does_not_create_body_fact(self):
        mod = load_module()
        text = (
            "Dust lies thick over broken furniture and fallen fragments. The chapel "
            "has the ugly atmosphere of a place people stopped tending long ago, but "
            "not of a place history has finished with; its remains still carry that "
            "sense of bad history. On this pass you do not recover a decisive record "
            "tying Corbitt, Reverend Michael Thomas, or the 1912 business together."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=4,
            pc_name="Evelyn Pierce",
        )

        facts = mod.facts_from_visible(state)

        self.assertFalse(any("direct focus" in fact.lower() for fact in facts))
        self.assertFalse(any("walter" in fact.lower() for fact in facts))

    def test_walter_named_without_body_does_not_create_body_fact(self):
        mod = load_module()
        text = (
            "The Michael Thomas / Chapel of Contemplation / 1912 trail remains known. "
            "The official trail points back to Walter Corbitt and to the Corbitt House, "
            "but it does not reveal a corpse, grave, or moving threat."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=5,
            pc_name="Evelyn Pierce",
        )

        facts = mod.facts_from_visible(state)

        self.assertFalse(any("direct focus" in fact.lower() for fact in facts))

    def test_generic_bed_movement_does_not_create_corbitt_body_fact(self):
        mod = load_module()
        text = (
            "Earlier records mention the Corbitt House and Reverend Michael Thomas. "
            "Upstairs, Evelyn sees a bed shape under dust and hears a faint movement "
            "deeper in the room, but no body, corpse, remains, or named figure is visible."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=9,
            pc_name="Evelyn Pierce",
        )

        facts = mod.facts_from_visible(state)

        self.assertFalse(any("direct focus" in fact.lower() for fact in facts))

    def test_generic_bed_movement_does_not_trigger_corbitt_attack(self):
        mod = load_module()
        text = (
            "Earlier records mention the Corbitt House and Reverend Michael Thomas. "
            "Upstairs, Evelyn sees a bed shape under dust and hears a faint movement "
            "deeper in the room, but no body, corpse, remains, or named figure is visible."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=9,
            pc_name="Evelyn Pierce",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "second_chapel_method",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertNotEqual(
            decision["response_contract"]["intent"],
            "attack_visible_corbitt_threat",
        )

    def test_violent_bed_lurch_triggers_defensive_response_not_clarification(self):
        mod = load_module()
        text = (
            "You are still where you wanted to be: on the landing side, retreat open. "
            "Then the room answers. The bed gives a sudden, violent lurch that is not "
            "caused by your stick, your footing, or any ordinary settling of furniture. "
            "The movement is abrupt, forceful, and plainly impossible. Evelyn is already "
            "backing toward the landing as promised."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=9,
            pc_name="Evelyn Hart",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "retreat_from_moving_bedroom_threat",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("landing", decision["sent_to_gm"].lower())

    def test_bed_stirs_after_upstairs_success_triggers_retreat_not_probe(self):
        mod = load_module()
        text = (
            "[roll]Cautious upstairs inspection from the landing: 1d100 = 16 vs Spot Hidden 70. "
            "Hard success.[/roll] From the landing and thresholds, Evelyn can see disturbed "
            "bedclothes, a wardrobe, window grime, and loose papers. Then, as she watches from "
            "the doorway, the room betrays itself. The bed stirs. Not with the sliding collapse "
            "of rotten springs or the settling of old wood after a touch, but with a movement "
            "that does not match her position at the threshold. The landing is still open behind her."
        )
        state = mod.VisibleState(
            transcript=(
                "The ground floor search identified the stair leading up and the way down toward the basement.\n"
                + text
            ),
            last_reply=text,
            turn=11,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "retreat_from_moving_bedroom_threat",
        )
        self.assertNotIn("probe_clarified_bedroom_threshold", decision_text)

    def test_sanity_failed_impossible_bedroom_hazard_withdraws_not_reprobe(self):
        mod = load_module()
        text = (
            "[roll]Sanity roll -- impossible movement of the bed: 1d100 = 80 vs 65 -- failure.[/roll]\n\n"
            "The sight of the bed moving with no visible hand on it is too much to file away as "
            "creaking boards or tricks of light. A cold, involuntary certainty crawls up Evelyn's "
            "spine: something in that room is wrong in a way the ordinary world does not permit.\n\n"
            "You recoil onto the landing, keeping the doorframe between yourself and the bedroom. "
            "Your breath catches; the house seems to go still around that one violated fact.\n\n"
            "[roll]INT roll -- comprehend the impossible shock: 1d100 = 37 vs 11 -- failure.[/roll]\n\n"
            "The shock lands hard, but your mind refuses to fully take it in all at once. You do "
            "not break immediately; instead, the horror sits in you like a swallowed shard.\n\n"
            "For a long second, nothing lunges through the doorway.\n\n"
            "Then the silence returns--thick, dusty, waiting. From where you hold on the landing, "
            "you hear only the old house settling and the faint aftermath of disturbed furniture "
            "within the room.\n\n"
            "SAN reduced by 5. Current SAN: 60/65.\n\n"
            "What does Evelyn do now?"
        )
        state = mod.VisibleState(
            transcript="The bed moved by itself and Evelyn retreated to the landing.\n" + text,
            last_reply=text,
            turn=12,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "moving_bedroom_threat",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "withdraw_from_confirmed_impossible_bedroom_hazard",
        )
        self.assertIn("san", decision["sent_to_gm"].lower())
        self.assertNotIn("probe_clarified_bedroom_threshold", decision_text)

    def test_successful_bedroom_probe_with_bed_papers_window_marks_follows_up_not_clarify(self):
        mod = load_module()
        text = (
            "[roll]Probe the upstairs bedroom safely from the doorway: 1d100 = 42 vs Spot Hidden 70 -- success.[/roll]\n"
            "From the landing side of the threshold, with your escape line still open behind you, you keep "
            "the flashlight beam low and controlled. That caution pays off. Before touching anything more "
            "than your walking stick can safely reach, you make out that this bedroom is not just neglected. "
            "It is disturbed in a way the other rooms did not clearly yield from the doorway. The bed does "
            "not sit quite naturally in the room: not smashed, not overturned, but wrong in its placement "
            "and in the way the bedding lies, as if force or violent motion once involved it rather than "
            "ordinary use. The floorboards near it show scuffing that reads less like random age and more "
            "like repeated strain in a confined space. Your beam catches loose papers as well--real papers, "
            "not just trash--and at least some of them look worth preserving on film before any hand goes "
            "near them. From here, you can also see marks around the window frame and nearby woodwork that "
            "suggest the room has been under stress before. Nothing moves on its own yet. Nothing rushes you. "
            "You still have the landing immediately behind you, the doorway between you and the room."
        )
        state = mod.VisibleState(
            transcript="Evelyn safely probed the bedroom from the threshold.\n" + text,
            last_reply=text,
            turn=10,
            pc_name="Evelyn Marsh",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "inspect_bed_centered_scene_from_threshold",
        )
        self.assertIn("papers", decision["sent_to_gm"].lower())
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertNotIn("abandon_unproductive_bedroom", decision_text)

    def test_landing_crash_after_bed_attack_recovers_position_not_basement_switch(self):
        mod = load_module()
        text = (
            "[roll]Seize the landing before the bed crashes through the doorway: "
            "1d100 = 79 vs DEX 65 — failure.[/roll] "
            "[roll]Dodge the lunging bed as it bursts through the doorway: "
            "1d100 = 64 vs Dodge 57 — failure.[/roll] "
            "You back onto the landing just as the bedroom erupts. The bed does not stop. "
            "With a wooden shriek it bucks sideways and slams through the doorway hard enough "
            "to shake the frame. A smaller piece of furniture skids and clips after it, turning "
            "the narrow landing into a sudden crush of polished wood, torn bedding, and splintering "
            "legs. Something catches your hip and shin and hurls you sideways onto the landing boards. "
            "You are down, sprawled hard near the head of the stairs, the doorframe rattling above "
            "you while the bed jams half across the threshold. The sound is furniture moving with "
            "purpose where no human hand touches it."
        )
        state = mod.VisibleState(
            transcript="The bed moved by itself and Evelyn tried to retreat from the bedroom.\n" + text,
            last_reply=text,
            turn=11,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "moving_bedroom_threat",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "recover_from_bedroom_landing_crash",
        )
        self.assertIn("down on the landing", action)
        self.assertNotIn("basement route", action)
        self.assertNotIn("ground floor", action)

    def test_bed_moved_on_its_own_after_probe_triggers_retreat_not_fallback(self):
        mod = load_module()
        text = (
            "[roll]Witnessing the bed move on its own: 1d100 = 79 vs Sanity 60 — failure. "
            "You lose 4 Sanity.[/roll] The moment the mattress shifts again—without weight, "
            "without touch, without any human cause your mind can accept—your stomach drops cold. "
            "The movement is real. Evelyn recoils exactly as planned, backing off the threshold "
            "toward the landing, retreat path still open. You connect these observations: the bed "
            "moved on its own; the loose papers are still in the room beyond the threshold; you are "
            "not trapped in the bedroom; the stairs remain behind you."
        )
        state = mod.VisibleState(
            transcript=(
                "The ground floor search identified the stair leading up and the way down toward the basement.\n"
                + text
            ),
            last_reply=text,
            turn=12,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "retreat_from_moving_bedroom_threat",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_bedroom_hard_success_with_actual_document_inspects_paper_not_reclarify(self):
        mod = load_module()
        text = (
            "[roll]Careful doorway inspection of the upstairs bedroom -- 1d100: 30 vs Spot Hidden 65 -- Hard success.[/roll] "
            "Keeping to the landing side of the threshold pays off. Your flashlight, kept low, picks out "
            "the room in pieces: the stale bed, warped floorboards, the wardrobe, window frame, and loose "
            "papers scattered where they fell. Several of the papers are not just rubbish. At least one "
            "looks like an actual document rather than torn scrap, and there are concrete disturbances "
            "in the room worth recording before anything is touched further--subtle disarray around the "
            "bed area and the papers. Nothing in your reach needs to be guessed at; the papers can be "
            "photographed from the doorway before deciding whether to hook one closer."
        )
        state = mod.VisibleState(
            transcript="Evelyn is probing the upstairs bedroom from the threshold.\n" + text,
            last_reply=text,
            turn=9,
            pc_name="Evelyn Hart",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "inspect_reachable_bedroom_paper_from_threshold",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_failed_bedroom_threshold_probe_abandons_route_not_fallback(self):
        mod = load_module()
        text = (
            "[roll]Careful doorway search of the bedroom — 1d100: 86 vs Spot Hidden 55 — failure.[/roll] "
            "From the landing side of the bedroom threshold, Evelyn keeps her retreat path open. "
            "Nothing in that first cautious pass yields a concrete mark, legible document, or obvious tampering. "
            "The bed and papers remain inside the stale room, but the room itself has not yet forced the issue."
        )
        state = mod.VisibleState(
            transcript=(
                "The ground floor search identified the stair leading up and the way down toward the basement.\n"
                + text
            ),
            last_reply=text,
            turn=14,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "abandon_unproductive_bedroom_after_threshold_probe",
        )
        self.assertIn("ground floor", decision["sent_to_gm"].lower())
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_plain_bed_affordance_after_threshold_probe_abandons_without_reclarify(self):
        mod = load_module()
        text = (
            "The plain thing in front of Evelyn is the upstairs bedroom itself: doorway, stale air, "
            "bed as its most immediate feature, and a floor she has not committed to crossing. "
            "Nothing in it is moving right now. The nearest actionable feature is the bed, but its "
            "significance is not yet proven. She cannot yet prove intent, movement, or threat from "
            "this position, and the room has not yet yielded anything conclusive. The landing and "
            "stairward retreat remain open behind her."
        )
        state = mod.VisibleState(
            transcript=(
                "The ground floor search identified the stair leading up and the way down toward the basement.\n"
                + text
            ),
            last_reply=text,
            turn=15,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "abandon_unproductive_bedroom_after_threshold_probe",
        )
        self.assertIn("basement", decision["sent_to_gm"].lower())
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_lurching_bed_attack_after_bed_edge_probe_triggers_retreat_not_clarify(self):
        mod = load_module()
        text = (
            "The whole bed answers. Not with the loose slump of old furniture, but with a "
            "violent, sudden heave. The mattress bucks toward the doorway hard enough to "
            "turn a cautious probe into an attack. Nell catches the truth of it: this is no "
            "simple shift of rotten wood. The motion is purposeful. The attack came from the "
            "bed itself. Because she was already braced, Nell recoils cleanly out of the room. "
            "The upstairs bedroom is no longer merely eerie; it is actively dangerous; the "
            "attack came from the bed area when disturbed; Nell is not hit, not trapped, and "
            "has fallen back out of immediate reach; the landing and stairward retreat are "
            "still open behind her."
        )
        state = mod.VisibleState(
            transcript=(
                "The ground floor search identified the stair leading up and the way down toward the basement.\n"
                + text
            ),
            last_reply=text,
            turn=15,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "bed_edge_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = str(decision)

        self.assertEqual(
            decision["response_contract"]["intent"],
            "retreat_from_moving_bedroom_threat",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_self_driven_bed_lurch_and_moving_by_itself_triggers_retreat(self):
        mod = load_module()
        text = (
            "The stick catches the hanging spread and lifts. For one heartbeat, it feels like "
            "ordinary weight. Then the whole bed answers. Not a settling creak. Not a shifted "
            "blanket. The frame heaves with a violent, self-driven lurch, surging toward the "
            "doorway. The bed's violent movement is unmistakably not natural. "
            "[roll]Sanity roll at the sight of a bed moving by itself -- 1d100: 43 vs Sanity 70 "
            "-- Success.[/roll] Evelyn is now back on the landing side, retreat path still open "
            "toward the stairs, with the bedroom in front of her and the bed inside plainly "
            "capable of moving on its own."
        )
        state = mod.VisibleState(
            transcript=(
                "The ground floor search identified the stair leading up and the way down toward the basement.\n"
                + text
            ),
            last_reply=text,
            turn=13,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "bed_edge_probe",
            },
        )

        self.assertTrue(mod.moving_bedroom_threat_visible(text))
        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "retreat_from_moving_bedroom_threat",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_paper_and_bed_self_movement_after_probe_abandons_unsafe_bedroom(self):
        mod = load_module()
        text = (
            "[roll]Search the bedroom from the threshold without entering — 1d100: 85 vs target 70 — Failure.[/roll] "
            "One of the loose papers shifts across the floor without any draft she can account for. "
            "A beat later, the bed gives a hard scraping jerk against the boards. "
            "Not a settling creak. Movement. The thing has moved by itself. "
            "Evelyn's retreat path is still open, and she is outside the room, on the landing side "
            "of the threshold, with the stairs behind her."
        )
        state = mod.VisibleState(
            transcript=(
                "Hall of Records showed Reverend Michael Thomas of the Chapel of Contemplation, "
                "and the Chapel trail includes 1912. "
                "The ground floor search identified the stair leading up and the way down toward the basement.\n"
                + text
            ),
            last_reply=text,
            turn=10,
            pc_name="Evelyn Hart",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "moving_bedroom_threat",
                "bedroom_threshold_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "abandon_unsafe_bedroom_after_repeated_movement",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("basement", decision["sent_to_gm"].lower())

    def test_bedroom_probe_after_prior_movement_without_new_clue_abandons_room(self):
        mod = load_module()
        text = (
            "From the landing side of the threshold, your flashlight stays low and controlled. "
            "The bed sits in the stale dark with its covers disturbed from the earlier violence, "
            "but from here nothing lunges at you. The beam picks out scuffed floorboards, dust, "
            "and loose papers lying still. Nothing answers with a sudden movement. "
            "You remain outside the room proper, with the stairs behind you still open."
        )
        state = mod.VisibleState(
            transcript=(
                "The ground floor search identified the stair leading up and the way down toward the basement.\n"
                "The bed previously moved by itself in the upstairs bedroom.\n"
                + text
            ),
            last_reply=text,
            turn=12,
            pc_name="Evelyn Hart",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "moving_bedroom_threat",
                "bedroom_threshold_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "abandon_unsafe_bedroom_after_repeated_movement",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("basement", decision["sent_to_gm"].lower())

    def test_canted_bed_after_lunge_abandons_room_without_second_clarification(self):
        mod = load_module()
        text = (
            "The single most concrete visible affordance right now is the bed itself in the upstairs "
            "bedroom. It is just inside the bedroom beyond your current threshold, a few feet in from "
            "the doorway, no longer sitting square where a normal bed should. From where you are on "
            "the landing side, you can see that it is canted at an unnatural angle across the floorboards, "
            "as though it struck forward and then stopped wrong. As for reach: it is visible from where "
            "you are, but not safely reachable. Closing distance to the bed would mean re-entering the "
            "dangerous room."
        )
        state = mod.VisibleState(
            transcript=(
                "The ground floor search identified the stair leading up and the way down toward the basement.\n"
                "The bed previously moved by itself in the upstairs bedroom.\n"
                + text
            ),
            last_reply=text,
            turn=13,
            pc_name="Evelyn Hart",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "moving_bedroom_threat",
                "bedroom_threshold_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "abandon_unsafe_bedroom_after_repeated_movement",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("basement", decision["sent_to_gm"].lower())

    def test_bed_affordance_with_safer_retreat_line_abandons_without_reclarify(self):
        mod = load_module()
        text = (
            "From here, you can perceive a disturbed upstairs bedroom just beyond you: stale bedding, "
            "the close air of a room left unused too long, and the kind of silence that makes the house "
            "answer its own creaks. The bed is the immediate object in front of you inside that room. "
            "It is the nearest thing that matters. Your retreat path is still behind you, not through "
            "the room. To get out, you back away to the hall and then toward the stairs. Nothing you "
            "must touch to withdraw lies past the bed. What remains uncertain is the important part: "
            "you have enough to say the room is wrong, but not enough to say why. You cannot yet prove "
            "whether what just happened came from the bed itself, from something else in the room, or "
            "from something in the house using the room as its stage. You also cannot yet say whether "
            "approaching the bed will reveal evidence, trigger another violent movement, or both. "
            "So the latest actionable thing, put plainly, is this: the bed in the upstairs room is the "
            "nearest suspicious object, it sits ahead of you rather than along your escape, and examining "
            "it more closely would mean choosing to leave the safer line of retreat and commit to the room."
        )
        state = mod.VisibleState(
            transcript=(
                "The ground floor search identified the stair leading up and the way down toward the basement.\n"
                "The bed moved by itself in the upstairs bedroom, and Evelyn stayed on the landing.\n"
                + text
            ),
            last_reply=text,
            turn=13,
            pc_name="Evelyn Pierce",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "moving_bedroom_threat",
                "bedroom_threshold_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "abandon_unsafe_bedroom_after_repeated_movement",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("basement", decision["sent_to_gm"].lower())

    def test_bedroom_doorway_after_impossible_bed_movement_abandons_without_reclarify(self):
        mod = load_module()
        text = (
            "The single most concrete affordance in front of Evelyn is the bedroom doorway itself. "
            "It is directly in front of her off the upper landing, a few steps away at most. "
            "Beyond it, she can still make out the small sagging bed, dim and motionless again. "
            "What she gets from it now is not fresh movement but aftermath: a dry, faint rustle "
            "of disturbed bedding. The room is there; the bed is inside; something in that room "
            "moved impossibly a moment ago; it is quiet again for the moment; what caused the "
            "movement; whether anything else in the room is active; whether the danger is over "
            "or merely paused. The doorway is safely reachable from her current position in the "
            "limited sense that she does not have to move deeper into the room to use it as a "
            "reference point, but crossing into the room itself would no longer be safe in any "
            "assured sense. The stair retreat is still open behind her."
        )
        state = mod.VisibleState(
            transcript=(
                "The ground floor search identified the stair leading up and the way down toward the basement.\n"
                "The bed moved impossibly in the upstairs bedroom.\n"
                + text
            ),
            last_reply=text,
            turn=12,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "moving_bedroom_threat",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = str(decision)

        self.assertEqual(
            decision["response_contract"]["intent"],
            "abandon_unsafe_bedroom_after_repeated_movement",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("basement", decision["sent_to_gm"].lower())

    def test_doorway_after_bed_lunge_with_uncertain_movement_abandons_without_reclarify(self):
        mod = load_module()
        text = (
            "The single most concrete visible affordance is the bedroom doorway ahead of you. "
            "It is just beyond the upper hall, open enough to give you a partial angle into the "
            "room. Inside, you can see the edge of a small bed and the dim outline of stale "
            "bedding. The faint rattling and answering creak of floorboards seems to come from "
            "that room and its threshold. What you cannot yet prove from here is whether the "
            "movement is caused by a draft, loose boards, your nerves, or something acting inside "
            "the room. The doorway itself is safely reachable, but what is not yet safe to assume "
            "is that crossing that threshold would be equally safe."
        )
        state = mod.VisibleState(
            transcript=(
                "The ground floor search identified the stair leading up and the way down toward the basement.\n"
                "The bed jerked violently toward the doorway and the room is actively hostile.\n"
                + text
            ),
            last_reply=text,
            turn=13,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "moving_bedroom_threat",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = str(decision)

        self.assertEqual(
            decision["response_contract"]["intent"],
            "abandon_unsafe_bedroom_after_repeated_movement",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("basement", decision["sent_to_gm"].lower())

    def test_clarified_reachable_bedroom_paper_triggers_probe_not_repeat_clarification(self):
        mod = load_module()
        text = (
            "The clearest concrete affordance from Evelyn's current position is the loose paper "
            "nearest the bedroom doorway. Exact location: on the bedroom floor, just a little past "
            "the threshold. It is still within reach of your walking stick while you remain on the "
            "landing side of the doorway. Whether it is safely reachable: yes. From your present "
            "position, you can probe or draw it closer with the walking stick without stepping deeper "
            "into the room."
        )
        state = mod.VisibleState(
            transcript=(
                "The ground floor search identified the stair leading up and the way down toward the basement.\n"
                + text
            ),
            last_reply=text,
            turn=11,
            pc_name="Evelyn Marsh",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "inspect_reachable_bedroom_paper_from_threshold",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("walking stick", decision["sent_to_gm"].lower())
        self.assertIn("paper", decision["sent_to_gm"].lower())

    def test_loose_paper_near_bedframe_after_threshold_success_triggers_probe(self):
        mod = load_module()
        text = (
            "The most concrete visible affordance is the loose sheet of paper near the bedframe. "
            "Its exact situation, from where you are now. You connect these observations: "
            "Location: inside the disturbed upstairs bedroom, on the floor close to the side of "
            "the bed, a little beyond the threshold rather than out on the landing; Appearance: "
            "a single loose page, pale in your flashlight beam, lying near the bed where the bedding's "
            "fresh sag and drag also stand out. It is distinct as a document, but not yet readable "
            "from your present angle; Sound: the page itself makes no sound now. A loose sheet lies "
            "near the bed and is worth checking without stepping into the room."
        )
        state = mod.VisibleState(
            transcript=(
                "The ground floor search identified the stair leading up and the way down toward the basement.\n"
                + text
            ),
            last_reply=text,
            turn=10,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "inspect_reachable_bedroom_paper_from_threshold",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("paper", decision["sent_to_gm"].lower())

    def test_visible_papers_and_reachable_item_after_threshold_success_triggers_probe(self):
        mod = load_module()
        text = (
            "[roll]Scan the bedroom from the threshold — 1d100: 6 vs 75 — Extreme Success.[/roll] "
            "Keeping yourself on the landing side of the doorway, you let the flashlight skim low and slow. "
            "From there, the bedroom resolves into a clearer picture: a small bed with stale, sunken bedding, "
            "loose papers scattered near it, and religious bric-a-brac. "
            "More importantly, from the threshold you pick out what is safe to distinguish before committing "
            "yourself farther in: nothing is moving on its own right now. "
            "You can start by photographing the visible papers and marks from where you stand, or use the stick "
            "to draw one reachable item nearer without crossing fully into the room."
        )
        state = mod.VisibleState(
            transcript=(
                "The ground floor search identified the stair leading up and the way down toward the basement.\n"
                "The bed heaved by itself in the upstairs bedroom, so Evelyn stayed on the landing.\n"
                + text
            ),
            last_reply=text,
            turn=12,
            pc_name="Evelyn Ward",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "moving_bedroom_threat",
                "bedroom_threshold_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "inspect_reachable_bedroom_paper_from_threshold",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("paper", decision["sent_to_gm"].lower())

    def test_doorway_success_worth_documenting_papers_triggers_probe(self):
        mod = load_module()
        text = (
            "[roll]Careful doorway search of the bedroom: 1d100 = 24 vs Spot Hidden 60 — Hard success.[/roll] "
            "From the landing-side threshold, with your light kept low and your stick doing the first touching, "
            "the room gives up more than it did at a glance. The loose papers are the most useful part of the "
            "sweep. Without stepping in, you identify a few sheets that look worth documenting before handling: "
            "not random trash, but household papers left in disorder. You can photograph their visible arrangement "
            "from the doorway before deciding whether to reach farther. From this angle, nothing you can reach "
            "reacts on its own."
        )
        state = mod.VisibleState(
            transcript=(
                "The bed moved by itself, so Evelyn stayed on the landing side of the bedroom threshold.\n"
                + text
            ),
            last_reply=text,
            turn=14,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "moving_bedroom_threat",
                "bedroom_threshold_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "inspect_reachable_bedroom_paper_from_threshold",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("photograph", decision["sent_to_gm"].lower())
        self.assertIn("paper", decision["sent_to_gm"].lower())

    def test_safe_photograph_visible_bedroom_papers_and_marks_triggers_probe(self):
        mod = load_module()
        text = (
            "[roll]Careful doorway inspection of the upstairs bedroom — 1d100: 7 vs Spot Hidden 70 — Extreme Success.[/roll] "
            "Keeping to the landing side of the threshold pays off. Your low flashlight beam picks out the room "
            "in usable detail without you having to step in: the stale bedding, the loose papers, the line of "
            "the wardrobe, the window frame, and more importantly, the places where the room's neglect doesn't "
            "quite look natural. From the doorway, before touching anything, you catch concrete disturbances: "
            "papers shifted irregularly rather than merely fallen, scuffing and drag-like marks around the bed "
            "area and floorboards, and the sort of small physical inconsistencies that justify photographing "
            "first and probing second. Nothing moves by itself yet. From where you are, you can safely photograph "
            "the visible papers and marks before disturbing them, and you still have your retreat path open onto "
            "the landing."
        )
        state = mod.VisibleState(
            transcript="The upstairs bedroom was inspected from the doorway.\n" + text,
            last_reply=text,
            turn=11,
            pc_name="Evelyn Pierce",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "inspect_reachable_bedroom_paper_from_threshold",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("photograph", decision["sent_to_gm"].lower())
        self.assertIn("paper", decision["sent_to_gm"].lower())

    def test_bedroom_documents_worth_photographing_prioritized_over_bed_edge(self):
        mod = load_module()
        text = (
            "[roll]Cautious doorway sweep of the upstairs bedroom — 1d100: 25 vs Spot Hidden 65. Success, Hard.[/roll] "
            "From the landing-side threshold, your flashlight’s beam crawls low across the room without forcing you inside. "
            "The bed is narrow and dust-filmed, its coverlet slumped but not freshly disturbed. "
            "The floorboards just beyond the doorway show age and grime, but no immediate sag, snap, or obvious trip of movement. "
            "Loose papers near the room's reach from the door are real enough to be worth your care rather than random trash. "
            "More importantly, among the scattered papers and the neglect, you pick out a few items that look deliberately kept "
            "rather than merely abandoned—documents worth photographing before handling. Nothing in the room shifts on its own "
            "while you watch."
        )
        state = mod.VisibleState(
            transcript="The upper floor room was searched from the threshold.\n" + text,
            last_reply=text,
            turn=14,
            pc_name="Nora Callahan",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "crawl_space_probe",
                "crawl_space_entry",
                "upper_floor_search",
                "bedroom_threshold_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "inspect_reachable_bedroom_paper_from_threshold",
        )
        self.assertNotEqual(
            decision["response_contract"]["intent"],
            "probe_disturbed_bed_edge_from_threshold",
        )

    def test_visible_bedroom_papers_requiring_step_trigger_bounded_entry_not_reclarify(self):
        mod = load_module()
        text = (
            "From where Evelyn is standing now, the clearest actionable thing is this: The papers are real, "
            "visible, and close enough to matter. They sit inside the bedroom ahead of you, farther in than "
            "the threshold but not beyond the room's center. To get at them properly, you would have to lean "
            "farther than is comfortable or step into the room. They are in front of you; your retreat is "
            "behind you through the landing and back to the stairs, and that path is still open. Nothing is "
            "moving now. Nothing has lunged, shifted, or declared itself while you watch."
        )
        state = mod.VisibleState(
            transcript=(
                "The ground floor search identified the stair leading up and the way down toward the basement.\n"
                + text
            ),
            last_reply=text,
            turn=9,
            pc_name="Evelyn Reed",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "inspect_visible_bedroom_papers_with_bounded_entry",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("paper", decision["sent_to_gm"].lower())
        self.assertIn("retreat", decision["sent_to_gm"].lower())

    def test_retrieved_blank_bedroom_paper_triggers_close_examination_not_reclarify(self):
        mod = load_module()
        text = (
            "The most concrete visible affordance now is the paper you already drew closer. "
            "It is at the bedroom threshold now, near your feet on the upstairs side of the "
            "doorway rather than farther in by the bed. In the beam of your flashlight it "
            "looks like a single light-colored sheet, slightly worn and flat after being "
            "dragged across the dusty boards. And yes: it is safely reachable from your "
            "current position. You do not need to step deeper into the bedroom to touch it, "
            "pick it up, or examine it more closely from where you are standing on the landing "
            "side of the threshold. It showed no clear visible writing, symbol, date, or name; "
            "whether there is faint, hidden, impressed, or otherwise subtle detail only visible "
            "on closer handling; whether the reverse side shows anything; or whether the "
            "blankness is exactly what it first appears to be."
        )
        state = mod.VisibleState(
            transcript=(
                "The ground floor search identified the stair leading up and the way down toward the basement.\n"
                "The bedroom paper was drawn to the threshold and looked blank from a first look.\n"
                + text
            ),
            last_reply=text,
            turn=12,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "bedroom_paper_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "closely_examine_retrieved_bedroom_paper",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("reverse", decision["sent_to_gm"].lower())
        self.assertIn("threshold", decision["sent_to_gm"].lower())

    def test_conspicuously_blank_bedroom_paper_at_threshold_does_not_reclarify(self):
        mod = load_module()
        text = (
            "[roll]Read the loose paper from the bedroom threshold — 1d100: 45 vs Spot Hidden 70 — success.[/roll] "
            "At the doorway, close enough to inspect without crossing the threshold, you can make out that "
            "the paper is not a written letter at all. There is no message, no date, no name, no symbol worked "
            "into it. It is conspicuously blank—an ordinary loose sheet, but set here in a way that feels deliberate "
            "rather than accidental. The upstairs remains very still. The stair behind you is clear."
        )
        state = mod.VisibleState(
            transcript="Evelyn is upstairs at the bedroom threshold; the basement route was mapped earlier.\n" + text,
            last_reply=text,
            turn=14,
            pc_name="Evelyn March",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "basement_concealed_probe",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "bedroom_paper_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "closely_examine_retrieved_bedroom_paper",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("threshold", decision["sent_to_gm"].lower())

    def test_drawn_nearer_unmarked_bedroom_paper_triggers_close_examination(self):
        mod = load_module()
        text = (
            "The single most concrete visible affordance is still the old loose paper you’ve "
            "drawn nearer the doorway. It is now on the bedroom floor just inside the threshold, "
            "slightly right of center from your stance on the landing side. It lies in your "
            "flashlight beam close enough that you do not need to enter the room to inspect it "
            "further. It looks like a single old sheet, dust-dulled, thin, and slightly curled "
            "at the edges, with the surface turned enough toward your light to show that it is "
            "not boldly marked—no large heading, no obvious emblem, no striking date or name "
            "leaping out at a glance. It is safely reachable from your current position: you can "
            "look more closely, illuminate it better, read what can be read, or manipulate it "
            "with the stick while keeping your retreat path open."
        )
        state = mod.VisibleState(
            transcript="The bedroom paper was nudged nearer the doorway.\n" + text,
            last_reply=text,
            turn=15,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "bedroom_paper_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "closely_examine_retrieved_bedroom_paper",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("read", decision["sent_to_gm"].lower())

    def test_brought_closer_faint_bedroom_paper_triggers_close_examination(self):
        mod = load_module()
        text = (
            "The sheet shifts with a dry, light scrape across the floorboards and comes nearer "
            "in a small, obedient drag. Brought closer to the doorway, the paper is easier to "
            "read from safety. What stands out first is not a name or date but the condition "
            "of it: a single loose sheet, old and dusty, with no obvious seal or fresh handling "
            "on it. In the light from where she stands, there is no bold symbol, no dark stain, "
            "no instantly unmistakable name jumping off the page. If there is writing, it is "
            "faint enough from this angle and light that she must study the surface carefully "
            "rather than glance and know. The paper has moved normally, and the immediate result "
            "is that it now lies closer to the doorway, within safer viewing distance than before."
        )
        state = mod.VisibleState(
            transcript="The bedroom paper was drawn closer from the threshold.\n" + text,
            last_reply=text,
            turn=12,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "bedroom_paper_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "closely_examine_retrieved_bedroom_paper",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("faint", decision["sent_to_gm"].lower())

    def test_drawn_close_enough_apparent_blankness_triggers_close_examination(self):
        mod = load_module()
        text = (
            "The walking stick nudges and hooks only enough to bring the sheet nearer the doorway. "
            "Drawn close enough to the threshold for a real look, the sheet proves to be less useful "
            "than its mere presence suggested. What can be honestly made out from here is not a name, "
            "not a date, not a symbol, not a clear written line. If there was once anything meaningful "
            "on this face, it is not plainly visible from this angle and in this condition. The immediate "
            "result is concrete but narrow: Evelyn has successfully brought one loose sheet to the doorway "
            "and established, without stepping into the room, that it shows no clearly legible writing, "
            "symbol, date, name, or anything but apparent blankness from this side of the threshold."
        )
        state = mod.VisibleState(
            transcript="The bedroom paper was drawn close enough to the threshold.\n" + text,
            last_reply=text,
            turn=13,
            pc_name="Evelyn Hart",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "bedroom_paper_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "closely_examine_retrieved_bedroom_paper",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("reverse", decision["sent_to_gm"].lower())

    def test_threshold_paper_with_no_obvious_message_triggers_close_examination(self):
        mod = load_module()
        text = (
            "[roll]Careful inspection of the bedroom paper with a walking stick: success.[/roll]\n\n"
            "Evelyn can hook the loose paper and draw it toward the doorway without putting her "
            "weight past the threshold. The paper comes like ordinary paper, with a dry scrape "
            "over the old boards. In the flashlight beam there is no obvious message leaping "
            "off the page: no bold name, no clear date, no large symbol visible at a glance. "
            "If there is writing, it is faint, small, obscured by age, or turned away from her "
            "current angle. The bed is wrong in a subtler way. Not moving yet, but tense. "
            "Stepping farther in would be a separate risk. You are still in the doorway area "
            "with the stairs clear behind you, the paper now as near as the threshold allows."
        )
        state = mod.VisibleState(
            transcript="The bedroom paper was drawn toward the threshold.\n" + text,
            last_reply=text,
            turn=14,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "bedroom_paper_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "closely_examine_retrieved_bedroom_paper",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("threshold", decision["sent_to_gm"].lower())
        self.assertIn("faint", decision["sent_to_gm"].lower())

    def test_disturbed_bed_edge_reachable_from_threshold_triggers_probe_not_reclarify(self):
        mod = load_module()
        text = (
            "The single most concrete visible affordance is still the near edge of the bed and "
            "its hanging bedspread. Exact location: a few feet inside the bedroom, directly in "
            "front of Evelyn from the doorway. The mattress has a slight settled sag, but the "
            "important part is the bedding: it does not lie flat in the ordinary way of an "
            "abandoned room. The cover and upper surface look shifted, uneven, and faintly "
            "dragged. At this moment, nothing sounds from the bed itself. With the walking stick: "
            "yes. She can probe, lift, or tug the nearest edge from the doorway without moving "
            "deeper. By hand: not safely guaranteed."
        )
        state = mod.VisibleState(
            transcript=(
                "The ground floor search identified the stair leading up and the way down toward the basement.\n"
                + text
            ),
            last_reply=text,
            turn=13,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "probe_disturbed_bed_edge_from_threshold",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("walking stick", decision["sent_to_gm"].lower())
        self.assertIn("bed", decision["sent_to_gm"].lower())

    def test_downstairs_does_not_count_as_upper_route(self):
        mod = load_module()

        self.assertFalse(mod.upper_route_visible("The route downstairs is the clearest lead."))
        self.assertTrue(mod.basement_route_visible("The route downstairs is the clearest lead."))
        self.assertFalse(
            mod.upper_route_visible(
                "The concealed section is in the basement, with the stairs and marked retreat route still behind her."
            )
        )

    def test_downward_route_from_last_reply_triggers_basement_not_upstairs(self):
        mod = load_module()
        text = (
            "The entry hall has been searched. The downward route is the one that most "
            "clearly asks to be tested. From where Evelyn stands, the basement access is "
            "under close inspection and the ground-floor route back remains open."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=9,
            pc_name="Evelyn Marsh",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "second_chapel_method",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "safe_basement_descent_and_search",
        )
        self.assertNotIn("Upstairs", decision["sent_to_gm"])

    def test_returned_basement_access_after_bedroom_retreat_resumes_route_not_fallback(self):
        mod = load_module()
        text = (
            "From where Evelyn stands now, the most immediate thing is the basement access. "
            "You can see the way down: a door or stair opening on the ground floor leading into the dark below. "
            "It is the nearest concrete lead because it is already identified as the route you came back to on purpose. "
            "Your retreat path is simple from here. Behind you is the way back through the ground floor and out of the house. "
            "What remains uncertain is everything below the threshold."
        )
        state = mod.VisibleState(
            transcript=(
                "Earlier, Evelyn found disturbed earth and a concealed crawl-space line in the basement.\n"
                + text
            ),
            last_reply=text,
            turn=18,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "second_chapel_method",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "crawl_space_probe",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "moving_bedroom_threat",
                "bedroom_abandoned",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "resume_mapped_basement_route_after_bedroom_retreat",
        )
        self.assertIn("basement", decision["sent_to_gm"].lower())
        self.assertIn("crawl", decision["sent_to_gm"].lower())
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_successful_basement_edge_with_chapel_carving_inspects_opened_crawl_space(self):
        mod = load_module()
        text = (
            "[roll]Careful probe of the altered basement section -- 1d100: 54 vs 65 -- success.[/roll] "
            "What first looked like mere irregularity proves to have a concealed edge: not a body, "
            "not anything that lunges, but a hidden seam in the altered section. Your cautious pressure "
            "finds where the surface gives slightly, and beyond it there is a cramped dark space rather "
            "than solid wall. As the concealment yields enough to inspect, you make out words cut into "
            "the inner wall: Chapel of Contemplation. The basement air seems colder there, and the gap "
            "beyond looks less like ordinary storage than something deliberately shut away. Nothing attacks "
            "and your retreat line to the stairs remains open."
        )
        state = mod.VisibleState(
            transcript="The basement follow-up just produced a local hidden seam.\n" + text,
            last_reply=text,
            turn=18,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "inspect_opened_crawl_space_and_chapel_carving",
        )
        self.assertIn("crawl-space", decision["sent_to_gm"].lower())
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_successful_concealed_boundary_with_rough_opening_inspects_crawl_space(self):
        mod = load_module()
        text = (
            "[roll]Examine the rough boards and concealed boundary in the basement -- "
            "1d100: 18 vs Spot Hidden 65 -- Hard success.[/roll] "
            "The boundary is real: a concealed section has been boarded over on purpose. "
            "Your flashlight catches a carved line on the inner wall beyond the rough opening. "
            "Once you get the angle, the words become legible: Chapel of Contemplation. "
            "The air coming from beyond is colder and wetter than the rest of the basement. "
            "The space past the boards looks less like storage and more like something deliberately hidden. "
            "You are still braced near the stairs, with room to fall back if needed, and you have not yet "
            "reached into the opening."
        )
        state = mod.VisibleState(
            transcript="Evelyn has already found and probed the concealed basement section.\n" + text,
            last_reply=text,
            turn=21,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "basement_concealed_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "inspect_opened_crawl_space_and_chapel_carving",
        )
        self.assertIn("chapel of contemplation", decision["sent_to_gm"].lower())
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_crawl_space_chapel_clue_plus_rats_withdraws_to_chapel_not_repeat_probe(self):
        mod = load_module()
        text = (
            "[roll]Cautious one-body-length probe into the crawl space: "
            "1d100 = 1 vs target 70 — critical success.[/roll] "
            "The crawl space was made to conceal. It continues inward as a real concealed passage "
            "and bears signs of deliberate human ritual use. In the beam's edge, you spot rats "
            "deeper inside. The passage is tight and unpleasantly confining, but you are not trapped "
            "where you are now and your retreat remains open exactly as intended."
        )
        state = mod.VisibleState(
            transcript=(
                "Earlier in the basement, Evelyn found carved words on the crawl-space wall: "
                "Chapel of Contemplation.\n"
                + text
            ),
            last_reply=text,
            turn=11,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "crawl_space_probe",
                "crawl_space_entry",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "withdraw_from_unresolved_crawl_space_and_investigate_chapel",
        )
        self.assertIn("chapel", decision_text)
        self.assertIn("backs out", decision_text)
        self.assertNotEqual(
            decision["response_contract"]["intent"],
            "probe_deeper_crawl_space_from_marked_exit",
        )

    def test_ground_floor_failure_without_downstairs_route_does_not_descend(self):
        mod = load_module()
        text = (
            "[roll]Methodical ground-floor search of the Corbitt House — 1d100: 99 vs target 75 — Failure.[/roll] "
            "You photograph papers where you find them, but on this pass nothing yields an obvious, "
            "trustworthy document fit to explain the house by itself. No clear footprint stands out. "
            "No scrape mark resolves into a definite recent trail. And while the house plainly continues "
            "beyond the rooms you have checked, no unmistakable route downstairs presents itself yet from "
            "where you are. You are still inside on the ground floor, with your back path marked and the "
            "entry route behind you."
        )
        state = mod.VisibleState(
            transcript=(
                "Knott believed one key might fit a cellar or interior lock, but Evelyn has not found a current basement route.\n"
                + text
            ),
            last_reply=text,
            turn=9,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
            },
        )

        self.assertFalse(mod.basement_route_visible(text))
        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertNotEqual(
            decision["response_contract"]["intent"],
            "safe_basement_descent_and_search",
        )
        self.assertNotIn("descends one step at a time", decision_text)

    def test_unconfirmed_cellar_smell_question_does_not_count_as_basement_route(self):
        mod = load_module()
        text = (
            "The single most concrete visible affordance is the first interior doorway off the entry hall. "
            "It is safely reachable while the exterior door remains open behind her. "
            "What remains unproven is whether the room beyond is empty, whether there are footprints, "
            "whether there is a cellar smell, draft, or another exit tied to that room."
        )
        state = mod.VisibleState(
            transcript=(
                "The chapel record said Corbitt was buried in the basement, but no current basement route has been found.\n"
                + text
            ),
            last_reply=text,
            turn=11,
            pc_name="Eleanor Price",
            attempted={
                "hall_records",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
            },
        )

        self.assertFalse(mod.basement_route_visible(text))
        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertNotEqual(
            decision["response_contract"]["intent"],
            "safe_basement_descent_and_search",
        )
        self.assertNotIn("basement door handle", decision_text)

    def test_fallback_clarification_action_avoids_field_manifest_prompt(self):
        mod = load_module()
        text = "The house remains confusing, with stale air and a few nearby features, but no clear result."
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=6,
            pc_name="Evelyn Price",
            attempted={"hall_records", "chapel", "house_exterior"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(decision["response_contract"]["intent"], "clarify_latest_visible_affordance")
        self.assertNotIn("exact location", action)
        self.assertNotIn("what it looks", action)
        self.assertNotIn("sounds, or smells", action)
        self.assertIn("plain", action)

    def test_clarified_basement_door_after_ground_floor_beats_old_upper_route(self):
        mod = load_module()
        text = (
            "The single most concrete visible affordance from your last search is the way down "
            "to the basement. Its exact location: on the ground floor, off the hall you have "
            "been working around clockwise from the entry, at the point where the house's stale "
            "smell turns heavier and more damp than in the front rooms. What it looks like: a "
            "closed basement door set low and plain into the interior line of the house, more "
            "workmanlike than decorative, with age on the wood and neglect around the frame. "
            "What it sounds like: nothing distinct from behind it. What it smells like: old "
            "plaster, dust, trapped damp--and here, more of a cellar taint than elsewhere. "
            "Whether it is safely reachable from your current position: yes. From where you "
            "are now on the ground floor with your retreat path marked, you can reach that "
            "door without going upstairs or committing yourself down the stairs."
        )
        state = mod.VisibleState(
            transcript=(
                "The entry hall has a staircase rising upward, openings into front rooms, and "
                "a possible basement route.\n" + text
            ),
            last_reply=text,
            turn=9,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "safe_basement_descent_and_search",
        )
        self.assertIn("basement", action)
        self.assertNotIn("upstairs", action)

    def test_ground_floor_strongest_hint_points_downward_beats_reachable_upstairs(self):
        mod = load_module()
        text = (
            "You establish the house's practical routes: the stair upward is real and reachable "
            "from the ground floor, and you do identify the downward way as well: a basement "
            "access point on this level, consistent with the colder draft and cellar taint. "
            "What you have now, concretely, is this: the ground floor is navigable, the upstairs "
            "route is found, and the basement route is found--and the strongest physical hint "
            "on this floor points downward. The faint draft and cellar smell make that lower "
            "access feel less like an abstract lead and more like the house quietly breathing "
            "from below."
        )
        state = mod.VisibleState(
            transcript="The ground floor search just succeeded.\n" + text,
            last_reply=text,
            turn=12,
            pc_name='Eleanor "Nell" Ward',
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "safe_basement_descent_and_search",
        )
        self.assertIn("basement", action)
        self.assertNotIn("upstairs", action)

    def test_buried_in_basement_priority_beats_reachable_upper_floor(self):
        mod = load_module()
        text = (
            "The ground-floor search succeeds. You establish both lines: there is a usable "
            "route upstairs from the hall, and there is also a way downward into the basement. "
            "The parlor dust and upper rooms remain available, but if Corbitt was buried in "
            "the basement of his own house, the downward route matters more than any parlor "
            "dust. What you have now is concrete: both upper floor and basement are reachable, "
            "but the next meaningful pressure lies either above or below."
        )
        state = mod.VisibleState(
            transcript="The chapel record said Corbitt was buried in the basement.\n" + text,
            last_reply=text,
            turn=9,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "safe_basement_descent_and_search",
        )
        self.assertIn("basement", action)
        self.assertNotIn("upstairs", action)

    def test_ground_floor_success_with_way_down_and_basement_presence_follows_basement(self):
        mod = load_module()
        text = (
            "[roll]Methodical ground-floor search of the Corbitt House -- 1d100: 56 vs Spot Hidden 70 -- Success.[/roll] "
            "Inside, the ground floor smells exactly wrong for an ordinary shut-up house: old plaster, dust, "
            "and damp held too long. In more than one room, the dust is unevenly disturbed near the floor, "
            "not in the clean pattern of recent footprints, but in faint scrape marks as though something "
            "has been dragged or shifted at some point. A sourer thread of air appears deeper in the house, "
            "intermittent rather than constant: not enough to fix with certainty from the threshold of each "
            "room, but enough to suggest a way downward somewhere on this floor. You confirm both vertical "
            "routes in the house's layout: stairs leading up, and a way down to the basement from the ground "
            "floor. Among the ordinary papers and household remnants, you do find documents worth photographing "
            "rather than pocketing. The floor does begin to tell you its character: stale rooms, old disorder, "
            "and a basement presence you can smell before you fully see it."
        )
        state = mod.VisibleState(
            transcript="The chapel record said Walter Corbitt was buried in the basement of his house.\n" + text,
            last_reply=text,
            turn=9,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "safe_basement_descent_and_search",
        )
        self.assertIn("basement", action)
        self.assertNotIn("upstairs", action)

    def test_lower_cellar_hint_after_ground_floor_beats_reachable_upstairs(self):
        mod = load_module()
        text = (
            "What does stand out, as she continues the sweep, is not a single clean discovery "
            "but the house's internal logic of movement. There is a way up from the ground "
            "floor, plainly enough, and there is also a downward route: a basement access on "
            "this level, distinct from the upstairs line. From more than one point in the "
            "sweep, the lower part of the house announces itself faintly--not by sight, but "
            "by that cooler, shut-in hint below the ordinary dust and damp, the sort of air "
            "that suggests cellar space rather than just neglected rooms."
        )
        state = mod.VisibleState(
            transcript="The ground-floor search mapped the house routes.\n" + text,
            last_reply=text,
            turn=9,
            pc_name="Evelyn Marsh",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "safe_basement_descent_and_search",
        )
        self.assertIn("basement", action)
        self.assertNotIn("upstairs", action)

    def test_clarified_concealed_basement_section_triggers_probe_not_upstairs(self):
        mod = load_module()
        text = (
            "The single most concrete visible affordance established by your last result is "
            "the concealed section itself—the hidden barrier masking a cavity beyond. "
            "Exact location: it is in the basement, near the patch of disturbed earth and the "
            "scrape marks you just examined. From where Evelyn stands now, it is a few steps "
            "in front of her, with the stairs and marked retreat route still behind her. "
            "It reads as part of the cellar structure that is slightly wrong: a section at the "
            "wall-and-floor junction whose resistance does not match the surrounding basement."
        )
        state = mod.VisibleState(
            transcript=(
                "The basement search revealed disturbed earth, scrape marks, and a hidden barrier.\n"
                + text
            ),
            last_reply=text,
            turn=11,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "probe_clarified_concealed_basement_section",
        )
        self.assertIn("concealed", action)
        self.assertIn("basement", action)
        self.assertNotIn("upstairs", action)

    def test_rough_basement_boards_affordance_triggers_probe_not_reclarify(self):
        mod = load_module()
        text = (
            "The single most concrete, presently visible affordance is a section of rough "
            "old boards set against the basement's far wall, where the dark under-space "
            "begins. Exact location: along the basement wall ahead and slightly off to one "
            "side, at the edge of the darker under-space rather than out in the open middle "
            "of the room. What it looks like: aged, rough planking, badly fitted and more "
            "concealing than structural, with darkness pooled around and behind it. Whether "
            "it is safely reachable: reachable, yes; safely, not guaranteed. Visible: rough "
            "boards concealing a darker under-space along the basement wall. Not yet proven: "
            "opening, depth, occupant, or immediate danger."
        )
        state = mod.VisibleState(
            transcript=(
                "The basement search revealed disturbed earth, scrape marks, and a suspicious "
                "concealed section.\n" + text
            ),
            last_reply=text,
            turn=14,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "probe_clarified_concealed_basement_section",
        )
        self.assertIn("boards", action)
        self.assertNotIn("clarify_latest_visible_affordance", str(decision))

    def test_boarded_over_basement_recess_triggers_concealed_probe_not_upstairs(self):
        mod = load_module()
        text = (
            "The most concrete visible affordance from where Evelyn is standing is this. "
            "At the base of the basement wall, a little left of where your flashlight first "
            "caught the scrape marks, there is a boarded-over section that does not match "
            "the rest of the cellar finish. It looks like rough, old planks fastened across "
            "a shallow recess or opening, darker with damp than the surrounding wood and "
            "masonry. The edges are irregular rather than neatly framed. Your earlier "
            "careful probing has already shown that one part answers with a slightly "
            "hollower feel than the surrounding wall. As for reach: yes, it is safely "
            "reachable from your current position. You connect these observations: there "
            "is a visible boarded irregularity in the basement wall; it is close enough to "
            "inspect directly; it may conceal a cavity or altered space. What you still "
            "cannot yet prove is what lies behind it."
        )
        state = mod.VisibleState(
            transcript=(
                "The basement follow-up confirmed a real altered section.\n" + text
            ),
            last_reply=text,
            turn=12,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "probe_clarified_concealed_basement_section",
        )
        self.assertIn("basement", action)
        self.assertIn("board", action)
        self.assertNotIn("upstairs", action)

    def test_loosened_disturbed_basement_wall_triggers_concealed_probe_not_reclarify(self):
        mod = load_module()
        text = (
            "The single most concrete visible affordance is this. A disturbed section low "
            "on the basement wall ahead of you, a little off from the rest of the cellar "
            "line--roughly waist-high to knee-high at its most suspicious point, with the "
            "irregularity strongest near the floor where the dust and scrape marks gather. "
            "In your flashlight beam it does not look like clean, solid cellar masonry. "
            "It looks like a patch that has been altered, covered, or loosened: the surface "
            "sits wrong, with a dry, uneven edge and a bit of material that already shifted "
            "under your probing."
        )
        state = mod.VisibleState(
            transcript=(
                "The basement follow-up made the barrier shift but did not reveal what lies behind it.\n"
                + text
            ),
            last_reply=text,
            turn=12,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "probe_clarified_concealed_basement_section",
        )
        self.assertIn("basement", decision["sent_to_gm"].lower())
        self.assertNotIn("clarify_latest_visible_affordance", str(decision))

    def test_opened_crawl_space_with_chapel_words_triggers_specific_inspection_not_template_probe(self):
        mod = load_module()
        text = (
            "The single most concrete visible affordance established by the last result is "
            "the opened crawl-space entrance behind the loosened basement boards. Its exact "
            "location: in the basement, along the wall you just tested. Rough old boards have "
            "shifted enough to reveal a narrow black opening beyond them, more slit than doorway. "
            "Just inside, where your flashlight catches the inner surface at the right angle, "
            "the carved words are plainly visible. Chapel of Contemplation. Cold damp air is "
            "coming from within. You can stand at the opening, photograph it, inspect the boards, "
            "and study the carving without yet crawling in or committing deeper."
        )
        state = mod.VisibleState(
            transcript="The basement boards opened onto a crawl space.\n" + text,
            last_reply=text,
            turn=13,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "inspect_opened_crawl_space_and_chapel_carving",
        )
        self.assertIn("crawl", action)
        self.assertIn("chapel of contemplation", action)
        self.assertNotIn("disturbed earth", action)
        self.assertNotIn("scrape marks", action)

    def test_cramped_under_space_with_chapel_words_stays_local_not_chapel_trip(self):
        mod = load_module()
        text = (
            "[roll]Careful inspection and probing of the concealed boards — 1d100: 47 vs Spot Hidden 70 — Success.[/roll] "
            "The notebook edge finds it first: not just rough boarding, but a boundary. One section gives a little "
            "differently under controlled pressure. Beyond it is a cramped, dark under-space. Your flashlight beam "
            "slides over dirt, old wood, and the close, wet chill of the cavity before it catches carved lettering "
            "on the inner wall. Chapel of Contemplation. Nothing lunges at you yet, and the opening does not suddenly "
            "move on its own. But the air from inside is colder than the rest of the basement, and the cramped black "
            "beyond the boards looks deep enough to hide more than rubbish."
        )
        state = mod.VisibleState(
            transcript="The basement boards opened a hidden under-space.\n" + text,
            last_reply=text,
            turn=16,
            pc_name="Evelyn Ward",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "bedroom_abandoned",
                "basement_descent",
                "basement_followup",
                "basement_concealed_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "inspect_opened_crawl_space_and_chapel_carving",
        )
        self.assertIn("crawl", action)
        self.assertIn("chapel of contemplation", action)
        self.assertNotIn("drives to the chapel", action)

    def test_failed_crawl_space_entry_with_back_out_option_continues_crawlspace_not_fallback(self):
        mod = load_module()
        text = (
            "You lower yourself only as far as you meant to—shoulders and light inside, hips still "
            "near the lip, one foot already thinking about the retreat. [roll]Cautious crawl-space "
            "inspection — 1d100: 79 vs Spot Hidden 70 — Failure.[/roll] The beam slides over packed "
            "earth, rough timber, damp stone, and a confusion of cobwebs and old grime. You do not "
            "catch any clear remains, tools, ritual marks, or side passage from this limited angle. "
            "What you do get is the feeling that the space continues inward beyond the first body "
            "length, with the darkness flattening detail before your light can make sense of it. "
            "Nothing immediately tries to trap you. You can back out now exactly as planned, or "
            "commit farther in if you want a better look."
        )
        state = mod.VisibleState(
            transcript="The opened crawl-space entrance is in the basement wall.\n" + text,
            last_reply=text,
            turn=13,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "crawl_space_probe",
                "crawl_space_entry",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "probe_deeper_crawl_space_from_marked_exit",
        )
        self.assertNotIn("clarify_latest_visible_affordance", str(decision))

    def test_unstable_crawl_space_strong_failure_withdraws_not_deeper_or_fallback(self):
        mod = load_module()
        text = (
            "[roll]Cautious crawl-space search: 1d100=[100] target:<=65, result:strong_failure[/roll] "
            "你把身体压得很低，只让手电的白光和相机先探进那道湿冷的缝隙，脚下仍稳稳朝着外面的退路留着方向。"
            "行走杖先一步伸出去，杖尖轻轻顶进前方发黑的泥土和粗糙边缘，随即那层看着还算结实的表面忽然塌松下去。"
            "一声闷闷的碎裂响在狭窄空间里被放大。冷湿的土腥气一下翻上来，细灰和碎屑被你自己的动作惊起，"
            "白亮的光顿时被搅成一片乱舞的颗粒。前方原本就局促的视野彻底花掉，你什么也来不及分辨。"
            "更糟的是，杖尖陷进去之后，周围的土层还在缓慢地往下掉，像是在提醒你这地方远没有表面看起来那么稳。"
            "那种会把人卡住的危险感一下变得非常具体。你立刻收回探进去的上半身，把相机和手电一起带出来，"
            "后脚跟重新找回地下室地面的实感，退路仍然在你身后敞着。"
        )
        state = mod.VisibleState(
            transcript="The crawl-space mouth is open in the basement.\n" + text,
            last_reply=text,
            turn=19,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "crawl_space_probe",
                "crawl_space_entry",
            },
        )

        self.assertTrue(mod.unstable_crawl_space_failure_visible(text))
        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "withdraw_from_unstable_crawl_space_after_failed_probe",
        )
        self.assertIn("backs fully out", decision["sent_to_gm"].lower())
        self.assertNotIn("probe_deeper_crawl_space_from_marked_exit", decision_text)
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_crawl_space_entry_no_yield_with_basement_stairs_does_not_trigger_upstairs(self):
        mod = load_module()
        text = (
            "[roll]Careful crawl-space inspection — 1d100: 81 vs Spot Hidden 60 — failure.[/roll]\n\n"
            "You lower yourself just far enough for the flashlight beam and camera to pierce the cramped dark. "
            "The stick goes first, tapping earth, splinter, and old grit. Nothing lunges. Nothing seizes your ankle. "
            "The air inside is colder, damp, and close enough that each breath seems to come back at you off the boards and dirt.\n\n"
            "But from this cautious angle, with most of your weight still committed to retreat, the crawl space yields only fragments: "
            "rough earth, old timber, pockets of cobweb, and darkness swallowing the reach of the light. No obvious remains, "
            "no clear tools, no immediate side passage, and no certain opening in the airflow reveal themselves from where you are.\n\n"
            "What does stand out is the feeling of concealment. This under-space is not merely storage spillover; it feels deliberately shut away.\n\n"
            "Your marked retreat remains clear behind you, the basement stairs still your clean line out. Nothing has trapped you yet."
        )
        state = mod.VisibleState(
            transcript="The opened crawl-space entrance is in the basement wall.\n" + text,
            last_reply=text,
            turn=13,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "crawl_space_probe",
                "crawl_space_entry",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertFalse(mod.upper_route_visible(text))
        self.assertEqual(
            decision["response_contract"]["intent"],
            "probe_deeper_crawl_space_from_marked_exit",
        )
        self.assertIn("crawl-space", action)
        self.assertNotIn("upstairs", action)
        self.assertNotIn("tests each stair", action)

    def test_limited_crawl_space_failure_with_no_new_leads_withdraws_not_deeper(self):
        mod = load_module()
        text = (
            "[roll]Careful crawl-space inspection -- 1d100: 82 vs Spot Hidden 65 -- failure.[/roll] "
            "You get your shoulders and light into the crawl space, hips still close to the lip and the "
            "marked basement stairs behind you. The flashlight catches packed earth, old boards, stale "
            "dust, and a cramped blackness that keeps swallowing the beam. You do not see remains, tools, "
            "a side passage, fresh markings, or a trustworthy airflow from this limited angle. The passage "
            "is tight enough that going farther in would mean committing more of yourself rather than just "
            "looking. Your retreat behind you remains open."
        )
        state = mod.VisibleState(
            transcript="The opened crawl-space entrance is in the basement wall.\n" + text,
            last_reply=text,
            turn=13,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "crawl_space_probe",
                "crawl_space_entry",
            },
        )

        self.assertTrue(mod.limited_crawl_space_no_new_leads_withdrawal_visible(text))
        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "withdraw_from_unresolved_crawl_space_to_marked_stairs",
        )
        self.assertIn("backs out", decision["sent_to_gm"].lower())
        self.assertNotIn("probe_deeper_crawl_space_from_marked_exit", decision_text)
        self.assertNotIn("retry_chapel", decision_text)
        self.assertNotIn("upstairs", decision_text)

    def test_crawl_space_lip_failure_with_clean_retreat_continues_local_space(self):
        mod = load_module()
        text = (
            "The crawl space takes your flashlight beam and gives almost nothing back. "
            "[roll]Careful inspection of the crawl space while probing from the lip — 1d100: 77 vs Spot Hidden 55 — failure.[/roll] "
            "On your belly at the lip, with the walking stick testing ahead, you make out damp earth, old dust, rough timber, "
            "and the cramped continuation of the passage disappearing into black. The beam keeps breaking on shadows and "
            "irregular boards before you can make clean sense of what lies farther in. You do not spot any clear remains, "
            "tools, side passage, ritual layout, or obvious object from this cautious angle. If anything is there, it is "
            "either deeper in, tucked behind the broken geometry of the space, or masked by the cramped sightlines. "
            "Nothing answers your probing with a sudden rush or lunge. The space remains still. You are still braced at "
            "the opening, with the marked stairs behind and a clean retreat."
        )
        state = mod.VisibleState(
            transcript="The opened crawl-space entrance is in the basement wall.\n" + text,
            last_reply=text,
            turn=15,
            pc_name="Evelyn Hart",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "basement_gap_stabilized",
                "crawl_space_probe",
                "crawl_space_entry",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertTrue(mod.current_crawl_space_affordance_visible(text))
        self.assertFalse(mod.upper_route_visible(text))
        self.assertEqual(
            decision["response_contract"]["intent"],
            "probe_deeper_crawl_space_from_marked_exit",
        )
        self.assertIn("crawl", action)
        self.assertNotIn("upstairs", action)

    def test_clarified_crawl_space_mouth_triggers_probe_not_reclarify(self):
        mod = load_module()
        text = (
            "From where you are now, the immediate, usable fact is this: Your light reaches only "
            "the near mouth of the under-space. You can see rough floorboards, close-packed dirt, "
            "and the low black gap continuing ahead beyond the edge of the beam. The crawl space "
            "is directly in front of you; your retreat is directly behind you, back through the "
            "opening you used to lower yourself partway in. Nothing visible is between you and "
            "that exit right now. What remains uncertain is everything past that shallow cone of light."
        )
        state = mod.VisibleState(
            transcript="The opened crawl-space entrance is in the basement wall.\n" + text,
            last_reply=text,
            turn=15,
            pc_name="Evelyn Shaw",
            attempted={
                "hall_records",
                "official_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "crawl_space_probe",
                "crawl_space_entry",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "probe_deeper_crawl_space_from_marked_exit",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("crawl-space", decision["sent_to_gm"].lower())

    def test_clarified_boarded_under_space_near_exit_triggers_probe_not_second_reclarify(self):
        mod = load_module()
        text = (
            "From where you are now, the one concrete thing you can actually act on is this: "
            "The basement opens around you in cold, wet stillness, and the stairs back up remain "
            "behind you as your clean way out. Off to one side of that retreat path--not beyond it, "
            "not deeper past it--you can make out a rough, boarded section and a low dark space "
            "beneath or behind it. It looks less like ordinary storage and more like something "
            "deliberately covered over. What you can truthfully say is that there is a concealed-looking "
            "under-space near your exit route. What you cannot yet prove is what lies beyond those boards, "
            "how far that space runs, whether anything is inside it now, or whether it connects to "
            "anything more important. So the nearest actionable affordance, in plain terms, is: "
            "you can examine the boarded under-space from where you are, or approach it while keeping "
            "the stairs close at hand."
        )
        state = mod.VisibleState(
            transcript="Evelyn had entered the crawl-space one body length and asked for a grounded affordance.\n" + text,
            last_reply=text,
            turn=20,
            pc_name="Evelyn Hart",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "crawl_space_probe",
                "crawl_space_entry",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertTrue(mod.current_crawl_space_affordance_visible(text))
        self.assertEqual(
            decision["response_contract"]["intent"],
            "probe_deeper_crawl_space_from_marked_exit",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_repeated_crawl_space_mouth_after_deeper_probe_withdraws_not_reclarify(self):
        mod = load_module()
        text = (
            "The concrete thing in front of Evelyn is the crawl-space mouth. From where she stands, "
            "the flashlight gives her only the near part of it: rough boards framing a low opening, "
            "packed dirt at its lip, and darkness continuing forward beyond the reach of the beam. "
            "Relative to her retreat path, it is between her and the deeper hidden space, not between "
            "her and the stairs. What she cannot honestly prove from here is what lies beyond the "
            "first stretch of darkness."
        )
        state = mod.VisibleState(
            transcript="Evelyn probed one reach deeper but the crawl space stayed unreadable.\n" + text,
            last_reply=text,
            turn=20,
            pc_name="Evelyn Shaw",
            attempted={
                "hall_records",
                "official_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "crawl_space_probe",
                "crawl_space_entry",
                "crawl_space_deeper_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "withdraw_from_unresolved_crawl_space_to_marked_stairs",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("backs out", decision["sent_to_gm"].lower())

    def test_failed_crawl_space_mouth_probe_no_new_facts_withdraws_not_reclarify(self):
        mod = load_module()
        text = (
            "[roll]Careful inspection of the crawl-space mouth: 1d100 = 89 vs Spot Hidden 60 -- failure.[/roll]\n\n"
            "The camera catches the rough boards, the packed dirt, and the first strip of black beyond the opening. "
            "When you ease the flashlight, camera, and walking stick just past the mouth, the beam wobbles over "
            "cramped earth and splintered wood, but nothing resolves into a new, solid detail before the dark swallows "
            "it again.\n\n"
            "No fresh mark shows itself. No definite object. No visible remains. If air is moving in there, it is too "
            "faint and stale to read from where you are. The crawl-space gives you no clear answer--only that same "
            "close, mean darkness ahead, with the marked stairs still at your back if you want to withdraw cleanly.\n\n"
            "What does Evelyn do next?"
        )
        state = mod.VisibleState(
            transcript="Evelyn probed one reach deeper but the crawl space stayed unreadable.\n" + text,
            last_reply=text,
            turn=23,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "crawl_space_probe",
                "crawl_space_entry",
                "crawl_space_deeper_probe",
            },
        )

        self.assertTrue(mod.repeated_crawl_space_probe_no_new_facts_visible(text))
        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "withdraw_from_unresolved_crawl_space_to_marked_stairs",
        )
        self.assertIn("backs out", decision["sent_to_gm"].lower())
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertNotIn("cautious_upper_floor_search", decision_text)

    def test_successful_crawl_space_watch_with_no_new_facts_withdraws_not_reclarify(self):
        mod = load_module()
        text = (
            "[roll]Careful watch at the crawl-space mouth -- 1d100: 33 vs Spot Hidden 60 -- success.[/roll] "
            "Keeping your weight back and your retreat line clear, you let the light, stick, and lens do "
            "the risking for you. The flashlight beam slides over rough-packed earth and splintered boards, "
            "then farther into the black throat of the crawl space. Nothing lunges. Nothing shifts. The dirt "
            "ahead does not show a fresh turn, widening chamber, discarded object, remains, or a new carved "
            "mark beyond what you had already recorded. The air from within is faintly colder and damp, but "
            "steady rather than gusting. Your walking stick meets earth and wood, not a sudden drop or moving "
            "body. For this careful turn, the space answers with stillness. You remain at the mouth, with the "
            "marked stairs behind you and a clear path to back away."
        )
        state = mod.VisibleState(
            transcript="Evelyn probed one reach deeper but the crawl space gave no new fact.\n" + text,
            last_reply=text,
            turn=26,
            pc_name="Evelyn Mercer",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "crawl_space_probe",
                "crawl_space_entry",
                "crawl_space_deeper_probe",
            },
        )

        self.assertTrue(mod.repeated_crawl_space_probe_no_new_facts_visible(text))
        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "withdraw_from_unresolved_crawl_space_to_marked_stairs",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_failed_deeper_crawl_probe_with_no_branch_withdraws_not_reclarify(self):
        mod = load_module()
        text = (
            "[roll]Probe the deeper crawl space from a braced position -- 1d100: 65 vs Spot Hidden 61 -- Failure.[/roll]\n\n"
            "Still half-braced in the opening, Evelyn keeps the marked gap, the basement stairs, and the handkerchief line "
            "clearly to her rear. That much remains steady and knowable.\n\n"
            "She eases the flashlight beam, camera, and walking stick farther into the dark without committing her body. "
            "The beam skims rough earth and cramped boards, catches tight inner angles, and finds only more close blackness "
            "and cluttered shadow. No new branch shows itself. No chamber opens. No fresh mark separates itself from the grime. "
            "If there is a turn, a cache, or a deeper recess beyond, it does not yield itself to this cautious reach.\n\n"
            "What does answer is the space itself: damp, tight, muffled, with the unpleasant sense that another few inches "
            "forward would mean giving it more of you than you intended. Nothing visibly lunges or shifts, but neither does "
            "the crawl space become any safer or more readable from here.\n\n"
            "You are still poised to back out toward the stairs, or to change methods from this position."
        )
        state = mod.VisibleState(
            transcript="Evelyn is half inside the crawl space and just made one deeper bounded probe.\n" + text,
            last_reply=text,
            turn=24,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "crawl_space_probe",
                "crawl_space_entry",
                "crawl_space_deeper_probe",
            },
        )

        self.assertTrue(mod.repeated_crawl_space_probe_no_new_facts_visible(text))
        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "withdraw_from_unresolved_crawl_space_to_marked_stairs",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("backs out", decision["sent_to_gm"].lower())

    def test_after_crawl_space_withdrawal_leaves_for_tools_not_reclarify(self):
        mod = load_module()
        text = (
            "You ease yourself back from the crawl-space mouth without turning your back on it, flashlight beam "
            "pinned on the dark gap and the walking stick angled defensively in front of you. The retreat to the "
            "basement stairs is slow, careful, and controlled. [roll]Spot Hidden -- checking the crawl-space mouth "
            "for movement or anything following as Evelyn withdraws to the basement stairs: 1d100 = 50 vs 60, success.[/roll] "
            "Nothing lunges after you. Nothing slides out into the light. The opening remains dark and watchful, "
            "but from this safer position you can see that your way back to the stairs is still clear and the exit "
            "is not being blocked. The crawl space remains unresolved, but for the moment it is contained at a "
            "distance rather than pressing directly on you. From the stair position, you have a little breathing "
            "room to reassess."
        )
        state = mod.VisibleState(
            transcript="Evelyn has withdrawn from the crawl-space mouth to the basement stairs.\n" + text,
            last_reply=text,
            turn=14,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "crawl_space_probe",
                "crawl_space_entry",
                "crawl_space_deeper_probe",
                "crawl_space_withdrawal",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "leave_house_for_tools_after_crawl_space_withdrawal",
        )
        self.assertIn("stronger light", decision["sent_to_gm"].lower())
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_crawlspace_continuation_one_word_triggers_deeper_probe_not_reclarify(self):
        mod = load_module()
        text = (
            "From where Evelyn has stopped, this much is concrete: Your flashlight gives you a shallow tunnel "
            "of sight under the house--tight earth, rough timber, dampness, and the stale smell of long-shut wood. "
            "Behind you is the way back out: the opening you used to lower yourself in, leading straight to the "
            "basement proper. Ahead, the under-space continues into deeper darkness. The most immediate actionable "
            "thing is not go farther, but that there is more of the crawlspace beyond the edge of your current light. "
            "What remains uncertain is what exactly lies farther in."
        )
        state = mod.VisibleState(
            transcript="Evelyn is one body length into the crawlspace.\n" + text,
            last_reply=text,
            turn=12,
            pc_name="Evelyn Hart",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "crawl_space_probe",
                "crawl_space_entry",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "probe_deeper_crawl_space_from_marked_exit",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_crawl_space_inner_boundary_success_triggers_boundary_probe_not_reclarify(self):
        mod = load_module()
        text = (
            "[roll]Spot Hidden -- Inspect the wall around the carving for seams, hollows, or a hidden edge: "
            "1d100 = 8 vs 60. Extreme success.[/roll] The carving itself is cut into the crawl-space wall, "
            "not an opening and not proof of a chamber beyond. But beside and slightly below the lettering, "
            "the wall does not read as solid earth all the way through. One section gives itself away by small "
            "inconsistencies: a straighter line than surrounding dirt should make, a change in texture where "
            "packed earth meets something more deliberate, and the faint suggestion of a concealed inner boundary "
            "behind the rough surface. You do not see bones, loose objects, or a ritual cache exposed."
        )
        state = mod.VisibleState(
            transcript="Evelyn inspected the crawl-space wall around the Chapel carving.\n" + text,
            last_reply=text,
            turn=22,
            pc_name="Evelyn Hart",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "crawl_space_probe",
                "crawl_space_entry",
                "crawl_space_deeper_probe",
                "crawl_space_wall_inspection",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "probe_crawl_space_inner_boundary_from_braced_position",
        )
        self.assertIn("inner boundary", decision_text)
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_outside_after_crawl_exit_returns_with_tools_not_reclarify(self):
        mod = load_module()
        text = (
            "Keeping the flashlight trained as long as she can, Evelyn backs up the basement stairs one careful "
            "step at a time. Nothing darts from the opening. Nothing scrapes after her in the dark. She retraces "
            "her route out of the basement, out through the house, and back into the open air with her photographs "
            "and notes still together in hand. You are out of the house. A little time passes as you complete the withdrawal. "
            "From where Evelyn is now, the house itself is in front of her; the entrance remains the nearest way back "
            "toward the interior she withdrew from and whatever lies below."
        )
        state = mod.VisibleState(
            transcript="Evelyn left the house after deciding to return only with tools.\n" + text,
            last_reply=text,
            turn=25,
            pc_name="Evelyn Hart",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "crawl_space_probe",
                "crawl_space_entry",
                "crawl_space_deeper_probe",
                "crawl_space_withdrawal",
                "basement_tools_return",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "return_with_tools_to_documented_crawl_space_route",
        )
        self.assertIn("rope", decision["sent_to_gm"].lower())
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_returned_with_tools_at_crawl_route_uses_tools_not_reclarify(self):
        mod = load_module()
        text = (
            "Daylight washes some of the menace out of the street, but not out of the house. "
            "You spend the time making your preparations and return with better light, gloves, a marked line, "
            "and a tool fit for levering damaged boards. No obvious trustworthy helper falls into place quickly enough. "
            "The Corbitt House stands where you left it. The key still fits. The entry route appears unchanged. "
            "Inside, the basement stairs remain where and how you left them. The crawl-space approach is still there as before, "
            "waiting under the house. Your exterior line of retreat can be kept clear. You are back at the point where you can "
            "begin the renewed descent under better conditions. What does Evelyn do first--check the crawl-space mouth closely "
            "before entering, rig the line and light, start prying at the broken boards, or go straight in?"
        )
        state = mod.VisibleState(
            transcript="Evelyn returned with light, line, gloves, and tools.\n" + text,
            last_reply=text,
            turn=29,
            pc_name="Evelyn Hart",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "crawl_space_probe",
                "crawl_space_entry",
                "crawl_space_deeper_probe",
                "crawl_space_withdrawal",
                "basement_tools_return",
                "returned_with_tools",
            },
        )

        self.assertTrue(mod.prepared_return_crawl_space_ready_visible(text))
        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "use_prepared_tools_on_documented_crawl_space_route",
        )
        self.assertIn("marked line", decision["sent_to_gm"].lower())
        self.assertIn("tool", decision["sent_to_gm"].lower())
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_outside_after_unstable_crawl_withdrawal_prepares_tools_not_deeper_probe(self):
        mod = load_module()
        text = (
            "You back out without pressing your luck. Keeping the flashlight fixed on the gap and the stick "
            "between yourself and the opening, you withdraw to the basement stairs, then retrace your marked "
            "path up and out of Corbitt House. Nothing lunges from the crawl-space, and the retreat line holds "
            "long enough for you to leave cleanly. Outside, with the house behind you and the night air less "
            "close than the basement's damp chill, you've established a few hard facts for yourself: the "
            "crawl-space mouth is unsafe, the space beyond it is unstable enough that probing it blind is a bad "
            "gamble, and if you mean to go back in, you'll want better light, proper tools, a line, and ideally "
            "another pair of eyes. Time passes: about 3 minutes."
        )
        state = mod.VisibleState(
            transcript="Nell withdrew from the unstable crawl space and left the house.\n" + text,
            last_reply=text,
            turn=12,
            pc_name='Eleanor "Nell" Ward',
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "crawl_space_probe",
                "crawl_space_entry",
                "crawl_space_withdrawal",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "leave_house_for_tools_after_crawl_space_withdrawal",
        )
        self.assertNotIn("probe_deeper_crawl_space_from_marked_exit", decision_text)
        self.assertIn("tool", decision["sent_to_gm"].lower())
        self.assertIn("outside", decision["sent_to_gm"].lower())

    def test_active_self_moving_knife_after_defense_retreats_not_reclarify(self):
        mod = load_module()
        text = (
            "[roll]Sanity roll at the sight of the self-moving knife: 1d100 = 71 vs target 65 -- failure.[/roll] "
            "The sight of it hits you like a physical wrongness: a knife moving with no hand on it, with purpose. "
            "[roll]INT roll to comprehend the horror and resist temporary insanity: 1d100 = 36 vs target 11 -- failure.[/roll] "
            "The knife is ahead of you. It is between you and the deeper interior, not on the retreat path behind you. "
            "The blade is moving on its own. It is the nearest immediate threat. Retreat is still physically possible "
            "from your present position, and you have a clear way to fall back toward the marked exit."
        )
        state = mod.VisibleState(
            transcript="Evelyn already defended once against the self-moving knife.\n" + text,
            last_reply=text,
            turn=12,
            pc_name="Evelyn Hart",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "knife_defense",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "retreat_from_active_blade_threat_to_marked_exit",
        )
        self.assertIn("marked path", decision["sent_to_gm"].lower())
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_stair_side_crawl_space_restatement_after_withdrawal_leaves_for_tools(self):
        mod = load_module()
        text = (
            "From the stair-side position, the one concrete thing in front of you is the opening beyond the broken boards. "
            "It sits ahead of you across the basement floor, low to the ground and black enough that it reads less like "
            "a doorway and more like a cramped gap where the house seems to give way to a concealed space. To leave, "
            "your retreat is simple and clean: the cellar stairs are behind you, and nothing visible stands between you "
            "and them. The boards are broken open; there is a crawl-space mouth there; the air around that part of the "
            "basement is colder and damper. So the actionable thing is either examining that opening more closely or "
            "abandoning it and taking the stairs back out."
        )
        state = mod.VisibleState(
            transcript="Evelyn already withdrew from the crawl-space mouth.\n" + text,
            last_reply=text,
            turn=15,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "crawl_space_probe",
                "crawl_space_entry",
                "crawl_space_deeper_probe",
                "crawl_space_withdrawal",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "leave_house_for_tools_after_crawl_space_withdrawal",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertNotIn("probe_deeper_crawl_space_from_marked_exit", decision_text)

    def test_basement_door_reset_after_crawlspace_reanchors_not_fallback(self):
        mod = load_module()
        text = (
            "The single most concrete visible affordance is the basement door. It is across the "
            "ground-floor hall, under the staircase. What Evelyn still cannot prove is what lies "
            "beyond it, whether it is locked, and whether the air near it is colder or wetter until "
            "she closes the distance."
        )
        state = mod.VisibleState(
            transcript=(
                "Evelyn had already reached the basement, opened a concealed crawl-space entrance, "
                "and lowered herself one body length into the crawl space.\n"
                + text
            ),
            last_reply=text,
            turn=14,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "crawl_space_probe",
                "crawl_space_entry",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "reject_basement_door_state_reset_after_crawlspace",
        )
        self.assertIn("crawl space", action)
        self.assertNotIn("clarify_latest_visible_affordance", str(decision))

    def test_after_crawl_space_probe_continues_local_space_not_chapel_jump(self):
        mod = load_module()
        text = (
            "The flashlight beam cuts low across the lip of the opening, catching damp earth, "
            "splintered wood, and the rough inner face of the crawl space. The carved words are "
            "there exactly as you copied them: Chapel of Contemplation. From where you remain at "
            "the entrance, without committing your weight inside, you can make out only the near "
            "stretch of the cramped passage. The darkness beyond the first reach of your light "
            "swallows depth quickly. No obvious movement answers your probing. Nothing lunges at "
            "the opening. Nothing immediately blocks your retreat to the stairs. What you do have, "
            "clearly, is a newly exposed, deliberately concealed space under the basement."
        )
        state = mod.VisibleState(
            transcript="The crawl space under the basement is open and marked.\n" + text,
            last_reply=text,
            turn=14,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "moving_bedroom_threat",
                "bedroom_abandoned",
                "basement_descent",
                "basement_followup",
                "crawl_space_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "enter_crawl_space_one_body_length_from_marked_exit",
        )
        self.assertIn("crawl", action)
        self.assertNotIn("michael thomas", action)
        self.assertNotIn("1912", action)
        self.assertNotIn("drives to the chapel", action)

    def test_after_crawl_space_entry_continues_deeper_local_space_not_chapel_jump(self):
        mod = load_module()
        text = (
            "The beam of Evelyn's flashlight slides low across damp earth and rough timber as "
            "she lowers herself only a body length into the opening, one leg still braced for "
            "retreat. No immediate lunge answers it. No sudden collapse. The crawl space "
            "continues inward beyond the first reach of her light, cramped and black between "
            "the foundations. Rough boards and stored debris hem the place in, making it feel "
            "less like overflow storage than something intentionally shut away. As she angles "
            "the light farther, the inner wall and the darkness beyond it suggest there is "
            "more under here than the lip first revealed--but from this cautious position, "
            "with only her upper body committed, the details remain half-swallowed by shadow "
            "and angle. Nothing seizes her. Nothing yet moves to trap her. She remains half "
            "inside the crawl space, with the marked opening and basement stairs still behind "
            "her, retreat line intact."
        )
        state = mod.VisibleState(
            transcript=(
                "The crawl-space entrance showed the words Chapel of Contemplation.\n" + text
            ),
            last_reply=text,
            turn=15,
            pc_name="Evelyn Mercer",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "moving_bedroom_threat",
                "bedroom_abandoned",
                "basement_descent",
                "basement_followup",
                "crawl_space_probe",
                "crawl_space_entry",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "probe_deeper_crawl_space_from_marked_exit",
        )
        self.assertIn("crawl", action)
        self.assertIn("backs out", action)
        self.assertNotIn("drives to the chapel", action)
        self.assertNotIn("chapel of contemplation", action)

    def test_repeated_crawl_space_carving_half_out_does_not_trigger_upper_floor(self):
        mod = load_module()
        text = (
            "You ease yourself down only partway, keeping your legs braced toward the opening and the "
            "basement stairs fixed in memory behind you. The flashlight beam skims over packed earth, "
            "splintered wood, and a low, stale pocket of darkness that seems to swallow the light rather "
            "than reflect it. Your walking stick finds no immediate drop, no sudden give, no waiting jaws. "
            "[roll]Careful search of the crawl space: 1d100 = 22 vs Spot Hidden 70 -- Hard success.[/roll] "
            "With the beam held low and steady, you catch something cut into the crawl-space wall rather "
            "than left there: letters carved into the surface, old but still legible. Chapel of Contemplation. "
            "Nothing in the space immediately shifts or surges at you, but the words make the cramped hollow "
            "feel less accidental--less like storage, more like something made to be found by the wrong person, "
            "or the right one. You are still half out of the opening, with your retreat line clear."
        )
        state = mod.VisibleState(
            transcript="The crawl-space entrance and Chapel carving were already recorded.\n" + text,
            last_reply=text,
            turn=14,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "crawl_space_probe",
                "crawl_space_entry",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertTrue(mod.current_crawl_space_affordance_visible(text))
        self.assertFalse(mod.upper_route_visible(text))
        self.assertEqual(
            decision["response_contract"]["intent"],
            "probe_deeper_crawl_space_from_marked_exit",
        )
        self.assertIn("crawl", action)
        self.assertIn("already recorded", action)
        self.assertNotIn("additional mark", action)
        self.assertNotIn("upstairs", action)
        self.assertNotIn("tests each stair", action)

    def test_crawl_space_success_at_lip_with_continuation_does_not_trigger_upper_floor(self):
        mod = load_module()
        text = (
            "[roll]Cautious crawl-space inspection -- 1d100: 69 vs Spot Hidden 70 -- Success.[/roll] "
            "The beam of your flashlight pushes a narrow tunnel out of the wet dark. The air under the "
            "boards is colder still, with that stale, mineral damp that seems to have been trapped there "
            "for years. Your walking stick finds earth, splintered wood, and then empty space enough to "
            "keep going a little farther in. Nothing rushes you. Nothing grabs the stick. The crawl space "
            "does not immediately shift or answer with movement. But on the wall ahead--low, half-lost "
            "under grime and age--your light catches deliberate carving in the surface. Not random scratches. "
            "Words. Chapel of Contemplation. They have been cut into the crawl-space wall by human hands. "
            "From this angle you can also tell the under-space continues beyond the first body length of "
            "darkness rather than ending immediately in foundation stone. It feels less like simple dead "
            "storage and more like something made to conceal. You are still at the lip, with the marked "
            "stairs and retreat line behind you, and nothing has yet forced your withdrawal."
        )
        state = mod.VisibleState(
            transcript="The crawl-space entrance and Chapel carving were already recorded.\n" + text,
            last_reply=text,
            turn=12,
            pc_name='Eleanor "Nell" Price',
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "crawl_space_probe",
                "crawl_space_entry",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertTrue(mod.current_crawl_space_affordance_visible(text))
        self.assertEqual(
            decision["response_contract"]["intent"],
            "probe_deeper_crawl_space_from_marked_exit",
        )
        self.assertIn("crawl", action)
        self.assertIn("already recorded", action)
        self.assertNotIn("additional mark", action)
        self.assertNotIn("upstairs", action)
        self.assertNotIn("tests each stair", action)

    def test_crawl_space_failure_with_chapel_carving_does_not_trigger_chapel_retry(self):
        mod = load_module()
        text = (
            "[roll]Spot Hidden -- Inspect the wall around the carving for seams, hollows, or hidden structure: "
            "1d100 = 98 vs 55 -- failure.[/roll] You keep your body angled to the crawl-space wall, "
            "the marked stairs behind you, and work the scene like evidence rather than invitation. "
            "The words are real enough in the flashlight beam: Chapel of Contemplation, carved into "
            "the wall itself. You test only what is immediately around the carving: timber, packed earth, "
            "any shift in texture, any seam that might outline a cavity, any draft or hollow note. "
            "What you find is frustratingly little. No clear chamber edge. No object glint. No bones or "
            "remains. No second mark. No trustworthy airflow. If there is a continuation here, it does "
            "not yield itself to a careful surface read."
        )
        state = mod.VisibleState(
            transcript="Evelyn previously searched the Chapel, then found this local crawl-space carving.\n" + text,
            last_reply=text,
            turn=14,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "crawl_space_probe",
                "crawl_space_entry",
                "crawl_space_deeper_probe",
                "crawl_space_wall_inspection",
            },
        )

        self.assertFalse(mod.chapel_search_failure_visible(text))

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertNotEqual(
            decision["response_contract"]["intent"],
            "retry_chapel_with_distinct_visual_method_after_failure",
        )
        self.assertNotIn("pulpit", decision_text)
        self.assertNotIn("drives to the chapel", decision_text)

    def test_prior_chapel_visit_does_not_turn_basement_chapel_carving_into_chapel_arrival(self):
        mod = load_module()
        text = (
            "[roll]Careful probe of the crawl space — 1d100: 25 vs Spot Hidden 50. Success, Hard success.[/roll] "
            "Keeping your body back and only your tools extended, you let the walking stick feel ahead. "
            "The crawl space answers first with damp earth, old wood, and a wall surface less rough "
            "than the surrounding dirt and foundation. As your flashlight beam steadies, cut marks "
            "resolve out of the grime. There are words carved into the crawl-space wall. Chapel of "
            "Contemplation. Nothing immediately lunges, shifts, or cuts off your retreat to the "
            "basement stairs. The air ahead remains colder and wetter, with that same shut-in heaviness."
        )
        state = mod.VisibleState(
            transcript=(
                "Evelyn already visited the ruined chapel earlier, then returned to the Corbitt House basement.\n"
                + text
            ),
            last_reply=text,
            turn=20,
            pc_name="Evelyn Hart",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "second_chapel_method",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "bedroom_abandoned",
                "basement_descent",
                "basement_followup",
                "resumed_basement_after_bedroom",
                "crawl_space_probe",
                "crawl_space_entry",
                "crawl_space_deeper_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertNotEqual(
            decision["response_contract"]["intent"],
            "continue_chapel_search_after_scene_establishment",
        )
        self.assertNotIn("treats the last reply as arrival at the chapel", decision_text)
        self.assertIn("crawl", decision["sent_to_gm"].lower())

    def test_deeper_recess_table_papers_after_breach_trigger_bounded_inspection(self):
        mod = load_module()
        text = (
            "From where Evelyn has stopped, this is what is concrete: You are at the mouth of the broken opening, "
            "not fully committed to the deeper space yet. Behind you is the way you came in: back through the gap, "
            "across the basement, and from there toward the house exit. Ahead of you, just beyond the broken wall, "
            "the space turns meaner and closer. The air is worse there—stale, damp, and touched by a sweet, corrupt "
            "smell that is stronger than the ordinary cellar rot. In that darker pocket, the clearest actionable "
            "thing is a small table set off in the corner with papers on it. They are not in your hand's reach from "
            "where you are now, but they are the nearest distinct object that looks like it might yield information "
            "if you choose to edge in far enough to inspect or take them."
        )
        state = mod.VisibleState(
            transcript="The crawl-space opening led to a deeper broken wall and foul pocket.\n" + text,
            last_reply=text,
            turn=18,
            pc_name="Evelyn Ward",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "bedroom_abandoned",
                "basement_descent",
                "basement_followup",
                "basement_concealed_probe",
                "crawl_space_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "inspect_deeper_recess_table_papers_from_breach",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("papers", decision["sent_to_gm"].lower())
        self.assertIn("retreat", decision["sent_to_gm"].lower())

    def test_reachable_broken_cellar_opening_triggers_probe_not_reclarify(self):
        mod = load_module()
        text = (
            "From where Evelyn has stopped, the one concrete thing she can actually work with is the broken "
            "opening in the cellar wall ahead of her. She can see the opening itself: rough, broken masonry, "
            "darkness beyond it, and the sense that the air past it is fouler than the cellar air on her side. "
            "It is a little in front of her, not under her hands yet, but close enough that one cautious advance "
            "would bring her right up to it. Her retreat path is still behind her. What remains uncertain is "
            "everything past that threshold."
        )
        state = mod.VisibleState(
            transcript="Evelyn is at the basement breach after probing the concealed boards.\n" + text,
            last_reply=text,
            turn=21,
            pc_name="Evelyn Ward",
            attempted={
                "hall_records",
                "official_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "basement_concealed_probe",
                "crawl_space_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "probe_broken_cellar_opening_from_retreat_line",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("opening", decision["sent_to_gm"].lower())

    def test_stubborn_boarded_basement_after_failed_probe_changes_strategy_not_reclarify(self):
        mod = load_module()
        text = (
            "From where Evelyn stands, the one concrete thing she can honestly act on is this: The basement ahead "
            "stops looking like ordinary storage and starts looking deliberately shut off. A section of wall is "
            "crowded by rough boards and old bins, as if that part of the room was meant to be covered rather than "
            "merely used. It is in front of you and a little off to one side, not directly across the line back to "
            "the stairs. So your retreat path is still open. What you can actually perceive is the feeling of "
            "concealment made physical: damp cold, clutter, boards, and a part of the basement that seems less "
            "accidental than the rest. What you cannot yet prove is what those boards are hiding, how deep that "
            "concealed space goes, whether anything is moving beyond it, or whether opening it is merely a matter "
            "of force or proper tools."
        )
        state = mod.VisibleState(
            transcript=(
                "Evelyn already tried controlled pressure on the concealed boards and failed to open them.\n"
                + text
            ),
            last_reply=text,
            turn=16,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "basement_concealed_probe",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "withdraw_from_stubborn_concealed_basement_boards_for_tools",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("tools", decision["sent_to_gm"].lower())
        self.assertIn("retreat", decision["sent_to_gm"].lower())

    def test_documented_basement_withdrawal_leaves_for_tools_not_reclarify(self):
        mod = load_module()
        text = (
            "You back up exactly the way you planned: stick out in front, eyes on the boarded section, "
            "careful feet finding the marks you made. Nothing lunges from the dark. Nothing slides across "
            "the floor behind you. The basement's clammy chill follows you to the stairs, but the retreat "
            "itself stays clear. On the way out, you get the photographs you wanted: the rough boards, "
            "the old bins, the disturbed floor marks, and their relation to the stair line. Your chalk mark "
            "and notebook note give you a clean reference point for a safer return. A few moments later "
            "you are back on the ground floor, with the house's stale stillness around you and the basement "
            "below left undisturbed for now. What do you do next?"
        )
        state = mod.VisibleState(
            transcript=(
                "Evelyn already withdrew from stubborn concealed basement boards for tools.\n"
                + text
            ),
            last_reply=text,
            turn=26,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "basement_concealed_probe",
                "stubborn_basement_boards_withdrawal",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "leave_house_for_tools_after_documenting_stubborn_basement_boards",
        )
        self.assertIn("tool", decision["sent_to_gm"].lower())
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_documented_basement_return_access_gets_tools_not_reclarify(self):
        mod = load_module()
        text = (
            "From where Evelyn stands now, the concrete thing is not a hidden chamber or a proven discovery. "
            "It is the way back down. What she can actually perceive on the ground floor is the open route "
            "to the basement she just came from: the stair leading down into the colder, damper part of the "
            "house. Plain: something in the basement drew her attention strongly enough to photograph, mark, "
            "and withdraw from. She has an identified point to revisit, but not a confirmed explanation. "
            "The actionable thing is the basement access leading back to the suspicious area she already "
            "identified; the retreat path remains open."
        )
        state = mod.VisibleState(
            transcript=(
                "Evelyn documented rough boards and withdrew from the stubborn concealed basement section.\n"
                + text
            ),
            last_reply=text,
            turn=27,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "basement_concealed_probe",
                "stubborn_basement_boards_withdrawal",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "leave_house_for_tools_after_documenting_stubborn_basement_boards",
        )
        self.assertIn("marked basement section", decision["sent_to_gm"].lower())
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_failed_tool_assisted_basement_board_test_exits_not_reclarify(self):
        mod = load_module()
        text = (
            "By daylight, the house feels less uncanny, but no less dead. "
            "[roll]Find a willing helper or witness for a daylight return — 1d100: 17 vs target 60 — Hard success.[/roll] "
            "With pry tool, gloves, camera, notebook, and your retreat route kept open, you return to the marked basement section. "
            "At the seam, you photograph the board again, set the tool carefully into the loosest edge you identified before, "
            "and test it with controlled pressure rather than force. "
            "[roll]Controlled tool-assisted test of the marked basement seam — 1d100: 92 vs target 70 — Failure.[/roll] "
            "The tool catches, then slips with a dull wooden complaint. The boarded under-space along the basement wall "
            "is still there, off to one side rather than across the stairs. You cannot prove what lies behind those boards."
        )
        state = mod.VisibleState(
            transcript="Evelyn documented the stubborn basement boards, left for tools, and returned with a witness.\n" + text,
            last_reply=text,
            turn=18,
            pc_name="Evelyn March",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "basement_concealed_probe",
                "stubborn_basement_boards_withdrawal",
                "basement_tools_return",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "exit_after_failed_tool_assisted_basement_board_test",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("witness", decision["sent_to_gm"].lower())

    def test_structural_irregularity_basement_success_triggers_followup(self):
        mod = load_module()
        text = (
            "[roll]Cautious basement descent and search for hidden threats or signs: "
            "1d100 = 1 vs target 65 -- critical success.[/roll] "
            "The cellar air is cooler and wetter than the rooms above. "
            "You connect these observations: no immediate moving attacker in the stairwell "
            "or at the foot of the steps; patches of disturbed dust and grime that make "
            "this place feel used rather than merely forgotten; and, more importantly, "
            "a structural irregularity below--part of the basement does not read like "
            "ordinary foundation and storage space. Before you fully commit your weight "
            "into the room, you catch it: a section deeper in the cellar where the surfaces "
            "and spacing seem wrong for the rest of the basement, as though something there "
            "was closed off, altered, or concealed after the house was built. It is not "
            "just a vague hunch; from this angle, with this light, it stands out as the "
            "single strongest lead in the basement."
        )
        state = mod.VisibleState(
            transcript=(
                "The upstairs bedroom was marked unsafe. Evelyn has now entered the basement.\n"
                + text
            ),
            last_reply=text,
            turn=14,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "bedroom_threshold_probe",
                "moving_bedroom_threat",
                "bedroom_abandoned",
                "basement_descent",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "follow_basement_disturbed_earth_and_concealed_section",
        )
        self.assertIn("basement", decision["sent_to_gm"].lower())
        self.assertNotIn("clarify_latest_visible_affordance", str(decision))

    def test_successful_basement_disturbance_worth_following_does_not_jump_upstairs(self):
        mod = load_module()
        text = (
            "[roll]Cautious descent into the basement while searching for hidden signs, "
            "disturbances, and immediate threats — 1d100: 38 vs Spot Hidden 65 — success.[/roll]. "
            "Your flashlight finds rough cellar walls, old masonry, patches of earth, and the "
            "cluttered neglect of a place long left to itself. Because you are moving slowly enough "
            "to actually read the place, a few things stand out. The basement air is cooler and "
            "distinctly damper than the rooms above. There are drafts here, faint but real, enough "
            "to suggest that the cellar is not as sealed as it first appears. Parts of the masonry "
            "and ground look naturally old and dirty, but not every surface has been left in exactly "
            "the same condition. There are signs of disturbance worth following more closely, not yet "
            "a full explanation, but enough to say this is not just an untouched junk cellar. "
            "You are now down in the basement proper, with the stairs and your marked exit still "
            "behind you, and at least one area below that deserves closer inspection."
        )
        state = mod.VisibleState(
            transcript="Evelyn is in the basement after a successful cautious descent.\n" + text,
            last_reply=text,
            turn=8,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertTrue(mod.basement_actionable_lead_visible(text))
        self.assertEqual(
            decision["response_contract"]["intent"],
            "follow_basement_disturbed_earth_and_concealed_section",
        )
        self.assertIn("basement", action)
        self.assertNotIn("upstairs", action)

    def test_successful_basement_low_stairs_draft_and_scuffing_continue_search_not_upstairs(self):
        mod = load_module()
        text = (
            "[roll]Cautious descent into the basement while searching for signs of danger or "
            "concealment: 1d100 = 43 vs Spot Hidden 70 -- success.[/roll]\n\n"
            "Because you move with such care, you catch more than a reckless descent would have allowed. "
            "The air below is colder and wetter, and the draft is clearer here now--subtle, but not "
            "imaginary. It is not coming only from the stair behind you. Somewhere in the basement, air "
            "is finding another path.\n\n"
            "By the time you reach the lower stretch of steps, your beam begins to pick out details in "
            "the dim: dirt that does not lie perfectly even, scuffing where something has disturbed the "
            "floor before, and rough cellar surfaces that deserve closer inspection. Nothing immediately "
            "lunges from the dark. No body lies plain in the beam. No obvious ritual object presents "
            "itself at once from the stairs alone.\n\n"
            "But this much is clear: the basement is not a dead, featureless storage hole. There are "
            "signs here worth following, and the draft suggests some further irregularity below--an "
            "opening, a gap, or some hidden way not explained by the stair alone.\n\n"
            "You are now low on the basement stairs with the open route back still marked behind you. "
            "From here, you can continue the close search into the cellar proper."
        )
        state = mod.VisibleState(
            transcript="Evelyn descended to the lower stretch of the basement stairs.\n" + text,
            last_reply=text,
            turn=8,
            pc_name="Evelyn Marsh",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()
        action = decision["sent_to_gm"].lower()

        self.assertTrue(mod.basement_actionable_lead_visible(text))
        self.assertEqual(
            decision["response_contract"]["intent"],
            "follow_basement_disturbed_earth_and_concealed_section",
        )
        self.assertIn("basement", action)
        self.assertIn("stair", action)
        self.assertNotIn("cautious_upper_floor_search", decision_text)
        self.assertNotIn("upstairs", action)

    def test_scrape_traces_and_dust_difference_after_basement_success_triggers_followup(self):
        mod = load_module()
        text = (
            "[roll]Cautious basement descent and initial threat scan: 1d100 = 18 vs target 65. "
            "Success -- Hard.[/roll] The basement air is colder, close, and earth-heavy. "
            "Along parts of the wall and floor, the neglect is uneven. Here and there the "
            "surfaces show interruption rather than simple decay: scrape traces, patches "
            "where dust lies differently, and an impression that some section below deserves "
            "closer attention than the rest. Evelyn has reached the foot of the stairs with "
            "the wedged door and marked return still behind her."
        )
        state = mod.VisibleState(
            transcript="Evelyn has descended into the basement.\n" + text,
            last_reply=text,
            turn=11,
            pc_name="Evelyn Marsh",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "follow_basement_disturbed_earth_and_concealed_section",
        )
        self.assertIn("basement", decision["sent_to_gm"].lower())
        self.assertIn("surface marks", decision["sent_to_gm"].lower())
        self.assertNotIn("clarify_latest_visible_affordance", str(decision))

    def test_mixed_listen_failure_and_basement_search_success_triggers_followup(self):
        mod = load_module()
        text = (
            "[roll]Listen at the basement stair for movement or threat — 1d100: 82 vs 45 — failure.[/roll]\n"
            "At the head of the stairs, you pause and listen. Nothing resolves cleanly enough to trust.\n"
            "[roll]Careful search of the basement approach for hidden features or signs of disturbance — "
            "1d100: 19 vs 60 — hard success.[/roll]\n"
            "You angle the flashlight down and descend carefully, testing each board before you trust it. "
            "A few steps down, the beam picks out small but telling details: scuffed marks on the stair "
            "treads where something heavy has been moved more than once, dust disturbed in uneven streaks "
            "rather than a simple blanket of neglect, and along the lower masonry a faint suggestion of "
            "airflow that does not match the rest of the stale house. From here, the cellar opens ahead "
            "into shadow and damp, your light catching rough walls, old grime, and the suggestion of earth "
            "and stone beyond the last few steps. Whatever more is down there, you are now close enough to "
            "search the basement proper."
        )
        state = mod.VisibleState(
            transcript="Evelyn abandoned the bedroom and descended toward the basement.\n" + text,
            last_reply=text,
            turn=13,
            pc_name="Evelyn Harper",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "bedroom_abandoned",
                "basement_descent",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "follow_basement_disturbed_earth_and_concealed_section",
        )
        self.assertIn("surface marks", decision["sent_to_gm"].lower())
        self.assertNotIn("resume_mapped_basement_route_after_bedroom_retreat", decision_text)
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_basement_proper_with_rough_boards_after_resume_triggers_board_probe(self):
        mod = load_module()
        text = (
            "The marked handkerchief gives you a pale point of reference behind you as you go down. "
            "At the bottom, the beam of your flashlight settles across the basement proper: rough boards, "
            "old bins, patches of darkness under the structure, and a clammy stillness that makes the room "
            "feel less like storage than concealment. For the moment, nothing lunges, no stair gives way, "
            "and the route behind you remains open. You are in the basement now, standing just off the foot "
            "of the stairs with your light and walking stick ready. What does Evelyn examine first?"
        )
        state = mod.VisibleState(
            transcript="Evelyn resumed the basement route after abandoning the unsafe bedroom.\n" + text,
            last_reply=text,
            turn=14,
            pc_name="Evelyn Harper",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "bedroom_abandoned",
                "basement_descent",
                "resumed_basement_after_bedroom",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "approach_clarified_basement_boards_from_stairs",
        )
        self.assertIn("boards", decision["sent_to_gm"].lower())
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_dark_underspace_without_chapel_carving_does_not_invent_open_crawl_space(self):
        mod = load_module()
        text = (
            "[roll]Cautious descent into the basement -- 1d100: 7 vs DEX 70 -- Extreme success.[/roll]\n"
            "The way down has not shifted. Evelyn tests each stair before trusting it, and the old wood "
            "answers with only a tired creak, not a break. Her handkerchief mark gives her a clean point "
            "of reference at the door above, and the flashlight beam reaches the bottom without catching "
            "movement on the steps themselves. She reaches the basement floor safely. The air down here is "
            "colder and wetter than the rooms above. Rough boards, old bins, and the dark under-space beyond "
            "them make the cellar feel less like storage than something meant to be hidden. Nothing immediately "
            "rushes her. Nothing blocks the stairs behind her. Your light now plays over the basement proper "
            "and the rougher shadows beyond."
        )
        state = mod.VisibleState(
            transcript=(
                "Evelyn knows the Chapel of Contemplation from earlier research, but the latest basement reply "
                "has not exposed any Chapel carving.\n"
                + text
            ),
            last_reply=text,
            turn=15,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "bedroom_abandoned",
                "basement_descent",
                "resumed_basement_after_bedroom",
            },
        )

        self.assertFalse(mod.opened_basement_crawl_space_visible(text))

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "approach_clarified_basement_boards_from_stairs",
        )
        self.assertNotIn("chapel of contemplation", decision["sent_to_gm"].lower())
        self.assertNotIn("carved words", decision_text)
        self.assertNotIn("opened basement crawl-space entrance", decision_text)

    def test_loosened_basement_gap_with_skittering_is_handled_before_upstairs(self):
        mod = load_module()
        text = (
            "[roll]Careful probing of disturbed earth and concealed section in the basement: "
            "1d100 = 91 vs Spot Hidden 50 — failure.[/roll]\n"
            "The suspicious section does react, but not by neatly yielding a hidden object. "
            "A partial collapse of old concealment follows: rough board, packed dirt, and stale "
            "debris slough inward in a muffled spill. A pocket of colder, fouler air breathes "
            "out from behind it. You do not get a clear sight yet of a body or ritual object. "
            "What you do get is confirmation that there really is hidden space here. From within "
            "the newly loosened gap, you hear scratching, skittering movement deeper inside the "
            "space. Your retreat path to the marked stairs is still open behind you."
        )
        state = mod.VisibleState(
            transcript=(
                "The basement search revealed disturbed earth, scrape marks, and a suspicious "
                "concealed section.\n" + text
            ),
            last_reply=text,
            turn=9,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "stabilize_loosened_basement_gap",
        )
        self.assertIn("basement", action)
        self.assertIn("gap", action)
        self.assertNotIn("upstairs", action)

    def test_failed_board_probe_with_space_and_skitter_stabilizes_not_reclarify(self):
        mod = load_module()
        text = (
            "[roll]Carefully probe and inspect the rough basement boards -- 1d100: 84 vs 61 (Spot Hidden) -- failure.[/roll]. "
            "The walking stick finds the floor first: damp, gritty, but not immediately giving way between you and the boards. "
            "You edge forward just far enough to work. From that braced stance, you get your photograph and test the edges "
            "without putting your fingers anywhere near the gaps. The boards answer with a dull, swollen creak. One corner "
            "flexes slightly under the stick, enough to suggest age and looseness, but not enough to cleanly expose what lies "
            "beneath. Dust shifts. Something small and dry skitters deeper underneath--too quick to see clearly, more sound "
            "than shape. Then, as you press a little farther along one edge, the board gives with a sharper crack than the "
            "others. Not a collapse--just enough sudden movement to make the footing feel treacherous. You pull back at once, "
            "retreating to the marked stairs before your weight commits any further. From there, you know three things for "
            "certain. the boards are not solidly trustworthy. there is space beneath them. and something under there can move. "
            "You have not yet opened the way or gotten a clear look into the cavity. What do you do?"
        )
        state = mod.VisibleState(
            transcript="Evelyn just tested the rough basement boards from the marked stair line.\n" + text,
            last_reply=text,
            turn=17,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
            },
        )

        self.assertTrue(mod.loosened_basement_gap_visible(text))
        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "stabilize_loosened_basement_gap",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertNotIn("upstairs", decision["sent_to_gm"].lower())

    def test_clarified_basement_boards_after_failed_descent_triggers_followup_not_reclarify(self):
        mod = load_module()
        text = (
            "From where you are now—at the bottom of the basement stairs, not advancing "
            "farther—the single most concrete visible affordance is a patch of rough wooden "
            "boards set low along one basement wall, off to one side of the storage clutter. "
            "They look older and cruder than the rest of the cellar finish: weathered, uneven, "
            "and more like something put up to cover an opening than part of the original "
            "structure. Location: low on the wall across the basement proper. Looks like: "
            "rough planks, dark with age and damp, fitted over a shallow recessed space. "
            "It appears physically reachable from your current position, but it is not yet "
            "proven safe; between you and it are dim footing, clutter, and concealment."
        )
        state = mod.VisibleState(
            transcript=(
                "The basement descent failed to reveal a trustworthy hidden panel, body, or ritual object.\n"
                + text
            ),
            last_reply=text,
            turn=15,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "moving_bedroom_threat",
                "bedroom_abandoned",
                "basement_descent",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "approach_clarified_basement_boards_from_stairs",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertNotIn("concrete lead", decision_text)
        self.assertIn("boards", decision["sent_to_gm"].lower())
        self.assertIn("basement", decision["sent_to_gm"].lower())

    def test_rough_wooden_boarding_sits_wrong_triggers_followup_not_reclarify(self):
        mod = load_module()
        text = (
            "The most concrete visible affordance is this. At the far side of the basement, "
            "along the wall beyond the old bins, a section of the rough wooden boarding sits "
            "wrong compared to the rest. It is not open, and it does not yet prove a hidden "
            "chamber, but it is the one place that visibly invites closer inspection. "
            "Location: against the basement wall, partially framed by stored junk and shadow. "
            "Look: the boards are crude, old, and uneven, more like something used to cover or "
            "conceal than to finish. One stretch appears less settled and less regular than "
            "the surrounding wall. Reachability from your current position is uncertain but "
            "physically approachable from the marked stair line."
        )
        state = mod.VisibleState(
            transcript=(
                "The basement descent failed, then the Keeper clarified the visible board section.\n"
                + text
            ),
            last_reply=text,
            turn=13,
            pc_name="Evelyn Pierce",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "moving_bedroom_threat",
                "basement_descent",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "approach_clarified_basement_boards_from_stairs",
        )
        self.assertIn("basement", action)
        self.assertNotIn("clarify_latest_visible_affordance", str(decision))

    def test_cluttered_basement_under_space_with_rough_boards_triggers_followup(self):
        mod = load_module()
        text = (
            "From where Evelyn has stopped, the plainest actionable thing is the basement room ahead of her. "
            "She can actually perceive a colder, wetter space opening out in front of her than the house above. "
            "Rough boards and old storage bins sit down there, and beyond them the basement falls away into a "
            "darker under-space that feels more like concealment than storage. Her retreat path is simple and "
            "still open: the stairs are behind her and back the way she came. To go farther in, she would have "
            "to move off that line of easy withdrawal and into the room itself, toward those boards and the dark "
            "space beyond them. What remains uncertain is everything the dimmer part of the basement might be "
            "hiding. From here, she can tell that the room invites closer inspection; she cannot yet prove what, "
            "if anything, is concealed there."
        )
        state = mod.VisibleState(
            transcript=(
                "Evelyn resumed the basement route after abandoning the unsafe bedroom.\n"
                + text
            ),
            last_reply=text,
            turn=28,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "moving_bedroom_threat",
                "bedroom_threshold_probe",
                "bedroom_abandoned",
                "basement_descent",
                "resumed_basement_after_bedroom",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "approach_clarified_basement_boards_from_stairs",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("boards", decision["sent_to_gm"].lower())

    def test_clarified_boarded_shadowed_under_space_triggers_probe_not_reclarify(self):
        mod = load_module()
        text = (
            "From where Evelyn stands, the basement itself is plain enough: the air is colder and wetter "
            "than upstairs, the room is cluttered with rough storage boards and old bins, and ahead of you "
            "there is a darker under-space that feels less like ordinary storage and more like something "
            "concealed.\n\n"
            "The actionable thing is this: there is a boarded, shadowed section deeper in the basement "
            "that looks like it was meant to close something off. It is in front of you, farther in than "
            "the stairs. Your retreat path is simple and still open--the stairs back up are behind you, "
            "and nothing visible is between you and them.\n\n"
            "What you can actually say, right now, is that the basement contains a hidden-looking, "
            "boarded-off under-space.\n\n"
            "What you cannot yet prove is what lies beyond those boards, how deep that space goes, "
            "whether anything is inside it, or whether there is a second chamber past it.\n\n"
            "So the concrete affordance is not go deeper in the abstract. It is this: if you choose, "
            "you can approach that boarded section and examine it directly as the next meaningful point "
            "of contact. Or you can hold where you are with the stairs at your back."
        )
        state = mod.VisibleState(
            transcript=(
                "Evelyn followed a basement clue, returned after abandoning an unproductive bedroom, "
                "and asked for one grounded clarification.\n" + text
            ),
            last_reply=text,
            turn=15,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "bedroom_abandoned",
                "resumed_basement_after_bedroom",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "probe_clarified_concealed_basement_section",
        )
        self.assertIn("board", decision["sent_to_gm"].lower())
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_blocked_darker_area_under_rough_boarding_triggers_probe_not_reclarify(self):
        mod = load_module()
        text = (
            "From where Evelyn stands, the basement gives her one clear truth and one clear invitation.\n\n"
            "What she can actually perceive is this: the air down here is colder and wetter than upstairs, "
            "the room is choked with rough old storage and boards, and ahead of her there is a darker "
            "under-space that feels less like ordinary basement clutter and more like something concealed. "
            "The most actionable feature in sight is that shadowed under-space beyond the rough boards. "
            "It is farther in than Evelyn's present footing, while the stairs back up remain behind her "
            "as the clean retreat route.\n\n"
            "So, in plain terms: the thing in front of her is a blocked, darker area under or beyond the "
            "basement's rough boarding. The thing behind her is the staircase out.\n\n"
            "What remains uncertain is everything that matters once she commits to it. From here, she "
            "cannot yet prove whether that dark space is only storage, a crawl-space, a hidden recess, "
            "or something deliberately screened off. She also cannot yet tell whether anything is inside "
            "it, whether the boards are loose, or whether going closer changes the danger.\n\n"
            "In other words, Evelyn does not yet have proof--only a concrete affordance: there is a "
            "darker, partially concealed space ahead that can be approached and examined, and the stairs "
            "remain open behind her if she wants to keep her withdrawal clean."
        )
        state = mod.VisibleState(
            transcript=(
                "Evelyn followed a basement clue, returned after abandoning an unproductive bedroom, "
                "and already received one clarification of the same concealed under-space.\n" + text
            ),
            last_reply=text,
            turn=16,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "bedroom_abandoned",
                "resumed_basement_after_bedroom",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "probe_clarified_concealed_basement_section",
        )
        self.assertIn("basement", decision["sent_to_gm"].lower())
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_boarded_irregularity_after_basement_followup_triggers_probe_not_reclarify(self):
        mod = load_module()
        text = (
            "From where you are, the only plainly actionable thing is the boarded irregularity "
            "in the basement structure--the rough, concealed section you have been photographing "
            "and testing from a safe distance. What you can actually perceive is this: the basement "
            "is colder and wetter than the floor above, and one part of it does not read like ordinary "
            "storage. Between the rough boards, bins, and the dark under-space beyond, there is a "
            "concealed area here rather than just clutter. It stands out as something intentionally "
            "closed off. Relative to your retreat, it is still in the basement with you, not beyond "
            "some new threshold. Your way back is the stairs up to the ground floor, and this "
            "obstruction lies forward of that escape."
        )
        state = mod.VisibleState(
            transcript=(
                "Evelyn followed a successful basement irregularity result, then asked for one "
                "grounded clarification.\n" + text
            ),
            last_reply=text,
            turn=13,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "probe_clarified_concealed_basement_section",
        )
        self.assertIn("basement", decision["sent_to_gm"].lower())
        self.assertIn("boards", decision["sent_to_gm"].lower())
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_successful_basement_board_probe_with_covered_opening_triggers_concealed_probe(self):
        mod = load_module()
        text = (
            "The stick taps ahead first--board, then earth, then board again. Nothing gives way under "
            "the cautious testing, and when you ease forward into a braced stance the rough planks answer "
            "with only a faint, old-wood complaint. [roll]Careful examination of the rough basement boards "
            "-- 1d100: 27 vs Spot Hidden 70 -- Hard success.[/roll] Up close, the boards do not read as "
            "random cellar junk. They have been laid over something. The edges show a narrow seam and a "
            "slight hollow beneath; when you probe along one side with the stick, you confirm they flex "
            "just a little over open space rather than solid ground. No sudden movement answers you, and "
            "no immediate sound comes back beyond the scrape of wood and the wet hush of the basement. "
            "What you have found is not just loose debris: it is a covered opening. From where you are now, "
            "you can photograph it clearly and retreat safely to the marked stairs if you choose."
        )
        state = mod.VisibleState(
            transcript="Evelyn already approached the rough basement boards from the marked stairs.\n" + text,
            last_reply=text,
            turn=15,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "moving_bedroom_threat",
                "bedroom_abandoned",
                "basement_descent",
                "basement_followup",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "probe_clarified_concealed_basement_section",
        )
        self.assertIn("covered", text.lower())
        self.assertIn("boards", decision["sent_to_gm"].lower())
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_basement_boarded_under_space_after_bedroom_retreat_stays_on_boards_not_resume_route(self):
        mod = load_module()
        text = (
            "From where you've stopped on the basement stairs, the house is giving you one "
            "immediate, usable fact: you are in the basement now, but not committed to its depths. "
            "The stairs are still directly behind you, rising back to the kitchen door and then out "
            "the way you came. Nothing visible has cut off that retreat. Ahead and below, the basement "
            "itself feels colder and wetter than the rooms above. You can make out rough boards, old "
            "storage clutter, bins and broken odds and ends in the dimness, and beyond that a darker "
            "under-space where the light does not settle cleanly. That deeper dark is the thing in "
            "front of you--the nearest meaningful place this room wants you to deal with. There is "
            "a darker, more concealed space ahead in the basement, partly masked by rough boards and "
            "shadow. In plain terms: your way back is still open; the thing in front of you is the "
            "shadowed, boarded under-space farther into the basement."
        )
        state = mod.VisibleState(
            transcript="The bedroom was abandoned; Evelyn descended to the basement and asked for a grounded affordance.\n" + text,
            last_reply=text,
            turn=18,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "bedroom_abandoned",
                "basement_descent",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "approach_clarified_basement_boards_from_stairs",
        )
        self.assertIn("boards", decision["sent_to_gm"].lower())
        self.assertNotIn("resume_mapped_basement_route_after_bedroom_retreat", decision_text)

    def test_suspicious_basement_area_after_first_clarification_probes_not_reclarify(self):
        mod = load_module()
        text = (
            "From where Evelyn stands, the concrete thing is the basement itself. The stairs are still "
            "directly behind her. If she backs away now, her retreat is simple: turn, climb, out. "
            "Nothing visible is between her and that exit. In front of her, at the foot of the stairs "
            "and spreading outward, the cellar opens into a colder, wetter dark. She can make out rough "
            "boards, old storage bins, damp masonry, and the kind of low, cluttered under-space that "
            "makes the room feel less like ordinary storage and more like something tucked away on purpose. "
            "That is the nearest actionable feature: the basement floor ahead, especially the darker, "
            "rougher section beyond the obvious footing near the stairs. What she can actually say, "
            "truthfully, is this: there is space ahead worth examining, and the room changes character "
            "the farther it goes from the stairs. What she cannot yet prove is whether that concealment "
            "means a crawl space, hidden object, or only old clutter."
        )
        state = mod.VisibleState(
            transcript="Evelyn descended to the basement and asked for one grounded affordance.\n" + text,
            last_reply=text,
            turn=12,
            pc_name="Evelyn Mercer",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "approach_clarified_basement_boards_from_stairs",
        )
        self.assertIn("boards", decision["sent_to_gm"].lower())
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_suspicious_boarded_cluttered_basement_area_probes_after_one_clarification(self):
        mod = load_module()
        text = (
            "From where Evelyn stands, the concrete thing is the basement itself. You can perceive "
            "the stair behind you and the way back up to the ground floor; that retreat path is still "
            "there, direct and uncomplicated. Ahead and around the foot of the stairs, the basement "
            "opens into colder, wetter dark. The air is heavier here. There are rough boards, old bins, "
            "and the sense of an under-space beyond ordinary storage, but from here it is still only a "
            "shape in the room, not a confirmed secret. Nothing visible from your present position proves "
            "what is concealed there, whether anything is moving in it, or whether the boards are just "
            "debris or deliberately covering something. So the latest actionable affordance, in plain "
            "terms, is this: there is a suspicious boarded, cluttered area deeper in the basement, in "
            "front of you and beyond the stairs, while your clear retreat remains immediately behind "
            "you and up."
        )
        state = mod.VisibleState(
            transcript="Evelyn descended to the basement and asked for one grounded affordance.\n" + text,
            last_reply=text,
            turn=12,
            pc_name="Evelyn Mercer",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "approach_clarified_basement_boards_from_stairs",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_basement_affordance_with_upstairs_comparison_does_not_trigger_upstairs(self):
        mod = load_module()
        text = (
            "From where you are now, the clearest thing you can honestly rely on is this: "
            "You are at the top reach of the basement descent, with the door you wedged open "
            "still marking your way back to the ground floor. Ahead of you, the basement does "
            "not yet resolve into firm answers. What you can tell is that the air below is "
            "colder, wetter, and more oppressive than the rooms upstairs. The space beyond "
            "your light reads as rough storage at first glance--boards, bins, and low dark "
            "stretches--but it also feels wrong, as though parts of the room are meant less "
            "for use than for concealment. So the latest actionable thing is not go deeper "
            "in the abstract. It is this specific affordance: there is a basement space "
            "directly in front of your current light and footing, reachable without abandoning "
            "your retreat, and it is the first place where a closer look might turn unease "
            "into something definite. In plain terms: you have a clear way back, a clear "
            "space immediately ahead worth examining, and no proof yet of what in that space "
            "is ordinary neglect and what is not."
        )
        state = mod.VisibleState(
            transcript="Evelyn is holding at the basement threshold after a grounded clarification.\n" + text,
            last_reply=text,
            turn=10,
            pc_name="Evelyn Cross",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "approach_clarified_basement_boards_from_stairs",
        )
        self.assertIn("basement", decision["sent_to_gm"].lower())
        self.assertNotIn("upstairs", decision["sent_to_gm"].lower())
        self.assertNotIn("cautious_upper_floor_search", decision_text)

    def test_clarified_basement_stair_foot_triggers_near_wall_probe_not_reclarify(self):
        mod = load_module()
        text = (
            "At the bottom of the cellar stairs, the single most concrete affordance you have "
            "is this: the stairs end in a small, uneven patch of basement floor immediately in "
            "front of the last step. Your flashlight shows packed dirt underfoot, the stone "
            "foundation wall running off into shadow, and rough boards and old storage clutter "
            "farther out in the dark rather than right on top of you. From where Evelyn is now, "
            "that patch at the foot of the stairs is safely reachable because she is already on "
            "it or at its very edge. The only firm, concrete next affordance presently established "
            "is the basement floor at the stair-foot and the nearby foundation wall within "
            "flashlight range."
        )
        state = mod.VisibleState(
            transcript="The basement descent failed to produce a hidden-panel read.\n" + text,
            last_reply=text,
            turn=10,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "basement_descent",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "examine_basement_stair_foot_and_near_wall",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("foundation wall", decision["sent_to_gm"].lower())
        self.assertIn("stair", decision["sent_to_gm"].lower())

    def test_basement_bottom_cellar_ahead_triggers_near_wall_probe_not_reclarify(self):
        mod = load_module()
        text = (
            "You ease the basement door open and leave the handkerchief where it will catch your eye "
            "on the way back. The air below is colder. Your flashlight beam slides down a narrow run "
            "of steps into a basement that feels older than the rest of the house. The stairs hold. "
            "Nothing has shifted behind your back. Nothing blocks the way. At the foot of the stairs, "
            "your light reaches only partway into the cellar: rough foundation walls, cluttered dimness, "
            "a low spread of shadow where the beam seems to thin rather than end. You are now at the "
            "bottom of the basement stairs, with the marked way back behind you and the cellar ahead."
        )
        state = mod.VisibleState(
            transcript="Evelyn resumed the mapped basement route after leaving the bedroom.\n" + text,
            last_reply=text,
            turn=15,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "bedroom_abandoned",
                "basement_descent",
                "resumed_basement_after_bedroom",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertTrue(mod.clarified_basement_stair_foot_visible(text))
        self.assertEqual(
            decision["response_contract"]["intent"],
            "examine_basement_stair_foot_and_near_wall",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("foundation wall", decision["sent_to_gm"].lower())
        self.assertIn("stair", decision["sent_to_gm"].lower())

    def test_basement_safe_first_foothold_triggers_stair_foot_probe_not_reclarify(self):
        mod = load_module()
        text = (
            "[roll]Careful basement descent and search: 1d100 = 26 vs Spot Hidden 70 -- Hard Success.[/roll] "
            "Testing each board before trusting it, you make it down without a sudden collapse, rush, or ambush. "
            "At the foot of the stairs, your flashlight picks out the near details cleanly: old dust, damp masonry, "
            "and floor marks that are easier to distinguish here than from above. Nothing immediately lunges from "
            "the dark, and there is no body lying in plain sight at the bottom of the steps. The air below feels "
            "colder and more stagnant than the rooms above. More importantly, you've established a safe first foothold: "
            "the stairs behind you remain usable, the door above is still marked and wedged open, and you have a "
            "moment to choose how to press the search."
        )
        state = mod.VisibleState(
            transcript="Evelyn descended into the basement and established her retreat line.\n" + text,
            last_reply=text,
            turn=14,
            pc_name="Evelyn Pierce",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "bedroom_abandoned",
                "basement_descent",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "examine_basement_stair_foot_and_near_wall",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("floor", decision["sent_to_gm"].lower())
        self.assertIn("stair", decision["sent_to_gm"].lower())

    def test_failed_basement_descent_with_stairs_back_triggers_near_wall_probe_not_reclarify(self):
        mod = load_module()
        text = (
            "[roll]Descend the basement stairs cautiously and search for hidden threats or concealed features: "
            "1d100 = 50 vs Spot Hidden 45 -- failure.[/roll] You listen first. Nothing answers at once. "
            "Then you go down carefully, one step at a time, testing each board with the stick before trusting it. "
            "The wood complains in small dry creaks, but nothing gives way under you. Your flashlight beam drifts "
            "across rough foundation walls, old dust, and the packed dimness below. At the foot of the stairs, "
            "the basement opens out in a way that feels more oppressive than spacious. You do not find everything "
            "you were hoping to catch at a glance. No unmistakable hidden panel presents itself. No clean scrape "
            "mark plainly declares recent movement. What is immediately true is simpler and less comforting: "
            "You are now in the basement. The stairs back up remain behind you. Nothing has attacked yet. "
            "And the cellar invites closer inspection if you want more than this first impression."
        )
        state = mod.VisibleState(
            transcript="Evelyn descended into the basement and reached the stair foot.\n" + text,
            last_reply=text,
            turn=8,
            pc_name="Evelyn Marsh",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertTrue(mod.clarified_basement_stair_foot_visible(text))
        self.assertEqual(
            decision["response_contract"]["intent"],
            "examine_basement_stair_foot_and_near_wall",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("foundation", decision["sent_to_gm"].lower())
        self.assertIn("stair", decision["sent_to_gm"].lower())

    def test_basement_foot_take_in_cellar_triggers_stair_foot_probe_not_reclarify(self):
        mod = load_module()
        text = (
            "[roll]Careful descent to the basement stairs -- 1d100: 37 vs Spot Hidden 55 -- success.[/roll] "
            "You return to the marked basement door and find the route unchanged. Your flashlight beam catches "
            "the narrow descent in slices: old wood, dust along the edges, damp staining where the house seems "
            "to sweat into itself. You test each step before putting your weight down. Nothing gives way. "
            "Nothing has been dragged across the stairs. Nothing is waiting in plain sight at the turn. "
            "The air grows cooler as you descend, and the smell shifts from stale plaster and trapped damp to "
            "something earthier and more shut-in below. The house carries small sounds oddly, but for the moment "
            "the stair remains passable. At the foot of the basement stairs, you can pause and take in the cellar "
            "before committing farther."
        )
        state = mod.VisibleState(
            transcript="Evelyn returned to the basement route after a failed first descent.\n" + text,
            last_reply=text,
            turn=14,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_recovery",
                "basement_exit",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "examine_basement_stair_foot_and_near_wall",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertIn("stair", decision["sent_to_gm"].lower())
        self.assertIn("cellar", decision["sent_to_gm"].lower())

    def test_failed_basement_stair_descent_keeps_next_action_in_current_position(self):
        mod = load_module()
        text = (
            "[roll]Careful descent and search of the Corbitt House basement stairs for "
            "disturbances or immediate threats — 1d100: 90 vs target 65 — Failure.[/roll]\n"
            "Evelyn is partway down the basement stairs, with the open door and her marked "
            "retreat behind her, and the unseen cellar below now fully aware of her presence."
        )
        state = mod.VisibleState(
            transcript=(
                "The entry hall has a main stair up and a basement access. "
                "The house has both upper rooms and a cellar.\n" + text
            ),
            last_reply=text,
            turn=8,
            pc_name="Evelyn Marsh",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "second_chapel_method",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "recover_from_failed_basement_stair_descent",
        )
        self.assertIn("basement stairs", action)
        self.assertNotIn("upstairs", action)
        self.assertNotIn("bedroom", action)

    def test_chinese_failed_basement_stair_descent_recovers_not_generic_clarify(self):
        mod = load_module()
        text = (
            "[roll]谨慎下行并检查地下室楼梯周围迹象 1d100=[100] ≤60，结果:强失败[/roll]\n"
            "你先停在楼梯口上方，侧着头去听。下方只有一股潮烂的冷气慢慢顶上来。"
            "当你继续往下时，脚下那块木板忽然发出一声又脆又响的爆裂呻吟。"
            "你本能地把重量抽回来，手一下攥紧门边和手杖，鞋底在积灰的木阶上蹭出刺耳的一道响。"
            "那一下几乎让你失去平衡。你没能从楼梯与地下室口看出任何可靠的迹象。"
            "你只能僵在台阶间，听着那声木板裂响的余音一点点沉下去。"
            "手帕仍拴在门把上，门缝也还替你留着；而你此刻最清楚的，只是这条向下的路比刚才显得更险。"
        )
        state = mod.VisibleState(
            transcript="The house has both upper rooms and a basement route.\n" + text,
            last_reply=text,
            turn=10,
            pc_name="Evelyn Hart",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "bedroom_abandoned",
                "basement_descent",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "recover_from_failed_basement_stair_descent",
        )
        self.assertIn("basement stairs", action)
        self.assertNotIn("clarify_latest_visible_affordance", str(decision))

    def test_failed_basement_recovery_still_on_stairs_exits_before_upstairs(self):
        mod = load_module()
        text = (
            "[roll]Listen for movement below after the stair creak -- 1d100: 72 vs 55 -- Failure.[/roll] "
            "You hold still on the stair, flashlight fixed into the black below, and strain for the slightest answer. "
            "What comes back is not certainty. The cellar stays mute long enough to become worse than noise. "
            "Then, as you ease your weight backward, one board under your foot gives a dry complaining groan--"
            "louder than the first. The sound seems to run down into the wet dark beneath the house. For an "
            "instant, nothing answers. Then, from below and out of the beam, you catch a low disturbance: not a "
            "voice, not a clear footstep, but a faint scrape or shift somewhere in the cellar gloom. You cannot "
            "tell where exactly, only that the silence no longer feels empty. You are still on the stairs, the open "
            "basement doorway behind you, your handkerchief marker above and back at the threshold, flashlight "
            "trained downward."
        )
        state = mod.VisibleState(
            transcript="Evelyn failed the basement descent and tried to recover toward the open doorway.\n" + text,
            last_reply=text,
            turn=10,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_recovery",
            },
        )

        self.assertTrue(mod.basement_exit_reachable_after_failed_recovery_visible(text))
        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "exit_basement_after_noisy_recovery_failure",
        )
        self.assertIn("basement doorway", decision["sent_to_gm"].lower())
        self.assertNotIn("cautious_upper_floor_search", decision_text)
        self.assertNotIn("upstairs", decision["sent_to_gm"].lower())

    def test_empty_result_after_basement_descent_reanchors_to_basement_not_generic_fallback(self):
        mod = load_module()
        text = "你把动作停在当前已经确认的位置。没有新的公开通路、伤害或警报被确认；当前位置、退路和仍未确认的细节需要继续结算。"
        state = mod.VisibleState(
            transcript=(
                "The ground floor search identified a basement route.\n"
                "Evelyn tied a handkerchief to the basement door handle and began descending the basement stairs.\n"
                + text
            ),
            last_reply=text,
            turn=15,
            pc_name="Evelyn Pierce",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "bedroom_abandoned",
                "basement_descent",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "clarify_empty_basement_descent_result",
        )
        self.assertIn("basement", action)
        self.assertIn("previous action", action)
        self.assertNotIn("clarify_latest_visible_affordance", json.dumps(decision, ensure_ascii=False))

    def test_failed_basement_partway_to_floor_recovers_not_clarifies(self):
        mod = load_module()
        text = (
            "[roll]Careful descent and threat-search of the basement stairs: 1d100 = 79 vs "
            "target 75 -- failure.[/roll] The caution keeps you from blundering, but it does "
            "not make the cellar yield its secrets. The steps groan under the probing stick. "
            "Dust shifts. No obvious loose panel shows itself from the stair. If there are "
            "scrape marks, disturbed earth, or hidden workings down there, they are subtler "
            "than this first descent lets you read. Nothing attacks you in this instant. "
            "You are now partway to the basement floor, with the marked door still open above "
            "you and the cellar spreading out ahead in uncertain dark."
        )
        state = mod.VisibleState(
            transcript="The house has both upper rooms and a cellar.\n" + text,
            last_reply=text,
            turn=12,
            pc_name="Evelyn Pierce",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "moving_bedroom_threat",
                "basement_descent",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "recover_from_failed_basement_stair_descent",
        )
        self.assertIn("basement", action)
        self.assertNotIn("clarify_latest_visible_affordance", str(decision))

    def test_failed_basement_recovery_at_open_door_exits_not_fallback(self):
        mod = load_module()
        text = (
            "You hold still on the stairs, flashlight fixed into the black below. "
            "[roll]Careful retreat up the basement stairs — 1d100: 96 vs DEX 65 — failure.[/roll] "
            "You catch yourself hard against the stair rail and the doorframe instead of tumbling. "
            "The open basement door is just behind you now, with your handkerchief marker still at "
            "the threshold. The cellar below remains a dark well under the beam of your light."
        )
        state = mod.VisibleState(
            transcript="The house has a cellar route.\n" + text,
            last_reply=text,
            turn=15,
            pc_name="Evelyn Pierce",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "second_chapel_method",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_recovery",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "exit_basement_after_noisy_recovery_failure",
        )
        self.assertNotIn("clarify the single most concrete", action)
        self.assertNotIn("disturbed earth", action)
        self.assertIn("basement door", action)
        self.assertIn("ground floor", action)

    def test_failed_basement_recovery_still_on_stairs_exits_not_upstairs(self):
        mod = load_module()
        text = (
            "[roll]Freeze on the basement stairs and listen for any response below after the loud creak: "
            "1d100 = 76 vs 50 — failure.[/roll] The effort gives you no certainty. "
            "[roll]Retreat one careful step at a time up the basement stairs without slipping or exposing yourself: "
            "1d100 = 74 vs 60 — failure.[/roll] You begin to ease backward toward the open door and the "
            "handkerchief marker, testing each step before trusting it-but the stairs do not cooperate. "
            "One board gives a sharp protesting snap under your shifting weight. You catch yourself before "
            "tumbling, but not neatly. The retreat is no longer silent, controlled withdrawal. You are still "
            "on the stairs, higher than before but not yet through the doorway, half-turned between cellar "
            "and escape, with the flashlight beam jolting across rough boards, old bins, and the dark "
            "under-space below."
        )
        state = mod.VisibleState(
            transcript="The house has both upper rooms and a basement route.\n" + text,
            last_reply=text,
            turn=9,
            pc_name="Evelyn Mercer",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_recovery",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "exit_basement_after_noisy_recovery_failure",
        )
        self.assertIn("basement", action)
        self.assertIn("ground floor", action)
        self.assertNotIn("upstairs", action)

    def test_basement_recovery_with_marker_visible_and_disturbance_exits_not_upstairs(self):
        mod = load_module()
        text = (
            "[roll]Listen for any response below after the stair creak — 1d100: 79 vs 55 — Failure.[/roll] "
            "The basement gives you no clear answer. The flashlight beam quivers faintly across rough boards "
            "and black gaps below, but nothing comes rushing up the stairs. Keeping your weight low, you ease "
            "one foot back, then the next. The open basement door remains above you, with your handkerchief "
            "marker just visible in the spill of weaker light from behind. You gain another careful step toward "
            "the doorway. Then—still from below, not yet seen—there comes the faintest suggestion of disturbance "
            "in the dark cellar: not a charge, not even a definite footfall."
        )
        state = mod.VisibleState(
            transcript="The house has both upper rooms and a basement route.\n" + text,
            last_reply=text,
            turn=11,
            pc_name="Miriam Vale",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_recovery",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "exit_basement_after_noisy_recovery_failure",
        )
        self.assertIn("basement", action)
        self.assertIn("ground floor", action)
        self.assertNotIn("upstairs", action)
        self.assertNotIn("bedroom", action)

    def test_basement_recovery_in_progress_toward_open_door_exits_not_fallback(self):
        mod = load_module()
        text = (
            "The stair gives that dry complaint under Evelyn's lowered weight. She eases back "
            "a step, then another, toward the open basement doorway above. The handkerchief marker "
            "remains visible at the top, pale against the door hardware, and the wedge still holds "
            "the door open. What is concrete now: Evelyn is still on the basement stairs, retreating "
            "carefully toward the doorway rather than committed to the cellar floor."
        )
        state = mod.VisibleState(
            transcript="The house has a cellar route.\n" + text,
            last_reply=text,
            turn=12,
            pc_name="Evelyn Mercer",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_recovery",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "exit_basement_after_noisy_recovery_failure",
        )
        self.assertNotIn("clarify the single most concrete", decision["sent_to_gm"].lower())

    def test_basement_recovery_backing_toward_doorway_exits_not_upstairs(self):
        mod = load_module()
        text = (
            "[roll]Listen for any response from the cellar after the loud stair-creak -- "
            "1d100: 85 vs 45 -- Failure.[/roll] Evelyn goes still on the stair, lowering "
            "her weight and holding the flashlight beam steady into the cellar dark. For a few "
            "seconds there is nothing clear enough to trust. Not wanting to give the darkness below "
            "any more of her than it already has, she begins to retreat exactly as planned--one "
            "tested step, then another, back toward the open basement door and the handkerchief marker "
            "above. The stairs complain softly under careful pressure, but nothing immediately rushes "
            "up at her. She is now backing up toward the doorway, still facing down into the cellar, "
            "with her escape line intact for the moment."
        )
        state = mod.VisibleState(
            transcript="Evelyn is recovering from a failed basement descent.\n" + text,
            last_reply=text,
            turn=10,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_recovery",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "exit_basement_after_noisy_recovery_failure",
        )
        self.assertNotIn("upstairs", decision["sent_to_gm"].lower())
        self.assertNotIn("bedroom", decision["sent_to_gm"].lower())

    def test_clarified_basement_top_landing_after_recovery_exits_not_repeat(self):
        mod = load_module()
        text = (
            "The most concrete affordance is still the basement stair and its top landing, not "
            "the cellar floor below. Location: you are on the basement stairs, partway up from "
            "the lower darkness and partway below the ground-floor doorway. Above you is the "
            "open basement door you wedged earlier. The handkerchief marker is tied to that "
            "door handle and remains visible from where you are. The doorway above is the "
            "clearest fixed point: a rectangle of safer return."
        )
        state = mod.VisibleState(
            transcript="The house has a cellar route.\n" + text,
            last_reply=text,
            turn=13,
            pc_name="Evelyn Mercer",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_recovery",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "exit_basement_after_noisy_recovery_failure",
        )
        self.assertNotIn("clarify the single most concrete", decision["sent_to_gm"].lower())

    def test_after_exiting_failed_basement_reassesses_stair_mouth_not_bedroom_fallback(self):
        mod = load_module()
        text = (
            "You are now clear of the basement threshold, with the basement door still open "
            "and marked, your flashlight on the stair mouth, and room to either hold this "
            "position or fall back farther toward the exit. Nothing surges up after her. "
            "The cold, wet air still breathes out of the basement, and the darkness below "
            "remains thick and unreadable. Behind you, the exterior route remains the cleanest "
            "escape line you have established in the house."
        )
        state = mod.VisibleState(
            transcript=(
                "The upstairs bedroom was marked unsafe after the bed moved by itself.\n" + text
            ),
            last_reply=text,
            turn=15,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
                "bedroom_threshold_probe",
                "moving_bedroom_threat",
                "bedroom_abandoned",
                "basement_descent",
                "basement_recovery",
                "basement_exit",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "reassess_open_basement_stair_mouth_after_failed_descent",
        )
        self.assertIn("basement", action)
        self.assertIn("stair", action)
        self.assertNotIn("bedroom", action)
        self.assertNotIn("clarify_latest_visible_affordance", str(decision))

    def test_speculative_basement_door_does_not_become_disturbed_earth_lead(self):
        mod = load_module()
        text = (
            "The most concrete visible affordance is the door directly in front of you. "
            "The air here smells cold, wet, and stale, with that mold-and-old-timber odor "
            "the whole basement seems to carry. From your current position, yes: it is safely "
            "reachable. What you can honestly write down is: there is a closed, reachable door here. "
            "What you still cannot prove is: whether it is locked, what lies beyond it, and whether "
            "it merely leads to another cellar space or conceals something more significant."
        )
        state = mod.VisibleState(
            transcript="The house has a basement route.\n" + text,
            last_reply=text,
            turn=17,
            pc_name="Evelyn Pierce",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "second_chapel_method",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_recovery",
            },
        )

        self.assertFalse(mod.basement_actionable_lead_visible(text))

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "resolve_clarified_basement_door_affordance",
        )
        self.assertNotIn("disturbed earth", action)
        self.assertNotIn("last search produced a concrete lead", action)
        self.assertNotIn("clarify the single most concrete", action)

    def test_clarified_exterior_door_affordance_triggers_concrete_entry_attempt(self):
        mod = load_module()
        text = (
            "The single most concrete visible affordance is the side door you already tested. "
            "Exact location: on the least exposed side of the house, slightly set back from the street. "
            "What it looks like: weathered wooden door with dull old metal lock hardware. "
            "What it sounds like: when you tried the key, the lock gave a dry scraping resistance. "
            "Safely reachable?: yes, you can stand at it while keeping your retreat path open behind you."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=11,
            pc_name="Evelyn Marsh",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "second_chapel_method",
                "house_exterior",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "enter_threshold_via_clarified_exterior_door",
        )
        self.assertNotIn("clarify the single most concrete", decision["sent_to_gm"])
        self.assertIn("turns the key", decision["sent_to_gm"])

    def test_clarified_front_door_with_marked_key_triggers_entry_attempt(self):
        mod = load_module()
        text = (
            "The single most concrete visible affordance is the front door of the Corbitt House itself. "
            "It is directly at the front threshold from your present position outside the house, close "
            "enough to approach without going deeper into the property than the entry step. The door "
            "sits old and shut in its frame, with the front lock available for the key Knott marked for it. "
            "Location: the front entrance, at the main threshold of the house; Sounds like: nothing. "
            "Is it safely reachable from where you are? Yes."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=7,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "enter_threshold_via_clarified_exterior_door",
        )

    def test_uncertain_key_fit_does_not_trigger_unconditional_unlock(self):
        mod = load_module()
        text = (
            "From where Evelyn stands now, the clearest actionable thing is the front door. "
            "It sits ahead of you at the end of the front approach, with the open route back "
            "to the street still behind you. What you cannot yet prove is whether this key "
            "fits that lock cleanly, whether the door is merely locked or swollen shut, or "
            "whether anything on the other side is immediately visible once opened. "
            "So in plain terms: the door is the affordance; the street behind you is the retreat; "
            "the unknown begins at the lock and everything past it."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=7,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
            },
        )

        self.assertFalse(mod.entry_available_visible(text))

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "enter_threshold_via_clarified_exterior_door",
        )
        self.assertIn("turns the key", action)
        self.assertIn("if the lock opens", action)
        self.assertNotIn("unlocks the safest exterior door", action)
        self.assertNotIn("clarify the single most concrete", decision["sent_to_gm"])
        self.assertNotIn("side door", decision["sent_to_gm"].lower())
        self.assertIn("clarified exterior door", decision["sent_to_gm"].lower())

    def test_open_ground_behind_closed_door_does_not_count_as_open_threshold(self):
        mod = load_module()
        text = (
            "From where Evelyn stands, the one solid thing in front of her is simple: "
            "The lock she tested works. The house can be entered through that door. "
            "Physically, that means the actionable point is the threshold itself: one closed "
            "exterior door at the least exposed side of the property, with the yard and her line "
            "back out behind her. If she changes her mind, retreat is easy--she is still outside, "
            "in daylight, with open ground between herself and the street. The door is real, "
            "accessible, and matched to Knott's key."
        )
        state = mod.VisibleState(
            transcript="The exterior survey confirmed a closed matched door.\n" + text,
            last_reply=text,
            turn=7,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
            },
        )

        self.assertFalse(mod.open_exterior_threshold_affordance_visible(text))

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "enter_threshold_and_map_ground_floor_affordances",
        )
        self.assertNotIn("already open", decision["sent_to_gm"].lower())

    def test_key_fit_enough_to_test_does_not_trigger_unconditional_unlock(self):
        mod = load_module()
        text = (
            "You do mark the obvious approaches: front access, rear access, side exposure, "
            "and a likely cellar way. When you finally choose the least exposed lock and try "
            "one of Knott's keys, it does fit well enough to test--but you are not yet rewarded "
            "with any obvious discovery beyond that fact. The old house remains closed, watchful, "
            "and not yet explained. You are outside, still with an exit route behind you, at the "
            "point of testing that exterior access further if you wish."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=7,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
            },
        )

        self.assertFalse(mod.entry_available_visible(text))
        self.assertTrue(mod.clarified_exterior_door_affordance_visible(text))

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "enter_threshold_via_clarified_exterior_door",
        )
        self.assertIn("turns the key", action)
        self.assertIn("if the lock opens", action)
        self.assertNotIn("unlocks the safest exterior door", action)

    def test_stubborn_keyed_exterior_door_triggers_controlled_lock_test_not_reclarify(self):
        mod = load_module()
        text = (
            "From where you are now, the clearest practical fact is this: The door you tested "
            "is real, close, and in front of you. The key bit enough in the lock to tell you it "
            "belongs here, but the door has not yielded. It stands between you and the house, "
            "and it is still the nearest concrete way in. Your retreat path is equally plain: "
            "back the way you came, off the threshold, away from the wall, and toward the street. "
            "The concrete affordance is that one stubborn outer door at arm's reach: a possible "
            "entry point that has answered your key, but has not yet yielded."
        )
        state = mod.VisibleState(
            transcript="The exterior survey failed to find a better entrance, then the door was clarified.\n" + text,
            last_reply=text,
            turn=8,
            pc_name="Evelyn Hart",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "enter_threshold_via_clarified_exterior_door",
        )
        self.assertIn("if the lock jams", decision["sent_to_gm"].lower())
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_key_seated_in_exterior_lock_triggers_entry_not_second_reclarify(self):
        mod = load_module()
        text = (
            "You hold one concrete thing, and it is close at hand. The key is in an exterior "
            "lock you judged the least exposed from outside. The metal is old and stiff, "
            "but it fit well enough to meet the mechanism instead of skidding uselessly. "
            "That door is the latest actionable point. It is right there in front of you, "
            "with the wall at your side and the yard behind you. Your retreat path is simple "
            "from this position: back off from the door and retrace the same approach."
        )
        state = mod.VisibleState(
            transcript="The exterior survey failed to find a better entrance, then the door was clarified.\n" + text,
            last_reply=text,
            turn=9,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertTrue(mod.clarified_exterior_door_affordance_visible(text))
        self.assertEqual(
            decision["response_contract"]["intent"],
            "enter_threshold_via_clarified_exterior_door",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_successful_exterior_lock_test_triggers_entry_not_clarification(self):
        mod = load_module()
        text = (
            "[roll]Careful exterior survey of the Corbitt House: 1d100 = 61 vs target 60 — Failure.[/roll]\n"
            "You do not get the clean, telling read you wanted. What does become clear is practical "
            "rather than revelatory: the keys do match the house. When you choose the least exposed "
            "lock and test it carefully, it yields. The house can be entered from here."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=5,
            pc_name="Eleanor Price",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "enter_threshold_and_map_ground_floor_affordances",
        )
        self.assertNotIn("clarify the single most concrete", decision["sent_to_gm"])

    def test_no_new_visible_entry_result_clarifies_instead_of_ground_floor_search(self):
        mod = load_module()
        text = "（本回合按已落账的机械结果继续；无新增可公开的机械事实。）"
        state = mod.VisibleState(
            transcript=(
                "The key fits and will work there. The door is not yet opened. "
                "Walter Corbitt was buried in the basement of his house.\n" + text
            ),
            last_reply=text,
            turn=6,
            pc_name="Evelyn Hart",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
                "enter_threshold",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "clarify_empty_visible_result_before_continuing",
        )
        self.assertNotEqual(
            decision["response_contract"]["intent"],
            "systematic_ground_floor_search",
        )

    def test_successful_first_threshold_strip_triggers_ground_floor_search_not_reclarify(self):
        mod = load_module()
        text = (
            "[roll]Cautious threshold inspection at the side entrance: "
            "1d100=[33] vs Spot Hidden 50 — success.[/roll] "
            "Keeping one hand on the frame and the daylight at your back, you ease just far "
            "enough inside to test the first visible strip of floor. The board under your leading "
            "weight does not sag, crack, or drop. It complains softly with old-house creak, but "
            "it holds. Your low flashlight catches only the immediate margin of the interior: "
            "not a large open room, but the near edge of a dim passage or service entry, "
            "close-walled and dust-muted. The air inside is stale and shut-in, with old wood and "
            "long-settled dust. There is also a faint draft somewhere deeper in the house. "
            "Near the threshold itself, the dust is uneven."
        )
        state = mod.VisibleState(
            transcript=(
                "The side door is open and Evelyn has begun a cautious threshold entry.\n" + text
            ),
            last_reply=text,
            turn=13,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
                "door_opened",
                "enter_threshold",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "systematic_ground_floor_search",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_plural_keys_fit_and_lock_accepted_key_triggers_entry_not_clarification(self):
        mod = load_module()
        text = (
            "The Corbitt House waits behind its old address. The keys fit. "
            "The windows are still, and the air near the threshold is stale. "
            "When you try the least exposed matching lock, the key meets it cleanly enough "
            "to tell you Knott did not send you here with junk metal. "
            "You are now at the house exterior with a viable entry point under your hand "
            "and a clear retreat behind you."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=5,
            pc_name="Evelyn Hart",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "enter_threshold_and_map_ground_floor_affordances",
        )
        self.assertNotIn("clarify the single most concrete", decision["sent_to_gm"])

    def test_failed_exterior_safety_read_does_not_claim_safest_door(self):
        mod = load_module()
        text = (
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
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=7,
            pc_name="Eleanor Price",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "enter_threshold_and_map_ground_floor_affordances",
        )
        self.assertIn("workable exterior entrance", action)
        self.assertIn("without treating it as proven safest", action)
        self.assertNotIn("safest exterior door", action)

    def test_lock_accepts_one_key_and_door_before_pc_triggers_entry_not_clarification(self):
        mod = load_module()
        text = (
            "After photographing the exterior, she tries the least exposed of the exterior locks "
            "with Knott's keys. The key ring is no bluff: one of them does match. "
            "The lock accepts it, and at close range the house gives off stale, shut-in air from "
            "the seam of the entrance. The door is there before her now, matched key in hand, "
            "her exit route still clear behind."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=6,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "enter_threshold_and_map_ground_floor_affordances",
        )
        self.assertNotIn("clarify the single most concrete", decision["sent_to_gm"])

    def test_exterior_door_proven_to_take_key_triggers_entry_not_repeat_clarification(self):
        mod = load_module()
        text = (
            "The single most concrete visible affordance is the exterior door you just tested successfully. "
            "It is directly in front of you. The lock is now proven to take Knott's key, and when tested "
            "it gave a plain mechanical click and yield. Whether it is safely reachable: yes. "
            "One specific exterior entrance is real, matched to Knott's keys, and can be opened from "
            "her current position."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=7,
            pc_name="Eleanor Price",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertIn(
            decision["response_contract"]["intent"],
            {
                "enter_threshold_and_map_ground_floor_affordances",
                "enter_threshold_via_clarified_exterior_door",
            },
        )
        self.assertNotIn("clarify the single most concrete", decision["sent_to_gm"])

    def test_enter_threshold_via_clarified_door_marker_does_not_claim_inside_yet(self):
        mod = load_module()
        state = mod.VisibleState(transcript="", last_reply="", turn=8, pc_name="Eleanor Price")
        decision = {
            "response_contract": {"intent": "enter_threshold_via_clarified_exterior_door"}
        }

        mod.apply_attempt_marker(decision, state)

        self.assertIn("door_opened", state.attempted)
        self.assertNotIn("enter_threshold", state.attempted)

    def test_systematic_ground_floor_marker_implies_threshold_entered(self):
        mod = load_module()
        state = mod.VisibleState(transcript="", last_reply="", turn=10, pc_name="Evelyn Hart")
        decision = {
            "response_contract": {"intent": "systematic_ground_floor_search"}
        }

        mod.apply_attempt_marker(decision, state)

        self.assertIn("ground_floor_search", state.attempted)
        self.assertIn("enter_threshold", state.attempted)

    def test_already_open_handspan_live_recap_triggers_threshold_entry_not_second_key_turn(self):
        mod = load_module()
        text = (
            "The door is already open that careful handspan. Stale air leaks out through "
            "the narrow gap, and the first boards inside look dusty but still. Nothing in "
            "the entry lunges or shifts. Evelyn is still at the threshold outside, with "
            "daylight and the quickest exit behind her."
        )
        state = mod.VisibleState(
            transcript="Evelyn already opened the least exposed exterior door a handspan.\n" + text,
            last_reply=text,
            turn=8,
            pc_name="Evelyn Hart",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
                "door_opened",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "enter_threshold_and_map_ground_floor_affordances",
        )
        self.assertIn("steps just inside", action)
        self.assertNotIn("turns the key", action)

    def test_ease_open_only_a_handspan_triggers_threshold_entry_not_second_unlock(self):
        mod = load_module()
        text = (
            "The key turns this time. Not easily, and not with any friendly smoothness; "
            "the mechanism gives resistance first, then yields under steady pressure. "
            "Keeping to the hinge side, you ease it open only a handspan. A thread of "
            "stale interior air slips out at once. Through the narrow opening, the "
            "threshold shows dim floorboards just inside and a strip of interior shadow. "
            "No immediate movement answers you, and the quickest exit remains behind you."
        )
        state = mod.VisibleState(
            transcript="Evelyn opened the least exposed exterior door a handspan.\n" + text,
            last_reply=text,
            turn=8,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
                "door_opened",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "enter_threshold_and_map_ground_floor_affordances",
        )
        self.assertIn("steps just inside", action)
        self.assertNotIn("unlocks the safest exterior door", action)
        self.assertNotIn("turns the key", action)

    def test_open_handspan_side_door_triggers_threshold_entry_not_clarification(self):
        mod = load_module()
        text = (
            "The key turns with slow, reluctant metal movement. You ease the side door inward only "
            "a handspan. From that narrow opening, the immediate threshold shows dim flooring just "
            "inside, dust lying still near the entrance, and a cramped strip of passage beyond the jamb. "
            "Nothing at the threshold immediately forces you back. You are still outside the side door, "
            "with it opened only slightly, your retreat path clear behind you."
        )
        state = mod.VisibleState(
            transcript=(
                "Hall of Records showed Reverend Michael Thomas of the Chapel of Contemplation, "
                "and the Chapel trail includes 1912. "
                + text
            ),
            last_reply=text,
            turn=8,
            pc_name="Eleanor Price",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
                "door_opened",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "enter_threshold_and_map_ground_floor_affordances",
        )
        self.assertNotIn("clarify the single most concrete", decision["sent_to_gm"])

    def test_partly_opened_side_entry_gap_triggers_threshold_entry_not_second_key_turn(self):
        mod = load_module()
        text = (
            "The single most concrete affordance is the partly opened side entry door itself. "
            "Its exact location: immediately in front of Nell at the exterior wall. She is still "
            "outside; the door is only open by about a handspan. What it looks like: weathered "
            "wood, old paint, a dull lockplate, and a narrow black gap into the house. Through "
            "that gap, she can make out only the nearest slice of interior: a dim entry space, "
            "dusty and shut up, with no obvious obstruction pressed against the threshold. "
            "No footsteps, no breathing, no shifting furniture. What it smells like: stale "
            "indoor air, dust, old wood, and long neglect."
        )
        state = mod.VisibleState(
            transcript="Nell already opened the side door a handspan with Knott's key.\n" + text,
            last_reply=text,
            turn=9,
            pc_name='Eleanor "Nell" Ward',
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "house_exterior",
                "door_opened",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "enter_threshold_and_map_ground_floor_affordances",
        )
        self.assertIn("steps just inside", action)
        self.assertNotIn("turns the key", action)

    def test_open_side_doorway_first_step_triggers_entry_not_reclarify(self):
        mod = load_module()
        text = (
            "The opened door she just tested is a real way into the house. It sits at the least "
            "exposed side entrance you identified on your circuit, not the front face of the property. "
            "Daylight is still behind you. The ground outside and the way back off the lot remain clear "
            "enough that, if you turn around now, your retreat is direct: out through this same door, "
            "back along the side approach, and away from the house without having to cross its deeper "
            "interior first. The threshold is open. Beyond it is stale, shut-in interior air and the "
            "first stretch of dim interior space just past the entrance. You can only say it is open "
            "enough to enter, not cleared, not safe, and not explained. You have a side entrance "
            "standing open in front of you, with daylight and an unbroken retreat path at your back."
        )
        state = mod.VisibleState(
            transcript="The exterior survey found and opened a least exposed side entrance.\n" + text,
            last_reply=text,
            turn=8,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "enter_threshold_and_map_ground_floor_affordances",
        )
        self.assertIn("steps just inside", action)
        self.assertNotIn("unlocks", action)
        self.assertNotIn("clarify_latest_visible_affordance", str(decision))

    def test_clarified_bedroom_doorway_triggers_threshold_probe(self):
        mod = load_module()
        text = (
            "From the landing, the most concrete visible affordance is the nearest bedroom doorway. "
            "It opens directly off the landing. From here, the clearest object inside is the bed; "
            "the bedding looks flat, grimy, and long undisturbed. What it sounds like: nothing distinct. "
            "Whether it is safely reachable: yes, in the limited sense that Evelyn can stand at the threshold "
            "with the way back to the landing clear."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=10,
            pc_name="Evelyn Hart",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "probe_clarified_bedroom_threshold",
        )
        self.assertNotIn("clarify the single most concrete", decision["sent_to_gm"])
        self.assertIn("threshold", decision["sent_to_gm"])

    def test_negated_upper_floor_movement_does_not_trigger_bed_threat_retreat(self):
        mod = load_module()
        text = (
            "You keep the landing clear at your back and work from the thresholds. "
            "The upper rooms do not immediately present movement or a rushing threat. "
            "From the doorways, you can make out bedrooms gone stale: beds with coverings "
            "left in neglect, wardrobes standing mute and dark, windows dulled by grime, "
            "and loose papers here and there. Nothing from the first doorway compels a "
            "panicked retreat. Nothing yet moves by itself."
        )
        state = mod.VisibleState(
            transcript=(
                "The ground floor search identified the stair leading up and the way down toward the basement.\n"
                + text
            ),
            last_reply=text,
            turn=8,
            pc_name="Evelyn Mercer",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
            },
        )

        self.assertFalse(mod.moving_bedroom_threat_visible(text))
        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "probe_clarified_bedroom_threshold",
        )
        self.assertNotEqual(
            decision["response_contract"]["intent"],
            "retreat_from_moving_bedroom_threat",
        )

    def test_explicit_no_upstairs_object_movement_does_not_trigger_bed_threat_retreat(self):
        mod = load_module()
        text = (
            "[roll]Inspect the stairs and upstairs rooms from the doorway for recent disturbance, "
            "unsafe footing, and anything that moved on its own: 1d100 = 59 vs 80, strong success.[/roll]\n\n"
            "At the top, the landing stays usable as a retreat line behind you. "
            "From the doorways, several things become clear. The furniture and bedding upstairs have not "
            "been neatly kept, but neither do they read like a place recently lived in. "
            "Loose papers lie where neglect left them. More importantly, your caution does pay off: "
            "nothing in the immediate upstairs survey suggests you are about to be trapped in a room "
            "if you keep to the landing and thresholds. No bedclothes twitch. No wardrobe door swings by itself. "
            "No object moves on its own while you watch. You remain on the upper floor with the landing behind you clear."
        )
        state = mod.VisibleState(
            transcript=(
                "The ground floor search identified the stair leading up and the way down toward the basement.\n"
                + text
            ),
            last_reply=text,
            turn=10,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "upper_floor_search",
            },
        )

        self.assertFalse(mod.moving_bedroom_threat_visible(text))
        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertNotEqual(
            decision["response_contract"]["intent"],
            "retreat_from_moving_bedroom_threat",
        )
        self.assertNotIn("impossible movement", decision["sent_to_gm"].lower())

    def test_chapel_success_area_question_does_not_trigger_bedroom_threat(self):
        mod = load_module()
        text = (
            "By the time there is enough light to trust your eyes, the drive out to the Chapel of "
            "Contemplation feels lonelier than the city did. The place stands in a condition beyond "
            "mere neglect: weather-stained, sagging, and half surrendered to time. "
            "[roll]Careful survey and search of the Chapel of Contemplation grounds and surviving "
            "interior for signs of recent use, records, symbols, and links to Reverend Michael Thomas "
            "or Corbitt: 1d100 = 51 vs 60 — success.[/roll] "
            "You make a slow, deliberate pass around the exterior and come away with a usable sense "
            "of which portions of the structure look merely rotten versus actively dangerous. "
            "Inside, the air is stale with dust, damp wood, and old plaster. Daylight leaks through "
            "cracks and broken panes in narrow strips. You move slowly, photographing what stands out "
            "before touching it. Offices and storage spaces have been left in disorder by age more than "
            "by recent hands: warped drawers, mildewed papers, collapsed shelving, scraps too far gone "
            "to read at a glance, and the remains of church fittings. "
            "Your search is productive in one important way even before it yields a clean documentary "
            "answer: you are able to separate old ruin from anything that looks newly disturbed. "
            "If there is a trail connecting this place to Corbitt or to Michael Thomas, it is buried "
            "in remnants rather than sitting out in the open. "
            "You can keep pressing deeper into the surviving records and debris from here, but the work "
            "will take patience and care rather than a quick glance. What area do you focus on first: "
            "the offices, the pulpit and worship space, or storage and loose papers?"
        )
        state = mod.VisibleState(
            transcript=(
                "The newspaper archive connected Michael Thomas to the Chapel of Contemplation.\n"
                + text
            ),
            last_reply=text,
            turn=6,
            pc_name="Evelyn Hart",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
            },
        )

        self.assertFalse(mod.moving_bedroom_threat_visible(text))
        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "continue_chapel_search_after_scene_establishment",
        )
        self.assertNotEqual(
            decision["response_contract"]["intent"],
            "retreat_from_moving_bedroom_threat",
        )

    def test_absent_burial_statement_does_not_create_burial_fact(self):
        mod = load_module()
        text = (
            "The chapel search failed. Nothing here confirms a burial clue, "
            "but the official trail still points back to the Corbitt House."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=5,
            pc_name="Evelyn Pierce",
        )

        facts = mod.facts_from_visible(state)

        self.assertFalse(any("burial" in fact.lower() for fact in facts))

    def test_negated_burial_note_after_chapel_symbol_does_not_create_burial_fact(self):
        mod = load_module()
        text = (
            "[roll]Pushed Spot Hidden search of the Chapel of Contemplation: "
            "1d100 = 18 vs Spot Hidden 70 -- Hard Success.[/roll] "
            "The low-angled flashlight reveals a repeated symbol worked into the place by human hands. "
            "It is not a full record, burial note, or explanatory text. "
            "But it is a concrete chapel clue at last: a specific symbol tied to the Chapel of Contemplation."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=14,
            pc_name="Evelyn Mercer",
        )

        facts = mod.facts_from_visible(state)

        self.assertFalse(mod.explicit_burial_clue_visible(text))
        self.assertFalse(any("burial" in fact.lower() for fact in facts))

    def test_buried_in_routine_does_not_create_burial_fact(self):
        mod = load_module()
        text = (
            "What you do get, and only because it is already too buried in routine "
            "to be worth guarding, is a dusty file index. Knott once guessed that "
            "one key might fit bedroom doors, perhaps a cellar door."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=3,
            pc_name="Evelyn Pierce",
        )

        facts = mod.facts_from_visible(state)

        self.assertFalse(any("burial" in fact.lower() for fact in facts))

    def test_buried_in_records_maze_does_not_create_burial_fact(self):
        mod = load_module()
        text = (
            "The deeper thread remains buried in the records maze, and the clerk "
            "suggests court files or Central Police Station for anything serious."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=3,
            pc_name="Evelyn Pierce",
        )

        facts = mod.facts_from_visible(state)

        self.assertFalse(any("burial" in fact.lower() for fact in facts))

    def test_chinese_chapel_burial_statement_creates_burial_fact(self):
        mod = load_module()
        text = (
            "[roll]Search the ruined Chapel records for Corbitt links: "
            "1d100=[24] target 60, result strong_success[/roll]\n"
            "这里最具体的一条记录写得很清楚：沃尔特·科比特依照他本人的意愿，"
            "被埋在他那栋房子的地下室里。教堂自己的记录又把科比特，直接指回了那栋房子的地下室。"
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=5,
            pc_name="Evelyn Pierce",
        )

        facts = mod.facts_from_visible(state)

        self.assertTrue(mod.explicit_burial_clue_visible(text))
        self.assertTrue(
            any("burial" in fact.lower() or "basement" in fact.lower() for fact in facts)
        )

    def test_chapel_cabinet_burial_record_returns_to_marked_basement_not_clarify(self):
        mod = load_module()
        text = (
            "[roll]Careful examination of the chapel cabinet -- 1d100: 4 vs Spot Hidden 70 -- "
            "Extreme success.[/roll] The cabinet opens without collapse or violent movement. "
            "Inside, among the surviving records, two things stand out. One is a symbol preserved "
            "in the chapel papers. The other is far more important: a record stating that Walter "
            "Corbitt was buried in the basement of his house, in accordance with his wishes. "
            "The ruin stays quiet around you. The cabinet has yielded what it had without forcing "
            "you into a worse risk."
        )
        state = mod.VisibleState(
            transcript=(
                "Earlier, Evelyn marked the Corbitt House basement stairs, opened a concealed "
                "crawl-space, copied the words Chapel of Contemplation, and withdrew to investigate "
                "the chapel in daylight.\n"
                + text
            ),
            last_reply=text,
            turn=16,
            pc_name="Evelyn Mercer",
            attempted={
                "hall_records",
                "newspaper_archive",
                "chapel",
                "second_chapel_method",
                "chapel_cabinet_probe",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
                "basement_followup",
                "crawl_space_probe",
                "crawl_space_entry",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "return_to_marked_basement_after_chapel_burial_record",
        )
        self.assertIn("basement", action)
        self.assertIn("buried", action)
        self.assertIn("marked", action)
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_chinese_chapel_burial_statement_goes_to_house_not_repeat_chapel(self):
        mod = load_module()
        text = (
            "[roll]Search the ruined Chapel records for Corbitt links: "
            "1d100=[24] target 60, result strong_success[/roll]\n"
            "这里最具体的一条记录写得很清楚：沃尔特·科比特依照他本人的意愿，"
            "被埋在他那栋房子的地下室里。教堂自己的记录又把科比特，直接指回了那栋房子的地下室。"
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=6,
            pc_name="Evelyn Pierce",
            attempted={"hall_records", "official_records", "newspaper_archive", "chapel"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "inspect_house_exterior_and_safe_entry",
        )
        self.assertNotIn("continue_chapel_search", json.dumps(decision, ensure_ascii=False))
        self.assertNotIn("last reply as arrival", json.dumps(decision, ensure_ascii=False).lower())

    def test_house_exterior_after_failed_chapel_does_not_use_unseen_burial_term(self):
        mod = load_module()
        text = (
            "[roll]Pushed search of the Chapel of Contemplation — 1d100: 78 vs 75 — Failure.[/roll] "
            "No fresh chapel clue won here. "
            "The Michael Thomas / Chapel of Contemplation / 1912 trail remains known. "
            "What you already established still holds: the official trail points "
            "back to Walter Corbitt and, more importantly, to the Corbitt House."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=5,
            pc_name="Evelyn Pierce",
            attempted={"hall_records", "official_records", "chapel", "second_chapel_method"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "inspect_house_exterior_and_safe_entry",
        )
        self.assertNotIn("burial", decision_text)
        self.assertNotIn("buried", decision_text)

    def test_house_exterior_after_public_record_failures_does_not_claim_unvisited_chapel(self):
        mod = load_module()
        text = (
            "[roll]Boston Globe morgue search for Corbitt House clippings: "
            "1d100=[99] target 60, result strong_failure[/roll] "
            "The public archive gives no usable date, name, incident, or next lead. "
            "The Corbitt House itself remains the commissioned site."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=4,
            pc_name="Evelyn Ward",
            attempted={"hall_records", "newspaper_archive"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "inspect_house_exterior_and_safe_entry",
        )
        self.assertNotIn("chapel did not give", decision_text)
        self.assertNotIn("no chapel clue", decision_text)

    def test_entry_ready_does_not_jump_to_upstairs_from_old_bedroom_key_guess(self):
        mod = load_module()
        transcript = (
            "Knott guesses one key might fit bedroom doors, perhaps a cellar door.\n"
            "The Michael Thomas / Chapel of Contemplation / 1912 trail remains known.\n"
            "GM: When Evelyn tests the least exposed matching lock, the key fits. "
            "You have not committed yourself inside yet, but you now have a workable "
            "entrance. The door stands ready, the stale interior air just beginning "
            "to press at the seam."
        )
        state = mod.VisibleState(
            transcript=transcript,
            last_reply=(
                "When Evelyn tests the least exposed matching lock, the key fits. "
                "You have not committed yourself inside yet, but you now have a workable "
                "entrance. The door stands ready, the stale interior air just beginning "
                "to press at the seam."
            ),
            turn=6,
            pc_name="Evelyn Pierce",
            attempted={"hall_records", "official_records", "chapel", "second_chapel_method", "house_exterior"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "enter_threshold_and_map_ground_floor_affordances",
        )

    def test_key_there_it_fits_threshold_triggers_entry_not_clarification_loop(self):
        mod = load_module()
        text = (
            "When you try Knott's key there, it fits the house's old practical logic. "
            "The threshold remains before you, stale and still."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=4,
            pc_name="Evelyn Marsh",
            attempted={"hall_records", "newspaper_archive", "house_exterior"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "enter_threshold_and_map_ground_floor_affordances",
        )

    def test_confirmed_key_fit_with_remaining_lock_uncertainty_tests_door_next(self):
        mod = load_module()
        text = (
            "What you can actually perceive is this: the least exposed entrance is right "
            "in front of you, close enough to touch, and the key you chose does fit that lock. "
            "You felt it seat properly in the cylinder. From where you stand, this door is "
            "the nearest concrete way into the house, and it sits directly between you and "
            "the interior while leaving open ground at your back if you want to step away fast. "
            "What remains uncertain is whether the lock will turn freely, whether the hinges "
            "will complain, and what the first strip of interior floor will show."
        )
        state = mod.VisibleState(
            transcript="The exterior survey found the least exposed matching lock.\n" + text,
            last_reply=text,
            turn=7,
            pc_name="Evelyn Hart",
            attempted={"hall_records", "official_records", "chapel", "house_exterior"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "enter_threshold_and_map_ground_floor_affordances",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)
        self.assertNotIn("ground the latest actionable thing", decision_text)

    def test_unlocked_shut_door_with_retreat_triggers_entry_not_grounding_loop(self):
        mod = load_module()
        text = (
            "From here, Nell can honestly say this much: The door in front of her is shut "
            "but unlocked. The key turned cleanly enough to prove it belongs here. The "
            "threshold itself looks still. Nothing is moving in the crack of the frame, "
            "and the air leaking around it has that flat, stale quality of a place kept "
            "closed too long. Her retreat path is good; if she abandons the attempt the "
            "moment the door opens, she can withdraw the same way she came."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=6,
            pc_name="Nell",
            attempted={"hall_records", "newspaper_archive", "house_exterior"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "enter_threshold_and_map_ground_floor_affordances",
        )
        self.assertNotIn("ground the latest actionable thing", decision_text)

    def test_shut_not_sealed_door_with_keys_and_retreat_triggers_entry_not_reclarify(self):
        mod = load_module()
        text = (
            "From where you are now, the most concrete thing in front of you is this: "
            "The house is shut, but not sealed off from you. You have a door close enough "
            "to test with Knott's keys, and you already made sure that if it goes badly, "
            "your way back is open behind you. Your retreat path is the same one you "
            "preserved on purpose: away from the lock, back across the exposed ground "
            "you already crossed, and out toward open daylight rather than deeper into "
            "the property. What you can actually perceive is limited but useful."
        )
        state = mod.VisibleState(
            transcript="The exterior survey found the least exposed testable lock.\n" + text,
            last_reply=text,
            turn=4,
            pc_name="Ruth Caldwell",
            attempted={"hall_records", "house_exterior"},
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "enter_threshold_and_map_ground_floor_affordances",
        )
        self.assertNotIn("clarify_latest_visible_affordance", decision_text)

    def test_basement_route_fact_does_not_claim_burial_from_player_search_terms(self):
        mod = load_module()
        transcript = (
            "PLAYER: Evelyn searches the ground floor and watches for drafts, cellar smells, "
            "scrape marks, footprints, or a route upstairs or downstairs.\n"
            "GM: The ground floor gives surface facts, not answers. You confirm the upstairs "
            "route from within, and you also identify the downward way: a basement access "
            "from the ground floor, accompanied by faint cellar taint.\n"
            "GM: You connect these observations: a marked return path to the exterior; "
            "the interior route upstairs; and a definite route down into the basement."
        )
        state = mod.VisibleState(
            transcript=transcript,
            last_reply=transcript,
            turn=7,
            pc_name="Evelyn Parker",
            attempted={"enter_threshold"},
        )

        facts = mod.facts_from_visible(state)

        self.assertTrue(any("cellar or basement route" in fact.lower() for fact in facts))
        self.assertFalse(any("burial" in fact.lower() for fact in facts))

    def test_ground_floor_success_with_strong_basement_route_descends_before_upstairs(self):
        mod = load_module()
        text = (
            "[roll]Methodical search of the Corbitt House ground floor: 1d100 = 17 "
            "vs target 60 — Hard Success.[/roll] "
            "You identify the interior ways onward: a staircase leading up from the "
            "ground floor and a downward way toward the basement, matching the lead "
            "from the chapel records. The cellar direction is the one that best fits "
            "the faint damp-earth smell. The basement route now stands out as the "
            "most meaningful lead inside."
        )
        state = mod.VisibleState(
            transcript=(
                "The chapel record states Walter Corbitt was buried in the basement "
                "of his house.\n" + text
            ),
            last_reply=text,
            turn=8,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "safe_basement_descent_and_search",
        )
        self.assertNotIn("upstairs", decision["sent_to_gm"].lower())

    def test_ground_floor_extreme_success_with_basement_stands_out_descends_before_upstairs(self):
        mod = load_module()
        text = (
            "[roll]Spot Hidden -- Search the ground floor room by room for disturbances, "
            "documents, drafts, and access routes: 1d100 = 11 vs target 70. Extreme success.[/roll] "
            "One current is especially telling: a thinner, cooler pull that suggests air moving "
            "through an interior vertical route rather than only from outside. Your search clarifies "
            "the house's routes. You now have the staircase up from the ground floor, and a more "
            "suspicious downward access, marked not just by architecture but by the faint cellar "
            "taint you were watching for--cooler air, trapped damp, and that particular below-house "
            "smell that does not belong to the upper rooms. The basement route stands out more "
            "strongly than it did from the entry. The lower part of the house may matter more than "
            "the front rooms first suggested."
        )
        state = mod.VisibleState(
            transcript=text,
            last_reply=text,
            turn=5,
            pc_name="Evelyn Pierce",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "safe_basement_descent_and_search",
        )
        self.assertNotIn("upstairs", decision["sent_to_gm"].lower())

    def test_ground_floor_failure_with_known_basement_route_descends_before_upstairs(self):
        mod = load_module()
        text = (
            "[roll]Methodical ground-floor search of the Corbitt House — 1d100: 97 "
            "vs target 50 — failure.[/roll] "
            "You do identify the obvious interior routes onward—the stairs up, and "
            "the way down toward the basement—but you do not, on this pass, turn up "
            "a reliable hidden sign. Nothing attacks you. Nothing visibly moves. "
            "The open way back remains behind you."
        )
        state = mod.VisibleState(
            transcript=(
                "Knott's keys included possible cellar access. The exterior survey "
                "noted the cellar way as a distinct point of entry.\n" + text
            ),
            last_reply=text,
            turn=8,
            pc_name="Evelyn Pierce",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")

        self.assertEqual(
            decision["response_contract"]["intent"],
            "safe_basement_descent_and_search",
        )

    def test_clarified_way_down_beats_upstairs_after_ground_floor_confusion(self):
        mod = load_module()
        text = (
            "What you can **actually** act on from here is simple: The house has "
            "stopped being just a creepy place and become a set of choices you can "
            "physically see. On this floor, the clearest one is the way **down**: "
            "the basement door or stair access is a real, present route, not a theory. "
            "It is part of the ground floor space you've already been working through, "
            "and it is not beyond your retreat path. The stair upward is still visible, "
            "but the way down is the grounded affordance."
        )
        state = mod.VisibleState(
            transcript=(
                "The first ground-floor pass was confused; stairs upward and adjoining "
                "rooms were visible.\n" + text
            ),
            last_reply=text,
            turn=9,
            pc_name="Evelyn Price",
            attempted={
                "hall_records",
                "official_records",
                "newspaper_archive",
                "chapel",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "safe_basement_descent_and_search",
        )
        self.assertIn("basement", action)
        self.assertNotIn("upstairs", action)

    def test_failed_basement_search_clarified_return_route_withdraws_not_followup(self):
        mod = load_module()
        transcript = (
            "[roll]Cautious descent and basement sweep: 1d100 = 62 vs Spot Hidden 60 — Failure.[/roll]\n"
            "Somewhere below, you catch the smell of disturbed earth or old damp brick—but "
            "not cleanly enough to tell from where. You have not identified a hidden panel, "
            "a body, a ritual object, or any clear moving threat.\n"
            "GM: The single most concrete, presently established affordance is the basement "
            "stair behind you—your marked return route."
        )
        last_reply = (
            "The single most concrete, presently established affordance is the basement stair "
            "behind you—your marked return route. Exact location: immediately behind and "
            "slightly above her, rising back to the ground floor through the stair she just "
            "descended. Safely reachable from your current position? Yes—it is the one route "
            "here that is already confirmed, legible, and immediately reachable without "
            "pressing deeper into the basement. What you do not yet have is a confirmed "
            "hidden panel, body, ritual object, moving figure, or identified side passage."
        )
        state = mod.VisibleState(
            transcript=transcript,
            last_reply=last_reply,
            turn=10,
            pc_name="Evelyn Parker",
            attempted={
                "hall_records",
                "newspaper_archive",
                "house_exterior",
                "enter_threshold",
                "ground_floor",
                "upper_floor",
                "basement_descent",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        decision_text = json.dumps(decision, ensure_ascii=False).lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "withdraw_from_failed_basement_search_to_marked_stairs",
        )
        self.assertNotIn("last search produced a concrete lead", decision_text)
        sent = decision["sent_to_gm"].lower()
        self.assertNotIn("photographs the disturbed earth", sent)
        self.assertNotIn("suspicious concealed section", sent)

    def test_failed_descent_to_lower_stairs_recovers_position_not_upstairs(self):
        mod = load_module()
        text = (
            "[roll]Cautious descent into the cellar while searching for hidden disturbance "
            "or immediate threat: 1d100 = 81 vs Spot Hidden 65 — Failure.[/roll] "
            "The handkerchief on the door handle gives you a pale point of reassurance "
            "behind you, and the wedged cellar door leaves a narrow seam of safer air above. "
            "Then you descend, one careful step at a time, testing before trusting. "
            "The upper steps hold. The air grows cooler and wetter as you go. Your flashlight "
            "picks out rough wall, shadowed treads, and the suggestion of packed earth below, "
            "but the angle and the gloom keep swallowing detail just past the edge of the beam. "
            "Nothing attacks you. You have not confirmed any loose brick, hidden panel, scrape "
            "trail, disturbed earth, body, or ritual object. What you do know is narrower but "
            "real: the route back remains marked and open behind you, the stairs have held so "
            "far, and the cellar proper lies just ahead of your light."
        )
        state = mod.VisibleState(
            transcript="The ground floor search already identified both upstairs and cellar routes.\n" + text,
            last_reply=text,
            turn=8,
            pc_name="Evelyn Marsh",
            attempted={
                "hall_records",
                "house_exterior",
                "enter_threshold",
                "ground_floor_search",
                "basement_descent",
            },
        )

        decision = mod.choose_next_decision(state, "cautious_investigator")
        action = decision["sent_to_gm"].lower()

        self.assertEqual(
            decision["response_contract"]["intent"],
            "recover_from_failed_basement_stair_descent",
        )
        self.assertIn("basement", action)
        self.assertNotIn("upstairs", action)


if __name__ == "__main__":
    unittest.main()

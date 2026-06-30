# Negative Fixture: coc_10turn

This compact structured excerpt captures the known CoC-style failure where a
successful investigation says that information exists but withholds the success
payload.

```json eval-fixture
{
  "fixture_id": "negative.coc_10turn",
  "title": "CoC 10-turn bad transcript excerpt",
  "turns": [
    {
      "turn": 4,
      "player_decision": {
        "perceived_facts": [
          "The old building may contain visible clues."
        ],
        "active_goal": "learn concrete details from observing the old building",
        "declared_action": "我仔细观察旧建筑的细部，找出任何不寻常的线索。",
        "response_contract": {
          "intent": "observe_old_building_details",
          "requested_information": ["specific visible detail", "concrete clue", "reason nothing can be seen"],
          "acceptable_resolutions": ["specific detail revealed", "clue revealed", "observation check requested", "clear obstruction with next step"],
          "unacceptable": ["says the character sees many details but names none"]
        }
      },
      "gm_response": {
        "text": "困难成功。你看清了很多细部，旧建筑的状态比第一眼所见更加复杂，似乎确实隐藏着值得注意的地方。",
        "resolution_status": "resolved",
        "check_outcome": "success",
        "facts_added": [],
        "facts_exposed_to_player": [],
        "state_delta": [],
        "choices_opened": [],
        "pending": []
      },
      "trace": {
        "mechanical_debt_markers": [],
        "pending_id": null,
        "rolls": [
          {
            "action": "observe_old_building_details",
            "skill": "Spot Hidden",
            "die": "1d100",
            "die_result": 21,
            "total": 21,
            "target": "hard",
            "outcome": "success",
            "source_refs": ["coc7e:spot_hidden"],
            "state_delta": []
          }
        ]
      }
    },
    {
      "turn": 5,
      "player_decision": {
        "perceived_facts": [
          "The NPC is hiding something."
        ],
        "active_goal": "learn what the NPC is hiding",
        "declared_action": "我压低声音问他，他到底在隐瞒什么。",
        "response_contract": {
          "intent": "interrogate_hidden_information",
          "requested_information": ["hidden_topic", "reason_for_fear", "concrete_lead_or_refusal"],
          "acceptable_resolutions": ["NPC says the concrete secret", "NPC refuses with clear reason or cost", "social check requested", "failed check consequence"],
          "unacceptable": ["only says he is hiding something", "only describes facial expression"]
        }
      },
      "gm_response": {
        "text": "成功。他的表情明显僵住了，你确认他确实在隐瞒什么，而且这件事让他非常害怕。",
        "resolution_status": "resolved",
        "check_outcome": "success",
        "facts_added": ["npc_is_hiding_something"],
        "facts_exposed_to_player": ["npc_is_hiding_something"],
        "state_delta": [],
        "choices_opened": [],
        "pending": []
      },
      "trace": {
        "mechanical_debt_markers": [],
        "pending_id": null,
        "rolls": [
          {
            "action": "interrogate_hidden_information",
            "skill": "Psychology",
            "die": "1d100",
            "die_result": 32,
            "total": 32,
            "target": "regular",
            "outcome": "success",
            "source_refs": ["coc7e:psychology"],
            "state_delta": []
          }
        ]
      }
    }
  ]
}
```

# Negative Fixture: cyber_50turn

This is a compact structured excerpt of the known bad Cyberpunk battle report.
It represents the failure modes the offline evaluator must catch before live
autoplay is trusted again.

```json eval-fixture
{
  "fixture_id": "negative.cyber_50turn",
  "title": "Cyberpunk 50-turn bad transcript excerpt",
  "turns": [
    {
      "turn": 43,
      "player_decision": {
        "perceived_facts": [
          "The boss is physically threatened at gunpoint.",
          "Stopping the firefight matters now."
        ],
        "active_goal": "force the boss to order a ceasefire",
        "declared_action": "我用枪顶住头目的脑袋，逼他立刻下令让手下停火。",
        "response_contract": {
          "intent": "coerce_ceasefire_order",
          "requested_information": ["whether the boss complies", "whether the firefight stops"],
          "acceptable_resolutions": ["boss orders ceasefire", "boss refuses with consequence", "social check requested", "violent backlash resolved", "pending with pending_id"],
          "unacceptable": ["only describes tension", "says the situation is unresolved without pending_id"]
        }
      },
      "gm_response": {
        "text": "头目的喉结滚动，仓库里的枪声像潮水一样压在耳边。他的眼神闪烁，局势仍然悬着，所有人都在等待下一秒会发生什么。",
        "resolution_status": "invalid",
        "facts_added": [],
        "facts_exposed_to_player": [],
        "state_delta": [],
        "choices_opened": [],
        "pending": []
      },
      "trace": {
        "mechanical_debt_markers": ["局势仍然悬着", "等待下一秒"],
        "pending_id": null,
        "rolls": []
      }
    },
    {
      "turn": 49,
      "player_decision": {
        "perceived_facts": [
          "The boss still has not given a concrete ceasefire order."
        ],
        "active_goal": "force the boss to order a ceasefire",
        "declared_action": "我继续用枪压住他，重复要求他马上喊停火。",
        "response_contract": {
          "intent": "coerce_ceasefire_order",
          "requested_information": ["whether the boss complies", "whether the firefight stops"],
          "acceptable_resolutions": ["boss orders ceasefire", "boss refuses with consequence", "social check requested", "violent backlash resolved", "pending with pending_id"],
          "unacceptable": ["only describes tension", "repeats the same unresolved standoff"]
        }
      },
      "gm_response": {
        "text": "他喘着气，汗水顺着脸颊往下滑，枪口的压力让空气紧绷起来。枪声没有真正停下，仓库内外都像被按在临界点。",
        "resolution_status": "invalid",
        "facts_added": [],
        "facts_exposed_to_player": [],
        "state_delta": [],
        "choices_opened": [],
        "pending": []
      },
      "trace": {
        "mechanical_debt_markers": ["枪声没有真正停下", "临界点"],
        "pending_id": null,
        "rolls": []
      }
    },
    {
      "turn": 57,
      "player_decision": {
        "perceived_facts": [
          "Repeated coercion has not changed the fight."
        ],
        "active_goal": "force the boss to order a ceasefire",
        "declared_action": "我把枪更用力地顶住他，逼他现在就下停火命令。",
        "response_contract": {
          "intent": "coerce_ceasefire_order",
          "requested_information": ["whether the boss complies", "whether the firefight stops"],
          "acceptable_resolutions": ["boss orders ceasefire", "boss refuses with consequence", "social check requested", "violent backlash resolved", "pending with pending_id"],
          "unacceptable": ["only describes tension", "leaves result waiting"]
        }
      },
      "gm_response": {
        "text": "他的嘴唇动了动，却被爆响吞没。你能感觉到这一刻随时可能翻盘，但真正的结果还没有落定。",
        "resolution_status": "pending",
        "facts_added": [],
        "facts_exposed_to_player": [],
        "state_delta": [],
        "choices_opened": [],
        "pending": []
      },
      "trace": {
        "mechanical_debt_markers": ["结果还没有落定"],
        "pending_id": null,
        "rolls": []
      }
    },
    {
      "turn": 70,
      "player_decision": {
        "perceived_facts": [
          "The player is checking the boss name against records."
        ],
        "active_goal": "verify the boss identity",
        "declared_action": "我用资料库核对头目的名字，确认这个名字是真是假。",
        "response_contract": {
          "intent": "cross_check_boss_name",
          "requested_information": ["truth status of boss name"],
          "acceptable_resolutions": ["name verified", "name disproved", "Library Search check resolved", "pending with pending_id"],
          "unacceptable": ["hidden meta says pending but report claims no pending"]
        }
      },
      "gm_response": {
        "text": "资料库窗口亮起，搜索结果似乎正在逼近答案。屏幕上还有几个匹配项等待进一步确认。",
        "resolution_status": "pending",
        "facts_added": [],
        "facts_exposed_to_player": [],
        "state_delta": [],
        "choices_opened": [],
        "pending": []
      },
      "trace": {
        "mechanical_debt_markers": ["等待进一步确认"],
        "pending_id": null,
        "rolls": [
          {
            "action": "cross_check_boss_name",
            "skill": "Library Search",
            "die": "1d10",
            "die_result": null,
            "total": null,
            "target": "DV13",
            "outcome": null,
            "source_refs": [],
            "state_delta": []
          }
        ]
      }
    }
  ]
}
```

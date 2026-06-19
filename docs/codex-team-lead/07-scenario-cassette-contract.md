# Scenario and Replay Cassette Contract

## One scenario, multiple executors

The scenario is the source of truth. Deterministic, live-model, and replay runs must consume the same scenario ID/version and produce the same evidence schema.

## Scenario fields

```yaml
id: JRNY-HOME-01
version: 1
acceptance_ids: [DA-KNOW-02, DA-KNOW-05, DA-SPOIL-01, DA-SPOIL-02, DA-SPOIL-04]
ruleset_id: cyberpunk_red
module_id: cyberpunk_red.homecoming

setup:
  database: isolated
  assets: current_verified_bundle
  character:
    create_via: cli
    preferences: "技术型角色，擅长观察、电子和维修"
    auto_complete: true
    require_persisted: true
    require_session_binding: true
  session:
    start_via: created_character_session
    require_playable_scene: true

player:
  mode: adaptive_script
  knowledge_source: player_view_only
  max_trigger_attempts: 2

chapters:
  - id: first_observation
    when: session_ready
    action: "我先观察现场，不靠近，看看无人机、警察和仓库门口分别是什么情况。"
    trigger:
      reachability: [turn_completed]
      mechanism: [ContextSurfaced, PlayerExposed]
      forbid_mechanism: [PlayerLearnedFact:athena_hidden_truth]
    assert:
      player_text_forbids: [hidden_fact_terms]
      state: [player_does_not_know_hidden_fact]

  - id: technical_interaction
    when: first_observation_passed
    actions:
      - "我检查无人机背后的线缆，看它接到哪里。"
      - if_not_triggered: "我实际拆开护盖，判断能不能安全切断，并开始操作。"
    trigger:
      reachability: [technical_intent]
      mechanism_one_of: [AwaitingPlayerRoll, GmRollStarted]
      require_dice: true
      require_check_resolution: true

  - id: resolve_or_branch
    when: check_resolved
    branch:
      success:
        assert: [player_learned_fact, later_narration_may_use_fact]
      failure:
        assert: [fact_remains_hidden]
        next_action: "我先停手，去找懂这套设备的人问问。"

  - id: continuity
    when: reveal_obtained
    restart_process: true
    action: "根据我刚才发现的东西，我现在能确认这台无人机的什么情况？"
    assert:
      state: [knowledge_survives_reload]
      player_text_requires_semantic: [revealed_fact_summary]
```

## Evidence bundle

Every run writes:

```text
run_manifest.json
character_creation/events.jsonl
character_creation/player_view.json
session_start.json
each_step/player_input.txt
each_step/player_visible_output.txt
each_step/turn_events.jsonl
each_step/turn_trace.json
each_step/db_readonly_snapshot.json
each_step/evidence_chain.json
final_projection.json
journey_result.json
```

## Replay cassette

A cassette captures only external nondeterminism:

```text
LLM request class + ordered response/tool-call deltas
embedding/rerank responses when external
random seed or dice outputs
clock values when relevant
```

It must not capture and restore final DB state. Replay starts from the scenario setup and reproduces the journey through the real runtime.

Cassette rules:

- keyed by scenario version and ordered call index/type;
- unexpected calls fail;
- missing calls fail;
- no live fallback in replay mode;
- secrets remain in evaluator/private artifacts, never player input;
- a cassette is promoted only from an accepted live run.

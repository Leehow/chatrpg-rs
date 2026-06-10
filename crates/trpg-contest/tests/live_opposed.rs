//! DB-gated:验证对抗 check(target_actor + opponent_tested_parameter)经 contest 出真胜负。
//! 需 DATABASE_URL 指向含 CoC 测试角色 + 一个带合成防御技能的 NPC 卡的库;否则 SKIP。
//!
//! Run:  DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
//!       cargo test -p trpg-contest --test live_opposed -- --nocapture

use chrono::Utc;
use serde_json::json;
use trpg_contest::ContestService;
use trpg_db::Db;
use trpg_model::*;

const SESSION: &str = "session_6def47593a094513a75b01b69a52c986";
const RULESET: &str = "call_of_cthulhu_7e";

fn opposed_contract() -> CheckContract {
    CheckContract {
        check_id: format!("check_opp_{}", uuid::Uuid::new_v4().simple()),
        session_id: SESSION.into(),
        turn_id: "turn_test".into(),
        ruleset_id: RULESET.into(),
        module_id: None,
        initiator: ActorRef {
            actor_id: "pc.current".into(),
            actor_kind: ActorKind::PlayerCharacter,
            display_name: None,
        },
        target_actor: Some(ActorRef {
            actor_id: "npc.opposition".into(),
            actor_kind: ActorKind::Npc,
            display_name: Some("拉斯".into()),
        }),
        opposition: OppositionModel::NoMechanicalOpposition,
        action_summary: "潜行绕过拉斯".into(),
        intent_kind: "hide".into(),
        check_label: "潜行 check".into(),
        dice_expression: "1d100".into(),
        modifiers: vec![],
        target: CheckTargetModel::UnknownUntilLookup,
        tested_parameter: Some(TestedParameter {
            domain: None,
            key: "Stealth".into(),
            label: "Stealth".into(),
        }),
        // NPC「拉斯」的对抗防御技能在 Phase 2 被现搓成 perception(非 Spot Hidden);
        // 这里的键必须与当前夹具 npc.opposition.mechanical_profile.skills 对齐,
        // 否则 derive_tested_source 匹配不到 → defender_value 为 null。skip-gate 已接受
        // perception/Spot Hidden,此处补齐契约键完成迁移。零硬编码、fail-closed。
        opponent_tested_parameter: Some(TestedParameter {
            domain: None,
            key: "perception".into(),
            label: "perception".into(),
        }),
        actor_snapshot_ids: vec![],
        source_refs: vec![],
        learned_packet_ids: vec![],
        roll_visibility: RollVisibility::PublicGmRoll,
        roll_authority: RollAuthority::System,
        disclosure: RollDisclosurePolicy::for_visibility(RollVisibility::PublicGmRoll),
        stakes: CheckStakes::default(),
        confidence: RulingConfidence::Medium,
        ruling_status: RulingStatus::SourceBacked,
        advice_refs: vec![],
        expires_at_turn: None,
    }
}

fn roll(total: i64) -> DiceRollRecord {
    DiceRollRecord {
        roll_id: format!("roll_test_{}", uuid::Uuid::new_v4().simple()),
        session_id: SESSION.into(),
        turn_id: "turn_test".into(),
        check_id: None,
        roller_kind: ActorKind::PlayerCharacter,
        roller_id: Some("pc.current".into()),
        visibility: RollVisibility::PublicGmRoll,
        expression: "1d100".into(),
        result: json!({"total": total, "rolls": [total]}),
        seed_commitment: String::new(),
        revealed_at: None,
        created_at: Utc::now(),
    }
}

#[tokio::test]
async fn opposed_check_produces_a_verdict_when_both_values_present() {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; }
    };
    let db = match Db::connect(&url).await {
        Ok(d) => d,
        Err(e) => { eprintln!("SKIP: connect failed: {e}"); return; }
    };

    // 检查 pc.current 有 Stealth 技能
    let pc_params = trpg_params::RuntimeParameterService::new(db.clone())
        .load_actor_parameters(SESSION, "pc.current").await.ok().flatten();
    let npc_params = trpg_params::RuntimeParameterService::new(db.clone())
        .load_actor_parameters(SESSION, "npc.opposition").await.ok().flatten();

    let pc_has_stealth = pc_params.as_ref()
        .and_then(|p| p.mechanical_profile.get("skills"))
        .and_then(|s| s.get("Stealth"))
        .is_some();
    let npc_has_perception = npc_params.as_ref()
        .and_then(|p| p.mechanical_profile.get("skills"))
        .and_then(|s| {
            // 接受 Spot Hidden 或 perception/Perception
            s.as_object().and_then(|m| {
                m.keys().find(|k| {
                    let kl = k.to_ascii_lowercase();
                    kl == "spot hidden" || kl == "perception"
                }).map(|_| &serde_json::Value::Null)
            })
        })
        .is_some();

    if !pc_has_stealth || !npc_has_perception {
        eprintln!("SKIP: opposed fixture (pc Stealth + npc Spot Hidden/perception) not found");
        eprintln!("  pc_has_stealth={pc_has_stealth} npc_has_perception={npc_has_perception}");
        return;
    }

    let svc = ContestService::new(db.clone());
    let def_roll = roll(95); // 防御方掷 95(高失败)
    let atk_roll = roll(10); // 攻击方掷 10(低成功)

    let out = svc.resolve_outcome(&opposed_contract(), &atk_roll, Some(&def_roll))
        .await.expect("resolve_outcome must not error");

    let success = out.get("success").and_then(|v| v.as_bool());
    let opposed = out.get("opposed");
    println!("[opposed] atk=10 def=95 success={:?}", success);
    println!("[opposed] outcome.opposed={:?}", opposed);

    assert!(success.is_some(), "对抗结算必须给出胜负 (success != null)，而非 Provisional-null");
    assert!(
        opposed.and_then(|o| o.get("defender_value")).is_some(),
        "outcome 必须富化 opposed.defender_value (消费了 NPC 卡的值)"
    );

    println!("PASS: opposed check yields a verdict and enriches outcome.");
}

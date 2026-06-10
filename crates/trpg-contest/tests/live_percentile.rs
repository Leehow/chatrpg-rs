//! Live end-to-end check that percentile (roll-under) resolution reads the
//! ACTOR'S REAL value (CoC Sanity track / Spot Hidden skill) — not the old
//! fabricated env default of 50. Skips unless DATABASE_URL points at a DB that
//! already holds the CoC test character.
//!
//! Run:  DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
//!       cargo test -p trpg-contest --test live_percentile -- --nocapture

use chrono::Utc;
use serde_json::json;
use trpg_contest::ContestService;
use trpg_db::Db;
use trpg_model::*;

const SESSION: &str = "session_6def47593a094513a75b01b69a52c986";
const RULESET: &str = "call_of_cthulhu_7e";
const ACTOR: &str = "pc.current";

fn contract(tested_key: &str, label: &str) -> CheckContract {
    CheckContract {
        check_id: format!("check_test_{}", uuid::Uuid::new_v4().simple()),
        session_id: SESSION.into(),
        turn_id: "turn_test".into(),
        ruleset_id: RULESET.into(),
        module_id: None,
        initiator: ActorRef { actor_id: ACTOR.into(), actor_kind: ActorKind::PlayerCharacter, display_name: None },
        target_actor: None,
        opposition: OppositionModel::NoMechanicalOpposition,
        action_summary: label.into(),
        intent_kind: "ability:skill_use".into(),
        check_label: format!("{} (core mechanic)", label),
        dice_expression: "1d100".into(),
        modifiers: vec![],
        target: CheckTargetModel::UnknownUntilLookup,
        tested_parameter: Some(TestedParameter { domain: None, key: tested_key.into(), label: tested_key.into() }),
        opponent_tested_parameter: None,
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
        roller_id: Some(ACTOR.into()),
        visibility: RollVisibility::PublicGmRoll,
        expression: "1d100".into(),
        result: json!({"total": total, "rolls": [total]}),
        seed_commitment: String::new(),
        revealed_at: None,
        created_at: Utc::now(),
    }
}

#[tokio::test]
async fn percentile_reads_real_actor_value_not_fifty() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect failed: {e}"); return; } };
    // Skip unless this DB holds the CoC test character (avoids false failures
    // when DATABASE_URL points at an unrelated ruleset DB).
    let params = trpg_params::RuntimeParameterService::new(db.clone())
        .load_actor_parameters(SESSION, ACTOR).await.ok().flatten();
    let has_fixture = params.as_ref()
        .and_then(|p| p.mechanical_profile.get("skills"))
        .and_then(|s| s.get("Spot Hidden")).is_some();
    if !has_fixture { eprintln!("SKIP: CoC test fixture (session/{ACTOR} Spot Hidden) not found in this DB"); return; }
    let svc = ContestService::new(db.clone());

    // Spot Hidden is 75 on the sheet — a static skill value, strong assertion.
    let out = svc.resolve_outcome(&contract("Spot Hidden", "Spot Hidden"), &roll(30), None).await.expect("resolve");
    let target = out.get("target").and_then(|v| v.as_i64());
    let success = out.get("success").and_then(|v| v.as_bool());
    println!("[Spot Hidden] roll=30 target={:?} success={:?} label={:?}", target, success, out.get("check_label"));
    assert_eq!(target, Some(75), "Spot Hidden must resolve against the sheet value 75, not 50");
    assert_eq!(success, Some(true), "30 <= 75 must succeed");

    // Sanity is a live resource track — assert it read a REAL value, not 50.
    let out = svc.resolve_outcome(&contract("理智", "理智检定"), &roll(90), None).await.expect("resolve");
    let san = out.get("target").and_then(|v| v.as_i64());
    let success = out.get("success").and_then(|v| v.as_bool());
    println!("[Sanity/理智] roll=90 target={:?} success={:?} (resolution_model={:?})", san, success, out.get("resolution_model").and_then(|m| m.get("PercentileRollUnder")));
    let san = san.expect("sanity target resolved");
    assert!((0..=99).contains(&san), "sanity target {} must be a real track value", san);
    assert_ne!(san, 50, "sanity must NOT resolve against the fabricated env default 50");

    println!("PASS: percentile resolution reads real actor values (no fake 50).");
}

#[tokio::test]
async fn percentile_emits_coc_success_tiers() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect failed: {e}"); return; } };
    let params = trpg_params::RuntimeParameterService::new(db.clone())
        .load_actor_parameters(SESSION, ACTOR).await.ok().flatten();
    let has_bands = params.as_ref().and_then(|p| p.mechanical_profile.get("skills")).and_then(|s| s.get("Spot Hidden")).is_some();
    if !has_bands { eprintln!("SKIP: CoC fixture not found"); return; }
    let svc = ContestService::new(db.clone());

    // Spot Hidden = 75 → CoC tiers: 1=critical, ≤15 extreme, ≤37 hard, ≤75 regular, 100 fumble, else failure.
    let cases = [(1, "critical"), (10, "extreme"), (30, "hard"), (70, "regular"), (90, "failure"), (100, "fumble")];
    for (total, want) in cases {
        let out = svc.resolve_outcome(&contract("Spot Hidden", "Spot Hidden"), &roll(total), None).await.expect("resolve");
        let tier = out.get("success_tier").and_then(|v| v.as_str());
        let rank = out.get("success_tier_rank").and_then(|v| v.as_i64());
        println!("[tier] Spot Hidden(75) roll={:<3} -> tier={:?} rank={:?}", total, tier, rank);
        assert_eq!(tier, Some(want), "roll {} vs 75 should be tier {}", total, want);
    }
    println!("PASS: CoC success tiers computed generically from kernel bands.");
}

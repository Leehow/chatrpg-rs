//! GOLD live e2e (TC-CHECK-01): close the source-backed-target product gap.
//!
//! The live gap: a Cyberpunk RED technical action reached the GM `roll_check`
//! settlement path, rolled real dice, but the contest row came back
//! `target_value=null / success=null / degree=null` with an explicit
//! `awaiting_binding` — "no source-backed target number/opposition model was
//! bound for ruleset `cyberpunk_red`". TC-JRNY-01 correctly classified that as
//! TRIGGERED_NO_MECHANISM (never PASS).
//!
//! THE FIX: the engine now binds the module's source-backed
//! `technical_option_table` DV (Homecoming: cut/power/cable -> DV 14,
//! hack/server -> DV 12) when the kernel left the check unbound — the GM
//! `roll_check` counterpart of the combat path's tech-DV binding. A matching
//! technical action resolves against that DV (real dice + non-null
//! target/success/degree, no `awaiting_binding`); a pure perception action with
//! no matching DV row STAYS fail-closed (awaiting_binding), preserving the
//! TC-JRNY-01 honesty gate. No invented values — the DV is read verbatim from
//! the embedded module config.
//!
//! Needs DATABASE_URL with a `cyberpunk_red`(compare=meet_or_beat) kernel
//! (:54346); else SKIP. Fully deterministic (no LLM): self-contained session,
//! the engine rolls the dice itself, run leaves no actor params behind.
//!
//! Run:  DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54346/chatrpg \
//!       cargo test -p trpg-runtime --test live_tech_dv_binding -- --nocapture
use serde_json::json;
use trpg_db::Db;
use trpg_model::CheckContract;
use trpg_runtime::RuntimeEngine;

const RULESET: &str = "cyberpunk_red";
const MODULE: &str = "cyberpunk_red.homecoming";

/// A "pre-binding" GM `roll_check` contract: target UnknownUntilLookup, no
/// source refs — exactly the shape that produced the live `awaiting_binding`.
fn tech_contract(session: &str, check_label: &str, action_summary: &str) -> CheckContract {
    serde_json::from_value(json!({
        "check_id": format!("check_tech_{}", uuid::Uuid::new_v4().simple()),
        "session_id": session, "turn_id": "turn_test", "ruleset_id": RULESET, "module_id": MODULE,
        "initiator": {"actor_id":"pc.current","actor_kind":"player_character","display_name":null},
        "target_actor": null,
        "opposition": {"kind":"no_mechanical_opposition"},
        "action_summary": action_summary, "intent_kind": "agent_selected_check",
        "check_label": check_label, "dice_expression": "1d10", "modifiers": [],
        "target": {"kind":"unknown_until_lookup"},
        "tested_parameter": {"domain":null,"key":"tech","label":"Basic Tech"},
        "opponent_tested_parameter": null,
        "actor_snapshot_ids": [], "source_refs": [], "learned_packet_ids": [],
        "roll_visibility": "public_gm_roll", "roll_authority": "system",
        "disclosure": {"show_roll_to_player":true,"show_formula_to_player":true,"show_dc_to_player":true,"show_success_failure_to_player":true,"reveal_after_scene":false,"reveal_after_session":false},
        "stakes": {"before_roll_public":"","success_public":"","failure_public":"","critical_public":null,"fumble_public":null,"success_patches_allowed":[],"failure_patches_allowed":[],"irreversible":false},
        "confidence": "medium", "ruling_status": "provisional", "advice_refs": [], "expires_at_turn": null
    })).unwrap()
}

#[tokio::test]
async fn technical_action_resolves_against_source_backed_module_dv() {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP: DATABASE_URL unset");
            return;
        }
    };
    let db = match Db::connect(&url).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("SKIP: connect failed: {e}");
            return;
        }
    };
    let kernel = db.load_rule_kernel(RULESET).await.ok().flatten();
    let is_mob = kernel
        .as_ref()
        .and_then(|k| k.dice_core.get("compare").and_then(|v| v.as_str()))
        == Some("meet_or_beat");
    if !is_mob {
        eprintln!("SKIP: no cyberpunk_red meet_or_beat kernel in this DB (need :54346)");
        return;
    }

    let session = format!("session_tech_dv_e2e_{}", uuid::Uuid::new_v4().simple());
    db.create_session(&session, RULESET, Some(MODULE))
        .await
        .expect("create session");
    let engine = RuntimeEngine::new(db.clone());

    // —— ① THE FIX: a technical action that hits the module DV table resolves.
    let contract = tech_contract(
        &session,
        "Basic Tech check to cut power to the exposed cable",
        "切断外露电缆的供电",
    );
    let exec = engine
        .execute_system_roll_bundle(&session, "turn_test", &contract)
        .await
        .expect("bundle must not error");
    let outcome = &exec.primary.outcome;
    println!(
        "[tech-dv] roll={} outcome={}",
        serde_json::to_string(&exec.primary.roll.result).unwrap(),
        serde_json::to_string(outcome).unwrap()
    );

    // DiceRolled evidence exists (the engine rolled 1d10 itself).
    assert!(
        exec.primary
            .roll
            .result
            .get("total")
            .and_then(|v| v.as_i64())
            .is_some(),
        "DiceRolled evidence: a total must be present"
    );

    // CheckResolved evidence: non-null target/success/degree, NO awaiting_binding.
    assert_eq!(
        outcome.get("target").and_then(|v| v.as_i64()),
        Some(14),
        "target must bind to the source-backed module DV (cut/power/cable -> 14): {outcome}"
    );
    assert!(
        outcome
            .get("success")
            .map(|v| v.is_boolean())
            .unwrap_or(false),
        "success must be a resolved boolean, not null: {outcome}"
    );
    assert!(
        outcome
            .get("degree")
            .map(|v| v.is_string())
            .unwrap_or(false),
        "degree must be resolved, not null: {outcome}"
    );
    assert!(
        outcome.get("awaiting_binding").is_none(),
        "a resolved source-backed check must NOT carry awaiting_binding: {outcome}"
    );
    assert!(
        !exec
            .primary
            .outcome
            .get("blocked")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        "a source-backed technical check must not be blocked"
    );

    let coverage_cases = [
        (
            "Rip the tether free at the wall entry point",
            "Rip the tether free at the wall entry point",
            14,
        ),
        (
            "Trace the tether signal from the damaged sheath toward its warehouse entry point",
            "Trace the tether signal from the damaged sheath toward its warehouse entry point",
            14,
        ),
        (
            "Blast the tether path with interference pulses to destabilize the rogue drone for a moment",
            "Blast the tether path with interference pulses to destabilize the rogue drone for a moment",
            14,
        ),
        (
            "Scan the live terminal's recent remote commands and hard-route the control channel through your deck",
            "Scan the live terminal's recent remote commands and hard-route the control channel through your deck",
            12,
        ),
        (
            "Seize the rogue drone control node and lock its weapon permissions",
            "Seize the rogue drone control node and lock its weapon permissions",
            12,
        ),
    ];
    for (label, summary, expected_target) in coverage_cases {
        let c = tech_contract(&session, label, summary);
        let e = engine
            .execute_system_roll_bundle(&session, "turn_test", &c)
            .await
            .expect("coverage expression must resolve");
        let out = &e.primary.outcome;
        println!(
            "[tech-dv coverage] target={} outcome={}",
            expected_target,
            serde_json::to_string(out).unwrap()
        );
        assert_eq!(
            out.get("target").and_then(|v| v.as_i64()),
            Some(expected_target),
            "live scene expression must bind source-backed target: {out}"
        );
        assert!(
            out.get("success").map(|v| v.is_boolean()).unwrap_or(false),
            "success must be resolved for live scene expression: {out}"
        );
        assert!(
            out.get("degree").map(|v| v.is_string()).unwrap_or(false),
            "degree must be resolved for live scene expression: {out}"
        );
        assert!(
            out.get("awaiting_binding").is_none(),
            "coverage expression must not await binding: {out}"
        );
    }

    // —— ② FAIL-CLOSED preserved: a pure perception action (the live-gap text) has
    //    no matching DV row, so it stays honestly unresolved (awaiting_binding) —
    //    the TC-JRNY-01 gate still classifies it TRIGGERED_NO_MECHANISM, not PASS.
    let perception = tech_contract(
        &session,
        "Perception check to spot an ambush",
        "我压低声音靠近公寓门口，先仔细观察有没有埋伏",
    );
    let exec2 = engine
        .execute_system_roll_bundle(&session, "turn_test", &perception)
        .await
        .expect("bundle must not error");
    let out2 = &exec2.primary.outcome;
    println!(
        "[perception fail-closed] outcome={}",
        serde_json::to_string(out2).unwrap()
    );
    let unresolved = out2.get("success").map(|v| v.is_null()).unwrap_or(true)
        && out2.get("awaiting_binding").is_some();
    assert!(unresolved, "a no-source perception action must stay fail-closed (success=null + awaiting_binding): {out2}");

    // 清场：删临时 session 行（留库干净）。
    sqlx::query("delete from sessions where session_id=$1")
        .bind(&session)
        .execute(&db.pool)
        .await
        .ok();
    println!("PASS: source-backed technical action -> target=14/success/degree resolved (no awaiting_binding); perception stays fail-closed");
}

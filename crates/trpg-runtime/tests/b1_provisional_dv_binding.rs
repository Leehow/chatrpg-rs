//! B1 regression: a partial check that arrives with a usable (LLM/GM-suggested)
//! provisional StaticNumber DV must NOT be degraded to `provisional` by the
//! system-roll path when the ruleset kernel cannot supply a better replacement
//! target. Before the fix, `apply_kernel_defaults_if_unsourced` unconditionally
//! cleared any "provisional"-labeled StaticNumber to `UnknownUntilLookup`; for
//! Cyberpunk RED (meet_or_beat kernel with NO static target_number) the kernel
//! then resolved to nothing → the check fell to `provisional` and never bound.
//! The player-input path (`resolve_check_with_input`) never cleared it, so the
//! two paths diverged. This test pins both invariants:
//!
//!  1. cyberpunk_red provisional-DV check resolves to a bound StaticTargetNumber
//!     on BOTH the system-roll path and the player-input path (no `provisional`).
//!  2. an ALREADY source-backed check is byte/semantically unchanged (the kernel
//!     default binding is a no-op when the contract is already sourced).
//!
//! Generic / data-driven: the gate keys on the kernel's typed `compare` operator
//! and `target_number`, never on a ruleset_id/module_id name (constitution §二-⑪).
//!
//! DB-gated: needs DATABASE_URL pointing at a store with the cyberpunk_red kernel
//! + a materialized PC; SKIPs cleanly otherwise. Fully deterministic (player-
//! reported total, no LLM, no RNG); self-cleans its check_results/contest_profiles.
use serde_json::Value;
use trpg_db::Db;
use trpg_model::*;
use trpg_runtime::RuntimeEngine;

const RULESET: &str = "cyberpunk_red";
const ACTOR: &str = "pc.current";

fn base(session: &str, check_id: &str) -> CheckContract {
    CheckContract {
        check_id: check_id.into(),
        session_id: session.into(),
        turn_id: "turn_b1".into(),
        ruleset_id: RULESET.into(),
        module_id: None,
        initiator: ActorRef {
            actor_id: ACTOR.into(),
            actor_kind: ActorKind::PlayerCharacter,
            display_name: None,
        },
        target_actor: None,
        opposition: OppositionModel::NoMechanicalOpposition,
        action_summary: "Roll Athletics to scramble over the fence".into(),
        intent_kind: "ability:skill_use".into(),
        check_label: "Athletics check".into(),
        dice_expression: "1d10".into(),
        modifiers: vec![],
        target: CheckTargetModel::UnknownUntilLookup,
        tested_parameter: Some(TestedParameter {
            domain: None,
            key: "Athletics".into(),
            label: "Athletics".into(),
        }),
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

fn kind(o: &Value) -> Option<String> {
    o.pointer("/resolution_model/kind")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

async fn cyberpunk_session_with_pc(db: &Db) -> Option<String> {
    // Pick any cyberpunk_red session whose pc.current sheet carries real stats.
    let rows = sqlx::query_scalar::<_, String>(
        r#"select rap.session_id
             from runtime_actor_parameters rap
             join sessions s on s.session_id = rap.session_id
            where s.ruleset_id = 'cyberpunk_red'
              and rap.actor_id = 'pc.current'
              and rap.mechanical_profile ? 'stats'
            limit 1"#,
    )
    .fetch_optional(&db.pool)
    .await
    .ok()
    .flatten();
    rows
}

async fn cleanup(db: &Db, ids: &[&str]) {
    for cid in ids {
        sqlx::query("delete from check_results where check_id = $1")
            .bind(cid)
            .execute(&db.pool)
            .await
            .ok();
        sqlx::query("delete from contest_profiles where check_id = $1")
            .bind(cid)
            .execute(&db.pool)
            .await
            .ok();
    }
}

#[tokio::test]
async fn cyberpunk_provisional_dv_binds_on_both_paths_and_sourced_is_noop() {
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
    // Gate: cyberpunk_red kernel must be meet_or_beat with NO static target_number
    // (the exact shape that exposed the regression). Otherwise SKIP.
    let kernel = db.load_rule_kernel(RULESET).await.ok().flatten();
    let dc = kernel.as_ref().map(|k| k.dice_core.clone());
    let is_mob_no_tnum = dc
        .as_ref()
        .map(|d| {
            d.get("compare").and_then(|v| v.as_str()) == Some("meet_or_beat")
                && d.get("target_number").and_then(|v| v.as_i64()).is_none()
        })
        .unwrap_or(false);
    if !is_mob_no_tnum {
        eprintln!("SKIP: cyberpunk_red kernel is not meet_or_beat/no-tnum (needs :54347)");
        return;
    }
    let session = match cyberpunk_session_with_pc(&db).await {
        Some(s) => s,
        None => {
            eprintln!("SKIP: no cyberpunk_red PC fixture");
            return;
        }
    };

    std::env::set_var("TRPG_PLAYER_REPORTED_ROLL_TOTALS", "1");
    let engine = RuntimeEngine::new(db.clone());

    // Pre-clean any residue from a prior (possibly failed) run: contest profiles
    // are cached by check_id, so a stale profile would mask the real resolution.
    cleanup(
        &db,
        &[
            "b1_prov_agent",
            "b1_prov_input",
            "b1_sourced_agent",
            "b1_sourced_input",
        ],
    )
    .await;

    // —— Invariant 1a: SYSTEM-ROLL path (execute_agent_roll). Provisional DV=13
    // must bind to a StaticTargetNumber(13), NOT fall to provisional.
    let mut prov_agent = base(&session, "b1_prov_agent");
    prov_agent.target = CheckTargetModel::StaticNumber {
        value: 13,
        label: "GM suggested provisional DV".into(),
    };
    let res_agent = engine
        .execute_agent_roll(&session, "t_agent", &prov_agent)
        .await
        .expect("agent roll");
    assert_eq!(
        kind(&res_agent.outcome).as_deref(),
        Some("static_target_number"),
        "system-roll path must keep the usable provisional DV (kernel has no replacement), got: {}",
        res_agent.outcome
    );
    assert_eq!(
        res_agent.outcome.get("target").and_then(|v| v.as_i64()),
        Some(13),
        "bound DV must be 13"
    );
    assert!(
        res_agent
            .outcome
            .get("awaiting_binding")
            .map(|v| v.is_null())
            .unwrap_or(true),
        "must NOT carry awaiting_binding/provisional"
    );

    // —— Invariant 1b: PLAYER-INPUT path (resolve_check_with_input) — same bind.
    let mut prov_input = base(&session, "b1_prov_input");
    prov_input.target = CheckTargetModel::StaticNumber {
        value: 13,
        label: "GM suggested provisional DV".into(),
    };
    let res_input = engine
        .resolve_check_with_input(&session, "t_input", &prov_input, "5")
        .await
        .expect("input roll");
    assert_eq!(
        kind(&res_input.outcome).as_deref(),
        Some("static_target_number"),
        "player-input path must also bind the provisional DV, got: {}",
        res_input.outcome
    );
    assert_eq!(
        res_input.outcome.get("target").and_then(|v| v.as_i64()),
        Some(13)
    );

    // Both paths must agree on the bound target (symmetry).
    assert_eq!(
        res_agent.outcome.get("target").and_then(|v| v.as_i64()),
        res_input.outcome.get("target").and_then(|v| v.as_i64()),
        "system-roll and player-input paths must bind the same DV (symmetry)"
    );

    // —— Invariant 2: an ALREADY source-backed check is unchanged (kernel default
    // binding early-returns when sourced → no-op). Bind a concrete sourced DV=15.
    let sourced = |cid: &str| {
        let mut c = base(&session, cid);
        c.target = CheckTargetModel::StaticNumber {
            value: 15,
            label: "bound DV".into(),
        };
        c.source_refs = vec![SourceRef {
            source_id: "cpr_core".into(),
            page: Some(130),
            anchor_id: None,
            section_path: vec!["Skill Checks".into()],
            char_start: None,
            char_end: None,
            text_hash: None,
            note: None,
        }];
        c
    };
    let r_sourced_agent = engine
        .execute_agent_roll(&session, "t_sa", &sourced("b1_sourced_agent"))
        .await
        .expect("sourced agent");
    let r_sourced_input = engine
        .resolve_check_with_input(&session, "t_si", &sourced("b1_sourced_input"), "5")
        .await
        .expect("sourced input");
    for (label, r) in [("agent", &r_sourced_agent), ("input", &r_sourced_input)] {
        assert_eq!(
            kind(&r.outcome).as_deref(),
            Some("static_target_number"),
            "sourced {label} stays static"
        );
        assert_eq!(
            r.outcome.get("target").and_then(|v| v.as_i64()),
            Some(15),
            "sourced {label} DV unchanged at 15 (binding is a no-op when sourced)"
        );
    }

    cleanup(
        &db,
        &[
            "b1_prov_agent",
            "b1_prov_input",
            "b1_sourced_agent",
            "b1_sourced_input",
        ],
    )
    .await;
    std::env::remove_var("TRPG_PLAYER_REPORTED_ROLL_TOTALS");
    println!("PASS: cyberpunk provisional DV binds on both paths; sourced check is a no-op.");
}

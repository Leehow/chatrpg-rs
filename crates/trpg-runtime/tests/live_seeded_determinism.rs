//! P6 revision — §24-#11 REAL-DB seeded-determinism ON-path acceptance (codex Gap 1).
//!
//! The pure tests (`seeded_rolls.rs`) only prove `roll_seeded(seed) == roll_seeded(seed)` in
//! isolation. They do NOT prove that the SAME check, driven through the actual production roll
//! path (`execute_system_roll_bundle` → `resolve_roll_input`) against a REAL Postgres, yields
//! the SAME rolls. This file closes that gap end-to-end.
//!
//! Precise claim (codex#5): "same check_id ⇒ same roll". The seed is derived from
//! `session:turn:check_id:roller:expression` (computed BEFORE rolling, EXCLUDING result_json), so
//! re-driving the IDENTICAL check_id replays the IDENTICAL dice sequence. We do NOT claim
//! per-action determinism (each new action mints a fresh check_id UUID) nor whole-row byte
//! equality (roll_id / created_at stay UUID/now).
//!
//! ON  (`TRPG_SEEDED_ROLLS=1`): two drives of the same (session,turn,check_id,roller,expr) ⇒
//!     byte-identical `rolls`/`total`.
//! OFF (flag unset): the path uses `thread_rng` ⇒ two drives of a WIDE expression (high entropy)
//!     diverge (proves OFF really is non-seeded; OFF==baseline).
//!
//! Run:
//!   DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!   cargo test -p trpg-runtime --test live_seeded_determinism -- --nocapture --test-threads=1
//! No DATABASE_URL / unreachable ⇒ SKIP (fail-closed, never blocks CI).
//!
//! NOTE: `TRPG_SEEDED_ROLLS` is process-global; this file is single-threaded
//! (`--test-threads=1`) so the OFF test never observes the ON test's flag.

use serde_json::json;
use trpg_db::Db;
use trpg_model::CheckContract;
use trpg_runtime::RuntimeEngine;

const SESSION: &str = "sess_seeded_det";
const RULESET: &str = "call_of_cthulhu_7e";

async fn connect_or_skip() -> Option<Db> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP: DATABASE_URL unset");
            return None;
        }
    };
    let db = match Db::connect(&url).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("SKIP: connect failed: {e}");
            return None;
        }
    };
    db.migrate().await.expect("migrate after connect");
    Some(db)
}

/// A plain source-backed solo check on a WIDE expression so the OFF (thread_rng) path has enough
/// entropy that two independent drives almost never collide. The expression has explicit sources
/// and a bound target so the missing-source-parameter guard never blocks it (we want a REAL roll,
/// not a blocked result).
fn seeded_contract(session: &str, turn: &str, check_id: &str, expr: &str) -> CheckContract {
    serde_json::from_value(json!({
        "check_id": check_id,
        "session_id": session, "turn_id": turn, "ruleset_id": RULESET, "module_id": null,
        "initiator": {"actor_id":"pc.current","actor_kind":"player_character","display_name":null},
        "target_actor": null,
        "opposition": {"kind":"no_mechanical_opposition"},
        "action_summary": "搜索房间", "intent_kind": "investigate",
        "check_label": "Library Use check", "dice_expression": expr, "modifiers": [],
        "target": {"kind":"static_number","value":50,"label":"DC 50"},
        "tested_parameter": {"domain":null,"key":"Library Use","label":"Library Use"},
        "opponent_tested_parameter": null,
        "actor_snapshot_ids": [], "source_refs": [{"source_id":"coc.library_use","section_path":["Library Use"]}], "learned_packet_ids": [],
        "roll_visibility": "public_gm_roll", "roll_authority": "system",
        "disclosure": {"show_roll_to_player":true,"show_formula_to_player":true,"show_dc_to_player":true,"show_success_failure_to_player":true,"reveal_after_scene":false,"reveal_after_session":false},
        "stakes": {"before_roll_public":"","success_public":"","failure_public":"","critical_public":null,"fumble_public":null,"success_patches_allowed":[],"failure_patches_allowed":[],"irreversible":false},
        "confidence": "medium", "ruling_status": "source_backed", "advice_refs": [], "expires_at_turn": null
    })).unwrap()
}

async fn prepare(db: &Db, session: &str) {
    db.create_session(session, RULESET, None)
        .await
        .expect("create_session");
}

async fn purge(db: &Db, session: &str) {
    for sql in [
        "delete from check_results where session_id=$1",
        "delete from dice_rolls where session_id=$1",
        "delete from roll_plans where session_id=$1",
        "delete from agent_tool_calls where session_id=$1",
        "delete from check_gates where session_id=$1",
        "delete from pending_checks where session_id=$1",
        "delete from sessions where session_id=$1",
    ] {
        // Some tables may not exist in every schema slice; ignore individual failures.
        let _ = sqlx::query(sql).bind(session).execute(&db.pool).await;
    }
}

/// Extract the `rolls` array + `total` from a produced primary check result's dice roll.
fn rolls_and_total(result: &serde_json::Value) -> (Vec<i64>, i64) {
    let r = &result;
    let rolls = r
        .get("rolls")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_i64()).collect::<Vec<_>>())
        .unwrap_or_default();
    let total = r.get("total").and_then(|v| v.as_i64()).unwrap_or(i64::MIN);
    (rolls, total)
}

/// §24-#11 ON-path: same (session,turn,check_id,roller,expr) driven through the REAL roll bundle
/// against a REAL DB TWICE ⇒ byte-identical rolls. This is the production seam the pure tests
/// could not reach.
#[tokio::test]
async fn same_check_id_same_roll_through_real_db() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("{SESSION}_on_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;
    prepare(&db, &session).await;
    let engine = RuntimeEngine::new(db.clone());

    // Fixed check_id + wide expression: the seed is (session:turn:check_id:roller:expr).
    let check_id = format!("check_seed_{}", uuid::Uuid::new_v4().simple());
    let expr = "8d100"; // wide ⇒ thread_rng would almost surely diverge; seeded must NOT.

    std::env::set_var("TRPG_SEEDED_ROLLS", "1");

    let first = engine
        .execute_system_roll_bundle(
            &session,
            "turn_seed",
            &seeded_contract(&session, "turn_seed", &check_id, expr),
        )
        .await
        .expect("first seeded roll bundle");
    let (rolls_a, total_a) = rolls_and_total(&first.primary.roll.result);

    // Clean prior rows for the same check_id so the second drive re-resolves from scratch
    // (the seed is derived from inputs, not from any persisted roll).
    let _ = sqlx::query("delete from check_results where session_id=$1")
        .bind(&session)
        .execute(&db.pool)
        .await;
    let _ = sqlx::query("delete from dice_rolls where session_id=$1")
        .bind(&session)
        .execute(&db.pool)
        .await;
    let _ = sqlx::query("delete from check_gates where session_id=$1")
        .bind(&session)
        .execute(&db.pool)
        .await;

    let second = engine
        .execute_system_roll_bundle(
            &session,
            "turn_seed",
            &seeded_contract(&session, "turn_seed", &check_id, expr),
        )
        .await
        .expect("second seeded roll bundle");
    let (rolls_b, total_b) = rolls_and_total(&second.primary.roll.result);

    std::env::remove_var("TRPG_SEEDED_ROLLS");

    println!("[seeded ON] check_id={check_id} expr={expr}");
    println!("[seeded ON] drive#1 rolls={rolls_a:?} total={total_a}");
    println!("[seeded ON] drive#2 rolls={rolls_b:?} total={total_b}");
    assert!(
        !rolls_a.is_empty(),
        "the roll path must actually roll (not a blocked-missing-source result)"
    );
    assert_eq!(
        rolls_a, rolls_b,
        "§24-#11 ON: same check_id ⇒ IDENTICAL rolls through the REAL DB roll path"
    );
    assert_eq!(
        total_a, total_b,
        "§24-#11 ON: same check_id ⇒ IDENTICAL total"
    );
    println!("PASS: same check_id ⇒ same roll (seeded ON, real DB)");

    purge(&db, &session).await;
}

/// OFF==baseline: with the flag unset, the SAME wide check driven twice uses `thread_rng` and
/// (with overwhelming probability) diverges — proving OFF is genuinely non-seeded.
#[tokio::test]
async fn flag_off_uses_thread_rng_and_diverges() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    std::env::remove_var("TRPG_SEEDED_ROLLS"); // ensure OFF regardless of ordering

    let session = format!("{SESSION}_off_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;
    prepare(&db, &session).await;
    let engine = RuntimeEngine::new(db.clone());

    let check_id = format!("check_off_{}", uuid::Uuid::new_v4().simple());
    let expr = "12d100"; // very wide ⇒ collision probability of two thread_rng draws ~ negligible.

    let first = engine
        .execute_system_roll_bundle(
            &session,
            "turn_off",
            &seeded_contract(&session, "turn_off", &check_id, expr),
        )
        .await
        .expect("first OFF roll bundle");
    let (rolls_a, _ta) = rolls_and_total(&first.primary.roll.result);

    let _ = sqlx::query("delete from check_results where session_id=$1")
        .bind(&session)
        .execute(&db.pool)
        .await;
    let _ = sqlx::query("delete from dice_rolls where session_id=$1")
        .bind(&session)
        .execute(&db.pool)
        .await;
    let _ = sqlx::query("delete from check_gates where session_id=$1")
        .bind(&session)
        .execute(&db.pool)
        .await;

    let second = engine
        .execute_system_roll_bundle(
            &session,
            "turn_off",
            &seeded_contract(&session, "turn_off", &check_id, expr),
        )
        .await
        .expect("second OFF roll bundle");
    let (rolls_b, _tb) = rolls_and_total(&second.primary.roll.result);

    println!("[seeded OFF] drive#1 rolls={rolls_a:?}");
    println!("[seeded OFF] drive#2 rolls={rolls_b:?}");
    assert!(!rolls_a.is_empty(), "OFF path must actually roll");
    assert_ne!(
        rolls_a, rolls_b,
        "OFF: thread_rng must diverge across drives (proves OFF is non-seeded)"
    );
    println!("PASS: flag OFF uses thread_rng (non-seeded), real DB");

    purge(&db, &session).await;
}

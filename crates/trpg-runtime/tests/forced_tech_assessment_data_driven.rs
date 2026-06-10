//! The forced "technical assessment" plan must be DATA-DRIVEN from the
//! ruleset's parsed kernel — NOT the old hardcoded Cyberpunk `1d10` / DV `14` /
//! "TECH / repair / hacking" check that leaked into every ruleset. In a Call of
//! Cthulhu session that bug made a technical check resolve as roll-HIGH vs 14 on
//! 1d10 instead of CoC's 1d100 roll-UNDER vs the actor's real skill.
//!
//! Live test: skips unless DATABASE_URL points at a DB holding a parsed kernel
//! (and, for the end-to-end case, the CoC test character `pc.current`).
//!
//! Run (CoC fixture DB):
//!   DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
//!     cargo test -p trpg-runtime --test forced_tech_assessment_data_driven -- --nocapture

use trpg_contest::ContestService;
use trpg_db::Db;
use trpg_model::*;
use trpg_runtime::RuntimeEngine;

const COC_SESSION: &str = "session_6def47593a094513a75b01b69a52c986";

async fn connect() -> Option<Db> {
    let url = std::env::var("DATABASE_URL").ok()?;
    Db::connect(&url).await.ok()
}

fn request(ruleset: &str, session: &str) -> ContextRequest {
    ContextRequest {
        ruleset_id: ruleset.into(),
        module_id: None,
        session_id: session.into(),
        turn_id: "turn_test_forced_tech".into(),
        viewer: VisibilityProfile::player("player_test", "pc.current"),
        token_budget: TokenBudget::default(),
    }
}

/// For every ruleset whose kernel lives in the connected DB, the forced
/// technical-assessment plan must use that kernel's core dice and a GENERIC
/// target / opposition — never the hardcoded `1d10` / `StaticNumber 14` /
/// `StaticDc` / "TECH" label that the Cyberpunk-era code injected everywhere.
#[tokio::test]
async fn forced_plan_is_data_driven_not_hardcoded() {
    let db = match connect().await {
        Some(d) => d,
        None => { eprintln!("SKIP: DATABASE_URL unset/unreachable"); return; }
    };
    let engine = RuntimeEngine::new(db.clone());

    // (ruleset_id, expected kernel core dice) for the rulesets we ship fixtures
    // for. Only the ruleset whose kernel is in the connected DB is exercised.
    let cases = [("call_of_cthulhu_7e", "1d100"), ("cyberpunk_red", "1d10")];
    let mut checked = 0;
    for (ruleset, want_dice) in cases {
        let kernel = match db.load_rule_kernel(ruleset).await.ok().flatten() {
            Some(k) => k,
            None => { eprintln!("SKIP {ruleset}: kernel not in this DB"); continue; }
        };
        let kernel_dice = kernel.dice_core.get("dice").and_then(|v| v.as_str()).unwrap_or("");
        assert_eq!(kernel_dice, want_dice, "{ruleset}: kernel core dice");

        let req = request(ruleset, "sess_forced_tech_shape");
        let plan = engine
            .forced_technical_assessment_plan(&req, "I carefully inspect the device for hazards")
            .await;
        let check = plan
            .check
            .expect("forced plan must still produce a check when the kernel has dice");

        // 1) dice come from the kernel core mechanic, NOT a hardcoded 1d10.
        assert_eq!(check.dice_expression, want_dice, "{ruleset}: dice must equal kernel core dice");
        // 2) target is generic so the contest kernel resolves it per-ruleset —
        //    NOT a fabricated StaticNumber 14.
        assert!(
            matches!(check.target, CheckTargetModel::UnknownUntilLookup),
            "{ruleset}: target must be UnknownUntilLookup, got {:?}",
            check.target
        );
        // 3) opposition is generic — NOT the old StaticDc(14).
        assert!(
            matches!(check.opposition, OppositionModel::NoMechanicalOpposition),
            "{ruleset}: opposition must be NoMechanicalOpposition, got {:?}",
            check.opposition
        );
        // 4) the Cyberpunk-flavored "TECH / repair / hacking" label is gone.
        let label = check.check_label.to_ascii_lowercase();
        assert!(
            !label.contains("tech / repair") && !label.contains("hacking"),
            "{ruleset}: check_label must not hardcode the Cyberpunk TECH label, got {:?}",
            check.check_label
        );
        checked += 1;
        eprintln!(
            "OK {ruleset}: dice={} target=UnknownUntilLookup opposition=NoMechanicalOpposition label={:?}",
            check.dice_expression, check.check_label
        );
    }
    if checked == 0 {
        eprintln!("SKIP: no known ruleset kernel found in this DB");
    }
}

/// End-to-end CoC proof: the forced technical check resolves as 1d100
/// ROLL-UNDER against the actor's REAL skill — not roll-HIGH vs a fabricated 14.
/// Uses Locksmith (30 on the fixture): a roll of `skill + 10` FAILS roll-under
/// yet would have SUCCEEDED under the old roll-high-vs-14 model, so the
/// assertion pins the model FLIP, not merely the number.
#[tokio::test]
async fn coc_technical_check_resolves_percentile_roll_under_not_dv14() {
    let db = match connect().await {
        Some(d) => d,
        None => { eprintln!("SKIP: DATABASE_URL unset/unreachable"); return; }
    };
    // Require the CoC fixture character (a Locksmith skill in a range where
    // roll-under and roll-high-vs-14 give opposite verdicts).
    let params = trpg_params::RuntimeParameterService::new(db.clone())
        .load_actor_parameters(COC_SESSION, "pc.current")
        .await
        .ok()
        .flatten();
    let locksmith = params
        .as_ref()
        .and_then(|p| p.mechanical_profile.get("skills"))
        .and_then(|s| s.get("Locksmith"))
        .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));
    let skill = match locksmith {
        Some(v) if (5..=89).contains(&v) => v,
        _ => {
            eprintln!("SKIP: CoC fixture (pc.current Locksmith in 5..=89) not found in this DB");
            return;
        }
    };

    let engine = RuntimeEngine::new(db.clone());
    let req = request("call_of_cthulhu_7e", COC_SESSION);
    let plan = engine
        .forced_technical_assessment_plan(
            &req,
            "I try to pick the lock on the strange cabinet using my Locksmith skill",
        )
        .await;
    let check = plan.check.expect("CoC forced plan must produce a check");
    assert_eq!(check.dice_expression, "1d100", "CoC forced check must roll the kernel d100");

    let total = skill + 10; // > skill ⇒ roll-under FAILS; > 14 ⇒ old roll-high model PASSES
    let roll = DiceRollRecord {
        roll_id: format!("roll_test_{}", uuid::Uuid::new_v4().simple()),
        session_id: COC_SESSION.into(),
        turn_id: check.turn_id.clone(),
        check_id: Some(check.check_id.clone()),
        roller_kind: ActorKind::PlayerCharacter,
        roller_id: Some("pc.current".into()),
        visibility: RollVisibility::PublicGmRoll,
        expression: "1d100".into(),
        result: serde_json::json!({"total": total, "rolls": [total]}),
        seed_commitment: String::new(),
        revealed_at: None,
        created_at: chrono::Utc::now(),
    };
    let out = ContestService::new(db.clone())
        .resolve_outcome(&check, &roll, None)
        .await
        .expect("resolve");
    let target = out.get("target").and_then(|v| v.as_i64());
    let success = out.get("success").and_then(|v| v.as_bool());
    eprintln!(
        "[CoC forced tech] Locksmith={skill} roll={total} -> target={:?} success={:?} model={:?}",
        target,
        success,
        out.get("resolution_model")
    );

    assert_eq!(target, Some(skill), "must resolve against the actor's REAL Locksmith value, not 14");
    assert_eq!(
        success,
        Some(false),
        "roll {} > skill {} must FAIL roll-under (the old roll-high-vs-14 model would have passed)",
        total,
        skill
    );
}

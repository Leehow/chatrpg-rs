//! apply_effect_roll regression + SSOT-unification net (drives the public after_check_resolved).
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
//!      cargo test -p trpg-mechanics --test live_apply_effect_roll -- --nocapture --test-threads=1
use chrono::Utc;
use serde_json::json;
use trpg_db::Db;
use trpg_model::*;
use trpg_mechanics::RefereeCombatService;

const SESSION: &str = "sess_slice_b_effect";
const TARGET: &str = "npc.slice_b_target";
const INITIATOR: &str = "pc.slice_b_src";
const RULESET: &str = "call_of_cthulhu_7e";

async fn connect() -> Option<Db> {
    let url = std::env::var("DATABASE_URL").ok()?;
    Db::connect(&url).await.ok()
}

async fn reset(db: &Db) {
    sqlx::query("delete from generic_parameter_states where session_id=$1").bind(SESSION).execute(&db.pool).await.unwrap();
    sqlx::query("delete from actor_mechanical_states where session_id=$1").bind(SESSION).execute(&db.pool).await.unwrap();
    sqlx::query("delete from parameter_facet_bindings where session_id=$1").bind(SESSION).execute(&db.pool).await.ok();
}

async fn seed_actor(db: &Db, actor: &str, kind: &str, resources: serde_json::Value) {
    sqlx::query(r#"insert into runtime_actor_parameters
        (id, actor_param_id, session_id, actor_id, actor_kind, ruleset_id, source_kind, template_id, display_name, sheet_json, mechanical_profile, status_json, visibility, created_at_tick, updated_at_tick)
        values (gen_random_uuid(), $1, $2, $3, $4, $5, 'test_seed', null, $3, $6, '{}'::jsonb, '{}'::jsonb, 'gm_only', 0, 0)
        on conflict (session_id, actor_id) do update set sheet_json=excluded.sheet_json, ruleset_id=excluded.ruleset_id, actor_kind=excluded.actor_kind"#)
        .bind(format!("ap_{actor}")).bind(SESSION).bind(actor).bind(kind).bind(RULESET)
        .bind(json!({"resources": resources}))
        .execute(&db.pool).await.unwrap();
}

fn contract(intent: &str, label: &str, target: &str) -> CheckContract {
    CheckContract {
        check_id: format!("check_{}", uuid::Uuid::new_v4().simple()),
        session_id: SESSION.into(), turn_id: "turn_test".into(), ruleset_id: RULESET.into(), module_id: None,
        initiator: ActorRef { actor_id: INITIATOR.into(), actor_kind: ActorKind::PlayerCharacter, display_name: None },
        target_actor: Some(ActorRef { actor_id: target.into(), actor_kind: ActorKind::Npc, display_name: None }),
        opposition: OppositionModel::NoMechanicalOpposition,
        action_summary: label.into(), intent_kind: intent.into(), check_label: label.into(),
        dice_expression: "1d6".into(), modifiers: vec![], target: CheckTargetModel::UnknownUntilLookup,
        tested_parameter: None, opponent_tested_parameter: None, actor_snapshot_ids: vec![], source_refs: vec![], learned_packet_ids: vec![],
        roll_visibility: RollVisibility::PublicGmRoll, roll_authority: RollAuthority::System,
        disclosure: RollDisclosurePolicy::for_visibility(RollVisibility::PublicGmRoll),
        stakes: CheckStakes::default(), confidence: RulingConfidence::Medium, ruling_status: RulingStatus::SourceBacked,
        advice_refs: vec![], expires_at_turn: None,
    }
}

fn result(total: i64) -> CheckResultRecord {
    CheckResultRecord {
        check_id: format!("check_{}", uuid::Uuid::new_v4().simple()),
        roll: DiceRollRecord {
            roll_id: format!("roll_{}", uuid::Uuid::new_v4().simple()),
            session_id: SESSION.into(), turn_id: "turn_test".into(), check_id: None,
            roller_kind: ActorKind::PlayerCharacter, roller_id: Some(INITIATOR.into()),
            visibility: RollVisibility::PublicGmRoll, expression: "1d6".into(),
            result: json!({"total": total, "rolls": [total]}),
            seed_commitment: String::new(), revealed_at: None, created_at: Utc::now(),
        },
        outcome: json!({"success": true}),  // NOTE: no "damage" key -> apply_outcome HP rule skips
        committed_patches: vec![], created_at: Utc::now(),
    }
}

async fn hp(db: &Db, actor: &str) -> Option<i32> {
    let k = db.load_rule_kernel(RULESET).await.unwrap().unwrap();
    let id = trpg_model::hp_resource_track_id(&k.resource_tracks).expect("coc hp track id");
    db.load_resource_current(SESSION, actor, &id, &k).await
}

async fn guard() -> Option<Db> {
    let db = connect().await?;
    if db.load_rule_kernel(RULESET).await.ok().flatten().is_none() { eprintln!("SKIP: no {RULESET} kernel"); return None; }
    Some(db)
}

#[tokio::test]
async fn hp_damage_no_armor_writes_ssot() {
    let Some(db) = guard().await else { return };
    reset(&db).await;
    seed_actor(&db, TARGET, "npc", json!({"hit_points": 12})).await;
    RefereeCombatService { db: db.clone() }.after_check_resolved(&contract("damage_roll","Damage",TARGET), &mut result(5)).await.expect("effect");
    assert_eq!(hp(&db, TARGET).await, Some(7), "12 - 5 = 7 in the SSOT (generic_parameter_states)");
    reset(&db).await;
}

#[tokio::test]
async fn hp_damage_with_armor_mitigates() {
    let Some(db) = guard().await else { return };
    reset(&db).await;
    seed_actor(&db, TARGET, "npc", json!({"hit_points": 12})).await;
    // materialize armor on the ams row (combat metadata, still read from actor_mechanical_states)
    sqlx::query(r#"insert into actor_mechanical_states (id, state_id, session_id, actor_id, actor_kind, hp_current, hp_max, armor_current, wound_state, morale, resources_json, conditions_json, source_refs, world_tick, updated_at)
        values (gen_random_uuid(), $1, $2, $3, 'npc', 12, 12, 5, 'unhurt', 55, '{}'::jsonb, '[]'::jsonb, '[]'::jsonb, 0, now())
        on conflict (session_id, actor_id) do update set armor_current = 5"#)
        .bind(format!("ams_{TARGET}")).bind(SESSION).bind(TARGET).execute(&db.pool).await.unwrap();
    RefereeCombatService { db: db.clone() }.after_check_resolved(&contract("damage_roll","Damage",TARGET), &mut result(10)).await.expect("effect");
    assert_eq!(hp(&db, TARGET).await, Some(7), "10 dmg - 5 SP = 5 effective; 12 - 5 = 7");
    reset(&db).await;
}

#[tokio::test]
async fn hp_damage_floors_at_zero_when_lethal() {
    let Some(db) = guard().await else { return };
    reset(&db).await;
    seed_actor(&db, TARGET, "npc", json!({"hit_points": 12})).await;
    RefereeCombatService { db: db.clone() }.after_check_resolved(&contract("damage_roll","Damage",TARGET), &mut result(20)).await.expect("effect");
    assert_eq!(hp(&db, TARGET).await, Some(0), "lethal damage floors HP at 0, never negative");
    reset(&db).await;
}

#[tokio::test]
async fn resource_effect_writes_ssot() {
    let Some(db) = guard().await else { return };
    reset(&db).await;
    seed_actor(&db, TARGET, "npc", json!({"sanity": 60})).await;
    // Route the effect to the sanity track via a source-bound facet (the unbound
    // fallback only routes to hp.current). lookup_bound_facet_decision reads
    // facet_json.target_parameter / target_kind / target_id / operation.
    sqlx::query(r#"insert into parameter_facet_bindings
        (id, facet_binding_id, session_id, target_kind, target_id, facet_kind, binding_id, demand_id, facet_json, source_refs, confidence, verification_status, world_tick)
        values (gen_random_uuid(), $1, $2, 'actor', $3, 'effect_resource_binding', $4, null, $5, '[]'::jsonb, 'high', 'verified', 0)"#)
        .bind(format!("fb_{}", uuid::Uuid::new_v4().simple())).bind(SESSION).bind(TARGET)
        .bind(format!("bind_{}", uuid::Uuid::new_v4().simple()))
        .bind(json!({"target_kind":"actor","target_id":TARGET,"target_parameter":"resources.sanity.current","operation":"subtract"}))
        .execute(&db.pool).await.unwrap();
    RefereeCombatService { db: db.clone() }.after_check_resolved(&contract("effect_roll","Effect",TARGET), &mut result(4)).await.expect("effect");
    let k = db.load_rule_kernel(RULESET).await.unwrap().unwrap();
    assert_eq!(db.load_resource_current(SESSION, TARGET, "sanity", &k).await, Some(56), "sanity 60 - 4 = 56 in SSOT");
    reset(&db).await;
}

#[tokio::test]
async fn combat_write_visible_at_contest_read_path_and_accumulates() {
    let Some(db) = guard().await else { return };
    reset(&db).await;
    seed_actor(&db, TARGET, "npc", serde_json::json!({"hit_points": 12})).await;
    let svc = RefereeCombatService { db: db.clone() };

    // Combat/effect HP damage: 12 - 5 = 7, written via the SSOT primitive.
    svc.after_check_resolved(&contract("damage_roll", "Damage", TARGET), &mut result(5)).await.expect("effect 1");

    // (a) The write landed in the SHARED store (generic_parameter_states) at the
    //     EXACT path the contest read path (read_track_value -> load_resource_current)
    //     reads: target_kind='actor', resources.{hp_id}.current. This is the
    //     divergence the slice fixes — formerly combat wrote actor_mechanical_states.
    let kernel = db.load_rule_kernel(RULESET).await.unwrap().unwrap();
    let hp_id = trpg_model::hp_resource_track_id(&kernel.resource_tracks).expect("coc hp track");
    let path = format!("resources.{}.current", hp_id);
    let row_val: Option<i64> = sqlx::query_scalar(
        "select (value_json #>> '{}')::int8 from generic_parameter_states where session_id=$1 and target_kind='actor' and target_id=$2 and parameter_path=$3")
        .bind(SESSION).bind(TARGET).bind(&path)
        .fetch_optional(&db.pool).await.unwrap().flatten();
    assert_eq!(row_val, Some(7), "combat HP write must be in generic_parameter_states at the contest-read path (single source of truth)");

    // (b) The contest read path returns the same value (read_track_value delegates to load_resource_current).
    assert_eq!(db.load_resource_current(SESSION, TARGET, &hp_id, &kernel).await, Some(7));

    // (c) A SECOND effect accumulates on the SAME row (not a separate store): 7 - 3 = 4.
    svc.after_check_resolved(&contract("damage_roll", "Damage", TARGET), &mut result(3)).await.expect("effect 2");
    assert_eq!(db.load_resource_current(SESSION, TARGET, &hp_id, &kernel).await, Some(4), "second hit must decrement the same SSOT row");

    reset(&db).await;
}

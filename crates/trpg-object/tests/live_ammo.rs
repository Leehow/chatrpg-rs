//! Live check that firing a ranged weapon decrements loaded rounds via the
//! generic object-rule-effects executor (the fix for: combat attacks went
//! through the auto-roll path that never decremented ammo). Skips unless
//! DATABASE_URL points at a DB holding the Cyberpunk test pistol.
//!
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54346/chatrpg \
//!      cargo test -p trpg-object --test live_ammo -- --nocapture

use serde_json::json;
use trpg_db::Db;
use trpg_model::*;
use trpg_object::ObjectService;

const SESSION: &str = "session_03718b108b9b44188163752d6d788e7d";
const ACTOR: &str = "npc.opposition";
const OBJ: &str = "obj.session_03718b108b9b44188163752d6d788e7d.npc_opposition.cyberpunk_red_weapon_heavy_pistol";

fn attack(intent: &str) -> CheckContract {
    CheckContract {
        check_id: format!("check_test_{}", uuid::Uuid::new_v4().simple()),
        session_id: SESSION.into(),
        turn_id: "turn_test".into(),
        ruleset_id: "cyberpunk_red".into(),
        module_id: None,
        initiator: ActorRef { actor_id: ACTOR.into(), actor_kind: ActorKind::Npc, display_name: None },
        target_actor: Some(ActorRef { actor_id: "pc.current".into(), actor_kind: ActorKind::PlayerCharacter, display_name: None }),
        opposition: OppositionModel::NoMechanicalOpposition,
        action_summary: "fire at the scav".into(),
        intent_kind: intent.into(),
        check_label: "ranged attack".into(),
        dice_expression: "1d10".into(),
        modifiers: vec![],
        target: CheckTargetModel::UnknownUntilLookup,
        tested_parameter: None,
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

async fn ammo(db: &Db) -> Option<i64> {
    sqlx::query_scalar::<_, serde_json::Value>("select mechanical_state from object_instances where object_id=$1")
        .bind(OBJ).fetch_optional(&db.pool).await.ok().flatten()
        .and_then(|m| m.get("ammo_current").and_then(|v| v.as_i64()))
}

#[tokio::test]
async fn firing_decrements_ammo_generically() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect: {e}"); return; } };
    if ammo(&db).await.is_none() { eprintln!("SKIP: {OBJ} fixture not found"); return; }

    // reset to a known full magazine
    sqlx::query("update object_instances set mechanical_state = jsonb_set(mechanical_state, '{ammo_current}', '8') where object_id=$1")
        .bind(OBJ).execute(&db.pool).await.unwrap();
    let svc = ObjectService::new(db.clone());

    svc.apply_object_rule_effects(SESSION, &attack("attack")).await.unwrap();
    let a1 = ammo(&db).await.unwrap();
    println!("[ammo] after 1st attack: {a1}/8");
    assert_eq!(a1, 7, "one attack must spend one round (8 -> 7)");

    svc.apply_object_rule_effects(SESSION, &attack("attack")).await.unwrap();
    let a2 = ammo(&db).await.unwrap();
    println!("[ammo] after 2nd attack: {a2}/8");
    assert_eq!(a2, 6, "second attack 7 -> 6");

    // the damage/effect follow-up roll must NOT spend another round
    svc.apply_object_rule_effects(SESSION, &attack("effect_roll")).await.unwrap();
    let a3 = ammo(&db).await.unwrap();
    println!("[ammo] after effect_roll (should be no-op): {a3}/8");
    assert_eq!(a3, 6, "effect/damage roll must not consume ammo");

    println!("PASS: firing decrements ammo on the universal hook; effect roll is a no-op.");
}

#[tokio::test]
async fn use_resolves_against_the_actors_own_object() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect: {e}"); return; } };
    if ammo(&db).await.is_none() { eprintln!("SKIP: fixture not found"); return; }
    let svc = ObjectService::new(db.clone());

    // The actor's OWN held weapon is found by the words the player typed — proving
    // self-directed interactions resolve against the actor's inventory, not a seed.
    let found = svc.find_owned_object_by_reference(SESSION, "npc.opposition", "use the heavy pistol").await.unwrap();
    println!("[owned] 'use the heavy pistol' -> {:?}", found.as_ref().map(|o| &o.display_name));
    assert_eq!(found.as_ref().map(|o| o.display_name.as_str()), Some("Heavy Pistol"));

    // An unrelated reference must NOT match (so the caller falls back to seeding).
    let none = svc.find_owned_object_by_reference(SESSION, "npc.opposition", "我念一段咒语").await.unwrap();
    println!("[owned] unrelated input -> {:?}", none.as_ref().map(|o| &o.display_name));
    assert!(none.is_none(), "no owned object matches an unrelated reference");

    println!("PASS: Use/self-directed interactions resolve against the actor's own named object.");
}

#[tokio::test]
async fn loot_source_a_transfers_targets_owned_items() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect: {e}"); return; } };
    if ammo(&db).await.is_none() { eprintln!("SKIP: fixture not found"); return; }
    let svc = ObjectService::new(db.clone());

    // The searched NPC owns a structured Heavy Pistol (with ammo) — source-backed loot.
    let owned = svc.list_owned_objects(SESSION, "npc.opposition").await.unwrap();
    println!("[loot] npc.opposition owns: {:?}", owned.iter().map(|o| &o.display_name).collect::<Vec<_>>());
    assert!(owned.iter().any(|o| o.display_name.contains("Heavy Pistol")), "npc owns the pistol");
    assert!(owned.iter().all(|o| !o.object_id.ends_with(".body")), "body container excluded from loot");

    // 搜身 builds reveal + transfer patches moving each item to the looter (pc.current).
    let patches = svc.loot_owned_patches(SESSION, "npc.opposition", "pc.current").await.unwrap();
    let transfers: Vec<(String, ObjectLocation)> = patches.iter().filter_map(|p| match p {
        ObjectPatch::TransferObject { object_id, to, .. } => Some((object_id.clone(), to.clone())),
        _ => None,
    }).collect();
    println!("[loot] transfers: {:?}", transfers.iter().map(|(id, _)| id).collect::<Vec<_>>());
    assert!(transfers.iter().any(|(id, _)| id.contains("heavy_pistol")), "pistol is transferred to looter");
    assert!(transfers.iter().all(|(_, to)| matches!(to, ObjectLocation::Carried { actor_id, .. } if actor_id == "pc.current")), "items go to the looter");
    assert!(patches.iter().any(|p| matches!(p, ObjectPatch::SetObjectVisibility { .. })), "items revealed on search");

    // Ownership scoping (fail-closed / no cross-actor leak): the npc's pistol is NOT pc-owned.
    let pc_owned = svc.list_owned_objects(SESSION, "pc.current").await.unwrap();
    assert!(!pc_owned.iter().any(|o| o.object_id.contains("npc_opposition")), "npc items are not enumerated as pc-owned");

    println!("PASS: 搜身 transfers the searched actor's own structured items (weapon+ammo) to the looter, source-backed.");
}

#[tokio::test]
async fn weapon_params_come_from_source_def_not_hardcoded_table() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect: {e}"); return; } };
    let def_id = "test.object.improvised_handcannon";
    // Insert a GENUINELY source-backed materialized def (source_refs non-empty) — the
    // equivalent of MaterializationService having verified it against the rules.
    sqlx::query(r#"insert into object_definitions
        (id, object_def_id, ruleset_id, name, object_kind, tags, mechanical_profile, rule_bindings, default_affordances, equip_slots, visibility_default, source_refs)
        values ($1,$2,'cyberpunk_red','改装手炮','weapon', ARRAY['materialized_v1_10']::text[], $3::jsonb, '[]'::jsonb, '{}'::text[], '{}'::text[], 'gm_only', $4::jsonb)
        on conflict (object_def_id) do update set mechanical_profile=excluded.mechanical_profile, source_refs=excluded.source_refs"#)
        .bind(uuid::Uuid::new_v4()).bind(def_id)
        .bind(json!({"damage":"5d6","ammo_max":6,"ammo_current":6,"source":"verified_against_rules"}))
        .bind(json!([{"source_id":"cyberpunk_red_core","anchor_id":"p0042","page":42}]))
        .execute(&db.pool).await.unwrap();

    let svc = ObjectService::new(db.clone());
    // A narratively-introduced item whose name matches a SOURCE def -> params from source.
    let found = svc.find_materialized_object_def("cyberpunk_red", "我从裤裆掏出一把改装手炮顶住他").await.unwrap();
    println!("[srcparam] '改装手炮' -> {:?}", found.as_ref().map(|(id, n, p)| (id, n, p.get("damage"))));
    let (id, name, profile) = found.expect("source-backed def matched by name");
    assert_eq!(id, def_id);
    assert_eq!(name, "改装手炮");
    assert_eq!(profile.get("damage").and_then(|v| v.as_str()), Some("5d6"), "params come from the SOURCE def, not weapon_definition_for");

    // An unrelated item has no source def -> None (caller falls back / the GM clarifies).
    let none = svc.find_materialized_object_def("cyberpunk_red", "我捡起一根普通的树枝").await.unwrap();
    println!("[srcparam] unrelated item -> {:?}", none.as_ref().map(|(_, n, _)| n));
    assert!(none.is_none(), "no fabricated params for an item the source doesn't cover");

    sqlx::query("delete from object_definitions where object_def_id=$1").bind(def_id).execute(&db.pool).await.unwrap();
    println!("PASS: item params come from the source-backed def; unknown items return None (no hardcoded fabrication).");
}

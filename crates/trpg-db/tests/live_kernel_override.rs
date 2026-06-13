//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
//!      cargo test -p trpg-db --test live_kernel_override -- --nocapture
use trpg_db::Db;

const DND: &str = "dnd5e"; // verify via: select distinct ruleset_id from rule_kernels;

// Durability proof for the dice_core override layer: a DB kernel whose
// success_bands were under-extracted (no `critical` band — the exact CoC
// re-parse hazard) is HEALED at read time by the override file, WITHOUT
// touching the DB row. Mirrors the resource_tracks override convention,
// extended to dice_core. Self-contained: throwaway ruleset + temp data dir.
#[tokio::test]
async fn dice_core_success_bands_override_heals_missing_critical() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect: {e}"); return; } };
    const RS: &str = "ut_dice_override_7e";

    // Temp TRPG_DATA_DIR holding ONLY this ruleset's override file.
    let dir = std::env::temp_dir().join(format!("trpg_dice_override_{}", std::process::id()));
    let rules_dir = dir.join("parsed").join("rules");
    std::fs::create_dir_all(&rules_dir).unwrap();
    std::fs::write(
        rules_dir.join(format!("{RS}.rule_kernel.override.json")),
        serde_json::to_vec(&serde_json::json!({
            "dice_core": { "success_bands": [
                {"id":"critical","label":"Critical Success","rank":5,"test":{"kind":"exact","value":1}},
                {"id":"regular","label":"Success","rank":2,"test":{"kind":"roll_under_or_equal"}},
                {"id":"failure","label":"Failure","rank":1,"test":{"kind":"otherwise"}}
            ] }
        })).unwrap(),
    ).unwrap();
    let prev = std::env::var("TRPG_DATA_DIR").ok();
    std::env::set_var("TRPG_DATA_DIR", &dir);

    // Seed a throwaway kernel with the BROKEN bands (no `critical`) — what a bad
    // re-parse leaves behind.
    let mut kernel = trpg_model::RuleKernel { kernel_id: format!("kernel_{RS}"), ruleset_id: RS.into(), version: "1".into(), ..Default::default() };
    kernel.dice_core = serde_json::json!({
        "compare": "roll_under",
        "success_bands": [
            {"id":"regular","rank":2,"test":{"kind":"roll_under_or_equal"}},
            {"id":"extreme","rank":4,"test":{"kind":"roll_under_fraction","numerator":1,"denominator":5}}
        ]
    });
    db.upsert_rule_kernel(&kernel).await.unwrap();

    let loaded = db.load_rule_kernel(RS).await.unwrap().expect("kernel loaded");
    let ids: Vec<String> = loaded.dice_core.get("success_bands").and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|b| b.get("id").and_then(|x| x.as_str()).map(String::from)).collect())
        .unwrap_or_default();
    // Cleanup BEFORE asserting so a failure still leaves a clean DB/env.
    sqlx::query("delete from rule_kernels where ruleset_id=$1").bind(RS).execute(&db.pool).await.ok();
    match prev { Some(v) => std::env::set_var("TRPG_DATA_DIR", v), None => std::env::remove_var("TRPG_DATA_DIR") }
    std::fs::remove_dir_all(&dir).ok();

    assert!(ids.iter().any(|i| i == "critical"), "override must inject the missing `critical` band; got {ids:?}");
    assert!(ids.iter().any(|i| i == "failure"), "override should replace the whole success_bands array; got {ids:?}");
    assert!(!ids.iter().any(|i| i == "extreme"), "override REPLACES dice_core.success_bands wholesale; stale `extreme` must be gone; got {ids:?}");
}

#[tokio::test]
async fn dnd_kernel_has_valid_hp_track_after_override() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect: {e}"); return; } };
    let kernel = match db.load_rule_kernel(DND).await.ok().flatten() { Some(k) => k, None => { eprintln!("SKIP: no {DND} kernel in this DB"); return; } };
    // The override only applies when TRPG_DATA_DIR (default "data", relative to the
    // test process cwd) contains the dnd5e override file. Skip — don't fail — when
    // it's not reachable, so `cargo test --workspace` without TRPG_DATA_DIR is clean.
    let dir = std::env::var("TRPG_DATA_DIR").unwrap_or_else(|_| "data".into());
    let ov = std::path::Path::new(&dir).join("parsed/rules/dnd5e.rule_kernel.override.json");
    if !ov.exists() { eprintln!("SKIP: dnd5e override not found at {ov:?} (set TRPG_DATA_DIR to the D&D data dir)"); return; }
    assert!(kernel.resource_tracks.iter().all(|t| t.get("field_id").is_none()), "sheet-field track survived normalize");
    assert_eq!(trpg_model::hp_resource_track_id(&kernel.resource_tracks).as_deref(), Some("hit_points"), "D&D HP track not resolvable after override");
}

// Triangle Agency: the pass-one reader extracted commendations/demerits as SHEET
// fields but omitted them from core.resource_tracks (anchored on the survival-meter
// examples), so the GM's apply_effect/change_track had no track to resolve and the
// reward/discipline economy was un-awardable. The data-only override must inject
// both ECONOMY tracks at read time without clobbering the extracted `chaos` track.
#[tokio::test]
async fn triangle_kernel_gains_commendation_demerit_economy_tracks_via_override() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect: {e}"); return; } };
    let kernel = match db.load_rule_kernel("triangle_agency").await.ok().flatten() { Some(k) => k, None => { eprintln!("SKIP: no triangle_agency kernel in this DB"); return; } };
    let dir = std::env::var("TRPG_DATA_DIR").unwrap_or_else(|_| "data".into());
    let ov = std::path::Path::new(&dir).join("parsed/rules/triangle_agency.rule_kernel.override.json");
    if !ov.exists() { eprintln!("SKIP: triangle override not found at {ov:?} (set TRPG_DATA_DIR to the Triangle data dir)"); return; }
    let track = |id: &str| kernel.resource_tracks.iter().find(|t| t.get("id").and_then(|v| v.as_str()) == Some(id));
    let comm = track("commendations").expect("commendations economy track missing after override");
    let dem = track("demerits").expect("demerits economy track missing after override");
    for (label, t) in [("commendations", comm), ("demerits", dem)] {
        assert_eq!(t.get("owner_kind").and_then(|v| v.as_str()), Some("actor"), "{label} must be a per-Agent track");
        assert_eq!(t.get("initial").and_then(|v| v.as_i64()), Some(0), "{label} must seed at 0 so load_resource_current resolves without a sheet seed");
    }
    assert!(dem.get("thresholds").and_then(|v| v.as_array()).map(|a| !a.is_empty()).unwrap_or(false),
        "demerits must carry the Agency Standing ladder as thresholds");
    assert!(track("chaos").is_some(), "override merge must not clobber the reader-extracted chaos track");
}

// CoC 7e: the reader extracted the survival meters (sanity/hp) but omitted the
// book's two runtime SPEND economies — Luck (spend 1-for-1 after a skill roll to
// alter the result, cap 99) and Magic Points (spell casting, regen 1/hour, POW/5).
// The data-only override must inject both at read time without clobbering the
// extracted tracks. Both are judgment spends: NO on_outcome, and NO static initial
// (per-investigator values seed from the character sheet, fail-closed).
#[tokio::test]
async fn coc_kernel_gains_luck_and_magic_point_economy_tracks_via_override() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect: {e}"); return; } };
    let kernel = match db.load_rule_kernel("call_of_cthulhu_7e").await.ok().flatten() { Some(k) => k, None => { eprintln!("SKIP: no call_of_cthulhu_7e kernel in this DB"); return; } };
    let dir = std::env::var("TRPG_DATA_DIR").unwrap_or_else(|_| "data".into());
    let ov = std::path::Path::new(&dir).join("parsed/rules/call_of_cthulhu_7e.rule_kernel.override.json");
    if !ov.exists() { eprintln!("SKIP: CoC override not found at {ov:?} (set TRPG_DATA_DIR to the CoC data dir)"); return; }
    let track = |id: &str| kernel.resource_tracks.iter().find(|t| t.get("id").and_then(|v| v.as_str()) == Some(id));
    let luck = track("luck").expect("luck spend-economy track missing after override");
    let mp = track("mp").expect("mp (Magic Points) spend-economy track missing after override");
    assert_eq!(luck.get("max").and_then(|v| v.as_i64()), Some(99), "Luck may never exceed 99");
    for (label, t) in [("luck", luck), ("mp", mp)] {
        assert!(t.get("on_outcome").is_none(), "{label} is a judgment-spend economy: an on_outcome would be a dead rule");
        assert!(t.get("initial").is_none(), "{label} is per-investigator: no static initial (fail-closed sheet seed)");
    }
    assert!(track("sanity").is_some() && track("hp").is_some(), "override merge must not clobber reader-extracted sanity/hp");
}

// BRP (ORC document): the reader extracted NOTHING into resource_tracks even
// though the book defines four numeric pools and the sheet template already
// derives hit_points/power_points. After the override, combat must resolve an HP
// track semantically and the power-point spend economy must be addressable; the
// optional systems (fatigue, sanity) ride along with judgment-driven losses only.
#[tokio::test]
async fn orc_kernel_gains_core_pools_via_override() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect: {e}"); return; } };
    let kernel = match db.load_rule_kernel("brp_orc").await.ok().flatten() { Some(k) => k, None => { eprintln!("SKIP: no brp_orc kernel in this DB"); return; } };
    let dir = std::env::var("TRPG_DATA_DIR").unwrap_or_else(|_| "data".into());
    let ov = std::path::Path::new(&dir).join("parsed/rules/brp_orc.rule_kernel.override.json");
    if !ov.exists() { eprintln!("SKIP: BRP override not found at {ov:?} (set TRPG_DATA_DIR to the ORC data dir)"); return; }
    assert_eq!(trpg_model::hp_resource_track_id(&kernel.resource_tracks).as_deref(), Some("hit_points"),
        "BRP combat HP must be semantically resolvable after the override (kernel shipped EMPTY)");
    let track = |id: &str| kernel.resource_tracks.iter().find(|t| t.get("id").and_then(|v| v.as_str()) == Some(id));
    assert!(track("power_points").is_some(), "power_points spend economy missing after override");
    for id in ["power_points", "fatigue_points", "sanity"] {
        let t = track(id).unwrap_or_else(|| panic!("{id} track missing after override"));
        assert!(t.get("on_outcome").is_none(), "{id} losses are GM adjudication: no on_outcome");
    }
}

// D&D 5e: Inspiration is the PHB's only universal DM-award/spend economy (cap 1,
// no stacking, spend for advantage); it was absent from the kernel. The override
// must add it NEXT TO the slice-B hit_points heal, not instead of it. XP stays in
// the growth track layer on purpose, so it must NOT appear as a kernel track.
#[tokio::test]
async fn dnd_kernel_gains_inspiration_economy_track_via_override() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect: {e}"); return; } };
    let kernel = match db.load_rule_kernel(DND).await.ok().flatten() { Some(k) => k, None => { eprintln!("SKIP: no {DND} kernel in this DB"); return; } };
    let dir = std::env::var("TRPG_DATA_DIR").unwrap_or_else(|_| "data".into());
    let ov = std::path::Path::new(&dir).join("parsed/rules/dnd5e.rule_kernel.override.json");
    if !ov.exists() { eprintln!("SKIP: dnd5e override not found at {ov:?} (set TRPG_DATA_DIR to the D&D data dir)"); return; }
    let track = |id: &str| kernel.resource_tracks.iter().find(|t| t.get("id").and_then(|v| v.as_str()) == Some(id));
    let insp = track("inspiration").expect("inspiration economy track missing after override");
    assert_eq!(insp.get("initial").and_then(|v| v.as_i64()), Some(0), "inspiration starts unawarded");
    assert_eq!(insp.get("max").and_then(|v| v.as_i64()), Some(1), "you either have inspiration or you don't — it does not stack");
    assert!(insp.get("on_outcome").is_none(), "inspiration is a pure GM-judgment economy: no on_outcome");
    assert_eq!(trpg_model::hp_resource_track_id(&kernel.resource_tracks).as_deref(), Some("hit_points"),
        "inspiration must not displace the slice-B HP heal");
    assert!(track("xp").is_none() && track("experience_points").is_none(),
        "XP is growth-layer state (sheet_json.tracks), never a kernel track");
}

// Cyberpunk RED: the kernel had only hp/wound_state. LUCK (spend 1-for-1 before a
// roll, pool = LUCK STAT, refills each session) and Humanity (cyberware loss /
// therapy recovery, EMP = HUM/10, cyberpsychosis ladder) are the two runtime
// pools the GM operates; IP and REP stay in the growth layer on purpose.
#[tokio::test]
async fn cyberpunk_kernel_gains_luck_and_humanity_via_override() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect: {e}"); return; } };
    let kernel = match db.load_rule_kernel("cyberpunk_red").await.ok().flatten() { Some(k) => k, None => { eprintln!("SKIP: no cyberpunk_red kernel in this DB"); return; } };
    let dir = std::env::var("TRPG_DATA_DIR").unwrap_or_else(|_| "data".into());
    let ov = std::path::Path::new(&dir).join("parsed/rules/cyberpunk_red.rule_kernel.override.json");
    if !ov.exists() { eprintln!("SKIP: cyberpunk override not found at {ov:?} (set TRPG_DATA_DIR to the cyberpunk data dir)"); return; }
    let track = |id: &str| kernel.resource_tracks.iter().find(|t| t.get("id").and_then(|v| v.as_str()) == Some(id));
    let luck = track("luck").expect("luck spend-pool track missing after override");
    let hum = track("humanity").expect("humanity track missing after override");
    for (label, t) in [("luck", luck), ("humanity", hum)] {
        assert!(t.get("on_outcome").is_none(), "{label} spends/losses are GM adjudication: no on_outcome");
        assert!(t.get("initial").is_none(), "{label} is per-character (LUCK stat / EMP x10): no static initial");
    }
    assert!(hum.get("thresholds").and_then(|v| v.as_array()).map(|a| !a.is_empty()).unwrap_or(false),
        "humanity must carry the cyberpsychosis threshold");
    assert!(trpg_model::hp_resource_track_id(&kernel.resource_tracks).is_some(),
        "override merge must not clobber the reader-extracted HP track");
    assert!(track("improvement_points").is_none() && track("ip").is_none(),
        "IP is growth-layer state (sheet_json.tracks), never a kernel track");
}

// End-to-end proof that the D&D fix makes combat HP NON-None: a D&D actor with a
// character-derived hit_points resolves a real current value via the SSOT primitive
// (formerly None, because the dirty kernel had no valid HP track). Throwaway session.
#[tokio::test]
async fn dnd_actor_resolves_real_hp_after_fix() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect: {e}"); return; } };
    let kernel = match db.load_rule_kernel(DND).await.ok().flatten() { Some(k) => k, None => { eprintln!("SKIP: no {DND} kernel"); return; } };
    let dir = std::env::var("TRPG_DATA_DIR").unwrap_or_else(|_| "data".into());
    if !std::path::Path::new(&dir).join("parsed/rules/dnd5e.rule_kernel.override.json").exists() {
        eprintln!("SKIP: dnd5e override not reachable (set TRPG_DATA_DIR)"); return;
    }
    let hp_id = match trpg_model::hp_resource_track_id(&kernel.resource_tracks) { Some(id) => id, None => { eprintln!("SKIP: no HP track"); return; } };
    const SESSION: &str = "sess_slice_b_dnd_hp";
    const ACTOR: &str = "pc.slice_b_dnd";
    sqlx::query("delete from generic_parameter_states where session_id=$1 and target_id=$2").bind(SESSION).bind(ACTOR).execute(&db.pool).await.unwrap();
    sqlx::query(r#"insert into runtime_actor_parameters
        (id, actor_param_id, session_id, actor_id, actor_kind, ruleset_id, source_kind, template_id, display_name, sheet_json, mechanical_profile, status_json, visibility, created_at_tick, updated_at_tick)
        values (gen_random_uuid(), $1, $2, $3, 'player_character', $4, 'test_seed', null, $3, $5, '{}'::jsonb, '{}'::jsonb, 'gm_only', 0, 0)
        on conflict (session_id, actor_id) do update set sheet_json=excluded.sheet_json, ruleset_id=excluded.ruleset_id"#)
        .bind(format!("ap_{ACTOR}")).bind(SESSION).bind(ACTOR).bind(DND)
        .bind(serde_json::json!({"resources": {"hit_points": 30}}))
        .execute(&db.pool).await.unwrap();
    // No gps row yet -> falls back to the character-derived seed (30), not None.
    assert_eq!(db.load_resource_current(SESSION, ACTOR, &hp_id, &kernel).await, Some(30),
        "D&D combat HP must resolve the derived hit_points (non-None) after the kernel override fix");
    sqlx::query("delete from runtime_actor_parameters where session_id=$1 and actor_id=$2").bind(SESSION).bind(ACTOR).execute(&db.pool).await.unwrap();
}

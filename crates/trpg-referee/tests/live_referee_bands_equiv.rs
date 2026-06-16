//! Equivalence tests: referee value-bands produce identical plausibility verdicts
//! before and after the migration (hardcoded strings now come from kernel data).
//!
//! Requires a live DB; skips silently when DATABASE_URL is unset.
//!   DATABASE_URL=postgres://postgres:password@localhost:54347/chatrpg \
//!     cargo test -p trpg-referee --test live_referee_bands_equiv -- --nocapture

#[tokio::test]
async fn coc_bands_equiv() {
    let Ok(url) = std::env::var("DATABASE_URL") else { return; };
    let db = match trpg_db::Db::connect(&url).await {
        Ok(d) => d,
        Err(_) => { eprintln!("SKIP: cannot connect to DB"); return; }
    };
    let Some(kernel) = db.load_rule_kernel("call_of_cthulhu_7e").await.unwrap_or(None) else {
        eprintln!("SKIP: no call_of_cthulhu_7e kernel"); return;
    };
    let bands = kernel.referee_value_bands.as_ref()
        .expect("override should supply referee_value_bands for CoC");
    // Latent behavior: call_of_cthulhu_7e did NOT match contains("coc") -> GENERIC
    assert_eq!(bands.damage_family, "generic_trpg_damage",
        "CoC latent behavior: GENERIC (contains('coc') is FALSE for canonical id)");
    assert_eq!(bands.damage_plausible_range, (1, 30));
    assert_eq!(bands.difficulty_plausible_range, (1, 100));
}

#[tokio::test]
async fn cyberpunk_bands_equiv() {
    let Ok(url) = std::env::var("DATABASE_URL") else { return; };
    let db = match trpg_db::Db::connect(&url).await {
        Ok(d) => d,
        Err(_) => { eprintln!("SKIP: cannot connect to DB"); return; }
    };
    let Some(kernel) = db.load_rule_kernel("cyberpunk_red").await.unwrap_or(None) else {
        eprintln!("SKIP: no cyberpunk_red kernel"); return;
    };
    let bands = kernel.referee_value_bands.as_ref()
        .expect("override should supply referee_value_bands for Cyberpunk");
    assert_eq!(bands.damage_family, "cyberpunk_red_weapon_damage");
    assert_eq!(bands.damage_plausible_range, (1, 12), "cyberpunk: orig max was 12");
    assert_eq!(bands.difficulty_plausible_range, (5, 35));
    // DV 17 ok, DV 50 not
    assert!(17 >= bands.difficulty_plausible_range.0 && 17 <= bands.difficulty_plausible_range.1);
    assert!(!(50 >= bands.difficulty_plausible_range.0 && 50 <= bands.difficulty_plausible_range.1));
}

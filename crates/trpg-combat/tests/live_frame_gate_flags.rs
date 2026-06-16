//! FIX 1 (P0-2 复审): triangle frame-gate flags must resolve TRUE from the LIVE
//! advice profile (the missed gap — the unit tests fed literal flags, not the
//! resolved profile). Legacy behavior: `ruleset_id.contains("triangle")` made
//! BOTH `low_confidence_frame_start` and `investigate_opens_frame` true for
//! triangle_agency ONLY; every other ruleset stayed false.

use std::path::PathBuf;
use trpg_combat::CombatProfilePack;

/// Worktree advice dir (the live data path), resolved via the `data/` symlink.
fn advice_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("data")
        .join("ruleset_advice")
}

#[test]
fn live_triangle_profile_opens_frame_and_allows_low_confidence() {
    let dir = advice_dir();
    // The advice dir lives under the un-versioned data symlink. Require it: the
    // whole point of this test is to exercise the real resolved profile.
    assert!(
        dir.exists(),
        "advice dir missing at {} — cannot exercise live profile",
        dir.display()
    );
    let pack = CombatProfilePack::load_dir(&dir).expect("load advice dir");
    let tri = pack.resolve("triangle_agency", None);
    assert!(
        tri.low_confidence_frame_start,
        "triangle low_confidence_frame_start must be TRUE from live advice (legacy contains(\"triangle\"))"
    );
    assert!(
        tri.investigate_opens_frame,
        "triangle investigate_opens_frame must be TRUE from live advice (legacy contains(\"triangle\"))"
    );

    // Equivalence guard: ONLY triangle gets them. Another curated ruleset must
    // resolve both flags to false (legacy = contains(\"triangle\") only).
    let cp = pack.resolve("cyberpunk_red", None);
    assert!(
        !cp.low_confidence_frame_start,
        "cyberpunk_red low_confidence_frame_start must stay false"
    );
    assert!(
        !cp.investigate_opens_frame,
        "cyberpunk_red investigate_opens_frame must stay false"
    );
}

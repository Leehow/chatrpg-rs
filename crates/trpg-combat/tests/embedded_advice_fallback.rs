//! P0-2 clean-checkout equivalence: with NO advice dir present, the binary-
//! embedded advice profiles must reproduce the migrated values. `load_dir` of a
//! nonexistent path forces the embedded baseline (no data/ on disk).

use trpg_combat::CombatProfilePack;

#[test]
fn embedded_profiles_resolve_triangle_frame_flags_without_dir() {
    // A path that cannot exist → load_dir returns the embedded baseline.
    let pack =
        CombatProfilePack::load_dir("/nonexistent/trpg-no-advice-dir").expect("embedded baseline");

    // Triangle frame-gate flags must still be TRUE from the embedded copy (it
    // was copied from the fixed data/ file).
    let tri = pack.resolve("triangle_agency", None);
    assert!(
        tri.low_confidence_frame_start,
        "embedded triangle low_confidence_frame_start must be TRUE"
    );
    assert!(
        tri.investigate_opens_frame,
        "embedded triangle investigate_opens_frame must be TRUE"
    );

    // Equivalence guard: a different ruleset keeps both flags false (legacy
    // contains(\"triangle\") only).
    let cp = pack.resolve("cyberpunk_red", None);
    assert!(
        !cp.low_confidence_frame_start,
        "cyberpunk low_confidence stays false"
    );
    assert!(
        !cp.investigate_opens_frame,
        "cyberpunk investigate stays false"
    );

    // The cyberpunk profile resolves to the embedded cyberpunk advice profile.
    assert_eq!(cp.ruleset_id, "cyberpunk_red");
    assert_eq!(cp.profile_id, "cyberpunk_red.firefight.v1_3");
}

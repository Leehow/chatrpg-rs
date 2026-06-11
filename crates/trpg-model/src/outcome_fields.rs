//! Check-outcome JSON field vocabulary — the single source of truth shared by
//! the emitter (trpg-contest `resolve_outcome`, which uses these consts AS its
//! JSON keys) and the validators (trpg-rule-agent `mechanics_finalize`'s
//! on_outcome `=field` reference guard). Because both sides name the same
//! consts, the legal set cannot drift from what the engine actually emits.
//!
//! Only fields that `resolve_track_amount` can read as an amount belong here:
//! numbers and booleans (bool coerces to 1/0). String/array/object outcome
//! fields (check_label, degree, success_tier, dice, opposed, ...) resolve to
//! None at runtime and are deliberately NOT part of this vocabulary.

pub const TOTAL: &str = "total";
pub const TARGET: &str = "target";
pub const SUCCESS: &str = "success";
pub const SUCCESS_COUNT: &str = "success_count";
pub const POOL_MISS_COUNT: &str = "pool_miss_count";
pub const SUCCESS_TIER_RANK: &str = "success_tier_rank";

/// The legal vocabulary for `=<field>` amounts and `when` fields in
/// `resource_tracks[*].on_outcome[*]`.
pub const AMOUNT_RESOLVABLE: &[&str] =
    &[TOTAL, TARGET, SUCCESS, SUCCESS_COUNT, POOL_MISS_COUNT, SUCCESS_TIER_RANK];

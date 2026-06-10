//! 通用对抗结算(数据驱动,零 per-ruleset):按 kernel `compare` 方向算双方成败,
//! 用 `success_bands` 的 tier rank 比质量。fail-closed:缺值返 (None,None,None)。
use serde_json::Value;
use crate::success_tier_for; // lib.rs 中的既有纯函数(本任务改为 pub(crate))

/// 算一侧是否成功(compare 方向数据驱动)。
fn side_succeeds(compare: &str, total: i64, value: i32) -> bool {
    match compare {
        "meet_or_beat" => total >= value as i64,
        _ /* roll_under */ => total <= value as i64,
    }
}

/// 一侧的 (是否成功, tier rank, margin)。margin:两种 compare 都"越大越好"。
fn side_rank(compare: &str, bands: &[Value], total: i64, value: i32) -> (bool, i64, i64) {
    let succeeds = side_succeeds(compare, total, value);
    let tier = success_tier_for(bands, total, value as i64).map(|(_, r)| r).unwrap_or(0);
    let margin = match compare {
        "meet_or_beat" => total - value as i64,
        _ => value as i64 - total,
    };
    (succeeds, tier, margin)
}

/// 通用对抗结算。返回 (target, success=attacker_wins, degree)。
/// fail-closed:任一值缺 → (None,None,None)。平局 → 防御方胜
/// (engine convention — ties favor the defender / status quo; a future kernel field may override)。
pub fn resolve_opposed(
    compare: &str, bands: &[Value],
    atk_total: i64, atk_value: Option<i32>,
    def_total: i64, def_value: Option<i32>,
) -> (Option<i64>, Option<bool>, Option<String>) {
    let (av, dv) = match (atk_value, def_value) { (Some(a), Some(d)) => (a, d), _ => return (None, None, None) };
    let a = side_rank(compare, bands, atk_total, av);
    let d = side_rank(compare, bands, def_total, dv);
    let (atk_ok, def_ok) = (a.0, d.0);
    let attacker_wins = match (atk_ok, def_ok) {
        (true, false) => true,
        (false, true) => false,
        (false, false) => false, // 双败:主动方未达成 → 防御方胜(status quo)
        (true, true) => {
            if a.1 != d.1 { a.1 > d.1 }        // 比 tier rank
            else if a.2 != d.2 { a.2 > d.2 }   // 再比 margin
            else { false }                      // 平局归防御方
        }
    };
    let degree = Some(if !atk_ok && !def_ok { "mutual_failure".to_string() }
        else if attacker_wins { "attacker_wins".to_string() }
        else { "defender_wins".to_string() });
    (None, Some(attacker_wins), degree)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    // 真实 CoC bands:无 otherwise/critical;失败 roll → success_tier_for 返 None。
    fn coc_bands() -> Vec<Value> {
        serde_json::from_value(json!([
            {"id":"regular","rank":1,"test":{"kind":"roll_under_or_equal"}},
            {"id":"hard","rank":2,"test":{"kind":"roll_under_fraction","denominator":2}},
            {"id":"extreme","rank":3,"test":{"kind":"roll_under_fraction","denominator":5}},
            {"id":"fumble","rank":0,"test":{"kind":"in_range","min":96,"max":100}}
        ])).unwrap()
    }

    #[test]
    fn missing_value_fails_closed() {
        let (t, s, d) = resolve_opposed("roll_under", &coc_bands(), 30, None, 50, Some(60));
        assert_eq!((t, s, d), (None, None, None));
        let (t, s, _) = resolve_opposed("roll_under", &coc_bands(), 30, Some(60), 50, None);
        assert_eq!((t, s), (None, None));
    }

    #[test]
    fn one_succeeds_one_fails_winner_is_success_side() {
        let (_t, s, _d) = resolve_opposed("roll_under", &coc_bands(), 30, Some(60), 80, Some(40));
        assert_eq!(s, Some(true));
        let (_t, s, _d) = resolve_opposed("roll_under", &coc_bands(), 80, Some(60), 30, Some(40));
        assert_eq!(s, Some(false));
    }

    #[test]
    fn both_succeed_higher_tier_wins() {
        // 攻击 5<=60(extreme: <=12);防御 35<=40(regular)。攻击 tier 高 → 胜。
        let (_t, s, _d) = resolve_opposed("roll_under", &coc_bands(), 5, Some(60), 35, Some(40));
        assert_eq!(s, Some(true));
    }

    #[test]
    fn both_fail_defender_wins_status_quo() {
        let (_t, s, d) = resolve_opposed("roll_under", &coc_bands(), 90, Some(60), 95, Some(40));
        assert_eq!(s, Some(false));
        assert_eq!(d.as_deref(), Some("mutual_failure"));
    }

    #[test]
    fn exact_tie_defender_wins_engine_convention() {
        // 双方成功、同 tier(都 regular)、同 margin(value-total 都=10)→ 引擎约定防御方胜。
        let (_t, s, _d) = resolve_opposed("roll_under", &coc_bands(), 50, Some(60), 30, Some(40));
        assert_eq!(s, Some(false), "tie favors defender (engine convention)");
    }

    #[test]
    fn meet_or_beat_direction() {
        let bands: Vec<Value> = vec![];
        let (_t, s, _d) = resolve_opposed("meet_or_beat", &bands, 18, Some(10), 11, Some(10));
        assert_eq!(s, Some(true));
    }
}

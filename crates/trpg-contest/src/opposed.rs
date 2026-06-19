//! 通用对抗结算(数据驱动,零 per-ruleset):按 kernel `compare` 方向算双方成败,
//! 用 `success_bands` 的 tier rank 比质量。fail-closed:缺值返 (None,None,None)。
use crate::success_tier_for;
use serde_json::Value; // lib.rs 中的既有纯函数(本任务改为 pub(crate))

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
    let tier = success_tier_for(bands, total, value as i64)
        .map(|(_, r)| r)
        .unwrap_or(0);
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
    compare: &str,
    bands: &[Value],
    atk_total: i64,
    atk_value: Option<i32>,
    def_total: i64,
    def_value: Option<i32>,
) -> (Option<i64>, Option<bool>, Option<String>) {
    let (av, dv) = match (atk_value, def_value) {
        (Some(a), Some(d)) => (a, d),
        _ => return (None, None, None),
    };
    let a = side_rank(compare, bands, atk_total, av);
    let d = side_rank(compare, bands, def_total, dv);
    let (atk_ok, def_ok) = (a.0, d.0);
    let attacker_wins = match (atk_ok, def_ok) {
        (true, false) => true,
        (false, true) => false,
        (false, false) => false, // 双败:主动方未达成 → 防御方胜(status quo)
        (true, true) => {
            if a.1 != d.1 {
                a.1 > d.1
            }
            // 比 tier rank
            else if a.2 != d.2 {
                a.2 > d.2
            }
            // 再比 margin
            else {
                false
            } // 平局归防御方
        }
    };
    let degree = Some(if !atk_ok && !def_ok {
        "mutual_failure".to_string()
    } else if attacker_wins {
        "attacker_wins".to_string()
    } else {
        "defender_wins".to_string()
    });
    (None, Some(attacker_wins), degree)
}

/// 通用骰池对抗(count_faces,如 Triangle):双方各掷自己的池,数出现 `target_face` 的骰子
/// (hits),hits 多者胜。`threshold` 是"该侧算达成"的下限(默认 ≥1):双方都未达阈值 → status
/// quo 防御方守成(degree=mutual_failure)。平局(都达阈值、hits 相等)→ 防御方胜
/// (engine convention,与 resolve_opposed 一致)。fail-closed:任一池为空(未掷)→ (None,None,None)。
/// 返回 (target=None, success=attacker_wins, degree)——形态与 resolve_opposed 对齐,供
/// resolve_outcome 直接覆盖 target/success/degree。
pub fn resolve_pool_opposed(
    target_face: i32,
    threshold: i32,
    atk_rolls: &[i64],
    def_rolls: &[i64],
) -> (Option<i64>, Option<bool>, Option<String>) {
    if atk_rolls.is_empty() || def_rolls.is_empty() {
        return (None, None, None);
    }
    let face = target_face as i64;
    let atk_hits = atk_rolls.iter().filter(|&&d| d == face).count() as i64;
    let def_hits = def_rolls.iter().filter(|&&d| d == face).count() as i64;
    let (atk_ok, def_ok) = (atk_hits >= threshold as i64, def_hits >= threshold as i64);
    let attacker_wins = match (atk_ok, def_ok) {
        (true, false) => true,
        (false, true) => false,
        (false, false) => false, // 双方未达阈值:防御方守成(status quo)
        (true, true) => atk_hits > def_hits, // 都达阈值:hits 多者胜,平局归防御方
    };
    let degree = Some(if !atk_ok && !def_ok {
        "mutual_failure".to_string()
    } else if attacker_wins {
        "attacker_wins".to_string()
    } else {
        "defender_wins".to_string()
    });
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
        ]))
        .unwrap()
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

    // ─── count_faces 骰池对抗(Triangle):双方各掷池、数 hits 定胜负 ───

    #[test]
    fn pool_opposed_more_hits_wins() {
        // 面=3,阈值=1。攻击池 [3,3,1,2,4,3]=3 hits;防御池 [3,1,2,2,4,1]=1 hit → 攻击方胜。
        let (t, s, d) = resolve_pool_opposed(3, 1, &[3, 3, 1, 2, 4, 3], &[3, 1, 2, 2, 4, 1]);
        assert_eq!(t, None, "对抗无单一 target 数,target=None");
        assert_eq!(s, Some(true), "攻击 3 hits > 防御 1 hit → 攻击方胜");
        assert_eq!(d.as_deref(), Some("attacker_wins"));
    }

    #[test]
    fn pool_opposed_fewer_hits_loses() {
        // 攻击 1 hit、防御 4 hits → 防御方胜(攻击被压制)。
        let (_t, s, d) = resolve_pool_opposed(3, 1, &[3, 1, 2, 2, 4, 1], &[3, 3, 3, 3, 1, 2]);
        assert_eq!(s, Some(false), "攻击 1 hit < 防御 4 hits → 防御方胜");
        assert_eq!(d.as_deref(), Some("defender_wins"));
    }

    #[test]
    fn pool_opposed_tie_favors_defender() {
        // 双方各 2 hits、都达阈值 → 平局归防御方(engine convention,与 resolve_opposed 一致)。
        let (_t, s, _d) = resolve_pool_opposed(3, 1, &[3, 3, 1, 1], &[3, 3, 2, 4]);
        assert_eq!(s, Some(false), "平局(2=2)→ 防御方胜");
    }

    #[test]
    fn pool_opposed_both_zero_is_mutual_failure_defender_holds() {
        // 双方都 0 hits(都没达阈值)→ status quo,防御方胜,degree=mutual_failure。
        let (_t, s, d) = resolve_pool_opposed(3, 1, &[1, 2, 4, 1], &[2, 4, 1, 2]);
        assert_eq!(s, Some(false), "双 0 hits → 防御方守成");
        assert_eq!(d.as_deref(), Some("mutual_failure"));
    }

    #[test]
    fn pool_opposed_threshold_gates_more_raw_hits() {
        // 阈值=2:攻击 1 hit(未达)、防御 0 hit(未达)→ 双败,防御方胜(即便攻击 hits 更多)。
        let (_t, s, d) = resolve_pool_opposed(3, 2, &[3, 1, 2, 4], &[1, 2, 4, 1]);
        assert_eq!(s, Some(false), "攻击虽 1>0 但未达阈值 2 → 双败,防御方守成");
        assert_eq!(d.as_deref(), Some("mutual_failure"));
    }

    #[test]
    fn pool_opposed_empty_pool_fails_closed() {
        // 任一池为空(未掷)→ fail-closed (None,None,None),绝不乱判胜负。
        assert_eq!(
            resolve_pool_opposed(3, 1, &[], &[3, 3, 1]),
            (None, None, None)
        );
        assert_eq!(
            resolve_pool_opposed(3, 1, &[3, 3, 1], &[]),
            (None, None, None)
        );
    }
}

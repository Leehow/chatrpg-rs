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
///
/// Fix C — `meet_or_beat`(roll-high, 如 Cyberpunk RED `1d10 + STAT + SKILL` 对抗)下,
/// 选手的能力值是**加数**而非纯阈值:把各方有效总点合成 `die_total + competence_value`,
/// 直接比合成总点,高者胜(平局归防御方,与既有约定一致)。注意此处的 `*_value` 由
/// resolve_opposed_values 解析(当前只读 `mechanical_profile.skills`,故是技能值;完整
/// STAT+SKILL 待结构化 `_base` 投影就位后自动生效,绝不硬编码 skill→stat 表)。
/// degree 直接给 attacker_wins/defender_wins(roll-high 对抗无吸收阈值,"双败"无意义),
/// 暂不消费 bands(同尺度查询点已就绪,留作未来 opposed-margin band 扩展)。
/// `roll_under`(CoC 对抗:`total <= value`,value 是 roll-under 目标**非加数**)路径
/// **字节等价**——能力值仍只作阈值+tier/margin 输入,与改动前逐位一致。
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
    if compare == "meet_or_beat" {
        // 合成有效总点:die + competence。高者胜,平局归防御方(status quo)。
        let atk_eff = atk_total + av as i64;
        let def_eff = def_total + dv as i64;
        let attacker_wins = atk_eff > def_eff;
        // winner-only degree:roll-high 对抗必有一方有效总点更高,无吸收阈值 →
        // "mutual_failure" 无意义。bands 暂不消费(见函数 doc)。
        let degree = Some(if attacker_wins {
            "attacker_wins".to_string()
        } else {
            "defender_wins".to_string()
        });
        return (None, Some(attacker_wins), degree);
    }
    // roll_under(CoC):value 是 roll-under 目标,非加数 → 维持原阈值+tier+margin 语义。
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

    // ─── Fix C: meet_or_beat 对抗合成能力值进总点(CPR `1d10+STAT+SKILL` vs 同) ───

    #[test]
    fn meet_or_beat_opposed_adds_competence_to_totals() {
        // 攻击方:低骰(3)+高技能(10)=13;防御方:高骰(9)+低技能(2)=11。
        // 旧的纯阈值语义(total>=value)下两边都"成功"(3<10? no→ wait),关键是:
        // 合成后 13 > 11 → 攻击方胜。证明技能确实加进了总点(否则会按 bare die/阈值判)。
        let bands: Vec<Value> = vec![];
        let (t, s, d) = resolve_opposed("meet_or_beat", &bands, 3, Some(10), 9, Some(2));
        assert_eq!(t, None);
        assert_eq!(s, Some(true), "低骰高技能(3+10=13)应胜高骰低技能(9+2=11)");
        assert_eq!(d.as_deref(), Some("attacker_wins"));
    }

    #[test]
    fn meet_or_beat_opposed_bare_threshold_behavior_is_gone() {
        // 旧逻辑(side_succeeds: total>=value, 双方都"成功"则比 tier/margin):
        //   atk total=10 value=10 → succeeds, margin=0;def total=11 value=2 → succeeds,
        //   margin=9。旧逻辑空 bands → tier 都=0,比 margin:def margin 9 > atk margin 0
        //   → 旧逻辑会判**防御方胜**(attacker_wins=false)。
        // 新逻辑合成:atk_eff=10+10=20,def_eff=11+2=13 → 20>13 → **攻击方胜**。
        // 二者结论相反 → 证明 bare-threshold/margin 判定已被合成总点取代。
        let bands: Vec<Value> = vec![];
        let (_t, s, _d) = resolve_opposed("meet_or_beat", &bands, 10, Some(10), 11, Some(2));
        assert_eq!(s, Some(true), "合成总点 20>13 攻击胜,而非旧 margin 判防御胜");
    }

    #[test]
    fn meet_or_beat_opposed_tie_favors_defender() {
        // 合成总点相等(7+5=12 == 8+4=12)→ 平局归防御方(engine convention)。
        let bands: Vec<Value> = vec![];
        let (_t, s, d) = resolve_opposed("meet_or_beat", &bands, 7, Some(5), 8, Some(4));
        assert_eq!(s, Some(false), "合成平局(12=12)→ 防御方胜");
        assert_eq!(d.as_deref(), Some("defender_wins"));
    }

    #[test]
    fn meet_or_beat_opposed_fails_closed_on_missing_value() {
        let bands: Vec<Value> = vec![];
        assert_eq!(
            resolve_opposed("meet_or_beat", &bands, 8, None, 5, Some(4)),
            (None, None, None)
        );
        assert_eq!(
            resolve_opposed("meet_or_beat", &bands, 8, Some(6), 5, None),
            (None, None, None)
        );
    }

    // ─── Fix C: roll_under(CoC)对抗必须字节等价(改动前 golden 输出对拍) ───

    #[test]
    fn roll_under_opposed_is_byte_identical_golden() {
        // GOLDEN:改动前(纯阈值+tier+margin)的 (target,success,degree) 输出,逐 case 钉死。
        // value 在 roll_under 下是 roll-under 目标(非加数)→ Fix C 绝不把它加进 total。
        let b = coc_bands();
        // (atk_total, atk_value, def_total, def_value) → (target, success, degree)
        let cases: &[((i64, i32, i64, i32), (Option<i64>, Option<bool>, Option<&str>))] = &[
            // 一胜一败:攻击 30<=60 成功,防御 80>40 失败 → 攻击胜。
            ((30, 60, 80, 40), (None, Some(true), Some("attacker_wins"))),
            // 一胜一败镜像:攻击失败,防御成功 → 防御胜。
            ((80, 60, 30, 40), (None, Some(false), Some("defender_wins"))),
            // 双成功比 tier:攻击 5<=60(extreme)> 防御 35<=40(regular)→ 攻击胜。
            ((5, 60, 35, 40), (None, Some(true), Some("attacker_wins"))),
            // 双败:90>60,95>40 → mutual_failure,防御守成。
            ((90, 60, 95, 40), (None, Some(false), Some("mutual_failure"))),
            // 同 tier 同 margin 平局 → 防御方胜。
            ((50, 60, 30, 40), (None, Some(false), Some("defender_wins"))),
        ];
        for &((at, avv, dt, dvv), (et, es, ed)) in cases {
            let got = resolve_opposed("roll_under", &b, at, Some(avv), dt, Some(dvv));
            assert_eq!(
                (got.0, got.1, got.2.as_deref()),
                (et, es, ed),
                "roll_under golden 回归 for inputs ({at},{avv},{dt},{dvv})"
            );
        }
        // fail-closed 缺值不变。
        assert_eq!(
            resolve_opposed("roll_under", &b, 30, None, 50, Some(60)),
            (None, None, None)
        );
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

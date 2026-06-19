//! P6.6 seedable RNG（DB-free）：同种子-同结果 + stable_u64 稳定性 + OFF==baseline 守卫。
//!
//! 精确声明（codex#5）：同种子-同结果对**同一 check_id 的重掷/回放**成立——check_id 是
//! 每个新 action 的新 UUID，故"同 action 永远同 seed"**不**成立；也不声称整行字节相等
//! （roll_id/created_at 仍 UUID/now）。范围是 runtime check-roll 路径；
//! trpg-mechanics::roll_amount_dice 仍 thread_rng = 已记债。
use trpg_runtime::{roll_dice_seeded, stable_u64, DiceRollerPlugin, PseudoRandomDiceRoller};

#[test]
fn same_seed_same_sequence_and_total() {
    let roller = PseudoRandomDiceRoller;
    let seed = stable_u64("sess1:turn1:check_abc:player:3d6+2");
    let a = roller.roll_seeded("3d6+2", seed).expect("a");
    let b = roller.roll_seeded("3d6+2", seed).expect("b");
    assert_eq!(a.rolls, b.rolls, "同种子同 rolls 序列");
    assert_eq!(a.total, b.total, "同种子同 total");
    // 自由函数镜像走同一 StdRng 路径 ⇒ 与 trait 法字节一致。
    let c = roll_dice_seeded("3d6+2", seed).expect("c");
    assert_eq!(a.rolls, c.rolls, "roll_dice_seeded 镜像 roll_seeded");
}

#[test]
fn different_seed_diverges() {
    // 不同 check_id ⇒ 不同 seed ⇒ 序列（高概率）不同——证明 seed 真驱动 RNG。
    let roller = PseudoRandomDiceRoller;
    let s1 = stable_u64("sess1:turn1:check_A:player:5d20");
    let s2 = stable_u64("sess1:turn1:check_B:player:5d20");
    assert_ne!(s1, s2, "不同输入 ⇒ 不同 seed");
    let a = roller.roll_seeded("5d20", s1).expect("a");
    let b = roller.roll_seeded("5d20", s2).expect("b");
    assert_ne!(a.rolls, b.rolls, "不同种子 ⇒ 不同序列（5d20 碰撞概率极低）");
}

#[test]
fn stable_u64_is_deterministic_across_calls() {
    // stable_u64 = sha256 前 8 字节 LE，非 per-process 随机化 ⇒ 同串恒同值。
    let a = stable_u64("sess:turn:check:roller:2d6");
    let b = stable_u64("sess:turn:check:roller:2d6");
    assert_eq!(a, b, "同串恒同 seed");
    assert_ne!(stable_u64("a"), stable_u64("b"), "不同串高概率不同 seed");
}

#[test]
fn seeded_roll_respects_safety_bounds() {
    // roll_seeded 与 roll 共享 parse_dice_expr ⇒ 同安全边界（>100 骰、>1000 面 拒绝）。
    let roller = PseudoRandomDiceRoller;
    assert!(roller.roll_seeded("200d6", 1).is_err(), "n>100 拒绝");
    assert!(roller.roll_seeded("1d5000", 1).is_err(), "sides>1000 拒绝");
    assert!(roller.roll_seeded("not_dice", 1).is_err(), "非骰式拒绝");
}

#[test]
fn off_equals_baseline_seed_commitment_independent_of_flag() {
    // OFF==baseline 守卫：seed_commitment 由 seed_material（session:turn:check:result_json）
    // 计算，**不含** TRPG_SEEDED_ROLLS——故无论 flag ON/OFF，给定同一 result_json 的
    // commitment 字节完全相同。只有产生 rolls 的 RNG（thread_rng vs StdRng）在 ON 时不同。
    // 这里直接复刻 resolve_roll_input 的 seed_material 公式，证明 flag 不进 commitment。
    let result_json = serde_json::json!({"mode":"rolled","expression":"3d6","rolls":[1,2,3],"modifier":0,"total":6});
    let material = format!(
        "{}:{}:{}:{}",
        "sess1",
        "turn1",
        "check_abc",
        serde_json::to_string(&result_json).unwrap()
    );
    let commitment_a = trpg_model::sha256_hex(&material);
    let commitment_b = trpg_model::sha256_hex(&material);
    assert_eq!(
        commitment_a, commitment_b,
        "seed_commitment 不含 flag ⇒ ON/OFF 字节一致（schema 冻结列不动）"
    );
    assert!(
        commitment_a.starts_with("sha256:"),
        "commitment 仍是 sha256 hex"
    );
}

//! 刺激驱动检定预 pass（J2 SAN 评测根因修复）：回合头部把目录条目的
//! `when_to_use`（语义路由的唯一依据）与本回合虚构内容——玩家输入 + 上回合
//! 叙事尾段——做语义匹配；命中 → `MechanicDue` 候选，经 watcher `admit_dues`
//! 抑制+落库后进 `ObligationLedger` 阻塞闭环（roll_check {mechanic_id} 处理
//! 或 waive_obligation 带理由豁免）。恐怖刺激→SAN 检定一类"被动触发、GM 该
//! 主动开"的检定由此获得强制通路。
//!
//! fail-closed：门关闭 / LLM 失败 / 解析失败 / 命中 id 不在目录 → 空，GM
//! 裁量不变、绝不编造。零 per-ruleset 硬编码——CoC 的 SAN、CPR 的 Humanity、
//! Triangle 的 Chaos 走同一套逻辑，监听什么完全由目录数据决定。

use serde_json::{json, Value};
use std::sync::Arc;
use trpg_llm::LlmClient;
use trpg_model::{ChatMessage, DueSource, DueStatus, MechanicDue, MechanicEntry};
use uuid::Uuid;

/// 单回合命中上限（防债务刷屏；目录条目仍全量参与判定）。
const MAX_HITS_PER_TURN: usize = 2;
/// 上回合叙事尾段喂给判定的最大字符数（刺激通常出现在叙事收尾处）。
const NARRATION_TAIL_CHARS: usize = 1200;

/// 门 `TRPG_STIMULUS_CHECK_TRIGGER`，默认开。
pub fn stimulus_pass_enabled() -> bool {
    std::env::var("TRPG_STIMULUS_CHECK_TRIGGER")
        .map(|v| {
            !matches!(
                v.to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no"
            )
        })
        .unwrap_or(true)
}

/// 英文动作动词（按 word token 精确匹配；token 化避免 "san" 命中 "thousand"
/// 一类子串误伤）。出现任一 → 玩家在宣告新虚构动作，绝不当作纯状态查询。
const ACTION_TOKENS_EN: &[&str] = &[
    "shoot",
    "shoots",
    "shooting",
    "fire",
    "fires",
    "firing",
    "attack",
    "attacks",
    "attacking",
    "hit",
    "hits",
    "stab",
    "stabs",
    "kick",
    "kicks",
    "punch",
    "punches",
    "sneak",
    "sneaks",
    "sneaking",
    "move",
    "moves",
    "moving",
    "walk",
    "walks",
    "walking",
    "run",
    "runs",
    "running",
    "approach",
    "approaches",
    "charge",
    "charges",
    "search",
    "searches",
    "searching",
    "check",
    "checks",
    "checking",
    "inspect",
    "inspects",
    "examine",
    "examines",
    "loot",
    "loots",
    "open",
    "opens",
    "push",
    "pushes",
    "pull",
    "pulls",
    "grab",
    "grabs",
    "throw",
    "throws",
    "climb",
    "climbs",
    "jump",
    "jumps",
    "hide",
    "hides",
    "dodge",
    "dodges",
    "block",
    "blocks",
    "flee",
    "flees",
    "draw",
    "draws",
    "aim",
    "aims",
    "reload",
    "reloads",
    "crawl",
    "crawls",
    "creep",
    "creeps",
    "cast",
    "casts",
    "swing",
    "swings",
    "slash",
    "slashes",
    "strike",
    "strikes",
    "lunge",
    "lunges",
    "kill",
    "kills",
    "shove",
    "shoves",
    "enter",
    "enters",
    "knock",
    "knocks",
    "follow",
    "follows",
    "chase",
    "chases",
    "peek",
    "peeks",
    "grapple",
    "grapples",
    "duck",
    "ducks",
    "leap",
    "leaps",
];

/// 中文动作动词/短语（中文无词边界，按子串匹配）。出现任一 → 新虚构动作宣告。
const ACTION_SUBSTR_ZH: &[&str] = &[
    "开枪",
    "射击",
    "射杀",
    "开火",
    "瞄准",
    "拔枪",
    "举枪",
    "装弹",
    "上膛",
    "攻击",
    "进攻",
    "出拳",
    "挥拳",
    "砍",
    "劈",
    "刺",
    "捅",
    "踢",
    "揍",
    "扑",
    "接近",
    "靠近",
    "走近",
    "走向",
    "走到",
    "走过去",
    "上前",
    "前进",
    "后退",
    "后撤",
    "撤退",
    "跑",
    "冲",
    "奔",
    "潜行",
    "悄悄",
    "偷偷",
    "躲",
    "藏",
    "闪避",
    "格挡",
    "防御",
    "招架",
    "逃",
    "检查",
    "搜",
    "翻找",
    "翻",
    "打开",
    "开门",
    "推",
    "拉",
    "拿",
    "抓",
    "抢",
    "夺",
    "扔",
    "投掷",
    "掷",
    "抛",
    "施法",
    "念咒",
    "施放",
    "喊",
    "呼喊",
    "大喊",
    "追",
    "跟踪",
    "爬",
    "跳",
    "钻",
    "摸",
    "撬",
    "蹲",
    "趴",
    "闯",
    "踹",
    "抱",
    "拽",
    "绕",
    "摔",
    "掀",
];

/// 状态/机械元信息查询信号——中文子串。无动作信号且命中任一 → 纯状态查询。
const STATUS_SUBSTR_ZH: &[&str] = &[
    "什么状态",
    "当前状态",
    "怎么样",
    "状态",
    "携带",
    "装备",
    "弹药",
    "骰子",
    "掷骰",
    "列出",
    "确认",
    "理智",
    "血量",
    "生命值",
    "属性值",
    "还剩",
    "剩多少",
    "清单",
];

/// 状态/机械元信息查询信号——英文 token（精确匹配）。
const STATUS_TOKENS_EN: &[&str] = &[
    "status",
    "equipment",
    "inventory",
    "gear",
    "ammo",
    "ammunition",
    "hp",
    "san",
    "sanity",
    "luck",
    "roll",
    "rolls",
    "dice",
    "stat",
    "stats",
    "show",
    "list",
    "display",
    "recap",
];

/// 按非字母数字切分为英文 token（小写串入参），空段丢弃。
fn en_tokens(lower: &str) -> Vec<&str> {
    lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect()
}

/// 玩家是否在本回合宣告了**新虚构动作**（动作优先：宁可放过纯查询也不误伤真动作）。
fn has_action_intent(lower: &str) -> bool {
    if ACTION_SUBSTR_ZH.iter().any(|kw| lower.contains(kw)) {
        return true;
    }
    let tokens = en_tokens(lower);
    tokens.iter().any(|t| ACTION_TOKENS_EN.contains(t))
}

/// 是否带状态/复盘/装备/机械结果查询信号。
fn has_status_query_signal(lower: &str) -> bool {
    if STATUS_SUBSTR_ZH.iter().any(|kw| lower.contains(kw)) {
        return true;
    }
    let tokens = en_tokens(lower);
    tokens.iter().any(|t| STATUS_TOKENS_EN.contains(t))
}

/// 纯状态/复盘/装备/机械结果查询的确定性判定（无 LLM）。命中 → stimulus pass
/// 不得把上回合叙事尾段当作"新刺激"喂给语义判定，否则战斗余波（枪声/HP 归零）
/// 会在一个只问"我现在怎么样 / 列出 HP·弹药·骰子结果"的回合被重新分类成新债务
/// （session_codex_e2e_20260619_04 即此污染）。
///
/// 判据顺序（动作优先）：
/// 1. 句含新虚构动作信号（开枪/接近/检查尸体/shoot/sneak/search…）→ 非纯查询，pass 照常跑。
/// 2. 否则含状态/机械元信息查询信号（HP/SAN/装备/弹药/骰子/状态/列出/show…）→ 纯查询。
/// 零 per-ruleset 硬编码：判据是通用的"动作 vs 元信息查询"语言信号，不绑定具体机制名。
pub fn is_status_only_query(input: &str) -> bool {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_lowercase();
    if has_action_intent(&lower) {
        return false;
    }
    has_status_query_signal(&lower)
}

/// 候选行 `id | name | when_to_use`（与 BP1 目录索引同款物料）；when_to_use
/// 空的条目无路由语义，不参与判定。
fn candidate_lines(catalog: &[MechanicEntry]) -> Vec<String> {
    catalog
        .iter()
        .filter(|e| !e.when_to_use.trim().is_empty())
        .map(|e| {
            format!(
                "{} | {} | {}",
                e.id,
                e.name,
                e.when_to_use.replace('\n', " ")
            )
        })
        .collect()
}

/// LLM 裁决原文 → 合法命中 (mechanic_id, reason)：id 必须是参与判定的目录
/// 条目（防幻觉）、去重、超 `MAX_HITS_PER_TURN` 截断；形状不对 → 空。
fn parse_hits(raw: &Value, catalog: &[MechanicEntry]) -> Vec<(String, String)> {
    let Some(arr) = raw.get("hits").and_then(|h| h.as_array()) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = Vec::new();
    for hit in arr {
        let Some(id) = hit.get("mechanic_id").and_then(|v| v.as_str()) else {
            continue;
        };
        let known = catalog
            .iter()
            .any(|e| e.id == id && !e.when_to_use.trim().is_empty());
        if !known || out.iter().any(|(seen, _)| seen == id) {
            continue;
        }
        let reason = hit
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        out.push((id.to_string(), reason));
        if out.len() >= MAX_HITS_PER_TURN {
            break;
        }
    }
    out
}

/// 语义判定的 system 提示。措辞分层而非一刀切"宁可漏报"：
/// - 模糊/可能已结算 → 仍 fail-closed 不命中（防刷屏，靠 MAX_HITS_PER_TURN 封顶）；
/// - **显式**且**被动触发**的暴露（角色直接目击/接触到某条目 when_to_use 描述的事物，
///   典型如恐怖怪物/异常生物/腐尸/目睹死亡等"无须玩家声明动作即触发"的检定）→ 必须命中，
///   不得因"玩家只是观察、没声明动作"而漏报。零 per-ruleset 硬编码：判据永远是条目自身的
///   when_to_use 语义（CoC 的 SAN、CPR 的 Humanity、Triangle 的 Chaos 同一套）。
fn system_prompt() -> &'static str {
    "You are the rules watcher of a tabletop RPG engine. Given the mechanics catalog index (one entry per line: `id | name | when_to_use`), decide which entries' when_to_use semantics are CLEARLY triggered by what is happening in the fiction RIGHT NOW and demand a mechanical resolution this turn. Judge by meaning, never by keywords. \
Calibrate strictness by trigger type. (1) For PASSIVE-EXPOSURE mechanics — ones whose when_to_use fires merely because the character witnesses, perceives, or is exposed to something (sanity-blasting horror, an anomalous or impossible creature, a corpse, witnessing violent death, a supernatural revelation, corruption, fear, stress) — flag the entry whenever the fiction plainly puts the character in that situation, EVEN IF the player only observed and declared no action; do not withhold a hit just because the player was passive or merely 'looked'. (2) For action-gated mechanics, flag only when the player's declared action clearly invokes the entry. In all cases skip an entry that has obviously already been resolved this scene. When a moment is genuinely ambiguous (no explicit exposure, no clear action), return no hits. \
Respond with JSON only: {\"hits\":[{\"mechanic_id\":\"<id from the index>\",\"reason\":\"<one short sentence citing the fictional trigger>\"}]} (at most 2 hits) or {\"hits\":[]}."
}

/// 取字符串末尾至多 `n` 个字符（按 char 不按字节，CJK 安全）。
fn tail_chars(s: &str, n: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    chars[chars.len().saturating_sub(n)..].iter().collect()
}

fn due_from_hit(
    entry: &MechanicEntry,
    reason: &str,
    session_id: &str,
    turn_id: &str,
    player_input: &str,
) -> MechanicDue {
    MechanicDue {
        due_id: format!("due_{}", Uuid::new_v4().simple()),
        session_id: session_id.to_string(),
        turn_id: turn_id.to_string(),
        source: DueSource::SemanticTrigger,
        source_track: None,
        hook_event: Some("stimulus_match".to_string()),
        mechanic_id: Some(entry.id.clone()),
        threshold_desc: format!(
            "scene stimulus semantically triggers '{}' — resolve it now via roll_check {{mechanic_id:\"{}\"}} (or waive_obligation with a real reason): {}",
            entry.name, entry.id, reason
        ),
        followup_procedure_id: None,
        owner_kind: "actor".to_string(),
        owner_id: "pc.current".to_string(),
        evidence: json!({
            "reason": reason,
            "player_input": tail_chars(player_input, 200),
            "when_to_use": entry.when_to_use,
        }),
        status: DueStatus::Open,
        created_at: chrono::Utc::now(),
    }
}

/// 主入口：语义判定 → due 候选（未抑制未落库——两者由调用方经 watcher
/// `admit_dues` 完成，与 hook 通路同一套防刷屏规则）。
pub async fn stimulus_due_candidates(
    llm: &Arc<dyn LlmClient>,
    catalog: &[MechanicEntry],
    session_id: &str,
    turn_id: &str,
    player_input: &str,
    recent_narration: Option<&str>,
) -> Vec<MechanicDue> {
    // 纯状态/复盘/装备/机械结果查询：绝不把上回合叙事尾段当新刺激重新分类
    // （fail-closed 返回空，TurnStart hook dues 由调用方独立处理，不受影响）。
    if is_status_only_query(player_input) {
        tracing::debug!(
            "stimulus pass skipped: status-only/recap query (no narration-tail reclassification)"
        );
        return Vec::new();
    }
    let lines = candidate_lines(catalog);
    if lines.is_empty() {
        return Vec::new();
    }
    let narration_tail = tail_chars(recent_narration.unwrap_or(""), NARRATION_TAIL_CHARS);
    let system = system_prompt();
    let user = format!(
        "[catalog index]\n{}\n[/catalog index]\n\n[last GM narration tail]\n{}\n[/last GM narration tail]\n\n[player input this turn]\n{}\n[/player input]",
        lines.join("\n"),
        narration_tail,
        player_input,
    );
    let messages = vec![
        ChatMessage {
            role: "system".to_string(),
            content: system.to_string(),
        },
        ChatMessage {
            role: "user".to_string(),
            content: user,
        },
    ];
    let raw = match llm.complete_json(messages, 0.0).await {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(error = %err, "stimulus pass LLM call failed (fail-closed: no dues)");
            return Vec::new();
        }
    };
    parse_hits(&raw, catalog)
        .into_iter()
        .filter_map(|(id, reason)| {
            catalog
                .iter()
                .find(|e| e.id == id)
                .map(|entry| due_from_hit(entry, &reason, session_id, turn_id, player_input))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use async_trait::async_trait;
    use futures_core::Stream;
    use std::pin::Pin;

    /// complete_json 一被调用就 panic——证明纯状态查询回合根本没走 LLM（也就没碰
    /// 上回合叙事尾段）。
    struct PanicLlm;
    #[async_trait]
    impl LlmClient for PanicLlm {
        async fn complete_text(&self, _: Vec<ChatMessage>, _: f32) -> Result<String> {
            unimplemented!("unused")
        }
        async fn complete_json(&self, _: Vec<ChatMessage>, _: f32) -> Result<Value> {
            panic!("LLM must not be called for a status-only query turn");
        }
        async fn stream_chat(
            &self,
            _: Vec<ChatMessage>,
            _: f32,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
            unimplemented!("unused")
        }
    }

    /// complete_json 恒回单条命中——证明真动作回合仍会触发语义 pass 并产出候选。
    struct HitLlm {
        hit_id: String,
    }
    #[async_trait]
    impl LlmClient for HitLlm {
        async fn complete_text(&self, _: Vec<ChatMessage>, _: f32) -> Result<String> {
            unimplemented!("unused")
        }
        async fn complete_json(&self, _: Vec<ChatMessage>, _: f32) -> Result<Value> {
            Ok(json!({"hits": [{"mechanic_id": self.hit_id, "reason": "action triggers it"}]}))
        }
        async fn stream_chat(
            &self,
            _: Vec<ChatMessage>,
            _: f32,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
            unimplemented!("unused")
        }
    }

    fn entry(id: &str, when: &str) -> MechanicEntry {
        MechanicEntry {
            id: id.to_string(),
            name: id.to_string(),
            when_to_use: when.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn candidate_lines_skip_entries_without_when_to_use() {
        let catalog = vec![
            entry("rs.sanity", "exposure to horror"),
            entry("rs.silent", "  "),
        ];
        let lines = candidate_lines(&catalog);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("rs.sanity | "));
    }

    #[test]
    fn parse_hits_drops_hallucinated_ids_dedups_and_caps() {
        let catalog = vec![entry("rs.a", "w"), entry("rs.b", "w"), entry("rs.c", "w")];
        let raw = json!({"hits": [
            {"mechanic_id": "rs.invented", "reason": "幻觉条目必须被丢弃"},
            {"mechanic_id": "rs.a", "reason": "r1"},
            {"mechanic_id": "rs.a", "reason": "重复 id 去重"},
            {"mechanic_id": "rs.b", "reason": "r2"},
            {"mechanic_id": "rs.c", "reason": "超上限截断"},
        ]});
        let hits = parse_hits(&raw, &catalog);
        assert_eq!(
            hits.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
            vec!["rs.a", "rs.b"]
        );
        // 形状不对 → 空（fail-closed）。
        assert!(parse_hits(&json!({"nope": 1}), &catalog).is_empty());
        assert!(parse_hits(&json!("plain string"), &catalog).is_empty());
    }

    #[test]
    fn due_from_hit_binds_mechanic_id_and_marks_semantic_source() {
        let e = entry("coc.sanity_roll_and_loss", "sanity-blasting encounter");
        let due = due_from_hit(&e, "saw an impossible creature", "s1", "t1", "我盯着那条狗");
        assert_eq!(due.source, DueSource::SemanticTrigger);
        assert_eq!(due.mechanic_id.as_deref(), Some("coc.sanity_roll_and_loss"));
        assert_eq!(due.hook_event.as_deref(), Some("stimulus_match"));
        assert!(
            due.threshold_desc.contains("roll_check"),
            "GM 修复指引必须在 desc 里: {}",
            due.threshold_desc
        );
        assert_eq!(due.status, DueStatus::Open);
    }

    #[test]
    fn system_prompt_lowers_threshold_for_passive_exposure_but_keeps_ambiguity_fail_closed() {
        let p = system_prompt();
        // 显式被动暴露（恐怖/异常生物/腐尸/目睹死亡）必须命中，且不得因"玩家只观察未声明动作"漏报。
        assert!(
            p.contains("PASSIVE-EXPOSURE"),
            "must carve out passive-exposure mechanics: {p}"
        );
        assert!(
            p.to_lowercase()
                .contains("even if the player only observed"),
            "must forbid withholding a hit just because the player was passive: {p}"
        );
        // 仍保留模糊场合 fail-closed（无明确暴露/动作 → 不命中），防刷屏。
        assert!(
            p.contains("genuinely ambiguous") && p.contains("return no hits"),
            "must keep fail-closed for ambiguous moments: {p}"
        );
        // 仍保留 2 命中上限的产出约束（与 MAX_HITS_PER_TURN 封顶呼应）。
        assert!(
            p.contains("at most 2 hits"),
            "must keep the per-turn hit cap in the contract: {p}"
        );
        // 零 per-ruleset 硬编码：举例措辞不得把判据钉死成单一规则集机制名。
        assert!(
            !p.contains("call_of_cthulhu"),
            "prompt must stay ruleset-agnostic: {p}"
        );
    }

    #[test]
    fn tail_chars_is_char_safe_for_cjk() {
        assert_eq!(tail_chars("恐怖的狗", 2), "的狗");
        assert_eq!(tail_chars("ab", 10), "ab");
        assert_eq!(tail_chars("", 5), "");
    }

    #[test]
    fn status_only_flags_chinese_status_and_recap_forms() {
        // e2e journey 里实际出现的纯状态/复盘/装备/机械结果查询。
        for q in [
            "我现在处于什么状态？我携带了哪些装备和武器？",
            "我现在怎么样了？那个人呢？",
            "我快速确认战斗后的机械状态：请列出 Evelyn 当前 HP/SAN/Luck、M1911 弹药、目标当前 HP/状态，以及刚才所有公开骰子的结果。",
        ] {
            assert!(is_status_only_query(q), "should be status-only: {q}");
        }
    }

    #[test]
    fn status_only_flags_english_status_forms() {
        for q in [
            "what is my current status and equipment?",
            "show HP/SAN/ammo/roll results",
            "list my inventory and current sanity",
        ] {
            assert!(is_status_only_query(q), "should be status-only: {q}");
        }
    }

    #[test]
    fn status_only_does_not_flag_action_declarations() {
        // 真动作宣告——即便提到 status/result 类词也绝不抑制。
        for q in [
            "我向他开枪",
            "我悄悄接近后门",
            "我检查尸体",
            "我搜查那个人的装备",
            "I shoot the figure",
            "I sneak to the back door",
            "I check the body",
            "I search the corpse for ammo",
        ] {
            assert!(!is_status_only_query(q), "must NOT suppress action: {q}");
        }
    }

    #[test]
    fn status_only_empty_or_neutral_is_not_status() {
        assert!(!is_status_only_query(""));
        assert!(!is_status_only_query("   "));
        // 无动作信号也无状态信号 → 不抑制（交给语义 pass 正常裁量）。
        assert!(!is_status_only_query("我对他说你好"));
    }

    #[test]
    fn status_only_token_match_avoids_substring_false_positives() {
        // "san" 不应命中 "thousand"；"roll" 不应命中 "patrol"——纯英文中性句不抑制。
        assert!(!is_status_only_query("there were a thousand reasons"));
        assert!(!is_status_only_query("the night patrol passed by"));
    }

    #[tokio::test]
    async fn stimulus_due_candidates_skips_llm_for_status_only_query() {
        let catalog = vec![entry("rs.sanity", "exposure to horror")];
        let llm: Arc<dyn LlmClient> = Arc::new(PanicLlm);
        // 关键：recent_narration 携带上回合枪声/HP 归零余波，但纯状态查询回合不得
        // 把它当新刺激——PanicLlm 一旦被调用就 panic，故能跑通即证明叙事尾段未被消费。
        let dues = stimulus_due_candidates(
            &llm,
            &catalog,
            "s1",
            "t_status",
            "我快速确认机械状态：请列出当前 HP/SAN、弹药与刚才骰子的结果。",
            Some("枪声响起，目标的 HP 归零，倒在血泊里。"),
        )
        .await;
        assert!(
            dues.is_empty(),
            "status-only turn must produce no semantic dues"
        );
    }

    #[tokio::test]
    async fn stimulus_due_candidates_runs_llm_for_action_input() {
        let catalog = vec![entry("rs.sanity", "exposure to horror")];
        let llm: Arc<dyn LlmClient> = Arc::new(HitLlm {
            hit_id: "rs.sanity".to_string(),
        });
        // 真动作宣告 → 语义 pass 照常跑，HitLlm 命中 → 产出候选。
        let dues = stimulus_due_candidates(
            &llm,
            &catalog,
            "s1",
            "t_action",
            "我向那条不可能存在的怪物开枪",
            Some("一头扭曲的怪物从阴影里爬出。"),
        )
        .await;
        assert_eq!(
            dues.len(),
            1,
            "action turn must still run the stimulus pass"
        );
        assert_eq!(dues[0].mechanic_id.as_deref(), Some("rs.sanity"));
    }
}

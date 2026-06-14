//! 对抗语义预 pass（Phase 3 §4.1 上游"双保险"上半段）：回合头部一次小 LLM 语义
//! 判断——「玩家这回合是否在攻击一个场景中的对手」。命中 → 解析防御方 NPC 的
//! 防御键（按 kernel compare 经 check_param_need 数据映射：meet_or_beat→defense /
//! roll_under→dodge），现搓落卡（ensure_npc_parameter，Phase 1 通路），并为本回合
//! roll_check 契约预备 `OpposedBinding`——GM 自己填对走 GM 的，漏了由此补。
//!
//! 呼应 `stimulus.rs` 的语义预 pass 模式（zero 关键词路由、fail-closed）：
//! - 内部 AI 语义判断（非关键词扫），通用于任何"攻击对手"场景；
//! - 模糊/无对手/查不到防御值 → 不命中（fail-closed，绝不乱绑平衡值）；
//! - 零 per-ruleset 硬编码：防御键由 kernel.dice_core.compare 数据驱动，
//!   CPR（meet_or_beat）/剑世界 走同一套逻辑，规则集名不出现在判定里。
//!
//! 边界（一期铁律）：AI 只"判意图+下指令"，引擎才"取数据+落卡"。本预 pass 是
//! 引擎侧兜底，产出的是契约预参数，不替 GM 改库——契约最终经 roll_check 工具的
//! stamp_opposed_check 落账（与 GM 显式 opposed 同一通路）。

use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use trpg_llm::LlmClient;
use trpg_model::{ChatMessage, ContextRequest, RuntimeState};
use trpg_need::{EntityNeed, Need, NeedBus, NeedScopes};
use trpg_runtime::npc_synth::NpcPersona;
use trpg_runtime::{encode_entity_hint, EntityNeedResolver, RuntimeEngine};

/// 玩家输入尾段喂给判定的最大字符数（攻击意图通常在玩家声明动作处）。
const PLAYER_INPUT_CHARS: usize = 800;
/// 上回合叙事尾段（对手是否在场的语境）最大字符数。
const NARRATION_TAIL_CHARS: usize = 800;

/// 门 `TRPG_OPPOSED_PREPASS`，默认开（与 stimulus 同款增量成本，仅攻击意图触发现搓）。
pub fn opposed_prepass_enabled() -> bool {
    std::env::var("TRPG_OPPOSED_PREPASS")
        .map(|v| !matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no"))
        .unwrap_or(true)
}

/// 预 pass 为本回合 roll_check 备好的对抗参数。roll_check 工具在 `args.opposed`
/// 缺失时注入它（与 GM 显式 opposed 同走 stamp_opposed_check）。
#[derive(Debug, Clone)]
pub struct OpposedBinding {
    /// 语义解析的真实场景 NPC（actor_id + name/prose 供现搓 persona-judge）。
    pub persona: NpcPersona,
    /// 防御参数所在桶（check_param_need 映射，如 stats/skills）。
    pub bucket: String,
    /// 防御方该测的键（defense/DV/dodge…，由 kernel compare 决定）。
    pub opponent_parameter: String,
}

/// LLM 语义裁决的反序列化形状。`is_attack` 为真且 `target_npc_id` 命中场景 NPC
/// 才构成对抗；其余字段（reason）仅供可观测。
#[derive(Debug, Deserialize)]
struct AttackVerdict {
    #[serde(default)]
    is_attack: bool,
    #[serde(default)]
    target_npc_id: String,
    #[serde(default)]
    #[allow(dead_code)]
    reason: String,
}

/// 语义判定的 system 提示。判据永远是"玩家声明的动作是否在对一个具名场景对手
/// 发起带机械后果的攻击"，按语义不按关键词；fail-closed：模糊/无明确对手 →
/// is_attack=false。零 per-ruleset 硬编码——任何规则集的战斗攻击走同一判据。
fn system_prompt() -> &'static str {
    "You are the combat watcher of a tabletop RPG engine. You are given (a) the list of NPCs present in the current scene (one per line `id | name`), (b) the last GM narration tail, and (c) the player's declared action this turn. \
Decide, by MEANING not keywords, whether the player's declared action this turn is an ATTACK on one specific present NPC opponent — i.e. an action that should be resolved as a contested combat roll against that NPC's defense (striking, shooting, firing at, lunging at, grappling to harm, casting a harmful power at a named foe). \
Be strict and fail-closed: if the action is exploration, movement, talking, looking, a non-combat skill, an attack on an object/environment, or there is no clearly identified present NPC target, return is_attack=false. Only flag when the target is one of the present-scene NPCs in the provided list; never invent an id. \
Respond with JSON only: {\"is_attack\":<bool>,\"target_npc_id\":\"<id from the scene NPC list, or empty>\",\"reason\":\"<one short sentence>\"}."
}

/// 取字符串末尾至多 `n` 个字符（按 char 不按字节，CJK 安全）。
fn tail_chars(s: &str, n: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    chars[chars.len().saturating_sub(n)..].iter().collect()
}

/// 场景 NPC 候选行 `id | name`（喂给判定 + 防幻觉校验的白名单）。
fn npc_lines(scene_npcs: &[(String, String)]) -> Vec<String> {
    scene_npcs.iter().map(|(id, name)| format!("{} | {}", id, name)).collect()
}

/// LLM 裁决原文 → 命中的场景 NPC id（必须在白名单内，防幻觉）；非攻击 / 空 id /
/// id 不在场景列表 → None（fail-closed）。纯函数，易测。
fn parse_target(raw: &Value, scene_npcs: &[(String, String)]) -> Option<String> {
    let verdict: AttackVerdict = serde_json::from_value(raw.clone()).ok()?;
    if !verdict.is_attack {
        return None;
    }
    let target = verdict.target_npc_id.trim();
    if target.is_empty() {
        return None;
    }
    scene_npcs
        .iter()
        .find(|(id, _)| id == target)
        .map(|(id, _)| id.clone())
}

/// 主入口（async 语义判定，不落卡——现搓+stamp 由调用方 turn_loop 完成）：
/// 给定本回合玩家输入、上回合叙事尾段、场景在场 NPC（id,name,prose），语义判定
/// 是否攻击其中某个对手；命中 → 返回该 NPC 的 persona（供现搓 + 盖章）。
/// fail-closed：LLM 失败 / 解析失败 / 无命中 → None。
pub async fn detect_attack_target(
    llm: &Arc<dyn LlmClient>,
    player_input: &str,
    recent_narration: Option<&str>,
    scene_npcs: &[NpcPersona],
) -> Option<NpcPersona> {
    if scene_npcs.is_empty() {
        return None;
    }
    let pairs: Vec<(String, String)> = scene_npcs
        .iter()
        .map(|n| (n.actor_id.clone(), n.name.clone()))
        .collect();
    let lines = npc_lines(&pairs);
    let user = format!(
        "[scene NPCs present]\n{}\n[/scene NPCs present]\n\n[last GM narration tail]\n{}\n[/last GM narration tail]\n\n[player declared action this turn]\n{}\n[/player declared action]",
        lines.join("\n"),
        tail_chars(recent_narration.unwrap_or(""), NARRATION_TAIL_CHARS),
        tail_chars(player_input, PLAYER_INPUT_CHARS),
    );
    let messages = vec![
        ChatMessage { role: "system".to_string(), content: system_prompt().to_string() },
        ChatMessage { role: "user".to_string(), content: user },
    ];
    let raw = match llm.complete_json(messages, 0.0).await {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(error = %err, "opposed prepass LLM call failed (fail-closed: no opposed binding)");
            return None;
        }
    };
    let target_id = parse_target(&raw, &pairs)?;
    scene_npcs.iter().find(|n| n.actor_id == target_id).cloned()
}

/// 回合头部对抗预 pass 主流程（turn_loop 调用）：判定本回合是否攻击场景对手并备好
/// OpposedBinding。① 门控/有模组在场 NPC 才跑；② 语义判定攻击目标；③ 按 kernel.compare
/// 取防御键（attack_defense_param 数据映射）；④ 现搓防御方 NPC 防御值落卡
/// （ensure_npc_parameter，Phase1 通路）；⑤ 现搓成功才返回 binding。
/// fail-closed：任一步缺失 / 现搓不成（Ok(None)/Err）→ None，绝不乱绑、绝不阻断回合。
pub async fn prepare_binding(
    engine: &RuntimeEngine,
    llm: &Arc<dyn LlmClient>,
    request: &ContextRequest,
    state: &RuntimeState,
    user_input: &str,
    recent_transcript: Option<&str>,
    history: &[ChatMessage],
) -> Option<OpposedBinding> {
    if !opposed_prepass_enabled() { return None; }
    // ① 场景在场 NPC（真实 graph id，治 npc.opposition 占位符串台坑 §6③）。
    let scene_npcs = engine.scene_npc_personas(request, state).await;
    if scene_npcs.is_empty() { return None; }
    // ② 语义判定攻击目标（recent_transcript 缺则回退上回合 assistant 叙事尾段）。
    let recent = recent_transcript
        .or_else(|| history.iter().rev().find(|m| m.role == "assistant").map(|m| m.content.as_str()));
    let target = detect_attack_target(llm, user_input, recent, &scene_npcs).await?;
    // ③ 防御键（meet_or_beat→stats.defense / roll_under→skills.dodge，数据驱动）。
    let (bucket, param) = engine.attack_defense_param(&request.ruleset_id).await?;
    // ④+⑤ 现搓防御值 + HP 预热（写 NPC 卡，供本回合对抗结算）：
    // R2 收口 — NPC 现搓只经 NeedBus 的 EntityNeedResolver 触发副作用（无 env fallback）。
    // ④ 是绑定 gate（防御值现搓不成 → 不绑），⑤ HP 是 fire-and-forget（失败不影响绑定）。
    // 构造 scopes + 两个 EntityNeed（防御值 + HP），emit → resolve_all 触发现搓。
    // resolver 内部 swallow Ok(None)/Err 并恒返回空 outcome（副作用为主），故 bus 本身
    // 判不出防御值是否真落卡 → 用幂等的存在性 re-check 复刻 gate 语义。
    let hp_ctx = format!("opposed combat prepass: NPC HP seed for damage resolution ({})", target.actor_id);
    let scopes = NeedScopes {
        ruleset_id: request.ruleset_id.clone(),
        module_id: request.module_id.clone().or_else(|| state.module_id.clone()),
        session_id: request.session_id.clone(),
        turn_id: request.turn_id.clone(),
        scene_id: state.scene_id.clone(),
    };
    let defense_hint = encode_entity_hint(&target.actor_id, &bucket, &param,
        "opposed combat prepass: defending against player attack");
    let hp_hint = encode_entity_hint(&target.actor_id, "resources", "hp", &hp_ctx);

    let resolver = EntityNeedResolver::new(Arc::new(engine.clone()));
    let mut bus = NeedBus::new();
    bus.register(Box::new(resolver));
    bus.emit(Need::Entity(EntityNeed { scopes: scopes.clone(), entity_hint: Some(defense_hint) }));
    bus.emit(Need::Entity(EntityNeed { scopes, entity_hint: Some(hp_hint) }));
    let _ = bus.resolve_all().await; // 副作用写卡；outcomes 恒空 blocks 不用

    // ④ gate：现搓通路完成后，幂等 re-check 防御值是否已在卡上（缓存命中即 Ok(Some)）。
    // 这是廉价的缓存读回，保留 defense-gate 语义（防御值现搓不成则不绑）。
    match engine.ensure_npc_parameter(&request.session_id, &request.ruleset_id, &target, &bucket, &param,
        "opposed prepass: post-bus param existence check (idempotent read-back)").await {
        Ok(Some(_)) => {}
        Ok(None) => {
            tracing::info!(target: "opposed_prepass", npc = %target.actor_id, "defense synth unavailable after bus (gate off / no LLM / no card) — no opposed binding");
            return None;
        }
        Err(err) => {
            tracing::warn!(target: "opposed_prepass", error = %err, npc = %target.actor_id, "defense post-check failed — no opposed binding (turn continues)");
            return None;
        }
    }
    tracing::info!(target: "opposed_prepass", npc = %target.actor_id, bucket = %bucket, param = %param, "opposed binding prepared (GM-omitted opposed will be injected)");
    Some(OpposedBinding { persona: target, bucket, opponent_parameter: param })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn npcs() -> Vec<(String, String)> {
        vec![
            ("npc.scav_boss".to_string(), "Scav Boss".to_string()),
            ("npc.thug".to_string(), "Thug".to_string()),
        ]
    }

    #[test]
    fn parse_target_returns_present_npc_on_attack() {
        let raw = json!({"is_attack": true, "target_npc_id": "npc.scav_boss", "reason": "fires at the boss"});
        assert_eq!(parse_target(&raw, &npcs()).as_deref(), Some("npc.scav_boss"));
    }

    #[test]
    fn parse_target_fail_closed_when_not_attack() {
        let raw = json!({"is_attack": false, "target_npc_id": "npc.scav_boss", "reason": "just looking"});
        assert!(parse_target(&raw, &npcs()).is_none());
    }

    #[test]
    fn parse_target_drops_hallucinated_id() {
        // 攻击为真但 target 不在场景白名单 → fail-closed（防幻觉 id）。
        let raw = json!({"is_attack": true, "target_npc_id": "npc.invented", "reason": "x"});
        assert!(parse_target(&raw, &npcs()).is_none());
        // 空 id 同样不命中。
        let raw_empty = json!({"is_attack": true, "target_npc_id": "", "reason": "x"});
        assert!(parse_target(&raw_empty, &npcs()).is_none());
    }

    #[test]
    fn parse_target_fail_closed_on_bad_shape() {
        // 形状不对（缺字段/非对象）→ serde default + 不命中（绝不 panic）。
        assert!(parse_target(&json!({"nope": 1}), &npcs()).is_none());
        assert!(parse_target(&json!("plain string"), &npcs()).is_none());
    }

    #[test]
    fn system_prompt_is_semantic_and_ruleset_agnostic() {
        let p = system_prompt();
        // 语义判据（按 meaning 非 keyword），fail-closed。
        assert!(p.contains("by MEANING not keywords"), "must judge by meaning: {p}");
        assert!(p.to_lowercase().contains("fail-closed"), "must be fail-closed: {p}");
        // 防幻觉：只许命中提供的场景 NPC，绝不造 id。
        assert!(p.contains("never invent an id"), "must forbid inventing ids: {p}");
        // 零 per-ruleset 硬编码：提示不得把判据钉死成单一规则集名。
        for rs in ["cyberpunk_red", "call_of_cthulhu", "sword_world", "剑世界"] {
            assert!(!p.contains(rs), "prompt must stay ruleset-agnostic, found `{rs}`: {p}");
        }
    }

    #[test]
    fn npc_lines_renders_id_and_name() {
        let lines = npc_lines(&npcs());
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "npc.scav_boss | Scav Boss");
    }

    #[test]
    fn tail_chars_is_char_safe_for_cjk() {
        assert_eq!(tail_chars("我朝清道夫开枪", 3), "夫开枪");
        assert_eq!(tail_chars("ab", 10), "ab");
        assert_eq!(tail_chars("", 5), "");
    }

    // R2 T6: the bus path encodes the four synthesis fields into the EntityNeed
    // `entity_hint` via `encode_entity_hint`; the resolver decodes with splitn(4,'|').
    // These guard that the emit-site encoding the resolver consumes stays in lock-step.
    #[test]
    fn entity_need_hint_encodes_all_fields() {
        let hint = encode_entity_hint("npc.ras", "skills", "perception",
            "opposed combat prepass: defending against player attack");
        let parts: Vec<&str> = hint.splitn(4, '|').collect();
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0], "npc.ras");
        assert_eq!(parts[1], "skills");
        assert_eq!(parts[2], "perception");
        assert_eq!(parts[3], "opposed combat prepass: defending against player attack");
    }

    #[test]
    fn entity_need_hint_with_pipe_in_ctx_survives_splitn4() {
        // check_context 本身含 '|' 时，splitn(4) 第 4 段保留全文（resolver 解码不丢字段）。
        let hint = encode_entity_hint("npc.x", "stats", "defense", "context with | pipe inside");
        let parts: Vec<&str> = hint.splitn(4, '|').collect();
        assert_eq!(parts[3], "context with | pipe inside");
    }
}

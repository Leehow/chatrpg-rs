//! KnowledgeEdge 账本（v1）：把「事实身份/真相」与「谁知道/怀疑/相信/误信它」分离。
//! durable 持久化 holder 当前为 gm / player_party / system / npc（见 [`KnowledgeHolderKind::is_durable_persistable`]）；
//! TC-KNOW-04 已打开具体 NPC 的 durable 边（actor-id 契约见 TC-KNOW-00）；pc / faction holder 的
//! durable 边仍 gated（留待后续任务），写入对其 fail-closed。
//! revealed-facts 兼容投影 = 本表 (player_party, knows_true) 子集；
//! 与 domain_events.PlayerLearnedFact 写穿对齐（见 trpg-db record_revealed_fact）。
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 知识 holder 种类（v1）。durable 持久化为 gm / player_party / system / npc；
/// pc / faction 为模型层可表达态，但 durable knowledge_edges 写入对其 fail-closed
/// （schema CHECK 为 gm/player_party/system/npc；NPC durable 边见 TC-KNOW-04，pc/faction 仍 gated）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeHolderKind {
    /// GM / 叙事主权方：其 knows_true 边即世界真相（durable）。
    Gm,
    /// 玩家方集合 holder（durable）；revealed-facts 兼容投影只取此 holder 的 knows_true。
    PlayerParty,
    /// 运行时/系统 holder（durable）：runtime 拥有的非角色知识（如系统裁决记录）。
    System,
    /// 具体玩家角色（模型层；durable 写入 gated 到后续 actor-id 任务）。
    Pc,
    /// 具体 NPC（durable）：TC-KNOW-04 已打开持久化，须挂稳定 actor id（见 [`KnowledgeHolder::npc_from_actor_id`]）。
    Npc,
    /// 阵营 / 派系（模型层；durable 写入 gated 到后续任务）。
    Faction,
}

impl KnowledgeHolderKind {
    /// 与 knowledge_edges.holder_kind 列对齐的稳定 token（snake_case）。
    pub fn as_token(&self) -> &'static str {
        match self {
            KnowledgeHolderKind::Gm => "gm",
            KnowledgeHolderKind::PlayerParty => "player_party",
            KnowledgeHolderKind::System => "system",
            KnowledgeHolderKind::Pc => "pc",
            KnowledgeHolderKind::Npc => "npc",
            KnowledgeHolderKind::Faction => "faction",
        }
    }

    /// 从列 token 解析 holder kind；未知 token 返回 None（调用方据此 fail-closed）。
    pub fn from_token(token: &str) -> Option<Self> {
        match token {
            "gm" => Some(KnowledgeHolderKind::Gm),
            "player_party" => Some(KnowledgeHolderKind::PlayerParty),
            "system" => Some(KnowledgeHolderKind::System),
            "pc" => Some(KnowledgeHolderKind::Pc),
            "npc" => Some(KnowledgeHolderKind::Npc),
            "faction" => Some(KnowledgeHolderKind::Faction),
            _ => None,
        }
    }

    /// 当前是否允许把该 holder kind 的边写入 durable knowledge_edges。
    /// gm / player_party / system / npc 为真——pc / faction 的 durable 持久化仍 gated。
    /// system 可 durable 的理由：它是 runtime 拥有的稳定身份（非 LLM 即兴文本）。
    /// npc（TC-KNOW-04）可 durable 的前提：写库前必经 actor-identity 契约
    /// （[`KnowledgeHolder::npc_from_actor_id`]）解析出稳定 actor id —— 仅 kind 通过门
    /// 不代表 holder_id 合法，未解析/即兴 NPC 串仍 fail-closed（见 db upsert_knowledge_edge）。
    pub fn is_durable_persistable(&self) -> bool {
        matches!(
            self,
            KnowledgeHolderKind::Gm
                | KnowledgeHolderKind::PlayerParty
                | KnowledgeHolderKind::System
                | KnowledgeHolderKind::Npc
        )
    }
}

/// holder 对某 fact 的知识态（v1 超集）。把「相信/怀疑/误信」与「确知为真」严格区分：
/// 只有 `knows_true` 进真知识投影（[`KnowledgeState::is_true_knowledge`]）；
/// 各 belief 态（believes_true / believes_false / misinformed 等）是信念而非世界真相。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeState {
    /// 不知道。
    Unknown,
    /// 见过/接触过实体但未揭示秘密（旧 P0b 态，保留向后兼容）。
    Exposed,
    /// 感知到（看到/听到现场迹象）但未确证。
    Perceived,
    /// 听说过（二手传闻）。
    HeardAbout,
    /// 怀疑（有指向但未确证）。
    Suspects,
    /// 相信为真（信念，非世界真相确证）。
    BelievesTrue,
    /// 相信为假 / 错误否定（信念，非世界真相）。
    BelievesFalse,
    /// 确知为真：唯一进真知识投影的态。
    KnowsTrue,
    /// 被误导（持有与真相相反的错误信念）。
    Misinformed,
    /// 知道但被要求保留/未披露。
    Withheld,
    /// 曾知道但已遗忘。
    Forgotten,
}

impl KnowledgeState {
    /// 与 knowledge_edges.knowledge_state 列对齐的稳定 token（snake_case）。
    pub fn as_token(&self) -> &'static str {
        match self {
            KnowledgeState::Unknown => "unknown",
            KnowledgeState::Exposed => "exposed",
            KnowledgeState::Perceived => "perceived",
            KnowledgeState::HeardAbout => "heard_about",
            KnowledgeState::Suspects => "suspects",
            KnowledgeState::BelievesTrue => "believes_true",
            KnowledgeState::BelievesFalse => "believes_false",
            KnowledgeState::KnowsTrue => "knows_true",
            KnowledgeState::Misinformed => "misinformed",
            KnowledgeState::Withheld => "withheld",
            KnowledgeState::Forgotten => "forgotten",
        }
    }

    /// 从列 token 解析知识态（与 [`Self::as_token`] 互逆）；未知 token 返回 None
    /// （调用方据此 fail-closed，绝不把未知态误当某已知态）。
    pub fn from_token(token: &str) -> Option<Self> {
        match token {
            "unknown" => Some(KnowledgeState::Unknown),
            "exposed" => Some(KnowledgeState::Exposed),
            "perceived" => Some(KnowledgeState::Perceived),
            "heard_about" => Some(KnowledgeState::HeardAbout),
            "suspects" => Some(KnowledgeState::Suspects),
            "believes_true" => Some(KnowledgeState::BelievesTrue),
            "believes_false" => Some(KnowledgeState::BelievesFalse),
            "knows_true" => Some(KnowledgeState::KnowsTrue),
            "misinformed" => Some(KnowledgeState::Misinformed),
            "withheld" => Some(KnowledgeState::Withheld),
            "forgotten" => Some(KnowledgeState::Forgotten),
            _ => None,
        }
    }

    /// 是否为「确知为真」——唯一进真知识/世界真相投影的态。
    /// belief 态（哪怕 believes_true）一律不算确知，避免把信念当真相泄露给玩家。
    pub fn is_true_knowledge(&self) -> bool {
        matches!(self, KnowledgeState::KnowsTrue)
    }

    /// 是否为信念态（相信/误信，非确证）。false belief 据此与世界真相区分。
    pub fn is_belief(&self) -> bool {
        matches!(
            self,
            KnowledgeState::BelievesTrue
                | KnowledgeState::BelievesFalse
                | KnowledgeState::Misinformed
        )
    }
}

/// 一条知识边（v1）：某 session 内 holder 对某 fact 的知识态、置信度、来源与披露策略。
/// 事实身份（fact_id）与真相同 holder 知识解耦：同一 fact 可被 GM 确知、被玩家未知、
/// 被某 holder 误信，互不影响。v1 新增字段 serde-default，向后兼容旧 P0b 序列化。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeEdge {
    pub edge_id: String,
    pub session_id: String,
    pub holder_kind: KnowledgeHolderKind,
    pub holder_id: Option<String>,
    pub fact_id: String,
    pub knowledge_state: KnowledgeState,
    /// 置信度 [0,1]（可空，旧边缺省 None）。
    #[serde(default)]
    pub confidence: Option<f64>,
    /// 习得该知识的回合 id（可空）。
    #[serde(default)]
    pub learned_at_turn_id: Option<String>,
    /// 披露策略（如 open / secret / gm_only；可空，语义留待 no-spoiler 任务细化）。
    #[serde(default)]
    pub disclosure_policy: Option<String>,
    pub source_event_id: Option<String>,
    pub reason: Option<String>,
    pub created_at: DateTime<Utc>,
    /// 末次更新时间（可空，旧边缺省 None）。
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
}

// -----------------------------------------------------------------------------
// TC-KNOW-00 Actor Identity Contract（稳定知识 holder 身份门）
//
// durable NPC（及未来 pc/faction）knowledge holder 必须挂在 source-backed / runtime-owned
// 的稳定 actor id 上，绝不能用展示名、念白文本、对抗方标签或 LLM 即兴串冒充。本契约提供：
//   1) 稳定 holder 身份类型 `KnowledgeHolder`（gm / player_party / pc:<id> / npc:<id> /
//      faction:<id>），扩展走「新增 variant」而非让文本凭空造 holder —— fail-closed 扩展。
//   2) NPC（及 pc/faction）actor id 的保守校验，不安全输入返回 typed `UnresolvedHolder`。
//   3) 从 source/runtime 的 `ActorRef` 解析到稳定 holder 的 resolver；解析不出即 Unresolved。
// gm / player_party 行为保持兼容（见 `KnowledgeHolderKind` 与 db 写穿路径不变）。
// durable knowledge_edges(holder_kind='npc') 持久化仍 gated 到 TC-KNOW-04；本契约只定义身份。
// -----------------------------------------------------------------------------

/// holder 身份解析失败的归因（typed，绝不退化成「拿原串当 holder」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnresolvedReason {
    /// 空串 / 仅空白。
    Empty,
    /// 命中占位/即兴串黑名单（unknown、enemy、npc、??? 之类）。
    Placeholder,
    /// 含空白或非 id 字符——典型为展示名 / 念白 / 对抗方标签（"The Butler"、"Goblin #2"）。
    NotIdShaped,
    /// 源 actor kind 不是可作知识 holder 的角色（environment / hazard / system 等）。
    UnsupportedActorKind,
}

/// 身份解析失败结果：带原始证据（evidence）便于上层 fail-closed 上报，不发明 holder。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedHolder {
    pub reason: UnresolvedReason,
    /// 触发拒绝的原始输入（截断保护见 `new`），仅作证据/日志，绝不被当作 holder id 使用。
    pub evidence: String,
}

impl UnresolvedHolder {
    fn new(reason: UnresolvedReason, evidence: &str) -> Self {
        // 证据截断，避免把整段念白塞进错误链。
        let evidence: String = evidence.chars().take(80).collect();
        Self { reason, evidence }
    }
}

impl std::fmt::Display for UnresolvedHolder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let why = match self.reason {
            UnresolvedReason::Empty => "empty actor id",
            UnresolvedReason::Placeholder => "placeholder/ad-hoc actor id",
            UnresolvedReason::NotIdShaped => "not a stable id (display name / narration / label)",
            UnresolvedReason::UnsupportedActorKind => "actor kind cannot hold knowledge",
        };
        write!(
            f,
            "unresolved knowledge holder: {why} (evidence: {:?})",
            self.evidence
        )
    }
}

/// 占位/即兴串黑名单（小写精确匹配）。这些绝不能成为稳定 holder id。
const PLACEHOLDER_IDS: &[&str] = &[
    "unknown",
    "none",
    "null",
    "nil",
    "na",
    "tbd",
    "todo",
    "n/a",
    "?",
    "??",
    "???",
    "-",
    "--",
    "enemy",
    "enemies",
    "npc",
    "npcs",
    "actor",
    "target",
    "foe",
    "monster",
    "mob",
    "placeholder",
    "anonymous",
    "someone",
    "somebody",
    "stranger",
    "person",
    "guy",
    "thing",
    "it",
    "them",
];

/// 校验一个 actor id 是否「稳定」：source-backed / runtime-owned 的 id 形态，
/// 而非展示名 / 念白 / 对抗方标签 / 占位串。保守 fail-closed：
///   - trim 后非空；
///   - 首字符必须是字母或数字；
///   - 仅允许 `[A-Za-z0-9_.-]`（无空白、无冒号、无标点段落）——冒号是 holder token 分隔符，
///     故 actor id 自身不得含 `:`；
///   - 不在占位黑名单内（大小写不敏感）。
/// 通过则返回 trim 规范化后的 id。
fn validate_actor_id(raw: &str) -> Result<String, UnresolvedHolder> {
    let id = raw.trim();
    if id.is_empty() {
        return Err(UnresolvedHolder::new(UnresolvedReason::Empty, raw));
    }
    if PLACEHOLDER_IDS.contains(&id.to_ascii_lowercase().as_str()) {
        return Err(UnresolvedHolder::new(UnresolvedReason::Placeholder, raw));
    }
    let mut chars = id.chars();
    let first = chars.next().unwrap();
    let id_shaped = first.is_ascii_alphanumeric()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if !id_shaped {
        return Err(UnresolvedHolder::new(UnresolvedReason::NotIdShaped, raw));
    }
    Ok(id.to_string())
}

/// 稳定知识 holder 身份（actor identity contract）。
///
/// 派生 token：`gm` / `player_party` / `pc:<id>` / `npc:<id>` / `faction:<id>`。
/// 扩展策略 = 新增 variant（受 source/runtime 控制），绝不让 LLM 文本凭空构造 holder。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KnowledgeHolder {
    /// GM / 叙事主权方（全知，durable 已支持）。
    Gm,
    /// 玩家方集合 holder（durable 已支持；与具体 PC 区分）。
    PlayerParty,
    /// 具体玩家角色（稳定 actor id）。
    PlayerCharacter(String),
    /// 具体 NPC（稳定 actor id）。durable 持久化 TC-KNOW-04 已打开。
    Npc(String),
    /// 阵营 / 派系（稳定 id）。durable 持久化仍 gated（留待后续任务）。
    Faction(String),
}

impl KnowledgeHolder {
    /// holder 类 token（与 knowledge_edges.holder_kind 列对齐的扩展集）。
    pub fn kind_token(&self) -> &'static str {
        match self {
            KnowledgeHolder::Gm => "gm",
            KnowledgeHolder::PlayerParty => "player_party",
            KnowledgeHolder::PlayerCharacter(_) => "pc",
            KnowledgeHolder::Npc(_) => "npc",
            KnowledgeHolder::Faction(_) => "faction",
        }
    }

    /// 稳定 holder id。gm / player_party 是集合 holder（无 id）；其余返回稳定 actor id。
    pub fn holder_id(&self) -> Option<&str> {
        match self {
            KnowledgeHolder::Gm | KnowledgeHolder::PlayerParty => None,
            KnowledgeHolder::PlayerCharacter(id)
            | KnowledgeHolder::Npc(id)
            | KnowledgeHolder::Faction(id) => Some(id.as_str()),
        }
    }

    /// 全局稳定 token：集合 holder 即 kind 本身；带 id 的 holder 为 `kind:id`。
    /// 两个不同 actor id → 不同 token，且带前缀故绝不与 `gm` / `player_party` 相撞。
    pub fn token(&self) -> String {
        match self.holder_id() {
            None => self.kind_token().to_string(),
            Some(id) => format!("{}:{}", self.kind_token(), id),
        }
    }

    /// durable knowledge_edges 是否已允许该 holder 持久化。
    /// gm / player_party / npc 为真——TC-KNOW-04 已打开 NPC 持久化；faction / pc 仍 gated。
    /// 持久化逻辑据此判断是否写 knowledge_edges；本契约自身不写任何边。
    /// 注意：NPC 的 durable 写入还要求 holder_id 经 actor-identity 校验通过（kind 通过 ≠ id 合法）。
    pub fn is_durable_today(&self) -> bool {
        matches!(
            self,
            KnowledgeHolder::Gm | KnowledgeHolder::PlayerParty | KnowledgeHolder::Npc(_)
        )
    }

    /// 从一个 NPC 的原始 actor id 串解析为稳定 NPC holder。
    /// 校验失败返回 typed `UnresolvedHolder`（绝不发明 holder）。
    pub fn npc_from_actor_id(raw: &str) -> Result<KnowledgeHolder, UnresolvedHolder> {
        Ok(KnowledgeHolder::Npc(validate_actor_id(raw)?))
    }

    /// 从一个玩家角色的原始 actor id 串解析为稳定 PC holder。
    pub fn player_character_from_actor_id(raw: &str) -> Result<KnowledgeHolder, UnresolvedHolder> {
        Ok(KnowledgeHolder::PlayerCharacter(validate_actor_id(raw)?))
    }

    /// 从一个阵营的原始 id 串解析为稳定 faction holder。
    pub fn faction_from_id(raw: &str) -> Result<KnowledgeHolder, UnresolvedHolder> {
        Ok(KnowledgeHolder::Faction(validate_actor_id(raw)?))
    }

    /// 从 source/runtime 的 [`ActorRef`](crate::ActorRef) 解析为稳定知识 holder。
    /// 用 **actor_id**（source/runtime 拥有的稳定 id）而非 display_name；display_name 仅作证据。
    /// 不支持作知识 holder 的 actor kind（environment / hazard / system）→ Unresolved，
    /// 不可解析的 id（空 / 占位 / 展示名形态）→ Unresolved（fail-closed，不退化用展示名）。
    pub fn from_actor_ref(actor: &crate::ActorRef) -> Result<KnowledgeHolder, UnresolvedHolder> {
        use crate::ActorKind;
        match actor.actor_kind {
            ActorKind::Npc => KnowledgeHolder::npc_from_actor_id(&actor.actor_id),
            ActorKind::PlayerCharacter => {
                KnowledgeHolder::player_character_from_actor_id(&actor.actor_id)
            }
            ActorKind::Environment | ActorKind::Hazard | ActorKind::System => {
                // 这些 kind 不在 actor-identity 知识 holder 契约内；带证据 fail-closed。
                let evidence = if !actor.actor_id.trim().is_empty() {
                    actor.actor_id.as_str()
                } else {
                    actor.display_name.as_deref().unwrap_or("")
                };
                Err(UnresolvedHolder::new(
                    UnresolvedReason::UnsupportedActorKind,
                    evidence,
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ActorKind, ActorRef};
    use serde_json::json;

    #[test]
    fn knowledge_enums_are_snake_case() {
        assert_eq!(
            serde_json::to_value(KnowledgeHolderKind::PlayerParty).unwrap(),
            json!("player_party")
        );
        assert_eq!(
            serde_json::to_value(KnowledgeState::KnowsTrue).unwrap(),
            json!("knows_true")
        );
    }

    /// v1 holder kind：gm/player_party/system/npc durable（TC-KNOW-04 打开 npc），
    /// pc/faction 仍 fail-closed；token round-trip 稳定，序列化与 token 对齐。
    #[test]
    fn holder_kind_durability_and_tokens() {
        for (k, tok, durable) in [
            (KnowledgeHolderKind::Gm, "gm", true),
            (KnowledgeHolderKind::PlayerParty, "player_party", true),
            (KnowledgeHolderKind::System, "system", true),
            (KnowledgeHolderKind::Npc, "npc", true),
            (KnowledgeHolderKind::Pc, "pc", false),
            (KnowledgeHolderKind::Faction, "faction", false),
        ] {
            assert_eq!(k.as_token(), tok);
            assert_eq!(KnowledgeHolderKind::from_token(tok), Some(k));
            assert_eq!(k.is_durable_persistable(), durable, "durable 判定: {tok}");
            assert_eq!(serde_json::to_value(k).unwrap(), json!(tok));
        }
        assert_eq!(KnowledgeHolderKind::from_token("dragon"), None);
    }

    /// v1 knowledge state：只有 knows_true 是真知识；believes_* / misinformed 是信念非真相。
    #[test]
    fn knowledge_state_true_vs_belief() {
        assert!(KnowledgeState::KnowsTrue.is_true_knowledge());
        for s in [
            KnowledgeState::Unknown,
            KnowledgeState::Exposed,
            KnowledgeState::Perceived,
            KnowledgeState::HeardAbout,
            KnowledgeState::Suspects,
            KnowledgeState::BelievesTrue,
            KnowledgeState::BelievesFalse,
            KnowledgeState::Misinformed,
            KnowledgeState::Withheld,
            KnowledgeState::Forgotten,
        ] {
            assert!(!s.is_true_knowledge(), "{} 不应算确知为真", s.as_token());
        }
        // belief 态：相信/误信，永远不等于世界真相确证。
        for s in [
            KnowledgeState::BelievesTrue,
            KnowledgeState::BelievesFalse,
            KnowledgeState::Misinformed,
        ] {
            assert!(s.is_belief());
            assert!(
                !s.is_true_knowledge(),
                "信念态 {} 不得进真知识投影",
                s.as_token()
            );
        }
        // token round-trip 与序列化对齐。
        assert_eq!(
            serde_json::to_value(KnowledgeState::HeardAbout).unwrap(),
            json!("heard_about")
        );
        assert_eq!(KnowledgeState::Misinformed.as_token(), "misinformed");
    }

    /// from_token 与 as_token 互逆，未知 token fail-closed 返回 None。
    #[test]
    fn knowledge_state_token_round_trip() {
        for s in [
            KnowledgeState::Unknown,
            KnowledgeState::Exposed,
            KnowledgeState::Perceived,
            KnowledgeState::HeardAbout,
            KnowledgeState::Suspects,
            KnowledgeState::BelievesTrue,
            KnowledgeState::BelievesFalse,
            KnowledgeState::KnowsTrue,
            KnowledgeState::Misinformed,
            KnowledgeState::Withheld,
            KnowledgeState::Forgotten,
        ] {
            assert_eq!(KnowledgeState::from_token(s.as_token()), Some(s));
        }
        assert_eq!(KnowledgeState::from_token("teleported"), None);
    }

    fn npc_ref(actor_id: &str, display: Option<&str>) -> ActorRef {
        ActorRef {
            actor_id: actor_id.to_string(),
            actor_kind: ActorKind::Npc,
            display_name: display.map(str::to_string),
        }
    }

    /// source/runtime 的 NPC ActorRef 映射到稳定 NPC holder，token 带 `npc:` 前缀。
    #[test]
    fn actor_identity_maps_source_npc_to_runtime_actor() {
        let holder = KnowledgeHolder::from_actor_ref(&npc_ref("npc_alice", Some("Alice the Maid")))
            .expect("source-backed NPC actor id 必须可解析");
        assert_eq!(holder, KnowledgeHolder::Npc("npc_alice".to_string()));
        assert_eq!(holder.kind_token(), "npc");
        assert_eq!(holder.holder_id(), Some("npc_alice"));
        assert_eq!(holder.token(), "npc:npc_alice");
        // TC-KNOW-04：稳定 NPC 身份现已可 durable 持久化（pc/faction 仍 gated）。
        assert!(holder.is_durable_today());
        assert!(!KnowledgeHolder::Faction("f_guild".into()).is_durable_today());
        assert!(!KnowledgeHolder::PlayerCharacter("pc_hero".into()).is_durable_today());
    }

    /// 仅有展示名、无稳定 actor id 的 NPC 输入必须 fail-closed，绝不拿展示名当 holder。
    #[test]
    fn actor_identity_rejects_display_name_only_holder() {
        // actor_id 为空、只有 display_name "The Butler" → Unresolved，证据不是被当成 holder。
        let err = KnowledgeHolder::from_actor_ref(&npc_ref("", Some("The Butler")))
            .expect_err("display-name-only 必须被拒");
        assert_eq!(err.reason, UnresolvedReason::Empty);
        // 直接把展示名喂给 npc 解析（含空格的标签）也必须拒，归因为 NotIdShaped。
        let err2 =
            KnowledgeHolder::npc_from_actor_id("The Butler").expect_err("展示名形态的串必须被拒");
        assert_eq!(err2.reason, UnresolvedReason::NotIdShaped);
    }

    /// NpcLearnedFact 写入要求稳定 actor id：空 / 占位 / 战斗对抗标签全部 fail-closed，
    /// 合法 source id 通过。（DB record_npc_learned_fact 路由此校验。）
    #[test]
    fn npc_learned_fact_requires_stable_actor_id() {
        for bad in [
            "",
            "   ",
            "unknown",
            "NPC",
            "enemy",
            "Goblin #2",
            "the masked figure",
            "n/a",
        ] {
            assert!(
                KnowledgeHolder::npc_from_actor_id(bad).is_err(),
                "ad-hoc/占位/标签串必须被拒: {bad:?}"
            );
        }
        let ok = KnowledgeHolder::npc_from_actor_id("  npc_butler  ")
            .expect("trim 后的稳定 id 必须通过");
        assert_eq!(ok.holder_id(), Some("npc_butler"));
    }

    /// 不可解析 / 即兴 NPC 文本 fail-closed，返回 typed Unresolved 而非 best-effort 串。
    #[test]
    fn ad_hoc_npc_text_fails_closed() {
        let cases = [
            ("", UnresolvedReason::Empty),
            ("   ", UnresolvedReason::Empty),
            ("enemy", UnresolvedReason::Placeholder),
            ("a masked stranger lunges", UnresolvedReason::NotIdShaped),
        ];
        for (text, want) in cases {
            let err =
                KnowledgeHolder::npc_from_actor_id(text).expect_err("即兴文本必须 fail-closed");
            assert_eq!(err.reason, want, "归因不符: {text:?}");
            // evidence 仅作日志/上报，绝不等于一个 holder token。
            assert!(!err.evidence.starts_with("npc:"));
        }
    }

    /// 两个不同 NPC id 保持互异，且任一都不会与 gm / player_party 撞 token。
    #[test]
    fn npc_holders_stay_distinct_and_never_collide_with_gm_or_party() {
        let alice = KnowledgeHolder::npc_from_actor_id("npc_alice").unwrap();
        let bob = KnowledgeHolder::npc_from_actor_id("npc_bob").unwrap();
        assert_ne!(alice, bob);
        assert_ne!(alice.token(), bob.token());

        let gm = KnowledgeHolder::Gm;
        let party = KnowledgeHolder::PlayerParty;
        for npc in [&alice, &bob] {
            assert_ne!(npc.token(), gm.token());
            assert_ne!(npc.token(), party.token());
            // 即便某 NPC 的 actor id 字面就叫 "gm"/"player_party"，前缀也保证不撞。
            assert_ne!(npc.token(), "gm");
            assert_ne!(npc.token(), "player_party");
        }
        let evil = KnowledgeHolder::npc_from_actor_id("player_party").unwrap();
        assert_eq!(evil.token(), "npc:player_party");
        assert_ne!(evil.token(), party.token());
    }

    /// gm / player_party 行为不被 actor identity 契约改变：token / holder_id / durable 语义不变，
    /// 且仍对齐既有 KnowledgeHolderKind 序列化（player reveal 写穿路径不受影响）。
    #[test]
    fn player_reveal_behavior_unchanged_by_actor_identity() {
        assert_eq!(KnowledgeHolder::Gm.token(), "gm");
        assert_eq!(KnowledgeHolder::PlayerParty.token(), "player_party");
        assert_eq!(KnowledgeHolder::PlayerParty.holder_id(), None);
        assert!(KnowledgeHolder::Gm.is_durable_today());
        assert!(KnowledgeHolder::PlayerParty.is_durable_today());
        // 与既有 durable holder_kind 序列化对齐（player reveal 写 player_party 边的列值不变）。
        assert_eq!(
            serde_json::to_value(KnowledgeHolderKind::PlayerParty).unwrap(),
            json!(KnowledgeHolder::PlayerParty.kind_token())
        );
        assert_eq!(
            serde_json::to_value(KnowledgeHolderKind::Gm).unwrap(),
            json!(KnowledgeHolder::Gm.kind_token())
        );
    }
}

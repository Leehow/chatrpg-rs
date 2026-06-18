//! KnowledgeEdge 账本（P0b）：揭示事实的统一超集/投影源。
//! P0b 落 gm / player_party 两类 holder；P1 slice-1（holder identity gate 已开）起新增
//! `Npc` holder——durable NPC 知识边，holder_id 为校验过的稳定 NPC actor id（只读投影面，
//! 见 trpg-db list_npc_knowledge_entries / runtime load_npc_mind_view）。
//! revealed-facts 兼容投影 = 本表 (player_party, knows_true) 子集；
//! 与 domain_events.FactRevealed 写穿对齐（见 trpg-db record_revealed_fact）。
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 知识 holder 种类。P0b 起 gm / player_party；P1 slice-1（NPC holder identity gate 已开）
/// 起新增 `Npc`——durable `knowledge_edges` 的 NPC holder 种类。`Npc` 行的 `holder_id` 必须是
/// 经 [`KnowledgeHolder::npc_from_actor_id`] 校验的稳定 module-graph NPC id，绝不能是
/// `npc.opposition` 这类占位（DB 0034 的 CHECK 也作 backstop 拒绝）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeHolderKind {
    Gm,
    PlayerParty,
    Npc,
}

/// holder 对某 fact 的知识态。knows_true = 已确知为真（revealed 兼容投影只取此态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeState {
    Unknown,
    Exposed,
    KnowsTrue,
    BelievesFalse,
}

impl KnowledgeState {
    /// 是否为「确知为真」——唯一进真知识/世界真相投影的态。信念态（哪怕将来扩展的
    /// believes_true）一律不算确知，避免把信念当真相泄露给玩家。P0b 当前仅 `KnowsTrue`。
    pub fn is_true_knowledge(&self) -> bool {
        matches!(self, KnowledgeState::KnowsTrue)
    }

    /// 是否为信念态（相信/误信，非世界真相确证）。NPC mind view 据此把 false belief
    /// 与 known truth 区分。P0b 当前仅 `BelievesFalse` 属信念态。
    pub fn is_belief(&self) -> bool {
        matches!(self, KnowledgeState::BelievesFalse)
    }
}

/// 一条知识边：某 session 内 holder 对某 fact 的知识态及其来源事件/理由。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeEdge {
    pub edge_id: String,
    pub session_id: String,
    pub holder_kind: KnowledgeHolderKind,
    pub holder_id: Option<String>,
    pub fact_id: String,
    pub knowledge_state: KnowledgeState,
    pub source_event_id: Option<String>,
    pub reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

// -----------------------------------------------------------------------------
// TC-KNOW-00 Actor Identity Contract（稳定知识 holder 身份门）
//
// NPC（及未来 pc/faction）的 relationship / mind 必须挂在 source-backed / runtime-owned
// 的稳定 actor id 上，绝不能用展示名、念白文本、对抗方标签或 LLM 即兴串冒充。本契约提供
// 一个**纯内存**的身份校验类型 `KnowledgeHolder`，供 NpcRelationship / NpcMindView 做
// fail-closed 的 id 校验+规范化。
//
// 重要边界：本契约只定义并校验 holder **身份**，本身不写任何 durable knowledge_edges。
// P1 slice-1 起 `knowledge_edges` 已可挂 `npc` holder（见上方 `KnowledgeHolderKind::Npc`
// 与 DB 0034），但**只读投影面**：list_npc_knowledge_entries 读、测试用 SQL 直接 seed，
// 不存在「NPC 何时学到某事实」的 gameplay 写门（见 npc-holder-identity-gate spec slice-3）。
// 该读路径仍以本契约的稳定 id 校验为门：`npc.opposition` 等占位 fail-closed（见 PLACEHOLDER_IDS）。
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

impl std::error::Error for UnresolvedHolder {}

/// 占位/即兴串黑名单（小写精确匹配）。这些绝不能成为稳定 holder id。
const PLACEHOLDER_IDS: &[&str] = &[
    "unknown", "none", "null", "nil", "na", "tbd", "todo", "n/a", "?", "??", "???", "-", "--",
    "enemy", "enemies", "npc", "npcs", "actor", "target", "foe", "monster", "mob", "placeholder",
    "anonymous", "someone", "somebody", "stranger", "person", "guy", "thing", "it", "them",
    // 单槽 / 对抗占位：跨场景复用、非持久化的运行时占位 id（npc-holder-identity-gate spec
    // §2「Synthetic npc.opposition placeholder」）。绝不能当稳定 holder id 持久化。
    "npc.opposition", "opposition", "pc.current",
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
    let first = id.chars().next().unwrap();
    let id_shaped = first.is_ascii_alphanumeric()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if !id_shaped {
        return Err(UnresolvedHolder::new(UnresolvedReason::NotIdShaped, raw));
    }
    Ok(id.to_string())
}

/// 稳定知识 holder 身份（actor identity contract，纯内存校验类型）。
///
/// 派生 token：`gm` / `player_party` / `pc:<id>` / `npc:<id>` / `faction:<id>`。
/// 扩展策略 = 新增 variant（受 source/runtime 控制），绝不让 LLM 文本凭空构造 holder。
/// 注意：本类型只做 holder **身份**校验，不授权 durable/写面。P1 slice-1 起 `npc` durable
/// 读面已开（list_npc_knowledge_entries）；「NPC 何时学到某事实」gameplay 写门仍 gated。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KnowledgeHolder {
    /// GM / 叙事主权方。
    Gm,
    /// 玩家方集合 holder（与具体 PC 区分）。
    PlayerParty,
    /// 具体玩家角色（稳定 actor id）。
    PlayerCharacter(String),
    /// 具体 NPC（稳定 actor id）。
    Npc(String),
    /// 阵营 / 派系（稳定 id）。
    Faction(String),
}

impl KnowledgeHolder {
    /// holder 类 token。
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

    /// P0b 4-态语义：仅 knows_true 是真知识，believes_false 是信念，其余（unknown/exposed）皆非。
    #[test]
    fn knowledge_state_true_vs_belief() {
        assert!(KnowledgeState::KnowsTrue.is_true_knowledge());
        assert!(!KnowledgeState::KnowsTrue.is_belief());
        assert!(KnowledgeState::BelievesFalse.is_belief());
        assert!(!KnowledgeState::BelievesFalse.is_true_knowledge());
        for s in [KnowledgeState::Unknown, KnowledgeState::Exposed] {
            assert!(!s.is_true_knowledge());
            assert!(!s.is_belief());
        }
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
    }

    /// 仅有展示名、无稳定 actor id 的 NPC 输入必须 fail-closed，绝不拿展示名当 holder。
    #[test]
    fn actor_identity_rejects_display_name_only_holder() {
        let err = KnowledgeHolder::from_actor_ref(&npc_ref("", Some("The Butler")))
            .expect_err("display-name-only 必须被拒");
        assert_eq!(err.reason, UnresolvedReason::Empty);
        let err2 =
            KnowledgeHolder::npc_from_actor_id("The Butler").expect_err("展示名形态的串必须被拒");
        assert_eq!(err2.reason, UnresolvedReason::NotIdShaped);
    }

    /// 稳定 actor id 校验：空 / 占位 / 战斗对抗标签全部 fail-closed，合法 source id 通过且 trim。
    #[test]
    fn npc_holder_requires_stable_actor_id() {
        for bad in [
            "", "   ", "unknown", "NPC", "enemy", "Goblin #2", "the masked figure", "n/a",
        ] {
            assert!(
                KnowledgeHolder::npc_from_actor_id(bad).is_err(),
                "ad-hoc/占位/标签串必须被拒: {bad:?}"
            );
        }
        let ok = KnowledgeHolder::npc_from_actor_id("  npc_butler  ")
            .expect("trim 后的稳定 id 必须通过");
        assert_eq!(ok.holder_id(), Some("npc_butler"));
        assert_eq!(
            KnowledgeHolder::npc_from_actor_id("enemy").unwrap_err().reason,
            UnresolvedReason::Placeholder
        );
    }

    /// 战斗对抗占位 `npc.opposition`（及裸 `opposition` / `pc.current` 占位）绝不能成为
    /// 稳定 holder id——它跨场景复用、非持久化（见 npc-holder-identity-gate spec §2）。
    /// 必须 fail-closed，否则 durable npc 知识边会挂到一个下个场景指向别的 NPC 的占位上。
    #[test]
    fn npc_opposition_placeholder_fails_closed() {
        for placeholder in ["npc.opposition", "NPC.Opposition", "opposition", "pc.current"] {
            let err = KnowledgeHolder::npc_from_actor_id(placeholder)
                .expect_err("对抗/单槽占位串必须被拒，绝不持久化 NPC 知识边到占位");
            assert_eq!(
                err.reason,
                UnresolvedReason::Placeholder,
                "{placeholder:?} 应归因为 Placeholder"
            );
        }
    }

    /// 两个不同 NPC id 保持互异，且任一都不会与 gm / player_party 撞 token（前缀保证）。
    #[test]
    fn npc_holders_stay_distinct_and_never_collide_with_gm_or_party() {
        let alice = KnowledgeHolder::npc_from_actor_id("npc_alice").unwrap();
        let bob = KnowledgeHolder::npc_from_actor_id("npc_bob").unwrap();
        assert_ne!(alice, bob);
        assert_ne!(alice.token(), bob.token());
        let evil = KnowledgeHolder::npc_from_actor_id("player_party").unwrap();
        assert_eq!(evil.token(), "npc:player_party");
        assert_ne!(evil.token(), KnowledgeHolder::PlayerParty.token());
        assert_eq!(KnowledgeHolder::Gm.token(), "gm");
        assert_eq!(KnowledgeHolder::PlayerParty.holder_id(), None);
    }
}

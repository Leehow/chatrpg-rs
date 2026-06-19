//! Runtime KnowledgeProjection（P0b + TC-D3-05）：把 DB 的 KnowledgeEdge 账本投影成
//! 运行时反剧透放行集，并提供四个**显式 viewer/speaker projection**（替换"一个通用
//! retrieve"）：玩家叙事 / GM 裁决 / NPC 言谈 / NPC 行动。
//!
//! 设计契约（与 TC-D3-05 架构约束一致）：
//! - **玩家叙事投影**只含 player_party 已确知为真（knows_true）的 fact_id —— 绝不含
//!   GM-only 隐藏真相或某 NPC 私有知识。供玩家可见泄漏校验放行集用。
//! - **GM 裁决投影**可含隐藏真相，但隐藏真相**结构上独立**于玩家已知集
//!   （独立字段 [`GmAdjudicationProjection::gm_truth_fact_ids`]），绝不被复用为
//!   玩家/NPC-safe 真相。
//! - **NPC 言谈 / 行动投影**只从该 NPC 自身的 durable profile/mind/relationship 路径
//!   载入（[`NpcMindView`] + secret-gated [`NpcBehaviorPlan`]）——GM 世界真相与其它
//!   holder 的知识绝不进入。言谈与行动 v1 共享内部装载，但各自独立类型以便后续分化。
//!
//! 这些投影是 fact-id / marker / plan 取向的，**不**做广义语义检索/搜索，也不取原文散文。
use std::collections::HashSet;

use anyhow::Result;
use trpg_db::Db;
use trpg_model::{
    ActorRef, KnowledgeHolder, NpcBehaviorPlan, NpcMindView, NpcProfile, NpcRelationshipTarget,
    UnresolvedHolder,
};

use crate::npc_behavior::{derive_npc_behavior_plan, viewer_behavior_context};
use crate::npc_mind::load_npc_mind_view;

/// 玩家方知识投影：本会话 player_party 已确知为真（knows_true）的 fact_id 集。
pub struct KnowledgeProjection {
    pub revealed_fact_ids: HashSet<String>,
}

/// 从 KnowledgeEdge 账本拉取本会话 player_party 的 knows_true 事实集。
/// DB 抖动直接向上抛 Err，由调用方决定 fail-closed 策略（SceneNeedResolver
/// 取不到 → 空集 = 全部按未揭示裁剪，宁可不泄不赌 DB）。
pub async fn player_knowledge_projection(db: &Db, session_id: &str) -> Result<KnowledgeProjection> {
    let ids = db.list_player_known_fact_ids(session_id).await?;
    Ok(KnowledgeProjection {
        revealed_fact_ids: ids.into_iter().collect(),
    })
}

// ============================ TC-D3-05 viewer/speaker projections ============================

/// **玩家叙事投影**（`for_player_narration`）：只含 player_party 已知/已揭示的 fact_id。
/// 这是玩家可见念白泄漏校验的放行集——某 fact 不在集内即视为玩家未知，提前点名即泄漏。
/// **绝不**含 GM-only 隐藏真相或某 NPC 的私有知识。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlayerNarrationProjection {
    /// player_party 已知为真的 fact_id 集（player-safe）。
    pub player_known_fact_ids: HashSet<String>,
}

impl PlayerNarrationProjection {
    /// 纯构造：从一组玩家已知 fact_id 直接建投影（无 IO，便于单测与缓存）。
    pub fn from_player_known<I, S>(ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            player_known_fact_ids: ids.into_iter().map(Into::into).collect(),
        }
    }

    /// 玩家是否已知该 fact（已知 → 可自由复述，不算泄漏）。
    pub fn allows(&self, fact_id: &str) -> bool {
        self.player_known_fact_ids.contains(fact_id)
    }

    /// 稳定排序的玩家已知 fact_id 列表（喂给纯校验器的 `player_known_fact_ids` 参数）。
    pub fn known_fact_ids_sorted(&self) -> Vec<String> {
        let mut v: Vec<String> = self.player_known_fact_ids.iter().cloned().collect();
        v.sort();
        v
    }
}

/// **GM 裁决投影**（`for_gm_adjudication`）：可含隐藏真相，但隐藏真相与玩家已知集
/// **结构上分离**——`gm_truth_fact_ids` 是 GM holder 的 knows_true 全集（含玩家未知部分），
/// `player_known_fact_ids` 是玩家已知子集。二者绝不混用：`gm_truth_fact_ids` 永不被当作
/// 玩家/NPC-safe 集复用。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GmAdjudicationProjection {
    /// GM 世界真相 fact_id 集（可含玩家未知的隐藏真相）。
    pub gm_truth_fact_ids: HashSet<String>,
    /// 玩家已知子集（player-safe，与 GM 真相结构分离）。
    pub player_known_fact_ids: HashSet<String>,
}

impl GmAdjudicationProjection {
    /// 纯构造：从 GM 真相集与玩家已知集分别建投影（无 IO）。
    pub fn from_views<I, J, S, T>(gm_truth: I, player_known: J) -> Self
    where
        I: IntoIterator<Item = S>,
        J: IntoIterator<Item = T>,
        S: Into<String>,
        T: Into<String>,
    {
        Self {
            gm_truth_fact_ids: gm_truth.into_iter().map(Into::into).collect(),
            player_known_fact_ids: player_known.into_iter().map(Into::into).collect(),
        }
    }

    /// 隐藏真相 = GM 真相中**玩家尚不知道**的部分（裁决可读，绝不当作玩家可见集）。
    pub fn hidden_truth_fact_ids(&self) -> HashSet<String> {
        self.gm_truth_fact_ids
            .difference(&self.player_known_fact_ids)
            .cloned()
            .collect()
    }

    /// 把玩家已知子集投成 player-safe 的 [`PlayerNarrationProjection`]（显式降权，
    /// 调用方拿玩家叙事放行集时绝不会误取到隐藏真相）。
    pub fn player_narration_view(&self) -> PlayerNarrationProjection {
        PlayerNarrationProjection {
            player_known_fact_ids: self.player_known_fact_ids.clone(),
        }
    }
}

/// 一个 NPC 自身的 prompt-safe 投影内核：仅该 NPC 的 mind view + secret-gated 行为计划。
/// 言谈与行动投影 v1 共享它（各自 newtype 包装），GM 世界真相与其它 holder 知识绝不进入。
#[derive(Debug, Clone, PartialEq)]
pub struct NpcOwnedProjection {
    pub npc_id: String,
    /// 该 NPC 自身的 prompt-safe mind view（已 drop GM-only secret，区分 known/belief）。
    pub mind: NpcMindView,
    /// 该 NPC 的 secret-gated 行为计划（facts_can_reveal / facts_will_withhold 等）。
    pub plan: NpcBehaviorPlan,
}

impl NpcOwnedProjection {
    fn new(mind: NpcMindView, plan: NpcBehaviorPlan) -> Self {
        Self {
            npc_id: mind.npc_id.clone(),
            mind,
            plan,
        }
    }

    /// 该 NPC 确知为真的 fact_id（绝不含 belief、绝不含 GM-only / 它者知识）。
    pub fn known_fact_ids(&self) -> Vec<&str> {
        self.mind.known_fact_ids()
    }

    /// 该 NPC 本回合**允许披露**的 fact_id（⊆ known，secret 门控后）。
    pub fn revealable_fact_ids(&self) -> &[String] {
        &self.plan.facts_can_reveal
    }

    /// 该 NPC 本回合**保留不说**的 fact_id（门下 secret）。
    pub fn withheld_fact_ids(&self) -> &[String] {
        &self.plan.facts_will_withhold
    }
}

/// **NPC 言谈投影**（`for_npc_speech`）：包装 [`NpcOwnedProjection`]，供 NPC 言谈一致性
/// 校验（披露是否越出 facts_can_reveal、是否把 belief 当真相断言）。
#[derive(Debug, Clone, PartialEq)]
pub struct NpcSpeechProjection(pub NpcOwnedProjection);

impl std::ops::Deref for NpcSpeechProjection {
    type Target = NpcOwnedProjection;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// **NPC 行动投影**（`for_npc_action`）：独立类型供 NPC 行动一致性校验；v1 与言谈共享内部
/// 装载（同 NPC-owned mind/plan），但单独命名以便后续分化（如行动专属门控）。
#[derive(Debug, Clone, PartialEq)]
pub struct NpcActionProjection(pub NpcOwnedProjection);

impl std::ops::Deref for NpcActionProjection {
    type Target = NpcOwnedProjection;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// `for_player_narration`：拉取本会话玩家叙事投影（只含玩家已知 fact）。DB 抖动向上抛 Err，
/// 由调用方 fail-closed（取不到 → 空集 = 全按未知）。
pub async fn project_for_player_narration(
    db: &Db,
    session_id: &str,
) -> Result<PlayerNarrationProjection> {
    let ids = db.player_knowledge_view(session_id).await?;
    Ok(PlayerNarrationProjection::from_player_known(ids))
}

/// `for_gm_adjudication`：拉取 GM 裁决投影（GM 真相 + 玩家已知子集，结构分离）。
/// 隐藏真相留在 `gm_truth_fact_ids`，绝不混入玩家可见集。
pub async fn project_for_gm_adjudication(
    db: &Db,
    session_id: &str,
) -> Result<GmAdjudicationProjection> {
    let gm_truth = db.gm_truth_view(session_id).await?;
    let player_known = db.player_knowledge_view(session_id).await?;
    Ok(GmAdjudicationProjection::from_views(gm_truth, player_known))
}

/// 纯装配：从已载入的 NPC mind view + 玩家已知 fact 集派生 NPC-owned 投影内核。
/// secret 门复用 [`viewer_behavior_context`]（玩家未知的「NPC 已知为真」fact → withheld）。
fn assemble_npc_owned_projection(
    mind: NpcMindView,
    player_known_fact_ids: &[String],
) -> NpcOwnedProjection {
    let ctx = viewer_behavior_context(&mind, player_known_fact_ids);
    let plan = derive_npc_behavior_plan(&mind, &ctx);
    NpcOwnedProjection::new(mind, plan)
}

/// `for_npc_speech`：从该 NPC 自身 durable profile/mind/relationship 路径载入言谈投影。
/// 只读该 NPC 自己的边（DB 查询按 holder 过滤）—— GM 世界真相与其它 NPC 知识绝不进入。
/// secret 门控按 `player_known_fact_ids`（玩家未知的「NPC 已知为真」fact → 不可披露）。
pub async fn project_for_npc_speech(
    db: &Db,
    session_id: &str,
    npc_actor_id: &str,
    profile: &NpcProfile,
    relationship_targets: &[NpcRelationshipTarget],
    player_known_fact_ids: &[String],
) -> Result<NpcSpeechProjection> {
    let mind =
        load_npc_mind_view(db, session_id, npc_actor_id, profile, relationship_targets).await?;
    Ok(NpcSpeechProjection(assemble_npc_owned_projection(
        mind,
        player_known_fact_ids,
    )))
}

/// `for_npc_action`：同 [`project_for_npc_speech`] 的装载路径，产 NPC 行动投影。
/// v1 共享 NPC-owned mind/plan 内核，独立类型供行动一致性校验。
pub async fn project_for_npc_action(
    db: &Db,
    session_id: &str,
    npc_actor_id: &str,
    profile: &NpcProfile,
    relationship_targets: &[NpcRelationshipTarget],
    player_known_fact_ids: &[String],
) -> Result<NpcActionProjection> {
    let mind =
        load_npc_mind_view(db, session_id, npc_actor_id, profile, relationship_targets).await?;
    Ok(NpcActionProjection(assemble_npc_owned_projection(
        mind,
        player_known_fact_ids,
    )))
}

/// 纯装配 seam（言谈）：从已构建的 mind view 直接产言谈投影（无 IO，供单测）。
pub fn npc_speech_projection_from_mind(
    mind: NpcMindView,
    player_known_fact_ids: &[String],
) -> NpcSpeechProjection {
    NpcSpeechProjection(assemble_npc_owned_projection(mind, player_known_fact_ids))
}

/// 纯装配 seam（行动）：从已构建的 mind view 直接产行动投影（无 IO，供单测）。
pub fn npc_action_projection_from_mind(
    mind: NpcMindView,
    player_known_fact_ids: &[String],
) -> NpcActionProjection {
    NpcActionProjection(assemble_npc_owned_projection(mind, player_known_fact_ids))
}

/// 运行时入口：把一个 runtime/source 的 [`ActorRef`] 解析为稳定知识 holder（TC-KNOW-00
/// actor identity 契约）。委托 [`KnowledgeHolder::from_actor_ref`] —— 用稳定 actor_id 而非
/// 展示名，解析不出即返回 typed [`UnresolvedHolder`]（fail-closed，不发明 holder）。
/// NPC durable 持久化仍 gated 到 TC-KNOW-04；本函数只产出身份，不写任何边。
pub fn resolve_runtime_holder(actor: &ActorRef) -> Result<KnowledgeHolder, UnresolvedHolder> {
    KnowledgeHolder::from_actor_ref(actor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use trpg_model::{
        ActorKind, KnowledgeState, NpcKnowledgeEntry, NpcProfile, NpcSecret, Visibility,
    };

    fn npc_profile_with_gm_secret() -> NpcProfile {
        NpcProfile {
            actor_id: "npc_alice".into(),
            name: "Alice".into(),
            secrets: vec![NpcSecret {
                secret_id: "s".into(),
                content: "GMONLY_secret".into(),
                visibility: Visibility::GmOnly,
                source_refs: vec![],
            }],
            ..Default::default()
        }
    }

    fn mind_view(entries: &[NpcKnowledgeEntry]) -> NpcMindView {
        NpcMindView::build(
            "s",
            "npc_alice",
            &npc_profile_with_gm_secret(),
            &[],
            entries,
        )
        .unwrap()
    }

    /// 玩家叙事投影只放行玩家已知 fact；玩家未知 fact 一律排除（提前点名即泄漏）。
    #[test]
    fn projection_for_player_narration_excludes_unknown_fact() {
        let proj = PlayerNarrationProjection::from_player_known(["fact_known"]);
        assert!(proj.allows("fact_known"), "玩家已知 fact 必放行");
        assert!(
            !proj.allows("fact_hidden"),
            "玩家未知 fact 绝不进玩家叙事投影"
        );
        assert_eq!(proj.known_fact_ids_sorted(), vec!["fact_known".to_string()]);
    }

    /// GM 裁决投影保留隐藏真相，但隐藏真相与玩家已知集结构分离；玩家视图绝不含隐藏真相。
    #[test]
    fn projection_for_gm_adjudication_keeps_hidden_truth_separate() {
        let proj =
            GmAdjudicationProjection::from_views(["fact_shared", "fact_hidden"], ["fact_shared"]);
        // GM 真相含隐藏真相。
        assert!(proj.gm_truth_fact_ids.contains("fact_hidden"));
        // 玩家已知集**不**含隐藏真相（结构分离）。
        assert!(!proj.player_known_fact_ids.contains("fact_hidden"));
        // 隐藏真相 = GM 真相 − 玩家已知。
        let hidden = proj.hidden_truth_fact_ids();
        assert_eq!(hidden.len(), 1);
        assert!(hidden.contains("fact_hidden"));
        // 降权到玩家叙事视图后绝不暴露隐藏真相。
        let player_view = proj.player_narration_view();
        assert!(player_view.allows("fact_shared"));
        assert!(
            !player_view.allows("fact_hidden"),
            "玩家叙事视图绝不含隐藏真相"
        );
    }

    /// NPC 言谈投影只取该 NPC 自身 mind（自有 known fact），不含 GM-only secret 或它者知识。
    #[test]
    fn projection_for_npc_speech_uses_npc_mind_only() {
        let entries = vec![
            NpcKnowledgeEntry {
                fact_id: "npc_known".into(),
                state: KnowledgeState::KnowsTrue,
            },
            NpcKnowledgeEntry {
                fact_id: "npc_belief".into(),
                state: KnowledgeState::BelievesFalse,
            },
        ];
        // 玩家什么都不知道 → NPC 已知为真的 fact 默认是 withheld secret。
        let proj = npc_speech_projection_from_mind(mind_view(&entries), &[]);
        assert_eq!(proj.npc_id, "npc_alice");
        // 只含该 NPC 自身 known fact，不含 belief、不含 GM-only secret 正文。
        assert_eq!(proj.known_fact_ids(), vec!["npc_known"]);
        assert!(
            !proj.known_fact_ids().contains(&"npc_belief"),
            "belief 绝不当作 known truth"
        );
        assert!(
            !proj.mind.speech_context().contains("GMONLY_secret"),
            "GM-only secret 绝不进 NPC 言谈投影"
        );
        // 不含外来/GM-only fact_id。
        assert!(!proj.known_fact_ids().contains(&"fact_hidden"));
    }

    /// NPC 行动投影与言谈共享 NPC-owned 内核（同自有 mind/plan），独立类型。
    #[test]
    fn projection_for_npc_action_shares_npc_owned_core() {
        let entries = vec![NpcKnowledgeEntry {
            fact_id: "npc_known".into(),
            state: KnowledgeState::KnowsTrue,
        }];
        let action = npc_action_projection_from_mind(mind_view(&entries), &[]);
        let speech = npc_speech_projection_from_mind(mind_view(&entries), &[]);
        assert_eq!(action.npc_id, "npc_alice");
        assert_eq!(action.known_fact_ids(), speech.known_fact_ids());
        assert_eq!(action.plan, speech.plan, "v1 共享 NPC-owned plan 内核");
    }

    fn actor(actor_id: &str, kind: ActorKind, display: Option<&str>) -> ActorRef {
        ActorRef {
            actor_id: actor_id.to_string(),
            actor_kind: kind,
            display_name: display.map(str::to_string),
        }
    }

    /// 运行时 ActorRef → 稳定 NPC holder：用 source actor_id，holder token 稳定带前缀。
    #[test]
    fn actor_identity_resolves_runtime_actor_ref_to_holder() {
        let holder = resolve_runtime_holder(&actor("npc_alice", ActorKind::Npc, Some("Alice")))
            .expect("runtime NPC actor 必须解析为稳定 holder");
        assert_eq!(holder.token(), "npc:npc_alice");
        // environment / hazard / system 不是知识 holder → fail-closed。
        assert!(resolve_runtime_holder(&actor("env_storm", ActorKind::Environment, None)).is_err());
        // 仅有展示名、无稳定 id → fail-closed，绝不拿展示名当 holder。
        assert!(resolve_runtime_holder(&actor("", ActorKind::Npc, Some("The Butler"))).is_err());
    }

    /// holder-keyed 存储下，写某个目标 NPC 的事实只动该 holder——稳定 token 保证隔离，
    /// 绝不外溢到别的 NPC 或 player_party。（durable 持久化逻辑将据此 key 隔离。）
    #[test]
    fn npc_learned_fact_updates_only_target_actor() {
        let alice = resolve_runtime_holder(&actor("npc_alice", ActorKind::Npc, None)).unwrap();
        let bob = resolve_runtime_holder(&actor("npc_bob", ActorKind::Npc, None)).unwrap();
        let party = KnowledgeHolder::PlayerParty;

        // 以 holder token 作 key 的每-holder 知识集（模拟 durable 投影的隔离不变量）。
        let mut store: HashMap<String, HashSet<String>> = HashMap::new();
        store
            .entry(alice.token())
            .or_default()
            .insert("fact_poison".to_string());

        // 只更新 Alice 这一个 holder。
        assert_eq!(
            store.get(&alice.token()).map(|s| s.len()),
            Some(1),
            "目标 NPC 习得事实"
        );
        assert!(store.get(&bob.token()).is_none(), "另一个 NPC 不被波及");
        assert!(
            store.get(&party.token()).is_none(),
            "player_party 绝不被 NPC 习得污染"
        );
    }
}

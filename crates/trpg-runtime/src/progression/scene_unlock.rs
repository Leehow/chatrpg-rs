//! E1 `scene_transition_gated_v1` — turn a COMPLETED `obj.scene_advance.<scene>` (D2's
//! ObjectiveResolved, `data.signal="scene_advance"`) into a PROGRESSION-GATED scene transition,
//! WITHOUT adding a second `current_scene` writer.
//!
//! This is the "engine emits `SceneUnlocked` only" half: pure, data-driven next-scene resolution +
//! a durable [`DomainEventKind::SceneUnlocked`] builder. The other half (the actual transition) is
//! the existing NavigationResolver ([`crate::scene_navigation::scene_navigate_critical`]) consuming
//! the unlock via [`pending_unlock_target`] — it stays the SOLE owner of `current_scene` (nav-split
//! preserved at the ownership level; E1 only evolves WHEN it transitions, not WHO writes the scene).
//!
//! Reconciliation with design §七 (anti-railroad, architect-AUTHORIZED): the transition fires ONLY
//! when the player EARNED it (the scene's advance objective completed from the player's own real
//! admitted observations) — the player drove the progression by engaging the scene's salient
//! content. E1 honors D2's definition of "earned" (objective completion); the strictness of that
//! threshold (how much engagement counts) is D2/D3's concern, OUT of E1 scope.
//!
//! Discipline:
//! - **standalone flag** `TRPG_PROGRESS_SCENE_TRANSITION_GATED_V1`, default OFF, NOT ORed into any
//!   master ⇒ OFF == byte-identical to 39e949e (no SceneUnlocked emitted; the navigator's consume
//!   block is skipped). The system-level OFF arm is the supervisor's 30-turn A/B.
//! - **data-driven / structural target, ZERO name-branching**: authored out-edge (an
//!   `is_authored_flow_link` link) if present, else — only for CONTINUOUS-spine document types
//!   (OneShot/Campaign, never a ScenarioCollection anthology) and a non-anthology graph — the next
//!   scene in [`super::next_spine_scene`] (page sequence). Keys on `DocumentType` (structural
//!   classification) + `links` + page order, never on `ruleset_id`/`module_id` NAME.
//! - **fail-closed**: no current scene / no resolvable+sensible next scene ⇒ NO SceneUnlocked (never
//!   teleport/fabricate). The_vault (ScenarioCollection, 12 independent missions) ⇒ no spine
//!   fallback ⇒ no teleport, even though its `scene_advance` objective can complete.
//! - **true-consumption idempotency**: an unlock(from→next) is consumed once a
//!   `SceneTransitioned(from→next)` is committed, so RETURNING to `from` later never re-fires it.
use std::env;
use trpg_model::{DomainEvent, DomainEventKind, ModuleGraph};

use crate::scene_navigation::is_authored_flow_link;

const SCENE_TRANSITION_GATED_ENV: &str = "TRPG_PROGRESS_SCENE_TRANSITION_GATED_V1";

fn flag_on(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "on" | "yes"
    )
}

/// Whether E1's progression-gated scene transition is active this run. Default OFF and STANDALONE
/// (no master OR) ⇒ OFF == byte-identical baseline (no unlock emitted, navigator consume skipped).
pub fn scene_transition_gated_enabled() -> bool {
    env::var(SCENE_TRANSITION_GATED_ENV)
        .map(|v| flag_on(&v))
        .unwrap_or(false)
}

/// Resolve the data-driven next-scene target for an EARNED `from_scene` advance, or None
/// (fail-closed). Pure / deterministic.
///
/// 1. **Authored out-edge** (always honored, any document type): the first `is_authored_flow_link`
///    (non-Spatial + non-empty `source_anchor`: sequential/trigger/branch) link of `from_scene`
///    whose `to_node_id` is a real scene `!= from`. This is an author-intended transition.
/// 2. **Spine-order fallback** — ONLY when the module forms a CONTINUOUS narrative spine
///    (`DocumentType::scenes_form_continuous_spine`) AND the graph is not a mission anthology
///    (`missions.is_empty()`, belt-and-suspenders for anthology induction): the next scene after
///    `from` in page sequence ([`super::next_spine_scene`]), if it is a real scene `!= from`.
/// 3. Else None (fail-closed: honest freeze, never teleport across unrelated units).
pub fn resolve_next_scene(graph: &ModuleGraph, from_scene: &str) -> Option<String> {
    let from = from_scene.trim();
    if from.is_empty() {
        return None;
    }
    let from_node = graph.scenes.iter().find(|s| s.node_id.trim() == from)?;
    let is_real_scene =
        |id: &str| -> bool { graph.scenes.iter().any(|s| s.node_id.trim() == id.trim()) };

    // 1) authored out-edge (always honored).
    if let Some(next) = from_node
        .links
        .iter()
        .filter(|l| is_authored_flow_link(l))
        .map(|l| l.to_node_id.trim().to_string())
        .find(|to| !to.is_empty() && to != from && is_real_scene(to))
    {
        return Some(next);
    }

    // 2) spine-order fallback — gated on continuous-spine doc type AND non-anthology graph.
    if graph.module_type.scenes_form_continuous_spine() && graph.missions.is_empty() {
        if let Some(next) = super::next_spine_scene(graph, from) {
            let next = next.trim().to_string();
            if !next.is_empty() && next != from && is_real_scene(&next) {
                return Some(next);
            }
        }
    }

    // 3) fail-closed: no sensible target ⇒ no transition.
    None
}

/// Build the durable [`DomainEventKind::SceneUnlocked`] event for an EARNED `from_scene` advance,
/// or None when no sensible next scene resolves (fail-closed). The runtime appends this AFTER a
/// `scene_advance` ObjectiveResolved fires for `from_scene` (E1 flag on). **nav-split**: this event
/// only records "next scene unlocked"; it NEVER writes `current_scene` — the NavigationResolver
/// does that. Idempotent `event_id = de_sceneunlocked_{session}_{from_scene}` (one per source scene
/// per session; `append_domain_event` dedups). `turn_id` records when it first fired.
pub fn scene_unlock_event(
    session_id: &str,
    turn_id: &str,
    graph: &ModuleGraph,
    from_scene: &str,
) -> Option<DomainEvent> {
    let from = from_scene.trim();
    if from.is_empty() {
        return None;
    }
    let next = resolve_next_scene(graph, from)?; // fail-closed
    let data = serde_json::json!({
        "from_scene": from,
        "next_scene": next,
        "basis_objective": format!("obj.scene_advance.{from}"),
        "signal": "scene_unlock",
    });
    Some(DomainEvent::new(
        format!("de_sceneunlocked_{session_id}_{from}"),
        session_id.to_string(),
        turn_id.to_string(),
        DomainEventKind::SceneUnlocked,
        data,
    ))
}

/// E1 consume gate (pure): given the session's committed domain `events` and the `current` scene,
/// return the next scene to transition to IFF there is an UNCONSUMED progression-gated unlock for
/// the current scene. None otherwise (fail-closed). The NavigationResolver calls this and, on Some,
/// performs the one scene write.
///
/// Gates:
/// - the unlock's `from_scene == current` (only act on an unlock for the scene we are in; once we
///   transition, `current` changes, so it stops matching);
/// - **true-consumption**: skip if a `SceneTransitioned(from=current → to=next)` is already
///   committed — so returning to `current` later never re-fires the same unlock (codex idempotency
///   hole). The within-turn emit→consume still works (execute.rs writes SceneTransitioned AFTER the
///   navigator returns, so the check is false on the firing turn and true forever after).
/// - `next` non-empty and `!= current`.
pub fn pending_unlock_target(events: &[DomainEvent], current: &str) -> Option<String> {
    let cur = current.trim();
    if cur.is_empty() {
        return None;
    }
    let unlock = events.iter().rev().find(|e| {
        e.kind == DomainEventKind::SceneUnlocked
            && e.data
                .get("from_scene")
                .and_then(|v| v.as_str())
                .map(str::trim)
                == Some(cur)
    })?;
    let next = unlock
        .data
        .get("next_scene")
        .and_then(|v| v.as_str())?
        .trim()
        .to_string();
    if next.is_empty() || next == cur {
        return None;
    }
    let already_transitioned = events.iter().any(|e| {
        e.kind == DomainEventKind::SceneTransitioned
            && e.data.get("from").and_then(|v| v.as_str()).map(str::trim) == Some(cur)
            && e.data.get("to").and_then(|v| v.as_str()).map(str::trim) == Some(next.as_str())
    });
    if already_transitioned {
        return None;
    }
    Some(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use trpg_model::{DocumentType, LinkType, ScenarioLink, ScenarioNode};

    fn scene(id: &str, page: u32) -> ScenarioNode {
        ScenarioNode {
            node_id: id.into(),
            node_type: "scene".into(),
            title: id.into(),
            page_start: Some(page),
            ..Default::default()
        }
    }

    fn link(to: &str, lt: LinkType, anchor: Option<&str>) -> ScenarioLink {
        ScenarioLink {
            to_node_id: to.into(),
            reason: "r".into(),
            clue_id: None,
            link_type: lt,
            source_anchor: anchor.map(str::to_string),
        }
    }

    /// homecoming-shaped: OneShot, scenes only spatial-bridged (no authored flow link), no missions.
    fn one_shot_graph() -> ModuleGraph {
        ModuleGraph {
            module_id: "homecoming".into(),
            module_type: DocumentType::OneShot,
            scenes: vec![
                {
                    let mut s = scene("scene_01", 5);
                    s.links = vec![
                        link("scene_02", LinkType::Spatial, None),
                        link("scene_03", LinkType::Spatial, None),
                    ];
                    s
                },
                scene("scene_02", 7),
                scene("scene_03", 9),
            ],
            ..Default::default()
        }
    }

    /// the_vault-shaped: ScenarioCollection, scene_001 has only an anchorless fallback sequential
    /// (NOT an authored flow link), independent missions by page gap.
    fn anthology_graph() -> ModuleGraph {
        ModuleGraph {
            module_id: "the_vault".into(),
            module_type: DocumentType::ScenarioCollection,
            scenes: vec![
                {
                    let mut s = scene("scene_001", 8);
                    s.links = vec![link("scene_002", LinkType::Sequential, None)]; // anchorless fallback
                    s
                },
                scene("scene_002", 22),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn flag_defaults_off_and_is_standalone() {
        assert!(!flag_on(""));
        assert!(!flag_on("0"));
        assert!(flag_on("1"));
        assert!(flag_on("on"));
        // unset env ⇒ OFF (no test in this binary sets the var; not ORed into any master).
        assert!(!scene_transition_gated_enabled());
    }

    #[test]
    fn one_shot_resolves_spine_next_when_no_authored_edge() {
        // homecoming: scene_01's links are all anchorless spatial ⇒ no authored out-edge ⇒
        // continuous-spine fallback ⇒ scene_02 (page order).
        let g = one_shot_graph();
        assert_eq!(
            resolve_next_scene(&g, "scene_01").as_deref(),
            Some("scene_02")
        );
        assert_eq!(
            resolve_next_scene(&g, "scene_02").as_deref(),
            Some("scene_03")
        );
        assert_eq!(
            resolve_next_scene(&g, "scene_03"),
            None,
            "terminal ⇒ fail-closed"
        );
    }

    #[test]
    fn anthology_never_spine_teleports_even_when_advance_completes() {
        // the_vault: ScenarioCollection + anchorless fallback link ⇒ NO authored out-edge AND
        // spine fallback gated off ⇒ None. No teleport scene_001→scene_002 across missions.
        let g = anthology_graph();
        assert_eq!(
            resolve_next_scene(&g, "scene_001"),
            None,
            "anthology must NOT page-teleport across independent missions"
        );
    }

    #[test]
    fn authored_flow_link_is_honored_in_any_document_type() {
        // Even a ScenarioCollection honors a REAL authored flow link (non-spatial + anchored):
        // that is an author-intended transition (e.g. a sub-beat within one mission).
        let mut g = anthology_graph();
        g.scenes[0].links = vec![link(
            "scene_002",
            LinkType::Sequential,
            Some("press onward"),
        )];
        assert_eq!(
            resolve_next_scene(&g, "scene_001").as_deref(),
            Some("scene_002"),
            "authored flow link honored regardless of document type"
        );
    }

    #[test]
    fn authored_flow_link_wins_over_spine_order() {
        // A OneShot scene with BOTH an authored flow link and a spine successor → authored wins.
        let mut g = one_shot_graph();
        g.scenes[0].links = vec![
            link("scene_03", LinkType::Trigger, Some("once the alarm trips")),
            link("scene_02", LinkType::Spatial, None),
        ];
        assert_eq!(
            resolve_next_scene(&g, "scene_01").as_deref(),
            Some("scene_03"),
            "authored out-edge takes precedence over page-order spine"
        );
    }

    #[test]
    fn non_continuous_doc_types_fail_closed_without_authored_edge() {
        // Supplement/Unknown/CoreRulebook carry no playable continuous spine ⇒ no spine fallback.
        for dt in [
            DocumentType::Supplement,
            DocumentType::Unknown,
            DocumentType::AssetCatalog,
        ] {
            let mut g = one_shot_graph();
            g.module_type = dt;
            assert_eq!(
                resolve_next_scene(&g, "scene_01"),
                None,
                "non-continuous doc type ⇒ spine fallback off"
            );
        }
    }

    #[test]
    fn populated_missions_blocks_spine_fallback_belt_and_suspenders() {
        // If anthology induction populated `missions`, treat as anthology even if mis-typed OneShot.
        let mut g = one_shot_graph();
        g.missions = vec![scene("mission_a", 5)];
        assert_eq!(
            resolve_next_scene(&g, "scene_01"),
            None,
            "non-empty missions ⇒ anthology ⇒ no spine teleport"
        );
    }

    #[test]
    fn blank_or_unknown_from_scene_fails_closed() {
        let g = one_shot_graph();
        assert_eq!(resolve_next_scene(&g, "   "), None);
        assert_eq!(resolve_next_scene(&g, "not_a_scene"), None);
    }

    #[test]
    fn scene_unlock_event_built_for_resolvable_target_only() {
        let g = one_shot_graph();
        let ev = scene_unlock_event("sess-1", "turn-7", &g, "scene_01").expect("resolvable");
        assert_eq!(ev.kind, DomainEventKind::SceneUnlocked);
        assert_eq!(ev.event_id, "de_sceneunlocked_sess-1_scene_01");
        assert_eq!(ev.session_id, "sess-1");
        assert_eq!(ev.turn_id, "turn-7");
        assert_eq!(ev.data["from_scene"], "scene_01");
        assert_eq!(ev.data["next_scene"], "scene_02");
        assert_eq!(ev.data["basis_objective"], "obj.scene_advance.scene_01");
        // idempotent, turn-independent id.
        let ev2 = scene_unlock_event("sess-1", "turn-9", &g, "scene_01").unwrap();
        assert_eq!(ev.event_id, ev2.event_id);
    }

    #[test]
    fn scene_unlock_event_fail_closed_no_target() {
        // anthology / terminal ⇒ no event (fail-closed, never fabricate an unlock).
        let g = anthology_graph();
        assert!(scene_unlock_event("s", "t", &g, "scene_001").is_none());
        let one = one_shot_graph();
        assert!(
            scene_unlock_event("s", "t", &one, "scene_03").is_none(),
            "terminal ⇒ none"
        );
    }

    // ── consume gate ──────────────────────────────────────────────────────────────────────
    fn unlock_ev(from: &str, next: &str) -> DomainEvent {
        DomainEvent::new(
            format!("de_sceneunlocked_s_{from}"),
            "s".to_string(),
            "t1".to_string(),
            DomainEventKind::SceneUnlocked,
            json!({"from_scene": from, "next_scene": next}),
        )
    }
    fn transition_ev(from: &str, to: &str) -> DomainEvent {
        DomainEvent::new(
            format!("de_t_{from}_{to}"),
            "s".to_string(),
            "t2".to_string(),
            DomainEventKind::SceneTransitioned,
            json!({"from": from, "to": to, "reason": "x"}),
        )
    }

    #[test]
    fn consume_returns_next_for_pending_unlock_at_current() {
        let events = vec![unlock_ev("scene_01", "scene_02")];
        assert_eq!(
            pending_unlock_target(&events, "scene_01").as_deref(),
            Some("scene_02")
        );
    }

    #[test]
    fn consume_skips_unlock_whose_from_is_not_current() {
        // unlock for scene_01 but we are in scene_02 ⇒ no force.
        let events = vec![unlock_ev("scene_01", "scene_02")];
        assert_eq!(pending_unlock_target(&events, "scene_02"), None);
    }

    #[test]
    fn consume_true_consumption_no_refire_after_transition() {
        // codex idempotency hole: unlock scene_01→scene_02 ALREADY transitioned; returning to
        // scene_01 must NOT re-force scene_02.
        let events = vec![
            unlock_ev("scene_01", "scene_02"),
            transition_ev("scene_01", "scene_02"),
        ];
        assert_eq!(
            pending_unlock_target(&events, "scene_01"),
            None,
            "consumed unlock (transition committed) must not re-fire on return"
        );
    }

    #[test]
    fn consume_within_turn_before_transition_committed_fires() {
        // On the firing turn the SceneTransitioned is not yet committed (execute.rs writes it AFTER
        // the navigator returns) ⇒ consume returns the target.
        let events = vec![unlock_ev("scene_01", "scene_02")];
        assert_eq!(
            pending_unlock_target(&events, "scene_01").as_deref(),
            Some("scene_02")
        );
    }

    #[test]
    fn consume_fail_closed_on_blank_or_self_target() {
        assert_eq!(pending_unlock_target(&[], "scene_01"), None);
        assert_eq!(
            pending_unlock_target(&[unlock_ev("scene_01", "scene_01")], "scene_01"),
            None
        );
        assert_eq!(
            pending_unlock_target(&[unlock_ev("scene_01", "  ")], "scene_01"),
            None
        );
        assert_eq!(
            pending_unlock_target(&[unlock_ev("scene_01", "scene_02")], "   "),
            None
        );
    }
}

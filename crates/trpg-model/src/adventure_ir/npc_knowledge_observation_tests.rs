//! Unit tests for [`super`] (`npc_knowledge_observation`). Split out of the module file to
//! keep each source file < 400 lines (factory constraint); included as a child `mod tests`
//! via `#[path]` so `use super::*` retains access to the parent module's private items.

use super::*;
use crate::adventure_ir::compile_authored_observations;
use crate::ScenarioNode;
use serde_json::json;
use std::sync::Mutex;

/// Serializes the env-mutating flag tests (process-global env).
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn npc(id: &str, name: &str, body: &str) -> Value {
    json!({"id": id, "name": name, "body": body})
}

/// A scene referencing `npc_ids`, with NO scene_mechanics / gm_notes / clue refs
/// (a talk scene — exactly scene_02's structural shape).
fn talk_scene(node_id: &str, page: u32, npc_ids: &[&str]) -> ScenarioNode {
    ScenarioNode {
        node_id: node_id.into(),
        node_type: "scene".into(),
        title: "Questions".into(),
        page_start: Some(page),
        referenced_npc_ids: npc_ids.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

fn graph(npcs: Vec<Value>, scenes: Vec<ScenarioNode>) -> ModuleGraph {
    ModuleGraph {
        module_id: "cyberpunk_red.homecoming".into(),
        npcs,
        scenes,
        ..Default::default()
    }
}

#[test]
fn talk_scene_npc_with_body_yields_scene_tagged_entity_encountered_atom() {
    // scene_02-shape: a talk scene whose referenced NPC carries authored body.
    let g = graph(
        vec![npc(
            "npc_athena",
            "Athena",
            "A rogue drone that can answer questions...",
        )],
        vec![talk_scene(
            "scene_02_questions_for_athena",
            12,
            &["npc_athena"],
        )],
    );
    let atoms = from_referenced_npc_knowledge(&g);
    assert_eq!(atoms.len(), 1, "one salient NPC ⇒ one atom");
    let a = &atoms[0];
    assert_eq!(
        a.kind,
        EvidenceKind::EntityEncountered,
        "entity-engagement kind"
    );
    assert_eq!(
        a.progress_role,
        ProgressRole::CarrierOnly,
        "compiler emits CarrierOnly"
    );
    assert!(
        a.bindings.iter().any(|b| b == "npc_athena"),
        "bound to the NPC ref"
    );
    assert!(
        a.bindings
            .iter()
            .any(|b| b == "scene:scene_02_questions_for_athena"),
        "scene-tagged for in-scene offer + scene_advance filter"
    );
    assert_eq!(
        a.grounding, "entity:scene_02_questions_for_athena:npc_athena",
        "scene-scoped grounding"
    );
    assert!(!a.source_refs.is_empty(), "source-grounded");
    assert_eq!(a.source_refs[0].source_id, "cyberpunk_red.homecoming");
    assert_eq!(a.source_refs[0].page, Some(12), "cites the scene page");
    assert!(
        a.source_refs[0]
            .note
            .as_deref()
            .unwrap_or("")
            .contains("Athena"),
        "note cites the authored NPC name/body"
    );
}

#[test]
fn bare_npc_reference_yields_no_atom_fail_closed() {
    // A scene referencing an NPC that is a bare id+name (no body, no secrets) —
    // the 4 non-Athena scene_02 refs. Never fabricate an advance condition.
    let g = graph(
        vec![json!({"id": "npc_shelob", "name": "Shelob"})],
        vec![talk_scene("scene_02", 12, &["npc_shelob"])],
    );
    assert!(
        from_referenced_npc_knowledge(&g).is_empty(),
        "fail-closed: bare NPC mention ⇒ no salient atom"
    );
}

#[test]
fn npc_with_only_secret_terms_yields_atom() {
    // body empty but authored spoiler.secret_terms present (knowledge to elicit).
    let g = graph(
        vec![json!({
            "id": "npc_athena", "name": "Athena", "body": "",
            "spoiler": {"secret_terms": ["Athena", "AI installed"]}
        })],
        vec![talk_scene("scene_02", 12, &["npc_athena"])],
    );
    let atoms = from_referenced_npc_knowledge(&g);
    assert_eq!(
        atoms.len(),
        1,
        "authored secrets count as salient knowledge"
    );
    assert_eq!(atoms[0].kind, EvidenceKind::EntityEncountered);
}

#[test]
fn same_npc_in_two_scenes_yields_two_distinct_atoms() {
    // codex GAP#5 regression: scene-scoped grounding + scene-scoped source_span
    // ⇒ the same NPC referenced by scene_01 AND scene_02 produces two atoms with
    // DISTINCT grounding AND atom_id, so neither is dropped by the grounding-dedup.
    let g = graph(
        vec![npc("npc_athena", "Athena", "A rogue drone...")],
        vec![
            talk_scene("scene_01_lawmen_in_trouble", 8, &["npc_athena"]),
            talk_scene("scene_02_questions_for_athena", 12, &["npc_athena"]),
        ],
    );
    let atoms = from_referenced_npc_knowledge(&g);
    assert_eq!(atoms.len(), 2, "one atom per scene for the same NPC");
    let groundings: Vec<&str> = atoms.iter().map(|a| a.grounding.as_str()).collect();
    assert!(groundings.contains(&"entity:scene_01_lawmen_in_trouble:npc_athena"));
    assert!(groundings.contains(&"entity:scene_02_questions_for_athena:npc_athena"));
    assert_ne!(
        atoms[0].atom_id, atoms[1].atom_id,
        "distinct atom_ids per scene"
    );
    // and they survive the real grounding-dedup in compile_authored_observations.
    let _g = ENV_LOCK.lock().unwrap();
    std::env::set_var(SCENE_NPC_KNOWLEDGE_ENV, "1");
    let compiled = compile_authored_observations(&g);
    std::env::remove_var(SCENE_NPC_KNOWLEDGE_ENV);
    let s1 = compiled.iter().any(|a| {
        a.bindings
            .iter()
            .any(|b| b == "scene:scene_01_lawmen_in_trouble")
    });
    let s2 = compiled.iter().any(|a| {
        a.bindings
            .iter()
            .any(|b| b == "scene:scene_02_questions_for_athena")
    });
    assert!(
        s1 && s2,
        "BOTH scenes' atoms survive grounding-dedup (s1={s1} s2={s2})"
    );
}

#[test]
fn dangling_npc_reference_yields_no_atom() {
    // referenced_npc_ids names an NPC absent from graph.npcs ⇒ fail-closed.
    let g = graph(vec![], vec![talk_scene("scene_02", 12, &["npc_ghost"])]);
    assert!(from_referenced_npc_knowledge(&g).is_empty());
}

#[test]
fn read_aloud_is_never_a_source() {
    // A scene with rich read_aloud but a BARE referenced NPC ⇒ no atom: the
    // flavour box is never parsed; salience comes only from NPC authored content.
    let mut scene = talk_scene("scene_02", 12, &["npc_bare"]);
    scene.read_aloud = Some("Athena shouts: investigate, examine, question her now!".into());
    let g = graph(vec![json!({"id": "npc_bare", "name": "Bare"})], vec![scene]);
    assert!(
        from_referenced_npc_knowledge(&g).is_empty(),
        "read_aloud verbs never fabricate an atom; bare NPC stays non-salient"
    );
}

#[test]
fn empty_module_id_yields_nothing_source_grounded() {
    let mut g = graph(
        vec![npc("npc_a", "A", "knows things")],
        vec![talk_scene("scene_02", 12, &["npc_a"])],
    );
    g.module_id = "  ".into();
    assert!(
        from_referenced_npc_knowledge(&g).is_empty(),
        "no real source id ⇒ no atom"
    );
}

#[test]
fn compile_off_excludes_npc_atom_on_byte_identical_baseline() {
    let _g = ENV_LOCK.lock().unwrap();
    std::env::remove_var(SCENE_NPC_KNOWLEDGE_ENV);
    let g = graph(
        vec![npc("npc_athena", "Athena", "A rogue drone...")],
        vec![talk_scene(
            "scene_02_questions_for_athena",
            12,
            &["npc_athena"],
        )],
    );
    // OFF: the talk scene compiles ZERO atoms (investigative-only baseline) — the
    // exact freeze condition the diagnostic found.
    let off = compile_authored_observations(&g);
    assert!(
        !off.iter().any(|a| a
            .bindings
            .iter()
            .any(|b| b == "scene:scene_02_questions_for_athena")),
        "OFF ⇒ no scene_02 atom (byte-identical to investigative-only baseline)"
    );
}

#[test]
fn compile_on_includes_npc_atom_for_talk_scene() {
    let _g = ENV_LOCK.lock().unwrap();
    std::env::set_var(SCENE_NPC_KNOWLEDGE_ENV, "1");
    let g = graph(
        vec![npc("npc_athena", "Athena", "A rogue drone...")],
        vec![talk_scene(
            "scene_02_questions_for_athena",
            12,
            &["npc_athena"],
        )],
    );
    let on = compile_authored_observations(&g);
    std::env::remove_var(SCENE_NPC_KNOWLEDGE_ENV);
    let athena = on.iter().find(|a| {
        a.bindings
            .iter()
            .any(|b| b == "scene:scene_02_questions_for_athena")
    });
    assert!(
        athena.is_some(),
        "ON ⇒ talk scene gains its salient NPC atom"
    );
    assert_eq!(athena.unwrap().kind, EvidenceKind::EntityEncountered);
    // compiler still only emits CarrierOnly (the validator's GuardLeaf drop holds).
    assert!(on
        .iter()
        .all(|a| a.progress_role == ProgressRole::CarrierOnly));
}

#[test]
fn flag_strict_whitelist() {
    let _g = ENV_LOCK.lock().unwrap();
    for off in ["0", "false", "off", "no", "", "enabled", "2"] {
        std::env::set_var(SCENE_NPC_KNOWLEDGE_ENV, off);
        assert!(!scene_npc_knowledge_enabled(), "{off:?} ⇒ OFF");
    }
    for on in ["1", "true", "on", "yes", "ON", "Yes"] {
        std::env::set_var(SCENE_NPC_KNOWLEDGE_ENV, on);
        assert!(scene_npc_knowledge_enabled(), "{on:?} ⇒ ON");
    }
    std::env::remove_var(SCENE_NPC_KNOWLEDGE_ENV);
}

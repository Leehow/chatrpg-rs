//! EV-P3 `progress_observable_leaf_catalog_v1` — the **deterministic authored-
//! observation compiler** (GPT Pro follow-up `design/GPTpro-producer-firing-
//! followup.md` §observable-leaf catalog). Pure, NO IO, NO LLM.
//!
//! It enriches the EV-2 [`EvidenceAtomCatalog`] with **action / state / location /
//! entity** atoms parsed from authored module structure — affordance items,
//! scene-mechanic intents, and gm-notes affordance sentences — so the EV-3 offer
//! frontier has something to offer the GM beyond surfaced clues.
//!
//! Design law honored:
//! - **source-grounded** — every emitted atom carries ≥1 [`SourceRef`] (the authored
//!   span the verb came from); an atom with no source is dropped.
//! - **fail-closed / anti-tautology** — no verb in the lexicon ⇒ no atom; no target
//!   binding that resolves to a REAL module ref ⇒ no atom; a phrase that asserts an
//!   objective conclusion ("the player completed the objective") ⇒ rejected. We
//!   NEVER fabricate an atom the authored text does not support.
//! - **NO ruleset/module name branching** — we parse structure + a closed verb
//!   lexicon, never `if module=="the_vault"`. The only module-name literals are in
//!   `#[cfg(test)]`.
//! - **read_aloud is NEVER parsed for actions** — boxed narration is flavour, not an
//!   affordance; deriving actions from it would fabricate observability.
//!
//! `progress_role` distinguishes a **GuardLeaf** (an authored objective success
//! leaf — the thing a guard checks) from a **CarrierOnly** atom (an affordance /
//! mechanic the player MAY do but which is not itself a scored objective).

use crate::adventure_ir::{AtomId, EvidenceAtomSpec, EvidenceKind, ObjectiveSpec, PredicateExpr};
use crate::{ModuleGraph, SourceRef};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Whether an atom is an authored objective **success leaf** (a guard checks it) or
/// a mere **carrier** affordance/mechanic the player MAY do. CarrierOnly is the
/// default and serializes away (`skip_serializing_if`) so EV-2 atoms stay
/// byte-identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressRole {
    GuardLeaf,
    CarrierOnly,
}

impl Default for ProgressRole {
    fn default() -> Self {
        ProgressRole::CarrierOnly
    }
}

impl ProgressRole {
    pub fn is_carrier_only(&self) -> bool {
        matches!(self, ProgressRole::CarrierOnly)
    }
}

// ───────────────────────── Closed verb lexicon ─────────────────────────

/// Closed verb token → [`EvidenceKind`] table. First matching verb in a phrase
/// wins. Exact (lowercased) token match — never fuzzy.
const VERB_LEXICON: &[(&str, EvidenceKind)] = &[
    // ActionResolved
    ("investigate", EvidenceKind::ActionResolved),
    ("examine", EvidenceKind::ActionResolved),
    ("inspect", EvidenceKind::ActionResolved),
    ("search", EvidenceKind::ActionResolved),
    ("study", EvidenceKind::ActionResolved),
    ("conduct", EvidenceKind::ActionResolved),
    ("experiment", EvidenceKind::ActionResolved),
    ("test", EvidenceKind::ActionResolved),
    ("sample", EvidenceKind::ActionResolved),
    ("question", EvidenceKind::ActionResolved),
    ("interrogate", EvidenceKind::ActionResolved),
    ("ask", EvidenceKind::ActionResolved),
    ("hack", EvidenceKind::ActionResolved),
    ("use", EvidenceKind::ActionResolved),
    ("follow", EvidenceKind::ActionResolved),
    ("observe", EvidenceKind::ActionResolved),
    ("trace", EvidenceKind::ActionResolved),
    ("case", EvidenceKind::ActionResolved),
    ("scout", EvidenceKind::ActionResolved),
    // LocationEntered
    ("visit", EvidenceKind::LocationEntered),
    ("enter", EvidenceKind::LocationEntered),
    ("go", EvidenceKind::LocationEntered),
    ("travel", EvidenceKind::LocationEntered),
    ("reach", EvidenceKind::LocationEntered),
    ("approach", EvidenceKind::LocationEntered),
    ("infiltrate", EvidenceKind::LocationEntered),
    // FactLearned
    ("learn", EvidenceKind::FactLearned),
    ("discover", EvidenceKind::FactLearned),
    ("find", EvidenceKind::FactLearned),
    ("uncover", EvidenceKind::FactLearned),
    ("reveal", EvidenceKind::FactLearned),
    ("read", EvidenceKind::FactLearned),
    // StateEstablished
    ("unlock", EvidenceKind::StateEstablished),
    ("open", EvidenceKind::StateEstablished),
    ("breach", EvidenceKind::StateEstablished),
    ("restore", EvidenceKind::StateEstablished),
    ("disable", EvidenceKind::StateEstablished),
    ("set", EvidenceKind::StateEstablished),
    ("capture", EvidenceKind::StateEstablished),
    ("neutralize", EvidenceKind::StateEstablished),
    ("destroy", EvidenceKind::StateEstablished),
    // EntityEncountered
    ("meet", EvidenceKind::EntityEncountered),
    ("encounter", EvidenceKind::EntityEncountered),
    ("confront", EvidenceKind::EntityEncountered),
];

/// Classify a single (already-lowercased) verb token. Exact match against the
/// closed lexicon; `None` if it is not a known affordance verb (fail-closed).
pub fn classify_verb(token: &str) -> Option<EvidenceKind> {
    let t = token.trim().to_ascii_lowercase();
    VERB_LEXICON
        .iter()
        .find(|(v, _)| *v == t)
        .map(|(_, k)| *k)
}

/// Short grounding prefix per kind (used to build `grounding = "<prefix>:<ref>"`).
fn kind_prefix(kind: EvidenceKind) -> &'static str {
    match kind {
        EvidenceKind::ActionResolved => "action",
        EvidenceKind::StateEstablished => "state",
        EvidenceKind::LocationEntered => "location",
        EvidenceKind::EntityEncountered => "entity",
        EvidenceKind::FactLearned | EvidenceKind::FactRevealed => "fact",
        EvidenceKind::ChoiceCommitted => "choice",
        EvidenceKind::RelationshipChanged => "relationship",
        EvidenceKind::ResourceChanged => "resource",
    }
}

// ───────────────────────── Anti-tautology reject set ─────────────────────────

/// Substrings (lowercased) that mark a phrase as asserting an objective
/// CONCLUSION rather than an observable authored action. Such phrases are
/// rejected — there is no `ObjectiveSatisfied` evidence kind and we never mint a
/// tautology atom ("did the required thing" proves the required thing).
const TAUTOLOGY_MARKERS: &[&str] = &[
    "objective_completed",
    "mission_successful",
    "mission complete",
    "complete the objective",
    "player_did_the_required_thing",
    "objective_satisfied",
    "objectivesatisfied",
    "win the mission",
    "succeed at the mission",
    "accomplish the mission",
];

/// Whether a phrase asserts an objective conclusion (anti-tautology reject).
pub fn is_tautology(phrase: &str) -> bool {
    let p = phrase.to_ascii_lowercase();
    TAUTOLOGY_MARKERS.iter().any(|m| p.contains(m))
}

// ───────────────────────── Graph ref index ─────────────────────────

/// A lowercased lookup of every real module reference (id + name/title) across
/// npcs / locations / clues / factions / scene titles. `resolve` returns the
/// canonical ref (prefer the id) for a token that exactly matches an id OR is a
/// name fragment — mirroring the documented `AnyFactMatches` substring scheme.
pub struct GraphRefIndex {
    /// (lowercased id, canonical id) — exact-id match.
    ids: Vec<(String, String)>,
    /// (lowercased name/title, canonical ref) — case-insensitive contains match.
    names: Vec<(String, String)>,
}

impl GraphRefIndex {
    pub fn build(graph: &ModuleGraph) -> Self {
        let mut ids = Vec::new();
        let mut names = Vec::new();
        let mut push_value = |v: &serde_json::Value| {
            let id = v
                .get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty());
            if let Some(id) = id {
                ids.push((id.to_ascii_lowercase(), id.to_string()));
                let name = v
                    .get("name")
                    .or_else(|| v.get("title"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                if let Some(name) = name {
                    names.push((name.to_ascii_lowercase(), id.to_string()));
                }
            }
        };
        for v in &graph.npcs {
            push_value(v);
        }
        for v in &graph.locations {
            push_value(v);
        }
        for v in &graph.clues {
            push_value(v);
        }
        for v in &graph.factions {
            push_value(v);
        }
        for scene in &graph.scenes {
            let t = scene.title.trim();
            if !t.is_empty() {
                names.push((t.to_ascii_lowercase(), scene.node_id.clone()));
            }
        }
        GraphRefIndex { ids, names }
    }

    /// Resolve a token to a canonical module ref. Exact id match wins; otherwise a
    /// name whose lowercased form is contained in (or contains) the token resolves.
    pub fn resolve(&self, token: &str) -> Option<String> {
        let t = token.trim().to_ascii_lowercase();
        if t.is_empty() {
            return None;
        }
        if let Some((_, canon)) = self.ids.iter().find(|(lid, _)| *lid == t) {
            return Some(canon.clone());
        }
        // name fragment: the authored token IS a name fragment, or vice-versa.
        self.names
            .iter()
            .find(|(name, _)| t.contains(name.as_str()) || name.contains(t.as_str()))
            .map(|(_, canon)| canon.clone())
    }
}

// ───────────────────────── Phrase parser ─────────────────────────

fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    format!("{:x}", h.finalize())
}

/// Tokenize on non-alphanumeric, lowercased.
fn tokenize(phrase: &str) -> Vec<String> {
    phrase
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
        .collect()
}

/// Parse one authored action phrase into a source-grounded [`EvidenceAtomSpec`], or
/// `None` (fail-closed). See module docs for the rejection rules.
///
/// `candidate_bindings` are author-supplied target hints (e.g. `implies_vectors`);
/// they are tried alongside the phrase's own noun tokens, but ONLY refs that
/// resolve via `graph_index` to a real module reference are kept.
#[allow(clippy::too_many_arguments)]
pub fn parse_action_phrase(
    phrase: &str,
    source: SourceRef,
    candidate_bindings: &[String],
    graph_index: &GraphRefIndex,
    role: ProgressRole,
    module_digest: &str,
) -> Option<EvidenceAtomSpec> {
    if is_tautology(phrase) {
        return None; // anti-tautology
    }
    let tokens = tokenize(phrase);
    // First token with a lexicon verb wins.
    let kind = tokens.iter().find_map(|t| classify_verb(t))?;

    // Resolve target bindings from candidate hints + phrase nouns.
    let mut resolved: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut try_resolve = |raw: &str, resolved: &mut Vec<String>, seen: &mut std::collections::HashSet<String>| {
        if let Some(canon) = graph_index.resolve(raw) {
            if seen.insert(canon.clone()) {
                resolved.push(canon);
            }
        }
    };
    for cand in candidate_bindings {
        try_resolve(cand, &mut resolved, &mut seen);
    }
    for tok in &tokens {
        try_resolve(tok, &mut resolved, &mut seen);
    }
    // Also try multi-word name fragments straight from the phrase (e.g. "Aquifer
    // commercial" where only the lowercased name "aquifer" is a known ref).
    if resolved.is_empty() {
        try_resolve(phrase, &mut resolved, &mut seen);
    }

    if resolved.is_empty() {
        return None; // fail-closed: no real target ref → no atom
    }

    let first = resolved[0].clone();
    let grounding = format!("{}:{}", kind_prefix(kind), first);

    let source_span = source
        .anchor_id
        .clone()
        .or_else(|| source.note.clone())
        .or_else(|| source.page.map(|p| format!("p{p}")))
        .unwrap_or_else(|| sha256_hex(phrase));

    if source.source_id.trim().is_empty() {
        return None; // source-grounded: must carry a real source
    }

    let atom_id = AtomId::from_parts(module_digest, &source_span, kind, &resolved);
    Some(EvidenceAtomSpec {
        atom_id,
        kind,
        bindings: resolved,
        source_refs: vec![source],
        grounding,
        progress_role: role,
    })
}

/// The `scene:` binding prefix that [`tag_scene`] uses to mark an atom's owning
/// scene. Exposed so scene-scoped consumers (D2 scene-advance objectives) filter the
/// catalog by the SAME prefix instead of re-hardcoding the literal.
pub(crate) fn tag_scene_prefix() -> &'static str {
    "scene:"
}

/// Tag the atom with its owning scene (`scene:<node_id>` binding) so the offer
/// frontier can intersect by current scene. Idempotent.
pub(crate) fn tag_scene(mut atom: EvidenceAtomSpec, scene_id: &str) -> EvidenceAtomSpec {
    let tag = format!("{}{scene_id}", tag_scene_prefix());
    if !atom.bindings.iter().any(|b| b == &tag) {
        atom.bindings.push(tag);
    }
    atom
}

// ───────────────────────── Source-specific derivers ─────────────────────────

/// The entry scene's node_id (lowest `page_start`, ties broken by first listed).
pub(crate) fn entry_scene_id(graph: &ModuleGraph) -> Option<String> {
    graph
        .scenes
        .iter()
        .filter(|s| !s.node_id.trim().is_empty())
        .min_by_key(|s| s.page_start.unwrap_or(u32::MAX))
        .map(|s| s.node_id.clone())
}

/// Compile authored **objective success leaves** into GuardLeaf atoms. Only an
/// objective whose `success_when` is an [`PredicateExpr::OpaqueAuthoredText`] is
/// parsed (a normalized predicate is already machine-evaluable and needs no leaf
/// atom). Live the_vault has no objectives field → the live caller passes `&[]`;
/// the unit + induct-from-source paths exercise this.
pub fn compile_objective_leaves(
    objectives: &[ObjectiveSpec],
    index: &GraphRefIndex,
    module_digest: &str,
) -> Vec<EvidenceAtomSpec> {
    let mut out = Vec::new();
    for obj in objectives {
        let PredicateExpr::OpaqueAuthoredText { raw_text } = &obj.success_when else {
            continue; // only un-normalized authored leaves need an atom
        };
        let source = obj
            .source_evidence
            .first()
            .cloned()
            .unwrap_or_else(|| SourceRef {
                source_id: module_digest.to_string(),
                note: Some(raw_text.clone()),
                ..Default::default()
            });
        if let Some(atom) =
            parse_action_phrase(raw_text, source, &[], index, ProgressRole::GuardLeaf, module_digest)
        {
            out.push(atom);
        }
    }
    out
}

/// Compile `director_facilitation.affordance_items` into CarrierOnly action atoms.
fn from_affordance_items(graph: &ModuleGraph, index: &GraphRefIndex) -> Vec<EvidenceAtomSpec> {
    let Some(cfg) = &graph.director_facilitation else {
        return Vec::new();
    };
    let module_id = graph.module_id.as_str();
    let scene_tag = entry_scene_id(graph);
    let mut out = Vec::new();
    for item in &cfg.affordance_items {
        let source = SourceRef {
            source_id: module_id.to_string(),
            anchor_id: Some("director_facilitation.affordance_items".to_string()),
            note: Some(item.description.clone()),
            ..Default::default()
        };
        if let Some(atom) = parse_action_phrase(
            &item.description,
            source,
            &item.implies_vectors,
            index,
            ProgressRole::CarrierOnly,
            module_id,
        ) {
            let atom = match &scene_tag {
                Some(s) => tag_scene(atom, s),
                None => atom,
            };
            out.push(atom);
        }
    }
    out
}

/// Compile scene-mechanic intents into CarrierOnly ACTION atoms (the STATE side is
/// already projected by EV-P2 from the effect leaves — we only emit the action).
fn from_scene_mechanics(graph: &ModuleGraph, index: &GraphRefIndex) -> Vec<EvidenceAtomSpec> {
    let module_id = graph.module_id.as_str();
    let mut out = Vec::new();
    for scene in &graph.scenes {
        for intent in &scene.scene_mechanics {
            let mut candidates: Vec<String> = Vec::new();
            if !intent.tested_parameter.trim().is_empty() {
                candidates.push(intent.tested_parameter.clone());
            }
            candidates.extend(scene.referenced_npc_ids.iter().cloned());
            candidates.extend(scene.referenced_location_ids.iter().cloned());
            candidates.extend(scene.referenced_clue_ids.iter().cloned());
            let anchor = intent.source_anchor.trim();
            let note_anchor: String = anchor.chars().take(120).collect();
            let source = SourceRef {
                source_id: module_id.to_string(),
                note: Some(if note_anchor.is_empty() {
                    intent.description.clone()
                } else {
                    note_anchor
                }),
                ..Default::default()
            };
            if let Some(atom) = parse_action_phrase(
                &intent.description,
                source,
                &candidates,
                index,
                ProgressRole::CarrierOnly,
                module_id,
            ) {
                out.push(tag_scene(atom, &scene.node_id));
            }
        }
    }
    out
}

/// Affordance markers that flag a gm_notes sentence as an explicit interaction
/// handle (not pure narration). Case-insensitive substring.
const GM_NOTE_AFFORDANCE_MARKERS: &[&str] = &[
    "if ",
    "when ",
    "after ",
    "by searching",
    "by questioning",
    "can be accessed",
    "sampling",
    "reveals",
    "contains evidence",
];

/// Compile gm_notes affordance sentences into CarrierOnly atoms. read_aloud is
/// NEVER touched (only `gm_notes`). Conservative: a sentence with no marker, no
/// lexicon verb, or no resolvable binding is skipped.
fn from_gm_notes_affordances(graph: &ModuleGraph, index: &GraphRefIndex) -> Vec<EvidenceAtomSpec> {
    let module_id = graph.module_id.as_str();
    let mut out = Vec::new();
    for scene in &graph.scenes {
        let Some(notes) = &scene.gm_notes else {
            continue;
        };
        let mut candidates: Vec<String> = Vec::new();
        candidates.extend(scene.referenced_npc_ids.iter().cloned());
        candidates.extend(scene.referenced_location_ids.iter().cloned());
        candidates.extend(scene.referenced_clue_ids.iter().cloned());

        let mut cursor = 0usize; // byte offset into notes for char_start/end
        for sentence in notes.split(|c| c == '.' || c == ';') {
            let start = cursor;
            cursor += sentence.len() + 1; // +1 for the delimiter
            let trimmed = sentence.trim();
            if trimmed.is_empty() {
                continue;
            }
            let lower = trimmed.to_ascii_lowercase();
            if !GM_NOTE_AFFORDANCE_MARKERS.iter().any(|m| lower.contains(m)) {
                continue; // not an explicit affordance sentence
            }
            let source = SourceRef {
                source_id: module_id.to_string(),
                page: scene.page_start,
                char_start: Some(start),
                char_end: Some(start + sentence.len()),
                text_hash: Some(sha256_hex(trimmed)),
                note: Some(trimmed.to_string()),
                ..Default::default()
            };
            if let Some(atom) = parse_action_phrase(
                trimmed,
                source,
                &candidates,
                index,
                ProgressRole::CarrierOnly,
                module_id,
            ) {
                out.push(tag_scene(atom, &scene.node_id));
            }
        }
    }
    out
}

/// Compile the full set of authored-observation atoms for a module (excluding
/// objective leaves — the live graph has none; the caller adds GuardLeaf atoms via
/// [`compile_objective_leaves`] when objectives are available). Deduped by
/// `grounding` (first wins). Final validator pass drops source-less / tautology /
/// mis-roled atoms (defensive).
pub fn compile_authored_observations(graph: &ModuleGraph) -> Vec<EvidenceAtomSpec> {
    let index = GraphRefIndex::build(graph);
    let mut atoms = Vec::new();
    atoms.extend(from_affordance_items(graph, &index));
    atoms.extend(from_scene_mechanics(graph, &index));
    atoms.extend(from_gm_notes_affordances(graph, &index));
    // F1 (default OFF ⇒ byte-identical baseline): scene-salient NPC-knowledge atoms
    // for talk/combat scenes that carry no affordance/mechanic/gm-note (see
    // `super::npc_knowledge_observation`). Single gated extend; no other insertion.
    if super::npc_knowledge_observation::scene_npc_knowledge_enabled() {
        atoms.extend(super::npc_knowledge_observation::from_referenced_npc_knowledge(
            graph,
        ));
    }

    // dedup by grounding (keep first) + validator pass.
    let mut seen = std::collections::HashSet::new();
    atoms
        .into_iter()
        .filter(|a| {
            if a.source_refs.is_empty() {
                return false; // source-grounded invariant
            }
            if a.source_refs
                .iter()
                .any(|s| s.note.as_deref().map(is_tautology).unwrap_or(false))
            {
                return false; // defensive anti-tautology
            }
            // GuardLeaf may only appear on objective-leaf-sourced atoms; this
            // compiler never emits GuardLeaf (objective leaves are caller-added).
            if a.progress_role == ProgressRole::GuardLeaf {
                return false;
            }
            seen.insert(a.grounding.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ScenarioNode;
    use serde_json::json;

    fn idx(graph: &ModuleGraph) -> GraphRefIndex {
        GraphRefIndex::build(graph)
    }

    fn vault_with_npc(npc_id: &str, npc_name: &str) -> ModuleGraph {
        ModuleGraph {
            module_id: "the_vault".into(),
            npcs: vec![json!({"id": npc_id, "name": npc_name})],
            ..Default::default()
        }
    }

    #[test]
    fn objective_leaf_yields_one_guard_leaf_action_atom() {
        // index where "anomaly" resolves (as a location name).
        let graph = ModuleGraph {
            module_id: "the_vault".into(),
            locations: vec![json!({"id": "loc_anomaly", "name": "Anomaly"})],
            ..Default::default()
        };
        let index = idx(&graph);
        let obj = ObjectiveSpec {
            id: "obj.experiment".into(),
            mission_id: None,
            mandatory: true,
            success_when: PredicateExpr::OpaqueAuthoredText {
                raw_text: "conduct an experiment on the anomaly".into(),
            },
            failure_when: None,
            score_effects: vec![],
            rewards: vec![],
            deadline: None,
            source_evidence: vec![SourceRef {
                source_id: "the_vault".into(),
                page: Some(12),
                ..Default::default()
            }],
        };
        let atoms = compile_objective_leaves(&[obj], &index, "digest_v1");
        assert_eq!(atoms.len(), 1, "exactly one guard-leaf atom");
        let a = &atoms[0];
        assert_eq!(a.kind, EvidenceKind::ActionResolved);
        assert!(a.grounding.starts_with("action:"), "action grounding prefix");
        assert!(a.bindings.iter().any(|b| b == "loc_anomaly"), "binds the anomaly ref");
        assert!(!a.source_refs.is_empty(), "source-grounded");
        assert_eq!(a.progress_role, ProgressRole::GuardLeaf, "objective leaf = GuardLeaf");
    }

    #[test]
    fn affordance_with_implies_vector_yields_carrier_action_atom() {
        let graph = ModuleGraph {
            module_id: "the_vault".into(),
            clues: vec![json!({"id": "clue_aquifer_commercial", "name": "Aquifer"})],
            ..Default::default()
        };
        let index = idx(&graph);
        let source = SourceRef {
            source_id: "the_vault".into(),
            anchor_id: Some("director_facilitation.affordance_items".into()),
            note: Some("Investigate the Aquifer commercial.".into()),
            ..Default::default()
        };
        let atom = parse_action_phrase(
            "Investigate the Aquifer commercial.",
            source,
            &["Aquifer".to_string()],
            &index,
            ProgressRole::CarrierOnly,
            "the_vault",
        )
        .expect("affordance with a resolvable target yields an atom");
        assert_eq!(atom.kind, EvidenceKind::ActionResolved);
        assert_eq!(atom.progress_role, ProgressRole::CarrierOnly);
        assert_eq!(
            atom.source_refs[0].note.as_deref(),
            Some("Investigate the Aquifer commercial."),
            "source note = the authored description"
        );
    }

    #[test]
    fn no_verb_phrase_yields_no_atom() {
        let graph = vault_with_npc("npc_x", "Marguerite");
        let index = idx(&graph);
        let out = parse_action_phrase(
            "The room is dark and quiet",
            SourceRef { source_id: "the_vault".into(), ..Default::default() },
            &[],
            &index,
            ProgressRole::CarrierOnly,
            "the_vault",
        );
        assert!(out.is_none(), "fail-closed: no lexicon verb → no atom");
    }

    #[test]
    fn unresolved_binding_yields_no_atom() {
        let graph = vault_with_npc("npc_x", "Marguerite");
        let index = idx(&graph);
        // verb present ("investigate") but the only target token resolves to nothing.
        let out = parse_action_phrase(
            "investigate the nonexistent_widget_9000",
            SourceRef { source_id: "the_vault".into(), ..Default::default() },
            &[],
            &index,
            ProgressRole::CarrierOnly,
            "the_vault",
        );
        assert!(out.is_none(), "fail-closed: no resolvable target → no fabricated atom");
    }

    #[test]
    fn tautology_phrase_rejected() {
        let graph = ModuleGraph {
            module_id: "the_vault".into(),
            locations: vec![json!({"id": "loc_x", "name": "objective"})],
            ..Default::default()
        };
        let index = idx(&graph);
        for phrase in [
            "the player completed the objective",
            "mission_successful at last",
            "complete the objective now",
        ] {
            let out = parse_action_phrase(
                phrase,
                SourceRef { source_id: "the_vault".into(), ..Default::default() },
                &[],
                &index,
                ProgressRole::CarrierOnly,
                "the_vault",
            );
            assert!(out.is_none(), "tautology rejected: {phrase:?}");
        }
    }

    #[test]
    fn read_aloud_noun_string_never_yields_action_atom() {
        // A read_aloud-style noun dump placed in read_aloud (NOT gm_notes). The
        // compiler must never derive an action from it.
        let mut scene = ScenarioNode {
            node_id: "scene_001".into(),
            node_type: "scene".into(),
            title: "Springs Eternal".into(),
            ..Default::default()
        };
        scene.read_aloud = Some("water, rivers, city streets, vines".into());
        scene.gm_notes = None;
        let graph = ModuleGraph {
            module_id: "the_vault".into(),
            scenes: vec![scene],
            ..Default::default()
        };
        let atoms = compile_authored_observations(&graph);
        assert!(
            atoms.is_empty(),
            "read_aloud is never parsed for actions (no gm_notes/affordance path touches it)"
        );
    }

    #[test]
    fn affordance_atom_is_carrier_only_objective_atom_is_guard_leaf() {
        let graph = ModuleGraph {
            module_id: "the_vault".into(),
            clues: vec![json!({"id": "clue_aquifer_commercial", "name": "Aquifer"})],
            director_facilitation: Some(crate::DirectorModuleConfig {
                affordance_items: vec![crate::DirectorAffordanceItem {
                    description: "Investigate the Aquifer commercial.".into(),
                    implies_vectors: vec!["Aquifer".into()],
                }],
                ..Default::default()
            }),
            scenes: vec![ScenarioNode {
                node_id: "scene_001".into(),
                node_type: "scene".into(),
                title: "Springs Eternal".into(),
                page_start: Some(8),
                ..Default::default()
            }],
            ..Default::default()
        };
        let atoms = compile_authored_observations(&graph);
        assert!(!atoms.is_empty(), "affordance yields an atom");
        assert!(
            atoms.iter().all(|a| a.progress_role == ProgressRole::CarrierOnly),
            "compiler emits CarrierOnly only"
        );
        // scene tag present
        assert!(
            atoms[0].bindings.iter().any(|b| b == "scene:scene_001"),
            "affordance atom tagged with entry scene"
        );
    }

    #[test]
    fn carrier_only_atom_serializes_without_progress_role_key() {
        let atom = EvidenceAtomSpec {
            atom_id: AtomId::from_parts("d", "p8", EvidenceKind::FactLearned, &["c".into()]),
            kind: EvidenceKind::FactLearned,
            bindings: vec!["c".into()],
            source_refs: vec![SourceRef { source_id: "the_vault".into(), ..Default::default() }],
            grounding: "fact:c".into(),
            progress_role: ProgressRole::CarrierOnly,
        };
        let v = serde_json::to_value(&atom).unwrap();
        assert!(
            v.get("progress_role").is_none(),
            "CarrierOnly (default) skips the key → EV-2 atom bytes unchanged"
        );
    }

    #[test]
    fn guard_leaf_atom_serializes_with_progress_role_key() {
        let atom = EvidenceAtomSpec {
            atom_id: AtomId::from_parts("d", "p8", EvidenceKind::ActionResolved, &["c".into()]),
            kind: EvidenceKind::ActionResolved,
            bindings: vec!["c".into()],
            source_refs: vec![SourceRef { source_id: "the_vault".into(), ..Default::default() }],
            grounding: "action:c".into(),
            progress_role: ProgressRole::GuardLeaf,
        };
        let v = serde_json::to_value(&atom).unwrap();
        assert_eq!(
            v.get("progress_role").and_then(|x| x.as_str()),
            Some("guard_leaf"),
            "GuardLeaf serializes the role"
        );
    }

    #[test]
    fn action_atom_id_is_deterministic() {
        let graph = ModuleGraph {
            module_id: "the_vault".into(),
            clues: vec![json!({"id": "clue_aquifer_commercial", "name": "Aquifer"})],
            ..Default::default()
        };
        let index = idx(&graph);
        let make = || {
            parse_action_phrase(
                "Investigate the Aquifer commercial.",
                SourceRef {
                    source_id: "the_vault".into(),
                    anchor_id: Some("director_facilitation.affordance_items".into()),
                    ..Default::default()
                },
                &["Aquifer".to_string()],
                &index,
                ProgressRole::CarrierOnly,
                "the_vault",
            )
            .unwrap()
        };
        assert_eq!(make().atom_id, make().atom_id, "same parts ⇒ same AtomId");
    }
}

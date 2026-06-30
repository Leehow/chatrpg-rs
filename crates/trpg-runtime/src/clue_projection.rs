//! CL-1 — Clue → scene projection (feed progression by surfacing module clues).
//!
//! Root cause (P2 Vault J3, verified): `the_vault` scenes carry
//! `referenced_clue_ids = []` even though `module_graph.clues` HAS the clues (each
//! `{ id, name, page, content_class }`) and scenes carry `page_start` / `page_end`.
//! `clue_affordance` therefore iterates an empty list → 0 `PlayerLearnedFact` →
//! the progression adapter feeds the engine nothing → the spine objective never fires.
//!
//! This module projects each clue onto the scene whose `[page_start, page_end]`
//! CONTAINS the clue's `page`, appending the clue `id` to that scene's
//! `referenced_clue_ids` (so `clue_affordance` has clues to reveal). The projection
//! is **source-grounded, deterministic, structure-driven, and fail-closed**.
//!
//! # Fail-closed (NEVER fabricate a clue↔scene link)
//! A clue is SKIPPED (and counted) when:
//!   - it has no usable `page`, or
//!   - its page falls outside EVERY scene's `[page_start, page_end]` range, or
//!   - its page falls inside MORE THAN ONE scene's range (ambiguous overlap).
//! Never guess, never invent a link.
//!
//! # Zero ruleset / module name-branching
//! The projection is generic over any module whose clues have pages and whose scenes
//! have page ranges. `the_vault` is not special-cased.
//!
//! # Additive / OFF == byte-identical baseline
//! The pure function below mutates an in-memory `ModuleGraph`; it is invoked ONLY
//! behind [`clue_projection_enabled`] (env `TRPG_CLUE_PROJECTION`, default OFF). When
//! OFF the projection never runs, `referenced_clue_ids` stays empty, and the loaded
//! graph is byte-identical to today.

use serde_json::Value;
use trpg_model::ModuleGraph;

/// Environment flag gating the clue→scene projection. Default OFF (byte-baseline).
pub const CLUE_PROJECTION_ENV: &str = "TRPG_CLUE_PROJECTION";

/// Whether clue→scene projection is enabled. Default OFF; only `1`/`true`/`on`
/// (case-insensitive) turn it ON. Mirrors the reveal-gating flag discipline.
pub fn clue_projection_enabled() -> bool {
    std::env::var(CLUE_PROJECTION_ENV)
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            v == "1" || v == "true" || v == "on"
        })
        .unwrap_or(false)
}

/// Outcome of a projection pass — counts of what was linked vs fail-closed skipped.
/// Surfaced so callers/tests can prove nothing was silently dropped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClueProjectionReport {
    /// Clues successfully linked to exactly one containing scene.
    pub linked: usize,
    /// Skipped: clue has no usable numeric `page`.
    pub skipped_no_page: usize,
    /// Skipped: clue page is outside every scene's `[page_start, page_end]`.
    pub skipped_out_of_range: usize,
    /// Skipped: clue page falls inside more than one scene range (ambiguous).
    pub skipped_ambiguous: usize,
}

/// Read a clue Value's `page` as a `u32` (fail-closed: missing / non-numeric → None).
fn clue_page(clue: &Value) -> Option<u32> {
    clue.get("page")
        .and_then(Value::as_u64)
        .and_then(|p| u32::try_from(p).ok())
}

/// Read a clue Value's `id` (non-empty, trimmed).
fn clue_id(clue: &Value) -> Option<String> {
    clue.get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Project each clue in `graph.clues` onto the scene whose `[page_start, page_end]`
/// contains the clue's `page`, appending the clue id to that scene's
/// `referenced_clue_ids` (deduped). Pure, deterministic, fail-closed. Returns a
/// [`ClueProjectionReport`] of what was linked vs skipped.
pub fn project_clues_onto_scenes(graph: &mut ModuleGraph) -> ClueProjectionReport {
    let mut report = ClueProjectionReport::default();
    // Snapshot each scene's full `[page_start, page_end]` range (both bounds present),
    // paired with its index. Scenes without a full range can never contain a clue.
    let ranges: Vec<(usize, u32, u32)> = graph
        .scenes
        .iter()
        .enumerate()
        .filter_map(|(i, s)| match (s.page_start, s.page_end) {
            (Some(start), Some(end)) if start <= end => Some((i, start, end)),
            _ => None,
        })
        .collect();

    for clue in &graph.clues {
        let Some(id) = clue_id(clue) else {
            // A clue with no id cannot be referenced; fail-closed skip (not counted as
            // a page failure — it is structurally unlinkable, mirrors clue_affordance).
            continue;
        };
        let Some(page) = clue_page(clue) else {
            report.skipped_no_page += 1;
            continue;
        };
        // Find every scene whose range contains the page (inclusive).
        let mut containing = ranges
            .iter()
            .filter(|(_, start, end)| *start <= page && page <= *end)
            .map(|(i, _, _)| *i);
        let Some(first) = containing.next() else {
            report.skipped_out_of_range += 1; // outside every scene range
            continue;
        };
        if containing.next().is_some() {
            report.skipped_ambiguous += 1; // page falls in >1 scene → never guess
            continue;
        }
        // Exactly one containing scene → append (deduped, order-preserving).
        let refs = &mut graph.scenes[first].referenced_clue_ids;
        if !refs.iter().any(|e| e == &id) {
            refs.push(id);
        }
        report.linked += 1;
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use trpg_model::ScenarioNode;

    fn scene(id: &str, start: u32, end: u32) -> ScenarioNode {
        ScenarioNode {
            node_id: id.to_string(),
            page_start: Some(start),
            page_end: Some(end),
            ..Default::default()
        }
    }

    fn graph(scenes: Vec<ScenarioNode>, clues: Vec<Value>) -> ModuleGraph {
        ModuleGraph {
            scenes,
            clues,
            ..Default::default()
        }
    }

    // ===== TDD #1: page-in-range clue links to its containing scene =====
    #[test]
    fn clue_in_range_links_to_containing_scene() {
        let mut g = graph(
            vec![scene("scene_001", 8, 21), scene("scene_002", 22, 38)],
            vec![
                json!({"id":"clue_a","name":"A","page":10}),
                json!({"id":"clue_b","name":"B","page":30}),
            ],
        );
        let report = project_clues_onto_scenes(&mut g);
        assert_eq!(
            report.linked, 2,
            "both clues land in exactly one scene: {report:?}"
        );
        assert_eq!(g.scenes[0].referenced_clue_ids, vec!["clue_a"]);
        assert_eq!(g.scenes[1].referenced_clue_ids, vec!["clue_b"]);
    }

    // ===== TDD #2: boundary pages (start/end inclusive) link =====
    #[test]
    fn boundary_pages_are_inclusive() {
        let mut g = graph(
            vec![scene("s", 8, 21)],
            vec![json!({"id":"low","page":8}), json!({"id":"high","page":21})],
        );
        let report = project_clues_onto_scenes(&mut g);
        assert_eq!(report.linked, 2);
        assert_eq!(g.scenes[0].referenced_clue_ids, vec!["low", "high"]);
    }

    // ===== TDD #3: out-of-range clue is skipped + counted (fail-closed) =====
    #[test]
    fn out_of_range_clue_is_skipped_and_counted() {
        let mut g = graph(
            vec![scene("s", 8, 21)],
            vec![json!({"id":"orphan","page":99})],
        );
        let report = project_clues_onto_scenes(&mut g);
        assert_eq!(report.linked, 0);
        assert_eq!(report.skipped_out_of_range, 1, "{report:?}");
        assert!(
            g.scenes[0].referenced_clue_ids.is_empty(),
            "never fabricate a link"
        );
    }

    // ===== TDD #4: clue with no page is skipped + counted (fail-closed) =====
    #[test]
    fn clue_without_page_is_skipped_and_counted() {
        let mut g = graph(
            vec![scene("s", 8, 21)],
            vec![
                json!({"id":"nopage","name":"no page field"}),
                json!({"id":"strpage","page":"twelve"}),
            ],
        );
        let report = project_clues_onto_scenes(&mut g);
        assert_eq!(report.linked, 0);
        assert_eq!(report.skipped_no_page, 2, "{report:?}");
        assert!(g.scenes[0].referenced_clue_ids.is_empty());
    }

    // ===== TDD #5: ambiguous (overlapping ranges) clue is skipped + counted =====
    #[test]
    fn ambiguous_overlapping_ranges_skip_the_clue() {
        // two scenes whose ranges both contain page 15 → ambiguous → never guess.
        let mut g = graph(
            vec![scene("a", 8, 21), scene("b", 15, 30)],
            vec![json!({"id":"amb","page":15})],
        );
        let report = project_clues_onto_scenes(&mut g);
        assert_eq!(report.linked, 0);
        assert_eq!(report.skipped_ambiguous, 1, "{report:?}");
        assert!(g.scenes[0].referenced_clue_ids.is_empty());
        assert!(g.scenes[1].referenced_clue_ids.is_empty());
    }

    // ===== TDD #6: dedup — projecting twice does not double-append =====
    #[test]
    fn projection_is_idempotent_no_double_append() {
        let mut g = graph(
            vec![scene("s", 8, 21)],
            vec![json!({"id":"clue_a","page":10})],
        );
        project_clues_onto_scenes(&mut g);
        let report = project_clues_onto_scenes(&mut g);
        assert_eq!(
            g.scenes[0].referenced_clue_ids,
            vec!["clue_a"],
            "no duplicate"
        );
        // second pass re-links the same clue (idempotent), still counts it linked.
        assert_eq!(report.linked, 1);
    }

    // ===== TDD #7: clue with no id is skipped (cannot link a nameless clue) =====
    #[test]
    fn clue_without_id_is_skipped() {
        let mut g = graph(
            vec![scene("s", 8, 21)],
            vec![json!({"name":"anon","page":10})],
        );
        let report = project_clues_onto_scenes(&mut g);
        assert_eq!(report.linked, 0);
        assert!(g.scenes[0].referenced_clue_ids.is_empty());
    }

    // ===== TDD #8: scene missing a page bound never matches (fail-closed) =====
    #[test]
    fn scene_without_full_page_range_never_matches() {
        let mut g = graph(
            vec![ScenarioNode {
                node_id: "s".into(),
                page_start: Some(8),
                page_end: None,
                ..Default::default()
            }],
            vec![json!({"id":"c","page":10})],
        );
        let report = project_clues_onto_scenes(&mut g);
        assert_eq!(report.linked, 0);
        assert_eq!(
            report.skipped_out_of_range, 1,
            "no full range ⇒ no containment"
        );
        assert!(g.scenes[0].referenced_clue_ids.is_empty());
    }

    // ===== TDD #9: empty graph / no clues → no-op =====
    #[test]
    fn empty_is_noop() {
        let mut g = graph(vec![], vec![]);
        let report = project_clues_onto_scenes(&mut g);
        assert_eq!(report, ClueProjectionReport::default());
    }

    // ===== TDD #10: flag defaults OFF; only 1/true/on enable =====
    #[test]
    fn flag_defaults_off() {
        // env is process-global; this test only asserts the parse table via a guard.
        let prev = std::env::var(CLUE_PROJECTION_ENV).ok();
        std::env::remove_var(CLUE_PROJECTION_ENV);
        assert!(!clue_projection_enabled(), "unset ⇒ OFF (baseline)");
        for on in ["1", "true", "on", "ON", " true "] {
            std::env::set_var(CLUE_PROJECTION_ENV, on);
            assert!(clue_projection_enabled(), "{on:?} ⇒ ON");
        }
        for off in ["0", "false", "off", "no", "garbage", ""] {
            std::env::set_var(CLUE_PROJECTION_ENV, off);
            assert!(!clue_projection_enabled(), "{off:?} ⇒ OFF");
        }
        match prev {
            Some(v) => std::env::set_var(CLUE_PROJECTION_ENV, v),
            None => std::env::remove_var(CLUE_PROJECTION_ENV),
        }
    }
}

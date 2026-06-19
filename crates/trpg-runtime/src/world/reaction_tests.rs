use super::*;
use trpg_model::{
    NpcBehaviorContext, NpcKnowledgeEntry, NpcMindView, NpcRelationship, NpcRelationshipDelta,
};

fn profile(id: &str) -> NpcProfile {
    NpcProfile {
        actor_id: id.into(),
        name: id.into(),
        ..Default::default()
    }
}

fn rel(id: &str, trust: i16, fear: i16, hostility: i16) -> NpcRelationship {
    let mut r = NpcRelationship::new("s", id, NpcRelationshipTarget::PlayerParty).unwrap();
    r.apply_delta(&NpcRelationshipDelta {
        trust,
        fear,
        hostility,
        evidence_event_ids: vec!["e".into()],
        ..Default::default()
    })
    .unwrap();
    r
}

fn plan(
    id: &str,
    trust: i16,
    fear: i16,
    hostility: i16,
    entries: &[NpcKnowledgeEntry],
) -> NpcBehaviorPlan {
    let view = NpcMindView::build(
        "s",
        id,
        &profile(id),
        &[rel(id, trust, fear, hostility)],
        entries,
    )
    .unwrap();
    NpcBehaviorPlan::derive(&view, &NpcBehaviorContext::default())
}

#[test]
fn assemble_empty_plans_yields_empty_set() {
    let set = assemble_world_reaction_set(&[]);
    assert!(set.is_empty());
}

#[test]
fn assemble_only_reflects_supplied_plans_no_phantom_candidate() {
    // §24-#3 negative pool (pure mirror of the live counter-example): the assembled set
    // contains ONLY the plans handed in. A candidate cannot appear for an NPC that was
    // never loaded into the pool — assembling from an empty plan slice yields no
    // candidate, and assembling from one plan never invents a second.
    assert!(assemble_world_reaction_set(&[]).reactions.is_empty());
    let one = vec![plan("npc_only", 0, 0, 80, &[])];
    let set = assemble_world_reaction_set(&one);
    assert_eq!(set.reactions.len(), 1, "exactly the one supplied NPC");
    assert!(
        !set.reactions.iter().any(|c| c.npc_id == "npc_phantom"),
        "§24-#3: a non-supplied NPC must never leak a candidate"
    );
}

#[test]
fn assemble_preserves_plan_order_and_count() {
    let plans = vec![plan("npc_a", 0, 0, 70, &[]), plan("npc_b", 80, 0, 0, &[])];
    let set = assemble_world_reaction_set(&plans);
    assert_eq!(set.reactions.len(), 2);
    assert_eq!(set.reactions[0].npc_id, "npc_a");
    assert_eq!(set.reactions[1].npc_id, "npc_b");
    // clock/knowledge channels stay empty in a bare projection.
    assert!(set.clock_advances.is_empty());
    assert!(set.knowledge_deltas.is_empty());
}

#[test]
fn assemble_matches_projection_per_plan() {
    // The set's candidates must equal each plan's own to_reaction_candidate output
    // (single source of truth is the P4.3 projection; no re-derivation here).
    let plans = vec![plan("npc_a", 20, 10, 40, &[]), plan("npc_b", 60, 0, 0, &[])];
    let set = assemble_world_reaction_set(&plans);
    for (p, cand) in plans.iter().zip(set.reactions.iter()) {
        assert_eq!(*cand, p.to_reaction_candidate());
    }
}

#[test]
fn assemble_never_widens_knowledge_basis_beyond_revealable() {
    // §24-#4: a withheld secret id must never appear in any candidate's basis.
    let entries = vec![
        NpcKnowledgeEntry {
            fact_id: "shared".into(),
            state: trpg_model::KnowledgeState::KnowsTrue,
        },
        NpcKnowledgeEntry {
            fact_id: "secret".into(),
            state: trpg_model::KnowledgeState::KnowsTrue,
        },
    ];
    // Hostile + a context that marks "secret" as withheld.
    let view = NpcMindView::build(
        "s",
        "npc_a",
        &profile("npc_a"),
        &[rel("npc_a", 0, 0, 80)],
        &entries,
    )
    .unwrap();
    let ctx = NpcBehaviorContext {
        secret_fact_ids: vec!["secret".into()],
        ..Default::default()
    };
    let p = NpcBehaviorPlan::derive(&view, &ctx);
    assert!(p.facts_will_withhold.iter().any(|f| f == "secret"));
    let set = assemble_world_reaction_set(&[p]);
    let basis = &set.reactions[0].knowledge_basis;
    assert!(
        !basis.iter().any(|f| f == "secret"),
        "withheld secret leaked into World basis"
    );
}

// ── Part A: render byte-identity to the legacy per-NPC join ──────────────────────

#[test]
fn render_empty_plans_is_none() {
    // No plans ⇒ no block (dynamic tail writes nothing; non-NPC turns byte-stable).
    assert!(render_world_reaction_block(&[]).is_none());
}

#[test]
fn render_is_byte_identical_to_legacy_join() {
    // The new render MUST equal the legacy `blocks.push(plan.to_guidance_block())` +
    // `blocks.join("\n\n")` path, byte-for-byte. This is the Part A behavior-preserving
    // lock: the GM-context bytes do not change on the default path.
    let plans = vec![
        plan("npc_a", 0, 0, 80, &[]),
        plan("npc_b", 70, 0, 0, &[]),
        plan("npc_c", 10, 60, 10, &[]),
    ];
    let legacy: Vec<String> = plans.iter().map(|p| p.to_guidance_block()).collect();
    let legacy_joined = legacy.join("\n\n");
    let rendered = render_world_reaction_block(&plans).expect("non-empty plans render");
    assert_eq!(
        rendered, legacy_joined,
        "render diverged from legacy join bytes"
    );
}

#[test]
fn render_single_plan_matches_its_block() {
    let plans = vec![plan("npc_solo", 0, 0, 75, &[])];
    let rendered = render_world_reaction_block(&plans).unwrap();
    assert_eq!(rendered, plans[0].to_guidance_block());
    // A single block carries no "\n\n" separator (no trailing/leading join artifact).
    assert!(!rendered.starts_with("\n\n") && !rendered.ends_with("\n\n"));
}

// ── Part B: Attack intent derivation (World emits intent, never resolves) ─────────

#[test]
fn attack_intent_derived_for_hostile_combative_plan() {
    // Hostility 90 ⇒ hostile stance + high willingness_to_fight (≥ threshold) ⇒ Attack.
    let plans = vec![plan("npc_brute", 0, 0, 90, &[])];
    assert!(
        plans[0].willingness_to_fight >= ATTACK_INTENT_FIGHT_THRESHOLD,
        "fixture must clear the fight threshold"
    );
    assert_eq!(plans[0].stance, RelationshipStance::Hostile);
    let mut set = assemble_world_reaction_set(&plans);
    derive_attack_intents(&plans, &mut set);
    let intent = set.reactions[0]
        .action_intent
        .as_ref()
        .expect("hostile combative plan yields an Attack intent");
    assert_eq!(intent.kind, NpcActionKind::Attack);
    assert_eq!(intent.target_ref.as_deref(), Some("player_party"));
    // The intent carries NO mechanical number — prose hint only (World resolves nothing).
    assert!(!intent.description.chars().any(|c| c.is_ascii_digit()));
}

#[test]
fn no_attack_intent_for_friendly_or_passive_plan() {
    // Trusting/warm NPC: not hostile, low fight willingness ⇒ candidate stays posture-only.
    let plans = vec![plan("npc_friend", 80, 0, 0, &[])];
    assert!(plans[0].willingness_to_fight < ATTACK_INTENT_FIGHT_THRESHOLD);
    let mut set = assemble_world_reaction_set(&plans);
    derive_attack_intents(&plans, &mut set);
    assert!(
        set.reactions[0].action_intent.is_none(),
        "friendly plan must not propose an attack"
    );
}

#[test]
fn attack_intent_gate_is_independent_of_knowledge() {
    // P3 ⑤反思: the action gate is its OWN trigger — secret/knowledge state must not
    // change whether an Attack is proposed. Two hostile combative plans, one holding a
    // withheld secret, both still get the intent (gate keys on fight willingness/stance).
    let entries = vec![NpcKnowledgeEntry {
        fact_id: "secret".into(),
        state: trpg_model::KnowledgeState::KnowsTrue,
    }];
    let view = NpcMindView::build(
        "s",
        "npc_secretkeeper",
        &profile("npc_secretkeeper"),
        &[rel("npc_secretkeeper", 0, 0, 90)],
        &entries,
    )
    .unwrap();
    let ctx = NpcBehaviorContext {
        secret_fact_ids: vec!["secret".into()],
        ..Default::default()
    };
    let p_secret = NpcBehaviorPlan::derive(&view, &ctx);
    let plans = vec![plan("npc_brute", 0, 0, 90, &[]), p_secret];
    let mut set = assemble_world_reaction_set(&plans);
    derive_attack_intents(&plans, &mut set);
    assert!(set.reactions[0].action_intent.is_some());
    assert!(
        set.reactions[1].action_intent.is_some(),
        "withheld-secret holder still attacks; gate keyed on fight willingness, not secrets"
    );
}

// ── §二-⑤/⑥ guard: the World layer resolves NOTHING ─────────────────────────────

#[test]
fn world_dir_contains_no_dice_or_resolution_calls() {
    // The World layer MUST NOT roll dice or resolve outcomes — it only emits typed
    // proposals (NpcActionIntent). Mechanical settlement lives in Rules/Kernel. This
    // scans every .rs under crates/trpg-runtime/src/world/ for resolution primitives.
    // (Scoped to non-test code lines + comments; the markers below would only appear if
    // someone wired settlement INTO the World layer, which is the boundary violation.)
    use std::fs;
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/world");
    let banned = [
        "resolve_check_with_input",
        "thread_rng",
        "roll_seeded",
        "resolve_outcome",
    ];
    let mut hits: Vec<String> = Vec::new();
    for entry in fs::read_dir(dir).expect("world dir readable") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        // Test scaffolding (the externalized `*_tests.rs` siblings) is not World
        // production code; it legitimately names these markers in assertions and in
        // this guard's own `banned` literal. The guard scans production World source.
        let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if fname.ends_with("_tests.rs") {
            continue;
        }
        let src = fs::read_to_string(&path).expect("read world source");
        for needle in &banned {
            // Allow the needle inside THIS guard test's own `banned` array literal.
            let occurrences = src.matches(needle).count();
            let in_guard = src.contains("let banned = [") && path.ends_with("reaction.rs");
            let allowed = if in_guard && *needle != "" { 1 } else { 0 };
            if occurrences > allowed {
                hits.push(format!("{}: {needle}", path.display()));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "World layer must not resolve mechanics (§二-⑤/⑥); offending refs: {hits:?}"
    );
}

//! TC-D3-02 acceptance: active NPC behavior guidance reaches GM prompt assembly.
//!
//! These exercise the real production prompt assembler ([`TurnMessages::assemble`]) fed
//! the exact runtime+model seam that `GmLoop::build_npc_behavior_guidance` composes:
//! project the NPC's own prompt-safe mind view, derive the player-party secret-gated
//! [`NpcBehaviorContext`] ([`trpg_runtime::viewer_behavior_context`]), derive the plan,
//! and render [`NpcBehaviorPlan::to_guidance_block`]. This is fully deterministic and
//! needs no DB; a DB-gated round-trip below proves the durable load path end to end.
//!
//! The whole module is wrapped in `mod npc_behavior` so every required test
//! (`active_npc_*`) is matched by `cargo test -p trpg-gm npc_behavior`.

mod npc_behavior {
    use trpg_gm::{DynamicTailInput, TurnMessages};
    use trpg_model::{
        CompiledContext, KnowledgeState, NpcKnowledgeEntry, NpcMindView, NpcProfile,
        NpcRelationship, NpcRelationshipDelta, NpcRelationshipTarget, NpcSecret, Visibility,
    };

    fn profile() -> NpcProfile {
        NpcProfile {
            actor_id: "npc_lars".into(),
            name: "Lars".into(),
            role: Some("mechanic".into()),
            goals: vec!["protect the workshop".into()],
            behavioral_boundaries: vec!["never harm a child".into()],
            ..Default::default()
        }
    }

    fn rel(trust: i16, fear: i16, hostility: i16) -> NpcRelationship {
        let mut r =
            NpcRelationship::new("s", "npc_lars", NpcRelationshipTarget::PlayerParty).unwrap();
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

    fn entry(id: &str) -> NpcKnowledgeEntry {
        NpcKnowledgeEntry {
            fact_id: id.into(),
            state: KnowledgeState::KnowsTrue,
        }
    }

    fn view(p: &NpcProfile, rel: NpcRelationship, entries: &[NpcKnowledgeEntry]) -> NpcMindView {
        NpcMindView::build("s", "npc_lars", p, &[rel], entries).unwrap()
    }

    /// The exact seam `GmLoop::build_npc_behavior_guidance` runs per active NPC, minus the
    /// durable profile/mind-view reads (supplied in-memory here).
    fn guidance_for(view: &NpcMindView, player_known: &[String]) -> String {
        let ctx = trpg_runtime::viewer_behavior_context(view, player_known);
        let plan = trpg_runtime::derive_npc_behavior_plan(view, &ctx);
        plan.to_guidance_block()
    }

    /// Assemble a turn and return the full prompt text (all message contents joined). The
    /// guidance block rides the production dynamic tail exactly as the GM loop injects it.
    fn assembled_prompt(npc_guidance: Option<&str>) -> String {
        let compiled = CompiledContext {
            prefix_text: "BP1".into(),
            pinned_text: "BP2".into(),
            dynamic_text: "BP3".into(),
            ..Default::default()
        };
        let tail = DynamicTailInput {
            user_input: "I approach the workshop.",
            resolved_gate_facts: &[],
            errata_blocks: &[],
            obligations_block: None,
            npc_guidance_block: npc_guidance,
        };
        let messages = TurnMessages::assemble(&compiled, "GM_SKILL", &[], &tail);
        messages
            .to_request_messages()
            .iter()
            .filter_map(|m| {
                m.get("content")
                    .and_then(|c| c.as_str())
                    .map(str::to_string)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn active_npc_behavior_plan_enters_prompt() {
        // Hostile NPC knows two true facts; the party already knows one. The player-unknown
        // fact is secret-gated to withheld; the shared one is freely revealable.
        let v = view(
            &profile(),
            rel(0, 0, 80),
            &[entry("shared_fact"), entry("hidden_fact")],
        );
        let block = guidance_for(&v, &["shared_fact".to_string()]);
        let prompt = assembled_prompt(Some(&block));

        assert!(
            prompt.contains("[npc_behavior_guidance npc=npc_lars]"),
            "{prompt}"
        );
        assert!(prompt.contains("[/npc_behavior_guidance]"));
        // Guidance reflects relationship stance + reveal/withhold sets.
        assert!(prompt.contains("Stance toward focus: hostile"), "{prompt}");
        assert!(
            prompt.contains("May reveal fact ids: shared_fact"),
            "{prompt}"
        );
        assert!(
            prompt.contains("Withhold fact ids: hidden_fact"),
            "{prompt}"
        );
        // It rides the dynamic tail, before the player input.
        assert!(
            prompt.find("[npc_behavior_guidance").unwrap() < prompt.find("[Player Input]").unwrap()
        );
    }

    #[test]
    fn active_npc_prompt_excludes_gm_only_profile_secret() {
        let mut p = profile();
        p.secrets = vec![
            NpcSecret {
                secret_id: "doom".into(),
                content: "GMONLY_THE_REACTOR_WILL_BLOW".into(),
                visibility: Visibility::GmOnly,
                source_refs: vec![],
            },
            NpcSecret {
                secret_id: "rumor".into(),
                content: "PLAYER_SAFE_TOWN_RUMOR".into(),
                visibility: Visibility::PlayerVisible,
                source_refs: vec![],
            },
        ];
        let v = view(&p, rel(40, 0, 0), &[entry("open_fact")]);
        let block = guidance_for(&v, &[]);
        let prompt = assembled_prompt(Some(&block));

        assert!(prompt.contains("[npc_behavior_guidance npc=npc_lars]"));
        assert!(
            !prompt.contains("GMONLY_THE_REACTOR_WILL_BLOW"),
            "GM-only secret text leaked into prompt: {prompt}"
        );
    }

    #[test]
    fn active_npc_prompt_excludes_unknown_fact() {
        // The NPC holds only `alice_knows`. A GM/world fact the NPC does not hold is never
        // in its mind view, so it can never enter the plan or the prompt — even though we
        // pass max trust (proving the gate is knowledge, not willingness).
        let v = view(&profile(), rel(100, 0, 0), &[entry("alice_knows")]);
        let block = guidance_for(&v, &[]);
        let prompt = assembled_prompt(Some(&block));

        assert!(
            prompt.contains("alice_knows"),
            "known fact should be present: {prompt}"
        );
        assert!(
            !prompt.contains("gm_world_secret_fact"),
            "a fact the NPC does not know must never enter guidance: {prompt}"
        );
    }

    #[test]
    fn active_npc_missing_profile_fails_soft() {
        // `GmLoop::build_npc_behavior_guidance` returns None when an active NPC has no
        // durable profile (Ok(None)) or a corrupt/failed read (Err): the NPC is skipped
        // and no guidance is injected. Assembly must then succeed with no guidance block
        // and never panic. (The DB-driven skip itself is covered by the gated test below.)
        let prompt = assembled_prompt(None);
        assert!(
            !prompt.contains("[npc_behavior_guidance"),
            "no profile => no guidance block: {prompt}"
        );
        assert!(
            prompt.contains("[Player Input]"),
            "assembly still produces a turn"
        );
        // An empty/blank guidance string is likewise dropped (no empty block).
        let blank = assembled_prompt(Some("   "));
        assert!(!blank.contains("[npc_behavior_guidance"));
    }

    // ============ TC-D3-06: projection-driven NPC consistency after-stream pass ============
    //
    // These exercise the exact pure seam `run_after_llm_stream_hook` runs per active NPC
    // (minus the durable profile/projection reads): build the NPC's own speech projection
    // and feed `npc_consistency_findings` the same private `secret_terms` the verifier
    // already consumes. Deterministic, no DB.

    use trpg_agent::{VerifierFinding, VerifierFindingKind};
    use trpg_gm::plugin::SecretTerm;
    use trpg_gm::turn_loop::npc_consistency_findings;
    use trpg_runtime::{npc_speech_projection_from_mind, NpcSpeechProjection};

    fn secret_term(term: &str, fact_id: &str) -> SecretTerm {
        SecretTerm {
            term: term.into(),
            fact_id: Some(fact_id.into()),
        }
    }

    /// Build the speaking NPC's own speech projection from in-memory knowledge, secret-gated
    /// against the player party, exactly as `project_for_npc_speech` does after the DB load.
    fn speech_proj(entries: &[NpcKnowledgeEntry], player_known: &[String]) -> NpcSpeechProjection {
        let v = view(&profile(), rel(0, 0, 80), entries);
        npc_speech_projection_from_mind(v, player_known)
    }

    /// No active NPC → the consistency pass is a no-op (empty), regardless of narration.
    #[test]
    fn active_npc_consistency_no_npc_is_noop() {
        let terms = vec![secret_term("the portal in the cellar", "hidden_fact")];
        let out = npc_consistency_findings(
            "Lars whispers: the portal in the cellar is real.",
            &[],
            &terms,
            &[],
        );
        assert!(out.is_empty(), "no active NPC must produce no findings");
    }

    /// Active NPC whose narration mentions a fact it KNOWS but the party does not (withheld
    /// secret) → a redacted `SecretLeak` finding that references the fact id, never the term.
    #[test]
    fn active_npc_withheld_fact_mention_yields_redacted_secret_leak() {
        const TERM: &str = "the portal in the cellar";
        let terms = vec![secret_term(TERM, "hidden_fact")];
        // Lars knows `hidden_fact`; the party knows nothing → it is a withheld secret.
        // Display name ("Lars") rides on the projection's persona safe-view, so no tuple.
        let proj = speech_proj(&[entry("hidden_fact")], &[]);
        let npcs = vec![proj];
        // Narration plausibly involves Lars (name) and surfaces the secret term.
        let narration = format!("Lars leans in: {TERM} hums faintly.");

        let out = npc_consistency_findings(&narration, &npcs, &terms, &[]);
        assert_eq!(
            out.len(),
            1,
            "withheld fact disclosure → one finding: {out:?}"
        );
        assert_eq!(out[0].kind, VerifierFindingKind::SecretLeak);
        assert!(
            out[0].detail.contains("hidden_fact"),
            "finding must reference the fact id: {}",
            out[0].detail
        );
        assert!(
            !out[0].detail.contains(TERM),
            "finding must never echo the matched secret term: {}",
            out[0].detail
        );

        // De-dupe: if the player projection already flagged this same fact, the NPC pass
        // suppresses the duplicate (same fact already covered by an existing SecretLeak).
        let existing = vec![VerifierFinding {
            kind: VerifierFindingKind::SecretLeak,
            severity: out[0].severity,
            detail: "player-visible narration exposes player-unknown fact 'hidden_fact'".into(),
        }];
        assert!(
            npc_consistency_findings(&narration, &npcs, &terms, &existing).is_empty(),
            "existing SecretLeak for the same fact → NPC pass de-dupes to empty"
        );

        // Visibility gate: narration that does not name the NPC is skipped (low false positives).
        let unrelated = format!("A draft stirs; {TERM} hums faintly.");
        assert!(
            npc_consistency_findings(&unrelated, &npcs, &terms, &[]).is_empty(),
            "narration not naming the NPC → skipped"
        );
    }

    // ============ 设计3 §13-P3: NPC behavior-consistency after-stream pass ============
    //
    // The behavioral sibling of the knowledge consistency pass: narration that exhibits
    // an action the speaking NPC's own plan `forbidden_actions` prohibits → an advisory
    // (Warning / InventedEffect) finding. Markers are derived deterministically from the
    // plan's forbidden actions; taboo-topic forbiddens bind to the concrete topic term.

    use trpg_gm::turn_loop::npc_behavior_consistency_findings;
    use trpg_model::SpeechStyle;

    /// Build a speech projection for an NPC whose persona declares a taboo topic, so the
    /// derived plan forbids "raise taboo topic: <term>".
    fn taboo_speech_proj(taboo: &str) -> NpcSpeechProjection {
        let p = NpcProfile {
            actor_id: "npc_lars".into(),
            name: "Lars".into(),
            speech_style: SpeechStyle {
                taboo_topics: vec![taboo.into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let v = view(&p, rel(0, 0, 0), &[]);
        npc_speech_projection_from_mind(v, &[])
    }

    /// NPC narration that raises its own forbidden taboo topic → advisory behavior finding
    /// that references the action label, never echoing the matched narration substring.
    #[test]
    fn active_npc_forbidden_behavior_is_flagged_advisory() {
        const TABOO: &str = "the duke's downfall";
        let proj = taboo_speech_proj(TABOO);
        assert!(
            proj.0
                .plan
                .forbidden_actions
                .iter()
                .any(|a| a == &format!("raise taboo topic: {TABOO}")),
            "precondition: plan forbids the taboo topic"
        );
        let npcs = vec![proj];
        let narration = format!("Lars loudly brings up {TABOO} in front of everyone.");
        let out = npc_behavior_consistency_findings(&narration, &npcs, &[]);
        assert_eq!(
            out.len(),
            1,
            "forbidden behavior exhibited → one finding: {out:?}"
        );
        assert_eq!(out[0].kind, VerifierFindingKind::InventedEffect);
        assert!(
            out[0].detail.contains("npc_lars"),
            "finding references the speaking NPC: {}",
            out[0].detail
        );
        assert!(
            !out[0].detail.contains(TABOO) || out[0].detail.contains("raise taboo topic"),
            "detail references the action label, not a bare narration echo: {}",
            out[0].detail
        );
    }

    /// Consistent narration (no forbidden action surfaced) → no finding; and a narration
    /// that does not name the NPC is skipped by the visibility gate.
    #[test]
    fn active_npc_consistent_behavior_and_visibility_gate_pass() {
        const TABOO: &str = "the duke's downfall";
        let proj = taboo_speech_proj(TABOO);
        let npcs = vec![proj];
        // Names Lars but never raises the taboo topic → no contradiction.
        let ok = npc_behavior_consistency_findings(
            "Lars greets the party and offers them tea.",
            &npcs,
            &[],
        );
        assert!(ok.is_empty(), "consistent behavior must pass: {ok:?}");
        // Surfaces the taboo term but does not name the NPC → visibility gate skips it.
        let unrelated = format!("Somewhere, a rumor about {TABOO} spreads on its own.");
        assert!(
            npc_behavior_consistency_findings(&unrelated, &npcs, &[]).is_empty(),
            "narration not naming the NPC → skipped"
        );
    }

    /// A fact the PLAYER already knows (so the player projection leak check allows it) but
    /// the speaking NPC does NOT hold still yields an NPC consistency finding — the NPC is
    /// the speaker, and it cannot honestly state a fact it does not know.
    #[test]
    fn active_npc_player_known_but_npc_unknown_fact_flags_when_speaker() {
        const TERM: &str = "the duke's hidden heir";
        let terms = vec![secret_term(TERM, "shared_fact")];
        let player_known = vec!["shared_fact".to_string()];
        // Lars knows only `own_fact`; he does NOT hold `shared_fact` at all.
        let proj = speech_proj(&[entry("own_fact")], &player_known);
        let npcs = vec![proj];
        let narration = format!("Lars confides: {TERM} walks among us.");

        // No existing SecretLeak (the player projection allows a player-known fact), yet the
        // NPC-speaker consistency check still fires because Lars cannot know `shared_fact`.
        let out = npc_consistency_findings(&narration, &npcs, &terms, &[]);
        assert!(
            !out.is_empty(),
            "NPC-unknown fact stated by the speaker → consistency finding: {out:?}"
        );
        assert!(
            out.iter().any(|f| f.detail.contains("shared_fact")),
            "finding must reference the fact id: {out:?}"
        );
        assert!(
            out.iter().any(|f| f.detail.contains("npc_lars")),
            "finding must reference the speaking NPC: {out:?}"
        );
        assert!(
            out.iter().all(|f| !f.detail.contains(TERM)),
            "finding must never echo the matched secret term: {out:?}"
        );
    }

    /// DB-gated end-to-end: persist a profile + relationship + knowledge edges, then run
    /// the real durable load path (`load_npc_profile` → `load_active_npc_guidance`) exactly
    /// as the GM loop does, and assert a prompt-safe block is produced with GM-only secrets
    /// excluded. SKIP (never fake-PASS) when DATABASE_URL is unreachable.
    #[tokio::test]
    async fn active_npc_behavior_guidance_db_roundtrip() {
        let Some(db) = connect_or_skip().await else {
            return;
        };
        let session = format!("sess_npc_beh_{}", uuid::Uuid::new_v4().simple());
        purge(&db, &session).await;

        // Durable profile with a GM-only secret that must never reach the prompt.
        let mut p = profile();
        p.secrets = vec![NpcSecret {
            secret_id: "doom".into(),
            content: "GMONLY_THE_REACTOR_WILL_BLOW".into(),
            visibility: Visibility::GmOnly,
            source_refs: vec![],
        }];
        db.upsert_npc_profile(&session, &p).await.unwrap();

        // NPC-known facts: one the party already knows, one it does not (=> secret).
        edge(
            &db,
            &session,
            "npc",
            "npc_lars",
            "shared_fact",
            "knows_true",
        )
        .await;
        edge(
            &db,
            &session,
            "npc",
            "npc_lars",
            "hidden_fact",
            "knows_true",
        )
        .await;
        edge(
            &db,
            &session,
            "player_party",
            "",
            "shared_fact",
            "knows_true",
        )
        .await;
        // A GM world truth the NPC does not hold — must never surface.
        edge(
            &db,
            &session,
            "gm",
            "",
            "gm_world_secret_fact",
            "knows_true",
        )
        .await;

        // Hostile relationship (under THIS session) so the player-unknown secret stays
        // withheld and the stance projects as hostile through the durable load path.
        let mut r =
            NpcRelationship::new(&session, "npc_lars", NpcRelationshipTarget::PlayerParty).unwrap();
        r.apply_delta(&NpcRelationshipDelta {
            hostility: 80,
            evidence_event_ids: vec!["e".into()],
            ..Default::default()
        })
        .unwrap();
        db.upsert_npc_relationship(&r).await.unwrap();

        let player_known = db.list_player_known_fact_ids(&session).await.unwrap();
        let loaded = db
            .load_npc_profile(&session, "npc_lars")
            .await
            .unwrap()
            .unwrap();
        let targets = [NpcRelationshipTarget::PlayerParty];
        let plan = trpg_runtime::load_active_npc_guidance(
            &db,
            &session,
            "npc_lars",
            &loaded,
            &targets,
            &player_known,
        )
        .await
        .unwrap();
        let block = plan.to_guidance_block();
        let prompt = assembled_prompt(Some(&block));

        assert!(
            prompt.contains("[npc_behavior_guidance npc=npc_lars]"),
            "{prompt}"
        );
        assert!(prompt.contains("Stance toward focus: hostile"), "{prompt}");
        assert!(
            prompt.contains("May reveal fact ids: shared_fact"),
            "{prompt}"
        );
        assert!(
            prompt.contains("Withhold fact ids: hidden_fact"),
            "{prompt}"
        );
        assert!(
            !prompt.contains("gm_world_secret_fact"),
            "world truth leaked: {prompt}"
        );
        assert!(
            !prompt.contains("GMONLY_THE_REACTOR_WILL_BLOW"),
            "secret leaked: {prompt}"
        );

        // Missing profile fails soft at the durable layer: no row => Ok(None), no panic.
        let absent = db.load_npc_profile(&session, "npc_nobody").await.unwrap();
        assert!(
            absent.is_none(),
            "missing profile must be Ok(None), not a row"
        );

        purge(&db, &session).await;
    }

    async fn connect_or_skip() -> Option<trpg_db::Db> {
        let url = match std::env::var("DATABASE_URL") {
            Ok(u) => u,
            Err(_) => {
                eprintln!("SKIP: DATABASE_URL unset");
                return None;
            }
        };
        let db = match trpg_db::Db::connect(&url).await {
            Ok(d) => d,
            Err(e) => {
                eprintln!("SKIP: connect failed: {e}");
                return None;
            }
        };
        if let Err(e) = db.migrate().await {
            eprintln!("SKIP: migrate failed: {e}");
            return None;
        }
        Some(db)
    }

    async fn purge(db: &trpg_db::Db, session: &str) {
        for table in ["knowledge_edges", "npc_profiles", "npc_relationships"] {
            let _ = sqlx::query(&format!("delete from {table} where session_id=$1"))
                .bind(session)
                .execute(&db.pool)
                .await;
        }
    }

    async fn edge(
        db: &trpg_db::Db,
        session: &str,
        kind: &str,
        holder_id: &str,
        fact: &str,
        state: &str,
    ) {
        db.upsert_knowledge_edge(trpg_db::KnowledgeEdgeInput {
            session_id: session,
            holder_kind: kind,
            holder_id,
            fact_id: fact,
            knowledge_state: state,
            confidence: None,
            learned_at_turn_id: None,
            disclosure_policy: None,
            source_event_id: None,
            reason: None,
        })
        .await
        .unwrap();
    }
}

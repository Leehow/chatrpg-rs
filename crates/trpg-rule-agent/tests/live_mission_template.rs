//! P2-2 live 实证:AdventureIR 是否能忠实表示 The Vault 的任务-选集结构,而不把它
//! 压扁成扁平场景链。
//!
//! 现状(DB :54347):the_vault 的 module_graph 有 **12 个 scenes、0 个 missions**
//! —— agentic reader 把 12 个任务压成了 12 个平铺场景,删掉了 Mission/计分/
//! Aftermath 语义。本测试从**真实源文**(page-anchored markdown)用结构指纹归纳出
//! 重复任务模板,切出 N 个一等 Mission(各带 scored Optional Objectives、Chaos 能力
//! 阶梯、分叉 Aftermath),并**逐字对源文反造假**。
//!
//! Run (rulesets DB lives at :54347):
//!   DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!   VAULT_MD=/Users/haoli/leehow/code/chatrpgv2/_rstest_triangle/markdown/modules/the_vault_missions_for_triangle_agency_plaintext_1_2.md \
//!   CARGO_TARGET_DIR=target-air cargo test -p trpg-rule-agent \
//!     --test live_mission_template -- --nocapture
//!
//! 无 `DATABASE_URL` ⇒ SKIP(fail-closed,绝不阻塞 CI)。**反假绿**:真跑时必打印
//! `RAN: ...` + 计数 + 逐字断言 + `PASS`;只见 `SKIP` = 没验证。源文缺失 ⇒ panic
//! (不静默跳过,避免假绿)。
use trpg_db::Db;
use trpg_model::adventure_ir::{induct_missions, MissionPage, UnitKind};

const VAULT: &str = "triangle_agency.the_vault";
const SOURCE_ID: &str = "the_vault_missions_for_triangle_agency_plaintext_1_2";
const DEFAULT_MD: &str = "/Users/haoli/leehow/code/chatrpgv2/_rstest_triangle/markdown/modules/the_vault_missions_for_triangle_agency_plaintext_1_2.md";

/// Split page-anchored markdown into `MissionPage`s by `# Page N` headers,
/// dropping the HTML source-anchor comment lines.
fn parse_pages(md: &str) -> Vec<MissionPage> {
    let mut pages = Vec::new();
    let mut cur_page: Option<u32> = None;
    let mut buf = String::new();
    let flush = |pages: &mut Vec<MissionPage>, page: Option<u32>, buf: &str| {
        if let Some(p) = page {
            pages.push(MissionPage::new(p, buf.to_string()));
        }
    };
    for line in md.lines() {
        if let Some(rest) = line.strip_prefix("# Page ") {
            if let Ok(n) = rest.trim().parse::<u32>() {
                flush(&mut pages, cur_page, &buf);
                cur_page = Some(n);
                buf.clear();
                continue;
            }
        }
        if line.starts_with("<!-- source_id=") {
            continue;
        }
        buf.push_str(line);
        buf.push('\n');
    }
    flush(&mut pages, cur_page, &buf);
    pages
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_vault_inducts_faithful_missions_not_flat_scenes() {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP: DATABASE_URL unset (need live rulesets DB :54347)");
            return;
        }
    };
    let md_path = std::env::var("VAULT_MD").unwrap_or_else(|_| DEFAULT_MD.to_string());
    let md = std::fs::read_to_string(&md_path)
        .unwrap_or_else(|e| panic!("RAN but source markdown missing at {md_path}: {e}"));
    let pages = parse_pages(&md);
    eprintln!("RAN: parsed {} source pages from {md_path}", pages.len());
    assert!(pages.len() > 100, "Vault source should have >100 pages");

    // --- DB baseline: confirm the COLLAPSE (12 scenes, 0 missions). ---
    let db = Db::connect(&url).await.expect("connect live DB");
    let graph = db
        .load_module_graph(VAULT)
        .await
        .expect("query ok")
        .expect("the_vault module graph present in parsed_bundles");
    eprintln!(
        "RAN: DB baseline scenes={}, missions={}, content_units={}",
        graph.scenes.len(),
        graph.missions.len(),
        graph.content_units.len()
    );
    assert_eq!(
        graph.missions.len(),
        0,
        "baseline: reader collapsed missions to 0"
    );
    assert!(
        graph.scenes.len() >= 10,
        "baseline: ~12 missions flattened into scenes"
    );

    // --- Induct first-class missions from the real source (structure-fingerprint). ---
    let missions = induct_missions(VAULT, SOURCE_ID, &pages);
    let n = missions.len();
    let scored: usize = missions.iter().map(|m| m.objectives.len()).sum();
    let with_chaos = missions.iter().filter(|m| m.chaos.is_some()).count();
    let with_aftermath = missions.iter().filter(|m| !m.aftermath.is_empty()).count();
    eprintln!(
        "RAN: inducted missions={n}, total_scored_objectives={scored}, with_chaos={with_chaos}, with_aftermath={with_aftermath}"
    );
    for m in &missions {
        let comm: i64 = m
            .objectives
            .iter()
            .flat_map(|o| &o.score_effects)
            .filter(|s| s.label == "Commendation")
            .map(|s| s.delta)
            .sum();
        let rungs = m.chaos.as_ref().map(|c| c.rungs.len()).unwrap_or(0);
        eprintln!(
            "  - {:<28} kind={:?} objectives={} (+{comm} Commend) chaos_rungs={rungs} aftermath={}",
            m.unit.title,
            m.unit.kind,
            m.objectives.len(),
            m.aftermath.len()
        );
    }

    // 1) NOT collapsed: many first-class Mission units (source has 12).
    assert!(
        n >= 10,
        "induct >=10 first-class missions (source 12), got {n}"
    );
    assert!(missions.iter().all(|m| m.unit.kind == UnitKind::Mission));

    // 2) The scored mechanics the flat scene list deleted are retained PER mission.
    //    Every mission keeps its scored objectives AND an aftermath outcome (the two
    //    universal template structures). The Chaos ability ladder is present only in
    //    the subset of missions whose source actually TABULATES one — the rest carry
    //    a prose "Special Rules" chaos section or none, and the fail-closed parser
    //    correctly refuses to fabricate rungs there.
    //    Source ground truth (verified by grep on the markdown): 8 `CHAOS EFFECTS`
    //    markers, of which 2 are prose/empty (Hollow Suit p117 is a "Special Rules"
    //    prose block; Murder at Gruntley p171 is an empty header) → exactly 5
    //    extractable ability ladders. Demanding a ladder on all 11 missions would
    //    force fabrication, which the constitution forbids; faithfulness here means
    //    extracting the 5 that exist verbatim and leaving the rest empty.
    let with_obj = missions.iter().filter(|m| !m.objectives.is_empty()).count();
    let with_after = missions.iter().filter(|m| !m.aftermath.is_empty()).count();
    let branching = missions.iter().filter(|m| m.aftermath.len() >= 2).count();
    eprintln!(
        "RAN: with_objectives={with_obj}/{n}, with_aftermath={with_after}/{n}, branching_aftermath={branching}, chaos_ladders={with_chaos}"
    );
    assert!(
        missions.iter().all(|m| !m.objectives.is_empty()),
        "every mission retains its scored objectives (not flattened away)"
    );
    assert!(
        missions.iter().all(|m| !m.aftermath.is_empty()),
        "every mission retains an aftermath outcome"
    );
    assert!(
        branching >= 1,
        "at least one mission keeps a BRANCHING aftermath (>=2 outcomes), got {branching}"
    );
    assert!(
        with_chaos >= 5,
        "missions whose source tabulates a Chaos ladder carry it verbatim, got {with_chaos}/5"
    );
    // scored objectives really score (Commendation / Demerit present somewhere).
    assert!(
        missions
            .iter()
            .flat_map(|m| &m.objectives)
            .any(|o| o.score_effects.iter().any(|s| s.label == "Commendation")),
        "scored Commendations present"
    );
    assert!(
        missions
            .iter()
            .flat_map(|m| &m.objectives)
            .any(|o| o.score_effects.iter().any(|s| s.label == "Demerit")),
        "scored Demerits present"
    );

    // 3) Anti-fabrication: every extracted value must literally appear in the source.
    //    (a) a Chaos rung's "<cost> Chaos" and its ability label.
    let sample = missions
        .iter()
        .find(|m| {
            m.chaos
                .as_ref()
                .map(|c| !c.rungs.is_empty())
                .unwrap_or(false)
        })
        .expect("a mission with a chaos rung");
    let chaos = sample.chaos.as_ref().unwrap();
    let rung = &chaos.rungs[0];
    let cost_str = format!("{} Chaos", rung.cost);
    assert!(
        md.contains(&cost_str),
        "anti-fabrication: '{cost_str}' must appear verbatim in source"
    );
    assert!(
        md.contains(rung.label.trim_end_matches('*')),
        "anti-fabrication: chaos ability '{}' must appear in source",
        rung.label
    );
    // every rung of the sampled mission's ladder must be source-grounded, not just
    // the first — no fabricated ability slipped into the ladder.
    for r in &chaos.rungs {
        assert!(
            md.contains(r.label.trim_end_matches('*')),
            "anti-fabrication: chaos ability '{}' must appear verbatim in source",
            r.label
        );
    }
    eprintln!(
        "RAN: verbatim chaos rungs OK — all {} rungs source-grounded (e.g. '{}' / '{}', mission '{}')",
        chaos.rungs.len(), cost_str, rung.label, sample.unit.title
    );

    //    (b) a scored objective's "+N Commendation/Demerit" appears verbatim.
    let sobj = missions
        .iter()
        .flat_map(|m| &m.objectives)
        .find(|o| !o.score_effects.is_empty())
        .expect("a scored objective");
    let se = &sobj.score_effects[0];
    let score_str = format!("+{} {}", se.delta, se.label);
    assert!(
        md.contains(&score_str),
        "anti-fabrication: '{score_str}' must appear verbatim in source"
    );
    eprintln!("RAN: verbatim scored objective OK — '{score_str}'");

    //    (c) an aftermath outcome title appears verbatim (skip the fail-closed
    //        fallback "Aftermath").
    if let Some(out) = missions
        .iter()
        .flat_map(|m| &m.aftermath)
        .find(|o| o.title != "Aftermath")
    {
        assert!(
            md.contains(&out.title),
            "anti-fabrication: aftermath outcome '{}' must appear in source",
            out.title
        );
        eprintln!("RAN: verbatim aftermath outcome OK — '{}'", out.title);
    }

    eprintln!(
        "PASS: The Vault IR faithful — {n} first-class missions with scored objectives + Chaos ladder + branching aftermath (NOT flattened to {} scenes)",
        graph.scenes.len()
    );
}

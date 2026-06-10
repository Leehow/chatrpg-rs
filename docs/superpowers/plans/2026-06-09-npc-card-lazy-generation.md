# NPC Card Lazy-Generation Implementation Plan (Phase 1 + Phase 2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
> **NO GIT in this working copy** — every "Commit" step is replaced by a **checkpoint = run the task's `cargo test` and confirm green**. Do NOT run git.

**Goal:** When an NPC appears, lazily build its card per-(NPC × parameter): each needed parameter is synthesized via T1 source > T2 archetype > T3 persona-judge (flagged provisional, drives resolution), accumulating on the card; Phase 2 de-collapses the single `npc.opposition` into per-NPC cards.

**Architecture:** T3 (LLM persona-judge) lives in `trpg-runtime` (the only turn-orchestrating crate holding both an `LlmClient` and `Db`); it writes a flagged-provisional value into the NPC's `runtime_actor_parameters.sheet_json` via an **independent write path that bypasses `trpg-material`'s strict gate**, then `trpg-contest` (which is sync + has NO LLM dep) consumes the now-bound value unchanged. Provenance lives in `sheet_json` (NOT `mechanical_profile`, which is reprojected/clobbered by `refresh_mechanical_profile`).

**Tech Stack:** Rust workspace (~26 crates); `trpg-llm::LlmClient`; `serde_json::Value` sheets; sqlx/Postgres; codex-relay LLM (rejects temperature).

**Spec:** `docs/superpowers/specs/2026-06-09-npc-card-lazy-generation-design.md`

---

## File Structure

**Phase 1 (core value — no `npc.opposition` de-collapse):**
- Create `crates/trpg-runtime/src/npc_synth.rs` — the NPC parameter synthesis slice: persona type, T3 LLM judge, the hybrid `ensure_npc_parameter` orchestrator, the independent provisional write (into `sheet_json`), the `TRPG_NPC_PERSONA_SYNTHESIS` gate. Pure helpers unit-tested; LLM via injected `&dyn LlmClient`.
- Modify `crates/trpg-runtime/src/lib.rs` — `mod npc_synth;`; call the hybrid as a pre-resolution pass in the turn flow; read scene NPC persona via the existing `scene_node_to_blocks` id→persona pattern.
- Modify `crates/trpg-material/src/lib.rs` — stamp `material_target_kind="npc_stat_block"` on the opposition demand so the `NpcStatblock` search plan runs (T1).
- Modify `crates/trpg-cli/src/main.rs` — GM prompt NPC 3-branch (mirror ITEM POLICY); document the gate.

**Phase 2 (full per-NPC):**
- Modify `crates/trpg-semantics/src/lib.rs` + `crates/trpg-model/src/lib.rs` — `target_search_keys` (mirror `object_search_keys`).
- Modify `crates/trpg-runtime/src/lib.rs` — `resolve_scene_npcs`; per-NPC ensure replacing the single `npc.opposition` warmup; retire `mentions_runtime_npc` (last).
- Modify `crates/trpg-combat/src/lib.rs`, `crates/trpg-mechanics/src/lib.rs`, `crates/trpg-contest/src/lib.rs`, `crates/trpg-object/src/lib.rs` — thread a resolved per-NPC defender id instead of the `npc.opposition` literal.

---

# PHASE 1

## Task 1: NPC synthesis slice — persona type + T3 persona-judge (LLM)

**Files:**
- Create: `crates/trpg-runtime/src/npc_synth.rs`
- Modify: `crates/trpg-runtime/src/lib.rs` (add `mod npc_synth;` near the other `mod` decls)
- Test: inline `#[cfg(test)]` in `npc_synth.rs`

- [ ] **Step 1: Write the failing test** — test the PURE response-parse (no LlmClient stub: the trait is `#[async_trait]` with `complete_text`/`complete_json`/`stream_chat` and no convenient default, so stubbing it is heavy; split the LLM call into pure `build_synthesis_messages` + pure `parse_synthesis_response` and test the latter).

```rust
// in crates/trpg-runtime/src/npc_synth.rs  #[cfg(test)] mod tests
use super::*;
use serde_json::json;

#[test]
fn parse_response_is_flagged_provisional_with_audit() {
    let persona = NpcPersona { actor_id: "npc.guard_1".into(), name: "City Watch Officer".into(),
        prose: "A trained municipal guard, alert and suspicious of pickpockets.".into() };
    let resp = json!({"value": 65, "reason": "trained guard, high vigilance vs theft"});
    let out = parse_synthesis_response(&resp, &persona, "anti_theft_dv", "player attempts to pickpocket").unwrap();
    assert_eq!(out.value, json!(65));
    assert_eq!(out.status, "provisional");
    assert_eq!(out.provenance["tier"], json!("persona_judge"));
    assert_eq!(out.provenance["parameter"], json!("anti_theft_dv"));
    assert_eq!(out.provenance["persona_actor_id"], json!("npc.guard_1"));
    assert!(out.provenance["reason"].as_str().unwrap().contains("guard"));
    // missing value => Err (fail-closed, no fabricated default)
    assert!(parse_synthesis_response(&json!({"reason":"x"}), &persona, "p", "c").is_err());
}

#[test]
fn build_messages_grounds_in_persona_and_param() {
    let persona = NpcPersona { actor_id: "npc.guard_1".into(), name: "Guard".into(), prose: "trained guard".into() };
    let msgs = build_synthesis_messages(&persona, "anti_theft_dv", "pickpocket attempt", "call_of_cthulhu_7e");
    let joined = msgs.iter().map(|m| m.content.clone()).collect::<Vec<_>>().join("\n");
    assert!(joined.contains("anti_theft_dv") && joined.contains("trained guard") && joined.contains("call_of_cthulhu_7e"));
}
```

- [ ] **Step 2: Run it, verify it FAILS**

Run: `cargo test -p trpg-runtime persona_judge_returns_flagged_provisional_with_audit -- --nocapture`
Expected: FAIL — `NpcPersona`/`synthesize_npc_parameter` not found.

- [ ] **Step 3: Implement `npc_synth.rs` (persona type + T3 judge)**

```rust
//! NPC card lazy synthesis: per-(NPC × parameter) value, hybrid T1 source > T2
//! archetype > T3 persona-judge. T3 is LLM-backed, ALWAYS flagged provisional +
//! audited, drives resolution (per the design's owned fail-closed-default flip),
//! and is upgradeable by a later source/archetype hit. Provenance lives in
//! `sheet_json` (NOT mechanical_profile, which is reprojected by refresh).
use serde_json::{json, Value};
use trpg_llm::LlmClient;

/// Who the NPC is — the persona that grounds T2/T3 synthesis. Sourced from the
/// module `graph.npcs[id]` (name + body/summary), or GM-supplied for an
/// improvised NPC (smart-GM, spec §7.7).
#[derive(Debug, Clone)]
pub struct NpcPersona { pub actor_id: String, pub name: String, pub prose: String }

/// One synthesized parameter value + its provenance (for audit + upgrade).
#[derive(Debug, Clone)]
pub struct SynthesizedParam { pub value: Value, pub status: String, pub provenance: Value }

use trpg_llm::ChatMessage;

/// PURE: build the T3 prompt messages (grounded in persona + the triggering check).
pub fn build_synthesis_messages(persona: &NpcPersona, parameter_id: &str, check_context: &str, ruleset_id: &str) -> Vec<ChatMessage> {
    let sys = format!(
        "You judge ONE mechanical parameter for an NPC in tabletop ruleset '{ruleset_id}', grounded in the NPC's \
         persona and the specific check that needs it. Output a value an experienced GM would set for THIS persona \
         (a trained guard is more vigilant than a random civilian). Do NOT invent a balanced default; reason from \
         the persona. Return STRICT JSON: {{\"value\": <number or dice string>, \"reason\": \"<one sentence>\"}}.");
    let user = format!(
        "NPC: {} — {}\nParameter needed: {parameter_id}\nTriggering check / context: {check_context}\n\
         Give the persona-appropriate value for `{parameter_id}`.", persona.name, persona.prose);
    vec![trpg_llm::system(sys), trpg_llm::user(user)]
}

/// PURE: turn the LLM JSON into a flagged-provisional SynthesizedParam + audit.
/// Missing `value` => Err (fail-closed; never fabricate a default).
pub fn parse_synthesis_response(resp: &Value, persona: &NpcPersona, parameter_id: &str, check_context: &str) -> anyhow::Result<SynthesizedParam> {
    let value = resp.get("value").cloned().ok_or_else(|| anyhow::anyhow!("persona-judge returned no value"))?;
    let reason = resp.get("reason").and_then(Value::as_str).unwrap_or("").to_string();
    Ok(SynthesizedParam {
        value, status: "provisional".into(),
        provenance: json!({
            "tier": "persona_judge", "parameter": parameter_id, "reason": reason,
            "persona_actor_id": persona.actor_id, "check_context": check_context,
            "source_policy": "persona_grounded_provisional_upgradeable",
        }),
    })
}

/// T3 thin async wrapper: build messages -> LLM (note `complete_json` takes
/// `Vec<ChatMessage>`) -> parse. Never a hardcoded label table; persona-grounded + audited.
pub async fn synthesize_npc_parameter(
    llm: &dyn LlmClient, persona: &NpcPersona, parameter_id: &str, check_context: &str, ruleset_id: &str,
) -> anyhow::Result<SynthesizedParam> {
    let resp = llm.complete_json(build_synthesis_messages(persona, parameter_id, check_context, ruleset_id), 0.4).await?;
    parse_synthesis_response(&resp, persona, parameter_id, check_context)
}
```

Add `mod npc_synth;` and `pub(crate) use npc_synth::*;` (or keep `npc_synth::` paths) in `lib.rs`. Imports needed in `npc_synth.rs`: `use trpg_llm::{LlmClient, ChatMessage};` `use serde_json::{json, Value};`. **No new dev-deps needed** — the unit tests call the PURE `parse_synthesis_response`/`build_synthesis_messages` (no LlmClient stub, no async test). `trpg-llm`/`serde_json`/`anyhow` are already deps.

- [ ] **Step 4: Run the test, verify PASS**

Run: `cargo test -p trpg-runtime persona_judge_returns_flagged_provisional_with_audit`
Expected: PASS.

- [ ] **Step 5: Checkpoint** — `cargo test -p trpg-runtime npc_synth 2>&1 | tail -5` shows `test result: ok.` (no git commit).

---

## Task 2: Independent provisional write path (bypasses strict; provenance in sheet_json)

**Files:**
- Modify: `crates/trpg-runtime/src/npc_synth.rs`
- Test: inline

The strict gate (`trpg-material/src/lib.rs:489-493`) returns `Ok(vec![])` for any non-`VerifiedExact` writeback, so a T3 value CANNOT go through material's `write_back`. T3 writes the value + provenance directly into the NPC's `sheet_json` (which survives `refresh_mechanical_profile`, since that reprojects FROM sheet_json — `trpg-runtime/src/lib.rs:1500/1522`).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn provisional_write_lands_in_sheet_and_survives_refresh() {
    let mut sheet = json!({"stats": {}, "skills": {}});
    let p = SynthesizedParam { value: json!(65), status: "provisional".into(),
        provenance: json!({"tier":"persona_judge","parameter":"anti_theft_dv"}) };
    write_synthesized_param(&mut sheet, "skills", "anti_theft_dv", &p);
    // value routed into the named bucket
    assert_eq!(sheet["skills"]["anti_theft_dv"], json!(65));
    // provenance + status recorded under a dedicated audit map IN sheet_json
    assert_eq!(sheet["npc_param_provenance"]["anti_theft_dv"]["tier"], json!("persona_judge"));
    assert_eq!(sheet["npc_param_provenance"]["anti_theft_dv"]["status"], json!("provisional"));
    // a later T1/T2 source value UPGRADES (overwrites) and is NOT downgraded by a subsequent provisional
    let src = SynthesizedParam { value: json!(70), status: "source_backed".into(), provenance: json!({"tier":"source"}) };
    write_synthesized_param(&mut sheet, "skills", "anti_theft_dv", &src);
    assert_eq!(sheet["skills"]["anti_theft_dv"], json!(70));
    assert_eq!(sheet["npc_param_provenance"]["anti_theft_dv"]["status"], json!("source_backed"));
    let prov_again = SynthesizedParam { value: json!(50), status: "provisional".into(), provenance: json!({"tier":"persona_judge"}) };
    write_synthesized_param(&mut sheet, "skills", "anti_theft_dv", &prov_again);
    assert_eq!(sheet["skills"]["anti_theft_dv"], json!(70), "source_backed not downgraded by later provisional");
}
```

- [ ] **Step 2: Run, verify FAIL** — `cargo test -p trpg-runtime provisional_write_lands` → FAIL (fn missing).

- [ ] **Step 3: Implement**

```rust
/// Tier ranking for the no-downgrade rule (higher wins). Mirrors spec ordering T1>T2>T3.
fn tier_rank(status: &str) -> u8 { match status { "source_backed" => 3, "source_backed_archetype" => 2, "provisional" => 1, _ => 0 } }

/// Write a synthesized parameter into the NPC sheet: route the value into the
/// named bucket (stats/skills/resources) and record status+provenance under
/// `sheet_json.npc_param_provenance.<param>`. NEVER downgrades: a higher-tier
/// existing value is kept (upgrade-only). Provenance lives in sheet_json so
/// `refresh_mechanical_profile` (which reprojects FROM sheet_json) cannot clobber it.
pub fn write_synthesized_param(sheet: &mut Value, bucket: &str, param: &str, p: &SynthesizedParam) {
    let obj = match sheet.as_object_mut() { Some(o) => o, None => return };
    let prov = obj.entry("npc_param_provenance").or_insert_with(|| json!({}));
    let existing_rank = prov.get(param).and_then(|x| x.get("status")).and_then(Value::as_str).map(tier_rank).unwrap_or(0);
    if tier_rank(&p.status) < existing_rank { return; } // no downgrade
    if let Some(pm) = prov.as_object_mut() {
        let mut entry = p.provenance.clone();
        if let Some(em) = entry.as_object_mut() { em.insert("status".into(), json!(p.status)); }
        pm.insert(param.to_string(), entry);
    }
    let b = obj.entry(bucket.to_string()).or_insert_with(|| json!({}));
    if let Some(bm) = b.as_object_mut() { bm.insert(param.to_string(), p.value.clone()); }
}
```

- [ ] **Step 4: Run, verify PASS** — `cargo test -p trpg-runtime provisional_write_lands` → PASS.
- [ ] **Step 5: Checkpoint** — `cargo test -p trpg-runtime npc_synth` green.

---

## Task 3: T2 archetype tier (new — persona→archetype best-effort, honestly bounded)

**Files:** Modify `crates/trpg-runtime/src/npc_synth.rs`; Test inline.

Ground truth: there is NO archetype retrieval index anywhere (confirmed); `RecommendedArchetype.fit_tags` is the only semantic handle and lives in the chargen onboarding pack. T2 is therefore a **best-effort LLM mapping of persona → fit_tags → an onboarding archetype's parameter**, and (honest, per spec §5/§10.1) it mostly WHIFFS for opposed-check DVs that rulebooks don't tabulate. It returns `None` on no match (→ caller falls to T3).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn archetype_tier_returns_none_when_no_param_match() {
    // No archetype carries a printed `anti_theft_dv` scalar -> T2 whiffs -> None (caller -> T3).
    let archetypes: Vec<(String, Value)> = vec![("guard".into(), json!({"fit_tags":["combat"],"params":{"hp":12}}))];
    let got = archetype_param(&archetypes, "anti_theft_dv");
    assert!(got.is_none());
    // But a printed scalar (hp) IS found.
    let got2 = archetype_param(&archetypes, "hp");
    assert_eq!(got2.unwrap().0, json!(12));
}
```

- [ ] **Step 2: Run, verify FAIL.**

- [ ] **Step 3: Implement** (pure lookup over already-matched archetype param maps; the persona→archetype MATCH is done by the orchestrator via LLM/fit_tags in Task 4)

```rust
/// T2: given the candidate archetype(s) already matched to this persona (each a
/// `(archetype_id, json)` whose `params` map holds printed scalars), return the
/// requested parameter if an archetype actually prints it. Returns the value +
/// the archetype id for provenance. None when no archetype prints this param
/// (the common case for opposed-check DVs — caller falls to T3).
pub fn archetype_param(archetypes: &[(String, Value)], param: &str) -> Option<(Value, String)> {
    for (id, a) in archetypes {
        if let Some(v) = a.pointer(&format!("/params/{param}")).cloned() {
            return Some((v, id.clone()));
        }
    }
    None
}
```

- [ ] **Step 4: Run, verify PASS.** **Step 5: Checkpoint** `cargo test -p trpg-runtime npc_synth` green.

> NOTE for executor: the persona→archetype *matching* (which archetypes are candidates for "a city guard") is an LLM/fit_tags step performed in Task 4's orchestrator using the onboarding pack's `RecommendedArchetype.fit_tags`; Task 3 is only the param read-off. Keep T2 honest: it is best-effort and whiffs for un-tabulated opposed DVs.

---

## Task 4: Hybrid orchestrator `ensure_npc_parameter` (T1→T2→T3) + persona resolution

**Files:** Modify `crates/trpg-runtime/src/npc_synth.rs` and `crates/trpg-runtime/src/lib.rs`; Test inline (orchestration logic with stub source/archetype/llm).

- [ ] **Step 1: Write the failing test** (ordering: source wins; else archetype; else persona-judge)

```rust
// PURE tier selection — no LlmClient stub (the trait is heavy #[async_trait]); the
// LLM-only T3 fallback is integration-verified, not unit-tested here.
#[test]
fn non_llm_tiers_pick_source_then_archetype_else_none() {
    // T1 source present -> source_backed
    let r1 = resolve_tiered_non_llm(Some(json!(70)), &[("guard".into(), json!({"params":{"x":50}}))], "x").unwrap();
    assert_eq!(r1.value, json!(70)); assert_eq!(r1.status, "source_backed");
    // no source, archetype prints x -> source_backed_archetype (+ archetype_id in provenance)
    let r2 = resolve_tiered_non_llm(None, &[("guard".into(), json!({"params":{"x":50}}))], "x").unwrap();
    assert_eq!(r2.value, json!(50)); assert_eq!(r2.status, "source_backed_archetype");
    assert_eq!(r2.provenance["archetype_id"], json!("guard"));
    // no source, archetype whiffs -> None (caller falls to T3 persona-judge)
    assert!(resolve_tiered_non_llm(None, &[("guard".into(), json!({"params":{}}))], "x").is_none());
}
```

- [ ] **Step 2: Run, verify FAIL.**

- [ ] **Step 3: Implement the pure tiered resolver** (the I/O — DB source fetch + archetype matching — is injected as already-resolved args, keeping this unit-testable; the async wiring is Task 5)

```rust
/// PURE T1/T2 selection (no LLM): T1 source value -> source_backed; else T2
/// archetype-printed param -> source_backed_archetype; else None (caller -> T3).
pub fn resolve_tiered_non_llm(source_value: Option<Value>, archetypes: &[(String, Value)], param: &str) -> Option<SynthesizedParam> {
    if let Some(v) = source_value {
        return Some(SynthesizedParam { value: v, status: "source_backed".into(),
            provenance: json!({"tier":"source","parameter":param}) });
    }
    if let Some((v, aid)) = archetype_param(archetypes, param) {
        return Some(SynthesizedParam { value: v, status: "source_backed_archetype".into(),
            provenance: json!({"tier":"archetype","parameter":param,"archetype_id":aid}) });
    }
    None
}

/// The full hybrid ladder over a SINGLE parameter: pure T1/T2 first, else T3
/// persona-judge (LLM). Ordering is the spec's T1>T2>T3 rule.
pub async fn resolve_param_tiered(
    source_value: Option<Value>, archetypes: &[(String, Value)], llm: &dyn LlmClient,
    persona: &NpcPersona, param: &str, check_context: &str, ruleset_id: &str,
) -> anyhow::Result<SynthesizedParam> {
    if let Some(p) = resolve_tiered_non_llm(source_value, archetypes, param) { return Ok(p); }
    synthesize_npc_parameter(llm, persona, param, check_context, ruleset_id).await
}
```

- [ ] **Step 4: Run, verify PASS.**

- [ ] **Step 5: Wire the async orchestrator in `lib.rs`** (no new test — integration-verified in Task 5/manual). Add to `RuntimeEngine`:

```rust
/// Ensure an NPC's parameter exists on its card: if absent, run the hybrid ladder
/// and write the result (provisional values bypass the strict materialization gate
/// by writing straight to sheet_json). Returns the value. Gated by
/// TRPG_NPC_PERSONA_SYNTHESIS (default ON); when off, returns None (fail-closed).
pub async fn ensure_npc_parameter(&self, session_id: &str, ruleset_id: &str, npc: &npc_synth::NpcPersona,
    bucket: &str, param: &str, check_context: &str) -> anyhow::Result<Option<serde_json::Value>> {
    if !npc_synth::persona_synthesis_enabled() { return Ok(None); }
    let service = trpg_params::RuntimeParameterService::new(self.db.clone());
    let mut p = match service.load_actor_parameters(session_id, &npc.actor_id).await? {
        Some(p) => p, None => return Ok(None), // card must exist (ensure_actor_parameters ran)
    };
    // already on the card? return it (per-parameter cache).
    if let Some(v) = p.sheet_json.pointer(&format!("/{bucket}/{param}")).cloned() { return Ok(Some(v)); }
    // T1: a source value may already be on the card from materialization (status/hp etc.) — none here -> None.
    let source_value = p.sheet_json.pointer(&format!("/source/{param}")).cloned();
    // T2: persona-matched archetypes from the onboarding pack (best-effort).
    // T2: archetype-stat retrieval has NO source index yet — RecommendedArchetype carries
    // fit_tags + option refs, NOT stat params. So T2 feeds empty and falls to T3. The T2 code
    // path (resolve_tiered_non_llm/archetype_param) stays + is unit-tested with synthetic data,
    // ready for a future bestiary/archetype-stat index. Honest per spec §5.
    let archetypes: Vec<(String, serde_json::Value)> = Vec::new();
    // RuntimeEngine has NO LlmClient field; build per-call from env like MaterializationService::from_env (trpg-material:62-65).
    let llm = match trpg_llm::LlmConfig::from_env().ok().and_then(|cfg| trpg_llm::OpenAiCompatibleClient::new(cfg).ok()) {
        Some(c) => c, None => return Ok(None), // no LLM configured -> fail-closed (no synthesis)
    };
    let synth = npc_synth::resolve_param_tiered(source_value, &archetypes, &llm, npc, param, check_context, ruleset_id).await?;
    npc_synth::write_synthesized_param(&mut p.sheet_json, bucket, param, &synth);
    service.upsert_actor_parameters(&p).await?;
    Ok(Some(synth.value))
}
```

Add the gate helper to `npc_synth.rs`:
```rust
/// Default ON (user decision 2026-06-09: NPCs must have usable cards). Off -> fail-closed null.
pub fn persona_synthesis_enabled() -> bool {
    std::env::var("TRPG_NPC_PERSONA_SYNTHESIS").ok()
        .map(|v| !matches!(v.to_ascii_lowercase().as_str(), "0"|"false"|"no"|"off"))
        .unwrap_or(true)
}
```
**LLM access (corrected — RuntimeEngine holds NO LlmClient, only `db`+`search`):** build per-call via `trpg_llm::LlmConfig::from_env().ok().and_then(|cfg| trpg_llm::OpenAiCompatibleClient::new(cfg).ok())` (mirror `MaterializationService::from_env`, trpg-material:62-65); `&llm` (concrete `OpenAiCompatibleClient`) coerces to `&dyn LlmClient`. **T2 archetypes feed empty** (`Vec::new()`) for now — `RecommendedArchetype` has fit_tags, not stat params, so there is no archetype-stat source to read; T2 falls to T3 (expected, honest per spec §5/§10.1). No `persona_archetypes` method needed.

- [ ] **Step 6: Checkpoint** — `cargo build -p trpg-runtime` clean; `cargo test -p trpg-runtime npc_synth` green.

---

## Task 5: Pre-resolution trigger — synthesize the NPC param before the contest reads it

**Files:** Modify `crates/trpg-cli/src/main.rs` (`run_turn_once`, the materialize→context window ~1054-1075) and `crates/trpg-runtime/src/lib.rs`. Test: integration (DB+LLM), verified manually.

`trpg-contest` is sync + has no LLM dep, and returns `Provisional` (→ `(None,None,None)`, contest.rs:305/336) when no source-backed DV. So the synthesis must run in `trpg-cli`/`trpg-runtime` (which hold the LLM) BEFORE contest resolves, writing the value onto the NPC card so the existing contest/`actor_hp_from_params` path reads it.

- [ ] **Step 1: Add a runtime entrypoint** `prepare_npc_for_check` in `lib.rs`:

```rust
/// Pre-resolution pass: for the NPC the current check targets, ensure the needed
/// parameter is on its card (hybrid). Called from the turn flow AFTER materialize,
/// BEFORE contest resolution. `param`/`bucket` derive from the check kind
/// (attack -> defense bucket=stats; stealth-opposed -> the opposing skill).
pub async fn prepare_npc_for_check(&self, session_id: &str, ruleset_id: &str, npc: &npc_synth::NpcPersona,
    bucket: &str, param: &str, check_context: &str) -> anyhow::Result<()> {
    let _ = self.ensure_npc_parameter(session_id, ruleset_id, npc, bucket, param, check_context).await?;
    Ok(())
}
```

- [ ] **Step 2: Call it in `run_turn_once`** (`crates/trpg-cli/src/main.rs`, after `try_materialize_turn` at ~1055, before `prepare_turn_context` at ~1075). Resolve the current-scene NPC persona via the existing pattern (Task 8 generalizes to per-NPC; Phase 1 uses the single opposition + its scene persona):

```rust
// after materialization, before prepare_turn_context
if let Some(npc) = runtime.current_check_npc_persona(&request, &state, user_input).await {
    let (bucket, param) = runtime.check_param_need(user_input); // e.g. ("stats","defense") or ("skills","perception")
    let _ = runtime.prepare_npc_for_check(&request.session_id, &request.ruleset_id, &npc, &bucket, &param, user_input).await;
}
```

- [ ] **Step 3: Implement `current_check_npc_persona` + `check_param_need`** in `lib.rs`. `current_check_npc_persona` reuses the `scene_node_to_blocks` id→persona resolution (lib.rs:2036-2048): load the module graph, find the active scene's `referenced_npc_ids`, build an `NpcPersona { actor_id: "npc.opposition" (Phase 1), name, prose }` from `graph.npcs`. `check_param_need` maps the semantic action kind (NOT raw keywords — read `SemanticIntentResult.primary_action_kind`) to `(bucket, param)`: attack→`("stats","defense")`, theft/stealth-opposed→`("skills", <opposing skill>)`. Return `None` if no scene NPC.

- [ ] **Step 4: Checkpoint** — `cargo build --workspace` clean. Manual integration verify (DB :54347): create a D&D char + a 1-NPC scene session, run a `turn` whose input pickpockets a guard, confirm the NPC card's `sheet_json` gains a provisional `defense`/skill with `npc_param_provenance.<param>.tier == "persona_judge"` and `status == "provisional"`, and the check resolves (not `(None,None,None)`).

---

## Task 6: NpcCard search light-up + GM 3-branch prompt + gate doc

**Files:** Modify `crates/trpg-material/src/lib.rs` (opposition demand ~144-156), `crates/trpg-cli/src/main.rs` (GM prompt + `.env` template).

- [ ] **Step 1 (T1 light-up):** In `demands_from_runtime_context` opposition demand (`trpg-material/src/lib.rs:144-156`), set the demand metadata `material_target_kind="npc_stat_block"` so `material_kind_from_rule_kind` (~903-914) routes to `NpcStatBlock` → the `NpcStatblock` search plan (prefers module NPC cards) instead of generic `ActorProfile`. Exact edit: add to the demand's `metadata` json `"material_target_kind": "npc_stat_block"`. (Honest: extractor schema is identical to ActorProfile — this only changes the search query plan to prefer module cards.)

- [ ] **Step 2 (GM 3-branch):** In the GM resolve prompt (`trpg-cli/src/main.rs`, the "ITEM POLICY" block ~1863), add an "NPC POLICY" mirroring §2b for NPCs: ambiguous/no-persona → ask; reasonable → use module card/archetype; over-power/genre-mismatch → push back + may force flagged. No code logic — prompt text only.

- [ ] **Step 3 (gate doc):** Add `TRPG_NPC_PERSONA_SYNTHESIS=true` to the `.env` template emitted in `main.rs` (grep the existing `TRPG_*` env block ~591), with a comment: "NPC persona-judge synthesis (flagged provisional, drives resolution). Set false for strict fail-closed."

- [ ] **Step 4: Checkpoint** — `cargo build --workspace` clean; `./target/debug/trpg --help` runs. Phase 1 done.

---

# PHASE 2

## Task 7: Structured `target_search_keys` on the classifier (mirror object_search_keys)

**Files:** Modify `crates/trpg-model/src/lib.rs` (SemanticIntentResult ~5198), `crates/trpg-semantics/src/lib.rs` (schema 48, system prompt 71, parser 184/188). Test: `crates/trpg-semantics` unit test on the parser.

- [ ] **Step 1: Write the failing test**

```rust
// crates/trpg-semantics tests
#[test]
fn parser_reads_target_search_keys() {
    let v = serde_json::json!({"target_refs":["the city guard"], "target_search_keys":["guard"], "object_refs":[], "ability_refs":[]});
    let r = semantic_result_from_json(&sample_req(), v, "test");
    assert_eq!(r.target_search_keys, vec!["guard".to_string()]);
}
```

- [ ] **Step 2: Run, verify FAIL** (field missing).
- [ ] **Step 3: Implement** — add `#[serde(default)] pub target_search_keys: Vec<String>,` to `SemanticIntentResult` (trpg-model:5198, right after `target_refs`); in `trpg-semantics/src/lib.rs` add `"target_search_keys": []` to the schema string (line 48), append a "For EACH target_refs item emit target_search_keys[i] = the minimal distinguishing token (a name/role/proper-noun; drop generic words)" sentence to the system prompt (line 71, mirroring object_search_keys), and in the parser add `let target_search_keys = string_vec(value.get("target_search_keys"));` (line 184) + the field in the struct construction (line 188). Bump the classifier tag to `llm_semantic_classifier_v1_10` if behavior changes.
- [ ] **Step 4: Run, verify PASS.** **Step 5: Checkpoint** `cargo test -p trpg-semantics` green; `cargo build --workspace` clean (new field is `#[serde(default)]` → back-compat).

---

## Task 8: Per-NPC resolution + per-NPC ensure (replace single warmup; keep keyword fallback)

**Files:** Modify `crates/trpg-runtime/src/lib.rs` (ensure trigger 1537-1539; add `resolve_scene_npcs`).

- [ ] **Step 1: Implement `resolve_scene_npcs`** — load the module graph, take the active scene's `referenced_npc_ids`, match against `graph.npcs` by id (the scene_node_to_blocks:2036-2048 pattern), and for the current check use `SemanticIntentResult.target_refs` + `target_search_keys` to pick WHICH npc the action targets; return `Vec<NpcPersona>` with per-NPC `actor_id = format!("npc.{}", safe_id(npc_id))`.

- [ ] **Step 2: Replace the ensure trigger** at `lib.rs:1537-1539`. Current:
```rust
if active_frame_exists || current_input.map(mentions_runtime_npc).unwrap_or(false) {
    let _ = service.ensure_actor_parameters(&request.session_id, &request.ruleset_id, "npc.opposition", ActorKind::Npc, world_tick).await;
}
```
New: ensure per resolved scene NPC, AND keep the single `npc.opposition` ensure as a fallback when resolution yields none (so frameless social scenes still get a card — spec §6 migration order):
```rust
let scene_npcs = self.resolve_scene_npcs(&request, current_input).await;
if scene_npcs.is_empty() {
    if active_frame_exists || current_input.map(mentions_runtime_npc).unwrap_or(false) {
        let _ = service.ensure_actor_parameters(&request.session_id, &request.ruleset_id, "npc.opposition", ActorKind::Npc, world_tick).await;
    }
} else {
    for npc in &scene_npcs {
        let _ = service.ensure_actor_parameters(&request.session_id, &request.ruleset_id, &npc.actor_id, ActorKind::Npc, world_tick).await;
    }
}
```

- [ ] **Step 3: Checkpoint** — `cargo build -p trpg-runtime` clean. Integration: a scene with two NPCs (guard + civilian) → two distinct `npc.<id>` param rows created.

---

## Task 9: De-collapse the combat frame (multi-defender)

**Files:** Modify `crates/trpg-combat/src/lib.rs` (`create_frame` 619-648; sites 1049/1058/1290/1392/1527).

- [ ] **Step 1:** Change `create_frame` to accept a resolved `Vec<ActorRef>` of opposition actors (thread via `ConflictTurnInput` or resolve from the active frame's participants). Replace the single `npc_actor_id = "npc.opposition"` (L627) + the fixed 2-element `initiative_order`/`participants`/`sides.actor_ids` (L637-648) with: keep the PC slot; push one `InitiativeSlot` + `CombatParticipantState` per opposition actor (each with its own `ensure_actor_parameters`-hydrated HP); `sides[opposition].actor_ids` collects all NPC ids. Keep a single `npc.opposition` synthesized member only when the resolved set is empty (fallback).
- [ ] **Step 2:** Update the other combat sites to derive from the frame's participants rather than the literal: `1058` `defender_actor` fallback, `1290` `default_npc_drive_for_mode`, `1392`, `1527` `default_tactic_palette_for_mode`, `1049` id→side tuple. For these defaults, prefer the frame's first opposition participant; only use `"npc.opposition"` when the frame has no resolved opposition.
- [ ] **Step 3: Checkpoint** — `cargo build -p trpg-combat` clean; existing combat tests green (`cargo test -p trpg-combat`).

---

## Task 10: De-collapse mechanics/contest/object defaults + two-HP-store per-NPC alignment

**Files:** Modify `crates/trpg-mechanics/src/lib.rs` (334/530/579, ensure_actor_state 596-625), `crates/trpg-contest/src/lib.rs` (defender_for_contract 357-361), `crates/trpg-object/src/lib.rs` (190/196/749).

- [ ] **Step 1:** `record_attack` (mechanics:530), `apply_effect_roll` (334), `make_effect_roll_check` (579): the `target` resolution must come from the resolved per-NPC defender (threaded from the contract's `target_actor`, set upstream by Task 8's resolution) — keep `"npc.opposition"` only as terminal fallback when truly unresolved.
- [ ] **Step 2:** `defender_for_contract` (contest:357-361) already prefers `contract.target_actor.actor_id` then `actor_snapshot_ids` matching `npc.*`; ensure the upstream sets `contract.target_actor` to the resolved per-NPC id so the `npc.opposition` fallback (L360) is reached only when unresolved. (No structural change to contest — it already threads a real id first.)
- [ ] **Step 3 (two-HP-store alignment):** Confirm `ensure_actor_state` (mechanics:596-625) + `actor_hp_from_params` (590-594) are keyed purely on `(session_id, actor_id)` — they are. The invariant: every site that writes `runtime_actor_parameters` and `actor_mechanical_states` for an NPC must use the SAME resolved per-NPC `actor_id`. Add an integration assertion: after a per-NPC attack, `actor_mechanical_states` and `runtime_actor_parameters` rows for `npc.<id>` agree on HP (the bridge at L604 lines up).
- [ ] **Step 4:** `trpg-object` 190/196/749 (`infer_target_actor` + weapon ensure): resolve target from the real ref, `"npc.opposition"` terminal fallback only.
- [ ] **Step 5: Checkpoint** — `cargo build --workspace` clean; `cargo test -p trpg-mechanics -p trpg-contest -p trpg-object` green.

---

## Task 11: Retire `mentions_runtime_npc` (LAST — only after Task 8 proven)

**Files:** Modify `crates/trpg-runtime/src/lib.rs` (1723 + the call at 1537).

- [ ] **Step 1:** Only after Task 8's `resolve_scene_npcs` is integration-proven to reliably create NPC cards in BOTH combat and frameless social scenes, remove the `mentions_runtime_npc` keyword path: drop the `|| current_input.map(mentions_runtime_npc)` clause and the `fn mentions_runtime_npc` (1723). The semantic trigger is now scene-graph presence + `target_refs`. Keep `active_frame_exists` as the combat trigger.
- [ ] **Step 2: Checkpoint** — `cargo build --workspace` clean; integration: a frameless social scene that mentions an NPC still creates the per-NPC card via `resolve_scene_npcs` (NOT via the deleted keyword gate). If it does NOT, DO NOT delete the keyword gate yet — `resolve_scene_npcs` needs strengthening first.

---

## Self-Review notes (author)
- Spec coverage: T1/T2/T3 (Tasks 1-4), T3-drives-resolution + pre-pass (Task 5), strict-bypass write to sheet_json + provenance + no-downgrade upgrade (Task 2), NpcCard light-up + GM 3-branch + gate default-on (Task 6), structured target_refs (Task 7), de-collapse 22 sites (Tasks 8-10), two-HP-store alignment (Task 10), keyword retirement sequenced last (Task 11). Invariants in spec §7 each map to a task assertion.
- Consistent names across tasks: `NpcPersona`, `SynthesizedParam`, `synthesize_npc_parameter`, `write_synthesized_param`, `archetype_param`, `resolve_param_tiered`, `ensure_npc_parameter`, `prepare_npc_for_check`, `resolve_scene_npcs`, `persona_synthesis_enabled`, `tier_rank`. Statuses: `source_backed` / `source_backed_archetype` / `provisional`.
- Honesty carried from review: T2 mostly whiffs for opposed-check DVs (Task 3 note); contest stays LLM-free (Task 5 runs synth in the caller); keyword retirement gated on semantic proof (Task 11).

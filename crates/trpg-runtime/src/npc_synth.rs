//! NPC card lazy synthesis: per-(NPC × parameter) value, hybrid T1 source > T2
//! archetype > T3 persona-judge. T3 is LLM-backed, ALWAYS flagged provisional +
//! audited, drives resolution, and is upgradeable by a later source/archetype hit.
//! Provenance lives in `sheet_json` (NOT mechanical_profile, which is reprojected).
use serde_json::{json, Value};
use trpg_llm::LlmClient;
use trpg_model::ChatMessage;

/// Who the NPC is — the persona grounding T2/T3. Sourced from module `graph.npcs[id]`
/// (name + body/summary), or GM-supplied for an improvised NPC.
#[derive(Debug, Clone)]
pub struct NpcPersona { pub actor_id: String, pub name: String, pub prose: String }

/// One synthesized parameter value + provenance (for audit + upgrade).
#[derive(Debug, Clone)]
pub struct SynthesizedParam { pub value: Value, pub status: String, pub provenance: Value }

/// PURE: build the T3 prompt messages, grounded in persona + the triggering check.
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

/// T3 thin async wrapper: build messages -> LLM (`complete_json` takes Vec<ChatMessage>) -> parse.
pub async fn synthesize_npc_parameter(llm: &dyn LlmClient, persona: &NpcPersona, parameter_id: &str, check_context: &str, ruleset_id: &str) -> anyhow::Result<SynthesizedParam> {
    let resp = llm.complete_json(build_synthesis_messages(persona, parameter_id, check_context, ruleset_id), 0.4).await?;
    parse_synthesis_response(&resp, persona, parameter_id, check_context)
}

/// Tier ranking for the no-downgrade rule (higher wins): T1 source > T2 archetype > T3 provisional.
fn tier_rank(status: &str) -> u8 {
    match status { "source_backed" => 3, "source_backed_archetype" => 2, "provisional" => 1, _ => 0 }
}

/// Write a synthesized parameter onto the NPC sheet: route the value into the named
/// bucket (stats/skills/resources) and record status+provenance under
/// `sheet_json.npc_param_provenance.<param>`. UPGRADE-ONLY: a higher-tier existing
/// value is kept (never downgraded). Provenance lives in `sheet_json` so
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

/// T2 read-off: given candidate archetype(s) already matched to this persona (each
/// `(archetype_id, json)` whose `params` map holds printed scalars), return the
/// requested parameter value + the archetype id (for provenance) if an archetype
/// actually prints it. Returns None when no archetype prints this param — the
/// common case for opposed-check DVs, where the caller falls to T3 persona-judge.
pub fn archetype_param(archetypes: &[(String, Value)], param: &str) -> Option<(Value, String)> {
    for (id, a) in archetypes {
        if let Some(v) = a.pointer(&format!("/params/{param}")).cloned() {
            return Some((v, id.clone()));
        }
    }
    None
}

/// PURE T1/T2 selection (no LLM): T1 source value -> source_backed; else T2 archetype-printed
/// param -> source_backed_archetype; else None (caller -> T3).
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

/// Full hybrid ladder over ONE parameter: pure T1/T2 first, else T3 persona-judge (LLM). T1>T2>T3.
pub async fn resolve_param_tiered(
    source_value: Option<Value>, archetypes: &[(String, Value)], llm: &dyn LlmClient,
    persona: &NpcPersona, param: &str, check_context: &str, ruleset_id: &str,
) -> anyhow::Result<SynthesizedParam> {
    if let Some(p) = resolve_tiered_non_llm(source_value, archetypes, param) { return Ok(p); }
    synthesize_npc_parameter(llm, persona, param, check_context, ruleset_id).await
}

/// Gate: default ON (user decision — NPCs must have usable cards). Off -> fail-closed null.
pub fn persona_synthesis_enabled() -> bool {
    std::env::var("TRPG_NPC_PERSONA_SYNTHESIS").ok()
        .map(|v| !matches!(v.to_ascii_lowercase().as_str(), "0"|"false"|"no"|"off"))
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
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
        assert!(parse_synthesis_response(&json!({"reason":"x"}), &persona, "p", "c").is_err());
    }

    #[test]
    fn build_messages_grounds_in_persona_and_param() {
        let persona = NpcPersona { actor_id: "npc.guard_1".into(), name: "Guard".into(), prose: "trained guard".into() };
        let msgs = build_synthesis_messages(&persona, "anti_theft_dv", "pickpocket attempt", "call_of_cthulhu_7e");
        let joined = msgs.iter().map(|m| m.content.clone()).collect::<Vec<_>>().join("\n");
        assert!(joined.contains("anti_theft_dv") && joined.contains("trained guard") && joined.contains("call_of_cthulhu_7e"));
    }

    #[test]
    fn provisional_write_lands_in_sheet_and_survives_refresh() {
        let mut sheet = json!({"stats": {}, "skills": {}});
        let p = SynthesizedParam { value: json!(65), status: "provisional".into(),
            provenance: json!({"tier":"persona_judge","parameter":"anti_theft_dv"}) };
        write_synthesized_param(&mut sheet, "skills", "anti_theft_dv", &p);
        assert_eq!(sheet["skills"]["anti_theft_dv"], json!(65));
        assert_eq!(sheet["npc_param_provenance"]["anti_theft_dv"]["tier"], json!("persona_judge"));
        assert_eq!(sheet["npc_param_provenance"]["anti_theft_dv"]["status"], json!("provisional"));
        // a later SOURCE value upgrades (overwrites) and records source_backed
        let src = SynthesizedParam { value: json!(70), status: "source_backed".into(), provenance: json!({"tier":"source"}) };
        write_synthesized_param(&mut sheet, "skills", "anti_theft_dv", &src);
        assert_eq!(sheet["skills"]["anti_theft_dv"], json!(70));
        assert_eq!(sheet["npc_param_provenance"]["anti_theft_dv"]["status"], json!("source_backed"));
        // a subsequent PROVISIONAL must NOT downgrade the source_backed value
        let prov_again = SynthesizedParam { value: json!(50), status: "provisional".into(), provenance: json!({"tier":"persona_judge"}) };
        write_synthesized_param(&mut sheet, "skills", "anti_theft_dv", &prov_again);
        assert_eq!(sheet["skills"]["anti_theft_dv"], json!(70), "source_backed not downgraded");
        assert_eq!(sheet["npc_param_provenance"]["anti_theft_dv"]["status"], json!("source_backed"));
    }

    #[test]
    fn non_llm_tiers_pick_source_then_archetype_else_none() {
        let r1 = resolve_tiered_non_llm(Some(json!(70)), &[("guard".into(), json!({"params":{"x":50}}))], "x").unwrap();
        assert_eq!(r1.value, json!(70)); assert_eq!(r1.status, "source_backed");
        let r2 = resolve_tiered_non_llm(None, &[("guard".into(), json!({"params":{"x":50}}))], "x").unwrap();
        assert_eq!(r2.value, json!(50)); assert_eq!(r2.status, "source_backed_archetype");
        assert_eq!(r2.provenance["archetype_id"], json!("guard"));
        assert!(resolve_tiered_non_llm(None, &[("guard".into(), json!({"params":{}}))], "x").is_none());
    }

    #[test]
    fn archetype_tier_returns_value_when_printed_else_none() {
        let archetypes: Vec<(String, Value)> = vec![("guard".into(), json!({"fit_tags":["combat"],"params":{"hp":12}}))];
        // a param the archetype does NOT print -> None (caller falls to T3)
        assert!(archetype_param(&archetypes, "anti_theft_dv").is_none());
        // a printed scalar IS found, with the archetype id for provenance
        let got = archetype_param(&archetypes, "hp").unwrap();
        assert_eq!(got.0, json!(12));
        assert_eq!(got.1, "guard");
        // empty candidate set -> None
        assert!(archetype_param(&[], "hp").is_none());
    }
}

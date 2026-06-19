//! P5.6 — Director brief-packet GM-tail rendering (content-safe).
//!
//! [`render_director_packet_block`] turns a typed [`DirectorPlan`] into the
//! `[director_packet]…[/director_packet]` block appended to the GM-only `[gm][BP3]` user
//! message (mirroring `npc_guidance_block`). It is a **GM-tail string that bypasses the
//! ContextFilter typed-block surface** (codex fold #3), so it owns its OWN content-safety
//! guard: the block carries ONLY ids / enums / short generic structural strings — NEVER
//! secret fact bodies or free prose.
//!
//! ## Content-safety contract (codex fold #2/#3)
//! - Reveals appear as a `fact_id` LIST, never fact bodies.
//! - `beat_kind` / `dramatic_function` / `desired_change` are structural snake_case tokens
//!   produced by the pure core (`select.rs`), already asserted whitespace-free there.
//! - Thread / actor / candidate references are ids only.
//! - No `StoryState` prose, no NPC dialogue, no GM-only narrative is ever interpolated.
//!
//! The block is intentionally machine-terse: it is GM decision scaffolding, not narration.

use trpg_model::{BeatKind, DirectorPlan};

/// The structural token for a [`BeatKind`] (its serde snake_case name). Used as the packet's
/// signature token in the wiring proof. Pure mapping, allocation-free.
fn beat_kind_token(kind: BeatKind) -> &'static str {
    match kind {
        BeatKind::Respond => "respond",
        BeatKind::Reveal => "reveal",
        BeatKind::Complicate => "complicate",
        BeatKind::Consequence => "consequence",
        BeatKind::Choice => "choice",
        BeatKind::Reaction => "reaction",
        BeatKind::Callback => "callback",
        BeatKind::Foreshadow => "foreshadow",
        BeatKind::Escalate => "escalate",
        BeatKind::Relief => "relief",
        BeatKind::Payoff => "payoff",
        BeatKind::Transition => "transition",
    }
}

/// Render the GM-only director packet block from a typed plan. Content-safe: emits ONLY ids,
/// enum tokens, and short structural strings — never secret prose. Returns an empty string
/// for a no-op plan (default `DirectorPlan`) so the caller writes no block.
///
/// Layout (one `key: value` per line, all values ids / tokens):
/// ```text
/// [director_packet]
/// beat_kind: <token>
/// dramatic_function: <token>
/// desired_change: <token>
/// primary_thread: <thread_id>
/// secondary_threads: <id>, <id>
/// focus_actors: <npc_id>, <npc_id>
/// reveal_candidate_fact_ids: <fact_id>, <fact_id>
/// spotlight_target: <player_id>
/// world_query: <intent_token> [<fact_id>, ...]
/// [/director_packet]
/// ```
pub fn render_director_packet_block(plan: &DirectorPlan) -> String {
    // A default/no-op plan renders nothing — the caller appends no block (byte-stable).
    if *plan == DirectorPlan::default() {
        return String::new();
    }
    let mut out = String::from("[director_packet]\n");
    out.push_str("beat_kind: ");
    out.push_str(beat_kind_token(plan.beat_kind));
    out.push('\n');
    if !plan.dramatic_function.is_empty() {
        out.push_str("dramatic_function: ");
        out.push_str(&plan.dramatic_function);
        out.push('\n');
    }
    if !plan.desired_change.is_empty() {
        out.push_str("desired_change: ");
        out.push_str(&plan.desired_change);
        out.push('\n');
    }
    if let Some(primary) = &plan.primary_thread_id {
        out.push_str("primary_thread: ");
        out.push_str(primary);
        out.push('\n');
    }
    if !plan.secondary_thread_ids.is_empty() {
        out.push_str("secondary_threads: ");
        out.push_str(&plan.secondary_thread_ids.join(", "));
        out.push('\n');
    }
    if !plan.focus_actor_ids.is_empty() {
        out.push_str("focus_actors: ");
        out.push_str(&plan.focus_actor_ids.join(", "));
        out.push('\n');
    }
    // Reveals are ids ONLY (codex fold #2): a fact_id list, never fact bodies.
    if !plan.reveal_candidate_fact_ids.is_empty() {
        out.push_str("reveal_candidate_fact_ids: ");
        out.push_str(&plan.reveal_candidate_fact_ids.join(", "));
        out.push('\n');
    }
    if !plan.open_player_affordances.is_empty() {
        out.push_str("open_player_affordances: ");
        out.push_str(&plan.open_player_affordances.join(", "));
        out.push('\n');
    }
    if let Some(target) = &plan.spotlight_target {
        out.push_str("spotlight_target: ");
        out.push_str(target);
        out.push('\n');
    }
    if let Some(q) = &plan.world_query {
        out.push_str("world_query: ");
        out.push_str(world_query_token(q.intent));
        if !q.constraint_fact_ids.is_empty() {
            out.push_str(" [");
            out.push_str(&q.constraint_fact_ids.join(", "));
            out.push(']');
        }
        out.push('\n');
    }
    out.push_str("[/director_packet]");
    out
}

/// Structural token for a [`trpg_model::WorldQueryIntent`].
fn world_query_token(intent: trpg_model::WorldQueryIntent) -> &'static str {
    use trpg_model::WorldQueryIntent::*;
    match intent {
        ExistsKnowingNpc => "exists_knowing_npc",
        ExistsMotivatedNpc => "exists_motivated_npc",
        ExistsAvailableThread => "exists_available_thread",
        Generic => "generic",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::{WorldCandidateRef, WorldQuery, WorldQueryIntent};

    fn plan_with_reveal() -> DirectorPlan {
        DirectorPlan {
            beat_kind: BeatKind::Reveal,
            primary_thread_id: Some("thread_betrayal".into()),
            secondary_thread_ids: vec!["thread_debt".into()],
            focus_actor_ids: vec!["npc_kane".into()],
            reveal_candidate_fact_ids: vec!["fact_secret_door".into(), "fact_hidden_ally".into()],
            dramatic_function: "advance_primary_thread".into(),
            desired_change: "shift_situation".into(),
            spotlight_target: Some("player_2".into()),
            selected_world_candidates: vec![WorldCandidateRef {
                npc_id: "npc_kane".into(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn renders_beat_kind_signature_token() {
        let block = render_director_packet_block(&plan_with_reveal());
        assert!(block.starts_with("[director_packet]"));
        assert!(block.contains("beat_kind: reveal"));
        assert!(block.ends_with("[/director_packet]"));
    }

    #[test]
    fn default_plan_renders_empty_string() {
        // No-op plan ⇒ empty render ⇒ caller writes no block (byte-stable OFF path support).
        assert!(render_director_packet_block(&DirectorPlan::default()).is_empty());
    }

    // ── Content safety (codex fold #2/#3): ids / enums / short tokens ONLY, never prose. ──
    #[test]
    fn reveal_appears_as_fact_id_list_not_bodies() {
        // The reveal field carries fact_IDS; we assert exactly those ids appear and that NO
        // secret body text could leak (the renderer never has access to fact bodies — it only
        // ever sees ids — so the block can only contain the ids we feed it).
        let block = render_director_packet_block(&plan_with_reveal());
        assert!(block.contains("reveal_candidate_fact_ids: fact_secret_door, fact_hidden_ally"));
        // The ids are the literal strings — there is no "body"/prose form anywhere.
        assert!(!block.to_lowercase().contains("secret door is behind"));
    }

    #[test]
    fn block_contains_no_freeform_prose_lines() {
        // Every non-delimiter line is `key: <id/token list>`. Assert no line carries a
        // sentence-like body (a crude guard: no line ends with sentence punctuation, no CJK).
        let block = render_director_packet_block(&plan_with_reveal());
        for line in block.lines() {
            assert!(
                !line.ends_with('.') && !line.ends_with('。') && !line.ends_with('!'),
                "packet line looks like prose, not structural: {line:?}"
            );
            assert!(
                line.chars().all(|c| c.is_ascii()),
                "packet must be ASCII ids/tokens only, found non-ASCII: {line:?}"
            );
        }
    }

    #[test]
    fn world_query_renders_intent_token_and_constraint_ids() {
        let plan = DirectorPlan {
            beat_kind: BeatKind::Respond,
            world_query: Some(WorldQuery {
                intent: WorldQueryIntent::ExistsKnowingNpc,
                question: "no_candidate_in_pool".into(),
                constraint_fact_ids: vec!["fact_x".into()],
            }),
            reveal_candidate_fact_ids: vec!["fact_x".into()],
            ..Default::default()
        };
        let block = render_director_packet_block(&plan);
        assert!(block.contains("world_query: exists_knowing_npc [fact_x]"));
        // The free-form `question` field is NOT rendered (it could be prose); only the token.
        assert!(!block.contains("no_candidate_in_pool"));
    }
}

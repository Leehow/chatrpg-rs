//! Typed TurnDocument AST (TRPG_TurnDocument_Protocol_v1 §四/§五, Phase 1-2 minimal).
//!
//! The `[tag]…[/tag]` wire format is an LLM-friendly transport, NOT the data model. After parse
//! it becomes a typed `TurnDocument` of `TurnBlock`s, which every downstream consumer
//! (post-processing, storage, context, UI) reads instead of re-parsing the raw string.
//!
//! Dual-route output (protocol §二十一):
//!   - `player_text()` = deterministic concatenation of player-visible blocks
//!     (narration / dialogue / roll / system / choice).
//!   - internal route = `hide_blocks()` (each authority=Proposal, NEVER auto-committed) +
//!     `meta_blocks()` (audit). `hide`/`meta` content NEVER appears in `player_text()`.
//!
//! A.0 correction (protocol appendix A.0): `[system]` is **player-visible** in-flow process /
//! prompt text. The internal out-of-game meta tag is `[meta]`, NOT `[system]`. The strip set is
//! {meta, hide}; the keep set is {narration, dialogue, roll, system, choice}.
//!
//! Phase scope: pragmatic. We carry kind / content / player-visibility / authority. We do NOT
//! yet model the full AudienceSet / BlockRefs / DB schema (deferred to the director-layer build).

/// Whether a block's content reaches the player text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Audience {
    /// Player-visible (narration / dialogue / roll / system / choice).
    Player,
    /// Internal only (hide / meta) — routed to canon-proposal / audit sinks, never player_text.
    Internal,
}

/// Whether the block has become a fact. LLM-emitted `hide` is only ever a `Proposal`
/// (protocol §五.3 / appendix A.1): the parser must never mint `CanonicalReference`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockAuthority {
    /// Display-only; carries no state authority.
    PresentationOnly,
    /// May become fact but unvalidated — must pass Rust validation before commit.
    Proposal,
}

/// The seven v1 wire kinds plus an extension escape hatch (protocol §二/§四).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TurnBlockKind {
    Narration,
    Dialogue,
    Roll,
    System,
    Choice,
    /// `[hide]` — in-fiction event the player did not perceive. Always a Proposal.
    HiddenEvent,
    /// `[meta]` — internal out-of-game note (decision summary / audit / director). Player-invisible.
    InternalMeta,
    /// Unknown tag → ExtensionBlock; default no state semantics (protocol §十一/§十七).
    /// Reserved for the plugin BlockRegistry (Phase 6); not yet emitted by the v1 parser.
    #[allow(dead_code)]
    Extension {
        tag: String,
    },
}

impl TurnBlockKind {
    fn audience(&self) -> Audience {
        match self {
            TurnBlockKind::Narration
            | TurnBlockKind::Dialogue
            | TurnBlockKind::Roll
            | TurnBlockKind::System
            | TurnBlockKind::Choice => Audience::Player,
            TurnBlockKind::HiddenEvent | TurnBlockKind::InternalMeta => Audience::Internal,
            // Unknown extension tags default to internal (fail-closed: never leak unknown
            // content into player text without an explicit player-visible kind).
            TurnBlockKind::Extension { .. } => Audience::Internal,
        }
    }

    fn default_authority(&self) -> BlockAuthority {
        match self {
            // hide is the only LLM-emitted kind that can become fact → Proposal (never auto-commit).
            TurnBlockKind::HiddenEvent => BlockAuthority::Proposal,
            _ => BlockAuthority::PresentationOnly,
        }
    }
}

/// One typed block in the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TurnBlock {
    pub kind: TurnBlockKind,
    /// The block's player-or-internal facing content (wire wrapper already removed).
    pub content: String,
    pub audience: Audience,
    pub authority: BlockAuthority,
}

impl TurnBlock {
    pub(crate) fn new(kind: TurnBlockKind, content: String) -> Self {
        let audience = kind.audience();
        let authority = kind.default_authority();
        TurnBlock {
            kind,
            content,
            audience,
            authority,
        }
    }

    pub(crate) fn is_player_visible(&self) -> bool {
        self.audience == Audience::Player
    }
}

/// Parsed turn output: typed blocks + dual-route accessors.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TurnDocument {
    pub blocks: Vec<TurnBlock>,
    /// Count of empty `[roll]` wrappers unwrapped to Narration (Q-5 repair).
    pub empty_rolls_unwrapped: usize,
}

impl TurnDocument {
    /// Deterministic player-visible text (protocol §二十 test 14): concatenation of all
    /// player-visible block contents, joined by a blank line, trimmed. `hide`/`meta` excluded.
    pub(crate) fn player_text(&self) -> String {
        let parts: Vec<&str> = self
            .blocks
            .iter()
            .filter(|b| b.is_player_visible())
            .map(|b| b.content.as_str())
            .filter(|c| !c.trim().is_empty())
            .collect();
        parts.join("\n\n").trim().to_string()
    }

    /// Internal route: `hide` blocks (each authority=Proposal) for canon-proposal validation.
    pub(crate) fn hide_blocks(&self) -> Vec<&TurnBlock> {
        self.blocks
            .iter()
            .filter(|b| b.kind == TurnBlockKind::HiddenEvent)
            .collect()
    }

    /// Internal route: `meta` blocks for audit / Flight Recorder.
    pub(crate) fn meta_blocks(&self) -> Vec<&TurnBlock> {
        self.blocks
            .iter()
            .filter(|b| b.kind == TurnBlockKind::InternalMeta)
            .collect()
    }

    /// True iff the rebuilt player text differs from the original wire string.
    /// (Used by tests + available to callers; the prod call site uses the
    /// `StrippedNarration::changed` adapter.)
    #[allow(dead_code)]
    pub(crate) fn changed(&self, original: &str) -> bool {
        self.player_text() != original
    }
}

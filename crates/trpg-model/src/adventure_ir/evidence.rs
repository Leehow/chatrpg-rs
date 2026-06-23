//! EV-2 `exact_evidence_projector_v1` — the Progression Evidence Layer's **core
//! types** (GPT Pro design `GPTpro-progression-evidence-layer.md` §1/§4/§5).
//!
//! Design law honored here:
//! - **`LLM output ∩ ProgressSignal = ∅`** — these types model only *admitted
//!   evidence*, never a progress conclusion. An [`AcceptedEvidence`] is the *input*
//!   to a guard, never an `ObjectiveCompleted`/`SceneUnlocked` (those stay
//!   engine-private, see [`super::ProgressSignal`]).
//! - **source-grounded** — every [`EvidenceAtomSpec`] and every [`AcceptedEvidence`]
//!   carries ≥1 [`SourceRef`]; an atom with no source ref is structurally
//!   impossible to *accept* (the builder/projector enforce it).
//! - **stable identity** — [`AtomId`] is a content hash of
//!   `(module_digest, source_span, kind, bound_refs)` so the same authored atom
//!   gets the same id across runs/sessions (replay-safe).
//!
//! These types are **additive** and unreferenced by the live pipeline until the
//! flag-gated projector (`exact_evidence_projector_v1`) wires them in
//! (`trpg-runtime::evidence_projection`). The engine does NOT consume the ledger
//! yet — that is EV-6 (`witnessed_progression_apply_v1`).

use crate::adventure_ir::ProgressRole;
use crate::SourceRef;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The finite, closed two-axis evidence vocabulary (design §Q2 / §5.1). A small
/// enum that classifies *what kind of observable thing* an authored atom is —
/// distinct from the open-ended free-text payload it would replace. Paired with a
/// module-source-constrained [`EvidenceAtomSpec`] it forms the "二轴词表".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    FactLearned,
    FactRevealed,
    ActionResolved,
    StateEstablished,
    EntityEncountered,
    LocationEntered,
    ChoiceCommitted,
    RelationshipChanged,
    ResourceChanged,
}

impl EvidenceKind {
    /// Stable token (also the hash input for [`AtomId`]).
    pub fn as_str(self) -> &'static str {
        match self {
            EvidenceKind::FactLearned => "fact_learned",
            EvidenceKind::FactRevealed => "fact_revealed",
            EvidenceKind::ActionResolved => "action_resolved",
            EvidenceKind::StateEstablished => "state_established",
            EvidenceKind::EntityEncountered => "entity_encountered",
            EvidenceKind::LocationEntered => "location_entered",
            EvidenceKind::ChoiceCommitted => "choice_committed",
            EvidenceKind::RelationshipChanged => "relationship_changed",
            EvidenceKind::ResourceChanged => "resource_changed",
        }
    }
}

/// Stable content-addressed identity for an authored evidence atom:
/// `atom:<sha256(module_digest | source_span | kind | bound_refs)>`. Same authored
/// atom ⇒ same id across runs/sessions (replay-safe; design §Q2 "ID 稳定").
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AtomId(pub String);

impl AtomId {
    /// Deterministic constructor. `bound_refs` are sorted so the id is
    /// order-independent over the bound module references.
    pub fn from_parts(
        module_digest: &str,
        source_span: &str,
        kind: EvidenceKind,
        bound_refs: &[String],
    ) -> Self {
        let mut refs: Vec<&str> = bound_refs.iter().map(String::as_str).collect();
        refs.sort_unstable();
        let canon = format!(
            "{module_digest}|{source_span}|{}|{}",
            kind.as_str(),
            refs.join(",")
        );
        let mut h = Sha256::new();
        h.update(canon.as_bytes());
        AtomId(format!("atom:{:x}", h.finalize()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An authorized evidence atom derived from **authored module content** (clue /
/// fact / entity / location / objective text). The only payloads a projector or a
/// gateway may ever accept — there is no open natural-language atom.
///
/// Invariants the builder upholds (and the projector relies on):
/// - `source_refs` is non-empty (source-grounded);
/// - `grounding` is the canonical authored reference the projector matches a
///   committed event against (e.g. `fact:clue_aquifer_commercial`);
/// - `bindings` resolve to the module graph (clue id / entity id), never open text.
///
/// (No `Eq`: embeds `Vec<SourceRef>`, which is only `PartialEq`.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceAtomSpec {
    pub atom_id: AtomId,
    pub kind: EvidenceKind,
    pub bindings: Vec<String>,
    pub source_refs: Vec<SourceRef>,
    pub grounding: String,
    /// EV-P3: whether this atom is an authored objective success **leaf**
    /// (`GuardLeaf`) or a mere affordance/mechanic **carrier** (`CarrierOnly`).
    /// Default `CarrierOnly` is skipped on serialize → EV-2 clue/exact atoms stay
    /// byte-identical (proves OFF == baseline).
    #[serde(default, skip_serializing_if = "ProgressRole::is_carrier_only")]
    pub progress_role: ProgressRole,
}

/// Who admitted the evidence. EV-2 only ever mints [`EvidenceAuthority::ExactDomain`]
/// (Rust, deterministic, no LLM). [`EvidenceAuthority::GmWitnessed`] is reserved for
/// the later witness path (EV-4/EV-5) and defined here so the ledger type is stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceAuthority {
    ExactDomain,
    GmWitnessed,
}

/// Rust-admitted evidence — the only legal *input* to a progression guard
/// (design table §核心方案). Never a progress conclusion. Carries the basis
/// DomainEvent id(s) it was projected from and the atom's source refs.
///
/// (No `Eq`: embeds `Vec<SourceRef>`.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcceptedEvidence {
    pub evidence_id: String,
    pub atom_id: AtomId,
    pub evidence_kind: EvidenceKind,
    pub basis_event_ids: Vec<String>,
    pub source_refs: Vec<SourceRef>,
    pub turn_id: String,
    pub authority: EvidenceAuthority,
}

/// Append-only ledger of admitted evidence. In-memory and turn-local for EV-2
/// (the engine does not consume it until EV-6); append is idempotent on
/// `evidence_id` so a same-turn replay never double-records.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EvidenceLedger {
    entries: Vec<AcceptedEvidence>,
}

impl EvidenceLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append unless an entry with the same `evidence_id` already exists. Returns
    /// `true` if it was newly added (idempotent / no duplicate completion).
    pub fn append(&mut self, ev: AcceptedEvidence) -> bool {
        if self.entries.iter().any(|e| e.evidence_id == ev.evidence_id) {
            return false;
        }
        self.entries.push(ev);
        true
    }

    pub fn entries(&self) -> &[AcceptedEvidence] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A compiled catalog of authorized evidence atoms for a module (design §1
/// `EvidenceAtomCatalog`). Built at compile/prep time from authored content; the
/// projector resolves a committed event's authored reference against it. Holds no
/// open text — every atom is a source-grounded [`EvidenceAtomSpec`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EvidenceAtomCatalog {
    atoms: Vec<EvidenceAtomSpec>,
}

impl EvidenceAtomCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert an atom unless one with the same `grounding` already exists
    /// (deduped by canonical authored reference). Returns `true` if newly added.
    pub fn insert(&mut self, atom: EvidenceAtomSpec) -> bool {
        if self.atoms.iter().any(|a| a.grounding == atom.grounding) {
            return false;
        }
        self.atoms.push(atom);
        true
    }

    /// Resolve a `fact:<id>` grounding reference to its atom (exact match — never
    /// fuzzy/substring; design §Wall B "精确 capability 映射非 fuzzy token").
    pub fn resolve_fact(&self, fact_id: &str) -> Option<&EvidenceAtomSpec> {
        let grounding = format!("fact:{fact_id}");
        self.atoms.iter().find(|a| a.grounding == grounding)
    }

    /// Resolve any exact `grounding` reference (`fact:<id>` / `location:<id>` /
    /// `state:<id>`) to its atom (exact match — never fuzzy). EV-P2's
    /// Location/State exact projectors resolve their committed-event refs through
    /// this; `resolve_fact` stays a convenience wrapper for the `fact:` prefix.
    pub fn resolve_grounding(&self, grounding: &str) -> Option<&EvidenceAtomSpec> {
        self.atoms.iter().find(|a| a.grounding == grounding)
    }

    /// Resolve an [`AtomId`] back to its source-grounded atom (EV-4 gateway uses this
    /// to recover an offered atom's `source_refs`/`bindings` after Rust maps the
    /// opaque cap→atom). An offer whose atom is absent here has no verifiable
    /// provenance → the gateway rejects it (`SourceHashMismatch`).
    pub fn resolve_atom_id(&self, atom_id: &AtomId) -> Option<&EvidenceAtomSpec> {
        self.atoms.iter().find(|a| &a.atom_id == atom_id)
    }

    pub fn atoms(&self) -> &[EvidenceAtomSpec] {
        &self.atoms
    }

    pub fn len(&self) -> usize {
        self.atoms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.atoms.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(page: u32) -> SourceRef {
        SourceRef {
            source_id: "the_vault".into(),
            page: Some(page),
            ..Default::default()
        }
    }

    fn atom(clue_id: &str, page: u32) -> EvidenceAtomSpec {
        let atom_id = AtomId::from_parts(
            "digest_v1",
            &format!("p{page}"),
            EvidenceKind::FactLearned,
            &[clue_id.to_string()],
        );
        EvidenceAtomSpec {
            atom_id,
            kind: EvidenceKind::FactLearned,
            bindings: vec![clue_id.to_string()],
            source_refs: vec![src(page)],
            grounding: format!("fact:{clue_id}"),
            progress_role: ProgressRole::CarrierOnly,
        }
    }

    #[test]
    fn atom_id_is_deterministic_for_same_parts() {
        let a = AtomId::from_parts("d", "p8", EvidenceKind::FactLearned, &["clue_a".into()]);
        let b = AtomId::from_parts("d", "p8", EvidenceKind::FactLearned, &["clue_a".into()]);
        assert_eq!(a, b, "same parts ⇒ same AtomId (replay-safe)");
        assert!(a.as_str().starts_with("atom:"), "hashed prefix present");
        assert_eq!(a.as_str().len(), "atom:".len() + 64, "sha256 hex body");
    }

    #[test]
    fn atom_id_differs_when_any_part_differs() {
        let base = AtomId::from_parts("d", "p8", EvidenceKind::FactLearned, &["clue_a".into()]);
        assert_ne!(
            base,
            AtomId::from_parts("d", "p9", EvidenceKind::FactLearned, &["clue_a".into()]),
            "different source_span ⇒ different id"
        );
        assert_ne!(
            base,
            AtomId::from_parts("d", "p8", EvidenceKind::FactRevealed, &["clue_a".into()]),
            "different kind ⇒ different id"
        );
        assert_ne!(
            base,
            AtomId::from_parts("d", "p8", EvidenceKind::FactLearned, &["clue_b".into()]),
            "different bound ref ⇒ different id"
        );
        assert_ne!(
            base,
            AtomId::from_parts("d2", "p8", EvidenceKind::FactLearned, &["clue_a".into()]),
            "different module digest ⇒ different id"
        );
    }

    #[test]
    fn atom_id_is_order_independent_over_bound_refs() {
        let a = AtomId::from_parts("d", "p8", EvidenceKind::FactLearned, &["x".into(), "y".into()]);
        let b = AtomId::from_parts("d", "p8", EvidenceKind::FactLearned, &["y".into(), "x".into()]);
        assert_eq!(a, b, "bound-ref order must not change the id");
    }

    #[test]
    fn catalog_resolves_fact_grounding_exactly() {
        let mut cat = EvidenceAtomCatalog::new();
        cat.insert(atom("clue_aquifer_commercial", 8));
        assert!(
            cat.resolve_fact("clue_aquifer_commercial").is_some(),
            "exact authored ref resolves"
        );
        assert!(
            cat.resolve_fact("aquifer").is_none(),
            "substring/partial ref must NOT resolve (no fuzzy match — Wall B)"
        );
        assert!(cat.resolve_fact("wf_chk_check_abc").is_none(), "synthetic ref unresolved");
    }

    #[test]
    fn catalog_insert_is_deduped_by_grounding() {
        let mut cat = EvidenceAtomCatalog::new();
        assert!(cat.insert(atom("clue_a", 8)));
        assert!(!cat.insert(atom("clue_a", 8)), "same grounding ⇒ not re-inserted");
        assert_eq!(cat.len(), 1);
    }

    #[test]
    fn ledger_append_is_idempotent_on_evidence_id() {
        let mut led = EvidenceLedger::new();
        let ev = AcceptedEvidence {
            evidence_id: "ev_1".into(),
            atom_id: AtomId("atom:x".into()),
            evidence_kind: EvidenceKind::FactLearned,
            basis_event_ids: vec!["de_1".into()],
            source_refs: vec![src(8)],
            turn_id: "t1".into(),
            authority: EvidenceAuthority::ExactDomain,
        };
        assert!(led.append(ev.clone()), "first append adds");
        assert!(!led.append(ev), "duplicate evidence_id ⇒ no double-record");
        assert_eq!(led.len(), 1);
    }

    #[test]
    fn types_roundtrip_serde() {
        let ev = AcceptedEvidence {
            evidence_id: "ev_1".into(),
            atom_id: AtomId::from_parts("d", "p8", EvidenceKind::FactRevealed, &["clue_a".into()]),
            evidence_kind: EvidenceKind::FactRevealed,
            basis_event_ids: vec!["de_1".into()],
            source_refs: vec![src(8)],
            turn_id: "t1".into(),
            authority: EvidenceAuthority::ExactDomain,
        };
        let v = serde_json::to_value(&ev).unwrap();
        let back: AcceptedEvidence = serde_json::from_value(v).unwrap();
        assert_eq!(back, ev, "AcceptedEvidence serde round-trips byte-stable");
    }
}

//! Rulebook-reader agent: reads an unknown TRPG rulebook like a GM (toc ->
//! hypothesize -> targeted search/read -> 6-question GM run-kit -> stop),
//! driven by the LLM via function-calling. Replaces the brute-force first-pass.

pub mod agent;
pub mod behavior_align;
pub mod chargen_compile;
pub mod facilitation;
pub mod mechanics_compile;
pub mod mechanics_finalize;
pub mod module_graph_build;
pub mod module_graph_edges;
pub mod module_graph_validator;
pub mod module_reader;
pub mod module_reader_loop;
pub mod object_compile;
mod object_regrab;
pub mod onboarding_compile;
pub mod parallel;
pub mod run_kit;
pub(crate) mod scene_mechanics;
pub mod tools;
pub mod units;

pub use agent::{run_reader, ReaderResult};
pub use behavior_align::{align, audit_alignment, AlignmentReport, validate_emitted_on_outcome, apply_emitted_behavior, fill_behavior_from_prose};
pub use chargen_compile::{compile_chargen_formulas, CompileCtx};
pub use facilitation::extract_facilitation_facts;
pub use mechanics_compile::{compile_mechanics_catalog, MechCompileCtx};
pub use mechanics_finalize::finalize_catalog;
pub use module_graph_edges::apply_bridge_edges;
pub use module_reader::{run_module_reader, ModuleReaderCtx, ModuleReadout};
pub use module_reader_loop::{complete_skeleton_stubs, deep_extract_scene_in_place};
pub use object_compile::{compile_object_schemas, discover_object_categories, extract_object_category, ObjectCtx};
pub use onboarding_compile::{compile_starter_pack, OnboardingCtx};
pub use parallel::{plan_phase, read_character_slice, read_resolution_and_gm, run_reader_parallel, CharacterSlice, Plan, ResolutionGm};
pub use run_kit::{CoreRules, GmRunKit};
pub use units::{load_units, Unit};

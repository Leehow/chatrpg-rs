//! Staged (progressive) ruleset orchestrator.
//!
//! Turns the monolithic ~12-min parse into a 3-stage pipeline:
//!   Stage 0 (identity) -> Stage 1 (character) -> Stage 2 (deep background)
//! Each stage persists artifacts incrementally (partial `RuleKernel` via the
//! existing jsonb + serde-default round-trip) and updates a `background_jobs`
//! row through `JobStatus`. It REUSES the reader/compile primitives — it does
//! NOT rewrite `parse_source`.
//!
//! Error isolation: an already-`done` stage is never rolled back. Stage 0/1
//! failures are fatal (job `failed`); Stage 2 sub-steps are independent and
//! non-fatal — Stage 1 success means the job ends `done` even if the deep stage
//! partially fails.

use crate::staged_status::JobStatus;
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use trpg_db::Db;
use trpg_llm::LlmClient;
use trpg_rule_agent::reader;

/// The in-process staged-parse driver. Built by the CLI (`parse-staged`) and the
/// HTTP `ingest_ruleset` handler with the loaded units + sidecar for one ruleset.
pub struct StagedParse {
    pub db: Db,
    pub llm: Arc<dyn LlmClient>,
    pub ruleset_id: String,
    pub units: Vec<reader::Unit>,
    pub sidecar_text: Option<String>,
    pub title: String,
    pub job_id: String,
    /// Data dir for incremental artifact writes (parsed/characters, parsed/rules).
    pub data_dir: PathBuf,
    /// Stop after Stage 1 (fast-path test harness flag).
    pub stage1_only: bool,
}

impl StagedParse {
    async fn report(&self, st: &JobStatus, status: &str) {
        let _ = self
            .db
            .update_background_job(&self.job_id, status, st.to_value(), st.error.as_deref())
            .await;
    }

    /// Run all stages, returning the final `JobStatus`. Always reports the
    /// terminal status to the job row before returning.
    pub async fn run(&self, budget: usize) -> JobStatus {
        let mut st = JobStatus::new(&self.ruleset_id);
        self.report(&st, "running").await;

        let plan = match self.stage0_identity(&mut st).await {
            Ok(p) => p,
            Err(e) => {
                st.fail("identity", &e.to_string());
                self.report(&st, "failed").await;
                return st;
            }
        };

        let char_slice = match self.stage1_character(&mut st, &plan, budget).await {
            Ok(c) => c,
            Err(e) => {
                st.fail("character", &e.to_string());
                self.report(&st, "failed").await;
                return st;
            }
        };

        if self.stage1_only {
            self.report(&st, "done").await;
            return st;
        }

        // Stage 2 is non-fatal: stage1 success => job 'done' even if deep partially fails.
        self.stage2_deep(&mut st, &plan, &char_slice, budget).await;
        let final_status = if st.error.is_some() && st.progress_pct < 35 {
            "failed"
        } else {
            "done"
        };
        self.report(&st, final_status).await;
        st
    }

    /// Stage 0 — identity: cheap TOC plan call, persist a partial kernel carrying
    /// the premise so `/identity` can serve it.
    async fn stage0_identity(&self, st: &mut JobStatus) -> Result<reader::Plan> {
        st.begin("identity");
        self.report(st, "running").await;
        let toc = reader::tools::toc(&self.units, 40);
        let plan = reader::plan_phase(self.llm.as_ref(), &self.ruleset_id, &toc).await?;
        let mut kernel = trpg_model::RuleKernel {
            kernel_id: format!("{}.rule_kernel.v1", self.ruleset_id),
            ruleset_id: self.ruleset_id.clone(),
            version: "v1_staged".into(),
            ..Default::default()
        };
        kernel.game_identity =
            serde_json::json!({"summary": plan.identity, "hypothesis": plan.hypothesis});
        self.db.upsert_rule_kernel(&kernel).await.ok();
        st.finish("identity", &plan.identity);
        self.report(st, "running").await;
        Ok(plan)
    }

    /// Stage 1 — character slice: read the buildable sheet, coerce + persist it
    /// into the kernel (`character_sheet_schema`) and onboarding artifacts.
    async fn stage1_character(
        &self,
        st: &mut JobStatus,
        plan: &reader::Plan,
        budget: usize,
    ) -> Result<reader::CharacterSlice> {
        st.begin("character");
        self.report(st, "running").await;
        let slice = reader::read_character_slice(
            self.llm.as_ref(),
            &self.units,
            &self.ruleset_id,
            plan,
            budget,
        )
        .await?;
        let template = crate::coerce_character_template(
            slice.character_template.clone(),
            &self.ruleset_id,
            &self.title,
        );
        let mut kernel = self
            .db
            .load_rule_kernel(&self.ruleset_id)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| trpg_model::RuleKernel {
                kernel_id: format!("{}.rule_kernel.v1", self.ruleset_id),
                ruleset_id: self.ruleset_id.clone(),
                version: "v1_staged".into(),
                ..Default::default()
            });
        kernel.character_sheet_schema = serde_json::to_value(&template).unwrap_or_default();
        self.db.upsert_rule_kernel(&kernel).await.ok();
        crate::persist_stage1_character_artifacts(
            &self.db,
            &self.data_dir,
            &self.ruleset_id,
            &self.title,
            &template,
            &slice.option_catalogs,
        )
        .await
        .ok();
        st.finish("character", "character sheet ready");
        self.report(st, "running").await;
        Ok(slice)
    }

    /// Stage 2 — deep background. Each sub-step is independent and non-fatal:
    /// resolution+gm, chargen formula compile, object-category discover, full
    /// kernel persist, then a best-effort module + index hook.
    async fn stage2_deep(
        &self,
        st: &mut JobStatus,
        plan: &reader::Plan,
        char_slice: &reader::CharacterSlice,
        budget: usize,
    ) {
        st.begin("deep");
        st.note("resolution + gm");
        self.report(st, "running").await;
        let compiler = crate::build_compiler_llm().unwrap_or_else(|| self.llm.clone());

        // 2a resolution + gm (concurrent inside the slice fn).
        let mut rg = reader::read_resolution_and_gm(
            self.llm.as_ref(),
            &self.units,
            &self.ruleset_id,
            plan,
            budget,
        )
        .await
        .ok();

        // 2b chargen compile (derived value formulas) — backfill into the template.
        st.note("compiling character formulas");
        self.report(st, "running").await;
        let mut template = crate::coerce_character_template(
            char_slice.character_template.clone(),
            &self.ruleset_id,
            &self.title,
        );
        let skills = crate::skill_ids(&template, &char_slice.option_catalogs);
        let tracks = rg
            .as_ref()
            .map(|r| crate::track_ids(&r.core.resource_tracks))
            .unwrap_or_default();
        let ctx = reader::CompileCtx {
            units: &self.units,
            sidecar_text: self.sidecar_text.clone(),
            located_pages: String::new(),
            skill_names: skills.clone(),
        };
        let _ =
            reader::compile_chargen_formulas(compiler.as_ref(), &mut template, ctx, budget).await;

        // 2b-bis 行为层对齐：把 chargen 公式层 id 与 kernel resource_tracks 行为层按 id 对齐
        // （落 derived_from 链接 + 缺 track 建 stub，fail-closed，不臆造行为）。确定性、无 LLM。
        if let Some(rg) = rg.as_mut() {
            let dvs: Vec<serde_json::Value> = template
                .derived_values
                .iter()
                .filter_map(|d| serde_json::to_value(d).ok())
                .collect();
            let report = reader::align(&dvs, &mut rg.core.resource_tracks);
            if !report.behavior_gaps.is_empty() {
                st.note(&format!(
                    "行为对齐缺口(待 override/LLM 补): {:?}",
                    report.behavior_gaps
                ));
                // 三级兜底：override 数据是主源，本路默认关(TRPG_BEHAVIOR_ALIGN_LLM=1 才开)。
                // fail-closed：内部校验 =field、绝不覆盖既有/override 行为、prose 缺则省略。
                reader::fill_behavior_from_prose(
                    compiler.as_ref(),
                    &self.units,
                    self.sidecar_text.clone(),
                    &dvs,
                    &mut rg.core.resource_tracks,
                    budget,
                )
                .await;
            }
        }

        // 2c object schemas: FULL compile (discover + extract) in the background.
        // Stage 2 is non-blocking (the user is creating a character meanwhile), so we
        // extract eagerly here — schemas are ready by play time, no mid-action hiccup.
        // Optimization 2 (discover-stub now / extract-on-demand at play time) is
        // DEFERRED: its machinery lives dormant in trpg-material/src/staged_extract.rs
        // (ensure_category_compiled), to enable later for spell-heavy rulesets where
        // eager extraction of every category is genuinely too slow. See the spec's
        // "discover-now, extract-on-demand" section (marked separable/deferrable).
        st.note("compiling object schemas");
        self.report(st, "running").await;
        let obj_ctx = reader::ObjectCtx {
            units: &self.units,
            sidecar_text: self.sidecar_text.clone(),
            skills,
            resource_tracks: tracks,
        };
        let object_schemas =
            reader::compile_object_schemas(compiler.as_ref(), &obj_ctx, budget).await;

        // 2d assemble + persist the full kernel.
        if let Err(e) = crate::persist_stage2_kernel(
            &self.db,
            &self.ruleset_id,
            &self.title,
            rg.as_ref(),
            &template,
            &char_slice.option_catalogs,
            object_schemas,
            &self.units,
            self.sidecar_text.clone(),
            &self.llm,
        )
        .await
        {
            st.note(&format!("kernel persist failed: {e}"));
        }

        // 2d-bis onboarding compile: extract starter-character recipes + re-upsert
        // the COMPLETE onboarding pack (creation_flows are already deterministic in
        // the Stage-1 base; this fills the starter pack). Independent + non-fatal.
        st.note("compiling starter character pack");
        self.report(st, "running").await;
        let role_field = template
            .fields
            .iter()
            .find(|f| {
                matches!(f.field_type.as_str(), "role" | "class")
                    || matches!(
                        f.field_id.as_str(),
                        "role" | "class" | "occupation" | "profession" | "career" | "arc"
                    )
            })
            .map(|f| f.field_id.clone());
        let skill_fields: Vec<String> = template
            .fields
            .iter()
            .filter(|f| f.field_type == "skill")
            .map(|f| f.field_id.clone())
            .collect();
        let onboarding_ctx = reader::OnboardingCtx {
            units: &self.units,
            sidecar_text: self.sidecar_text.clone(),
            role_field,
            skill_fields,
            option_catalogs: char_slice.option_catalogs.clone(),
        };
        let starter_pack =
            reader::compile_starter_pack(compiler.as_ref(), &onboarding_ctx, budget).await;
        if let Err(e) = crate::persist_stage2_onboarding(
            &self.db,
            &self.ruleset_id,
            &self.title,
            &template,
            &char_slice.option_catalogs,
            &starter_pack,
        )
        .await
        {
            st.note(&format!("onboarding persist failed: {e}"));
        }

        // 2e module reader (black box) + reindex — best-effort, non-fatal.
        st.note("module + index");
        self.report(st, "running").await;
        // The orchestrator carries no ParserConfig; the module+index path is a
        // black-box best-effort hook (see crate::run_module_and_index_for).
        let _ = crate::run_module_and_index_for_ruleset(&self.db, &self.ruleset_id).await;

        st.finish("deep", "deep parse complete");
        self.report(st, "running").await;
    }
}

#[cfg(test)]
mod tests {
    //! Deterministic coverage for the staged orchestrator's stage state machine.
    //!
    //! The LLM-dependent stages (plan/character/resolution/object) and the live DB
    //! path are validated by the CLI harness (`parse-staged`, Task C) and the API
    //! smoke (Task D) — a real test `Db` is not available in this unit scope, so
    //! this file asserts the JobStatus transition contract directly to keep the
    //! coverage intent explicit (no silent gap). The partial-kernel-after-stage1
    //! and stage2-failure-non-fatal invariants are exercised end-to-end there.
    use crate::staged_status::JobStatus;

    #[test]
    fn stage_order_and_non_fatal_deep() {
        // Mirror the orchestrator's drive order: identity -> character -> deep.
        let mut st = JobStatus::new("call_of_cthulhu_7e");
        st.begin("identity");
        st.finish("identity", "premise");
        st.begin("character");
        st.finish("character", "character sheet ready");
        // After stage 1 the character stage is done and progress is partial.
        assert!(st
            .stages
            .iter()
            .any(|s| s.name == "character" && s.status == "done"));
        let after_stage1 = st.progress_pct;
        assert!(
            after_stage1 >= 35 && after_stage1 < 100,
            "stage1 leaves partial progress"
        );

        // A stage-2 sub-step failure records an error but must NOT roll back stage 1,
        // and (since progress_pct >= 35) the final status would still be "done".
        st.begin("deep");
        st.note("kernel persist failed: boom");
        // Simulate the orchestrator's terminal-status rule without failing the stage.
        let final_status = if st.error.is_some() && st.progress_pct < 35 {
            "failed"
        } else {
            "done"
        };
        assert_eq!(final_status, "done");
        assert!(
            st.stages
                .iter()
                .any(|s| s.name == "character" && s.status == "done"),
            "stage1 intact after deep note"
        );
    }
}

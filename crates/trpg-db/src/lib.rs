use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::json;
use sqlx::{PgPool, Row};
use trpg_model::*;
use uuid::Uuid;

mod resource_current;

#[derive(Clone)]
pub struct Db {
    pub pool: PgPool,
}

impl Db {
    pub async fn connect(database_url: &str) -> Result<Self> {
        let pool = PgPool::connect(database_url)
            .await
            .with_context(|| format!("failed to connect PostgreSQL at {database_url}"))?;
        Ok(Self { pool })
    }

    pub async fn migrate(&self) -> Result<()> {
        let migrations = [
            include_str!("../../../migrations/0001_init.sql"),
            include_str!("../../../migrations/0002_search_runtime_autoload_v07.sql"),
            include_str!("../../../migrations/0003_learning_audit_v084.sql"),
            include_str!("../../../migrations/0004_agentic_checks_v09.sql"),
            include_str!("../../../migrations/0005_interaction_gates_state_frames_v091.sql"),
            include_str!("../../../migrations/0006_conflict_combat_v10.sql"),
            include_str!("../../../migrations/0007_semantic_situation_orchestrator_v11.sql"),
            include_str!("../../../migrations/0008_actionable_situation_director_v12.sql"),
            include_str!("../../../migrations/0009_situation_novelty_director_v13.sql"),
            include_str!("../../../migrations/0010_world_time_spine_v14.sql"),
            include_str!("../../../migrations/0011_interaction_lifecycle_kernel_v15.sql"),
            include_str!("../../../migrations/0012_object_possession_kernel_v16.sql"),
            include_str!("../../../migrations/0013_turn_orchestration_kernel_v17.sql"),
            include_str!("../../../migrations/0014_runtime_parameter_hydration_v18.sql"),
            include_str!("../../../migrations/0015_semantic_rule_binding_ability_v19.sql"),
            include_str!("../../../migrations/0016_semantic_route_stability_v191.sql"),
            include_str!("../../../migrations/0017_real_materialization_extractor_v110.sql"),
            include_str!("../../../migrations/0018_player_value_referee_v1102.sql"),
            include_str!("../../../migrations/0019_referee_combat_slice_v1102.sql"),
            include_str!("../../../migrations/0020_contest_opposition_kernel_v111.sql"),
            include_str!("../../../migrations/0021_mechanics_search_skills_v112.sql"),
            include_str!("../../../migrations/0022_unified_roll_effect_executor_v1121.sql"),
            include_str!("../../../migrations/0023_roll_binding_mechanical_gate_priority_v1122.sql"),
            include_str!("../../../migrations/0024_parameter_facet_executor_v113.sql"),
            include_str!("../../../migrations/0025_rule_steward_character_onboarding_v116.sql"),
            include_str!("../../../migrations/0026_session_current_scene_v120.sql"),
        ];
        for sql in migrations {
            for statement in split_sql_statements(sql) {
                let trimmed = statement.trim();
                if trimmed.is_empty() {
                    continue;
                }
                sqlx::query(trimmed).execute(&self.pool).await.with_context(|| {
                    format!("failed migration statement: {}", trimmed.chars().take(120).collect::<String>())
                })?;
            }
        }
        Ok(())
    }

    pub async fn upsert_source_document(&self, doc: &SourceDocument) -> Result<()> {
        sqlx::query(
            r#"
            insert into source_documents
              (id, source_id, source_kind, title, file_path, markdown_path, source_hash, parse_config_hash, parse_status, metadata)
            values ($1,$2,$3,$4,$5,$6,$7,$8,'parsed',$9)
            on conflict (source_id) do update set
              source_kind = excluded.source_kind,
              title = excluded.title,
              file_path = excluded.file_path,
              markdown_path = excluded.markdown_path,
              source_hash = excluded.source_hash,
              parse_config_hash = excluded.parse_config_hash,
              parse_status = excluded.parse_status,
              metadata = excluded.metadata,
              updated_at = now()
            "#,
        )
        .bind(doc.id)
        .bind(&doc.source_id)
        .bind(doc.source_kind.as_str())
        .bind(&doc.title)
        .bind(&doc.file_path)
        .bind(&doc.markdown_path)
        .bind(&doc.source_hash)
        .bind(&doc.parse_config_hash)
        .bind(&doc.metadata)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn has_bundle_for_source(&self, source_hash: &str, parse_config_hash: &str, bundle_kind: &str) -> Result<bool> {
        let count: i64 = sqlx::query_scalar(
            r#"select count(*) from parsed_bundles where source_hash = $1 and parse_config_hash = $2 and bundle_kind = $3"#,
        )
        .bind(source_hash)
        .bind(parse_config_hash)
        .bind(bundle_kind)
        .fetch_one(&self.pool)
        .await?;
        Ok(count > 0)
    }

    pub async fn upsert_rule_bundle(&self, bundle: &RuleBundle, artifact_path: Option<&str>, source_hash: &str, parse_config_hash: &str) -> Result<()> {
        self.upsert_parsed_bundle(&bundle.bundle_id, "ruleset", &bundle.title, &bundle.schema_version, artifact_path, source_hash, parse_config_hash, bundle, &bundle.validation_report).await?;
        for block in &bundle.context_blocks {
            self.upsert_context_block(&bundle.bundle_id, block).await?;
        }
        for entry in &bundle.material_index {
            self.upsert_material_entry(&bundle.bundle_id, entry).await?;
        }
        for template in &bundle.character_templates {
            self.upsert_character_template(&bundle.bundle_id, template).await?;
        }
        for pack in &bundle.character_onboarding_packs {
            self.upsert_character_onboarding_pack(pack).await?;
        }
        if let Some(kernel) = &bundle.rule_kernel {
            self.upsert_rule_kernel(kernel).await?;
        }
        if let Some(onboarding) = &bundle.gm_onboarding {
            self.upsert_onboarding_bundle(onboarding).await?;
            for locator in onboarding.book_locator.iter().chain(onboarding.cold_data_locator.iter()) {
                self.upsert_book_locator_entry(locator).await?;
            }
        }
        Ok(())
    }

    pub async fn upsert_module_bundle(&self, bundle: &ModuleBundle, artifact_path: Option<&str>, source_hash: &str, parse_config_hash: &str) -> Result<()> {
        self.upsert_parsed_bundle(&bundle.bundle_id, "module", &bundle.title, &bundle.schema_version, artifact_path, source_hash, parse_config_hash, bundle, &bundle.validation_report).await?;
        for block in &bundle.context_blocks {
            self.upsert_context_block(&bundle.bundle_id, block).await?;
        }
        for entry in &bundle.material_index {
            self.upsert_material_entry(&bundle.bundle_id, entry).await?;
            if matches!(entry.material_type, MaterialType::BookLocator | MaterialType::ColdDataLocator) {
                let locator = locator_entry_from_material(&bundle.module_id, "module", entry);
                self.upsert_book_locator_entry(&locator).await?;
            }
        }
        for packet in &bundle.module_prep_packets {
            self.upsert_module_prep_packet(packet).await?;
        }
        for locator in &bundle.module_locators {
            self.upsert_book_locator_entry(locator).await?;
        }
        Ok(())
    }

    pub async fn upsert_project_bundle(&self, bundle: &ProjectBundle, artifact_path: Option<&str>, source_hash: &str, parse_config_hash: &str) -> Result<()> {
        self.upsert_parsed_bundle(&bundle.project_id, "project", "Local Project", &bundle.schema_version, artifact_path, source_hash, parse_config_hash, bundle, &bundle.validation_report).await
    }

    pub async fn upsert_parsed_bundle<T: serde::Serialize>(
        &self,
        bundle_id: &str,
        bundle_kind: &str,
        title: &str,
        schema_version: &str,
        artifact_path: Option<&str>,
        source_hash: &str,
        parse_config_hash: &str,
        content: &T,
        validation_report: &ValidationReport,
    ) -> Result<()> {
        let content_json = serde_json::to_value(content)?;
        let validation_json = serde_json::to_value(validation_report)?;
        sqlx::query(
            r#"
            insert into parsed_bundles
              (id, bundle_id, bundle_kind, title, schema_version, artifact_path, source_hash, parse_config_hash, content_json, validation_report)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
            on conflict (bundle_id) do update set
              bundle_kind = excluded.bundle_kind,
              title = excluded.title,
              schema_version = excluded.schema_version,
              artifact_path = excluded.artifact_path,
              source_hash = excluded.source_hash,
              parse_config_hash = excluded.parse_config_hash,
              content_json = excluded.content_json,
              validation_report = excluded.validation_report,
              updated_at = now()
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(bundle_id)
        .bind(bundle_kind)
        .bind(title)
        .bind(schema_version)
        .bind(artifact_path.map(str::to_string))
        .bind(source_hash)
        .bind(parse_config_hash)
        .bind(content_json)
        .bind(validation_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn upsert_context_block(&self, bundle_id: &str, block: &ContextBlock) -> Result<()> {
        let content_json = serde_json::to_value(&block.content)?;
        let content_text = block.content.render_text();
        let source_refs = serde_json::to_value(&block.source_refs)?;
        let dependencies = serde_json::to_value(&block.dependencies)?;
        sqlx::query(
            r#"
            insert into context_blocks
              (id, bundle_id, block_id, block_kind, title, scope_type, scope_id, visibility, stability, cache_zone,
               priority, version, content_json, content_text, content_hash, token_estimate, source_refs, dependencies,
               tags, expires_at_turn, expires_at_scene, load_reason, active)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,true)
            on conflict (bundle_id, block_id, version) do update set
              block_kind = excluded.block_kind,
              title = excluded.title,
              scope_type = excluded.scope_type,
              scope_id = excluded.scope_id,
              visibility = excluded.visibility,
              stability = excluded.stability,
              cache_zone = excluded.cache_zone,
              priority = excluded.priority,
              content_json = excluded.content_json,
              content_text = excluded.content_text,
              content_hash = excluded.content_hash,
              token_estimate = excluded.token_estimate,
              source_refs = excluded.source_refs,
              dependencies = excluded.dependencies,
              tags = excluded.tags,
              expires_at_turn = excluded.expires_at_turn,
              expires_at_scene = excluded.expires_at_scene,
              load_reason = excluded.load_reason,
              active = true,
              updated_at = now()
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(bundle_id)
        .bind(&block.block_id)
        .bind(block.kind.as_str())
        .bind(&block.title)
        .bind(format!("{:?}", block.scope.scope_type).to_snake())
        .bind(&block.scope.scope_id)
        .bind(block.visibility.as_str())
        .bind(block.stability.as_str())
        .bind(block.cache_zone.as_str())
        .bind(block.priority)
        .bind(block.version as i32)
        .bind(content_json)
        .bind(content_text)
        .bind(&block.content_hash)
        .bind(block.token_estimate.map(|v| v as i32))
        .bind(source_refs)
        .bind(dependencies)
        .bind(&block.tags)
        .bind(&block.expires_at_turn)
        .bind(&block.expires_at_scene)
        .bind(&block.load_reason)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn upsert_runtime_context_block(&self, session_id: &str, block: &ContextBlock) -> Result<()> {
        let bundle_id = runtime_bundle_id(session_id);
        self.upsert_context_block(&bundle_id, block).await
    }

    pub async fn list_runtime_context_blocks(&self, session_id: &str, turn_id: &str, scene_id: Option<&str>) -> Result<Vec<ContextBlock>> {
        let bundle_id = runtime_bundle_id(session_id);
        let rows = sqlx::query(
            r#"
            select block_id, block_kind, title, scope_type, scope_id, visibility, stability, cache_zone, priority, version,
                   content_json, content_hash, token_estimate, source_refs, dependencies, tags, expires_at_turn, expires_at_scene, load_reason
            from context_blocks
            where bundle_id = $1 and active = true
              and (expires_at_turn is null or expires_at_turn = $2)
              and (expires_at_scene is null or ($3::text is not null and expires_at_scene = $3::text))
            order by priority desc, updated_at desc
            "#,
        )
        .bind(&bundle_id)
        .bind(turn_id)
        .bind(scene_id.map(str::to_string))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_context_block).collect()
    }

    pub async fn deactivate_runtime_turn_blocks(&self, session_id: &str, turn_id: &str) -> Result<()> {
        let bundle_id = runtime_bundle_id(session_id);
        sqlx::query(
            r#"update context_blocks set active = false, updated_at = now() where bundle_id = $1 and expires_at_turn is not null and expires_at_turn <> $2"#,
        )
        .bind(&bundle_id)
        .bind(turn_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn upsert_material_entry(&self, bundle_id: &str, entry: &MaterialIndexEntry) -> Result<()> {
        sqlx::query(
            r#"
            insert into material_index
              (id, bundle_id, material_id, material_type, title, summary, load_when, source_refs, dependencies,
               estimated_tokens, default_cache_zone, visibility, stability, extracted_block_id, extraction_status, tags)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,'extracted',$15)
            on conflict (bundle_id, material_id) do update set
              material_type = excluded.material_type,
              title = excluded.title,
              summary = excluded.summary,
              load_when = excluded.load_when,
              source_refs = excluded.source_refs,
              dependencies = excluded.dependencies,
              estimated_tokens = excluded.estimated_tokens,
              default_cache_zone = excluded.default_cache_zone,
              visibility = excluded.visibility,
              stability = excluded.stability,
              extracted_block_id = excluded.extracted_block_id,
              extraction_status = excluded.extraction_status,
              tags = excluded.tags,
              updated_at = now()
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(bundle_id)
        .bind(&entry.material_id)
        .bind(format!("{:?}", entry.material_type).to_snake())
        .bind(&entry.title)
        .bind(&entry.summary)
        .bind(serde_json::to_value(&entry.load_when)?)
        .bind(serde_json::to_value(&entry.source_refs)?)
        .bind(serde_json::to_value(&entry.dependencies)?)
        .bind(entry.estimated_tokens.map(|v| v as i32))
        .bind(entry.default_cache_zone.as_str())
        .bind(entry.visibility.as_str())
        .bind(entry.stability.as_str())
        .bind(&entry.extracted_block_id)
        .bind(&entry.tags)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn upsert_character_template(&self, bundle_id: &str, template: &CharacterTemplate) -> Result<()> {
        sqlx::query(
            r#"
            insert into character_templates
              (id, bundle_id, template_id, ruleset_id, title, content_json, source_refs)
            values ($1,$2,$3,$4,$5,$6,$7)
            on conflict (template_id) do update set
              bundle_id = excluded.bundle_id,
              ruleset_id = excluded.ruleset_id,
              title = excluded.title,
              content_json = excluded.content_json,
              source_refs = excluded.source_refs,
              updated_at = now()
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(bundle_id)
        .bind(&template.template_id)
        .bind(&template.ruleset_id)
        .bind(&template.title)
        .bind(serde_json::to_value(template)?)
        .bind(serde_json::to_value(&template.source_refs)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_bundles(&self) -> Result<Vec<serde_json::Value>> {
        let rows = sqlx::query(
            r#"select bundle_id, bundle_kind, title, schema_version, artifact_path, updated_at from parsed_bundles order by updated_at desc"#,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| json!({
            "bundle_id": r.get::<String,_>("bundle_id"),
            "bundle_kind": r.get::<String,_>("bundle_kind"),
            "title": r.get::<String,_>("title"),
            "schema_version": r.get::<String,_>("schema_version"),
            "artifact_path": r.get::<Option<String>,_>("artifact_path"),
        })).collect())
    }


    pub async fn load_rule_bundle_by_source(&self, source_hash: &str, parse_config_hash: &str) -> Result<Option<RuleBundle>> {
        let row = sqlx::query(
            r#"select content_json from parsed_bundles where source_hash = $1 and parse_config_hash = $2 and bundle_kind = 'ruleset' order by updated_at desc limit 1"#,
        )
        .bind(source_hash)
        .bind(parse_config_hash)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => Ok(Some(serde_json::from_value(r.get("content_json"))?)),
            None => Ok(None),
        }
    }

    pub async fn load_module_bundle_by_source(&self, source_hash: &str, parse_config_hash: &str) -> Result<Option<ModuleBundle>> {
        let row = sqlx::query(
            r#"select content_json from parsed_bundles where source_hash = $1 and parse_config_hash = $2 and bundle_kind = 'module' order by updated_at desc limit 1"#,
        )
        .bind(source_hash)
        .bind(parse_config_hash)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => Ok(Some(serde_json::from_value(r.get("content_json"))?)),
            None => Ok(None),
        }
    }

    /// Load the most recent module bundle for `module_id` together with the
    /// `source_hash` + `parse_config_hash` the row was stored under. The
    /// background-continue job needs both hashes to re-`upsert_module_bundle`
    /// into the same `parsed_bundles` row (its `on conflict (bundle_id)` key)
    /// after deep-extracting remaining scenes. Matches on the bundle's
    /// top-level `module_id` so the caller need not reconstruct the bundle_id.
    pub async fn load_module_bundle_for_continue(
        &self,
        module_id: &str,
    ) -> Result<Option<(ModuleBundle, String, String)>> {
        let row = sqlx::query(
            r#"select content_json, source_hash, parse_config_hash from parsed_bundles
               where bundle_kind = 'module' and content_json->>'module_id' = $1
               order by updated_at desc limit 1"#,
        )
        .bind(module_id)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => {
                let bundle: ModuleBundle = serde_json::from_value(r.get("content_json"))?;
                let source_hash: String = r.get("source_hash");
                let parse_config_hash: String = r.get("parse_config_hash");
                Ok(Some((bundle, source_hash, parse_config_hash)))
            }
            None => Ok(None),
        }
    }

    /// Load ONLY the current `module_graph` of the latest module bundle for
    /// `module_id`. This is the single source of truth for scene data at
    /// runtime: both `parse_module` and the P5 background-continue job keep this
    /// row's graph up to date (`upsert_module_bundle`), whereas the project
    /// bundle carries a frozen parse-time snapshot. Selects just the graph
    /// subtree to stay cheap (called once per turn). Fail-closed: no row -> None.
    pub async fn load_module_graph(&self, module_id: &str) -> Result<Option<ModuleGraph>> {
        let row = sqlx::query(
            r#"select content_json->'module_graph' as module_graph from parsed_bundles
               where bundle_kind = 'module' and content_json->>'module_id' = $1
               order by updated_at desc limit 1"#,
        )
        .bind(module_id)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => {
                let raw: Option<serde_json::Value> = r.try_get("module_graph").ok();
                match raw {
                    Some(v) if !v.is_null() => Ok(Some(serde_json::from_value(v)?)),
                    _ => Ok(None),
                }
            }
            None => Ok(None),
        }
    }

    pub async fn load_project_bundle(&self, project_id: &str) -> Result<Option<ProjectBundle>> {
        let row = sqlx::query(r#"select content_json from parsed_bundles where bundle_id = $1 and bundle_kind = 'project'"#)
            .bind(project_id)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(r) => Ok(Some(serde_json::from_value(r.get("content_json"))?)),
            None => Ok(None),
        }
    }

    pub async fn load_latest_project_bundle(&self) -> Result<Option<ProjectBundle>> {
        let row = sqlx::query(r#"select content_json from parsed_bundles where bundle_kind = 'project' order by updated_at desc limit 1"#)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(r) => Ok(Some(serde_json::from_value(r.get("content_json"))?)),
            None => Ok(None),
        }
    }

    /// Load the most recent project bundle whose rulesets include `ruleset_id`.
    /// Needed when several rulesets share one DB: `load_latest_project_bundle`
    /// would return whichever ruleset was parsed last, not the one being played.
    pub async fn load_project_bundle_for_ruleset(&self, ruleset_id: &str) -> Result<Option<ProjectBundle>> {
        let row = sqlx::query(r#"select content_json from parsed_bundles where bundle_kind = 'project' and content_json->'rulesets' @> jsonb_build_array(jsonb_build_object('ruleset_id', $1::text)) order by updated_at desc limit 1"#)
            .bind(ruleset_id)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(r) => Ok(Some(serde_json::from_value(r.get("content_json"))?)),
            None => Ok(None),
        }
    }

    pub async fn load_character_template(&self, ruleset_id: &str) -> Result<Option<CharacterTemplate>> {
        let row = sqlx::query(
            r#"select content_json from character_templates where ruleset_id = $1 order by updated_at desc limit 1"#,
        )
        .bind(ruleset_id)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => Ok(Some(serde_json::from_value(r.get("content_json"))?)),
            None => Ok(None),
        }
    }

    pub async fn upsert_character_onboarding_pack(&self, pack: &CharacterOnboardingPack) -> Result<()> {
        let content_json = serde_json::to_value(pack)?;
        let content_hash = stable_json_hash(pack);
        sqlx::query(
            r#"
            insert into character_onboarding_packs
              (id, pack_id, ruleset_id, title, content_json, sheet_template_json, creation_flows_json,
               option_catalogs_json, derived_formula_pack_json, starter_character_pack_json, runtime_bindings_json,
               source_refs, validation_report, content_hash)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
            on conflict (pack_id) do update set
              ruleset_id = excluded.ruleset_id,
              title = excluded.title,
              content_json = excluded.content_json,
              sheet_template_json = excluded.sheet_template_json,
              creation_flows_json = excluded.creation_flows_json,
              option_catalogs_json = excluded.option_catalogs_json,
              derived_formula_pack_json = excluded.derived_formula_pack_json,
              starter_character_pack_json = excluded.starter_character_pack_json,
              runtime_bindings_json = excluded.runtime_bindings_json,
              source_refs = excluded.source_refs,
              validation_report = excluded.validation_report,
              content_hash = excluded.content_hash,
              updated_at = now()
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&pack.pack_id)
        .bind(&pack.ruleset_id)
        .bind(&pack.title)
        .bind(content_json)
        .bind(serde_json::to_value(&pack.sheet_template)?)
        .bind(serde_json::to_value(&pack.creation_flows)?)
        .bind(serde_json::to_value(&pack.option_catalogs)?)
        .bind(serde_json::to_value(&pack.derived_formula_pack)?)
        .bind(serde_json::to_value(&pack.starter_character_pack)?)
        .bind(serde_json::to_value(&pack.runtime_bindings)?)
        .bind(serde_json::to_value(&pack.source_refs)?)
        .bind(serde_json::to_value(&pack.validation_report)?)
        .bind(content_hash)
        .execute(&self.pool)
        .await?;

        for flow in &pack.creation_flows {
            sqlx::query(
                r#"
                insert into character_creation_flows
                  (id, flow_id, ruleset_id, pack_id, mode, title, content_json, source_refs)
                values ($1,$2,$3,$4,$5,$6,$7,$8)
                on conflict (flow_id) do update set
                  ruleset_id = excluded.ruleset_id,
                  pack_id = excluded.pack_id,
                  mode = excluded.mode,
                  title = excluded.title,
                  content_json = excluded.content_json,
                  source_refs = excluded.source_refs,
                  updated_at = now()
                "#,
            )
            .bind(Uuid::new_v4())
            .bind(&flow.flow_id)
            .bind(&flow.ruleset_id)
            .bind(&pack.pack_id)
            .bind(flow.mode.as_str())
            .bind(&flow.title)
            .bind(serde_json::to_value(flow)?)
            .bind(serde_json::to_value(&flow.source_refs)?)
            .execute(&self.pool)
            .await?;
        }

        for catalog in &pack.option_catalogs {
            sqlx::query(
                r#"
                insert into character_option_catalogs
                  (id, catalog_id, ruleset_id, pack_id, title, content_json, source_refs)
                values ($1,$2,$3,$4,$5,$6,$7)
                on conflict (catalog_id) do update set
                  ruleset_id = excluded.ruleset_id,
                  pack_id = excluded.pack_id,
                  title = excluded.title,
                  content_json = excluded.content_json,
                  source_refs = excluded.source_refs,
                  updated_at = now()
                "#,
            )
            .bind(Uuid::new_v4())
            .bind(&catalog.catalog_id)
            .bind(&catalog.ruleset_id)
            .bind(&pack.pack_id)
            .bind(&catalog.title)
            .bind(serde_json::to_value(catalog)?)
            .bind(serde_json::to_value(&catalog.source_refs)?)
            .execute(&self.pool)
            .await?;
        }

        sqlx::query(
            r#"
            insert into starter_character_packs
              (id, pack_id, ruleset_id, module_id, title, content_json, source_refs)
            values ($1,$2,$3,$4,$5,$6,$7)
            on conflict (pack_id) do update set
              ruleset_id = excluded.ruleset_id,
              module_id = excluded.module_id,
              title = excluded.title,
              content_json = excluded.content_json,
              source_refs = excluded.source_refs,
              updated_at = now()
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&pack.starter_character_pack.pack_id)
        .bind(&pack.starter_character_pack.ruleset_id)
        .bind(&pack.starter_character_pack.module_id)
        .bind("Starter Character Pack")
        .bind(serde_json::to_value(&pack.starter_character_pack)?)
        .bind(serde_json::to_value(&pack.starter_character_pack.source_refs)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn load_character_onboarding_pack(&self, ruleset_id: &str) -> Result<Option<CharacterOnboardingPack>> {
        let row = sqlx::query(r#"select content_json from character_onboarding_packs where ruleset_id = $1 order by updated_at desc limit 1"#)
            .bind(ruleset_id)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(r) => Ok(Some(serde_json::from_value(r.get("content_json"))?)),
            None => Ok(None),
        }
    }

    pub async fn upsert_rule_kernel(&self, kernel: &RuleKernel) -> Result<()> {
        let content_hash = stable_json_hash(kernel);
        sqlx::query(
            r#"
            insert into rule_kernels
              (id, kernel_id, ruleset_id, version, content_json, source_refs, validation_report, content_hash, active)
            values ($1,$2,$3,$4,$5,$6,$7,$8,true)
            on conflict (kernel_id) do update set
              ruleset_id = excluded.ruleset_id,
              version = excluded.version,
              content_json = excluded.content_json,
              source_refs = excluded.source_refs,
              validation_report = excluded.validation_report,
              content_hash = excluded.content_hash,
              active = true,
              updated_at = now()
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&kernel.kernel_id)
        .bind(&kernel.ruleset_id)
        .bind(&kernel.version)
        .bind(serde_json::to_value(kernel)?)
        .bind(serde_json::to_value(&kernel.source_refs)?)
        .bind(serde_json::to_value(&kernel.validation_report)?)
        .bind(content_hash)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn load_rule_kernel(&self, ruleset_id: &str) -> Result<Option<RuleKernel>> {
        let row = sqlx::query(r#"select content_json from rule_kernels where ruleset_id = $1 and active = true order by updated_at desc limit 1"#)
            .bind(ruleset_id)
            .fetch_optional(&self.pool)
            .await?;
        let Some(r) = row else { return Ok(None); };
        let mut kernel: RuleKernel = serde_json::from_value(r.get("content_json"))?;
        // Read-time cleanup of dirty resource_tracks (e.g. a mis-submitted character
        // sheet field-def) so stale DB kernels are sanitized without a re-parse,
        // then layer an optional data-only override (override track id wins).
        kernel.resource_tracks = trpg_model::normalize_resource_tracks(&kernel.resource_tracks);
        // Layer the optional data-only override file (override wins). Covers BOTH
        // resource_tracks (merged by id) AND dice_core (shallow key merge — e.g. a
        // corrected success_bands array) so a kernel re-parse that under-extracts is
        // self-healing at read time, with no per-ruleset Rust.
        if let Some(doc) = read_kernel_override_file(ruleset_id) {
            if let Some(tracks) = doc.get("resource_tracks").and_then(|t| t.as_array()) {
                kernel.resource_tracks = merge_resource_tracks(kernel.resource_tracks, tracks.to_vec());
            }
            if let Some(dc) = doc.get("dice_core").and_then(|d| d.as_object()) {
                kernel.dice_core = merge_dice_core(kernel.dice_core, dc);
            }
        }
        Ok(Some(kernel))
    }

    /// The ruleset's core dice expression from the parsed kernel
    /// (`dice_core.dice`), trimmed; None if unparsed/empty. This replaces the
    /// hardcoded per-ruleset dice tables so roll sites read the data instead.
    pub async fn core_dice_expression(&self, ruleset_id: &str) -> Option<String> {
        let row = sqlx::query(r#"select content_json->'dice_core'->>'dice' as dice from rule_kernels where ruleset_id = $1 and active = true order by updated_at desc limit 1"#)
            .bind(ruleset_id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()?;
        let dice: Option<String> = row.get("dice");
        dice.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    }

    pub async fn upsert_rule_kernel_patch(&self, patch: &RuleKernelPatch) -> Result<()> {
        sqlx::query(
            r#"
            insert into rule_kernel_patches
              (id, patch_id, ruleset_id, target_kernel_version, patch_kind, old_hash, proposed_hash, diff_summary,
               json_patch, source_refs, confidence, contradiction_report, regression_tests, status, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,coalesce($15, now()))
            on conflict (patch_id) do update set
              status = excluded.status,
              diff_summary = excluded.diff_summary,
              json_patch = excluded.json_patch,
              source_refs = excluded.source_refs,
              confidence = excluded.confidence,
              contradiction_report = excluded.contradiction_report,
              regression_tests = excluded.regression_tests,
              updated_at = now()
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&patch.patch_id)
        .bind(&patch.ruleset_id)
        .bind(&patch.target_kernel_version)
        .bind(&patch.patch_kind)
        .bind(&patch.old_hash)
        .bind(&patch.proposed_hash)
        .bind(&patch.diff_summary)
        .bind(&patch.json_patch)
        .bind(serde_json::to_value(&patch.source_refs)?)
        .bind(patch.confidence)
        .bind(serde_json::to_value(&patch.contradiction_report)?)
        .bind(serde_json::to_value(&patch.regression_tests)?)
        .bind(&patch.status)
        .bind(patch.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_rule_kernel_patches(&self, ruleset_id: &str, status: Option<&str>, limit: i64) -> Result<Vec<RuleKernelPatch>> {
        let rows = sqlx::query(
            r#"
            select patch_id, ruleset_id, target_kernel_version, patch_kind, old_hash, proposed_hash, diff_summary,
                   json_patch, source_refs, confidence, contradiction_report, regression_tests, status, created_at
            from rule_kernel_patches
            where ruleset_id = $1 and ($2::text is null or status = $2)
            order by created_at desc
            limit $3
            "#,
        )
        .bind(ruleset_id)
        .bind(status.map(str::to_string))
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(|r| {
            let source_refs_json: serde_json::Value = r.get("source_refs");
            let regression_tests_json: serde_json::Value = r.get("regression_tests");
            let contradiction_report: Option<serde_json::Value> = r.get("contradiction_report");
            Ok(RuleKernelPatch {
                patch_id: r.get("patch_id"),
                ruleset_id: r.get("ruleset_id"),
                target_kernel_version: r.get("target_kernel_version"),
                patch_kind: r.get("patch_kind"),
                old_hash: r.get("old_hash"),
                proposed_hash: r.get("proposed_hash"),
                diff_summary: r.get("diff_summary"),
                json_patch: r.get("json_patch"),
                source_refs: serde_json::from_value(source_refs_json)?,
                confidence: r.get("confidence"),
                contradiction_report,
                regression_tests: serde_json::from_value(regression_tests_json)?,
                status: r.get("status"),
                created_at: r.get("created_at"),
            })
        }).collect()
    }

    pub async fn upsert_rule_agent_run(&self, run: &RuleAgentRun) -> Result<()> {
        sqlx::query(
            r#"
            insert into rule_agent_runs
              (id, run_id, session_id, turn_id, ruleset_id, module_id, trigger, selected_skill, tool_calls,
               source_refs_read, outputs_written, confidence, unresolved_count, contradiction_count, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,coalesce($15, now()))
            on conflict (run_id) do update set
              tool_calls = excluded.tool_calls,
              source_refs_read = excluded.source_refs_read,
              outputs_written = excluded.outputs_written,
              confidence = excluded.confidence,
              unresolved_count = excluded.unresolved_count,
              contradiction_count = excluded.contradiction_count
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&run.run_id)
        .bind(&run.session_id)
        .bind(&run.turn_id)
        .bind(&run.ruleset_id)
        .bind(&run.module_id)
        .bind(&run.trigger)
        .bind(&run.selected_skill)
        .bind(serde_json::to_value(&run.tool_calls)?)
        .bind(serde_json::to_value(&run.source_refs_read)?)
        .bind(serde_json::to_value(&run.outputs_written)?)
        .bind(run.confidence)
        .bind(run.unresolved_count as i32)
        .bind(run.contradiction_count as i32)
        .bind(run.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn save_playability_gate_report(&self, report: &PlayabilityGateReport) -> Result<()> {
        sqlx::query(
            r#"
            insert into playability_gate_reports
              (id, report_id, ruleset_id, module_id, ready, content_json, blocking_gaps, warnings, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,coalesce($9, now()))
            on conflict (report_id) do update set
              ready = excluded.ready,
              content_json = excluded.content_json,
              blocking_gaps = excluded.blocking_gaps,
              warnings = excluded.warnings
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&report.report_id)
        .bind(&report.ruleset_id)
        .bind(&report.module_id)
        .bind(report.ready)
        .bind(serde_json::to_value(report)?)
        .bind(serde_json::to_value(&report.blocking_gaps)?)
        .bind(serde_json::to_value(&report.warnings)?)
        .bind(report.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn has_module_first_session_packet(&self, module_id: &str) -> Result<bool> {
        let exists: bool = sqlx::query_scalar(r#"select exists(select 1 from module_prep_packets where module_id = $1)"#)
            .bind(module_id)
            .fetch_one(&self.pool)
            .await?;
        Ok(exists)
    }

    pub async fn list_context_blocks_for_bundles(&self, bundle_ids: &[String]) -> Result<Vec<ContextBlock>> {
        if bundle_ids.is_empty() {
            return Ok(vec![]);
        }
        let rows = sqlx::query(
            r#"
            select block_id, block_kind, title, scope_type, scope_id, visibility, stability, cache_zone, priority, version,
                   content_json, content_hash, token_estimate, source_refs, dependencies, tags, expires_at_turn, expires_at_scene, load_reason
            from context_blocks
            where bundle_id = any($1) and active = true
            "#,
        )
        .bind(bundle_ids)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_context_block).collect()
    }

    pub async fn find_material_blocks(&self, bundle_ids: &[String], material_refs: &[String]) -> Result<Vec<ContextBlock>> {
        if bundle_ids.is_empty() || material_refs.is_empty() {
            return Ok(vec![]);
        }
        let rows = sqlx::query(
            r#"
            select cb.block_id, cb.block_kind, cb.title, cb.scope_type, cb.scope_id, cb.visibility, cb.stability,
                   cb.cache_zone, cb.priority, cb.version, cb.content_json, cb.content_hash, cb.token_estimate,
                   cb.source_refs, cb.dependencies, cb.tags, cb.expires_at_turn, cb.expires_at_scene, cb.load_reason
            from material_index mi
            join context_blocks cb on cb.bundle_id = mi.bundle_id and cb.block_id = mi.extracted_block_id
            where mi.bundle_id = any($1) and mi.material_id = any($2) and cb.active = true
            "#,
        )
        .bind(bundle_ids)
        .bind(material_refs)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_context_block).collect()
    }

    pub async fn save_character(&self, character: &CharacterSheet, postprocess_status: &str) -> Result<()> {
        sqlx::query(
            r#"
            insert into characters
              (id, character_id, ruleset_id, template_id, name, sheet_json, validation_report, postprocess_status)
            values ($1,$2,$3,$4,$5,$6,$7,$8)
            on conflict (character_id) do update set
              ruleset_id = excluded.ruleset_id,
              template_id = excluded.template_id,
              name = excluded.name,
              sheet_json = excluded.sheet_json,
              validation_report = excluded.validation_report,
              postprocess_status = excluded.postprocess_status,
              updated_at = now()
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&character.character_id)
        .bind(&character.ruleset_id)
        .bind(&character.template_id)
        .bind(&character.name)
        .bind(serde_json::to_value(&character.sheet)?)
        .bind(serde_json::to_value(&character.validation_report)?)
        .bind(postprocess_status)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn create_session(&self, session_id: &str, ruleset_id: &str, module_id: Option<&str>) -> Result<()> {
        sqlx::query(
            r#"
            insert into sessions (id, session_id, ruleset_id, module_id)
            values ($1,$2,$3,$4)
            on conflict (session_id) do update set
              ruleset_id = excluded.ruleset_id,
              module_id = excluded.module_id,
              updated_at = now()
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(session_id)
        .bind(ruleset_id)
        .bind(module_id.map(str::to_string))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 设置该会话当前所在的模组场景 node_id（场景导航）。
    pub async fn set_session_scene(&self, session_id: &str, scene_id: &str) -> Result<()> {
        sqlx::query("update sessions set current_scene_id = $2, updated_at = now() where session_id = $1")
            .bind(session_id)
            .bind(scene_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// 读取该会话当前模组场景 node_id（无则 None）。
    pub async fn load_session_scene(&self, session_id: &str) -> Result<Option<String>> {
        let row: Option<(Option<String>,)> = sqlx::query_as("select current_scene_id from sessions where session_id = $1")
            .bind(session_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.and_then(|r| r.0).filter(|s| !s.trim().is_empty()))
    }

    pub async fn save_turn(&self, session_id: &str, turn_id: &str, user_input: &str, assistant_output: &str, context_hashes: serde_json::Value, postprocess_status: &str) -> Result<()> {
        sqlx::query(
            r#"
            insert into turns (id, session_id, turn_id, user_input, assistant_output, context_hashes, postprocess_status)
            values ($1,$2,$3,$4,$5,$6,$7)
            on conflict (session_id, turn_id) do update set
              user_input = excluded.user_input,
              assistant_output = excluded.assistant_output,
              context_hashes = excluded.context_hashes,
              postprocess_status = excluded.postprocess_status,
              updated_at = now()
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(session_id)
        .bind(turn_id)
        .bind(user_input)
        .bind(assistant_output)
        .bind(context_hashes)
        .bind(postprocess_status)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn record_load_event(&self, session_id: Option<&str>, turn_id: Option<&str>, block: &ContextBlock, reason: &str) -> Result<()> {
        sqlx::query(
            r#"insert into context_block_load_events
               (id, session_id, turn_id, block_id, block_version, cache_zone, load_reason, token_estimate)
               values ($1,$2,$3,$4,$5,$6,$7,$8)"#,
        )
        .bind(Uuid::new_v4())
        .bind(session_id.map(str::to_string))
        .bind(turn_id.map(str::to_string))
        .bind(&block.block_id)
        .bind(block.version as i32)
        .bind(block.cache_zone.as_str())
        .bind(reason)
        .bind(block.token_estimate.map(|v| v as i32))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn save_memory_event(&self, event: &MemoryEvent) -> Result<()> {
        sqlx::query(
            r#"
            insert into memory_events
              (id, event_id, session_id, turn_id, ruleset_id, module_id, scene_id, location_id, actor_ids,
               visibility, event_kind, summary, transcript_excerpt, source_json, tags, importance, occurred_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)
            on conflict (event_id) do update set
              session_id = excluded.session_id,
              turn_id = excluded.turn_id,
              ruleset_id = excluded.ruleset_id,
              module_id = excluded.module_id,
              scene_id = excluded.scene_id,
              location_id = excluded.location_id,
              actor_ids = excluded.actor_ids,
              visibility = excluded.visibility,
              event_kind = excluded.event_kind,
              summary = excluded.summary,
              transcript_excerpt = excluded.transcript_excerpt,
              source_json = excluded.source_json,
              tags = excluded.tags,
              importance = excluded.importance,
              occurred_at = excluded.occurred_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&event.event_id)
        .bind(&event.session_id)
        .bind(&event.turn_id)
        .bind(&event.ruleset_id)
        .bind(&event.module_id)
        .bind(&event.scene_id)
        .bind(&event.location_id)
        .bind(&event.actor_ids)
        .bind(event.visibility.as_str())
        .bind(format!("{:?}", event.event_kind).to_snake())
        .bind(&event.summary)
        .bind(&event.transcript_excerpt)
        .bind(&event.source)
        .bind(&event.tags)
        .bind(event.importance)
        .bind(event.occurred_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn upsert_memory_fact(&self, fact: &MemoryFact) -> Result<()> {
        sqlx::query(
            r#"
            insert into memory_facts
              (id, fact_id, session_id, scope_type, scope_id, visibility, subject, predicate, object_json,
               summary, status, confidence, source_event_ids, tags, importance, created_at, updated_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)
            on conflict (fact_id) do update set
              scope_type = excluded.scope_type,
              scope_id = excluded.scope_id,
              visibility = excluded.visibility,
              subject = excluded.subject,
              predicate = excluded.predicate,
              object_json = excluded.object_json,
              summary = excluded.summary,
              status = excluded.status,
              confidence = excluded.confidence,
              source_event_ids = excluded.source_event_ids,
              tags = excluded.tags,
              importance = excluded.importance,
              updated_at = excluded.updated_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&fact.fact_id)
        .bind(&fact.session_id)
        .bind(format!("{:?}", fact.scope.scope_type).to_snake())
        .bind(&fact.scope.scope_id)
        .bind(fact.visibility.as_str())
        .bind(&fact.subject)
        .bind(&fact.predicate)
        .bind(&fact.object)
        .bind(&fact.summary)
        .bind(fact.status.as_str())
        .bind(fact.confidence)
        .bind(&fact.source_event_ids)
        .bind(&fact.tags)
        .bind(fact.importance)
        .bind(fact.created_at)
        .bind(fact.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn upsert_memory_snapshot(&self, snapshot: &MemorySnapshot) -> Result<()> {
        sqlx::query(
            r#"
            insert into memory_snapshots
              (id, snapshot_id, session_id, ruleset_id, module_id, scope_type, scope_id, visibility, title,
               summary_markdown, included_event_ids, included_fact_ids, version, token_estimate, content_hash, created_at, updated_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)
            on conflict (snapshot_id, version) do update set
              ruleset_id = excluded.ruleset_id,
              module_id = excluded.module_id,
              scope_type = excluded.scope_type,
              scope_id = excluded.scope_id,
              visibility = excluded.visibility,
              title = excluded.title,
              summary_markdown = excluded.summary_markdown,
              included_event_ids = excluded.included_event_ids,
              included_fact_ids = excluded.included_fact_ids,
              token_estimate = excluded.token_estimate,
              content_hash = excluded.content_hash,
              updated_at = excluded.updated_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&snapshot.snapshot_id)
        .bind(&snapshot.session_id)
        .bind(&snapshot.ruleset_id)
        .bind(&snapshot.module_id)
        .bind(format!("{:?}", snapshot.scope.scope_type).to_snake())
        .bind(&snapshot.scope.scope_id)
        .bind(snapshot.visibility.as_str())
        .bind(&snapshot.title)
        .bind(&snapshot.summary_markdown)
        .bind(&snapshot.included_event_ids)
        .bind(&snapshot.included_fact_ids)
        .bind(snapshot.version as i32)
        .bind(snapshot.token_estimate.map(|v| v as i32))
        .bind(&snapshot.content_hash)
        .bind(snapshot.created_at)
        .bind(snapshot.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_memory_events(&self, session_id: &str, limit: i64) -> Result<Vec<MemoryEvent>> {
        let rows = sqlx::query(
            r#"select event_id, session_id, turn_id, ruleset_id, module_id, scene_id, location_id, actor_ids,
                      visibility, event_kind, summary, transcript_excerpt, source_json, tags, importance, occurred_at
               from memory_events
               where session_id = $1
               order by occurred_at desc
               limit $2"#,
        )
        .bind(session_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_memory_event).collect()
    }

    pub async fn list_memory_facts(&self, session_id: &str, limit: i64) -> Result<Vec<MemoryFact>> {
        let rows = sqlx::query(
            r#"select fact_id, session_id, scope_type, scope_id, visibility, subject, predicate, object_json,
                      summary, status, confidence, source_event_ids, tags, importance, created_at, updated_at
               from memory_facts
               where session_id = $1 and status = 'active'
               order by importance desc, updated_at desc
               limit $2"#,
        )
        .bind(session_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_memory_fact).collect()
    }

    pub async fn list_memory_snapshots(&self, session_id: &str, limit: i64) -> Result<Vec<MemorySnapshot>> {
        let rows = sqlx::query(
            r#"select snapshot_id, session_id, ruleset_id, module_id, scope_type, scope_id, visibility, title,
                      summary_markdown, included_event_ids, included_fact_ids, version, token_estimate, content_hash, created_at, updated_at
               from memory_snapshots
               where session_id = $1
               order by updated_at desc, version desc
               limit $2"#,
        )
        .bind(session_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_memory_snapshot).collect()
    }

    pub async fn retrieve_memory(&self, query: &MemoryQuery) -> Result<MemoryRetrievalResult> {
        let limit = i64::from(query.limit.max(1).min(50));
        let pattern = format!("%{}%", query.text.replace('%', "\\%").replace('_', "\\_"));
        let facts_rows = sqlx::query(
            r#"select fact_id, session_id, scope_type, scope_id, visibility, subject, predicate, object_json,
                      summary, status, confidence, source_event_ids, tags, importance, created_at, updated_at
               from memory_facts
               where session_id = $1
                 and status = 'active'
                 and ($2 = '' or subject ilike $3 or predicate ilike $3 or summary ilike $3 or object_json::text ilike $3 or tags && $4::text[])
               order by importance desc, updated_at desc
               limit $5"#,
        )
        .bind(&query.session_id)
        .bind(&query.text)
        .bind(&pattern)
        .bind(&query.tags)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        let events_rows = sqlx::query(
            r#"select event_id, session_id, turn_id, ruleset_id, module_id, scene_id, location_id, actor_ids,
                      visibility, event_kind, summary, transcript_excerpt, source_json, tags, importance, occurred_at
               from memory_events
               where session_id = $1
                 and ($2 = '' or summary ilike $3 or coalesce(transcript_excerpt, '') ilike $3 or tags && $4::text[])
               order by importance desc, occurred_at desc
               limit $5"#,
        )
        .bind(&query.session_id)
        .bind(&query.text)
        .bind(&pattern)
        .bind(&query.tags)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        let snapshots = self.list_memory_snapshots(&query.session_id, 3).await?;
        let facts: Result<Vec<_>> = facts_rows.into_iter().map(row_to_memory_fact).collect();
        let events: Result<Vec<_>> = events_rows.into_iter().map(row_to_memory_event).collect();
        Ok(MemoryRetrievalResult { snapshots, facts: facts?, events: events?, blocks: vec![] })
    }

    pub async fn insert_background_job(&self, job_id: &str, job_kind: &str, input_json: serde_json::Value) -> Result<()> {
        sqlx::query(
            r#"insert into background_jobs (id, job_id, job_kind, status, input_json) values ($1,$2,$3,'queued',$4)
               on conflict (job_id) do update set status = 'queued', input_json = excluded.input_json, error = null, updated_at = now()"#,
        )
        .bind(Uuid::new_v4())
        .bind(job_id)
        .bind(job_kind)
        .bind(input_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_background_job(&self, job_id: &str, status: &str, result_json: serde_json::Value, error: Option<&str>) -> Result<()> {
        sqlx::query(
            r#"update background_jobs set status = $2, result_json = $3, error = $4, updated_at = now() where job_id = $1"#,
        )
        .bind(job_id)
        .bind(status)
        .bind(result_json)
        .bind(error.map(str::to_string))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Read `background_jobs.status` for a job id (no such job -> `None`).
    /// Used by the extract/continue handler to skip re-enqueueing a job that is
    /// already `running`, avoiding a concurrent read-modify-write race.
    pub async fn background_job_status(&self, job_id: &str) -> Result<Option<String>> {
        let status: Option<String> = sqlx::query_scalar(r#"select status from background_jobs where job_id = $1"#)
            .bind(job_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(status)
    }

    /// Read a `background_jobs` row as a small JSON object carrying `status`,
    /// `result_json`, and `error`. Returns `None` if the job does not exist.
    /// Used by the staged-parse status/SSE endpoints to surface live progress.
    pub async fn load_background_job(&self, job_id: &str) -> Result<Option<serde_json::Value>> {
        let row = sqlx::query(r#"select status, result_json, error from background_jobs where job_id = $1"#)
            .bind(job_id)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(r) => {
                let status: String = r.get("status");
                let result_json: serde_json::Value = r.try_get("result_json").unwrap_or(serde_json::Value::Null);
                let error: Option<String> = r.try_get("error").unwrap_or(None);
                Ok(Some(serde_json::json!({
                    "status": status,
                    "result_json": result_json,
                    "error": error,
                })))
            }
            None => Ok(None),
        }
    }

    /// Find the most recent staged-parse job (`job_kind = 'ruleset_parse_staged'`)
    /// for a ruleset and return its `result_json` (the `JobStatus` snapshot).
    /// Used to populate the `{stage, progress_pct}` body of a 202 gate response.
    pub async fn latest_staged_job_for_ruleset(&self, ruleset_id: &str) -> Result<Option<serde_json::Value>> {
        let row = sqlx::query(
            r#"select result_json from background_jobs
               where job_kind = 'ruleset_parse_staged' and input_json->>'ruleset' = $1
               order by created_at desc limit 1"#,
        )
        .bind(ruleset_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|r| r.try_get::<serde_json::Value, _>("result_json").ok()))
    }

    pub async fn upsert_onboarding_bundle(&self, bundle: &GmOnboardingBundle) -> Result<()> {
        let bundle_json = serde_json::to_value(bundle)?;
        let source_refs = serde_json::to_value(&bundle.source_refs)?;
        let content_hash = stable_json_hash(bundle);
        sqlx::query(
            r#"
            insert into ruleset_onboarding_bundles
              (id, onboarding_id, ruleset_id, title, bundle_json, source_refs, content_hash)
            values ($1,$2,$3,$4,$5,$6,$7)
            on conflict (onboarding_id) do update set
              ruleset_id = excluded.ruleset_id,
              title = excluded.title,
              bundle_json = excluded.bundle_json,
              source_refs = excluded.source_refs,
              content_hash = excluded.content_hash,
              updated_at = now()
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&bundle.onboarding_id)
        .bind(&bundle.ruleset_id)
        .bind(&bundle.title)
        .bind(bundle_json)
        .bind(source_refs)
        .bind(content_hash)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn upsert_book_locator_entry(&self, entry: &BookLocatorEntry) -> Result<()> {
        let search_terms = serde_json::to_value(&entry.search_terms)?;
        let source_refs = serde_json::to_value(&entry.source_refs)?;
        sqlx::query(
            r#"
            insert into book_locator_entries
              (id, locator_id, owner_id, owner_kind, label, category, source_document_id,
               page_start, page_end, heading_path, search_terms, summary, confidence, parse_policy, tags, source_refs)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)
            on conflict (locator_id) do update set
              owner_id = excluded.owner_id,
              owner_kind = excluded.owner_kind,
              label = excluded.label,
              category = excluded.category,
              source_document_id = excluded.source_document_id,
              page_start = excluded.page_start,
              page_end = excluded.page_end,
              heading_path = excluded.heading_path,
              search_terms = excluded.search_terms,
              summary = excluded.summary,
              confidence = excluded.confidence,
              parse_policy = excluded.parse_policy,
              tags = excluded.tags,
              source_refs = excluded.source_refs,
              updated_at = now()
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&entry.locator_id)
        .bind(&entry.owner_id)
        .bind(&entry.owner_kind)
        .bind(&entry.label)
        .bind(&entry.category)
        .bind(&entry.source_document_id)
        .bind(entry.page_start.map(|v| v as i32))
        .bind(entry.page_end.map(|v| v as i32))
        .bind(&entry.heading_path)
        .bind(search_terms)
        .bind(&entry.summary)
        .bind(entry.confidence)
        .bind(&entry.parse_policy)
        .bind(&entry.tags)
        .bind(source_refs)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn upsert_module_prep_packet(&self, packet: &ModulePrepPacket) -> Result<()> {
        let packet_json = serde_json::to_value(packet)?;
        let source_refs = serde_json::to_value(&packet.source_refs)?;
        let content_hash = stable_json_hash(packet);
        sqlx::query(
            r#"
            insert into module_prep_packets
              (id, prep_id, module_id, ruleset_id, mode, title, packet_json, source_refs, content_hash)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9)
            on conflict (prep_id) do update set
              module_id = excluded.module_id,
              ruleset_id = excluded.ruleset_id,
              mode = excluded.mode,
              title = excluded.title,
              packet_json = excluded.packet_json,
              source_refs = excluded.source_refs,
              content_hash = excluded.content_hash,
              updated_at = now()
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&packet.prep_id)
        .bind(&packet.module_id)
        .bind(&packet.ruleset_id)
        .bind(&packet.mode)
        .bind(&packet.title)
        .bind(packet_json)
        .bind(source_refs)
        .bind(content_hash)
        .execute(&self.pool)
        .await?;
        Ok(())
    }


    pub async fn search_book_locator_entries(&self, owner_id: Option<&str>, query_text: &str, limit: i64) -> Result<Vec<serde_json::Value>> {
        let pattern = format!("%{}%", query_text.replace('%', "\\%").replace('_', "\\_"));
        let rows = sqlx::query(
            r#"
            select locator_id, owner_id, owner_kind, label, category, source_document_id,
                   page_start, page_end, heading_path, search_terms, summary, confidence, parse_policy, tags, source_refs
            from book_locator_entries
            where ($1::text is null or owner_id = $1)
              and ($2 = '' or label ilike $3 or category ilike $3 or summary ilike $3 or search_terms::text ilike $3)
            order by confidence desc, label asc
            limit $4
            "#,
        )
        .bind(owner_id.map(str::to_string))
        .bind(query_text)
        .bind(&pattern)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::new();
        for row in rows {
            out.push(json!({
                "locator_id": row.get::<String,_>("locator_id"),
                "owner_id": row.get::<String,_>("owner_id"),
                "owner_kind": row.get::<String,_>("owner_kind"),
                "label": row.get::<String,_>("label"),
                "category": row.get::<String,_>("category"),
                "source_document_id": row.get::<String,_>("source_document_id"),
                "page_start": row.get::<Option<i32>,_>("page_start"),
                "page_end": row.get::<Option<i32>,_>("page_end"),
                "heading_path": row.get::<Vec<String>,_>("heading_path"),
                "search_terms": row.get::<serde_json::Value,_>("search_terms"),
                "summary": row.get::<String,_>("summary"),
                "confidence": row.get::<f32,_>("confidence"),
                "parse_policy": row.get::<String,_>("parse_policy"),
                "tags": row.get::<Vec<String>,_>("tags"),
                "source_refs": row.get::<serde_json::Value,_>("source_refs"),
            }));
        }
        Ok(out)
    }

    pub async fn insert_lookup_event(&self, event: &LookupEvent) -> Result<()> {
        sqlx::query(
            r#"
            insert into lookup_events
              (id, event_id, session_id, ruleset_id, module_id, demand_id, query_text, search_terms, source_hits, result_status, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
            on conflict (event_id) do update set
              source_hits = excluded.source_hits,
              result_status = excluded.result_status
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&event.event_id)
        .bind(&event.session_id)
        .bind(&event.ruleset_id)
        .bind(&event.module_id)
        .bind(&event.demand_id)
        .bind(&event.query_text)
        .bind(serde_json::to_value(&event.search_terms)?)
        .bind(&event.source_hits)
        .bind(&event.result_status)
        .bind(event.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_ruling_log(&self, ruling: &RulingLog) -> Result<()> {
        sqlx::query(
            r#"
            insert into rulings_log
              (id, ruling_id, session_id, ruleset_id, module_id, demand_id, ruling_text, source_refs, status, confidence, provisional, superseded_by, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
            on conflict (ruling_id) do update set
              ruling_text = excluded.ruling_text,
              source_refs = excluded.source_refs,
              status = excluded.status,
              confidence = excluded.confidence,
              provisional = excluded.provisional,
              superseded_by = excluded.superseded_by
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&ruling.ruling_id)
        .bind(&ruling.session_id)
        .bind(&ruling.ruleset_id)
        .bind(&ruling.module_id)
        .bind(&ruling.demand_id)
        .bind(&ruling.ruling_text)
        .bind(serde_json::to_value(&ruling.source_refs)?)
        .bind(ruling.status.as_str())
        .bind(ruling.confidence.as_str())
        .bind(ruling.provisional)
        .bind(&ruling.superseded_by)
        .bind(ruling.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn upsert_learned_packet(&self, packet: &LearnedPacket) -> Result<()> {
        sqlx::query(
            r#"
            insert into learned_packets
              (id, packet_id, ruleset_id, module_id, packet_type, packet_key, title, summary, packet_json, source_refs,
               use_count, learning_stage, confidence, cache_zone, visibility, last_used_at, created_at, updated_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)
            on conflict (packet_id) do update set
              title = excluded.title,
              summary = excluded.summary,
              packet_json = excluded.packet_json,
              source_refs = excluded.source_refs,
              use_count = excluded.use_count,
              learning_stage = excluded.learning_stage,
              confidence = excluded.confidence,
              cache_zone = excluded.cache_zone,
              visibility = excluded.visibility,
              last_used_at = excluded.last_used_at,
              updated_at = excluded.updated_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&packet.packet_id)
        .bind(&packet.ruleset_id)
        .bind(&packet.module_id)
        .bind(&packet.packet_type)
        .bind(&packet.packet_key)
        .bind(&packet.title)
        .bind(&packet.summary)
        .bind(&packet.packet_json)
        .bind(serde_json::to_value(&packet.source_refs)?)
        .bind(packet.use_count)
        .bind(packet.learning_stage.as_str())
        .bind(packet.confidence.as_str())
        .bind(packet.cache_zone.as_str())
        .bind(packet.visibility.as_str())
        .bind(packet.last_used_at)
        .bind(packet.created_at)
        .bind(packet.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_learned_packets(&self, ruleset_id: &str, module_id: Option<&str>, limit: i64) -> Result<Vec<LearnedPacket>> {
        let rows = sqlx::query(
            r#"
            select packet_id, ruleset_id, module_id, packet_type, packet_key, title, summary, packet_json, source_refs,
                   use_count, learning_stage, confidence, cache_zone, visibility, last_used_at, created_at, updated_at
            from learned_packets
            where ruleset_id = $1 and (module_id is null or module_id = $2)
              and learning_stage in ('used_once', 'stable', 'memorized')
            order by case learning_stage when 'memorized' then 3 when 'stable' then 2 else 1 end desc,
                     use_count desc, updated_at desc
            limit $3
            "#,
        )
        .bind(ruleset_id)
        .bind(module_id.map(str::to_string))
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_learned_packet).collect()
    }


    pub async fn list_lookup_events_for_demand(&self, session_id: Option<&str>, demand_id: &str, limit: i64) -> Result<Vec<LookupEvent>> {
        let rows = sqlx::query(
            r#"
            select event_id, session_id, ruleset_id, module_id, demand_id, query_text, search_terms, source_hits, result_status, created_at
            from lookup_events
            where demand_id = $1 and ($2::text is null or session_id = $2)
            order by created_at desc
            limit $3
            "#,
        )
        .bind(demand_id)
        .bind(session_id.map(str::to_string))
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_lookup_event).collect()
    }

    pub async fn insert_learning_audit_run(&self, audit_run_id: &str, session_id: Option<&str>, turn_id: Option<&str>, ruleset_id: Option<&str>, module_id: Option<&str>, status: &str, input_json: serde_json::Value, result_json: serde_json::Value, error: Option<&str>) -> Result<()> {
        sqlx::query(
            r#"
            insert into learning_audit_runs
              (id, audit_run_id, session_id, turn_id, ruleset_id, module_id, status, input_json, result_json, error, created_at, updated_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,now(),now())
            on conflict (audit_run_id) do update set
              status = excluded.status,
              result_json = excluded.result_json,
              error = excluded.error,
              updated_at = now()
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(audit_run_id)
        .bind(session_id.map(str::to_string))
        .bind(turn_id.map(str::to_string))
        .bind(ruleset_id.map(str::to_string))
        .bind(module_id.map(str::to_string))
        .bind(status)
        .bind(input_json)
        .bind(result_json)
        .bind(error.map(str::to_string))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_learning_candidate(&self, candidate: &LearningAuditCandidate) -> Result<()> {
        sqlx::query(
            r#"
            insert into learning_candidates
              (id, candidate_id, session_id, turn_id, ruleset_id, module_id, demand_id, packet_type, packet_key, title, summary, packet_json, source_refs, evidence_score, risk_flags, verifier_status, verifier_notes, created_at, updated_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19)
            on conflict (candidate_id) do update set
              title = excluded.title,
              summary = excluded.summary,
              packet_json = excluded.packet_json,
              source_refs = excluded.source_refs,
              evidence_score = excluded.evidence_score,
              risk_flags = excluded.risk_flags,
              verifier_status = excluded.verifier_status,
              verifier_notes = excluded.verifier_notes,
              updated_at = excluded.updated_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&candidate.candidate_id)
        .bind(&candidate.session_id)
        .bind(&candidate.turn_id)
        .bind(&candidate.ruleset_id)
        .bind(&candidate.module_id)
        .bind(&candidate.demand_id)
        .bind(&candidate.packet_type)
        .bind(&candidate.packet_key)
        .bind(&candidate.title)
        .bind(&candidate.summary)
        .bind(&candidate.packet_json)
        .bind(serde_json::to_value(&candidate.source_refs)?)
        .bind(candidate.evidence_score)
        .bind(&candidate.risk_flags)
        .bind(candidate.verifier_status.as_str())
        .bind(&candidate.verifier_notes)
        .bind(candidate.created_at)
        .bind(candidate.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_learning_candidate(&self, candidate_id: &str) -> Result<Option<LearningAuditCandidate>> {
        let row = sqlx::query(
            r#"
            select candidate_id, session_id, turn_id, ruleset_id, module_id, demand_id, packet_type, packet_key, title, summary, packet_json, source_refs, evidence_score, risk_flags, verifier_status, verifier_notes, created_at, updated_at
            from learning_candidates
            where candidate_id = $1
            "#,
        )
        .bind(candidate_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_learning_candidate).transpose()
    }

    pub async fn list_learning_candidates(&self, ruleset_id: Option<&str>, status: Option<&str>, limit: i64) -> Result<Vec<LearningAuditCandidate>> {
        let rows = sqlx::query(
            r#"
            select candidate_id, session_id, turn_id, ruleset_id, module_id, demand_id, packet_type, packet_key, title, summary, packet_json, source_refs, evidence_score, risk_flags, verifier_status, verifier_notes, created_at, updated_at
            from learning_candidates
            where ($1::text is null or ruleset_id = $1)
              and ($2::text is null or verifier_status = $2)
            order by evidence_score desc, updated_at desc
            limit $3
            "#,
        )
        .bind(ruleset_id.map(str::to_string))
        .bind(status.map(str::to_string))
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_learning_candidate).collect()
    }

    pub async fn update_learning_candidate_status(&self, candidate_id: &str, status: LearningCandidateStatus, notes: Option<&str>) -> Result<()> {
        sqlx::query(
            r#"
            update learning_candidates
            set verifier_status = $2, verifier_notes = $3, updated_at = now()
            where candidate_id = $1
            "#,
        )
        .bind(candidate_id)
        .bind(status.as_str())
        .bind(notes.map(str::to_string))
        .execute(&self.pool)
        .await?;
        Ok(())
    }


    pub async fn insert_agent_turn(&self, plan: &AgentTurnPlan, status: &str) -> Result<()> {
        sqlx::query(
            r#"
            insert into agent_turns
              (id, session_id, turn_id, ruleset_id, module_id, plan_json, status, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8)
            on conflict (session_id, turn_id) do update set
              plan_json = excluded.plan_json,
              status = excluded.status
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&plan.session_id)
        .bind(&plan.turn_id)
        .bind(&plan.ruleset_id)
        .bind(&plan.module_id)
        .bind(serde_json::to_value(plan)?)
        .bind(status)
        .bind(plan.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_agent_tool_call(&self, call: &AgentToolCallRecord) -> Result<()> {
        sqlx::query(
            r#"
            insert into agent_tool_calls
              (id, tool_call_id, session_id, turn_id, tool_name, visibility, input_json, output_json, status, error, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
            on conflict (tool_call_id) do update set
              output_json = excluded.output_json,
              status = excluded.status,
              error = excluded.error
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&call.tool_call_id)
        .bind(&call.session_id)
        .bind(&call.turn_id)
        .bind(&call.tool_name)
        .bind(call.visibility.as_str())
        .bind(&call.input_json)
        .bind(&call.output_json)
        .bind(&call.status)
        .bind(&call.error)
        .bind(call.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_check_contract(&self, contract: &CheckContract, status: &str) -> Result<()> {
        sqlx::query(
            r#"
            insert into check_contracts
              (id, check_id, session_id, turn_id, ruleset_id, module_id, actor_id, target_actor_id, roll_visibility, contract_json, status, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,now())
            on conflict (check_id) do update set
              contract_json = excluded.contract_json,
              status = excluded.status
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&contract.check_id)
        .bind(&contract.session_id)
        .bind(&contract.turn_id)
        .bind(&contract.ruleset_id)
        .bind(&contract.module_id)
        .bind(&contract.initiator.actor_id)
        .bind(contract.target_actor.as_ref().map(|a| a.actor_id.clone()))
        .bind(contract.roll_visibility.as_str())
        .bind(serde_json::to_value(contract)?)
        .bind(status)
        .execute(&self.pool)
        .await?;
        Ok(())
    }


    // ------------------------------------------------------------------
    // v1.5 Interaction Lifecycle Kernel persistence helpers
    // ------------------------------------------------------------------

    // v1.5 Interaction Lifecycle Kernel persistence helpers
    // ------------------------------------------------------------------

    pub async fn ensure_interaction_generation(&self, session_id: &str) -> Result<i64> {
        sqlx::query("update sessions set interaction_generation = coalesce(interaction_generation, 0) where session_id = $1")
            .bind(session_id)
            .execute(&self.pool)
            .await
            .ok();
        let generation: Option<i64> = sqlx::query_scalar("select coalesce(interaction_generation, 0) from sessions where session_id = $1")
            .bind(session_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(generation.unwrap_or(0))
    }

    pub async fn bump_interaction_generation(&self, session_id: &str) -> Result<i64> {
        let generation: Option<i64> = sqlx::query_scalar(
            r#"
            update sessions
            set interaction_generation = coalesce(interaction_generation, 0) + 1,
                active_interaction_gate_id = null,
                active_interaction_context_id = null,
                updated_at = now()
            where session_id = $1
            returning interaction_generation
            "#,
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(generation.unwrap_or(0))
    }

    pub async fn upsert_interaction_context(&self, ctx: &InteractionContext) -> Result<()> {
        sqlx::query(
            r#"
            insert into interaction_contexts
              (id, context_id, session_id, frame_id, context_kind, status, generation,
               active_gate_ids, pending_check_ids, reaction_window_ids, child_context_ids,
               opened_at_tick, closed_at_tick, owner_json, context_json, created_at, updated_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)
            on conflict (context_id) do update set
              status = excluded.status,
              generation = excluded.generation,
              active_gate_ids = excluded.active_gate_ids,
              pending_check_ids = excluded.pending_check_ids,
              reaction_window_ids = excluded.reaction_window_ids,
              child_context_ids = excluded.child_context_ids,
              closed_at_tick = excluded.closed_at_tick,
              owner_json = excluded.owner_json,
              context_json = excluded.context_json,
              updated_at = excluded.updated_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&ctx.context_id)
        .bind(&ctx.session_id)
        .bind(&ctx.frame_id)
        .bind(ctx.context_kind.as_str())
        .bind(ctx.status.as_str())
        .bind(ctx.generation)
        .bind(&ctx.active_gate_ids)
        .bind(&ctx.pending_check_ids)
        .bind(&ctx.reaction_window_ids)
        .bind(&ctx.child_context_ids)
        .bind(ctx.opened_at_tick)
        .bind(ctx.closed_at_tick)
        .bind(serde_json::to_value(&ctx.owner)?)
        .bind(&ctx.context_json)
        .bind(ctx.created_at)
        .bind(ctx.updated_at)
        .execute(&self.pool)
        .await?;
        if matches!(ctx.status, InteractionContextStatus::Active | InteractionContextStatus::Paused | InteractionContextStatus::Resolving) {
            sqlx::query("update sessions set active_interaction_context_id = $2, updated_at = now() where session_id = $1")
                .bind(&ctx.session_id)
                .bind(&ctx.context_id)
                .execute(&self.pool)
                .await
                .ok();
        }
        Ok(())
    }

    pub async fn insert_invariant_repair(&self, session_id: &str, repair: &InvariantRepair) -> Result<()> {
        sqlx::query(
            r#"
            insert into invariant_repairs
              (id, repair_id, session_id, invariant, target_table, target_id, action, reason, world_tick, generation, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,now())
            on conflict (repair_id) do nothing
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&repair.repair_id)
        .bind(session_id)
        .bind(repair.kind.as_str())
        .bind(&repair.target_table)
        .bind(&repair.target_id)
        .bind(&repair.action)
        .bind(&repair.reason)
        .bind(repair.world_tick)
        .bind(repair.generation)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_interaction_event(&self, event: &InteractionEvent) -> Result<()> {
        sqlx::query(
            r#"
            insert into interaction_events
              (id, event_id, session_id, interaction_context_id, frame_id, world_tick, generation, event_kind, event_json, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
            on conflict (event_id) do nothing
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&event.event_id)
        .bind(&event.session_id)
        .bind(&event.interaction_context_id)
        .bind(&event.frame_id)
        .bind(event.world_tick)
        .bind(event.generation)
        .bind(event.event_kind.as_str())
        .bind(&event.event_json)
        .bind(event.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn attach_gate_to_frame(&self, gate_id: &str, frame_id: &str, generation: i64) -> Result<()> {
        sqlx::query(
            r#"
            update interaction_gates
            set owner_frame_id = $2,
                interaction_context_id = coalesce(interaction_context_id, 'ctx_' || $2),
                generation = $3,
                updated_at = now()
            where gate_id = $1
            "#,
        )
        .bind(gate_id)
        .bind(frame_id)
        .bind(generation)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn attach_pending_check_to_frame(&self, check_id: &str, frame_id: &str, gate_id: Option<&str>, generation: i64) -> Result<()> {
        sqlx::query(
            r#"
            update pending_checks
            set owner_frame_id = $2,
                interaction_context_id = coalesce(interaction_context_id, 'ctx_' || $2),
                gate_id = coalesce(gate_id, $3),
                generation = $4,
                updated_at = now()
            where check_id = $1
            "#,
        )
        .bind(check_id)
        .bind(frame_id)
        .bind(gate_id)
        .bind(generation)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn supersede_interaction_gate(&self, gate_id: &str, reason: &str, closed_at_tick: i64) -> Result<()> {
        sqlx::query(
            r#"
            update interaction_gates
            set status = 'superseded',
                superseded_reason = $2,
                closed_at_tick = $3,
                resolution_json = coalesce(resolution_json, '{}'::jsonb) || jsonb_build_object('superseded_reason', $2),
                updated_at = now()
            where gate_id = $1 and status = 'open'
            "#,
        )
        .bind(gate_id)
        .bind(reason)
        .bind(closed_at_tick)
        .execute(&self.pool)
        .await?;
        sqlx::query("update sessions set active_interaction_gate_id = null, updated_at = now() where active_interaction_gate_id = $1")
            .bind(gate_id)
            .execute(&self.pool)
            .await
            .ok();
        Ok(())
    }

    pub async fn supersede_pending_check(&self, check_id: &str, reason: &str, closed_at_tick: Option<i64>) -> Result<()> {
        sqlx::query(
            r#"
            update pending_checks
            set status = 'superseded',
                superseded_reason = $2,
                closed_at_tick = coalesce($3, closed_at_tick),
                updated_at = now()
            where check_id = $1 and status = 'open'
            "#,
        )
        .bind(check_id)
        .bind(reason)
        .bind(closed_at_tick)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn cascade_close_frame_interactions(&self, session_id: &str, frame_id: &str, reason: &str, world_tick: i64, generation: i64) -> Result<Vec<InvariantRepair>> {
        let mut repairs = Vec::new();
        let gate_count: i64 = sqlx::query_scalar(
            r#"
            with owned_gate as (
              update interaction_gates
              set status = 'superseded',
                  superseded_reason = $3,
                  closed_at_tick = $4,
                  resolution_json = coalesce(resolution_json, '{}'::jsonb) || jsonb_build_object('superseded_by', 'frame_close', 'reason', $3),
                  updated_at = now()
              where session_id = $1 and status = 'open'
                and (owner_frame_id = $2 or gate_id in (select unnest(active_gate_ids) from state_frames where frame_id = $2))
              returning gate_id
            ) select count(*)::bigint from owned_gate
            "#,
        )
        .bind(session_id)
        .bind(frame_id)
        .bind(reason)
        .bind(world_tick)
        .fetch_one(&self.pool)
        .await.unwrap_or(0);
        if gate_count > 0 {
            repairs.push(InvariantRepair { repair_id: format!("repair_{}", Uuid::new_v4().simple()), kind: InvariantRepairKind::FrameClosedChildrenTerminal, target_table: "interaction_gates".into(), target_id: frame_id.into(), action: "superseded_open_child_gates".into(), reason: reason.into(), world_tick, generation });
        }
        let check_count: i64 = sqlx::query_scalar(
            r#"
            with owned_check as (
              update pending_checks
              set status = 'superseded',
                  superseded_reason = $3,
                  closed_at_tick = $4,
                  updated_at = now()
              where session_id = $1 and status = 'open'
                and (owner_frame_id = $2 or (contract_json->'actor_snapshot_ids') ? $2)
              returning check_id
            ) select count(*)::bigint from owned_check
            "#,
        )
        .bind(session_id)
        .bind(frame_id)
        .bind(reason)
        .bind(world_tick)
        .fetch_one(&self.pool)
        .await.unwrap_or(0);
        if check_count > 0 {
            repairs.push(InvariantRepair { repair_id: format!("repair_{}", Uuid::new_v4().simple()), kind: InvariantRepairKind::FrameClosedChildrenTerminal, target_table: "pending_checks".into(), target_id: frame_id.into(), action: "superseded_open_child_pending_checks".into(), reason: reason.into(), world_tick, generation });
        }
        sqlx::query("update state_frames set status = case when status in ('active','paused','resolving') then 'completed' else status end, closed_at_tick = coalesce(closed_at_tick, $3), closed_reason = coalesce(closed_reason, $4), updated_at = now() where session_id = $1 and frame_id = $2")
            .bind(session_id)
            .bind(frame_id)
            .bind(world_tick)
            .bind(reason)
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("update interaction_contexts set status='closed', closed_at_tick=$3, updated_at=now() where session_id=$1 and frame_id=$2 and status in ('active','paused','resolving')")
            .bind(session_id)
            .bind(frame_id)
            .bind(world_tick)
            .execute(&self.pool)
            .await
            .ok();
        Ok(repairs)
    }

    pub async fn reconcile_interaction_lifecycle(&self, session_id: &str, world_tick: i64, current_generation: i64) -> Result<Vec<InvariantRepair>> {
        let mut repairs = Vec::new();
        // Backfill ownership for legacy gates by looking at frame.active_gate_ids.
        sqlx::query(
            r#"
            update interaction_gates g
            set owner_frame_id = f.frame_id,
                interaction_context_id = coalesce(g.interaction_context_id, 'ctx_' || f.frame_id),
                generation = case when g.generation = 0 then $2 else g.generation end
            from state_frames f
            where g.session_id = $1 and g.owner_frame_id is null
              and g.gate_id = any(f.active_gate_ids)
            "#,
        ).bind(session_id).bind(current_generation).execute(&self.pool).await.ok();
        // Backfill pending checks from check actor_snapshot_ids when possible.
        sqlx::query(
            r#"
            update pending_checks p
            set owner_frame_id = p.contract_json->'actor_snapshot_ids'->>0,
                interaction_context_id = coalesce(p.interaction_context_id, 'ctx_' || (p.contract_json->'actor_snapshot_ids'->>0)),
                gate_id = coalesce(p.gate_id, 'gate_' || p.check_id),
                generation = case when p.generation = 0 then $2 else p.generation end
            where p.session_id = $1 and p.owner_frame_id is null
              and jsonb_array_length(coalesce(p.contract_json->'actor_snapshot_ids','[]'::jsonb)) > 0
            "#,
        ).bind(session_id).bind(current_generation).execute(&self.pool).await.ok();
        let stale_gate_count: i64 = sqlx::query_scalar(
            r#"
            with stale as (
              update interaction_gates g
              set status='superseded',
                  superseded_reason='superseded_by_reconcile',
                  closed_at_tick=$2,
                  resolution_json = coalesce(resolution_json, '{}'::jsonb) || jsonb_build_object('superseded_by', 'interaction_reconcile'),
                  updated_at=now()
              where g.session_id=$1 and g.status='open' and (
                g.generation < $3
                or (g.owner_frame_id is not null and not exists (
                    select 1 from state_frames f where f.session_id=$1 and f.frame_id=g.owner_frame_id and f.status in ('active','paused','resolving')
                ))
              )
              returning gate_id
            ) select count(*)::bigint from stale
            "#,
        ).bind(session_id).bind(world_tick).bind(current_generation).fetch_one(&self.pool).await.unwrap_or(0);
        if stale_gate_count > 0 {
            repairs.push(InvariantRepair { repair_id: format!("repair_{}", Uuid::new_v4().simple()), kind: InvariantRepairKind::OpenGateWithoutActiveOwner, target_table: "interaction_gates".into(), target_id: session_id.into(), action: "superseded_stale_or_orphan_open_gates".into(), reason: "superseded_by_reconcile".into(), world_tick, generation: current_generation });
        }
        let stale_check_count: i64 = sqlx::query_scalar(
            r#"
            with stale as (
              update pending_checks p
              set status='superseded',
                  superseded_reason='superseded_by_reconcile',
                  closed_at_tick=$2,
                  updated_at=now()
              where p.session_id=$1 and p.status='open' and (
                p.generation < $3
                or (p.owner_frame_id is not null and not exists (
                    select 1 from state_frames f where f.session_id=$1 and f.frame_id=p.owner_frame_id and f.status in ('active','paused','resolving')
                ))
              )
              returning check_id
            ) select count(*)::bigint from stale
            "#,
        ).bind(session_id).bind(world_tick).bind(current_generation).fetch_one(&self.pool).await.unwrap_or(0);
        if stale_check_count > 0 {
            repairs.push(InvariantRepair { repair_id: format!("repair_{}", Uuid::new_v4().simple()), kind: InvariantRepairKind::OpenPendingCheckWithoutActiveOwner, target_table: "pending_checks".into(), target_id: session_id.into(), action: "superseded_stale_or_orphan_open_pending_checks".into(), reason: "superseded_by_reconcile".into(), world_tick, generation: current_generation });
        }
        sqlx::query("update sessions set active_interaction_gate_id = null where session_id=$1 and active_interaction_gate_id is not null and not exists (select 1 from interaction_gates g where g.session_id=$1 and g.gate_id=active_interaction_gate_id and g.status='open')")
            .bind(session_id)
            .execute(&self.pool)
            .await
            .ok();
        Ok(repairs)
    }

    pub async fn insert_pending_check(&self, pending: &PendingCheck) -> Result<()> {
        let generation = self.ensure_interaction_generation(&pending.session_id).await.unwrap_or(0);
        let owner_frame_id = pending.owner_frame_id.clone().or_else(|| pending.contract.actor_snapshot_ids.first().cloned());
        let gate_id = pending.gate_id.clone().or_else(|| Some(format!("gate_{}", pending.check_id)));
        let interaction_context_id = pending.interaction_context_id.clone().or_else(|| owner_frame_id.as_ref().map(|f| format!("ctx_{}", f)));
        sqlx::query(
            r#"
            insert into pending_checks
              (id, check_id, session_id, expected_input_kind, prompt_public, contract_json, expires_at, status,
               interaction_context_id, owner_frame_id, gate_id, generation, superseded_reason, closed_at_tick, created_at, updated_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,now())
            on conflict (check_id) do update set
              prompt_public = excluded.prompt_public,
              contract_json = excluded.contract_json,
              expires_at = excluded.expires_at,
              status = excluded.status,
              interaction_context_id = excluded.interaction_context_id,
              owner_frame_id = excluded.owner_frame_id,
              gate_id = excluded.gate_id,
              generation = excluded.generation,
              superseded_reason = excluded.superseded_reason,
              closed_at_tick = excluded.closed_at_tick,
              updated_at = now()
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&pending.check_id)
        .bind(&pending.session_id)
        .bind(&pending.expected_input_kind)
        .bind(&pending.prompt_public)
        .bind(serde_json::to_value(&pending.contract)?)
        .bind(pending.expires_at)
        .bind(pending.status.as_str())
        .bind(&interaction_context_id)
        .bind(&owner_frame_id)
        .bind(&gate_id)
        .bind(if pending.generation == 0 { generation } else { pending.generation })
        .bind(&pending.superseded_reason)
        .bind(pending.closed_at_tick)
        .bind(pending.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn cancel_open_pending_checks_for_session(&self, session_id: &str, status: PendingCheckStatus) -> Result<()> {
        sqlx::query(
            r#"
            update pending_checks
            set status = $2, updated_at = now()
            where session_id = $1 and status = 'open'
            "#,
        )
        .bind(session_id)
        .bind(status.as_str())
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            update interaction_gates
            set status = $2, updated_at = now(), resolution_json = coalesce(resolution_json, jsonb_build_object('reason', 'superseded_by_new_gate'))
            where session_id = $1 and status = 'open'
            "#,
        )
        .bind(session_id)
        .bind(status.as_str())
        .execute(&self.pool)
        .await.ok();
        Ok(())
    }

    pub async fn insert_interaction_gate(&self, gate: &InteractionGate) -> Result<()> {
        let generation = self.ensure_interaction_generation(&gate.session_id).await.unwrap_or(0);
        sqlx::query(
            r#"
            insert into interaction_gates
              (id, gate_id, session_id, turn_id, gate_kind, status, prompt_public, prompt_gm, required,
               allowed_options, expected_input, on_unparseable, on_new_action, on_timeout,
               bound_action_summary, source_refs, advice_refs, resolution_json, expires_at_turn, expires_at_time,
               interaction_context_id, owner_frame_id, generation, superseded_reason, closed_at_tick, created_at, updated_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,$24,$25,$26,$27)
            on conflict (gate_id) do update set
              status = excluded.status,
              prompt_public = excluded.prompt_public,
              expected_input = excluded.expected_input,
              resolution_json = excluded.resolution_json,
              interaction_context_id = excluded.interaction_context_id,
              owner_frame_id = excluded.owner_frame_id,
              generation = excluded.generation,
              superseded_reason = excluded.superseded_reason,
              closed_at_tick = excluded.closed_at_tick,
              updated_at = excluded.updated_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&gate.gate_id)
        .bind(&gate.session_id)
        .bind(&gate.turn_id)
        .bind(gate.gate_kind.as_str())
        .bind(gate.status.as_str())
        .bind(&gate.prompt_public)
        .bind(&gate.prompt_gm)
        .bind(gate.required)
        .bind(serde_json::to_value(&gate.allowed_options)?)
        .bind(serde_json::to_value(&gate.expected_input)?)
        .bind(gate.on_unparseable.as_str())
        .bind(gate.on_new_action.as_str())
        .bind(gate.on_timeout.as_str())
        .bind(&gate.bound_action_summary)
        .bind(serde_json::to_value(&gate.source_refs)?)
        .bind(&gate.advice_refs)
        .bind(&gate.resolution_json)
        .bind(&gate.expires_at_turn)
        .bind(gate.expires_at_time)
        .bind(&gate.interaction_context_id)
        .bind(&gate.owner_frame_id)
        .bind(if gate.generation == 0 { generation } else { gate.generation })
        .bind(&gate.superseded_reason)
        .bind(gate.closed_at_tick)
        .bind(gate.created_at)
        .bind(gate.updated_at)
        .execute(&self.pool)
        .await?;
        sqlx::query("update sessions set active_interaction_gate_id = $2, updated_at = now() where session_id = $1")
            .bind(&gate.session_id)
            .bind(if gate.status == GateStatus::Open { Some(gate.gate_id.clone()) } else { None })
            .execute(&self.pool)
            .await.ok();
        Ok(())
    }

    pub async fn get_open_interaction_gate(&self, session_id: &str) -> Result<Option<InteractionGate>> {
        let row = sqlx::query(
            r#"
            select gate_id, session_id, turn_id, gate_kind, status, prompt_public, prompt_gm, required,
                   allowed_options, expected_input, on_unparseable, on_new_action, on_timeout,
                   bound_action_summary, source_refs, advice_refs, resolution_json, expires_at_turn, expires_at_time,
                   interaction_context_id, owner_frame_id, generation, superseded_reason, closed_at_tick, created_at, updated_at
            from interaction_gates
            where session_id = $1 and status = 'open'
              and generation = coalesce((select interaction_generation from sessions where session_id = $1), 0)
              and (expires_at_time is null or expires_at_time > now())
            order by created_at desc
            limit 1
            "#,
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_interaction_gate).transpose()
    }

    pub async fn update_interaction_gate_status(&self, gate_id: &str, status: GateStatus, resolution_json: serde_json::Value) -> Result<()> {
        sqlx::query(
            r#"
            update interaction_gates
            set status = $2, resolution_json = $3, updated_at = now()
            where gate_id = $1
            "#,
        )
        .bind(gate_id)
        .bind(status.as_str())
        .bind(resolution_json)
        .execute(&self.pool)
        .await?;
        sqlx::query("update sessions set active_interaction_gate_id = null, updated_at = now() where active_interaction_gate_id = $1")
            .bind(gate_id)
            .execute(&self.pool)
            .await.ok();
        Ok(())
    }

    pub async fn get_open_pending_check(&self, session_id: &str) -> Result<Option<PendingCheck>> {
        let row = sqlx::query(
            r#"
            select check_id, session_id, expected_input_kind, prompt_public, contract_json, expires_at, status,
                   interaction_context_id, owner_frame_id, gate_id, generation, superseded_reason, closed_at_tick, created_at
            from pending_checks
            where session_id = $1 and status = 'open'
              and generation = coalesce((select interaction_generation from sessions where session_id = $1), 0)
              and (expires_at is null or expires_at > now())
            order by created_at desc
            limit 1
            "#,
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_pending_check).transpose()
    }

    pub async fn update_pending_check_status(&self, check_id: &str, status: PendingCheckStatus) -> Result<()> {
        sqlx::query(
            r#"update pending_checks set status = $2, updated_at = now() where check_id = $1"#,
        )
        .bind(check_id)
        .bind(status.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_check_contracts_for_turn(&self, session_id: &str, turn_id: &str) -> Result<Vec<CheckContract>> {
        let rows = sqlx::query(
            r#"
            select contract_json
            from check_contracts
            where session_id = $1 and turn_id = $2
            order by created_at asc
            "#,
        )
        .bind(session_id)
        .bind(turn_id)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::new();
        for row in rows {
            let v: serde_json::Value = row.get("contract_json");
            if let Ok(contract) = serde_json::from_value::<CheckContract>(v) {
                out.push(contract);
            }
        }
        Ok(out)
    }

    /// Return the latest check contract that has not yet produced a check_result.
    /// Used by the turn/API `/roll` path to bind a naked `/roll` command to
    /// the most recent mechanical question instead of creating an orphan die.
    pub async fn get_latest_unresolved_check_contract(&self, session_id: &str) -> Result<Option<CheckContract>> {
        let row = sqlx::query(
            r#"
            select cc.contract_json
            from check_contracts cc
            where cc.session_id = $1
              and cc.status in ('created', 'open', 'pending')
              and not exists (select 1 from check_results cr where cr.check_id = cc.check_id)
            order by cc.created_at desc
            limit 1
            "#,
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(row) = row {
            let v: serde_json::Value = row.get("contract_json");
            Ok(serde_json::from_value::<CheckContract>(v).ok())
        } else {
            Ok(None)
        }
    }

    pub async fn upsert_state_frame(&self, frame: &StateFrame) -> Result<()> {
        sqlx::query(
            r#"
            insert into state_frames
              (id, frame_id, frame_kind, session_id, ruleset_id, module_id, parent_frame_id, scope_type, scope_id,
               status, title, objective, static_refs, working_state, active_gate_ids, local_clocks, local_facts,
               local_modifiers, event_count, last_event_ids, retention_policy, compaction_policy, created_at, updated_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,$24)
            on conflict (frame_id) do update set
              status = excluded.status,
              working_state = excluded.working_state,
              active_gate_ids = excluded.active_gate_ids,
              local_clocks = excluded.local_clocks,
              local_facts = excluded.local_facts,
              local_modifiers = excluded.local_modifiers,
              event_count = excluded.event_count,
              last_event_ids = excluded.last_event_ids,
              updated_at = excluded.updated_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&frame.frame_id)
        .bind(frame.frame_kind.as_str())
        .bind(&frame.session_id)
        .bind(&frame.ruleset_id)
        .bind(&frame.module_id)
        .bind(&frame.parent_frame_id)
        .bind(frame.scope.scope_type.as_str())
        .bind(&frame.scope.scope_id)
        .bind(frame.status.as_str())
        .bind(&frame.title)
        .bind(&frame.objective)
        .bind(&frame.static_refs)
        .bind(&frame.working_state)
        .bind(&frame.active_gate_ids)
        .bind(serde_json::to_value(&frame.local_clocks)?)
        .bind(serde_json::to_value(&frame.local_facts)?)
        .bind(serde_json::to_value(&frame.local_modifiers)?)
        .bind(frame.event_count as i32)
        .bind(&frame.last_event_ids)
        .bind(serde_json::to_value(&frame.retention_policy)?)
        .bind(serde_json::to_value(&frame.compaction_policy)?)
        .bind(frame.created_at)
        .bind(frame.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_active_state_frames(&self, session_id: &str, limit: i64) -> Result<Vec<StateFrame>> {
        let rows = sqlx::query(
            r#"
            select frame_id, frame_kind, session_id, ruleset_id, module_id, parent_frame_id,
                   scope_type, scope_id, status, title, objective, static_refs, working_state,
                   active_gate_ids, local_clocks, local_facts, local_modifiers, event_count,
                   last_event_ids, retention_policy, compaction_policy, created_at, updated_at
            from state_frames
            where session_id = $1 and status in ('active','paused','resolving')
            order by updated_at desc
            limit $2
            "#,
        )
        .bind(session_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_state_frame).collect()
    }



    pub async fn get_world_time_state(&self, session_id: &str) -> Result<Option<WorldTimeState>> {
        let row = sqlx::query(
            r#"
            select session_id, campaign_id, world_tick, absolute_seconds, calendar_id, display_time,
                   time_scale, scene_epoch, turn_seq, event_seq, updated_at
            from world_time_state where session_id = $1
            "#,
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_world_time_state).transpose()
    }

    pub async fn upsert_world_time_state(&self, state: &WorldTimeState) -> Result<()> {
        sqlx::query(
            r#"
            insert into world_time_state
              (session_id, campaign_id, world_tick, absolute_seconds, calendar_id, display_time,
               time_scale, scene_epoch, turn_seq, event_seq, updated_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
            on conflict (session_id) do update set
              campaign_id = excluded.campaign_id,
              world_tick = excluded.world_tick,
              absolute_seconds = excluded.absolute_seconds,
              calendar_id = excluded.calendar_id,
              display_time = excluded.display_time,
              time_scale = excluded.time_scale,
              scene_epoch = excluded.scene_epoch,
              turn_seq = excluded.turn_seq,
              event_seq = excluded.event_seq,
              updated_at = excluded.updated_at
            "#,
        )
        .bind(&state.session_id)
        .bind(&state.campaign_id)
        .bind(state.world_tick)
        .bind(state.absolute_seconds)
        .bind(&state.calendar_id)
        .bind(&state.display_time)
        .bind(state.time_scale.as_str())
        .bind(&state.scene_epoch)
        .bind(state.turn_seq)
        .bind(state.event_seq)
        .bind(state.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_world_event(&self, event: &WorldEvent) -> Result<()> {
        sqlx::query(
            r#"
            insert into world_events
              (id, event_id, campaign_id, session_id, world_tick, event_seq, turn_id, frame_id,
               event_kind, event_json, visibility, source_refs, caused_by_event_ids, state_patch_ids, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)
            on conflict (event_id) do nothing
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&event.event_id)
        .bind(&event.campaign_id)
        .bind(&event.session_id)
        .bind(event.world_tick)
        .bind(event.event_seq)
        .bind(&event.turn_id)
        .bind(&event.frame_id)
        .bind(event.event_kind.as_str())
        .bind(&event.event_json)
        .bind(event.visibility.as_str())
        .bind(serde_json::to_value(&event.source_refs)?)
        .bind(&event.caused_by_event_ids)
        .bind(&event.state_patch_ids)
        .bind(event.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_world_events_since(&self, session_id: &str, since_tick: i64, since_event_seq: i64, limit: i64) -> Result<Vec<WorldEvent>> {
        let rows = sqlx::query(
            r#"
            select event_id, campaign_id, session_id, world_tick, event_seq, turn_id, frame_id,
                   event_kind, event_json, visibility, source_refs, caused_by_event_ids, state_patch_ids, created_at
            from world_events
            where session_id = $1
              and (world_tick > $2 or (world_tick = $2 and event_seq > $3))
            order by world_tick asc, event_seq asc
            limit $4
            "#,
        )
        .bind(session_id)
        .bind(since_tick)
        .bind(since_event_seq)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_world_event).collect()
    }

    pub async fn insert_world_time_advance(&self, from: &WorldTimeState, to: &WorldTimeState, event: &WorldEvent) -> Result<()> {
        sqlx::query(
            r#"
            insert into world_time_advances
              (id, advance_id, session_id, from_tick, to_tick, from_display, to_display, advance_event_id, advance_json, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,now())
            on conflict (advance_id) do nothing
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(format!("advance_{}", event.event_id))
        .bind(&to.session_id)
        .bind(from.world_tick)
        .bind(to.world_tick)
        .bind(&from.display_time)
        .bind(&to.display_time)
        .bind(&event.event_id)
        .bind(serde_json::json!({"from": from, "to": to, "event": event}))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_scheduled_event(&self, event: &ScheduledEvent) -> Result<()> {
        sqlx::query(
            r#"
            insert into scheduled_events
              (id, scheduled_event_id, campaign_id, session_id, due_tick, event_kind, payload_json,
               visibility, status, created_by_event_id, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
            on conflict (scheduled_event_id) do update set status = excluded.status, payload_json = excluded.payload_json
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&event.scheduled_event_id)
        .bind(&event.campaign_id)
        .bind(&event.session_id)
        .bind(event.due_tick)
        .bind(event.event_kind.as_str())
        .bind(&event.payload_json)
        .bind(event.visibility.as_str())
        .bind(event.status.as_str())
        .bind(&event.created_by_event_id)
        .bind(event.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_due_scheduled_events(&self, session_id: &str, tick: i64) -> Result<Vec<ScheduledEvent>> {
        let rows = sqlx::query(
            r#"
            select scheduled_event_id, campaign_id, session_id, due_tick, event_kind, payload_json,
                   visibility, status, created_by_event_id, created_at
            from scheduled_events
            where session_id = $1 and status = 'pending' and due_tick <= $2
            order by due_tick asc, created_at asc
            "#,
        )
        .bind(session_id)
        .bind(tick)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_scheduled_event).collect()
    }

    pub async fn update_scheduled_event_status(&self, scheduled_event_id: &str, status: ScheduledEventStatus) -> Result<()> {
        sqlx::query("update scheduled_events set status = $2 where scheduled_event_id = $1")
            .bind(scheduled_event_id)
            .bind(status.as_str())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn get_context_watermark(&self, session_id: &str) -> Result<Option<ContextWatermark>> {
        let row = sqlx::query(
            "select session_id, last_compiled_world_tick, last_compiled_event_seq, compiled_context_hash, updated_at from context_watermarks where session_id = $1",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_context_watermark).transpose()
    }

    pub async fn upsert_context_watermark(&self, watermark: &ContextWatermark) -> Result<()> {
        sqlx::query(
            r#"
            insert into context_watermarks
              (session_id, last_compiled_world_tick, last_compiled_event_seq, compiled_context_hash, updated_at)
            values ($1,$2,$3,$4,$5)
            on conflict (session_id) do update set
              last_compiled_world_tick = excluded.last_compiled_world_tick,
              last_compiled_event_seq = excluded.last_compiled_event_seq,
              compiled_context_hash = excluded.compiled_context_hash,
              updated_at = excluded.updated_at
            "#,
        )
        .bind(&watermark.session_id)
        .bind(watermark.last_compiled_world_tick)
        .bind(watermark.last_compiled_event_seq)
        .bind(&watermark.compiled_context_hash)
        .bind(watermark.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_frame_event(&self, event: &FrameEvent) -> Result<()> {
        sqlx::query(
            r#"
            insert into frame_events
              (id, event_id, frame_id, session_id, turn_id, event_kind, event_json, visibility, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9)
            on conflict (event_id) do nothing
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&event.event_id)
        .bind(&event.frame_id)
        .bind(&event.session_id)
        .bind(&event.turn_id)
        .bind(&event.event_kind)
        .bind(&event.event_json)
        .bind(event.visibility.as_str())
        .bind(event.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_frame_events(&self, frame_id: &str, limit: i64) -> Result<Vec<FrameEvent>> {
        let rows = sqlx::query(
            r#"
            select event_id, frame_id, session_id, turn_id, event_kind, event_json, visibility, created_at
            from frame_events
            where frame_id = $1
            order by created_at desc
            limit $2
            "#,
        )
        .bind(frame_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_frame_event).collect()
    }

    pub async fn insert_frame_compaction(&self, compaction: &FrameCompaction) -> Result<()> {
        sqlx::query(
            r#"
            insert into frame_compactions
              (id, compaction_id, frame_id, session_id, summary_markdown, persistent_world_patches,
               promoted_fact_ids, archived_event_ids, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9)
            on conflict (compaction_id) do nothing
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&compaction.compaction_id)
        .bind(&compaction.frame_id)
        .bind(&compaction.session_id)
        .bind(&compaction.summary_markdown)
        .bind(serde_json::to_value(&compaction.persistent_world_patches)?)
        .bind(&compaction.promoted_fact_ids)
        .bind(&compaction.archived_event_ids)
        .bind(compaction.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }


    pub async fn insert_actionable_situation_brief(&self, brief: &ActionableSituationBrief) -> Result<()> {
        sqlx::query(
            r#"
            insert into actionable_situation_briefs
              (id, brief_id, session_id, turn_id, frame_id, guidance_level, brief_json, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8)
            on conflict (brief_id) do update set brief_json = excluded.brief_json
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&brief.brief_id)
        .bind(&brief.session_id)
        .bind(&brief.turn_id)
        .bind(&brief.frame_id)
        .bind(brief.guidance.level.as_str())
        .bind(serde_json::to_value(brief)?)
        .bind(brief.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn upsert_player_facing_clue_board(&self, board: &PlayerFacingClueBoard) -> Result<()> {
        sqlx::query(
            r#"
            insert into player_facing_clue_boards
              (id, board_id, session_id, turn_id, board_json, updated_at)
            values ($1,$2,$3,$4,$5,$6)
            on conflict (board_id) do update set
              turn_id = excluded.turn_id,
              board_json = excluded.board_json,
              updated_at = excluded.updated_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&board.board_id)
        .bind(&board.session_id)
        .bind(&board.turn_id)
        .bind(serde_json::to_value(board)?)
        .bind(board.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_consequence_contract(&self, contract: &ConsequenceContract) -> Result<()> {
        sqlx::query(
            r#"
            insert into consequence_contracts
              (id, consequence_id, session_id, turn_id, frame_id, fail_forward, contract_json, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,now())
            on conflict (consequence_id) do update set contract_json = excluded.contract_json
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&contract.consequence_id)
        .bind(&contract.session_id)
        .bind(&contract.turn_id)
        .bind(&contract.frame_id)
        .bind(contract.fail_forward)
        .bind(serde_json::to_value(contract)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_clock_tick(&self, session_id: &str, turn_id: &str, tick: &ClockTick) -> Result<()> {
        sqlx::query(
            r#"
            insert into clock_tick_events
              (id, session_id, turn_id, clock_id, tick_json, created_at)
            values ($1,$2,$3,$4,$5,now())
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(session_id)
        .bind(turn_id)
        .bind(&tick.clock_id)
        .bind(serde_json::to_value(tick)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn upsert_spotlight_state(&self, session_id: &str, state: &SpotlightState) -> Result<()> {
        sqlx::query(
            r#"
            insert into spotlight_states
              (id, session_id, player_id, state_json, updated_at)
            values ($1,$2,$3,$4,now())
            on conflict (session_id, player_id) do update set
              state_json = excluded.state_json,
              updated_at = now()
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(session_id)
        .bind(&state.player_id)
        .bind(serde_json::to_value(state)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_effect_contract(&self, effect: &EffectContract, session_id: &str, turn_id: &str) -> Result<()> {
        sqlx::query(
            r#"
            insert into effect_contracts
              (id, effect_id, session_id, turn_id, source_event_id, effect_kind, target_actor_ids,
               effect_json, visibility, confidence, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,now())
            on conflict (effect_id) do update set effect_json = excluded.effect_json
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&effect.effect_id)
        .bind(session_id)
        .bind(turn_id)
        .bind(&effect.source_event_id)
        .bind(effect.effect_kind.as_str())
        .bind(&effect.target_actor_ids)
        .bind(serde_json::to_value(effect)?)
        .bind(effect.visibility.as_str())
        .bind(effect.confidence.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }


    pub async fn list_effect_contracts_for_turn(&self, session_id: &str, turn_id: &str) -> Result<Vec<EffectContract>> {
        let rows = sqlx::query(
            r#"
            select effect_json
            from effect_contracts
            where session_id = $1 and turn_id = $2
            order by created_at asc
            "#,
        )
        .bind(session_id)
        .bind(turn_id)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::new();
        for row in rows {
            let v: serde_json::Value = row.get("effect_json");
            if let Ok(effect) = serde_json::from_value::<EffectContract>(v) {
                out.push(effect);
            }
        }
        Ok(out)
    }

    pub async fn insert_dice_roll(&self, roll: &DiceRollRecord) -> Result<()> {
        sqlx::query(
            r#"
            insert into dice_rolls
              (id, roll_id, session_id, turn_id, check_id, roller_kind, roller_id, visibility, expression, result_json, seed_commitment, revealed_at, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
            on conflict (roll_id) do nothing
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&roll.roll_id)
        .bind(&roll.session_id)
        .bind(&roll.turn_id)
        .bind(&roll.check_id)
        .bind(match roll.roller_kind {
            ActorKind::PlayerCharacter => "player_character",
            ActorKind::Npc => "npc",
            ActorKind::Environment => "environment",
            ActorKind::Hazard => "hazard",
            ActorKind::System => "system",
        })
        .bind(&roll.roller_id)
        .bind(roll.visibility.as_str())
        .bind(&roll.expression)
        .bind(&roll.result)
        .bind(&roll.seed_commitment)
        .bind(roll.revealed_at)
        .bind(roll.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_check_result(&self, result: &CheckResultRecord) -> Result<()> {
        sqlx::query(
            r#"
            insert into check_results
              (id, check_id, roll_json, outcome_json, committed_patches, created_at)
            values ($1,$2,$3,$4,$5,$6)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&result.check_id)
        .bind(serde_json::to_value(&result.roll)?)
        .bind(&result.outcome)
        .bind(serde_json::to_value(&result.committed_patches)?)
        .bind(result.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_player_value_claim(&self, claim: &PlayerSuppliedValueClaim) -> Result<()> {
        sqlx::query(
            r#"
            insert into player_value_claims
              (id, claim_id, session_id, turn_id, ruleset_id, module_id, value_kind, label,
               supplied_value_json, evidence_span, context_summary, target_actor_id, target_object_id,
               target_ability_id, source_refs, visibility, world_tick, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)
            on conflict (claim_id) do update set supplied_value_json = excluded.supplied_value_json,
              source_refs = excluded.source_refs, world_tick = excluded.world_tick
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&claim.claim_id)
        .bind(&claim.session_id)
        .bind(&claim.turn_id)
        .bind(&claim.ruleset_id)
        .bind(&claim.module_id)
        .bind(claim.value_kind.as_str())
        .bind(&claim.label)
        .bind(&claim.supplied_value_json)
        .bind(&claim.evidence_span)
        .bind(&claim.context_summary)
        .bind(&claim.target_actor_id)
        .bind(&claim.target_object_id)
        .bind(&claim.target_ability_id)
        .bind(serde_json::to_value(&claim.source_refs)?)
        .bind(claim.visibility.as_str())
        .bind(claim.world_tick)
        .bind(claim.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_player_value_verification(&self, verification: &PlayerValueVerification) -> Result<()> {
        sqlx::query(
            r#"
            insert into player_value_verifications
              (id, verification_id, claim_id, session_id, turn_id, status, canonical_value_json,
               acceptable_range_json, comparison_json, source_refs, warning_public, suggestion_public,
               accepted_if_player_insists, balance_risk, verifier_json, world_tick, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)
            on conflict (verification_id) do update set status = excluded.status,
              canonical_value_json = excluded.canonical_value_json, acceptable_range_json = excluded.acceptable_range_json,
              comparison_json = excluded.comparison_json, warning_public = excluded.warning_public,
              suggestion_public = excluded.suggestion_public, verifier_json = excluded.verifier_json
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&verification.verification_id)
        .bind(&verification.claim_id)
        .bind(&verification.session_id)
        .bind(&verification.turn_id)
        .bind(verification.status.as_str())
        .bind(&verification.canonical_value_json)
        .bind(&verification.acceptable_range_json)
        .bind(&verification.comparison_json)
        .bind(serde_json::to_value(&verification.source_refs)?)
        .bind(&verification.warning_public)
        .bind(&verification.suggestion_public)
        .bind(verification.accepted_if_player_insists)
        .bind(&verification.balance_risk)
        .bind(&verification.verifier_json)
        .bind(verification.world_tick)
        .bind(verification.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_table_override_agreement(&self, agreement: &TableOverrideAgreement) -> Result<()> {
        sqlx::query(
            r#"
            insert into table_override_agreements
              (id, override_id, session_id, claim_id, verification_id, status, accepted_by, reason,
               warning_public, created_at_tick, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
            on conflict (override_id) do update set status = excluded.status,
              accepted_by = excluded.accepted_by, reason = excluded.reason
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&agreement.override_id)
        .bind(&agreement.session_id)
        .bind(&agreement.claim_id)
        .bind(&agreement.verification_id)
        .bind(agreement.status.as_str())
        .bind(&agreement.accepted_by)
        .bind(&agreement.reason)
        .bind(&agreement.warning_public)
        .bind(agreement.created_at_tick)
        .bind(agreement.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_recent_player_value_verifications(&self, session_id: &str, limit: i64) -> Result<Vec<PlayerValueVerification>> {
        let rows = sqlx::query(
            r#"
            select verification_id, claim_id, session_id, turn_id, status, canonical_value_json,
                   acceptable_range_json, comparison_json, source_refs, warning_public, suggestion_public,
                   accepted_if_player_insists, balance_risk, verifier_json, world_tick, created_at
            from player_value_verifications
            where session_id = $1
            order by created_at desc
            limit $2
            "#,
        )
        .bind(session_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|row| PlayerValueVerification {
            verification_id: row.get("verification_id"),
            claim_id: row.get("claim_id"),
            session_id: row.get("session_id"),
            turn_id: row.get("turn_id"),
            status: player_value_status_from_str(&row.get::<String, _>("status")),
            canonical_value_json: row.get("canonical_value_json"),
            acceptable_range_json: row.get("acceptable_range_json"),
            comparison_json: row.get("comparison_json"),
            source_refs: serde_json::from_value(row.get::<serde_json::Value, _>("source_refs")).unwrap_or_default(),
            warning_public: row.get("warning_public"),
            suggestion_public: row.get("suggestion_public"),
            accepted_if_player_insists: row.get("accepted_if_player_insists"),
            balance_risk: row.get("balance_risk"),
            verifier_json: row.get("verifier_json"),
            world_tick: row.get("world_tick"),
            created_at: row.get("created_at"),
        }).collect())
    }



}



fn player_value_status_from_str(s: &str) -> PlayerValueVerificationStatus {
    match s {
        "rule_confirmed" => PlayerValueVerificationStatus::RuleConfirmed,
        "table_consistent" => PlayerValueVerificationStatus::TableConsistent,
        "plausible_provisional" => PlayerValueVerificationStatus::PlausibleProvisional,
        "unreasonable_needs_warning" => PlayerValueVerificationStatus::UnreasonableNeedsWarning,
        "contradicts_known_rule" => PlayerValueVerificationStatus::ContradictsKnownRule,
        "needs_clarification" => PlayerValueVerificationStatus::NeedsClarification,
        "accepted_as_table_preference" => PlayerValueVerificationStatus::AcceptedAsTablePreference,
        _ => PlayerValueVerificationStatus::NeedsRuleLookup,
    }
}

fn row_to_interaction_gate(row: sqlx::postgres::PgRow) -> Result<InteractionGate> {
    let gate_kind = match row.get::<String, _>("gate_kind").as_str() {
        "required_reaction_choice" => GateKind::RequiredReactionChoice,
        "optional_reaction_window" => GateKind::OptionalReactionWindow,
        "confirm_risky_action" => GateKind::ConfirmRiskyAction,
        "choose_action_mode" => GateKind::ChooseActionMode,
        "spend_resource_window" => GateKind::SpendResourceWindow,
        "select_target" => GateKind::SelectTarget,
        "resolve_ambiguous_intent" => GateKind::ResolveAmbiguousIntent,
        _ => GateKind::PlayerRollRequired,
    };
    let status = gate_status_from_str(&row.get::<String, _>("status"));
    let allowed_options: Vec<ActionOption> = serde_json::from_value(row.get::<serde_json::Value, _>("allowed_options")).unwrap_or_default();
    let expected_input: ExpectedInput = serde_json::from_value(row.get::<serde_json::Value, _>("expected_input")).unwrap_or_default();
    let source_refs: Vec<SourceRef> = serde_json::from_value(row.get::<serde_json::Value, _>("source_refs")).unwrap_or_default();
    let on_unparseable = gate_fallback_from_str(&row.get::<String, _>("on_unparseable"));
    let on_new_action = gate_fallback_from_str(&row.get::<String, _>("on_new_action"));
    let on_timeout = gate_fallback_from_str(&row.get::<String, _>("on_timeout"));
    Ok(InteractionGate {
        gate_id: row.get("gate_id"),
        session_id: row.get("session_id"),
        turn_id: row.get("turn_id"),
        gate_kind,
        status,
        prompt_public: row.get("prompt_public"),
        prompt_gm: row.get("prompt_gm"),
        required: row.get("required"),
        allowed_options,
        expected_input,
        on_unparseable,
        on_new_action,
        on_timeout,
        bound_action_summary: row.get("bound_action_summary"),
        source_refs,
        advice_refs: row.get("advice_refs"),
        resolution_json: row.get("resolution_json"),
        expires_at_turn: row.get("expires_at_turn"),
        expires_at_time: row.get("expires_at_time"),
        interaction_context_id: row.try_get("interaction_context_id").ok().flatten(),
        owner_frame_id: row.try_get("owner_frame_id").ok().flatten(),
        generation: row.try_get("generation").unwrap_or(0),
        superseded_reason: row.try_get("superseded_reason").ok().flatten(),
        closed_at_tick: row.try_get("closed_at_tick").ok().flatten(),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}


fn row_to_frame_event(row: sqlx::postgres::PgRow) -> Result<FrameEvent> {
    Ok(FrameEvent {
        event_id: row.get("event_id"),
        frame_id: row.get("frame_id"),
        session_id: row.get("session_id"),
        turn_id: row.get("turn_id"),
        event_kind: row.get("event_kind"),
        event_json: row.get("event_json"),
        visibility: visibility_from_str(&row.get::<String, _>("visibility")),
        created_at: row.get("created_at"),
    })
}

fn row_to_state_frame(row: sqlx::postgres::PgRow) -> Result<StateFrame> {
    let frame_kind = match row.get::<String, _>("frame_kind").as_str() {
        "combat" => FrameKind::Combat,
        "chase" => FrameKind::Chase,
        "netrun" => FrameKind::Netrun,
        "investigation_node" => FrameKind::InvestigationNode,
        "social_conflict" => FrameKind::SocialConflict,
        "negotiation" => FrameKind::Negotiation,
        "infiltration" => FrameKind::Infiltration,
        "hazard_sequence" => FrameKind::HazardSequence,
        "downtime_project" => FrameKind::DowntimeProject,
        "anomaly_encounter" => FrameKind::AnomalyEncounter,
        "horror_encounter" => FrameKind::HorrorEncounter,
        _ => FrameKind::SideQuest,
    };
    let status = match row.get::<String, _>("status").as_str() {
        "paused" => FrameStatus::Paused,
        "resolving" => FrameStatus::Resolving,
        "completed" => FrameStatus::Completed,
        "failed" => FrameStatus::Failed,
        "abandoned" => FrameStatus::Abandoned,
        "compacted" => FrameStatus::Compacted,
        "archived" => FrameStatus::Archived,
        _ => FrameStatus::Active,
    };
    Ok(StateFrame {
        frame_id: row.get("frame_id"),
        frame_kind,
        session_id: row.get("session_id"),
        ruleset_id: row.get("ruleset_id"),
        module_id: row.get("module_id"),
        parent_frame_id: row.get("parent_frame_id"),
        scope: Scope { scope_type: scope_type_from_str(&row.get::<String, _>("scope_type")), scope_id: row.get("scope_id") },
        status,
        title: row.get("title"),
        objective: row.get("objective"),
        static_refs: row.get("static_refs"),
        working_state: row.get("working_state"),
        active_gate_ids: row.get("active_gate_ids"),
        local_clocks: serde_json::from_value(row.get::<serde_json::Value, _>("local_clocks")).unwrap_or_default(),
        local_facts: serde_json::from_value(row.get::<serde_json::Value, _>("local_facts")).unwrap_or_default(),
        local_modifiers: serde_json::from_value(row.get::<serde_json::Value, _>("local_modifiers")).unwrap_or_default(),
        event_count: row.get::<i32, _>("event_count") as u32,
        last_event_ids: row.get("last_event_ids"),
        retention_policy: serde_json::from_value(row.get::<serde_json::Value, _>("retention_policy")).unwrap_or_default(),
        compaction_policy: serde_json::from_value(row.get::<serde_json::Value, _>("compaction_policy")).unwrap_or_default(),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn row_to_pending_check(row: sqlx::postgres::PgRow) -> Result<PendingCheck> {
    let contract_json: serde_json::Value = row.get("contract_json");
    let contract: CheckContract = serde_json::from_value(contract_json).unwrap_or_else(|_| CheckContract {
        check_id: row.get("check_id"),
        session_id: row.get("session_id"),
        turn_id: "unknown".into(),
        ruleset_id: "unknown".into(),
        module_id: None,
        initiator: ActorRef { actor_id: "unknown".into(), actor_kind: ActorKind::PlayerCharacter, display_name: None },
        target_actor: None,
        opposition: OppositionModel::NoMechanicalOpposition,
        action_summary: String::new(),
        intent_kind: "unknown".into(),
        check_label: "Pending check".into(),
        dice_expression: "1d10".into(),
        modifiers: vec![],
        target: CheckTargetModel::UnknownUntilLookup,
        tested_parameter: None,
        opponent_tested_parameter: None,
        actor_snapshot_ids: vec![],
        source_refs: vec![],
        learned_packet_ids: vec![],
        roll_visibility: RollVisibility::PlayerRollRequired,
        roll_authority: RollAuthority::Player,
        disclosure: RollDisclosurePolicy::for_visibility(RollVisibility::PlayerRollRequired),
        stakes: CheckStakes::default(),
        confidence: RulingConfidence::Low,
        ruling_status: RulingStatus::Provisional,
        advice_refs: vec![],
        expires_at_turn: None,
    });
    let status_str: String = row.get("status");
    let status = match status_str.as_str() {
        "resolved" => PendingCheckStatus::Resolved,
        "cancelled" => PendingCheckStatus::Cancelled,
        "abandoned_by_new_action" => PendingCheckStatus::AbandonedByNewAction,
        "superseded" => PendingCheckStatus::Superseded,
        "superseded_by_frame_close" => PendingCheckStatus::SupersededByFrameClose,
        "superseded_by_scene_transition" => PendingCheckStatus::SupersededBySceneTransition,
        "superseded_by_new_frame" => PendingCheckStatus::SupersededByNewFrame,
        "superseded_by_exit_contract" => PendingCheckStatus::SupersededByExitContract,
        "superseded_by_world_time_advance" => PendingCheckStatus::SupersededByWorldTimeAdvance,
        "superseded_by_reconcile" => PendingCheckStatus::SupersededByReconcile,
        "stale_generation" => PendingCheckStatus::StaleGeneration,
        "expired" => PendingCheckStatus::Expired,
        _ => PendingCheckStatus::Open,
    };
    Ok(PendingCheck {
        check_id: row.get("check_id"),
        session_id: row.get("session_id"),
        expected_input_kind: row.get("expected_input_kind"),
        prompt_public: row.get("prompt_public"),
        contract,
        expires_at: row.get("expires_at"),
        status,
        interaction_context_id: row.try_get("interaction_context_id").ok().flatten(),
        owner_frame_id: row.try_get("owner_frame_id").ok().flatten(),
        gate_id: row.try_get("gate_id").ok().flatten(),
        generation: row.try_get("generation").unwrap_or(0),
        superseded_reason: row.try_get("superseded_reason").ok().flatten(),
        closed_at_tick: row.try_get("closed_at_tick").ok().flatten(),
        created_at: row.get("created_at"),
    })
}

fn row_to_lookup_event(row: sqlx::postgres::PgRow) -> Result<LookupEvent> {
    let search_terms_value: serde_json::Value = row.get("search_terms");
    let search_terms = serde_json::from_value(search_terms_value).unwrap_or_default();
    Ok(LookupEvent {
        event_id: row.get("event_id"),
        session_id: row.get("session_id"),
        ruleset_id: row.get("ruleset_id"),
        module_id: row.get("module_id"),
        demand_id: row.get("demand_id"),
        query_text: row.get("query_text"),
        search_terms,
        source_hits: row.get("source_hits"),
        result_status: row.get("result_status"),
        created_at: row.get("created_at"),
    })
}

fn row_to_learning_candidate(row: sqlx::postgres::PgRow) -> Result<LearningAuditCandidate> {
    let source_refs_value: serde_json::Value = row.get("source_refs");
    let source_refs = serde_json::from_value(source_refs_value).unwrap_or_default();
    Ok(LearningAuditCandidate {
        candidate_id: row.get("candidate_id"),
        session_id: row.get("session_id"),
        turn_id: row.get("turn_id"),
        ruleset_id: row.get("ruleset_id"),
        module_id: row.get("module_id"),
        demand_id: row.get("demand_id"),
        packet_type: row.get("packet_type"),
        packet_key: row.get("packet_key"),
        title: row.get("title"),
        summary: row.get("summary"),
        packet_json: row.get("packet_json"),
        source_refs,
        evidence_score: row.get::<f32, _>("evidence_score"),
        risk_flags: row.get("risk_flags"),
        verifier_status: candidate_status_from_str(&row.get::<String, _>("verifier_status")),
        verifier_notes: row.get("verifier_notes"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn candidate_status_from_str(s: &str) -> LearningCandidateStatus {
    match s {
        "approved" => LearningCandidateStatus::Approved,
        "rejected" => LearningCandidateStatus::Rejected,
        "auto_promoted" => LearningCandidateStatus::AutoPromoted,
        _ => LearningCandidateStatus::PendingReview,
    }
}

fn locator_entry_from_material(owner_id: &str, owner_kind: &str, entry: &MaterialIndexEntry) -> BookLocatorEntry {
    let first_ref = entry.source_refs.first().cloned().unwrap_or_default();
    let page = first_ref.page;
    let mut search_terms = entry.tags.clone();
    search_terms.push(entry.title.clone());
    search_terms.sort();
    search_terms.dedup();
    BookLocatorEntry {
        locator_id: entry.material_id.clone(),
        owner_id: owner_id.to_string(),
        owner_kind: owner_kind.to_string(),
        label: entry.title.clone(),
        category: entry.tags.iter().find(|t| t.as_str() != "locator" && t.as_str() != "cold_data").cloned().unwrap_or_else(|| "section".to_string()),
        source_document_id: first_ref.source_id.clone(),
        page_start: page,
        page_end: page,
        heading_path: first_ref.section_path.clone(),
        search_terms,
        summary: entry.summary.clone(),
        confidence: 0.65,
        parse_policy: if matches!(entry.material_type, MaterialType::ColdDataLocator) { "on_demand".to_string() } else { "located".to_string() },
        tags: entry.tags.clone(),
        source_refs: entry.source_refs.clone(),
    }
}

fn split_sql_statements(sql: &str) -> Vec<&str> {
    sql.split(";\n").collect()
}

fn row_to_context_block(row: sqlx::postgres::PgRow) -> Result<ContextBlock> {
    let kind = block_kind_from_str(&row.get::<String, _>("block_kind"));
    let visibility = visibility_from_str(&row.get::<String, _>("visibility"));
    let stability = stability_from_str(&row.get::<String, _>("stability"));
    let cache_zone = cache_zone_from_str(&row.get::<String, _>("cache_zone"));
    let scope_type = scope_type_from_str(&row.get::<String, _>("scope_type"));
    let content: BlockContent = serde_json::from_value(row.get("content_json"))?;
    let source_refs: Vec<SourceRef> = serde_json::from_value(row.get("source_refs"))?;
    let dependencies: Vec<String> = serde_json::from_value(row.get("dependencies"))?;
    Ok(ContextBlock {
        block_id: row.get("block_id"),
        kind,
        title: row.get("title"),
        content,
        visibility,
        stability,
        cache_zone,
        scope: Scope { scope_type, scope_id: row.get("scope_id") },
        priority: row.get("priority"),
        version: row.get::<i32, _>("version") as u32,
        tags: row.get::<Vec<String>, _>("tags"),
        source_refs,
        dependencies,
        content_hash: row.get("content_hash"),
        token_estimate: row.get::<Option<i32>, _>("token_estimate").map(|v| v as u32),
        expires_at_turn: row.get("expires_at_turn"),
        expires_at_scene: row.get("expires_at_scene"),
        load_reason: row.get("load_reason"),
    })
}


fn row_to_memory_event(row: sqlx::postgres::PgRow) -> Result<MemoryEvent> {
    Ok(MemoryEvent {
        event_id: row.get("event_id"),
        session_id: row.get("session_id"),
        turn_id: row.get("turn_id"),
        ruleset_id: row.get("ruleset_id"),
        module_id: row.get("module_id"),
        scene_id: row.get("scene_id"),
        location_id: row.get("location_id"),
        actor_ids: row.get::<Vec<String>, _>("actor_ids"),
        visibility: visibility_from_str(&row.get::<String, _>("visibility")),
        event_kind: memory_kind_from_str(&row.get::<String, _>("event_kind")),
        summary: row.get("summary"),
        transcript_excerpt: row.get("transcript_excerpt"),
        source: row.get("source_json"),
        tags: row.get::<Vec<String>, _>("tags"),
        importance: row.get("importance"),
        occurred_at: row.get("occurred_at"),
    })
}

fn row_to_memory_fact(row: sqlx::postgres::PgRow) -> Result<MemoryFact> {
    Ok(MemoryFact {
        fact_id: row.get("fact_id"),
        session_id: row.get("session_id"),
        scope: Scope { scope_type: scope_type_from_str(&row.get::<String, _>("scope_type")), scope_id: row.get("scope_id") },
        visibility: visibility_from_str(&row.get::<String, _>("visibility")),
        subject: row.get("subject"),
        predicate: row.get("predicate"),
        object: row.get("object_json"),
        summary: row.get("summary"),
        status: memory_status_from_str(&row.get::<String, _>("status")),
        confidence: row.get("confidence"),
        source_event_ids: row.get::<Vec<String>, _>("source_event_ids"),
        tags: row.get::<Vec<String>, _>("tags"),
        importance: row.get("importance"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn row_to_memory_snapshot(row: sqlx::postgres::PgRow) -> Result<MemorySnapshot> {
    Ok(MemorySnapshot {
        snapshot_id: row.get("snapshot_id"),
        session_id: row.get("session_id"),
        ruleset_id: row.get("ruleset_id"),
        module_id: row.get("module_id"),
        scope: Scope { scope_type: scope_type_from_str(&row.get::<String, _>("scope_type")), scope_id: row.get("scope_id") },
        visibility: visibility_from_str(&row.get::<String, _>("visibility")),
        title: row.get("title"),
        summary_markdown: row.get("summary_markdown"),
        included_event_ids: row.get::<Vec<String>, _>("included_event_ids"),
        included_fact_ids: row.get::<Vec<String>, _>("included_fact_ids"),
        version: row.get::<i32, _>("version") as u32,
        token_estimate: row.get::<Option<i32>, _>("token_estimate").map(|v| v as u32),
        content_hash: row.get("content_hash"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn runtime_bundle_id(session_id: &str) -> String {
    format!("runtime.{}", session_id)
}

trait SnakeCase {
    fn to_snake(&self) -> String;
}

impl SnakeCase for String {
    fn to_snake(&self) -> String {
        let mut out = String::new();
        for (i, ch) in self.chars().enumerate() {
            if ch.is_uppercase() {
                if i > 0 { out.push('_'); }
                for c in ch.to_lowercase() { out.push(c); }
            } else {
                out.push(ch);
            }
        }
        out
    }
}

fn cache_zone_from_str(s: &str) -> CacheZone {
    match s {
        "prefix" => CacheZone::Prefix,
        "dynamic_tail" => CacheZone::DynamicTail,
        "never_prompt" => CacheZone::NeverPrompt,
        _ => CacheZone::PinnedMiddle,
    }
}


fn row_to_learned_packet(row: sqlx::postgres::PgRow) -> Result<LearnedPacket> {
    let source_refs: Vec<SourceRef> = serde_json::from_value(row.get("source_refs"))?;
    Ok(LearnedPacket {
        packet_id: row.get("packet_id"),
        ruleset_id: row.get("ruleset_id"),
        module_id: row.get("module_id"),
        packet_type: row.get("packet_type"),
        packet_key: row.get("packet_key"),
        title: row.get("title"),
        summary: row.get("summary"),
        packet_json: row.get("packet_json"),
        source_refs,
        use_count: row.get("use_count"),
        learning_stage: learning_stage_from_str(&row.get::<String, _>("learning_stage")),
        confidence: ruling_confidence_from_str(&row.get::<String, _>("confidence")),
        cache_zone: cache_zone_from_str(&row.get::<String, _>("cache_zone")),
        visibility: visibility_from_str(&row.get::<String, _>("visibility")),
        last_used_at: row.get("last_used_at"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn visibility_from_str(s: &str) -> Visibility {
    match s {
        "public" => Visibility::Public,
        "player_visible" => Visibility::PlayerVisible,
        "npc_private" => Visibility::NpcPrivate,
        "system_only" => Visibility::SystemOnly,
        _ => Visibility::GmOnly,
    }
}

fn stability_from_str(s: &str) -> Stability {
    match s {
        "immutable" => Stability::Immutable,
        "rarely_changed" => Stability::RarelyChanged,
        "turn_dynamic" => Stability::TurnDynamic,
        "ephemeral" => Stability::Ephemeral,
        _ => Stability::SceneStable,
    }
}

fn scope_type_from_str(s: &str) -> ScopeType {
    match s {
        "global" => ScopeType::Global,
        "ruleset" => ScopeType::Ruleset,
        "module" => ScopeType::Module,
        "campaign" => ScopeType::Campaign,
        "chapter" => ScopeType::Chapter,
        "mission" => ScopeType::Mission,
        "location" => ScopeType::Location,
        "npc" => ScopeType::Npc,
        "session" => ScopeType::Session,
        "scene" => ScopeType::Scene,
        "turn" => ScopeType::Turn,
        "material" => ScopeType::Material,
        "character" => ScopeType::Character,
        "object" => ScopeType::Object,
        _ => ScopeType::Global,
    }
}

fn gate_status_from_str(s: &str) -> GateStatus {
    match s {
        "resolved" => GateStatus::Resolved,
        "cancelled" => GateStatus::Cancelled,
        "abandoned_by_new_action" => GateStatus::AbandonedByNewAction,
        "superseded" => GateStatus::Superseded,
        "superseded_by_frame_close" => GateStatus::SupersededByFrameClose,
        "superseded_by_scene_transition" => GateStatus::SupersededBySceneTransition,
        "superseded_by_new_frame" => GateStatus::SupersededByNewFrame,
        "superseded_by_exit_contract" => GateStatus::SupersededByExitContract,
        "superseded_by_world_time_advance" => GateStatus::SupersededByWorldTimeAdvance,
        "superseded_by_reconcile" => GateStatus::SupersededByReconcile,
        "stale_generation" => GateStatus::StaleGeneration,
        "expired" => GateStatus::Expired,
        _ => GateStatus::Open,
    }
}

fn gate_fallback_from_str(s: &str) -> GateFallbackPolicy {
    match s {
        "cancel_gate_and_continue" => GateFallbackPolicy::CancelGateAndContinue,
        "treat_as_option" => GateFallbackPolicy::TreatAsOption,
        "require_explicit_choice" => GateFallbackPolicy::RequireExplicitChoice,
        "resolve_as_no_reaction" => GateFallbackPolicy::ResolveAsNoReaction,
        "abort_current_action" => GateFallbackPolicy::AbortCurrentAction,
        _ => GateFallbackPolicy::Reprompt,
    }
}


fn memory_kind_from_str(s: &str) -> MemoryKind {
    match s {
        "fact" => MemoryKind::Fact,
        "snapshot" => MemoryKind::Snapshot,
        "relationship" => MemoryKind::Relationship,
        "clue_state" => MemoryKind::ClueState,
        "open_thread" => MemoryKind::OpenThread,
        _ => MemoryKind::Event,
    }
}

fn memory_status_from_str(s: &str) -> MemoryStatus {
    match s {
        "superseded" => MemoryStatus::Superseded,
        "contradicted" => MemoryStatus::Contradicted,
        "archived" => MemoryStatus::Archived,
        _ => MemoryStatus::Active,
    }
}


fn learning_stage_from_str(s: &str) -> LearningStage {
    match s {
        "unseen" => LearningStage::Unseen,
        "looked_up" => LearningStage::LookedUp,
        "used_once" => LearningStage::UsedOnce,
        "stable" => LearningStage::Stable,
        "memorized" => LearningStage::Memorized,
        _ => LearningStage::Located,
    }
}

fn ruling_confidence_from_str(s: &str) -> RulingConfidence {
    match s {
        "high" => RulingConfidence::High,
        "low" => RulingConfidence::Low,
        _ => RulingConfidence::Medium,
    }
}


fn row_to_world_time_state(row: sqlx::postgres::PgRow) -> Result<WorldTimeState> {
    Ok(WorldTimeState {
        session_id: row.get("session_id"),
        campaign_id: row.get("campaign_id"),
        world_tick: row.get("world_tick"),
        absolute_seconds: row.get("absolute_seconds"),
        calendar_id: row.get("calendar_id"),
        display_time: row.get("display_time"),
        time_scale: time_scale_from_str(&row.get::<String, _>("time_scale")),
        scene_epoch: row.get("scene_epoch"),
        turn_seq: row.get("turn_seq"),
        event_seq: row.get("event_seq"),
        updated_at: row.get("updated_at"),
    })
}

fn row_to_world_event(row: sqlx::postgres::PgRow) -> Result<WorldEvent> {
    let source_refs_value: serde_json::Value = row.get("source_refs");
    Ok(WorldEvent {
        event_id: row.get("event_id"),
        campaign_id: row.get("campaign_id"),
        session_id: row.get("session_id"),
        world_tick: row.get("world_tick"),
        event_seq: row.get("event_seq"),
        turn_id: row.get("turn_id"),
        frame_id: row.get("frame_id"),
        event_kind: world_event_kind_from_str(&row.get::<String, _>("event_kind")),
        event_json: row.get("event_json"),
        visibility: visibility_from_str(&row.get::<String, _>("visibility")),
        source_refs: serde_json::from_value(source_refs_value).unwrap_or_default(),
        caused_by_event_ids: row.get("caused_by_event_ids"),
        state_patch_ids: row.get("state_patch_ids"),
        created_at: row.get("created_at"),
    })
}

fn row_to_scheduled_event(row: sqlx::postgres::PgRow) -> Result<ScheduledEvent> {
    Ok(ScheduledEvent {
        scheduled_event_id: row.get("scheduled_event_id"),
        campaign_id: row.get("campaign_id"),
        session_id: row.get("session_id"),
        due_tick: row.get("due_tick"),
        event_kind: world_event_kind_from_str(&row.get::<String, _>("event_kind")),
        payload_json: row.get("payload_json"),
        visibility: visibility_from_str(&row.get::<String, _>("visibility")),
        status: scheduled_status_from_str(&row.get::<String, _>("status")),
        created_by_event_id: row.get("created_by_event_id"),
        created_at: row.get("created_at"),
    })
}

fn row_to_context_watermark(row: sqlx::postgres::PgRow) -> Result<ContextWatermark> {
    Ok(ContextWatermark {
        session_id: row.get("session_id"),
        last_compiled_world_tick: row.get("last_compiled_world_tick"),
        last_compiled_event_seq: row.get("last_compiled_event_seq"),
        compiled_context_hash: row.get("compiled_context_hash"),
        updated_at: row.get("updated_at"),
    })
}

fn time_scale_from_str(s: &str) -> TimeScale {
    match s {
        "instant" => TimeScale::Instant,
        "combat_round" => TimeScale::CombatRound,
        "exploration" => TimeScale::Exploration,
        "travel" => TimeScale::Travel,
        "downtime" => TimeScale::Downtime,
        "flashback" => TimeScale::Flashback,
        _ => TimeScale::SceneBeat,
    }
}

fn world_event_kind_from_str(s: &str) -> WorldEventKind {
    match s {
        "player_action" => WorldEventKind::PlayerAction,
        "npc_action" => WorldEventKind::NpcAction,
        "check_created" => WorldEventKind::CheckCreated,
        "check_resolved" => WorldEventKind::CheckResolved,
        "effect_created" => WorldEventKind::EffectCreated,
        "effect_applied" => WorldEventKind::EffectApplied,
        "exit_contract_created" => WorldEventKind::ExitContractCreated,
        "stalemate_contract_created" => WorldEventKind::StalemateContractCreated,
        "clock_tick" => WorldEventKind::ClockTick,
        "time_advanced" => WorldEventKind::TimeAdvanced,
        "scene_changed" => WorldEventKind::SceneChanged,
        "clue_discovered" => WorldEventKind::ClueDiscovered,
        "npc_attitude_changed" => WorldEventKind::NpcAttitudeChanged,
        "memory_fact_created" => WorldEventKind::MemoryFactCreated,
        "learning_candidate_created" => WorldEventKind::LearningCandidateCreated,
        "table_ruling" => WorldEventKind::TableRuling,
        "frame_opened" => WorldEventKind::FrameOpened,
        "frame_closed" => WorldEventKind::FrameClosed,
        "director_brief_created" => WorldEventKind::DirectorBriefCreated,
        "novelty_fresh_change" => WorldEventKind::NoveltyFreshChange,
        "scheduled_event_due" => WorldEventKind::ScheduledEventDue,
        "retcon_compensation" => WorldEventKind::RetconCompensation,
        _ => WorldEventKind::SystemEvent,
    }
}

fn scheduled_status_from_str(s: &str) -> ScheduledEventStatus {
    match s {
        "due" => ScheduledEventStatus::Due,
        "fired" => ScheduledEventStatus::Fired,
        "cancelled" => ScheduledEventStatus::Cancelled,
        "superseded" => ScheduledEventStatus::Superseded,
        _ => ScheduledEventStatus::Pending,
    }
}

fn block_kind_from_str(s: &str) -> BlockKind {
    match s {
        "engine_protocol" => BlockKind::EngineProtocol,
        "output_schema" => BlockKind::OutputSchema,
        "tool_protocol" => BlockKind::ToolProtocol,
        "ruleset_resident_core" => BlockKind::RulesetResidentCore,
        "ruleset_world_style" => BlockKind::RulesetWorldStyle,
        "ruleset_director_policy" => BlockKind::RulesetDirectorPolicy,
        "ruleset_action_router" => BlockKind::RulesetActionRouter,
        "ruleset_index" => BlockKind::RulesetIndex,
        "ruleset_character_kernel" => BlockKind::RulesetCharacterKernel,
        "ruleset_onboarding" => BlockKind::RulesetOnboarding,
        "rule_steward_kernel" => BlockKind::RuleStewardKernel,
        "rule_agent_run" => BlockKind::RuleAgentRun,
        "rule_kernel_patch" => BlockKind::RuleKernelPatch,
        "rule_entity_locator" => BlockKind::RuleEntityLocator,
        "mechanical_source_pack" => BlockKind::MechanicalSourcePack,
        "playability_gate_report" => BlockKind::PlayabilityGateReport,
        "character_onboarding_pack" => BlockKind::CharacterOnboardingPack,
        "character_creation_flow" => BlockKind::CharacterCreationFlow,
        "character_option_catalog" => BlockKind::CharacterOptionCatalog,
        "derived_formula_pack" => BlockKind::DerivedFormulaPack,
        "starter_character_pack" => BlockKind::StarterCharacterPack,
        "game_identity" => BlockKind::GameIdentity,
        "play_loop" => BlockKind::PlayLoop,
        "book_locator" => BlockKind::BookLocator,
        "gm_onboarding" => BlockKind::GmOnboarding,
        "book_locator_summary" => BlockKind::BookLocatorSummary,
        "cold_data_locator" => BlockKind::ColdDataLocator,
        "lookup_recipe" => BlockKind::LookupRecipe,
        "learned_packet" => BlockKind::LearnedPacket,
        "rule_package" => BlockKind::RulePackage,
        "procedure_detail" => BlockKind::ProcedureDetail,
        "procedure_variant" => BlockKind::ProcedureVariant,
        "parameter_family" => BlockKind::ParameterFamily,
        "parameter_entry" => BlockKind::ParameterEntry,
        "domain_actor" => BlockKind::DomainActor,
        "domain_object" => BlockKind::DomainObject,
        "object_definition" => BlockKind::ObjectDefinition,
        "object_instance" => BlockKind::ObjectInstance,
        "object_affordance" => BlockKind::ObjectAffordance,
        "object_interaction_contract" => BlockKind::ObjectInteractionContract,
        "object_patch" => BlockKind::ObjectPatch,
        "object_event" => BlockKind::ObjectEvent,
        "object_graph" => BlockKind::ObjectGraph,
        "domain_ability" => BlockKind::DomainAbility,
        "ability_definition" => BlockKind::AbilityDefinition,
        "ability_instance" => BlockKind::AbilityInstance,
        "ability_trigger_binding" => BlockKind::AbilityTriggerBinding,
        "ability_activation_contract" => BlockKind::AbilityActivationContract,
        "ability_graph" => BlockKind::AbilityGraph,
        "rule_binding_packet" => BlockKind::RuleBindingPacket,
        "semantic_classification" => BlockKind::SemanticClassification,
        "materialization_demand" => BlockKind::MaterializationDemand,
        "source_evidence_bundle" => BlockKind::SourceEvidenceBundle,
        "extraction_run" => BlockKind::ExtractionRun,
        "binding_verification" => BlockKind::BindingVerification,
        "actor_runtime_binding" => BlockKind::ActorRuntimeBinding,
        "object_runtime_binding" => BlockKind::ObjectRuntimeBinding,
        "ability_runtime_binding" => BlockKind::AbilityRuntimeBinding,
        "materialization_graph" => BlockKind::MaterializationGraph,
        "player_value_claim" => BlockKind::PlayerValueClaim,
        "player_value_verification" => BlockKind::PlayerValueVerification,
        "table_override_agreement" => BlockKind::TableOverrideAgreement,
        "referee_ruling" => BlockKind::RefereeRuling,
        "mechanical_ledger" => BlockKind::MechanicalLedger,
        "attack_resolution_contract" => BlockKind::AttackResolutionContract,
        "damage_packet" => BlockKind::DamagePacket,
        "roll_plan" => BlockKind::RollPlan,
        "effect_resolution_packet" => BlockKind::EffectResolutionPacket,
        "parameter_impact" => BlockKind::ParameterImpact,
        "actor_mechanical_state" => BlockKind::ActorMechanicalState,
        "combat_round_action" => BlockKind::CombatRoundAction,
        "combat_round_transition" => BlockKind::CombatRoundTransition,
        "contest_profile" => BlockKind::ContestProfile,
        "opposition_profile" => BlockKind::OppositionProfile,
        "contest_resolution" => BlockKind::ContestResolution,
        "contest_graph" => BlockKind::ContestGraph,
        "domain_info" => BlockKind::DomainInfo,
        "domain_place" => BlockKind::DomainPlace,
        "domain_scenario" => BlockKind::DomainScenario,
        "state_trait" => BlockKind::StateTrait,
        "state_track" => BlockKind::StateTrack,
        "state_status" => BlockKind::StateStatus,
        "state_aspect" => BlockKind::StateAspect,
        "state_modifier" => BlockKind::StateModifier,
        "scenario_node" => BlockKind::ScenarioNode,
        "mission_static" => BlockKind::MissionStatic,
        "clue" => BlockKind::Clue,
        "handout" => BlockKind::Handout,
        "npc_static" => BlockKind::NpcStatic,
        "location_static" => BlockKind::LocationStatic,
        "chapter_static" => BlockKind::ChapterStatic,
        "module_spine" => BlockKind::ModuleSpine,
        "module_style" => BlockKind::ModuleStyle,
        "module_specific_rule" => BlockKind::ModuleSpecificRule,
        "module_overview" => BlockKind::ModuleOverview,
        "current_session_packet" => BlockKind::CurrentSessionPacket,
        "session_summary" => BlockKind::SessionSummary,
        "memory_snapshot" => BlockKind::MemorySnapshot,
        "memory_fact" => BlockKind::MemoryFact,
        "memory_event" => BlockKind::MemoryEvent,
        "scene_static" => BlockKind::SceneStatic,
        "agent_plan" => BlockKind::AgentPlan,
        "agent_advice" => BlockKind::AgentAdvice,
        "check_contract" => BlockKind::CheckContract,
        "pending_check" => BlockKind::PendingCheck,
        "interaction_gate" => BlockKind::InteractionGate,
        "dice_roll" => BlockKind::DiceRoll,
        "state_frame" => BlockKind::StateFrame,
        "frame_event" => BlockKind::FrameEvent,
        "frame_compaction" => BlockKind::FrameCompaction,
        "combat_frame" => BlockKind::CombatFrame,
        "combat_event" => BlockKind::CombatEvent,
        "effect_contract" => BlockKind::EffectContract,
        "combat_profile" => BlockKind::CombatProfile,
        "reaction_window" => BlockKind::ReactionWindow,
        "director_policy" => BlockKind::DirectorPolicy,
        "actionable_situation_brief" => BlockKind::ActionableSituationBrief,
        "clue_board" => BlockKind::ClueBoard,
        "consequence_contract" => BlockKind::ConsequenceContract,
        "spotlight_state" => BlockKind::SpotlightState,
        "clock_event" => BlockKind::ClockEvent,
        "world_time" => BlockKind::WorldTime,
        "world_event" => BlockKind::WorldEvent,
        "time_advance" => BlockKind::TimeAdvance,
        "scheduled_event" => BlockKind::ScheduledEvent,
        "time_anchor" => BlockKind::TimeAnchor,
        "world_state" => BlockKind::WorldState,
        "retrieved_memory" => BlockKind::RetrievedMemory,
        "lookup_result" => BlockKind::LookupResult,
        "ruling" => BlockKind::Ruling,
        "ruling_log" => BlockKind::RulingLog,
        "recent_transcript" => BlockKind::RecentTranscript,
        "current_input" => BlockKind::CurrentInput,
        _ => BlockKind::RetrievedMemory,
    }
}

/// Data-only kernel patch file: `{TRPG_DATA_DIR}/parsed/rules/{ruleset}.rule_kernel.override.json`
/// shaped `{"resource_tracks": [ ... ], "dice_core": { ... }}`. Mirrors the chargen
/// override convention. Returns the parsed override doc, or None when absent/unreadable.
fn read_kernel_override_file(ruleset_id: &str) -> Option<serde_json::Value> {
    let safe: String = ruleset_id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
    let dir = std::env::var("TRPG_DATA_DIR").unwrap_or_else(|_| "data".into());
    let p = std::path::Path::new(&dir).join("parsed").join("rules").join(format!("{safe}.rule_kernel.override.json"));
    let text = std::fs::read_to_string(&p).ok()?;
    serde_json::from_str(&text).ok()
}

/// Shallow-merge an override `dice_core` object onto the base (override keys win).
/// Container values like `success_bands` (an array whose ids may repeat, e.g. two
/// fumble bands) are REPLACED wholesale, not deep-merged — the override supplies the
/// full corrected value for any key it sets. A non-object base is replaced entirely.
fn merge_dice_core(base: serde_json::Value, over: &serde_json::Map<String, serde_json::Value>) -> serde_json::Value {
    let mut obj = base.as_object().cloned().unwrap_or_default();
    for (k, v) in over { obj.insert(k.clone(), v.clone()); }
    serde_json::Value::Object(obj)
}

/// Merge override resource_tracks into base by track `id` (case-insensitive):
/// an override replaces a same-id base track; new ids are appended. Mirrors
/// chargen `merge_override`.
fn merge_resource_tracks(base: Vec<serde_json::Value>, overrides: Vec<serde_json::Value>) -> Vec<serde_json::Value> {
    let idof = |v: &serde_json::Value| v.get("id").and_then(|x| x.as_str()).unwrap_or("").trim().to_ascii_lowercase();
    let mut out = base;
    for o in overrides {
        let oid = idof(&o);
        if oid.is_empty() { continue; }
        if let Some(slot) = out.iter_mut().find(|b| idof(b) == oid) { *slot = o; } else { out.push(o); }
    }
    out
}

#[cfg(test)]
mod dice_core_override_tests {
    use super::merge_dice_core;
    use serde_json::json;

    #[test]
    fn override_replaces_bands_wholesale_and_keeps_untouched_keys() {
        let base = json!({"compare":"roll_under","success_bands":[{"id":"regular"},{"id":"extreme"}]});
        let over = json!({"success_bands":[{"id":"critical"},{"id":"failure"}]});
        let merged = merge_dice_core(base, over.as_object().unwrap());
        // untouched base key survives
        assert_eq!(merged.get("compare").and_then(|v| v.as_str()), Some("roll_under"));
        // success_bands REPLACED wholesale, not appended/merged-by-id
        let ids: Vec<&str> = merged.get("success_bands").and_then(|v| v.as_array()).unwrap()
            .iter().filter_map(|b| b.get("id").and_then(|x| x.as_str())).collect();
        assert_eq!(ids, vec!["critical", "failure"], "array key must be replaced, not deep-merged");
    }

    #[test]
    fn non_object_base_is_replaced_by_override() {
        let merged = merge_dice_core(json!(null), json!({"compare":"meet_or_beat"}).as_object().unwrap());
        assert_eq!(merged.get("compare").and_then(|v| v.as_str()), Some("meet_or_beat"));
    }
}

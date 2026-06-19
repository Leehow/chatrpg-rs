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

/// 迁移串行化用的 Postgres 事务级顾问锁键。任意稳定常量即可：所有 `Db::migrate()`
/// 取同一把锁，使跨连接/跨进程（含并行 cargo 测试）的迁移不会交错执行非幂等 DDL。
const MIGRATION_ADVISORY_LOCK_KEY: i64 = 0x6b6e_6f77_6d69_67;

/// 通用 durable KnowledgeEdge upsert 入参（v1）。borrow 短生命周期，避免拷贝。
/// holder_kind 必须是 durable 可持久化 token（gm/player_party/system/npc/pc/faction）；
/// 否则 [`Db::upsert_knowledge_edge`] 在写库前 fail-closed 拒绝。
/// 当 holder_kind 为带身份 holder（npc/pc/faction）时，holder_id 还须经 actor-identity 契约
/// 校验为稳定 id，否则同样 fail-closed（不写任何边）。
pub struct KnowledgeEdgeInput<'a> {
    pub session_id: &'a str,
    pub holder_kind: &'a str,
    /// 集合 holder（gm/player_party/system）用空串 `""`；带身份 holder（npc/pc/faction）用其稳定 id。
    pub holder_id: &'a str,
    pub fact_id: &'a str,
    pub knowledge_state: &'a str,
    pub confidence: Option<f64>,
    pub learned_at_turn_id: Option<&'a str>,
    pub disclosure_policy: Option<&'a str>,
    pub source_event_id: Option<&'a str>,
    pub reason: Option<&'a str>,
}

/// 一条 durable **world fact** 行（P3 WorldFact 一等化）。列同 `WorldFactCandidate`
/// 字段（fact_id 身份 + subject/predicate/object/summary + truth_status 命题真假轴 +
/// 证据 source_event_ids + turn_id provenance + confidence）。owned 结构，便于 runtime
/// 提交路径透传，绝不与 `memory_facts`（记忆三元组）/`knowledge_edges`（谁知道）混用。
#[derive(Debug, Clone, PartialEq)]
pub struct WorldFactRow {
    pub fact_id: String,
    pub session_id: String,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub summary: String,
    /// 命题自身真假分类（`FactTruthStatus::as_token`）；None = 未标（列存 NULL）。
    pub truth_status: Option<String>,
    pub source_event_ids: Vec<String>,
    pub turn_id: Option<String>,
    pub confidence: Option<f32>,
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
            include_str!(
                "../../../migrations/0023_roll_binding_mechanical_gate_priority_v1122.sql"
            ),
            include_str!("../../../migrations/0024_parameter_facet_executor_v113.sql"),
            include_str!("../../../migrations/0025_rule_steward_character_onboarding_v116.sql"),
            include_str!("../../../migrations/0026_session_current_scene_v120.sql"),
            include_str!("../../../migrations/0027_mechanic_dues_v120.sql"),
            include_str!("../../../migrations/0028_turn_pp_lifecycle_v120.sql"),
            include_str!("../../../migrations/0029_turn_trace_failure_v120.sql"),
            include_str!("../../../migrations/0030_domain_events.sql"),
            include_str!("../../../migrations/0031_memory_fact_turn_id.sql"),
            include_str!("../../../migrations/0032_knowledge_edges.sql"),
            include_str!("../../../migrations/0033_knowledge_edges_v1_fields.sql"),
            include_str!("../../../migrations/0034_knowledge_edges_npc_holders.sql"),
            include_str!("../../../migrations/0035_npc_relationships.sql"),
            include_str!("../../../migrations/0036_npc_profiles.sql"),
            include_str!("../../../migrations/0037_knowledge_edges_pc_faction_holders.sql"),
            // 补接预存在却漏接的 0038（memory_facts.truth_status 加性列）——文件早已落盘但从未
            // 进数组（P3 前缺口）。追加到数组 TAIL，不改动既有迁移顺序/内容（幂等 add column
            // if not exists，重放安全）。
            include_str!("../../../migrations/0038_memory_fact_truth_status.sql"),
            // 0039 world_facts 表（WorldFact 一等化，P3）。幂等 create table if not exists。
            include_str!("../../../migrations/0039_world_facts.sql"),
        ];
        // 在单一事务内先取事务级顾问锁，串行化所有并发/跨进程 migrate() 调用。
        // 0033 等迁移用 drop-then-add 重建命名 CHECK 约束（非幂等的两段式 DDL），
        // 并行测试若各自 migrate() 会让 drop 与 add 交错，触发重复约束错误。
        // 事务级顾问锁在 commit/rollback 时自动释放，不会泄漏；已确认所有迁移
        // 语句均可在事务中执行（无 CONCURRENTLY / VACUUM / CREATE DATABASE）。
        let mut tx = self
            .pool
            .begin()
            .await
            .context("failed to begin migration transaction")?;
        sqlx::query("select pg_advisory_xact_lock($1)")
            .bind(MIGRATION_ADVISORY_LOCK_KEY)
            .execute(&mut *tx)
            .await
            .context("failed to acquire migration advisory lock")?;
        for sql in migrations {
            for statement in split_sql_statements(sql) {
                let trimmed = statement.trim();
                if trimmed.is_empty() {
                    continue;
                }
                sqlx::query(trimmed)
                    .execute(&mut *tx)
                    .await
                    .with_context(|| {
                        format!(
                            "failed migration statement: {}",
                            trimmed.chars().take(120).collect::<String>()
                        )
                    })?;
            }
        }
        tx.commit()
            .await
            .context("failed to commit migration transaction")?;
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

    pub async fn has_bundle_for_source(
        &self,
        source_hash: &str,
        parse_config_hash: &str,
        bundle_kind: &str,
    ) -> Result<bool> {
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

    pub async fn upsert_rule_bundle(
        &self,
        bundle: &RuleBundle,
        artifact_path: Option<&str>,
        source_hash: &str,
        parse_config_hash: &str,
    ) -> Result<()> {
        self.upsert_parsed_bundle(
            &bundle.bundle_id,
            "ruleset",
            &bundle.title,
            &bundle.schema_version,
            artifact_path,
            source_hash,
            parse_config_hash,
            bundle,
            &bundle.validation_report,
        )
        .await?;
        for block in &bundle.context_blocks {
            self.upsert_context_block(&bundle.bundle_id, block).await?;
        }
        for entry in &bundle.material_index {
            self.upsert_material_entry(&bundle.bundle_id, entry).await?;
        }
        for template in &bundle.character_templates {
            self.upsert_character_template(&bundle.bundle_id, template)
                .await?;
        }
        for pack in &bundle.character_onboarding_packs {
            self.upsert_character_onboarding_pack(pack).await?;
        }
        if let Some(kernel) = &bundle.rule_kernel {
            self.upsert_rule_kernel(kernel).await?;
        }
        if let Some(onboarding) = &bundle.gm_onboarding {
            self.upsert_onboarding_bundle(onboarding).await?;
            for locator in onboarding
                .book_locator
                .iter()
                .chain(onboarding.cold_data_locator.iter())
            {
                self.upsert_book_locator_entry(locator).await?;
            }
        }
        Ok(())
    }

    pub async fn upsert_module_bundle(
        &self,
        bundle: &ModuleBundle,
        artifact_path: Option<&str>,
        source_hash: &str,
        parse_config_hash: &str,
    ) -> Result<()> {
        self.upsert_parsed_bundle(
            &bundle.bundle_id,
            "module",
            &bundle.title,
            &bundle.schema_version,
            artifact_path,
            source_hash,
            parse_config_hash,
            bundle,
            &bundle.validation_report,
        )
        .await?;
        for block in &bundle.context_blocks {
            self.upsert_context_block(&bundle.bundle_id, block).await?;
        }
        for entry in &bundle.material_index {
            self.upsert_material_entry(&bundle.bundle_id, entry).await?;
            if matches!(
                entry.material_type,
                MaterialType::BookLocator | MaterialType::ColdDataLocator
            ) {
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

    pub async fn upsert_project_bundle(
        &self,
        bundle: &ProjectBundle,
        artifact_path: Option<&str>,
        source_hash: &str,
        parse_config_hash: &str,
    ) -> Result<()> {
        self.upsert_parsed_bundle(
            &bundle.project_id,
            "project",
            "Local Project",
            &bundle.schema_version,
            artifact_path,
            source_hash,
            parse_config_hash,
            bundle,
            &bundle.validation_report,
        )
        .await
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

    pub async fn upsert_runtime_context_block(
        &self,
        session_id: &str,
        block: &ContextBlock,
    ) -> Result<()> {
        let bundle_id = runtime_bundle_id(session_id);
        self.upsert_context_block(&bundle_id, block).await
    }

    pub async fn list_runtime_context_blocks(
        &self,
        session_id: &str,
        turn_id: &str,
        scene_id: Option<&str>,
    ) -> Result<Vec<ContextBlock>> {
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

    pub async fn deactivate_runtime_turn_blocks(
        &self,
        session_id: &str,
        turn_id: &str,
    ) -> Result<()> {
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

    pub async fn upsert_material_entry(
        &self,
        bundle_id: &str,
        entry: &MaterialIndexEntry,
    ) -> Result<()> {
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

    pub async fn upsert_character_template(
        &self,
        bundle_id: &str,
        template: &CharacterTemplate,
    ) -> Result<()> {
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
        Ok(rows
            .into_iter()
            .map(|r| {
                json!({
                    "bundle_id": r.get::<String,_>("bundle_id"),
                    "bundle_kind": r.get::<String,_>("bundle_kind"),
                    "title": r.get::<String,_>("title"),
                    "schema_version": r.get::<String,_>("schema_version"),
                    "artifact_path": r.get::<Option<String>,_>("artifact_path"),
                })
            })
            .collect())
    }

    pub async fn load_rule_bundle_by_source(
        &self,
        source_hash: &str,
        parse_config_hash: &str,
    ) -> Result<Option<RuleBundle>> {
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

    pub async fn load_module_bundle_by_source(
        &self,
        source_hash: &str,
        parse_config_hash: &str,
    ) -> Result<Option<ModuleBundle>> {
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
    pub async fn load_project_bundle_for_ruleset(
        &self,
        ruleset_id: &str,
    ) -> Result<Option<ProjectBundle>> {
        let row = sqlx::query(r#"select content_json from parsed_bundles where bundle_kind = 'project' and content_json->'rulesets' @> jsonb_build_array(jsonb_build_object('ruleset_id', $1::text)) order by updated_at desc limit 1"#)
            .bind(ruleset_id)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(r) => Ok(Some(serde_json::from_value(r.get("content_json"))?)),
            None => Ok(None),
        }
    }

    pub async fn load_character_template(
        &self,
        ruleset_id: &str,
    ) -> Result<Option<CharacterTemplate>> {
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

    pub async fn upsert_character_onboarding_pack(
        &self,
        pack: &CharacterOnboardingPack,
    ) -> Result<()> {
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
        .bind(serde_json::to_value(
            &pack.starter_character_pack.source_refs,
        )?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn load_character_onboarding_pack(
        &self,
        ruleset_id: &str,
    ) -> Result<Option<CharacterOnboardingPack>> {
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
        let Some(r) = row else {
            return Ok(None);
        };
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
                kernel.resource_tracks =
                    merge_resource_tracks(kernel.resource_tracks, tracks.to_vec());
            }
            if let Some(dc) = doc.get("dice_core").and_then(|d| d.as_object()) {
                kernel.dice_core = merge_dice_core(kernel.dice_core, dc);
            }
            // P0-2: layer the typed strategy policy keys (override wins wholesale).
            // These migrate trpg-combat/trpg-referee's per-ruleset Rust branches
            // into data; a malformed override value is ignored (fail-closed → the
            // kernel field stays None → engine uses GENERIC_*).
            apply_kernel_strategy_overrides(&mut kernel, &doc);
            // 去重共享 derived_from 的 track（align 可能建 stub 而 override 又提供了正式 track）。
            kernel.resource_tracks =
                trpg_model::dedup_tracks_by_derived_from(&kernel.resource_tracks);
        }
        Ok(Some(kernel))
    }

    /// load a module's lightweight engine config: **extracted ⊕ override**.
    /// - `extracted`: director facilitation auto-extracted by the module reader,
    ///   stored in `ModuleGraph.director_facilitation` (read via `load_module_graph`).
    /// - `override`: the optional sidecar `{TRPG_DATA_DIR}/modules/{id}.module_config.json`
    ///   (file → embedded fallback), highest precedence (override > extracted).
    /// `merge_module_config` resolves the `director` field; ModuleConfig's other fields
    /// (npc_actor_bindings / technical_option_table / aliases / search profile) still come
    /// only from the sidecar. None when neither side present → engine falls back to neutral.
    pub async fn load_module_config(&self, module_id: &str) -> Option<ModuleConfig> {
        let extracted = self
            .load_module_graph(module_id)
            .await
            .ok()
            .flatten()
            .and_then(|g| g.director_facilitation);
        merge_module_config(read_module_config_file(module_id), extracted)
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

    pub async fn list_rule_kernel_patches(
        &self,
        ruleset_id: &str,
        status: Option<&str>,
        limit: i64,
    ) -> Result<Vec<RuleKernelPatch>> {
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
        rows.into_iter()
            .map(|r| {
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
            })
            .collect()
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
        let exists: bool = sqlx::query_scalar(
            r#"select exists(select 1 from module_prep_packets where module_id = $1)"#,
        )
        .bind(module_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(exists)
    }

    pub async fn list_context_blocks_for_bundles(
        &self,
        bundle_ids: &[String],
    ) -> Result<Vec<ContextBlock>> {
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

    pub async fn find_material_blocks(
        &self,
        bundle_ids: &[String],
        material_refs: &[String],
    ) -> Result<Vec<ContextBlock>> {
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

    pub async fn save_character(
        &self,
        character: &CharacterSheet,
        postprocess_status: &str,
    ) -> Result<()> {
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

    pub async fn create_session(
        &self,
        session_id: &str,
        ruleset_id: &str,
        module_id: Option<&str>,
    ) -> Result<()> {
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
        sqlx::query(
            "update sessions set current_scene_id = $2, updated_at = now() where session_id = $1",
        )
        .bind(session_id)
        .bind(scene_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 读取该会话当前模组场景 node_id（无则 None）。
    pub async fn load_session_scene(&self, session_id: &str) -> Result<Option<String>> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("select current_scene_id from sessions where session_id = $1")
                .bind(session_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.and_then(|r| r.0).filter(|s| !s.trim().is_empty()))
    }

    pub async fn save_turn(
        &self,
        session_id: &str,
        turn_id: &str,
        user_input: &str,
        assistant_output: &str,
        context_hashes: serde_json::Value,
        postprocess_status: &str,
    ) -> Result<()> {
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

    /// P1-2：拼接会话最近 N 回合的对白成单段 transcript（旧→新时间序），供 API
    /// ServerRecent 历史策略注入 prepare_turn_context——前端只传 user_input 时
    /// 第二回合不再丢上下文（与 CLI play 循环 `Player: …\nGM: …` 约定一致）。
    /// 无回合 → None（调用方据此不注入空 transcript 块）。
    pub async fn load_recent_transcript(&self, session_id: &str, n: i64) -> Result<Option<String>> {
        // 取最近 N 回合（created_at desc）再反转回时间序——transcript 自上而下 = 旧→新。
        let rows: Vec<(String, Option<String>)> = sqlx::query_as(
            "select user_input, assistant_output from turns \
             where session_id = $1 order by created_at desc limit $2",
        )
        .bind(session_id)
        .bind(n)
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        let mut transcript = String::new();
        for (user_input, assistant_output) in rows.into_iter().rev() {
            let gm = assistant_output.unwrap_or_default();
            transcript.push_str(&format!("\nPlayer: {user_input}\nGM: {gm}\n"));
        }
        Ok(Some(transcript))
    }

    /// R5：流转某回合的 postprocess 生命周期阶段（critical 末写 critical_done、heavy 末写 complete）。
    /// 只触 pp_lifecycle 列，绝不动 postprocess_status（两列正交）。
    pub async fn set_turn_pp_lifecycle(&self, turn_id: &str, phase: &str) -> Result<()> {
        sqlx::query("update turns set pp_lifecycle = $2, updated_at = now() where turn_id = $1")
            .bind(turn_id)
            .bind(phase)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// R5：取该会话最近一回合（created_at desc）的 pp_lifecycle；无回合返 None（守卫据此立即放行）。
    pub async fn load_last_turn_pp_lifecycle(&self, session_id: &str) -> Result<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as(
            "select pp_lifecycle from turns where session_id = $1 order by created_at desc limit 1",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| r.0))
    }

    /// obs T4：失败语义落库——回合失败时把归类值写进 turns.failure_kind
    /// （NULL=成功）。fail-closed：失败不再伪装成空 TurnComplete。只触 failure_kind
    /// 列，与 pp_lifecycle / postprocess_status 正交（mirror set_turn_pp_lifecycle）。
    pub async fn set_turn_failure_kind(&self, turn_id: &str, failure_kind: &str) -> Result<()> {
        sqlx::query("update turns set failure_kind = $2, updated_at = now() where turn_id = $1")
            .bind(turn_id)
            .bind(failure_kind)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// obs T4：Flight Recorder 持久化——TurnTrace 序列化为 jsonb 落 turn_traces，
    /// turn_id PK upsert（同回合重写覆盖 trace_json）。write-through、fail-soft：
    /// 调用方写失败仅 warn，绝不影响回合主流程或已发事件。
    pub async fn upsert_turn_trace(&self, trace: &trpg_model::TurnTrace) -> Result<()> {
        let trace_json = serde_json::to_value(trace)?;
        sqlx::query(
            r#"insert into turn_traces (turn_id, session_id, trace_json)
               values ($1, $2, $3)
               on conflict (turn_id) do update set trace_json = excluded.trace_json"#,
        )
        .bind(&trace.turn_id)
        .bind(&trace.session_id)
        .bind(trace_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// obs T4：按 turn_id 取回飞行记录；无记录返 None。jsonb 反序列化回 TurnTrace
    /// （字段全 #[serde(default)]，旧/残行向后兼容）。
    pub async fn load_turn_trace(&self, turn_id: &str) -> Result<Option<trpg_model::TurnTrace>> {
        let row: Option<(serde_json::Value,)> =
            sqlx::query_as("select trace_json from turn_traces where turn_id = $1")
                .bind(turn_id)
                .fetch_optional(&self.pool)
                .await?;
        match row {
            Some((value,)) => Ok(Some(serde_json::from_value(value)?)),
            None => Ok(None),
        }
    }

    /// 取一个会话全部回合的 Flight-Recorder trace（created_at 升序）。
    /// 反序列化镜像 `load_turn_trace`（jsonb → TurnTrace）。`trpg coverage`
    /// 聚合用：只读，单一 trace_json 事实源。
    pub async fn list_turn_traces(
        &self,
        session_id: &str,
        limit: i64,
    ) -> Result<Vec<trpg_model::TurnTrace>> {
        let rows: Vec<(serde_json::Value,)> = sqlx::query_as(
            "select trace_json from turn_traces where session_id = $1 order by created_at asc limit $2",
        )
        .bind(session_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|(value,)| serde_json::from_value(value).map_err(Into::into))
            .collect()
    }

    pub async fn record_load_event(
        &self,
        session_id: Option<&str>,
        turn_id: Option<&str>,
        block: &ContextBlock,
        reason: &str,
    ) -> Result<()> {
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
               summary, status, confidence, source_event_ids, tags, importance, turn_id, created_at, updated_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)
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
              turn_id = excluded.turn_id,
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
        .bind(&fact.turn_id)
        .bind(fact.created_at)
        .bind(fact.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// P3 WorldFact 一等化：upsert 一条 durable world fact（`world_facts` 表）。与
    /// `upsert_memory_fact` 同风格（on conflict (fact_id) do update），但落独立的
    /// world_facts 表（世界事实身份源）而非 memory_facts（记忆三元组）。幂等可重放。
    pub async fn upsert_world_fact(&self, fact: &WorldFactRow) -> Result<()> {
        let now = Utc::now();
        sqlx::query(
            r#"
            insert into world_facts
              (fact_id, session_id, subject, predicate, object, summary, truth_status,
               source_event_ids, turn_id, confidence, created_at, updated_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$11)
            on conflict (fact_id) do update set
              session_id = excluded.session_id,
              subject = excluded.subject,
              predicate = excluded.predicate,
              object = excluded.object,
              summary = excluded.summary,
              truth_status = excluded.truth_status,
              source_event_ids = excluded.source_event_ids,
              turn_id = excluded.turn_id,
              confidence = excluded.confidence,
              updated_at = excluded.updated_at
            "#,
        )
        .bind(&fact.fact_id)
        .bind(&fact.session_id)
        .bind(&fact.subject)
        .bind(&fact.predicate)
        .bind(&fact.object)
        .bind(&fact.summary)
        .bind(&fact.truth_status)
        .bind(&fact.source_event_ids)
        .bind(&fact.turn_id)
        .bind(fact.confidence)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// P3 WorldFact 一等化：按 (session_id, fact_id) 读一条 world fact（不存在 → None）。
    /// WorldFact↔KnowledgeEdge 引用契约（弱）的查存在性用。
    pub async fn load_world_fact(
        &self,
        session_id: &str,
        fact_id: &str,
    ) -> Result<Option<WorldFactRow>> {
        let row = sqlx::query(
            r#"
            select fact_id, session_id, subject, predicate, object, summary, truth_status,
                   source_event_ids, turn_id, confidence
            from world_facts
            where session_id = $1 and fact_id = $2
            "#,
        )
        .bind(session_id)
        .bind(fact_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| WorldFactRow {
            fact_id: r.get("fact_id"),
            session_id: r.get("session_id"),
            subject: r.get("subject"),
            predicate: r.get("predicate"),
            object: r.get("object"),
            summary: r.get("summary"),
            truth_status: r.get("truth_status"),
            source_event_ids: r.get("source_event_ids"),
            turn_id: r.get("turn_id"),
            confidence: r.get("confidence"),
        }))
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

    pub async fn list_memory_events(
        &self,
        session_id: &str,
        limit: i64,
    ) -> Result<Vec<MemoryEvent>> {
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
                      summary, status, confidence, source_event_ids, tags, importance, turn_id, created_at, updated_at
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

    pub async fn list_memory_snapshots(
        &self,
        session_id: &str,
        limit: i64,
    ) -> Result<Vec<MemorySnapshot>> {
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
                      summary, status, confidence, source_event_ids, tags, importance, turn_id, created_at, updated_at
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
        Ok(MemoryRetrievalResult {
            snapshots,
            facts: facts?,
            events: events?,
            blocks: vec![],
        })
    }

    pub async fn insert_background_job(
        &self,
        job_id: &str,
        job_kind: &str,
        input_json: serde_json::Value,
    ) -> Result<()> {
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

    pub async fn update_background_job(
        &self,
        job_id: &str,
        status: &str,
        result_json: serde_json::Value,
        error: Option<&str>,
    ) -> Result<()> {
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
        let status: Option<String> =
            sqlx::query_scalar(r#"select status from background_jobs where job_id = $1"#)
                .bind(job_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(status)
    }

    /// Read a `background_jobs` row as a small JSON object carrying `status`,
    /// `result_json`, and `error`. Returns `None` if the job does not exist.
    /// Used by the staged-parse status/SSE endpoints to surface live progress.
    pub async fn load_background_job(&self, job_id: &str) -> Result<Option<serde_json::Value>> {
        let row = sqlx::query(
            r#"select status, result_json, error from background_jobs where job_id = $1"#,
        )
        .bind(job_id)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => {
                let status: String = r.get("status");
                let result_json: serde_json::Value =
                    r.try_get("result_json").unwrap_or(serde_json::Value::Null);
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
    pub async fn latest_staged_job_for_ruleset(
        &self,
        ruleset_id: &str,
    ) -> Result<Option<serde_json::Value>> {
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

    pub async fn search_book_locator_entries(
        &self,
        owner_id: Option<&str>,
        query_text: &str,
        limit: i64,
    ) -> Result<Vec<serde_json::Value>> {
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

    pub async fn list_learned_packets(
        &self,
        ruleset_id: &str,
        module_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<LearnedPacket>> {
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

    pub async fn list_lookup_events_for_demand(
        &self,
        session_id: Option<&str>,
        demand_id: &str,
        limit: i64,
    ) -> Result<Vec<LookupEvent>> {
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

    pub async fn insert_learning_audit_run(
        &self,
        audit_run_id: &str,
        session_id: Option<&str>,
        turn_id: Option<&str>,
        ruleset_id: Option<&str>,
        module_id: Option<&str>,
        status: &str,
        input_json: serde_json::Value,
        result_json: serde_json::Value,
        error: Option<&str>,
    ) -> Result<()> {
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

    pub async fn insert_learning_candidate(
        &self,
        candidate: &LearningAuditCandidate,
    ) -> Result<()> {
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

    pub async fn get_learning_candidate(
        &self,
        candidate_id: &str,
    ) -> Result<Option<LearningAuditCandidate>> {
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

    pub async fn list_learning_candidates(
        &self,
        ruleset_id: Option<&str>,
        status: Option<&str>,
        limit: i64,
    ) -> Result<Vec<LearningAuditCandidate>> {
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

    pub async fn update_learning_candidate_status(
        &self,
        candidate_id: &str,
        status: LearningCandidateStatus,
        notes: Option<&str>,
    ) -> Result<()> {
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

    pub async fn insert_check_contract(
        &self,
        contract: &CheckContract,
        status: &str,
    ) -> Result<()> {
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
    // v1.20 mechanic_dues persistence (B4 watcher; 对标 insert_check_contract)
    // ------------------------------------------------------------------

    pub async fn insert_mechanic_due(&self, due: &MechanicDue) -> Result<()> {
        sqlx::query(
            r#"
            insert into mechanic_dues
              (due_id, session_id, turn_id, source, source_track, hook_event, mechanic_id,
               threshold_desc, followup_procedure_id, owner_kind, owner_id, evidence, status, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
            on conflict (due_id) do nothing
            "#,
        )
        .bind(&due.due_id)
        .bind(&due.session_id)
        .bind(&due.turn_id)
        .bind(due_source_str(&due.source))
        .bind(&due.source_track)
        .bind(&due.hook_event)
        .bind(&due.mechanic_id)
        .bind(&due.threshold_desc)
        .bind(&due.followup_procedure_id)
        .bind(&due.owner_kind)
        .bind(&due.owner_id)
        .bind(&due.evidence)
        .bind(due_status_str(&due.status))
        .bind(due.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_open_mechanic_dues(&self, session_id: &str) -> Result<Vec<MechanicDue>> {
        self.list_mechanic_dues_with_status(session_id, "open")
            .await
    }

    pub async fn list_mechanic_dues_with_status(
        &self,
        session_id: &str,
        status: &str,
    ) -> Result<Vec<MechanicDue>> {
        let rows = sqlx::query(
            r#"
            select due_id, session_id, turn_id, source, source_track, hook_event, mechanic_id,
                   threshold_desc, followup_procedure_id, owner_kind, owner_id, evidence, status, created_at
            from mechanic_dues
            where session_id = $1 and status = $2
            order by created_at asc
            "#,
        )
        .bind(session_id)
        .bind(status)
        .fetch_all(&self.pool)
        .await?;
        // fail-closed：source/status 列认不出的行跳过（不猜不编造）。
        Ok(rows.iter().filter_map(row_to_mechanic_due).collect())
    }

    pub async fn update_mechanic_due_status(
        &self,
        due_id: &str,
        status: &str,
        waive_reason: Option<&str>,
        waive_scope: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            r#"
            update mechanic_dues
            set status = $2, waive_reason = $3, waive_scope = $4, updated_at = now()
            where due_id = $1
            "#,
        )
        .bind(due_id)
        .bind(status)
        .bind(waive_reason)
        .bind(waive_scope)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// B6 场景切换重开（spec §5.3：scene 豁免"场景切换时清除豁免"）：该 session
    /// 内 waive_scope='scene' 且 status='waived' 的行重开为 open，waive 记账列
    /// 清空（审计留在 gm_waive 勘误记忆，不靠行残留）。返回重开行数。
    /// scope='turn' 的行不动（turn 豁免由内存侧 begin_turn 过期，db 行保持审计原样）。
    pub async fn reopen_scene_waived_dues(&self, session_id: &str) -> Result<u64> {
        let result = sqlx::query(
            r#"
            update mechanic_dues
            set status = 'open', waive_reason = null, waive_scope = null, updated_at = now()
            where session_id = $1 and status = 'waived' and waive_scope = 'scene'
            "#,
        )
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    // ------------------------------------------------------------------
    // v1.5 Interaction Lifecycle Kernel persistence helpers
    // ------------------------------------------------------------------

    pub async fn ensure_interaction_generation(&self, session_id: &str) -> Result<i64> {
        sqlx::query("update sessions set interaction_generation = coalesce(interaction_generation, 0) where session_id = $1")
            .bind(session_id)
            .execute(&self.pool)
            .await
            .ok();
        let generation: Option<i64> = sqlx::query_scalar(
            "select coalesce(interaction_generation, 0) from sessions where session_id = $1",
        )
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
        if matches!(
            ctx.status,
            InteractionContextStatus::Active
                | InteractionContextStatus::Paused
                | InteractionContextStatus::Resolving
        ) {
            sqlx::query("update sessions set active_interaction_context_id = $2, updated_at = now() where session_id = $1")
                .bind(&ctx.session_id)
                .bind(&ctx.context_id)
                .execute(&self.pool)
                .await
                .ok();
        }
        Ok(())
    }

    pub async fn insert_invariant_repair(
        &self,
        session_id: &str,
        repair: &InvariantRepair,
    ) -> Result<()> {
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

    pub async fn attach_gate_to_frame(
        &self,
        gate_id: &str,
        frame_id: &str,
        generation: i64,
    ) -> Result<()> {
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

    pub async fn attach_pending_check_to_frame(
        &self,
        check_id: &str,
        frame_id: &str,
        gate_id: Option<&str>,
        generation: i64,
    ) -> Result<()> {
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

    pub async fn supersede_interaction_gate(
        &self,
        gate_id: &str,
        reason: &str,
        closed_at_tick: i64,
    ) -> Result<()> {
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

    pub async fn supersede_pending_check(
        &self,
        check_id: &str,
        reason: &str,
        closed_at_tick: Option<i64>,
    ) -> Result<()> {
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

    pub async fn cascade_close_frame_interactions(
        &self,
        session_id: &str,
        frame_id: &str,
        reason: &str,
        world_tick: i64,
        generation: i64,
    ) -> Result<Vec<InvariantRepair>> {
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
            repairs.push(InvariantRepair {
                repair_id: format!("repair_{}", Uuid::new_v4().simple()),
                kind: InvariantRepairKind::FrameClosedChildrenTerminal,
                target_table: "interaction_gates".into(),
                target_id: frame_id.into(),
                action: "superseded_open_child_gates".into(),
                reason: reason.into(),
                world_tick,
                generation,
            });
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
        .await
        .unwrap_or(0);
        if check_count > 0 {
            repairs.push(InvariantRepair {
                repair_id: format!("repair_{}", Uuid::new_v4().simple()),
                kind: InvariantRepairKind::FrameClosedChildrenTerminal,
                target_table: "pending_checks".into(),
                target_id: frame_id.into(),
                action: "superseded_open_child_pending_checks".into(),
                reason: reason.into(),
                world_tick,
                generation,
            });
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

    pub async fn reconcile_interaction_lifecycle(
        &self,
        session_id: &str,
        world_tick: i64,
        current_generation: i64,
    ) -> Result<Vec<InvariantRepair>> {
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
        )
        .bind(session_id)
        .bind(current_generation)
        .execute(&self.pool)
        .await
        .ok();
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
            repairs.push(InvariantRepair {
                repair_id: format!("repair_{}", Uuid::new_v4().simple()),
                kind: InvariantRepairKind::OpenGateWithoutActiveOwner,
                target_table: "interaction_gates".into(),
                target_id: session_id.into(),
                action: "superseded_stale_or_orphan_open_gates".into(),
                reason: "superseded_by_reconcile".into(),
                world_tick,
                generation: current_generation,
            });
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
            repairs.push(InvariantRepair {
                repair_id: format!("repair_{}", Uuid::new_v4().simple()),
                kind: InvariantRepairKind::OpenPendingCheckWithoutActiveOwner,
                target_table: "pending_checks".into(),
                target_id: session_id.into(),
                action: "superseded_stale_or_orphan_open_pending_checks".into(),
                reason: "superseded_by_reconcile".into(),
                world_tick,
                generation: current_generation,
            });
        }
        sqlx::query("update sessions set active_interaction_gate_id = null where session_id=$1 and active_interaction_gate_id is not null and not exists (select 1 from interaction_gates g where g.session_id=$1 and g.gate_id=active_interaction_gate_id and g.status='open')")
            .bind(session_id)
            .execute(&self.pool)
            .await
            .ok();
        Ok(repairs)
    }

    pub async fn insert_pending_check(&self, pending: &PendingCheck) -> Result<()> {
        let generation = self
            .ensure_interaction_generation(&pending.session_id)
            .await
            .unwrap_or(0);
        let owner_frame_id = pending
            .owner_frame_id
            .clone()
            .or_else(|| pending.contract.actor_snapshot_ids.first().cloned());
        let gate_id = pending
            .gate_id
            .clone()
            .or_else(|| Some(format!("gate_{}", pending.check_id)));
        let interaction_context_id = pending
            .interaction_context_id
            .clone()
            .or_else(|| owner_frame_id.as_ref().map(|f| format!("ctx_{}", f)));
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

    pub async fn cancel_open_pending_checks_for_session(
        &self,
        session_id: &str,
        status: PendingCheckStatus,
    ) -> Result<()> {
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
        let generation = self
            .ensure_interaction_generation(&gate.session_id)
            .await
            .unwrap_or(0);
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

    pub async fn get_open_interaction_gate(
        &self,
        session_id: &str,
    ) -> Result<Option<InteractionGate>> {
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

    pub async fn update_interaction_gate_status(
        &self,
        gate_id: &str,
        status: GateStatus,
        resolution_json: serde_json::Value,
    ) -> Result<()> {
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

    pub async fn update_pending_check_status(
        &self,
        check_id: &str,
        status: PendingCheckStatus,
    ) -> Result<()> {
        sqlx::query(
            r#"update pending_checks set status = $2, updated_at = now() where check_id = $1"#,
        )
        .bind(check_id)
        .bind(status.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_check_contracts_for_turn(
        &self,
        session_id: &str,
        turn_id: &str,
    ) -> Result<Vec<CheckContract>> {
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
    pub async fn get_latest_unresolved_check_contract(
        &self,
        session_id: &str,
    ) -> Result<Option<CheckContract>> {
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

    pub async fn list_active_state_frames(
        &self,
        session_id: &str,
        limit: i64,
    ) -> Result<Vec<StateFrame>> {
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

    /// 优化2 #6：append-only domain_events 写入（确定性幂等——event_id 冲突 do nothing）。
    /// kind 绑为稳定 token（`DomainEventKind::as_str`，与 list 的 parse 单一映射源对齐）；
    /// data/source_refs 经 serde 入 jsonb。
    pub async fn append_domain_event(&self, ev: &trpg_model::DomainEvent) -> Result<()> {
        sqlx::query(
            r#"
            insert into domain_events
              (event_id, session_id, turn_id, kind, data, source_refs, created_at)
            values ($1,$2,$3,$4,$5,$6,$7)
            on conflict (event_id) do nothing
            "#,
        )
        .bind(&ev.event_id)
        .bind(&ev.session_id)
        .bind(&ev.turn_id)
        .bind(ev.kind.as_str())
        .bind(&ev.data)
        .bind(serde_json::to_value(&ev.source_refs)?)
        .bind(ev.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 按会话取 domain events（seq 升序时间线，limit 截断）。
    pub async fn list_domain_events(
        &self,
        session_id: &str,
        limit: i64,
    ) -> Result<Vec<trpg_model::DomainEvent>> {
        let rows = sqlx::query(
            r#"
            select event_id, session_id, turn_id, kind, data, source_refs, created_at
            from domain_events
            where session_id = $1
            order by seq asc
            limit $2
            "#,
        )
        .bind(session_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_domain_event).collect()
    }

    /// 按回合取 domain events（seq 升序；explain / inspect 附该回合摘要用）。
    pub async fn list_domain_events_for_turn(
        &self,
        turn_id: &str,
    ) -> Result<Vec<trpg_model::DomainEvent>> {
        let rows = sqlx::query(
            r#"
            select event_id, session_id, turn_id, kind, data, source_refs, created_at
            from domain_events
            where turn_id = $1
            order by seq asc
            "#,
        )
        .bind(turn_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_domain_event).collect()
    }

    /// 反剧透 TruthGraph：本会话**进入 GM/runtime context** 的实体集（P0c `ContextSurfaced`）。
    /// 这是隐藏的内部装载，**不**代表玩家见过——故与 [`Db::list_surfaced_entities`]（玩家暴露）
    /// 严格区分。从 `ContextSurfaced` 事件的 `data` jsonb 抽 distinct `(entity_id, entity_kind)`，
    /// 按 entity_id 稳定排序。
    pub async fn list_context_surfaced_entities(
        &self,
        session_id: &str,
    ) -> Result<Vec<(String, String)>> {
        let rows = sqlx::query(
            r#"
            select distinct
                   data->>'entity_id'   as entity_id,
                   data->>'entity_kind' as entity_kind
            from domain_events
            where session_id = $1
              and kind = 'ContextSurfaced'
              and data->>'entity_id' is not null
            order by entity_id
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                let id: String = r.get("entity_id");
                let kind: Option<String> = r.get("entity_kind");
                (id, kind.unwrap_or_default())
            })
            .collect())
    }

    /// 反剧透 TruthGraph 投影：本会话**已暴露给玩家**（玩家在可见虚构里真见过）的实体集。
    /// P0c 语义：玩家暴露 = `PlayerExposed` + 遗留 `EntitySurfaced`（向后兼容），**绝不**含
    /// `ContextSurfaced`（那是隐藏 context 装载，进 GM 不等于玩家知道）。从事件 `data` jsonb
    /// 抽 distinct `(entity_id, entity_kind)`；按 entity_id 稳定排序。关系抽取据此只对玩家
    /// 真见过的实体跑，不会把隐藏 context 当成玩家暴露。
    pub async fn list_surfaced_entities(&self, session_id: &str) -> Result<Vec<(String, String)>> {
        let rows = sqlx::query(
            r#"
            select distinct
                   data->>'entity_id'   as entity_id,
                   data->>'entity_kind' as entity_kind
            from domain_events
            where session_id = $1
              and kind in ('PlayerExposed', 'EntitySurfaced')
              and data->>'entity_id' is not null
            order by entity_id
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                let id: String = r.get("entity_id");
                let kind: Option<String> = r.get("entity_kind");
                (id, kind.unwrap_or_default())
            })
            .collect())
    }

    /// 反剧透 TruthGraph：本会话**本回合是否首次把新实体暴露给玩家**。
    ///
    /// P0c 语义：只数玩家暴露事件（`PlayerExposed` + 遗留 `EntitySurfaced`），**不**含
    /// `ContextSurfaced`——隐藏 context 装载不该触发关系抽取（否则把"GM 加载了它"误当
    /// "玩家见过它"）。暴露写入 per-session 幂等（`on conflict do nothing`）：实体首次暴露时
    /// 该行的 `turn_id` **冻结为当时回合**，后续重放 no-op、不改 turn_id。故「某行 turn_id ==
    /// 本回合」当且仅当该实体**本回合才头一回暴露**。关系抽取据此只在玩家暴露集真变化的
    /// 回合才跑（省掉每回合重抽同样三元组的 LLM 调用）。
    pub async fn has_entity_surfaced_in_turn(
        &self,
        session_id: &str,
        turn_id: &str,
    ) -> Result<bool> {
        let row = sqlx::query(
            r#"
            select exists(
                select 1 from domain_events
                where session_id = $1
                  and turn_id = $2
                  and kind in ('PlayerExposed', 'EntitySurfaced')
            ) as found
            "#,
        )
        .bind(session_id)
        .bind(turn_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.get::<bool, _>("found"))
    }

    /// 反剧透 revealed-facts 账本落账（LEDGER 切片）：把一条事实（按 entity_id/node_id
    /// 作 `fact_id`）记为 player_party 已习得/确认。复用 domain_events 表，P0c 起
    /// kind=`PlayerLearnedFact`（取代旧 `FactRevealed` 写路径，旧 variant 保留仅向后兼容
    /// 读取），幂等键 `de_revealed_{session}_{fact}`（同 session+fact 重放 on-conflict no-op）。
    /// 与 ContextSurfaced/PlayerExposed 立场区分：那两者是"实体进 context / 玩家见过实体"，
    /// 这里是"玩家确知某条事实"，并写穿 KnowledgeEdge(player_party, knows_true)。
    pub async fn record_revealed_fact(
        &self,
        session_id: &str,
        turn_id: &str,
        fact_id: &str,
        reason: Option<&str>,
    ) -> Result<()> {
        let data = serde_json::json!({ "fact_id": fact_id, "reason": reason });
        let ev = trpg_model::DomainEvent::new(
            format!("de_revealed_{session_id}_{fact_id}"),
            session_id,
            turn_id,
            trpg_model::DomainEventKind::PlayerLearnedFact,
            data,
        );
        self.append_domain_event(&ev).await?;
        // P0b 写穿：PlayerLearnedFact 同步落 KnowledgeEdge (player_party, knows_true)。
        // append 在同 (session,fact) 重放会 no-op，但 upsert 仍幂等保证边存在，
        // 使 revealed-facts 投影（现读 knowledge_edges）不漏旧事件。
        self.upsert_knowledge_edge_player_party(session_id, turn_id, fact_id, "knows_true", reason)
            .await
    }

    /// P0b KnowledgeEdge upsert：把 (session, player_party, fact) 的知识态落账。
    /// edge_id 在 SQL 内用 md5 确定性生成（uuid crate 无 v5 feature），与 0032 回填
    /// 同方案 → 跨回填/运行时幂等。on-conflict 更新态/来源/理由（coalesce 保留旧非空）。
    pub async fn upsert_knowledge_edge_player_party(
        &self,
        session_id: &str,
        turn_id: &str,
        fact_id: &str,
        knowledge_state: &str,
        reason: Option<&str>,
    ) -> Result<()> {
        // turn_id 当前不入 knowledge_edges 列；source_event_id 复用 record_revealed_fact
        // 的幂等键 de_revealed_{session}_{fact}（与 domain_events.event_id 对齐）。
        let _ = turn_id;
        sqlx::query(
            r#"
            insert into knowledge_edges
              (edge_id, session_id, holder_kind, holder_id, fact_id, knowledge_state, source_event_id, reason)
            values (
              'ke_' || md5($1 || ':player_party:' || $2),
              $1, 'player_party', '', $2, $3, $4, $5
            )
            on conflict (session_id, holder_kind, holder_id, fact_id)
            do update set
              knowledge_state = excluded.knowledge_state,
              source_event_id = coalesce(excluded.source_event_id, knowledge_edges.source_event_id),
              reason = coalesce(excluded.reason, knowledge_edges.reason),
              updated_at = now()
            "#,
        )
        .bind(session_id)
        .bind(fact_id)
        .bind(knowledge_state)
        .bind(format!("de_revealed_{session_id}_{fact_id}"))
        .bind(reason)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 通用 durable KnowledgeEdge upsert（v1）：把任意 durable holder 的一条知识边落账。
    /// **fail-closed 门**：写库前校验 holder_kind 必须是 durable 可持久化态
    /// （gm / player_party / system / npc / pc / faction，见
    /// [`trpg_model::KnowledgeHolderKind::is_durable_persistable`]）。未知 token 直接返回 Err，
    /// 绝不写 knowledge_edges。
    /// **身份门**（TC-KNOW-04/TC-KNOW-00 + 本任务 pc/faction）：holder_kind 为带身份 holder
    /// （npc/pc/faction）时 holder_id 还须经对应 actor-identity 契约构造器
    /// （[`trpg_model::KnowledgeHolder::npc_from_actor_id`] /
    /// [`trpg_model::KnowledgeHolder::player_character_from_actor_id`] /
    /// [`trpg_model::KnowledgeHolder::faction_from_id`]）校验为稳定 id（空/占位/展示名形态
    /// 全部 fail-closed），并以契约规范化（trim）后的 id 落库——kind 通过 ≠ id 合法。
    /// edge_id 由 (session, holder_kind, holder_id, fact) md5 确定性生成；on-conflict 唯一键
    /// 更新态/置信度/来源/理由/披露策略（coalesce 保留旧非空），重放幂等。
    /// 注意：player_party 的兼容写穿路径仍走 [`Db::upsert_knowledge_edge_player_party`]
    /// （edge_id 方案与 0032 回填对齐），本通用接口服务 gm / system / npc / pc / faction holder。
    pub async fn upsert_knowledge_edge(&self, edge: KnowledgeEdgeInput<'_>) -> Result<()> {
        // fail-closed：非 durable holder kind 在写库前拒绝（不发明 holder、不漏写半条）。
        let kind = trpg_model::KnowledgeHolderKind::from_token(edge.holder_kind)
            .filter(|k| k.is_durable_persistable())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "upsert_knowledge_edge: holder_kind {:?} 非 durable 可持久化（gated）",
                    edge.holder_kind
                )
            })?;
        let holder_kind = kind.as_token();
        // 带身份 holder 身份门：校验 holder_id 为稳定 id，并用规范化（trim）后的 id 落库。
        // 集合 holder（gm/player_party/system）维持空串。非法 id → fail-closed，不写任何边。
        use trpg_model::{KnowledgeHolder, KnowledgeHolderKind};
        let resolved_holder_id;
        let holder_id: &str = match kind {
            KnowledgeHolderKind::Npc | KnowledgeHolderKind::Pc | KnowledgeHolderKind::Faction => {
                let holder = match kind {
                    KnowledgeHolderKind::Npc => KnowledgeHolder::npc_from_actor_id(edge.holder_id),
                    KnowledgeHolderKind::Pc => {
                        KnowledgeHolder::player_character_from_actor_id(edge.holder_id)
                    }
                    _ => KnowledgeHolder::faction_from_id(edge.holder_id),
                }
                .map_err(|u| anyhow::anyhow!("upsert_knowledge_edge: {u}"))?;
                resolved_holder_id = holder
                    .holder_id()
                    .expect("带身份 holder 必有 stable id")
                    .to_string();
                resolved_holder_id.as_str()
            }
            _ => edge.holder_id,
        };
        sqlx::query(
            r#"
            insert into knowledge_edges
              (edge_id, session_id, holder_kind, holder_id, fact_id, knowledge_state,
               confidence, learned_at_turn_id, disclosure_policy, source_event_id, reason)
            values (
              'ke_' || md5($1 || ':' || $2 || ':' || $3 || ':' || $4),
              $1, $2, $3, $4, $5, $6, $7, $8, $9, $10
            )
            on conflict (session_id, holder_kind, holder_id, fact_id)
            do update set
              knowledge_state = excluded.knowledge_state,
              confidence = coalesce(excluded.confidence, knowledge_edges.confidence),
              learned_at_turn_id = coalesce(excluded.learned_at_turn_id, knowledge_edges.learned_at_turn_id),
              disclosure_policy = coalesce(excluded.disclosure_policy, knowledge_edges.disclosure_policy),
              source_event_id = coalesce(excluded.source_event_id, knowledge_edges.source_event_id),
              reason = coalesce(excluded.reason, knowledge_edges.reason),
              updated_at = now()
            "#,
        )
        .bind(edge.session_id)
        .bind(holder_kind)
        .bind(holder_id)
        .bind(edge.fact_id)
        .bind(edge.knowledge_state)
        .bind(edge.confidence)
        .bind(edge.learned_at_turn_id)
        .bind(edge.disclosure_policy)
        .bind(edge.source_event_id)
        .bind(edge.reason)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 通用 holder 真知识投影：本会话指定 (holder_kind, holder_id) 的 knows_true distinct
    /// fact_id 集，按 fact_id 稳定排序。只取确知为真态——信念态（believes_*/misinformed）
    /// 一律不入，故任何 holder 的「错误信念」都不会被当成其已知真相泄露。
    pub async fn list_holder_known_fact_ids(
        &self,
        session_id: &str,
        holder_kind: &str,
        holder_id: &str,
    ) -> Result<Vec<String>> {
        let rows = sqlx::query(
            r#"
            select fact_id
            from knowledge_edges
            where session_id = $1
              and holder_kind = $2
              and holder_id = $3
              and knowledge_state = 'knows_true'
            order by fact_id
            "#,
        )
        .bind(session_id)
        .bind(holder_kind)
        .bind(holder_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| r.get::<String, _>("fact_id"))
            .collect())
    }

    /// GM 真相投影：本会话 GM holder 确知为真（knows_true）的 distinct fact_id 集。
    /// = 世界真相视图。某 holder 的 believes_false/misinformed 信念边绝不混入此视图。
    pub async fn gm_truth_view(&self, session_id: &str) -> Result<Vec<String>> {
        self.list_holder_known_fact_ids(session_id, "gm", "").await
    }

    /// 玩家知识投影：本会话 player_party 确知为真的 distinct fact_id 集
    /// （= [`Db::list_player_known_fact_ids`]）。未知/暴露/信念态一律不入——
    /// context 装载的事实不会因此对玩家可见。
    pub async fn player_knowledge_view(&self, session_id: &str) -> Result<Vec<String>> {
        self.list_player_known_fact_ids(session_id).await
    }

    /// P0b 玩家已知事实投影：本会话 (player_party, knows_true) 的 distinct fact_id 集，
    /// 按 fact_id 稳定排序。revealed-facts 兼容投影的统一真相源（list_revealed_facts 委托）。
    pub async fn list_player_known_fact_ids(&self, session_id: &str) -> Result<Vec<String>> {
        self.list_holder_known_fact_ids(session_id, "player_party", "")
            .await
    }

    /// 反剧透 revealed-facts 账本投影（P0b 起委托 KnowledgeEdge）：本会话已揭示的
    /// distinct `fact_id` 集 = (player_party, knows_true) 子集，按 fact_id 稳定排序。
    /// 不再直接扫 domain_events.FactRevealed；写穿保证两者一致。EntitySurfaced 绝不混入
    /// （surfaced ≠ revealed，且根本不写 knowledge_edges）。spoiler_guard 据此放行实体
    /// secret_terms（不在集内 = 未揭示 = 裁剪）。
    pub async fn list_revealed_facts(&self, session_id: &str) -> Result<Vec<String>> {
        self.list_player_known_fact_ids(session_id).await
    }

    /// P0c 玩家暴露写穿（`PlayerExposed` 事件 API 面）：把一个实体记为"玩家在可见虚构里
    /// 见过/听说过"——计入玩家暴露投影（[`Db::list_surfaced_entities`]），但**不**揭示其
    /// 隐藏身份/秘密事实（不写 knowledge_edges，故不进 revealed-facts）。复用 domain_events
    /// 表，幂等键 `de_exposed_{session}_{entity}`（同 session+entity 重放 on-conflict no-op）。
    /// 与 `ContextSurfaced`（隐藏 context 装载）严格区分。
    pub async fn record_player_exposed_entity(
        &self,
        session_id: &str,
        turn_id: &str,
        entity_id: &str,
        entity_kind: &str,
        reason: Option<&str>,
    ) -> Result<()> {
        let data = serde_json::json!({
            "entity_id": entity_id,
            "entity_kind": entity_kind,
            "reason": reason,
        });
        let ev = trpg_model::DomainEvent::new(
            format!("de_exposed_{session_id}_{entity_id}"),
            session_id,
            turn_id,
            trpg_model::DomainEventKind::PlayerExposed,
            data,
        );
        self.append_domain_event(&ev).await
    }

    /// NPC 习得事实写穿（`NpcLearnedFact` 事件 API 面）：把一条事实记为**某个具体
    /// NPC**（stable `npc_actor_id`）已习得——这是 NPC 心智的知识输入，只作用于目标 NPC，
    /// **绝不**触碰 player_party 或其它 holder。
    ///
    /// **durable NPC holder（TC-KNOW-04）**：稳定 NPC 身份解析成功后，除落 `NpcLearnedFact`
    /// 事件账本外，还写穿 durable `knowledge_edges(holder_kind='npc', knows_true)`——NpcLearnedFact
    /// 不再 event-only。事件账本始终保留（即使将来 durable 路径放宽/收紧），两者经确定性
    /// 幂等键对齐。`npc_actor_id` 经 TC-KNOW-00 actor-identity 契约校验：空串 / 占位串 /
    /// 展示名/对抗方标签形态全部 fail-closed 拒绝（绝不拿 LLM 文本冒充 stable actor id），
    /// 此时既不落事件也不写边。
    /// 幂等键 `de_npc_learned_{session}_{npc}_{fact}`（事件 on-conflict no-op；durable 边唯一键
    /// (session,'npc',npc,fact) on-conflict 更新）→ 重放既不重复事件也不重复边。
    pub async fn record_npc_learned_fact(
        &self,
        session_id: &str,
        turn_id: &str,
        npc_actor_id: &str,
        fact_id: &str,
        reason: Option<&str>,
    ) -> Result<()> {
        // fail-closed：经 TC-KNOW-00 actor-identity 契约校验稳定 NPC holder 身份。
        // 空串 / 占位串 / 展示名/对抗方标签形态全部被拒，绝不拿 LLM 文本冒充 stable actor id。
        let holder = trpg_model::KnowledgeHolder::npc_from_actor_id(npc_actor_id)
            .map_err(|u| anyhow::anyhow!("record_npc_learned_fact: {u}"))?;
        // 用契约规范化（trim 后）的稳定 id 作事件主键/数据/holder_id，跨 raw 空白幂等。
        let npc_actor_id = holder.holder_id().expect("npc holder 必有 stable id");
        let data = serde_json::json!({
            "npc_actor_id": npc_actor_id,
            "fact_id": fact_id,
            "reason": reason,
        });
        let ev = trpg_model::DomainEvent::new(
            format!("de_npc_learned_{session_id}_{npc_actor_id}_{fact_id}"),
            session_id,
            turn_id,
            trpg_model::DomainEventKind::NpcLearnedFact,
            data,
        );
        // 先落事件账本（语义真相源之一），再写穿 durable NPC 边（TC-KNOW-04）。
        self.append_domain_event(&ev).await?;
        self.upsert_knowledge_edge(KnowledgeEdgeInput {
            session_id,
            holder_kind: "npc",
            holder_id: npc_actor_id,
            fact_id,
            knowledge_state: "knows_true",
            confidence: None,
            learned_at_turn_id: Some(turn_id),
            disclosure_policy: None,
            source_event_id: Some(&ev.event_id),
            reason,
        })
        .await
    }

    /// NPC 已知事实投影：本会话**指定 NPC**（`npc_actor_id`）确知为真（knows_true）的 distinct
    /// `fact_id` 集，从 durable `knowledge_edges`（holder_kind='npc'）投影（TC-KNOW-04）。
    /// 按 (holder_kind, holder_id) 严格过滤 → 只返回目标 NPC 的事实，绝不混入别的 NPC、
    /// player_party 或 gm。信念态（believes_*/misinformed）一律不入：NPC 的错误信念不算其已知真相，
    /// 更不是世界真相。按 fact_id 稳定排序（委托 [`Db::list_holder_known_fact_ids`]）。
    pub async fn list_npc_known_fact_ids(
        &self,
        session_id: &str,
        npc_actor_id: &str,
    ) -> Result<Vec<String>> {
        self.list_holder_known_fact_ids(session_id, "npc", npc_actor_id)
            .await
    }

    /// NPC 心智事实投影（TC-NPC-03）：本会话**指定 NPC** 自己持有的 `(fact_id, knowledge_state)`
    /// 条目，仅取 `knows_true` 与三种信念态（`believes_true`/`believes_false`/`misinformed`）。
    /// 与 [`Db::list_npc_known_fact_ids`] 的区别：后者只投影确知为真（供真知识/世界真相一致性
    /// 判定），本接口额外带回信念态，供 NPC 心智视图把「确知」与「相信（可能为假）」分别标注——
    /// 信念态绝不被升格为已知真相。严格按 (holder_kind='npc', holder_id=<目标 NPC>) 过滤：
    /// gm / player_party / 别的 NPC 的边一律不入。其余态（unknown/heard_about/suspects/…）排除。
    /// `npc_actor_id` 经 actor-identity 契约规范化后查询；未知 state token fail-closed 跳过该行
    /// （绝不把未知态误当某已知态）。按 fact_id 稳定排序。
    pub async fn list_npc_mind_facts(
        &self,
        session_id: &str,
        npc_actor_id: &str,
    ) -> Result<Vec<NpcKnowledgeEntry>> {
        let holder = trpg_model::KnowledgeHolder::npc_from_actor_id(npc_actor_id)
            .map_err(|u| anyhow::anyhow!("list_npc_mind_facts: {u}"))?;
        let npc_id = holder.holder_id().expect("npc holder 必有 stable id");
        let rows = sqlx::query(
            r#"
            select fact_id, knowledge_state
            from knowledge_edges
            where session_id = $1
              and holder_kind = 'npc'
              and holder_id = $2
              and knowledge_state in ('knows_true', 'believes_true', 'believes_false', 'misinformed')
            order by fact_id
            "#,
        )
        .bind(session_id)
        .bind(npc_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let fact_id = r.get::<String, _>("fact_id");
                let token = r.get::<String, _>("knowledge_state");
                // fail-closed：未知 state token 跳过该行，绝不误当已知态。
                KnowledgeState::from_token(&token).map(|state| NpcKnowledgeEntry { fact_id, state })
            })
            .collect())
    }

    /// 写穿 durable NPC relationship（TC-NPC-02）：把一个 NPC 对玩家方/PC/NPC/阵营的
    /// 结构化态度状态持久化，不依赖 free-form memory summary。fail-closed：`npc_id`
    /// 经 TC-KNOW-00 actor-identity 契约校验为稳定 actor id（空/占位/展示名形态全部被拒），
    /// 否则不写任何行。数值通道由模型层在每次 delta 时夹紧到边界（迁移 CHECK 为兜底）。
    /// 唯一键 (session, npc_id, target_kind, target_id) on-conflict 更新各通道与派生态、
    /// 证据集与 updated_at，重写幂等。
    pub async fn upsert_npc_relationship(&self, rel: &NpcRelationship) -> Result<()> {
        // 身份门：稳定 NPC holder 校验（kind 通过 ≠ id 合法）。规范化（trim）后落库。
        let holder = trpg_model::KnowledgeHolder::npc_from_actor_id(&rel.npc_id)
            .map_err(|u| anyhow::anyhow!("upsert_npc_relationship: {u}"))?;
        let npc_id = holder.holder_id().expect("npc holder 必有 stable id");
        // target 身份门：NpcRelationship 字段/枚举 variant 全 public 且可由 serde 构造，
        // 故写边界必须重新校验+规范化 target id（pc/npc/faction 经 TC-KNOW-00 kind-specific
        // 契约；非法 target id → fail-closed，不写任何行）。用规范化后的 kind/id 落库。
        let target = rel
            .target
            .validated()
            .map_err(|e| anyhow::anyhow!("upsert_npc_relationship target: {e}"))?;
        let evidence = serde_json::to_value(&rel.evidence_event_ids)?;
        let stance = serde_json::to_value(rel.stance)?
            .as_str()
            .expect("stance serializes to a string token")
            .to_string();
        sqlx::query(
            r#"
            insert into npc_relationships
              (session_id, npc_id, target_kind, target_id, trust, respect, affection, debt,
               fear, suspicion, hostility, leverage, talkativeness, interaction_desire, stance,
               last_interaction_turn_id, evidence_event_ids)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)
            on conflict (session_id, npc_id, target_kind, target_id)
            do update set
              trust = excluded.trust,
              respect = excluded.respect,
              affection = excluded.affection,
              debt = excluded.debt,
              fear = excluded.fear,
              suspicion = excluded.suspicion,
              hostility = excluded.hostility,
              leverage = excluded.leverage,
              talkativeness = excluded.talkativeness,
              interaction_desire = excluded.interaction_desire,
              stance = excluded.stance,
              last_interaction_turn_id = excluded.last_interaction_turn_id,
              evidence_event_ids = excluded.evidence_event_ids,
              updated_at = now()
            "#,
        )
        .bind(&rel.session_id)
        .bind(npc_id)
        .bind(target.kind_token())
        .bind(target.target_id())
        .bind(rel.trust)
        .bind(rel.respect)
        .bind(rel.affection)
        .bind(rel.debt)
        .bind(rel.fear)
        .bind(rel.suspicion)
        .bind(rel.hostility)
        .bind(rel.leverage)
        .bind(rel.talkativeness)
        .bind(rel.interaction_desire)
        .bind(&stance)
        .bind(rel.last_interaction_turn_id.as_deref())
        .bind(&evidence)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 读回 durable NPC relationship（TC-NPC-02）。无行返回 None（首次互动）。
    /// `npc_id` 同样经 actor-identity 契约规范化后查询，跨 raw 空白稳定命中。
    pub async fn load_npc_relationship(
        &self,
        session_id: &str,
        npc_id: &str,
        target_kind: &str,
        target_id: &str,
    ) -> Result<Option<NpcRelationship>> {
        let holder = trpg_model::KnowledgeHolder::npc_from_actor_id(npc_id)
            .map_err(|u| anyhow::anyhow!("load_npc_relationship: {u}"))?;
        let npc_id = holder.holder_id().expect("npc holder 必有 stable id");
        let row = sqlx::query(
            r#"
            select target_kind, target_id, trust, respect, affection, debt, fear, suspicion,
                   hostility, leverage, talkativeness, interaction_desire, stance,
                   last_interaction_turn_id, evidence_event_ids
            from npc_relationships
            where session_id = $1 and npc_id = $2 and target_kind = $3 and target_id = $4
            "#,
        )
        .bind(session_id)
        .bind(npc_id)
        .bind(target_kind)
        .bind(target_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(r) = row else { return Ok(None) };
        let target = NpcRelationshipTarget::from_parts(
            &r.get::<String, _>("target_kind"),
            &r.get::<String, _>("target_id"),
        )
        .map_err(|e| anyhow::anyhow!("load_npc_relationship: {e}"))?;
        let stance: RelationshipStance =
            serde_json::from_value(serde_json::Value::String(r.get::<String, _>("stance")))?;
        let evidence_event_ids: Vec<String> =
            serde_json::from_value(r.get::<serde_json::Value, _>("evidence_event_ids"))?;
        Ok(Some(NpcRelationship {
            session_id: session_id.to_string(),
            npc_id: npc_id.to_string(),
            target,
            trust: r.get("trust"),
            respect: r.get("respect"),
            affection: r.get("affection"),
            debt: r.get("debt"),
            fear: r.get("fear"),
            suspicion: r.get("suspicion"),
            hostility: r.get("hostility"),
            leverage: r.get("leverage"),
            talkativeness: r.get("talkativeness"),
            interaction_desire: r.get("interaction_desire"),
            stance,
            last_interaction_turn_id: r.get("last_interaction_turn_id"),
            evidence_event_ids,
        }))
    }

    /// 写穿 durable NPC profile（TC-D3-01）：把一个静态、源生 NPC persona
    /// （[`trpg_model::NpcProfile`]）整体持久化，供后续 mind/behavior production wiring
    /// 复用，而非只在测试里构造瞬时结构体。fail-closed：`profile.actor_id` 经 TC-KNOW-00
    /// actor-identity 契约校验为稳定 actor id（空/占位/展示名形态全部被拒），否则不写任何行。
    /// 完整 profile（含 GM-only secrets）落进 profile_json —— durable 存储是 GM 的真相源；
    /// 仅 safe view 可进入 prompt-facing 适配层。name/role 为冗余索引列。唯一键
    /// (session_id, actor_id) on-conflict 更新 profile_json/name/role 与 updated_at，重写幂等。
    pub async fn upsert_npc_profile(&self, session_id: &str, profile: &NpcProfile) -> Result<()> {
        // 身份门：稳定 NPC holder 校验（kind 通过 ≠ id 合法）。规范化（trim）后落库。
        let holder = trpg_model::KnowledgeHolder::npc_from_actor_id(&profile.actor_id)
            .map_err(|u| anyhow::anyhow!("upsert_npc_profile: {u}"))?;
        let actor_id = holder.holder_id().expect("npc holder 必有 stable id");
        // 完整 profile JSON（含 GM-only secrets）是 durable 真相源。
        let profile_json = serde_json::to_value(profile)?;
        sqlx::query(
            r#"
            insert into npc_profiles (session_id, actor_id, name, role, profile_json)
            values ($1,$2,$3,$4,$5)
            on conflict (session_id, actor_id)
            do update set
              name = excluded.name,
              role = excluded.role,
              profile_json = excluded.profile_json,
              updated_at = now()
            "#,
        )
        .bind(session_id)
        .bind(actor_id)
        .bind(&profile.name)
        .bind(profile.role.as_deref())
        .bind(&profile_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 读回 durable NPC profile（TC-D3-01）。无行返回 None（profile 未持久化）。
    /// `actor_id` 同样经 actor-identity 契约规范化后查询，跨 raw 空白稳定命中。
    /// fail-closed：若反序列化出的 `profile.actor_id` 与请求/行的稳定 actor id 不一致，
    /// 返回 Err 而非 profile（防止串行身份漂移）。
    pub async fn load_npc_profile(
        &self,
        session_id: &str,
        actor_id: &str,
    ) -> Result<Option<NpcProfile>> {
        let holder = trpg_model::KnowledgeHolder::npc_from_actor_id(actor_id)
            .map_err(|u| anyhow::anyhow!("load_npc_profile: {u}"))?;
        let actor_id = holder.holder_id().expect("npc holder 必有 stable id");
        let row = sqlx::query(
            r#"
            select profile_json
            from npc_profiles
            where session_id = $1 and actor_id = $2
            "#,
        )
        .bind(session_id)
        .bind(actor_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(r) = row else { return Ok(None) };
        let profile: NpcProfile =
            serde_json::from_value(r.get::<serde_json::Value, _>("profile_json"))?;
        // 身份一致性门：反序列化出的 profile 必须挂在被请求/落库的同一稳定 actor id 上。
        // 校验 profile.actor_id 自身可解析为稳定 holder，且其规范化值与行 actor_id 相等。
        let profile_holder = trpg_model::KnowledgeHolder::npc_from_actor_id(&profile.actor_id)
            .map_err(|u| anyhow::anyhow!("load_npc_profile: stored profile actor_id: {u}"))?;
        let profile_actor_id = profile_holder
            .holder_id()
            .expect("npc holder 必有 stable id");
        if profile_actor_id != actor_id {
            return Err(anyhow::anyhow!(
                "load_npc_profile: stored profile actor_id {:?} does not match row actor_id {:?}",
                profile_actor_id,
                actor_id
            ));
        }
        Ok(Some(profile))
    }

    pub async fn list_world_events_since(
        &self,
        session_id: &str,
        since_tick: i64,
        since_event_seq: i64,
        limit: i64,
    ) -> Result<Vec<WorldEvent>> {
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

    pub async fn insert_world_time_advance(
        &self,
        from: &WorldTimeState,
        to: &WorldTimeState,
        event: &WorldEvent,
    ) -> Result<()> {
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

    pub async fn list_due_scheduled_events(
        &self,
        session_id: &str,
        tick: i64,
    ) -> Result<Vec<ScheduledEvent>> {
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

    pub async fn update_scheduled_event_status(
        &self,
        scheduled_event_id: &str,
        status: ScheduledEventStatus,
    ) -> Result<()> {
        sqlx::query("update scheduled_events set status = $2 where scheduled_event_id = $1")
            .bind(scheduled_event_id)
            .bind(status.as_str())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn get_context_watermark(
        &self,
        session_id: &str,
    ) -> Result<Option<ContextWatermark>> {
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

    pub async fn insert_actionable_situation_brief(
        &self,
        brief: &ActionableSituationBrief,
    ) -> Result<()> {
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

    pub async fn upsert_player_facing_clue_board(
        &self,
        board: &PlayerFacingClueBoard,
    ) -> Result<()> {
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

    pub async fn insert_clock_tick(
        &self,
        session_id: &str,
        turn_id: &str,
        tick: &ClockTick,
    ) -> Result<()> {
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

    pub async fn upsert_spotlight_state(
        &self,
        session_id: &str,
        state: &SpotlightState,
    ) -> Result<()> {
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

    /// Prior spotlight states for a session — the persistence surface the director's
    /// spotlight tracker reads before a turn to carry + increment per-player counts.
    /// fail-soft: malformed `state_json` rows are skipped rather than failing the read.
    pub async fn load_spotlight_states(&self, session_id: &str) -> Result<Vec<SpotlightState>> {
        let rows = sqlx::query("select state_json from spotlight_states where session_id = $1")
            .bind(session_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                serde_json::from_value(r.get::<serde_json::Value, _>("state_json")).ok()
            })
            .collect())
    }

    pub async fn insert_effect_contract(
        &self,
        effect: &EffectContract,
        session_id: &str,
        turn_id: &str,
    ) -> Result<()> {
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

    pub async fn list_effect_contracts_for_turn(
        &self,
        session_id: &str,
        turn_id: &str,
    ) -> Result<Vec<EffectContract>> {
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
        // eventlog 写穿（additive / fail-soft）：主 insert 成功后追加一条 DiceRolled
        // 领域事件，幂等键 de_roll_{roll_id}。append 失败只 warn，绝不改变本函数成败。
        let ev = trpg_model::DomainEvent::new(
            format!("de_roll_{}", roll.roll_id),
            roll.session_id.clone(),
            roll.turn_id.clone(),
            trpg_model::DomainEventKind::DiceRolled,
            json!({
                "check_id": roll.check_id,
                "roller_id": roll.roller_id,
                "expression": roll.expression,
                "visibility": roll.visibility.as_str(),
            }),
        );
        if let Err(e) = self.append_domain_event(&ev).await {
            tracing::warn!(error=%e, "append DiceRolled domain event failed (non-fatal)");
        }
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
        // eventlog 写穿（additive / fail-soft）：主 insert 成功后追加一条 CheckResolved
        // 领域事件。session/turn 取自内嵌的 result.roll；幂等键 de_check_{check_id}。
        let ev = trpg_model::DomainEvent::new(
            format!("de_check_{}", result.check_id),
            result.roll.session_id.clone(),
            result.roll.turn_id.clone(),
            trpg_model::DomainEventKind::CheckResolved,
            json!({
                "check_id": result.check_id,
                "outcome": result.outcome,
            }),
        );
        if let Err(e) = self.append_domain_event(&ev).await {
            tracing::warn!(error=%e, "append CheckResolved domain event failed (non-fatal)");
        }
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

    pub async fn insert_player_value_verification(
        &self,
        verification: &PlayerValueVerification,
    ) -> Result<()> {
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

    pub async fn insert_table_override_agreement(
        &self,
        agreement: &TableOverrideAgreement,
    ) -> Result<()> {
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

    pub async fn list_recent_player_value_verifications(
        &self,
        session_id: &str,
        limit: i64,
    ) -> Result<Vec<PlayerValueVerification>> {
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
        Ok(rows
            .into_iter()
            .map(|row| PlayerValueVerification {
                verification_id: row.get("verification_id"),
                claim_id: row.get("claim_id"),
                session_id: row.get("session_id"),
                turn_id: row.get("turn_id"),
                status: player_value_status_from_str(&row.get::<String, _>("status")),
                canonical_value_json: row.get("canonical_value_json"),
                acceptable_range_json: row.get("acceptable_range_json"),
                comparison_json: row.get("comparison_json"),
                source_refs: serde_json::from_value(row.get::<serde_json::Value, _>("source_refs"))
                    .unwrap_or_default(),
                warning_public: row.get("warning_public"),
                suggestion_public: row.get("suggestion_public"),
                accepted_if_player_insists: row.get("accepted_if_player_insists"),
                balance_risk: row.get("balance_risk"),
                verifier_json: row.get("verifier_json"),
                world_tick: row.get("world_tick"),
                created_at: row.get("created_at"),
            })
            .collect())
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
    let allowed_options: Vec<ActionOption> =
        serde_json::from_value(row.get::<serde_json::Value, _>("allowed_options"))
            .unwrap_or_default();
    let expected_input: ExpectedInput =
        serde_json::from_value(row.get::<serde_json::Value, _>("expected_input"))
            .unwrap_or_default();
    let source_refs: Vec<SourceRef> =
        serde_json::from_value(row.get::<serde_json::Value, _>("source_refs")).unwrap_or_default();
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
        "downtime" => FrameKind::Downtime,
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
        scope: Scope {
            scope_type: scope_type_from_str(&row.get::<String, _>("scope_type")),
            scope_id: row.get("scope_id"),
        },
        status,
        title: row.get("title"),
        objective: row.get("objective"),
        static_refs: row.get("static_refs"),
        working_state: row.get("working_state"),
        active_gate_ids: row.get("active_gate_ids"),
        local_clocks: serde_json::from_value(row.get::<serde_json::Value, _>("local_clocks"))
            .unwrap_or_default(),
        local_facts: serde_json::from_value(row.get::<serde_json::Value, _>("local_facts"))
            .unwrap_or_default(),
        local_modifiers: serde_json::from_value(row.get::<serde_json::Value, _>("local_modifiers"))
            .unwrap_or_default(),
        event_count: row.get::<i32, _>("event_count") as u32,
        last_event_ids: row.get("last_event_ids"),
        retention_policy: serde_json::from_value(
            row.get::<serde_json::Value, _>("retention_policy"),
        )
        .unwrap_or_default(),
        compaction_policy: serde_json::from_value(
            row.get::<serde_json::Value, _>("compaction_policy"),
        )
        .unwrap_or_default(),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn row_to_pending_check(row: sqlx::postgres::PgRow) -> Result<PendingCheck> {
    let contract_json: serde_json::Value = row.get("contract_json");
    let contract: CheckContract =
        serde_json::from_value(contract_json).unwrap_or_else(|_| CheckContract {
            check_id: row.get("check_id"),
            session_id: row.get("session_id"),
            turn_id: "unknown".into(),
            ruleset_id: "unknown".into(),
            module_id: None,
            initiator: ActorRef {
                actor_id: "unknown".into(),
                actor_kind: ActorKind::PlayerCharacter,
                display_name: None,
            },
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

fn locator_entry_from_material(
    owner_id: &str,
    owner_kind: &str,
    entry: &MaterialIndexEntry,
) -> BookLocatorEntry {
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
        category: entry
            .tags
            .iter()
            .find(|t| t.as_str() != "locator" && t.as_str() != "cold_data")
            .cloned()
            .unwrap_or_else(|| "section".to_string()),
        source_document_id: first_ref.source_id.clone(),
        page_start: page,
        page_end: page,
        heading_path: first_ref.section_path.clone(),
        search_terms,
        summary: entry.summary.clone(),
        confidence: 0.65,
        parse_policy: if matches!(entry.material_type, MaterialType::ColdDataLocator) {
            "on_demand".to_string()
        } else {
            "located".to_string()
        },
        tags: entry.tags.clone(),
        source_refs: entry.source_refs.clone(),
    }
}

fn split_sql_statements(sql: &str) -> Vec<&str> {
    sql.split(";\n").collect()
}

fn due_source_str(s: &DueSource) -> &'static str {
    match s {
        DueSource::Threshold => "threshold",
        DueSource::Hook => "hook",
        DueSource::SemanticTrigger => "semantic_trigger",
    }
}

fn due_status_str(s: &DueStatus) -> &'static str {
    match s {
        DueStatus::Open => "open",
        DueStatus::Resolved => "resolved",
        DueStatus::Waived => "waived",
    }
}

/// 行→MechanicDue；source/status 列值认不出 → None（fail-closed 跳过该行）。
fn row_to_mechanic_due(row: &sqlx::postgres::PgRow) -> Option<MechanicDue> {
    let source = match row.get::<String, _>("source").as_str() {
        "threshold" => DueSource::Threshold,
        "hook" => DueSource::Hook,
        "semantic_trigger" => DueSource::SemanticTrigger,
        _ => return None,
    };
    let status = match row.get::<String, _>("status").as_str() {
        "open" => DueStatus::Open,
        "resolved" => DueStatus::Resolved,
        "waived" => DueStatus::Waived,
        _ => return None,
    };
    Some(MechanicDue {
        due_id: row.get("due_id"),
        session_id: row.get("session_id"),
        turn_id: row.get("turn_id"),
        source,
        source_track: row.get("source_track"),
        hook_event: row.get("hook_event"),
        mechanic_id: row.get("mechanic_id"),
        threshold_desc: row.get("threshold_desc"),
        followup_procedure_id: row.get("followup_procedure_id"),
        owner_kind: row.get("owner_kind"),
        owner_id: row.get("owner_id"),
        evidence: row.get("evidence"),
        status,
        created_at: row.get("created_at"),
    })
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
        scope: Scope {
            scope_type,
            scope_id: row.get("scope_id"),
        },
        priority: row.get("priority"),
        version: row.get::<i32, _>("version") as u32,
        tags: row.get::<Vec<String>, _>("tags"),
        source_refs,
        dependencies,
        content_hash: row.get("content_hash"),
        token_estimate: row
            .get::<Option<i32>, _>("token_estimate")
            .map(|v| v as u32),
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
        scope: Scope {
            scope_type: scope_type_from_str(&row.get::<String, _>("scope_type")),
            scope_id: row.get("scope_id"),
        },
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
        turn_id: row.get::<Option<String>, _>("turn_id"),
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
        scope: Scope {
            scope_type: scope_type_from_str(&row.get::<String, _>("scope_type")),
            scope_id: row.get("scope_id"),
        },
        visibility: visibility_from_str(&row.get::<String, _>("visibility")),
        title: row.get("title"),
        summary_markdown: row.get("summary_markdown"),
        included_event_ids: row.get::<Vec<String>, _>("included_event_ids"),
        included_fact_ids: row.get::<Vec<String>, _>("included_fact_ids"),
        version: row.get::<i32, _>("version") as u32,
        token_estimate: row
            .get::<Option<i32>, _>("token_estimate")
            .map(|v| v as u32),
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
                if i > 0 {
                    out.push('_');
                }
                for c in ch.to_lowercase() {
                    out.push(c);
                }
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

fn row_to_domain_event(row: sqlx::postgres::PgRow) -> Result<trpg_model::DomainEvent> {
    let source_refs_value: serde_json::Value = row.get("source_refs");
    // turn_id 列 nullable；模型字段是 String（serde default 空）——NULL 归一为空串。
    let turn_id: Option<String> = row.get("turn_id");
    Ok(trpg_model::DomainEvent {
        event_id: row.get("event_id"),
        session_id: row.get("session_id"),
        turn_id: turn_id.unwrap_or_default(),
        kind: trpg_model::DomainEventKind::from_str_token(&row.get::<String, _>("kind")),
        data: row.get("data"),
        source_refs: serde_json::from_value(source_refs_value).unwrap_or_default(),
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
    let safe: String = ruleset_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let dir = std::env::var("TRPG_DATA_DIR").unwrap_or_else(|_| "data".into());
    let p = std::path::Path::new(&dir)
        .join("parsed")
        .join("rules")
        .join(format!("{safe}.rule_kernel.override.json"));
    // Local data/ file wins (unchanged path). When it is absent/unreadable, fall
    // back to the binary-embedded copy so a clean checkout (no data/) reproduces
    // the migrated values byte-for-byte.
    if let Ok(text) = std::fs::read_to_string(&p) {
        return serde_json::from_str(&text).ok();
    }
    let embedded = embedded_kernel_override(ruleset_id)?;
    serde_json::from_str(embedded).ok()
}

/// P0-2: binary-embedded kernel overrides (clean-checkout fallback). The 6
/// canonical rulesets are matched by exact id; every arm is an `include_str!`
/// of the git-tracked `embedded_config/rules/` copy (byte-identical to data/).
fn embedded_kernel_override(ruleset_id: &str) -> Option<&'static str> {
    Some(match ruleset_id {
        "brp_orc" => include_str!("../embedded_config/rules/brp_orc.rule_kernel.override.json"),
        "call_of_cthulhu_7e" => {
            include_str!("../embedded_config/rules/call_of_cthulhu_7e.rule_kernel.override.json")
        }
        "cyberpunk_red" => {
            include_str!("../embedded_config/rules/cyberpunk_red.rule_kernel.override.json")
        }
        "dnd5e" => include_str!("../embedded_config/rules/dnd5e.rule_kernel.override.json"),
        "sword_world_2_5" => {
            include_str!("../embedded_config/rules/sword_world_2_5.rule_kernel.override.json")
        }
        "triangle_agency" => {
            include_str!("../embedded_config/rules/triangle_agency.rule_kernel.override.json")
        }
        _ => return None,
    })
}

/// P0-2: layer the typed strategy policy keys from a kernel override doc onto the
/// kernel (override wins wholesale). Each key is deserialized into its typed
/// field; a missing or malformed key is skipped (fail-closed → field stays None
/// → engine uses the GENERIC_* default). Covers combat_profile / combat_mode_policy
/// / check_label_policy / dice_qualification / referee_value_bands.
fn apply_kernel_strategy_overrides(kernel: &mut RuleKernel, doc: &serde_json::Value) {
    if let Some(v) = doc.get("combat_profile") {
        if let Ok(p) = serde_json::from_value::<trpg_model::CombatProfile>(v.clone()) {
            kernel.combat_profile = Some(p);
        }
    }
    if let Some(v) = doc.get("combat_mode_policy") {
        if let Ok(p) = serde_json::from_value::<trpg_model::CombatModePolicy>(v.clone()) {
            kernel.combat_mode_policy = Some(p);
        }
    }
    if let Some(v) = doc.get("check_label_policy") {
        if let Ok(p) = serde_json::from_value::<trpg_model::CheckLabelPolicy>(v.clone()) {
            kernel.check_label_policy = Some(p);
        }
    }
    if let Some(v) = doc.get("dice_qualification") {
        if let Ok(p) = serde_json::from_value::<trpg_model::DiceQualification>(v.clone()) {
            kernel.dice_qualification = Some(p);
        }
    }
    if let Some(v) = doc.get("referee_value_bands") {
        if let Ok(p) = serde_json::from_value::<trpg_model::RefereeValueBands>(v.clone()) {
            kernel.referee_value_bands = Some(p);
        }
    }
    if let Some(v) = doc.get("search_profile") {
        if let Ok(p) = serde_json::from_value::<trpg_model::RuleKernelSearchProfile>(v.clone()) {
            kernel.search_profile = Some(p);
        }
    }
    // P0 dehardcode: layer source-backed firearm profiles (additive — appended to
    // any base list). Migrates trpg-object's hardcoded CoC `.45 Automatic` branch
    // into data. A malformed value is ignored (fail-closed → no extra profiles).
    if let Some(v) = doc.get("firearm_profiles") {
        if let Ok(mut p) = serde_json::from_value::<Vec<trpg_model::FirearmProfile>>(v.clone()) {
            kernel.firearm_profiles.append(&mut p);
        }
    }
}

/// P0-2: read a module config file `{TRPG_DATA_DIR}/modules/{id}.module_config.json`.
/// Mirrors `read_kernel_override_file`. None when absent/unreadable/malformed.
fn read_module_config_file(module_id: &str) -> Option<ModuleConfig> {
    let safe: String = module_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let dir = std::env::var("TRPG_DATA_DIR").unwrap_or_else(|_| "data".into());
    let p = std::path::Path::new(&dir)
        .join("modules")
        .join(format!("{safe}.module_config.json"));
    // Local data/ file wins; else the binary-embedded copy (clean-checkout).
    if let Ok(text) = std::fs::read_to_string(&p) {
        return serde_json::from_str(&text).ok();
    }
    let embedded = embedded_module_config(module_id)?;
    serde_json::from_str(embedded).ok()
}

/// P0-2: binary-embedded module configs (clean-checkout fallback). The 3
/// shipped modules matched by exact id via `include_str!` of the git-tracked
/// `embedded_config/modules/` copy (byte-identical to data/).
fn embedded_module_config(module_id: &str) -> Option<&'static str> {
    Some(match module_id {
        "call_of_cthulhu_7e.masks_of_nyarlathotep" => include_str!("../embedded_config/modules/call_of_cthulhu_7e.masks_of_nyarlathotep.module_config.json"),
        "cyberpunk_red.homecoming" => include_str!("../embedded_config/modules/cyberpunk_red.homecoming.module_config.json"),
        "triangle_agency.the_vault" => include_str!("../embedded_config/modules/triangle_agency.the_vault.module_config.json"),
        _ => return None,
    })
}

/// 合并 director 引导事实:**override(sidecar) > extracted**。`sidecar.director` 存在则整块胜出;
/// 否则用 module reader 自动抽取的 `extracted`。仅 director 字段享 extracted 兜底 ——
/// ModuleConfig 其它字段(npc 绑定 / tech 表 / 别名 / 搜索画像)仍只来自 sidecar。
/// 三态:有 sidecar → 填/留 director 后返回;无 sidecar 有 extracted → 仅包 director;都无 → None。
fn merge_module_config(
    sidecar: Option<ModuleConfig>,
    extracted: Option<DirectorModuleConfig>,
) -> Option<ModuleConfig> {
    match (sidecar, extracted) {
        (Some(mut cfg), ext) => {
            if cfg.director.is_none() {
                cfg.director = ext; // override 胜:仅当无 sidecar.director 才用 extracted
            }
            Some(cfg)
        }
        (None, Some(ext)) => Some(ModuleConfig {
            director: Some(ext),
            ..Default::default()
        }),
        (None, None) => None,
    }
}

/// Shallow-merge an override `dice_core` object onto the base (override keys win).
/// Container values like `success_bands` (an array whose ids may repeat, e.g. two
/// fumble bands) are REPLACED wholesale, not deep-merged — the override supplies the
/// full corrected value for any key it sets. A non-object base is replaced entirely.
fn merge_dice_core(
    base: serde_json::Value,
    over: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    let mut obj = base.as_object().cloned().unwrap_or_default();
    for (k, v) in over {
        obj.insert(k.clone(), v.clone());
    }
    serde_json::Value::Object(obj)
}

/// Merge override resource_tracks into base by track `id` (case-insensitive):
/// an override replaces a same-id base track; new ids are appended. Mirrors
/// chargen `merge_override`.
fn merge_resource_tracks(
    base: Vec<serde_json::Value>,
    overrides: Vec<serde_json::Value>,
) -> Vec<serde_json::Value> {
    let idof = |v: &serde_json::Value| {
        v.get("id")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase()
    };
    let mut out = base;
    for o in overrides {
        let oid = idof(&o);
        if oid.is_empty() {
            continue;
        }
        if let Some(slot) = out.iter_mut().find(|b| idof(b) == oid) {
            *slot = o;
        } else {
            out.push(o);
        }
    }
    out
}

#[cfg(test)]
mod merge_module_config_tests {
    use super::merge_module_config;
    use trpg_model::{DirectorModuleConfig, DirectorSceneFact, ModuleConfig, NpcActorBinding};

    fn cfg_with_fact(text: &str) -> DirectorModuleConfig {
        DirectorModuleConfig {
            scene_facts: vec![DirectorSceneFact {
                text: text.into(),
                source: "x".into(),
            }],
            ..Default::default()
        }
    }

    /// 无 sidecar、有 extracted → director = extracted(验收 1 的合并侧)。
    #[test]
    fn extracted_only_when_no_sidecar() {
        let out = merge_module_config(None, Some(cfg_with_fact("从场景抽的"))).expect("应有配置");
        assert_eq!(out.director.unwrap().scene_facts[0].text, "从场景抽的");
    }

    /// sidecar.director 存在 → 整块盖掉 extracted(验收 2 override 胜)。
    #[test]
    fn sidecar_director_wins_over_extracted() {
        let sidecar = ModuleConfig {
            director: Some(cfg_with_fact("override")),
            ..Default::default()
        };
        let out = merge_module_config(Some(sidecar), Some(cfg_with_fact("extracted"))).unwrap();
        assert_eq!(out.director.unwrap().scene_facts[0].text, "override");
    }

    /// sidecar 有别的字段但 director=None → director 用 extracted,其它 sidecar 字段保留。
    #[test]
    fn extracted_fills_when_sidecar_has_no_director() {
        let sidecar = ModuleConfig {
            npc_actor_bindings: vec![NpcActorBinding {
                matcher: vec!["boss".into()],
                actor_id: "npc.boss".into(),
                display_name: None,
            }],
            director: None,
            ..Default::default()
        };
        let out = merge_module_config(Some(sidecar), Some(cfg_with_fact("extracted"))).unwrap();
        assert_eq!(out.director.unwrap().scene_facts[0].text, "extracted");
        assert_eq!(
            out.npc_actor_bindings.len(),
            1,
            "sidecar 非 director 字段必须保留"
        );
    }

    /// 都没有 → None(回退通用兜底)。
    #[test]
    fn both_absent_is_none() {
        assert!(merge_module_config(None, None).is_none());
    }
}

#[cfg(test)]
mod firearm_profile_override_tests {
    use super::{apply_kernel_strategy_overrides, embedded_kernel_override};
    use trpg_model::RuleKernel;

    /// P0 dehardcode: the CoC override doc carries the migrated `.45 Automatic`
    /// firearm profile, and `apply_kernel_strategy_overrides` layers it onto the
    /// kernel byte-for-byte the legacy hardcoded values. Asserts against the
    /// binary-embedded (git-tracked, shipped) override — the source of truth — so
    /// the test is independent of any local `data/` mirror.
    #[test]
    fn coc_override_supplies_firearm_profile() {
        let doc: serde_json::Value = serde_json::from_str(
            embedded_kernel_override("call_of_cthulhu_7e").expect("CoC override embedded"),
        )
        .expect("CoC override parses");
        let mut kernel: RuleKernel = serde_json::from_str(
            r#"{"kernel_id":"t","ruleset_id":"call_of_cthulhu_7e","version":"1"}"#,
        )
        .unwrap();
        assert!(kernel.firearm_profiles.is_empty());
        apply_kernel_strategy_overrides(&mut kernel, &doc);
        assert_eq!(
            kernel.firearm_profiles.len(),
            1,
            "CoC override must add exactly the .45 Automatic profile"
        );
        let fp = &kernel.firearm_profiles[0];
        assert_eq!(fp.def_id_suffix, "weapon.45_automatic");
        assert!(fp.matches("colt m1911 .45 automatic handgun"));
        assert_eq!(
            fp.profile.get("damage_expression").and_then(|v| v.as_str()),
            Some("1d10+2")
        );
        assert_eq!(
            fp.profile.get("ammo_capacity").and_then(|v| v.as_i64()),
            Some(7)
        );
        assert_eq!(fp.source_refs.len(), 1);
        assert_eq!(fp.source_refs[0].page, Some(414));
    }
}

#[cfg(test)]
mod turn_trace_serde_tests {
    use trpg_model::{TurnFailureRecord, TurnTrace};

    /// obs T4（DB-free）：`serde_json::to_value(&TurnTrace)` round-trips back to an
    /// equal TurnTrace —— 证明 upsert_turn_trace / load_turn_trace 用的 jsonb
    /// (反)序列化形态正确（无需 live DB 即可守护）。
    #[test]
    fn turn_trace_to_value_roundtrip() {
        let mut trace = TurnTrace::new("turn-t4", "sess-t4");
        trace.phases_run = vec!["context_assembly".into(), "finalize".into()];
        trace.bp1_hash = Some("sha256:bp1".into());
        trace.bp2_hash = Some("sha256:bp2".into());
        trace.bp3_hash = Some("sha256:bp3".into());
        trace.signal = "turn_complete".into();
        trace.warnings = vec!["audit lag".into()];
        trace.pp_lifecycle = "complete".into();
        trace.narration_hash = Some("sha256:narr".into());
        trace.failure = Some(TurnFailureRecord {
            phase: "finalize".into(),
            message: "save_turn timeout".into(),
            failure_kind: "failed_finalize".into(),
        });

        let value = serde_json::to_value(&trace).expect("serialize TurnTrace to Value");
        let back: TurnTrace =
            serde_json::from_value(value).expect("deserialize Value to TurnTrace");
        assert_eq!(
            trace, back,
            "jsonb round-trip must preserve the full TurnTrace"
        );
    }
}

#[cfg(test)]
mod dice_core_override_tests {
    use super::merge_dice_core;
    use serde_json::json;

    #[test]
    fn override_replaces_bands_wholesale_and_keeps_untouched_keys() {
        let base =
            json!({"compare":"roll_under","success_bands":[{"id":"regular"},{"id":"extreme"}]});
        let over = json!({"success_bands":[{"id":"critical"},{"id":"failure"}]});
        let merged = merge_dice_core(base, over.as_object().unwrap());
        // untouched base key survives
        assert_eq!(
            merged.get("compare").and_then(|v| v.as_str()),
            Some("roll_under")
        );
        // success_bands REPLACED wholesale, not appended/merged-by-id
        let ids: Vec<&str> = merged
            .get("success_bands")
            .and_then(|v| v.as_array())
            .unwrap()
            .iter()
            .filter_map(|b| b.get("id").and_then(|x| x.as_str()))
            .collect();
        assert_eq!(
            ids,
            vec!["critical", "failure"],
            "array key must be replaced, not deep-merged"
        );
    }

    #[test]
    fn non_object_base_is_replaced_by_override() {
        let merged = merge_dice_core(
            json!(null),
            json!({"compare":"meet_or_beat"}).as_object().unwrap(),
        );
        assert_eq!(
            merged.get("compare").and_then(|v| v.as_str()),
            Some("meet_or_beat")
        );
    }

    /// （A4 回归）override merge 与编译遍升级出的新字段兼容：
    /// ① base success_bands 带 semantics、override 只设 "dice" 键 → semantics
    ///   保留（shallow key 语义天然兼容）；
    /// ② override 设 success_bands 键 → 整值替换（文档化既有语义：override
    ///   文件必须带它想保留的全部字段）；
    /// ③ resource_tracks override：未被覆盖 track 的 followup_procedure_id
    ///   保留、被覆盖 track 整体替换（merge_resource_tracks 语义不变）。
    #[test]
    fn override_merge_keeps_upgraded_fields() {
        use super::merge_resource_tracks;
        // ① untouched key keeps the upgraded semantics
        let base =
            json!({"dice":"1d100","success_bands":[{"id":"regular","semantics":"plain success"}]});
        let merged = merge_dice_core(base, json!({"dice":"1d20"}).as_object().unwrap());
        assert_eq!(merged.get("dice").and_then(|v| v.as_str()), Some("1d20"));
        assert_eq!(
            merged
                .pointer("/success_bands/0/semantics")
                .and_then(|v| v.as_str()),
            Some("plain success"),
            "shallow merge: an untouched key keeps its upgraded fields"
        );
        // ② an override that sets success_bands replaces the WHOLE value
        let base = json!({"success_bands":[{"id":"regular","semantics":"plain success"}]});
        let merged = merge_dice_core(
            base,
            json!({"success_bands":[{"id":"critical"}]})
                .as_object()
                .unwrap(),
        );
        let bands = merged
            .get("success_bands")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(bands.len(), 1);
        assert_eq!(
            bands[0].get("id").and_then(|v| v.as_str()),
            Some("critical")
        );
        assert!(
            bands[0].get("semantics").is_none(),
            "wholesale replacement: an override file must carry the FULL fields it wants to keep"
        );
        // ③ resource_tracks: untouched track keeps its upgraded threshold key;
        //    an overridden track is replaced wholesale
        let base = vec![
            json!({"id":"sanity","thresholds":[{"loss_in_one_go":5,"followup_procedure_id":"x.temp"}]}),
            json!({"id":"luck","max":99}),
        ];
        let merged = merge_resource_tracks(base, vec![json!({"id":"luck","max":50})]);
        assert_eq!(
            merged[0]
                .pointer("/thresholds/0/followup_procedure_id")
                .and_then(|v| v.as_str()),
            Some("x.temp"),
            "untouched track keeps the upgraded followup_procedure_id"
        );
        assert_eq!(
            merged[1].get("max").and_then(|v| v.as_i64()),
            Some(50),
            "overridden track replaced wholesale"
        );
    }
}

#[cfg(test)]
mod kernel_strategy_override_tests {
    use super::{apply_kernel_strategy_overrides, read_module_config_file};
    use serde_json::json;
    use trpg_model::{CombatMode, RuleKernel};

    /// Serializes the env-var (`TRPG_DATA_DIR`) mutating tests in this module so
    /// the parallel test runner does not interleave their set_var/remove_var.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// P0-2: the new strategy policy keys layer onto the kernel; missing keys
    /// leave the field None (→ engine uses GENERIC_*).
    #[test]
    fn apply_strategy_overrides_layers_policy_keys() {
        let mut kernel = RuleKernel::default();
        let doc = json!({
            "combat_mode_policy": {
                "rules": [{"mode": "netrun", "when_action_kinds": ["hack"], "when_evidence_contains": []}],
                "fallback_mode": "firefight"
            },
            "check_label_policy": {"labels": {"hack": "TECH check"}},
            "dice_qualification": {"bare_dice_template": "{dice}+0"}
        });
        apply_kernel_strategy_overrides(&mut kernel, &doc);
        let cmp = kernel
            .combat_mode_policy
            .expect("combat_mode_policy layered");
        assert_eq!(cmp.fallback_mode, CombatMode::Firefight);
        assert_eq!(cmp.rules[0].mode, CombatMode::Netrun);
        assert_eq!(
            kernel
                .check_label_policy
                .unwrap()
                .labels
                .get("hack")
                .map(String::as_str),
            Some("TECH check")
        );
        assert_eq!(
            kernel.dice_qualification.unwrap().bare_dice_template,
            "{dice}+0"
        );
        // a doc with none of the keys leaves the kernel fields None
        let mut k2 = RuleKernel::default();
        apply_kernel_strategy_overrides(&mut k2, &json!({"resource_tracks": []}));
        assert!(k2.combat_mode_policy.is_none());
        assert!(k2.check_label_policy.is_none());
        assert!(k2.dice_qualification.is_none());
    }

    /// SKIP-gated: with the real data dir present, every shipped override /
    /// module_config deserializes into the typed structs (proves the migrated
    /// VALUES are well-formed for the equivalence gate).
    #[test]
    fn shipped_override_files_deserialize() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::var("TRPG_DATA_DIR").unwrap_or_else(|_| "../../data".to_string());
        if !std::path::Path::new(&dir).join("parsed/rules").exists() {
            eprintln!("SKIP: data dir absent");
            return;
        }
        // SAFETY (test): single-threaded set of an env var for read_*_file.
        unsafe {
            std::env::set_var("TRPG_DATA_DIR", &dir);
        }
        // cyberpunk override: firefight fallback + hack/disable_device netrun rule.
        let mut k = RuleKernel::default();
        let doc =
            super::read_kernel_override_file("cyberpunk_red").expect("cyberpunk override present");
        apply_kernel_strategy_overrides(&mut k, &doc);
        let cmp = k.combat_mode_policy.expect("cyberpunk combat_mode_policy");
        assert_eq!(cmp.fallback_mode, CombatMode::Firefight);
        assert!(cmp.rules.iter().any(
            |r| r.mode == CombatMode::Netrun && r.when_action_kinds.iter().any(|a| a == "hack")
        ));
        assert_eq!(
            k.check_label_policy
                .unwrap()
                .labels
                .get("hack")
                .map(String::as_str),
            Some("appropriate TECH / Interface / Basic Tech check")
        );
        assert_eq!(k.dice_qualification.unwrap().bare_dice_template, "{dice}+0");
        // module config: scav_boss / athena_drone bindings + DV 14/12 table.
        let cfg = read_module_config_file("cyberpunk_red.homecoming")
            .expect("homecoming module_config present");
        assert!(cfg
            .npc_actor_bindings
            .iter()
            .any(|b| b.actor_id == "npc.scav_boss"));
        assert!(cfg
            .npc_actor_bindings
            .iter()
            .any(|b| b.actor_id == "npc.athena_drone"));
        let dvs: Vec<i32> = cfg
            .technical_option_table
            .unwrap()
            .iter()
            .map(|t| t.dv)
            .collect();
        assert_eq!(dvs, vec![14, 12]);
    }

    /// P0-2 clean-checkout equivalence: point TRPG_DATA_DIR at a FRESH EMPTY dir
    /// (so no override/module files exist on disk) and prove the binary-embedded
    /// copies reproduce the migrated values. read_*_file read the env at call
    /// time, so a temp dir forces the embedded fallback branch.
    #[test]
    fn embedded_config_reproduces_values_without_data_dir() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("TRPG_DATA_DIR").ok();
        let tmp = std::env::temp_dir().join(format!("trpg_embed_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("mk empty temp data dir");
        // restore-on-drop guard so a panic mid-test doesn't poison other tests.
        struct Restore(Option<String>, std::path::PathBuf);
        impl Drop for Restore {
            fn drop(&mut self) {
                // SAFETY (test): env restore under ENV_LOCK (single active test).
                unsafe {
                    match &self.0 {
                        Some(v) => std::env::set_var("TRPG_DATA_DIR", v),
                        None => std::env::remove_var("TRPG_DATA_DIR"),
                    }
                }
                let _ = std::fs::remove_dir_all(&self.1);
            }
        }
        let _restore = Restore(prev, tmp.clone());
        // SAFETY (test): env set under ENV_LOCK.
        unsafe {
            std::env::set_var("TRPG_DATA_DIR", &tmp);
        }

        // sanity: the empty dir really has no override/module files.
        assert!(
            !tmp.join("parsed/rules").exists(),
            "temp data dir must be empty"
        );

        // cyberpunk override comes from the embedded copy: search_profile +
        // check_label_policy present, combat_mode_policy firefight fallback.
        let mut k = RuleKernel::default();
        let doc = super::read_kernel_override_file("cyberpunk_red")
            .expect("cyberpunk override via embedded fallback");
        assert!(
            doc.get("search_profile").is_some(),
            "embedded override carries search_profile"
        );
        assert!(
            doc.get("check_label_policy").is_some(),
            "embedded override carries check_label_policy"
        );
        apply_kernel_strategy_overrides(&mut k, &doc);
        assert_eq!(
            k.combat_mode_policy
                .expect("combat_mode_policy")
                .fallback_mode,
            CombatMode::Firefight
        );

        // triangle override also resolves from embedded.
        assert!(
            super::read_kernel_override_file("triangle_agency").is_some(),
            "triangle override via embedded fallback"
        );

        // module config from embedded: director + scav_boss/athena_drone bindings.
        let cfg = read_module_config_file("cyberpunk_red.homecoming")
            .expect("homecoming module_config via embedded fallback");
        assert!(cfg
            .npc_actor_bindings
            .iter()
            .any(|b| b.actor_id == "npc.scav_boss"));
        assert!(cfg
            .npc_actor_bindings
            .iter()
            .any(|b| b.actor_id == "npc.athena_drone"));

        // direct helper-fn check (no env / no Db needed).
        assert!(super::embedded_module_config("cyberpunk_red.homecoming").is_some());
        assert!(super::embedded_kernel_override("triangle_agency").is_some());
        assert!(super::embedded_kernel_override("not_a_ruleset").is_none());
    }

    /// §10.5 behavior alignment: the CoC embedded override now carries aligned
    /// resource_tracks (hit_points/magic_points) — no DB or data/ dir needed.
    #[test]
    fn coc_embedded_override_carries_aligned_behavior_tracks() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("TRPG_DATA_DIR").ok();
        let tmp = std::env::temp_dir().join(format!("trpg_embed_coc_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("mk empty temp data dir");
        struct Restore(Option<String>, std::path::PathBuf);
        impl Drop for Restore {
            fn drop(&mut self) {
                // SAFETY (test): env restore under ENV_LOCK (single active test).
                unsafe {
                    match &self.0 {
                        Some(v) => std::env::set_var("TRPG_DATA_DIR", v),
                        None => std::env::remove_var("TRPG_DATA_DIR"),
                    }
                }
                let _ = std::fs::remove_dir_all(&self.1);
            }
        }
        let _restore = Restore(prev, tmp.clone());
        // SAFETY (test): env set under ENV_LOCK.
        unsafe {
            std::env::set_var("TRPG_DATA_DIR", &tmp);
        }

        let doc = super::read_kernel_override_file("call_of_cthulhu_7e")
            .expect("CoC override via embedded fallback");
        let tracks = doc
            .get("resource_tracks")
            .and_then(|v| v.as_array())
            .expect("CoC embedded override carries resource_tracks");
        let find = |id: &str| {
            tracks
                .iter()
                .find(|t| t.get("id").and_then(|v| v.as_str()) == Some(id))
        };
        let hp = find("hit_points").expect("hit_points track present");
        assert_eq!(
            hp.get("derived_from").and_then(|v| v.as_str()),
            Some("hp_max"),
            "hit_points must link to formula id hp_max"
        );
        assert!(
            hp.get("on_outcome")
                .and_then(|v| v.as_array())
                .map(|a| !a.is_empty())
                .unwrap_or(false),
            "hit_points must carry damage on_outcome"
        );
        assert!(
            hp.get("thresholds")
                .and_then(|v| v.as_array())
                .map(|a| !a.is_empty())
                .unwrap_or(false),
            "hit_points must carry wound/dying thresholds"
        );
        let mp = find("magic_points").expect("magic_points track present");
        assert_eq!(
            mp.get("derived_from").and_then(|v| v.as_str()),
            Some("mp_max"),
            "magic_points must link to formula id mp_max"
        );
    }
}

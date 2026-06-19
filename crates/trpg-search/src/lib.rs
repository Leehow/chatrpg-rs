use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tantivy::collector::TopDocs;
use tantivy::directory::MmapDirectory;
use tantivy::query::{AllQuery, QueryParser, TermQuery};
use tantivy::schema::{
    Field, IndexRecordOption, Schema, Value as TantivyValue, STORED, STRING, TEXT,
};
use tantivy::{doc, Index, TantivyDocument, Term};
use tracing::{info, warn};
use trpg_model::*;
use uuid::Uuid;
use walkdir::WalkDir;

pub const TANTIVY_INDEX_SCHEMA_VERSION: &str =
    "chatrpg.tantivy_index.v3.runtime_load_incremental_cjk";

#[derive(Debug, Clone)]
pub struct SearchConfig {
    pub index_dir: PathBuf,
    pub data_dir: PathBuf,
    pub writer_memory_budget_bytes: usize,
}

impl SearchConfig {
    pub fn from_env_or_defaults(data_dir: impl Into<PathBuf>) -> Self {
        let data_dir = data_dir.into();
        let index_dir = std::env::var("TRPG_SEARCH_INDEX_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| data_dir.join("search/tantivy_v3"));
        Self {
            index_dir,
            data_dir,
            writer_memory_budget_bytes: std::env::var("TRPG_SEARCH_WRITER_MEMORY_BYTES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(96_000_000),
        }
    }
}

#[derive(Clone)]
struct SearchFields {
    search_doc_id: Field,
    origin: Field,
    domain: Field,
    logical_kind: Field,
    title: Field,
    body: Field,
    tags: Field,
    cjk_ngrams: Field,
    facets: Field,
    scopes_json: Field,
    visibility: Field,
    stability: Field,
    source_refs_json: Field,
    metadata_json: Field,
    updated_at: Field,
}

#[derive(Clone)]
pub struct TantivySearchEngine {
    index: Index,
    fields: SearchFields,
    config: SearchConfig,
}

impl TantivySearchEngine {
    pub fn open(config: SearchConfig) -> Result<Self> {
        let version_file = config.index_dir.join("schema_version.txt");
        if config.index_dir.exists() {
            let existing_version = fs::read_to_string(&version_file).unwrap_or_default();
            if existing_version.trim() != TANTIVY_INDEX_SCHEMA_VERSION {
                fs::remove_dir_all(&config.index_dir).with_context(|| {
                    format!(
                        "failed to reset stale Tantivy index dir {}",
                        config.index_dir.display()
                    )
                })?;
            }
        }
        fs::create_dir_all(&config.index_dir).with_context(|| {
            format!(
                "failed to create Tantivy index dir {}",
                config.index_dir.display()
            )
        })?;
        fs::write(&version_file, TANTIVY_INDEX_SCHEMA_VERSION).ok();
        let mut builder = Schema::builder();
        let search_doc_id = builder.add_text_field("search_doc_id", STRING | STORED);
        let origin = builder.add_text_field("origin", STRING | STORED);
        let domain = builder.add_text_field("domain", STRING | STORED);
        let logical_kind = builder.add_text_field("logical_kind", STRING | STORED);
        let title = builder.add_text_field("title", TEXT | STORED);
        let body = builder.add_text_field("body", TEXT | STORED);
        let tags = builder.add_text_field("tags", TEXT | STORED);
        let cjk_ngrams = builder.add_text_field("cjk_ngrams", TEXT | STORED);
        let facets = builder.add_text_field("facets", STRING | STORED);
        let scopes_json = builder.add_text_field("scopes_json", STRING | STORED);
        let visibility = builder.add_text_field("visibility", STRING | STORED);
        let stability = builder.add_text_field("stability", STRING | STORED);
        let source_refs_json = builder.add_text_field("source_refs_json", STRING | STORED);
        let metadata_json = builder.add_text_field("metadata_json", STRING | STORED);
        let updated_at = builder.add_text_field("updated_at", STRING | STORED);
        let schema = builder.build();
        let dir = MmapDirectory::open(&config.index_dir)?;
        let index = Index::open_or_create(dir, schema)?;
        Ok(Self {
            index,
            fields: SearchFields {
                search_doc_id,
                origin,
                domain,
                logical_kind,
                title,
                body,
                tags,
                cjk_ngrams,
                facets,
                scopes_json,
                visibility,
                stability,
                source_refs_json,
                metadata_json,
                updated_at,
            },
            config,
        })
    }

    pub fn clear_all(&self) -> Result<()> {
        let mut writer = self
            .index
            .writer::<TantivyDocument>(self.config.writer_memory_budget_bytes)?;
        writer.delete_all_documents()?;
        writer.commit()?;
        Ok(())
    }

    pub fn upsert_documents(&self, documents: &[SearchDocument]) -> Result<usize> {
        if documents.is_empty() {
            return Ok(0);
        }
        let mut writer = self
            .index
            .writer::<TantivyDocument>(self.config.writer_memory_budget_bytes)?;
        for source_doc in documents {
            writer.delete_term(Term::from_field_text(
                self.fields.search_doc_id,
                &source_doc.search_doc_id,
            ));
            let mut tdoc = doc!(
                self.fields.search_doc_id => source_doc.search_doc_id.clone(),
                self.fields.origin => source_doc.origin.clone(),
                self.fields.domain => source_doc.domain.clone(),
                self.fields.logical_kind => source_doc.logical_kind.clone(),
                self.fields.title => source_doc.title.clone(),
                self.fields.body => source_doc.body.clone(),
                self.fields.cjk_ngrams => build_cjk_index_terms(source_doc),
                self.fields.visibility => source_doc.visibility.as_str().to_string(),
                self.fields.stability => source_doc.stability.as_str().to_string(),
                self.fields.scopes_json => serde_json::to_string(&source_doc.scopes).unwrap_or_default(),
                self.fields.source_refs_json => serde_json::to_string(&source_doc.source_refs).unwrap_or_default(),
                self.fields.metadata_json => serde_json::to_string(&source_doc.metadata).unwrap_or_default(),
                self.fields.updated_at => source_doc.updated_at.to_rfc3339(),
            );
            for tag in &source_doc.tags {
                tdoc.add_text(self.fields.tags, tag);
            }
            for facet in source_doc.facets() {
                tdoc.add_text(self.fields.facets, &facet);
            }
            writer.add_document(tdoc)?;
        }
        writer.commit()?;
        Ok(documents.len())
    }

    pub fn search(&self, request: &SearchRequest) -> Result<SearchResponse> {
        let reader = self.index.reader()?;
        let searcher = reader.searcher();
        let mut parser = QueryParser::for_index(
            &self.index,
            vec![
                self.fields.title,
                self.fields.body,
                self.fields.tags,
                self.fields.cjk_ngrams,
                self.fields.facets,
            ],
        );
        parser.set_field_boost(self.fields.title, 2.0);
        parser.set_field_boost(self.fields.tags, 1.4);
        parser.set_field_boost(self.fields.cjk_ngrams, 1.1);
        parser.set_field_boost(self.fields.facets, 1.2);

        let query_text = build_tantivy_query_string(request);
        let top_limit = request
            .limit
            .saturating_mul(12)
            .max(request.limit)
            .min(1000) as usize;
        let top_docs = if query_text == "*" {
            searcher.search(&AllQuery, &TopDocs::with_limit(top_limit).order_by_score())?
        } else {
            let parsed = parser.parse_query(&query_text).or_else(|primary_err| {
                let fallback = quoted_query(&request.query);
                parser.parse_query(&fallback).with_context(|| format!("failed to parse Tantivy query `{query_text}`; fallback also failed after primary error: {primary_err}"))
            })?;
            searcher.search(&parsed, &TopDocs::with_limit(top_limit).order_by_score())?
        };

        let mut hits = Vec::new();
        let mut considered = 0usize;
        for (score, addr) in top_docs {
            let retrieved = searcher.doc::<TantivyDocument>(addr)?;
            let doc = self.tantivy_doc_to_search_doc(&retrieved)?;
            considered += 1;
            if !matches_request(&doc, request) {
                continue;
            }
            hits.push(search_hit_from_doc(&doc, score, request));
            if hits.len() >= request.limit as usize {
                break;
            }
        }

        Ok(SearchResponse {
            query_id: format!("search_{}", Uuid::new_v4().simple()),
            hits,
            total_considered: considered,
            index_generation: Some(TANTIVY_INDEX_SCHEMA_VERSION.to_string()),
            rewritten_query: Some(query_text),
        })
    }

    pub fn get_document(&self, search_doc_id: &str) -> Result<Option<SearchDocument>> {
        let reader = self.index.reader()?;
        let searcher = reader.searcher();
        let term = Term::from_field_text(self.fields.search_doc_id, search_doc_id);
        let query = TermQuery::new(term, IndexRecordOption::Basic);
        let top_docs = searcher.search(&query, &TopDocs::with_limit(1).order_by_score())?;
        if let Some((_score, addr)) = top_docs.into_iter().next() {
            let retrieved = searcher.doc::<TantivyDocument>(addr)?;
            return Ok(Some(self.tantivy_doc_to_search_doc(&retrieved)?));
        }
        Ok(None)
    }

    fn tantivy_doc_to_search_doc(&self, doc: &TantivyDocument) -> Result<SearchDocument> {
        let get = |field: Field| -> String {
            doc.get_first(field)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let tags = doc
            .get_all(self.fields.tags)
            .filter_map(|v| v.as_str().map(ToString::to_string))
            .collect::<Vec<_>>();
        let search_doc_id = get(self.fields.search_doc_id);
        let origin = get(self.fields.origin);
        let domain = get(self.fields.domain);
        let logical_kind = get(self.fields.logical_kind);
        let title = get(self.fields.title);
        let body = get(self.fields.body);
        let visibility = visibility_from_str_local(&get(self.fields.visibility));
        let stability = stability_from_str_local(&get(self.fields.stability));
        let scopes_json = get(self.fields.scopes_json);
        let source_refs_json = get(self.fields.source_refs_json);
        let metadata_json = get(self.fields.metadata_json);
        let updated_at = DateTime::parse_from_rfc3339(&get(self.fields.updated_at))
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        let scopes =
            serde_json::from_str::<BTreeMap<String, String>>(&scopes_json).unwrap_or_default();
        let source_refs =
            serde_json::from_str::<Vec<SourceRef>>(&source_refs_json).unwrap_or_default();
        let metadata = serde_json::from_str::<Value>(&metadata_json).unwrap_or_else(|_| json!({}));
        Ok(SearchDocument {
            search_doc_id,
            origin,
            domain,
            logical_kind,
            title,
            body,
            tags,
            scopes,
            visibility,
            stability,
            source_refs,
            metadata,
            updated_at,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchSourceConfig {
    pub source_config_id: String,
    pub source_kind: String,
    pub label: String,
    pub enabled: bool,
    pub priority: i32,
    pub config_json: Value,
}

#[derive(Clone)]
pub struct SearchService {
    pub pool: PgPool,
    pub engine: TantivySearchEngine,
    pub config: SearchConfig,
}

impl SearchService {
    pub fn open(pool: PgPool, config: SearchConfig) -> Result<Self> {
        let engine = TantivySearchEngine::open(config.clone())?;
        Ok(Self {
            pool,
            engine,
            config,
        })
    }

    pub async fn reindex_all(&self) -> Result<SearchIndexStats> {
        self.engine.clear_all()?;
        reset_search_watermarks(&self.pool).await.ok();
        self.reindex_with_mode("full_rebuild", false).await
    }

    pub async fn reindex_incremental(&self) -> Result<SearchIndexStats> {
        self.reindex_with_mode("incremental", true).await
    }

    async fn reindex_with_mode(&self, mode: &str, incremental: bool) -> Result<SearchIndexStats> {
        let started = Instant::now();
        let configs = load_search_source_configs(&self.pool).await?;
        let mut stats = SearchIndexStats {
            source_count: configs.len(),
            mode: mode.to_string(),
            incremental,
            watermark: if incremental {
                Some("per_source".to_string())
            } else {
                None
            },
            index_dir: self.config.index_dir.to_string_lossy().to_string(),
            ..Default::default()
        };
        for config in configs.into_iter().filter(|c| c.enabled) {
            let watermark = if incremental {
                source_search_watermark(&self.pool, &config.source_config_id).await?
            } else {
                None
            };
            match self.load_config_documents_since(&config, watermark).await {
                Ok(documents) => {
                    if documents.is_empty() {
                        stats.skipped_unchanged_documents += 1;
                        continue;
                    }
                    let max_updated_at = documents
                        .iter()
                        .map(|d| d.updated_at)
                        .max()
                        .unwrap_or_else(Utc::now);
                    let indexed = self.engine.upsert_documents(&documents)?;
                    stats.indexed_documents += indexed;
                    upsert_source_search_watermark(
                        &self.pool,
                        &config.source_config_id,
                        max_updated_at,
                        indexed as i64,
                    )
                    .await
                    .ok();
                    info!(source_config_id=%config.source_config_id, indexed, mode, "indexed search source");
                }
                Err(err) => {
                    stats.skipped_sources += 1;
                    let msg = format!("{}: {err}", config.source_config_id);
                    warn!(error=%err, source_config_id=%config.source_config_id, "failed to index search source");
                    stats.errors.push(msg);
                }
            }
        }
        stats.duration_ms = started.elapsed().as_millis();
        insert_search_index_run(&self.pool, &stats).await.ok();
        Ok(stats)
    }

    pub fn search(&self, request: &SearchRequest) -> Result<SearchResponse> {
        self.engine.search(request)
    }

    pub async fn search_async(&self, request: &SearchRequest) -> Result<SearchResponse> {
        let first = self.search(request)?;
        if !first.hits.is_empty()
            || !request.rewrite_query
            || !search_cjk_expansion_enabled()
            || !should_try_query_rewrites(&request.query)
        {
            return Ok(first);
        }
        for rewritten in rewrite_query_variants(&request.query) {
            if rewritten.trim().is_empty() || rewritten == request.query {
                continue;
            }
            let mut req = request.clone();
            req.query = rewritten;
            req.mode = SearchMode::Query;
            let mut response = self.search(&req)?;
            if !response.hits.is_empty() {
                let generation = response.index_generation.take().unwrap_or_default();
                response.index_generation = Some(format!("{generation};rewrite"));
                return Ok(response);
            }
        }
        Ok(first)
    }

    pub fn get_document(&self, search_doc_id: &str) -> Result<Option<SearchDocument>> {
        self.engine.get_document(search_doc_id)
    }

    pub fn load_hit_to_context_block(&self, req: &SearchLoadRequest) -> Result<ContextBlock> {
        let full_body = self
            .engine
            .get_document(&req.hit.search_doc_id)?
            .map(|doc| doc.body)
            .filter(|body| !body.trim().is_empty());
        let mut block = req.to_context_block();
        if let Some(body) = full_body {
            let clipped = body.chars().take(8_000).collect::<String>();
            block.content = BlockContent::Markdown(format!(
                "# {}\n\nRuling support status: source-backed search hit.\nOrigin: `{}` / `{}`. Score: {:.3}.\n\n{}\n\nSource refs: `{}`",
                req.hit.title,
                req.hit.origin,
                req.hit.logical_kind,
                req.hit.score,
                clipped,
                serde_json::to_string(&req.hit.source_refs).unwrap_or_default()
            ));
            block.content_hash = stable_json_hash(&block.content);
            block.token_estimate =
                Some((block.content.render_text().chars().count() as u32 / 4).max(1));
        }
        Ok(block)
    }

    async fn load_config_documents_since(
        &self,
        config: &SearchSourceConfig,
        watermark: Option<DateTime<Utc>>,
    ) -> Result<Vec<SearchDocument>> {
        match config.source_kind.as_str() {
            "sql_query" => load_sql_documents_since(&self.pool, config, watermark).await,
            "file_glob" => {
                load_file_documents_since(&self.config.data_dir, config, watermark).await
            }
            "jsonl" => load_jsonl_documents_since(&self.config.data_dir, config, watermark).await,
            other => Err(anyhow!("unsupported search source kind `{other}`")),
        }
    }
}

pub async fn load_search_source_configs(pool: &PgPool) -> Result<Vec<SearchSourceConfig>> {
    let rows = sqlx::query(
        r#"
        select source_config_id, source_kind, label, enabled, priority, config_json
        from search_source_configs
        where enabled = true
        order by priority desc, source_config_id asc
        "#,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| SearchSourceConfig {
            source_config_id: row.get("source_config_id"),
            source_kind: row.get("source_kind"),
            label: row.get("label"),
            enabled: row.get("enabled"),
            priority: row.get("priority"),
            config_json: row.get("config_json"),
        })
        .collect())
}

pub async fn upsert_search_source_config(pool: &PgPool, config: &SearchSourceConfig) -> Result<()> {
    sqlx::query(
        r#"
        insert into search_source_configs (id, source_config_id, source_kind, label, enabled, priority, config_json)
        values ($1,$2,$3,$4,$5,$6,$7)
        on conflict (source_config_id) do update set
          source_kind = excluded.source_kind,
          label = excluded.label,
          enabled = excluded.enabled,
          priority = excluded.priority,
          config_json = excluded.config_json,
          updated_at = now()
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(&config.source_config_id)
    .bind(&config.source_kind)
    .bind(&config.label)
    .bind(config.enabled)
    .bind(config.priority)
    .bind(&config.config_json)
    .execute(pool)
    .await?;
    Ok(())
}

async fn load_sql_documents_since(
    pool: &PgPool,
    config: &SearchSourceConfig,
    watermark: Option<DateTime<Utc>>,
) -> Result<Vec<SearchDocument>> {
    let sql = config
        .config_json
        .get("sql")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("sql_query source missing config_json.sql"))?;
    let rows = if let Some(watermark) = watermark {
        let wrapped = format!(
            "select * from ({}) as trpg_search_source where updated_at > $1",
            sql
        );
        sqlx::query(&wrapped)
            .bind(watermark)
            .fetch_all(pool)
            .await?
    } else {
        sqlx::query(sql).fetch_all(pool).await?
    };
    let mut docs = Vec::with_capacity(rows.len());
    for row in rows {
        if let Some(doc) = row_to_search_document(row, config)? {
            docs.push(doc);
        }
    }
    Ok(docs)
}

async fn load_file_documents_since(
    data_dir: &Path,
    config: &SearchSourceConfig,
    watermark: Option<DateTime<Utc>>,
) -> Result<Vec<SearchDocument>> {
    let root = relative_to_data_dir(
        data_dir,
        config
            .config_json
            .get("root")
            .and_then(Value::as_str)
            .unwrap_or("."),
    );
    if !root.exists() {
        return Ok(vec![]);
    }
    let extensions = config
        .config_json
        .get("extensions")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(|s| s.to_lowercase())
                .collect::<HashSet<_>>()
        })
        .unwrap_or_else(|| ["md".to_string(), "txt".to_string()].into_iter().collect());
    let domain = config
        .config_json
        .get("domain")
        .and_then(Value::as_str)
        .unwrap_or("source");
    let origin = config
        .config_json
        .get("origin")
        .and_then(Value::as_str)
        .unwrap_or("file");
    let logical_kind = config
        .config_json
        .get("logical_kind")
        .and_then(Value::as_str)
        .unwrap_or("source_excerpt");
    let visibility = visibility_from_str_local(
        config
            .config_json
            .get("visibility")
            .and_then(Value::as_str)
            .unwrap_or("gm_only"),
    );
    let mut docs = Vec::new();
    for entry in WalkDir::new(&root)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|e| e.file_type().is_file())
    {
        let ext = entry
            .path()
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_lowercase();
        if !extensions.contains(&ext) || !file_modified_after(entry.path(), watermark) {
            continue;
        }
        let text = fs::read_to_string(entry.path()).unwrap_or_default();
        if text.trim().is_empty() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(data_dir)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .to_string();
        for (idx, chunk) in chunk_text(&text, 12_000).into_iter().enumerate() {
            let mut scopes = BTreeMap::new();
            scopes.insert("path".into(), rel.clone());
            add_scope_hints_from_path(&rel, &mut scopes);
            let title = format!("{}#{}", rel, idx + 1);
            docs.push(SearchDocument {
                search_doc_id: format!("{}:{}:{}", origin, rel, idx + 1),
                origin: origin.to_string(),
                domain: domain.to_string(),
                logical_kind: logical_kind.to_string(),
                title,
                body: chunk,
                tags: vec!["file".into(), ext.clone()],
                scopes,
                visibility,
                stability: Stability::RarelyChanged,
                source_refs: vec![],
                metadata: json!({"path": rel, "chunk_index": idx + 1, "source_config_id": config.source_config_id}),
                updated_at: fs_modified_at(entry.path()),
            });
        }
    }
    Ok(docs)
}

async fn load_jsonl_documents_since(
    data_dir: &Path,
    config: &SearchSourceConfig,
    watermark: Option<DateTime<Utc>>,
) -> Result<Vec<SearchDocument>> {
    let root = relative_to_data_dir(
        data_dir,
        config
            .config_json
            .get("root")
            .and_then(Value::as_str)
            .unwrap_or("parsed"),
    );
    if !root.exists() {
        return Ok(vec![]);
    }
    let origin = config
        .config_json
        .get("origin")
        .and_then(Value::as_str)
        .unwrap_or("jsonl");
    let default_domain = config
        .config_json
        .get("domain")
        .and_then(Value::as_str)
        .unwrap_or("parsed");
    let default_visibility = visibility_from_str_local(
        config
            .config_json
            .get("visibility")
            .and_then(Value::as_str)
            .unwrap_or("gm_only"),
    );
    let mut docs = Vec::new();
    for entry in WalkDir::new(&root)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|e| e.file_type().is_file())
    {
        if entry.path().extension().and_then(|s| s.to_str()) != Some("jsonl")
            || !file_modified_after(entry.path(), watermark)
        {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(data_dir)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .to_string();
        let text = fs::read_to_string(entry.path()).unwrap_or_default();
        for (line_idx, line) in text.lines().enumerate() {
            let value: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let meta = value.get("metadata").cloned().unwrap_or_else(|| json!({}));
            let title = jsonl_title(&value);
            let body = jsonl_body_text(&value);
            if body.trim().is_empty() {
                continue;
            }
            let logical_kind = jsonl_logical_kind(&value);
            let mut tags = jsonl_tags(&value);
            let mut scopes = jsonl_scopes(&value, &rel);
            add_scope_hints_from_path(&rel, &mut scopes);
            let visibility = meta
                .get("visibility_hint")
                .or_else(|| meta.get("visibility"))
                .or_else(|| value.get("visibility"))
                .and_then(Value::as_str)
                .map(visibility_from_str_local)
                .unwrap_or(default_visibility);
            let source_refs = jsonl_source_refs(&value);
            let signal_class = meta
                .get("signal_class")
                .and_then(Value::as_str)
                .unwrap_or("");
            if signal_class == "noise" {
                tags.push("low_signal".into());
            }
            tags.sort();
            tags.dedup();
            docs.push(SearchDocument {
                search_doc_id: format!("{}:{}:{}", origin, rel, line_idx + 1),
                origin: origin.to_string(),
                domain: jsonl_domain(&value).unwrap_or_else(|| default_domain.to_string()),
                logical_kind,
                title,
                body,
                tags,
                scopes,
                visibility,
                stability: Stability::RarelyChanged,
                source_refs,
                metadata: json!({"path": rel, "line": line_idx + 1, "raw": value, "semantic_indexed": true}),
                updated_at: fs_modified_at(entry.path()),
            });
        }
    }
    Ok(docs)
}

fn jsonl_title(value: &Value) -> String {
    let meta = value.get("metadata");
    value
        .get("title")
        .and_then(Value::as_str)
        .or_else(|| meta.and_then(|m| m.get("title").and_then(Value::as_str)))
        .or_else(|| meta.and_then(|m| m.get("defined_entity").and_then(Value::as_str)))
        .or_else(|| {
            value
                .get("heading_context")
                .and_then(Value::as_array)
                .and_then(|arr| arr.last())
                .and_then(Value::as_str)
        })
        .or_else(|| {
            value
                .get("block_id")
                .or_else(|| value.get("material_id"))
                .and_then(Value::as_str)
        })
        .or_else(|| value.get("chunk_id").and_then(Value::as_str))
        .or_else(|| value.get("unit_id").and_then(Value::as_str))
        .unwrap_or("jsonl item")
        .to_string()
}

fn jsonl_domain(value: &Value) -> Option<String> {
    value
        .get("domain")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            value
                .get("metadata")
                .and_then(|m| m.get("owner_kind").and_then(Value::as_str))
                .map(|s| match s {
                    "module" => "modules".to_string(),
                    "ruleset" | "rulebook" => "rules".to_string(),
                    _ => s.to_string(),
                })
        })
}

fn jsonl_logical_kind(value: &Value) -> String {
    let meta = value.get("metadata");
    meta.and_then(|m| m.get("semantic_category").and_then(Value::as_str))
        .or_else(|| meta.and_then(|m| m.get("unit_kind").and_then(Value::as_str)))
        .or_else(|| {
            value
                .get("kind")
                .or_else(|| value.get("material_type"))
                .and_then(Value::as_str)
        })
        .or_else(|| value.get("clean_status").and_then(Value::as_str))
        .unwrap_or("jsonl_item")
        .to_string()
}

fn jsonl_tags(value: &Value) -> Vec<String> {
    let mut tags: Vec<String> = value
        .get("tags")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_else(Vec::new);
    tags.push("jsonl".into());
    for key in ["element_types"] {
        if let Some(arr) = value.get(key).and_then(Value::as_array) {
            tags.extend(
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string),
            );
        }
    }
    if let Some(meta) = value.get("metadata") {
        for key in [
            "semantic_category",
            "signal_class",
            "defined_entity",
            "unit_kind",
            "visibility_hint",
        ] {
            if let Some(v) = meta.get(key).and_then(Value::as_str) {
                if !v.trim().is_empty() {
                    tags.push(v.to_string());
                }
            }
        }
        if let Some(arr) = meta.get("mechanics_tags").and_then(Value::as_array) {
            tags.extend(
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string),
            );
        }
        if let Some(arr) = meta.get("quality_flags").and_then(Value::as_array) {
            tags.extend(
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string),
            );
        }
    }
    tags
}

fn jsonl_scopes(value: &Value, rel: &str) -> BTreeMap<String, String> {
    let mut scopes = BTreeMap::new();
    scopes.insert("path".into(), rel.to_string());
    if let Some(scope) = value.get("scope") {
        if let Some(scope_id) = scope.get("scope_id").and_then(Value::as_str) {
            scopes.insert("scope_id".into(), scope_id.to_string());
        }
        if let Some(scope_type) = scope.get("scope_type").and_then(Value::as_str) {
            scopes.insert("scope_type".into(), scope_type.to_string());
        }
    }
    for key in [
        "ruleset_id",
        "module_id",
        "session_id",
        "scene_id",
        "location_id",
        "mission_id",
        "chapter_id",
        "bundle_id",
        "owner_id",
        "source_id",
        "chunk_id",
        "unit_id",
    ] {
        if let Some(v) = value.get(key).and_then(Value::as_str) {
            scopes.insert(key.to_string(), v.to_string());
        }
    }
    if let Some(meta) = value.get("metadata") {
        for key in [
            "semantic_category",
            "signal_class",
            "defined_entity",
            "owner_kind",
            "source_unit_id",
        ] {
            if let Some(v) = meta.get(key).and_then(Value::as_str) {
                if !v.trim().is_empty() {
                    scopes.insert(key.to_string(), v.to_string());
                }
            }
        }
    }
    if let Some(page) = value.get("page").and_then(Value::as_u64) {
        scopes.insert("page".into(), page.to_string());
    }
    if let Some(page) = value.get("page_start").and_then(Value::as_u64) {
        scopes.insert("page_start".into(), page.to_string());
    }
    if let Some(page) = value.get("page_end").and_then(Value::as_u64) {
        scopes.insert("page_end".into(), page.to_string());
    }
    if let Some(pages) = value.get("page_numbers").and_then(Value::as_array) {
        if let Some(first) = pages.first().and_then(Value::as_u64) {
            scopes.entry("page".into()).or_insert(first.to_string());
            scopes
                .entry("page_start".into())
                .or_insert(first.to_string());
        }
        if let Some(last) = pages.last().and_then(Value::as_u64) {
            scopes.entry("page_end".into()).or_insert(last.to_string());
        }
    }
    if let Some(headings) = value.get("heading_context").and_then(Value::as_array) {
        let heading = headings
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" > ");
        if !heading.is_empty() {
            scopes.insert("heading".into(), heading);
        }
    }
    scopes
}

fn jsonl_source_refs(value: &Value) -> Vec<SourceRef> {
    if let Some(refs) = value
        .get("metadata")
        .and_then(|m| m.get("source_refs"))
        .or_else(|| value.get("source_refs"))
    {
        if let Ok(parsed) = serde_json::from_value::<Vec<SourceRef>>(refs.clone()) {
            if !parsed.is_empty() {
                return parsed;
            }
        }
    }
    let source_id = value
        .get("source_id")
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .get("metadata")
                .and_then(|m| m.get("source_id").and_then(Value::as_str))
        })
        .or_else(|| {
            value
                .get("unit_id")
                .and_then(Value::as_str)
                .and_then(|c| c.split('.').next())
        })
        .or_else(|| {
            value
                .get("chunk_id")
                .and_then(Value::as_str)
                .and_then(|c| c.split('.').next())
        })
        .unwrap_or("unknown")
        .to_string();
    let page = value
        .get("page")
        .and_then(Value::as_u64)
        .or_else(|| value.get("page_start").and_then(Value::as_u64))
        .or_else(|| {
            value
                .get("page_numbers")
                .and_then(Value::as_array)
                .and_then(|arr| arr.first())
                .and_then(Value::as_u64)
        })
        .map(|p| p as u32);
    let section_path = value
        .get("heading_context")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_else(Vec::new);
    vec![SourceRef {
        source_id,
        page,
        anchor_id: value
            .get("chunk_id")
            .or_else(|| value.get("unit_id"))
            .and_then(Value::as_str)
            .map(str::to_string),
        section_path,
        char_start: None,
        char_end: None,
        text_hash: value
            .get("text_hash")
            .and_then(Value::as_str)
            .map(str::to_string),
        note: Some("jsonl_semantic_source".into()),
    }]
}

fn row_to_search_document(
    row: sqlx::postgres::PgRow,
    config: &SearchSourceConfig,
) -> Result<Option<SearchDocument>> {
    let search_doc_id = row_string(&row, "search_doc_id").unwrap_or_default();
    if search_doc_id.trim().is_empty() {
        return Ok(None);
    }
    let origin = row_string(&row, "origin").unwrap_or_else(|| config.source_config_id.clone());
    let domain = row_string(&row, "domain").unwrap_or_else(|| "generic".to_string());
    let logical_kind = row_string(&row, "logical_kind").unwrap_or_else(|| "document".to_string());
    let title = row_string(&row, "title").unwrap_or_default();
    let body = row_string(&row, "body").unwrap_or_default();
    let tags = row_string_vec(&row, "tags").unwrap_or_default();
    let visibility = visibility_from_str_local(
        &row_string(&row, "visibility").unwrap_or_else(|| "gm_only".to_string()),
    );
    let stability = stability_from_str_local(
        &row_string(&row, "stability").unwrap_or_else(|| "rarely_changed".to_string()),
    );
    let scopes = row_json(&row, "scope_json")
        .and_then(json_to_string_map)
        .unwrap_or_default();
    let source_refs = row_json(&row, "source_refs")
        .and_then(|v| serde_json::from_value::<Vec<SourceRef>>(v).ok())
        .unwrap_or_default();
    let metadata = row_json(&row, "metadata")
        .unwrap_or_else(|| json!({"source_config_id": config.source_config_id}));
    let updated_at = row
        .try_get::<DateTime<Utc>, _>("updated_at")
        .unwrap_or_else(|_| Utc::now());
    Ok(Some(SearchDocument {
        search_doc_id,
        origin,
        domain,
        logical_kind,
        title,
        body,
        tags,
        scopes,
        visibility,
        stability,
        source_refs,
        metadata,
        updated_at,
    }))
}

fn row_string(row: &sqlx::postgres::PgRow, col: &str) -> Option<String> {
    row.try_get::<Option<String>, _>(col)
        .ok()
        .flatten()
        .or_else(|| row.try_get::<String, _>(col).ok())
}

fn row_string_vec(row: &sqlx::postgres::PgRow, col: &str) -> Option<Vec<String>> {
    row.try_get::<Vec<String>, _>(col).ok().or_else(|| {
        row.try_get::<Value, _>(col).ok().and_then(|v| {
            v.as_array().map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect()
            })
        })
    })
}

fn row_json(row: &sqlx::postgres::PgRow, col: &str) -> Option<Value> {
    row.try_get::<Value, _>(col).ok()
}

fn json_to_string_map(v: Value) -> Option<BTreeMap<String, String>> {
    let obj = v.as_object()?;
    let mut out = BTreeMap::new();
    for (k, v) in obj {
        if let Some(s) = v.as_str() {
            if !s.is_empty() {
                out.insert(k.clone(), s.to_string());
            }
        } else if !v.is_null() {
            out.insert(k.clone(), v.to_string());
        }
    }
    Some(out)
}

async fn insert_search_index_run(pool: &PgPool, stats: &SearchIndexStats) -> Result<()> {
    sqlx::query(
        r#"
        insert into search_index_runs (id, run_id, index_schema_version, stats_json, created_at)
        values ($1,$2,$3,$4,now())
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(format!("search_index_run_{}", Uuid::new_v4().simple()))
    .bind(TANTIVY_INDEX_SCHEMA_VERSION)
    .bind(serde_json::to_value(stats)?)
    .execute(pool)
    .await?;
    Ok(())
}

async fn last_search_index_run_at(pool: &PgPool) -> Result<Option<DateTime<Utc>>> {
    let value = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
        r#"select max(created_at) from search_index_runs where index_schema_version = $1"#,
    )
    .bind(TANTIVY_INDEX_SCHEMA_VERSION)
    .fetch_one(pool)
    .await?;
    Ok(value)
}

async fn reset_search_watermarks(pool: &PgPool) -> Result<()> {
    sqlx::query(r#"delete from search_index_watermarks"#)
        .execute(pool)
        .await?;
    Ok(())
}

async fn source_search_watermark(
    pool: &PgPool,
    source_config_id: &str,
) -> Result<Option<DateTime<Utc>>> {
    let value = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
        r#"select last_doc_updated_at from search_index_watermarks where source_config_id = $1"#,
    )
    .bind(source_config_id)
    .fetch_optional(pool)
    .await?
    .flatten();
    Ok(value)
}

async fn upsert_source_search_watermark(
    pool: &PgPool,
    source_config_id: &str,
    last_doc_updated_at: DateTime<Utc>,
    indexed_document_count: i64,
) -> Result<()> {
    sqlx::query(
        r#"
        insert into search_index_watermarks (id, source_config_id, last_indexed_at, last_doc_updated_at, indexed_document_count)
        values ($1,$2,now(),$3,$4)
        on conflict (source_config_id) do update set
          last_indexed_at = now(),
          last_doc_updated_at = greatest(search_index_watermarks.last_doc_updated_at, excluded.last_doc_updated_at),
          indexed_document_count = excluded.indexed_document_count,
          updated_at = now()
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(source_config_id)
    .bind(last_doc_updated_at)
    .bind(indexed_document_count)
    .execute(pool)
    .await?;
    Ok(())
}

fn file_modified_after(path: &Path, watermark: Option<DateTime<Utc>>) -> bool {
    let Some(watermark) = watermark else {
        return true;
    };
    fs_modified_at(path) > watermark
}

fn fs_modified_at(path: &Path) -> DateTime<Utc> {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .map(DateTime::<Utc>::from)
        .unwrap_or_else(|_| Utc::now())
}

fn search_cjk_expansion_enabled() -> bool {
    std::env::var("TRPG_SEARCH_CJK_EXPANSION")
        .map(|v| !matches!(v.as_str(), "0" | "false" | "FALSE" | "off" | "OFF"))
        .unwrap_or(true)
}

fn build_tantivy_query_string(req: &SearchRequest) -> String {
    let q = req.query.trim();
    if q.is_empty() || q == "*" {
        return "*".to_string();
    }
    match req.mode {
        SearchMode::Phrase | SearchMode::Literal => quoted_query(q),
        _ if req.rewrite_query && search_cjk_expansion_enabled() => expanded_query_clause(q),
        _ => q.to_string(),
    }
}

fn expanded_query_clause(q: &str) -> String {
    let mut terms = Vec::new();
    terms.push(q.to_string());
    if contains_cjk(q) {
        terms.extend(cjk_ngrams(q));
    }
    terms.extend(bilingual_alias_terms(q));
    terms.sort();
    terms.dedup();
    terms
        .into_iter()
        .filter(|t| !t.trim().is_empty())
        .map(|t| {
            if t.chars().any(char::is_whitespace) {
                quoted_query(&t)
            } else {
                t
            }
        })
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn build_cjk_index_terms(doc: &SearchDocument) -> String {
    let mut terms = cjk_ngrams(&format!(
        "{} {} {}",
        doc.title,
        doc.body,
        doc.tags.join(" ")
    ));
    terms.extend(bilingual_alias_terms(&doc.title));
    terms.extend(doc.tags.iter().flat_map(|t| bilingual_alias_terms(t)));
    terms.sort();
    terms.dedup();
    terms.join(" ")
}

fn should_try_query_rewrites(query: &str) -> bool {
    contains_cjk(query) || !bilingual_alias_terms(query).is_empty()
}

fn rewrite_query_variants(query: &str) -> Vec<String> {
    let mut variants = Vec::new();
    let grams = cjk_ngrams(query);
    if !grams.is_empty() {
        variants.push(grams.join(" "));
    }
    let aliases = bilingual_alias_terms(query);
    if !aliases.is_empty() {
        variants.push(aliases.join(" "));
    }
    if !grams.is_empty() && !aliases.is_empty() {
        let mut both = grams;
        both.extend(aliases);
        variants.push(both.join(" "));
    }
    variants
}

fn contains_cjk(input: &str) -> bool {
    input.chars().any(|c| matches!(c as u32, 0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x3040..=0x30FF | 0xAC00..=0xD7AF))
}

fn cjk_ngrams(input: &str) -> Vec<String> {
    let chars = input.chars().filter(|c| matches!(*c as u32, 0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x3040..=0x30FF | 0xAC00..=0xD7AF)).collect::<Vec<_>>();
    let mut out = Vec::new();
    for c in &chars {
        out.push(c.to_string());
    }
    for pair in chars.windows(2) {
        out.push(pair.iter().collect::<String>());
    }
    out.sort();
    out.dedup();
    out
}

fn bilingual_alias_terms(input: &str) -> Vec<String> {
    let lower = input.to_lowercase();
    let mut out = Vec::new();
    let pairs: &[(&str, &[&str])] = &[
        ("检定", &["check", "skill", "roll", "dv", "dc"]),
        ("技能", &["skill", "ability", "check"]),
        ("攻击", &["attack", "combat", "weapon", "ranged", "melee"]),
        ("战斗", &["combat", "initiative", "action", "damage"]),
        ("伤害", &["damage", "wound", "injury", "critical"]),
        ("治疗", &["healing", "stabilization", "trauma", "recovery"]),
        ("黑客", &["netrunning", "hack", "net", "architecture"]),
        ("黑入", &["netrunning", "hack", "net", "architecture"]),
        ("无人机", &["drone", "vehicle", "robot", "athena"]),
        ("线缆", &["cable", "tech", "basic", "repair"]),
        ("法术", &["magic", "spell", "casting"]),
        ("怪物", &["monster", "creature", "stat", "enemy"]),
        ("线索", &["clue", "lead", "revelation", "investigation"]),
    ];
    for (needle, aliases) in pairs {
        if input.contains(needle) {
            out.extend(aliases.iter().map(|s| s.to_string()));
        }
    }
    if lower.contains("hack") {
        out.extend(
            ["netrunning", "net", "architecture"]
                .iter()
                .map(|s| s.to_string()),
        );
    }
    if lower.contains("athena") {
        out.extend(
            ["drone", "hacking", "server", "behavior"]
                .iter()
                .map(|s| s.to_string()),
        );
    }
    if lower.contains("basic tech") {
        out.extend(
            ["tech", "repair", "dv", "skill"]
                .iter()
                .map(|s| s.to_string()),
        );
    }
    out.sort();
    out.dedup();
    out
}

fn quoted_query(q: &str) -> String {
    format!("\"{}\"", q.replace('"', " "))
}

fn matches_request(doc: &SearchDocument, req: &SearchRequest) -> bool {
    if !visibility_allowed(doc.visibility, &req.viewer) {
        return false;
    }
    if !req.domains.is_empty()
        && !req
            .domains
            .iter()
            .any(|d| normalize_facet_value(d) == normalize_facet_value(&doc.domain))
    {
        return false;
    }
    if !req.kinds.is_empty()
        && !req
            .kinds
            .iter()
            .any(|k| normalize_facet_value(k) == normalize_facet_value(&doc.logical_kind))
    {
        return false;
    }
    if !req.tags.is_empty() {
        let doc_tags = doc
            .tags
            .iter()
            .map(|t| normalize_facet_value(t))
            .collect::<HashSet<_>>();
        if !req
            .tags
            .iter()
            .all(|tag| doc_tags.contains(&normalize_facet_value(tag)))
        {
            return false;
        }
    }
    for (key, value) in &req.scopes {
        if !scope_matches(doc, key, value) {
            return false;
        }
    }
    let facets = doc.facets().into_iter().collect::<HashSet<_>>();
    for (key, values) in &req.filters {
        if values.is_empty() {
            continue;
        }
        let mut matched = false;
        for value in values {
            if doc.scopes.get(key).map(|v| normalize_facet_value(v))
                == Some(normalize_facet_value(value))
            {
                matched = true;
                break;
            }
            if facets.contains(&format!(
                "{}__{}",
                normalize_facet_value(key),
                normalize_facet_value(value)
            )) {
                matched = true;
                break;
            }
            if let Some(meta_value) = doc.metadata.get(key).and_then(Value::as_str) {
                if normalize_facet_value(meta_value) == normalize_facet_value(value) {
                    matched = true;
                    break;
                }
            }
        }
        if !matched {
            return false;
        }
    }
    true
}

fn scope_matches(doc: &SearchDocument, key: &str, value: &str) -> bool {
    let want = normalize_facet_value(value);
    if doc.scopes.get(key).map(|v| normalize_facet_value(v)) == Some(want.clone()) {
        return true;
    }
    if doc
        .scopes
        .get("path")
        .map(|v| normalize_facet_value(v).contains(&want))
        .unwrap_or(false)
    {
        return true;
    }
    match key {
        "ruleset_id" => {
            doc.scopes.get("scope_type").map(|v| v.as_str()) == Some("ruleset")
                && doc.scopes.get("scope_id").map(|v| normalize_facet_value(v)) == Some(want)
        }
        "module_id" => {
            doc.scopes.get("scope_type").map(|v| v.as_str()) == Some("module")
                && doc.scopes.get("scope_id").map(|v| normalize_facet_value(v)) == Some(want)
        }
        "session_id" => {
            doc.scopes.get("scope_type").map(|v| v.as_str()) == Some("session")
                && doc.scopes.get("scope_id").map(|v| normalize_facet_value(v)) == Some(want)
        }
        "scene_id" => {
            doc.scopes.get("scope_type").map(|v| v.as_str()) == Some("scene")
                && doc.scopes.get("scope_id").map(|v| normalize_facet_value(v)) == Some(want)
        }
        _ => doc
            .scopes
            .get("bundle_id")
            .map(|v| normalize_facet_value(v).contains(&want))
            .unwrap_or(false),
    }
}

fn visibility_allowed(visibility: Visibility, viewer: &VisibilityProfile) -> bool {
    match viewer.viewer_kind {
        ViewerKind::Gm | ViewerKind::System => true,
        ViewerKind::Player => matches!(visibility, Visibility::Public | Visibility::PlayerVisible),
        ViewerKind::Npc => matches!(
            visibility,
            Visibility::Public | Visibility::PlayerVisible | Visibility::NpcPrivate
        ),
    }
}

fn search_hit_from_doc(doc: &SearchDocument, score: f32, req: &SearchRequest) -> SearchHit {
    SearchHit {
        hit_id: format!("hit_{}", Uuid::new_v4().simple()),
        search_doc_id: doc.search_doc_id.clone(),
        origin: doc.origin.clone(),
        domain: doc.domain.clone(),
        logical_kind: doc.logical_kind.clone(),
        title: doc.title.clone(),
        snippet: make_snippet(&doc.body, &req.query, 520),
        score,
        scopes: doc.scopes.clone(),
        tags: doc.tags.clone(),
        visibility: doc.visibility,
        source_refs: doc.source_refs.clone(),
        metadata: doc.metadata.clone(),
        explain: if req.explain {
            json!({"score": score, "facets": doc.facets(), "updated_at": doc.updated_at})
        } else {
            json!({})
        },
    }
}

fn make_snippet(body: &str, query: &str, max_chars: usize) -> String {
    let body = body.trim();
    if body.len() <= max_chars {
        return body.to_string();
    }
    let lowered = body.to_lowercase();
    let terms = query
        .split_whitespace()
        .map(|s| {
            s.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|s| s.len() > 2)
        .collect::<Vec<_>>();
    let pos = terms
        .iter()
        .filter_map(|t| lowered.find(t))
        .min()
        .unwrap_or(0);
    let start = pos.saturating_sub(max_chars / 3);
    let mut end = (start + max_chars).min(body.len());
    while end < body.len() && !body.is_char_boundary(end) {
        end += 1;
    }
    let mut start = start;
    while start > 0 && !body.is_char_boundary(start) {
        start -= 1;
    }
    let prefix = if start > 0 { "…" } else { "" };
    let suffix = if end < body.len() { "…" } else { "" };
    format!("{}{}{}", prefix, &body[start..end], suffix)
}

fn add_scope_hints_from_path(rel: &str, scopes: &mut BTreeMap<String, String>) {
    let normalized_path = rel.replace('\\', "/");
    let stem = Path::new(&normalized_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(normalize_facet_value);
    if let Some(stem) = stem {
        if normalized_path.contains("markdown/modules/") || normalized_path.contains("modules/") {
            scopes.entry("module_id".into()).or_insert(stem);
        } else if normalized_path.contains("markdown/rulebooks/")
            || normalized_path.contains("rulebooks/")
        {
            scopes.entry("ruleset_id".into()).or_insert(stem);
        }
    }
}

fn relative_to_data_dir(data_dir: &Path, configured: &str) -> PathBuf {
    let path = PathBuf::from(configured);
    if path.is_absolute() {
        path
    } else {
        data_dir.join(path)
    }
}

fn chunk_text(text: &str, max_chars: usize) -> Vec<String> {
    if text.len() <= max_chars {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut start = 0usize;
    while start < text.len() {
        let mut end = (start + max_chars).min(text.len());
        while end < text.len() && !text.is_char_boundary(end) {
            end += 1;
        }
        let chunk = text[start..end].trim();
        if !chunk.is_empty() {
            chunks.push(chunk.to_string());
        }
        if end == text.len() {
            break;
        }
        start = end.saturating_sub(800);
        while start > 0 && !text.is_char_boundary(start) {
            start -= 1;
        }
    }
    chunks
}

fn jsonl_body_text(value: &Value) -> String {
    // For semantic-source JSONL, index the actual source unit text first. Pretty-printed JSON is
    // a last resort because it pollutes retrieval with schema keys instead of rules/module evidence.
    for key in ["content_text", "full_text", "text", "summary"] {
        if let Some(s) = value.get(key).and_then(Value::as_str) {
            if !s.trim().is_empty() {
                return s.to_string();
            }
        }
    }
    if let Some(content) = value.get("content") {
        if let Some(s) = content.get("value").and_then(Value::as_str) {
            return s.to_string();
        }
        if let Some(s) = content.get("text").and_then(Value::as_str) {
            return s.to_string();
        }
        return serde_json::to_string_pretty(content).unwrap_or_default();
    }
    serde_json::to_string_pretty(value).unwrap_or_default()
}

fn visibility_from_str_local(s: &str) -> Visibility {
    match s {
        "public" => Visibility::Public,
        "player_visible" => Visibility::PlayerVisible,
        "npc_private" => Visibility::NpcPrivate,
        "system_only" => Visibility::SystemOnly,
        _ => Visibility::GmOnly,
    }
}

fn stability_from_str_local(s: &str) -> Stability {
    match s {
        "immutable" => Stability::Immutable,
        "scene_stable" => Stability::SceneStable,
        "turn_dynamic" => Stability::TurnDynamic,
        "ephemeral" => Stability::Ephemeral,
        _ => Stability::RarelyChanged,
    }
}

/// One matched aligned-table row from a duotext `.layout.md` sidecar.
#[derive(Debug, Clone, Serialize)]
pub struct LayoutTableHit {
    pub source_id: String,
    pub source_kind: String, // "rulebooks" | "modules"
    pub page: Option<u32>,
    pub row: String,
    pub context: Vec<String>,
    pub column_score: usize,
}

impl SearchService {
    /// Grep the duotext `.layout.md` sidecars (aligned tables) for a query — exact param lookup
    /// (e.g. a weapon name -> its full stat row). Complements Tantivy semantic search.
    pub fn grep_layout_tables(&self, query: &str, max_hits: usize) -> Vec<LayoutTableHit> {
        grep_layout_tables_in(&self.config.data_dir, query, max_hits)
    }
}

fn layout_column_gaps(line: &str) -> usize {
    let (mut gaps, mut run) = (0usize, 0usize);
    for c in line.chars() {
        if c == ' ' {
            run += 1;
        } else {
            if run >= 2 {
                gaps += 1;
            }
            run = 0;
        }
    }
    if run >= 2 {
        gaps += 1;
    }
    gaps
}

/// Scan the duotext markdown(s) `<data_dir>/markdown/{rulebooks,modules}/*.md` for lines containing
/// ALL query tokens (case-insensitive). In single-file duotext the aligned table rows live on the
/// table pages of the main `.md`; they have more column gaps and are ranked first. Pure file grep.
pub fn grep_layout_tables_in(
    data_dir: &std::path::Path,
    query: &str,
    max_hits: usize,
) -> Vec<LayoutTableHit> {
    let tokens: Vec<String> = query
        .split_whitespace()
        .filter(|t| t.chars().count() >= 2)
        .map(|t| t.to_lowercase())
        .collect();
    if tokens.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<LayoutTableHit> = Vec::new();
    for kind in ["rulebooks", "modules"] {
        let dir = data_dir.join("markdown").join(kind);
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut files: Vec<std::path::PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .map(|n| {
                        let n = n.to_string_lossy();
                        n.ends_with(".md") && !n.ends_with(".layout.md")
                    })
                    .unwrap_or(false)
            })
            .collect();
        files.sort();
        for p in files {
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            let source_id = p
                .file_name()
                .map(|n| n.to_string_lossy().trim_end_matches(".md").to_string())
                .unwrap_or_default();
            let lines: Vec<&str> = text.lines().collect();
            let mut cur_page: Option<u32> = None;
            for (i, raw) in lines.iter().enumerate() {
                if let Some(pg) = raw
                    .trim()
                    .strip_prefix("<!-- source_id=")
                    .and_then(|s| s.split("page=").nth(1))
                    .and_then(|s| s.split_whitespace().next())
                    .and_then(|s| s.parse::<u32>().ok())
                {
                    cur_page = Some(pg);
                    continue;
                }
                let line = raw.trim_end();
                let trimmed = line.trim();
                if trimmed.len() < 4 || trimmed.starts_with("# Page") || trimmed.starts_with("<!--")
                {
                    continue;
                }
                let low = line.to_lowercase();
                if tokens.iter().all(|t| low.contains(t.as_str())) {
                    let ctx_start = i.saturating_sub(2);
                    let context: Vec<String> = lines[ctx_start..i]
                        .iter()
                        .map(|s| s.trim_end().to_string())
                        .filter(|s| {
                            let t = s.trim();
                            !t.is_empty() && !t.starts_with("<!--") && !t.starts_with("# Page")
                        })
                        .collect();
                    hits.push(LayoutTableHit {
                        source_id: source_id.clone(),
                        source_kind: kind.to_string(),
                        page: cur_page,
                        row: line.to_string(),
                        context,
                        column_score: layout_column_gaps(line),
                    });
                }
            }
        }
    }
    hits.sort_by(|a, b| b.column_score.cmp(&a.column_score));
    hits.truncate(max_hits);
    hits
}

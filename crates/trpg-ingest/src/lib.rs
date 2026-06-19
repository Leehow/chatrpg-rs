use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tokio::process::Command;
use tracing::warn;
use trpg_model::*;
use uuid::Uuid;
use walkdir::WalkDir;

#[derive(Debug, Clone)]
pub struct IngestConfig {
    pub data_dir: PathBuf,
    pub parse_config_hash: String,
    pub force_markdown: bool,
    pub pdf_backend: PdfBackendPreference,
    pub clean_mode: ExtractionCleanMode,
    pub chunk_target_chars: usize,
    pub write_raw_chunk_sidecar: bool,
}

impl IngestConfig {
    pub fn new(data_dir: impl Into<PathBuf>, parse_config_hash: impl Into<String>) -> Self {
        Self {
            data_dir: data_dir.into(),
            parse_config_hash: parse_config_hash.into(),
            force_markdown: false,
            pdf_backend: PdfBackendPreference::from_env(),
            clean_mode: ExtractionCleanMode::from_env(),
            chunk_target_chars: std::env::var("TRPG_OXIDIZE_CHUNK_TARGET_CHARS")
                .or_else(|_| std::env::var("TRPG_OXIDIZE_CHUNK_SIZE"))
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(4_000),
            write_raw_chunk_sidecar: std::env::var("TRPG_OXIDIZE_WRITE_RAW_CHUNKS")
                .or_else(|_| std::env::var("TRPG_OXIDIZE_WRITE_CHUNKS"))
                .ok()
                .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
                .unwrap_or(true),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PdfExtractionResult {
    pub source_document: SourceDocument,
    pub book: PlainTextBook,
    pub markdown_path: PathBuf,
    pub raw_chunks_path: Option<PathBuf>,
}

#[async_trait]
pub trait PdfMarkdownExtractor: Send + Sync {
    async fn extract(
        &self,
        pdf_path: &Path,
        source_kind: SourceKind,
        config: &IngestConfig,
    ) -> Result<PdfExtractionResult>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PdfBackendKind {
    Oxidize,
    Pdftotext,
    Mineru,
    Auto,
}

impl PdfBackendKind {
    pub fn from_env() -> Self {
        // Default "duotext": our two-view pdftotext strategy — default reading-order mode for clean
        // prose (indexed) + a -layout sidecar with aligned tables (param grep). Fast, pure-CPU, no
        // ligature loss. ("pdftotext"/"poppler" are legacy aliases for the same backend.) mineru
        // (vision) handles 2-col order natively but is ~500x slower and row-shifts dense tables.
        match std::env::var("TRPG_PDF_BACKEND")
            .unwrap_or_else(|_| "duotext".to_string())
            .to_ascii_lowercase()
            .as_str()
        {
            "duotext" | "pdftotext" | "poppler" => Self::Pdftotext,
            "oxidize" | "oxidize-pdf" => Self::Oxidize,
            "mineru" => Self::Mineru,
            _ => Self::Auto,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Oxidize => "oxidize",
            Self::Pdftotext => "duotext",
            Self::Mineru => "mineru",
            Self::Auto => "auto",
        }
    }
}

pub type PdfBackendPreference = PdfBackendKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractionCleanMode {
    Raw,
    Heuristic,
    LlmReady,
}

impl ExtractionCleanMode {
    pub fn from_env() -> Self {
        match std::env::var("TRPG_OXIDIZE_CLEAN_MODE")
            .unwrap_or_else(|_| "heuristic".to_string())
            .to_ascii_lowercase()
            .as_str()
        {
            "raw" => Self::Raw,
            "llm" | "llm_ready" | "llm-ready" => Self::LlmReady,
            _ => Self::Heuristic,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Heuristic => "heuristic",
            Self::LlmReady => "llm_ready",
        }
    }
}

#[derive(Debug, Clone)]
pub struct HybridPdfBackend {
    pub backend: PdfBackendKind,
}

impl Default for HybridPdfBackend {
    fn default() -> Self {
        Self {
            backend: PdfBackendKind::from_env(),
        }
    }
}

#[async_trait]
impl PdfMarkdownExtractor for HybridPdfBackend {
    async fn extract(
        &self,
        pdf_path: &Path,
        source_kind: SourceKind,
        config: &IngestConfig,
    ) -> Result<PdfExtractionResult> {
        match self.backend {
            PdfBackendKind::Oxidize => {
                OxidizePdfBackend
                    .extract(pdf_path, source_kind, config)
                    .await
            }
            PdfBackendKind::Pdftotext => {
                PdftotextBackend
                    .extract(pdf_path, source_kind, config)
                    .await
            }
            PdfBackendKind::Mineru => {
                MineruPdfBackend
                    .extract(pdf_path, source_kind, config)
                    .await
            }
            PdfBackendKind::Auto => match OxidizePdfBackend
                .extract(pdf_path, source_kind.clone(), config)
                .await
            {
                Ok(result) => Ok(result),
                Err(err) => {
                    warn!(path = %pdf_path.display(), error = %err, "oxidize-pdf failed; falling back to pdftotext");
                    PdftotextBackend
                        .extract(pdf_path, source_kind, config)
                        .await
                }
            },
        }
    }
}

pub fn default_pdf_extractor() -> Arc<dyn PdfMarkdownExtractor> {
    Arc::new(AutoPdfBackend)
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AutoPdfBackend;

#[async_trait]
impl PdfMarkdownExtractor for AutoPdfBackend {
    async fn extract(
        &self,
        pdf_path: &Path,
        source_kind: SourceKind,
        config: &IngestConfig,
    ) -> Result<PdfExtractionResult> {
        HybridPdfBackend {
            backend: config.pdf_backend,
        }
        .extract(pdf_path, source_kind, config)
        .await
    }
}

#[derive(Debug, Clone, Default)]
pub struct OxidizePdfBackend;

#[async_trait]
impl PdfMarkdownExtractor for OxidizePdfBackend {
    async fn extract(
        &self,
        pdf_path: &Path,
        source_kind: SourceKind,
        config: &IngestConfig,
    ) -> Result<PdfExtractionResult> {
        let source_hash = file_sha256(pdf_path).await?;
        let source_id = source_id_from_path(pdf_path);
        let title = title_from_path(pdf_path);
        let markdown_dir = markdown_dir_for(&config.data_dir, &source_kind);
        let chunks_dir = config.data_dir.join("parsed/source_chunks");
        fs::create_dir_all(&markdown_dir).await?;
        fs::create_dir_all(&chunks_dir).await?;

        let markdown_path = markdown_dir.join(format!("{source_id}.md"));
        let raw_chunks_path = chunks_dir.join(format!("{source_id}.oxidize_chunks.jsonl"));
        let cleanup_manifest_path = chunks_dir.join(format!("{source_id}.cleanup_manifest.json"));

        if !config.force_markdown && markdown_path.exists() && raw_chunks_path.exists() {
            let markdown_header = fs::read_to_string(&markdown_path).await.unwrap_or_default();
            if markdown_header.contains(&format!("source_hash: {source_hash}"))
                && markdown_header.contains("extractor: oxidize-pdf")
            {
                let chunks = read_chunks_jsonl(&raw_chunks_path)
                    .await
                    .unwrap_or_default();
                let pages = if chunks.is_empty() {
                    read_markdown_pages(&markdown_path).await?
                } else {
                    pages_from_chunks(&chunks)
                };
                let book = PlainTextBook {
                    source_id: source_id.clone(),
                    title: title.clone(),
                    source_hash: source_hash.clone(),
                    pages,
                    chunks,
                };
                let doc = SourceDocument {
                    id: Uuid::new_v4(),
                    source_id,
                    source_kind,
                    title,
                    file_path: pdf_path.to_string_lossy().to_string(),
                    markdown_path: Some(markdown_path.to_string_lossy().to_string()),
                    source_hash,
                    parse_config_hash: config.parse_config_hash.clone(),
                    metadata: json!({
                        "extractor":"oxidize-pdf",
                        "cached_markdown": true,
                        "raw_chunks_path": raw_chunks_path.to_string_lossy().to_string(),
                        "cleanup_manifest_path": cleanup_manifest_path.to_string_lossy().to_string(),
                        "cleaning_status": config.clean_mode.as_str(),
                    }),
                };
                return Ok(PdfExtractionResult {
                    source_document: doc,
                    book,
                    markdown_path,
                    raw_chunks_path: Some(raw_chunks_path),
                });
            }
        }

        let chunks = extract_oxidize_chunks_blocking(
            pdf_path.to_path_buf(),
            source_id.clone(),
            config.clean_mode,
        )
        .await?;
        if config.write_raw_chunk_sidecar {
            write_chunks_jsonl(&raw_chunks_path, &chunks).await?;
            write_cleanup_manifest(&cleanup_manifest_path, &source_id, &source_hash, &chunks)
                .await?;
        }
        let pages = pages_from_chunks(&chunks);
        let markdown = render_oxidize_markdown(
            &source_id,
            &title,
            &source_hash,
            &chunks,
            Some(&raw_chunks_path),
            Some(&cleanup_manifest_path),
            "oxidize-pdf",
        );
        fs::write(&markdown_path, markdown).await?;

        let cleanup_needed = chunks
            .iter()
            .filter(|c| c.clean_status.as_deref() == Some("needs_llm_cleanup"))
            .count();
        let book = PlainTextBook {
            source_id: source_id.clone(),
            title: title.clone(),
            source_hash: source_hash.clone(),
            pages,
            chunks,
        };
        let doc = SourceDocument {
            id: Uuid::new_v4(),
            source_id,
            source_kind,
            title,
            file_path: pdf_path.to_string_lossy().to_string(),
            markdown_path: Some(markdown_path.to_string_lossy().to_string()),
            source_hash,
            parse_config_hash: config.parse_config_hash.clone(),
            metadata: json!({
                "extractor":"oxidize-pdf",
                "cached_markdown": false,
                "raw_chunks_path": raw_chunks_path.to_string_lossy().to_string(),
                "cleanup_manifest_path": cleanup_manifest_path.to_string_lossy().to_string(),
                "chunk_count": book.chunks.len(),
                "cleanup_needed_count": cleanup_needed,
                "cleaning_status": config.clean_mode.as_str(),
            }),
        };
        Ok(PdfExtractionResult {
            source_document: doc,
            book,
            markdown_path,
            raw_chunks_path: Some(raw_chunks_path),
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct PdftotextBackend;

#[async_trait]
impl PdfMarkdownExtractor for PdftotextBackend {
    async fn extract(
        &self,
        pdf_path: &Path,
        source_kind: SourceKind,
        config: &IngestConfig,
    ) -> Result<PdfExtractionResult> {
        let source_hash = file_sha256(pdf_path).await?;
        let source_id = source_id_from_path(pdf_path);
        let title = title_from_path(pdf_path);
        let markdown_dir = markdown_dir_for(&config.data_dir, &source_kind);
        fs::create_dir_all(&markdown_dir).await?;
        let markdown_path = markdown_dir.join(format!("{source_id}.md"));
        // duotext = ONE markdown. Per page: table-heavy pages keep -layout COLUMN ALIGNMENT;
        // prose pages use reading-order mode (clean multi-column prose, intact glyphs). Both the
        // Tantivy index and the param grep read this single file (no sidecar).
        let legacy_sidecar = markdown_dir.join(format!("{source_id}.layout.md"));

        if !config.force_markdown && markdown_path.exists() {
            let markdown_header = fs::read_to_string(&markdown_path).await.unwrap_or_default();
            if markdown_header.contains(&format!("source_hash: {source_hash}"))
                && markdown_header.contains("extractor: duotext2")
            {
                let pages = read_markdown_pages(&markdown_path).await?;
                let book = PlainTextBook {
                    source_id: source_id.clone(),
                    title: title.clone(),
                    source_hash: source_hash.clone(),
                    pages,
                    chunks: vec![],
                };
                let doc = SourceDocument {
                    id: Uuid::new_v4(),
                    source_id,
                    source_kind,
                    title,
                    file_path: pdf_path.to_string_lossy().to_string(),
                    markdown_path: Some(markdown_path.to_string_lossy().to_string()),
                    source_hash,
                    parse_config_hash: config.parse_config_hash.clone(),
                    metadata: json!({"extractor":"duotext2", "cached_markdown": true}),
                };
                return Ok(PdfExtractionResult {
                    source_document: doc,
                    book,
                    markdown_path,
                    raw_chunks_path: None,
                });
            }
        }

        // Run BOTH pdftotext passes concurrently (independent subprocesses) — wall-time = max, not sum.
        let (main_raw, layout_raw) = tokio::try_join!(
            run_pdftotext(pdf_path, &[]),
            run_pdftotext(pdf_path, &["-layout"]),
        )?;
        // Merge per page: table pages keep -layout alignment, prose pages use reading order.
        let reading_pages = split_pdftotext_pages(&main_raw);
        let layout_pages = split_pdftotext_pages_keep_layout(&layout_raw);
        let pages = merge_duotext_pages(reading_pages, layout_pages);
        let markdown =
            render_page_anchored_markdown(&source_id, &title, &source_hash, &pages, "duotext2");
        fs::write(&markdown_path, markdown).await?;
        let _ = fs::remove_file(&legacy_sidecar).await; // remove any old two-file sidecar

        let book = PlainTextBook {
            source_id: source_id.clone(),
            title: title.clone(),
            source_hash: source_hash.clone(),
            pages,
            chunks: vec![],
        };
        let doc = SourceDocument {
            id: Uuid::new_v4(),
            source_id,
            source_kind,
            title,
            file_path: pdf_path.to_string_lossy().to_string(),
            markdown_path: Some(markdown_path.to_string_lossy().to_string()),
            source_hash,
            parse_config_hash: config.parse_config_hash.clone(),
            metadata: json!({"extractor":"duotext2", "cached_markdown": false}),
        };
        Ok(PdfExtractionResult {
            source_document: doc,
            book,
            markdown_path,
            raw_chunks_path: None,
        })
    }
}

fn duotext_line_gaps(line: &str) -> usize {
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

/// A page is "table-heavy" if its -layout text has several multi-column rows (>=2 column gaps).
/// 2-column PROSE has only ~1 gutter gap per line, so it stays prose; real tables have >=2.
fn is_duotext_table_page(layout_text: &str) -> bool {
    let (mut multi2, mut multi4) = (0usize, 0usize);
    for line in layout_text.lines() {
        let g = duotext_line_gaps(line);
        if g >= 2 {
            multi2 += 1;
        }
        if g >= 4 {
            multi4 += 1;
        }
    }
    multi2 >= 6 || multi4 >= 3
}

/// 列感知去栏：从 -layout 文本里按"空白河"检出 ≥2 个列，把每列从上到下读出、
/// 列间左→右拼接，消除 pdftotext reading-order 把多栏逐行交错的问题。
/// 检不出可信的多栏（单栏/短页/异常）→ 返回 None，调用方回退 reading-order。
/// 通用：纯几何，无任何规则/语言专有逻辑。
fn decolumnize_layout_page(layout_text: &str) -> Option<String> {
    // char 计数下限。汉字在 -layout 里视觉宽约 2 列却只算 1 char：真实窄栏 CJK 双栏页
    // （各栏约 10 汉字 + gutter）char 宽仅 ~21，远低于拉丁页。阈值取得低，只当作廉价
    // 预过滤挡空/极短页；真正的可信度护栏是下方"切分后每列都须有非空内容"。
    const MIN_PAGE_WIDTH: usize = 16;
    let lines: Vec<Vec<char>> = layout_text.lines().map(|l| l.chars().collect()).collect();
    let body: Vec<&Vec<char>> = lines
        .iter()
        .filter(|l| l.iter().any(|c| !c.is_whitespace()))
        .collect();
    if body.len() < 5 {
        return None;
    }
    let width = body.iter().map(|l| l.len()).max().unwrap_or(0);
    if width < MIN_PAGE_WIDTH {
        return None;
    }
    // 每个字符列：有多少 body 行在此处是空格（或更短）。
    let mut blank = vec![0usize; width];
    for l in &body {
        for c in 0..width {
            if l.get(c).map(|ch| *ch == ' ').unwrap_or(true) {
                blank[c] += 1;
            }
        }
    }
    let n = body.len();
    let is_gutter = |c: usize| blank[c] * 100 >= n * 90; // ≥90% 行此列为空 = 河
                                                         // 找连续河带，内部、宽度 ≥3 的取中点作为切分位。
    let mut splits: Vec<usize> = Vec::new();
    let mut c = 0;
    while c < width {
        if is_gutter(c) {
            let start = c;
            while c < width && is_gutter(c) {
                c += 1;
            }
            if c - start >= 3 && start > 2 && c < width - 2 {
                splits.push((start + c) / 2);
            }
        } else {
            c += 1;
        }
    }
    if splits.is_empty() {
        return None;
    }
    // 列边界。
    let mut bounds = vec![0usize];
    bounds.extend(splits);
    bounds.push(width);
    // 固定列窗切片：依赖 pdftotext -layout 用前导空格补位保持每行的 x 位置——
    // 实测（血色公路 p17 等）右栏行即便左栏整行为空也不会左移到第 0 列，仍停在其原始
    // 列区间，故按固定 cs..ce 切片正确（无 ragged 左移错填）。详见 decolumn_tests
    // ragged 回归测试。注意：当某行左栏文本异常长、与短右栏片段仅隔单空格时仍会少量
    // 串栏，这是 -layout 单空格 gutter 的固有歧义，非左移，且 fail-closed 下影响极小。
    let mut out = String::new();
    let mut cols: Vec<String> = Vec::new();
    for w in bounds.windows(2) {
        let (cs, ce) = (w[0], w[1]);
        let mut col = String::new();
        for l in &lines {
            // 用全部行（含空行）保留段落断点
            let seg: String = (cs..ce.min(l.len())).map(|i| l[i]).collect();
            let seg = seg.trim();
            col.push_str(seg);
            col.push('\n');
        }
        cols.push(col.trim().to_string());
    }
    // 可信度护栏：切分得到的每一列都必须有非空内容。任一列全空 = 把单栏误切成两半，
    // 不可信 → fail-closed 返回 None，调用方回退 reading-order。
    if cols.iter().any(|c| c.is_empty()) {
        return None;
    }
    for col in cols {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&col);
    }
    if out.trim().is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Merge reading-order pages (prose) with layout pages (aligned tables) into ONE page set:
/// table-heavy pages keep -layout alignment; everything else uses the clean reading-order text.
fn merge_duotext_pages(reading: Vec<PageText>, layout: Vec<PageText>) -> Vec<PageText> {
    let read_map: BTreeMap<u32, String> = reading.into_iter().map(|p| (p.page, p.text)).collect();
    let lay_map: BTreeMap<u32, String> = layout.into_iter().map(|p| (p.page, p.text)).collect();
    let mut nums: Vec<u32> = read_map.keys().chain(lay_map.keys()).copied().collect();
    nums.sort();
    nums.dedup();
    nums.into_iter()
        .filter_map(|pg| {
            let text = match (lay_map.get(&pg), read_map.get(&pg)) {
                // 表页：保留 -layout 列对齐（不变）
                (Some(l), _) if is_duotext_table_page(l) => l.clone(),
                // 散文页：优先从 -layout 去栏（消除交错）；检不出 → 回退 reading-order
                (Some(l), Some(r)) => decolumnize_layout_page(l).unwrap_or_else(|| r.clone()),
                (Some(l), None) => decolumnize_layout_page(l).unwrap_or_else(|| l.clone()),
                (None, Some(r)) => r.clone(),
                (None, None) => return None,
            };
            Some(PageText { page: pg, text })
        })
        .collect()
}

/// Run `pdftotext [extra...] -enc UTF-8 <pdf> -` and return stdout (CR-stripped).
/// extra=`[]` => default reading-order mode (clean prose); extra=`["-layout"]` => aligned tables.
async fn run_pdftotext(pdf_path: &Path, extra: &[&str]) -> Result<String> {
    let mut cmd = Command::new("pdftotext");
    for a in extra {
        cmd.arg(a);
    }
    let output = cmd
        .arg("-enc")
        .arg("UTF-8")
        .arg(pdf_path)
        .arg("-")
        .output()
        .await
        .with_context(|| {
            "failed to run pdftotext; install poppler-utils or set TRPG_PDF_BACKEND=oxidize/auto"
        })?;
    if !output.status.success() {
        return Err(anyhow!(
            "pdftotext failed for {}: {}",
            pdf_path.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).replace('\r', ""))
}

async fn extract_oxidize_chunks_blocking(
    pdf_path: PathBuf,
    source_id: String,
    clean_mode: ExtractionCleanMode,
) -> Result<Vec<DocumentChunk>> {
    tokio::task::spawn_blocking(move || -> Result<Vec<DocumentChunk>> {
        use oxidize_pdf::parser::PdfDocument;
        let doc = PdfDocument::open(&pdf_path)
            .with_context(|| format!("oxidize-pdf failed to open {}", pdf_path.display()))?;
        let raw_chunks = doc
            .rag_chunks()
            .with_context(|| format!("oxidize-pdf rag_chunks failed for {}", pdf_path.display()))?;
        let mut chunks = Vec::new();
        for raw in raw_chunks {
            let raw_text = raw.text.clone();
            let raw_full_text = raw.full_text.clone();
            let page_numbers = raw
                .page_numbers
                .iter()
                .map(|p| *p as u32)
                .collect::<Vec<_>>();
            let cleanup_reasons = detect_oxidize_cleanup_reasons(
                &raw_text,
                &raw_full_text,
                raw.is_oversized,
                raw.heading_context.as_deref(),
                raw.token_estimate as usize,
            );
            let text = match clean_mode {
                ExtractionCleanMode::Raw => raw_text,
                ExtractionCleanMode::Heuristic | ExtractionCleanMode::LlmReady => {
                    clean_extracted_spacing(&raw_text)
                }
            };
            let full_text = match clean_mode {
                ExtractionCleanMode::Raw => raw_full_text,
                ExtractionCleanMode::Heuristic | ExtractionCleanMode::LlmReady => {
                    clean_extracted_spacing(&raw_full_text)
                }
            };
            let heading_context = raw
                .heading_context
                .as_ref()
                .map(|h| split_heading_context(h, clean_mode))
                .unwrap_or_default();
            let bounding_boxes = raw
                .bounding_boxes
                .iter()
                .map(|bbox| {
                    let page = page_numbers.first().copied();
                    DocumentBoundingBox {
                        page,
                        x0: bbox.x as f32,
                        y0: bbox.y as f32,
                        x1: (bbox.x + bbox.width) as f32,
                        y1: (bbox.y + bbox.height) as f32,
                    }
                })
                .collect::<Vec<_>>();
            let chunk_id = format!("{}.chunk_{:05}", source_id, raw.chunk_index as u32);
            let hash_body = if full_text.trim().is_empty() {
                &text
            } else {
                &full_text
            };
            let text_hash = sha256_hex(hash_body);
            let clean_status = if cleanup_reasons.is_empty() {
                match clean_mode {
                    ExtractionCleanMode::Raw => "raw_ok",
                    ExtractionCleanMode::Heuristic => "heuristic_cleaned",
                    ExtractionCleanMode::LlmReady => "llm_ready",
                }
            } else {
                "needs_llm_cleanup"
            };
            chunks.push(DocumentChunk {
                chunk_id,
                text,
                full_text,
                page_numbers,
                element_types: raw.element_types.clone(),
                heading_context,
                token_estimate: Some(raw.token_estimate as u32),
                is_oversized: raw.is_oversized,
                bounding_boxes,
                text_hash: Some(text_hash),
                clean_status: Some(clean_status.to_string()),
                metadata: json!({
                    "extractor": "oxidize-pdf",
                    "cleanup_reasons": cleanup_reasons,
                    "raw_chunk_index": raw.chunk_index,
                }),
            });
        }
        Ok(chunks)
    })
    .await?
}

/// Path to the local MinerU wrapper (skill). Override with TRPG_MINERU_WRAPPER.
fn mineru_wrapper_path() -> String {
    std::env::var("TRPG_MINERU_WRAPPER").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{home}/.claude/skills/mineru/scripts/parse.sh")
    })
}

/// Run MinerU (vision layout+table model) on the whole PDF and convert its
/// per-block output (content_list.json) into engine DocumentChunks — one chunk
/// per page, tables kept as clean HTML inline. This is the source of CLEAN
/// tables that oxidize collapses. Heavy (vision model) — intended for the async
/// background upgrade path, not the fast first-pass.
fn extract_mineru_chunks_blocking(
    pdf_path: PathBuf,
    source_id: String,
) -> Result<Vec<DocumentChunk>> {
    // Per-process tmp dir so two parse-all runs of the same book never clobber each other's
    // MinerU output (the source_id alone is not unique across concurrent invocations).
    let tmp = std::env::temp_dir().join(format!("trpg_mineru_{source_id}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let wrapper = mineru_wrapper_path();
    let out = std::process::Command::new("bash")
        .arg(&wrapper)
        .arg("-p")
        .arg(&pdf_path)
        .arg("-o")
        .arg(&tmp)
        .arg("--plain")
        .arg("--")
        .arg("-m")
        .arg("auto")
        .arg("-f")
        .arg("false")
        .output()
        .with_context(|| format!("failed to spawn MinerU wrapper {wrapper}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "MinerU failed: {}",
            String::from_utf8_lossy(&out.stderr)
                .chars()
                .rev()
                .take(600)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>()
        );
    }
    let cl = WalkDir::new(&tmp)
        .into_iter()
        .filter_map(|e| e.ok())
        .map(|e| e.into_path())
        .find(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().ends_with("content_list.json"))
                .unwrap_or(false)
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "MinerU produced no content_list.json under {}",
                tmp.display()
            )
        })?;
    let blocks: Vec<serde_json::Value> = serde_json::from_str(&std::fs::read_to_string(&cl)?)?;
    let mut by_page: std::collections::BTreeMap<i64, (Vec<String>, Vec<String>)> =
        std::collections::BTreeMap::new();
    for b in &blocks {
        let page = b.get("page_idx").and_then(|v| v.as_i64()).unwrap_or(0);
        let ty = b.get("type").and_then(|v| v.as_str()).unwrap_or("text");
        let text = match ty {
            "table" => {
                let cap = b
                    .get("table_caption")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str())
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default();
                let body = b.get("table_body").and_then(|v| v.as_str()).unwrap_or("");
                format!("{cap}\n{body}").trim().to_string()
            }
            "image" => String::new(),
            _ => b
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        };
        if text.trim().is_empty() {
            continue;
        }
        let e = by_page.entry(page).or_default();
        e.0.push(text);
        if !e.1.contains(&ty.to_string()) {
            e.1.push(ty.to_string());
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    let mut chunks = Vec::new();
    for (page, (parts, types)) in by_page {
        let text = parts.join("\n\n");
        chunks.push(DocumentChunk {
            chunk_id: format!("{source_id}.mineru.p{page}"),
            full_text: text.clone(),
            page_numbers: vec![(page + 1) as u32],
            element_types: types,
            heading_context: vec![],
            token_estimate: Some((text.len() / 4) as u32),
            is_oversized: false,
            bounding_boxes: vec![],
            text_hash: Some(sha256_hex(text.as_bytes())),
            clean_status: Some("mineru_pipeline".into()),
            metadata: json!({"extractor": "mineru"}),
            text,
        });
    }
    Ok(chunks)
}

/// MinerU PDF backend: clean tables via the vision model. Heavy; use as the
/// async background upgrade (oxidize is the fast first pass).
#[derive(Debug, Clone, Default)]
pub struct MineruPdfBackend;

#[async_trait]
impl PdfMarkdownExtractor for MineruPdfBackend {
    async fn extract(
        &self,
        pdf_path: &Path,
        source_kind: SourceKind,
        config: &IngestConfig,
    ) -> Result<PdfExtractionResult> {
        let source_hash = file_sha256(pdf_path).await?;
        let source_id = source_id_from_path(pdf_path);
        let title = title_from_path(pdf_path);
        let markdown_dir = markdown_dir_for(&config.data_dir, &source_kind);
        let chunks_dir = config.data_dir.join("parsed/source_chunks");
        fs::create_dir_all(&markdown_dir).await?;
        fs::create_dir_all(&chunks_dir).await?;
        let markdown_path = markdown_dir.join(format!("{source_id}.md"));
        let raw_chunks_path = chunks_dir.join(format!("{source_id}.oxidize_chunks.jsonl"));
        let cleanup_manifest_path = chunks_dir.join(format!("{source_id}.cleanup_manifest.json"));

        // Cached-reuse: MinerU is SLOW (~14min/book). Once the clean markdown + chunk sidecar
        // exist for this exact source_hash, reuse them so test re-parses don't re-run the vision
        // model. Mirrors OxidizePdfBackend; re-extract only with --force.
        if !config.force_markdown && markdown_path.exists() && raw_chunks_path.exists() {
            let markdown_header = fs::read_to_string(&markdown_path).await.unwrap_or_default();
            if markdown_header.contains(&format!("source_hash: {source_hash}"))
                && markdown_header.contains("extractor: mineru")
            {
                let chunks = read_chunks_jsonl(&raw_chunks_path)
                    .await
                    .unwrap_or_default();
                let pages = if chunks.is_empty() {
                    read_markdown_pages(&markdown_path).await?
                } else {
                    pages_from_chunks(&chunks)
                };
                let book = PlainTextBook {
                    source_id: source_id.clone(),
                    title: title.clone(),
                    source_hash: source_hash.clone(),
                    pages,
                    chunks,
                };
                let doc = SourceDocument {
                    id: Uuid::new_v4(),
                    source_id,
                    source_kind,
                    title,
                    file_path: pdf_path.to_string_lossy().to_string(),
                    markdown_path: Some(markdown_path.to_string_lossy().to_string()),
                    source_hash,
                    parse_config_hash: config.parse_config_hash.clone(),
                    metadata: json!({"extractor":"mineru", "cached_markdown": true, "raw_chunks_path": raw_chunks_path.to_string_lossy().to_string()}),
                };
                return Ok(PdfExtractionResult {
                    source_document: doc,
                    book,
                    markdown_path,
                    raw_chunks_path: Some(raw_chunks_path),
                });
            }
        }

        let pdf_buf = pdf_path.to_path_buf();
        let sid = source_id.clone();
        let chunks =
            tokio::task::spawn_blocking(move || extract_mineru_chunks_blocking(pdf_buf, sid))
                .await??;

        if config.write_raw_chunk_sidecar {
            write_chunks_jsonl(&raw_chunks_path, &chunks).await?;
        }
        let pages = pages_from_chunks(&chunks);
        let markdown = render_oxidize_markdown(
            &source_id,
            &title,
            &source_hash,
            &chunks,
            Some(&raw_chunks_path),
            Some(&cleanup_manifest_path),
            "mineru",
        );
        fs::write(&markdown_path, markdown).await?;

        let book = PlainTextBook {
            source_id: source_id.clone(),
            title: title.clone(),
            source_hash: source_hash.clone(),
            pages,
            chunks,
        };
        let doc = SourceDocument {
            id: Uuid::new_v4(),
            source_id,
            source_kind,
            title,
            file_path: pdf_path.to_string_lossy().to_string(),
            markdown_path: Some(markdown_path.to_string_lossy().to_string()),
            source_hash,
            parse_config_hash: config.parse_config_hash.clone(),
            metadata: json!({"extractor": "mineru", "cached_markdown": false, "raw_chunks_path": raw_chunks_path.to_string_lossy().to_string(), "chunk_count": book.chunks.len()}),
        };
        Ok(PdfExtractionResult {
            source_document: doc,
            book,
            markdown_path,
            raw_chunks_path: Some(raw_chunks_path),
        })
    }
}

fn markdown_dir_for(data_dir: &Path, source_kind: &SourceKind) -> PathBuf {
    match source_kind {
        SourceKind::Rulebook => data_dir.join("markdown/rulebooks"),
        SourceKind::Module => data_dir.join("markdown/modules"),
        SourceKind::Unknown => data_dir.join("markdown/unknown"),
    }
}

pub fn find_pdfs(data_dir: &Path, source_kind: SourceKind) -> Vec<PathBuf> {
    let subdir = match source_kind {
        SourceKind::Rulebook => "rulebooks",
        SourceKind::Module => "modules",
        SourceKind::Unknown => "unknown",
    };
    let root = data_dir.join(subdir);
    if !root.exists() {
        return vec![];
    }
    WalkDir::new(root)
        .min_depth(1)
        .max_depth(3)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| {
            p.extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase() == "pdf")
                .unwrap_or(false)
        })
        .collect()
}

pub async fn file_sha256(path: &Path) -> Result<String> {
    let bytes = fs::read(path)
        .await
        .with_context(|| format!("failed to read {}", path.display()))?;
    Ok(sha256_hex(bytes))
}

pub fn source_id_from_path(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "document".to_string());
    let re = Regex::new(r"[^a-zA-Z0-9]+").unwrap();
    let cleaned = re
        .replace_all(&stem.to_ascii_lowercase(), "_")
        .trim_matches('_')
        .to_string();
    if cleaned.is_empty() {
        "document".to_string()
    } else {
        cleaned
    }
}

pub fn title_from_path(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Untitled PDF".to_string())
}

pub fn split_pdftotext_pages(raw: &str) -> Vec<PageText> {
    raw.split('\x0c')
        .enumerate()
        .filter_map(|(idx, page)| {
            let text = normalize_pdf_text(page);
            if text.is_empty() {
                None
            } else {
                Some(PageText {
                    page: idx as u32 + 1,
                    text,
                })
            }
        })
        .collect()
}

/// Like `normalize_pdf_text` but PRESERVES internal multi-space runs (column alignment) — used for
/// the duotext `-layout` table sidecar, where the spacing IS the table structure.
pub fn normalize_pdf_text_keep_layout(input: &str) -> String {
    let cleaned = input
        .replace('\r', "")
        .replace('\u{fffd}', "")
        .replace('\u{0000}', "");
    cleaned
        .lines()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
        .trim_matches('\n')
        .to_string()
}

/// Split pdftotext `-layout` output into pages WITHOUT collapsing column whitespace.
pub fn split_pdftotext_pages_keep_layout(raw: &str) -> Vec<PageText> {
    raw.split('\x0c')
        .enumerate()
        .filter_map(|(idx, page)| {
            let text = normalize_pdf_text_keep_layout(page);
            if text.is_empty() {
                None
            } else {
                Some(PageText {
                    page: idx as u32 + 1,
                    text,
                })
            }
        })
        .collect()
}

pub fn render_page_anchored_markdown(
    source_id: &str,
    title: &str,
    source_hash: &str,
    pages: &[PageText],
    extractor: &str,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("---\nsource_id: {source_id}\ntitle: {title:?}\nsource_hash: {source_hash}\nformat: page_anchored_markdown\nextractor: {extractor}\n---\n\n"));
    for page in pages {
        let text_hash = sha256_hex(&page.text);
        out.push_str(&format!(
            "<!-- source_id={source_id} page={} text_hash={text_hash} -->\n\n",
            page.page
        ));
        out.push_str(&format!("# Page {}\n\n", page.page));
        out.push_str(page.text.trim());
        out.push_str("\n\n");
    }
    out
}

pub fn render_oxidize_markdown(
    source_id: &str,
    title: &str,
    source_hash: &str,
    chunks: &[DocumentChunk],
    raw_chunks_path: Option<&Path>,
    cleanup_manifest_path: Option<&Path>,
    extractor: &str,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("---\nsource_id: {source_id}\ntitle: {title:?}\nsource_hash: {source_hash}\nformat: oxidize_rag_chunks\nextractor: {extractor}\nchunk_count: {}\n", chunks.len()));
    if let Some(path) = raw_chunks_path {
        out.push_str(&format!("raw_chunks_path: {:?}\n", path.to_string_lossy()));
    }
    if let Some(path) = cleanup_manifest_path {
        out.push_str(&format!(
            "cleanup_manifest_path: {:?}\n",
            path.to_string_lossy()
        ));
    }
    out.push_str("---\n\n");
    for chunk in chunks {
        let pages = chunk
            .page_numbers
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let types = chunk.element_types.join(",");
        let heading = chunk.heading_context.join(" > ");
        out.push_str(&format!("<!-- source_id={source_id} chunk_id={} pages={pages} text_hash={} clean_status={} -->\n\n", chunk.chunk_id, chunk.text_hash.clone().unwrap_or_default(), chunk.clean_status.clone().unwrap_or_default()));
        out.push_str(&format!(
            "## Chunk {} · p[{}] · [{}]\n\n",
            chunk.chunk_id.rsplit('_').next().unwrap_or("0"),
            pages,
            types
        ));
        if !heading.is_empty() {
            out.push_str(&format!("**heading_context:** {heading}\n\n"));
        }
        if let Some(status) = &chunk.clean_status {
            out.push_str(&format!("**clean_status:** {status}\n\n"));
        }
        let body = if chunk.full_text.trim().is_empty() {
            &chunk.text
        } else {
            &chunk.full_text
        };
        out.push_str(body.trim());
        out.push_str("\n\n");
    }
    out
}

pub async fn read_markdown_pages(path: &Path) -> Result<Vec<PageText>> {
    let content = fs::read_to_string(path).await?;
    let page_re = Regex::new(r"(?m)^# Page (\d+)\s*$").unwrap();
    let matches: Vec<_> = page_re.find_iter(&content).collect();
    if !matches.is_empty() {
        let mut pages = Vec::new();
        for (idx, m) in matches.iter().enumerate() {
            let start = m.end();
            let end = matches
                .get(idx + 1)
                .map(|n| n.start())
                .unwrap_or(content.len());
            let header = &content[m.start()..m.end()];
            let page_num = header
                .trim_start_matches("# Page ")
                .trim()
                .parse::<u32>()
                .unwrap_or(idx as u32 + 1);
            pages.push(PageText {
                page: page_num,
                text: content[start..end].trim().to_string(),
            });
        }
        return Ok(pages);
    }
    let chunk_re = Regex::new(r"(?m)^## Chunk .+?p\[(?P<page>\d+)").unwrap();
    let chunk_matches: Vec<_> = chunk_re.find_iter(&content).collect();
    if !chunk_matches.is_empty() {
        let mut pages: BTreeMap<u32, String> = BTreeMap::new();
        let header_page_re = Regex::new(r"p\[(\d+)").unwrap();
        for (idx, m) in chunk_matches.iter().enumerate() {
            let header = &content[m.start()..m.end()];
            let page_num = header_page_re
                .captures(header)
                .and_then(|c| c.get(1))
                .and_then(|m| m.as_str().parse::<u32>().ok())
                .unwrap_or(idx as u32 + 1);
            let start = m.end();
            let end = chunk_matches
                .get(idx + 1)
                .map(|n| n.start())
                .unwrap_or(content.len());
            let entry = pages.entry(page_num).or_default();
            if !entry.is_empty() {
                entry.push('\n');
            }
            entry.push_str(content[start..end].trim());
        }
        return Ok(pages
            .into_iter()
            .map(|(page, text)| PageText { page, text })
            .collect());
    }
    Ok(vec![PageText {
        page: 1,
        text: content,
    }])
}

pub async fn write_chunks_jsonl(path: &Path, chunks: &[DocumentChunk]) -> Result<()> {
    let mut out = String::new();
    for chunk in chunks {
        out.push_str(&serde_json::to_string(chunk)?);
        out.push('\n');
    }
    fs::write(path, out).await?;
    Ok(())
}

pub async fn read_chunks_jsonl(path: &Path) -> Result<Vec<DocumentChunk>> {
    let content = fs::read_to_string(path).await?;
    let mut chunks = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        chunks.push(serde_json::from_str::<DocumentChunk>(line)?);
    }
    Ok(chunks)
}

pub async fn write_cleanup_manifest(
    path: &Path,
    source_id: &str,
    source_hash: &str,
    chunks: &[DocumentChunk],
) -> Result<()> {
    let entries = chunks.iter()
        .filter(|chunk| chunk.clean_status.as_deref() == Some("needs_llm_cleanup"))
        .map(|chunk| json!({
            "chunk_id": chunk.chunk_id.clone(),
            "page_numbers": chunk.page_numbers.clone(),
            "heading_context": chunk.heading_context.clone(),
            "token_estimate": chunk.token_estimate,
            "is_oversized": chunk.is_oversized,
            "element_types": chunk.element_types.clone(),
            "cleanup_reasons": chunk.metadata.get("cleanup_reasons").cloned().unwrap_or_else(|| json!([])),
        }))
        .collect::<Vec<_>>();
    let manifest = json!({
        "schema_version": "chatrpg.oxidize_cleanup_manifest.v1",
        "source_id": source_id,
        "source_hash": source_hash,
        "total_chunks": chunks.len(),
        "needs_cleanup": entries.len(),
        "entries": entries,
    });
    fs::write(path, serde_json::to_string_pretty(&manifest)?).await?;
    Ok(())
}

pub fn pages_from_chunks(chunks: &[DocumentChunk]) -> Vec<PageText> {
    let mut pages: BTreeMap<u32, String> = BTreeMap::new();
    for chunk in chunks {
        let page = chunk.page_numbers.first().copied().unwrap_or(
            chunk
                .chunk_id
                .rsplit('_')
                .next()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1),
        );
        let body = if chunk.full_text.trim().is_empty() {
            &chunk.text
        } else {
            &chunk.full_text
        };
        let entry = pages.entry(page).or_default();
        if !entry.is_empty() {
            entry.push_str("\n\n");
        }
        entry.push_str(body.trim());
    }
    pages
        .into_iter()
        .map(|(page, text)| PageText { page, text })
        .collect()
}

pub fn normalize_pdf_text(input: &str) -> String {
    let mut text = input.replace('\r', "").replace('\u{fffd}', "");
    text = text.replace('\u{0000}', "");
    let re_spaces = Regex::new(r"[ \t]{2,}").unwrap();
    text = re_spaces.replace_all(&text, " ").to_string();
    text.trim().to_string()
}

pub fn clean_extracted_spacing(input: &str) -> String {
    let mut text = normalize_pdf_text(input);
    let re_bars = Regex::new(r"(?:\s*\|\s*){3,}").unwrap();
    text = re_bars.replace_all(&text, " | ").to_string();
    text = collapse_repeated_phrase(&text);
    let re_space_before_punc = Regex::new(r"\s+([，。！？；：、])").unwrap();
    text = re_space_before_punc.replace_all(&text, "$1").to_string();
    let re_after_open = Regex::new(r"([（《“])\s+").unwrap();
    text = re_after_open.replace_all(&text, "$1").to_string();
    text.trim().to_string()
}

/// Rust regex does not support backreferences. Collapse lines such as
/// "逃向沙漠 逃向沙漠" without using a backref regex, preserving normal prose.
pub fn collapse_repeated_phrase(input: &str) -> String {
    let mut out = Vec::new();
    for line in input.lines() {
        let trimmed = line.trim();
        if trimmed.chars().count() <= 160 {
            let collapsed = collapse_repeated_phrase_line(trimmed);
            out.push(collapsed);
        } else {
            out.push(line.to_string());
        }
    }
    out.join("\n")
}

fn collapse_repeated_phrase_line(line: &str) -> String {
    if line.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = line.chars().collect();
    for sep_idx in 1..chars.len() {
        if !chars[sep_idx].is_whitespace() {
            continue;
        }
        let left = chars[..sep_idx]
            .iter()
            .collect::<String>()
            .trim()
            .to_string();
        let right = chars[sep_idx + 1..]
            .iter()
            .collect::<String>()
            .trim()
            .to_string();
        if !left.is_empty() && left == right {
            return left;
        }
    }
    line.to_string()
}

fn split_heading_context(input: &str, clean_mode: ExtractionCleanMode) -> Vec<String> {
    let cleaned = match clean_mode {
        ExtractionCleanMode::Raw => input.trim().to_string(),
        ExtractionCleanMode::Heuristic | ExtractionCleanMode::LlmReady => {
            clean_extracted_spacing(input)
        }
    };
    cleaned
        .split(|c| matches!(c, '>' | '/' | '»'))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn detect_oxidize_cleanup_reasons(
    text: &str,
    full_text: &str,
    _is_oversized: bool,
    _heading_context: Option<&str>,
    _token_estimate: usize,
) -> Vec<String> {
    // v1.15.4: keep ingest cleanup flags high-leverage. Semantic boundary,
    // noise, heading, and CJK spacing issues are handled by the parser's
    // source-unit conditioner. LLM budget should not be spent on cosmetic
    // line breaks, duplicated headings, or pretty formatting here.
    let mut reasons = Vec::new();
    let joined = format!("{full_text}\n{text}");
    if joined.contains("þÿ") || joined.contains('\u{fffd}') || joined.contains('\u{0000}') {
        reasons.push("encoding_artifact".to_string());
    }
    if joined.matches('|').count() > 20 || joined.contains("| |") {
        reasons.push("table_columns_collapsed".to_string());
    }
    reasons.sort();
    reasons.dedup();
    reasons
}

fn has_cjk_spacing_noise(text: &str) -> bool {
    let mut suspicious = 0usize;
    let mut total_pairs = 0usize;
    let chars: Vec<char> = text.chars().take(2_000).collect();
    for window in chars.windows(3) {
        if is_cjk(window[0]) && window[1].is_whitespace() && is_cjk(window[2]) {
            suspicious += 1;
        }
        if is_cjk(window[0]) && is_cjk(window[2]) {
            total_pairs += 1;
        }
    }
    total_pairs > 50 && suspicious > total_pairs / 8
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x4E00..=0x9FFF |
        0x3400..=0x4DBF |
        0x20000..=0x2A6DF |
        0x2A700..=0x2B73F |
        0x2B740..=0x2B81F |
        0x2B820..=0x2CEAF |
        0xF900..=0xFAFF
    )
}

pub fn source_index_from_book(doc: &SourceDocument, book: &PlainTextBook) -> SourceIndex {
    let source_ref = SourceDocumentRef {
        source_id: doc.source_id.clone(),
        title: doc.title.clone(),
        source_kind: doc.source_kind.clone(),
        source_hash: doc.source_hash.clone(),
        file_path: Some(doc.file_path.clone()),
    };
    let mut anchors: Vec<SourceAnchor> = book
        .pages
        .iter()
        .map(|p| SourceAnchor {
            anchor_id: format!("{}.p{:04}", book.source_id, p.page),
            source_id: book.source_id.clone(),
            page: Some(p.page),
            section_path: vec![],
            char_start: None,
            char_end: None,
            text_hash: Some(sha256_hex(&p.text)),
        })
        .collect();
    anchors.extend(book.chunks.iter().map(|c| SourceAnchor {
        anchor_id: c.chunk_id.clone(),
        source_id: book.source_id.clone(),
        page: c.page_numbers.first().copied(),
        section_path: c.heading_context.clone(),
        char_start: None,
        char_end: None,
        text_hash: c.text_hash.clone(),
    }));
    SourceIndex {
        sources: vec![source_ref],
        anchors,
    }
}

#[cfg(test)]
mod mineru_smoke {
    use super::*;

    /// Glue test: MinerU content_list.json -> DocumentChunk(s) with CLEAN tables.
    /// Ignored by default (shells out to the heavy vision model). Run with:
    ///   TRPG_TEST_PDF=/tmp/coc_weapons_small.pdf \
    ///     cargo test -p trpg-ingest --release mineru_extracts_clean_table -- --ignored --nocapture
    #[test]
    #[ignore]
    fn mineru_extracts_clean_table() {
        let pdf = std::env::var("TRPG_TEST_PDF").expect("set TRPG_TEST_PDF to a table-bearing PDF");
        let chunks = extract_mineru_chunks_blocking(PathBuf::from(&pdf), "smoke_coc".to_string())
            .expect("mineru extraction failed");
        assert!(!chunks.is_empty(), "no chunks produced");
        let table_chunks: Vec<&DocumentChunk> = chunks
            .iter()
            .filter(|c| c.element_types.iter().any(|t| t == "table"))
            .collect();
        println!(
            "chunks={} table_chunks={}",
            chunks.len(),
            table_chunks.len()
        );
        for c in &chunks {
            println!(
                "--- {} page={:?} types={:?} chars={}",
                c.chunk_id,
                c.page_numbers,
                c.element_types,
                c.text.len()
            );
        }
        assert!(!table_chunks.is_empty(), "no table-typed chunk found");
        let all_table_text: String = table_chunks
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        // CLEAN table => structured HTML rows, weapon name + damage dice together on a row.
        assert!(
            all_table_text.contains("<table") || all_table_text.contains("<tr"),
            "table chunk has no HTML structure (collapsed?)"
        );
        let has_weapon = all_table_text.to_lowercase().contains("revolver")
            || all_table_text.to_lowercase().contains("automatic");
        assert!(has_weapon, "weapon rows not found in table text");
        println!(
            "\n=== first 1200 chars of table text ===\n{}",
            &all_table_text.chars().take(1200).collect::<String>()
        );
    }
}

#[cfg(test)]
mod oxidize_partition_smoke {
    /// Does oxidize's PARTITION pipeline (layout-aware, NOT rag_chunks) recover clean tables?
    /// TRPG_TEST_PDF=/tmp/coc_weapons_small.pdf \
    ///   cargo test -p trpg-ingest oxidize_partition_tables -- --ignored --nocapture
    #[test]
    #[ignore]
    fn oxidize_partition_tables() {
        use oxidize_pdf::parser::PdfDocument;
        use oxidize_pdf::pipeline::{Element, PartitionConfig, ReadingOrderStrategy};
        let pdf = std::env::var("TRPG_TEST_PDF").expect("set TRPG_TEST_PDF");
        let doc = PdfDocument::open(&pdf).expect("open pdf");
        let cfg = PartitionConfig::default()
            .with_reading_order(ReadingOrderStrategy::XYCut { min_gap: 10.0 });
        let t = std::time::Instant::now();
        let els = doc.partition_with(cfg).expect("partition");
        let elapsed = t.elapsed();
        let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
        for el in &els {
            *counts.entry(el.type_name()).or_default() += 1;
        }
        println!(
            "[oxidize partition] elements={} elapsed={:?} types={:?}",
            els.len(),
            elapsed,
            counts
        );
        // print any Table whose rows mention a firearm => clean weapon table?
        for el in &els {
            if let Element::Table(td) = el {
                let b = el.bbox();
                println!(
                    "TABLE page={} bbox=(x={:.1},y={:.1},w={:.1},h={:.1}) rows={} conf={:.2}",
                    el.page(),
                    b.x,
                    b.y,
                    b.width,
                    b.height,
                    td.rows.len(),
                    el.metadata().confidence
                );
            }
        }
        // reading-order sample: first 30 elements as type:text
        println!("--- reading order (first 30 elements) ---");
        for el in els.iter().take(30) {
            let s: String = el.display_text().chars().take(72).collect();
            println!("  [{}] {}", el.type_name(), s.replace('\n', " "));
        }
    }
}

#[cfg(test)]
mod decolumn_tests {
    use super::*;

    // 两栏 -layout 文本：左栏念白 + 右栏另一内容流，列间是稳定空白"河"。
    // 期望去栏后：左栏全部在前、右栏全部在后，绝不逐行交错。
    #[test]
    fn two_column_layout_is_read_column_by_column() {
        let layout = "\
漫无止境的沥青带仍在不断向前延伸          然后你们看到了它一个褪色的广告牌\n\
浪使人无法目测距离只得闷头驶向远方        正在阳光下熠熠生辉上面画了一个\n\
山丘与天穹模糊的边界线还不到上午          加油站服务员他戴着一顶牛仔帽\n\
温就已经超过了一百华氏度铁皮车内          纪五十年代的风格他头顶有气泡\n\
如蒸笼一般你们每个人就差脱光了            泡上面写着你就快到了伙计\n\
是跟刚从游泳池里爬出来一样湿透            个模糊不堪的埃索石油公司标志\n";
        let out = decolumnize_layout_page(layout).expect("应检出两栏");
        let l = out.find("漫无止境").unwrap();
        let l_last = out.find("是跟刚从游泳池").unwrap();
        let r = out.find("然后你们").unwrap();
        assert!(l < r, "左栏开头应在右栏开头之前");
        assert!(l_last < r, "左栏所有行应在任何右栏行之前（无交错）");
    }

    // 单栏散文：检不出栏 → None（调用方回退 reading-order）。
    #[test]
    fn single_column_returns_none() {
        let layout = "\
这是一段普通的单栏散文文字它没有任何分栏\n\
继续第二行依旧是单栏内容没有空白河\n\
第三行第四行第五行第六行都是单栏\n\
第四行内容第五行内容第六行内容收尾\n\
再来一行凑足行数阈值方便判定单栏\n\
最后一行同样是单栏不应被错误切分\n";
        assert!(decolumnize_layout_page(layout).is_none());
    }

    // 行数不足 → None（避免误判短页）。
    #[test]
    fn too_few_lines_returns_none() {
        assert!(decolumnize_layout_page("左          右\n甲          乙\n").is_none());
    }

    // C1 回归：真实窄栏 CJK 双栏页。glyph 取自 血色公路 p16 序幕实际 -layout 输出
    // （左栏念白 + 右栏广告牌描述），各栏约 10 汉字 + 4 空格 gutter → char 宽仅 24。
    // 汉字视觉宽约 2x，char 宽远小于旧裸阈值 30：旧代码 width<30 会早退返回 None，
    // 去栏对中文页完全失效（回退逐行交错的 reading-order）。本测试锁定 C1 已修。
    // gutter 取 4 空格（pdftotext -layout 实际对窄栏也渲染多空格 gutter，见 p16），
    // 未人为加宽列内容凑总宽——保持真实窄栏中文页的字符宽度。
    #[test]
    fn narrow_cjk_two_column_decolumnizes() {
        let layout = "\
漫无止境的沥青带仍在    然后你们看到了它一个\n\
浪使人无法目测距离只    正在阳光下熠熠生辉上\n\
山丘与天穹模糊的边界    加油站服务员他戴一顶\n\
温就已经超过了百华氏    纪五十年代的风格他头\n\
如蒸笼一般你们每个人    泡上面写着你就快到了\n\
是跟刚从游泳池里爬出    个模糊不堪的埃索标志\n";
        let out = decolumnize_layout_page(layout).expect("窄栏 CJK 应检出两栏并去栏");
        let l = out.find("漫无止境").unwrap();
        let l_last = out.find("是跟刚从游泳池").unwrap();
        let r = out.find("然后你们").unwrap();
        assert!(l < r, "左栏开头应在右栏开头之前");
        assert!(l_last < r, "左栏所有行应在任何右栏行之前（无交错）");
    }

    // C2 结论锁定（ragged 行不左移）：行取自 血色公路 p17 实际 -layout 输出。
    // 实测结论：pdftotext -layout 用前导空格补位保持每行 x 位置——右栏行即使左栏整行
    // 为空，也不会左移到第 0 列，仍停在其原始列窗（下方 "斯，内特" / "们会坐在" 两行
    // 左栏为空但右文本仍位于第 40 列）。故固定列窗切片正确：右栏内容应整体落在右栏块、
    // 不被错填进左栏开头。本测试把该正确行为钉死，防止将来误改成"按行左对齐"破坏它。
    #[test]
    fn ragged_right_column_keeps_position_no_leftshift() {
        // 用数组 join 构造，保留行首空格（Rust "\<换行>" 续行会吞掉下一行的前导空白，
        // 故不能用续行写法表达"右栏行左栏为空、靠前导空格保位"这种 fixture）。
        // 下面的前导空格列宽即 pdftotext -layout 实际渲染的 x 位置（左 0 / 右 40）。
        let layout = [
            "你们隐约能够看到一座教堂的尖顶掩藏在小路的尽                  如果再不用适合的工具进行维护，故障值会降低到",
            "头。几个男人正坐在加油站前面，从你们进镇的那                  80，维护这杆枪大概需要花费一个小时。",
            "一刻起，他们的目光就死死盯在你们身上。                     NPC：白天通常有三个人会待在这里：拉斯·威廉姆",
            "                                        斯，内特·帕特森和史蒂夫·布朗。一般情况下，他",
            "德克萨斯州阿巴托尔镇",
            "                                        们会坐在加油站宽敞的顶棚底下抽烟喝啤酒。",
        ].join("\n") + "\n";
        let out = decolumnize_layout_page(&layout).expect("ragged 双栏页应检出两栏");
        let l_first = out.find("你们隐约").unwrap(); // 左栏首行
        let l_last = out.find("德克萨斯州阿巴托尔镇").unwrap(); // 左栏末行
        let r_first = out.find("如果再不用").unwrap(); // 右栏首行
        let r_empty_left = out.find("斯，内特").unwrap(); // 左栏为空那行的右栏文本
                                                          // 左栏整段（含末行）应排在右栏首行之前——右栏没有被左移错填到左栏开头。
        assert!(l_last < r_first, "左栏末行应在右栏首行之前（右栏整体在后）");
        assert!(
            l_first < r_empty_left,
            "左栏为空那行的右栏文本应落在右栏块，未左移到开头"
        );
        assert!(
            r_first < r_empty_left,
            "右栏内部顺序：先 '如果再不用' 后 '斯，内特'"
        );
    }

    // fail-closed：整页带统一缩进的单栏页（无内部空白河）不应被误切。
    // 注：函数内"切分后每列须非空"的护栏属 defense-in-depth——在当前
    //（每河带取单一中点 + start>2/end<width-2 边界约束）的几何下，空结果列实际不可达，
    // 该护栏是为将来若改动河带→切分位逻辑时兜底；此处用真实的缩进单栏退化页锁定 None。
    #[test]
    fn indented_single_column_returns_none() {
        // 数组 join 保留行首缩进（续行写法会吞前导空白，使缩进失效）。
        let layout = [
            "          这是一段实际只有单栏的内容整体带统一缩进",
            "          第二行同样只是单栏没有内部空白河",
            "          第三行内容继续维持这种缩进单栏排版",
            "          第四行内容第五行内容第六行内容",
            "          第五行补足行数让其通过下限判定",
            "          第六行结尾仍然是缩进单栏无第二栏",
        ]
        .join("\n")
            + "\n";
        assert!(
            decolumnize_layout_page(&layout).is_none(),
            "缩进单栏页无内部空白河，应 fail-closed 返回 None"
        );
    }

    // 三栏（血色公路目录式）：两条空白河 → 切成 3 块，列内上→下、列间左→右顺序对。
    // glyph 取自 p3 目录条目（警告/周边路段/变异真菌 等），用整齐 gutter 表示排版良好
    // 的三栏页（真实 p3 有点线 leader 填满 gutter，属另类退化情形，不在此锁定）。
    #[test]
    fn three_column_layout_splits_in_order() {
        let layout = "\
警告事项        周边路段        变异真菌\n\
模组信息        基地大门        掘洞巨虫\n\
剧情梗概        基地大院        交叉路口\n\
如何使用        教堂尖顶        亵玩水池\n\
导入剧情        基地地图        转化失败\n\
事前准备        宿舍水井        粪池考验\n";
        let out = decolumnize_layout_page(layout).expect("应检出三栏");
        let c1 = out.find("警告事项").unwrap();
        let c1_last = out.find("事前准备").unwrap();
        let c2 = out.find("周边路段").unwrap();
        let c2_last = out.find("宿舍水井").unwrap();
        let c3 = out.find("变异真菌").unwrap();
        assert!(c1_last < c2, "第一栏整段应在第二栏之前");
        assert!(c2_last < c3, "第二栏整段应在第三栏之前");
    }

    // 表格页拦截：is_duotext_table_page 应识别多列表格（每行 ≥2 列间隙、≥6 行），
    // 这样 merge_duotext_pages 会保留 -layout 列对齐、不送进去栏（去栏会破坏表对齐）。
    // 同时确认 2 栏散文（每行仅 ~1 gutter 间隙）不被误判为表格，仍走去栏。
    #[test]
    fn table_page_detected_prose_not() {
        let table = "\
力量    体质    体型    敏捷\n\
50      60      55      45\n\
65      70      40      80\n\
30      55      75      60\n\
45      50      65      70\n\
80      40      55      50\n\
60      65      45      75\n";
        assert!(
            is_duotext_table_page(table),
            "多列表格应被识别为表页（保留 -layout 对齐）"
        );
        let prose = "\
漫无止境的沥青带仍在    然后你们看到了它\n\
浪使人无法目测距离只    正在阳光下熠熠生\n\
山丘与天穹模糊的边界    加油站服务员戴帽\n\
温就已经超过了百华氏    纪五十年代的风格\n\
如蒸笼一般你们每个人    泡上面写你就快到\n\
是跟刚从游泳池里爬出    个模糊不堪的标志\n";
        assert!(
            !is_duotext_table_page(prose),
            "2 栏散文不应被误判为表格（应走去栏）"
        );
    }
}

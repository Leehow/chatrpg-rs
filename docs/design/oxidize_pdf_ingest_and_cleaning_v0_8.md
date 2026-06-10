# Oxidize PDF Ingest & LLM Cleaning Design v0.8

## 1. Why this exists

The old ingestion backend used `pdftotext -layout` as a pragmatic external dependency. v0.8 adds `oxidize-pdf` as the preferred Rust-native backend because it can parse text pages and produce LLM-friendly chunks from the same document pipeline. The output is not treated as final truth; it is treated as **raw source material with anchors**.

The design assumption is:

```text
PDF layout extraction is allowed to be messy.
LLM cleanup may repair layout artifacts.
LLM cleanup must not invent rules, clues, NPC facts, or mechanics.
Every cleaned unit must keep source_id/page/chunk anchors.
```

## 2. Observations from the sample files

The sample `oxidize_CHUNKS_xueseguanglu_CN.md` shows that `rag_chunks()` is extremely fast for the sample PDF: open took about 37.6ms and chunking took about 245ms. It produced 456 chunks, 15,085 estimated tokens, one oversized chunk, and heading context for 444 out of 456 chunks.

That is good enough to become the default ingestion route, but the same sample also shows why we need cleanup:

- Table-of-contents pages can collapse into noisy table strings.
- Some tiny title chunks duplicate text, such as `蛇洞 蛇洞` or `14A. 14A.`.
- Chinese text sometimes contains layout spacing artifacts.
- Long location sections may span multiple pages and need recomposition.
- `heading_context` is useful but not always semantically clean.

The paired `oxidize_xueseguanglu_CN.md` page-anchored Markdown shows the other side: the prose is often readable, but headings and adjacent sections can run together, e.g. one location description may continue directly into the next numbered section. That is acceptable as raw input, but it is not the same as a playable module packet.

## 3. Product behavior

`TRPG_PDF_BACKEND=auto` now means:

```text
1. Try oxidize-pdf.
2. Write page-anchored Markdown.
3. Write raw oxidize chunks JSONL.
4. Write cleanup manifest JSON.
5. If oxidize fails, fall back to pdftotext.
```

Supported values:

```text
TRPG_PDF_BACKEND=auto
TRPG_PDF_BACKEND=oxidize
TRPG_PDF_BACKEND=pdftotext
```

CLI:

```bash
trpg parse-all --pdf-backend auto
trpg parse-all --pdf-backend oxidize
trpg parse-all --pdf-backend pdftotext
```

API:

```http
POST /api/ingest/parse-all?pdf_backend=oxidize
```

## 4. Generated artifacts

For a source PDF with `source_id = blood_highway_cn`, v0.8 writes:

```text
data/markdown/modules/blood_highway_cn.oxidize.md
  Page-anchored raw Markdown.

data/parsed/source_chunks/blood_highway_cn.oxidize_chunks.jsonl
  One raw search/cleanup chunk per line.

data/parsed/source_chunks/blood_highway_cn.llm_cleaned_chunks.jsonl
  Optional cleaned chunks when TRPG_INGEST_LLM_CLEAN=true.

data/markdown/cleaned/blood_highway_cn.llm_cleaned.md
  Optional cleaned Markdown for inspection.
```

`oxidize_chunks.jsonl` record shape:

```json
{
  "chunk_id": "blood_highway_cn.chunk_00042",
  "chunk_index": 42,
  "text": "raw extracted text",
  "full_text": "heading context + raw text",
  "page_numbers": [12, 13],
  "token_estimate": 512,
  "is_oversized": false,
  "heading_context": ["基地", "14A. 大门"],
  "element_types": ["text"],
  "extraction_confidence": 0.93,
  "needs_llm_cleanup": true,
  "cleanup_reasons": ["duplicated_heading", "cjk_spacing_noise"],
  "source_refs": []
}
```

## 5. Cleanup classification

The deterministic classifier only decides whether a chunk *needs cleanup*. It does not repair content.

Current reasons:

```text
oversized
  Chunk exceeds target token size.

table_columns_collapsed
  Many `|` separators or table artifacts.

encoding_artifact
  Nulls, replacement characters, broken UTF marker patterns.

duplicated_heading
  Short title repeated twice.

cjk_spacing_noise
  Suspicious spacing between many CJK characters.

orphan_heading
  Very short heading followed by body text.
```

## 6. LLM cleaning contract

LLM cleanup should be a parser head, not a blind postprocessor.

Input:

```json
{
  "source_id": "...",
  "chunk_id": "...",
  "page_numbers": [42, 43],
  "heading_context": ["14A.", "大门"],
  "raw_text": "...",
  "cleanup_reasons": ["duplicated_heading"]
}
```

Output:

```json
{
  "chunk_id": "...",
  "status": "cleaned | unchanged | needs_human_review",
  "cleaned_title": "14A. 大门",
  "cleaned_text": "...",
  "section_path": ["基地", "14A. 大门"],
  "visibility_hint": "gm_only | player_visible | public | unknown",
  "content_kind_hint": "location | npc | clue | rule | warning | timeline | table | unknown",
  "source_refs": [
    {"source_id":"...", "page":42, "anchor_id":"..."}
  ],
  "uncertainties": []
}
```

Hard rules:

```text
Do not invent missing rules.
Do not summarize away mechanical numbers.
Do not merge GM-only and player-visible content without tagging.
Do not remove content warnings.
Do not change page_numbers or chunk_id.
```

## 7. Integration with Tantivy search

The raw chunk JSONL lives under `data/parsed/source_chunks`, so it is picked up by the existing `files.parsed_jsonl` search source. This means even before cleanup, the LLM GM can retrieve source chunks by title, NPC name, location, clue, or rule term.

Cleaned chunks should later be indexed as richer `ContextBlock` or `MaterialIndex` entries with:

```text
origin = cleaned_source_chunk
logical_kind = location | npc | rule | clue | warning | scene_node
visibility = gm_only by default
source_refs = preserved from raw chunk
```

## 8. Why cleanup is not part of SSE output

PDF ingestion and cleanup are backend preparation jobs. They should emit job progress events if exposed through API, but they should not block play-turn SSE output. A play turn only retrieves already indexed chunks or uses a provisional ruling if source-backed lookup is not ready.


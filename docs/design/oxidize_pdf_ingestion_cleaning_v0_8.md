# v0.8 Oxidize PDF Ingestion and LLM Cleaning Design

## Purpose

v0.8 changes PDF ingestion from a `pdftotext`-first adapter to an `oxidize-pdf`-first ingestion pipeline. The goal is not to make raw PDF extraction perfect. The goal is to preserve source anchors, chunk metadata, and enough structure for the LLM GM to onboard quickly, search accurately, and clean only the pieces that need cleaning.

## Why oxidize-pdf

`oxidize-pdf` is a Rust-native PDF parser/extractor. The project now treats it as the default backend because it fits the rest of the stack: pure Rust service, no external Poppler dependency for the default path, page-aware text extraction, and chunk-oriented processing. `pdftotext` remains a fallback for local environments or unusual PDFs.

## Observed extraction characteristics

The uploaded `oxidize_CHUNKS_xueseguanglu_CN.md` sample shows useful chunk metadata: raw text, full text, page numbers, element types, heading context, token estimates, oversize flag, and bounding boxes. The same sample shows why the system needs a cleaning layer: the table-of-contents chunk is oversized and contains many empty table-cell delimiters. The companion Markdown output shows corrupted PDF metadata in the title and a densely compressed page-3 table of contents.

These are not reasons to reject the parser. They are reasons to split ingestion into raw, cleaned, and curated layers.

## Ingestion layers

```text
PDF
  → Oxidize extraction
  → raw chunks JSONL
  → heuristic cleanup
  → clean chunks JSONL
  → optional LLM cleanup for problematic chunks
  → page-anchored Markdown
  → parser/onboarding/search/runtime
```

### Raw layer

Stored as:

```text
data/parsed/source_chunks/<source_id>.raw_chunks.jsonl
```

Properties:

- untrusted for prompt use
- never directly used as resident context
- good for debugging parser regressions
- preserves raw chunk metadata

### Heuristic cleaned layer

Stored as:

```text
data/markdown/rulebooks/<source_id>.oxidize.md
data/markdown/modules/<source_id>.oxidize.md
```

Properties:

- removes common artifacts such as repeated empty pipes, duplicate whitespace, and obvious metadata garbage
- preserves all source text as much as possible
- safe enough for onboarding and indexing

### LLM cleaned layer

Enabled by:

```env
TRPG_INGEST_LLM_CLEAN=true
TRPG_INGEST_LLM_CLEAN_MAX_CHUNKS=24
```

Stored as:

```text
data/parsed/source_chunks/<source_id>.llm_cleaned_chunks.jsonl
data/markdown/cleaned/<source_id>.llm_cleaned.md
```

Properties:

- applies only to suspicious chunks by default
- repairs structure but does not summarize
- must preserve rule facts, NPC names, numbers, clues, GM-only notes, and source pages
- each cleaned chunk keeps `clean_status = llm_cleaned`

## Suspicious chunk detection

A chunk is a cleaning candidate when at least one condition is true:

- `is_oversized = true`
- contains repeated empty table delimiters such as `| |`
- contains mojibake such as `þÿ`
- contains replacement characters
- first few lines are extremely long
- no heading context and body is long

## Runtime policy

Raw and cleaned chunks do not automatically enter prompts. They become searchable documents and source references. Search hits must be converted into `ContextBlock`s with TTL before entering BP1/BP2/BP3.

## Configuration

```env
TRPG_PDF_BACKEND=oxidize
TRPG_PDF_ALLOW_PDFTOTEXT_FALLBACK=true
TRPG_OXIDIZE_CLEAN_MODE=heuristic
TRPG_OXIDIZE_CHUNK_TARGET_CHARS=4000
TRPG_OXIDIZE_WRITE_RAW_CHUNKS=true
TRPG_INGEST_LLM_CLEAN=false
TRPG_INGEST_LLM_CLEAN_MAX_CHUNKS=24
```

## CLI behavior

`parse-all` now logs and records the active backend and cleaning mode in the conversion trace. The parser still follows onboard-and-play by default; full extraction remains opt-in via `--full-parse`.

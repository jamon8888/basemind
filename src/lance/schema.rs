//! Arrow schemas for the LanceDB-backed `documents` and `memory` tables.
//!
//! The vector dimension is fixed once at table-creation time; mismatched dims
//! trigger a wipe-and-rebuild (see [`crate::lance::LanceStore::open`]).

use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};

/// Build the schema for the per-document-chunk `documents` table.
///
/// Columns:
/// - `scope`     UTF-8     repo identity (normalised git remote URL or workdir path)
/// - `path`      UTF-8     repo-relative path of the source file
/// - `chunk_idx` UInt32    0-based index of this chunk within the file
/// - `mime_type` UTF-8     IANA MIME type xberg detected
/// - `text`      UTF-8     the chunk text (snippet returned by search results)
/// - `heading_path`  UTF-8  the Markdown heading breadcrumb the chunk sits under, `""` when it has
///   none. Its own column because the keyword lane indexes it (see [`crate::lance::fts`]), and
///   because Lance builds at most one inverted index per column. Computed unconditionally by the
///   extractor, not behind `embed`: on a lexical-only store this is the only structural signal the
///   keyword lane has.
/// - `byte_start` UInt32   chunk start byte offset in the original document
/// - `byte_end`  UInt32    chunk end byte offset
/// - `rehydration_ref`  UTF-8    nullable vault key for the document's encrypted rehydration map
/// - `embedding` FixedSizeList<Float32, DIM>  the embedding vector, **nullable** — `embed = false`
///   stores null rather than omitting the column, so a lexical-only store and a vector store share
///   one schema and one `wipe_on_mismatch` path. The dimension still comes from the configured
///   `embedding_preset`: `embed = false` means "do not run the embedder", not "no model configured",
///   and the sibling `memory` / `code_chunks` tables in the same connection need a concrete dim
///   regardless. Nothing is written into the column on a lexical-only store.
pub fn documents_schema(dim: u16) -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("scope", DataType::Utf8, false),
        Field::new("path", DataType::Utf8, false),
        Field::new("chunk_idx", DataType::UInt32, false),
        Field::new("mime_type", DataType::Utf8, false),
        Field::new("text", DataType::Utf8, false),
        Field::new("heading_path", DataType::Utf8, false),
        Field::new("byte_start", DataType::UInt32, false),
        Field::new("byte_end", DataType::UInt32, false),
        Field::new("rehydration_ref", DataType::Utf8, true),
        Field::new(
            "embedding",
            DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), i32::from(dim)),
            true,
        ),
    ]))
}

/// Build the schema for the `memory` table.
///
/// Columns:
/// - `scope`       UTF-8     repo identity
/// - `key`         UTF-8     primary lookup key (unique within `(scope, visibility, agent_id)`)
/// - `value`       UTF-8     the stored value text
/// - `tags`        `List<UTF-8>`  optional tags
/// - `visibility`  UTF-8     memory tier: `"group"` (shared) or `"individual"` (per-agent)
/// - `agent_id`    UTF-8     owner of an individual-tier row (empty for the group tier)
/// - `embedding`   FixedSizeList<Float32, DIM>
/// - `created_at`  TimestampMicros
/// - `updated_at`  TimestampMicros
pub fn memory_schema(dim: u16) -> SchemaRef {
    let tags_inner = Arc::new(Field::new("item", DataType::Utf8, true));
    Arc::new(Schema::new(vec![
        Field::new("scope", DataType::Utf8, false),
        Field::new("key", DataType::Utf8, false),
        Field::new("value", DataType::Utf8, false),
        Field::new("tags", DataType::List(tags_inner), true),
        Field::new("visibility", DataType::Utf8, false),
        Field::new("agent_id", DataType::Utf8, false),
        Field::new(
            "embedding",
            DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), i32::from(dim)),
            false,
        ),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Microsecond, None), false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Microsecond, None), false),
    ]))
}

/// Build the schema for the per-chunk `code_chunks` table (semantic code search).
///
/// Columns mirror the pointer fields `search_code` returns plus the chunk `text` and the
/// embedding. Shares the single `meta.json` dim/model with the `documents` / `memory` tables.
///
/// - `scope`      UTF-8    repo identity
/// - `path`       UTF-8    repo-relative source path
/// - `chunk_id`   UTF-8    content-addressed `<hash>:<ordinal>`
/// - `symbol`     UTF-8    symbol name (empty for a module-level gap chunk)
/// - `kind`       UTF-8    symbol kind (`function`, `method`, `module`, …)
/// - `lang`       UTF-8    tree-sitter language pack name
/// - `line_start` UInt32   1-based inclusive start line
/// - `line_end`   UInt32   1-based inclusive end line
/// - `byte_start` UInt32   chunk start byte offset
/// - `byte_end`   UInt32   chunk end byte offset
/// - `text`       UTF-8    the chunk text
/// - `embedding`  FixedSizeList<Float32, DIM>
#[cfg(feature = "code-search")]
pub fn code_chunks_schema(dim: u16) -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("scope", DataType::Utf8, false),
        Field::new("path", DataType::Utf8, false),
        Field::new("chunk_id", DataType::Utf8, false),
        Field::new("symbol", DataType::Utf8, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("lang", DataType::Utf8, false),
        Field::new("line_start", DataType::UInt32, false),
        Field::new("line_end", DataType::UInt32, false),
        Field::new("byte_start", DataType::UInt32, false),
        Field::new("byte_end", DataType::UInt32, false),
        Field::new("text", DataType::Utf8, false),
        Field::new(
            "embedding",
            DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), i32::from(dim)),
            false,
        ),
    ]))
}

/// Build the schema for the `doc_links` table — the persisted document→code links (ADR-0008).
///
/// Vector-free (dim-independent): a link is a raw mention, resolution happens at graph-build time.
/// Columns:
/// - `scope`         UTF-8   ingestion scope (repo identity)
/// - `doc_path`      UTF-8   repo-relative path of the source document
/// - `chunk_idx`     UInt32  0-based chunk index within the document
/// - `mention_kind`  UTF-8   `"name"` (identifier / keyword / entity) or `"path"` (path citation)
/// - `mention_value` UTF-8   the raw mention text
#[cfg(feature = "documents")]
pub fn doc_links_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("scope", DataType::Utf8, false),
        Field::new("doc_path", DataType::Utf8, false),
        Field::new("chunk_idx", DataType::UInt32, false),
        Field::new("mention_kind", DataType::Utf8, false),
        Field::new("mention_value", DataType::Utf8, false),
    ]))
}

/// Identity key source for entity nodes — mirrors spec 0016 §7.2.
///
/// Under `token_replace` the graph cannot join across documents by the literal
/// (e.g. `[EMAIL_1]` appears in many documents). The key source distinguishes:
/// - `TokenOfDocument`: (scope, label, literal, document_path) — document-scoped
/// - `ClearLiteral`: (scope, label, normalized_literal) — global within scope
#[cfg(feature = "graphrag")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityKeySource {
    TokenOfDocument,
    ClearLiteral,
}

/// Build the schema for the `graph_entities` table — entity nodes over the documents corpus.
///
/// One row per unique entity key (scope × label × key_source × normalized_literal/document).
/// Columns:
/// - `scope`            UTF-8     ingestion scope (repo identity)
/// - `label`            UTF-8     GLiNER label (`person`, `organization`, `location`, `custom:*`)
/// - `key_source`       UInt8     0 = TokenOfDocument, 1 = ClearLiteral (EntityKeySource discriminant)
/// - `normalized_literal` UTF-8  normalized literal (case-fold + ascii-fold + whitespace collapse)
/// - `document_path`    UTF-8     document path (only for TokenOfDocument; empty for ClearLiteral)
/// - `mention_count`    UInt32    number of mentions aggregated into this node
/// - `first_seen_chunk` UInt32    chunk_idx of first mention (for ordering)
/// - `last_seen_chunk`  UInt32    chunk_idx of last mention
/// - `doc_path`         UTF-8     document path of the representative mention
/// - `doc_chunk_idx`    UInt32    chunk index of the representative mention
#[cfg(feature = "graphrag")]
pub fn graph_entities_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("scope", DataType::Utf8, false),
        Field::new("label", DataType::Utf8, false),
        Field::new("key_source", DataType::UInt8, false),
        Field::new("normalized_literal", DataType::Utf8, false),
        Field::new("document_path", DataType::Utf8, false),
        Field::new("mention_count", DataType::UInt32, false),
        Field::new("first_seen_chunk", DataType::UInt32, false),
        Field::new("last_seen_chunk", DataType::UInt32, false),
        Field::new("doc_path", DataType::Utf8, false),
        Field::new("doc_chunk_idx", DataType::UInt32, false),
    ]))
}

/// Build the schema for the `graph_edges` table — co-occurrence edges within chunks.
///
/// One row per unique (source_entity_key, target_entity_key, chunk) triple.
/// Columns:
/// - `scope`           UTF-8    ingestion scope (repo identity)
/// - `src_key_source`  UInt8    source EntityKeySource discriminant
/// - `src_normalized_literal` UTF-8  source normalized literal
/// - `src_document_path` UTF-8    source document path (for TokenOfDocument)
/// - `src_label`       UTF-8    source entity label
/// - `dst_key_source`  UInt8    target EntityKeySource discriminant
/// - `dst_normalized_literal` UTF-8  target normalized literal
/// - `dst_document_path` UTF-8    target document path (for TokenOfDocument)
/// - `dst_label`       UTF-8    target entity label
/// - `chunk_idx`       UInt32   chunk index where co-occurrence was observed
/// - `doc_path`        UTF-8    document path of the chunk
/// - `weight`          UInt32   number of times this edge was observed (always 1 per chunk, can accumulate across rebuilds)
#[cfg(feature = "graphrag")]
pub fn graph_edges_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("scope", DataType::Utf8, false),
        Field::new("src_key_source", DataType::UInt8, false),
        Field::new("src_normalized_literal", DataType::Utf8, false),
        Field::new("src_document_path", DataType::Utf8, false),
        Field::new("src_label", DataType::Utf8, false),
        Field::new("dst_key_source", DataType::UInt8, false),
        Field::new("dst_normalized_literal", DataType::Utf8, false),
        Field::new("dst_document_path", DataType::Utf8, false),
        Field::new("dst_label", DataType::Utf8, false),
        Field::new("chunk_idx", DataType::UInt32, false),
        Field::new("doc_path", DataType::Utf8, false),
        Field::new("weight", DataType::UInt32, false),
    ]))
}

/// Table names — small constants in one place so the `LanceStore` impl and any
/// future migration code agree on what's where.
pub const DOCUMENTS_TABLE: &str = "documents";
pub const MEMORY_TABLE: &str = "memory";
#[cfg(feature = "code-search")]
pub const CODE_CHUNKS_TABLE: &str = "code_chunks";
#[cfg(feature = "documents")]
pub const DOC_LINKS_TABLE: &str = "doc_links";
#[cfg(feature = "graphrag")]
pub const GRAPH_ENTITIES_TABLE: &str = "graph_entities";
#[cfg(feature = "graphrag")]
pub const GRAPH_EDGES_TABLE: &str = "graph_edges";

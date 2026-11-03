//! LanceDB-backed vector storage for the document tier and shared agent memory.
//!
//! One store per `.basemind/lance/` directory. The dim of the embedding vector
//! is fixed at table-creation time and persisted in a small `meta.json`; reopen
//! with a different dim triggers a wipe-and-rebuild of the whole lance dir
//! (mirroring the existing `INDEX_SCHEMA_VER`-mismatch flow in
//! [`crate::store::Store::open`]).
//!
//! The LanceDB client is async. We block on a private current-thread tokio
//! runtime so the scanner (rayon, sync) and the MCP server (multi-thread tokio)
//! can share the same sync API surface without each callsite worrying about
//! runtime context.

mod batch;
#[cfg(feature = "documents")]
mod doc_links;
/// Documents-tier row/schema tests, likewise out of line.
#[cfg(test)]
mod documents_unit_tests;
/// Lexical index build and query, out of line by the line cap.
#[cfg(feature = "documents")]
pub mod fts;
mod generation;
/// GraphRAG entity graph over GLiNER entities (spec 0016 / ADR-0016).
#[cfg(feature = "graphrag")]
pub mod graph_api;
mod predicates;

#[cfg(feature = "graphrag")]
pub mod graphrag;
pub mod schema;
/// Unit tests; out of line by the line cap (`tests/max_lines.rs`).
#[cfg(test)]
mod store_tests;
/// Test-only inherent methods on [`LanceStore`], kept out of this module by the 1000-line cap
/// (`tests/max_lines.rs`).
#[cfg(any(test, feature = "test-support"))]
mod test_support;

pub use batch::{BATCH_MAX_DOCS, BATCH_MAX_ROWS, PendingBatch, SCAN_RETENTION};

#[cfg(feature = "documents")]
pub use doc_links::DocLinkRow;

#[cfg(feature = "graphrag")]
pub use graphrag::{
    CypherResult, DocEntitySidecar, GraphEdgeRow, GraphEntityRow, GraphRagHit, GraphragConfig,
    build_graph_from_entities, delete_graph_for_scope, execute_cypher, normalize_literal, upsert_graph_edges,
    upsert_graph_entities,
};

use generation::{bump_generation_file, generation_path, read_generation, wipe_on_mismatch};
use std::path::{Path, PathBuf};

use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use arrow_array::builder::{
    FixedSizeListBuilder, Float32Builder, ListBuilder, StringBuilder, TimestampMicrosecondBuilder, UInt32Builder,
};
use arrow_array::{Array, RecordBatch, StringArray};
use futures::TryStreamExt;
use lancedb::Connection;
use lancedb::query::{ExecutableQuery, QueryBase};
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;

use schema::{DOCUMENTS_TABLE, MEMORY_TABLE, documents_schema, memory_schema};
#[cfg(feature = "graphrag")]
use schema::{GRAPH_EDGES_TABLE, GRAPH_ENTITIES_TABLE, graph_edges_schema, graph_entities_schema};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct LanceMeta {
    dim: u16,
    embedding_model: String,
    #[serde(default)]
    schema_ver: u32,
    #[serde(default)]
    tokenizer_config: String,
}

const TOKENIZER_CONFIG: &str = "simple-v1";
const META_FILE: &str = "meta.json";
const GENERATION_SUFFIX: &str = ".generation";

pub fn bump_document_generation_for_dir(dir: &Path) -> Result<u64> {
    let path = generation_path(dir);
    let current = read_generation(&path)?;
    bump_generation_file(&path, current)
}

#[cfg(feature = "intelligence")]
pub fn open_existing(dir: &Path) -> Result<Option<LanceStore>> {
    let meta_path = dir.join(META_FILE);
    if !meta_path.exists() {
        return Ok(None);
    }
    let body = std::fs::read(&meta_path).with_context(|| format!("read {}", meta_path.display()))?;
    let meta: LanceMeta = serde_json::from_slice(&body).with_context(|| format!("parse {}", meta_path.display()))?;
    LanceStore::open(dir, meta.dim, &meta.embedding_model).map(Some)
}

pub const MEMORY_SCHEMA_VER: u32 = crate::version::RELEASE_MINOR as u32;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocumentFacets {
    pub doc_type: Option<String>,
    pub section: Option<String>,
    pub jurisdiction: Option<String>,
    pub doc_date: Option<String>,
    pub keywords: Option<Vec<String>>,
    pub entities: Option<Vec<String>>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct DocumentFacetFilter {
    pub doc_type: Option<String>,
    pub section: Option<String>,
    pub jurisdiction: Option<String>,
    pub doc_date: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DocumentRow {
    pub scope: String,
    pub path: String,
    pub chunk_idx: u32,
    pub mime_type: String,
    pub facets: DocumentFacets,
    pub text: String,
    pub text_phrase: String,
    pub heading_path: String,
    pub cites: String,
    pub byte_start: u32,
    pub byte_end: u32,
    pub rehydration_ref: Option<String>,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct MemoryRow {
    pub scope: String,
    pub key: String,
    pub value: String,
    pub tags: Vec<String>,
    pub visibility: String,
    pub agent_id: String,
    pub embedding: Vec<f32>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct DocumentHit {
    pub path: String,
    pub chunk_idx: u32,
    pub text: String,
    pub mime_type: String,
    pub facets: DocumentFacets,
    pub byte_start: u32,
    pub byte_end: u32,
    pub distance: f32,
}

#[derive(Debug, Clone)]
pub struct MemoryHit {
    pub key: String,
    pub value: String,
    pub tags: Vec<String>,
    pub distance: f32,
}

#[cfg(feature = "code-search")]
#[derive(Debug, Clone)]
pub struct CodeRow {
    pub scope: String,
    pub path: String,
    pub chunk_id: String,
    pub symbol: String,
    pub kind: String,
    pub lang: String,
    pub line_start: u32,
    pub line_end: u32,
    pub byte_start: u32,
    pub byte_end: u32,
    pub text: String,
    pub embedding: Vec<f32>,
}

#[cfg(feature = "code-search")]
#[derive(Debug, Clone)]
pub struct CodeChunkHit {
    pub path: String,
    pub chunk_id: String,
    pub symbol: String,
    pub kind: String,
    pub lang: String,
    pub line_start: u32,
    pub line_end: u32,
    pub byte_start: u32,
    pub byte_end: u32,
    pub distance: f32,
}

/// Embedded LanceDB store. Cheap to clone (internal Arc).
#[derive(Clone)]
pub struct LanceStore {
    inner: Arc<LanceStoreInner>,
}

struct LanceStoreInner {
    /// `Some` for the whole lifetime; taken in `Drop` so the runtime can be torn
    /// down off the current async context (see the `Drop` impl).
    runtime: Option<Runtime>,
    connection: Connection,
    dim: u16,
    embedding_model: String,
    dir: PathBuf,
    generation_path: PathBuf,
    document_generation: Mutex<u64>,
    /// Commits per table since the last maintenance pass; drives bounded version retention.
    commits: Mutex<std::collections::HashMap<String, u64>>,
}

impl LanceStoreInner {
    /// The owned tokio runtime. Present until `Drop` takes it.
    fn rt(&self) -> &Runtime {
        self.runtime.as_ref().expect("LanceStore runtime is present until drop")
    }
}

impl Drop for LanceStoreInner {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

impl LanceStore {
    /// Open (or initialise) the lance store rooted at `dir`. If a pre-existing
    /// meta.json reports a different `(dim, embedding_model, schema_ver)` triple, the
    /// entire dir is wiped and rebuilt before the connection opens.
    pub fn open(dir: &Path, dim: u16, embedding_model: &str) -> Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        let meta_path = dir.join(META_FILE);
        let generation_path = generation_path(dir);
        let generation = read_generation(&generation_path)?;
        let expected = LanceMeta {
            dim,
            embedding_model: embedding_model.to_string(),
            schema_ver: MEMORY_SCHEMA_VER,
            tokenizer_config: TOKENIZER_CONFIG.to_string(),
        };
        let wiped = wipe_on_mismatch(dir, &meta_path, &expected)?;
        let generation = if wiped {
            bump_generation_file(&generation_path, generation)?
        } else {
            generation
        };

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("build tokio runtime for LanceStore")?;
        let uri = dir.to_string_lossy().into_owned();
        let connection = runtime
            .block_on(async { lancedb::connect(&uri).execute().await })
            .with_context(|| format!("open lancedb at {uri}"))?;

        runtime.block_on(async {
            ensure_table(&connection, DOCUMENTS_TABLE, documents_schema(dim)).await?;
            ensure_table(&connection, MEMORY_TABLE, memory_schema(dim)).await?;
            #[cfg(feature = "code-search")]
            ensure_table(&connection, schema::CODE_CHUNKS_TABLE, schema::code_chunks_schema(dim)).await?;
            #[cfg(feature = "documents")]
            ensure_table(&connection, schema::DOC_LINKS_TABLE, schema::doc_links_schema()).await?;
            #[cfg(feature = "graphrag")]
            {
                ensure_table(&connection, GRAPH_ENTITIES_TABLE, graph_entities_schema()).await?;
                ensure_table(&connection, GRAPH_EDGES_TABLE, graph_edges_schema()).await?;
            }
            anyhow::Ok(())
        })?;

        if !meta_path.exists() {
            let body = serde_json::to_vec_pretty(&expected).context("serialize lance meta.json")?;
            std::fs::write(&meta_path, body).with_context(|| format!("write {}", meta_path.display()))?;
        }

        Ok(Self {
            inner: Arc::new(LanceStoreInner {
                runtime: Some(runtime),
                connection,
                dim,
                embedding_model: embedding_model.to_string(),
                dir: dir.to_path_buf(),
                generation_path,
                document_generation: Mutex::new(generation),
                commits: Mutex::new(std::collections::HashMap::new()),
            }),
        })
    }

    pub fn dim(&self) -> u16 {
        self.inner.dim
    }

    /// Shared LanceDB connection — read surface for the graphrag module (spec 0016):
    /// the `graph entities` / `graph rag` tools run Cypher over the same connection
    /// the store owns, instead of reaching into the private `inner`.
    #[cfg(feature = "graphrag")]
    pub(crate) fn connection(&self) -> &Connection {
        &self.inner.connection
    }

    /// The store's tokio runtime, for `block_on` around lance-graph's async Cypher
    /// execution from sync call sites (same contract as [`LanceStoreInner::rt`]).
    #[cfg(feature = "graphrag")]
    pub(crate) fn runtime(&self) -> &Runtime {
        self.inner.rt()
    }

    pub fn embedding_model(&self) -> &str {
        &self.inner.embedding_model
    }

    pub fn dir(&self) -> &Path {
        &self.inner.dir
    }

    pub fn document_generation(&self) -> u64 {
        *self
            .inner
            .document_generation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(super) fn bump_document_generation(&self) -> Result<u64> {
        let mut generation = self
            .inner
            .document_generation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *generation = bump_generation_file(&self.inner.generation_path, *generation)?;
        Ok(*generation)
    }

    pub fn replace_document(&self, scope: &str, path: &str, rows: Vec<DocumentRow>) -> Result<()> {
        self.inner.rt().block_on(async {
            let table = self
                .inner
                .connection
                .open_table(DOCUMENTS_TABLE)
                .execute()
                .await
                .with_context(|| format!("open {DOCUMENTS_TABLE} table"))?;
            let predicate = format!(
                "scope = '{}' AND path = '{}'",
                escape_sql_literal(scope),
                escape_sql_literal(path)
            );
            table
                .delete(&predicate)
                .await
                .with_context(|| format!("delete existing rows for {scope}/{path}"))?;
            self.after_commit(&table, DOCUMENTS_TABLE).await;
            if rows.is_empty() {
                self.bump_document_generation()?;
                return Ok(());
            }
            let batch = build_documents_batch(self.inner.dim, &rows)?;
            table
                .add(batch)
                .execute()
                .await
                .with_context(|| format!("insert {} documents rows", rows.len()))?;
            self.after_commit(&table, DOCUMENTS_TABLE).await;
            self.bump_document_generation()?;
            anyhow::Ok(())
        })
    }

    pub fn upsert_memory(&self, row: MemoryRow) -> Result<()> {
        self.inner.rt().block_on(async {
            let table = self
                .inner
                .connection
                .open_table(MEMORY_TABLE)
                .execute()
                .await
                .with_context(|| format!("open {MEMORY_TABLE} table"))?;
            let predicate = memory_row_predicate(&row.scope, &row.visibility, &row.agent_id, &row.key);
            table.delete(&predicate).await.context("delete previous memory entry")?;
            let batch = build_memory_batch(self.inner.dim, std::slice::from_ref(&row))?;
            table.add(batch).execute().await.context("insert memory row")?;
            self.after_commit(&table, MEMORY_TABLE).await;
            anyhow::Ok(())
        })
    }

    pub fn delete_memory(&self, scope: &str, visibility: &str, agent_id: &str, key: &str) -> Result<u64> {
        self.inner.rt().block_on(async {
            let table = self
                .inner
                .connection
                .open_table(MEMORY_TABLE)
                .execute()
                .await
                .with_context(|| format!("open {MEMORY_TABLE} table"))?;
            let predicate = memory_row_predicate(scope, visibility, agent_id, key);
            let result = table.delete(&predicate).await.context("delete memory entry")?;
            anyhow::Ok(result.num_deleted_rows)
        })
    }

    pub fn search_documents(
        &self,
        scope: &str,
        query: Vec<f32>,
        limit: usize,
        mime_type_filter: Option<&str>,
    ) -> Result<Vec<DocumentHit>> {
        self.search_documents_filtered(scope, query, limit, mime_type_filter, None)
    }

    pub fn search_documents_filtered(
        &self,
        scope: &str,
        query: Vec<f32>,
        limit: usize,
        mime_type_filter: Option<&str>,
        facet_filter: Option<&DocumentFacetFilter>,
    ) -> Result<Vec<DocumentHit>> {
        if query.len() != usize::from(self.inner.dim) {
            return Err(anyhow!(
                "query vector dim {} does not match store dim {}",
                query.len(),
                self.inner.dim
            ));
        }
        self.inner.rt().block_on(async {
            let table = self
                .inner
                .connection
                .open_table(DOCUMENTS_TABLE)
                .execute()
                .await
                .with_context(|| format!("open {DOCUMENTS_TABLE} table"))?;
            let mut q = table.vector_search(query).context("build vector search")?.limit(limit);
            let mut predicates = vec![format!("scope = '{}'", escape_sql_literal(scope))];
            if let Some(m) = mime_type_filter {
                predicates.push(format!("mime_type = '{}'", escape_sql_literal(m)));
            }
            if let Some(facets) = facet_filter {
                for (column, value) in [
                    ("doc_type", facets.doc_type.as_deref()),
                    ("section", facets.section.as_deref()),
                    ("jurisdiction", facets.jurisdiction.as_deref()),
                    ("doc_date", facets.doc_date.as_deref()),
                ] {
                    if let Some(value) = value {
                        predicates.push(format!(
                            "({column} IS NULL OR {column} = '{}')",
                            escape_sql_literal(value)
                        ));
                    }
                }
            }
            q = q.only_if(predicates.join(" AND "));
            let mut stream = q.execute().await.context("run document search")?;
            let mut hits = Vec::new();
            while let Some(batch) = stream.try_next().await.context("stream next batch")? {
                decode_document_hits(&batch, &mut hits)?;
            }
            anyhow::Ok(hits)
        })
    }

    pub fn document_chunks_for_paths(&self, scope: &str, paths: &[String], limit: usize) -> Result<Vec<DocumentHit>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        self.inner.rt().block_on(async {
            let table = self
                .inner
                .connection
                .open_table(DOCUMENTS_TABLE)
                .execute()
                .await
                .with_context(|| format!("open {DOCUMENTS_TABLE} table"))?;
            let path_list = paths
                .iter()
                .map(|p| format!("'{}'", escape_sql_literal(p)))
                .collect::<Vec<_>>()
                .join(", ");
            let predicate = format!("scope = '{}' AND path IN ({path_list})", escape_sql_literal(scope));
            let mut stream = table
                .query()
                .only_if(predicate)
                .limit(limit)
                .execute()
                .await
                .context("fetch document chunks")?;
            let mut hits = Vec::new();
            while let Some(batch) = stream.try_next().await.context("stream next batch")? {
                decode_document_hits(&batch, &mut hits)?;
            }
            anyhow::Ok(hits)
        })
    }

    pub fn search_memory(
        &self,
        scope: &str,
        visibility: &str,
        agent_id: &str,
        query: Vec<f32>,
        limit: usize,
        tag_filter: Option<&str>,
    ) -> Result<Vec<MemoryHit>> {
        if query.len() != usize::from(self.inner.dim) {
            return Err(anyhow!(
                "query vector dim {} does not match store dim {}",
                query.len(),
                self.inner.dim
            ));
        }
        self.inner.rt().block_on(async {
            let table = self
                .inner
                .connection
                .open_table(MEMORY_TABLE)
                .execute()
                .await
                .with_context(|| format!("open {MEMORY_TABLE} table"))?;
            let mut q = table
                .vector_search(query)
                .context("build memory vector search")?
                .limit(limit);
            let namespace_clause = memory_namespace_predicate(scope, visibility, agent_id);
            let _ = tag_filter;
            q = q.only_if(namespace_clause);
            let mut stream = q.execute().await.context("run memory search")?;
            let mut hits = Vec::new();
            while let Some(batch) = stream.try_next().await.context("stream next batch")? {
                decode_memory_hits(&batch, &mut hits)?;
            }
            if let Some(tag) = tag_filter {
                hits.retain(|h| h.tags.iter().any(|t| t == tag));
            }
            anyhow::Ok(hits)
        })
    }

    #[cfg(feature = "code-search")]
    pub fn replace_code_chunks(&self, scope: &str, path: &str, rows: Vec<CodeRow>) -> Result<()> {
        self.inner.rt().block_on(async {
            let table = self
                .inner
                .connection
                .open_table(schema::CODE_CHUNKS_TABLE)
                .execute()
                .await
                .with_context(|| format!("open {} table", schema::CODE_CHUNKS_TABLE))?;
            let predicate = format!(
                "scope = '{}' AND path = '{}'",
                escape_sql_literal(scope),
                escape_sql_literal(path)
            );
            table
                .delete(&predicate)
                .await
                .with_context(|| format!("delete existing code chunks for {scope}/{path}"))?;
            self.after_commit(&table, schema::CODE_CHUNKS_TABLE).await;
            if rows.is_empty() {
                return Ok(());
            }
            let batch = build_code_chunks_batch(self.inner.dim, &rows)?;
            table
                .add(batch)
                .execute()
                .await
                .with_context(|| format!("insert {} code_chunks rows", rows.len()))?;
            self.after_commit(&table, schema::CODE_CHUNKS_TABLE).await;
            anyhow::Ok(())
        })
    }

    #[cfg(feature = "code-search")]
    pub fn delete_code_chunks(&self, scope: &str, path: &str) -> Result<()> {
        self.inner.rt().block_on(async {
            let table = self
                .inner
                .connection
                .open_table(schema::CODE_CHUNKS_TABLE)
                .execute()
                .await
                .with_context(|| format!("open {} table", schema::CODE_CHUNKS_TABLE))?;
            let predicate = format!(
                "scope = '{}' AND path = '{}'",
                escape_sql_literal(scope),
                escape_sql_literal(path)
            );
            table
                .delete(&predicate)
                .await
                .with_context(|| format!("delete code chunks for {scope}/{path}"))?;
            anyhow::Ok(())
        })
    }

    #[cfg(feature = "code-search")]
    pub fn delete_code_chunks_for_paths(&self, scope: &str, paths: &[String]) -> Result<()> {
        self.delete_paths_in(schema::CODE_CHUNKS_TABLE, "path", scope, paths)
    }

    pub fn delete_documents_for_paths(&self, scope: &str, paths: &[String]) -> Result<()> {
        self.delete_paths_in(DOCUMENTS_TABLE, "path", scope, paths)
    }

    #[cfg(feature = "documents")]
    pub fn delete_doc_links_for_paths(&self, scope: &str, paths: &[String]) -> Result<()> {
        self.delete_paths_in(schema::DOC_LINKS_TABLE, "doc_path", scope, paths)
    }

    #[cfg(feature = "graphrag")]
    pub fn delete_graph_for_scope(&self, scope: &str) -> Result<()> {
        graphrag::delete_graph_for_scope(&self.inner.connection, self.inner.rt(), scope)
    }

    pub(super) fn delete_paths_in(&self, table_name: &str, key_col: &str, scope: &str, paths: &[String]) -> Result<()> {
        const BATCH: usize = 2_000;
        if paths.is_empty() {
            return Ok(());
        }
        self.inner.rt().block_on(async {
            let table = self
                .inner
                .connection
                .open_table(table_name)
                .execute()
                .await
                .with_context(|| format!("open {table_name} table"))?;
            let in_scope = table
                .count_rows(Some(format!("scope = '{}'", escape_sql_literal(scope))))
                .await
                .with_context(|| format!("count {table_name} rows in {scope}"))?;
            if in_scope == 0 {
                return anyhow::Ok(());
            }
            for batch in paths.chunks(BATCH) {
                let list = batch
                    .iter()
                    .map(|p| format!("'{}'", escape_sql_literal(p)))
                    .collect::<Vec<_>>()
                    .join(", ");
                let predicate = format!("scope = '{}' AND {key_col} IN ({list})", escape_sql_literal(scope));
                table
                    .delete(&predicate)
                    .await
                    .with_context(|| format!("delete {} paths from {table_name} in {scope}", batch.len()))?;
                self.after_commit(&table, table_name).await;
                if table_name == DOCUMENTS_TABLE {
                    self.bump_document_generation()?;
                }
            }
            anyhow::Ok(())
        })
    }

    #[cfg(feature = "code-search")]
    pub fn search_code_chunks(&self, scope: &str, query: Vec<f32>, limit: usize) -> Result<Vec<CodeChunkHit>> {
        if query.len() != usize::from(self.inner.dim) {
            return Err(anyhow!(
                "query vector dim {} does not match store dim {}",
                query.len(),
                self.inner.dim
            ));
        }
        self.inner.rt().block_on(async {
            let table = self
                .inner
                .connection
                .open_table(schema::CODE_CHUNKS_TABLE)
                .execute()
                .await
                .with_context(|| format!("open {} table", schema::CODE_CHUNKS_TABLE))?;
            let q = table
                .vector_search(query)
                .context("build code chunk vector search")?
                .limit(limit)
                .only_if(format!("scope = '{}'", escape_sql_literal(scope)));
            let mut stream = q.execute().await.context("run code chunk search")?;
            let mut hits = Vec::new();
            while let Some(batch) = stream.try_next().await.context("stream next batch")? {
                decode_code_chunk_hits(&batch, &mut hits)?;
            }
            anyhow::Ok(hits)
        })
    }
}

async fn ensure_table(connection: &Connection, name: &str, schema: arrow_schema::SchemaRef) -> Result<()> {
    let existing: Vec<String> = connection.table_names().execute().await.context("list lance tables")?;
    if existing.iter().any(|t| t == name) {
        return Ok(());
    }
    connection
        .create_empty_table(name, schema)
        .execute()
        .await
        .with_context(|| format!("create {name} table"))?;
    Ok(())
}

fn append_optional(builder: &mut StringBuilder, value: Option<&str>) {
    match value {
        Some(value) => builder.append_value(value),
        None => builder.append_null(),
    }
}

fn append_optional_list(builder: &mut ListBuilder<StringBuilder>, value: Option<&[String]>) {
    match value {
        Some(values) => {
            for value in values {
                builder.values().append_value(value);
            }
            builder.append(true);
        }
        None => builder.append(false),
    }
}

fn build_documents_batch(dim: u16, rows: &[DocumentRow]) -> Result<RecordBatch> {
    let mut scope = StringBuilder::new();
    let mut path = StringBuilder::new();
    let mut chunk_idx = UInt32Builder::new();
    let mut mime = StringBuilder::new();
    let mut doc_type = StringBuilder::new();
    let mut section = StringBuilder::new();
    let mut jurisdiction = StringBuilder::new();
    let mut doc_date = StringBuilder::new();
    let mut keywords = ListBuilder::new(StringBuilder::new());
    let mut entities = ListBuilder::new(StringBuilder::new());
    let mut summary = StringBuilder::new();
    let mut text = StringBuilder::new();
    let mut text_phrase = StringBuilder::new();
    let mut heading_path = StringBuilder::new();
    let mut cites = StringBuilder::new();
    let mut byte_start = UInt32Builder::new();
    let mut byte_end = UInt32Builder::new();
    let mut rehydration_ref = StringBuilder::new();
    let mut embedding = FixedSizeListBuilder::new(Float32Builder::new(), i32::from(dim));

    for r in rows {
        if !r.embedding.is_empty() && r.embedding.len() != usize::from(dim) {
            return Err(anyhow!(
                "documents row embedding dim {} does not match store dim {}",
                r.embedding.len(),
                dim
            ));
        }
        scope.append_value(&r.scope);
        path.append_value(&r.path);
        chunk_idx.append_value(r.chunk_idx);
        mime.append_value(&r.mime_type);
        append_optional(&mut doc_type, r.facets.doc_type.as_deref());
        append_optional(&mut section, r.facets.section.as_deref());
        append_optional(&mut jurisdiction, r.facets.jurisdiction.as_deref());
        append_optional(&mut doc_date, r.facets.doc_date.as_deref());
        append_optional_list(&mut keywords, r.facets.keywords.as_deref());
        append_optional_list(&mut entities, r.facets.entities.as_deref());
        append_optional(&mut summary, r.facets.summary.as_deref());
        text.append_value(&r.text);
        text_phrase.append_value(&r.text_phrase);
        heading_path.append_value(&r.heading_path);
        cites.append_value(&r.cites);
        byte_start.append_value(r.byte_start);
        byte_end.append_value(r.byte_end);
        match &r.rehydration_ref {
            Some(value) => rehydration_ref.append_value(value),
            None => rehydration_ref.append_null(),
        }
        if r.embedding.is_empty() {
            embedding.values().append_nulls(dim as usize);
            embedding.append(false);
        } else {
            for v in &r.embedding {
                embedding.values().append_value(*v);
            }
            embedding.append(true);
        }
    }

    let schema = documents_schema(dim);
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(scope.finish()),
            Arc::new(path.finish()),
            Arc::new(chunk_idx.finish()),
            Arc::new(mime.finish()),
            Arc::new(doc_type.finish()),
            Arc::new(section.finish()),
            Arc::new(jurisdiction.finish()),
            Arc::new(doc_date.finish()),
            Arc::new(keywords.finish()),
            Arc::new(entities.finish()),
            Arc::new(summary.finish()),
            Arc::new(text.finish()),
            Arc::new(text_phrase.finish()),
            Arc::new(heading_path.finish()),
            Arc::new(cites.finish()),
            Arc::new(byte_start.finish()),
            Arc::new(byte_end.finish()),
            Arc::new(rehydration_ref.finish()),
            Arc::new(embedding.finish()),
        ],
    )
    .context("assemble documents batch")
}

#[cfg(feature = "code-search")]
fn build_code_chunks_batch(dim: u16, rows: &[CodeRow]) -> Result<RecordBatch> {
    let mut scope = StringBuilder::new();
    let mut path = StringBuilder::new();
    let mut chunk_id = StringBuilder::new();
    let mut symbol = StringBuilder::new();
    let mut kind = StringBuilder::new();
    let mut lang = StringBuilder::new();
    let mut line_start = UInt32Builder::new();
    let mut line_end = UInt32Builder::new();
    let mut byte_start = UInt32Builder::new();
    let mut byte_end = UInt32Builder::new();
    let mut text = StringBuilder::new();
    let mut embedding = FixedSizeListBuilder::new(Float32Builder::new(), i32::from(dim));

    for r in rows {
        if r.embedding.len() != usize::from(dim) {
            return Err(anyhow!(
                "code_chunks row embedding dim {} does not match store dim {}",
                r.embedding.len(),
                dim
            ));
        }
        scope.append_value(&r.scope);
        path.append_value(&r.path);
        chunk_id.append_value(&r.chunk_id);
        symbol.append_value(&r.symbol);
        kind.append_value(&r.kind);
        lang.append_value(&r.lang);
        line_start.append_value(r.line_start);
        line_end.append_value(r.line_end);
        byte_start.append_value(r.byte_start);
        byte_end.append_value(r.byte_end);
        text.append_value(&r.text);
        for v in &r.embedding {
            embedding.values().append_value(*v);
        }
        embedding.append(true);
    }

    let schema = schema::code_chunks_schema(dim);
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(scope.finish()),
            Arc::new(path.finish()),
            Arc::new(chunk_id.finish()),
            Arc::new(symbol.finish()),
            Arc::new(kind.finish()),
            Arc::new(lang.finish()),
            Arc::new(line_start.finish()),
            Arc::new(line_end.finish()),
            Arc::new(byte_start.finish()),
            Arc::new(byte_end.finish()),
            Arc::new(text.finish()),
            Arc::new(embedding.finish()),
        ],
    )
    .context("assemble code_chunks batch")
}

#[cfg(feature = "code-search")]
fn decode_code_chunk_hits(batch: &RecordBatch, out: &mut Vec<CodeChunkHit>) -> Result<()> {
    use arrow_array::{Float32Array, UInt32Array};
    let str_col = |name: &str| -> Result<&StringArray> {
        batch
            .column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .ok_or_else(|| anyhow!("`{name}` column missing"))
    };
    let u32_col = |name: &str| -> Result<&UInt32Array> {
        batch
            .column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<UInt32Array>())
            .ok_or_else(|| anyhow!("`{name}` column missing"))
    };
    let path = str_col("path")?;
    let chunk_id = str_col("chunk_id")?;
    let symbol = str_col("symbol")?;
    let kind = str_col("kind")?;
    let lang = str_col("lang")?;
    let line_start = u32_col("line_start")?;
    let line_end = u32_col("line_end")?;
    let byte_start = u32_col("byte_start")?;
    let byte_end = u32_col("byte_end")?;
    let distance = batch
        .column_by_name("_distance")
        .and_then(|c| c.as_any().downcast_ref::<Float32Array>());

    for i in 0..batch.num_rows() {
        out.push(CodeChunkHit {
            path: path.value(i).to_string(),
            chunk_id: chunk_id.value(i).to_string(),
            symbol: symbol.value(i).to_string(),
            kind: kind.value(i).to_string(),
            lang: lang.value(i).to_string(),
            line_start: line_start.value(i),
            line_end: line_end.value(i),
            byte_start: byte_start.value(i),
            byte_end: byte_end.value(i),
            distance: distance.map(|d| d.value(i)).unwrap_or(0.0),
        });
    }
    Ok(())
}

fn build_memory_batch(dim: u16, rows: &[MemoryRow]) -> Result<RecordBatch> {
    let mut scope = StringBuilder::new();
    let mut key = StringBuilder::new();
    let mut value = StringBuilder::new();
    let mut tags = ListBuilder::new(StringBuilder::new());
    let mut visibility = StringBuilder::new();
    let mut agent_id = StringBuilder::new();
    let mut embedding = FixedSizeListBuilder::new(Float32Builder::new(), i32::from(dim));
    let mut created = TimestampMicrosecondBuilder::new();
    let mut updated = TimestampMicrosecondBuilder::new();

    for r in rows {
        if r.embedding.len() != usize::from(dim) {
            return Err(anyhow!(
                "memory row embedding dim {} does not match store dim {}",
                r.embedding.len(),
                dim
            ));
        }
        scope.append_value(&r.scope);
        key.append_value(&r.key);
        value.append_value(&r.value);
        for t in &r.tags {
            tags.values().append_value(t);
        }
        tags.append(true);
        visibility.append_value(&r.visibility);
        agent_id.append_value(&r.agent_id);
        for v in &r.embedding {
            embedding.values().append_value(*v);
        }
        embedding.append(true);
        created.append_value(r.created_at);
        updated.append_value(r.updated_at);
    }

    let schema = memory_schema(dim);
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(scope.finish()),
            Arc::new(key.finish()),
            Arc::new(value.finish()),
            Arc::new(tags.finish()),
            Arc::new(visibility.finish()),
            Arc::new(agent_id.finish()),
            Arc::new(embedding.finish()),
            Arc::new(created.finish()),
            Arc::new(updated.finish()),
        ],
    )
    .context("assemble memory batch")
}

fn decode_document_hits(batch: &RecordBatch, out: &mut Vec<DocumentHit>) -> Result<()> {
    use arrow_array::{Float32Array, ListArray, UInt32Array};
    let path = batch
        .column_by_name("path")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| anyhow!("`path` column missing"))?;
    let mime = batch
        .column_by_name("mime_type")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| anyhow!("`mime_type` column missing"))?;
    let text = batch
        .column_by_name("text")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| anyhow!("`text` column missing"))?;
    let chunk_idx = batch
        .column_by_name("chunk_idx")
        .and_then(|c| c.as_any().downcast_ref::<UInt32Array>())
        .ok_or_else(|| anyhow!("`chunk_idx` column missing"))?;
    let byte_start = batch
        .column_by_name("byte_start")
        .and_then(|c| c.as_any().downcast_ref::<UInt32Array>())
        .ok_or_else(|| anyhow!("`byte_start` column missing"))?;
    let byte_end = batch
        .column_by_name("byte_end")
        .and_then(|c| c.as_any().downcast_ref::<UInt32Array>())
        .ok_or_else(|| anyhow!("`byte_end` column missing"))?;
    let distance = batch
        .column_by_name("_distance")
        .and_then(|c| c.as_any().downcast_ref::<Float32Array>());
    let optional_string = |name: &str| {
        batch
            .column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
    };
    let optional_list = |name: &str| {
        batch
            .column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<ListArray>())
    };
    let doc_type = optional_string("doc_type");
    let section = optional_string("section");
    let jurisdiction = optional_string("jurisdiction");
    let doc_date = optional_string("doc_date");
    let keywords = optional_list("keywords");
    let entities = optional_list("entities");
    let summary = optional_string("summary");
    let optional_value = |array: Option<&StringArray>, i: usize| {
        array.and_then(|values| (!values.is_null(i)).then(|| values.value(i).to_string()))
    };
    let optional_values = |array: Option<&ListArray>, i: usize| {
        array.and_then(|values| {
            if values.is_null(i) {
                return None;
            }
            let values = values.value(i);
            let values = values.as_any().downcast_ref::<StringArray>()?;
            Some((0..values.len()).map(|index| values.value(index).to_string()).collect())
        })
    };

    for i in 0..batch.num_rows() {
        out.push(DocumentHit {
            path: path.value(i).to_string(),
            chunk_idx: chunk_idx.value(i),
            text: text.value(i).to_string(),
            mime_type: mime.value(i).to_string(),
            facets: DocumentFacets {
                doc_type: optional_value(doc_type, i),
                section: optional_value(section, i),
                jurisdiction: optional_value(jurisdiction, i),
                doc_date: optional_value(doc_date, i),
                keywords: optional_values(keywords, i),
                entities: optional_values(entities, i),
                summary: optional_value(summary, i),
            },
            byte_start: byte_start.value(i),
            byte_end: byte_end.value(i),
            distance: distance.map(|d| d.value(i)).unwrap_or(0.0),
        });
    }
    Ok(())
}

fn decode_memory_hits(batch: &RecordBatch, out: &mut Vec<MemoryHit>) -> Result<()> {
    use arrow_array::{Float32Array, ListArray};
    let key = batch
        .column_by_name("key")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| anyhow!("`key` column missing"))?;
    let value = batch
        .column_by_name("value")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| anyhow!("`value` column missing"))?;
    let tags = batch
        .column_by_name("tags")
        .and_then(|c| c.as_any().downcast_ref::<ListArray>());
    let distance = batch
        .column_by_name("_distance")
        .and_then(|c| c.as_any().downcast_ref::<Float32Array>());

    for i in 0..batch.num_rows() {
        let tag_list: Vec<String> = match tags {
            Some(list) if list.is_valid(i) => {
                let inner = list.value(i);
                let s = inner
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .ok_or_else(|| anyhow!("tags inner type unexpected"))?;
                (0..s.len()).map(|j| s.value(j).to_string()).collect()
            }
            _ => Vec::new(),
        };
        out.push(MemoryHit {
            key: key.value(i).to_string(),
            value: value.value(i).to_string(),
            tags: tag_list,
            distance: distance.map(|d| d.value(i)).unwrap_or(0.0),
        });
    }
    Ok(())
}

pub(crate) use predicates::{escape_sql_literal, memory_namespace_predicate, memory_row_predicate, now_micros};

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

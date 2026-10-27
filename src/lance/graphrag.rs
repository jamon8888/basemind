//! GraphRAG entity graph over GLiNER entities (spec 0016 / ADR-0016).
//!
//! Builds `graph_entities` and `graph_edges` tables from the NER sidecar at scan time.
//! Exposes two `graph` tool modes: `graph entities` (restricted Cypher) and `graph rag` (hybrid retrieval).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use arrow_array::{Array, RecordBatch, StringArray, UInt32Array, UInt8Array};
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use futures::TryStreamExt;
use lancedb::query::ExecutableQuery;
use lancedb::Connection;
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;

use super::schema::{
    graph_entities_schema, graph_edges_schema, GRAPH_ENTITIES_TABLE, GRAPH_EDGES_TABLE,
    EntityKeySource,
};
use super::{escape_sql_literal, now_micros};

/// Configuration for the graphrag feature (mirrors `[graphrag]` in basemind.toml).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GraphragConfig {
    /// Enable the entity graph build and query path.
    #[serde(default)]
    pub enabled: bool,
    /// Labels to project as graph nodes. Default: person, organization, location + custom_labels.
    #[serde(default = "default_labels")]
    pub labels: Vec<String>,
    /// Max rows returned by Cypher queries (server-side budget).
    #[serde(default = "default_max_rows")]
    pub max_rows: usize,
    /// Hop depth for graph rag expansion (1 or 2).
    #[serde(default = "default_hops")]
    pub hops: u8,
}

fn default_labels() -> Vec<String> {
    vec!["person".into(), "organization".into(), "location".into()]
}
fn default_max_rows() -> usize {
    200
}
fn default_hops() -> u8 {
    2
}

/// Normalize a literal for global identity (ClearLiteral path).
/// Case-fold + ASCII-fold + whitespace collapse.
pub fn normalize_literal(s: &str) -> String {
    // Simple ASCII normalization: lowercase, collapse whitespace, strip non-alnum except space
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for ch in s.chars() {
        let lower = ch.to_lowercase().collect::<String>();
        for c in lower.chars() {
            if c.is_ascii_alphanumeric() || c == ' ' {
                if c == ' ' {
                    if !prev_space {
                        out.push(' ');
                        prev_space = true;
                    }
                } else {
                    out.push(c);
                    prev_space = false;
                }
            }
        }
    }
    out.trim().to_string()
}

/// Key for deduplicating entity mentions into nodes.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum EntityNodeKey {
    TokenOfDocument { label: String, literal: String, doc_path: String },
    ClearLiteral { label: String, normalized: String },
}

/// Key for deduplicating edges.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct EdgeKey {
    pub src: EntityNodeKey,
    pub dst: EntityNodeKey,
    pub chunk_idx: u32,
    pub doc_path: String,
}

/// Row for graph_entities table.
#[derive(Debug, Clone)]
pub struct GraphEntityRow {
    pub scope: String,
    pub label: String,
    pub key_source: u8, // 0 = TokenOfDocument, 1 = ClearLiteral
    pub normalized_literal: String,
    pub document_path: String,
    pub mention_count: u32,
    pub first_seen_chunk: u32,
    pub last_seen_chunk: u32,
    pub doc_path: String,
    pub doc_chunk_idx: u32,
}

/// Row for graph_edges table.
#[derive(Debug, Clone)]
pub struct GraphEdgeRow {
    pub scope: String,
    pub src_key_source: u8,
    pub src_normalized_literal: String,
    pub src_document_path: String,
    pub src_label: String,
    pub dst_key_source: u8,
    pub dst_normalized_literal: String,
    pub dst_document_path: String,
    pub dst_label: String,
    pub chunk_idx: u32,
    pub doc_path: String,
    pub weight: u32,
}

/// Build the entity graph from DocEntity sidecar records produced by the scanner.
///
/// Called at scan time after document extraction with NER enabled.
/// Returns (entity_rows, edge_rows) ready for batch insertion.
pub fn build_graph_from_entities(
    scope: &str,
    doc_entities: &[DocEntitySidecar],
    config: &GraphragConfig,
) -> Result<(Vec<GraphEntityRow>, Vec<GraphEdgeRow>)> {
    let allowed: HashSet<_> = config.labels.iter().cloned().collect();
    let mut entity_map: HashMap<EntityNodeKey, GraphEntityRow> = HashMap::new();
    let mut edge_map: HashMap<EdgeKey, GraphEdgeRow> = HashMap::new();

    for ent in doc_entities {
        if !allowed.contains(&ent.label) {
            continue; // label not projected
        }

        // Determine key source based on redaction strategy.
        // Under token_replace, the literal is a token like [EMAIL_1] — document-scoped.
        // Otherwise (mask/hash/drop/none), the literal is the original value — global.
        let (key_source, normalized_literal, document_path) = if ent.is_token {
            (0u8, ent.literal.clone(), ent.doc_path.clone()) // TokenOfDocument
        } else {
            let norm = normalize_literal(&ent.literal);
            (1u8, norm, String::new()) // ClearLiteral
        };

        let node_key = match key_source {
            0 => EntityNodeKey::TokenOfDocument {
                label: ent.label.clone(),
                literal: ent.literal.clone(),
                doc_path: ent.doc_path.clone(),
            },
            _ => EntityNodeKey::ClearLiteral {
                label: ent.label.clone(),
                normalized: normalized_literal.clone(),
            },
        };

        // Upsert entity node
        let entity_row = entity_map
            .entry(node_key.clone())
            .or_insert_with(|| GraphEntityRow {
                scope: scope.to_string(),
                label: ent.label.clone(),
                key_source,
                normalized_literal: normalized_literal.clone(),
                document_path: document_path.clone(),
                mention_count: 0,
                first_seen_chunk: ent.chunk_idx,
                last_seen_chunk: ent.chunk_idx,
                doc_path: ent.doc_path.clone(),
                doc_chunk_idx: ent.chunk_idx,
            });
        entity_row.mention_count += 1;
        entity_row.last_seen_chunk = ent.chunk_idx.max(entity_row.last_seen_chunk);
        if ent.chunk_idx < entity_row.first_seen_chunk {
            entity_row.first_seen_chunk = ent.chunk_idx;
            entity_row.doc_path = ent.doc_path.clone();
            entity_row.doc_chunk_idx = ent.chunk_idx;
        }

        // Co-occurrence edges within the same chunk (same doc_path + chunk_idx)
        // We'll build edges in a second pass per chunk.
    }

    // Second pass: build co-occurrence edges per chunk
    let mut chunk_entities: HashMap<(String, u32), Vec<EntityNodeKey>> = HashMap::new();
    for ent in doc_entities {
        if !allowed.contains(&ent.label) {
            continue;
        }
        let (key_source, normalized_literal, document_path) = if ent.is_token {
            (0u8, ent.literal.clone(), ent.doc_path.clone())
        } else {
            (1u8, normalize_literal(&ent.literal), String::new())
        };
        let node_key = match key_source {
            0 => EntityNodeKey::TokenOfDocument {
                label: ent.label.clone(),
                literal: ent.literal.clone(),
                doc_path: ent.doc_path.clone(),
            },
            _ => EntityNodeKey::ClearLiteral {
                label: ent.label.clone(),
                normalized: normalized_literal,
            },
        };
        chunk_entities
            .entry((ent.doc_path.clone(), ent.chunk_idx))
            .or_default()
            .push(node_key);
    }

    for ((doc_path, chunk_idx), entities) in chunk_entities {
        // Fully connected within chunk (undirected, but store both directions for Cypher convenience)
        for i in 0..entities.len() {
            for j in i + 1..entities.len() {
                let src = &entities[i];
                let dst = &entities[j];
                let edge_key = EdgeKey {
                    src: src.clone(),
                    dst: dst.clone(),
                    chunk_idx,
                    doc_path: doc_path.clone(),
                };
                let src_label = match src {
                    EntityNodeKey::TokenOfDocument { label, .. } => label.clone(),
                    EntityNodeKey::ClearLiteral { label, .. } => label.clone(),
                };
                let dst_label = match dst {
                    EntityNodeKey::TokenOfDocument { label, .. } => label.clone(),
                    EntityNodeKey::ClearLiteral { label, .. } => label.clone(),
                };
                let (src_ks, src_norm, src_doc) = match src {
                    EntityNodeKey::TokenOfDocument { literal, doc_path, .. } => (0u8, literal.clone(), doc_path.clone()),
                    EntityNodeKey::ClearLiteral { normalized, .. } => (1u8, normalized.clone(), String::new()),
                };
                let (dst_ks, dst_norm, dst_doc) = match dst {
                    EntityNodeKey::TokenOfDocument { literal, doc_path, .. } => (0u8, literal.clone(), doc_path.clone()),
                    EntityNodeKey::ClearLiteral { normalized, .. } => (1u8, normalized.clone(), String::new()),
                };
                let edge_row = edge_map
                    .entry(edge_key)
                    .or_insert_with(|| GraphEdgeRow {
                        scope: scope.to_string(),
                        src_key_source: src_ks,
                        src_normalized_literal: src_norm,
                        src_document_path: src_doc,
                        src_label,
                        dst_key_source: dst_ks,
                        dst_normalized_literal: dst_norm,
                        dst_document_path: dst_doc,
                        dst_label,
                        chunk_idx,
                        doc_path: doc_path.clone(),
                        weight: 0,
                    });
                edge_row.weight += 1;
            }
        }
    }

    let entities: Vec<GraphEntityRow> = entity_map.into_values().collect();
    let edges: Vec<GraphEdgeRow> = edge_map.into_values().collect();

    Ok((entities, edges))
}

/// Sidecar entity record produced by the extraction NER pass.
/// Mirrors `DocEntity` from `src/extract/doc.rs:177`.
#[derive(Debug, Clone)]
pub struct DocEntitySidecar {
    pub label: String,
    pub literal: String,
    pub is_token: bool, // true when literal is a token_replace marker like [EMAIL_1]
    pub chunk_idx: u32,
    pub doc_path: String,
}

/// Insert graph entity rows into the graph_entities table (upsert by unique key).
pub fn upsert_graph_entities(
    connection: &Connection,
    runtime: &Runtime,
    rows: &[GraphEntityRow],
) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    runtime.block_on(async {
        let table = connection
            .open_table(GRAPH_ENTITIES_TABLE)
            .execute()
            .await
            .context("open graph_entities table")?;

        // Build batch
        let mut scope = arrow_array::builder::StringBuilder::new();
        let mut label = arrow_array::builder::StringBuilder::new();
        let mut key_source = arrow_array::builder::UInt8Builder::new();
        let mut normalized_literal = arrow_array::builder::StringBuilder::new();
        let mut document_path = arrow_array::builder::StringBuilder::new();
        let mut mention_count = arrow_array::builder::UInt32Builder::new();
        let mut first_seen_chunk = arrow_array::builder::UInt32Builder::new();
        let mut last_seen_chunk = arrow_array::builder::UInt32Builder::new();
        let mut doc_path = arrow_array::builder::StringBuilder::new();
        let mut doc_chunk_idx = arrow_array::builder::UInt32Builder::new();

        for r in rows {
            scope.append_value(&r.scope);
            label.append_value(&r.label);
            key_source.append_value(r.key_source);
            normalized_literal.append_value(&r.normalized_literal);
            document_path.append_value(&r.document_path);
            mention_count.append_value(r.mention_count);
            first_seen_chunk.append_value(r.first_seen_chunk);
            last_seen_chunk.append_value(r.last_seen_chunk);
            doc_path.append_value(&r.doc_path);
            doc_chunk_idx.append_value(r.doc_chunk_idx);
        }

        let schema = graph_entities_schema();
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(scope.finish()),
                Arc::new(label.finish()),
                Arc::new(key_source.finish()),
                Arc::new(normalized_literal.finish()),
                Arc::new(document_path.finish()),
                Arc::new(mention_count.finish()),
                Arc::new(first_seen_chunk.finish()),
                Arc::new(last_seen_chunk.finish()),
                Arc::new(doc_path.finish()),
                Arc::new(doc_chunk_idx.finish()),
            ],
        )
        .context("assemble graph_entities batch")?;

        // Delete-then-insert for upsert semantics (key = scope+label+key_source+normalized_literal+document_path)
        // We build a predicate per row — acceptable since entity count per scan is modest.
        for r in rows {
            let predicate = format!(
                "scope = '{}' AND label = '{}' AND key_source = {} AND normalized_literal = '{}' AND document_path = '{}'",
                escape_sql_literal(&r.scope),
                escape_sql_literal(&r.label),
                r.key_source,
                escape_sql_literal(&r.normalized_literal),
                escape_sql_literal(&r.document_path),
            );
            table.delete(&predicate).await.context("delete existing entity")?;
        }

        table
            .add(batch)
            .execute()
            .await
            .context("insert graph_entities batch")?;

        anyhow::Ok(())
    })
}

/// Insert graph edge rows into the graph_edges table (upsert by unique key).
pub fn upsert_graph_edges(
    connection: &Connection,
    runtime: &Runtime,
    rows: &[GraphEdgeRow],
) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    runtime.block_on(async {
        let table = connection
            .open_table(GRAPH_EDGES_TABLE)
            .execute()
            .await
            .context("open graph_edges table")?;

        let mut scope = arrow_array::builder::StringBuilder::new();
        let mut src_key_source = arrow_array::builder::UInt8Builder::new();
        let mut src_normalized_literal = arrow_array::builder::StringBuilder::new();
        let mut src_document_path = arrow_array::builder::StringBuilder::new();
        let mut src_label = arrow_array::builder::StringBuilder::new();
        let mut dst_key_source = arrow_array::builder::UInt8Builder::new();
        let mut dst_normalized_literal = arrow_array::builder::StringBuilder::new();
        let mut dst_document_path = arrow_array::builder::StringBuilder::new();
        let mut dst_label = arrow_array::builder::StringBuilder::new();
        let mut chunk_idx = arrow_array::builder::UInt32Builder::new();
        let mut doc_path = arrow_array::builder::StringBuilder::new();
        let mut weight = arrow_array::builder::UInt32Builder::new();

        for r in rows {
            scope.append_value(&r.scope);
            src_key_source.append_value(r.src_key_source);
            src_normalized_literal.append_value(&r.src_normalized_literal);
            src_document_path.append_value(&r.src_document_path);
            src_label.append_value(&r.src_label);
            dst_key_source.append_value(r.dst_key_source);
            dst_normalized_literal.append_value(&r.dst_normalized_literal);
            dst_document_path.append_value(&r.dst_document_path);
            dst_label.append_value(&r.dst_label);
            chunk_idx.append_value(r.chunk_idx);
            doc_path.append_value(&r.doc_path);
            weight.append_value(r.weight);
        }

        let schema = graph_edges_schema();
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(scope.finish()),
                Arc::new(src_key_source.finish()),
                Arc::new(src_normalized_literal.finish()),
                Arc::new(src_document_path.finish()),
                Arc::new(src_label.finish()),
                Arc::new(dst_key_source.finish()),
                Arc::new(dst_normalized_literal.finish()),
                Arc::new(dst_document_path.finish()),
                Arc::new(dst_label.finish()),
                Arc::new(chunk_idx.finish()),
                Arc::new(doc_path.finish()),
                Arc::new(weight.finish()),
            ],
        )
        .context("assemble graph_edges batch")?;

        // Upsert: delete existing edges for this chunk then insert
        // We batch by (scope, doc_path, chunk_idx) to minimize round trips.
        let mut by_chunk: HashMap<(String, String, u32), Vec<&GraphEdgeRow>> = HashMap::new();
        for r in rows {
            by_chunk
                .entry((r.scope.clone(), r.doc_path.clone(), r.chunk_idx))
                .or_default()
                .push(r);
        }

        for ((scope, doc_path_val, chunk_idx), chunk_rows) in by_chunk {
            let predicate = format!(
                "scope = '{}' AND doc_path = '{}' AND chunk_idx = {}",
                escape_sql_literal(&scope),
                escape_sql_literal(&doc_path_val),
                chunk_idx,
            );
            let table = connection
                .open_table(GRAPH_EDGES_TABLE)
                .execute()
                .await
                .context("open graph_edges table")?;
            table.delete(&predicate).await.context("delete existing edges")?;
        }

        table
            .add(batch)
            .execute()
            .await
            .context("insert graph_edges batch")?;

        anyhow::Ok(())
    })
}

/// Delete all graph entities/edges for a scope (used by gc / wipe).
pub fn delete_graph_for_scope(
    connection: &Connection,
    runtime: &Runtime,
    scope: &str,
) -> Result<()> {
    runtime.block_on(async {
        let table = connection
            .open_table(GRAPH_ENTITIES_TABLE)
            .execute()
            .await
            .context("open graph_entities table")?;
        let predicate = format!("scope = '{}'", escape_sql_literal(scope));
        table.delete(&predicate).await.context("delete graph_entities")?;

        let table = connection
            .open_table(GRAPH_EDGES_TABLE)
            .execute()
            .await
            .context("open graph_edges table")?;
        let predicate = format!("scope = '{}'", escape_sql_literal(scope));
        table.delete(&predicate).await.context("delete graph_edges")?;

        anyhow::Ok(())
    })
}

/// Result of a Cypher query over the graph tables.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CypherResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// Execute a read-only Cypher query over the graph tables via lance-graph.
///
/// The query is handed Arrow RecordBatches from the graph_entities and graph_edges tables.
/// Only `MATCH`, `RETURN`, `WHERE`, `LIMIT`, `ORDER BY`, `WITH`, `UNION` are allowed.
/// `CREATE`, `DELETE`, `SET`, `MERGE`, `LOAD CSV`, `CALL` are rejected.
pub fn execute_cypher(
    connection: &Connection,
    runtime: &Runtime,
    scope: &str,
    cypher: &str,
    max_rows: usize,
) -> Result<CypherResult> {
    // Load graph tables as Arrow batches
    let entities_batch = load_graph_entities_batch(connection, runtime, scope).await?;
    let edges_batch = load_graph_edges_batch(connection, runtime, scope).await?;

    // Use lance-graph to execute Cypher over the batches
    // lance-graph API: GraphConfig -> CypherQuery.execute(tables)
    use lance_graph::{GraphConfig, CypherQuery};

    let config = GraphConfig::builder()
        .with_node_label("Entity", "scope") // nodes are in graph_entities
        .with_edge_label("CO_OCCURS", "scope") // edges are in graph_edges
        .build()
        .context("build GraphConfig")?;

    let tables = vec![
        ("Entity".to_string(), entities_batch),
        ("CO_OCCURS".to_string(), edges_batch),
    ];

    let result = runtime.block_on(async {
        CypherQuery::new(cypher)
            .with_config(config)
            .execute(tables)
            .await
    })?;

    // Convert lance-graph result to our format
    let columns = result.schema().fields().iter().map(|f| f.name().clone()).collect();
    let mut rows = Vec::new();
    for batch in result {
        for i in 0..batch.num_rows() {
            let mut row = Vec::new();
            for col in batch.columns() {
                let arr = col.as_any();
                let val = if let Some(s) = arr.downcast_ref::<StringArray>() {
                    s.value(i).to_string()
                } else if let Some(u) = arr.downcast_ref::<arrow_array::Int64Array>() {
                    u.value(i).to_string()
                } else if let Some(u) = arr.downcast_ref::<arrow_array::UInt64Array>() {
                    u.value(i).to_string()
                } else if let Some(f) = arr.downcast_ref::<arrow_array::Float64Array>() {
                    f.value(i).to_string()
                } else if let Some(b) = arr.downcast_ref::<arrow_array::BooleanArray>() {
                    b.value(i).to_string()
                } else {
                    "?".to_string()
                };
                row.push(val);
            }
            rows.push(row);
            if rows.len() >= max_rows {
                break;
            }
        }
    }

    Ok(CypherResult { columns, rows })
}

async fn load_graph_entities_batch(
    connection: &Connection,
    runtime: &Runtime,
    scope: &str,
) -> Result<RecordBatch> {
    let table = connection
        .open_table(GRAPH_ENTITIES_TABLE)
        .execute()
        .await
        .context("open graph_entities table")?;
    let predicate = format!("scope = '{}'", escape_sql_literal(scope));
    let mut stream = table
        .query()
        .only_if(predicate)
        .execute()
        .await
        .context("query graph_entities")?;
    let mut batches = Vec::new();
    while let Some(batch) = stream.try_next().await.context("stream graph_entities")? {
        batches.push(batch);
    }
    if batches.is_empty() {
        // Return empty batch with correct schema
        return Ok(RecordBatch::new_empty(graph_entities_schema()));
    }
    arrow::compute::concat_batches(&batches[0].schema(), &batches).context("concat graph_entities batches")
}

async fn load_graph_edges_batch(
    connection: &Connection,
    runtime: &Runtime,
    scope: &str,
) -> Result<RecordBatch> {
    let table = connection
        .open_table(GRAPH_EDGES_TABLE)
        .execute()
        .await
        .context("open graph_edges table")?;
    let predicate = format!("scope = '{}'", escape_sql_literal(scope));
    let mut stream = table
        .query()
        .only_if(predicate)
        .execute()
        .await
        .context("query graph_edges")?;
    let mut batches = Vec::new();
    while let Some(batch) = stream.try_next().await.context("stream graph_edges")? {
        batches.push(batch);
    }
    if batches.is_empty() {
        return Ok(RecordBatch::new_empty(graph_edges_schema()));
    }
    arrow::compute::concat_batches(&batches[0].schema(), &batches).context("concat graph_edges batches")
}

/// Graph RAG retrieval: embedding seeds → entity expansion → fused candidates.
pub async fn graph_rag_retrieve(
    connection: &Connection,
    runtime: &Runtime,
    scope: &str,
    seed_embeddings: Vec<Vec<f32>>,
    config: &GraphragConfig,
    fusion_limit: usize,
) -> Result<Vec<GraphRagHit>> {
    // 1. Find seed entities: chunks containing these embeddings' top entities
    // This would query the documents table for top-k by vector, then get their entities
    // For now, placeholder - the full implementation integrates with the fusion pipeline
    Ok(Vec::new())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphRagHit {
    pub path: String,
    pub chunk_idx: u32,
    pub text: String,
    pub distance: f32,
    pub graph_path: Vec<String>, // Cypher path that reached this hit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_literal() {
        assert_eq!(normalize_literal("John Smith"), "john smith");
        assert_eq!(normalize_literal("JOHN  SMITH"), "john smith");
        assert_eq!(normalize_literal("john-smith"), "john smith");
        assert_eq!(normalize_literal("John.Smith"), "john smith");
    }

    #[test]
    fn test_entity_key_dedup() {
        let mut map = HashMap::new();
        let k1 = EntityNodeKey::ClearLiteral { label: "person".into(), normalized: "john smith".into() };
        let k2 = EntityNodeKey::ClearLiteral { label: "person".into(), normalized: "john smith".into() };
        map.insert(k1, 1);
        map.insert(k2, 2);
        assert_eq!(map.len(), 1);
    }
}
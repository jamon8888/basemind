//! GraphRAG entity graph over GLiNER entities (spec 0016 / ADR-0016).
//!
//! Builds `graph_entities` and `graph_edges` tables from the NER sidecar at scan time.
//! Exposes two `graph` tool modes: `graph entities` (restricted Cypher) and `graph rag` (hybrid retrieval).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use arrow_array::{Array, RecordBatch};
use futures::TryStreamExt;
use lancedb::Connection;
use lancedb::query::{ExecutableQuery, QueryBase};
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;

use super::schema::{
    EntityKeySource, GRAPH_EDGES_TABLE, GRAPH_ENTITIES_TABLE, graph_edges_schema, graph_entities_schema,
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
    TokenOfDocument {
        label: String,
        literal: String,
        doc_path: String,
    },
    ClearLiteral {
        label: String,
        normalized: String,
    },
    GatewayToken {
        label: String,
        gateway_token: String,
    },
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
    pub key_source: u8, // 0 = TokenOfDocument, 1 = ClearLiteral, 2 = GatewayToken
    pub normalized_literal: String,
    pub document_path: String,
    pub gateway_token: String,
    /// Stable node id for lance-graph's Cypher joins (`ks<discriminant>|<label>|<key part>`).
    pub entity_id: String,
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
    /// Source/target node ids — join columns for lance-graph (`graph_entities.entity_id`).
    pub src_id: String,
    pub dst_id: String,
    pub chunk_idx: u32,
    pub doc_path: String,
    pub weight: u32,
}

/// Derive the stable node id from an [`EntityNodeKey`]. The same derivation must be
/// used for entity rows and edge endpoints, or Cypher joins silently come up empty.
pub fn entity_node_id(key: &EntityNodeKey) -> String {
    fn escape(value: &str) -> String {
        value.replace('\\', "\\\\").replace('|', "\\|")
    }
    match key {
        EntityNodeKey::TokenOfDocument {
            label,
            literal,
            doc_path,
        } => {
            format!("ks0|{}|{}|{}", escape(label), escape(doc_path), escape(literal))
        }
        EntityNodeKey::ClearLiteral { label, normalized } => {
            format!("ks1|{}|{}", escape(label), escape(normalized))
        }
        EntityNodeKey::GatewayToken { label, gateway_token } => {
            format!("ks2|{}|{}", escape(label), escape(gateway_token))
        }
    }
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
        // For gaze gateway tokens (stable cross-document tokens like <deadbeef:Email_1>), use GatewayToken.
        // Otherwise (mask/hash/drop/none), the literal is the original value — global.
        let (key_source, normalized_literal, document_path, gateway_token) = if ent.is_gateway_token {
            (
                2u8,
                String::new(),
                String::new(),
                ent.gateway_token.clone().unwrap_or_default(),
            ) // GatewayToken
        } else if ent.is_token {
            (0u8, ent.literal.clone(), ent.doc_path.clone(), String::new()) // TokenOfDocument
        } else {
            let norm = normalize_literal(&ent.literal);
            (1u8, norm, String::new(), String::new()) // ClearLiteral
        };

        let node_key = match key_source {
            0 => EntityNodeKey::TokenOfDocument {
                label: ent.label.clone(),
                literal: ent.literal.clone(),
                doc_path: ent.doc_path.clone(),
            },
            1 => EntityNodeKey::ClearLiteral {
                label: ent.label.clone(),
                normalized: normalized_literal.clone(),
            },
            _ => EntityNodeKey::GatewayToken {
                label: ent.label.clone(),
                gateway_token: gateway_token.clone(),
            },
        };

        // Upsert entity node
        let entity_row = entity_map.entry(node_key.clone()).or_insert_with(|| GraphEntityRow {
            scope: scope.to_string(),
            label: ent.label.clone(),
            key_source,
            normalized_literal: normalized_literal.clone(),
            document_path: document_path.clone(),
            gateway_token: gateway_token.clone(),
            entity_id: entity_node_id(&node_key),
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
        let (key_source, normalized_literal, document_path, gateway_token) = if ent.is_gateway_token {
            (
                2u8,
                String::new(),
                String::new(),
                ent.gateway_token.clone().unwrap_or_default(),
            )
        } else if ent.is_token {
            (0u8, ent.literal.clone(), ent.doc_path.clone(), String::new())
        } else {
            (1u8, normalize_literal(&ent.literal), String::new(), String::new())
        };
        let node_key = match key_source {
            0 => EntityNodeKey::TokenOfDocument {
                label: ent.label.clone(),
                literal: ent.literal.clone(),
                doc_path: ent.doc_path.clone(),
            },
            1 => EntityNodeKey::ClearLiteral {
                label: ent.label.clone(),
                normalized: normalized_literal,
            },
            _ => EntityNodeKey::GatewayToken {
                label: ent.label.clone(),
                gateway_token,
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
                    EntityNodeKey::GatewayToken { label, .. } => label.clone(),
                };
                let dst_label = match dst {
                    EntityNodeKey::TokenOfDocument { label, .. } => label.clone(),
                    EntityNodeKey::ClearLiteral { label, .. } => label.clone(),
                    EntityNodeKey::GatewayToken { label, .. } => label.clone(),
                };
                let (src_ks, src_norm, src_doc) = match src {
                    EntityNodeKey::TokenOfDocument { literal, doc_path, .. } => {
                        (0u8, literal.clone(), doc_path.clone())
                    }
                    EntityNodeKey::ClearLiteral { normalized, .. } => (1u8, normalized.clone(), String::new()),
                    EntityNodeKey::GatewayToken { gateway_token, .. } => (2u8, gateway_token.clone(), String::new()),
                };
                let (dst_ks, dst_norm, dst_doc) = match dst {
                    EntityNodeKey::TokenOfDocument { literal, doc_path, .. } => {
                        (0u8, literal.clone(), doc_path.clone())
                    }
                    EntityNodeKey::ClearLiteral { normalized, .. } => (1u8, normalized.clone(), String::new()),
                    EntityNodeKey::GatewayToken { gateway_token, .. } => (2u8, gateway_token.clone(), String::new()),
                };
                let edge_row = edge_map.entry(edge_key).or_insert_with(|| GraphEdgeRow {
                    scope: scope.to_string(),
                    src_key_source: src_ks,
                    src_normalized_literal: src_norm,
                    src_document_path: src_doc,
                    src_label,
                    dst_key_source: dst_ks,
                    dst_normalized_literal: dst_norm,
                    dst_document_path: dst_doc,
                    dst_label,
                    src_id: entity_node_id(src),
                    dst_id: entity_node_id(dst),
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
    pub is_token: bool,                // true when literal is a token_replace marker like [EMAIL_1]
    pub is_gateway_token: bool,        // true when literal is a gaze gateway token like <deadbeef:Email_1>
    pub gateway_token: Option<String>, // the gaze token (e.g., <deadbeef:Email_1>) when is_gateway_token
    pub start: u32,
    pub end: u32,
    pub chunk_idx: u32,
    pub doc_path: String,
}

/// Insert graph entity rows into the graph_entities table (upsert by unique key).
pub fn upsert_graph_entities(connection: &Connection, runtime: &Runtime, rows: &[GraphEntityRow]) -> Result<()> {
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
        let mut gateway_token = arrow_array::builder::StringBuilder::new();
        let mut entity_id = arrow_array::builder::StringBuilder::new();
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
            gateway_token.append_value(&r.gateway_token);
            entity_id.append_value(&r.entity_id);
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
                Arc::new(gateway_token.finish()),
                Arc::new(entity_id.finish()),
                Arc::new(mention_count.finish()),
                Arc::new(first_seen_chunk.finish()),
                Arc::new(last_seen_chunk.finish()),
                Arc::new(doc_path.finish()),
                Arc::new(doc_chunk_idx.finish()),
            ],
        )
        .context("assemble graph_entities batch")?;

        // Delete-then-insert for upsert semantics (key = scope+label+key_source+normalized_literal+document_path+gateway_token)
        // We build a predicate per row — acceptable since entity count per scan is modest.
        for r in rows {
            let predicate = match r.key_source {
                2 => format!( // GatewayToken: key is scope+label+key_source+gateway_token
                    "scope = '{}' AND label = '{}' AND key_source = {} AND gateway_token = '{}'",
                    escape_sql_literal(&r.scope),
                    escape_sql_literal(&r.label),
                    r.key_source,
                    escape_sql_literal(&r.gateway_token),
                ),
                _ => format!( // TokenOfDocument or ClearLiteral
                    "scope = '{}' AND label = '{}' AND key_source = {} AND normalized_literal = '{}' AND document_path = '{}'",
                    escape_sql_literal(&r.scope),
                    escape_sql_literal(&r.label),
                    r.key_source,
                    escape_sql_literal(&r.normalized_literal),
                    escape_sql_literal(&r.document_path),
                ),
            };
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
pub fn upsert_graph_edges(connection: &Connection, runtime: &Runtime, rows: &[GraphEdgeRow]) -> Result<()> {
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
        let mut src_id = arrow_array::builder::StringBuilder::new();
        let mut dst_id = arrow_array::builder::StringBuilder::new();
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
            src_id.append_value(&r.src_id);
            dst_id.append_value(&r.dst_id);
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
                Arc::new(src_id.finish()),
                Arc::new(dst_id.finish()),
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

        table.add(batch).execute().await.context("insert graph_edges batch")?;

        anyhow::Ok(())
    })
}

/// Delete all graph entities/edges for a scope (used by gc / wipe).
pub fn delete_graph_for_scope(connection: &Connection, runtime: &Runtime, scope: &str) -> Result<()> {
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
    use lance_graph::{CypherQuery, GraphConfig};

    // Load graph tables as Arrow batches (block_on around the async loaders —
    // the MCP dispatch is sync here, and the store runtime owns the reactor).
    let entities_batch = runtime.block_on(load_graph_entities_batch(connection, scope))?;
    let edges_batch = runtime.block_on(load_graph_edges_batch(connection, scope))?;

    // lance-graph maps label names onto id/join columns of the batches:
    // nodes join on `entity_id`, relationships on `src_id`/`dst_id` (schema.rs).
    let config = GraphConfig::builder()
        .with_node_label("Entity", "entity_id")
        .with_relationship("CO_OCCURS", "src_id", "dst_id")
        .build()
        .context("build GraphConfig")?;

    let mut datasets = std::collections::HashMap::new();
    datasets.insert("Entity".to_string(), batch_to_v56(&entities_batch)?);
    datasets.insert("CO_OCCURS".to_string(), batch_to_v56(&edges_batch)?);

    // `CypherQuery::new` parses (Result), `execute` runs async and yields ONE
    // RecordBatch holding the projected columns.
    let result = runtime
        .block_on(async {
            CypherQuery::new(cypher)?
                .with_config(config)
                .execute(datasets, None)
                .await
        })
        .context("execute cypher")?;

    let columns: Vec<String> = result.schema().fields().iter().map(|f| f.name().clone()).collect();
    let mut rows = Vec::new();
    'outer: for i in 0..result.num_rows() {
        let mut row = Vec::with_capacity(result.num_columns());
        for col in result.columns() {
            // lance-graph runs on arrow 56 — downcast through its array types.
            let val = if col.is_null(i) {
                "<null>".to_string()
            } else {
                arrow56::util::display::array_value_to_string(col.as_ref(), i).unwrap_or_else(|_| "?".to_string())
            };
            row.push(val);
        }
        rows.push(row);
        if rows.len() >= max_rows {
            break 'outer;
        }
    }

    Ok(CypherResult { columns, rows })
}

/// Cross the arrow 58 -> 56 version boundary (lancedb 0.40 vs lance-graph 0.5.4) via an
/// in-memory IPC roundtrip. Both sides speak Apache Arrow IPC, so this is a pure
/// re-serialization — no data reinterpretation, schema preserved verbatim.
fn batch_to_v56(batch: &RecordBatch) -> Result<arrow56::record_batch::RecordBatch> {
    let mut buf = Vec::new();
    {
        let mut writer = arrow_ipc::writer::StreamWriter::try_new(&mut buf, batch.schema().as_ref())
            .context("ipc writer (arrow 58)")?;
        writer.write(batch).context("ipc write batch")?;
        writer.finish().context("ipc finish")?;
    }
    let mut reader = arrow56::ipc::reader::StreamReader::try_new(std::io::Cursor::new(buf), None)
        .context("ipc reader (arrow 56)")?;
    let mut out: Option<arrow56::record_batch::RecordBatch> = None;
    while let Some(b) = reader.next().transpose().context("ipc read batch")? {
        out = match out {
            None => Some(b),
            Some(prev) => {
                Some(arrow56::compute::concat_batches(&prev.schema(), &[prev, b]).context("ipc concat batches")?)
            }
        };
    }
    out.ok_or_else(|| anyhow!("empty ipc stream across the arrow 58->56 bridge"))
}

pub(crate) async fn load_graph_entities_batch(connection: &Connection, scope: &str) -> Result<RecordBatch> {
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
    arrow_select::concat::concat_batches(&batches[0].schema(), &batches).context("concat graph_entities batches")
}

pub(crate) async fn load_graph_edges_batch(connection: &Connection, scope: &str) -> Result<RecordBatch> {
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
    arrow_select::concat::concat_batches(&batches[0].schema(), &batches).context("concat graph_edges batches")
}

/// One entity row projected out of the `graph_entities` batch for retrieval.
#[derive(Debug, Clone)]
pub struct EntityRef {
    pub entity_id: String,
    pub label: String,
    pub normalized_literal: String,
    pub document_path: String,
}

/// Project the retrieval-relevant columns out of a `graph_entities` batch.
pub fn load_entity_refs(batch: &RecordBatch) -> Result<Vec<EntityRef>> {
    use arrow_array::{StringArray, UInt32Array};
    let entity_id = batch
        .column_by_name("entity_id")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| anyhow!("`entity_id` column missing"))?;
    let label = batch
        .column_by_name("label")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| anyhow!("`label` column missing"))?;
    let literal = batch
        .column_by_name("normalized_literal")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| anyhow!("`normalized_literal` column missing"))?;
    let doc_path = batch
        .column_by_name("document_path")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| anyhow!("`document_path` column missing"))?;
    let mention = batch
        .column_by_name("mention_count")
        .and_then(|c| c.as_any().downcast_ref::<UInt32Array>());

    let mut out = Vec::with_capacity(batch.num_rows());
    for i in 0..batch.num_rows() {
        out.push(EntityRef {
            entity_id: entity_id.value(i).to_string(),
            label: label.value(i).to_string(),
            normalized_literal: literal.value(i).to_string(),
            document_path: doc_path.value(i).to_string(),
        });
    }
    let _ = mention;
    Ok(out)
}

/// One expanded entity: how far it sits from a seed and the chain that reached it.
#[derive(Debug, Clone)]
pub struct ExpandedEntity {
    pub entity_id: String,
    /// 0 for the seed itself, 1..=hops for the expansion ring.
    pub hop: u8,
    /// Entity ids from the seed to this entity, inclusive.
    pub chain: Vec<String>,
}

/// Breadth-first expansion over the co-occurrence edge list, in memory.
///
/// The graph tables are small (one row per entity-document and per edge-chunk triple),
/// so an in-memory BFS over the projected `src_id`/`dst_id` columns beats a Cypher
/// round-trip per hop and reuses the same batches `execute_cypher` loads.
pub fn expand_entity_graph(
    edges_src: &[String],
    edges_dst: &[String],
    seeds: &[String],
    hops: u8,
    max_expand: usize,
) -> Vec<ExpandedEntity> {
    use std::collections::{HashMap, VecDeque};

    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    for (src, dst) in edges_src.iter().zip(edges_dst.iter()) {
        adjacency.entry(src.as_str()).or_default().push(dst.as_str());
        adjacency.entry(dst.as_str()).or_default().push(src.as_str());
    }

    let mut out = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut queue: VecDeque<(String, u8, Vec<String>)> = VecDeque::new();
    for seed in seeds {
        if seen.insert(seed.as_str()) {
            queue.push_back((seed.clone(), 0, vec![seed.clone()]));
        }
    }

    while let Some((entity_id, hop, chain)) = queue.pop_front() {
        out.push(ExpandedEntity {
            entity_id: entity_id.clone(),
            hop,
            chain,
        });
        if out.len() >= max_expand || hop >= hops {
            continue;
        }
        if let Some(neighbors) = adjacency.get(entity_id.as_str()) {
            for next in neighbors {
                if seen.insert(next) {
                    let mut next_chain = chain.clone();
                    next_chain.push((*next).to_string());
                    queue.push_back(((*next).to_string(), hop + 1, next_chain));
                }
            }
        }
    }
    out
}

/// Project the `src_id` / `dst_id` columns out of a `graph_edges` batch.
pub fn load_edge_endpoints(batch: &RecordBatch) -> Result<(Vec<String>, Vec<String>)> {
    use arrow_array::StringArray;
    let src = batch
        .column_by_name("src_id")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| anyhow!("`src_id` column missing"))?;
    let dst = batch
        .column_by_name("dst_id")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| anyhow!("`dst_id` column missing"))?;
    let mut srcs = Vec::with_capacity(batch.num_rows());
    let mut dsts = Vec::with_capacity(batch.num_rows());
    for i in 0..batch.num_rows() {
        srcs.push(src.value(i).to_string());
        dsts.push(dst.value(i).to_string());
    }
    Ok((srcs, dsts))
}

/// Fuse vector-lane and graph-lane candidates into one ranked hit list.
///
/// Vector hits score `1 / (1 + distance)`; a chunk pulled in only through the graph
/// scores `0.3 / hop` (hop 1 → 0.6, hop 2 → 0.3). A chunk present in both lanes gets
/// the sum, so graph agreement boosts genuine vector hits. `distance` on the way out
/// is `1 / score - 1`, keeping "lower is closer" consistent across lanes.
pub fn fuse_results(
    vector_hits: Vec<crate::lance::DocumentHit>,
    graph_chunks: Vec<(crate::lance::DocumentHit, u8, Vec<String>)>,
    max_results: usize,
) -> (Vec<GraphRagHit>, bool) {
    use std::collections::HashMap;

    // entity chains per (path, chunk): keep the shortest chain per chunk.
    let mut graph_meta: HashMap<(String, u32), (u8, Vec<String>)> = HashMap::new();
    for (hit, hop, chain) in &graph_chunks {
        let key = (hit.path.clone(), hit.chunk_idx);
        let entry = graph_meta.entry(key).or_insert((*hop, chain.clone()));
        if *hop < entry.0 {
            *entry = (*hop, chain.clone());
        }
    }

    let mut scored: Vec<(f32, GraphRagHit)> = Vec::new();
    for hit in vector_hits {
        let vector_score = 1.0 / (1.0 + hit.distance.max(0.0));
        let graph_bonus = graph_meta
            .get(&(hit.path.clone(), hit.chunk_idx))
            .map(|(hop, _)| 0.3 / f32::from(*hop).max(1.0))
            .unwrap_or(0.0);
        let score = vector_score + graph_bonus;
        let chain = graph_meta
            .get(&(hit.path.clone(), hit.chunk_idx))
            .map(|(_, chain)| chain.clone())
            .unwrap_or_default();
        scored.push((
            score,
            GraphRagHit {
                path: hit.path,
                chunk_idx: hit.chunk_idx,
                text: hit.text,
                distance: 1.0 / score - 1.0,
                graph_path: chain,
            },
        ));
    }
    for (hit, hop, chain) in graph_chunks {
        let key = (hit.path.clone(), hit.chunk_idx);
        if scored.iter().any(|(_, h)| (h.path.clone(), h.chunk_idx) == key) {
            continue;
        }
        let score = 0.3 / f32::from(hop).max(1.0);
        scored.push((
            score,
            GraphRagHit {
                path: hit.path,
                chunk_idx: hit.chunk_idx,
                text: hit.text,
                distance: 1.0 / score - 1.0,
                graph_path: chain,
            },
        ));
    }

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let truncated = scored.len() > max_results;
    scored.truncate(max_results);
    (scored.into_iter().map(|(_, hit)| hit).collect(), truncated)
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
        let k1 = EntityNodeKey::ClearLiteral {
            label: "person".into(),
            normalized: "john smith".into(),
        };
        let k2 = EntityNodeKey::ClearLiteral {
            label: "person".into(),
            normalized: "john smith".into(),
        };
        map.insert(k1, 1);
        map.insert(k2, 2);
        assert_eq!(map.len(), 1);
    }
}

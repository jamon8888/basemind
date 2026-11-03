//! Helper bodies for the 6 memory + document-search MCP tools.

use std::sync::Arc;

use rmcp::ErrorData as McpError;
#[cfg(any(feature = "memory", feature = "documents"))]
use rmcp::model::CallToolResult;

use super::ServerState;
#[cfg(any(feature = "memory", feature = "documents"))]
use super::helpers::elapsed_us;
#[cfg(feature = "documents")]
use super::helpers::format_response;
#[cfg(feature = "memory")]
use super::helpers::json_result;
#[cfg(feature = "documents")]
use super::types::{DocumentSearchHit, SearchDocumentsParams, SearchDocumentsResponse};
#[cfg(feature = "memory")]
use super::types_memory::{
    MemoryDeleteParams, MemoryDeleteResponse, MemoryEntry, MemoryGetParams, MemoryListParams, MemoryListResponse,
    MemoryPutParams, MemoryPutResponse, MemorySearchHit, MemorySearchParams, MemorySearchResponse, Visibility,
};
#[cfg(feature = "documents")]
use crate::extract::doc::{DocEntity, DocKeyword, DocSummary};

#[cfg(feature = "intelligence")]
pub(super) async fn embed_query(state: &ServerState, text: &str) -> Result<Vec<f32>, McpError> {
    embed_query_shared(&state.shared, text).await
}

/// Embed one query text through the workspace's shared embedder (lazy-loaded once).
#[cfg(feature = "intelligence")]
pub(super) async fn embed_query_shared(
    shared: &crate::mcp::shared_state::SharedReadStack,
    text: &str,
) -> Result<Vec<f32>, McpError> {
    let preset = shared.config.documents.embedding_preset.clone();
    let max_embed_threads = shared
        .config
        .resources
        .effective_embed_threads(shared.config.documents.embed_max_threads);
    let embed_batch_size = shared.config.resources.embed_batch_size;
    let provider = shared.config.resources.onnx_provider;
    let embedder = shared
        .embedder
        .get_or_try_init(|| async {
            crate::embeddings::SharedEmbedder::load_with_provider(
                &preset,
                max_embed_threads,
                embed_batch_size,
                provider,
            )
            .map(Arc::new)
            .map_err(|e| format!("load embedder: {e}"))
        })
        .await
        .map_err(|e| McpError::internal_error(e.clone(), None))?;
    let embedder = Arc::clone(embedder);
    let text = text.to_string();
    tokio::task::spawn_blocking(move || embedder.embed(&text))
        .await
        .map_err(|e| McpError::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| McpError::internal_error(format!("embed: {e}"), None))
}

#[cfg(any(feature = "memory", feature = "documents", feature = "code-search"))]
pub(super) async fn lance_store(state: &ServerState) -> Result<Arc<crate::lance::LanceStore>, McpError> {
    let preset = state.shared.config.documents.embedding_preset.clone();
    let max_embed_threads = state
        .shared
        .config
        .resources
        .effective_embed_threads(state.shared.config.documents.embed_max_threads);
    let embed_batch_size = state.shared.config.resources.embed_batch_size;
    let provider = state.shared.config.resources.onnx_provider;
    state
        .shared
        .lance
        .get_or_try_init(|| async {
            let embedder = state
                .shared
                .embedder
                .get_or_try_init(|| async {
                    crate::embeddings::SharedEmbedder::load_with_provider(
                        &preset,
                        max_embed_threads,
                        embed_batch_size,
                        provider,
                    )
                    .map(Arc::new)
                    .map_err(|e| format!("load embedder: {e}"))
                })
                .await
                .map_err(|e| format!("embedder init: {e}"))?;
            let dim = embedder.dim();
            let model = embedder.model().to_string();
            let lance_dir = state
                .shared
                .store
                .read()
                .await
                .basemind_dir
                .join(crate::store::LANCE_DIR);
            let model_for_open = model.clone();
            tokio::task::spawn_blocking(move || crate::lance::LanceStore::open(&lance_dir, dim, &model_for_open))
                .await
                .map_err(|e| format!("lance open join: {e}"))?
                .map(Arc::new)
                .map_err(|e| format!("open LanceStore: {e}"))
        })
        .await
        .cloned()
        .map_err(|e| McpError::internal_error(e.clone(), None))
}

/// The Lance store opened **without** constructing an embedder.
///
/// [`lance_store`] eagerly loads the ONNX embedder (`SharedEmbedder::load`) and derives the store's
/// dimension from it, so every caller pays for a model download even when it only wants to read
/// rows. For the lexical lane that is the whole cost of the query and none of the benefit: the
/// answer does not depend on a vector.
///
/// The dimension is resolved from the configured preset by name instead
/// ([`crate::scanner_docs::preset_dim`]), which is the same lookup the scanner uses. It refuses to
/// guess for an unknown preset, so an unresolvable preset errors here exactly as it would during a
/// scan — rather than opening a store at a wrong dimension and triggering a wipe.
///
/// Shares the same `state.shared.lance` cache as [`lance_store`], so the two paths cannot open the
/// directory at different dimensions: whichever runs first wins, and a mismatch is caught by
/// `LanceStore::open`'s `wipe_on_mismatch` rather than producing two stores.
#[cfg(feature = "documents")]
pub(super) async fn lance_store_lexical(state: &ServerState) -> Result<Arc<crate::lance::LanceStore>, McpError> {
    let preset = state.shared.config.documents.embedding_preset.clone();
    let dim = crate::scanner_docs::preset_dim(&preset)
        .map_err(|e| McpError::internal_error(format!("resolve embedding preset '{preset}': {e}"), None))?;
    state
        .shared
        .lance
        .get_or_try_init(|| async {
            let lance_dir = state
                .shared
                .store
                .read()
                .await
                .basemind_dir
                .join(crate::store::LANCE_DIR);
            let model_for_open = preset.clone();
            tokio::task::spawn_blocking(move || crate::lance::LanceStore::open(&lance_dir, dim, &model_for_open))
                .await
                .map_err(|e| format!("lance open join: {e}"))?
                .map(Arc::new)
                .map_err(|e| format!("open LanceStore: {e}"))
        })
        .await
        .cloned()
        .map_err(|e| McpError::internal_error(e.clone(), None))
}

/// Run the lexical lane on the `spawn_blocking` thread.
///
/// Named rather than inlined because that closure must be `'static`; building the term list and
/// the scope predicate inside it keeps the borrowed config out of the captured environment.
fn basemind_lexical(
    lance: &crate::lance::LanceStore,
    terms: &[String],
    predicate: &str,
    limit: usize,
    max_relaxations: u32,
) -> anyhow::Result<Vec<crate::lance::fts::LexicalHit>> {
    crate::lance::fts::search_relaxed_on(lance, terms, predicate, limit, max_relaxations)
}

#[cfg(feature = "documents")]
fn basemind_phrase(
    lance: &crate::lance::LanceStore,
    phrase: &str,
    predicate: &str,
    limit: usize,
) -> anyhow::Result<Vec<crate::lance::fts::LexicalHit>> {
    crate::lance::fts::search_phrase_on(lance, phrase, predicate, limit)
}

#[cfg(feature = "documents")]
fn basemind_exact(
    lance: &crate::lance::LanceStore,
    term: &str,
    predicate: &str,
    limit: usize,
) -> anyhow::Result<Vec<crate::lance::fts::LexicalHit>> {
    crate::lance::fts::search_exact_on(lance, term, predicate, limit)
}

#[cfg(feature = "documents")]
fn facets_to_wire(facets: crate::lance::DocumentFacets) -> (Vec<DocKeyword>, Vec<DocEntity>, Option<DocSummary>) {
    let keywords = facets
        .keywords
        .unwrap_or_default()
        .into_iter()
        .map(|text| DocKeyword {
            text,
            score: 0.0,
            algorithm: "stored".to_string(),
        })
        .collect();
    let entities = facets
        .entities
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entity| {
            let (category, text) = entity.split_once(':')?;
            Some(DocEntity {
                category: category.to_string(),
                text: text.to_string(),
                start: 0,
                end: text.len() as u32,
                confidence: None,
            })
        })
        .collect();
    let summary = facets.summary.map(|text| DocSummary {
        text,
        // Lance stores the normalized summary text; the current document
        // pipeline only persists extractive summaries, so preserve the wire
        // contract expected by existing clients.
        strategy: "extractive".to_string(),
        token_count: None,
    });
    (keywords, entities, summary)
}

#[cfg(feature = "documents")]
fn lexical_hit_to_document_hit(hit: crate::lance::fts::LexicalHit, distance: f32) -> DocumentSearchHit {
    let (keywords, entities, summary) = facets_to_wire(hit.facets);
    DocumentSearchHit {
        path: hit.path,
        chunk_idx: hit.chunk_idx,
        text: hit.text,
        mime_type: hit.mime_type,
        byte_start: hit.byte_start,
        byte_end: hit.byte_end,
        distance,
        matched_lanes: vec!["lexical".to_string()],
        retrieval_mode: "lexical".to_string(),
        citation: hit.cites.lines().next().map(str::to_string),
        rerank_score: None,
        keywords,
        entities,
        summary,
    }
}

/// Push `memory_put` concurrency docs anchor.
/// `memory_put` is a read-modify-write across two stores (Fjall + LanceDB).
/// Without serialization, two concurrent puts for the same key both read "no
/// existing record" and stamp different `created_at` values, and their two-phase
/// (Fjall, then async Lance) writes can interleave so the stores disagree.
///
/// We serialize per key — unrelated keys still write in parallel — by handing
/// out a per-key `tokio::sync::Mutex` from a process-global registry. The
/// registry itself is guarded by a short-lived `std::sync::Mutex` (held only to
/// clone an `Arc`, never across an `.await`).
///
/// The registry is an `LruCache` bounded at [`MEMORY_PUT_LOCK_CAP`] so it cannot
/// grow without limit as distinct `(scope, key)` pairs are written over the
/// process lifetime. The cap is generous enough that realistic key counts never
/// evict. Eviction is safe for correctness: any task already holding the `Arc`
/// keeps its mutex alive after the entry is dropped from the cache. The single
/// rare-eviction caveat is that if a key's lock is evicted while one put holds
/// it and a *second* put for the same key arrives, the second put mints a fresh
/// mutex and the two no longer serialize — a window only reachable when more
/// than [`MEMORY_PUT_LOCK_CAP`] distinct keys are written between two racing
/// puts on the same key.
#[cfg(feature = "memory")]
const MEMORY_PUT_LOCK_CAP: std::num::NonZeroUsize = std::num::NonZeroUsize::new(4096).unwrap();

#[cfg(feature = "memory")]
type MemoryPutLockKey = (String, u8, String, String);

#[cfg(feature = "memory")]
type MemoryPutLockRegistry = std::sync::Mutex<lru::LruCache<MemoryPutLockKey, Arc<tokio::sync::Mutex<()>>>>;

#[cfg(feature = "memory")]
fn memory_put_lock(scope: &str, vis_byte: u8, owner: &str, key: &str) -> Arc<tokio::sync::Mutex<()>> {
    use std::sync::OnceLock;
    static LOCKS: OnceLock<MemoryPutLockRegistry> = OnceLock::new();
    let registry = LOCKS.get_or_init(|| std::sync::Mutex::new(lru::LruCache::new(MEMORY_PUT_LOCK_CAP)));
    let mut guard = registry.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let registry_key = (scope.to_string(), vis_byte, owner.to_string(), key.to_string());
    Arc::clone(guard.get_or_insert(registry_key, || Arc::new(tokio::sync::Mutex::new(()))))
}

/// String form of a [`Visibility`] for the LanceDB `visibility` column. Matches the serde
/// `rename_all = "lowercase"` discriminants so the column reads back as the same enum.
#[cfg(feature = "memory")]
fn lance_visibility(visibility: Visibility) -> &'static str {
    match visibility {
        Visibility::Group => "group",
        Visibility::Individual => "individual",
    }
}

/// Resolve the `(vis_byte, owner)` namespace coordinates for a memory call.
///
/// `group` → `owner = ""` (shared tier, today's behavior). `individual` → `owner` is the
/// server's resolved `agent_id`, which was validated through `AgentId` at boot and is
/// therefore NUL-free — safe to embed in the length-prefixed Fjall key segment.
#[cfg(feature = "memory")]
fn namespace(state: &ServerState, visibility: Visibility) -> (u8, &str) {
    let owner: &str = match visibility {
        Visibility::Individual => &state.agent_id,
        Visibility::Group => "",
    };
    (visibility.vis_byte(), owner)
}

#[cfg(feature = "memory")]
pub(super) async fn run_memory_put(state: &ServerState, params: MemoryPutParams) -> Result<CallToolResult, McpError> {
    let (vis_byte, owner) = namespace(state, params.visibility);
    let tags = params.tags.clone().unwrap_or_default();

    let (created_at, updated_at) = memory_put_fjall(state, vis_byte, owner, &params.key, &params.value, &tags).await?;

    if params.embed {
        let embedding = embed_query(state, &params.value).await?;
        let lance = lance_store(state).await?;
        let row = crate::lance::MemoryRow {
            scope: state.shared.scope.clone(),
            key: params.key.clone(),
            value: params.value.clone(),
            tags,
            visibility: lance_visibility(params.visibility).to_string(),
            agent_id: owner.to_string(),
            embedding,
            created_at,
            updated_at,
        };
        let lance_clone = Arc::clone(&lance);
        tokio::task::spawn_blocking(move || lance_clone.upsert_memory(row))
            .await
            .map_err(|e| McpError::internal_error(format!("spawn_blocking: {e}"), None))?
            .map_err(|e| McpError::internal_error(format!("upsert_memory: {e}"), None))?;
    }
    json_result(&MemoryPutResponse {
        key: params.key,
        created_at,
        updated_at,
    })
}

/// Run the fjall half of `memory_put`, returning the resolved `(created_at, updated_at)`.
///
/// `daemon_writer` builds a [`MemoryOp::Put`] and forwards it (the daemon serializes same-workspace
/// writes, so the RMW is atomic without a per-key lock). Every other build takes the per-key
/// [`memory_put_lock`] and calls [`put_core`](super::memory_ops::put_core) against the local index.
#[cfg(feature = "memory")]
async fn memory_put_fjall(
    state: &ServerState,
    vis_byte: u8,
    owner: &str,
    key: &str,
    value: &str,
    tags: &[String],
) -> Result<(i64, i64), McpError> {
    #[cfg(all(feature = "comms", any(unix, windows)))]
    if state.shared.daemon_writer {
        use crate::comms::memory_proto::{MemoryOp, MemoryOutcome};

        let op = MemoryOp::Put {
            vis_byte,
            owner: owner.to_string(),
            key: key.to_string(),
            value: value.to_string(),
            tags: tags.to_vec(),
        };
        let outcome = super::helpers_comms::dispatch_memory_op(state, op).await?;
        return match outcome {
            MemoryOutcome::Put { created_at, updated_at } => Ok((created_at, updated_at)),
            other => Err(McpError::internal_error(
                format!("memory_put: unexpected daemon outcome {other:?}"),
                None,
            )),
        };
    }

    let key_lock = memory_put_lock(&state.shared.scope, vis_byte, owner, key);
    let _put_guard = key_lock.lock().await;
    let store = state.shared.store.read().await;
    let idx = store
        .index_db
        .as_ref()
        .ok_or_else(|| McpError::internal_error("memory_by_key index not available", None))?;
    Ok(super::memory_ops::put_core(
        idx,
        &state.shared.scope,
        vis_byte,
        owner,
        key,
        value,
        tags,
    )?)
}

#[cfg(feature = "memory")]
pub(super) async fn run_memory_get(state: &ServerState, params: MemoryGetParams) -> Result<CallToolResult, McpError> {
    let (vis_byte, owner) = namespace(state, params.visibility);

    #[cfg(all(feature = "comms", any(unix, windows)))]
    if state.shared.daemon_writer {
        use crate::comms::memory_proto::{MemoryOp, MemoryOutcome};

        let op = MemoryOp::Get {
            vis_byte,
            owner: owner.to_string(),
            key: params.key.clone(),
        };
        let outcome = super::helpers_comms::dispatch_memory_op(state, op).await?;
        let entry = match outcome {
            MemoryOutcome::Got(entry) => entry.map(wire_to_entry),
            other => {
                return Err(McpError::internal_error(
                    format!("memory_get: unexpected daemon outcome {other:?}"),
                    None,
                ));
            }
        };
        return json_result(&entry);
    }

    let store = state.shared.store.read().await;
    let idx = store
        .index_db
        .as_ref()
        .ok_or_else(|| McpError::internal_error("memory_by_key index not available", None))?;
    let entry: Option<MemoryEntry> =
        super::memory_ops::get_core(idx, &state.shared.scope, vis_byte, owner, &params.key)?.map(wire_to_entry);
    json_result(&entry)
}

/// Map a wire memory entry onto the MCP [`MemoryEntry`] response shape.
#[cfg(feature = "memory")]
fn wire_to_entry(w: super::memory_ops::WireMemoryEntry) -> MemoryEntry {
    MemoryEntry {
        key: w.key,
        value: w.value,
        tags: w.tags,
        created_at: w.created_at,
        updated_at: w.updated_at,
    }
}

#[cfg(feature = "memory")]
pub(super) async fn run_memory_list(state: &ServerState, params: MemoryListParams) -> Result<CallToolResult, McpError> {
    use super::cursor::Cursor;

    let __body = std::time::Instant::now();
    let limit = params
        .limit
        .unwrap_or(super::helpers::SEARCH_LIMIT_DEFAULT)
        .min(super::helpers::SEARCH_LIMIT_MAX) as usize;
    let (vis_byte, owner) = namespace(state, params.visibility);
    let key_prefix_filter = params.prefix.clone().unwrap_or_default();
    let cursor_bytes = params.cursor.as_ref().map(|c| c.decode_fjall()).transpose()?;

    #[cfg(all(feature = "comms", any(unix, windows)))]
    if state.shared.daemon_writer {
        use crate::comms::memory_proto::{MemoryOp, MemoryOutcome};

        let op = MemoryOp::List {
            vis_byte,
            owner: owner.to_string(),
            prefix: key_prefix_filter,
            tag: params.tag.clone(),
            limit: limit as u32,
            cursor: cursor_bytes,
        };
        let outcome = super::helpers_comms::dispatch_memory_op(state, op).await?;
        return match outcome {
            MemoryOutcome::Listed {
                entries,
                total,
                truncated,
                next_cursor,
            } => json_result(&MemoryListResponse {
                total: total as usize,
                truncated,
                entries: entries.into_iter().map(wire_to_entry).collect(),
                next_cursor: next_cursor.as_deref().map(Cursor::encode_fjall),
                elapsed_us: elapsed_us(__body),
            }),
            other => Err(McpError::internal_error(
                format!("memory_list: unexpected daemon outcome {other:?}"),
                None,
            )),
        };
    }

    let store = state.shared.store.read().await;
    let idx = store
        .index_db
        .as_ref()
        .ok_or_else(|| McpError::internal_error("memory_by_key index not available", None))?;
    let result = super::memory_ops::list_core(
        idx,
        &state.shared.scope,
        &super::memory_ops::ListQuery {
            vis_byte,
            owner,
            key_prefix: &key_prefix_filter,
            tag: params.tag.as_deref(),
            limit,
            cursor: cursor_bytes.as_deref(),
        },
    )?;
    json_result(&MemoryListResponse {
        total: result.total as usize,
        truncated: result.truncated,
        entries: result.entries.into_iter().map(wire_to_entry).collect(),
        next_cursor: result.next_cursor.as_deref().map(Cursor::encode_fjall),
        elapsed_us: elapsed_us(__body),
    })
}

#[cfg(feature = "memory")]
pub(super) async fn run_memory_search(
    state: &ServerState,
    params: MemorySearchParams,
) -> Result<CallToolResult, McpError> {
    let __body = std::time::Instant::now();
    let limit = params.limit.unwrap_or(10).min(100) as usize;
    if limit == 0 {
        return Err(McpError::invalid_params(
            "document search limit must be greater than zero",
            None,
        ));
    }
    let (_, owner) = namespace(state, params.visibility);
    let visibility = lance_visibility(params.visibility).to_string();
    let agent_id = owner.to_string();
    let embedding = embed_query(state, &params.query).await?;
    let lance = lance_store(state).await?;
    let scope = state.shared.scope.clone();
    let tag = params.tag.clone();
    let hits_raw = tokio::task::spawn_blocking(move || {
        lance.search_memory(&scope, &visibility, &agent_id, embedding, limit, tag.as_deref())
    })
    .await
    .map_err(|e| McpError::internal_error(format!("spawn_blocking: {e}"), None))?
    .map_err(|e| McpError::internal_error(format!("search_memory: {e}"), None))?;
    let hits = hits_raw
        .into_iter()
        .map(|h| MemorySearchHit {
            key: h.key,
            value: h.value,
            tags: h.tags,
            distance: h.distance,
        })
        .collect();
    json_result(&MemorySearchResponse {
        query: params.query,
        hits,
        elapsed_us: elapsed_us(__body),
    })
}

#[cfg(feature = "memory")]
pub(super) async fn run_memory_delete(
    state: &ServerState,
    params: MemoryDeleteParams,
) -> Result<CallToolResult, McpError> {
    let (vis_byte, owner) = namespace(state, params.visibility);
    let deleted_fjall = memory_delete_fjall(state, vis_byte, owner, &params.key).await?;
    if let Some(lance) = state.shared.lance.get() {
        let lance = Arc::clone(lance);
        let scope = state.shared.scope.clone();
        let key = params.key.clone();
        let visibility = lance_visibility(params.visibility).to_string();
        let agent_id = owner.to_string();
        match tokio::task::spawn_blocking(move || lance.delete_memory(&scope, &visibility, &agent_id, &key)).await {
            Ok(Ok(_rows_deleted)) => {}
            Ok(Err(error)) => {
                tracing::warn!(
                    key = %params.key,
                    ?error,
                    "lance delete_memory failed; embedding may be stale relative to Fjall"
                );
            }
            Err(join_error) => {
                tracing::warn!(
                    key = %params.key,
                    ?join_error,
                    "lance delete_memory task panicked; embedding may be stale relative to Fjall"
                );
            }
        }
    }
    json_result(&MemoryDeleteResponse { deleted: deleted_fjall })
}

/// Run the fjall half of `memory_delete`, returning whether a record existed.
///
/// `daemon_writer` forwards a [`MemoryOp::Delete`]; every other build calls
/// [`delete_core`](super::memory_ops::delete_core) against the local index.
#[cfg(feature = "memory")]
async fn memory_delete_fjall(state: &ServerState, vis_byte: u8, owner: &str, key: &str) -> Result<bool, McpError> {
    #[cfg(all(feature = "comms", any(unix, windows)))]
    if state.shared.daemon_writer {
        use crate::comms::memory_proto::{MemoryOp, MemoryOutcome};

        let op = MemoryOp::Delete {
            vis_byte,
            owner: owner.to_string(),
            key: key.to_string(),
        };
        let outcome = super::helpers_comms::dispatch_memory_op(state, op).await?;
        return match outcome {
            MemoryOutcome::Deleted { deleted } => Ok(deleted),
            other => Err(McpError::internal_error(
                format!("memory_delete: unexpected daemon outcome {other:?}"),
                None,
            )),
        };
    }

    let store = state.shared.store.read().await;
    let idx = store
        .index_db
        .as_ref()
        .ok_or_else(|| McpError::internal_error("memory_by_key index not available", None))?;
    Ok(super::memory_ops::delete_core(
        idx,
        &state.shared.scope,
        vis_byte,
        owner,
        key,
    )?)
}

/// Pick the ingestion scope to search: the caller's if they named one, else this repo's.
///
/// Documents are partitioned by ingestion scope, and the lanes do not agree on one: the scanner
/// writes under the repo scope while `web_scrape` / `web_crawl` write under `web:<host>`. Reading
/// only `state.shared.scope`, with no way to ask for another, made every scraped page permanently
/// invisible — the write landed, LanceDB grew on disk, and the query filtered it out and returned
/// an empty, error-free, 14 ms answer.
#[cfg(feature = "documents")]
fn resolve_doc_scope(requested: Option<&str>, repo_scope: &str) -> String {
    requested.unwrap_or(repo_scope).to_string()
}

#[cfg(feature = "documents")]
pub(super) async fn run_search_documents(
    state: &ServerState,
    params: SearchDocumentsParams,
) -> Result<CallToolResult, McpError> {
    let __body = std::time::Instant::now();
    // An explicit Mcp preset override always means a compiled-in preset;
    // otherwise the workspace `custom_model` (if any) wins over the default.
    let explicit_preset = params.overrides.reranker_preset.clone();
    let (output_format, reranker_enabled, _reranker_preset, reranker_top_k) = if params.overrides.any() {
        let mut effective = (*state.shared.config).clone();
        crate::config::layered::apply_documents_overrides(
            &mut effective,
            &params.overrides,
            crate::config::ConfigSource::Mcp,
            None,
        );
        let r = &effective.documents.reranker;
        (effective.documents.output.format, r.enabled, r.preset.clone(), r.top_k)
    } else {
        let r = &state.shared.config.documents.reranker;
        (
            state.shared.config.documents.output.format,
            r.enabled,
            r.preset.clone(),
            r.top_k,
        )
    };

    let limit = params.limit.unwrap_or(10).min(100) as usize;
    let scope = resolve_doc_scope(params.scope.as_deref(), &state.shared.scope);
    let facet_filter = crate::lance::DocumentFacetFilter {
        doc_type: crate::extract::doc::facets::doc_type(params.doc_type.as_deref()),
        section: crate::extract::doc::facets::section(params.section.as_deref()),
        jurisdiction: crate::extract::doc::facets::jurisdiction(params.jurisdiction.as_deref()),
        doc_date: crate::extract::doc::facets::doc_date(params.doc_date.as_deref()),
    };
    let facet_sql = {
        let mut clauses = Vec::new();
        for (column, value) in [
            ("doc_type", facet_filter.doc_type.as_deref()),
            ("section", facet_filter.section.as_deref()),
            ("jurisdiction", facet_filter.jurisdiction.as_deref()),
            ("doc_date", facet_filter.doc_date.as_deref()),
        ] {
            if let Some(value) = value {
                clauses.push(format!(
                    "({column} IS NULL OR {column} = '{}')",
                    crate::lance::escape_sql_literal(value)
                ));
            }
        }
        clauses.join(" AND ")
    };

    // With `embed = false` there are no vectors, so the vector lane cannot run at all — and
    // reaching for it would embed the query and download a model to answer a question that does
    // not need one. The lexical lane is not a fallback here; on a lexical-only store it is the only
    // lane, and it answers through a store opened without an embedder.
    let lexical_only = !state.shared.config.documents.embed;
    let has_metadata_filters = params.entity_category.is_some() || params.keywords_contains.is_some();
    let candidate_limit = if has_metadata_filters {
        limit.saturating_mul(8).clamp(limit, 1_000)
    } else {
        limit
    };
    let reranker_model = if reranker_enabled {
        match state
            .shared
            .config
            .documents
            .reranker
            .resolve_model(explicit_preset.as_deref())
        {
            crate::config::ResolvedRerankerModel::Preset { name } => Some(format!("preset:{name}")),
            crate::config::ResolvedRerankerModel::Custom { model_id, model_file } => {
                Some(format!("custom:{model_id}:{model_file}"))
            }
        }
    } else {
        None
    };
    let query_fingerprint = document_query_fingerprint(
        &params,
        &scope,
        lexical_only,
        state.shared.config.documents.fusion.max_relaxations,
        reranker_enabled,
        reranker_top_k,
        reranker_model.as_deref(),
    );

    let (mut hits, cursor_generation, cursor_offset, raw_page_len, has_more, cursor_invalidated) = if lexical_only {
        let max_relaxations = state.shared.config.documents.fusion.max_relaxations;
        let lance = lance_store_lexical(state).await?;
        let query = params.query.clone();
        let scope_for_lane = scope.clone();
        let facet_sql_for_lane = facet_sql.clone();
        let mime_for_lane = params.mime_type.clone();
        let generation = lance.document_generation();
        let (offset, invalidated) = document_cursor_offset(params.cursor.as_ref(), generation, query_fingerprint)?;
        let fetch_limit = offset
            .checked_add(candidate_limit)
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| McpError::invalid_params("document cursor offset is too large", None))?;
        let hits_raw = tokio::task::spawn_blocking(move || {
            if invalidated {
                return Ok(Vec::new());
            }
            let mut predicate = format!("scope = '{}'", crate::lance::escape_sql_literal(&scope_for_lane));
            if let Some(mime) = mime_for_lane.as_deref() {
                predicate.push_str(" AND mime_type = '");
                predicate.push_str(&crate::lance::escape_sql_literal(mime));
                predicate.push('\'');
            }
            if !facet_sql_for_lane.is_empty() {
                predicate.push_str(" AND ");
                predicate.push_str(&facet_sql_for_lane);
            }
            let phrases = crate::lance::fts::quoted_phrases(&query);
            let terms: Vec<String> = query.replace('"', " ").split_whitespace().map(str::to_string).collect();
            let mut merged = Vec::new();
            let mut seen = std::collections::HashSet::new();
            if let Some(candidate) = crate::lance::fts::exact_candidate(&query) {
                match basemind_exact(&lance, &candidate, &predicate, fetch_limit) {
                    Ok(hits) => {
                        for hit in hits {
                            if !crate::extract::doc::citations::extract_citations(&hit.cites, 64)
                                .iter()
                                .any(|cite| cite.eq_ignore_ascii_case(&candidate))
                            {
                                continue;
                            }
                            if seen.insert((hit.path.clone(), hit.chunk_idx)) {
                                merged.push(hit);
                            }
                            if merged.len() >= fetch_limit {
                                break;
                            }
                        }
                    }
                    Err(error) => tracing::warn!(?error, "exact citation lane failed; continuing"),
                }
            }
            for phrase in phrases {
                if merged.len() >= fetch_limit {
                    break;
                }
                match basemind_phrase(&lance, &phrase, &predicate, fetch_limit) {
                    Ok(hits) => {
                        for hit in hits {
                            if seen.insert((hit.path.clone(), hit.chunk_idx)) {
                                merged.push(hit);
                            }
                            if merged.len() >= fetch_limit {
                                break;
                            }
                        }
                    }
                    Err(error) => tracing::warn!(?error, "phrase lane failed; continuing"),
                }
            }
            if !terms.is_empty() && merged.len() < fetch_limit {
                for hit in basemind_lexical(&lance, &terms, &predicate, fetch_limit, max_relaxations)? {
                    if seen.insert((hit.path.clone(), hit.chunk_idx)) {
                        merged.push(hit);
                    }
                }
            }
            merged.truncate(fetch_limit);
            Ok::<Vec<crate::lance::fts::LexicalHit>, anyhow::Error>(merged)
        })
        .await
        .map_err(|e| McpError::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| McpError::internal_error(format!("lexical search: {e}"), None))?;
        let has_more = hits_raw.len() > offset.saturating_add(candidate_limit);
        let hits: Vec<DocumentSearchHit> = hits_raw
            .into_iter()
            .skip(offset)
            .take(candidate_limit)
            // No meaning for a lexical hit: FTS returns a relevance score, not a distance.
            .map(|h| lexical_hit_to_document_hit(h, 0.0))
            .collect();
        let raw_page_len = hits.len();
        (
            hits,
            generation,
            offset as u64 + raw_page_len as u64,
            raw_page_len,
            has_more,
            invalidated,
        )
    } else {
        let embedding = embed_query(state, &params.query).await?;
        let lance = lance_store(state).await?;
        let mime = params.mime_type.clone();
        let scope_for_lane = scope.clone();
        let query = params.query.clone();
        let facet_filter_for_lane = facet_filter.clone();
        let generation = lance.document_generation();
        let (offset, invalidated) = document_cursor_offset(params.cursor.as_ref(), generation, query_fingerprint)?;
        let fetch_limit = offset
            .checked_add(candidate_limit)
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| McpError::invalid_params("document cursor offset is too large", None))?;
        let hits_raw = tokio::task::spawn_blocking(move || {
            if invalidated {
                return Ok(Vec::new());
            }
            let mut predicate = format!("scope = '{}'", crate::lance::escape_sql_literal(&scope_for_lane));
            if let Some(mime) = mime.as_deref() {
                predicate.push_str(" AND mime_type = '");
                predicate.push_str(&crate::lance::escape_sql_literal(mime));
                predicate.push('\'');
            }
            if !facet_sql.is_empty() {
                predicate.push_str(" AND ");
                predicate.push_str(&facet_sql);
            }
            let mut merged = Vec::with_capacity(fetch_limit);
            let mut seen = std::collections::HashSet::new();
            if let Some(candidate) = crate::lance::fts::exact_candidate(&query) {
                match basemind_exact(&lance, &candidate, &predicate, fetch_limit) {
                    Ok(hits) => {
                        for hit in hits {
                            if !crate::extract::doc::citations::extract_citations(&hit.cites, 64)
                                .iter()
                                .any(|cite| cite.eq_ignore_ascii_case(&candidate))
                            {
                                continue;
                            }
                            if seen.insert((hit.path.clone(), hit.chunk_idx)) {
                                merged.push(lexical_hit_to_document_hit(hit, 0.0));
                            }
                            if merged.len() >= fetch_limit {
                                break;
                            }
                        }
                    }
                    Err(error) => tracing::warn!(?error, "exact citation lane failed; continuing"),
                }
            }
            for phrase in crate::lance::fts::quoted_phrases(&query) {
                if merged.len() >= fetch_limit {
                    break;
                }
                match basemind_phrase(&lance, &phrase, &predicate, fetch_limit) {
                    Ok(hits) => {
                        for hit in hits {
                            if seen.insert((hit.path.clone(), hit.chunk_idx)) {
                                merged.push(lexical_hit_to_document_hit(hit, 0.0));
                            }
                            if merged.len() >= fetch_limit {
                                break;
                            }
                        }
                    }
                    Err(error) => tracing::warn!(?error, "phrase lane failed; continuing"),
                }
            }
            if merged.len() < fetch_limit {
                for hit in lance.search_documents_filtered(
                    &scope_for_lane,
                    embedding,
                    fetch_limit,
                    mime.as_deref(),
                    Some(&facet_filter_for_lane),
                )? {
                    if seen.insert((hit.path.clone(), hit.chunk_idx)) {
                        let (keywords, entities, summary) = facets_to_wire(hit.facets.clone());
                        merged.push(DocumentSearchHit {
                            path: hit.path,
                            chunk_idx: hit.chunk_idx,
                            text: hit.text,
                            mime_type: hit.mime_type,
                            byte_start: hit.byte_start,
                            byte_end: hit.byte_end,
                            distance: hit.distance,
                            matched_lanes: vec!["vector".to_string()],
                            retrieval_mode: "vector".to_string(),
                            citation: None,
                            rerank_score: None,
                            keywords,
                            entities,
                            summary,
                        });
                    }
                    if merged.len() >= fetch_limit {
                        break;
                    }
                }
            }
            merged.truncate(fetch_limit);
            Ok::<Vec<DocumentSearchHit>, anyhow::Error>(merged)
        })
        .await
        .map_err(|e| McpError::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| McpError::internal_error(format!("search_documents: {e}"), None))?;
        let has_more = hits_raw.len() > offset.saturating_add(candidate_limit);
        let hits: Vec<DocumentSearchHit> = hits_raw.into_iter().skip(offset).take(candidate_limit).collect();
        let raw_page_len = hits.len();
        (
            hits,
            generation,
            offset as u64 + raw_page_len as u64,
            raw_page_len,
            has_more,
            invalidated,
        )
    };

    attach_doc_metadata(
        state,
        &mut hits,
        params.entity_category.as_deref(),
        params.keywords_contains.as_deref(),
    )
    .await?;

    if reranker_enabled && !hits.is_empty() {
        let cfg = &state.shared.config.documents.reranker;
        let krz_model = match cfg.resolve_model(explicit_preset.as_deref()) {
            crate::config::ResolvedRerankerModel::Preset { name } => {
                if xberg::get_reranker_preset(&name).is_none() {
                    return Err(McpError::invalid_params(
                        format!("unknown reranker preset: {name:?}"),
                        None,
                    ));
                }
                xberg::core::config::RerankerModelType::Preset { name }
            }
            crate::config::ResolvedRerankerModel::Custom { model_id, model_file } => {
                xberg::core::config::RerankerModelType::Custom {
                    model_id,
                    model_file: Some(model_file),
                    additional_files: Vec::new(),
                    max_length: None,
                    head: xberg::core::config::RerankerHead::CrossEncoder,
                }
            }
        };
        let krz_config = xberg::core::config::RerankerConfig {
            model: krz_model,
            top_k: Some(reranker_top_k),
            ..Default::default()
        };
        let documents: Vec<String> = hits.iter().map(|h| h.text.clone()).collect();
        let reranked = xberg::rerank_async(params.query.clone(), documents, &krz_config)
            .await
            .map_err(|e| {
                let msg = e.to_string();
                let kind = if msg.contains("download") || msg.contains("HuggingFace") || msg.contains("model") {
                    "rerank model load"
                } else {
                    "rerank inference"
                };
                McpError::internal_error(format!("{kind}: {msg}"), None)
            })?;
        let original_hits = std::mem::take(&mut hits);
        hits = reranked
            .into_iter()
            .map(|r| {
                original_hits
                    .get(r.index)
                    .cloned()
                    .map(|mut hit| {
                        hit.rerank_score = Some(r.score);
                        hit
                    })
                    .ok_or_else(|| {
                        McpError::internal_error(
                            format!(
                                "reranker returned out-of-range index {} (got {} hits)",
                                r.index,
                                original_hits.len()
                            ),
                            None,
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
    }

    hits.truncate(limit);

    let output_format = match params.format.as_deref().map(str::trim) {
        Some(f) if f.eq_ignore_ascii_case("toon") => crate::config::OutputFormat::Toon,
        Some(f) if f.eq_ignore_ascii_case("json") => crate::config::OutputFormat::Json,
        _ => output_format,
    };

    let budget = super::budget::apply_budget(hits, params.max_tokens);
    let budgeted = budget.budgeted;
    let budget_kept_len = budget.items.len();
    let page_start = cursor_offset.saturating_sub(raw_page_len as u64);
    let next_offset = if budgeted {
        page_start.saturating_add(budget_kept_len as u64)
    } else {
        cursor_offset
    };
    let next_cursor = if !cursor_invalidated && (has_more || budgeted) && next_offset <= MAX_DOCUMENT_CURSOR_OFFSET {
        Some(crate::mcp::cursor::Cursor::encode_documents(
            next_offset,
            cursor_generation,
            query_fingerprint,
        ))
    } else {
        None
    };
    format_response(
        &SearchDocumentsResponse {
            query: params.query,
            budgeted,
            hits: budget.items,
            next_cursor,
            cursor_invalidated,
            elapsed_us: elapsed_us(__body),
        },
        output_format,
    )
}

#[path = "memory_documents.rs"]
mod document_helpers;
use document_helpers::{
    MAX_DOCUMENT_CURSOR_OFFSET, attach_doc_metadata, document_cursor_offset, document_query_fingerprint,
};
#[cfg(all(test, feature = "crawl"))]
mod scope_tests {
    use super::resolve_doc_scope;
    use crate::web::ingest::default_scope;

    #[test]
    fn defaults_to_the_repo_scope_when_the_caller_names_none() {
        assert_eq!(resolve_doc_scope(None, "repo-abc123"), "repo-abc123");
    }

    #[test]
    fn a_scraped_page_is_reachable_only_by_naming_its_web_scope() {
        let url = crate::url::Url::parse("https://example.com/page").unwrap();
        let write_scope = default_scope(&url);
        assert_eq!(write_scope, "web:example.com");

        let repo_scope = "repo-abc123";
        assert_ne!(resolve_doc_scope(None, repo_scope), write_scope);

        assert_eq!(resolve_doc_scope(Some(&write_scope), repo_scope), write_scope);
    }
}

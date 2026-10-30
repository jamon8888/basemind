//! Document-tier branch helpers for the scanner.
//!
//! Gated on `feature = "documents"`. Lives in its own file so the core
//! `src/scanner.rs` module stays under the 1000-line cap and so the
//! intelligence-only code path is easy to inspect in isolation.
//!
//! The flow that the scanner drives is:
//!
//! 1. `process_file` sees `lang::detect` return `None` (non-source file).
//! 2. `should_extract_document` decides whether the file qualifies for the
//!    document tier based on `[documents]` config + MIME allowlist.
//! 3. `extract_and_persist_doc` runs xberg, writes the msgpack blob, and
//!    returns a `PendingDocBatch` carrying only the lightweight metadata (path,
//!    blob hash, scope, counts) needed to rebuild rows later — never the
//!    embedding vectors themselves.
//! 4. The single-threaded apply pass calls `flush_document_batches`, which — per
//!    file — re-reads that file's already-persisted `.doc.msgpack` blob, builds
//!    its LanceDB rows, writes them, and drops them before moving on. Peak embed
//!    RAM is one file's rows, independent of corpus size (the streaming write
//!    that replaced the old "accumulate every file's rows, then write once" pass
//!    which held multiple GB resident on a large repo).

#![cfg(feature = "documents")]

use std::path::Path;
use std::sync::{Arc, OnceLock};

use ahash::AHashSet;
use anyhow::Context as _;
use xberg::core::mime;

use crate::config::{DocumentsConfig, LlmConfig, ResourcesConfig};
use crate::extract::doc::ocr_signal;
use crate::extract::doc::{DocConfig, FileMapDoc, extract_doc_with_metrics};
use crate::extract::doc_guard::ChunkCutovers;
use crate::hashing::{self, Hash};
#[cfg(feature = "graphrag")]
use crate::lance::DocEntitySidecar;
use crate::lance::{DocumentRow, PendingBatch};
use crate::pii::{ChunkSpan, FindingInput};
use crate::scanner::EmbedMode;
use crate::scanner_filter::Filters;
use crate::store::Store;

/// Per-file deferred-LanceDB-write descriptor. Constructed inside `process_file`'s parallel
/// worker; consumed in the single-threaded apply pass via [`flush_document_batches`].
///
/// Deliberately **metadata only** — it carries the content hash needed to re-read the persisted
/// `.doc.msgpack` blob at flush time, NOT the chunk text or the embedding vectors. Accumulating the
/// vectors here (one `Vec<f32>` per chunk, hundreds of chunks per document) across an entire corpus
/// was the memory leak this shape fixes: the flush now rebuilds one file's rows at a time.
#[derive(Debug, Clone)]
pub(crate) struct PendingDocBatch {
    /// Repository-relative path, forward-slash separated. Becomes the `path`
    /// column in LanceDB.
    pub rel_path: String,
    /// Content hash (hex) of the source document — the key under which the `.doc.msgpack` blob is
    /// content-addressed. The flush re-reads that blob to rebuild rows on demand.
    pub blob_hash: String,
    /// LanceDB scope stamped onto this file's emitted rows (the repo scope, or `path:<extra_root>`
    /// for an external-root document). **Both** the delete predicate and the inserted rows use it:
    /// they must match or the delete targets a scope the rows were never written under, and stale
    /// rows accumulate on every re-extraction. `scanner_doc_links` has always done this.
    pub doc_scope: String,
    /// Number of chunks indexed (zero is valid — xberg may yield no chunks
    /// when the file body is empty or below the chunk threshold).
    pub chunk_count: usize,
    /// Approximate whitespace-delimited token count used for benchmark throughput.
    pub token_count: usize,
    /// UTF-8 bytes in the extracted content, used as processed-work throughput.
    pub content_bytes: usize,
    /// Length of each chunk's embedding vector. Zero when embeddings are off.
    pub embedding_dim: u16,
    /// Whether this file should emit LanceDB rows at flush time. False for the no-embed, empty, and
    /// over-`max_chunks_per_document` cases — the blob is still tracked, but nothing lands in LanceDB
    /// (mirrors the old rows-empty batch). Decided at construction, where the document is in hand.
    pub emit_rows: bool,
    /// Whether the doc leaves this pass with its embedding requirement satisfied — persisted onto
    /// the [`crate::store::DocEntry`] so the next pass's unchanged fast path can tell a fully
    /// embedded doc from a tracked-but-vectorless one (issue #44). True when the pass didn't ask to
    /// embed, when vectors are present, or when the doc has no chunks to embed.
    pub embedded: bool,
    /// Whether this pass actually ran an embedding attempt on the doc (embedding requested, freshly
    /// extracted) — even if xberg returned no vectors. Persisted onto
    /// [`crate::store::DocEntry::embed_attempted`] so the unchanged fast path stops re-extracting and
    /// re-embedding a deterministically-unembeddable doc on every scan (issue #44 follow-up). See that
    /// field for the full healing contract.
    pub embed_attempted: bool,
    /// True when this batch came from the cached-blob reuse branch of `extract_and_persist_doc`
    /// rather than a fresh xberg extraction. Drives the `reused_doc_extraction` scan counter — the
    /// observable proof that churn (renames, rewrites) does not re-run extraction or embedding.
    pub reused: bool,
    /// Hash of the encrypted rehydration blob, if redaction captured one.
    /// Threaded into [`crate::store::DocEntry`] so the GC live set protects it.
    pub rehydration_ref: Option<String>,
    /// True when the document was chunked by the linear fallback instead of xberg's markdown splitter
    /// (large text-like file). Drives the `docs_degraded` scan counter.
    pub degraded: bool,
    /// True when structured extraction failed and the file was re-read as `text/plain` (implies
    /// `degraded`). Drives the `docs_text_fallback` scan counter.
    pub text_fallback: bool,
}

/// Look the configured embedding preset up in xberg's preset table and
/// return its vector dimension as a `u16` (LanceDB's `FixedSizeList<Float32, N>`
/// uses `i32` but we treat the value as a `u16` everywhere because every
/// shipped preset is < 65 535 dims).
///
/// Returns an error rather than guessing when the preset name is unknown — a
/// silent fallback would create a LanceDB table with the wrong dim and force a
/// later wipe-and-rebuild.
pub(crate) fn preset_dim(name: &str) -> anyhow::Result<u16> {
    let dimensions = crate::embeddings::resolve_embedding_dims(name)
        .with_context(|| format!("unknown xberg embedding preset: {name}"))?;
    u16::try_from(dimensions).with_context(|| format!("preset {name} dimensions {dimensions} exceeds u16"))
}

/// Translate the project-level `[documents]` config into the xberg-facing
/// [`DocConfig`] the extractor expects. Pulled out so the wiring in
/// `process_file` stays a single call.
pub(crate) fn doc_config_from(
    cfg: &DocumentsConfig,
    llm: &LlmConfig,
    resources: &ResourcesConfig,
    embed: bool,
) -> DocConfig {
    DocConfig {
        max_pages: cfg.max_pages,
        extraction_timeout_secs: cfg.extraction_timeout_secs,
        max_abandoned_extractions: cfg.max_abandoned_extractions,
        phase: None,
        max_chunks_per_document: cfg.max_chunks_per_document,
        chunk_cutovers: ChunkCutovers {
            markdown_bytes: cfg.markdown_chunk_max_bytes,
            plain_text_bytes: cfg.plain_text_chunk_max_bytes,
        },
        max_characters: cfg.max_characters,
        overlap: cfg.overlap,
        embedding_preset: Some(cfg.embedding_preset.clone()),
        embed,
        language: cfg.language.clone(),
        keywords: cfg.keywords.clone(),
        ner: cfg.ner.clone(),
        summarization: cfg.summarization.clone(),
        llm: llm.clone(),
        embed_max_threads: resources.effective_embed_threads(cfg.embed_max_threads),
        embed_batch_size: resources.embed_batch_size,
        document_models: resources.document_models,
        redaction: cfg.redaction.clone(),
        onnx_provider: resources.onnx_provider,
        max_archive_bytes: usize::try_from(cfg.max_archive_bytes).unwrap_or(usize::MAX),
        max_archive_entries: cfg.max_archive_entries,
    }
}
/// Archive / compressed-container extensions. xberg routes `.zip/.tar/.gz/...` into its
/// `ZipExtractor`/`GzipExtractor`/`build_archive_doc` path, which recursively unpacks the archive and
/// embeds every entry — an enormous, pointless cost during a code-map scan (the observed 1119%-CPU /
/// 15 GB footgun). Rejected by default so a single archive can't explode into thousands of embeds;
/// gated OFF (relaxed → routed to xberg's archive extractor) when `documents.extract_archives` is
/// set. This extension floor is the primary guard because `mime::detect_mime_type` is extension-based
/// and several of these (`.jar/.whl/.war/...`) collapse to `application/octet-stream` via the
/// `mime_guess` fallback, so a MIME-only denylist would miss them. Kept separate from
/// [`BINARY_EXTENSIONS`] so the toggle only affects archives, never true binaries.
const ARCHIVE_EXTENSIONS: &[&str] = &[
    "zip", "tar", "gz", "tgz", "bz2", "tbz2", "xz", "txz", "zst", "zstd", "7z", "rar", "lz", "lz4", "lzma", "br",
    "cab", "ar", "iso", "dmg", "jar", "war", "ear", "apk", "whl", "egg", "deb", "rpm", "nupkg", "pkg", "msi", "crate",
];

/// Native / compiled-binary extensions. ALWAYS rejected — these carry no extractable text, and
/// `extract_archives` never relaxes them (they are not archives).
const BINARY_EXTENSIONS: &[&str] = &[
    "so", "dylib", "dll", "a", "o", "obj", "bin", "exe", "wasm", "class", "pyc", "pyo", "pyd", "node", "pack", "idx",
];

/// Document formats temporarily rejected because xberg 1.0.14 routes them through biblib 0.4.3's
/// vulnerable quick-xml 0.37 parser (RUSTSEC-2026-0194). Remove with the matching MIME guard once
/// xberg #1398 ships its already-landed biblib 0.8 / quick-xml 0.41 dependency update.
const UNSAFE_DOCUMENT_EXTENSIONS: &[&str] = &["enw"];

/// MIME counterpart to [`UNSAFE_DOCUMENT_EXTENSIONS`]. Keeping both guards makes the rejection
/// explicit even if xberg's extension-based MIME detection learns another EndNote XML suffix.
const UNSAFE_DOCUMENT_MIME: &[&str] = &["application/x-endnote+xml"];

/// Archive MIME denylist (belt-and-suspenders with [`ARCHIVE_EXTENSIONS`]) — catches content-typed
/// archives xberg maps to a real archive MIME. Gated OFF by `documents.extract_archives`, mirroring
/// the extension floor. Entries ending in `/` are prefix matches (see [`matches_mime`]).
const ARCHIVE_MIME: &[&str] = &[
    "application/zip",
    "application/x-tar",
    "application/gzip",
    "application/x-7z-compressed",
    "application/java-archive",
    "application/vnd.rar",
];

/// Binary MIME denylist — the unambiguously non-extractable native-binary + audio/video/font
/// families. ALWAYS rejected (never relaxed by `extract_archives`). `image/` is deliberately NOT
/// denied: xberg OCR can extract text from images, so images retain their allowlist behavior.
const BINARY_MIME: &[&str] = &[
    "application/wasm",
    "application/x-executable",
    "application/x-sharedlib",
    "application/x-mach-binary",
    "application/octet-stream",
    "audio/",
    "video/",
    "font/",
];

/// Lazily-initialized `AHashSet` view of `exts`, built once and reused across all scanner threads.
///
/// Replaces the O(n) `slice::contains` call in [`is_denied_binary_or_archive`] with an O(1) hash
/// lookup. The `OnceLock` ensures thread-safe initialisation.
fn ext_set(
    cell: &'static OnceLock<AHashSet<&'static str>>,
    exts: &'static [&'static str],
) -> &'static AHashSet<&'static str> {
    cell.get_or_init(|| exts.iter().copied().collect())
}

fn archive_ext_set() -> &'static AHashSet<&'static str> {
    static SET: OnceLock<AHashSet<&'static str>> = OnceLock::new();
    ext_set(&SET, ARCHIVE_EXTENSIONS)
}

fn binary_ext_set() -> &'static AHashSet<&'static str> {
    static SET: OnceLock<AHashSet<&'static str>> = OnceLock::new();
    ext_set(&SET, BINARY_EXTENSIONS)
}

/// True when a file must be skipped by the document tier because it is unsafe to parse, a binary
/// blob, or (unless `documents.extract_archives` is set) an archive / compressed container. Unsafe
/// formats, true binaries, and the user-configured `extension_denylist` are always rejected; the
/// archive floor is relaxed only when the caller opts into archive extraction.
fn is_denied_binary_or_archive(abs: &Path, mime_type: &str, cfg: &DocumentsConfig) -> bool {
    if let Some(ext) = abs.extension().and_then(|e| e.to_str()) {
        let ext_lower = ext.to_ascii_lowercase();
        if UNSAFE_DOCUMENT_EXTENSIONS.contains(&ext_lower.as_str())
            || binary_ext_set().contains(ext_lower.as_str())
            || cfg
                .extension_denylist
                .iter()
                .any(|e| crate::config::rules::normalize_extension(e) == ext_lower)
        {
            return true;
        }
        if !cfg.extract_archives && archive_ext_set().contains(ext_lower.as_str()) {
            return true;
        }
    }
    if BINARY_MIME.iter().any(|entry| matches_mime(entry, mime_type)) {
        return true;
    }
    if UNSAFE_DOCUMENT_MIME.iter().any(|entry| matches_mime(entry, mime_type)) {
        return true;
    }
    !cfg.extract_archives && ARCHIVE_MIME.iter().any(|entry| matches_mime(entry, mime_type))
}

/// Quick filter run before any xberg work happens. Returns the detected
/// MIME type when the file should be document-extracted, or `None` when it
/// should be skipped (configured-off, archive/binary, MIME unknown, MIME outside the allowlist).
///
/// The MIME allowlist is treated as "match this exact MIME OR a prefix ending
/// in `/`" so callers can say `"image/"` to whitelist every image type.
pub(crate) fn should_extract_document(abs: &Path, cfg: &DocumentsConfig) -> Option<String> {
    if !cfg.enabled {
        return None;
    }
    let mime_type = mime::detect_mime_type(abs, false).ok()?;
    if is_denied_binary_or_archive(abs, &mime_type, cfg) {
        return None;
    }
    if cfg.mime_allowlist.is_empty() {
        return Some(mime_type);
    }
    let allowed = cfg.mime_allowlist.iter().any(|entry| matches_mime(entry, &mime_type));
    if allowed { Some(mime_type) } else { None }
}

fn matches_mime(entry: &str, mime_type: &str) -> bool {
    if entry == mime_type {
        return true;
    }
    if let Some(prefix) = entry.strip_suffix('/') {
        return mime_type.starts_with(prefix) && mime_type.as_bytes().get(prefix.len()) == Some(&b'/');
    }
    false
}

/// Reconcile NER spans with gaze gateway tokens (spec 0016 §15-Q5 / #111).
///
/// xberg's NER returns the matched span (`Name_1`, or even the bare anchor
/// `4b31c7ee`), not the full gateway token `<4b31c7ee:Name_1>` that gaze wrote
/// into the document. Any entity whose text sits inside a gateway-token
/// occurrence in the document body is re-attributed to that token, so the graph
/// keys on the stable cross-document token identity
/// (`EntityKeySource::GatewayToken`) instead of a document-local span literal.
/// Matching is normalization-tolerant (lowercase, alphanumerics only) because
/// the span text and the token interior are normalized differently.
#[cfg(feature = "graphrag")]
fn reconcile_gateway_tokens(entities: &mut [DocEntitySidecar], content: &str) {
    static TOKEN_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = TOKEN_RE.get_or_init(|| {
        // gaze token shape: <anchor:Class_N> with an 8-hex anchor (the
        // workspace_stability_probe contract, #107).
        regex::Regex::new(r"<([0-9a-fA-F]{8}):([A-Za-z]+)_(\d+)>").expect("gateway token regex")
    });

    // (normalized key, full token) per occurrence — inner and bare anchor.
    let mut candidates: Vec<(usize, usize, String, String)> = Vec::new();
    for m in re.captures_iter(content) {
        let full = m.get(0).map(|t| t.as_str()).unwrap_or_default().to_string();
        let class = m.get(2).map(|value| value.as_str()).unwrap_or_default();
        let ordinal = m.get(3).map(|value| value.as_str()).unwrap_or_default();
        let anchor = m.get(1).map(|value| value.as_str()).unwrap_or_default();
        let span = m.get(0).expect("regex match");
        candidates.push((
            span.start(),
            span.end(),
            format!("{}{}", class.to_lowercase(), ordinal),
            full.clone(),
        ));
        candidates.push((span.start(), span.end(), anchor.to_lowercase(), full));
    }
    if candidates.is_empty() {
        return;
    }

    let norm = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect()
    };

    for ent in entities.iter_mut() {
        if ent.is_gateway_token || ent.gateway_token.is_some() {
            continue;
        }
        let needle = norm(&ent.literal);
        if needle.len() < 3 {
            continue;
        }
        // Exact needle match wins over containment.
        let hit = candidates
            .iter()
            .filter(|(start, end, _, _)| ent.start as usize >= *start && ent.end as usize <= *end)
            .find(|(_, _, k, _)| *k == needle)
            .or_else(|| {
                candidates
                    .iter()
                    .filter(|(start, end, _, _)| ent.start as usize >= *start && ent.end as usize <= *end)
                    .find(|(_, _, k, _)| k.contains(&needle))
            });
        if let Some((_, _, _, full)) = hit {
            ent.is_gateway_token = true;
            ent.gateway_token = Some(full.clone());
            ent.literal = full.clone();
        }
    }
}

/// Run xberg against `abs`, write the document blob to the content-addressed
/// store, and assemble a [`PendingDocBatch`] for the apply pass. Returns
/// `Ok(None)` when extraction succeeded but produced no embeddings (we still
/// persist the blob; the LanceDB side is just a no-op for that file).
#[allow(clippy::too_many_arguments)]
pub(crate) fn extract_and_persist_doc(
    store: &Store,
    rel: &str,
    abs: &Path,
    hash: &Hash,
    mime_type: &str,
    cfg: &DocumentsConfig,
    llm: &LlmConfig,
    resources: &ResourcesConfig,
    filters: &Filters,
    scope: &str,
    mode: EmbedMode,
    metrics: &crate::scan_metrics::ScanStageMetrics,
) -> Result<Option<PendingDocBatch>, anyhow::Error> {
    let embed = doc_embed_requested(rel, cfg, mode, filters);
    let hex_buf = hashing::hex_buf(hash);
    let hash_hex = hashing::hex_str(&hex_buf);

    if let Some(mut cached) = store.read_doc_by_hex(hash_hex).ok().flatten()
        && cached_doc_is_reusable(&cached, cfg, embed, &filters.doc_digest)
    {
        // The same pass rewrites whatever the blob still carries from an older regime —
        // a digest-less blob is stamped with the current one, and a rehydration_ref left
        // by the dropped rehydration vault is cleared: basemind keeps no key at rest, so
        // the index must not point at an old token→original blob (which would also keep
        // it in the GC's live set instead of letting it be reaped).
        let mut rewrite = false;
        // Cached blobs predate the OCR signal or may have been created before the
        // signal was enabled. Re-evaluate the cached OCR text under the current
        // profile so reusable images receive the same warning and metadata note
        // as freshly extracted images.
        if let Some(ocr_chars) = ocr_signal::check(
            abs,
            Some(&cached.mime_type),
            &resources.document_models,
            &cached.content,
        ) && !cached.metadata.iter().any(|(key, _)| key == "ocr_empty")
        {
            cached.metadata.push(ocr_signal::note(ocr_chars));
            rewrite = true;
        }
        if cached.config_digest.is_empty() {
            cached.config_digest.clone_from(&filters.doc_digest);
            rewrite = true;
        }
        if cached.rehydration_ref.is_some() {
            cached.rehydration_ref = None;
            rewrite = true;
        }
        if rewrite {
            store
                .write_doc(hash, &cached)
                .with_context(|| format!("rewrite doc blob for {rel}"))?;
        }
        // Keep this reused blob "young" so the blob GC's grace window protects it through a NoCache
        // rename's transient entry-less gap (issue #44) instead of reaping it and forcing a re-embed.
        store.touch_doc_blob(hash_hex);

        // Build GraphRAG entity graph from cached doc's NER sidecar (spec 0016).
        #[cfg(feature = "graphrag")]
        if cfg.graphrag.enabled {
            // The entity graph lives in lance even when `embed = false`. This path holds
            // `&Store` (immutable) so `lance_or_open` is out of reach — open a standalone
            // handle against the same `.basemind/lance/` dir instead; LanceDB connections
            // to one store are independent and safe to hold in parallel.
            let lance = store.lance.clone().or_else(|| {
                preset_dim(&cfg.embedding_preset).ok().and_then(|dim| {
                    let dir = store.basemind_dir.join(crate::store_layout::LANCE_DIR);
                    crate::lance::LanceStore::open(&dir, dim, &cfg.embedding_preset)
                        .map_err(|error| {
                            tracing::warn!(?error, "graphrag: open LanceStore failed; skipping graph build")
                        })
                        .ok()
                })
            });
            if let Some(lance) = lance {
                use crate::lance::graphrag::{
                    DocEntitySidecar, GraphragConfig, build_graph_from_entities, upsert_graph_edges,
                    upsert_graph_entities,
                };

                let mut doc_entities: Vec<DocEntitySidecar> = cached
                    .entities
                    .iter()
                    .map(|e| {
                        let is_token = e.category.starts_with('[') && e.category.ends_with(']');
                        // Detect gaze gateway tokens: <anchor:Class_N> or anchor.Class_N formats
                        let is_gateway_token = e.text.starts_with('<') && e.text.contains(':') && e.text.ends_with('>')
                            || e.text.contains('.') && e.text.contains('@') && e.text.contains("gaze-fake.invalid");
                        let gateway_token = if is_gateway_token { Some(e.text.clone()) } else { None };
                        DocEntitySidecar {
                            label: e.category.clone(),
                            literal: e.text.clone(),
                            is_token,
                            is_gateway_token,
                            gateway_token,
                            start: e.start,
                            end: e.end,
                            chunk_idx: 0,
                            doc_path: rel.to_string(),
                        }
                    })
                    .collect();
                reconcile_gateway_tokens(&mut doc_entities, &cached.content);

                let graphrag_cfg = GraphragConfig {
                    enabled: true,
                    labels: cfg.graphrag.labels.clone(),
                    max_rows: cfg.graphrag.max_rows,
                    hops: cfg.graphrag.hops,
                };

                if let Ok((entity_rows, edge_rows)) = build_graph_from_entities(scope, &doc_entities, &graphrag_cfg) {
                    if !entity_rows.is_empty() {
                        let _ = crate::lance::graphrag::upsert_graph_entities(
                            lance.connection(),
                            lance.runtime(),
                            &entity_rows,
                        );
                    }
                    if !edge_rows.is_empty() {
                        let _ =
                            crate::lance::graphrag::upsert_graph_edges(lance.connection(), lance.runtime(), &edge_rows);
                    }
                }
            }
        }

        return Ok(Some(pending_from_doc(&cached, rel, hash_hex, scope, cfg, embed, true)));
    }

    let mut doc_config = doc_config_from(cfg, llm, resources, embed);
    doc_config.phase = Some(Arc::clone(&filters.doc_phase));
    let mut doc: FileMapDoc = extract_doc_with_metrics(abs, Some(mime_type), &doc_config, Some(metrics))
        .with_context(|| format!("extract document {rel}"))?;
    doc.config_digest.clone_from(&filters.doc_digest);

    // The rehydration map captured during extraction is NOT persisted here.
    //
    // It used to be encrypted with a passphrase derived from the workspace scope and the data
    // directory (a 64-bit hash of two values the reader can compute), stored as a
    // content-addressed blob, and referenced from the index through `rehydration_ref`. Anyone
    // who knew the scope and the data directory could recompute that key and decrypt the map —
    // the token→original table of every redacted document in the workspace. In the gateway
    // (passerelle) architecture the true values belong to the gateway, which keeps its keys in
    // the system keychain; basemind is the engine and keeps no key at rest.
    //
    // The map stays in memory for the length of the extraction (the tax-id relabeling reads it)
    // and is dropped with the document. `rehydration_ref` therefore stays `None`; the field and
    // its index column remain for schema stability, and the blob GC reaps the blobs older
    // workspaces left behind.

    // Wire redaction findings into the pii_lineage GDPR Article 30 audit trail.
    if !doc.redaction_findings.is_empty()
        && let Some(idx) = store.index_db.as_ref()
    {
        let findings_input: Vec<FindingInput> = doc
            .redaction_findings
            .iter()
            .map(|f| FindingInput {
                start: f.start,
                end: f.end,
                category: f.category.clone(),
                token: f.replacement_token.clone(),
            })
            .collect();
        let chunk_spans: Vec<ChunkSpan> = doc
            .chunks
            .iter()
            .map(|c| ChunkSpan {
                byte_start: c.byte_start,
                byte_end: c.byte_end,
            })
            .collect();
        let detected_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as i64;
        let (entities, _stats) =
            crate::pii::translate_findings(scope, rel, &findings_input, &chunk_spans, detected_at, "basemind-scan");
        for entity in &entities {
            if let Err(e) = crate::index::pii_lineage::put_entity(idx, scope, rel, entity) {
                tracing::warn!(rel, ?e, "pii_lineage: failed to write entity");
            }
        }
    }
    // Build GraphRAG entity graph from NER sidecar (spec 0016).
    // Only runs when graphrag feature is enabled and config has graphrag.enabled = true.
    #[cfg(feature = "graphrag")]
    if cfg.graphrag.enabled {
        // Same standalone-open rationale as the cached-doc path above: `&Store` is
        // immutable here, and the graph must survive `embed = false` on a fresh workspace.
        let lance = store.lance.clone().or_else(|| {
            preset_dim(&cfg.embedding_preset).ok().and_then(|dim| {
                let dir = store.basemind_dir.join(crate::store_layout::LANCE_DIR);
                crate::lance::LanceStore::open(&dir, dim, &cfg.embedding_preset)
                    .map_err(|error| tracing::warn!(?error, "graphrag: open LanceStore failed; skipping graph build"))
                    .ok()
            })
        });
        if let Some(lance) = lance {
            use crate::lance::graphrag::{
                DocEntitySidecar, GraphragConfig, build_graph_from_entities, upsert_graph_edges, upsert_graph_entities,
            };

            // Convert FileMapDoc entities to DocEntitySidecar format
            let mut doc_entities: Vec<DocEntitySidecar> = doc
                .entities
                .iter()
                .map(|e| {
                    let is_token = e.category.starts_with('[') && e.category.ends_with(']'); // token_replace markers like [EMAIL_1]
                    // Detect gaze gateway tokens: <anchor:Class_N> or anchor.Class_N formats
                    let is_gateway_token = e.text.starts_with('<') && e.text.contains(':') && e.text.ends_with('>')
                        || e.text.contains('.') && e.text.contains('@') && e.text.contains("gaze-fake.invalid");
                    let gateway_token = if is_gateway_token { Some(e.text.clone()) } else { None };
                    DocEntitySidecar {
                        label: e.category.clone(),
                        literal: e.text.clone(),
                        is_token,
                        is_gateway_token,
                        gateway_token,
                        start: e.start,
                        end: e.end,
                        chunk_idx: 0, // We don't have chunk_idx in DocEntity, approximate
                        doc_path: rel.to_string(),
                    }
                })
                .collect();
            reconcile_gateway_tokens(&mut doc_entities, &doc.content);

            let graphrag_cfg = GraphragConfig {
                enabled: true,
                labels: cfg.graphrag.labels.clone(),
                max_rows: cfg.graphrag.max_rows,
                hops: cfg.graphrag.hops,
            };

            if let Ok((entity_rows, edge_rows)) = build_graph_from_entities(scope, &doc_entities, &graphrag_cfg) {
                if !entity_rows.is_empty() {
                    let _ = crate::lance::graphrag::upsert_graph_entities(
                        lance.connection(),
                        lance.runtime(),
                        &entity_rows,
                    );
                }
                if !edge_rows.is_empty() {
                    let _ = crate::lance::graphrag::upsert_graph_edges(lance.connection(), lance.runtime(), &edge_rows);
                }
            }
        }
    }

    let persistence_started = std::time::Instant::now();
    store
        .write_doc(hash, &doc)
        .with_context(|| format!("write doc blob for {rel}"))?;
    metrics.add_persistence(persistence_started.elapsed());

    Ok(Some(pending_from_doc(&doc, rel, hash_hex, scope, cfg, embed, false)))
}

/// Single source of truth for "will this pass embed this doc": embedding runs only on an `Inline`
/// pass, with `[documents] embed` on, for a path not excluded by `embed_exclude`. Shared by
/// `extract_and_persist_doc` (whether to ask xberg for vectors) and the `process_doc` unchanged
/// fast path (whether a tracked-but-unembedded doc must be re-processed) so the two sides can
/// never disagree about the requirement — a disagreement is exactly the issue-#44 loop.
pub(crate) fn doc_embed_requested(rel: &str, cfg: &DocumentsConfig, mode: EmbedMode, filters: &Filters) -> bool {
    matches!(mode, EmbedMode::Inline) && cfg.embed && filters.doc_embed_allowed(rel)
}

/// The pure (IO-free) half of the `process_doc` unchanged fast path: does a tracked entry settle the
/// current pass without re-processing? True when the content hash and preset both match AND either
/// the doc is embedded, a prior pass already attempted embedding (issue #44 follow-up — a failed
/// attempt must not be re-extracted + retried every scan), or this pass does not ask to embed. The
/// caller ANDs this with a blob-existence probe (the blob may have been GC'd out from under a
/// still-tracked entry).
///
/// The entry must also carry the current `config_digest` (so a changed chunk / extraction knob
/// re-processes the doc), and `reflush` (the embed policy changed since the last scan) sends every
/// doc that wants vectors back through the pipeline so its rows are rebuilt from the cached blob.
/// An entry that still carries a legacy `rehydration_ref` does not settle either: basemind keeps
/// no key at rest, so the doc is re-processed (an Inline pass clears the reference and rewrites
/// both blob and entry) instead of staying pinned in the GC's live set forever.
pub(crate) fn doc_entry_settled(
    existing: &crate::store::DocEntry,
    hash_hex: &str,
    embedding_preset: &str,
    embed_requested: bool,
    config_digest: &str,
    reflush: bool,
) -> bool {
    existing.hash_hex == hash_hex
        && existing.embedding_preset == embedding_preset
        && existing.config_digest == config_digest
        && !(embed_requested && reflush)
        && existing.rehydration_ref.is_none()
        && (existing.embedded || existing.embed_attempted || !embed_requested)
}

/// True when a cached document blob can be reused without re-extraction. When embedding is on the
/// cached blob must carry embeddings produced by the current preset — matching both its **dimension**
/// AND its **model**. The model check is load-bearing: `balanced` and `multilingual` share dim 768,
/// so a dim-only gate would falsely reuse stale-model vectors when switching between them. A preset
/// change (dim OR model) therefore forces recompute — same gate as `chunk_and_embed`. An
/// empty-of-chunks doc is always reusable (recompute would yield nothing anyway). When embedding is
/// off, any cached doc is reusable (chunks only). In every case the blob must have been produced
/// under the current `config_digest` (or predate digests, in which case the caller stamps it).
fn cached_doc_is_reusable(cached: &FileMapDoc, cfg: &DocumentsConfig, embed: bool, config_digest: &str) -> bool {
    if !cached.config_digest.is_empty() && cached.config_digest != config_digest {
        return false;
    }
    // Mirror the write path (extract/doc.rs): redaction disabled stamps `None`, enabled
    // stamps `Some(hash)`. Demanding `Some` unconditionally kills reuse for every
    // redaction-off corpus; a config change or enable/disable flip still forces re-extract.
    let want_fingerprint: Option<String> = if cfg.redaction.enabled {
        Some(crate::extract::doc::hash_redaction_config(&cfg.redaction))
    } else {
        None
    };
    if cached.redaction_fingerprint != want_fingerprint {
        return false;
    }
    if !embed || cached.chunks.is_empty() {
        return true;
    }
    let want_dim = preset_dim(&cfg.embedding_preset).ok();
    cached.embedding_dim > 0
        && Some(cached.embedding_dim) == want_dim
        && cached.embedding_model == cfg.embedding_preset
        && cached
            .chunks
            .iter()
            .all(|c| c.embedding.len() == cached.embedding_dim as usize)
}

/// Assemble the deferred-write descriptor from an extracted-or-cached document. Decides — while the
/// document is in hand — whether this file will emit LanceDB rows at flush time: only when embedding
/// is on, the doc has embeddings, it has chunks, and it is under the per-doc chunk cap. The heavy
/// row-building (chunk text + embedding vectors) is deferred to [`flush_document_batches`], which
/// re-reads the persisted blob per file so no embeddings are held across the corpus.
fn pending_from_doc(
    doc: &FileMapDoc,
    rel: &str,
    blob_hash: &str,
    scope: &str,
    cfg: &DocumentsConfig,
    embed: bool,
    reused: bool,
) -> PendingDocBatch {
    let chunk_count = doc.chunks.len();
    let token_count = doc.content.split_whitespace().count();
    let content_bytes = doc.content.len();
    let embedding_dim = doc.embedding_dim;
    let over_cap = chunk_count > cfg.max_chunks_per_document;
    if over_cap {
        tracing::warn!(
            rel,
            chunks = chunk_count,
            cap = cfg.max_chunks_per_document,
            "document exceeds max_chunks_per_document; caching blob but skipping vector rows"
        );
    }
    // A lexical-only scan (`embed = false`) still emits rows — it stores a null vector, not no row.
    // That is the whole point of the nullable column. An embed that was *requested and came back empty*
    // is a different case and keeps emitting nothing: that document is unembeddable, it stays
    // `embedded: false` so the next scan retries it, and writing a vectorless row for it would blur the
    // two states the reuse fast-path depends on.
    let have_vectors = embedding_dim > 0;
    let emit_rows = (!embed || have_vectors) && chunk_count > 0 && !over_cap;
    PendingDocBatch {
        rel_path: rel.to_string(),
        blob_hash: blob_hash.to_string(),
        doc_scope: scope.to_string(),
        chunk_count,
        token_count,
        content_bytes,
        embedding_dim,
        emit_rows,
        embedded: !embed || embedding_dim > 0 || chunk_count == 0,
        // Only a fresh extraction (`!reused`) ran an attempt; a reused blob already carries vectors ~keep
        // (the reuse gate requires them), so `embedded` covers it and the flag is moot there. ~keep
        embed_attempted: embed && !reused,
        reused,
        rehydration_ref: doc.rehydration_ref.clone(),
        degraded: doc.linear_chunked,
        text_fallback: doc.text_fallback,
    }
}

/// Turn a document's chunks into LanceDB rows. Hoists the per-row constant strings out of the map so
/// they allocate once instead of once per chunk (a doc can have hundreds of chunks).
///
/// Takes `doc` by value to move `chunk.text` and `chunk.embedding` directly into each row —
/// avoids cloning the chunk text (potentially large) and the embedding vector (`Vec<f32>`,
/// hundreds of kilobytes per chunk at typical preset dimensions).
fn build_doc_rows(doc: FileMapDoc, rel: &str, scope: &str) -> Vec<DocumentRow> {
    let scope_owned = scope.to_string();
    let rel_owned = rel.to_string();
    let mime_owned = doc.mime_type;
    let rehydration_ref = doc.rehydration_ref;
    doc.chunks
        .into_iter()
        .enumerate()
        .map(|(idx, chunk)| DocumentRow {
            scope: scope_owned.clone(),
            path: rel_owned.clone(),
            chunk_idx: u32::try_from(idx).unwrap_or(u32::MAX),
            mime_type: mime_owned.clone(),
            text: chunk.text,
            heading_path: chunk.heading_path,
            byte_start: chunk.byte_start,
            byte_end: chunk.byte_end,
            rehydration_ref: rehydration_ref.clone(),
            embedding: chunk.embedding,
        })
        .collect()
}

/// Purge the LanceDB `documents` rows AND the `index.doc_files` entries of docs that no longer
/// exist (or are no longer routed to the doc tier). Serial apply pass; mirrors
/// `scanner_code::delete_stale_code_chunks`. The tracking entry is dropped unconditionally (the doc
/// cache metadata must not leak for a removed file); the LanceDB delete is best-effort and never
/// *creates* the store. External-root docs were written under a per-root scope, so the delete uses
/// [`doc_scope_for`] to match it.
pub(crate) fn delete_stale_documents(store: &mut Store, config: &crate::config::Config, scope: &str, stale: &[String]) {
    if stale.is_empty() {
        return;
    }
    for path in stale {
        store.remove_doc(path);
        store.remove_doc_tombstone(path);
    }
    let Some(lance) = open_existing_doc_lance(store, config, "doc stale purge") else {
        return;
    };
    // Group by doc scope and delete in bulk: one commit per scope per 2000 paths, not two per path.
    let mut by_scope: ahash::AHashMap<String, Vec<String>> = ahash::AHashMap::new();
    for path in stale {
        by_scope
            .entry(doc_scope_for(path, scope, config).into_owned())
            .or_default()
            .push(path.clone());
    }
    for (doc_scope, paths) in &by_scope {
        if let Err(error) = lance.delete_documents_for_paths(doc_scope, paths) {
            tracing::warn!(
                paths = paths.len(),
                ?error,
                "doc stale purge failed; search_documents may return a removed path"
            );
        }
        // Drop the removed docs' document→code links too (ADR-0008), or graph_export/neighbors would
        // keep surfacing edges from docs that no longer exist.
        if let Err(error) = lance.delete_doc_links_for_paths(doc_scope, paths) {
            tracing::warn!(
                paths = paths.len(),
                ?error,
                "doc links stale purge failed; doc↔code edges may be stale"
            );
        }
        // Drop the removed docs' graph entities/edges (spec 0016).
        #[cfg(feature = "graphrag")]
        if let Err(error) = lance.delete_graph_for_scope(doc_scope) {
            tracing::warn!(
                paths = paths.len(),
                ?error,
                "graph entities/edges stale purge failed; graph may have stale nodes"
            );
        }
    }
}

/// Open the LanceDB store for a purge, or `None` when there is nothing to purge from: the store
/// was never built (a purge must not *create* it) or the preset / open failed (logged).
fn open_existing_doc_lance(
    store: &mut Store,
    config: &crate::config::Config,
    what: &str,
) -> Option<crate::lance::LanceStore> {
    try_open_existing_doc_lance(store, config, what).ok().flatten()
}

/// [`open_existing_doc_lance`] that tells "there is no vector store" (`Ok(None)`) apart from "there
/// is one and it could not be opened" (`Err`).
fn try_open_existing_doc_lance(
    store: &mut Store,
    config: &crate::config::Config,
    what: &str,
) -> Result<Option<crate::lance::LanceStore>, ()> {
    if store.lance.is_none() && !store.lance_dir_exists() {
        return Ok(None);
    }
    let model = &config.documents.embedding_preset;
    let dim = match preset_dim(model) {
        Ok(dim) => dim,
        Err(error) => {
            tracing::warn!(?error, preset = %model, "{what}: unknown preset; skipping lance delete");
            return Err(());
        }
    };
    match store.lance_or_open(dim, model) {
        Ok(lance) => Ok(Some(lance.clone())),
        Err(error) => {
            tracing::warn!(?error, "{what}: open LanceStore failed; skipping");
            Err(())
        }
    }
}

/// Delete the `documents` vector rows of every tracked doc that is no longer embed-eligible
/// (`[documents] embed = false`, or the path now falls outside `embed_include` / inside
/// `embed_exclude`). The docs stay indexed and keyword-searchable; only their vectors go. Run when
/// the embed policy changed since the last scan; see [`crate::scanner_policy`]. Returns whether the
/// purge fully applied (no vector store counts as applied).
pub(crate) fn purge_unembedded_documents(
    store: &mut Store,
    config: &crate::config::Config,
    filters: &Filters,
    scope: &str,
) -> bool {
    let cfg = &config.documents;
    let mut by_scope: ahash::AHashMap<String, Vec<String>> = ahash::AHashMap::new();
    for key in store.index.doc_files.keys() {
        let path = key.to_str_lossy();
        if cfg.embed && filters.doc_embed_allowed(&path) {
            continue;
        }
        let doc_scope = doc_scope_for(&path, scope, config).into_owned();
        by_scope.entry(doc_scope).or_default().push(path.into_owned());
    }
    if by_scope.is_empty() {
        return true;
    }
    let lance = match try_open_existing_doc_lance(store, config, "doc embed-policy purge") {
        Ok(Some(lance)) => lance,
        Ok(None) => return true,
        Err(()) => return false,
    };
    let mut applied = true;
    for (doc_scope, paths) in by_scope {
        if let Err(error) = lance.delete_documents_for_paths(&doc_scope, &paths) {
            tracing::warn!(
                paths = paths.len(),
                ?error,
                "doc embed-policy purge failed; search_documents may still return vectors for excluded paths"
            );
            applied = false;
        }
    }
    applied
}

/// Stream every pending document batch into LanceDB, one file at a time. Opens the store lazily — if
/// no batch will emit rows (no embeddings configured) no LanceDB connection is ever made.
///
/// For each row-emitting file this re-reads the file's already-persisted `.doc.msgpack` blob, builds
/// its [`DocumentRow`]s, writes them, and drops them before the next file — so peak embed-write RAM
/// is one document's rows plus one Arrow batch, independent of corpus size. (The blob is guaranteed
/// on disk here: every doc that produced a batch was written by `extract_and_persist_doc` or was a
/// cache hit, both before this post-barrier lane runs.)
///
/// Returns the number of files for which rows were written. Errors are logged and skipped on a
/// per-file basis so one malformed embedding doesn't abort the scan.
pub(crate) fn flush_document_batches(store: &mut Store, batches: Vec<PendingDocBatch>, embedding_model: &str) -> usize {
    // Rows are emitted for a lexical-only scan too: `embed = false` stores a null vector, it does not
    // suppress the row. The dimension comes from the configured preset, not from a batch — on a
    // lexical-only scan no batch carries one, and requiring one here is what used to make the whole
    // flush a no-op before a single row was built.
    let emit = batches.iter().any(|b| b.emit_rows);
    if !emit {
        return 0;
    }
    let dim = match preset_dim(embedding_model) {
        Ok(dim) => dim,
        Err(error) => {
            tracing::error!(
                ?error,
                preset = %embedding_model,
                "unknown embedding preset — refusing to write document batch"
            );
            return 0;
        }
    };
    // Every emitting batch that carries vectors must agree with the preset's dimension. A lexical-only
    // batch carries none, so the previous "find a batch with a dimension" lookup could not express
    // this: it both gated the whole flush and skipped the check whenever it found nothing.
    if let Some(actual) = batches
        .iter()
        .filter(|b| b.emit_rows && b.embedding_dim > 0)
        .map(|b| b.embedding_dim)
        .find(|actual| *actual != dim)
    {
        tracing::error!(
            preset = %embedding_model,
            expected = dim,
            actual,
            "preset/runtime dim mismatch — refusing to write document batch"
        );
        return 0;
    }
    let mut inserted = 0usize;

    let lance = match store.lance_or_open(dim, embedding_model) {
        Ok(s) => s.clone(),
        Err(error) => {
            tracing::error!(?error, "open LanceStore for document batch failed");
            return 0;
        }
    };

    // One delete+append commit pair per BATCH_MAX_DOCS documents instead of per document (#76);
    // rows are still built one document at a time, so peak RAM is one batch's rows.
    let mut pending: PendingBatch<DocumentRow> = PendingBatch::default();
    let flush = |pending: &mut PendingBatch<DocumentRow>, inserted: &mut usize| {
        let items = pending.take();
        let docs = items.len();
        match lance.replace_documents_many(items) {
            Ok(()) => *inserted += docs,
            Err(error) => {
                tracing::warn!(
                    docs,
                    ?error,
                    "lance replace_documents_many failed; document search may be incomplete"
                );
            }
        }
    };
    for batch in batches {
        if !batch.emit_rows {
            continue;
        }
        let doc = match store.read_doc_by_hex(&batch.blob_hash) {
            Ok(Some(doc)) => doc,
            Ok(None) => {
                tracing::warn!(rel = %batch.rel_path, "doc blob missing at flush; skipping vector rows");
                continue;
            }
            Err(error) => {
                tracing::warn!(rel = %batch.rel_path, ?error, "re-read doc blob failed; skipping vector rows");
                continue;
            }
        };
        let rows = build_doc_rows(doc, &batch.rel_path, &batch.doc_scope);
        if rows.is_empty() {
            continue;
        }
        pending.push(&batch.doc_scope, &batch.rel_path, rows);
        if pending.is_full() {
            flush(&mut pending, &mut inserted);
        }
    }
    if !pending.is_empty() {
        flush(&mut pending, &mut inserted);
    }
    inserted
}

/// Build the keyword index over the rows just written, then compact.
///
/// Called once per flush, after every batch. Per-file indexing would rebuild the index once per
/// document, which on a large corpus dwarfs the extraction cost.
///
/// Best-effort and warned, not fatal: a missing lexical index degrades the query path to
/// vector-only, whereas failing the scan would lose the embeddings that *were* just written. An
/// index that silently did not build is the failure worth watching for, so it is logged at warn.
pub(crate) fn ensure_document_fts_index(store: &mut Store, config: &crate::config::Config) {
    if !config.documents.enabled {
        return;
    }
    let model = config.documents.embedding_preset.clone();
    let dim = match preset_dim(&model) {
        Ok(dim) => dim,
        Err(error) => {
            tracing::warn!(?error, preset = %model, "documents FTS index skipped: unknown preset");
            return;
        }
    };
    let lance = match store.lance_or_open(dim, &model) {
        Ok(s) => s.clone(),
        Err(error) => {
            tracing::warn!(?error, "documents FTS index skipped: could not open the Lance store");
            return;
        }
    };
    if let Err(error) = crate::lance::fts::build_index_after_ingest(&lance, &config.documents.fts) {
        tracing::warn!(
            ?error,
            "documents FTS index build failed; keyword search will miss rows until the next scan"
        );
    }
}

/// Choose the LanceDB scope for a document. Repo files keep the scan-wide `default_scope`;
/// external-root files (absolute key, see [`crate::path::RelPath::is_external`]) are scoped
/// `path:<extra_root>` so they group under the out-of-repo tree they came from rather than the
/// repository's own doc scope.
///
/// `scope` is a retrieval partition, and every reader must honour it: `search_documents` filters on
/// it unconditionally (`src/lance/mod.rs`), so a row under `path:<root>` is only returned to a caller
/// that asks for that scope. An earlier version of this comment claimed "`search_documents` has no
/// scope filter — so this only partitions storage", which was false and had been for long enough to
/// justify the write/delete mismatch described on `PendingDocBatch::doc_scope`.
pub(crate) fn doc_scope_for<'a>(
    rel: &str,
    default_scope: &'a str,
    config: &crate::config::Config,
) -> std::borrow::Cow<'a, str> {
    if !crate::path::is_external_key(rel.as_bytes()) {
        return std::borrow::Cow::Borrowed(default_scope);
    }
    // Longest root wins, and the match is on a path-segment boundary: `/data/foo` must not claim
    // `/data/foobar/x`.
    let mut best: Option<String> = None;
    for raw_root in &config.scan.extra_roots {
        if let Ok(canonical) = raw_root.canonicalize()
            && let Some(prefix) = canonical.to_str()
        {
            #[cfg(windows)]
            let prefix = prefix.replace('\\', "/");
            #[cfg(windows)]
            let prefix = prefix.as_str();
            let prefix = prefix.trim_end_matches('/');
            let inside = rel == prefix || rel.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('/'));
            if inside && best.as_ref().is_none_or(|b| prefix.len() > b.len()) {
                best = Some(prefix.to_string());
            }
        }
    }
    if let Some(prefix) = best {
        return std::borrow::Cow::Owned(format!("path:{prefix}"));
    }
    std::borrow::Cow::Owned(format!("path:{rel}"))
}

#[cfg(test)]
#[path = "scanner_docs_security_tests.rs"]
mod security_tests;

#[cfg(test)]
#[path = "scanner_docs_tests.rs"]
mod tests;

//! Stage-specific validity checks for cached document artifacts.
//!
//! Kept separate from the document writer so the scanner's cache policy stays small and easy to
//! audit. The content predicate deliberately ignores embedding identity: changing a preset should
//! invalidate vectors, not force PDF/Office extraction and NER to run again.

#![cfg(feature = "documents")]

use crate::config::{DocumentModelProfile, DocumentsConfig, ResourcesConfig};
use crate::extract::doc::{DocChunk, FileMapDoc};
use crate::hashing::Hash;
use crate::scan_metrics::ScanStageMetrics;
use crate::scanner_filter::Filters;
use crate::store::Store;

use super::preset_dim;

/// True when a cached document's extracted content, chunks, and enrichment outputs can be reused
/// without re-extraction. Embedding identity is checked separately by
/// [`cached_embeddings_are_reusable`], allowing a preset change to re-embed the existing chunks
/// without repeating format extraction or NER. In every case the blob must have been produced
/// under the current `config_digest` (or predate digests, in which case the caller stamps it).
pub(crate) fn cached_doc_content_is_reusable(cached: &FileMapDoc, cfg: &DocumentsConfig, config_digest: &str) -> bool {
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
    cached.redaction_fingerprint == want_fingerprint
}

/// True when the cached chunks already carry vectors from the current embedding preset.
pub(crate) fn cached_embeddings_are_reusable(cached: &FileMapDoc, cfg: &DocumentsConfig) -> bool {
    if cached.chunks.is_empty() {
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

/// Compatibility predicate used by focused unit tests and older callers: all cached output,
/// including vectors when requested, must match the active configuration.
pub(crate) fn cached_doc_is_reusable(
    cached: &FileMapDoc,
    cfg: &DocumentsConfig,
    embed: bool,
    config_digest: &str,
) -> bool {
    cached_doc_content_is_reusable(cached, cfg, config_digest)
        && (!embed || cached_embeddings_are_reusable(cached, cfg))
}

/// Reconstruct the exact heading-aware input used by fresh extraction from persisted chunk fields.
pub(crate) fn cached_dense_input(chunk: &DocChunk) -> String {
    if chunk.heading_path.is_empty() {
        chunk.text.clone()
    } else {
        format!("{}\n\n{}", chunk.heading_path, chunk.text)
    }
}

/// Refresh only the embedding stage of a cached document, preserving extracted content and NER.
pub(crate) fn refresh_cached_embeddings(
    store: &Store,
    hash: &Hash,
    rel: &str,
    cached: &mut FileMapDoc,
    cfg: &DocumentsConfig,
    embed_requested: bool,
    resources: &ResourcesConfig,
    filters: &Filters,
    metrics: &ScanStageMetrics,
) -> anyhow::Result<()> {
    let embedding_allowed = embed_requested && !matches!(resources.document_models, DocumentModelProfile::None_);
    let under_chunk_cap = cached.chunks.len() <= cfg.max_chunks_per_document;
    if embedding_allowed && under_chunk_cap && !cached_embeddings_are_reusable(cached, cfg) && !cached.chunks.is_empty()
    {
        filters.doc_phase.admit().map_err(|spent| {
            anyhow::Error::new(crate::extract::ExtractError::DocDeferred(format!(
                "the {}s document-phase budget ([documents] phase_budget_secs) is spent",
                spent.elapsed.as_secs()
            )))
        })?;
        let started = std::time::Instant::now();
        let embedder = crate::embeddings::SharedEmbedder::load_with_provider(
            cfg.embedding_preset.as_str(),
            resources.effective_embed_threads(cfg.embed_max_threads),
            resources.embed_batch_size,
            resources.onnx_provider,
        )
        .map_err(|error| anyhow::anyhow!("load embedding model for cached document {rel}: {error}"))?;
        let inputs: Vec<String> = cached.chunks.iter().map(cached_dense_input).collect();
        let input_refs: Vec<&str> = inputs.iter().map(String::as_str).collect();
        let vectors = embedder
            .embed_batch(&input_refs)
            .map_err(|error| anyhow::anyhow!("embed cached document {rel}: {error}"))?;
        if vectors.len() != cached.chunks.len() {
            anyhow::bail!(
                "cached document embedding returned {} vectors for {} chunks",
                vectors.len(),
                cached.chunks.len()
            );
        }
        for (chunk, vector) in cached.chunks.iter_mut().zip(vectors) {
            chunk.embedding = vector;
        }
        cached.embedding_model = cfg.embedding_preset.clone();
        cached.embedding_dim = embedder.dim();
        store
            .write_doc(hash, cached)
            .map_err(|error| anyhow::anyhow!("write re-embedded doc blob for {rel}: {error}"))?;
        metrics.add_embedding(started.elapsed());
    }
    if !embedding_allowed && cached.embedding_dim > 0 {
        for chunk in &mut cached.chunks {
            chunk.embedding.clear();
        }
        cached.embedding_model.clear();
        cached.embedding_dim = 0;
        store
            .write_doc(hash, cached)
            .map_err(|error| anyhow::anyhow!("clear disabled document vectors for {rel}: {error}"))?;
    }
    Ok(())
}

//! BM25 posting construction for the code tier.
//!
//! Split out of [`super::bm25`] so the scoring half can move behind `intelligence` while this
//! half stays behind `code-search`: building postings needs [`crate::chunk::CodeChunk`], which
//! only exists for the code substrate, and the documents tier's lexical lane does not go through
//! here at all — it is LanceDB FTS over `cites` / `text`, not BM25 over Fjall postings.

use super::bm25::{ChunkPosting, tokenize_counts};
use crate::chunk::CodeChunk;

/// Build the BM25 postings for a file's chunks. `doclen` is the total token count (with repetition);
/// `terms` are the distinct `(term, tf)` pairs. Called from the scanner's parallel per-file worker.
///
/// Tokenization stays [`super::bm25`]'s, so the index side and the query side cannot drift — that
/// agreement is what `bm25_idf` and the fusion weights assume.
pub fn build_chunk_postings(chunks: &[CodeChunk]) -> Vec<ChunkPosting> {
    chunks
        .iter()
        .map(|c| {
            let counts = tokenize_counts(&c.searchable_text);
            // `doclen` is the sum of the counts, not `terms.len()`: a token appearing three times
            // contributes three to the length, which is what BM25's length normalisation assumes.
            let doclen: u32 = counts.values().copied().sum();
            let terms: Vec<(String, u32)> = counts.into_iter().collect();
            ChunkPosting {
                chunk_id: c.chunk_id.clone(),
                doclen,
                terms,
            }
        })
        .collect()
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(chunk_id: &str, searchable_text: &str) -> CodeChunk {
        CodeChunk {
            chunk_id: chunk_id.to_string(),
            path: "src/lib.rs".to_string(),
            lang: "rust".to_string(),
            kind: None,
            symbol: None,
            signature: None,
            doc: None,
            byte_start: 0,
            byte_end: 0,
            line_start: 1,
            line_end: 1,
            text: searchable_text.to_string(),
            searchable_text: searchable_text.to_string(),
        }
    }

    /// `doclen` is the sum of the term frequencies, not `terms.len()`. A token appearing three times
    /// contributes three to the length, which is what BM25's length normalisation assumes — and the
    /// mistake is invisible in a fixture where no term repeats.
    #[test]
    fn build_postings_reports_doclen_as_total_token_count() {
        let postings = build_chunk_postings(&[chunk("h:0", "alpha beta alpha")]);
        assert_eq!(postings.len(), 1);
        assert_eq!(postings[0].chunk_id, "h:0");
        assert_eq!(postings[0].doclen, 3, "three tokens total incl. the repeat");
        let alpha = postings[0].terms.iter().find(|(t, _)| t == "alpha").unwrap();
        assert_eq!(alpha.1, 2, "alpha appears twice");
    }
}

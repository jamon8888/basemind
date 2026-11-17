//! Ranked retrieval: the BM25 keyword lane, the exact-symbol lane, and RRF fusion.
//!
//! [`bm25`] is the native Okapi BM25 scoring half — tokenization, idf, term scoring, and the query
//! over the Fjall postings. It sits behind `intelligence` rather than `code-search`, because
//! [`rrf`] needs it and the documents tier will fuse its own lanes through the same code.
//!
//! [`bm25_postings`] is the other half: building postings needs the code substrate's `CodeChunk`,
//! so it stays behind `code-search`. Tokenization lives in [`bm25`] and both halves call it, so the
//! index side and the query side cannot drift.
//!
//! [`rrf`] blends the vector, keyword and exact lanes by rank. It reads ranks only — no score from
//! any lane enters the fusion, which is what lets a lane whose scores live on a different scale
//! (BM25, cosine distance) be weighted against a vector lane at all.

#[cfg(feature = "intelligence")]
pub mod bm25;
#[cfg(feature = "code-search")]
pub mod bm25_postings;
#[cfg(feature = "code-search")]
pub mod exact;
#[cfg(feature = "intelligence")]
pub mod rrf;

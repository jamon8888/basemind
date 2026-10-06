# ADR-0012: Multi-lane document retrieval — multi-lane, embeddings optional

- **Status:** Accepted
- **Date:** 2026-10-06
- **Accepted:** 2026-10-06, via wayfinder ticket jamon8888/basemind#36
- **Deciders:** jamon8888 (proposer and decider)
- **Related:** ADR-0008 (documents ↔ code graph), ADR-0011 (MCP tool surface)

## Context

The documents tier retrieves with a single lane: vector KNN. `LanceStore::search_documents`
builds `table.vector_search(query)` behind a `scope`/`mime_type` predicate
(`src/lance/mod.rs:315`). Every document and memory query embeds the query string first
(`src/mcp/memory.rs:379`), so retrieval quality is bounded by the embedding model, and the
store cannot exist at all in an embedding-free deployment:

- `documents_schema(dim)` declares `embedding` as non-nullable (`src/lance/schema.rs:22-38`), so
  a lexical-only documents table cannot be created.
- With `[documents] embed = false` no vector rows are written, and the only query path still
  demands a query vector. `website/src/content/docs/capabilities/document-search.mdx:162` and
  `website/src/content/docs/reference/configuration.mdx:157` both promise "keyword search" in that
  mode. No such lane exists — the promise is unimplemented.
- No FTS index is ever created. `grep` finds no `Index::FTS` / `create_index` call for the
  documents table. LanceDB 0.37.1 (pinned at `Cargo.toml:170`; 0.38.0 does not build without its
  `remote` feature) exposes the machinery in Rust: `Index::FTS(FtsIndexBuilder)`,
  `Query::full_text_search(FullTextSearchQuery)`, `.rerank(Arc<dyn Reranker>)` with
  `lancedb::rerankers::rrf::RRFReranker`, and `lancedb::tokenize`.

The code tier already solved this shape and its answer is not reused. `src/search/bm25.rs` runs a
native Okapi BM25 lane (`k1 = 1.2`, `b = 0.75`) over chunks in Fjall keyspaces
`code_bm25_postings` / `code_bm25_by_path`; `src/search/rrf.rs` fuses BM25 + vector + exact-symbol
with `DEFAULT_RRF_K = 60` and weights exact 2.0 / vector 1.0 / keyword 1.0; hits carry
`matched_lanes` / `lane_ranks` provenance. Both modules are `#[cfg(feature = "code-search")]`, so
the documents tier cannot reach them.

Two further facts shape the decision:

1. **Legal work is identifier-driven.** Practitioners search docket numbers, statute cites,
   party names, and quoted case names — high-precision lexical lookups. Published benchmarks agree
   lexical retrieval is not a fallback for this domain: on CUAD, BM25 reaches nDCG@10 0.245 vs
   0.133 for a dense bi-encoder (BGE-small), and equal-weight RRF over both lands at 0.230 —
   *below* BM25 alone, because RRF gives a weak lane equal say. These figures are quoted from
   third-party sources and have **not** been reproduced in this repository; the weights in this ADR
   are a starting point to be measured, not a result.
2. **Reranking needs no corpus embeddings.** The cross-encoder rerank already runs at query time
   over hits only (`src/mcp/memory.rs:554-578`, preset `bge-reranker-v2-m3`, `top_k = 20`,
   `enabled = false` by default). It is query×chunk ONNX inference, so it is fully available to an
   embedding-free deployment and is the cheapest quality lever in the system.

Forces: multi-tenant per-agent scope isolation; module-size cap; schema/blob compatibility
(`.ai-rulez/rules/schema-and-blob-compat.md`); offline-first (no hosted rerankers); and the fact
that adding config keys invalidates `schema/basemind-config-v1.schema.json`, whose snapshot test
(`tests/config_schema.rs`) is already `#[ignore]`d with a "out of sync" note.

## Decision

Document retrieval becomes a **multi-lane** design in which embeddings are
optional rather than load-bearing.

1. **Add a native lexical lane to the documents tier**, mirroring the code tier: BM25 over chunk
   text stored in LanceDB via an FTS index, not a parallel store. We build the index rather than
   depending on an implicit "hybrid" default — hybrid in LanceDB means *both* a vector index and an
   FTS index; it is not a fallback when vectors are absent.
2. **Add a citation / exact-identifier lane** over a dedicated normalized `cites` column with an
   n-gram tokenizer, so `362`, `§ 362(a)(1)`, `2-24-1234`, and `17-1234` are all reachable.
   Booleans and `OR` are unavailable in the FTS query string, so quoted-phrase support is a
   separate decision (an index with `with_position = true` and `remove_stop_words = false`), not an
   assumption.

   **Precondition — redaction.** This lane presupposes that the identifiers it indexes are
   retrievable in clear text. Under `redaction.strategy = "token_replace"` — the default strategy —
   the stored `text` column is pseudonymised (`[PERSON_1]`), reversible only through the encrypted
   rehydration map keyed by `rehydration_ref` (`src/extract/doc.rs:490`). A keyword lane over
   pseudonymised text cannot match `Smith v. Acme`. Making the exact lane work therefore requires
   indexing `cites` from pre-redaction text, which stores clear-text identifiers on disk beside
   pseudonymised content. That is a confidentiality trade-off, not a retrieval detail, and it is
   **not settled by this ADR**: the lexical lanes assume redaction is off, and the `token_replace`
   case is owned by #7 (query masking at the inference boundary) and #12. Re-dating this decision
   requires resolving the `cites` index question first.
3. **Add scalar facet lanes** — `section`, `doc_type`, `jurisdiction`, `court`, `date` — as
   prefilterable columns. Section rows follow the facets/issues/decision/reasoning decomposition,
   which per-section weighting with score normalization beats plain union RRF on legal retrieval.
4. **Keep the vector lane, demoted.** Its weight drops below keyword. Embeddings become a
   retrieval *option*; a store without them must be valid, so `embedding` becomes nullable and
   `documents_schema` accepts an optional dimension.
5. **Fuse with reciprocal rank fusion using in-repo RRF.** Move `src/search/rrf.rs` (and the
   scoring parts of `bm25.rs`) out of the `code-search` feature gate so both tiers share one
   implementation, one set of lane weights, and one provenance shape. Keep `matched_lanes` on every
   hit so a caller can see *why* a passage surfaced — a requirement when the answer carries a
   citation.
6. **Rerank stays the last stage** and is enabled by default for the documents tier. It needs no
   corpus vectors, so it is the quality floor for embedding-free deployments.
7. **Tokenizer configuration is a first-class surface.** BM25 for legal prose fails without
   citation-aware tokenization and boilerplate stop-word removal; the current code-tier tokenizer
   (lowercase, split on non-alphanumeric, no stemming, no stop-words — `src/search/bm25.rs:55`)
   shreds `§ 362(a)(1)` and `C.F.R.` and lets "pursuant"/"herein" dominate document length
   normalization.

## Consequences

Easier: document search works in an embedding-free deployment; identifier lookups get a
first-class lane; the two tiers share fusion code instead of drifting; reranking gives precision
without any vector infrastructure; per-hit lane provenance is auditable.

Harder: two index kinds to build and keep fresh (`optimize()` cadence becomes part of ingest);
`documents_schema` changes, so existing document tables need a version bump and a rescan — the
same wipe semantics already applied to `embedding_preset`; `embedding` becoming nullable is a
schema change with `dim`-parameterized call sites to audit; RRF weights become a tuning surface
that needs an evaluation harness to justify, not vibes; the ignored schema snapshot test must be
regenerated and re-enabled.

## Alternatives considered

- **Rely on LanceDB hybrid search as configured today.** Rejected: hybrid is the union of a vector
  lane and an FTS lane. With no FTS index and no vectors there is no retrieval at all, so it does
  not answer the embedding-free case.
- **Point the document tier at the existing Fjall BM25 keyspaces.** Rejected: those postings are
  built from code chunks (`build_chunk_postings` takes `&[CodeChunk]`), are content-addressed per
   source file, and carry `max_chunks_per_file` caps tuned for source files. Documents need
   chunk-level FTS with facets, inside LanceDB, next to the rows results are served from.
- **Add a third-party sparse/lexical engine (SPLADE-style) beside LanceDB.** Rejected for now: it
   reintroduces a corpus-side model download and a second store to keep in sync, which is exactly
   the cost this decision removes. `FtsIndexBuilder` covers the requirement.
- **Drop embeddings from documents entirely.** Rejected: the semantic lane earns its place on
  paraphrase queries ("can they break the lease" vs "termination for convenience"). Demote and
   weight it, do not delete it.
- **Correct the documentation and stop.** Rejected: the doc/behavior gap is real, but it is a
  symptom. A correction without a lane leaves the product weaker than advertised.

Implementation detail, phases, tests, and acceptance criteria live in
[`../specs/0012-lexical-document-retrieval.md`](../specs/0012-lexical-document-retrieval.md).

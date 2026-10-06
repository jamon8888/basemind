# Spec 0012: Multi-lane document retrieval (multi-lane, embeddings optional)

- **Status:** Draft — implements [ADR-0012](../adr/0012-multi-lane-document-retrieval.md)
- **Date:** 2026-10-06
- **Scope:** `documents` and `memory`-documents retrieval paths, the documents LanceDB table, the
  `[documents]` config tree, and the shared BM25/RRF modules.
- **Feature gates:** `documents` (required), `intelligence` (LanceDB), `code-search` (today the
  only gate on the shared fusion code — this spec removes that coupling).

---

## 1. Problem

Retrieval quality for documents is bounded by a single embedding model, and an embedding-free
deployment cannot retrieve documents at all. Concretely, three defects:

1. **No lexical lane.** `search_documents` is `vector_search` only (`src/lance/mod.rs:315`).
2. **Embedding is mandatory in practice.** `documents_schema` makes `embedding` non-nullable
   (`src/lance/schema.rs:22-38`) and every query embeds first (`src/mcp/memory.rs:379`), so
   `[documents] embed = false` yields no retrieval. The published docs claim otherwise
   (`website/src/content/docs/capabilities/document-search.mdx:162`,
   `website/src/content/docs/reference/configuration.mdx:157`).
3. **Facets are post-filtered in Rust.** `entity_category` / `keywords_contains` filter hits after
   retrieval (`src/mcp/memory.rs:638-710`) because keywords, entities, and summary live in the Fjall
   sidecar, not in the LanceDB row.

Why this matters for a legal-facing product: the dominant query is an *identifier* — docket
number, statute cite, party, quoted case name — not a topic. Benchmarks on legal corpora show
BM25 ahead of dense retrieval (CUAD nDCG@10: BM25 0.245, dense bi-encoder 0.133), and naive
equal-weight RRF (0.230) landing *below* BM25 alone because a weak lane gets equal say. Fusion
weights are therefore a correctness concern, not a tuning nicety.

## 2. Current state, mapped

| Concern | Location | Behaviour |
|---|---|---|
| Document KNN | `src/lance/mod.rs:315` `search_documents` | `table.vector_search(query)` + `only_if(scope, mime_type)` |
| Document schema | `src/lance/schema.rs:22-38` | `scope, path, chunk_idx, mime_type, text, byte_start, byte_end, rehydration_ref, embedding`; `embedding` non-nullable; `dim` required |
| Query embedding | `src/mcp/memory.rs:27,379` | ONNX embed per query; `memory search` and `memory documents` both |
| Code BM25 lane | `src/search/bm25.rs` | Okapi `k1 = 1.2`, `b = 0.75`, `MAX_TERM_LEN = 80`, lowercase + split on non-alphanumeric, no stop-words, no stemming |
| Fusion | `src/search/rrf.rs` | `DEFAULT_RRF_K = 60`; weights exact 2.0 / vector 1.0 / keyword 1.0; `matched_lanes`, `lane_ranks` |
| Feature gates | `src/search/mod.rs:7-13` | `bm25`, `exact`, `rrf` all `#[cfg(feature = "code-search")]` |
| Chunking | `src/config/documents.rs`, `src/extract/doc.rs:280` | `max_characters = 800`, `overlap = 100`, `chunker_type = Markdown` |
| Rerank | `src/mcp/memory.rs:498-580`, `src/config/enrichment.rs:13` | query-time cross-encoder, preset `bge-reranker-v2-m3`, `top_k = 20`, `enabled = false` |
| Keywords | `src/config/enrichment.rs:102` | `enabled = false`, `max_keywords = 10`, `min_score = 0.0`, `ngram_range = [1, 3]` |
| NER | `src/config/documents.rs` (`NerConfig`) | `enabled = false`, `custom_labels` supported (GLiNER zero-shot) |
| MCP hit shape | `src/mcp/types_documents.rs:59` | `path, chunk_idx, text, mime_type, byte_start, byte_end, distance, rerank_score, keywords, entities, summary` |
| Config snapshot | `schema/basemind-config-v1.schema.json`, `tests/config_schema.rs` | snapshot test is `#[ignore]`d with a "out of sync" note |

## 3. Goals

- **G1** Document retrieval works with embeddings disabled (lexical + facets + rerank) and is
  strictly better than today with them enabled.
- **G2** Identifier lookups (docket, cite, party, quoted case name) resolve in one query.
- **G3** Every hit states which lane(s) surfaced it.
- **G4** One fusion implementation shared by the code and document tiers.
- **G5** Config knobs for chunking, tokenizer, and lanes, all documented and schema-regenerable.
- **G6** Documented behaviour matches implemented behaviour.

## 4. Non-goals

- Replacing LanceDB or introducing a second retrieval store.
- Learned sparse retrieval (SPLADE) — revisit only if `FtsIndexBuilder` measurably underperforms.
- Replacing the code tier's hand-rolled BM25 with LanceDB FTS (out of scope; different row model).
- Provider-hosted rerankers (offline-first).
- Any change to the MCP tool *names*; only wire fields are added.

## 5. Target architecture

Four lanes over one table, fused by RRF, then reranked.

| Lane | Source | Signal | RRF weight |
|---|---|---|---|
| `exact` | `cites` column, n-gram FTS index | normalized identifier substring match | 3.0 |
| `keyword` | `text` + `heading_path` FTS index (BM25) | topical lexical match | 2.0 |
| `facet` | scalar columns, prefiltered not scored | doc type / jurisdiction / date / section | 1.0 (rank of surviving set) |
| `vector` | `embedding`, IVF_FLAT/PQ | paraphrase similarity | 1.0 |

`k = 60` unchanged. Weights are the ADR decision: keyword above vector because legal lexical
retrieval measurably outperforms dense, and exact above both because an identifier match is a
high-precision signal (the same reasoning the code tier already encodes at weight 2.0 for its exact
lane). Weights become configurable (`[documents.fusion]`) only after the evaluation harness in §13
exists — otherwise they are constants with a comment pointing at the benchmark.

Dataflow:

```
query
 ├─ normalize (case-fold, citation split, stop-word trim, quoted-phrase extraction)
 ├─ lane: exact   → FTS(ngram) over cites          ─┐
 ├─ lane: keyword → FTS(stem, stopwords) over text ─┤→ RRF(k=60, weights §5)
 ├─ lane: facet   → prefilter on scalar columns    ─┤
 ├─ lane: vector  → vector_search (if rows exist)  ─┘
 ├─ per-document cap (≤3 chunks/doc), byte-span overlap dedupe
 ├─ rerank top `reranker.top_k` (cross-encoder, no corpus vectors)
 └─ emit hits with `matched_lanes` + `lane_ranks` + `citation`/`byte_span`
```

## 6. Schema changes (`documents_v2`)

New table name `documents_v2`; the v1 table is left on disk and dropped by the existing GC
(`src/store_gc*.rs`) once v2 is healthy. `documents_schema(dim: Option<u16>)`:

| Column | Type | Null | Notes |
|---|---|---|---|
| `scope` | Utf8 | no | unchanged — **retrieval partition, not a confidentiality boundary**. It is derived from the repository (`MemoryScopeStrategy`, `src/config/v1.rs:360-368`), so it cannot separate two repositories. The confidentiality question is #7's, not this spec's (#40) |
| `path` | Utf8 | no | unchanged |
| `chunk_idx` | UInt32 | no | unchanged |
| `mime_type` | Utf8 | no | unchanged |
| `doc_type` | Utf8 | no | `pleading`, `contract`, `memo`, `email`, `statute`, `exhibit`, `web`, `unknown` |
| `section` | Utf8 | no | `caption`, `facts`, `issues`, `argument`, `holding`, `reasoning`, `signature`, `body` |
| `heading_path` | Utf8 | no | Markdown heading breadcrumb, e.g. `IV. TERMINATION > 4. Convenience` |
| `cites` | List<Utf8> | no | normalized identifiers: statute cites, docket numbers, case short names. **Self-references excluded** — the document's own caption and its own docket number are not citations of authority and are never indexed in clear (see ADR-0012 precondition, ticket #44) |
| `jurisdiction` | Utf8 | yes | ISO-ish code when detectable |
| `doc_date` | Utf8 | yes | ISO-8601 `YYYY-MM-DD`; lexically sortable, avoids date-type filter cost |
| `text` | Utf8 | no | chunk text (snippet returned to caller) |
| `byte_start` / `byte_end` | UInt32 | no | unchanged — citation anchors |
| `keywords` | List<Utf8> | yes | lifted out of the Fjall sidecar so it can filter in-store |
| `entities` | List<Utf8> | yes | `category:value` pairs, e.g. `PERSON:Acme Corp` |
| `summary` | Utf8 | yes | lifted from sidecar |
| `rehydration_ref` | Utf8 | yes | unchanged |
| `embedding` | FixedSizeList\<Float32, DIM\> | **yes** | nullable; `None` on a lexical-only store |

Indexes: FTS on `text` and `heading_path`; n-gram FTS on `cites`; scalar indexes on `scope`,
`path`, `mime_type`, `doc_type`, `section`, `jurisdiction`, `doc_date`.

Rationale for lifting `keywords` / `entities` / `summary` into the row: today they can only be
post-filtered in Rust (`src/mcp/memory.rs:638-710`), which silently shrinks the candidate set
*after* top-k. In-row they become prefilterable, and `summary` becomes a boostable lexical field.

## 7. Index build

### 7.1 Tokenizers

Two properties of the builder shape every call below, and both are easy to get wrong.

The builder is `lancedb::index::scalar::FtsIndexBuilder`, an alias of `InvertedIndexParams`
(`lance-index-10.0.0/src/scalar/inverted/tokenizer.rs:49`). **Its methods carry no `with_` prefix** —
`with_position` is the sole exception among its 30 public methods. The rest are `base_tokenizer`,
`language`, `stem`, `remove_stop_words`, `ascii_folding`, `max_token_length`, `custom_stop_words`,
`ngram_min_length`, `ngram_max_length`. And **`language` returns `Result<Self>`, not `Self`**
(`tokenizer.rs:675`), so it breaks a fluent chain: assembly resumes after a `?`, and the
`Result` has to be handled wherever the builder is built.

Primary lexical index, over `text` and `heading_path`:

```rust
let fts = FtsIndexBuilder::default()
    .base_tokenizer("simple".to_string())   // explicit: do not rely on a default
    .language("English")?                  // drives stem + stop-word behaviour
    .stem(true)
    .remove_stop_words(true)
    .ascii_folding(true)                    // FR/ES matters: "société" → "societe"
    .max_token_length(Some(64))             // default 40 truncates long Spanish/French tokens
    .custom_stop_words(Some(vec![
        "pursuant".into(), "herein".into(), "hereto".into(), "aforesaid".into(),
        "wherein".into(), "notwithstanding".into(), "provided that".into(),
        "hereinafter".into(), "thereunder".into(),
    ]));
```

Citation index, on `cites` only:

```rust
let cites = FtsIndexBuilder::default()
    .base_tokenizer("ngram".to_string())
    .ngram_min_length(3)
    .ngram_max_length(3)
    .remove_stop_words(false)
    .stem(false);
```

Why: n-gram is what makes `362` reach `§ 362(a)(1)` and `2-24-1234` reach `No. 2-24-1234`. Keep it
off `text` — trigram indexing of full prose is large and noisy.

`base_tokenizer` is deliberately not a configuration key. The two indexes need different tokenizers,
so no single setting could govern both: which tokenizer an index gets is the content of this
section, not a setting (§10).

### 7.2 Phrase queries — a decision, not an assumption

Lance-native FTS accepts a phrase query **only** if the index was built with `with_position = true`
**and** `remove_stop_words = false`. Quoted case names (`"Smith v. Acme"`) are common in legal
queries, so:

- Phase 3 builds a **third** index, `text_phrase`, with `with_position = true` and
  `remove_stop_words = false`, over a position-preserving copy of the chunk text.
- **That index must be word-tokenized, never n-gram.** The documentation of `with_position` states
  outright that it "doesn't work with `ngram` tokenizer" (`tokenizer.rs:685`), so `text_phrase`
  cannot reuse the citation index's tokenizer. Stated explicitly because the obvious
  implementation — take the `cites` builder from §7.1 and add `with_position(true)` — compiles and
  produces a silently empty phrase index.
- The query path routes a quoted span to `text_phrase` and merges its hits into the `exact` lane.
- If index size on the target corpus is unacceptable, the build skips the phrase index and the MCP
  layer reports `phrase_unsupported` rather than silently ignoring quotes.

### 7.3 Build cadence

- Build indexes after the document pass completes, not per file.
- Call `table.optimize(OptimizeAction::All)` after every ingest batch and on a cadence for
  continuously-written stores (rule of thumb from LanceDB docs: ~100k row changes or 20
  modification ops). Unindexed rows fall back to a flat scan, so a missed `optimize()` shows up as
  a latency cliff, not a correctness bug.
- Track `index_stats().num_unindexed_rows`; warn above a build-cadence threshold, 50k unindexed rows
  by default. Cadence and thresholds are build settings, not tokenizer parameters, so they are not
  part of `[documents.fts]` (§10).
- Never `fast_search()`: for legal work a silently missing filing is a wrong answer.

## 8. Query path

1. **Normalize.** Case-fold; extract quoted spans; split citation-shaped tokens (§, U.S.C., C.F.R.,
   `No.`, `v.`, `§`, `¶`); trim legal boilerplate stop-words; keep every numeric/docket token.
   Normalization is shared with ingest so index and query tokenize identically — `lancedb::tokenize`
   is used to assert that in a test (§13), not to hand-stem queries.
2. **Facet prefilter.** Build `only_if` from `scope` (mandatory) plus any of
   `mime_type` / `doc_type` / `jurisdiction` / `section` / date range. Prefilter, not postfilter:
   postfilter can return fewer than `limit` rows when top-k is mostly filtered out.

   **The scope predicate applies to every lane, not only the vector one.** Steps 3, 4 and 5 are FTS
   queries and must carry the same predicate; Lance supports a filter on a full-text query. A lane
   that searched outside `scope` would leak across matters with no observable symptom, so this is a
   correctness requirement, not a style note (#40).

   **On this lane**, `scope` resolves to the repository's own scope or one of its own `web:<host>`
   siblings, never to an arbitrary caller-named string: `resolve_doc_scope`
   (`src/mcp/memory.rs:486-488`) returns the requested value verbatim today, which is what makes
   scraped pages reachable but also what lets a caller name another repository's scope. Constraining
   it to the repository and its `web:*` siblings keeps the behaviour that fix was made for and
   removes the arbitrary read (#40).

   **That constraint is on `resolve_doc_scope`, not on `scope` in general.** It states which
   caller-named scopes the documents lane will honour; it is not a rule about how a caller may narrow
   retrieval. The two are different code paths today. `resolve_doc_scope` has exactly one production
   caller — `run_search_documents` (`src/mcp/memory.rs:522`) — and `documents` is the only tier that
   accepts a caller-supplied scope (`src/mcp/types_documents.rs:28-34`). The code lane accepts none:
   `CodeParams` (`src/mcp/types_code.rs:32-128`) has no `scope` field, and its semantic lane reads
   the daemon-wide `state.shared.scope` (`src/mcp/helpers_code_search.rs:347`), computed once per
   server. A caller-side scope selector there would change the daemon's shape rather than refine a
   predicate, so it is out of scope for this spec; whether to add one is the integration's decision
   (#53, and hacienda-cowork #15).
3. **Exact lane.** `full_text_search` over the `cites` n-gram index (and `text_phrase` for quoted
   spans). No fuzziness here — typo tolerance corrupts statute numbers. Allow fuzziness 1 only on a
   party-name lane if one is added later.
4. **Keyword lane.** `full_text_search` over `text` + `heading_path`.
5. **Conjunctive-then-disjunctive fallback.** Lance FTS has no `AND`/`OR` in the query string. Run
   all terms; while `hits < limit` and terms remain, drop the lowest-idf term and re-run; union the
   results (dedupe by `(scope, path, chunk_idx)`). This is the standard high-recall BM25 pattern and
   is what rescues multi-clause legal questions. Cap at 3 relaxations to bound latency.
6. **Vector lane.** Only when `embedding` is non-null for the scope and `[documents] embed = true`.
   Skip cleanly otherwise — never embed the query in a lexical-only store.
7. **Fuse.** `rrf_fuse_detailed` with the weights in §5. Retain per-lane ranks.
8. **Post-fusion shaping.** Per-document cap (default 3 chunks, `[documents].max_hits_per_document`);
   drop hits whose `byte_span` overlaps an already-selected hit by more than 50%; prefer hits whose
   `section` matches the query's detected intent (`holding` for "what did the court rule").
9. **Rerank and emit.** Rerank the top `[documents.reranker].top_k` with the cross-encoder, then
   trim to `limit`. Emit `matched_lanes`, `lane_ranks`, `rerank_score`, `citation` string
   (`path#byte_start-byte_end`), plus a `retrieval_mode` field (`lexical` / `hybrid` / `vector`)
   so callers and tests can tell which lanes ran.

Pagination: existing `next_cursor` semantics carry `(lane ranks, last row id)`; cursors must be
invalidated by any index rebuild (see `with_row_id`).

## 9. Extraction and chunking defaults

| Key | Current | Proposed | Why |
|---|---|---|---|
| `max_characters` | 800 | 1800 | BM25 length normalization punishes small chunks; 800 chars splits statutes mid-subsection. Claim-level legal RAG reports recall rising with chunk size (600 tokens / 120 overlap). |
| `overlap` | 100 | 220 | ~12% — keeps §/subsection boundaries whole without inflating index size. |
| `chunker_type` | `Markdown` | `Markdown` (unchanged) | heading structure survives; feeds `heading_path`. |
| `max_chunks_per_document` | 2000 | 2000 | fine; guard only. |
| `max_pages` | 500 | 500 | fine; scanned bundles may need more. |
| `extraction_timeout_secs` | 600 | 900 | OCR-bound extraction of large bundles. |
| `reranker.enabled` | `false` | `true` | works without corpus vectors; largest precision win available. |
| `reranker.top_k` | 20 | 40 | over-fetch before rerank. |
| `keywords.enabled` | `false` | `false` | doc-level yaKe/raKe keywords are a *filter*, not a BM25 field; precision collapses if concatenated into `text`. |
| `ner.enabled` | `false` | `true` (opt-in per deployment) | populates `entities`; `custom_labels` for `CaseName`, `DocketNumber`, `Citation`, `StatutorySection`, `Court`, `Judge`, `PartyName`. |
| `summarization.enabled` | `false` | `true`, `extractive` | boosts recall when fused as its own field (party names in the summary are matched explicitly by BM25 in published legal work). No LLM, no tokens. |
| OCR | off unless configured | on for scans | OCR quality caps lexical quality; no BM25 tuning recovers a bad extraction. |

```toml
[documents]
max_characters = 1800
overlap = 220
extract_archives = false          # keep off: one archive can explode chunk + index growth

[documents.reranker]
enabled = true
top_k = 40

[documents.summarization]
enabled = true
strategy = "extractive"

[documents.ner]
enabled = true
custom_labels = ["CaseName", "DocketNumber", "Citation", "StatutorySection", "Court", "Judge", "PartyName"]
```

## 10. Configuration surface (new keys)

```toml
[documents.fusion]
k = 60.0
weight_exact = 3.0
weight_keyword = 2.0
weight_facet = 1.0
weight_vector = 1.0
per_document_cap = 3
max_relaxations = 3

[documents.fts]
ngram_min_length = 3
ngram_max_length = 3
stem = true
remove_stop_words = true
ascii_folding = true
max_token_length = 64
stemmer_language = "English"
custom_stop_words = ["pursuant", "herein", ...]

[documents.citations]
extract = true
max_per_chunk = 64
```

Every key gets `#[serde(default)]` so older TOML files keep loading, per the module convention in
`src/config/documents.rs`. Adding these invalidates `schema/basemind-config-v1.schema.json`;
regenerate with `cargo test --features full --test config_schema -- --ignored regenerate_schema`
and re-enable `schema_snapshot_matches_derived` (currently `#[ignore]`d — fix that in the same
change, it is the guard that makes config drift visible).

`[documents.fts]` holds only what maps to a real `InvertedIndexParams` field. Three things were
deliberately kept out of it:

- **`stemmer_language`, not `language`.** `[documents].language` already exists and is a
  *detection* table (`auto_detect`, `min_confidence`, `detect_multiple`, `preferred_languages`).
  Two different `language` keys of different shapes at different depths is a trap; the stemmer's
  language is named for what it does. Note that the detection table is largely inert today —
  `preferred_languages` is documented as reserved, and the xberg release in use does not honour a
  preferred-language hint.
- **No `enabled`.** `[documents].enabled` is already the master switch for the tier.
- **No cadence keys.** `optimize_every_rows` and `unindexed_rows_warn_threshold` are build-cadence
  settings, not tokenizer parameters; they belong to §7.3, which now carries their defaults.

Module-size cap (`.ai-rulez/rules/module-size-cap.md`): `src/config/documents.rs` is already
near its limit, so `[documents.fusion]` / `[documents.fts]` / `[documents.citations]` go in a new
`src/config/documents/retrieval.rs` alongside the existing `pii_patterns.rs`.

## 11. Delivery phases

Each phase lands independently with its own tests and is safe to stop after.

**Phase 0 — honesty and unblocking.** Make `documents_schema` accept `Option<u16>` with a nullable
`embedding` column so a lexical-only store can exist. Correct the two website claims that promise
keyword search under `embed = false` (`document-search.mdx:162`, `configuration.mdx:157`) to
describe actual behaviour until Phase 2 ships. Regenerate and re-enable
`tests/config_schema.rs::schema_snapshot_matches_derived`. Audit `src/store_gc*.rs` for
`documents_v2` handling.
*Exit:* a build with `[documents] embed = false` creates a valid, empty lexical-only table, and no
public doc claims a lane that does not exist.

**Phase 1 — shared fusion.** Move `rrf.rs` + BM25 scoring behind `intelligence`, add `Lane`,
`FusionWeights`, `LaneProvenance`; keep code-tier weights as defaults so code search ranking is
unchanged. New units: lane-weight defaults, empty-lane behaviour, tie-break stability.
*Exit:* existing code-search smoke tests pass unchanged; a unit test fuses four lanes and asserts
the documented weights.

**Phase 2 — lexical lane.** FTS index on `text` + `heading_path` after the document pass,
`optimize()` cadence, `search_documents_lexical` with conjunctive-then-disjunctive fallback and
§10 config keys. Correct the website docs to describe the lane that now exists. New smoke:
`tests/document_fts_smoke.rs` (lexical-only query in an empty-`BASEMIND_DATA_HOME` store — proves
no embedder is constructed).
*Exit:* a lexical-only store answers topical queries; `embed = false` is documented truthfully.

**Phase 3 — exact and phrase lanes.** `cites` extraction at ingest, n-gram index, quoted-span
routing to the `text_phrase` position index, phrase-unsupported reporting. New smoke:
`tests/document_fusion_smoke.rs` — docket query ranks the exact lane first, quote query resolves the
case name.
*Exit:* `§362(a)(1)`, `2-24-1234`, and `"Smith v. Acme"` each resolve in one query.

**Phase 4 — facets and schema v2 columns.** `doc_type`, `section`, `jurisdiction`, `doc_date`,
`heading_path`, `keywords`, `entities`, `summary` written at scan; scalar indexes; in-store
prefilter replacing the Rust post-filter; MCP hit gains `matched_lanes`, `lane_ranks`,
`retrieval_mode`, `citation`.
*Exit:* `entity_category` / `keywords_contains` return exactly `limit` rows when selective; result
count no longer depends on top-k luck.

**Phase 5 — quality and tuning.** Rerank on by default with `top_k = 40`; per-document cap and
byte-span overlap dedupe; section-intent boost; build the evaluation harness (§13) and settle the
weights and chunk sizes on the real matter corpus.
*Exit:* a written benchmark result justifying every weight and default in §5 and §9 — and a
documented decision to change any of them.

## 12. Shared fusion refactor

- Move `src/search/rrf.rs` and the scoring half of `src/search/bm25.rs` behind `#[cfg(feature =
  "intelligence")]`; keep the code-tier *indexing* half (postings build/write) under
  `code-search`. Genericize `build_chunk_postings(&[CodeChunk])` into a `posting_text(&str) ->
  Vec<(String, u32)>` entry point so the documents tier reuses tokenization rules rather than
  copying them.
- One `Lane` enum (`Exact`, `Keyword`, `Facet`, `Vector`) and one `FusionWeights` struct replace the
  three `WEIGHT_*` constants, with the code tier's current values as the default so existing code
  search ranking is unchanged (regression-checked by the existing smoke tests).
- `matched_lanes` / `lane_ranks` become a shared `LaneProvenance` struct returned by both tiers.

## 13. Test plan

Unit (Bun-free, `cargo test --lib`):

- Tokenizer: `§ 362(a)(1)`, `C.F.R.`, `No. 2-24-1234`, `Smith v. Acme`, `"quoted phrase"`,
  accented FR/ES tokens, boilerplate stop-words. Assert ingest and query tokenization agree by
  comparing against `lancedb::tokenize(query, &builder)`.
- Normalizer: quote extraction, citation splitting, idf-ordered relaxation (assert the term order
  is preserved from the BM25 idf formula).
- Fusion: known lane ranks → expected fused order; empty lanes contribute nothing; stable tie-break
  on `chunk_id`; per-document cap and 50% byte-span overlap dedupe.
- Schema: `documents_schema(None)` builds, `embedding` is nullable, `documents_v2` field list
  matches this spec.

Integration / smoke (`tests/`, existing `*_smoke.rs` convention):

- `tests/document_fts_smoke.rs` — index a small fixture corpus, assert a lexical-only query returns
  the expected path with `retrieval_mode = "lexical"` and no embedder is ever constructed
  (guard the "no ONNX model download" property by pointing `BASEMIND_DATA_HOME` at an empty dir).
- `tests/document_fusion_smoke.rs` — lexical + vector fixtures, assert keyword outranks vector on a
  lexical-overlap query and exact outranks both on a docket-number query.
- `tests/mcp_smoke.rs` extension — `memory documents` with `entity_category` / `keywords_contains`
  now prefilters in-store (assert `num_unindexed_rows` untouched and result count == limit when a
  selective facet is supplied).
- `tests/config_schema.rs` — snapshot regenerated and no longer `#[ignore]`d.

Evaluation harness (new, `tests/legal_eval/` or `benches/`): BM25 vs dense vs RRF vs
rerank, reporting Recall@k / MRR / nDCG@k per lane and post-fusion, over LegalBench-RAG
(CUAD / ContractNLI / PrivacyQA) and, if useful, COLIEE. Weight and chunk-size decisions in §5
and §9 are **not** finalized until this harness runs on the real matter corpus; the defaults above
are the starting point to be measured, not a claim.

## 14. Migration, ops, rollback

### 14.0 At-rest encryption (requirement, not option)

The Lance store holds chunk text, `cites`, facet columns and FTS postings. It is written **in
clear** — LanceDB OSS has no at-rest encryption, and Lance requires plaintext to build FTS indexes
and run vector search, so column-level encryption is not an available answer. Encryption at rest
therefore comes from the storage layer **below** the data home: LUKS on Linux, FileVault on macOS,
BitLocker on Windows. This is a deployment requirement for any deployment that stores documents,
not a basemind feature.

The scope is the whole of `cache_root()` — `cache/blobs/` (which holds extracted text), every
per-workspace Lance store, the registry snapshot, and the `.chunk.msgpack` sidecars. Encrypting a
subdirectory is not an accepted configuration.

Enforcement: when `redaction.enabled = true` and the volume cannot be verified, the scan **warns
loudly**; hard refusal exists only behind an explicit opt-in. macOS and Windows verify authoritatively
(FileVault, BitLocker), Linux is best-effort, and `unknown` is never reported as `not encrypted` —
otherwise every container, CI runner and network mount warns, and a warning that always fires is a
warning nobody reads. The check runs at scan and its verdict is surfaced in `basemind doctor`.

See [ADR-0013](../adr/0013-encryption-at-rest-comes-from-the-volume.md).

- Table rename `documents` → `documents_v2` means existing workspaces keep serving v1 rows until
  rescan; `memory documents` reads v2 when present and falls back to v1 with a warning.
- Reindex triggers: any tokenizer/`FtsIndexBuilder` change, `embedding_preset` change (existing
  behaviour), schema bump. Same wipe semantics as `embedding_preset` — surfaced at scan time, not
  silently.
- Rollback: drop `documents_v2`, repoint reads at `documents`. No data loss (extraction is cached
  in `.chunk.msgpack` sidecars), only re-embed cost if vectors were involved.
- Perf: FTS index adds disk and build time proportional to chunk count. `max_chunks_per_document`
  stays the blast-radius control; `optimize()` cadence is the read-latency control.

## 15. Risks

| Risk | Mitigation |
|---|---|
| Trigram `cites` index inflates store size | Scoped to one short column; `[documents.citations].extract = false` escape hatch |
| `with_position` phrase index is large | Separate table-level index, config-gated, disabled by measurement if too big |
| Fusion weights regress code search | Code-tier defaults preserved; existing smoke tests assert unchanged order |
| `documents_v2` breaks `store_gc` / compaction paths | Phase 0 includes the GC audit; GC must not treat v2 as garbage |
| Schema snapshot drift | Re-enable `schema_snapshot_matches_derived` in the same PR |
| Rerank ONNX download on first use | Already opt-in per call; document the download, keep `enabled = false` override |
| Redaction (`token_replace`) breaks identifier search | Index `cites` from **pre-redaction** text, **excluding self-references** (own caption, own docket number), so the clear-text column holds public authority only. `rehydration_ref` unchanged. Residual risk is egress, not at rest — handled by #7. At-rest protection is a volume requirement, tracked in #45 |
| Legal stop-word removal harms exact phrase recall | Two-index split (§7.2): stemmed/stop-worded index for topical recall, position index for exact recall |

## 16. Open questions

1. Is `bge-reranker-v2-m3` the right reranker for citation-heavy queries, or should a
   citation-aware cross-encoder be evaluated?
2. Should `doc_type` be user-supplied at scan time for a curated corpus, or inferred? Inference is
   cheap but noisy; a curated `doc_type` is a much stronger facet.
3. Do we need cross-workspace/cross-matter retrieval (a global citation index) as a separate tier,
   or is per-scope isolation sufficient for v1?
4. Section decomposition: xberg emits Markdown, not labeled legal sections. Rule-based heading
   heuristics, or an optional LLM pass behind `[llm]`?

## 17. References

- LanceDB FTS index and tokenizer options — <https://docs.lancedb.com/indexing/fts-index>
- LanceDB hybrid search + `RRFReranker` — <https://docs.lancedb.com/search/hybrid-search>
- lancedb Rust API: `Index::FTS`, `FtsIndexBuilder`, `Query::full_text_search`,
  `lancedb::rerankers::rrf::RRFReranker`, `lancedb::tokenize` — <https://docs.rs/lancedb/0.37.1/lancedb/>
- xberg configuration reference (chunking, keywords, NER, reranker, OCR) — <https://docs.xberg.io/reference/configuration>
- Legal retrieval benchmark (BM25 vs dense vs RRF vs rerank on CUAD) — <https://github.com/Akshitha024/legal-retrieval-benchmark>
- Section-weighted hybrid retrieval for legal cases — <https://arxiv.org/html/2606.03138v1>
- Claim-level RAG for law: chunk size / overlap ablation — <https://arxiv.org/html/2605.21071v1>
- Legal BM25 reliability study — <https://aclanthology.org/2025.nllp-1.3.pdf>
- ADR-0012 — [`../adr/0012-multi-lane-document-retrieval.md`](../adr/0012-multi-lane-document-retrieval.md)

# Spec 0016: The entity graph — factual GraphRAG over GLiNER entities

- **Status:** Proposed — implements [ADR-0016](../adr/0016-entity-graph.md). Every decision map #98
  left in its fog is closed here; implementation is tracked in #99–#104.
- **Date:** 2026-10-27
- **Scope:** the entity/edge Lance tables, the `graphrag` feature and `[graphrag]` config block, the
  graph Cypher path, and two new `graph` tool modes. It does not touch the documents row schema, the
  extraction NER pass, or the redaction engine.
- **Feature gates:** `documents` (required — the graph is over document chunks), `intelligence`
  (LanceDB + embeddings), `dep:lance-graph` (the new dependency, behind `graphrag`), `xberg/ner-onnx`
  (the extraction NER backend, today carried by `memory`).

---

## 1. Problem

Retrieval today answers with chunks. The relationship between entities across the corpus —
which mentions co-occur, which chunks carry the same party, which passages connect through a
shared organization — exists nowhere the agent can reach:

1. **Entities are per-chunk sidecar data.** The extraction NER writes `DocEntity` values
   (`src/extract/doc.rs:177`) consumed by the scanner's document pass (`src/scanner_docs.rs:381-383`)
   into the sidecar surfaced on hits. There is no persisted node, and no way to ask
   "everything about this entity".
2. **Vector retrieval cannot make a second-order hop.** A query about a secondary entity — one the
   question implies but the best-ranked chunk does not name — returns the chunk that misses it.
   The connections are present in the corpus and unused.
3. **The code tier already has a graph surface.** ADR-0001/0002/0003/0004 built `codegraph`
   (`src/mcp/codegraph.rs:1-15`) with provenance-tagged edges, traversal, and communities, and the
   `graph` domain tool already serves nine modes for it (ADR-0011). Documents have no equivalent:
   the strongest retrieval tier for scan corpora has the weakest structural retrieval.

Why lance-graph rather than a hand-rolled join: the engine gives a Cypher surface over Arrow tables
whose dependency set (`lance 13.0.0`, `datafusion 54.1`, `arrow 58.4`) is already resolved in the
lockfile under `lancedb 0.40` — the crate is commit-cheap, and its grammar keeps the server from
inventing a query language.

## 2. Current state, mapped

| Concern | Location | Behaviour |
|---|---|---|
| Extraction NER | `src/config/documents.rs:349-380` (`NerConfig`) | `enabled = false`; `backend = onnx` (gline-rs ONNX weights, downloads on first use) or `llm`; `categories`, `custom_labels` (GLiNER zero-shot) |
| Entity records | `src/extract/doc.rs:71-79`, `:177`, `:793-825` | `DocEntity` (mention + label) per extraction, carried into the document pass |
| Sidecar consumption | `src/scanner_docs.rs:381-383` | entities iterated once per document; surfaced on hits (spec 0012 §1: sidecar lives outside the row) |
| Documents row | `src/lance/schema.rs:30-51` | `scope, path, chunk_idx, mime_type, text, heading_path, byte_start, byte_end, rehydration_ref, embedding` — `embedding` nullable (lexical-only stores exist); **no `entities` column yet (spec 0012 Phase 4 pending)** |
| Fusion | `src/config/documents/retrieval.rs:19+` (`FusionConfig`) | RRF lanes `exact 3.0 / keyword 2.0 / facet 1.0 / vector 1.0`, `k = 60`; implementation shared (`src/search/rrf.rs:138`) |
| Redaction | `src/config/documents.rs:429-505` | default off; default strategy `token_replace` — stable `[TYPE_N]` tokens per category, reversible via rehydration map; `preserve_offsets` default true; `mask` / `hash` / `drop` irreversible |
| Redact-time NER thresholds | `src/pii/pipeline.rs:86-92` | `gliner_label_threshold`: `person/full_name 0.7`, `api_key/password/iban/ip_address 0.3`, else `0.5` — redaction-precision values, not graph values |
| Rehydration map | ADR-0013 (encryption from the volume), ticket #89 | the engine does not persist the key; map storage eliminated from the derivable-key blob path |
| Lance tables | `src/lance/mod.rs:237-238`, `:278`, `:313`, `:437` | `documents`, `memory`, `code_chunks`, `doc_links` — `ensure_table` at open; `delete_doc_links_for_paths` (`:504-506`) is the tombstone-sweep precedent |
| Code graph | `src/mcp/codegraph.rs:1-15`, ADR-0001–0004 | in-memory, provenance-tagged, build-on-demand — code tier only |
| MCP surface | ADR-0011 | nine domain tools; `graph` currently: `calls, neighbors, path, subgraph, communities, map, export, display, open`; CLI parity gated by `tests/cli_parity/` |
| Boundary | ADR-0015 (on branch `feat/privacy-gateway-boundary`) | agent sees tokens; `redact_text`/`vault` withdrawn in protected workspaces; the `safe/` mirror is the marker |

**Version facts.** `lance-graph` 0.5.4 is published on crates.io (updated 2026-03-21) with
`lance = "13.0.0"`, `datafusion = "54.1"`, `arrow 58.4`; the lockfile already carries `lance 13.0.0`,
`lancedb 0.40.0`, `datafusion 54.1.0`. The upstream repo warns of a workspace reorg (`docs/
project_structure.md`) — the pinning rationale in §10.

## 3. Goals

- **G1** A persisted, cheaply rebuildable entity graph over the documents corpus exists — nodes are
  observed GLiNER mentions, edges are observed co-occurrences.
- **G2** The graph is faithful to redaction by construction: it can never contain a value the
  `documents` table cannot already contain (§7).
- **G3** Cypher over the graph is reachable as a `graph` mode with read-only enforcement that is
  structural, not inspected (§8.1).
- **G4** `graph rag` closes the second-order-hop gap: embedding seeds → entity expansion → fused
  relevance, with per-hit provenance of how it was reached (§9).
- **G5** Graph builds and rebuilds follow the scan lifecycle without a merge protocol — a scope
  rebuild is a drop-and-refill, and tombstones sweep like `doc_links` (§11).
- **G6** Every new knob is documented and schema-regenerable; nothing new configures the stack
  silently (spec 0012 §10 precedent).

## 4. Non-goals

- Inferred relations, summaries, community detection, or any LLM pass over the document graph —
  this graph is factual. The code tier's ADR-0004 machinery stays code-tier.
- Cross-document identity for token-replace stores (§7.2) — deferred to the gateway contract;
  basemind will not approximate it.
- Node embeddings — retrieval uses chunk embeddings only.
- Second NER pass, graph-tuned thresholds — rejected with the rationale in §6.
- Any change to tool *names*; only `mode` values are added (spec 0012 §4 precedent).
- The gateway itself (map #87): the graph obeys the boundary; it does not define it.

## 5. Target architecture

Two new Lance tables in the existing per-scope store, written at scan, queried at serve time over
Arrow batches handed to `lance-graph`:

```text
scan (document pass, extraction NER enabled, [graphrag] enabled = true)
  ├─ entities: DocEntity records of the chunk        (sidecar, unchanged)
  ├─ normalize + identity key (§7.2)                 (shared fn — build and query)
  ├─ upsert per-node mention rows  → graph_entities   (Lance table)
  └─ co-occurrence edge rows within the same chunk → graph_edges  (Lance table)

query — graph entities
  └─ load batches from both tables → lance-graph Cypher (read-only) → rows

query — graph rag
  ├─ embed query (existing embedder) → vector lane top-k chunks
  ├─ seed nodes := entities of those chunks
  ├─ Cypher expansion 1–2 hops over the lattice
  ├─ expanded chunks join the fused candidates as the graph lane
  └─ emit hits with graph_path provenance
```

The engine executes over **Arrow batches supplied in memory** — the `CypherQuery.execute(<tables>)`
shape of the Python API, whose Rust twin is the same project. Basemind never wires `lance-graph` to
a catalog or write path; the mode hands it read snapshots and takes rows back. This is what makes
read-only enforcement structural (§8.1) and keeps the `lance-graph-catalog` sub-crate out of the
dependency graph. Verifying this path on the real lockfile is prototype ticket #99's exit
criterion.

Rejected at design time, not deferred: feeding the engine native Lance datasets via its catalog
path. It would couple two engines to one store for no correctness gain — the tables are small
(scalable by tens-of-thousands of mentions per scope), and in-memory batches keep one owner (the
existing connection) over the physical store.

## 6. Entities: one pass, the label set decides

**One NER pass; the graph does not re-run the model.** The graph consumes the `DocEntity`
records the documents pass already produces. A graph-tuned second pass doubles per-document model
latency for the whole extraction — the exact cost regression ticket #97 records for the *redact*
surface (`latence ×2 à ×3`). Recall for graph retrieval is tuned through the label set, not
through a confidence knob.

**The label set is a filtered projection.** `[graphrag] labels` selects which of the produced
`DocEntity` labels become nodes. Default: `["person", "organization", "location"]` plus any
`[documents.ner] custom_labels`, minus the PII-shaped numeric/secret labels
(`email`, `phone`, `iban`, `swift_bic`, `ip_address`, `api_key`, `password`, `ssn`,
`credit_card`, `date_of_birth`) — secret- or identifier-shaped nodes carry no retrieval value
across a corpus and would invite "the graph leaks PII" audits for a feature nobody asked for.
Date-of-birth is excluded outright. Every edge stores the label of each endpoint so an edge is
never a label-loss claim.

There is deliberately **no per-label threshold** for the graph. The redact-time thresholds
(`gliner_label_threshold`, `src/pii/pipeline.rs:86-92`) are precision-tuned for redaction — a
different decision with different stakes — and copying them would inherit their stakes; lowering
them would smuggle a second NER pass. Threshold tuning for a factual graph is a question for an
evaluation harness on a real corpus (§15), and until then the extraction pass's own behaviour is
the single trusted source.

## 7. Privacy contract — the graph stores what the documents table stores (closed decision)

7.1 **The graph is a derived subset of post-redaction content.** The builder reads the records
that produced the sidecar and wrote the row: the content after xberg redaction (`token_replace`
rewrites the document in place and captures the map — `src/config/documents.rs:435-440`).
The graph's `literal` and `mention` columns contain exactly what the chunk `text` column
contains. Under `mask` / `hash` / `drop` (or redaction disabled) nothing changes for the graph:
it holds the values the `documents` table already holds — an exposure class that exists today,
not a new one. This is the answer to map #98's fog question "tokens only, or raw values where
redaction is off": **the graph gets no separate answer from the index itself.** (ADR-0013: at-rest
encryption is the volume's requirement; §13 carries it for the graph the way spec 0012 §14.0
carries it for the rows.)

7.2 **Identity under `token_replace` is document-scoped.** This is the load-bearing subtlety and
the reason a naïve `(scope, literal)` key is a defect on a redacted store: `[EMAIL_1]` is unique
within a document but shared across documents, and the only true join key lives in the rehydration
map, which the engine must not persist (ADR-0013 / ADR-0015). The identity key therefore carries a
storage-shape split:

```rust
enum EntityKeySource {
    /// A `token_replace` marker (the `\[[A-Z_]+_[0-9]+\]` shape): identifies
    /// (scope, label, literal, document). Document-scoped.
    TokenOfDocument,
    /// Everything else: identifies (scope, label, normalized_literal). Global within a scope.
    ClearLiteral,
}
NodeIdentity = scope × label × key_source × (normalized_literal | document_key)
```

The result: on a redacted store the graph answers within-document structure faithfully — with zero
cross-document false merges — and connectivity across documents is honestly absent rather than
spuriously present; on a clear store (redaction off, or `mask` / `hash` / `drop`, whose outputs
do not decode to document-scoped tokens) literals merge globally as the FTS fold does. The same
normalization runs at build and query time — case-fold + ASCII-fold + whitespace collapse, chosen
to sit beside the keyword lane's FTS fold (spec 0012 §7.1 documents the index's fold; the two do
not have to be byte-identical, but they are one shared function here, asserted by a unit test, §12).

7.3 **The graph never carries a rehydration reference.** `rehydration_ref` stays a
`documents`-row column. No graph row links to a vault key: an entity node is an address into
chunks, not a claim about identity beyond what §7.1 covers.

## 8. Tool surface — two modes of the existing `graph` domain tool (closed decision)

ADR-0011's nine-tool surface is closed. Both new capabilities are `mode` values of `graph`, with
the CLI subcommands `basemind graph entities` / `basemind graph rag` and gate extension in
`tests/cli_parity/`.

### 8.1 `graph entities` — restricted Cypher, read-only by construction

Parameters: `cypher` (string) and nothing else listed on the wire. `limit` is **not** a
parameter: the helper rewrites or enforces a LIMIT (buys a deterministic budget; config §10
`max_rows`, default 200, bounded server-side). Per ADR-0011's tool-surface rules, mode-specific
parameters are optional siblings validated in the helper with precise error variants:

- `graph_disabled` — feature or config off (fail-closed default).
- `cypher_rejected` — grammar refusal (non-MATCH / into-update shapes). The engine is the first
  line: CypherQuery is a query language over supplied batches; the surface never hands the store
  to it, so even a hostile pattern terminates on a read snapshot. The helper is the second line,
  refusing `CREATE/DELETE/SET/DROP` tokens outright with the variant naming the token.
- `schema_mismatch` — table missing (store predates the rollout; §13).

The `node_label` contract with the engine is fixed at one logical label, `Entity`, with
properties carrying `label`, `literal`, `token_scoped`, `path`, `chunk_idx` — a one-dataset
design (the multi-`with_node_label` shape stays available for a later typed split read on a
measured need, and the Cypher surface records that pivot in one config constant, not in an
engine migration).

### 8.2 `graph rag` — hybrid retrieval with provenance, not a new retrieval stack

Parameters (all sibling-validated): `query` (text), `k` (seed chunks, default 10, bounded),
`hops` (1 default, 2 max), optional `label` filter. One gate, with a precise variant: the mode
refuses a lexical-only store (`embedding` null for the scope, `src/lance/schema.rs:44-48`) with
`rag_unsupported` naming the gap instead of an empty result that reads as "the graph found
nothing".

Flow (§5 diagram, right branch): existing embedder → vector top-k on `documents` → seed nodes
from the graph side (chunks' entity identity keys) → Cypher expansion → expanded chunks join.
Fusion reuses the shared implementation: an expanded chunk is a ranked hit on a `graph` lane
(weight default `1.0`, §10) — `rrf_fuse_detailed` reads ranks (`src/search/rrf.rs:121-138`), so
a lane is a rank list plus provenance, exactly what expansion returns. Emit `graph_path` (a
hop-ordered node list) and `graph_distance` per hit that expanded arrived — a hit's `matched_lanes`
gets `graph` appended like any other lane, no bespoke shape.

## 9. Builder semantics — write path, idempotence

- **Row shape `graph_entities`:** `scope, entity_id (identity-key hash, §7.2), label, literal
  (the normalized key), mention (the stored lexeme), path, chunk_idx, token_scoped (bool),
  source_kind (`gliner_onnx` or `gliner_llm` — the only backends `[documents.ner]` offers today).
  No embedding on a mention — retrieval embeds chunks only (§4).
- **Row shape `graph_edges`:** `scope, src_id, dst_id, weight, chunk_ctx` (List<Utf8> of the
  chunk coordinates `(path, chunk_idx)` that observed the pair). Repeating the pair adds one to
  `weight` and appends to `chunk_ctx`; self-edges (a chunk mentioning one entity twice) are
  dropped — a self-loop carries no second-order signal the chunk text does not already carry.
  Because a node id already encodes the §7.2 source, an edge between two ids is one claim with
  chunk provenance, never a re-derivation of identity.
- **Idempotence:** a re-scan deletes a document's mention rows and every edge row whose `chunk_ctx`
  names it (batch delete like the `doc_links` pattern, `src/lance/mod.rs:504-506`) before refill —
  it never accumulates. Edge rows survive another document's re-scan with their `weight` intact;
  the per-document sweep is sufficient because every edge row carries chunk provenance (§9).
- **Rebuild = the same write path.** No merge protocol: drop-and-refill per scope. The store
  layout that governs everything else (ADR-0013: the volume carries the encryption) is untouched —
  the graph lives inside the same store the connection already opens.

## 10. Configuration surface (new keys, all defaults documented)

```toml
[graphrag]
enabled = false                 # fail-closed: the tables are not created while false
labels = ["person", "organization", "location"]   # projection over the NER pass's output
                                # custom_labels of [documents.ner] pass through automatically
max_hops = 1                    # graph rag expansion ceiling (1..2)
seed_chunks = 10                # vector top-k seeds for graph rag
weight_graph = 1.0              # RRF weight of the graph lane (secondary to the lanes of spec 0012 §5)
max_rows = 200                  # graph entities server-side LIMIT ceiling
```

Placement rule: `[graphrag]` is its own top-level block (not a `[documents]` child — the graph
spans the store's tables and is not a documents-side extraction knob). Every key is
`#[serde(default)]` per the parent-module convention and the schema snapshot regenerates with
`cargo test --features full --test config_schema -- --ignored regenerate_schema`
(`tests/config_schema.rs`), re-enabling `schema_snapshot_matches_derived` as spec 0012 §10 did.

**Dependency placement (closed decision):** `lance-graph = "0.5.4"` pinned (version-only) in
`[workspace.dependencies]` with `default-features = false` and only the features the mode consumes
per the workspace rule (compatible versions, no capability unions). A minor-pin upgrade procedure
means one note in `CHANGELOG.md` per bump; the upstream workspace reorg (§2) is a *watch item*,
not a reason to vendor — the crate is pre-1.0, its grammar is the product, and a break of the
execute-over-batches shape reopens ticket #99's exit criteria in a fix-up note.

## 11. Lifecycle — scan, rescan, tombstones, GC

- Created by `ensure_table` at store open behind the `documents` feature, beside `doc_links`
  (`src/lance/mod.rs:237-238` pattern) — graph tables live in the same store, participate in the
  same per-scope wipe on schema mismatch.
- A document's deletion removes its mention rows and every edge row whose `chunk_ctx` names one
  of its chunks — the edge's `weight` drops accordingly, or the row disappears when the last
  observation is gone. The per-document sweep is sufficient because every edge row is
  chunk-anchored (§9).
- `store_gc` treats the tables as derived, document-indexed state: the per-document sweep in §9
  is the whole lifecycle; no compaction protocol, no `.chunk.msgpack` change, no dossier
  contract. A scope wipe (`store_gc`'s existing workspace path) removes them with the documents
  they derived from.
- Renames (`path` change with content stable) re-key the `path` column of affected mention rows
  and the `chunk_ctx` coordinates of their edges on the next sweep of that scope (§12 asserts
  this; the `doc_links` bulk-delete pattern already accepts this cost).

## 12. Test plan

Unit (in-file, existing convention):

- Normalization: case-fold, ASCII-fold, whitespace collapse — idempotent, and build/query share
  the fn (assert by construction: one module, both callers).
- Token-shape classification: `[EMAIL_1]`, `[ORG_12]` → `TokenOfDocument`; `Dupont`,
  `Acme Corp` → `ClearLiteral`; `[REDACTED]` (mask) and `[HASH:…]` (hash) → `ClearLiteral` — the
  §7.2 split is keyed on the `token_replace` shape, and masked stores get global identity by
  intent, documented.
- Identity key: the §7.2 table as code; `entity_id` stability under reordering.
- Edge weight: two mentions in one chunk = weight 1; rescan of the document replaces, never
  doubles; rescan of a renamed path rekeys per §11.
- Config: defaults; `[graphrag] enabled = false` creates nothing; schema snapshot regenerated.

Integration / smoke (`tests/`):

- `tests/graph_entities_smoke.rs` — ingest fixture → `graph entities` Cypher: node filter, one
  hop, two hops; LIMIT enforcement; `graph_disabled` and `cypher_rejected` variants.
- `tests/graph_rag_smoke.rs` — the §1 offer: the secondary entity the best vector chunk does not
  name arrives through expansion, `matched_lanes` says `graph`, and `graph_path` walks a real
  edge; lexical-only store refuses with `rag_unsupported`.
- `tests/cli_parity/` — `graph entities` / `graph rag` modes mirror (ADR-0011 gate).

Explicitly deferred, not skipped silently: no latency bench in this spec. The shared bench tooling
wraps store-level datasets today, and a graph-latency bench becomes an addendum when a real corpus
topology says it is needed (§15, question 3).

## 13. Migration, ops, rollback

- New tables alongside `doc_links`; no dim meta change, no store-format bump, no
  `.chunk.msgpack` change. First store upgrade creates empty tables; `graphrag.enabled = false`
  store stays byte-identical except for the `ensure_table` (§11).
- Rollback: drop the two tables (or set `enabled = false` and keep them empty); no documents-table
  migration happens in this spec, so undo is cheap — this spec's blast radius is the graph tables,
  the config block, and the dependency.
- Deployment note for legal-facing installs (ADR-0015, spec 0012 §14.0): the graph adds no
  at-rest exposure class beyond the documents table, so it inherits the same volume-encryption
  requirement, not a new one.

## 14. Risks

| Risk | Mitigation |
|---|---|
| `lance-graph` pre-1.0, upstream workspace reorg | Version pin in `[workspace.dependencies]` (§10); enforcement surface stays on two stable shapes (GraphConfig, execute-over-batches); re-opening #99's criteria is one note |
| Entities missing where NER is off | `graphrag.enabled` gates the write path but the read tools always fail-closed on empty tables — never silently empty-*looking* (§8.1 variants) |
| `graph rag` pollutes fusion for code-search ranks | The graph lane exists only in the document fusion path; the code lane set is untouched (spec 0012 §12's regression story, unchanged) |
| Token-scoped identity can look over-broad merges to evaluators | `token_scoped` is a stored column; a redacted-store audit can distinguish per-document scoping from a merge (§7.2 is auditable by construction) |
| Edge explosion on entity-dense corpora | weight = 1 per (chunk, pair); per-chunk entity count is the driver; a cap lands as config only with a measured corpus need (§15) |
| Cypher escapes the budget | `max_rows` enforced server-side; engine over in-memory read snapshots; no write grammar accepted (§8.1) |

## 15. Open questions

1. ~~Do we integrate lance-graph over catalogs or in-memory batches?~~ **Decided at design time
   (verified by #99):** in-memory batches — §5. The prototype still verifies compilation of that
   path against the shared lock; that is the design-facts check, not an open design.
2. ~~Which tools?~~ **Decided: graph tool modes** — §8, per ADR-0011.
3. Graph-latency bench on a real corpus topology — deferred with intent recorded; add as an
   addendum when a real corpus is in hand (§12, last note).
4. Per-community surface (ADR-0004-style) for the document graph — out of scope; revisit only if
   `graph rag` evaluation shows hub-entity dominance that the label set cannot express.
5. A cross-document identity channel for token-replace stores, should the passerelle ever offer
   one — reserved for the map #87 contract's owner; the §7.2 source enum accepts a third variant
   without a schema bump (the affected nodes take §9's drop-and-refill on the next build).

## 16. References

- lance-graph (GitHub) — <https://github.com/lance-format/lance-graph>
- lance-graph 0.5.4 (crates.io) — <https://crates.io/crates/lance-graph>
- Map #98 — <https://github.com/jamon8888/basemind/issues/98> (tickets #99–#104)
- ADR-0002 — [`../adr/0002-edge-provenance-and-confidence.md`](../adr/0002-edge-provenance-and-confidence.md)
- ADR-0011 — [`../adr/0011-mcp-tool-surface-redesign.md`](../adr/0011-mcp-tool-surface-redesign.md)
- ADR-0015 — [`../adr/0015-privacy-gateway-boundary.md`](../adr/0015-privacy-gateway-boundary.md)
  (branch `feat/privacy-gateway-boundary`)
- Spec 0012 — [`0012-lexical-document-retrieval.md`](0012-lexical-document-retrieval.md) (the
  §6.1-null rule, the fusion lanes, the §10 config-conventions this spec reuses)
- ADR-0008 (documents/code graph shared boundary) — [`../adr/0008-documents-code-graph.md`](../adr/0008-documents-code-graph.md)

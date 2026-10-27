# ADR-0016: The entity graph is a factual, redact-faithful co-occurrence graph queried in Cypher

- **Status:** Proposed
- **Date:** 2026-10-27
- **Deciders:** basemind maintainers
- **Related:** ADR-0002 (edge provenance and confidence), ADR-0004 (community detection), ADR-0008
  (documents/code graph), ADR-0011 (MCP tool-surface redesign), ADR-0015 (privacy gateway boundary,
  on branch `feat/privacy-gateway-boundary`), map
  [#98](https://github.com/jamon8888/basemind/issues/98), spec
  [0016](../specs/0016-entity-graph.md)

## Context

Map #98 asked for GraphRAG: layer a knowledge graph over the existing document index, built on GLiNER
entities (`[documents.ner]`, the gline-rs ONNX backend), queried in Cypher via
[`lance-graph` 0.5.4](https://crates.io/crates/lance-graph), combined with the existing embedding
lane. Three facts shaped the decision:

- **The dependencies are already resolved.** `lance-graph` 0.5.4 depends on `lance 13.0.0` and
  `datafusion 54.1` — both already in the lockfile under `lancedb 0.40`. The crate adds no new
  heavyweight dependency.
- **The MCP surface is closed.** ADR-0011 consolidated 86 tools into nine domain tools, each with a
  required `mode`. A `graph_query` / `graph_ask` pair as standalone tools would reopen the dilution
  the consolidation exists to remove; they belong as `graph` modes.
- **Privacy does not get a new answer here.** ADR-0015's line is that the agent sees tokens; the
  documents index already stores post-redaction text (`text`, `src/lance/schema.rs:30-51`) and the
  rehydration map is not persisted by the engine. Whatever the entity graph stores, it must be a
  subset of what the `documents` table may already store — a new exposure class would be a
  regression independent of GraphRAG.

The five open questions on map #98's fog — graph content vs PII, entity identity, label set and
thresholds, graph lifecycle, and `lance-graph` pinning — are closed by
[spec 0016](../specs/0016-entity-graph.md) rather than left open. This ADR records the decisions the
spec encodes.

## Decision

Basemind builds and serves one **entity graph** — nodes are GLiNER mentions resolved to an
identity key, edges are same-chunk co-occurrences with nothing inferred — persisted as two Lance
tables (`graph_entities`, `graph_edges`) beside `doc_links` (ADR-0008), written at scan from the
sidecar records the documents pass already produces, and queried read-only via `lance-graph`
executed over Arrow batches loaded from the existing connection. The agent-facing surface is two
new modes of the existing `graph` domain tool: `graph entities` (restricted read-only Cypher) and
`graph rag` (embedding seeds → Cypher expansion), gated by a new `graphrag` feature and a
default-off `[graphrag]` config block.

The graph stores exactly the lexemes the `documents` table stores. Under `token_replace`
(default strategy, `src/config/documents.rs:429-469`) the stored lexemes are the stable
`[TYPE_N]` tokens, so a redacted store's graph is a token graph and every answer it serves is
token-faithful. Under `mask`, `hash` or `drop` — and in unredacted stores — the graph holds the values the index already holds, and the graph introduces no new exposure class.

Entity identity under `token_replace` is **document-scoped**, because `[EMAIL_1]` in one document
and `[EMAIL_1]` in another are different identities whose only join key lives in the rehydration
map the engine must never persist (ADR-0013: encryption at rest; ADR-0015: boundary). Clear-text
literals resolve globally within a scope via the same normalization the FTS index uses
(case-fold + ASCII-fold + whitespace collapse).

## Consequences

- Two new Lance tables and their GC handling mirror `doc_links` (ADR-0008), including the
  tombstone sweep; a scope rebuild is a drop-and-refill, not an incremental merge.
- Cypher read-only enforcement is structural: the engine executes over read snapshots passed in
  memory, so it is never given a write path to the store.
- `graph rag` requires a vector store; on a lexical-only store the mode errors with a dedicated
  variant while `graph entities` keeps working.
- A new dependency enters behind the `graphrag` feature (`lance = 13.0.0` and friends are already
  compiled); the config schema snapshot regenerates (`tests/config_schema.rs`).
- CLI parity gates the new modes (`tests/cli_parity/`), per ADR-0011's completeness gate.
- Cross-document identity for redacted stores is deferred to the gateway's side of the boundary;
  basemind deliberately does not approximate it, reserving the decision for the passerelle
  contract (map #87).

## Alternatives considered

- **Reuse the in-memory `codegraph` model** (`src/mcp/codegraph.rs:1-15`) for entities — rejected:
  `codegraph` is build-on-demand and discarded, the right shape for the code tier; a document
  entity graph wants persistence adjacent to embeddings, not a second in-memory graph.
- **LLM-inferred relations / community summarization (Microsoft-GraphRAG style)** — rejected for
  this card: offline-first and LLM-free is the product baseline; the graph stays factual
  (mentions + co-occurrence). ADR-0004's community machinery remains code-tier scope.
- **A second NER pass tuned for graph recall** — rejected: doubles model latency for every
  document (the same latency objection recorded on #97), and graph recall is adjustable through
  the label set without re-running the model.
- **Standalone `graph_query` / `graph_ask` tools** — rejected: violates ADR-0011's consolidation;
  a standalone tool name earns nothing over a mode description and costs a competitor in deferred
  retrieval.
- **Persist an entity→entity merge table to recover cross-document identity in redacted stores** —
  rejected: any basemind-side join key would have to come from rehydration values, which ADR-0015
  puts beyond the engine's reach. If the passerelle ever offers a stable cross-document entity
  channel, the graph accepts it without a schema bump (§15, question 5).

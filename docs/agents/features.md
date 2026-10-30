# Feature catalog

This catalog connects user capabilities to their implementation and build gates.
[`Cargo.toml`](../../Cargo.toml) is authoritative for feature composition;
[`src/main.rs`](../../src/main.rs), [`src/cli/`](../../src/cli/), and
[`src/mcp/mode.rs`](../../src/mcp/mode.rs) define the public vocabulary.
Consult CLI `--help` for exact flags rather than inferring them from MCP fields.

## Build profiles

Default features are empty. The base build includes scanning, structural code
navigation, code graphs, git queries, configuration, and supporting CLI tools.
Some domain schemas remain present even when their optional implementation is
unavailable; advertised modes alone do not prove that an operation can run.

| Cargo feature | Adds or enables |
| --- | --- |
| `intelligence` | Shared xberg/LanceDB/Arrow infrastructure. |
| `tokenizer` | Real o200k token counting without the full intelligence stack. |
| `documents` | Intelligence + tokenizer, document formats, OCR/layout processing, embeddings/reranking, enrichment and redaction dependencies. Runtime settings still select which operations run. |
| `code-search` | Intelligence plus embeddings/reranking for semantic code retrieval. |
| `memory` | Intelligence plus embedding/entity/keyword dependencies for durable agent memory. |
| `crawl` | Documents plus crawlberg and URL handling. |
| `comms` | Agent broker, workspace registry, daemon-hosted MCP and optional streamable HTTP; daemon surfaces target Unix/Windows. |
| `shells` | rmux-backed detached shell service on Unix/Windows. |
| `code-intel-js` | oxc-based JavaScript/TypeScript resolution. |
| `code-intel-stack` | In-tree stack-graph engines for Python/Java. |
| `code-intel` | Both precise resolution engines. |
| `ner-candle` | Documents plus Candle GLiNER2 NER and model-tokenizer support. |
| `full` | Documents, memory, crawl, comms, shells, code intelligence, code search and Candle NER. **Does not include `graphrag`.** |
| `hacienda` | Documents, memory, code search, Candle NER and comms for gateway integration. |
| `graphrag` | Optional entity graph dependencies and partially implemented query surface; see limitations below. |
| `ort-dynamic` | Dynamic ONNX Runtime loading through xberg when that dependency is enabled. |
| `test-support` | Test helper dependency support. |

## Capability map

| Capability | Behavior and entry points | Implementation / constraints |
| --- | --- | --- |
| Full and incremental indexing | `scan`, `rescan`, `watch`; working tree, staged tree and revision views. Hash/metadata reuse, ignore rules, pruning, cancellation and filesystem refresh. | `scanner*.rs`, `watcher.rs`, `store*.rs`. Document extraction is on the working-tree path. |
| Language support | Grammar discovery/install/cleanup, language overrides, pooled parsers and extraction query overrides. | `lang.rs`, `lang_rules.rs`, `queries/`, `cli`/binary language commands. Grammar availability and query quality differ by language. |
| Structural code navigation | `code` outline, symbols, grep, files, find, references, callers, implementations, dependents, expand. | `extract/`, `index/`, `mcp/helpers_code.rs`, `helpers_calls.rs`, `helpers_grep.rs`, `helpers_files.rs`, `helpers_impls.rs`. |
| Definition resolution | `code definition`; scope and import resolution where supported. | `intel/`, `extract/locals.rs`. Name-based reference search is broader than precise resolution; preserve that distinction in result descriptions. |
| Semantic code search | `code semantic` returns chunk pointers; `code chunk` fetches source. Vector, BM25 and exact lanes can be fused/reranked. | `code-search`; `chunk.rs`, `scanner_code.rs`, `search/`, `mcp/helpers_code_search.rs`. |
| Code graph analysis | Calls, neighbors, paths, bounded subgraphs, communities, architecture map, centrality, churn and dependency cycles. | `mcp/codegraph.rs`, `traverse.rs`, `community.rs`, `helpers_archmap.rs`; provenance/confidence belong to edges. |
| Graph visualization | `graph export`, `display`, `open`; JSON/DOT/Mermaid/GraphML/Cypher/HTML/SVG renderers. | `mcp/graph_view.rs`, `graph_html.rs`, `graph_svg.rs`. Live UI depends on opt-in daemon HTTP; file export is a separate path. |
| Git intelligence | Status, recent commits, path history, churn, textual/structural diff, blame, symbol history, commit search and `show`. | `git/`, `git_cache.rs`, `git_history/`, `mcp/helpers_git*.rs`; gix-backed live access plus cached/precomputed history. |
| Document indexing | PDF/Office/HTML/email/images and prose/config/data; extraction, OCR, chunking, headings, optional keywords/entities/summary and code links. | `documents`; `scanner_docs.rs`, `extract/doc*`, `lance/`, `scanner_doc_links.rs`. Bounds, MIME policy and enrichment settings matter. |
| Document retrieval | `memory documents`; exact, keyword/FTS, facet and optional vector lanes, fusion, filters and reranking. | `mcp/helpers_documents.rs`, `mcp/memory.rs`, `lance/fts.rs`, `config/documents/retrieval.rs`. Lexical retrieval does not require vectors. |
| Shared and private agent memory | Put/get/list/search/delete, scoped visibility, audit and stale-entry handling. | `memory`; `mcp/memory_ops.rs`, `helpers_memory.rs`, `comms/memory_proto.rs`, Lance enrichment. Shared scope and workspace path identity are different concepts. |
| Memory governance | Mine co-change evidence from git, list proposals, accept/reject and audit. | `memory`; `helpers_governance.rs`, `helpers_proposals.rs`, `proposals_ops.rs`, `index/keys_governance.rs`. Mining produces proposals rather than silently accepting notes. |
| Web ingestion | `web scrape`, `crawl`, `map`; scraped content joins document retrieval. | `crawl`; `web/`, `url.rs`, `helpers_web.rs`. URL/private-host policy is enforced at the boundary. |
| Agent coordination | Identity registration, scoped thread discovery/membership, posts, front-matter inbox/history, body fetch, acknowledgements, waits, retention and health. | `comms`; `comms/`, `mcp/helpers_comms*.rs`, `cli/agents.rs`. Body reads use `message`; posts support idempotency keys. |
| Workspace coordination | Registered workspaces, worktrees, branches and claims. | `comms`; `registry/`, `cli/registry.rs`, `helpers_registry.rs`. Claims are advisory coordination, not filesystem exclusion. |
| Detached shells | Spawn/send/capture/kill/list/broadcast; retained output and optional visual attach. | `shells`; `shells/`, `cli/shell.rs`, `helpers_shells.rs`, `config/shells.rs`. |
| Privacy engine | `redact_text` MCP and `redact` CLI; pattern/custom-term redaction, pseudonyms, PII labels and optional NER. | `documents`; `pii/`, `tools_redact.rs`, `ner_windowing.rs`, `cli/redact.rs`. `require_ner` rejects fallback or incomplete NER. |
| Vault operations | Encrypt/decrypt/find/forget/inspect rehydration maps in memory. | `documents`; `tools_vault.rs`, `helpers_vault.rs`, `cli/vault.rs`. Caller owns blob persistence and passphrase management. |
| Protected workspaces | `safe/` mirror restricts tools, shared memory and ordinary resources; safe files have a checked resource path. | `protected.rs`, `server_handler.rs`, `safe_resources.rs`; [privacy boundary](../ARCHITECTURE.md#privacy-and-protected-workspaces). |
| Cache lifecycle | Stats, GC, component clearing, cross-workspace liveness, orphan cleanup and schema reset. | `store_gc*.rs`, `store_cache_admin.rs`, `cli/admin.rs`. MCP `admin gc` reports; CLI `cache gc` performs the sweep. Global blobs are shared. |
| Context economy | Compression, delta, checkpoint, waste analysis, token counting and savings telemetry. | `textcompress/`, `mcp/helpers_compress.rs`, `helpers_telemetry.rs`, `savings.rs`; real token counting needs `tokenizer`. |
| Retrieval evaluation | `admin eval` consumes JSONL tasks, measures retrieval/cost and compares baselines. | `eval/`, [`benchmarks/eval/README.md`](../../benchmarks/eval/README.md). |
| Onboarding/operations | `init`, `doctor`, `status`, `statusline`, completion/man generation, CPU detection and pre-commit scan hook. | `cli/init*.rs`, `cli/hook.rs`, `doctor_cli.rs`, `main.rs`, `comms_cli.rs`. |

With `documents` compiled in, prose/data/config grammars are routed away from the
code map into the document tier. With it compiled out, recognized grammars keep
their code-map route. See the compile-time branches in
[`scanner_file.rs`](../../src/scanner_file.rs); do not promise identical file
populations across the two builds.

## MCP and CLI contracts

The main domain tools use a required `mode`: `code`, `graph`, `git`, `memory`,
`admin`, and feature-gated `web`, `agents`, `workspace`, `shell`, `vault`.
`redact_text` is a separate feature-gated tool with direct parameters. Router
assembly and protected-workspace policy determine the visible tool list.

MCP uses underscore mode spellings such as `blame_symbol`; CLI commands generally
use hyphens. Some administration verbs are top-level CLI commands: `status`,
`rescan`, `cache`, `delta`, `checkpoint`, `detect-waste`. The parity table and its
explicit exceptions live in [`tests/cli_parity/`](../../tests/cli_parity/).

MCP also offers resources, prompts, completions, progress notifications and
task-backed slow calls. Ordinary resource URIs include `basemind://status`,
`basemind://repo/map`, `basemind://outline/{path}`, and `basemind://memory/{key}`.
Task/cancellation eligibility is explicit in `mcp/tasks.rs`; durable writes must
not inherit the cancellation policy for disposable reads.

## Incomplete and proposed work

These are implementation observations, not completed-feature claims:

- **GraphRAG:** `lance/graphrag.rs` contains entity/edge construction and restricted
  Cypher support. `graph entities` has an MCP handler, but `run_graph_rag` in
  `mcp/helpers_graph.rs` returns empty hits when enabled. `src/cli/graph.rs` has
  no `entities`/`rag` subcommands. Entity graph ingestion is configured under
  `[documents.graphrag]` (`config/documents.rs`). Treat the retrieval and CLI
  surface as incomplete; `full` does not exercise this feature.
- **Desktop UI:** the Tauri ADR is proposed. The implemented graph UI lives in
  HTML/SVG rendering and the daemon HTTP frontend; there is no Tauri application
  package in this checkout.
- **Schema verification:** the snapshot comparison in `tests/config_schema.rs`
  is currently ignored with a drift notice. A green ordinary test run does not
  establish that the snapshot matches the current types.
- **Historical prose:** several ai-rulez contexts still describe nine tools,
  old cache paths, or per-file commits. Use current router, layout and batching
  code when working in those areas.

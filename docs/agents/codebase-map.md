# Codebase map

Use this map to find implementation ownership, then read only the relevant
modules. [Architecture](../ARCHITECTURE.md) explains the data flows;
[features](features.md) connects them to public capabilities.

## Repository structure

| Path | Responsibility |
| --- | --- |
| [`src/`](../../src/) | Rust library, executable, CLI, MCP server, storage, scanner, and daemon. |
| [`crates/`](../../crates/) | In-tree stack-graph dependency forks; workspace members, not independent Basemind services. |
| [`tests/`](../../tests/) | Integration, protocol, CLI, security, resource, upgrade, and parity contracts; synthetic fixtures. |
| [`benches/`](../../benches/), [`benchmarks/`](../../benchmarks/), [`examples/`](../../examples/) | Criterion benches, retrieval evaluation corpora, benchmark scripts and Rust examples. |
| [`docs/`](../) | Architecture, ADRs, implementation specs, upgrades, agent guide, demos, and systemd example. |
| [`website/`](../../website/) | Astro/Starlight user documentation and static website. |
| [`.ai-rulez/`](../../.ai-rulez/) | Repository rules, contexts, development skills, agent definitions, and generation config. |
| [`skills/`](../../skills/), [`commands/`](../../commands/), [`hooks/`](../../hooks/) | Canonical product skills, slash-command content, and harness hooks. |
| [`.claude-plugin/`](../../.claude-plugin/), [`.codex-plugin/`](../../.codex-plugin/), [`.cursor-plugin/`](../../.cursor-plugin/) | Harness-specific plugin metadata and bundled content. |
| [`opencode-plugin/`](../../opencode-plugin/), [`.pi/`](../../.pi/) | OpenCode plugin/TUI and Pi extension; packaging shares canonical skills. |
| [`pip-package-hermes/`](../../pip-package-hermes/), [`.hermes/`](../../.hermes/) | Hermes source/tests and generated installable plugin packages. |
| [`npm-package/`](../../npm-package/), [`pip-package/`](../../pip-package/) | Binary download/launch wrappers for npm and PyPI. |
| [`scripts/`](../../scripts/), [`.github/`](../../.github/) | Launchers, synchronization, packaging, release, CI, and hardening automation. |
| [`schema/`](../../schema/), [`build.rs`](../../build.rs) | Derived config schema and Cargo rebuild triggers for schema/query changes. |
| [`Taskfile.yaml`](../../Taskfile.yaml), [`poly.toml`](../../poly.toml), [`deny.toml`](../../deny.toml), [`rustfmt.toml`](../../rustfmt.toml) | Task, lint/hook, dependency policy, and formatting configuration. |

`target/`, dependency directories, downloaded binaries, release archives, and
machine cache state are build/runtime artifacts. They are not source modules.

## Entry points and contracts

| Area | Start here | Follow into |
| --- | --- | --- |
| Executable | [`main.rs`](../../src/main.rs) | `scan_args.rs`, `comms_cli.rs`, `doctor_cli.rs`, `lang_cli.rs`, `stdio_relay.rs`. |
| CLI domains | [`cli/mod.rs`](../../src/cli/mod.rs) | `code`, `git`, `graph`, `memory`, `web`, `admin`, `agents`, `registry`, `shell`, `redact`, `vault`. |
| CLI context/output | [`cli/context.rs`](../../src/cli/context.rs) | `cli/render.rs`, `cli/exit.rs`, `cli/interrupt.rs`, `render.rs`. |
| MCP registration | [`mcp/mod.rs`](../../src/mcp/mod.rs) | `router_cache.rs`, `server_handler.rs`, `mode.rs`. |
| MCP request path | [`mcp/tools.rs`](../../src/mcp/tools.rs) | `tools_<area>.rs` → `helpers_<area>.rs`; schemas/results in `types_<area>.rs`. |
| MCP state | [`mcp/shared_state.rs`](../../src/mcp/shared_state.rs) | `state.rs`, `host.rs`, `daemon_forward.rs`, `index_route.rs`, `index_read.rs`. |
| MCP protocol support | [`mcp/server_handler.rs`](../../src/mcp/server_handler.rs) | `resources.rs`, `safe_resources.rs`, `prompts.rs`, `completions.rs`, `tasks.rs`, `notifications.rs`. |
| MCP admission/background work | [`mcp/admission.rs`](../../src/mcp/admission.rs) | `background.rs`, `identity.rs`; heavy-call permits, startup work, watchers and per-session identity. |
| Response size/cost | [`mcp/budget.rs`](../../src/mcp/budget.rs) | `cursor.rs`, `lean.rs`, `toon.rs`, `lenient.rs`, `kneedle.rs`, `tokens.rs`, `savings.rs`, `telemetry.rs`. |

File names sometimes predate public domain names: `tools_comms.rs` serves
`agents`, `tools_registry.rs` serves `workspace`, and `tools_shells.rs` serves
`shell`. Search by both the public name and its implementation owner.

## Scanning and extraction

| Area | Implementation owners |
| --- | --- |
| Full/incremental scan | [`scanner.rs`](../../src/scanner.rs), `scanner_scope.rs`, `scanner_drive.rs`, `chunk_drive.rs`. |
| Enumeration and filtering | `scanner_candidates.rs`, `scanner_filter.rs`, `config/rules.rs`, `lang_rules.rs`, `path.rs`. |
| Per-file extraction and batching | [`scanner_file.rs`](../../src/scanner_file.rs), `scanner_index_batch.rs`. |
| Language/parser/query lookup | [`lang.rs`](../../src/lang.rs), [`queries/`](../../src/queries/). |
| L1/L2/L3 extraction | [`extract/`](../../src/extract/): `l1.rs` outlines, `l2.rs` calls, `l3.rs` structural body hashes, `rationale.rs` rationale nodes. |
| Scope-aware navigation | [`intel/`](../../src/intel/): `resolve.rs`, `resolve_pass.rs`, `resolver.rs`, `xfile.rs`, `js.rs`, `stackgraph.rs`; grammar-native fallback in `extract/locals.rs`. |
| Document extraction | `scanner_docs.rs`, `scanner_doc_lane.rs`, `extract/doc.rs`, `extract/doc/`, `extract/doc_fallback.rs`, `extract/doc_guard.rs`, `extract/doc_cost.rs`, `extract/doc_metrics.rs`. |
| Optional enrichment | `scanner_lanes.rs`, `scanner_code.rs`, `scanner_doc_links.rs`, `scanner_policy.rs`, `chunk.rs`, `embeddings.rs`. |
| Resource bounds and evidence | [`backpressure.rs`](../../src/backpressure.rs), `sysres.rs`, `sysres_cgroup.rs`, `scan_metrics.rs`, `scan_evidence.rs`, `alloc_tag.rs`. |
| Filesystem freshness | [`watcher.rs`](../../src/watcher.rs), `cli/rescan.rs`. |

## Storage and querying

| Area | Implementation owners |
| --- | --- |
| Store facade and workspace/views | [`store.rs`](../../src/store.rs), `store_layout.rs`, `store_lock.rs`, `store_seed.rs`. |
| Content-addressed blob I/O | `store_blob.rs`, `store_blob_codec.rs`, `hashing.rs`. |
| Fjall indexes | [`index/mod.rs`](../../src/index/mod.rs), `index/writer.rs`, `index/keys.rs`, `index/name_dict.rs`, `index/grep_bloom.rs`. |
| Governance/PII keys | `index/keys_governance.rs`, `index/keys_pii.rs`, `index/pii_lineage.rs`. |
| GC and cache administration | `store_gc.rs`, `store_gc_live.rs`, `store_gc_budget.rs`, `store_gc_workspace.rs`, `store_gc_protect.rs`, `store_doc_tombstone.rs`, `store_cache_admin.rs`. |
| Read projections | [`query.rs`](../../src/query.rs), `mcp/in_memory.rs`, `mcp/l1_cache.rs`, `mcp/term_index.rs`, `mcp/map_fingerprint.rs`, `mcp/doc_links_cache.rs`. |
| Vector/document tables | [`lance/`](../../src/lance/): `mod.rs`, `schema.rs`, `batch.rs`, `fts.rs`, `doc_links.rs`, optional `graphrag.rs`. |
| Code search ranking | [`search/`](../../src/search/): `bm25.rs`, `bm25_postings.rs`, `exact.rs`, `rrf.rs`; `mcp/helpers_code_search.rs`. |
| Document retrieval | `mcp/helpers_documents.rs`, `mcp/memory.rs`, `config/documents/retrieval.rs`. |
| Code graph | `mcp/codegraph.rs`, `traverse.rs`, `community.rs`, `helpers_archmap.rs`, `helpers_traverse.rs`, `helpers_graph.rs`, `helpers_community.rs`. |
| Graph rendering | `mcp/graph_view.rs`, `graph_html.rs`, `graph_svg.rs`, `helpers_graphview.rs`. |
| Git | [`git/`](../../src/git/), `git_cache.rs`, [`git_history/`](../../src/git_history/), `mcp/helpers_git*.rs`. |
| Memory/proposals | `mcp/memory_ops.rs`, `proposals_ops.rs`, `helpers_memory.rs`, `helpers_proposals.rs`, `helpers_governance.rs`. |

## Daemons, configuration, and boundaries

| Area | Implementation owners |
| --- | --- |
| Broker runtime | [`comms/daemon.rs`](../../src/comms/daemon.rs), `daemon_lifecycle.rs`, `daemon_threads.rs`, `daemon_handlers.rs`, `daemon_forward_handlers.rs`, `daemon_workspace_handlers.rs`. |
| IPC and clients | `comms/protocol.rs`, `transport.rs`, `client.rs`, `client_ops.rs`, `client_forward.rs`, `frontend_uds.rs`, `frontend_named_pipe.rs`, `frontend_inproc.rs`, `relay.rs`. |
| HTTP frontend | `comms/http_frontend.rs`, `http_auth.rs`, `daemon_http.rs`. |
| Workspace ownership | [`comms/workspace_pool.rs`](../../src/comms/workspace_pool.rs), `mcp/host.rs`, `comms/index_read_proto.rs`, other `*_proto.rs` files. |
| Daemon lifecycle/health | `daemon_lock.rs`, `comms/singleton*.rs`, `daemon_health.rs`, `daemon_guards.rs`, `daemon_store_blocking.rs`, `store_health.rs`. |
| Agent threads and delivery | `comms/model.rs`, `store.rs`, `store/idempotency.rs`, `store/retention.rs`, `identity.rs`, `scope.rs`, `ids.rs`, `cursor.rs`. |
| Worktree registry | [`registry/mod.rs`](../../src/registry/mod.rs), `cli/registry.rs`, `mcp/helpers_registry.rs`. |
| Detached shells | [`shells/`](../../src/shells/): `daemon.rs`, `launcher.rs`, `session.rs`, `attach.rs`. |
| Config loading | [`config/mod.rs`](../../src/config/mod.rs), `v1.rs`, `validate.rs`, `layered.rs`, `source.rs`, `overrides.rs`, `daemon.rs`. |
| Config subtrees | `config/code.rs`, `documents.rs`, `documents/`, `documents_validate.rs`, `resources.rs`, `enrichment.rs`, `comms.rs`, `shells.rs`. |
| Root/repository trust | `config/root_guard.rs`, `config/trust.rs`, `url.rs`, `protected.rs`. |
| Privacy | [`pii.rs`](../../src/pii.rs), [`pii/`](../../src/pii/), `mcp/tools_redact.rs`, `ner_windowing.rs`, `helpers_vault.rs`, `safe_resources.rs`. |
| Web ingestion | [`web/`](../../src/web/): `engine.rs`, `ingest.rs`; `mcp/helpers_web.rs`. |
| Context compression | [`textcompress/`](../../src/textcompress/): format detection/handlers, safety, delta, checkpoint, waste analysis, CLI. |
| Retrieval evaluation | [`eval/`](../../src/eval/): task parsing, extraction, running, scoring, baseline gates, reporting. |
| Onboarding | `cli/init.rs`, `init_rules.rs`, `init_settings.rs`, `init_gitignore.rs`, `cli/hook.rs`. |
| Version compatibility | [`version.rs`](../../src/version.rs), [`docs/UPGRADING.md`](../UPGRADING.md). |

## Workspace dependency forks

`crates/lsp-positions` supplies text positions; `crates/stack-graphs` supplies the
graph model and path resolution; `crates/tree-sitter-graph` evaluates graph rules;
`crates/tree-sitter-stack-graphs` integrates parsing and graph construction.
Basemind-specific Python/Java rules live in `src/intel/tsg/`. Preserve fork
attributions and licenses when changing these crates; their manifests and tests
define their compatibility requirements.

# Development workflow

Read [`CONTRIBUTING.md`](../../CONTRIBUTING.md) for contributor conventions and
[`.ai-rulez/rules/`](../../.ai-rulez/rules/index.md) for area-specific rules.
This guide identifies change seams and the checks that exercise them.

## Setup and validation

The core uses stable Rust, edition 2024. Task and poly provide the repository
workflow. `task setup` fetches Cargo dependencies and installs poly hooks;
`task check` runs lint and workspace tests, including Hermes validation through
the task definitions. It is broader than `cargo test` alone.

For Rust changes, the standard checks are:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --tests -- -D warnings
cargo test --workspace --no-fail-fast
```

Default features are empty. Also test the affected feature configuration, for
example:

```sh
cargo test --features comms --test serve_relay_smoke
cargo test --features code-search --test code_search_smoke
cargo test --features documents --test document_fts_smoke
cargo test --features full --test mcp_smoke --test mcp_schema_wire --test cli_parity
```

Feature builds may need native libraries, protobuf tooling and model artifacts.
Use [CI](../../.github/workflows/ci.yaml) and its
[system dependencies action](../../.github/actions/install-system-deps/action.yml)
for the current platform setup. CI covers base/full builds on Linux/macOS and a
`comms shells` Windows leg; publishing has additional platform/runtime checks.
`--all-features` is not equivalent to the release `full` feature set.

For documentation-only changes, check links, Markdown formatting and
`git diff --check`. Build the website when changing its MDX/configuration. Report
which checks actually ran and any unavailable checks; changing docs does not
by itself require building the native document stack.

## Change recipes

### Add or change a public operation

1. Locate the existing domain's mode enum, flat parameter type, tool shim and
   helper using the [codebase map](codebase-map.md).
2. Extend `mode.rs` and `types_<area>.rs`; keep the mode schema a flat string enum
   and fields optional siblings. Match per-mode required-field validation and
   reject fields belonging to other modes.
3. Implement the behavior in helpers and expose the matching clap command in
   `src/cli/`. Update `tests/cli_parity/capabilities.rs` and document intentional
   differences in its exceptions file.
4. Test the observable behavior, schema advertisement and error cases. Update
   the corresponding product docs/skills when their usage contract changes.

Follow the [MCP checklist](../../.ai-rulez/skills/mcp-tool-checklist/SKILL.md),
reading its nine-domain wording in the context of the current router, which
also contains the privacy tools. Preserve existing top-level tool names.
Completion means the relevant CLI/MCP contract and feature-gated build are
accounted for, not merely that a new handler exists.

### Change scanning, extraction, or persistence

Trace full scan, incremental scan, deletion, tier changes and cancellation.
Keep file results and index staging bounded, use the parser pool, and preserve
the code-map persist barrier before optional enrichment. Same-file replacement
must remove stale secondary rows atomically with insertion.

Use [`docs/UPGRADING.md`](../UPGRADING.md) for new persisted components. Blob and
index compatibility derive from `RELEASE_MINOR` in `src/version.rs`; patch-level
extraction behavior invalidation uses `EXTRACT_EPOCH`. Account for cache
clear/stats, GC liveness and upgrade cleanup along with serialization.

For language changes, follow the
[language support checklist](../../.ai-rulez/skills/language-support/SKILL.md).
Queries in `src/queries/` override the language-pack fallback; prose/data grammars
also need the tier-routing decision in `lang::is_non_code`.

### Change configuration

Update the Rust config type/defaults, validation, trust handling, and any
document override/provenance entries. The layer merger covers selected document
settings; adding a field does not automatically create CLI/MCP/env overrides.
Check root discovery and hot daemon config refresh when relevant.

Regenerate the schema from Rust, with intelligence enabled:

```sh
cargo test --features full --test config_schema -- --ignored regenerate_schema
cargo test --features full --test config_schema -- --ignored schema_snapshot_matches_derived
```

The snapshot comparison is currently ignored; invoking it explicitly is needed
when validating schema synchronization. Review the generated diff and run
`config_schema`, `config_load`, `config_trust` and the relevant override tests.

### Change daemon or privacy behavior

Exercise both hosted and forwarded paths. Keep synchronous store work off the
async transport path, preserve writer ownership, and avoid daemon self-RPC when
the host seam can reach the pool directly. Check reconnection, request identity,
idempotency and workspace/view isolation for lifecycle changes.

For redaction, resource or memory changes, exercise protected `safe/` workspaces
as well as ordinary roots. Preserve tool withholding, path containment and
caller-owned rehydration. Read the
[privacy ADR](../adr/0015-privacy-gateway-boundary.md) before changing that boundary.

## Test map

| Changed contract | Useful tests / harness |
| --- | --- |
| Public CLI/MCP | `cli_smoke`, `cli_output`, `mcp_smoke`, `mcp_schema_wire`, `cli_parity`, `tasks_smoke`, `mcp_resources`. |
| Scan/filter/incremental/durability | `scan_smoke`, `scan_paths_incremental`, `scan_pruning_smoke`, `config_scan_rules`, `index_write_batching`, `scan_durability_smoke`. |
| Memory/resource bounds | `scan_memory_budget`, `scan_drive_bounds_smoke`, `doc_memory_bounds`, `doc_extraction_bounds`, `doc_abandoned_cap`, `embed_streaming_smoke`, `embed_pool_reentrancy`. |
| Language/navigation/search | `lang_fallback_smoke`, `code_intel_smoke`, `precise_resolution_smoke`, `incremental_scope_resolve_smoke`, `goto_definition_xfile_smoke`, `code_search_smoke`, `grep_bloom_scan`. |
| Document retrieval/routing | `document_fts_smoke`, `non_code_routing`, `doc_scope_consistency_smoke`, `doc_text_fallback`, `scan_policy_retry`; module tests in `extract/doc/` and `lance/`. |
| Git | `git_smoke`, `git_parity`, `git_cache_smoke`, `git_history_*`, `git_body_bound`. |
| Broker/transport | `comms_smoke`, `daemon_integration`, `daemon_singleton_smoke`, `serve_relay_smoke`, `serve_contention_smoke`, `multisession_smoke`, `post_idempotency`, `http_frontend*_smoke`. |
| Scope/trust/privacy | `config_trust`, `root_guard_smoke`, `extra_roots_containment_smoke`, `mcp_protected_surface`, `mcp_safe_resources`, `pii_patterns_smoke`. |
| Memory/registry/shells | `governance_smoke`, `agent_identity_smoke`, `registry_prune_smoke`, `worktree_smoke`, `shells_smoke`. |
| Persistence upgrades | `schema_bump`, `cache_paths`, `upgrade_cleanup`, `upgrade_from_release`; module tests in `store_gc/`, `index/`. |
| Integrations/release | `init_smoke`, `plugin_manifest_smoke`, `publish_workflow_smoke`, `ci_workflow_smoke`, `tests/statusline_smoke.sh`, Python package tests. |
| Retrieval quality/performance | `eval_smoke`, `benches/`, `benchmarks/eval/`, `scripts/bench.sh`, `scripts/harden.sh`. |

These are Cargo integration-test target names unless a path is shown. Read each
test's feature/ignore attributes before using its name as proof of coverage.
The hardening harness clones real repositories and is explicitly ignored in
ordinary test runs; reserve it for substantive extraction/query/performance work.

## Editing constraints and completion

Keep modules focused and split helpers as they grow. Repository prose targets
1,000 lines for source modules; the current enforced ceiling is the constant in
[`tests/max_lines.rs`](../../tests/max_lines.rs) (1,120 when this guide was written).
The test, not a poly custom rule, implements that check.

For a completed change, account for relevant feature gates, public behavior,
tests, generated artifacts and user-facing docs. Keep generated product copies
in sync via the [integration workflow](integrations.md). Release/version work
uses `task release:sync-version VERSION=...`; ordinary changes do not need a
version bump. User-visible behavior changes get an Unreleased changelog entry.

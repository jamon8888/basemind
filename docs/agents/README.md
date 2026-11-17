# Agent guide

This guide maps the implementation for agents developing Basemind. Start from
[`AGENTS.md`](../../AGENTS.md) in Codex or [`CLAUDE.md`](../../CLAUDE.md) in Claude
Code. Both lead here. Product usage is documented in the root
[`README.md`](../../README.md), [`skills/`](../../skills/), and the
[`website/`](../../website/src/content/docs/).

## Read by task

| Need | Read | Completion criterion |
| --- | --- | --- |
| First orientation | [Codebase map](codebase-map.md) | Identify the entry point, implementation owner, and relevant test family. |
| Runtime or data-flow change | [Architecture](../ARCHITECTURE.md) | Trace the request or scan through its writer, reader, cache, and transport. |
| Feature work | [Feature catalog](features.md) | Identify build gates, runtime settings, and implemented versus incomplete surfaces. |
| Code/config/schema change | [Development workflow](development.md) | Select checks for the changed contract and feature configuration. |
| Agent integration or packaging | [Integrations](integrations.md) | Identify canonical inputs and generated copies before editing. |
| Architectural decision | [ADR index](../adr/README.md) | Read the relevant decision and its status. |
| Issue or triage work | [Issue tracker](issue-tracker.md), [labels](triage-labels.md) | Resolve the repository and use its existing issue/label conventions. |

## Working model

Basemind's root Cargo package exposes a library (`src/lib.rs`) and one executable
(`src/main.rs`). Four workspace crates supply stack-graph infrastructure. The
CLI and MCP share query helpers; the daemon owns active workspace writers and
shares expensive read stacks between sessions. A standalone command can operate
locally when it can acquire the appropriate store ownership.

Code extraction produces content-addressed blobs and Fjall indexes. Documents,
code embeddings, and memory enrichment use optional intelligence components,
including LanceDB. Build features, workspace configuration, and protected-workspace
checks all affect which capabilities can actually run.

## Sources of truth

Use these small lookups instead of assuming a release description is current:

- Build features and workspace members: [`Cargo.toml`](../../Cargo.toml).
- Library modules: [`src/lib.rs`](../../src/lib.rs); executable commands:
  [`src/main.rs`](../../src/main.rs) and [`src/cli/`](../../src/cli/).
- MCP registration: `BasemindServer::assemble_router` in
  [`src/mcp/mod.rs`](../../src/mcp/mod.rs). Domain mode vocabulary:
  [`src/mcp/mode.rs`](../../src/mcp/mode.rs). Actual visibility checks:
  [`server_handler.rs`](../../src/mcp/server_handler.rs).
- Config model: [`src/config/v1.rs`](../../src/config/v1.rs) and sub-configs;
  validation snapshot: [`schema/`](../../schema/).
- Verification commands: [`Taskfile.yaml`](../../Taskfile.yaml),
  [`poly.toml`](../../poly.toml), and [CI](../../.github/workflows/ci.yaml).

The guides describe the checked-in implementation, including known gaps; they
are not promises that every feature configuration has passed a build. In
particular, `full` does not include `graphrag`, and its retrieval/CLI surface is
incomplete. See [feature status](features.md#incomplete-and-proposed-work).

## Maintaining these docs

Update the feature catalog when behavior or feature gates change; update the
codebase map when ownership moves; update architecture when data flow or
invariants change. Keep command syntax in the source/CLI help and user reference
instead of duplicating every flag here. Check relative links after moving files.

The entry points are maintained files. `.gitignore` explicitly re-includes them
outside the ai-rulez managed block. The ai-rulez
[crate-layout context](../../.ai-rulez/context/crate-layout.md) points here too;
review diffs if regenerating harness configuration replaces an entry point.

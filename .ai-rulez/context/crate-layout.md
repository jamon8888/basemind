---
type: Concept
title: Crate Layout
x-ai-rulez:
  kind: context
  id: crate-layout
  metadata:
    priority: high
---

# Crate Layout

When locating a module, tracing a feature, or changing repository structure, read
[the shared agent guide](../../docs/agents/README.md). It routes to:

- [Codebase map](../../docs/agents/codebase-map.md): source ownership, workspace
  crates, tests, integrations, tooling and packaging.
- [Architecture](../../docs/ARCHITECTURE.md): writer ownership, scan pipeline,
  storage, daemon, MCP and privacy boundaries.
- [Feature catalog](../../docs/agents/features.md): public capabilities, build
  gates, runtime constraints and incomplete surfaces.
- [Development workflow](../../docs/agents/development.md): change recipes and
  validation targets.
- [Integrations](../../docs/agents/integrations.md): canonical versus generated
  content and Claude Code/Codex entry points.

Keep the structural inventory in those shared documents so harness generation
reuses the same map instead of maintaining another copy here. For current module
exports and build gates, inspect `src/lib.rs` and `Cargo.toml`.

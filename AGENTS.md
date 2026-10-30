# Agent instructions

Basemind is a Rust library and CLI providing code navigation, document retrieval,
git intelligence, memory, and agent coordination over MCP. This file is the shared
repository entry point for Codex and Claude Code; `CLAUDE.md` imports it.

## Start here

Read [the agent guide](docs/agents/README.md) when entering this codebase. Then load
the reference for the area being changed:

| Task | Reference |
| --- | --- |
| Locate modules or trace ownership | [Codebase map](docs/agents/codebase-map.md) |
| Understand runtime, indexing, persistence, or privacy | [Architecture](docs/ARCHITECTURE.md) |
| Find a capability, build gate, or implementation limitation | [Feature catalog](docs/agents/features.md) |
| Implement and validate a change | [Development workflow](docs/agents/development.md) |
| Change Claude Code, Codex, other plugins, or distribution | [Integrations](docs/agents/integrations.md) |

## Repository conventions

- Read [CONTRIBUTING.md](CONTRIBUTING.md) and the relevant
  [.ai-rulez rules](.ai-rulez/rules/index.md) before changing code. Keep MCP shims
  thin, preserve bounded scan work and the code-map durability barrier, and test
  the feature set that contains the changed behavior.
- Use [GitHub Issues through `gh`](docs/agents/issue-tracker.md) for issues/specs.
  Use the five [canonical triage labels](docs/agents/triage-labels.md) as written.
- Follow the [domain documentation convention](docs/agents/domain.md): one root
  `GLOSSARY.md`, when present, and [ADRs](docs/adr/README.md). Read relevant ADRs;
  distinguish proposed decisions from implemented behavior.
- Keep shared guidance in `docs/agents/`; maintain the entry points as links.
  Configuration types, Cargo feature gates, router assembly, and tests are the
  authority when an older prose description disagrees with the code.

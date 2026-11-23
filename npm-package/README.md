# basemind

The context and communication layer for coding agents — a shared code-map, document RAG, memory,
web crawl, git history, and agent-to-agent comms so multiple agents coordinate while they work.
300+ languages, one MCP server.

<!-- markdownlint-disable-next-line MD013 -->
[![Docs](https://img.shields.io/badge/docs-basemind.ai-965aff)](https://basemind.ai)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/Goldziher/basemind/blob/main/LICENSE)
[![npm](https://img.shields.io/npm/v/basemind.svg)](https://www.npmjs.com/package/basemind)

Full documentation: **[basemind.ai](https://basemind.ai)**

## Install

```bash
pnpm add -g basemind
```

This legacy name installs the **full** profile by default. To install the
confidential `hacienda-legal` profile (documents + memory + NER + GraphRAG, no
code/comms/crawl tools), use the explicit name or the explicit choice:

```bash
BASEMIND_PROFILE=hacienda-legal pnpm add -g basemind
basemind-hacienda-legal --version   # explicit command name for the same profile
```

The installer selects the profile-specific asset for your OS/architecture
(`basemind-<triple>` or `basemind-hacienda-legal-<triple>`), verifies its
SHA-256 against the release checksums file, and fails with an explicit error if
that asset or its checksum is missing — it never falls back silently. Archives
(`.tar.gz` / `.zip`) and installers (`.exe` / `.dmg`) ship together at the same
version. The Intel macOS asset requires macOS 15 or newer.

## Quickstart

```bash
cd /path/to/your/repo
basemind scan        # index the working tree
basemind serve       # run the MCP stdio server
```

Wire `basemind serve` into Claude Code or any MCP client.

## Full documentation

See the [main README](https://github.com/Goldziher/basemind#readme) for complete docs,
architecture, MCP tool reference, and per-harness setup instructions.

## License

[MIT](https://github.com/Goldziher/basemind/blob/main/LICENSE).

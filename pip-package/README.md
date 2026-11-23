# basemind

The context and communication layer for coding agents — a shared code-map, document RAG, memory,
web crawl, git history, and agent-to-agent comms so multiple agents coordinate while they work.
300+ languages, one MCP server.

<!-- markdownlint-disable-next-line MD013 -->
[![Docs](https://img.shields.io/badge/docs-basemind.ai-965aff)](https://basemind.ai)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/Goldziher/basemind/blob/main/LICENSE)
[![PyPI](https://img.shields.io/pypi/v/basemind.svg)](https://pypi.org/project/basemind/)

Full documentation: **[basemind.ai](https://basemind.ai)**

## Install

```bash
pip install basemind
```

This legacy name installs the **full** profile by default. To install the
confidential `hacienda-legal` profile (documents + memory + NER + GraphRAG, no
code/comms/crawl tools), use the explicit choice or the explicit command name:

```bash
BASEMIND_PROFILE=hacienda-legal pip install basemind
basemind-hacienda-legal --version   # explicit command name for the same profile
```

On first invocation, the profile-specific pre-compiled Rust binary for your
OS/architecture (`basemind-<triple>` or `basemind-hacienda-legal-<triple>`)
is downloaded from [GitHub Releases](https://github.com/Goldziher/basemind/releases),
verified against the SHA-256 entry in the shared checksums file, and cached under
`~/.cache/basemind/<version>/`. A missing asset or checksum fails with an
explicit error naming the profile, triple and version — no silent fallback.
Archives (`.tar.gz` / `.zip`) and installers (`.exe` / `.dmg`) ship together at
the same version; the Intel macOS asset requires macOS 15 or newer.

Override the binary location with `BASEMIND_BINARY=/path/to/basemind`.

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

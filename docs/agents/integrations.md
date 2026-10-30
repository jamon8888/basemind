# Agent integrations and distribution

There are two documentation surfaces: repository instructions for agents
developing Basemind, and product skills/hooks shipped to agents using Basemind.
Keep their audiences and canonical sources explicit.

## Claude Code and Codex development entry points

[`AGENTS.md`](../../AGENTS.md) is the shared maintained entry point.
[`CLAUDE.md`](../../CLAUDE.md) imports it with `@AGENTS.md`; the detailed guides
live in this directory. Both files are explicitly re-included by `.gitignore`.

The repository also has ai-rulez configuration with `claude` and `codex` presets.
Its rules, contexts and development skills live in [`.ai-rulez/`](../../.ai-rulez/).
The [crate-layout context](../../.ai-rulez/context/crate-layout.md) links to the
shared guide. Harness generation can rewrite entry files, so inspect generated
diffs and retain the shared guide and existing issue/domain instructions.

## Product adapters

| Harness / package | Inputs and entry points | Role |
| --- | --- | --- |
| Claude Code | [`.claude-plugin/plugin.json`](../../.claude-plugin/plugin.json), marketplace metadata, `scripts/mcp-launch.sh`, root `skills/`, `commands/`, `hooks/` | Plugin declares a stdio server using `serve` and passes the project root. |
| Codex | [`.codex-plugin/plugin.json`](../../.codex-plugin/plugin.json), root `.mcp.json`, [`scripts/codex-mcp-launch.mjs`](../../scripts/codex-mcp-launch.mjs) | Codex launcher resolves a release and delegates to the shared launcher; plugin copies include skills/commands/hooks. |
| Cursor | [`.cursor-plugin/`](../../.cursor-plugin/), shared launcher and copied content | Harness-specific packaging around the same server. |
| OpenCode | [`opencode-plugin/basemind.js`](../../opencode-plugin/basemind.js), `tui.js`, `package.json` | Native adapter/TUI with bundled skills and commands. |
| Pi | [`.pi/extensions/basemind.ts`](../../.pi/extensions/basemind.ts), root `package.json` Pi metadata | Native extension, also staged into the combined OpenCode/Pi package. |
| Hermes | [`pip-package-hermes/basemind_hermes_plugin/hermes.py`](../../pip-package-hermes/basemind_hermes_plugin/hermes.py), `.ai-rulez/config.toml` | Canonical adapter source used to generate `.hermes/plugins/basemind/` and `.hermes/package/`. |
| Gemini / Kimi / Agent Plugins | `gemini-extension.json`, `kimi.plugin.json`, root `plugin.json` / `mcp.json` | Additional manifests exposing the same product capabilities. |
| npm binary wrapper | [`npm-package/`](../../npm-package/) | Downloads a release executable and launches it; not the Rust implementation. |
| PyPI binary wrapper | [`pip-package/`](../../pip-package/) | Platform-aware download and CLI entry; separate from the Hermes plugin wheel. |

The Codex manifest refers to `./.mcp.json`. Inspect the staged plugin root and
packaging script when resolving that path: it is not a reason to invent a
`.codex-plugin/mcp.json` source file. This checkout's root `.mcp.json` is local
harness configuration and may contain other servers.

## Canonical versus generated content

| Change | Edit first | Synchronize / verify |
| --- | --- | --- |
| Product skill | Root `skills/<name>/SKILL.md` | `bash scripts/sync-plugin-skills.sh` copies to Codex/Cursor/OpenCode and runs ai-rulez plugin generation. |
| Slash-command text | Root `commands/*.md` | Same sync script. |
| Shared hook logic | Root `hooks/` | Sync script copies selected hook scripts to Codex/Cursor; inspect harness-specific hook manifests separately. |
| Pi implementation | `.pi/extensions/basemind.ts` | Sync script copies it into the OpenCode package's Pi extension tree. |
| Hermes implementation | `pip-package-hermes/basemind_hermes_plugin/hermes.py` | `ai-rulez generate --plugin`; tests in `pip-package-hermes/tests/`. |
| Repository context | `.ai-rulez/context/`, `.ai-rulez/rules/`, shared `docs/agents/` | Review harness generation output; do not fork shared guidance between AGENTS and CLAUDE. |
| Config schema | Rust config types | See [schema workflow](development.md#change-configuration). |
| Version | Root Cargo package version through release script | `task release:sync-version VERSION=X.Y.Z` updates shipped surfaces together. |

`scripts/sync-plugin-skills.sh` requires the `ai-rulez` executable. Its inputs are
root skills/commands, selected hooks, the Pi extension and plugin configuration.
`.ai-rulez-generated.json` records generated plugin outputs. Edit the source and
regenerate affected copies rather than patching a single mirrored file.

## Hooks and launch behavior

`hooks/session-start` and `hooks/inbox-notify` supply session/coordination support.
The root also contains optional `pre-tool-guard`, `read-cache` and
`compress-output` helpers; consult each harness's `hooks.json` and `settings.json`
for what is actually installed. Merely having a hook file does not activate it.

`scripts/mcp-launch.sh` handles executable acquisition and launch.
`basemind serve` connects a stdio client to the shared daemon; it is not a
separate per-client index writer. The Codex-specific launcher has release
resolution logic before invoking the common launcher. Test launch/version
changes across wrappers rather than assuming the host runs a checkout build.

## Packaging and verification

- `task package:plugin` stages the combined OpenCode/Pi package in a temporary
  directory via `scripts/stage-agent-package.sh` and packs it there.
- `task hermes:test` runs Python adapter tests and builds the generated wheel.
- `tests/plugin_manifest_smoke.rs` checks plugin contracts;
  `tests/statusline_smoke.sh` checks statusline behavior. Binary wrapper tests
  also live under `pip-package/tests/`.
- `scripts/package-release.sh` packages compiled binaries/runtime dependencies;
  `scripts/release-dry-run.sh` exercises the host packaging path.
- `.github/workflows/publish.yaml` handles platform builds and registry releases.
  `scripts/sync-to-codex-plugin.sh` is a separate external plugin-repository sync
  workflow; use it only for that distribution task.

## Website and documentation

The website is Astro/Starlight. Edit user pages under
`website/src/content/docs/{start,concepts,capabilities,reference}/`, with styles in
`website/src/styles/`. Use the pinned package manager in `website/package.json`:

```sh
cd website
pnpm install --frozen-lockfile
pnpm run build
```

Root `README.md` is the product overview, `docs/ARCHITECTURE.md` is the engineering
architecture, and `docs/agents/` is the development navigation guide. Update the
surface whose audience needs the change; generated skill copies should continue
to derive from the canonical product skill.

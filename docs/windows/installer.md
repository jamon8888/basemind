# Windows x64 user installers

Basemind ships two per-user Windows installers in addition to the existing release
archives. Both install the **CLI and the stdio MCP server only** — Basemind has no
GUI, so no GUI binary is shipped and no Start Menu shortcut is created.

| Artifact | Profile | Contents |
| --- | --- | --- |
| `basemind-x86_64-pc-windows-msvc-setup.exe` | `full` | code map, git, documents/RAG, memory, comms, shells, stdio MCP |
| `basemind-hacienda-legal-x86_64-pc-windows-msvc-setup.exe` | `hacienda-legal` | documents/RAG, vault, memory, NER, GraphRAG, stdio MCP |

The existing `basemind-x86_64-pc-windows-msvc.zip` is **preserved unchanged**:
`scripts/package-release.sh` still produces it, and the installer is an additional
artifact built from the same staged payload. Nothing in this tooling publishes or
merges anything.

## What the installer does

- Installs to `%LOCALAPPDATA%\Basemind` (`...\Basemind-Hacienda-Legal` for the
  legal profile) with `PrivilegesRequired=lowest` — no administrator rights and no
  machine-wide state.
- Adds the install directory **once** to the *user* `Path` (`HKCU\Environment\Path`)
  and broadcasts `WM_SETTINGCHANGE`, so a newly opened terminal finds `basemind`.
  Uninstall removes exactly that entry.
- Writes a config template to
  `%APPDATA%\Basemind\<profile>\basemind.toml`. The legal profile ships the canonical
  `examples/basemind-hacienda-legal.toml` preset. Basemind reads `basemind.toml` from
  each **project root**, so copy the template there (or run `basemind init`) before
  scanning. An existing template is never overwritten on upgrade.
- Supports unattended installation: `setup.exe /VERYSILENT /NORESTART /DIR=<path>`.

MCP clients point at the installed binary with the `serve` argument, for example:

```json
{ "mcpServers": { "basemind": { "command": "basemind", "args": ["serve"] } } }
```

## Prerequisites (build side)

Building a Windows `.exe` requires a **Windows runner/host** and **Inno Setup 6**
(`ISCC.exe`, preinstalled on GitHub-hosted `windows-2022` images). This repository's
normal Linux/macOS CI does not have either; see *Missing runner* below.

## Build from a checkout

On a Windows x64 host with Rust and Inno Setup 6:

```sh
# 1. Build and package each profile (the same features publish uses).
cargo build --release --features full --bin basemind --target x86_64-pc-windows-msvc
./scripts/package-release.sh x86_64-pc-windows-msvc
cargo build --release --features hacienda-legal --bin basemind --target x86_64-pc-windows-msvc
./scripts/package-release.sh x86_64-pc-windows-msvc hacienda-legal

# 2. Build both installers. The wrapper picks up the two .zip files above as the
#    payload, so each installer carries exactly its own profile's shipped bytes.
./installer/windows/build-installers.sh
```

Both profiles compile to the same `target/<triple>/release/basemind.exe`, so raw
build output always belongs to whichever profile was built last. The wrapper
therefore prefers the two release archives when present; pass `BASEMIND_ZIP_FULL`
/ `BASEMIND_ZIP_LEGAL` explicitly to override, or delete the archives to fall
back to raw build output (only safe when building one installer at a time). The
wrapper fingerprints the archives before and after and fails if either changed.

Outputs land in `dist/installer/` (override with `BASEMIND_INSTALLER_OUT_DIR`).
If `ISCC.exe` is not on `PATH`, set `ISCC` to its absolute path.

## Verify

The `build-installers.sh` wrapper checks that both expected artifacts exist and
the PowerShell builder verifies each is a real PE executable (`MZ` header, sane
size). For the full install → PATH → config → launch → uninstall cycle:

```powershell
powershell -File installer/windows/smoke-test.ps1 `
  -SetupExe dist\installer\basemind-x86_64-pc-windows-msvc-setup.exe `
  -Variant full -ExpectedVersion 0.39.0
```

The smoke test restores the user `Path` and any pre-existing config when it
finishes. `cargo test --test windows_installer_smoke` runs static contract checks
on any platform, including on a non-Windows host where the installers cannot be
built; on Windows, set `BASEMIND_SETUP_EXE` (plus optional
`BASEMIND_SETUP_VARIANT` and `BASEMIND_SETUP_VERSION`) and run the ignored test
`windows_setup_end_to_end_when_provided` to drive the PowerShell smoke test.

## Missing runner / tool

On Linux or macOS, `build-installers.sh` exits with code 2 and names the missing
runner and `ISCC.exe`. The artifacts cannot be produced on this host: Inno Setup
is Windows-only and the `x86_64-pc-windows-msvc` toolchain is not installed here.
Use a Windows x64 runner (for example `actions/runner` `windows-2022`) with Inno
Setup 6, then run the commands above there. This is a build-tool limitation only —
no source-code or feature changes are required.

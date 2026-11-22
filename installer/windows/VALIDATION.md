# Windows installer validation record

This file records what was actually verified for the Windows x64 user installers
and what could not be verified on the development host. It is intentionally part
of the Windows tooling/docs surface; it does not change release code, features or
the publish workflow.

## Host probe

| Requirement | Status on this host |
| --- | --- |
| Host OS | macOS (darwin), `x86_64-apple-darwin` Rust host |
| Windows x64 runner | **MISSING** (no `windows-2022`/Windows host available) |
| Inno Setup 6 (`ISCC.exe`) | **MISSING** (`iscc`, `ISCC.exe` not on PATH) |
| PowerShell (`pwsh` / `powershell`) | **MISSING** |
| `x86_64-pc-windows-msvc` Rust target | **MISSING** (only `aarch64-apple-darwin`, `x86_64-apple-darwin`, `wasm32-unknown-unknown` installed) |
| Wine | **MISSING** (no way to run a Windows installer here) |

Consequence: the two expected PE artifacts
`basemind-x86_64-pc-windows-msvc-setup.exe` and
`basemind-hacienda-legal-x86_64-pc-windows-msvc-setup.exe` **cannot be compiled on
this host**. They are produced by `installer/windows/build-installers.sh` on a
Windows x64 runner with Inno Setup 6 — see `docs/windows/installer.md`.

## Validation actually performed here

| Check | Command | Result |
| --- | --- | --- |
| Static installer contract (5 checks) | `CARGO_MANIFEST_DIR="$PWD" rustc --edition 2024 --test tests/windows_installer_smoke.rs -o /tmp/bm-windows-installer-smoke-test && /tmp/bm-windows-installer-smoke-test` | **PASS** — 5 passed, 0 failed |
| Canonical `cargo test --test windows_installer_smoke` | attempted, but the shared Cargo target directory was saturated by other concurrent builds and the run did not finish on this host | **NOT COMPLETED** — the standalone `rustc --test` invocation above executes the same test binary and did pass |
| Build wrapper shell syntax | `bash -n installer/windows/build-installers.sh` | PASS |
| Whitespace / patch hygiene | `git diff --check` | PASS |
| Scope guard | `git diff --name-only` | no tracked files changed; publish workflow and features untouched |
| Archive preservation | asserted in the shell wrapper (SHA-256 fingerprint before/after) and in the static test over `scripts/package-release.sh` | PASS (static); runtime check runs on Windows |

The static test asserts: both artifacts are named and verified by the build
wrapper; `basemind.iss` uses `PrivilegesRequired=lowest`, `{localappdata}`, an
idempotent `HKCU\Environment\Path` add/remove, a per-profile `%APPDATA%` config
template, only `basemind.exe` (+ DLLs) and no `[Icons]` section; the smoke test
covers install, `--version`, `serve --help`, user PATH, config and uninstall; and
`scripts/package-release.sh` still emits the Windows `.zip`.

## Validation still required on Windows

On a `windows-2022` runner with Inno Setup 6:

```sh
./installer/windows/build-installers.sh
powershell -File installer/windows/smoke-test.ps1 \
  -SetupExe dist\installer\basemind-x86_64-pc-windows-msvc-setup.exe \
  -Variant full -ExpectedVersion 0.39.0
powershell -File installer/windows/smoke-test.ps1 \
  -SetupExe dist\installer\basemind-hacienda-legal-x86_64-pc-windows-msvc-setup.exe \
  -Variant hacienda-legal -ExpectedVersion 0.39.0
```

(or drive the same scripts with the ignored test
`windows_setup_end_to_end_when_provided` after setting `BASEMIND_SETUP_EXE`.)

That run is the only thing that can prove the PE artifacts launch, install
per-user, modify PATH/config, and uninstall cleanly. Until it is executed on a
Windows host, it is **not** claimed as validated here. No publish or merge was
performed.

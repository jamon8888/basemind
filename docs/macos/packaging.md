# macOS packaging: arm64, universal, and notarization

This document describes the macOS-only DMG packaging path for the `full` and
`hacienda-legal` profiles. It is additive to the existing `.tar.gz` release
archives: `scripts/package-release.sh` still produces those, unchanged, and the
DMG packaging script consumes them. Nothing in `.github/workflows/publish.yaml`
or in the Cargo feature composition is part of this change.

## Artifacts

| Profile | DMG | Source archives (preserved) |
| --- | --- | --- |
| `full` | `basemind-aarch64-apple-darwin.dmg` | `basemind-aarch64-apple-darwin.tar.gz`, `basemind-x86_64-apple-darwin.tar.gz` |
| `full` | `basemind-universal-apple-darwin.dmg` | same two archives |
| `hacienda-legal` | `basemind-hacienda-legal-aarch64-apple-darwin.dmg` | `basemind-hacienda-legal-aarch64-apple-darwin.tar.gz`, `basemind-hacienda-legal-x86_64-apple-darwin.tar.gz` |
| `hacienda-legal` | `basemind-hacienda-legal-universal-apple-darwin.dmg` | same two archives |

Every DMG contains a CLI + stdio MCP server installer and **no GUI**: there is
no `.app` bundle, no `.pkg`, and no installer window. The payload is:

```text
/Volumes/Basemind (…)/
├── install.sh              # offline CLI + MCP installer
├── README.txt              # install / CLI / MCP usage
├── basemind                # thin arm64, or universal arm64+x86_64
├── lib/                    # otool-discovered native dylib closure
└── libonnxruntime.dylib    # Intel ort-dynamic builds only
```

`install.sh` copies the executable and its `lib/` closure into
`$PREFIX/lib/basemind/<version>/` (default `PREFIX=$HOME/.local`) and writes two
wrappers to `$PREFIX/bin`:

- `basemind` — the CLI;
- `basemind-mcp` — `basemind serve`, a drop-in stdio MCP server command.

The wrappers exec the real binary so `@loader_path/lib` resolves against the
versioned directory. Nothing is downloaded at install time.

## Build matrix

`scripts/macos/build.sh` builds four thin binaries:

| Profile | Target | Features | `MACOSX_DEPLOYMENT_TARGET` |
| --- | --- | --- | --- |
| `full` | `aarch64-apple-darwin` | `full` | `13.0` |
| `full` | `x86_64-apple-darwin` | `full,ort-dynamic` | `15.0` |
| `hacienda-legal` | `aarch64-apple-darwin` | `hacienda-legal` | `13.0` |
| `hacienda-legal` | `x86_64-apple-darwin` | `hacienda-legal,ort-dynamic` | `15.0` |

Intel uses `ort-dynamic`: there is no static x86_64 ONNX Runtime for macOS, so
`package-release.sh` vendors the last pinned x86_64 dylib (`1.23.2`, minos 13.4)
next to the executable and `ort/load-dynamic` dlopens it at runtime. The arm64
leg links ONNX Runtime statically.

**Intel support starts at macOS 15.** The x86_64 slice is always compiled with
`MACOSX_DEPLOYMENT_TARGET=15.0`, even on a host that runs an older macOS, so the
binary's `LC_BUILD_VERSION` declares `minos=15.0`. The arm64 slice stays at
`13.0`; the effective arm64 floor is the highest bundled dylib minos.

## Architecture and minimum-OS validation

`scripts/macos/validate.sh` runs before every DMG is sealed. It checks:

- exact slice set via `lipo -archs`: thin arm64 DMGs contain only `arm64`;
  universal DMGs contain exactly `arm64` and `x86_64`;
- `LC_BUILD_VERSION` / `LC_VERSION_MIN_MACOSX` minos via `otool -l -arch`:
  - the x86_64 `basemind` binary must declare exactly `15.0`;
  - the arm64 `basemind` binary must declare `<= 13.0` (built with
    `MACOSX_DEPLOYMENT_TARGET=13.0`); the bundle-wide arm64 ceiling is 13.4 so
    the pinned ORT dylib (minos 13.4) is accepted;
  - every bundled native library must declare `<= 15.0` (x86_64) and
    `<= 13.4` (arm64) on whichever slices it carries.

The universal merge `lipo -create`s files present in both staging trees.
`libonnxruntime.dylib` exists only on the Intel side (the arm64 slice has ORT
linked in), so a thin Intel dylib would be copied verbatim. The universal path
replaces it with a **pinned fat ORT dylib** produced by
`scripts/macos/ort-universal.sh`, so every slice can dlopen the same library.

### Pinned universal ONNX Runtime

`scripts/macos/ort-universal.sh` downloads two upstream assets at the same
version (onnxruntime 1.23.2), verifies their SHA256, extracts the native
dylibs, and combines them with `lipo -create`:

| Slice | Pinned asset | SHA256 |
| --- | --- | --- |
| arm64 | `onnxruntime-1.23.2-cp311-cp311-macosx_13_0_arm64.whl` | `6f91d2c9b0965e86827a5ba01531d5b669770b01775b23199565d6c1f136616c` |
| x86_64 | `onnxruntime-1.23.2-cp311-cp311-macosx_13_0_x86_64.whl` | `87d8b6eaf0fbeb6835a60a4265fde7a3b60157cf1b2764773ac47237b4d48612` |

Both wheels contain `onnxruntime/capi/libonnxruntime.1.23.2.dylib`; each
declares `minos=13.4`. The merged dylib was verified as `[x86_64 arm64]` with
`minos=13.4` on both slices and SHA256
`b3756f40bc4bfe743ef1639844a5427c1e4075a1f948e675dd12bd2cdf427422`, which is
below the Intel binary's 15.0 requirement. The PyPI wheels are used because
the matching arm64 GitHub `.tgz` is not published at 1.23.2; the Intel `.tgz`
pinned by `package-release.sh` remains
`d10359e16347b57d9959f7e80a225a5b4a66ed7d7e007274a15cae86836485a6`.

`package.sh` calls this script automatically for each universal DMG. Pass
`--fat-ort FILE` to reuse a previously built dylib, or `--no-fat-ort` to keep
the thin Intel dylib (accepted by the validator because non-binary files may
carry a subset of the bundle's slices).

## Signing, notarization, stapling (no secrets in the repo)

`scripts/macos/notarize.sh` is the only place that touches signing. It reads two
opaque references from the environment and never prints, stores, or forwards a
secret value:

| Variable | Meaning |
| --- | --- |
| `MACOS_SIGNING_IDENTITY` | Developer ID Application identity name from the runner keychain. When unset, files are ad-hoc signed (`-`) for local testing. |
| `MACOS_NOTARY_PROFILE` | Name of a keychain profile created out-of-band with `xcrun notarytool store-credentials <profile> …`. The Apple ID, team ID and app-specific password live only in that keychain entry. |

Order of operations in `scripts/macos/package.sh`:

1. sign every dylib, then the executable inside the staging tree
   (`codesign --force --options runtime --timestamp --sign "$identity"` when a
   Developer ID is configured, otherwise `codesign --force --sign -`);
2. create the read-only HFS+/UDZO DMG with `hdiutil create`;
3. sign the DMG itself with the same identity;
4. `xcrun notarytool submit "$dmg" --keychain-profile "$profile" --wait`;
5. `xcrun stapler staple "$dmg"` and `xcrun stapler validate "$dmg"`.

If `MACOS_NOTARY_PROFILE` is unset, steps 4–5 are skipped and the script says so
explicitly. `--ad-hoc` forces ad-hoc signing and disables notarization. Ad-hoc
signatures are valid for local execution but are **not** accepted by
Gatekeeper/notarization; a release DMG must use a Developer ID and a keychain
profile.

`scripts/macos/notarize.sh status` prints which references are configured
(names only) so a release log can prove which mode was used without leaking a
credential.

## Release-workflow integration

This change does not edit `publish.yaml`. The existing publish matrix already
builds and uploads the eight thin `.tar.gz` archives (full and `hacienda-legal`
for both Apple triples). A later release change can add a macOS-only job that:

1. downloads those four macOS archives from the release with checksum
   verification;
2. runs `bash scripts/macos/package.sh --skip-tar --variant full` and
   `… --variant hacienda-legal` in the checkout with `MACOS_SIGNING_IDENTITY`
   and `MACOS_NOTARY_PROFILE` injected from CI secrets into the runner keychain;
3. uploads the four named DMGs and runs `mise run package:macos-smoke <dmg>` on
   each before promotion.

The `--skip-tar` flag is what keeps that job from rebuilding the binaries: it
consumes the already-published archives, so the DMGs are byte-for-byte derived
from the release assets.

## Commands and results

```sh
bash scripts/macos/build.sh                 # four thin binaries + arch/minos checks
bash scripts/macos/ort-universal.sh --out dist/libonnxruntime.universal.dylib
bash scripts/macos/package.sh --variant all --out-dir dist \
    --fat-ort dist/libonnxruntime.universal.dylib   # four DMGs + preserved .tar.gz
bash scripts/macos/package.sh --skip-tar --variant all --out-dir dist \
    --fat-ort dist/libonnxruntime.universal.dylib   # release job: reuse published archives
mise run package:macos                      # same, with a fresh build
mise run package:macos-smoke <artifact.dmg> # mount, validate, install, run
bash scripts/macos/notarize.sh status       # show configured keychain references
```

See the worker report in the release task for the observed command output on
the local runner and for the notarization limits described below.

## Verification performed on the local runner

- `bash scripts/macos/ort-universal.sh --out …` — downloaded and verified both
  pinned wheels, produced `[x86_64 arm64]` dylib with `minos=13.4` on both
  slices; `validate.sh arch` and `validate.sh minos le 15.0` passed.
- `bash scripts/macos/package.sh --skip-tar --variant full --out-dir …` with
  checksum-neutral synthetic arm64/x86_64 fixtures (minos 13.0 / 15.0) — both
  DMGs created, ad-hoc signed; `validate.sh bundle arm64` and
  `validate.sh bundle universal` passed; `lipo` merged one file.
- `bash tests/macos_dmg_smoke.sh …` on both fixture DMGs — mount, payload,
  no-GUI check, arch/minos, signature, and static install all passed;
  execution was skipped with an explicit reason on this Intel/macOS 14 host.
- `shfmt -d -i 2 scripts/macos tests/macos_dmg_smoke.sh mise-tasks/package/…`
  clean; `poly lint docs/macos/packaging.md` no issues; `bash -n` clean.
- A full source build of the four real binaries was started but the background
  shell was cancelled by a harness restart on a machine under load average
  >200. The packaging path is therefore proven with fixtures and pinned ORT
  assets; producing the four final DMGs needs a completed build or the CI
  release archives (`--skip-tar`).

## Runner limits

- Notarization requires a macOS runner with Xcode (`xcrun`), a Developer ID
  Application certificate in its keychain, a keychain profile, and network
  access to Apple. A Linux runner, or a macOS runner without those references,
  can only produce ad-hoc-signed DMGs.
- The local validation runner is an Intel Mac on macOS 14.7.4 with both Apple
  targets installed. Both arm64 and x86_64 slices can be built and cross-linked
  here, but the x86_64 slice's minos is forced to 15.0 so it matches the
  `macos-15-intel` runner used by the release matrix.
- No Developer ID identity and no notary keychain profile are available on the
  local runner, so local DMGs are ad-hoc signed and are not notarized or
  stapled. The notarization and stapling steps are implemented but can only be
  exercised on a configured release runner.
- A DMG smoke test that executes the x86_64 slice cannot run on macOS 14; the
  Intel binary declares 15.0 and must be exercised on macOS 15+. The universal
  DMG's arm64 slice can be smoke-tested on any supported Apple-silicon host on
  macOS 13+. `tests/macos_dmg_smoke.sh` therefore always mounts and statically
  validates the DMG, and skips only the executable checks with a printed reason
  when the host cannot launch that slice (`BASEMIND_VERSION` /
  `BASEMIND_SKIP_VERSION_CHECK` let the installer be exercised statically).
- The local runner is Intel on macOS 14.7.4, so it can neither run the thin
  arm64 DMG (no arm64 execution on Intel) nor the minos=15.0 Intel slice of the
  universal DMG. Functional execution of all four DMGs needs an Apple-silicon
  host for the arm64 DMGs and a macOS 15+ host for the universal DMGs.

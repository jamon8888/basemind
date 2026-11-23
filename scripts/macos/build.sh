#!/usr/bin/env bash
# Build the four thin macOS binaries consumed by scripts/macos/package.sh.
#
# Feature / deployment-target matrix:
#   full            arm64   -> features=full,                     MACOSX_DEPLOYMENT_TARGET=13.0
#   full            x86_64  -> features=full,ort-dynamic,         MACOSX_DEPLOYMENT_TARGET=15.0
#   hacienda-legal  arm64   -> features=hacienda-legal,           MACOSX_DEPLOYMENT_TARGET=13.0
#   hacienda-legal  x86_64  -> features=hacienda-legal,ort-dynamic, MACOSX_DEPLOYMENT_TARGET=15.0
#
# The Intel legs use ort-dynamic because Microsoft stopped publishing a static
# x86_64 ONNX Runtime for macOS; package-release.sh vendors the last pinned
# x86_64 dylib next to the binary. Intel support starts at macOS 15 (the same
# runner the release workflow uses), so x86_64 is compiled with minos=15.0 even
# when the local host runs an older macOS. The arm64 legs target macOS 13+.
#
# Usage: scripts/macos/build.sh [--variant full|hacienda-legal|all] [--skip-fetch]
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../.."

# package-release.sh looks for binaries under target/<triple>/release, so the
# build must land there even when a global cargo config sets a shared target-dir.
export CARGO_TARGET_DIR="$PWD/target"

variant="all"
skip_fetch=0
while [ "$#" -gt 0 ]; do
  case "$1" in
  --variant)
    variant="${2:?--variant needs a value}"
    shift 2
    ;;
  --skip-fetch)
    skip_fetch=1
    shift
    ;;
  *)
    printf 'build: error: unknown option: %s\n' "$1" >&2
    exit 2
    ;;
  esac
done

case "$variant" in
all) variants=(full hacienda-legal) ;;
full | hacienda-legal) variants=("$variant") ;;
*)
  printf 'build: error: unknown variant: %s\n' "$variant" >&2
  exit 2
  ;;
esac

if [ "$skip_fetch" -eq 0 ]; then
  printf '==> cargo fetch --locked\n'
  cargo fetch --locked
fi

build_one() {
  local v="$1" triple="$2" features="$3" minos="$4"
  printf '==> building %s / %s (features: %s, minos: %s)\n' "$v" "$triple" "$features" "$minos"
  env MACOSX_DEPLOYMENT_TARGET="$minos" \
    cargo build --locked --release --features "$features" --bin basemind --target "$triple"
  local bin="target/${triple}/release/basemind"
  [ -x "$bin" ] || {
    printf 'build: error: expected binary missing: %s\n' "$bin" >&2
    exit 1
  }
  bash scripts/macos/validate.sh arch "$bin" "${triple%%-*}" >/dev/null
  if [ "$triple" = x86_64-apple-darwin ]; then
    bash scripts/macos/validate.sh minos "$bin" x86_64 eq "$minos" >/dev/null
  else
    bash scripts/macos/validate.sh minos "$bin" arm64 le "$minos" >/dev/null
  fi
  printf '    OK: %s\n' "$bin"
}

for v in "${variants[@]}"; do
  case "$v" in
  full)
    build_one "$v" aarch64-apple-darwin "full" "13.0"
    build_one "$v" x86_64-apple-darwin "full,ort-dynamic" "15.0"
    ;;
  hacienda-legal)
    build_one "$v" aarch64-apple-darwin "hacienda-legal" "13.0"
    build_one "$v" x86_64-apple-darwin "hacienda-legal,ort-dynamic" "15.0"
    ;;
  esac
done

printf 'OK: all requested macOS binaries built and validated\n'

#!/usr/bin/env bash
# Build a universal (arm64+x86_64) ONNX Runtime dylib from pinned upstream
# assets. Both slices come from the SAME onnxruntime version (1.23.2), so no
# "undefined ORT fat dylib input" remains in the universal DMG path.
#
# Why PyPI wheels and not the GitHub .tgz release: the pinned x86_64 macOS .tgz
# (onnxruntime-osx-x86_64-1.23.2.tgz, sha256
# d10359e16347b57d9959f7e80a225a5b4a66ed7d7e007274a15cae86836485a6) is the
# source scripts/package-release.sh uses for the thin Intel archive. The
# matching arm64 .tgz is not published at that version; the Python wheels ARE
# published for both macOS 13_0 architectures and carry the same native dylib.
# Their SHA256s are pinned below and verified before extraction.
#
# Result: libonnxruntime with minos 13.4 on both slices, safely below the
# macOS 15 minimum of the Intel basemind binary.
#
# Usage: scripts/macos/ort-universal.sh [--out FILE] [--cache DIR] [--force]
set -euo pipefail

die() {
  printf 'ort-universal: error: %s\n' "$*" >&2
  exit 1
}

ORT_VERSION="1.23.2"
WHEEL_CPY="cp311"
DYLIB="onnxruntime/capi/libonnxruntime.${ORT_VERSION}.dylib"
ARM_WHEEL="onnxruntime-${ORT_VERSION}-${WHEEL_CPY}-${WHEEL_CPY}-macosx_13_0_arm64.whl"
INTEL_WHEEL="onnxruntime-${ORT_VERSION}-${WHEEL_CPY}-${WHEEL_CPY}-macosx_13_0_x86_64.whl"
ARM_SHA="6f91d2c9b0965e86827a5ba01531d5b669770b01775b23199565d6c1f136616c"
INTEL_SHA="87d8b6eaf0fbeb6835a60a4265fde7a3b60157cf1b2764773ac47237b4d48612"
ARM_URL="https://files.pythonhosted.org/packages/44/be/467b00f09061572f022ffd17e49e49e5a7a789056bad95b54dfd3bee73ff/${ARM_WHEEL}"
INTEL_URL="https://files.pythonhosted.org/packages/9f/a8/3c23a8f75f93122d2b3410bfb74d06d0f8da4ac663185f91866b03f7da1b/${INTEL_WHEEL}"

out=""
cache="${TMPDIR:-/tmp}/basemind-ort-${ORT_VERSION}"
force=0
while [ "$#" -gt 0 ]; do
  case "$1" in
  --out)
    out="${2:?--out needs a path}"
    shift 2
    ;;
  --cache)
    cache="${2:?--cache needs a directory}"
    shift 2
    ;;
  --force)
    force=1
    shift
    ;;
  *)
    die "unknown option: $1"
    ;;
  esac
done
[ -n "$out" ] || die "--out is required"
mkdir -p "$cache"

fetch_pinned() {
  local url="$1" sha="$2" name="$3"
  local path="$cache/$name" got
  if [ ! -f "$path" ] || [ "$force" -eq 1 ]; then
    printf 'ort-universal: downloading %s\n' "$name"
    python3 - "$url" "$path" <<'PY'
import sys, urllib.request
url, path = sys.argv[1], sys.argv[2]
urllib.request.urlretrieve(url, path)
PY
  fi
  got="$(shasum -a 256 "$path" | awk '{print $1}')"
  [ "$got" = "$sha" ] || die "checksum mismatch for $name: expected $sha, got $got"
  printf 'ort-universal: checksum OK %s (%s)\n' "$name" "$got"
}

fetch_pinned "$ARM_URL" "$ARM_SHA" "$ARM_WHEEL"
fetch_pinned "$INTEL_URL" "$INTEL_SHA" "$INTEL_WHEEL"

extract_dylib() {
  local wheel="$1" dest="$2"
  mkdir -p "$dest"
  unzip -oq "$cache/$wheel" "$DYLIB" -d "$dest"
  [ -f "$dest/$DYLIB" ] || die "dylib $DYLIB missing from $wheel"
}

extract_dylib "$ARM_WHEEL" "$cache/arm"
extract_dylib "$INTEL_WHEEL" "$cache/intel"

arm_dylib="$cache/arm/$DYLIB"
intel_dylib="$cache/intel/$DYLIB"
[ "$(lipo -archs "$arm_dylib")" = "arm64" ] || die "arm64 wheel dylib has unexpected slices"
[ "$(lipo -archs "$intel_dylib")" = "x86_64" ] || die "Intel wheel dylib has unexpected slices"

mkdir -p "$(dirname "$out")"
lipo -create -output "$out" "$arm_dylib" "$intel_dylib"
archs="$(lipo -archs "$out")"
[ "$archs" = "x86_64 arm64" ] || [ "$archs" = "arm64 x86_64" ] ||
  die "merged dylib has unexpected slices: $archs"

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
bash "$repo/scripts/macos/validate.sh" arch "$out" arm64,x86_64 >/dev/null
bash "$repo/scripts/macos/validate.sh" minos "$out" arm64 le 15.0 >/dev/null
bash "$repo/scripts/macos/validate.sh" minos "$out" x86_64 le 15.0 >/dev/null

printf 'ort-universal: %s [%s], minos arm64=%s x86_64=%s, sha256=%s\n' \
  "$out" "$archs" \
  "$(otool -l -arch arm64 "$out" | awk '/minos /{print $2;exit}')" \
  "$(otool -l -arch x86_64 "$out" | awk '/minos /{print $2;exit}')" \
  "$(shasum -a 256 "$out" | awk '{print $1}')"

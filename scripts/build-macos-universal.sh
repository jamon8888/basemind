#!/usr/bin/env bash
set -euo pipefail

# Build the macOS universal2 release archive (basemind-universal2-apple-darwin.tar.gz).
#
# The per-arch slices already exist as release assets built by publish.yaml's build-binaries
# matrix, so no extra compile is needed: the `from-release` mode downloads both darwin archives
# off the draft release and merges them at the Mach-O level.
#
#   arm64 slice    aarch64-apple-darwin  `full`             ONNX Runtime statically linked (ort-sys)
#   x86_64 slice   x86_64-apple-darwin   `full,ort-dynamic` dlopen's libonnxruntime.dylib
#
# A universal binary is two complete binaries glued by `lipo`, each slice keeping its own load
# commands — so the mixed linking models above are fine: on Apple Silicon the merged binary runs
# the statically linked arm64 slice, on Intel it runs the x86_64 slice which dlopens the vendored
# dylib shipped at the archive root (same pinned ORT release as scripts/package-release.sh —
# keep the version and sha256 in sync there).
#
# lib/ holds the union of both slices' bundled dylibs: same-named dylibs (the Homebrew
# tesseract/leptonica/heif closure) are lipo-merged into fat dylibs; a dylib present in only one
# closure is copied as-is, since the slice that references it is its only loader.
#
# usage:
#   build-macos-universal.sh merge <dir-arm64> <dir-x64> <out-dir>
#       Merge two extracted archives (root layout: basemind + lib/) into out-dir.
#       For local testing, set BM_UNIVERSAL_ORT_DYLIB to a pre-staged dylib to skip the pinned
#       ORT download.
#   build-macos-universal.sh from-release <release_id>
#       Download the two archives from the draft release (GITHUB_TOKEN/GITHUB_REPOSITORY from
#       the environment), merge, smoke-test, and package basemind-universal2-apple-darwin.tar.gz
#       in the current directory.

usage() {
  sed -n '2,40p' "$0" | grep '^#' | sed 's/^# \{0,1\}//' >&2
  exit 2
}

ASSET_SCRIPT="$(cd "$(dirname "$0")/../.github/scripts" && pwd)/release-asset.sh"
ARCHIVE="basemind-universal2-apple-darwin.tar.gz"

# Keep in sync with scripts/package-release.sh (Intel macOS vendored ORT).
ORT_VERSION="1.23.2"
ORT_SHA256="d10359e16347b57d9959f7e80a225a5b4a66ed7d7e007274a15cae86836485a6"
ORT_ASSET="onnxruntime-osx-x86_64-${ORT_VERSION}.tgz"

require_bin() {
  [ -f "$1" ] || { echo "missing expected Mach-O binary: $1" >&2; exit 1; }
}

contains_arch() { # <lipo-info-output> <arch>
  printf '%s\n' "$1" | grep -qE "(^|[[:space:]])$2([[:space:]]|$)"
}

cmd_merge() {
  [ "$#" -eq 3 ] || usage
  local arm_dir="$1" x64_dir="$2" out="$3"

  local arm_bin="$arm_dir/basemind" x64_bin="$x64_dir/basemind"
  require_bin "$arm_bin"
  require_bin "$x64_bin"

  local arm_info x64_info
  arm_info="$(lipo -info "$arm_bin")"
  x64_info="$(lipo -info "$x64_bin")"
  contains_arch "$arm_info" arm64 || { echo "::error::$arm_bin is not arm64: $arm_info"; exit 1; }
  contains_arch "$x64_info" x86_64 || { echo "::error::$x64_bin is not x86_64: $x64_info"; exit 1; }

  rm -rf "$out"
  mkdir -p "$out/lib"

  lipo -create -output "$out/basemind" "$arm_bin" "$x64_bin"
  merged_info="$(lipo -info "$out/basemind")"
  contains_arch "$merged_info" arm64 && contains_arch "$merged_info" x86_64 || {
    echo "::error::merged binary lost a slice: $merged_info"; exit 1
  }
  echo "merged binary: $merged_info"

  # Union of the two bundled-lib closures. Same basename in both → fat dylib; one-sided → copy.
  # `lipo -create` also refuses same-arch inputs, so a mislabeled archive fails loudly here.
  processed=" "
  for f in "$arm_dir/lib/"*.dylib "$x64_dir/lib/"*.dylib; do
    [ -f "$f" ] || continue
    base="$(basename "$f")"
    case "$processed" in *" $base "*) continue ;; esac
    processed="$processed$base "
    if [ -f "$arm_dir/lib/$base" ] && [ -f "$x64_dir/lib/$base" ]; then
      lipo -create -output "$out/lib/$base" "$arm_dir/lib/$base" "$x64_dir/lib/$base"
    elif [ -f "$arm_dir/lib/$base" ]; then
      cp -L "$arm_dir/lib/$base" "$out/lib/$base"
    else
      cp -L "$x64_dir/lib/$base" "$out/lib/$base"
    fi
  done

  # Vendored ONNX Runtime for the x86_64 slice. The arm64 slice statically links ort-sys's
  # runtime, so a pure x86_64 dylib is correct here: it is dlopened only by the Intel slice.
  # ort resolves it relative to the executable, hence the archive-root placement.
  if [ -n "${BM_UNIVERSAL_ORT_DYLIB:-}" ]; then
    cp -L "$BM_UNIVERSAL_ORT_DYLIB" "$out/libonnxruntime.dylib"
  else
    local tmp
    tmp="$(mktemp -d)"
    curl -fsSL -o "$tmp/$ORT_ASSET" \
      "https://github.com/microsoft/onnxruntime/releases/download/v${ORT_VERSION}/${ORT_ASSET}"
    echo "$ORT_SHA256  $tmp/$ORT_ASSET" | shasum -a 256 -c - >/dev/null
    tar xzf "$tmp/$ORT_ASSET" -C "$tmp"
    cp "$tmp/onnxruntime-osx-x86_64-${ORT_VERSION}/lib/libonnxruntime.${ORT_VERSION}.dylib" \
      "$out/libonnxruntime.dylib"
    rm -rf "$tmp"
  fi
  chmod u+w "$out/libonnxruntime.dylib"
  install_name_tool -id "@loader_path/libonnxruntime.dylib" "$out/libonnxruntime.dylib"

  # Re-sign everything ad-hoc: lipo merges carry each slice's old signature, and install_name_tool
  # invalidates the ORT dylib's. Same signing as scripts/package-release.sh.
  for dylib in "$out/libonnxruntime.dylib" "$out/lib/"*.dylib; do
    [ -f "$dylib" ] || continue
    codesign --force --sign - "$dylib"
  done
  codesign --force --sign - "$out/basemind"

  # The binary's rpath (@loader_path/lib, added per slice by package-release.sh) must have
  # survived the merge; each slice carries an identical one so it cannot collide.
  otool -l "$out/basemind" | grep -q 'LC_RPATH' \
    && otool -l "$out/basemind" | grep -A3 'LC_RPATH' | grep -q '@loader_path/lib' || {
    echo "::error::merged binary lost the @loader_path/lib LC_RPATH"; exit 1
  }
  codesign --verify --strict "$out/basemind"
}

cmd_from_release() {
  [ "$#" -eq 1 ] || usage
  local release_id="$1" work
  : "${GITHUB_TOKEN:?GITHUB_TOKEN must be set}"
  : "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY must be set}"

  work="$(mktemp -d)"
  trap 'rm -rf "$work"' EXIT

  bash "$ASSET_SCRIPT" download "$release_id" basemind-aarch64-apple-darwin.tar.gz "$work/arm64.tar.gz"
  bash "$ASSET_SCRIPT" download "$release_id" basemind-x86_64-apple-darwin.tar.gz "$work/x64.tar.gz"
  mkdir "$work/arm64" "$work/x64"
  tar xzf "$work/arm64.tar.gz" -C "$work/arm64"
  tar xzf "$work/x64.tar.gz" -C "$work/x64"

  bash "$0" merge "$work/arm64" "$work/x64" "$work/universal2"

  # Smoke-test both slices of the merged binary in place:
  #  - arm64 slice runs natively on this runner;
  #  - x86_64 slice runs under Rosetta when present, together with a dlopen of the vendored
  #    ORT dylib — the exact load ort-dynamic performs at runtime on an Intel Mac.
  ( cd "$work/universal2" && ./basemind --version )
  if arch -x86_64 /usr/bin/true 2>/dev/null; then
    ( cd "$work/universal2" && arch -x86_64 ./basemind --version )
    ( cd "$work/universal2" && arch -x86_64 /usr/bin/python3 \
      -c "import ctypes; ctypes.CDLL('./libonnxruntime.dylib'); print('universal2 libonnxruntime.dylib load OK (x86_64 slice)')" )
  else
    echo "::warning::Rosetta unavailable on this runner — x86_64 slice smoke-test skipped"
  fi

  tar czf "$ARCHIVE" -C "$work/universal2" .
  echo "✓ Created $ARCHIVE"
  tar tzf "$ARCHIVE"
}

case "${1:-}" in
  merge) shift; cmd_merge "$@" ;;
  from-release) shift; cmd_from_release "$@" ;;
  *) usage ;;
esac

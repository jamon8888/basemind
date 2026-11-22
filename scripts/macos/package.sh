#!/usr/bin/env bash
# Package macOS release DMGs from the release .tar.gz archives.
#
# Produces, for each requested profile, an arm64-only DMG and a universal
# (arm64+x86_64) DMG. The .tar.gz archives produced by scripts/package-release.sh
# are preserved untouched — this script only consumes them.
#
#   full:           basemind-aarch64-apple-darwin.dmg
#                   basemind-universal-apple-darwin.dmg
#   hacienda-legal: basemind-hacienda-legal-aarch64-apple-darwin.dmg
#                   basemind-hacienda-legal-universal-apple-darwin.dmg
#
# Signing / notarization are driven only by opaque keychain references; see
# scripts/macos/notarize.sh. No secret is ever read or logged by this script.
#
# Usage: scripts/macos/package.sh [--variant full|hacienda-legal|all]
#                                 [--out-dir DIR] [--build]
#                                 [--skip-tar] [--ad-hoc]
#                                 [--fat-ort FILE] [--no-fat-ort]
#                                 [--identity NAME] [--notary-profile NAME]
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../.."

variant="all"
out_dir="$PWD"
do_build=0
skip_tar=0
ad_hoc=0
fat_ort_file=""
no_fat_ort=0

die() {
  printf 'package: error: %s\n' "$*" >&2
  exit 1
}

while [ "$#" -gt 0 ]; do
  case "$1" in
  --variant)
    variant="${2:?--variant needs a value}"
    shift 2
    ;;
  --out-dir)
    out_dir="${2:?--out-dir needs a directory}"
    shift 2
    ;;
  --build)
    do_build=1
    shift
    ;;
  --skip-tar)
    skip_tar=1
    shift
    ;;
  --ad-hoc)
    ad_hoc=1
    shift
    ;;
  --fat-ort)
    fat_ort_file="${2:?--fat-ort needs a dylib path}"
    shift 2
    ;;
  --no-fat-ort)
    no_fat_ort=1
    shift
    ;;
  --identity)
    export MACOS_SIGNING_IDENTITY="${2:?--identity needs a value}"
    shift 2
    ;;
  --notary-profile)
    export MACOS_NOTARY_PROFILE="${2:?--notary-profile needs a value}"
    shift 2
    ;;
  -h | --help)
    sed -n '2,20p' "${BASH_SOURCE[0]}"
    exit 0
    ;;
  *)
    die "unknown option: $1"
    ;;
  esac
done

case "$variant" in
all | full | hacienda-legal) ;;
*) die "unknown variant: $variant" ;;
esac

if [ "$ad_hoc" -eq 1 ]; then
  export MACOS_SIGNING_IDENTITY=""
  export MACOS_NOTARY_PROFILE=""
fi

mkdir -p "$out_dir"
if [ "$do_build" -eq 1 ]; then
  bash scripts/macos/build.sh --variant "$variant"
fi
bash scripts/macos/notarize.sh status

work="$(mktemp -d "${TMPDIR:-/tmp}/basemind-dmg.XXXXXX")"
trap 'rm -rf "$work"' EXIT

extract_archive() {
  local archive="$1" dest="$2"
  [ -f "$archive" ] || die "missing archive $archive (build it first, or drop --skip-tar)"
  mkdir -p "$dest"
  tar -xzf "$archive" -C "$dest"
  [ -x "$dest/basemind" ] || die "archive $archive has no executable basemind at its root"
}

add_meta() {
  local dir="$1" dmg_name="$2" mode="$3"
  cp scripts/macos/install.sh "$dir/install.sh"
  chmod +x "$dir/install.sh"
  cat >"$dir/README.txt" <<EOF
Basemind ($mode) — CLI + stdio MCP server, no GUI.

Install:  ./install.sh [--prefix DIR]     (default prefix: \$HOME/.local)
Uninstall: ./install.sh --uninstall
CLI:      <prefix>/bin/basemind
MCP:      <prefix>/bin/basemind-mcp  (stdio; register this as the MCP command)

Artifact: $dmg_name
EOF
}

is_macho() {
  [ -f "$1" ] || return 1
  [ -n "$(lipo -archs "$1" 2>/dev/null)" ]
}

# Merge two thin staging trees into one universal tree. Files present in both
# are lipo-merged; files present on only one side are copied verbatim (e.g.
# libonnxruntime.dylib, which only the Intel ort-dynamic build needs).
merge_universal() {
  local arm_dir="$1" intel_dir="$2" dest="$3" paths="$4"
  mkdir -p "$dest"
  {
    (cd "$arm_dir" && find . -type f -print)
    (cd "$intel_dir" && find . -type f -print)
  } | sed 's|^\./||' | sort -u >"$paths"

  local rel out arm_f intel_f universal_count=0 single_count=0
  while IFS= read -r rel; do
    [ -n "$rel" ] || continue
    case "$rel" in
    install.sh | README.txt) continue ;;
    esac
    arm_f="$arm_dir/$rel"
    intel_f="$intel_dir/$rel"
    out="$dest/$rel"
    mkdir -p "$(dirname "$out")"
    if [ -f "$arm_f" ] && [ -f "$intel_f" ] && is_macho "$arm_f" && is_macho "$intel_f"; then
      lipo -create -output "$out" "$arm_f" "$intel_f"
      chmod "$(stat -f '%Lp' "$arm_f")" "$out"
      universal_count=$((universal_count + 1))
    elif [ -f "$arm_f" ]; then
      cp -p "$arm_f" "$out"
      single_count=$((single_count + 1))
    else
      cp -p "$intel_f" "$out"
      single_count=$((single_count + 1))
    fi
  done <"$paths"
  printf 'universal merge: %d lipo-merged file(s), %d single-arch file(s)\n' \
    "$universal_count" "$single_count"
}

finalize_dmg() {
  local dir="$1" dmg="$2" mode="$3" volname="$4"
  bash scripts/macos/validate.sh bundle "$dir" "$mode"
  bash scripts/macos/notarize.sh sign "$dir"

  hdiutil create -volname "$volname" -srcfolder "$dir" -fs HFS+ -format UDZO -ov "$dmg"
  [ -f "$dmg" ] || die "hdiutil did not create $dmg"
  bash scripts/macos/notarize.sh sign "$dmg"
  bash scripts/macos/notarize.sh submit "$dmg"
  bash scripts/macos/notarize.sh staple "$dmg"

  local sum
  sum="$(shasum -a 256 "$dmg" | awk '{print $1}')"
  printf 'artifact: %s (%s, sha256 %s)\n' "$dmg" "$mode" "$sum"
}

for v in full hacienda-legal; do
  [ "$variant" = "all" ] || [ "$variant" = "$v" ] || continue

  if [ "$v" = "full" ]; then
    name="basemind"
    variant_arg=""
    arm_dmg="basemind-aarch64-apple-darwin.dmg"
    uni_dmg="basemind-universal-apple-darwin.dmg"
  else
    name="basemind-hacienda-legal"
    variant_arg="hacienda-legal"
    arm_dmg="basemind-hacienda-legal-aarch64-apple-darwin.dmg"
    uni_dmg="basemind-hacienda-legal-universal-apple-darwin.dmg"
  fi

  arm_archive="${name}-aarch64-apple-darwin.tar.gz"
  intel_archive="${name}-x86_64-apple-darwin.tar.gz"
  printf '==> profile %s\n' "$v"

  if [ "$skip_tar" -eq 0 ]; then
    ./scripts/package-release.sh aarch64-apple-darwin "$variant_arg"
    ./scripts/package-release.sh x86_64-apple-darwin "$variant_arg"
  fi

  arm_dir="$work/$v/aarch64"
  intel_dir="$work/$v/x86_64"
  uni_dir="$work/$v/universal"
  extract_archive "$arm_archive" "$arm_dir"
  extract_archive "$intel_archive" "$intel_dir"

  add_meta "$arm_dir" "$arm_dmg" "arm64 / macOS 13+"
  add_meta "$intel_dir" "staging only" "Intel slice / macOS 15+"

  printf '==> %s\n' "$arm_dmg"
  finalize_dmg "$arm_dir" "$out_dir/$arm_dmg" arm64 "Basemind ($v, arm64)"

  printf '==> %s\n' "$uni_dmg"
  merge_universal "$arm_dir" "$intel_dir" "$uni_dir" "$work/$v.paths"

  # The Intel slice dlopens libonnxruntime.dylib. Use a checksum-pinned fat
  # dylib so the universal DMG carries one ORT library that loads on both
  # slices; without it the arm64 side would carry a dead x86_64-only file.
  if [ -f "$uni_dir/libonnxruntime.dylib" ]; then
    fat_ort="$fat_ort_file"
    if [ -z "$fat_ort" ] && [ "$no_fat_ort" -eq 0 ]; then
      bash scripts/macos/ort-universal.sh --out "$work/ort/libonnxruntime.dylib"
      fat_ort="$work/ort/libonnxruntime.dylib"
    fi
    if [ -n "$fat_ort" ]; then
      [ -f "$fat_ort" ] || die "fat ORT dylib not found: $fat_ort"
      cp -p "$fat_ort" "$uni_dir/libonnxruntime.dylib"
      printf 'universal ORT: replaced thin x86_64 dylib with %s\n' "$fat_ort"
    else
      printf 'universal ORT: keeping thin x86_64 dylib (--no-fat-ort)\n'
    fi
  fi

  add_meta "$uni_dir" "$uni_dmg" "universal / Intel from macOS 15"
  finalize_dmg "$uni_dir" "$out_dir/$uni_dmg" universal "Basemind ($v, universal)"

  printf '==> profile %s done (archives preserved: %s, %s)\n' "$v" "$arm_archive" "$intel_archive"
done

printf 'OK: macOS DMG packaging complete\n'

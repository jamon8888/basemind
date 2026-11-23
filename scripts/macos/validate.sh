#!/usr/bin/env bash
# Validate macOS Mach-O artifacts before they are sealed into a DMG: the exact
# slice set (thin arm64 vs universal arm64+x86_64) and the minimum OS version
# declared by LC_BUILD_VERSION / LC_VERSION_MIN_MACOSX.
#
# The rules encode the release contract:
#   - the arm64 slice must run on macOS 13+ (the bundled onnxruntime dylib
#     declares minos 13.4), so a slice's minos is <= ARM_MAX (13.4);
#   - the x86_64 slice is only offered from macOS 15, so the Intel *binary*
#     declares exactly INTEL_MINOS (15.0). Vendored native libraries may
#     declare an older minos (a lower requirement still runs on 15) but never
#     a newer one than the binary that loads them.
#
# Usage:
#   validate.sh arch <file> <arm64|x86_64|arm64,x86_64>
#   validate.sh minos <file> <arch> <le|eq|ge> <version>
#   validate.sh bundle <root-dir> <arm64|x86_64|universal> \
#       [--intel-minos 15.0] [--arm-max 13.4]
set -euo pipefail

die() {
  printf 'validate: error: %s\n' "$*" >&2
  exit 1
}

usage() {
  sed -n '2,18p' "${BASH_SOURCE[0]}" >&2
  exit 2
}

# "13.4" -> 1304, so awk's numeric compare orders versions correctly.
ver_num() {
  awk -F. '{ printf "%d%02d\n", $1, $2 }' <<<"$1"
}

slice_archs() {
  lipo -archs "$1" 2>/dev/null || true
}

# Print the minos of one slice, or nothing when the file has no LC_BUILD_VERSION
# or LC_VERSION_MIN_MACOSX command.
slice_minos() {
  local file="$1" arch="$2"
  otool -l -arch "$arch" "$file" 2>/dev/null | awk '
    /^Load command/ { in_build = 0; in_min = 0 }
    /cmd LC_BUILD_VERSION/ { in_build = 1 }
    /cmd LC_VERSION_MIN_MACOSX/ { in_min = 1 }
    in_build && /minos / { print $2; exit }
    in_min && /version / { print $2; exit }
  '
}

cmd_arch() {
  [ "$#" -eq 2 ] || usage
  local file="$1" expected="$2"
  [ -f "$file" ] || die "not a file: $file"
  local actual
  actual="$(slice_archs "$file")"
  [ -n "$actual" ] || die "$file is not a Mach-O binary (lipo -archs found no slices)"

  local -a want got
  IFS=',' read -r -a want <<<"$expected"
  read -r -a got <<<"$actual"
  local w g matched
  for w in "${want[@]}"; do
    matched=0
    for g in "${got[@]}"; do
      [ "$g" = "$w" ] && matched=1 && break
    done
    [ "$matched" -eq 1 ] || die "$file: expected slice $w, got [${actual}]"
  done
  [ "${#want[@]}" -eq "${#got[@]}" ] ||
    die "$file: expected exactly [${expected}], got [${actual}]"
  printf 'arch OK: %s [%s]\n' "$file" "$actual"
}

cmd_minos() {
  [ "$#" -eq 4 ] || usage
  local file="$1" arch="$2" op="$3" expected="$4"
  [ -f "$file" ] || die "not a file: $file"
  local actual
  actual="$(slice_minos "$file" "$arch")"
  [ -n "$actual" ] || die "$file ($arch): no LC_BUILD_VERSION/LC_VERSION_MIN_MACOSX minos"

  local a e
  a="$(ver_num "$actual")"
  e="$(ver_num "$expected")"
  case "$op" in
  le) [ "$a" -le "$e" ] || die "$file ($arch): minos $actual > $expected" ;;
  eq) [ "$a" -eq "$e" ] || die "$file ($arch): minos $actual != $expected" ;;
  ge) [ "$a" -ge "$e" ] || die "$file ($arch): minos $actual < $expected" ;;
  *) die "unknown comparison: $op" ;;
  esac
  printf 'minos OK: %s (%s) %s %s [actual %s]\n' "$file" "$arch" "$op" "$expected" "$actual"
}

is_macho() {
  [ -f "$1" ] || return 1
  [ -n "$(slice_archs "$1")" ]
}

cmd_bundle() {
  [ "$#" -ge 2 ] || usage
  local root="$1" mode="$2"
  shift 2
  local intel_minos="15.0" arm_max="13.4"
  while [ "$#" -gt 0 ]; do
    case "$1" in
    --intel-minos)
      intel_minos="${2:?--intel-minos needs a version}"
      shift 2
      ;;
    --arm-max)
      arm_max="${2:?--arm-max needs a version}"
      shift 2
      ;;
    *) die "unknown bundle option: $1" ;;
    esac
  done
  [ -d "$root" ] || die "not a directory: $root"

  local expected
  case "$mode" in
  arm64) expected="arm64" ;;
  x86_64) expected="x86_64" ;;
  universal) expected="arm64,x86_64" ;;
  *) die "unknown bundle mode: $mode" ;;
  esac

  local -a files=()
  while IFS= read -r -d '' f; do
    is_macho "$f" && files+=("$f")
  done < <(find "$root" -type f -print0)

  [ "${#files[@]}" -gt 0 ] || die "no Mach-O files under $root"
  local f archs
  for f in "${files[@]}"; do
    archs="$(slice_archs "$f")"
    local -a want got
    IFS=',' read -r -a want <<<"$expected"
    read -r -a got <<<"$archs"
    local base_name
    base_name="$(basename "$f")"
    local w g matched
    if [ "$base_name" = "basemind" ]; then
      # The executable must carry the full expected slice set.
      for w in "${want[@]}"; do
        matched=0
        for g in "${got[@]}"; do
          [ "$g" = "$w" ] && matched=1 && break
        done
        [ "$matched" -eq 1 ] || die "$f: expected slice $w, got [${archs}]"
      done
      [ "${#want[@]}" -eq "${#got[@]}" ] ||
        die "$f: expected exactly [${expected}], got [${archs}]"
    else
      # Native libraries may be thin (e.g. an Intel-only ort-dynamic dylib in
      # a bundle whose arm64 slice links ORT statically). They may only carry
      # a subset of the expected slices, never a foreign architecture.
      for g in "${got[@]}"; do
        matched=0
        for w in "${want[@]}"; do
          [ "$g" = "$w" ] && matched=1 && break
        done
        [ "$matched" -eq 1 ] || die "$f: unexpected slice $g, allowed [${expected}]"
      done
    fi

    # The main executable pins the contract; dylibs only need to be compatible
    # (a lower minos is fine, a higher one would break the oldest supported OS).
    local base
    base="$(basename "$f")"
    if [[ " $archs " == *" x86_64 "* ]]; then
      if [ "$base" = "basemind" ]; then
        cmd_minos "$f" x86_64 eq "$intel_minos" >/dev/null
      else
        cmd_minos "$f" x86_64 le "$intel_minos" >/dev/null
      fi
    fi
    if [[ " $archs " == *" arm64 "* ]]; then
      cmd_minos "$f" arm64 le "$arm_max" >/dev/null
    fi
    printf 'bundle OK: %s [%s]\n' "$f" "$archs"
  done
  printf 'bundle validated: %d Mach-O file(s), mode=%s\n' "${#files[@]}" "$mode"
}

[ "$#" -gt 0 ] || usage
cmd="$1"
shift
case "$cmd" in
arch) cmd_arch "$@" ;;
minos) cmd_minos "$@" ;;
bundle) cmd_bundle "$@" ;;
*) usage ;;
esac

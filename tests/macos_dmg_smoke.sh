#!/usr/bin/env bash
# Smoke-test a macOS Basemind DMG: mount it, verify the payload is a CLI + stdio
# MCP server with no GUI components, check slice/minos contracts, then install
# into a throwaway prefix and run the wrapped CLI and MCP command.
#
# On a host that cannot execute a given slice (an arm64 DMG on Intel, or an
# Intel minos=15 binary before macOS 15) the executable checks are skipped with
# an explicit reason; the static checks (mount, payload, arch, minos, signature)
# always run. On non-macOS hosts this test is skipped with exit 0.
#
# Usage: tests/macos_dmg_smoke.sh <artifact.dmg> [arm64|universal]
set -euo pipefail

die() {
  printf 'dmg-smoke: error: %s\n' "$*" >&2
  exit 1
}

[ "$(uname -s)" = "Darwin" ] || {
  printf 'dmg-smoke: skipped (not macOS)\n'
  exit 0
}

[ "$#" -ge 1 ] || die "usage: $0 <artifact.dmg> [arm64|universal]"
dmg="$1"
[ -f "$dmg" ] || die "DMG not found: $dmg"

mode="${2:-}"
if [ -z "$mode" ]; then
  case "$(basename "$dmg")" in
  *universal*) mode="universal" ;;
  *) mode="arm64" ;;
  esac
fi
case "$mode" in
arm64 | universal) ;;
*) die "unknown mode: $mode" ;;
esac

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/basemind-dmg-smoke.XXXXXX")"
mountpoint=""
cleanup() {
  if [ -n "$mountpoint" ]; then
    hdiutil detach -force "$mountpoint" >/dev/null 2>&1 || true
  fi
  rm -rf "$tmp"
}
trap cleanup EXIT

printf '==> mounting %s\n' "$(basename "$dmg")"
hdiutil attach -readonly -nobrowse -mountpoint "$tmp/volume" "$dmg" >/dev/null
mountpoint="$tmp/volume"

[ -x "$mountpoint/install.sh" ] || die "DMG has no executable install.sh"
[ -x "$mountpoint/basemind" ] || die "DMG has no executable basemind"
gui="$(find "$mountpoint" -maxdepth 2 \( -name '*.app' -o -name '*.pkg' -o -name '*.mpkg' \) -print | head -n1)"
[ -z "$gui" ] || die "DMG contains a GUI installer component: $gui"

printf '==> validating architecture and minimum OS (%s)\n' "$mode"
bash "$repo/scripts/macos/validate.sh" bundle "$mountpoint" "$mode"
if [ "$mode" = "universal" ]; then
  bash "$repo/scripts/macos/validate.sh" arch "$mountpoint/basemind" arm64,x86_64
else
  bash "$repo/scripts/macos/validate.sh" arch "$mountpoint/basemind" arm64
fi
codesign --verify --strict "$mountpoint/basemind" ||
  die "payload binary has an invalid code signature"

# Whether this host can actually launch the payload. Apple silicon can run the
# arm64 slice of either DMG; an Intel host needs macOS 15+ for the universal
# DMG's minos=15.0 slice and can never run the thin arm64 DMG.
host_arch="$(uname -m)"
host_version="$(sw_vers -productVersion)"
host_major="${host_version%%.*}"
can_exec=1
skip_reason=""
case "$host_arch" in
arm64) can_exec=1 ;;
x86_64)
  if [ "$mode" = "arm64" ]; then
    can_exec=0
    skip_reason="arm64 payload cannot run on an Intel host"
  elif [ "${host_major:-0}" -lt 15 ]; then
    can_exec=0
    skip_reason="Intel slice requires macOS 15+, host is macOS ${host_version}"
  fi
  ;;
*)
  can_exec=0
  skip_reason="unknown host architecture ${host_arch}"
  ;;
esac

version=""
if [ "$can_exec" -eq 1 ]; then
  printf '==> running the payload from the read-only volume\n'
  version="$("$mountpoint/basemind" --version)"
  case "$version" in
  basemind\ *) printf '    %s\n' "$version" ;;
  *) die "unexpected version output: $version" ;;
  esac
  "$mountpoint/basemind" serve --help >/dev/null || die "serve (MCP) subcommand is missing"
else
  printf '==> execution skipped: %s\n' "$skip_reason"
fi

printf '==> installing into a throwaway prefix\n'
prefix_bin="$tmp/prefix/bin"
if [ "$can_exec" -eq 1 ]; then
  BASEMIND_PREFIX="$tmp/prefix" "$mountpoint/install.sh" >/dev/null
else
  BASEMIND_VERSION="0.0.0-static" BASEMIND_SKIP_VERSION_CHECK=1 \
    BASEMIND_PREFIX="$tmp/prefix" "$mountpoint/install.sh" >/dev/null
fi
[ -x "$prefix_bin/basemind" ] || die "installer did not create the CLI wrapper"
[ -x "$prefix_bin/basemind-mcp" ] || die "installer did not create the MCP wrapper"

if [ "$can_exec" -eq 1 ]; then
  got="$("$prefix_bin/basemind" --version)"
  [ "$got" = "$version" ] || die "installed CLI reports '$got', expected '$version'"
  "$prefix_bin/basemind-mcp" --help >/dev/null || die "installed MCP wrapper failed"
else
  grep -q '^exec ' "$prefix_bin/basemind" || die "CLI wrapper is not executable-shaped"
  grep -q 'basemind" serve' "$prefix_bin/basemind-mcp" || die "MCP wrapper does not launch serve"
fi

printf 'OK: %s (%s) mounted, validated, installed and smoke-tested%s\n' \
  "$(basename "$dmg")" "$mode" "$([ "$can_exec" -eq 1 ] || printf ' (static only)')"

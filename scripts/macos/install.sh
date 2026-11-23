#!/usr/bin/env bash
# basemind macOS installer — CLI + stdio MCP server, no GUI and no network.
#
# Ships at the root of the .dmg. Copies the bundled universal (or arm64)
# executable and its native libraries into a versioned, private directory and
# drops two small wrappers into <prefix>/bin:
#   basemind      -> the CLI (any subcommand)
#   basemind-mcp  -> `basemind serve`, a drop-in stdio MCP server command
#
# Usage:
#   install.sh [--prefix DIR] [--uninstall]
# Default prefix: $HOME/.local. Use BASEMIND_PREFIX to override it.
set -euo pipefail

die() {
  printf 'basemind-install: error: %s\n' "$*" >&2
  exit 1
}

prefix="${BASEMIND_PREFIX:-$HOME/.local}"
uninstall=0
while [ "$#" -gt 0 ]; do
  case "$1" in
  --prefix)
    prefix="${2:?--prefix needs a directory}"
    shift 2
    ;;
  --uninstall)
    uninstall=1
    shift
    ;;
  -h | --help)
    sed -n '2,13p' "${BASH_SOURCE[0]}"
    exit 0
    ;;
  *)
    die "unknown option: $1"
    ;;
  esac
done

src_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)" || die "cannot locate installer directory"
src_bin="$src_dir/basemind"
[ -x "$src_bin" ] || die "bundled basemind binary not found next to install.sh"

# BASEMIND_VERSION / BASEMIND_SKIP_VERSION_CHECK let a static smoke test install
# the payload on a host that cannot execute it (e.g. an arm64 DMG on Intel, or
# an Intel minos=15 binary on macOS 14). Release installs never set them.
version="${BASEMIND_VERSION:-}"
[ -n "$version" ] || version="$("$src_bin" --version 2>/dev/null | awk '{print $2}')"
[ -n "$version" ] || die "could not read the bundled basemind version"
case "$version" in
*[!0-9A-Za-z._-]*) die "unexpected version string: $version" ;;
esac

target_dir="$prefix/lib/basemind/$version"
bin_dir="$prefix/bin"

if [ "$uninstall" -eq 1 ]; then
  rm -f "$bin_dir/basemind" "$bin_dir/basemind-mcp"
  [ ! -d "$target_dir" ] || rm -rf "$target_dir"
  printf 'basemind-install: removed %s (v%s)\n' "$target_dir" "$version"
  exit 0
fi

mkdir -p "$target_dir/lib" "$bin_dir"

# The binary resolves its bundled dylibs through @loader_path/lib, so the lib/
# directory must live next to the executable — never in the shared prefix.
cp -p "$src_bin" "$target_dir/basemind"
chmod +x "$target_dir/basemind"
if [ -d "$src_dir/lib" ]; then
  cp -Rp "$src_dir/lib/." "$target_dir/lib/"
fi
if [ -f "$src_dir/libonnxruntime.dylib" ]; then
  cp -p "$src_dir/libonnxruntime.dylib" "$target_dir/libonnxruntime.dylib"
fi

cat >"$bin_dir/basemind" <<EOF
#!/usr/bin/env bash
# basemind CLI wrapper installed from the macOS DMG (no GUI, no network).
exec "$target_dir/basemind" "\$@"
EOF
cat >"$bin_dir/basemind-mcp" <<EOF
#!/usr/bin/env bash
# basemind stdio MCP server wrapper. Register this path as the MCP command.
exec "$target_dir/basemind" serve "\$@"
EOF
chmod +x "$bin_dir/basemind" "$bin_dir/basemind-mcp"

# Fail here rather than leaving a broken install behind, unless the host cannot
# execute this slice at all (static smoke test).
if [ "${BASEMIND_SKIP_VERSION_CHECK:-0}" != "1" ]; then
  got="$("$bin_dir/basemind" --version 2>/dev/null | awk '{print $2}')"
  [ "$got" = "$version" ] || die "installed binary reports '$got', expected '$version'"
fi

printf 'basemind-install: installed v%s under %s\n' "$version" "$target_dir"
printf 'basemind-install: CLI: %s/basemind\n' "$bin_dir"
printf 'basemind-install: MCP: %s/basemind-mcp (stdio; add it to your MCP client)\n' "$bin_dir"

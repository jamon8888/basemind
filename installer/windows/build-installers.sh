#!/usr/bin/env bash
# Build both Basemind per-user Windows setup.exe artifacts.
#
# This is the single entry point for producing, from one checkout:
#   basemind-x86_64-pc-windows-msvc-setup.exe
#   basemind-hacienda-legal-x86_64-pc-windows-msvc-setup.exe
#
# It requires a WINDOWS runner/host with Inno Setup 6 (ISCC.exe). The existing
# release .zip files are left untouched; the installer is an additional asset
# built from the same staged payload. Nothing here publishes or merges anything.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../.."

TRIPLE="x86_64-pc-windows-msvc"
OUT_DIR="${BASEMIND_INSTALLER_OUT_DIR:-dist/installer}"
ZIP_FULL="${BASEMIND_ZIP_FULL:-}"
ZIP_LEGAL="${BASEMIND_ZIP_LEGAL:-}"

# Both profiles compile to the same target/<triple>/release/basemind.exe, so raw
# build output belongs to whichever profile was built LAST. Prefer the two release
# archives (produced by scripts/package-release.sh) as the payload source: that
# also guarantees the installer ships exactly the bytes of the published .zip.
if [ -z "$ZIP_FULL" ] && [ -f "basemind-${TRIPLE}.zip" ]; then
  ZIP_FULL="basemind-${TRIPLE}.zip"
fi
if [ -z "$ZIP_LEGAL" ] && [ -f "basemind-hacienda-legal-${TRIPLE}.zip" ]; then
  ZIP_LEGAL="basemind-hacienda-legal-${TRIPLE}.zip"
fi

case "$(uname -s 2>/dev/null || echo unknown)" in
MINGW* | MSYS* | CYGWIN* | Windows_NT) ;;
*)
  echo "ERROR: Windows installer build requires a Windows runner/host." >&2
  echo "  Detected host: $(uname -s 2>/dev/null || echo unknown)." >&2
  echo "  Missing runner: Windows (e.g. GitHub actions windows-2022) and Inno Setup 6 (ISCC.exe)." >&2
  echo "  On a Windows host set ISCC to ISCC.exe if it is not on PATH, then rerun this script." >&2
  exit 2
  ;;
esac

if command -v pwsh >/dev/null 2>&1; then
  PS="pwsh"
elif command -v powershell >/dev/null 2>&1; then
  PS="powershell"
else
  echo "ERROR: PowerShell (pwsh or powershell) is required to run build-installer.ps1." >&2
  exit 2
fi

COMMON=(-Triple "$TRIPLE" -OutDir "$OUT_DIR")
if [ -n "${BASEMIND_INSTALLER_VERSION:-}" ]; then
  COMMON+=(-Version "$BASEMIND_INSTALLER_VERSION")
fi

# Record the existing Windows release archives so we can prove the installer
# build leaves them byte-for-byte untouched.
ZIP_ASSETS=(
  "basemind-${TRIPLE}.zip"
  "basemind-hacienda-legal-${TRIPLE}.zip"
)
fingerprint() {
  local file="$1"
  if [ -f "$file" ]; then
    if command -v sha256sum >/dev/null 2>&1; then
      sha256sum "$file" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
      shasum -a 256 "$file" | awk '{print $1}'
    else
      stat -c '%s:%Y' "$file" 2>/dev/null || echo "size=$(wc -c <"$file")"
    fi
  else
    echo absent
  fi
}
BEFORE=()
for zip_name in "${ZIP_ASSETS[@]}"; do
  BEFORE+=("$(fingerprint "$zip_name")")
done

build_variant() {
  local variant="$1"
  local zip="$2"
  local extra=()
  if [ -n "$zip" ]; then
    extra+=(-ZipPath "$zip")
  fi
  echo "==> building $variant installer"
  "$PS" -NoProfile -ExecutionPolicy Bypass \
    -File installer/windows/build-installer.ps1 -Variant "$variant" \
    "${COMMON[@]}" "${extra[@]}"
}

build_variant full "$ZIP_FULL"
build_variant hacienda-legal "$ZIP_LEGAL"

EXPECTED=(
  "basemind-${TRIPLE}-setup.exe"
  "basemind-hacienda-legal-${TRIPLE}-setup.exe"
)
for artifact in "${EXPECTED[@]}"; do
  path="${OUT_DIR}/${artifact}"
  if [ ! -s "$path" ]; then
    echo "ERROR: expected installer artifact missing or empty: $path" >&2
    exit 1
  fi
  echo "✓ $path"
done

idx=0
for zip_name in "${ZIP_ASSETS[@]}"; do
  after="$(fingerprint "$zip_name")"
  if [ "$after" != "${BEFORE[$idx]}" ]; then
    echo "ERROR: release archive changed during installer build: $zip_name" >&2
    exit 1
  fi
  echo "✓ release archive preserved: $zip_name"
  idx=$((idx + 1))
done

echo "✓ Both Windows user installers built and verified."

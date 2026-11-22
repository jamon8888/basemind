#!/usr/bin/env bash
#MISE description="Smoke-test a macOS Basemind DMG (mount, validate, install, run)"
#MISE alias="package:macos-smoke"
set -euo pipefail

[ "$#" -ge 1 ] || {
  printf 'usage: mise run package:macos-smoke <artifact.dmg> [arm64|universal]\n' >&2
  exit 2
}
bash tests/macos_dmg_smoke.sh "$@"

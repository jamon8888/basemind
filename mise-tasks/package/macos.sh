#!/usr/bin/env bash
#MISE description="Build and package macOS arm64 + universal DMGs with arch/minos validation"
#MISE alias="package:macos"
set -euo pipefail

bash scripts/macos/package.sh --build "$@"

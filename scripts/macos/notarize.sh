#!/usr/bin/env bash
# macOS code-signing / notarization / stapling helpers.
#
# No secret is stored in this repository or passed on a command line by this
# script. The two inputs are opaque references resolved from the local keychain:
#   MACOS_SIGNING_IDENTITY  Developer ID Application identity name ("Developer
#                           ID Application: ... (TEAMID)"). When unset, files
#                           are signed ad-hoc ("-") for local testing only.
#   MACOS_NOTARY_PROFILE    Name of a keychain profile created separately with
#                           `xcrun notarytool store-credentials <profile> ...`.
#                           The Apple ID / team ID / app password live only in
#                           the keychain; this script only names the profile.
#
# Usage:
#   notarize.sh sign <path>          # a Mach-O file or a staging directory
#   notarize.sh submit <dmg>         # notarytool submit --wait, then report
#   notarize.sh staple <dmg>         # xcrun stapler staple
#   notarize.sh status               # print which references are configured
set -euo pipefail

die() {
  printf 'notarize: error: %s\n' "$*" >&2
  exit 1
}

identity="${MACOS_SIGNING_IDENTITY:-}"
profile="${MACOS_NOTARY_PROFILE:-}"

cmd_status() {
  if [ -n "$identity" ]; then
    printf 'signing identity: configured (%s)\n' "$identity"
  else
    printf 'signing identity: none -> ad-hoc signing (local testing only)\n'
  fi
  if [ -n "$profile" ]; then
    printf 'notary profile: configured (%s)\n' "$profile"
  else
    printf 'notary profile: none -> notarization/stapling skipped\n'
  fi
}

sign_one() {
  local file="$1"
  if [ -n "$identity" ]; then
    codesign --force --options runtime --timestamp --sign "$identity" "$file"
  else
    codesign --force --sign - "$file"
  fi
  codesign --verify --strict "$file"
  printf 'signed: %s\n' "$file"
}

cmd_sign() {
  [ "$#" -eq 1 ] || die "usage: notarize.sh sign <path>"
  local path="$1"
  command -v codesign >/dev/null 2>&1 || die "codesign is unavailable"
  if [ -f "$path" ]; then
    sign_one "$path"
    return
  fi
  [ -d "$path" ] || die "not a file or directory: $path"

  # Sign nested dylibs before the executable that loads them, so the seal is
  # computed over an already-valid nested signature.
  local -a libs=() bins=()
  while IFS= read -r -d '' f; do
    if [ -n "$(lipo -archs "$f" 2>/dev/null)" ]; then
      case "$f" in
      *.dylib) libs+=("$f") ;;
      *) bins+=("$f") ;;
      esac
    fi
  done < <(find "$path" -type f -print0)
  local f
  if [ "${#libs[@]}" -gt 0 ]; then
    for f in "${libs[@]}"; do sign_one "$f"; done
  fi
  if [ "${#bins[@]}" -gt 0 ]; then
    for f in "${bins[@]}"; do sign_one "$f"; done
  fi
  [ "${#libs[@]}" -gt 0 ] || [ "${#bins[@]}" -gt 0 ] || die "no Mach-O files under $path"
}

cmd_submit() {
  [ "$#" -eq 1 ] || die "usage: notarize.sh submit <dmg>"
  local dmg="$1"
  [ -f "$dmg" ] || die "not a file: $dmg"
  if [ -z "$profile" ]; then
    printf 'notarize: no MACOS_NOTARY_PROFILE; skipping submission for %s\n' "$dmg"
    return 0
  fi
  if [ -z "$identity" ] || [ "$identity" = "-" ]; then
    die "notarization requires a Developer ID signed DMG, not an ad-hoc signature"
  fi
  command -v xcrun >/dev/null 2>&1 || die "xcrun is unavailable"
  printf 'notarize: submitting %s with keychain profile %s\n' "$(basename "$dmg")" "$profile"
  xcrun notarytool submit "$dmg" --keychain-profile "$profile" --wait
}

cmd_staple() {
  [ "$#" -eq 1 ] || die "usage: notarize.sh staple <dmg>"
  local dmg="$1"
  [ -f "$dmg" ] || die "not a file: $dmg"
  if [ -z "$profile" ]; then
    printf 'notarize: no MACOS_NOTARY_PROFILE; skipping stapling for %s\n' "$dmg"
    return 0
  fi
  command -v xcrun >/dev/null 2>&1 || die "xcrun is unavailable"
  xcrun stapler staple "$dmg"
  xcrun stapler validate "$dmg"
}

[ "$#" -gt 0 ] || die "usage: notarize.sh <sign|submit|staple|status> [path]"
cmd="$1"
shift
case "$cmd" in
sign) cmd_sign "$@" ;;
submit) cmd_submit "$@" ;;
staple) cmd_staple "$@" ;;
status) cmd_status "$@" ;;
*) die "unknown command: $cmd" ;;
esac

#!/usr/bin/env bash
#
# Release artifact shape checks.
#
# These checks run inside the platform release workflows before artifacts are
# uploaded. They are intentionally boring and strict: a release artifact that
# cannot expose `con-cli` must fail before it can reach GitHub Releases or an
# updater appcast.

set -euo pipefail

fail() {
  printf '[release-verify] ERROR: %s\n' "$*" >&2
  exit 1
}

log() {
  printf '[release-verify] %s\n' "$*"
}

require_file() {
  local path="$1"
  [[ -f "$path" ]] || fail "missing file: $path"
}

require_executable() {
  local path="$1"
  [[ -x "$path" ]] || fail "missing executable: $path"
}

require_desktop_entry() {
  local desktop="$1"
  local entry="$2"
  grep -Fqx "$entry" "$desktop" \
    || fail "$(basename "$desktop") is missing desktop entry: $entry"
}

verify_terminal_desktop_contract() {
  local desktop="$1"
  require_file "$desktop"
  require_desktop_entry "$desktop" "X-TerminalArgExec=-e"
  require_desktop_entry "$desktop" "X-TerminalArgDir=--working-directory="
  require_desktop_entry "$desktop" "X-TerminalArgTitle=--title="
  require_desktop_entry "$desktop" "X-TerminalArgAppId=--app-id="
}

verify_linux() {
  local tarball="$1"
  local checksum="$2"

  require_file "$tarball"
  require_file "$checksum"

  local tarball_name
  tarball_name="$(basename "$tarball")"

  log "checking Linux checksum"
  (
    cd "$(dirname "$checksum")"
    if command -v sha256sum >/dev/null 2>&1; then
      sha256sum -c "$(basename "$checksum")"
    else
      shasum -a 256 -c "$(basename "$checksum")"
    fi
  )

  log "checking Linux tarball layout"
  tar -tzf "$tarball" | grep -E "/con$" >/dev/null \
    || fail "$tarball_name does not contain con"
  tar -tzf "$tarball" | grep -E "/con-cli$" >/dev/null \
    || fail "$tarball_name does not contain con-cli"
  tar -tzf "$tarball" | grep -E "/co\\.nowledge\\.con\\.desktop$" >/dev/null \
    || fail "$tarball_name does not contain co.nowledge.con.desktop"

  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN

  tar -xzf "$tarball" -C "$tmp"
  local root_count
  root_count="$(find "$tmp" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d '[:space:]')"
  [[ "$root_count" == "1" ]] \
    || fail "$tarball_name must extract to exactly one top-level directory, found $root_count"

  local root
  root="$(find "$tmp" -mindepth 1 -maxdepth 1 -type d -print -quit)"
  [[ -n "$root" ]] || fail "$tarball_name did not extract to a top-level directory"

  require_executable "$root/con"
  require_executable "$root/con-cli"
  require_file "$root/co.nowledge.con.desktop"
  require_desktop_entry "$root/co.nowledge.con.desktop" "TryExec=con"
  verify_terminal_desktop_contract "$root/co.nowledge.con.desktop"
  "$root/con-cli" --help >/dev/null
  log "Linux artifact OK: $tarball_name"
}

verify_macos() {
  local app="$1"
  local zip="$2"
  local dmg="$3"
  local checksum="$4"

  [[ -d "$app" ]] || fail "missing app bundle: $app"
  require_file "$zip"
  require_file "$dmg"
  require_file "$checksum"

  require_executable "$app/Contents/MacOS/con"
  require_executable "$app/Contents/MacOS/con-cli"
  "$app/Contents/MacOS/con-cli" --help >/dev/null

  log "checking macOS checksum"
  (
    cd "$(dirname "$checksum")"
    shasum -a 256 -c "$(basename "$checksum")"
  )

  log "checking macOS ZIP layout"
  unzip -l "$zip" | grep -F ".app/Contents/MacOS/con-cli" >/dev/null \
    || fail "$(basename "$zip") does not contain bundled con-cli"

  log "checking macOS DMG layout"
  local mount_point
  mount_point="$(mktemp -d)"
  hdiutil attach -quiet -nobrowse -mountpoint "$mount_point" "$dmg"
  trap 'hdiutil detach -quiet "$mount_point" >/dev/null 2>&1 || true; rm -rf "$mount_point"' RETURN

  local mounted_app=""
  for candidate in "$mount_point"/*.app; do
    if [[ -d "$candidate" ]]; then
      mounted_app="$candidate"
      break
    fi
  done
  [[ -n "$mounted_app" ]] || fail "$(basename "$dmg") does not contain an app bundle"
  require_executable "$mounted_app/Contents/MacOS/con"
  require_executable "$mounted_app/Contents/MacOS/con-cli"
  "$mounted_app/Contents/MacOS/con-cli" --help >/dev/null

  log "macOS artifact OK: $(basename "$dmg")"
}

case "${1:-}" in
  desktop)
    [[ "$#" -eq 2 ]] || fail "usage: $0 desktop <desktop-entry>"
    verify_terminal_desktop_contract "$2"
    log "terminal desktop contract OK: $(basename "$2")"
    ;;
  linux)
    [[ "$#" -eq 3 ]] || fail "usage: $0 linux <tarball> <checksum>"
    verify_linux "$2" "$3"
    ;;
  macos)
    [[ "$#" -eq 5 ]] || fail "usage: $0 macos <app> <zip> <dmg> <checksum>"
    verify_macos "$2" "$3" "$4" "$5"
    ;;
  *)
    fail "usage: $0 {desktop|linux|macos} ..."
    ;;
esac

#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

log() {
  printf '[macos-release] %s\n' "$*"
}

fail() {
  printf '[macos-release] ERROR: %s\n' "$*" >&2
  exit 1
}

require_cmd() {
  local cmd="$1"
  command -v "$cmd" >/dev/null 2>&1 || fail "missing required command: $cmd"
}

decode_base64_to_file() {
  local output_path="$1"

  # Pre-create with restricted permissions to avoid exposing secrets
  # between write and chmod.
  install -m 600 /dev/null "$output_path"

  if printf 'TQ==' | base64 --decode >/dev/null 2>&1; then
    base64 --decode >"$output_path"
    return
  fi

  if printf 'TQ==' | base64 -D >/dev/null 2>&1; then
    base64 -D >"$output_path"
    return
  fi

  fail "base64 decoder with --decode or -D is required"
}

read_workspace_version() {
  awk -F'"' '
    /^\[workspace\.package\]$/ { in_workspace = 1; next }
    /^\[/ { in_workspace = 0 }
    in_workspace && /^version = / { print $2; exit }
  ' "$REPO_ROOT/Cargo.toml"
}

normalize_arch() {
  case "${1:-$(uname -m)}" in
    arm64|aarch64)
      printf 'arm64\n'
      ;;
    x86_64|amd64)
      printf 'x86_64\n'
      ;;
    *)
      fail "unsupported macOS architecture: ${1:-unknown}"
      ;;
  esac
}

default_rust_target() {
  case "$1" in
    arm64)
      printf 'aarch64-apple-darwin\n'
      ;;
    x86_64)
      printf 'x86_64-apple-darwin\n'
      ;;
    *)
      fail "unsupported architecture for Rust target: $1"
      ;;
  esac
}

derive_marketing_version() {
  local version="$1"
  printf '%s\n' "${version%%[-+]*}"
}

derive_build_number() {
  local build_number="${CON_BUILD_NUMBER:-}"

  if [[ -n "$build_number" ]]; then
    printf '%s\n' "$build_number"
    return
  fi

  # Use GITHUB_RUN_NUMBER when available — it is a monotonically increasing
  # integer scoped to the workflow, which gives Sparkle a reliable
  # "always-increasing" value for CFBundleVersion regardless of channel.
  if [[ -n "${GITHUB_RUN_NUMBER:-}" ]]; then
    printf '%s\n' "$GITHUB_RUN_NUMBER"
    return
  fi

  # Local fallback: 0.  Dev channel never polls for updates, so the
  # build number only appears in Finder "Get Info".  Using 0 ensures
  # any CI build (GITHUB_RUN_NUMBER >= 1) is always considered newer,
  # preventing the version-comparison trap where a local build's large
  # epoch timestamp makes CI releases look like downgrades.
  printf '0\n'
}

setup_release_env() {
  export CON_CHANNEL="${CON_CHANNEL:-stable}"
  case "$CON_CHANNEL" in
    stable|beta|dev)
      ;;
    *)
      fail "CON_CHANNEL must be 'stable', 'beta', or 'dev', got: $CON_CHANNEL"
      ;;
  esac

  export CON_ARCH="$(normalize_arch "${CON_ARCH:-}")"
  export CON_RUST_TARGET="${CON_RUST_TARGET:-$(default_rust_target "$CON_ARCH")}"
  export CON_APP_VERSION="${CON_APP_VERSION:-$(read_workspace_version)}"
  export CON_MARKETING_VERSION="${CON_MARKETING_VERSION:-$(derive_marketing_version "$CON_APP_VERSION")}"
  export CON_BUILD_NUMBER="$(derive_build_number)"
  export CON_BUNDLE_ID_BASE="${CON_BUNDLE_ID_BASE:-co.nowledge.con}"
  export CON_MINIMUM_SYSTEM_VERSION="${CON_MINIMUM_SYSTEM_VERSION:-10.15.7}"
  export CON_ICON_SOURCE="${CON_ICON_SOURCE:-$REPO_ROOT/assets/app-icons/con-raccoon-tail-A6.png}"
  export CON_DIST_ROOT="${CON_DIST_ROOT:-$REPO_ROOT/dist/macos/$CON_CHANNEL/$CON_ARCH}"

  local default_bundle_id="$CON_BUNDLE_ID_BASE"
  local default_app_name="con"
  if [[ "$CON_CHANNEL" == "beta" ]]; then
    default_bundle_id="${CON_BUNDLE_ID_BASE}.beta"
    default_app_name="con Beta"
  elif [[ "$CON_CHANNEL" == "dev" ]]; then
    default_bundle_id="${CON_BUNDLE_ID_BASE}.dev"
    default_app_name="con Dev"
  fi
  export CON_BUNDLE_ID="${CON_BUNDLE_ID:-$default_bundle_id}"
  export CON_APP_NAME="${CON_APP_NAME:-$default_app_name}"

  # Derive Sparkle feed URL from channel + arch if not explicitly set.
  # Dev builds never poll and must not inherit a public feed URL from
  # the caller's environment.
  # Pattern: https://con-releases.nowledge.co/appcast/{channel}-macos-{arch}.xml
  if [[ "$CON_CHANNEL" == "dev" ]]; then
    unset CON_SPARKLE_FEED_URL
  elif [[ -z "${CON_SPARKLE_FEED_URL:-}" && -n "${CON_SPARKLE_PUBLIC_ED_KEY:-}" ]]; then
    export CON_SPARKLE_FEED_URL="https://con-releases.nowledge.co/appcast/${CON_CHANNEL}-macos-${CON_ARCH}.xml"
  fi

  export CON_APP_BUNDLE_PATH="$CON_DIST_ROOT/$CON_APP_NAME.app"
  export CON_APP_ZIP_PATH="$CON_DIST_ROOT/${CON_APP_NAME// /-}-${CON_APP_VERSION}-macos-${CON_ARCH}.zip"
  export CON_DMG_PATH="$CON_DIST_ROOT/${CON_APP_NAME// /-}-${CON_APP_VERSION}-macos-${CON_ARCH}.dmg"
  export CON_CHECKSUM_PATH="$CON_DIST_ROOT/SHA256SUMS-macos-$CON_ARCH.txt"
}

signing_identity() {
  if [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]]; then
    printf '%s\n' "$APPLE_SIGNING_IDENTITY"
    return
  fi

  if [[ "${CON_ALLOW_ADHOC_SIGNING:-0}" == "1" ]]; then
    printf '%s\n' '-'
    return
  fi

  fail "APPLE_SIGNING_IDENTITY is required unless CON_ALLOW_ADHOC_SIGNING=1"
}

have_notary_credentials() {
  if [[ -n "${APPLE_NOTARY_KEY_PATH:-}" && -n "${APPLE_NOTARY_KEY_ID:-}" && -n "${APPLE_NOTARY_ISSUER_ID:-}" ]]; then
    return 0
  fi

  if [[ -n "${APPLE_NOTARY_API_KEY_BASE64:-}" && -n "${APPLE_NOTARY_KEY_ID:-}" && -n "${APPLE_NOTARY_ISSUER_ID:-}" ]]; then
    return 0
  fi

  if [[ -n "${APPLE_ID:-}" && -n "${APPLE_APP_SPECIFIC_PASSWORD:-}" && -n "${APPLE_TEAM_ID:-}" ]]; then
    return 0
  fi

  return 1
}

prepare_notary_key_if_needed() {
  if [[ -n "${APPLE_NOTARY_KEY_PATH:-}" ]]; then
    if ! grep -q "BEGIN PRIVATE KEY" "$APPLE_NOTARY_KEY_PATH" 2>/dev/null; then
      fail "APPLE_NOTARY_KEY_PATH does not point to a valid .p8 private key"
    fi
    printf '%s\n' "$APPLE_NOTARY_KEY_PATH"
    return
  fi

  if [[ -z "${APPLE_NOTARY_API_KEY_BASE64:-}" ]]; then
    return
  fi

  local key_path="$CON_DIST_ROOT/AuthKey_${APPLE_NOTARY_KEY_ID}.p8"
  mkdir -p "$CON_DIST_ROOT"
  printf '%s' "$APPLE_NOTARY_API_KEY_BASE64" | decode_base64_to_file "$key_path"
  if ! grep -q "BEGIN PRIVATE KEY" "$key_path" 2>/dev/null; then
    rm -f "$key_path"
    fail "APPLE_NOTARY_API_KEY_BASE64 is present but does not decode to a valid .p8 private key"
  fi
  chmod 600 "$key_path"
  export APPLE_NOTARY_KEY_PATH="$key_path"
  printf '%s\n' "$key_path"
}

notarytool_submit() {
  local artifact="$1"
  require_cmd xcrun

  if [[ "${CON_SKIP_NOTARIZATION:-0}" == "1" ]]; then
    log "Skipping notarization for $artifact because CON_SKIP_NOTARIZATION=1"
    return
  fi

  if ! have_notary_credentials; then
    if [[ "${CON_REQUIRE_NOTARIZATION:-0}" == "1" ]]; then
      fail "notarization credentials are missing"
    fi
    log "Skipping notarization for $artifact because no credentials are configured"
    return
  fi

  if [[ -n "${APPLE_NOTARY_KEY_ID:-}" && -n "${APPLE_NOTARY_ISSUER_ID:-}" ]]; then
    local key_path
    key_path="$(prepare_notary_key_if_needed)"
    log "Submitting $artifact for notarization with App Store Connect API key"
    xcrun notarytool submit "$artifact" \
      --key "$key_path" \
      --key-id "$APPLE_NOTARY_KEY_ID" \
      --issuer "$APPLE_NOTARY_ISSUER_ID" \
      --wait
    return
  fi

  log "Submitting $artifact for notarization with Apple ID credentials"
  xcrun notarytool submit "$artifact" \
    --apple-id "$APPLE_ID" \
    --password "$APPLE_APP_SPECIFIC_PASSWORD" \
    --team-id "$APPLE_TEAM_ID" \
    --wait
}

generate_info_plist() {
  local plist_path="$1"
  local sparkle_keys=""

  if [[ -n "${CON_SPARKLE_FEED_URL:-}" ]]; then
    sparkle_keys+="  <key>SUFeedURL</key>\n  <string>${CON_SPARKLE_FEED_URL}</string>\n"
  fi

  if [[ -n "${CON_SPARKLE_PUBLIC_ED_KEY:-}" ]]; then
    sparkle_keys+="  <key>SUPublicEDKey</key>\n  <string>${CON_SPARKLE_PUBLIC_ED_KEY}</string>\n"
  fi

  if [[ -n "$sparkle_keys" ]]; then
    sparkle_keys+="  <key>SUEnableAutomaticChecks</key>\n  <true/>\n"
  fi

  cat >"$plist_path" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "https://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>${CON_APP_NAME}</string>
  <key>CFBundleExecutable</key>
  <string>con</string>
  <key>CFBundleIconFile</key>
  <string>con.icns</string>
  <key>CFBundleIdentifier</key>
  <string>${CON_BUNDLE_ID}</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>${CON_APP_NAME}</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>${CON_MARKETING_VERSION}</string>
  <key>CFBundleVersion</key>
  <string>${CON_BUILD_NUMBER}</string>
  <key>LSMinimumSystemVersion</key>
  <string>${CON_MINIMUM_SYSTEM_VERSION}</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSPrincipalClass</key>
  <string>NSApplication</string>
  <key>ConReleaseChannel</key>
  <string>${CON_CHANNEL}</string>
$(printf '%b' "$sparkle_keys")
</dict>
</plist>
EOF
}

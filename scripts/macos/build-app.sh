#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=scripts/macos/common.sh
source "$SCRIPT_DIR/common.sh"

setup_release_env

require_cmd cargo
require_cmd iconutil
require_cmd sips
require_cmd rsync
require_cmd mkdir

mkdir -p "$CON_DIST_ROOT"

log "Building con and con-cli for $CON_RUST_TARGET"
(
  cd "$REPO_ROOT"
  CON_REQUIRE_GHOSTTY_INITIAL_OUTPUT="${CON_REQUIRE_GHOSTTY_INITIAL_OUTPUT:-1}" \
    cargo build --locked --release --target "$CON_RUST_TARGET" -p con -p con-cli
)

app_root="$CON_APP_BUNDLE_PATH"
contents_dir="$app_root/Contents"
macos_dir="$contents_dir/MacOS"
resources_dir="$contents_dir/Resources"
binary_path="$REPO_ROOT/target/$CON_RUST_TARGET/release/con"
cli_binary_path="$REPO_ROOT/target/$CON_RUST_TARGET/release/con-cli"

rm -rf "$app_root"
mkdir -p "$macos_dir" "$resources_dir"

log "Creating app bundle at $app_root"
rsync -a "$binary_path" "$macos_dir/con"
chmod 755 "$macos_dir/con"
rsync -a "$cli_binary_path" "$macos_dir/con-cli"
chmod 755 "$macos_dir/con-cli"
# Ghostty's bundled shell integration invokes `$GHOSTTY_BIN_DIR/ghostty
# +ssh-cache`. Con owns that protocol now, so keep the upstream integration
# unchanged and provide a bundle-local compatibility entry point.
ln -s con-cli "$macos_dir/ghostty"

ghostty_resources_dir="$(find "$REPO_ROOT/target/$CON_RUST_TARGET/release/build" -path '*/out/ghostty-src/zig-out/share/ghostty' | head -n 1)"
if [[ -z "$ghostty_resources_dir" || ! -d "$ghostty_resources_dir" ]]; then
  log "Ghostty resources not found in cargo build output"
  exit 1
fi
rsync -a "$ghostty_resources_dir/" "$resources_dir/ghostty/"
log "Embedded Ghostty resources from $ghostty_resources_dir"

ghostty_share_dir="$(dirname "$ghostty_resources_dir")"
ghostty_terminfo_dir="$ghostty_share_dir/terminfo"
if [[ ! -d "$ghostty_terminfo_dir" ]]; then
  fail "Ghostty terminfo directory not found in cargo build output: $ghostty_terminfo_dir"
fi
ghostty_terminfo_entry="$(find "$ghostty_terminfo_dir" -type f -name xterm-ghostty -print -quit)"
if [[ -z "$ghostty_terminfo_entry" || ! -r "$ghostty_terminfo_entry" ]]; then
  fail "Ghostty xterm-ghostty terminfo entry not found under cargo build output: $ghostty_terminfo_dir"
fi
rsync -a "$ghostty_terminfo_dir/" "$resources_dir/terminfo/"
log "Embedded Ghostty terminfo from $ghostty_terminfo_dir"

iconset_parent="$(mktemp -d "$CON_DIST_ROOT/iconset.XXXXXX")"
iconset_dir="$iconset_parent/con.iconset"
mkdir -p "$iconset_dir"
trap 'rm -rf "$iconset_parent"' EXIT

for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$CON_ICON_SOURCE" --out "$iconset_dir/icon_${size}x${size}.png" >/dev/null
done

sips -z 32 32 "$CON_ICON_SOURCE" --out "$iconset_dir/icon_16x16@2x.png" >/dev/null
sips -z 64 64 "$CON_ICON_SOURCE" --out "$iconset_dir/icon_32x32@2x.png" >/dev/null
sips -z 256 256 "$CON_ICON_SOURCE" --out "$iconset_dir/icon_128x128@2x.png" >/dev/null
sips -z 512 512 "$CON_ICON_SOURCE" --out "$iconset_dir/icon_256x256@2x.png" >/dev/null
cp "$CON_ICON_SOURCE" "$iconset_dir/icon_512x512@2x.png"

iconutil -c icns "$iconset_dir" -o "$resources_dir/con.icns"
generate_info_plist "$contents_dir/Info.plist"

printf 'APPL????' >"$contents_dir/PkgInfo"

# Embed Sparkle.framework if available (downloaded by scripts/sparkle/download.sh)
sparkle_framework="${SPARKLE_DIR:-$REPO_ROOT/.sparkle}/Sparkle.framework"
if [[ -d "$sparkle_framework" ]]; then
  frameworks_dir="$contents_dir/Frameworks"
  mkdir -p "$frameworks_dir"
  rsync -a "$sparkle_framework" "$frameworks_dir/"
  log "Embedded Sparkle.framework"
else
  log "Sparkle.framework not found — auto-update will be disabled at runtime"
fi

log "App bundle ready: $CON_APP_BUNDLE_PATH"

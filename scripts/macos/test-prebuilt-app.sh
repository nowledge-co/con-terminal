#!/usr/bin/env bash

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/common.sh"
[[ "$(uname -s)" == "Darwin" ]] || fail "prebuilt bundle tests require macOS"
root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT

run_case() (
  local name="$1" mutation="$2" expected="$3"
  export CON_CHANNEL=beta CON_ARCH=x86_64 CON_APP_VERSION=0.1.0-beta.132 CON_BUILD_NUMBER=53
  export CON_DIST_ROOT="$root/$name/output"
  setup_release_env
  local app="$root/$name/source/$CON_APP_NAME.app"
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Frameworks/Sparkle.framework"
  # Extract an existing OS binary slice, without compiling or executing it.
  lipo /usr/bin/true -thin x86_64 -output "$app/Contents/MacOS/con"
  chmod +x "$app/Contents/MacOS/con"
  cp "$app/Contents/MacOS/con" "$app/Contents/MacOS/con-cli"
  # Exercise the same real-tool check used before the build artifact is uploaded.
  verify_native_binary_architecture "$app/Contents/MacOS/con" "$CON_ARCH"
  verify_native_binary_architecture "$app/Contents/MacOS/con-cli" "$CON_ARCH"
  ln -s con-cli "$app/Contents/MacOS/ghostty"
  generate_info_plist "$app/Contents/Info.plist"
  [[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleDocumentTypes:0:LSItemContentTypes:0' "$app/Contents/Info.plist")" == public.folder ]] \
    || fail "$name: bundle does not advertise opening folders"
  case "$mutation" in
    version) /usr/libexec/PlistBuddy -c 'Set :CFBundleVersion 54' "$app/Contents/Info.plist" ;;
    channel) /usr/libexec/PlistBuddy -c 'Set :ConReleaseChannel dev' "$app/Contents/Info.plist" ;;
    identity) /usr/libexec/PlistBuddy -c 'Set :CFBundleIdentifier invalid.bundle' "$app/Contents/Info.plist" ;;
    missing-cli) rm "$app/Contents/MacOS/con-cli" ;;
    missing-link) rm "$app/Contents/MacOS/ghostty" ;;
    missing-framework) rm -r "$app/Contents/Frameworks/Sparkle.framework" ;;
    universal-cli) cp /usr/bin/true "$app/Contents/MacOS/con-cli" ;;
    invalid-binary) printf 'not a Mach-O executable\n' >"$app/Contents/MacOS/con" ;;
  esac
  export CON_PREBUILT_APP_ARCHIVE="$root/$name/native-app.zip"
  ditto -c -k --keepParent "$app" "$CON_PREBUILT_APP_ARCHIVE"
  if [[ "$mutation" == "architecture" ]]; then
    export CON_ARCH=arm64
  fi
  mkdir -p "$CON_APP_BUNDLE_PATH"
  touch "$CON_APP_BUNDLE_PATH/unchanged"
  if bash "$SCRIPT_DIR/prepare-app.sh" >"$root/$name/log" 2>&1; then
    [[ "$expected" == "success" ]] || fail "$name: invalid bundle was accepted"
    [[ ! -e "$CON_APP_BUNDLE_PATH/unchanged" ]] || fail "$name: bundle was not replaced"
    [[ -x "$CON_APP_BUNDLE_PATH/Contents/MacOS/con-cli" ]] || fail "$name: executable bit lost"
    [[ "$(readlink "$CON_APP_BUNDLE_PATH/Contents/MacOS/ghostty")" == "con-cli" ]] \
      || fail "$name: symlink lost"
  else
    [[ "$expected" == "failure" ]] || fail "$name: valid bundle rejected"
    [[ -f "$CON_APP_BUNDLE_PATH/unchanged" ]] || fail "$name: failed preparation replaced existing bundle"
  fi
  printf 'PASS: %s\n' "$name"
)

run_case valid none success
for mutation in version channel identity architecture missing-cli missing-link missing-framework universal-cli invalid-binary; do
  run_case "$mutation" "$mutation" failure
done

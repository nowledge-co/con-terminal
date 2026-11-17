#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
framework="${SPARKLE_FRAMEWORK:-/Applications/con Beta.app/Contents/Frameworks/Sparkle.framework}"
[[ -d "$framework" ]] || { echo 'Set SPARKLE_FRAMEWORK to an actual Sparkle.framework' >&2; exit 1; }
fixture_root="$(mktemp -d)"
trap 'rm -rf "$fixture_root"' EXIT
app="$fixture_root/Consent.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Frameworks"
cp -R "$framework" "$app/Contents/Frameworks/"
export CONSENT_APP="$app"
python3 - <<'PY'
import os,plistlib,uuid
from pathlib import Path
p=Path(os.environ['CONSENT_APP'])/'Contents/Info.plist'
p.write_bytes(plistlib.dumps(dict(CFBundleExecutable='probe', CFBundleIdentifier='co.nowledge.con.updater-test.'+uuid.uuid4().hex,
    CFBundleName='Consent Test', CFBundleVersion='1', CFBundleShortVersionString='0.1.0', CFBundlePackageType='APPL',
    SUFeedURL='https://example.invalid/consent-test.xml', SUPublicEDKey='Zs1gi5KLiOUODv6qM/rxxDzv4HKUsLh+n+CrpqPPdjM=',
    SUEnableAutomaticChecks=False, SUAllowsAutomaticUpdates=False, SUAutomaticallyUpdate=False)))
PY
xcrun clang -fobjc-arc -framework AppKit -framework Foundation \
  "$SCRIPT_DIR/test-update-preferences.m" "$SCRIPT_DIR/../../crates/con-app/src/objc/sparkle_trampoline.m" \
  -o "$app/Contents/MacOS/probe"
codesign --force --sign - "$app" >/dev/null 2>&1
"$app/Contents/MacOS/probe"

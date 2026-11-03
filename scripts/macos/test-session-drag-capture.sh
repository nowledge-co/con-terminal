#!/usr/bin/env bash
set -euo pipefail
repo_dir="$(cd "$(dirname "$0")/../.." && pwd)"
test_dir="$(mktemp -d "${TMPDIR:-/tmp}/con-session-drag.XXXXXX")"
trap 'rm -rf "$test_dir"' EXIT
clang -fobjc-arc -fmodules -Werror -framework AppKit -framework QuartzCore \
  "$repo_dir/crates/con-app/tests/session_drag_capture.m" -o "$test_dir/capture-test"
"$test_dir/capture-test"

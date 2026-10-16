#!/usr/bin/env bash
# Each installation case intentionally isolates its runner environment.
# shellcheck disable=SC2030,SC2031
set -euo pipefail
# shellcheck source=scripts/ci/install-zig.sh
source "$(cd "$(dirname "$0")" && pwd)/install-zig.sh"

fixture="$(mktemp -d "${TMPDIR:-/tmp}/con-zig-test.XXXXXX")"
trap 'rm -rf "$fixture"' EXIT
printf 'verified archive\n' > "$fixture/payload"
expected="$(shasum -a 256 "$fixture/payload")"
expected="${expected%% *}"

# Exercise the actual download policy without network delays. Reject missing
# bounds so a refactor cannot silently reintroduce the stalled-transfer bug.
curl() {
  local url output args=" $* "
  for required in '--connect-timeout 10' '--max-time 180' \
    '--speed-limit 1024' '--speed-time 30'; do
    if [[ "$args" != *" $required "* ]]; then
      echo "Missing transfer bound: $required" >&2
      exit 1
    fi
  done
  while [[ $# -gt 0 ]]; do
    case "$1" in
      https://*) url="$1" ;;
      --output) output="$2"; shift ;;
    esac
    shift
  done
  printf '%s\n' "$url" >> "$fixture/attempts"
  case "$url" in
    https://stalled) printf 'partial download' > "$output"; return 28 ;;
    https://corrupt) printf 'invalid archive' > "$output" ;;
    https://good) cp "$fixture/payload" "$output" ;;
    *) return 22 ;;
  esac
}

download_zig_archive "$fixture/archive" "$expected" \
  https://stalled https://corrupt https://good
cmp "$fixture/payload" "$fixture/archive"
[[ "$(wc -l < "$fixture/attempts" | tr -d ' ')" == 3 ]]
echo 'PASS: stalled and corrupt sources fall back to a verified archive'

: > "$fixture/attempts"
if download_zig_archive "$fixture/archive" "$expected" \
  https://stalled https://corrupt https://missing; then
  echo 'All failed sources unexpectedly succeeded' >&2
  exit 1
fi
[[ ! -e "$fixture/archive" ]]
[[ "$(wc -l < "$fixture/attempts" | tr -d ' ')" == 3 ]]
echo 'PASS: exhaustion fails closed and removes partial/stale archives'

download_zig_archive "$fixture/archive" "$expected" https://good https://missing
[[ "$(tail -1 "$fixture/attempts")" == https://good ]]
echo 'PASS: a verified first source stops further requests'

export GITHUB_PATH="$fixture/github-path"
if install_zig unknown-target > "$fixture/unsupported.log" 2>&1; then
  echo 'Unsupported target unexpectedly installed' >&2
  exit 1
fi
[[ ! -e "$GITHUB_PATH" ]]
echo 'PASS: unsupported targets never modify the runner PATH'

# Test the installation boundary with a tiny executable fixture, not a compiler.
(
  export RUNNER_TEMP="$fixture"
  mkdir "$fixture/zig-aarch64-macos-$ZIG_VERSION"
  printf '#!/bin/sh\necho %s\n' "$ZIG_VERSION" \
    > "$fixture/zig-aarch64-macos-$ZIG_VERSION/zig"
  chmod +x "$fixture/zig-aarch64-macos-$ZIG_VERSION/zig"
  tar -cJf "$fixture/valid.tar.xz" -C "$fixture" "zig-aarch64-macos-$ZIG_VERSION"
  download_zig_archive() { cp "$fixture/valid.tar.xz" "$1"; }
  install_zig aarch64-macos
  installed="$(tail -1 "$GITHUB_PATH")"
  [[ "$("$installed/zig" version)" == "$ZIG_VERSION" ]]
  [[ ! -e "$(dirname "$installed")/archive.tar.xz" ]]
  echo 'PASS: extraction and version validation precede PATH publication'
)

(
  export RUNNER_TEMP="$fixture"
  export GITHUB_PATH="$fixture/failed-path"
  printf '#!/bin/sh\necho wrong-version\n' \
    > "$fixture/zig-aarch64-macos-$ZIG_VERSION/zig"
  tar -cJf "$fixture/wrong.tar.xz" -C "$fixture" "zig-aarch64-macos-$ZIG_VERSION"
  download_zig_archive() { cp "$fixture/wrong.tar.xz" "$1"; }
  # Invoke as an ordinary command: an `if` around a shell function disables
  # errexit inside it and would not model the production script's behavior.
  set +e
  (set -e; install_zig aarch64-macos) > "$fixture/wrong.log" 2>&1
  result=$?
  set -e
  [[ "$result" != 0 && ! -e "$GITHUB_PATH" ]]
  echo 'PASS: a wrong executable version never modifies PATH'
)

(
  export RUNNER_TEMP="$fixture"
  export GITHUB_PATH="$fixture/broken-path"
  download_zig_archive() { printf 'not an archive' > "$1"; }
  set +e
  (set -e; install_zig aarch64-macos) > "$fixture/broken.log" 2>&1
  result=$?
  set -e
  [[ "$result" != 0 && ! -e "$GITHUB_PATH" ]]
  echo 'PASS: extraction failure never modifies PATH'
)

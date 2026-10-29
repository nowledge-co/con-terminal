#!/usr/bin/env bash
set -euo pipefail

# Keep in sync with mise.toml. Checksums: https://ziglang.org/download/index.json
ZIG_VERSION=0.17.0

download_zig_archive() {
  local archive="$1" expected="$2" base actual
  shift 2
  for base in "$@"; do
    echo "Downloading Zig from ${base}" >&2
    rm -f "$archive"
    # Try each independent source once, rather than spending the entire budget
    # retrying a dead source. All failure modes must reach the next mirror.
    if curl --fail --show-error --silent --location \
      --connect-timeout 10 --max-time 180 \
      --speed-limit 1024 --speed-time 30 \
      "$base" --output "$archive"; then
      if command -v sha256sum >/dev/null 2>&1; then
        actual="$(sha256sum "$archive")"
      else
        actual="$(shasum -a 256 "$archive")"
      fi
      actual="${actual%% *}"
      if [[ "$actual" == "$expected" ]]; then
        return 0
      fi
      echo "Checksum mismatch from ${base}; trying next source" >&2
    else
      echo "Download failed from ${base}; trying next source" >&2
    fi
  done
  rm -f "$archive"
  echo "Unable to download verified Zig ${ZIG_VERSION} from any source" >&2
  return 1
}

install_zig() (
  local target="${1:-}" expected directory staging version
  if [[ -z "$target" ]]; then
    case "$(uname -s):$(uname -m)" in
      Darwin:arm64) target=aarch64-macos ;;
      Darwin:x86_64) target=x86_64-macos ;;
      Linux:x86_64) target=x86_64-linux ;;
      *) echo "Unsupported native Zig host" >&2; return 1 ;;
    esac
  fi
  case "$target" in
    aarch64-macos) expected=b607e9b9234790a008116ae5bdb71c6243b84b9fb42a53a9e70fde41c06c536a ;;
    x86_64-macos) expected=4f9a1c5269aa17ebda5e6d3c2b89d6cbf36f7d2b22a0306e9ab98f25f95529c6 ;;
    x86_64-linux) expected=1cbe9df9f27e6b78d14ccbca43b6703a404ef79ef1c463de901d7f088d4e2026 ;;
    *) echo "Unsupported Zig target: $target" >&2; return 1 ;;
  esac
  : "${GITHUB_PATH:?GITHUB_PATH must be set by the CI runner}"
  directory="zig-${target}-${ZIG_VERSION}"
  staging="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/con-zig.XXXXXX")"
  trap 'rm -rf "$staging"' EXIT
  download_zig_archive "$staging/archive.tar.xz" "$expected" \
    "https://ziglang.org/download/${ZIG_VERSION}/${directory}.tar.xz" \
    "https://zig.linus.dev/zig/${directory}.tar.xz" \
    "https://pkg.hexops.org/zig/${directory}.tar.xz"
  tar -xJf "$staging/archive.tar.xz" -C "$staging"
  version="$("$staging/$directory/zig" version)"
  if [[ "$version" != "$ZIG_VERSION" ]]; then
    echo "Unexpected Zig version: $version (expected $ZIG_VERSION)" >&2
    return 1
  fi
  rm "$staging/archive.tar.xz"
  printf '%s\n' "$staging/$directory" >> "$GITHUB_PATH"
  trap - EXIT
  echo "Installed Zig $version for $target"
)

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  install_zig "$@"
fi

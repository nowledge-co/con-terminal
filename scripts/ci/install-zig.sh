#!/usr/bin/env bash
set -euo pipefail

# Keep in sync with mise.toml. Checksums: https://ziglang.org/download/index.json
ZIG_VERSION=0.16.0

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
    aarch64-macos) expected=b23d70deaa879b5c2d486ed3316f7eaa53e84acf6fc9cc747de152450d401489 ;;
    x86_64-macos) expected=0387557ed1877bc6a2e1802c8391953baddba76081876301c522f52977b52ba7 ;;
    x86_64-linux) expected=70e49664a74374b48b51e6f3fdfbf437f6395d42509050588bd49abe52ba3d00 ;;
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

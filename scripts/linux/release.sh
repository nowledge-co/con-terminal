#!/usr/bin/env bash
#
# Build a Linux release tarball for con. Mirrors the role of
# `scripts/macos/release.sh` and `release-windows.yml`'s "Package
# ZIP" step:
#   - cargo build -p con -p con-cli --release with the channel + version baked
#     in via CON_RELEASE_CHANNEL / CON_RELEASE_VERSION (the
#     non-macOS updater path uses option_env!() to read these),
#   - stage `con` + `con-cli` + LICENSE + README.md + a `.desktop`
#     entry + a 256x256 icon into a versioned directory,
#   - emit `con-<version>-linux-<arch>.tar.gz` plus a sha256 sum file
#     so the publish step can attach both to the GitHub release.
#
# Designed to be run both from CI (release-linux.yml) and locally
# (when verifying the artifact shape before tagging). Defaults to
# the host architecture; `CON_LINUX_ARCH=arm64` lets you label an
# aarch64 build the same way the macOS pipeline does.
#
# Output:
#   dist/con-<version>-linux-<arch>/         staging dir
#   dist/con-<version>-linux-<arch>.tar.gz   release artifact
#   dist/SHA256SUMS-linux.txt                checksum line for the publish step
#
# Required env (set by CI; can be overridden locally):
#   CON_RELEASE_VERSION   full tag-derived version, e.g. 0.1.0-beta.31
#   CON_RELEASE_CHANNEL   "stable" | "beta" | "dev"
#
# Optional:
#   CON_LINUX_ARCH        "x86_64" (default on x86_64 hosts) | "arm64"
#   CARGO_TARGET_DIR      forwarded to cargo if set

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$repo_root"

# ── Resolve metadata ────────────────────────────────────────────────────────

version="${CON_RELEASE_VERSION:-}"
if [[ -z "$version" ]]; then
  # Local invocation. Pull from the most recent annotated tag if we
  # can; otherwise fall back to the workspace package version.
  version="$(git describe --tags --abbrev=0 2>/dev/null | sed 's/^v//' || true)"
  if [[ -z "$version" ]]; then
    version="$(grep -E '^version' Cargo.toml | head -1 | cut -d'"' -f2)"
  fi
fi
[[ -n "$version" ]] || { echo "could not determine version" >&2; exit 1; }

channel="${CON_RELEASE_CHANNEL:-stable}"

host_arch="$(uname -m)"
case "${CON_LINUX_ARCH:-$host_arch}" in
  x86_64|amd64) art_arch="x86_64" ;;
  arm64|aarch64) art_arch="arm64" ;;
  *) echo "unsupported arch: ${CON_LINUX_ARCH:-$host_arch}" >&2; exit 1 ;;
esac

stage_name="con-${version}-linux-${art_arch}"
stage_dir="dist/${stage_name}"
tarball="dist/${stage_name}.tar.gz"
sha_file="dist/SHA256SUMS-linux.txt"
linux_app_id="co.nowledge.con"

echo "==> con linux release"
echo "    version : ${version}"
echo "    channel : ${channel}"
echo "    arch    : ${art_arch}"
echo "    tarball : ${tarball}"
echo

# Match the Windows + macOS pipelines: bake version + channel into
# the binary at compile time so option_env!() in
# crates/con-core/src/release_channel.rs picks them up.
export CON_RELEASE_VERSION="${version}"
export CON_RELEASE_CHANNEL="${channel}"

# Optional Zig-based cross-linker wrapper to target GLIBC 2.38 for
# backward compatibility and Flatpak runtime compatibility.
zig_bin=""
if command -v zig >/dev/null 2>&1; then
  zig_bin="$(command -v zig)"
elif [[ -n "${CON_ZIG_BIN:-}" && -x "${CON_ZIG_BIN:-}" ]]; then
  zig_bin="${CON_ZIG_BIN}"
else
  # Check common install locations
  zig_candidate="$(ls -d "$HOME"/.local/zig-*-linux-*/zig /tmp/zig-*-linux-*/zig /tmp/zig-*/zig 2>/dev/null | head -n 1 || true)"
  if [[ -n "$zig_candidate" && -x "$zig_candidate" ]]; then
    zig_bin="$zig_candidate"
  fi
fi

# Setup helper lib symlinks if missing (e.g. libxkbcommon-x11.so on systems without -devel)
mkdir -p target/syslibs
for libpath in /usr/lib64 /usr/lib /usr/lib/x86_64-linux-gnu /usr/lib/aarch64-linux-gnu; do
  if [[ -f "${libpath}/libxkbcommon-x11.so.0" && ! -f "target/syslibs/libxkbcommon-x11.so" ]]; then
    ln -sf "${libpath}/libxkbcommon-x11.so.0" "target/syslibs/libxkbcommon-x11.so"
  fi
done

if [[ -n "$zig_bin" ]]; then
  zig_target=""
  linker_wrapper=""
  case "$art_arch" in
    x86_64)
      zig_target="x86_64-linux-gnu.2.38"
      ;;
    arm64)
      zig_target="aarch64-linux-gnu.2.38"
      ;;
  esac

  if [[ -n "$zig_target" ]]; then
    linker_wrapper="$(mktemp "${TMPDIR:-/tmp}/con-zig-linker-${art_arch}.XXXXXX")"
    trap 'rm -f "$linker_wrapper"' EXIT
    case "$art_arch" in
      x86_64) export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="$linker_wrapper" ;;
      arm64) export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="$linker_wrapper" ;;
    esac
    cat > "$linker_wrapper" <<EOF
#!/bin/bash
args=()
for arg in "\$@"; do
  # Rust's aarch64 target adds this GNU ld workaround by default. Zig's
  # lld does not implement it; dropping only this optional flag keeps the
  # rest of the linker invocation and ABI unchanged.
  if [[ "\$arg" == "-Wl,--fix-cortex-a53-843419" ]]; then
    continue
  fi
  args+=("\$arg")
done
exec "$zig_bin" cc -target "$zig_target" -L"$repo_root/target/syslibs" "\${args[@]}"
EOF
    chmod +x "$linker_wrapper"
  fi
fi

echo "==> cargo build -p con -p con-cli --release"
cargo build -p con -p con-cli --release

# Resolve the actual target dir cargo wrote to. CARGO_TARGET_DIR
# overrides override the default.
target_dir="${CARGO_TARGET_DIR:-target}"
bin_path="${target_dir}/release/con"
cli_bin_path="${target_dir}/release/con-cli"
[[ -x "$bin_path" ]] || { echo "build did not produce $bin_path" >&2; exit 1; }
[[ -x "$cli_bin_path" ]] || { echo "build did not produce $cli_bin_path" >&2; exit 1; }

# ── Stage ───────────────────────────────────────────────────────────────────

echo "==> staging ${stage_dir}"
rm -rf "${stage_dir}"
mkdir -p "${stage_dir}"

# Strip debug info — drops the binary from ~300 MB to ~80 MB. The
# macOS pipeline does the equivalent via an Xcode strip phase; the
# Windows pipeline ships with debug-info stripped at link time.
cp "$bin_path" "${stage_dir}/con"
strip --strip-debug "${stage_dir}/con" || true
cp "$cli_bin_path" "${stage_dir}/con-cli"
strip --strip-debug "${stage_dir}/con-cli" || true

# Docs.
[[ -f LICENSE ]] && cp LICENSE "${stage_dir}/"
[[ -f README.md ]] && cp README.md "${stage_dir}/"

# Desktop entry. The filename, StartupWMClass, and runtime GPUI app_id must stay
# in sync so Wayland and X11 desktops group the running window with the launcher.
# The install.sh script rewrites the Exec line to
# point at the per-user install path before dropping it into
# ~/.local/share/applications, so the path here is just a default
# for users who hand-extract the tarball into /usr/local.
cat > "${stage_dir}/${linux_app_id}.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=con
GenericName=Terminal
Comment=GPU-accelerated terminal emulator with a built-in AI agent harness
Exec=/usr/local/bin/con %U
Icon=con
Terminal=false
Categories=System;TerminalEmulator;Utility;
Keywords=terminal;shell;command;cli;ai;agent;
StartupWMClass=${linux_app_id}
EOF

# 256x256 icon — freedesktop hicolor's bread-and-butter size, and
# what GNOME / KDE / xfce / wayland launchers all hit-test against
# first. Pull from the macOS app-icon set we already ship.
icon_src="assets/Con-macOS-Dark-256x256@2x.png"
if [[ -f "$icon_src" ]]; then
  cp "$icon_src" "${stage_dir}/con.png"
else
  echo "warning: ${icon_src} missing — Linux tarball ships without an app icon" >&2
fi

# ── Tarball + checksum ──────────────────────────────────────────────────────

echo "==> packaging ${tarball}"
tar -C dist -czf "${tarball}" "${stage_name}"

echo "==> sha256"
sha256_out=""
if command -v sha256sum >/dev/null 2>&1; then
  sha256_out="$(cd dist && sha256sum "$(basename "${tarball}")")"
elif command -v shasum >/dev/null 2>&1; then
  sha256_out="$(cd dist && shasum -a 256 "$(basename "${tarball}")")"
else
  echo "warning: no sha256sum / shasum found — skipping checksum file" >&2
fi
if [[ -n "$sha256_out" ]]; then
  echo "${sha256_out}" > "${sha_file}"
  echo "    ${sha256_out}"
fi

echo
echo "==> done"
# `sha_file` is always non-empty (it's a default-derived path), so the
# previous `${sha_file:+"$sha_file"}` always expanded — `ls` then
# failed loudly when checksum generation was skipped because neither
# sha256sum nor shasum was on PATH. Test for the file's existence
# instead so the listing matches what was actually produced.
if [[ -f "$sha_file" ]]; then
  ls -la "${tarball}" "${sha_file}"
else
  ls -la "${tarball}"
fi

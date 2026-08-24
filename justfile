# con — justfile
# https://github.com/casey/just
#
# Usage:
#   just          # list all recipes
#   just run      # run from source (current platform)
#   just install  # build and install (current platform)
#
# The `arch` parameter defaults to "" — each Unix recipe auto-detects via
# `uname -m` inside the shell body. Windows recipes never reference arch so
# `uname` is never invoked there.
# Override explicitly when needed: just arch=x86_64 macos-bundle

# Use cmd.exe on Windows so recipes work in a plain Developer Command Prompt
# without requiring Git Bash, Cygwin, or sh on PATH.
set windows-shell := ["cmd.exe", "/c"]

# ── defaults ──────────────────────────────────────────────────────────────────

# Release channel for macOS/Linux app bundles (stable | beta | dev)
channel := "stable"

# Target architecture. Empty = auto-detect inside each recipe (Unix only).
# Windows recipes never use this variable so uname is never called there.
arch := ""

# ── list ──────────────────────────────────────────────────────────────────────

# List all recipes (default)
default:
    @just --list

# ── universal dev commands ────────────────────────────────────────────────────
# These dispatch to the right platform recipe. Windows must use the `w*` cargo
# aliases because `CON` is a reserved DOS device name and the Windows binary is
# feature-gated as `con-app.exe`.

# Debug build — current platform
build:
    {{ if os() == "windows" { "cargo wbuild -p con" } else { "cargo build -p con" } }}

# Release build — current platform
build-release:
    {{ if os() == "windows" { "cargo wbuild -p con --release" } else { "cargo build --release -p con" } }}

# Run from source — current platform
run:
    {{ if os() == "windows" { "cargo wrun -p con" } else { "cargo run -p con" } }}

# Run the platform-appropriate test set
test:
    {{ if os() == "windows" { "cargo wtest -p con-core -p con-cli -p con-agent -p con-terminal" } else { "cargo test --workspace" } }}

# Check without building — current platform
check:
    {{ if os() == "windows" { "cargo wcheck -p con" } else { "cargo check --workspace" } }}

# Run clippy — current platform
lint:
    {{ if os() == "windows" { "cargo clippy --workspace --no-default-features --features con/bin-con-app -- -D warnings" } else { "cargo clippy --workspace -- -D warnings" } }}

# Clean cargo build artifacts
clean:
    cargo clean

# Build and install to the current platform's local development install path
install:
    just channel={{ channel }} arch={{ arch }} {{ if os() == "macos" { "macos-install" } else if os() == "linux" { "linux-install" } else if os() == "windows" { "windows-install" } else { "unsupported-platform" } }}

# Print the current package id, including the workspace version
version:
    @cargo pkgid -p con

unsupported-platform:
    @echo "Unsupported platform for this justfile"
    @exit 1

# ── macOS ─────────────────────────────────────────────────────────────────────

# [macOS] Build a local .app bundle — no signing, no notarization
# Output: dist/macos/{channel}/{arch}/con.app
macos-bundle channel=channel arch=arch:
    #!/usr/bin/env bash
    set -euo pipefail
    resolved_arch="{{ arch }}"
    if [[ -z "${resolved_arch}" ]]; then
        resolved_arch="$(uname -m | sed 's/aarch64/arm64/')"
    fi
    CON_CHANNEL={{ channel }} CON_ARCH="${resolved_arch}" ./scripts/macos/build-app.sh

# [macOS] Build .app and copy to /Applications (replaces existing)
macos-install channel=channel arch=arch: (macos-bundle channel arch)
    #!/usr/bin/env bash
    set -euo pipefail
    resolved_arch="{{ arch }}"
    if [[ -z "${resolved_arch}" ]]; then
        resolved_arch="$(uname -m | sed 's/aarch64/arm64/')"
    fi
    app_name="con"
    if [[ "{{ channel }}" == "beta" ]]; then app_name="con Beta"; fi
    if [[ "{{ channel }}" == "dev" ]];  then app_name="con Dev";  fi
    src="dist/macos/{{ channel }}/${resolved_arch}/${app_name}.app"
    dst="/Applications/${app_name}.app"
    echo "Installing ${src} → ${dst}"
    rm -rf "${dst}"
    cp -R "${src}" "${dst}"
    echo "Done. Launch ${app_name} from /Applications or Spotlight."

# [macOS] Ad-hoc signed bundle (no Apple Developer account needed; Gatekeeper will warn once)
macos-bundle-adhoc channel=channel arch=arch: (macos-bundle channel arch)
    #!/usr/bin/env bash
    set -euo pipefail
    resolved_arch="{{ arch }}"
    if [[ -z "${resolved_arch}" ]]; then
        resolved_arch="$(uname -m | sed 's/aarch64/arm64/')"
    fi
    app_name="con"
    if [[ "{{ channel }}" == "beta" ]]; then app_name="con Beta"; fi
    if [[ "{{ channel }}" == "dev" ]];  then app_name="con Dev";  fi
    bundle="dist/macos/{{ channel }}/${resolved_arch}/${app_name}.app"
    echo "Ad-hoc signing ${bundle}"
    codesign --force --deep --sign - "${bundle}"
    echo "Signed (ad-hoc): ${bundle}"

# [macOS] Install ad-hoc signed bundle to /Applications
macos-install-adhoc channel=channel arch=arch: (macos-bundle-adhoc channel arch)
    #!/usr/bin/env bash
    set -euo pipefail
    resolved_arch="{{ arch }}"
    if [[ -z "${resolved_arch}" ]]; then
        resolved_arch="$(uname -m | sed 's/aarch64/arm64/')"
    fi
    app_name="con"
    if [[ "{{ channel }}" == "beta" ]]; then app_name="con Beta"; fi
    if [[ "{{ channel }}" == "dev" ]];  then app_name="con Dev";  fi
    src="dist/macos/{{ channel }}/${resolved_arch}/${app_name}.app"
    dst="/Applications/${app_name}.app"
    echo "Installing ${src} → ${dst}"
    rm -rf "${dst}"
    cp -R "${src}" "${dst}"
    echo "Done. Launch ${app_name} from /Applications or Spotlight."

# [macOS] Full release: build + sign + notarize + DMG
# Requires: APPLE_SIGNING_IDENTITY + APPLE_NOTARY_* or APPLE_ID env vars
macos-release channel=channel arch=arch:
    #!/usr/bin/env bash
    set -euo pipefail
    resolved_arch="{{ arch }}"
    if [[ -z "${resolved_arch}" ]]; then
        resolved_arch="$(uname -m | sed 's/aarch64/arm64/')"
    fi
    CON_CHANNEL={{ channel }} CON_ARCH="${resolved_arch}" ./scripts/macos/release.sh

# [macOS] Download Sparkle.framework into .sparkle/ (enables auto-update in bundle)
macos-sparkle-download:
    ./scripts/sparkle/download.sh

# [macOS] Open the built app bundle in Finder
macos-open channel=channel arch=arch:
    #!/usr/bin/env bash
    resolved_arch="{{ arch }}"
    if [[ -z "${resolved_arch}" ]]; then
        resolved_arch="$(uname -m | sed 's/aarch64/arm64/')"
    fi
    app_name="con"
    if [[ "{{ channel }}" == "beta" ]]; then app_name="con Beta"; fi
    if [[ "{{ channel }}" == "dev" ]];  then app_name="con Dev";  fi
    open "dist/macos/{{ channel }}/${resolved_arch}/${app_name}.app"

# ── Linux ─────────────────────────────────────────────────────────────────────

# [Linux] Build a release binary and package it
# Output: dist/con-{version}-linux-{arch}.tar.gz
linux-release channel=channel arch=arch:
    #!/usr/bin/env bash
    set -euo pipefail
    resolved_arch="{{ arch }}"
    if [[ -z "${resolved_arch}" ]]; then
        resolved_arch="$(uname -m | sed 's/aarch64/arm64/')"
    fi
    CON_RELEASE_CHANNEL={{ channel }} CON_LINUX_ARCH="${resolved_arch}" ./scripts/linux/release.sh

# [Linux] Install the release binaries to ~/.local/bin
linux-install channel=channel arch=arch: (linux-release channel arch)
    #!/usr/bin/env bash
    set -euo pipefail
    resolved_arch="{{ arch }}"
    if [[ -z "${resolved_arch}" ]]; then
        resolved_arch="$(uname -m | sed 's/aarch64/arm64/')"
    fi
    # scripts/linux/release.sh stages to dist/con-{version}-linux-{arch}/
    # Use || true so set -e doesn't exit when the glob has no matches.
    stage_dir="$(ls -d dist/con-*-linux-${resolved_arch} 2>/dev/null | sort -V | tail -1 || true)"
    if [[ -z "${stage_dir}" || ! -f "${stage_dir}/con" ]]; then
        echo "Binary not found under dist/con-*-linux-${resolved_arch}/ — run 'just linux-release' first"
        exit 1
    fi
    mkdir -p "$HOME/.local/bin"
    cp "${stage_dir}/con" "$HOME/.local/bin/con"
    chmod 755 "$HOME/.local/bin/con"
    echo "Installed ${stage_dir}/con → $HOME/.local/bin/con"
    if [[ -f "${stage_dir}/con-cli" ]]; then
        cp "${stage_dir}/con-cli" "$HOME/.local/bin/con-cli"
        chmod 755 "$HOME/.local/bin/con-cli"
        echo "Installed ${stage_dir}/con-cli → $HOME/.local/bin/con-cli"
    fi

# [Linux] Build a Flatpak bundle using AetherPak Zero-Manifest mode
# Output: dist/co.nowledge.con.flatpak
flatpak-build channel=channel:
    #!/usr/bin/env bash
    set -euo pipefail
    resolved_arch="{{ arch }}"
    if [[ -z "${resolved_arch}" ]]; then
        resolved_arch="$(uname -m)"
    fi
    aetherpak_cmd="aetherpak"
    if ! command -v aetherpak &>/dev/null; then
        if [[ -x "$HOME/workspace/aetherpak/cli/bin/aetherpak" ]]; then
            aetherpak_cmd="$HOME/workspace/aetherpak/cli/bin/aetherpak"
        else
            echo "Error: aetherpak CLI not found on PATH or under ~/workspace/aetherpak/cli/bin/aetherpak" >&2
            exit 1
        fi
    fi
    "$aetherpak_cmd" build --config packaging/flatpak/aetherpak.yaml \
        --arch "${resolved_arch}" \
        --branch "{{ channel }}" \
        --bundle \
        --output-dir dist

# ── Windows (run from Developer Command Prompt for VS 2022) ───────────────────

# [Windows] Debug build (con-app.exe — CON is a reserved DOS device name)
windows-build:
    cargo wbuild -p con

# [Windows] Release build
windows-build-release:
    cargo wbuild -p con --release
    cargo build -p con-cli --release

# [Windows] Run
windows-run:
    cargo wrun -p con

# [Windows] Test
windows-test:
    cargo wtest -p con-core -p con-cli -p con-agent -p con-terminal

# [Windows] Build and install local release binaries to the user install root
windows-install: windows-build-release
    if not exist "%LOCALAPPDATA%\Programs\con" mkdir "%LOCALAPPDATA%\Programs\con"
    copy /Y "target\release\con-app.exe" "%LOCALAPPDATA%\Programs\con\con-app.exe"
    copy /Y "target\release\con-cli.exe" "%LOCALAPPDATA%\Programs\con\con-cli.exe"
    echo Installed con-app.exe and con-cli.exe to %LOCALAPPDATA%\Programs\con

# ── dist cleanup ──────────────────────────────────────────────────────────────

# Remove all dist/ output
clean-dist:
    {{ if os() == "windows" { "if exist dist rmdir /s /q dist" } else { "rm -rf dist/" } }}

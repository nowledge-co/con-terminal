# Hacking on con

Quick contributor map for `con`.

Read these first:
- `README.md` — public project overview
- `CLAUDE.md` — development conventions
- `DESIGN.md` — architecture and product direction
- `docs/README.md` — documentation index

## Workspace Map

Terminal crates do not depend on the UI. The agent crate does not depend on a specific terminal backend.

| Crate | Role |
|-------|------|
| `con` | GPUI shell: windows, tabs, panes, agent UI, settings, command surfaces |
| `con-core` | Config, sessions, shared app logic, harness wiring |
| `con-terminal` | Theme + palette helpers shared across backends |
| `con-ghostty` | Per-platform terminal backends: macOS embedded libghostty + Metal, Windows libghostty-vt + ConPTY + D3D11/DirectWrite, Linux Unix PTY + libghostty-vt + GPUI-owned `StyledText` paint |
| `con-agent` | Rig integration, tools, hooks, conversation, skills |
| `con-cli` | CLI + socket client for the live local control plane |

Each platform exposes the same `GhosttyApp` / `GhosttyTerminal` /
`TerminalColors` type names from `con-ghostty`, so the rest of the
workspace consumes the backend without per-call-site `cfg` gates.
See `docs/impl/{linux,windows}-port.md` for the per-platform plans
and the path to the long-term GPU-accelerated grid renderer.

## Agent Model

- Shared Tokio runtime per window
- Per-tab agent sessions
- Tool and model events flow to the UI over channels
- Rig `PromptHook` drives streaming, approvals, and lifecycle events

For the full breakdown, see `docs/impl/agent-harness.md`.

## Prerequisites

- Rust (stable, edition 2024)
- `cmake`
- **Zig**: use **Zig 0.16.0 exactly** for full terminal builds.
  The pinned Ghostty revision requires this version; older Zig releases
  cannot build it, and newer releases may change the build APIs again.

### Quick setup with mise (recommended)

If you use [mise](https://mise.jdx.dev/), the repo root `mise.toml`
pins the exact Zig version. Run:

```bash
mise install   # installs zig 0.16.0
```

Then use `just` for all build / run / test tasks (see below).

### Manual setup

If you do not use mise, install the prerequisites yourself:

- **Zig**: download the official 0.16.0 archive from
  `https://ziglang.org/download/0.16.0/` and put the directory on
  `PATH`, or set `CON_ZIG_BIN=/path/to/zig`.
- **macOS**: `cmake` plus Zig 0.16.0. The macOS release workflow installs Zig 0.16.0 explicitly before building embedded libghostty.
- **Windows**: Zig 0.16.0, Visual Studio 2022 Build Tools with the Windows 10/11 SDK. Run full builds from a _Developer Command Prompt for VS 2022_ so `rc.exe` is on `PATH`. If Windows Defender is on, either add an exclusion for the repo dir or disable real-time scanning — Zig's sub-build exes get briefly locked by MpEngine and spawn with `FileNotFound`.
- **Linux**: Zig 0.16.0, plus the GPUI runtime apt deps the CI job already installs:
  ```sh
  sudo apt-get install -y --no-install-recommends \
    libxcb-composite0-dev libxcb-dri2-0-dev libxcb-glx0-dev \
    libxcb-present-dev libxcb-xfixes0-dev libxkbcommon-x11-dev \
    libwayland-dev libvulkan-dev libfreetype-dev libfontconfig1-dev \
    mesa-vulkan-drivers
  ```
  The `mesa-vulkan-drivers` line gives you a software ICD (llvmpipe) as a fallback for headless / VM environments; on a real desktop with a hardware GPU you can skip it.

CI mirrors this deliberately:
- `release-macos.yml`, `release-linux.yml`, and `release-windows.yml` install Zig 0.16.0 before release builds.
- The Linux PR smoke check in `ci-portable.yml` also installs Zig 0.16.0 because it type-checks `con-ghostty` with `libghostty-vt`.
- The Windows and Linux PR jobs build and link the real libghostty-vt backend, then run `con-ghostty` tests. Do not replace these with `CON_SKIP_GHOSTTY_VT` or check-only coverage: removed symbols and calling-convention drift otherwise remain invisible until release.

## Build

```bash
# macOS / Linux
cargo build

# Windows — must use the `w*` aliases. The default `con` binary cannot
# exist on Windows because `CON` is a reserved DOS device name, so the
# Windows build ships as `con-app.exe` via a feature-gated alias bin
# target. `cargo wbuild` is `cargo build --no-default-features
# --features con/bin-con-app`; `wrun`, `wcheck`, `wtest` mirror it.
cargo wbuild -p con --release          # → target\release\con-app.exe
```

If you have `just` installed, the root `justfile` wraps the common local
flows:

```bash
just build          # debug build for the current platform
just run            # run from source
just test           # platform-appropriate test set
just check          # fast type check
just install        # build and install to the local platform install path
```

On Windows, those default recipes dispatch through the `cargo w*` aliases
above, so they produce and run `con-app.exe` instead of trying to build a
reserved `con.exe` name. Platform-specific release helpers are also available,
for example `just channel=beta macos-release`,
`just channel=beta linux-release`, `just arch=x86_64 macos-bundle`, and
`just windows-build-release`.

## Run

```bash
# macOS / Linux
cargo run -p con

# Windows
cargo wrun -p con
```

With optional arguments:

```bash
RUST_LOG=con_agent::flow=info,con_agent=warn,con_core=warn,con::suggestions=debug,con_core::suggestions=debug cargo run -p con
```

## Test

```bash
cargo test --workspace            # macOS / Linux
cargo wtest -p con-core -p con-cli -p con-agent -p con-terminal   # Windows (portable crates only)
```

## Release Build

```bash
cargo build --release -p con                  # macOS / Linux
cargo build --release -p con-cli              # control-plane CLI
cargo wbuild -p con --release                 # Windows → target\release\con-app.exe
cargo build --release -p con-cli              # Windows → target\release\con-cli.exe
```

For signed macOS release artifacts, use:

```bash
./scripts/macos/release.sh
```

The macOS app bundle contains both `Contents/MacOS/con` and
`Contents/MacOS/con-cli`; the release verifier fails if the CLI is
missing. The Homebrew cask and Unix installer expose that bundled
`con-cli` on PATH so orchestrators such as `pi-interactive-subagents`
do not need a separate source checkout.

Release CI also has a final promotion gate. Platform jobs verify the
artifact shape before upload, and `release-finalize.yml` keeps the
GitHub Release drafted unless all expected assets, appcasts, and
gh-pages installer scripts are present for the same tag. A broken
artifact should fail private, not become `/releases/latest`. Internal
`v*-dev.*` smoke tags are prereleases, never update public
stable/beta appcasts or Homebrew casks, do not embed a Sparkle feed URL,
and are only gated on artifact and installer-script shape.

For a Linux release tarball (un-signed; mirrors the Windows
preview's distribution shape), use:

```bash
CON_RELEASE_VERSION=0.1.0-beta.X CON_RELEASE_CHANNEL=beta \
  ./scripts/linux/release.sh
```

Output lands in `dist/con-<version>-linux-<arch>.tar.gz` with a
SHA256 sum next to it. The CI workflow at
`.github/workflows/release-linux.yml` runs the same script on every
`v*` tag, attaches the tarball to the shared GitHub release, and
updates the Sparkle-shaped appcast at
`https://con-releases.nowledge.co/appcast/<channel>-linux-x86_64.xml`
that the in-app notify-only updater polls.

## Useful Paths

- `crates/con-app/src` — app shell and GPUI surfaces (the crate directory is `con-app` because `con` is a reserved DOS device name on Windows; the Cargo package and binary are still named `con` on macOS, so `cargo run -p con` works as before — see `docs/impl/windows-port.md`)
- `crates/con-core/src` — shared app logic
- `crates/con-agent/src` — agent provider, hooks, tools, skills
- `docs/design` — design handoff set
- `docs/impl` — implementation notes
- `postmortem` — issue writeups and lessons learned

# Windows Port — Plan and Status

con ships on macOS and has a working Windows beta. This document captures
what we learned while preparing the codebase for the Windows port and lays
out the staged path that got it there. It is the single source of truth
for Windows-specific work.

This is a planning document, not an implementation log. The corresponding
postmortem (`postmortem/2026-04-16-prepare-windows-port.md`) records the
decisions made when this plan was first written. Linux now has its own
companion plan in `docs/impl/linux-port.md`.

## Building today

As of Phase 2 the workspace builds on Windows, Linux, and macOS. What
you get per platform:

| Target | UI binary | Terminal pane | Control socket | Agent / settings / CLI |
|:-------|:---------:|:-------------:|:--------------:|:----------------------:|
| macOS  | ✅ real   | ✅ libghostty + Metal | `/tmp/con.sock` | ✅ |
| Windows | ✅ real  | ✅ libghostty-vt + ConPTY + D3D11/DirectWrite | `\\.\pipe\con` | ✅ |
| Linux  | ✅ real preview | ✅ Unix PTY + libghostty-vt + GPUI styled-cell paint | `/tmp/con.sock` | ✅ |

On Windows (from a Developer Command Prompt for VS 2022):

```powershell
rustup default stable
git clone https://github.com/nowledge-co/con-terminal.git
cd con-terminal
cargo wbuild -p con --release          # → target\release\con-app.exe
cargo wtest  -p con-core -p con-cli -p con-agent -p con-terminal -p con-ghostty
```

Prerequisites:
- **Rust** (stable, edition 2024).
- **Visual Studio Build Tools 2022** with "Desktop development with
  C++" (for `link.exe` + Windows SDK).
- **Git for Windows**.
- **Zig 0.16.0 exactly** on PATH (for `libghostty-vt`); download from
  <https://ziglang.org/download/>. If the `zig` executable isn't on
  PATH, set `CON_ZIG_BIN` to its absolute path.

Build-time env vars for `con-ghostty`:

| Var | Effect |
|-----|--------|
| `CON_ZIG_BIN` | Absolute path to the `zig` executable. |
| `CON_GHOSTTY_SOURCE_DIR` | Reuse a local Ghostty checkout instead of fetching one into `OUT_DIR` (handy on Windows when `MAX_PATH` bites or Defender scans the `target` dir). |
| `CON_GHOSTTY_VT_STEP` | Exact Zig build step/flag to pass for libghostty-vt. Autodetected; override only if the probe picks wrong. |
| `CON_GHOSTTY_VT_TARGET` | Override the Zig target passed to the `libghostty-vt` build. Normally derived from Cargo's target triple and set explicitly so release artifacts do not inherit the build machine's native CPU features. |
| `CON_GHOSTTY_VT_SIMD=1` | Opt in to Ghostty's SIMD UTF-8 paths (`-Dsimd=true`). Default off on Windows because the resulting `ghostty-vt-static.lib` references `simdutf` C++ symbols that Zig doesn't bundle into the archive. Flip this once simdutf link-resolution is sorted (see TODO in `build.rs`). |
| `CON_STUB_GHOSTTY_VT=1` | Compile `src/windows/ghostty_vt_stub.c` and link it instead of libghostty-vt. The resulting `con-app.exe` launches the full GPUI app and ConPTY shell path, but the terminal grid is empty because the VT parser is stubbed. Useful for iterating the GPUI / renderer path while a real libghostty-vt build is broken. |
| `CON_GHOSTTY_VT_RENDER_STATE=0` | Skip `ghostty_render_state_new` at startup. Default on (render state is how we read cells back for display). The escape hatch exists because older Ghostty revisions have a broken render-state implementation on Windows; the `GHOSTTY_REV` pin as of 2026-04-17 tip-of-main works, but if you pull a regression from upstream you can ship a runnable app with `=0` while the fix is in flight. |
| `CON_SKIP_GHOSTTY_VT=1` | Skip both. `cargo build` will fail at link. Only useful for `cargo check`. |
| `ZIG_GLOBAL_CACHE_DIR` | Optional Zig cache override. On Windows, `con-ghostty` now defaults this to a short path (`C:\zc`, then `%TEMP%\zc` fallback) when unset so Ghostty/uucode helper spawns stay under `MAX_PATH`. |

Common Windows pitfalls when the Zig step fails mid-build with
`FileNotFound` on a just-compiled `uucode_build_tables.exe`:

1. **Windows Defender** is quarantining the newborn executable. Add an
   exclusion for `C:\path\to\con` + `%USERPROFILE%\.zig-cache` +
   `%LOCALAPPDATA%\zig`.
2. **MAX_PATH** — the default 260-char limit trips on Zig's deep cache
   layout. Enable long paths (`HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem\LongPathsEnabled = 1`)
   OR point `CON_GHOSTTY_SOURCE_DIR` at a short path like `C:\ghostty`.
   `con-ghostty` now also defaults `ZIG_GLOBAL_CACHE_DIR` to a short
   path on Windows; if your machine disallows `C:\zc`, set
   `ZIG_GLOBAL_CACHE_DIR` explicitly to another short writable path.
3. **Zig version mismatch** — the current pin is validated with Zig
   0.16.0. If a newer Zig breaks upstream, either install the validated
   Zig version or bump `GHOSTTY_REV` in `build.rs` (requires macOS
   re-validation of the full libghostty build).

The binary ships as `con-app.exe`, not `con.exe`: `CON` is a reserved
DOS device name and Windows refuses to create `con.exe` via most
Win32 file APIs. The package name is still `con` (so `cargo run -p
con` is unambiguous), and the `wbuild` / `wrun` / `wtest` / `wcheck`
aliases in `.cargo/config.toml` wrap the `--no-default-features
--features con/bin-con-app` incantation that selects the renamed bin.
macOS and Linux keep the plain `con` binary unchanged.

The produced `con-app.exe` launches into the full UI with a real,
GPU-rendered terminal pane — libghostty-vt parses the VT stream from a
ConPTY child, the D3D11/DirectWrite atlas renderer rasterizes glyphs
(including IoskeleyMono's Nerd-Font icon set), and the grid is drawn
in a single `DrawIndexedInstanced` per frame. Current beta builds also
reserve Unicode-derived two-cell atlas slots for wide CJK glyphs and
prefer installed monospace fallbacks before proportional UI fonts when
the requested/default terminal font is unavailable.

## What works today (macOS)

The macOS stack is fully native:

```
GPUI window (upstream zed-industries/zed)
  └── GhosttyView                          crates/con-app/src/ghostty_view.rs
        ├── child NSView (host)            cocoa::base::id
        ├── GhosttyApp     (per window)    crates/con-ghostty
        └── GhosttyTerminal (per surface)  ghostty_surface_t
              ├── PTY + child process      libghostty
              ├── VT parser + scrollback   libghostty
              ├── Metal renderer           libghostty (IOSurface layer)
              └── action callbacks         libghostty
```

The UI crate lives at `crates/con-app/` (not `crates/con/`) because `CON`
is a reserved DOS device name on Windows — `git clone` and `git checkout`
refuse to create any path component named `con`. The Cargo package name
and the binary name are both still `con` on macOS; only the filesystem
directory changed. See "Binary naming on Windows" below for how the
future Windows build will produce a valid executable filename.

`con-ghostty` is a thin Rust wrapper over libghostty's C embedding API.
libghostty is built from a pinned Ghostty revision via `zig build` and
linked statically as the final `libghostty-internal.a` archive. Surface creation hands a NSView
pointer to libghostty, which attaches a Metal `IOSurfaceLayer` and renders
the terminal directly into it. PTY, VT parsing, and rendering are all
inside libghostty.

## Per-platform status of the upstream layers

### libghostty (Ghostty embedded C API)

As of April 2026, libghostty's full embedded C API
(`ghostty_app_*`, `ghostty_surface_*`, `ghostty_config_*`) is **macOS-only**
upstream. The platform tag in `ghostty_platform_e` only has `MACOS` and
`IOS` variants — there is no `WINDOWS` variant in
`include/ghostty.h`.

Ghostty has split out a smaller library called **`libghostty-vt`**
(PR ghostty-org/ghostty#8840) which exposes only the VT parser and
terminal state machine and is buildable for macOS, Linux, Windows, and
WebAssembly. It does **not** include a renderer, font stack, or PTY —
those become the embedder's responsibility.

A Win32 apprt (`-Dapp-runtime=win32`) is in progress in community forks
(notably `InsipidPoint/ghostty-windows` and `adilahmeddev/windows-apprt`)
but is not merged upstream. None of these forks expose the embedded C API
that `con-ghostty` consumes — they ship a standalone `.exe`.

Cross-compiling libghostty for Windows from Linux is currently broken
(discussion ghostty-org/ghostty#11697); building **on** Windows works
after the fix in PR ghostty-org/ghostty#11698.

### GPUI (Zed)

The Zed Windows backend lives at `crates/gpui/src/platform/windows/`
in `zed-industries/zed`. It uses Direct3D 11.1 via DirectComposition,
DirectWrite for fonts, and is in public beta as of "Zed for Windows is
here" (zed.dev). Core features (rendering, dialogs, clipboard, fonts,
HiDPI, IME for most input methods, dark/light mode) work; rough edges are
multi-monitor, 120-144 Hz scheduling, certain IMEs, and some GPU
variants.

Two GPUI gaps matter for con's Windows port:

1. **HWND child-window embedding is missing.** PR
   zed-industries/zed#24330 attempted exactly this for Windows 11
   Taskbar embedding and was closed unmerged because maintainers wanted
   a cross-platform API first. The macOS pattern of "give libghostty an
   NSView and let it render into your view tree" has no upstream Windows
   equivalent.
2. **No tray icon, accessibility (UIA), or background-running app
   support.** Not blockers for con today but worth tracking.

The workspace currently uses upstream `zed-industries/zed` gpui directly
(see `Cargo.toml`), not the GPUI-CE community fork. CLAUDE.md previously
described a GPUI-CE pin; that wording is being corrected as part of this
prep work.

### gpui-component (longbridge)

Officially supports Windows x86_64 with CI. Depends on upstream Zed's
gpui by git, matching our workspace pin. Should require no per-target
changes.

### Rig, tokio, serde, etc.

All cross-platform. Rig only assumes std + reqwest, which we already
build with rustls.

## The path to a working Windows build

The port has to clear three independent technical hurdles. They can be
worked in parallel by different contributors.

### Hurdle 1 — Terminal backend on Windows

`con-ghostty` cannot be reused as-is. The realistic options are, in
order from lowest-risk to most-coupled:

**Option A. `libghostty-vt` + custom renderer + ConPTY (lowest risk).**

- Embed `libghostty-vt` for the VT parser and screen state machine.
- Implement a Direct3D 11 / DirectWrite renderer in a new
  `con-ghostty-win` crate (or as a `cfg(windows)` module inside
  `con-ghostty`).
- Drive a child shell via `CreatePseudoConsole` (ConPTY) and feed bytes
  into the libghostty-vt parser; pull back grid/scrollback diffs and
  render them.
- Reuse `con-ghostty`'s public Rust types (`TerminalColors`,
  `GhosttySurfaceEvent`, `GhosttyTerminal::write`, etc.) so
  `terminal_pane.rs` doesn't have to know which backend it is talking to.

This is what Ghostty maintainers point external embedders at today.
Largest engineering investment but cleanest layering.

**Option B. Carry a Win32 embedded apprt patch on libghostty.**

- Fork `InsipidPoint/ghostty-windows` and add an `apprt-embedded`
  variant that exposes `ghostty_surface_*` on Windows by accepting an
  HWND in a new `ghostty_platform_windows_s` variant.
- Update `con-ghostty/src/ffi.rs` to add the `WINDOWS` enum variant and
  the HWND union member.
- Update `con-ghostty/build.rs` to invoke `zig build` with the win32
  toolchain on Windows.

Smaller engineering investment per feature, but you own a Ghostty fork
forever unless the changes upstream.

**Option C. Wait for upstream.** Track `zig build` on Windows and the
in-progress Win32 apprt. No action item except subscribing to the
relevant issues.

We recommend **Option A** as the primary direction. Option B is a
viable fallback if libghostty-vt's surface area turns out to be
insufficient (e.g. if shell integration features that today live in the
full libghostty are needed and would be expensive to re-implement).

### Hurdle 2 — Embedding the terminal surface in the GPUI window

Whichever backend wins Hurdle 1, the rendered output has to land
inside a GPUI window. Three sub-options:

1. **Patch GPUI to support `WindowKind::Child` on Windows** (revive
   PR zed-industries/zed#24330). This is the structural equivalent of
   what we do on macOS. It needs upstream coordination, ideally with a
   cross-platform API that also covers macOS and Linux.
2. **Render the terminal grid via GPUI's own painter.** If the renderer
   from Hurdle 1A produces a pixel buffer or a textured quad, GPUI can
   paint it as part of its own scene. This avoids HWND embedding
   altogether but means we don't get libghostty's GPU pipeline — we'd
   re-render through GPUI's D3D11.
3. **Sibling HWND** with manual Z-order / position coordination. Fast
   to prototype but well-known to glitch on resize, alt-tab, and
   fullscreen.

Recommendation: prototype option 2 first because it keeps the rendering
contract entirely inside GPUI. If perf is unacceptable (likely fine for
text), pursue option 1.

### Binary naming on Windows

`CON` is a reserved DOS device name on Windows. That reservation covers
both filesystem paths (`git checkout` refuses to create `crates/con/`
even read-only, which is why the UI crate now lives at
`crates/con-app/`) and the produced executable — a `con.exe` file
cannot be created by most file-creation APIs. The `CON` reservation
applies regardless of extension.

This is implemented with feature-gated twin `[[bin]]` entries: macOS and
Linux keep the plain `con` binary, while Windows builds `con-app.exe`.
Cargo does not support per-target `[[bin]] name` in the manifest, so the
practical mechanisms were:

1. **Feature-gated twin `[[bin]]` entries pointing at the same
   `main.rs`** —

   ```toml
   [features]
   default = ["bin-con"]
   bin-con = []      # enabled on macOS/Linux via a .cargo/config.toml
   bin-con-app = []  # enabled on Windows

   [[bin]]
   name = "con"
   path = "src/main.rs"
   required-features = ["bin-con"]

   [[bin]]
   name = "con-app"
   path = "src/main.rs"
   required-features = ["bin-con-app"]
   ```

   Combined with a `.cargo/config.toml` that sets the right feature
   per `[target.cfg(...)]`, `cargo run -p con` works everywhere and
   produces `con` on Unix / `con-app.exe` on Windows.
2. **Rename the single `[[bin]]` to `con-app` and install a shell
   wrapper named `con`** in the macOS cask / Homebrew formula so the
   command-line experience is unchanged. Simpler manifest, more work
   in the packaging scripts.

Mechanism 1 is the active policy. It keeps the binary-name behavior
platform-local and does not require installers to hide the difference.

### Hurdle 3 — Host-side Windows-isms

These are small, but they compound into "the binary won't even compile"
if untouched. The Windows-prep PR series should land these incrementally:

- **Sockets.** Replace `tokio::net::UnixListener`/`UnixStream` with a
  small transport abstraction backed by Unix sockets on `cfg(unix)` and
  Windows Named Pipes (`\\.\pipe\con-<user>`) on `cfg(windows)`.
- **Path conventions.** Replace hard-coded `/tmp` fallbacks with
  `std::env::temp_dir()`. Use `con-paths` for config, data, auth,
  theme, and skills storage so Windows gets `con-terminal` instead of
  the reserved `con` path segment.
- **Permissions.** Gate `set_permissions(0o600)` and any
  `std::os::unix::*` imports behind `cfg(unix)`.
- **Bundle metadata.** `bundle_info_value`, `set_dock_icon`,
  `macos_major_version`, `global_hotkey`, `updater` (Sparkle) are
  macOS-only. Each call site needs a `cfg(target_os = "macos")` gate so
  the non-macOS paths use sensible defaults (e.g. version from
  `CARGO_PKG_VERSION`).
- **Global hotkey.** Carbon RegisterEventHotKey on macOS;
  `RegisterHotKey` on Windows. Out of scope for the first port but
  worth abstracting at the call site.
- **Auto-update.** Sparkle on macOS; the standard Windows pattern is
  WiX MSI + a separate updater binary, or `cargo dist`'s self-update
  flow. Out of scope for the first port.
- **Process spawn / shell discovery.** Currently delegated entirely to
  libghostty. The Windows backend (Hurdle 1) will need to discover
  `cmd.exe` / `powershell.exe` / `pwsh.exe` / `wsl.exe` and pipe
  through ConPTY itself.

## Phased work breakdown

The port should land in small, mergeable PRs in roughly this order. Each
phase keeps the macOS build green.

| Phase | Scope | What changes | Build state / status |
|------:|-------|--------------|----------------------|
| 0 | prep | Cfg-gates, transport abstraction, docs, cleaner non-macOS error message | macOS unchanged; non-UI crates compile on Windows. ✅ landed |
| 1 | Windows + Linux CI smoke test | `.github/workflows/ci-portable.yml` runs `cargo check` + `cargo test` for the portable crates and the UI binary on `windows-latest` and `ubuntu-latest` | CI green on both targets. ✅ landed |
| 2 | Stub terminal backend | `con-ghostty` exposes stub types on non-macOS; `stub_view.rs` is a GPUI placeholder view selected via `#[path]`; the `compile_error!` is gone | `cargo build -p con` worked on Windows and Linux with a placeholder pane during the bootstrap phase. ✅ landed |
| 3a | Windows backend scaffold | `con-ghostty/src/windows/` modules: ConPTY wrapper, libghostty-vt FFI, D3D11 + DirectWrite renderer skeleton, offscreen render session, and `WindowsGhosttyApp` / `WindowsGhosttyTerminal` facade. `con-app/src/windows_view.rs` instantiates a `RenderSession` lazily once GPUI pane bounds are known. `build.rs` builds `libghostty-vt` via `zig build` on Windows host targets (probes available step names; `CON_GHOSTTY_VT_STEP` override). Binary renamed to `con-app.exe` on Windows via feature-gated twin `[[bin]]` (CON is reserved DOS device name). | `cargo wbuild -p con --release` on Windows produces a `con-app.exe` that launches; terminal pane spawns a real ConPTY shell, drives libghostty-vt, and paints through the offscreen D3D11 -> GPUI image bridge. Compiles clean on Linux + Windows CI. ✅ landed |
| 3b | Glyph atlas + grid render | HLSL shaders embedded and runtime-compiled via `D3DCompile`. Skyline-packed (`etagere`) BGRA8 glyph atlas (`con-ghostty/src/windows/render/atlas.rs`), glyphs rasterized via Direct2D `DrawText` with grayscale AA. Three-case PUA rasterization (fits-cell / width-overflow with lsb shift / height-overflow with scale-around-centre) for Nerd-Font icons, plus per-slot `PushAxisAlignedClip` + black pre-fill to prevent antialias fringe bleeding between atlas neighbours. The shader collapses sampled atlas RGB to scalar glyph coverage, so driver or remote-display subpixel fallback cannot leak colored ClearType fringe into screenshots or transparent windows. CJK/wide fallback glyphs reserve two-cell atlas slots using `unicode-width`, so DirectWrite fallback faces match the width libghostty-vt reserved in the terminal grid instead of looking clipped followed by a blank spacer cell. D3D11 pipeline (`pipeline.rs`): per-instance IA layout, dynamic instance buffer with `Map(WRITE_DISCARD)`, single `DrawIndexedInstanced(6, cell_count)` per frame — matches Windows Terminal AtlasEngine's architecture. Wide PUA instances are stable-sorted after narrow ones so DX11's in-order per-pixel writes let them win overlap with neighbour backgrounds; the cursor inverse-colour instance is captured pre-sort and pushed post-sort so it always draws last. `vt.rs` uses the `ghostty_render_state` API (row iterator + `row_cells_get_multi` + DIRTY row skip) — not the `grid_ref` path that the upstream header explicitly warns against for render loops. | Real glyph rendering on Windows, runtime-validated on real hardware against oh-my-posh prompt stacks, CJK fallback text, and dense hyphen rules. Postmortems: `postmortem/2026-04-21-windows-atlas-pua-rasterization.md`, `postmortem/2026-04-30-windows-font-rgb-fringing.md`. ✅ landed |
| 3c | Input + selection | Beta-baseline input, selection, scrollback, clipboard, and ConPTY launch behavior are landed; details below. | Beta-baseline terminal interactivity is landed. Remaining polish belongs in Phase 4 hardening. ✅ largely landed |
| 3d | (Parallel, upstream) | `WindowsWindow::attach_external_swap_chain_handle(bounds, HANDLE)` PR to `zed-industries/zed`. When merged, swap the current offscreen readback/image bridge to a composition swap chain that GPUI can place directly in its DComp tree. | No con-side merge blocker; still upstream-facing work. This is the remaining structural performance cleanup. — |
| 3e | renderer perf tuning | The current pipeline draws into an offscreen D3D11 texture, reads back BGRA bytes (`RenderSession::render_frame`), wraps them as `ImageSource::Render(Arc<RenderImage>)`, and lets GPUI re-upload them into its DComp tree. The GPU→CPU readback per dirty frame plus the CPU→GPU re-upload adds structural overhead versus Windows Terminal's direct-DComp present path. Landed mitigations: (1) ✅ wake GPUI from the ConPTY reader thread so freshly arrived shell output paints on the next prepaint instead of waiting for user input; (2) ✅ move steady-state terminal image generation into the main render path so freshly rendered images can appear in the current frame rather than always one frame later; (3) ✅ skip default-background blank-cell instances that are already covered by the render-target clear; (4) ✅ avoid a full-frame zero-fill before D3D readback; (5) ✅ only stable-sort the instance stream when a frame actually contains overflowing wide glyphs; (6) ✅ keep low-latency presents armed until the triggering input actually advances the VT generation, so shell echo/prompt redraws do not miss the fresh-frame path; (7) ✅ treat the staging ring as a mailbox so maximize/fullscreen backlog never blocks GPUI's thread trying to rescue stale readbacks; (8) ✅ snapshot Ghostty's actual render-state rows/cols during asynchronous resize catch-up and include snapshot geometry in the renderer invalidation key so resize catch-up frames are not skipped as "unchanged"; (9) ✅ keep the fresh-frame preference armed across a short interactive typing/paste burst so repeated echoed generations do not periodically fall back to stale-frame presents mid-burst; (10) ✅ stop forcing speculative GPUI repaints on handled key input before ConPTY echo has advanced the VT, so steady typing rides the real output wake path instead of paying for an extra unchanged prepaint; (11) ✅ preserve successive output wakes from the ConPTY reader instead of draining them into one repaint request, so bursty commands like `ls` can advance across multiple prepaints rather than visibly batching intermediate output; (12) ✅ when a fresher VT snapshot has already been submitted, stop presenting an older completed readback ahead of it, so PTY-driven redraws use true mailbox semantics instead of sliding through stale intermediate frames; (13) ✅ compute exact changed VT rows even when libghostty-vt reports full dirty, and use D3D `CopySubresourceRegion` to read back only those pixel rows for small updates while keeping full-readback fallbacks for resize/selection/theme changes; (14) ✅ replace dirty rows inside a CPU-side BGRA backing frame before publishing one GPUI image, so partial D3D readbacks keep replacement semantics even with translucent terminal backgrounds; (15) ✅ keep command-start output latency-critical for long enough to cover delayed shell output; (16) ✅ control shell/profile as a benchmark variable by honoring Windows Terminal's `defaultProfile` where possible. Runtime instrumentation behind `CON_GHOSTTY_PROFILE` now logs the full chain: `conpty_read` for chunk cadence, `vt_snapshot` for shared render-state + clone cost, `win_renderer` for drain/draw/submit/readback timing, partial-readback row counts, and ring state, `win_render_frame` for session-level timing, and `win_sync_render` for GPUI image-wrap/upload timing. Use `CON_LOG_FILE=con-profile.log` to capture these logs without shell redirection; idle unchanged frames are filtered by default to keep the app usable during profiling, and `CON_GHOSTTY_PROFILE_VERBOSE=1` enables every-frame traces. Postmortem: `postmortem/2026-04-26-windows-command-render-latency.md`. Long-term plan: once Phase 3d lands, present the swap chain directly via `attach_external_swap_chain_handle` and drop the readback entirely; profile with PIX / GPUView / ETW to verify Enter→glyph latency parity with WT. | Wins in `crates/con-app/src/windows_view.rs` (output wake handling + same-frame image swap + CPU-backed row replacement + image-wrap timing), `crates/con-ghostty/src/windows/conpty.rs` (chunk cadence timing + spawn-handle marker + Windows Terminal default-profile shell resolution), `crates/con-ghostty/src/windows/host_view.rs` (VT-generation-aware low-latency presents + frame instrumentation), `crates/con-ghostty/src/vt.rs` (render-state geometry snapshot + exact dirty-row derivation + shared snapshot instrumentation), and `crates/con-ghostty/src/windows/render/mod.rs` (instance/readback hot path + partial-row staging copies + per-stage timing + true mailbox presentation policy). ✅ substantially improved; direct swap-chain composition remains the long-term cleanup |
| 4 | Hardening | Multi-pane, splits, IME edge-case validation, focus, resize, copy/paste, drag/drop, OSC 133 shell integration, ligatures | Beta-quality polish and correctness backlog. — |
| 5 | Distribution | Installer/signing strategy (MSI/MSIX/winget/`cargo dist`), code signing, update verification / hardening, `con-app.exe` rename via feature-gated twin `[[bin]]` | ZIP + PowerShell installer + in-place beta update flow exist; release-trust hardening remains. — |

Phase 3c details now covered by the beta baseline:

- `host_view` translates `VK_*` input into terminal-local xterm sequences, including Tab / Shift+Tab capture, while preserving mouse selection behavior.
- ASCII C0 control chords include the punctuation controls terminals expect,
  not only letters. `Ctrl+]` now reaches tmux as GS (`0x1d`), `Ctrl+/`
  reaches US (`0x1f`), and shifted punctuation such as `Ctrl+@` / `Ctrl+Space`
  reaches NUL (`0x00`). The shared surface-control parser also accepts the
  legacy `ctrl-2..8` aliases for automation. Interactive `Ctrl+1..9` stays
  reserved for app tab switching on Windows.
- Wheel and touchpad input scroll libghostty-vt viewport state when mouse tracking is inactive. Alternate-screen mode-1007 wheel gestures translate into cursor keys with the same fractional row accumulator used by primary scrollback.
- The Windows pane has a visible GPUI scrollback scrollbar backed by cached libghostty-vt scrollbar state, so render no longer polls the expensive scrollbar query on every paint.
- The Windows terminal view insets the measured renderer surface from the pane
  edge. Mouse hit testing, IME candidate bounds, link hover, scrollbar, and
  render dimensions all use the inset content bounds, so the visual padding is
  not a cosmetic overlay that desynchronizes terminal coordinates. The gutter is
  painted with the same terminal clear color and opacity as the renderer image
  rather than falling through to transparent window content.
- Alternate-screen TUIs keep a higher default-background opacity floor than the
  ordinary shell. That preserves Con's glass effect for prompts while keeping
  htop/vim-style application canvases readable on Windows.
- CJK fallback glyphs are baseline-aligned inside the glyph atlas instead of
  being top-aligned as one-character `DrawText` lines. This keeps fallback
  CJK runs anchored to the same terminal-cell baseline as the primary face.
- Normal-weight East Asian text uses a medium-weight DirectWrite text format
  with a CJK-only grayscale contrast profile. That keeps Chinese strokes closer
  to browser/native text weight without reintroducing ClearType RGB fringe or
  changing Latin / Nerd-Font prompt glyphs.
- OSC 52 and ordinary clipboard operations are wired through the Win32 clipboard.
- CJK IME commit/preedit input is wired through GPUI's platform
  `InputHandler`. The terminal still owns control, alt/meta, and
  special-key encoding, while printable text and IME commits enter the
  PTY as text. Composition ranges report cursor-relative bounds so
  candidate windows anchor to the terminal cursor.
- ConPTY child launch passes the pseudo-console with
  `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE`, avoids inheriting con's unrelated
  stdout/stderr handles, starts from a valid absolute cwd when provided, then
  falls back to the user's home directory and finally `%TEMP%`.
  PowerShell/pwsh shells are launched with a lightweight prompt hook that emits
  OSC 7 cwd updates, and Con's Rust VT wrapper captures OSC 7 directly because
  libghostty-vt's terminal-only handler does not mutate pwd for that report.
  Restored sessions and new panes can therefore follow `cd` changes instead of
  only remembering the process launch directory.
- Profiling uses `CON_LOG_FILE` instead of shell redirection so the child shell never sees redirected log handles.

Phases 0-3e are **substantially complete** for the current Windows beta
baseline. Phase 3b shipped the full glyph atlas, the three-case PUA
rasterization that lets IoskeleyMono's Nerd-Font icons and ASCII
punctuation coexist, and the cursor z-order fix. Phase 3e closed the
largest avoidable latency gaps in the current offscreen bridge. Phase
3d/direct composition remains the structural cleanup that removes the
GPU -> CPU -> GPUI round trip rather than optimizing around it.

### Renderer perf — current cost and tuning track

The macOS path uses libghostty's Metal pipeline, which presents directly
to a `CAMetalLayer` and is profiled (`docs/impl/macos-terminal-profiling.md`).
The Windows backend is a custom D3D11/DirectWrite stack that — to avoid
child-window z-order and modal-composition problems — currently composites through
GPUI's image path:

```
ConPTY reader thread ─→ vt.feed(bytes) ─→ updates grid
                                          (now: signals wake_tx)
GPUI prepaint tick   ─→ render_frame()
                       └→ D3D11 draw to OFFSCREEN texture     (GPU)
                       └→ GPU→CPU readback of BGRA bytes      ← intrinsic cost
                       └→ patch CPU BGRA backing, wrap as RenderImage(Arc)
                       └→ GPUI uploads back to GPU            (CPU→GPU)
                       └→ DComp composite                     (GPU)
```

Compared to Windows Terminal's AtlasEngine, two costs are structural:

1. **GPU→CPU→GPU round-trip per dirty frame.** Synchronous readback alone
   is typically 5–15ms; re-upload adds a few more. WT presents directly to
   a DComp swap chain — no readback.
2. **PTY arrival → repaint latency.** The reader thread used to update the
   grid in place but never poke the view. The next prompt only painted on
   the next user input event or animation tick (up to ~16ms slack).
   Phase 3e step (1) closes this — `RenderSession::new` now takes a
   `wake: Fn() + Send + Sync` callback that the view uses to push a
   coalesced `cx.notify()` per PTY chunk (`crates/con-app/src/windows_view.rs`).

The remaining structural cost (readback plus GPUI image upload) goes away
once Phase 3d (`attach_external_swap_chain_handle`) lands and we can
present the swap chain into GPUI's DComp tree directly.

The current Phase 3e mitigations reduce work inside that temporary image
path without changing rendering semantics: default-background blank cells
are represented by the clear color, explicit SGR backgrounds and cursor /
selection / underline state still get instances, readback copies only
changed pixel rows when the VT snapshot is row-local, GPUI receives
one image produced from a CPU BGRA backing frame patched with those row
readbacks, and the wide-glyph sort is skipped on ordinary text frames.
After the 2026-04-26 profiling pass,
command output should also be compared with the same shell/profile that
Windows Terminal uses; con now reads Windows Terminal's `defaultProfile`
when possible so PowerShell profile hooks or WSL startup do not masquerade
as renderer latency.

### What you can do *today* on Windows

```powershell
git clone https://github.com/nowledge-co/con-terminal.git
cd con-terminal
cargo wbuild -p con --release
target\release\con-app.exe
```

The window comes up with the full chrome and a live terminal pane.
ConPTY spawns the Windows Terminal default profile shell when its
settings are readable, libghostty-vt parses the VT
stream, and the D3D11/DirectWrite atlas renderer draws the grid at
native refresh rate — IoskeleyMono ASCII, box-drawing, Powerline
separators, CJK/wide fallback glyphs, CJK IME text input, and the Nerd-Font icon set
(folder, git, status, …) all render correctly, cursor lands on the
right column, Tab / Shift+Tab reach shells and TUIs, wheel gestures
respect the platform scroll intent, and normal-shell scrollback is
reachable through both the mouse wheel and the visible scrollbar.
Resize works. Window close kills the shell. The remaining Windows work
is now direct-composition presentation, IME edge-case validation,
maximize/resize feel (#125), CJK font-weight polish, advanced selection
polish, and distribution hardening rather than basic input/selection
bring-up.

Caveats:
- You must have **Zig 0.16.0 exactly** on `PATH` for `cargo build` or
  `cargo check` to compile `libghostty-vt`; install it from
  <https://ziglang.org/download/0.16.0/> or point `CON_ZIG_BIN` at an
  explicit 0.16.0 binary. `CON_SKIP_GHOSTTY_VT=1` remains a local
  check-only escape hatch, but CI intentionally builds and links the real
  library so removed symbols and calling-convention drift cannot hide.
- The shell is `CON_SHELL` if set, else the configured Windows Terminal
  default profile command when `%LOCALAPPDATA%` settings are readable,
  else `pwsh.exe`/`powershell.exe` if on `PATH`, else `$env:COMSPEC`,
  else `cmd.exe`. PowerShell/pwsh profiles without an explicit
  `-Command`/`-File` get Con's OSC 7 cwd hook automatically; custom
  command/file profiles are left untouched. A first-class config field is
  still Phase 4 work.
- The terminal still pays a GPU→CPU readback and GPUI image upload on
  dirty frames. Recent fixes made small updates row-local and removed
  avoidable extra-frame latency plus several backlog stalls, but Windows
  Terminal can still feel faster until the direct-composition
  presentation path lands.

## Remaining decisions

The Windows beta no longer has open questions about whether the local
ConPTY + libghostty-vt + D3D11/DirectWrite architecture works. The
remaining decisions are narrower:

1. **Direct composition boundary.** Land or otherwise obtain a GPUI API
   that lets con present a Windows composition swap chain directly in the
   GPUI tree, then remove the D3D readback + `RenderImage` re-upload
   bridge.
2. **Shell integration scope.** Decide whether OSC 133 and non-PowerShell
   OSC 7 support belong in the shared VT layer or in per-platform shell
   adapters. PowerShell/pwsh cwd tracking is currently handled as a
   Windows launch-time adapter because those shells do not emit OSC 7 by
   default.
3. **Windows hardening.** Broaden IME/dead-key/international-keyboard
   validation, drag-to-scroll selection polish, column selection,
   CJK font-weight tuning, extreme resize behavior, ligatures, and
   remaining multi-monitor/GPU edge cases.
4. **Distribution.** Choose the installer/signing path (MSIX/MSI/cargo
   dist/winget), code-sign the binary, and harden the existing beta
   in-place update flow with artifact signature verification.

## References

- Ghostty: <https://github.com/ghostty-org/ghostty>
- Ghostty Windows roadmap discussion #2563:
  <https://github.com/ghostty-org/ghostty/discussions/2563>
- libghostty-vt PR #8840:
  <https://github.com/ghostty-org/ghostty/pull/8840>
- Ghostty libxml2 Windows build issue #11697:
  <https://github.com/ghostty-org/ghostty/discussions/11697>
- Community Win32 apprt fork:
  <https://github.com/InsipidPoint/ghostty-windows>
- Zed for Windows launch: <https://zed.dev/blog/zed-for-windows-is-here>
- GPUI WindowKind::Child PR (closed):
  <https://github.com/zed-industries/zed/pull/24330>
- Microsoft ConPTY:
  <https://learn.microsoft.com/en-us/windows/console/creating-a-pseudoconsole-session>

# Implementation: Ghostty Rendering Pipeline

## Overview

con now runs a single terminal runtime: embedded Ghostty on macOS.

con does not maintain its own VT parser, PTY loop, scrollback grid, or canvas renderer anymore.
Ghostty owns terminal execution end to end. con owns:

- window, tab, and split layout
- AI harness integration
- pane metadata, context extraction, and product UI
- theme translation into Ghostty config

Important clarification:

- one `GhosttyApp` is shared across the whole con window
- each split is still its own Ghostty surface

That is not a temporary compromise. It matches Ghostty's own host-side architecture on macOS: native splits are modeled as a surface tree, not as one giant surface that renders every pane internally.

That split is intentional. Terminal correctness stays in Ghostty. Product intelligence stays in con.

## macOS display lifecycle

Each embedded macOS surface owns a Ghostty renderer and therefore participates
in CoreVideo frame pacing independently from the GPUI window around it. Con
keeps those native renderers aligned with AppKit's lifecycle:

- `NSWindowDidChangeOcclusionStateNotification` is mirrored to
  `ghostty_surface_set_occlusion`, so hidden or covered windows do not keep a
  surface display link active.
- `NSWorkspaceWillSleepNotification` occludes every live surface before the
  display server starts tearing down the current topology.
- After `NSWorkspaceDidWakeNotification`, screen and backing notifications are
  coalesced until WindowServer's display changes become quiet. Backing scale,
  display id, and pixel size are then synchronized before only actually visible
  surfaces are unoccluded.

This ordering is deliberate. Display identity, backing scale, renderer
visibility, and framebuffer size are one lifecycle transaction on macOS; they
must not be updated independently while the system is rebuilding its display
configuration.

Pane zoom follows the same boundary. Zooming a pane is a Con layout state,
not a terminal mutation: every split surface stays alive, but the active
tab renders and shows only the focused pane until the zoom shortcut is
pressed again. This keeps it equivalent to tmux-style zoom while avoiding
fake resize, detach, or process lifecycle events.

## Runtime stack

```text
GPUI window
  └── GhosttyView
        ├── native NSView child
        ├── GhosttyApp          one per con window
        └── GhosttyTerminal     one per split surface
              ├── PTY + child process
              ├── VT parser + screen + scrollback
              ├── Metal renderer
              └── action callbacks
```

## Surface creation

`GhosttyView` creates a native `NSView` and passes it to `GhosttyApp::new_surface`.

We currently configure each surface with:

- `scale_factor`
- `font_size`
- `working_directory`
- `context = TAB`

Those values are part of `ghostty_surface_config_s` in libghostty's public C API.

## Split model

The correct long-term model is a host-managed Ghostty surface tree.

That means:

- Con should keep one shared `GhosttyApp` per window
- split creation should flow through Ghostty split actions
- the host still creates and owns each new surface view for each split

This is why "make the whole window one Ghostty instance" is the wrong mental model.
At the app level, con already does that. The remaining migration work is about replacing con-specific split semantics with Ghostty-driven surface-tree semantics.

## Data flow

1. User input enters GPUI.
2. `GhosttyView` forwards keyboard and mouse events into `ghostty_surface_*`.
3. Ghostty uses its embedded runtime wakeup callback to schedule `ghostty_app_tick()` on the macOS main queue. Con does not drive the core renderer from a fixed workspace polling loop anymore.
4. Ghostty writes input to the pane PTY, parses output, updates screen state, and renders into its Metal-backed view.
5. The host `NSView` and child surface `NSView` both use native autoresizing plus `NSViewLayerContentsRedrawDuringViewResize` so AppKit keeps the embedded layer responsive during live window resize.
6. Ghostty emits action callbacks such as title updates, PWD changes, and command-finished events.
7. `con-ghostty` stores those facts in `TerminalState` and bumps a wake generation after each wakeup-driven app tick.
8. Con's workspace-level housekeeping loop notices that generation change, drains surface-local state once for the window, and handles one-shot init retries.
9. con reads `TerminalState` and visible text to build pane metadata for the agent, sidebar, and tab UI.

This distinction matters. Ghostty's embedded API is designed around wakeup-driven ticking, not "tick it every N milliseconds and hope that is close enough."

## Clipboard, Drop, And Media Ingress

Terminal-host clipboard support is an input contract, not an image-rendering
protocol. Con handles these ingress paths separately:

- Text clipboard paste is forwarded through the terminal paste path so shells and
  editors still get bracketed-paste behavior where the backend supports it.
- File drops and file-path clipboard entries are converted into quoted paths
  with surrounding spaces, matching Zed's terminal behavior and avoiding GPUI's
  lossy path-to-text fallback for multi-file clipboard contents.
- File-manager clipboard text that arrives as a Linux-style `text/uri-list`
  is parsed only when every non-comment line is a local `file://` URI; mixed
  prose stays ordinary text.
- Image clipboard entries are not converted to files by Con. Instead, Con
  forwards a raw Ctrl+V keypress to the TUI. Agent TUIs such as Codex can then
  read the OS clipboard directly and attach the image using their native flow.

If a clipboard item contains both image bytes and a text representation, image
bytes win. If it contains file paths, paths win. That gives predictable behavior
for the two common agent workflows: copy an actual image to attach it, or
copy/drag image and code files to paste their paths.

This is intentionally separate from output-side inline graphics protocols. On
macOS, full embedded Ghostty owns any inline graphics support it exposes. On the
Windows and Linux preview backends, Con's portable renderers still need explicit
work before they can display Kitty graphics, Sixel, or iTerm2/OSC 1337 image
output.

## What con reads from Ghostty

Today con relies on these Ghostty-facing facts:

- title
- current working directory
- recent command completion signal, exit code, and duration
- process alive / exited state
- visible screen text
- recent scrollback text
- selection text
- grid size

These are enough to support:

- honest pane summaries
- visible command execution for the built-in agent
- better tmux and TUI awareness
- theme and font propagation into new panes

## What con does not own anymore

The following responsibilities were removed from con:

- PTY creation and resize plumbing
- VTE parsing
- alternate-screen bookkeeping in a custom grid
- custom scrollback storage
- shell input suggestion overlays inside the terminal renderer
- GPUI text painting of terminal cells

This is a product win. Those paths were expensive to maintain and always weaker than the native Ghostty implementation.

## TerminalPane

`TerminalPane` is now a thin Ghostty-backed wrapper, not a multi-backend enum.

It exposes the capabilities that the rest of the app needs:

- lifecycle: focus, visibility, liveness
- visible text reads
- search
- command completion polling
- remote-host and pane-mode hints
- theme application

The point of `TerminalPane` is no longer backend abstraction.
It is product abstraction: one stable pane API for the workspace and agent layers.

One important integration detail: `GhosttyView` should not own its own perpetual 16ms GPUI polling loop. That duplicates host work once per pane and becomes visible during live resize. Con now keeps exactly one lightweight workspace pump for Ghostty-related housekeeping while leaving Ghostty's renderer itself wakeup-driven.

Another important macOS alignment point is window step-resize. Standalone Ghostty updates `contentResizeIncrements` from the focused surface's cell size so AppKit drags the window in terminal-cell steps instead of continuous sub-cell pixel states. Con now mirrors that behavior from the active terminal surface, which cuts a large amount of otherwise pointless intermediate resize work before it reaches either GPUI layout or `ghostty_surface_set_size`.

One more host-side rule matters for TUI performance: normal UI renders must not force fresh terminal observations. Con previously used runtime observation in the workspace render path just to derive pane labels and remote-host hints for the input bar. That path could pull visible/recent terminal text while a dense TUI was active. The UI now reads only cached runtime state during render; explicit terminal observation stays on agent/control paths where that cost is intentional.

One more embedded-macOS detail turned out to matter: standalone Ghostty does not host the surface in a bare `NSView`. It wraps the surface in a scroll container and consumes `GHOSTTY_ACTION_SCROLLBAR` updates so the visible viewport stays synchronized with Ghostty's scrollback model during reflow and resize. Con now mirrors that contract by caching Ghostty scrollbar actions in `con-ghostty` and positioning the embedded surface inside a native scroll container. Without that, heavy TUIs could briefly render from upper scrollback during resize and only later settle back to the bottom.

The host also must not invent viewport state that Ghostty has not emitted. Standalone Ghostty only scrolls its native container when a real scrollbar update exists. Con now follows that rule too: before Ghostty publishes scrollbar data, the scroll container stays at its current position instead of being forced to a synthetic `offset = 0` state. That matters for TUI startup and reflow, where a fake top-of-history viewport can make the embed briefly paint from upper scrollback before Ghostty settles on the real bottom-anchored viewport.

macOS scroll input must also preserve Ghostty's scroll-event contract. AppKit precise scroll events are not keyboard modifier events; they must be sent with Ghostty's packed `ScrollMods.precision` bit and without scaling by the window backing factor. Con mirrors Ghostty's AppKit host by sending precise deltas through that path with the same 2x multiplier, leaving Ghostty core to accumulate sub-row remainders and apply terminal-row scrolling.

The workspace pump still drains title/process/link events for every terminal surface, including background tabs, but macOS native scroll-container synchronization is visible-tab-only. Hidden tabs do not need AppKit scroll frame updates, and keeping that work out of fast scroll bursts avoids unnecessary main-thread native view churn without dropping terminal lifecycle events.

One macOS embedding invariant is subtle but important: after GPUI changes split or zoom layout, Con must not position the embedded Ghostty surface from `NSClipView.documentVisibleRect` in the same synchronization pass, and it must not skip native frame mutations from a local tuple cache. Ghostty's native app owns an AppKit `layout()` method for its scroll hierarchy, but Con drives the hierarchy imperatively from GPUI callbacks; AppKit can return a stale visible rect immediately after those mutations, and a cached leaf-local frame can be stale after GPUI reparents or resizes a split subtree. Con therefore computes and reapplies the surface frame from Con-owned pane bounds plus Ghostty scrollbar state and treats AppKit scroll geometry as an effect, not as the source of truth.

Con also no longer tries to outsmart Ghostty's resize path with host-side coalescing. The embedded surface now updates its core size immediately on layout using AppKit backing-size conversion, which is much closer to how Ghostty's own macOS app drives `ghostty_surface_set_size`.

The draw boundary matters as much as the size boundary. Ghostty's Metal renderer reads the hosted IOSurface `CALayer` bounds at draw time, so Con must not force `ghostty_surface_draw` from a pre-frame backing sync. The correct sequence during live resize is: sync content scale / PTY pixel size metadata, commit the AppKit host/document/surface frames, then force the synchronous draw. Drawing between those steps can present against stale layer bounds on Intel macOS compositors.

Do not set `NSWindow.contentResizeIncrements` from terminal cell size. It makes manual resizing feel cell-snapped, but AppKit also applies those increments to zoom/fullscreen sizing. That can leave a visible unfilled strip at the bottom of a maximized/fullscreen Con window. Let the window fill the platform-provided frame exactly; the embedded Ghostty surface and PTY resize path are responsible for mapping the resulting pixel size to terminal rows/columns.

Chrome seams around macOS embedded terminals are native-surface seams, not generic GPUI borders. The window root is transparent and the terminal can be translucent, so a fully transparent 1–4 px gap during agent-panel, input-bar, top-bar, split-divider, or vertical-sidebar motion can expose the desktop/backdrop as a white flash. Cover only the seam, and tint that cover from the adjacent terminal background at full opacity; GPUI seam covers do not get Ghostty's native blur/compositing, so making them translucent recreates the leak. Do not reintroduce a full terminal matte, which hides the leak by veiling the whole surface and creates a different blink.

Pane dividers still need to be legible. The divider color should not be the raw terminal background itself; that removes the user's visual split boundary. Use an opaque separator precomposed over the terminal background instead: visually it reads like a low-alpha foreground line, but the rendered `Hsla` alpha is `1.0`, so it never exposes the transparent window backing during fast split/zoom/chrome motion.

The more robust rule is that there should be no clear backing at terminal/chrome boundaries on macOS. Chrome regions that are outside the native Ghostty view but visually adjacent to it should be precomposed over the terminal background before GPUI paints them, preserving the intended translucent-over-terminal look without letting the desktop participate in the color. The native Ghostty host view also keeps a small backing overdraw under GPUI seams, while the actual Metal surface stays aligned to the pane bounds. That gives AppKit and GPUI a safe overlap area during fast split, zoom, sidebar, agent-panel, and input-bar layout transitions.

Fast chrome toggles have one more guard: while input bar, agent panel, or tab strip changes are in flight, the workspace enables one shared native underlay view below all Ghostty hosts in that window. This is not the old full-terminal matte; it sits below terminal content and only replaces otherwise-clear window backing during AppKit/GPUI geometry races. Visibility is parent-scoped, not pane-scoped: each `GhosttyView` owns an id in a native owner set, and the shared underlay hides only when the set becomes empty. That prevents a newly initialized pane from hiding the underlay while another pane is still inside a transition. When the guard expires, the underlay is hidden again so normal terminal glass returns.

The right agent panel has one extra rule: on macOS, its visual animation must not continuously animate the terminal layout width. A width-animated panel can move the native/GPUI boundary by dozens of pixels between frames, far beyond any reasonable seam cover. Con therefore snaps the terminal layout to the panel's stable open/closed geometry and uses the motion value only for the panel's visual/content reveal. Windows and Linux keep the normal GPUI width animation because they do not embed the full Ghostty AppKit surface.

One build-time rule matters too: the embedded Ghostty runtime itself must be compiled as a release-class library. Con now passes Zig `-Doptimize=ReleaseFast` for the macOS Ghostty build by default. Without that, traces are dominated by Ghostty's own formatter, reflow, integrity-check, and debug-allocation paths, which makes Con look fundamentally slower than standalone Ghostty even when the host integration is not the main bottleneck.

## Agent execution

The `terminal_exec` tool writes the command into the visible Ghostty pane.

Completion strategy:

- preferred: Ghostty `COMMAND_FINISHED`
- fallback: bounded recent-output capture after timeout

We no longer depend on a grid callback path.

## Design consequence

Ghostty is now the terminal runtime.

If con needs better pane intelligence, the right long-term move is:

1. consume stronger Ghostty facts when the C API exposes them
2. upstream missing observability hooks when needed
3. keep product inference in con's pane runtime observer

The wrong move is rebuilding another terminal engine beside Ghostty.

# macOS Wake and External Display Hang

## What Happened

Con became permanently unresponsive after the Mac woke while reconnecting to
an external 4K display. The problem occurred twice on the same machine. Force
quit was the only recovery.

Both system hang reports captured the same main-thread stack. The first sample
started six seconds after wake; the second remained blocked for more than an
hour. In both cases AppKit was processing a finalized display configuration
when CoreVideo blocked in `CVDisplayLink::stop()`.

## Root Cause

Ghostty's historically named `ghostty_surface_set_occlusion` API accepts a
`visible` boolean, not an `occluded` boolean. Con's Rust wrapper documented and
forwarded that value as `occluded`. Its only call passed `true` during final
teardown, which actually kept or restarted the renderer instead of pausing it.

The macOS host also did not mirror system sleep or ordinary window occlusion
into embedded Ghostty surfaces. Consequently, every visible or previously
visible pane could retain an active per-surface `CVDisplayLink` while
WindowServer removed and rebuilt displays during wake.

CoreVideo stops display links synchronously as part of that system display
reconfiguration. The two diagnostic reports show the main dispatch queue
waiting in that stop path, while three `CVDisplayLink` threads remained in the
process. This was a display-link lifecycle failure, not terminal parsing,
layout computation, Metal drawing, or a Rust mutex deadlock.

Con also allowed its ordinary GPUI layout pass to synchronize Ghostty's display
id, backing scale, and size during this unstable interval. That did not create
the sampled wait, but it made the display transition less deterministic.

## Fix Applied

- Replace Con's ambiguous `set_occlusion` wrapper with an explicit
  `set_visible` API, including the no-op Windows, Linux, and stub backends.
- Observe the AppKit window's occlusion state and mirror visibility to the
  embedded Ghostty surface.
- Observe workspace sleep and mark every surface invisible before display
  teardown.
- Keep each renderer paused after wake while coalescing AppKit screen and
  backing-property notifications. Every new display event restarts the quiet
  interval instead of relying on the first post-wake geometry.
- Once display changes become quiet, synchronize display id, scale, and size,
  then resume only surfaces whose native view and window are actually visible.
- Invalidate both window and workspace observers before releasing the raw
  Ghostty surface pointer; delayed wake callbacks carry a generation guard.

The change is compiled only into the macOS Objective-C bridge. Windows and
Linux terminal lifecycle and rendering paths are unchanged.

## What We Learned

Embedding a renderer means owning its power and display lifecycle, not only its
view geometry. AppKit hiding a child view is not sufficient evidence that the
embedded renderer has stopped its frame source.

A cross-display fix must treat sleep, occlusion, display identity, backing
scale, and framebuffer size as one ordered state transition. Reacting only to
`NSWindowDidChangeScreenNotification` is too late for display teardown and too
early for stable wake geometry.

Zed later replaced GPUI's per-window `CVDisplayLink` teardown with a
per-display registry upstream. Con cannot consume that single commit in
isolation because the corresponding GPUI API generation is incompatible with
the currently pinned gpui-component release. That coordinated dependency
migration remains worthwhile, but it is separate from this targeted lifecycle
fix and must pass full cross-platform UI regression testing.

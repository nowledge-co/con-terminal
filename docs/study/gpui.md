# Study: GPUI

## Overview

con currently builds against the upstream GPUI crates from the Zed repository, not a local GPUI-CE checkout. The framework remains Apache 2.0 licensed.

## Key Properties

- **Rendering**: Metal (macOS), Blade/OpenGL (Linux), Blade/D3D (Windows)
- **Text**: Core Text (macOS), cosmic-text (Linux), DirectWrite (Windows)
- **Layout**: taffy (flexbox/CSS Grid)
- **IME**: Full InputHandler trait — macOS AppKit, Linux X11 xim, Windows WM_IME
- **Source**: `gpui-pre` crates.io snapshots of `zed-industries/zed` (package `gpui-pre`, imported as `gpui`)

## Programming Model

Three registers:
1. **Entity** — state management (like React state + context)
2. **Views** — high-level declarative UI (struct + Render trait)
3. **Elements** — low-level imperative UI (custom paint, canvas)

Terminal rendering uses the **Element** register: custom `canvas()` calls to paint cells directly to GPU.

## Interaction API Notes

GPUI has two different hover concepts:

- `.hover(|style| ...)` is styling-only hover state on a normal `Div`.
- `.on_hover(...)` is an enter/exit event listener and is only available on a `Stateful<Div>`.

If a `div()` chain needs `.on_hover(...)`, assign a stable element id first:

```rust
div()
    .track_focus(&self.focus_handle)
    .id(&self.focus_handle)
    .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
        if !*hovered {
            // clear hover state
            cx.notify();
        }
    }))
```

This is easy to miss because macOS-only checks do not compile the Windows/Linux `#[path]` terminal views. For platform-specific GPUI files, validate on the target platform or let portable CI compile those branches before considering the change closed-loop.

## Virtual Font Names

GPUI reserves dot-prefixed font family names for framework-level virtual fonts:

- `.SystemUIFont` resolves to the platform UI font (`.AppleSystemUIFont` on macOS, the Windows system UI font on Windows, and the platform default on Linux). This is correct for GPUI-rendered prose, settings, labels, and other native UI text.
- `.ZedMono` resolves inside GPUI through `font_name_with_fallbacks` to Zed's bundled/editor mono family (`Lilex` upstream). It is a GPUI alias, not a terminal renderer font.

Do not pass dot-prefixed GPUI virtual family names into terminal backends. Ghostty config, DirectWrite terminal rendering, and the Linux terminal renderer need concrete terminal-capable font family names. Con keeps `appearance.ui_font_family = ".SystemUIFont"` as the UI default, but sanitizes `terminal.font_family` so pseudo families fall back to `Ioskeley Mono`.

## Terminal Canvas Pattern

```rust
// Pseudocode for terminal cell rendering
fn paint_terminal(grid: &Grid, cx: &mut PaintContext) {
    for row in 0..grid.rows {
        for col in 0..grid.cols {
            let cell = grid.cell(row, col);
            // Background
            cx.fill_rect(cell_rect(row, col), cell.bg_color);
            // Text
            cx.draw_text(cell.char, cell_origin(row, col), cell.fg_color, &font);
        }
    }
}
```

GPUI's text shaping handles ligatures, emoji, CJK — same quality as Zed's editor.

## Reference Sources

Read these upstream or read-only reference sources when you need framework details:

- upstream `gpui` crates under the Zed repository checkout
- `3pp/` reference material in this repo, when present, as read-only study material only

## Dependency

In our workspace manifest, GPUI resolves from exact-pinned `gpui-pre` crates.io snapshots of Zed rather than a local path dependency.

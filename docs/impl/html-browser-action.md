# HTML browser action

## Decision

Use crates.io `webbrowser` 1.2.4 on macOS and Windows to open local HTML in the default web browser.
This is a new action; no prior ADR is superseded. GPUI `open_url(file_url)`
uses the file association, which can select an editor. Maintaining native
browser discovery ourselves would duplicate platform code. The dependency
selects the browser on macOS and Windows. On Linux, resolve the HTTPS desktop
handler with `xdg-mime` and launch that desktop ID with `gtk-launch`; the dependency's
fallback to `gio open` could select an editor for a file URL. Missing desktop
tools or entries produce an error instead of opening the HTML MIME handler.
GTK resolves nested desktop IDs. Both commands run on the host through
`con_paths::host_command` in Flatpak; no sandbox/host path lookup is mixed.
Linux requires `xdg-mime` and `gtk-launch` on the host. Flatpak file access
still follows the package's filesystem permissions and requires platform testing.

## Contract

- PaneTree's merged editor bar shows a component Button for the active
  `.html` or `.htm` file, case-insensitively, including when pane titles are hidden.
- The action reads that editor's active path at click time, converts an absolute
  path to an escaped `file:` URL, and launches the browser off the UI thread.
- Open the saved file without changing its buffer or dirty state. No temporary
  HTML, local server, shell interpolation, terminal command or configuration change.
- Consume pointer events so clicking or dragging the button cannot drag a pane.
  Report path or launch errors through the existing notification surface.

## Implementation and verification

One writer owns `editor_browser.rs`, `editor_browser_linux.rs`, its registration in `main.rs`, the insertion
in `pane_tree.rs`, the app manifest/lockfile and editor documentation. Independent
review follows implementation and is read-only; there are no parallel writers.

1. Add URL/launch helpers and the component button; integrate into the merged bar.
2. Verify extension gating, URL round trips for special characters, active-tab
   switching, fixed button bounds and suppressed pane dragging with focused tests.
3. Run Rust formatting, affected app tests, build/check and a read-only review.
4. Launch an isolated app, open an HTML fixture in the right pane, click the
   button and inspect the browser URL/content. Inspect light/dark and hidden-title
   states. Local macOS evidence does not validate Windows/Linux runtime behavior.

Delivery requires reporting actual checks and unavailable runtime coverage.

## Local acceptance (2026-10-09)

- `cargo fmt --all -- --check` and `git diff --check` passed.
- `cargo test --locked -p con`: 540 passed. After the final accessibility/error
  wording changes, `cargo test --locked -p con editor_browser`: 7 passed.
- `just build` built the macOS app and sibling CLI. Native Ghostty used an
  official source archive at the manifest-pinned revision, downloaded through
  the configured proxy; no dependency revision or build script was changed.
- In an isolated session, clicking the action for `HTML 预览 #1%100.html`
  opened Edge with the escaped `file:` URL and expected heading/content.
  Switching to `second.htm` opened that file and its different heading.
  Switching to `README.md` removed the action; the right editor bar's icon
  and tooltip were inspected in the running dark UI.
- Live UI capture initially required window resizing to refresh the content.
  This observation was not diagnosed or attributed to this feature. Light
  theme and hidden-title behavior were exercised by the GPUI layout test,
  without live visual acceptance of those states.
- Windows/Linux runtime and Flatpak launch/file access remain unverified.
  Linux command-runner tests ran on macOS and do not prove native integration.

Build/test logs and disposable fixtures are under
`/tmp/con-html-browser-20261009/` on the validation machine.

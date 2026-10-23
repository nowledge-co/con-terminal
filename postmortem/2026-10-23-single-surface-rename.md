# Invisible rename input on an unnamed surface

## What happened

On a default terminal pane, Rename Current Surface could move keyboard focus
without showing an editor. The input existed, but there was no visible place to
type. This affected the shared pane UI, not a particular terminal backend.

## Root cause

The renderer showed surface navigation only for multiple surfaces, an explicit
name, or an orchestrator owner. It did not include the transient rename state.
An unnamed singleton therefore omitted the row containing the focused input.

Live macOS verification also found that the workspace's modal-dismissal path
unconditionally restored terminal focus after the palette action had explicitly
focused the rename input. Rendering the input alone was not sufficient: typed
text still went into the shell.
The old raw Escape-key listener also missed the input component's Escape
action. Cancellation must listen for that action after the input has had the
opportunity to handle its own composition or popup.
The tab's mouse handlers also focused the terminal for clicks inside its rename
input. Double-clicking the input could start another rename operation.

## Fix

Show the existing surface header while its active surface is being renamed.
Compare surface IDs, not pane IDs: a pane can contain a single surviving surface
whose ID differs from its pane. Keep the compact single-row header introduced
in #510 rather than reinstating the previous stacked layout.
Preserve the explicit focus transfer while a surface rename is active. Enter
and Escape return focus to the active terminal after ending the edit.
The workspace handles the input component's propagated Escape action, rather
than intercepting composition or relying only on a raw key event.
While editing, leave mouse clicks with the input and disable rename reentry.

## Lessons and regression coverage

- Visibility must include transient editing state, not just saved metadata.
- A focused input must have rendered, nonempty bounds.
- Exercise unnamed, named, owned and multiple surfaces, split panes, unrelated
  rename editors, and different pane/surface IDs.
- Check cancellation, confirmation and clearing the name restore the appropriate
  ordinary chrome, including the configuration that hides pane title bars.

Tracking: [#225](https://github.com/nowledge-co/con-terminal/issues/225).
Fix and regression contribution:
[#509](https://github.com/nowledge-co/con-terminal/pull/509) by
[@Nitjsefnie](https://github.com/Nitjsefnie).

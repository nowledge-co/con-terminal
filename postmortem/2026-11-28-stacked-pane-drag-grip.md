# Stacked pane drag grip

The space after pane-local terminal tabs looked like a pane grip. It relied
on the outer title bar, which can be hidden, and the tab drag preview used
one terminal's title even though its payload represented the entire pane.

The surface strip now gives its trailing space a pane drag target of its
own. Both entry points use the same stack preview and the existing pane
payload. Overflow continues to scroll the terminal tabs. When no spare
space remains, the tabs themselves remain valid grips.

The render-level regression checks both entry points with pane titles
visible and hidden. Drag previews should identify the unit being moved.

## Closing tabs in the same strip

Inactive close buttons previously depended on a hover repaint before they
could receive a click. Content-sized tabs also changed the next close
button's position immediately after removal. Inactive close buttons stay
visible at lower emphasis, and a click temporarily keeps the measured
tab slots so the next item can be closed at the same pointer position.
The terminal strip uses the same close callback for middle-click as the
button and only offers the action where the pane tree permits it. Existing
agent confirmation and unsaved-file protection still own the decision.

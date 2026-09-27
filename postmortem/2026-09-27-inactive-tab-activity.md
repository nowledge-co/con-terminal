# Visible inactive tabs stopped showing activity

## What happened

Switching to another application stopped Con's Busy ring even when its sidebar
remained visible. The stationary quarter-circle could look like stalled work or
determinate progress. This followed the previous documented rendering policy;
it did not establish that background status collection had stopped.

## Root cause

`TabActivity::render` required both window visibility and activation to animate.
Keyboard focus was therefore treated as a proxy for whether the user could see
the status indicator. Without animation, the phase returned to zero.

Removing the activation condition alone exposed another lifecycle gap: a render
while hidden can retire all animation callbacks. GPUI updates window visibility
without automatically invalidating this activity view, so uncovering without an
activation or data event could leave it static.

## Fix applied

Animate visible Busy markers independently of activation. Each activity entity
subscribes to window visibility changes and invalidates itself on a transition.
Both the sidebar and horizontal tabs use that constructor. Existing clipping,
reduced-motion and determinate-progress guards remain unchanged. Status polling
and aggregation are unchanged.

## What we learned

Visibility, keyboard focus and task activity are independent facts. Use the
platform's existing visibility signal rather than a new timer or focus proxy.
Stopping an animation also requires testing how it restarts after all pending
callbacks have drained.

The GPUI regression test exercises actual frame callbacks: active, inactive but
visible, hidden after a render, uncovered without activation, reduced motion,
determinate progress and clipping. It failed with the old activation guard and
again when the visibility subscription was removed in an ablation experiment.

On macOS, a separate native cover window reproduced partial and full occlusion.
Two-second samples recorded 94 activity-layer renders while inactive and visible,
zero while fully covered, and 90 after uncovering without activation. Each sample
recorded two process batches, rather than frame-rate process collection. Two
inactive-window captures showed different arc angles with stationary icon and
selection geometry. These counts describe the local fixture, not a cross-platform
performance budget; Windows and Linux GUI behavior was not exercised.

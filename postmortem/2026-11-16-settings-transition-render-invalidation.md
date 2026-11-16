# Settings transition looked like an immediate replacement

Settings retained animation values and requested animation frames, but did not
explicitly invalidate its entity for the next frame. Reusing a retained scene
could therefore leave the content at an endpoint or first sample until another
input caused a render. The previous subtle opacity-only change also provided
little spatial evidence of a transition.

Schedule an entity notification on the next frame only while the section or
selection motion is active. Use a bounded 220ms entrance with 4pt relative
settling and an opacity fade. Reduce Motion samples the endpoint immediately,
and idle settings stop scheduling callbacks. Retain fixed surface dimensions
and keep outgoing content out of the interaction tree.

Verification must exercise real page switches, not just inspect endpoint
screenshots or verify that a motion value changes numerically. A requested frame
and an invalidated view are separate requirements in retained GPUI rendering.

# Windows terminal glyph edges looked rough

## What happened

Windows terminal text could look rough, uneven, or overly thin, especially at small sizes, 125% display scaling, and with dark foreground text on the default light theme. Earlier work removed ClearType RGB fringes, but the remaining grayscale path still did not match DirectWrite's final text composition.

## Root cause

The glyph atlas rasterized a white brush onto black with hard-coded DirectWrite gamma and enhanced contrast. Those corrected coverage values were then reused for every foreground/background pair and composited in HLSL with a simple linear interpolation.

That bakes a white-on-black assumption into the atlas. DirectWrite grayscale correction depends on the actual foreground intensity, so applying the baked coverage to dark-on-light text produces different edge weight from light-on-dark text. Background-only instances also sampled atlas texel `(0, 0)`, allowing the first cached glyph's edge to contaminate cells that had no glyph.

## Fix applied

- Read the active DirectWrite gamma and grayscale contrast once at renderer startup, then rasterize the Direct2D atlas with neutral gamma and contrast through `IDWriteFactory1` while retaining grayscale antialiasing.
- Apply DirectWrite-compatible, foreground-aware gamma and grayscale contrast correction in the pixel shader, based on Windows Terminal's MIT-licensed AtlasEngine formulas. The shader receives the system-derived values rather than assuming the default text tuner settings.
- Preserve a separate CJK contrast profile through a renderer-private instance attribute while keeping the existing medium-weight CJK format.
- Mark zero-sized atlas instances as glyph-free in the vertex shader so the pixel shader forces their glyph coverage to zero.
- Fall back to the previous atlas-corrected path if the newer DirectWrite interface is unexpectedly unavailable, so rendering quality can degrade without preventing a terminal pane from opening.
- Add tests for the DirectWrite gamma coefficients, renderer initialization, and every embedded HLSL entry point.

## What we learned

A reusable glyph atlas should contain neutral coverage, not coverage corrected for the colors used while populating the atlas. Gamma/contrast correction belongs at the final composition stage where the real foreground color is available. Grayscale antialiasing remains preferable to ClearType for Con's offscreen, transparent, and rescaled composition path because it avoids device-specific RGB subpixel fringes.

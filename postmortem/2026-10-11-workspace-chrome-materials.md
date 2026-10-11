# Inconsistent workspace chrome materials

## What happened

The titlebar, left sidebar, bottom input bar, and agent sidebar looked like
separate materials. The bottom rule was stronger than the sidebar edges, and
the agent sidebar formed a darker block beneath the otherwise seamless header.

## Root cause

The titlebar, left sidebar, and input bar used terminal opacity. The agent
sidebar used elevated UI opacity and a separately blended base color. Sharing
only the base color did not fix the mismatch: different alpha values still
produced different surfaces. Decorative edges also had independent strengths.

## Fix

Use the same titlebar color and effective Window Chrome opacity for all four
auxiliary regions. Keep Terminal Glass independent. Use one subdued edge tone
and one resize-hover tone, preserving existing geometry and hit targets.

Leave pane dividers, native presentation synchronization, transition seam
covers, and terminal blur/opacity behavior unchanged. Decorative fills are not
substitutes for presentation synchronization.

## Verification and lesson

Native macOS workspace tests passed (156), as did the agent-panel tests (23),
including focus, resize, and narrow layout coverage. The app and companion CLI
built successfully; Rust formatting and diff whitespace checks passed. Light
and dark chrome were inspected with the panels visible, including a preview
using the user's normal configuration.

Windows/Linux desktop appearance still requires native acceptance; a macOS
build does not prove their visual result.

A shared material needs shared color and opacity ownership, not merely similar
RGB values. Controls should affect the regions their names describe.

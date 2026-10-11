# Agent Activity Chrome

The sidebar has three distinct jobs: choose the session's provider/model, show
what is happening now, and retain the conversation and tool trace. Keep those
regions legible without repeating the same status in each one.

## Live activity

- Show one live summary below the session controls. Approval takes precedence
  over a running tool, which takes precedence over thinking or writing.
- Animate only this summary. Approval is a static shield; it is waiting for the
  user, not work progressing in the background. Trace rows retain their tool
  icons and running/completed states without additional spinners.
- Use a readable UI-font action label and a separate mono-font argument line.
  Let both wrap at narrow widths rather than squeeze them into one row.
- Separate controls, activity, and messages with space and a quiet neutral fill,
  not stacked full-width and inset rules or a tiny uppercase kicker.
- Use solid semantic foreground tokens on transparent glass. Translucency
  belongs to the fill, not the text.

## Session controls

The provider icon identifies the provider; its tooltip names it. The model
selector takes remaining width and shrinks before the action buttons. Stop,
auto-approve, history, and new conversation remain component-library controls.
Do not add an unrelated branding glyph or a second busy indicator to this row.

## Performance and behavior

The summary is a small chrome subtree outside the virtualized transcript. It
reads existing panel state; it does not poll terminals, request extra events,
reparse markdown, or invalidate message caches. Approval callbacks, tool results,
streaming, follow-output behavior, composer focus, and transcript virtualization
are unchanged. Removal of duplicate spinners also removes their animation work.

The tool/approval footer has intrinsic content height with a 320px scrolling
limit. It must not request 100% height from an auto-height parent: that can
collapse the approval controls even while the live summary reports a request.

Layout tests cover a 280px panel, a wide panel, enlarged UI text, and CJK/long
tool details. Native visual acceptance should cover thinking, writing, tool
execution, approval, and idle in light and dark themes.

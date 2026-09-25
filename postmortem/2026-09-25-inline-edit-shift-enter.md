# Shift+Enter submitted an inline message edit

## What happened

Pressing Shift+Enter while editing a historical user message submitted the
draft and truncated the later conversation instead of leaving the editor open
with a newline. This predated the GPUI dependency upgrade.

## Root cause

`AgentPanel::start_editing` accepted every `PressEnter` event with
`secondary: false`. Both Enter and Shift+Enter satisfy that condition. The main
and compact composers already guarded Shift+Enter, but the historical-message
editor did not.

## Fix applied

Require both `secondary: false` and `shift: false` using the input event's
modifier fields. No additional keystroke observer or state is needed.

## What we learned

Test the keyboard interaction rather than only the submission helper. The GPUI
regression test renders the panel, focuses the historical-message editor, and
simulates Shift+Enter. It checks that the newline is retained, editing remains
active, and later messages survive; plain Enter then submits the edited text.
Before the fix, the test failed because the editing state had become `None`.

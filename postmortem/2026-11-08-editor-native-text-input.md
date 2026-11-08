# File editor bypassed native text input

## What happened

CJK input methods could not compose text in a file editor pane. Ordinary Latin
keystrokes worked, which hid the missing native text-input integration.

## Root cause

The workspace captured printable key-down events and directly inserted key
characters into EditorBuffer. EditorView never registered a GPUI
EntityInputHandler during paint. The platform therefore had no editable text,
UTF-16 selection, marked range or candidate-window anchor for the editor.
Physical key events cannot represent composition, dead keys or committed text.

## Fix

Register a text-input handler only for editable source views. Receive committed
and marked text through GPUI's native callbacks; keep physical-key handling for
editor commands and indentation. Translate document-wide UTF-16 ranges into
UTF-8 buffer positions without splitting surrogate pairs, with logical LF
newlines independent of the file's preserved disk line endings. Treat successive
preedit updates and the final commit as one undo operation. Use shaped text for
caret/selection positions, mouse hit testing and candidate-window bounds.

## Lessons

A custom editor needs the full platform text-input contract even when ASCII
looks correct. Never derive ordinary document text from physical key names.
Cover composition replacements, selected-range replacement, surrogate pairs,
multiline text, cancellation, read-only previews and the painted input handler.

## Verification

155 editor and affected-consumer tests pass, including the painted native-input
handler, UTF-16 queries, preedit/commit undo, long CJK candidate-anchor scrolling,
read-only previews, focus loss and existing workspace/pane/editor behavior. The
application and companion CLI build successfully; formatting and docs routing
checks pass. An isolated native preview confirms ordinary text inserts once,
selection and deletion work, and mixed CJK/emoji rendering is intact. Automated
key events did not trigger the installed input method's Chinese mode. The user
then confirmed real candidate presentation and Chinese text input worked in the
isolated native preview; that preview was closed afterward. Windows/Linux native
IME runtime has not been exercised in this change.

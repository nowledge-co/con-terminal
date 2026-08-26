#[cfg(target_os = "macos")]
use gpui::KeyDownEvent;

#[cfg(any(target_os = "windows", target_os = "linux"))]
pub(crate) fn restored_terminal_output(lines: Option<&[String]>) -> Option<Vec<u8>> {
    con_ghostty::restored_terminal_output_text(lines?).map(String::into_bytes)
}

#[cfg(target_os = "macos")]
pub(crate) fn key_down_may_write_terminal(event: &KeyDownEvent, special_key_writes: bool) -> bool {
    let keystroke = &event.keystroke;
    if keystroke.modifiers.platform {
        return false;
    }

    if keystroke.modifiers.control
        && keystroke.modifiers.shift
        && !keystroke.modifiers.alt
        && matches!(keystroke.key.as_str(), "v")
    {
        return true;
    }

    let single_character_key = keystroke.key.chars().count() == 1;

    special_key_writes
        || single_character_key
        || (keystroke.modifiers.alt && !keystroke.modifiers.control)
}

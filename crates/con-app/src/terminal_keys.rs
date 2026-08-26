#[cfg(any(target_os = "windows", target_os = "linux"))]
use con_ghostty::vt::{VtKeyAction, VtKeyEvent, VtKeyModifiers, is_supported_functional_key};
#[cfg(any(target_os = "windows", target_os = "linux"))]
use gpui::{KeyDownEvent, Keystroke, Modifiers};

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[derive(Debug, Clone)]
pub struct TrackedVtKey {
    text: String,
    unshifted_codepoint: Option<char>,
    modifiers: VtKeyModifiers,
    consumed_modifiers: VtKeyModifiers,
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
impl TrackedVtKey {
    pub fn from_press(event: &VtKeyEvent<'_>) -> Self {
        Self {
            text: event.text.to_owned(),
            unshifted_codepoint: event.unshifted_codepoint,
            modifiers: event.modifiers,
            consumed_modifiers: event.consumed_modifiers,
        }
    }

    pub fn release<'a>(&'a self, key: &'a str) -> VtKeyEvent<'a> {
        VtKeyEvent {
            key,
            text: &self.text,
            unshifted_codepoint: self.unshifted_codepoint,
            action: VtKeyAction::Release,
            modifiers: self.modifiers,
            consumed_modifiers: self.consumed_modifiers,
        }
    }

    pub fn release_with_modifiers<'a>(
        &'a self,
        key: &'a str,
        modifiers: &Modifiers,
    ) -> VtKeyEvent<'a> {
        let modifiers = vt_modifiers(modifiers);
        VtKeyEvent {
            key,
            text: &self.text,
            unshifted_codepoint: self.unshifted_codepoint,
            action: VtKeyAction::Release,
            modifiers,
            consumed_modifiers: VtKeyModifiers {
                shift: modifiers.shift && self.consumed_modifiers.shift,
                control: modifiers.control && self.consumed_modifiers.control,
                alt: modifiers.alt && self.consumed_modifiers.alt,
                platform: modifiers.platform && self.consumed_modifiers.platform,
            },
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
pub fn vt_key_down_event(event: &KeyDownEvent) -> Option<VtKeyEvent<'_>> {
    let keystroke = &event.keystroke;
    let functional = is_supported_functional_key(&keystroke.key);

    // GPUI explicitly sets this for character-producing input such as
    // Windows AltGr. Let the InputHandler deliver the committed text so
    // the encoder cannot turn Ctrl+Alt into a terminal chord or duplicate
    // an IME/dead-key commit.
    if event.prefer_character_input && !functional {
        return None;
    }

    Some(vt_key_event(
        keystroke,
        if event.is_held {
            VtKeyAction::Repeat
        } else {
            VtKeyAction::Press
        },
    ))
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
pub fn vt_key_event(keystroke: &Keystroke, action: VtKeyAction) -> VtKeyEvent<'_> {
    let functional = is_supported_functional_key(&keystroke.key);
    // Space has a physical-key identity but still requires its layout text
    // for legacy and Kitty encoding (including Ctrl-Space -> NUL).
    let text = if functional && keystroke.key != "space" {
        ""
    } else {
        printable_text(keystroke).unwrap_or("")
    };
    let modifiers = vt_modifiers(&keystroke.modifiers);
    let consumed_modifiers = VtKeyModifiers {
        shift: !functional && modifiers.shift && !text.is_empty(),
        ..VtKeyModifiers::default()
    };

    VtKeyEvent {
        key: &keystroke.key,
        text,
        unshifted_codepoint: conservative_unshifted_codepoint(text, modifiers.shift),
        action,
        modifiers,
        consumed_modifiers,
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn vt_modifiers(modifiers: &Modifiers) -> VtKeyModifiers {
    VtKeyModifiers {
        shift: modifiers.shift,
        control: modifiers.control,
        alt: modifiers.alt,
        platform: modifiers.platform,
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn printable_text(keystroke: &Keystroke) -> Option<&str> {
    keystroke
        .key_char
        .as_deref()
        .filter(|text| !text.is_empty() && text.chars().all(|ch| !ch.is_control()))
        .or_else(|| {
            let mut chars = keystroke.key.chars();
            let ch = chars.next()?;
            (chars.next().is_none() && !ch.is_control()).then_some(keystroke.key.as_str())
        })
}

#[cfg(any(target_os = "windows", target_os = "linux", test))]
fn conservative_unshifted_codepoint(text: &str, shift: bool) -> Option<char> {
    let mut chars = text.chars();
    let ch = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    if !shift {
        return Some(ch);
    }

    // GPUI can retain Shift while reporting an already-lowercase key_char for
    // shortcut chords such as Ctrl+Shift+P. Preserve that cased identity so
    // Kitty does not fall back to emitting bare text. Shifted punctuation is
    // intentionally excluded because its physical unshifted key is layout-
    // dependent. Otherwise lowercase one cased codepoint so non-ASCII keys
    // such as Ä/ä remain identifiable without guessing a keyboard layout.
    if ch.is_lowercase() {
        return Some(ch);
    }
    let mut lowercase = ch.to_lowercase();
    let unshifted = lowercase.next()?;
    (lowercase.next().is_none() && unshifted != ch).then_some(unshifted)
}

/// Map legacy terminal Ctrl-key aliases to the C0/DEL byte they emit.
///
/// This intentionally covers the classic byte layer only. Modern protocols
/// such as Kitty keyboard / modifyOtherKeys belong in libghostty-vt's key
/// encoder, not in a growing Rust-side terminal keyboard table.
pub fn ctrl_key_to_c0(key: &str) -> Option<u8> {
    let key = key.trim();
    if key.eq_ignore_ascii_case("space") {
        return Some(0x00);
    }

    let mut chars = key.chars();
    let ch = chars.next()?;
    if chars.next().is_some() {
        return None;
    }

    match ch {
        '@' => Some(0x00),
        '2' => Some(0x00),
        '3' => Some(0x1b),
        '4' => Some(0x1c),
        '5' => Some(0x1d),
        '6' => Some(0x1e),
        '7' => Some(0x1f),
        '8' => Some(0x7f),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' => Some(0x1f),
        '/' => Some(0x1f),
        // Mirrors Ghostty / Kitty ctrlSeq for Ctrl+~ rather than treating it
        // as Ctrl+`'s physical-key NUL alias.
        '~' => Some(0x1e),
        '?' => Some(0x7f),
        ch if ch.is_ascii_alphabetic() => Some(ch.to_ascii_uppercase() as u8 - b'@'),
        _ => None,
    }
}

pub fn ctrl_chord_to_c0(key: &str) -> Option<u8> {
    let key = key.trim();
    let lower = key.to_ascii_lowercase();
    for prefix in ["ctrl-", "control-", "c-"] {
        if lower.starts_with(prefix) {
            return ctrl_key_to_c0(&key[prefix.len()..]);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{conservative_unshifted_codepoint, ctrl_chord_to_c0, ctrl_key_to_c0};

    #[test]
    fn shifted_lowercase_retains_kitty_key_identity() {
        assert_eq!(conservative_unshifted_codepoint("p", true), Some('p'));
        assert_eq!(conservative_unshifted_codepoint("Ä", true), Some('ä'));
        assert_eq!(conservative_unshifted_codepoint("*", true), None);
        assert_eq!(conservative_unshifted_codepoint("ss", true), None);
    }

    #[test]
    fn ctrl_key_maps_letters_to_c0() {
        assert_eq!(ctrl_key_to_c0("a"), Some(0x01));
        assert_eq!(ctrl_key_to_c0("Z"), Some(0x1a));
    }

    #[test]
    fn ctrl_key_maps_defined_ascii_punctuation_to_c0() {
        assert_eq!(ctrl_key_to_c0("space"), Some(0x00));
        assert_eq!(ctrl_key_to_c0("@"), Some(0x00));
        assert_eq!(ctrl_key_to_c0("2"), Some(0x00));
        assert_eq!(ctrl_key_to_c0("3"), Some(0x1b));
        assert_eq!(ctrl_key_to_c0("4"), Some(0x1c));
        assert_eq!(ctrl_key_to_c0("5"), Some(0x1d));
        assert_eq!(ctrl_key_to_c0("6"), Some(0x1e));
        assert_eq!(ctrl_key_to_c0("7"), Some(0x1f));
        assert_eq!(ctrl_key_to_c0("8"), Some(0x7f));
        assert_eq!(ctrl_key_to_c0("["), Some(0x1b));
        assert_eq!(ctrl_key_to_c0("\\"), Some(0x1c));
        assert_eq!(ctrl_key_to_c0("]"), Some(0x1d));
        assert_eq!(ctrl_key_to_c0("^"), Some(0x1e));
        assert_eq!(ctrl_key_to_c0("_"), Some(0x1f));
        assert_eq!(ctrl_key_to_c0("/"), Some(0x1f));
        assert_eq!(ctrl_key_to_c0("~"), Some(0x1e));
        assert_eq!(ctrl_key_to_c0("?"), Some(0x7f));
    }

    #[test]
    fn ctrl_key_rejects_undefined_ascii_punctuation() {
        assert_eq!(ctrl_key_to_c0("}"), None);
        assert_eq!(ctrl_key_to_c0("1"), None);
        assert_eq!(ctrl_key_to_c0("0"), None);
        assert_eq!(ctrl_key_to_c0("9"), None);
        assert_eq!(ctrl_key_to_c0("enter"), None);
    }

    #[test]
    fn ctrl_chord_accepts_terminal_spellings() {
        assert_eq!(ctrl_chord_to_c0("Ctrl-C"), Some(0x03));
        assert_eq!(ctrl_chord_to_c0("control-]"), Some(0x1d));
        assert_eq!(ctrl_chord_to_c0("C-\\"), Some(0x1c));
        assert_eq!(ctrl_chord_to_c0("C-/"), Some(0x1f));
        assert_eq!(ctrl_chord_to_c0("ctrl-2"), Some(0x00));
        assert_eq!(ctrl_chord_to_c0("ctrl-~"), Some(0x1e));
    }
}

#[cfg(all(test, any(target_os = "windows", target_os = "linux")))]
mod vt_tests {
    use super::vt_key_event;
    use con_ghostty::vt::VtKeyAction;
    use gpui::{Keystroke, Modifiers};

    #[test]
    fn space_keeps_text_for_the_ghostty_encoder() {
        let keystroke = Keystroke {
            modifiers: Modifiers::default(),
            key: "space".into(),
            key_char: Some(" ".into()),
        };

        let event = vt_key_event(&keystroke, VtKeyAction::Press);
        assert_eq!(event.key, "space");
        assert_eq!(event.text, " ");
        assert_eq!(event.unshifted_codepoint, Some(' '));

        let shifted_non_ascii = Keystroke {
            modifiers: Modifiers {
                shift: true,
                ..Modifiers::default()
            },
            key: "adiaeresis".into(),
            key_char: Some("Ä".into()),
        };
        let event = vt_key_event(&shifted_non_ascii, VtKeyAction::Press);
        assert_eq!(event.text, "Ä");
        assert_eq!(event.unshifted_codepoint, Some('ä'));
    }
}

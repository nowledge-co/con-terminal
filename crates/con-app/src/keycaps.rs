use gpui::{
    Action, AnyElement, AsKeystroke as _, Div, IntoElement as _, Keystroke, ParentElement as _,
    Styled as _, Window, div, px,
};

fn key_label(key: &str) -> String {
    match key {
        "space" => "Space".to_string(),
        "enter" => enter_label().to_string(),
        "escape" => "Esc".to_string(),
        "backspace" => backspace_label().to_string(),
        "delete" => "Del".to_string(),
        "tab" => "Tab".to_string(),
        "left" => "←".to_string(),
        "right" => "→".to_string(),
        "up" => "↑".to_string(),
        "down" => "↓".to_string(),
        "-" => "-".to_string(),
        "+" => "+".to_string(),
        key if key.chars().count() == 1 => key.to_uppercase(),
        key => key
            .split('-')
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

pub(crate) fn keycap_labels_for_stroke(stroke: &Keystroke) -> Vec<String> {
    let mut parts = Vec::new();
    if stroke.modifiers.control {
        parts.push(control_label().to_string());
    }
    if stroke.modifiers.alt {
        parts.push(alt_label().to_string());
    }
    // GPUI's macOS event layer folds Shift into the key character for some
    // chords (e.g. cmd-shift-X arrives as key "X" with shift unset), and
    // bindings store that literal key so dispatch keeps matching. The keycap
    // row would then read as if no Shift were involved, so restore it for
    // display whenever the key itself carries an uppercase character.
    let has_implicit_shift = !stroke.modifiers.shift
        && stroke.key.chars().count() == 1
        && stroke
            .key
            .chars()
            .next()
            .is_some_and(|first| first.is_uppercase());
    if stroke.modifiers.shift || has_implicit_shift {
        parts.push(shift_label().to_string());
    }
    if stroke.modifiers.platform {
        parts.push(platform_label().to_string());
    }
    parts.push(key_label(&stroke.key));
    parts
}

#[cfg(target_os = "macos")]
fn control_label() -> &'static str {
    "⌃"
}

#[cfg(not(target_os = "macos"))]
fn control_label() -> &'static str {
    "Ctrl"
}

#[cfg(target_os = "macos")]
fn alt_label() -> &'static str {
    "⌥"
}

#[cfg(not(target_os = "macos"))]
fn alt_label() -> &'static str {
    "Alt"
}

#[cfg(target_os = "macos")]
fn shift_label() -> &'static str {
    "⇧"
}

#[cfg(not(target_os = "macos"))]
fn shift_label() -> &'static str {
    "Shift"
}

#[cfg(target_os = "macos")]
fn platform_label() -> &'static str {
    "⌘"
}

#[cfg(not(target_os = "macos"))]
fn platform_label() -> &'static str {
    "Ctrl"
}

#[cfg(target_os = "macos")]
fn enter_label() -> &'static str {
    "Return"
}

#[cfg(not(target_os = "macos"))]
fn enter_label() -> &'static str {
    "Enter"
}

#[cfg(target_os = "macos")]
fn backspace_label() -> &'static str {
    "Delete"
}

#[cfg(not(target_os = "macos"))]
fn backspace_label() -> &'static str {
    "Backspace"
}

fn keycap(label: String, theme: &gpui_component::Theme) -> Div {
    let wide = label.chars().count() > 1;
    div()
        .h(px(20.0))
        .min_w(if wide { px(30.0) } else { px(20.0) })
        .px(px(if wide { 6.0 } else { 0.0 }))
        .rounded(px(5.0))
        .flex()
        .items_center()
        .justify_center()
        .bg(theme.foreground.opacity(0.070))
        .font_family(theme.font_family.clone())
        .text_color(theme.foreground.opacity(0.76))
        .text_size(px(10.5))
        .line_height(px(12.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .child(label)
}

pub(crate) fn keycaps_for_stroke(stroke: &Keystroke, theme: &gpui_component::Theme) -> Div {
    div()
        .flex()
        .items_center()
        .justify_end()
        .gap(px(3.0))
        .children(
            keycap_labels_for_stroke(stroke)
                .into_iter()
                .map(|part| keycap(part, theme)),
        )
}

pub(crate) fn keycaps_for_binding(binding: &str, theme: &gpui_component::Theme) -> AnyElement {
    if let Ok(stroke) = Keystroke::parse(binding) {
        keycaps_for_stroke(&stroke, theme).into_any_element()
    } else {
        div()
            .text_size(px(11.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(theme.muted_foreground)
            .child(binding.to_string())
            .into_any_element()
    }
}

pub(crate) fn first_action_keystroke(action: &dyn Action, window: &Window) -> Option<Keystroke> {
    let binding = window.highest_precedence_binding_for_action(action)?;
    binding
        .keystrokes()
        .first()
        .map(|keystroke| keystroke.as_keystroke().clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uppercase_key_implies_shift_cap() {
        // Bindings recorded on macOS for chords like cmd-shift-X can store the
        // shifted character literally ("cmd-X") because GPUI clears the shift
        // flag for those keys; the cap row must still show Shift.
        let stroke = Keystroke::parse("cmd-X").unwrap();
        assert_eq!(
            keycap_labels_for_stroke(&stroke),
            vec![shift_label().to_string(), platform_label().to_string(), "X".to_string()],
        );
    }

    #[test]
    fn explicit_shift_binding_keeps_single_shift_cap() {
        let stroke = Keystroke::parse("cmd-shift-x").unwrap();
        assert_eq!(
            keycap_labels_for_stroke(&stroke),
            vec![shift_label().to_string(), platform_label().to_string(), "X".to_string()],
        );
    }

    #[test]
    fn lowercase_key_gets_no_phantom_shift() {
        let stroke = Keystroke::parse("cmd-a").unwrap();
        assert_eq!(
            keycap_labels_for_stroke(&stroke),
            vec![platform_label().to_string(), "A".to_string()],
        );
    }

    #[test]
    fn implicit_shift_orders_between_alt_and_platform() {
        let stroke = Keystroke::parse("ctrl-alt-X").unwrap();
        assert_eq!(
            keycap_labels_for_stroke(&stroke),
            vec![
                control_label().to_string(),
                alt_label().to_string(),
                shift_label().to_string(),
                "X".to_string(),
            ],
        );
    }
}

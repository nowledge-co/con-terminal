//! Persistent paste guidance belongs to the Con card, not the helper's TUI scrollback.
use std::io::{Result, Write};

pub(super) fn opening(out: &mut impl Write, label: &str) -> Result<()> {
    writeln!(out, "Opening {label}…")
}

pub(super) fn exited(out: &mut impl Write, label: &str, status: &str) -> Result<()> {
    writeln!(
        out,
        "\n{label} exited ({status}). Review Handoff in Con; Abandon leaves background work running."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn helper_stdout_leaves_clipboard_guidance_to_the_card() {
        for label in ["Kimi", "Codex", "Cursor"] {
            let mut out = Vec::new();
            opening(&mut out, label).unwrap();
            assert_eq!(
                String::from_utf8(out.clone()).unwrap(),
                format!("Opening {label}…\n")
            );
            exited(&mut out, label, "0").unwrap();
            let text = String::from_utf8(out).unwrap();
            assert!(text.contains("Abandon leaves background work running"));
            assert!(
                !text.contains("copied")
                    && !text.contains("clipboard")
                    && !text.contains("release")
            );
        }
    }
}

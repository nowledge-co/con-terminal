//! Rust wrapper around libghostty's embedded C API.
//!
//! Backend selection per target:
//!
//! | target | backend | source |
//! |---|---|---|
//! | macOS | full libghostty (Metal + AppKit NSView) | `terminal.rs` + `ffi.rs` |
//! | Windows | libghostty-vt + ConPTY + D3D11 + DirectWrite, hosted via GPUI image composition | `windows/` |
//! | Linux | local backend scaffold (Unix PTY + future GPUI-owned renderer) | `linux/` |
//! | other | no-op stub (UI compiles, terminal pane shows placeholder) | `stub.rs` |
//!
//! All backends expose the same public type names — `GhosttyApp`,
//! `GhosttyTerminal`, `TerminalColors`, etc. — so cross-platform UI
//! code in `con-app` consumes them without per-callsite cfg gates.

// Suppress warnings from objc 0.2's `sel_impl!` and `class!` macros.
#![allow(unexpected_cfgs)]

use std::time::Duration;

pub fn restored_terminal_output_text(lines: &[String]) -> Option<String> {
    if lines.is_empty() {
        return None;
    }

    let mut output = String::new();
    for line in lines {
        for ch in line.chars() {
            if ch == '\t' || !ch.is_control() {
                output.push(ch);
            }
        }
        output.push_str("\r\n");
    }

    (!output.trim().is_empty()).then_some(output)
}

pub(crate) fn clipboard_mime_is_text(mime: &[u8]) -> bool {
    let media_type = mime
        .iter()
        .position(|byte| *byte == b';')
        .map_or(mime, |separator| &mime[..separator]);
    mime.eq_ignore_ascii_case(b"UTF8_STRING")
        || mime.eq_ignore_ascii_case(b"TEXT")
        || mime.eq_ignore_ascii_case(b"STRING")
        || media_type.eq_ignore_ascii_case(b"text/plain")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalProgress {
    Running(Option<u8>),
    Error(Option<u8>),
    Indeterminate,
    Paused(Option<u8>),
}

pub(crate) const TERMINAL_PROGRESS_TIMEOUT: Duration = Duration::from_secs(15);

impl TerminalProgress {
    pub(crate) fn from_ghostty_report(state: i32, progress: i8) -> Option<Option<Self>> {
        let percent = u8::try_from(progress)
            .ok()
            .filter(|percent| *percent <= 100);
        match state {
            0 => Some(None),
            1 => Some(Some(Self::Running(percent))),
            2 => Some(Some(Self::Error(percent))),
            3 => Some(Some(Self::Indeterminate)),
            4 => Some(Some(Self::Paused(percent))),
            _ => None,
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
mod transcript;

#[cfg(any(target_os = "windows", target_os = "linux", test))]
mod pty_write;

#[cfg(target_os = "macos")]
pub mod ffi;
#[cfg(target_os = "macos")]
pub mod terminal;

// `stub` defines the shared shape (TerminalColors, GhosttySplitDirection,
// MouseButton, etc.) — also used by the Windows backend's facade as the
// concrete type of cross-cutting values. On macOS the stub module isn't
// compiled because all types come from `terminal.rs`.
#[cfg(not(target_os = "macos"))]
pub mod stub;

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(any(target_os = "windows", target_os = "linux"))]
pub mod vt;

#[cfg(target_os = "windows")]
pub mod windows;

// ── Re-exports per platform ────────────────────────────────────────────

#[cfg(target_os = "macos")]
pub use terminal::{
    CommandFinishedSignal, CommandRecord, GhosttyApp, GhosttyConfigPatch, GhosttyScrollbar,
    GhosttySplitDirection, GhosttySurfaceEvent, GhosttyTerminal, MouseButton, SurfaceSize,
    TerminalColors, TerminalState,
};

#[cfg(target_os = "windows")]
pub use stub::{
    CommandFinishedSignal, CommandRecord, GhosttyConfigPatch, GhosttyScrollbar,
    GhosttySplitDirection, GhosttySurfaceEvent, MouseButton, SurfaceSize, TerminalColors,
};
#[cfg(target_os = "windows")]
pub use windows::{WindowsGhosttyApp as GhosttyApp, WindowsGhosttyTerminal as GhosttyTerminal};

#[cfg(target_os = "linux")]
pub use linux::{LinuxGhosttyApp as GhosttyApp, LinuxGhosttyTerminal as GhosttyTerminal};
#[cfg(target_os = "linux")]
pub use stub::{
    CommandFinishedSignal, CommandRecord, GhosttyConfigPatch, GhosttyScrollbar,
    GhosttySplitDirection, GhosttySurfaceEvent, MouseButton, SurfaceSize, TerminalColors,
};
/// Re-exports for the Linux GPUI-owned terminal renderer in
/// `con-app/src/linux_view.rs`. These types are part of the cross-
/// platform `vt` parser surface and are stable enough for the view
/// to consume directly while we iterate on the Linux paint path.
#[cfg(target_os = "linux")]
pub use vt::{
    ATTR_BOLD, ATTR_INVERSE, ATTR_ITALIC, ATTR_STRIKE, ATTR_UNDERLINE, Cell as VtCell,
    Cursor as VtCursor, KittyImage, KittyPlacement, ScreenSnapshot,
};

#[cfg(all(
    not(target_os = "macos"),
    not(target_os = "windows"),
    not(target_os = "linux")
))]
pub use stub::{
    CommandFinishedSignal, CommandRecord, GhosttyApp, GhosttyConfigPatch, GhosttyScrollbar,
    GhosttySplitDirection, GhosttySurfaceEvent, GhosttyTerminal, MouseButton, SurfaceSize,
    TerminalColors,
};

#[cfg(test)]
mod tests {
    use super::clipboard_mime_is_text;

    #[test]
    fn clipboard_text_mime_matching_is_exact_and_case_insensitive() {
        assert!(clipboard_mime_is_text(b"text/plain"));
        assert!(clipboard_mime_is_text(b"TEXT/PLAIN"));
        assert!(clipboard_mime_is_text(b"text/plain;charset=utf-8"));
        assert!(clipboard_mime_is_text(b"UTF8_STRING"));
        assert!(clipboard_mime_is_text(b"text"));
        assert!(clipboard_mime_is_text(b"STRING"));
        assert!(!clipboard_mime_is_text(b"text/plainEVIL"));
        assert!(!clipboard_mime_is_text(b"image/png"));
    }
}

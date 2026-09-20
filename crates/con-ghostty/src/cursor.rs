//! Portable text-cursor presentation. VT supplies policy; the host owns time.

use std::time::{Duration, Instant};

/// Values match GhosttyRenderStateCursorVisualStyle in vt/render.h.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum CursorStyle {
    Bar = 0,
    #[default]
    Block = 1,
    Underline = 2,
    HollowBlock = 3,
}

impl CursorStyle {
    pub(crate) fn from_raw(value: i32) -> Self {
        match value {
            0 => Self::Bar,
            2 => Self::Underline,
            3 => Self::HollowBlock,
            _ => Self::Block,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    pub col: u16,
    pub row: u16,
    pub visible: bool,
    pub style: CursorStyle,
    pub blinking: bool,
}

/// One deadline per visible, focused blinking cursor, never a frame-rate poll.
#[derive(Default)]
pub struct CursorBlink {
    previous: Option<Cursor>,
    deadline: Option<Instant>,
    hidden: bool,
}

impl CursorBlink {
    const INTERVAL: Duration = Duration::from_millis(600);

    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    pub fn reset(&mut self) {
        self.previous = None;
    }

    pub fn update(&mut self, cursor: Cursor, focused: bool, now: Instant) -> Cursor {
        if !focused || !cursor.visible || !cursor.blinking {
            self.deadline = None;
            self.hidden = false;
        } else if self.previous != Some(cursor) || self.deadline.is_none() {
            self.hidden = false;
            self.deadline = Some(now + Self::INTERVAL);
        } else if self.deadline.is_some_and(|deadline| now >= deadline) {
            self.hidden = !self.hidden;
            self.deadline = Some(now + Self::INTERVAL);
        }
        self.previous = Some(cursor);
        Cursor {
            visible: cursor.visible && !self.hidden,
            ..cursor
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blink_deadline_focus_and_policy_transitions() {
        let mut blink = CursorBlink::default();
        let now = Instant::now();
        let at = |millis| now + Duration::from_millis(millis);
        let cursor = Cursor {
            col: 3,
            row: 1,
            visible: true,
            blinking: true,
            style: CursorStyle::Bar,
        };
        assert!(blink.update(cursor, true, now).visible);
        assert!(blink.update(cursor, true, at(599)).visible);
        assert!(!blink.update(cursor, true, at(600)).visible);
        assert!(blink.update(cursor, false, at(601)).visible);
        assert_eq!(blink.deadline(), None);
        assert!(blink.update(cursor, true, at(602)).visible);
        assert_eq!(blink.deadline(), Some(at(1202)));
        assert!(!blink.update(cursor, true, at(1202)).visible);
        blink.reset();
        assert!(blink.update(cursor, true, at(1203)).visible);
        let steady = Cursor {
            blinking: false,
            ..cursor
        };
        assert!(blink.update(steady, true, at(3000)).visible);
        assert_eq!(blink.deadline(), None);
        let hidden = Cursor {
            visible: false,
            ..cursor
        };
        assert!(!blink.update(hidden, true, at(4000)).visible);
        assert_eq!(blink.deadline(), None);
    }
}

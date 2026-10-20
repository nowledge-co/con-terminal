//! Wheel-to-row conversion shared by the Windows and Linux backends.
//!
//! GPUI follows the terminal/editor convention used by Zed: a positive
//! vertical scroll delta means "scroll up". libghostty-vt's viewport API uses
//! the opposite sign: negative rows are up.

/// Pixel remainder from high-resolution wheels and touchpads. Backends only
/// ask libghostty-vt or alternate-screen apps to scroll whole rows, so
/// fractional deltas accumulate here instead of turning every tiny touchpad
/// event into a full-row jump.
#[derive(Debug, Default)]
pub(crate) struct ScrollRemainder {
    px: f32,
    alternate_screen: Option<bool>,
}

impl ScrollRemainder {
    pub(crate) fn reset(&mut self) {
        self.px = 0.0;
        self.alternate_screen = None;
    }

    /// Add `delta_y_px` and return the whole rows it completes, in GPUI's
    /// sign. A remainder measured on the other screen is dropped, because
    /// rows on the primary and alternate screens are unrelated.
    pub(crate) fn rows_for_delta(
        &mut self,
        delta_y_px: f32,
        row_height_px: f32,
        alternate_screen: bool,
    ) -> isize {
        if self.alternate_screen != Some(alternate_screen) {
            self.px = 0.0;
            self.alternate_screen = Some(alternate_screen);
        }

        self.px += delta_y_px;
        let rows = (self.px / row_height_px).trunc() as isize;
        if rows != 0 {
            self.px -= rows as f32 * row_height_px;
        }
        rows
    }
}

/// The cursor key an alternate-screen app expects for `rows` of wheel
/// movement in GPUI's sign, honouring DECCKM (application cursor keys).
pub(crate) fn cursor_key_for_scroll_rows(rows: isize, decckm: bool) -> Option<&'static str> {
    match rows.cmp(&0) {
        std::cmp::Ordering::Greater => Some(if decckm { "\x1bOA" } else { "\x1b[A" }),
        std::cmp::Ordering::Less => Some(if decckm { "\x1bOB" } else { "\x1b[B" }),
        std::cmp::Ordering::Equal => None,
    }
}

/// Convert GPUI wheel rows to libghostty-vt viewport rows.
pub(crate) fn viewport_delta_for_scroll_rows(rows: isize) -> isize {
    -rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alt_scroll_rows_match_cursor_direction() {
        assert_eq!(cursor_key_for_scroll_rows(2, false), Some("\x1b[A"));
        assert_eq!(cursor_key_for_scroll_rows(-2, false), Some("\x1b[B"));
        assert_eq!(cursor_key_for_scroll_rows(2, true), Some("\x1bOA"));
        assert_eq!(cursor_key_for_scroll_rows(-2, true), Some("\x1bOB"));
        assert_eq!(cursor_key_for_scroll_rows(0, false), None);
    }

    #[test]
    fn viewport_scroll_inverts_gpui_rows_for_libghostty_vt() {
        assert_eq!(viewport_delta_for_scroll_rows(3), -3);
        assert_eq!(viewport_delta_for_scroll_rows(-3), 3);
        assert_eq!(viewport_delta_for_scroll_rows(0), 0);
    }

    #[test]
    fn small_deltas_add_up_to_whole_rows() {
        let mut remainder = ScrollRemainder::default();
        let rows: isize = (0..10)
            .map(|_| remainder.rows_for_delta(2.0, 20.0, false))
            .sum();
        assert_eq!(rows, 1, "ten tenth-of-a-row deltas should make one row");

        let rows: isize = (0..4)
            .map(|_| remainder.rows_for_delta(-15.0, 20.0, false))
            .sum();
        assert_eq!(rows, -3, "downward deltas carry their remainder too");
    }

    #[test]
    fn switching_screens_drops_the_remainder() {
        let mut remainder = ScrollRemainder::default();
        assert_eq!(remainder.rows_for_delta(15.0, 20.0, false), 0);
        assert_eq!(
            remainder.rows_for_delta(15.0, 20.0, true),
            0,
            "a primary-screen remainder must not complete an alternate-screen row"
        );
        assert_eq!(remainder.rows_for_delta(5.0, 20.0, true), 1);
    }

    #[test]
    fn reset_drops_the_remainder() {
        let mut remainder = ScrollRemainder::default();
        assert_eq!(remainder.rows_for_delta(15.0, 20.0, false), 0);
        remainder.reset();
        assert_eq!(remainder.rows_for_delta(10.0, 20.0, false), 0);
    }
}

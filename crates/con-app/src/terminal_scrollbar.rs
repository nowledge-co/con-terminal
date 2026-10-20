//! Geometry of the overlay scrollbar drawn by the Windows and Linux terminal
//! views. macOS uses libghostty's own scrollbar.
//!
//! Offsets are in scrollbar rows: `0` is the oldest scrollback row and
//! `total - len` is the live tail.

use con_ghostty::GhosttyScrollbar;

pub(crate) const SCROLLBAR_INSET_PX: f32 = 4.0;
pub(crate) const SCROLLBAR_WIDTH_PX: f32 = 6.0;
pub(crate) const SCROLLBAR_MIN_THUMB_PX: f32 = 28.0;

/// Where the thumb sits, in logical pixels from the top of the pane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ThumbLayout {
    pub track_height_px: f32,
    pub thumb_height_px: f32,
    pub thumb_top_px: f32,
}

/// The thumb for `scrollbar` in a pane `pane_height_px` tall, or `None` when
/// there is no scrollback to show.
pub(crate) fn thumb_layout(
    pane_height_px: f32,
    scrollbar: GhosttyScrollbar,
) -> Option<ThumbLayout> {
    let track_height_px = (pane_height_px - SCROLLBAR_INSET_PX * 2.0).max(0.0);
    if track_height_px <= 0.0 || scrollbar.len == 0 || scrollbar.total <= scrollbar.len {
        return None;
    }
    let thumb_height_px = (scrollbar.len as f32 / scrollbar.total as f32 * track_height_px)
        .clamp(SCROLLBAR_MIN_THUMB_PX.min(track_height_px), track_height_px);
    let travel_px = (track_height_px - thumb_height_px).max(0.0);
    let max_offset = scrollbar.total - scrollbar.len;
    let offset = scrollbar.offset.min(max_offset);
    Some(ThumbLayout {
        track_height_px,
        thumb_height_px,
        thumb_top_px: SCROLLBAR_INSET_PX + offset as f32 / max_offset as f32 * travel_px,
    })
}

/// A thumb drag in progress, remembered from the press.
// The Windows view keeps its own drag and track-click code.
#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Copy, Debug)]
pub(crate) struct ThumbDrag {
    start_y_px: f32,
    start_offset: u64,
    total: u64,
    len: u64,
    travel_px: f32,
}

#[cfg(any(target_os = "linux", test))]
impl ThumbDrag {
    pub(crate) fn start(
        pointer_y_px: f32,
        scrollbar: GhosttyScrollbar,
        layout: ThumbLayout,
    ) -> Self {
        Self {
            start_y_px: pointer_y_px,
            start_offset: scrollbar.offset,
            total: scrollbar.total,
            len: scrollbar.len,
            travel_px: (layout.track_height_px - layout.thumb_height_px).max(1.0),
        }
    }

    /// The offset that keeps the thumb under a pointer now at `pointer_y_px`,
    /// clamped to the oldest row and the live tail.
    pub(crate) fn target_offset(&self, pointer_y_px: f32) -> Option<u64> {
        let max_offset = self.total.saturating_sub(self.len);
        if max_offset == 0 {
            return None;
        }
        let delta_rows = (pointer_y_px - self.start_y_px) / self.travel_px * max_offset as f32;
        Some(
            (self.start_offset as f32 + delta_rows)
                .round()
                .clamp(0.0, max_offset as f32) as u64,
        )
    }
}

/// Rows to scroll for a click on the track at `local_y_px` (from the top of
/// the pane), in libghostty-vt's sign: one page up above the thumb, one page
/// down below it, and none on the thumb itself.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn track_click_rows(
    local_y_px: f32,
    layout: ThumbLayout,
    scrollbar: GhosttyScrollbar,
) -> isize {
    let page = scrollbar.len.max(1).min(isize::MAX as u64) as isize;
    if local_y_px < layout.thumb_top_px {
        -page
    } else if local_y_px > layout.thumb_top_px + layout.thumb_height_px {
        page
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(total: u64, offset: u64, len: u64) -> GhosttyScrollbar {
        GhosttyScrollbar { total, offset, len }
    }

    fn close(actual: f32, expected: f32) -> bool {
        (actual - expected).abs() < 0.01
    }

    #[test]
    fn thumb_at_the_live_tail_sits_at_the_bottom_of_the_track() {
        // 200 rows of scrollback above a 24-row viewport in a 520 px pane.
        let layout = thumb_layout(520.0, bar(224, 200, 24)).unwrap();
        assert!(close(layout.track_height_px, 512.0), "{layout:?}");
        assert!(
            close(layout.thumb_height_px, 24.0 / 224.0 * 512.0),
            "{layout:?}"
        );
        assert!(
            close(
                layout.thumb_top_px + layout.thumb_height_px,
                520.0 - SCROLLBAR_INSET_PX
            ),
            "{layout:?}"
        );

        let top = thumb_layout(520.0, bar(224, 0, 24)).unwrap();
        assert!(close(top.thumb_top_px, SCROLLBAR_INSET_PX), "{top:?}");
    }

    #[test]
    fn long_scrollback_keeps_the_thumb_grabbable() {
        let layout = thumb_layout(520.0, bar(100_000, 0, 24)).unwrap();
        assert!(
            close(layout.thumb_height_px, SCROLLBAR_MIN_THUMB_PX),
            "{layout:?}"
        );
    }

    #[test]
    fn no_thumb_without_scrollback_or_room() {
        assert_eq!(thumb_layout(520.0, bar(24, 0, 24)), None);
        assert_eq!(thumb_layout(520.0, bar(10, 0, 24)), None);
        assert_eq!(thumb_layout(520.0, bar(224, 0, 0)), None);
        assert_eq!(
            thumb_layout(2.0 * SCROLLBAR_INSET_PX, bar(224, 0, 24)),
            None
        );
    }

    #[test]
    fn dragging_the_thumb_moves_through_scrollback_and_clamps() {
        let scrollbar = bar(224, 200, 24);
        let layout = thumb_layout(520.0, scrollbar).unwrap();
        let grab_y = layout.thumb_top_px + layout.thumb_height_px / 2.0;
        let drag = ThumbDrag::start(grab_y, scrollbar, layout);
        let travel = layout.track_height_px - layout.thumb_height_px;

        assert_eq!(drag.target_offset(grab_y), Some(200));
        assert_eq!(drag.target_offset(grab_y - travel / 2.0), Some(100));
        assert_eq!(drag.target_offset(grab_y - travel), Some(0));
        assert_eq!(drag.target_offset(grab_y - 10.0 * travel), Some(0));
        assert_eq!(drag.target_offset(grab_y + 10.0 * travel), Some(200));
    }

    #[test]
    fn clicking_the_track_pages_toward_the_click() {
        let scrollbar = bar(224, 100, 24);
        let layout = thumb_layout(520.0, scrollbar).unwrap();
        let on_thumb = layout.thumb_top_px + layout.thumb_height_px / 2.0;

        assert_eq!(
            track_click_rows(layout.thumb_top_px - 1.0, layout, scrollbar),
            -24
        );
        assert_eq!(track_click_rows(on_thumb, layout, scrollbar), 0);
        assert_eq!(
            track_click_rows(
                layout.thumb_top_px + layout.thumb_height_px + 1.0,
                layout,
                scrollbar
            ),
            24
        );
    }
}

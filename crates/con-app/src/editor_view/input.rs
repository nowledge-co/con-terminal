//! Platform text-input bridge for the virtualized editor. Keystrokes are not text.
use super::*;
use gpui::{EntityInputHandler, UTF16Selection};
use std::ops::Range;

impl EditorView {
    fn accepts_editor_input(&self) -> bool {
        !self.tabs.is_empty() && !self.preview_active() && !self.active_tab_is_image()
    }

    pub(crate) fn is_composing(&self) -> bool {
        self.active_tab_ref()
            .is_some_and(|tab| tab.buffer.input_marked_range().is_some())
    }

    fn input_changed(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.cursor_visible = true;
        self.notify_lsp_active_did_change(cx);
        self.scroll_cursor_into_view();
        if let Some(bounds) = self.content_bounds
            && let Some(tab) = self.active_tab_ref()
            && let Some(line) = self.shape_input_line(tab.buffer.cursor().row, window, cx)
        {
            let cursor_x = ROW_TEXT_LEFT + f32::from(line.x_for_index(tab.buffer.cursor().column));
            let width = self
                .widest_row_min_width()
                .max(ROW_TEXT_LEFT + f32::from(line.width()) + self.metrics.char_width);
            let offset = self.scroll_handle.offset();
            let x = Self::scroll_x_for_cursor(
                cursor_x,
                f32::from(bounds.size.width),
                width,
                f32::from(offset.x),
            );
            self.scroll_handle.set_offset(gpui::point(px(x), offset.y));
        }
        cx.notify();
    }

    pub(super) fn shape_input_line(
        &self,
        row: usize,
        window: &Window,
        cx: &App,
    ) -> Option<gpui::ShapedLine> {
        let line = self.active_tab_ref()?.buffer.lines().get(row)?;
        Some(Self::shape_line(
            line,
            self.metrics,
            cx.theme().mono_font_family.clone(),
            cx.theme().foreground,
            window,
        ))
    }

    pub(super) fn shape_line(
        text: &str,
        metrics: EditorMetrics,
        font: SharedString,
        color: Hsla,
        window: &Window,
    ) -> gpui::ShapedLine {
        window.text_system().shape_line(
            text.to_owned().into(),
            px(metrics.font_size),
            &[gpui::TextRun {
                len: text.len(),
                font: gpui::font(font),
                color,
                ..Default::default()
            }],
            None,
        )
    }
}

impl EntityInputHandler for EditorView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        if !self.accepts_editor_input() {
            return None;
        }
        let (text, actual) = self.active_tab_ref()?.buffer.input_slice(range);
        *adjusted = Some(actual);
        Some(text)
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        if !self.accepts_editor_input() {
            return None;
        }
        let (range, reversed) = self.active_tab_ref()?.buffer.input_selection();
        Some(UTF16Selection { range, reversed })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.accepts_editor_input()
            .then(|| self.active_tab_ref()?.buffer.input_marked_range())
            .flatten()
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.active_tab_mut() {
            tab.buffer.unmark_input();
        }
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.accepts_editor_input() {
            return;
        }
        if let Some(tab) = self.active_tab_mut() {
            tab.buffer.replace_input(range, text, None, false);
        }
        self.input_changed(window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selection: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.accepts_editor_input() {
            return;
        }
        if let Some(tab) = self.active_tab_mut() {
            tab.buffer.replace_input(range, text, selection, true);
        }
        self.input_changed(window, cx);
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        if !self.accepts_editor_input() {
            return None;
        }
        let content = self.content_bounds?;
        let buffer = &self.active_tab_ref()?.buffer;
        let start = buffer.input_position(range.start);
        let end = buffer.input_position(range.end.max(range.start));
        let line = self.shape_input_line(start.row, window, cx)?;
        let x = line.x_for_index(start.column);
        let right = if start.row == end.row {
            line.x_for_index(end.column)
        } else {
            line.width()
        };
        let scroll = self.scroll_handle.offset();
        Some(Bounds::new(
            gpui::point(
                content.left() + scroll.x + px(ROW_TEXT_LEFT) + x,
                content.top() + scroll.y + px(start.row as f32 * self.metrics.line_height),
            ),
            gpui::size((right - x).max(px(1.0)), px(self.metrics.line_height)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        if !self.accepts_editor_input() || !self.point_in_content_bounds(point) {
            return None;
        }
        let position = self.position_for_window_point(point, window, cx);
        Some(self.active_tab_ref()?.buffer.input_offset(position))
    }

    fn set_selected_text_range(
        &mut self,
        range: Range<usize>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.accepts_editor_input() {
            return;
        }
        if let Some(tab) = self.active_tab_mut() {
            tab.buffer.select_input_range(range);
        }
        self.scroll_cursor_into_view();
        cx.notify();
    }

    fn text_length_utf16(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        self.accepts_editor_input()
            .then(|| {
                self.active_tab_ref()
                    .map(|tab| tab.buffer.input_text().encode_utf16().count())
            })
            .flatten()
    }

    fn accepts_text_input(&self, _: &mut Window, _: &mut Context<Self>) -> bool {
        self.accepts_editor_input()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn painted_editor_accepts_native_text_and_reports_composition(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut editor = EditorView::new_with_font_size(EDITOR_FONT_SIZE, cx);
            editor.open_file_from_content_with_activation(
                "ime-fixture.txt".into(),
                "prefix ".into(),
                true,
            );
            editor.tabs[0].buffer.move_end();
            editor.focus_handle.focus(window, cx);
            editor
        });
        cx.refresh().unwrap();
        // Dispatch through GPUI's painted input handler rather than calling
        // insert_text: this fails if the editor forgets Window::handle_input.
        cx.simulate_input("é");
        view.read_with(cx, |view, _| {
            assert_eq!(view.tabs[0].buffer.text(), "prefix é")
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.replace_and_mark_text_in_range(None, "ni", Some(2..2), window, cx);
                view.replace_and_mark_text_in_range(None, "你好😀", Some(2..4), window, cx);
                assert_eq!(view.marked_text_range(window, cx), Some(8..12));
                assert_eq!(
                    view.selected_text_range(false, window, cx).unwrap().range,
                    10..12
                );
                let mut adjusted = None;
                assert_eq!(
                    view.text_for_range(11..12, &mut adjusted, window, cx)
                        .as_deref(),
                    Some("😀")
                );
                assert_eq!(adjusted, Some(10..12));
                view.replace_text_in_range(None, "你好😀", window, cx);
                assert_eq!(view.tabs[0].buffer.text(), "prefix é你好😀");
                assert_eq!(view.marked_text_range(window, cx), None);
                assert!(view.undo(cx));
                assert_eq!(view.tabs[0].buffer.text(), "prefix é");
            })
        });
    }

    #[gpui::test]
    fn composition_does_not_survive_editor_focus_loss(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut editor = EditorView::new_with_font_size(EDITOR_FONT_SIZE, cx);
            editor.open_file_from_content_with_activation(
                "ime-fixture.txt".into(),
                String::new(),
                true,
            );
            editor.focus_handle.focus(window, cx);
            editor
        });
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        cx.refresh().unwrap();
        cx.update(|window, cx| {
            assert!(window.is_window_active());
            assert!(view.read(cx).focus_handle.is_focused(window));
            window.draw(cx).clear(cx);
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.replace_and_mark_text_in_range(None, "日本", Some(2..2), window, cx)
            });
            window.blur(cx);
            window.draw(cx).clear(cx);
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert_eq!(view.tabs[0].buffer.text(), "日本");
            assert_eq!(view.tabs[0].buffer.input_marked_range(), None);
        });
    }

    #[gpui::test]
    fn native_queries_follow_scroll_and_preview_rejects_edits(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut editor = EditorView::new_with_font_size(EDITOR_FONT_SIZE, cx);
            editor.open_file_from_content_with_activation(
                "ime-fixture.md".into(),
                "日本語😀\nnext".into(),
                true,
            );
            editor.focus_handle.focus(window, cx);
            editor
        });
        cx.refresh().unwrap();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let bounds = view
                    .bounds_for_range(5..5, Bounds::default(), window, cx)
                    .unwrap();
                let origin = view
                    .bounds_for_range(0..0, Bounds::default(), window, cx)
                    .unwrap();
                assert!(bounds.left() > origin.left() + px(20.0));
                let second = view
                    .bounds_for_range(6..6, Bounds::default(), window, cx)
                    .unwrap();
                assert_eq!(second.top() - origin.top(), px(view.metrics.line_height));
                let index = view.character_index_for_point(
                    gpui::point(bounds.left(), bounds.top() + px(2.0)),
                    window,
                    cx,
                );
                assert_eq!(index, Some(5));
                let original = view.tabs[0].buffer.text();
                view.replace_text_in_range(Some(0..usize::MAX), &"中".repeat(120), window, cx);
                let candidate = view
                    .bounds_for_range(120..120, Bounds::default(), window, cx)
                    .unwrap();
                let content = view.content_bounds.unwrap();
                assert!(
                    candidate.left() >= content.left() && candidate.right() <= content.right(),
                    "CJK candidate anchor escaped horizontal viewport: {candidate:?}, {content:?}"
                );
                assert!(view.undo(cx));
                assert_eq!(view.tabs[0].buffer.text(), original);
                view.tabs[0].preview = true;
                assert!(!view.accepts_text_input(window, cx));
                view.replace_and_mark_text_in_range(None, "wrong", None, window, cx);
                view.replace_text_in_range(None, "wrong", window, cx);
                assert_eq!(view.tabs[0].buffer.text(), "日本語😀\nnext");
                assert!(view.selected_text_range(false, window, cx).is_none());
            })
        });
    }
}

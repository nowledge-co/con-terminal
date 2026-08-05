use super::*;

#[derive(Clone, Copy)]
enum EditorMotion {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    LineStart,
    LineEnd,
}

fn editor_line_boundary_motion_for_key(event: &KeyDownEvent) -> Option<EditorMotion> {
    let mods = &event.keystroke.modifiers;
    match editor_line_boundary_for_key(
        event.keystroke.key.as_str(),
        mods.control,
        mods.platform,
        mods.alt,
        mods.shift,
    )? {
        EditorLineBoundary::Start => Some(EditorMotion::LineStart),
        EditorLineBoundary::End => Some(EditorMotion::LineEnd),
    }
}

fn editor_text_for_key(key: &str, key_char: Option<&str>) -> Option<String> {
    if let Some(key_char) = key_char.filter(|text| is_printable_key_char(Some(*text))) {
        return Some(key_char.to_string());
    }

    match key {
        "space" => Some(" ".to_string()),
        "tab" => Some("    ".to_string()),
        _ => {
            let mut chars = key.chars();
            let ch = chars.next()?;
            if chars.next().is_some() || ch.is_control() {
                return None;
            }
            Some(key.to_string())
        }
    }
}

fn is_printable_key_char(key_char: Option<&str>) -> bool {
    key_char.is_some_and(|text| !text.is_empty() && text.chars().all(|ch| !ch.is_control()))
}

fn editor_text_for_key_event(
    key: &str,
    key_char: Option<&str>,
    platform: bool,
    alt: bool,
    control: bool,
) -> Option<String> {
    if platform || ((alt || control) && !is_printable_key_char(key_char)) {
        return None;
    }
    editor_text_for_key(key, key_char)
}

impl ConWorkspace {
    fn with_focused_editor_view<R>(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut EditorView, &mut Context<EditorView>) -> R,
    ) -> Option<R> {
        let editor_view = self.focused_editor_view(window, cx)?;
        Some(editor_view.update(cx, f))
    }

    fn focused_editor_view(&self, window: &Window, cx: &App) -> Option<Entity<EditorView>> {
        if !self.has_active_tab() {
            return None;
        }

        let pane_tree = &self.tabs[self.active_tab].pane_tree;
        let pane_id = pane_tree.active_editor_pane_id(window, cx)?;
        pane_tree.editor_view_for_pane(pane_id)
    }

    pub(crate) fn editor_has_keyboard_focus(&self, window: &Window, cx: &App) -> bool {
        self.focused_editor_view(window, cx).is_some()
    }

    fn focused_pane_editor_view(&self, _cx: &App) -> Option<Entity<EditorView>> {
        if !self.has_active_tab() {
            return None;
        }

        let pane_tree = &self.tabs[self.active_tab].pane_tree;
        pane_tree.editor_view_for_pane(pane_tree.focused_pane_id())
    }

    fn notify_editor_action(cx: &mut Context<Self>) {
        cx.notify();
    }

    pub(crate) fn handle_editor_text_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let mods = &event.keystroke.modifiers;
        let Some(text) = editor_text_for_key_event(
            event.keystroke.key.as_str(),
            event.keystroke.key_char.as_deref(),
            mods.platform,
            mods.alt,
            mods.control,
        ) else {
            return false;
        };

        if self
            .with_focused_editor_view(window, cx, |editor, cx| editor.insert_text(&text, cx))
            .is_some()
        {
            window.prevent_default();
            cx.stop_propagation();
            cx.notify();
            true
        } else {
            false
        }
    }

    pub(crate) fn handle_focused_editor_pane_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(motion) = editor_line_boundary_motion_for_key(event) else {
            return false;
        };
        let Some(editor_view) = self.focused_pane_editor_view(cx) else {
            return false;
        };

        editor_view.update(cx, |editor, _cx| match motion {
            EditorMotion::LineStart => editor.move_line_start(),
            EditorMotion::LineEnd => editor.move_line_end(),
            _ => {}
        });
        let focus_handle = editor_view.read(cx).focus_handle(cx).clone();
        focus_handle.focus(window, cx);
        window.prevent_default();
        cx.stop_propagation();
        cx.notify();
        true
    }

    fn apply_editor_motion(
        &mut self,
        motion: EditorMotion,
        selecting: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .with_focused_editor_view(window, cx, |editor, _cx| match (motion, selecting) {
                (EditorMotion::Left, false) => editor.move_left(),
                (EditorMotion::Right, false) => editor.move_right(),
                (EditorMotion::Up, false) => editor.move_up(),
                (EditorMotion::Down, false) => editor.move_down(),
                (EditorMotion::Home, false) => editor.move_home(),
                (EditorMotion::End, false) => editor.move_end(),
                (EditorMotion::LineStart, false) => editor.move_line_start(),
                (EditorMotion::LineEnd, false) => editor.move_line_end(),
                (EditorMotion::Left, true) => editor.select_left(),
                (EditorMotion::Right, true) => editor.select_right(),
                (EditorMotion::Up, true) => editor.select_up(),
                (EditorMotion::Down, true) => editor.select_down(),
                (EditorMotion::Home | EditorMotion::LineStart, true) => editor.select_home(),
                (EditorMotion::End | EditorMotion::LineEnd, true) => editor.select_end(),
            })
            .is_some()
        {
            Self::notify_editor_action(cx);
        }
    }

    pub(crate) fn editor_move_left(
        &mut self,
        _action: &EditorMoveLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_editor_motion(EditorMotion::Left, false, window, cx);
    }

    pub(crate) fn editor_move_right(
        &mut self,
        _action: &EditorMoveRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_editor_motion(EditorMotion::Right, false, window, cx);
    }

    pub(crate) fn editor_move_up(
        &mut self,
        _action: &EditorMoveUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_editor_motion(EditorMotion::Up, false, window, cx);
    }

    pub(crate) fn editor_move_down(
        &mut self,
        _action: &EditorMoveDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_editor_motion(EditorMotion::Down, false, window, cx);
    }

    pub(crate) fn editor_move_home(
        &mut self,
        _action: &EditorMoveHome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_editor_motion(EditorMotion::Home, false, window, cx);
    }

    pub(crate) fn editor_move_end(
        &mut self,
        _action: &EditorMoveEnd,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_editor_motion(EditorMotion::End, false, window, cx);
    }

    pub(crate) fn editor_move_line_start(
        &mut self,
        _action: &EditorMoveLineStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_editor_motion(EditorMotion::LineStart, false, window, cx);
    }

    pub(crate) fn editor_move_line_end(
        &mut self,
        _action: &EditorMoveLineEnd,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_editor_motion(EditorMotion::LineEnd, false, window, cx);
    }

    pub(crate) fn editor_select_left(
        &mut self,
        _action: &EditorSelectLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_editor_motion(EditorMotion::Left, true, window, cx);
    }

    pub(crate) fn editor_select_right(
        &mut self,
        _action: &EditorSelectRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_editor_motion(EditorMotion::Right, true, window, cx);
    }

    pub(crate) fn editor_select_up(
        &mut self,
        _action: &EditorSelectUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_editor_motion(EditorMotion::Up, true, window, cx);
    }

    pub(crate) fn editor_select_down(
        &mut self,
        _action: &EditorSelectDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_editor_motion(EditorMotion::Down, true, window, cx);
    }

    pub(crate) fn editor_select_home(
        &mut self,
        _action: &EditorSelectHome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_editor_motion(EditorMotion::Home, true, window, cx);
    }

    pub(crate) fn editor_select_end(
        &mut self,
        _action: &EditorSelectEnd,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_editor_motion(EditorMotion::End, true, window, cx);
    }

    pub(crate) fn editor_delete_backward(
        &mut self,
        _action: &EditorDeleteBackward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .with_focused_editor_view(window, cx, |editor, cx| editor.delete_backward(cx))
            .is_some()
        {
            Self::notify_editor_action(cx);
        }
    }

    pub(crate) fn editor_delete_forward(
        &mut self,
        _action: &EditorDeleteForward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .with_focused_editor_view(window, cx, |editor, cx| editor.delete_forward(cx))
            .is_some()
        {
            Self::notify_editor_action(cx);
        }
    }

    pub(crate) fn editor_insert_newline(
        &mut self,
        _action: &EditorInsertNewline,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .with_focused_editor_view(window, cx, |editor, cx| editor.insert_newline(cx))
            .is_some()
        {
            Self::notify_editor_action(cx);
        }
    }

    pub(crate) fn editor_save(
        &mut self,
        _action: &EditorSave,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .with_focused_editor_view(window, cx, |editor, _cx| match editor.save_active() {
                Ok(Some(_)) => {}
                Ok(None) => {}
                Err(err) => log::error!("failed to save editor file: {err}"),
            })
            .is_some()
        {
            Self::notify_editor_action(cx);
        }
    }

    pub(crate) fn editor_toggle_preview(
        &mut self,
        _action: &EditorTogglePreview,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .with_focused_editor_view(window, cx, |editor, cx| editor.toggle_preview(cx))
            .is_some()
        {
            Self::notify_editor_action(cx);
        }
    }

    pub(crate) fn editor_undo(
        &mut self,
        _action: &Undo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let undo_result = self.with_focused_editor_view(window, cx, |editor, cx| editor.undo(cx));
        if let Some(undid) = undo_result {
            window.prevent_default();
            cx.stop_propagation();
            if undid {
                Self::notify_editor_action(cx);
            }
        }
    }

    pub(crate) fn editor_copy(
        &mut self,
        _action: &Copy,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self.with_focused_editor_view(window, cx, |editor, _cx| editor.selected_text());
        if let Some(Some(text)) = text {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
            window.prevent_default();
            cx.stop_propagation();
        }
    }

    pub(crate) fn editor_cut(
        &mut self,
        _action: &Cut,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self.with_focused_editor_view(window, cx, |editor, cx| editor.cut_selection(cx));
        if let Some(Some(text)) = text {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
            window.prevent_default();
            cx.stop_propagation();
            Self::notify_editor_action(cx);
        }
    }

    pub(crate) fn editor_paste(
        &mut self,
        _action: &Paste,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(item) = cx.read_from_clipboard() {
            if let Some(text) = item.text() {
                if self
                    .with_focused_editor_view(window, cx, |editor, cx| {
                        editor.insert_text(&text, cx)
                    })
                    .is_some()
                {
                    window.prevent_default();
                    cx.stop_propagation();
                    Self::notify_editor_action(cx);
                }
            }
        }
    }

    pub(crate) fn editor_select_all(
        &mut self,
        _action: &SelectAll,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .with_focused_editor_view(window, cx, |editor, _cx| editor.select_all())
            .is_some()
        {
            window.prevent_default();
            cx.stop_propagation();
            Self::notify_editor_action(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn editor_text_prefers_produced_key_char() {
        assert_eq!(
            super::editor_text_for_key("1", Some("!")).as_deref(),
            Some("!")
        );
        assert_eq!(
            super::editor_text_for_key("a", Some("A")).as_deref(),
            Some("A")
        );
    }

    #[test]
    fn editor_text_keeps_named_key_fallbacks() {
        assert_eq!(
            super::editor_text_for_key("space", None).as_deref(),
            Some(" ")
        );
        assert_eq!(
            super::editor_text_for_key("tab", Some("\t")).as_deref(),
            Some("    ")
        );
        assert_eq!(super::editor_text_for_key("enter", Some("\n")), None);
    }

    #[test]
    fn editor_text_allows_printable_altgr_or_option_key_char() {
        assert_eq!(
            super::editor_text_for_key_event("2", Some("@"), false, true, true).as_deref(),
            Some("@")
        );
        assert_eq!(
            super::editor_text_for_key_event("e", Some("é"), false, true, false).as_deref(),
            Some("é")
        );
    }

    #[test]
    fn editor_text_with_modifiers_does_not_fallback_to_physical_key() {
        assert_eq!(
            super::editor_text_for_key_event("a", None, false, false, true),
            None
        );
        assert_eq!(
            super::editor_text_for_key_event("2", None, false, true, true),
            None
        );
        assert_eq!(
            super::editor_text_for_key_event("a", Some("a"), true, false, false),
            None
        );
    }
}

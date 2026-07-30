//! Editor view — lightweight multi-file editor pane.
//!
//! Uses GPUI's `uniform_list` for virtualized rendering — only visible rows
//! are laid out each frame, so large files are fast.

use crate::{
    chat_markdown::ParsedChatMarkdown,
    editor_buffer::{CursorPosition, EditorBuffer},
    editor_lsp::{self, EditorDiagnostic, LspClient, LspClientEvent},
    editor_preview, editor_syntax,
};
use crossbeam_channel::{Receiver, Sender};
use gpui::{
    App, Bounds, Context, CursorStyle, EventEmitter, FocusHandle, Focusable, FontWeight, Hsla,
    InteractiveElement, IntoElement, ListHorizontalSizingBehavior, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Render, ScrollHandle,
    ScrollStrategy, SharedString, Styled, StyledText, Task, UniformListScrollHandle, Window, div,
    px, svg, uniform_list,
};
use gpui_component::{
    ActiveTheme, Icon, Sizable, Theme,
    button::{Button, ButtonVariants as _},
    scroll::{ScrollableElement, Scrollbar, ScrollbarHandle, ScrollbarShow},
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

/// Unique-per-process id for `EditorView` instances, used to namespace GPUI
/// element ids (multiple editor panes may show previews simultaneously).
static NEXT_EDITOR_VIEW_ID: AtomicU64 = AtomicU64::new(1);

const EDITOR_FONT_SIZE: f32 = 14.0;
#[cfg_attr(not(test), allow(dead_code))]
const LINE_HEIGHT: f32 = EDITOR_FONT_SIZE * 1.5;
const GUTTER_WIDTH: f32 = 44.0;
const TAB_BAR_HEIGHT: f32 = 28.0;
const TEXT_IN_CONTENT_LEFT: f32 = 12.0;
const ROW_TEXT_LEFT: f32 = GUTTER_WIDTH + TEXT_IN_CONTENT_LEFT;
#[cfg_attr(not(test), allow(dead_code))]
const CHAR_WIDTH: f32 = editor_char_width(EDITOR_FONT_SIZE);
const SCROLLBAR_HITBOX_SIZE: f32 = 16.0;
const CURSOR_SCROLL_PADDING: f32 = 32.0;
const LSP_DID_CHANGE_DEBOUNCE_MS: u64 = 150;
const PREVIEW_PARSE_DEBOUNCE_MS: u64 = 300;

const fn editor_char_width(font_size: f32) -> f32 {
    // GPUI's text rendering does the actual shaping, but this lightweight editor
    // uses a virtualized row list and draws cursor/selection overlays with fixed
    // pixel offsets. Berkeley/Ioskeley-style mono fonts render close to a 3/5-em
    // cell, so keep hit-testing and overlays on that same grid.
    font_size * 0.6
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct EditorMetrics {
    font_size: f32,
    line_height: f32,
    char_width: f32,
}

impl EditorMetrics {
    fn from_terminal_font_size(font_size: f32) -> Self {
        let font_size = if font_size.is_finite() && font_size > 0.0 {
            font_size
        } else {
            EDITOR_FONT_SIZE
        };
        Self {
            font_size,
            line_height: font_size * 1.5,
            char_width: editor_char_width(font_size),
        }
    }
}

/// Emitted when the active file tab changes so the workspace can sync
/// file-tree root/highlight to the editor file's parent directory.
pub struct ActiveFileChanged;

pub struct EditorEmptied;

impl EventEmitter<ActiveFileChanged> for EditorView {}
impl EventEmitter<EditorEmptied> for EditorView {}

#[derive(Clone)]
pub struct EditorTab {
    pub path: PathBuf,
    buffer: EditorBuffer,
    render_cache: EditorRenderCache,
    save_enabled: bool,
    preview: bool,
    preview_cache: Option<(u64, Arc<ParsedChatMarkdown>)>,
}

#[derive(Clone, Default)]
struct EditorRenderCache {
    key: Option<EditorRenderCacheKey>,
    lines: Arc<Vec<String>>,
    syntax_runs: Arc<Vec<Vec<gpui::TextRun>>>,
    widest_line_index: usize,
}

#[derive(Clone, PartialEq, Eq)]
struct EditorRenderCacheKey {
    revision: u64,
    language: Option<&'static str>,
    theme_name: String,
    is_dark: bool,
    highlight_theme_ptr: usize,
    mono_font_family: String,
    font_size_bits: u32,
}

#[derive(Clone)]
struct EditorRenderSnapshot {
    path: PathBuf,
    lines: Arc<Vec<String>>,
    syntax_runs: Arc<Vec<Vec<gpui::TextRun>>>,
    widest_line_index: usize,
    line_count: usize,
    cursor: CursorPosition,
    selection: Option<(CursorPosition, CursorPosition)>,
}

impl EditorTab {
    fn new(path: PathBuf, buffer: EditorBuffer) -> Self {
        Self {
            path,
            buffer,
            render_cache: EditorRenderCache::default(),
            save_enabled: true,
            preview: false,
            preview_cache: None,
        }
    }

    fn read_error(path: PathBuf, error: std::io::Error) -> Self {
        Self {
            path,
            buffer: EditorBuffer::from_text(format!("Error reading file: {error}")),
            render_cache: EditorRenderCache::default(),
            save_enabled: false,
            preview: false,
            preview_cache: None,
        }
    }

    fn render_snapshot(
        &mut self,
        theme: &Theme,
        mono_font: impl Into<SharedString>,
        metrics: EditorMetrics,
    ) -> EditorRenderSnapshot {
        let mono_font = mono_font.into();
        let language = editor_syntax::language_for_path(&self.path);
        let key = EditorRenderCacheKey {
            revision: self.buffer.revision(),
            language,
            theme_name: theme.theme_name().to_string(),
            is_dark: theme.is_dark(),
            highlight_theme_ptr: Arc::as_ptr(&theme.highlight_theme) as usize,
            mono_font_family: mono_font.to_string(),
            font_size_bits: metrics.font_size.to_bits(),
        };

        if self.render_cache.key.as_ref() != Some(&key) {
            let lines = Arc::new(self.buffer.lines().to_vec());
            let text = self.buffer.text();
            let syntax_runs = Arc::new(editor_syntax::highlighted_line_runs(
                &text,
                &lines,
                language,
                theme,
                mono_font.clone(),
                px(metrics.font_size),
                px(metrics.line_height),
            ));
            let widest_line_index = lines
                .iter()
                .enumerate()
                .max_by_key(|(_, line)| line.chars().count())
                .map(|(index, _)| index)
                .unwrap_or(0);

            self.render_cache = EditorRenderCache {
                key: Some(key),
                lines,
                syntax_runs,
                widest_line_index,
            };
        }

        let lines = self.render_cache.lines.clone();
        EditorRenderSnapshot {
            path: self.path.clone(),
            line_count: lines.len().max(1),
            lines,
            syntax_runs: self.render_cache.syntax_runs.clone(),
            widest_line_index: self.render_cache.widest_line_index,
            cursor: self.buffer.cursor(),
            selection: self.buffer.normalized_selection(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct LoadedFileApply {
    activated: bool,
    active_file_changed: bool,
}

pub struct EditorView {
    tabs: Vec<EditorTab>,
    active_tab: usize,
    open_generation: u64,
    scroll_handle: UniformListScrollHandle,
    cursor_visible: bool,
    cursor_blink: Option<Task<()>>,
    selection_anchor: Option<CursorPosition>,
    content_bounds: Option<Bounds<Pixels>>,
    focus_handle: FocusHandle,
    metrics: EditorMetrics,
    lsp_clients: HashMap<PathBuf, LspClient>,
    lsp_diagnostics: HashMap<PathBuf, Vec<EditorDiagnostic>>,
    lsp_event_tx: Sender<LspClientEvent>,
    lsp_event_rx: Receiver<LspClientEvent>,
    lsp_event_pump: Option<Task<()>>,
    lsp_change_generations: HashMap<PathBuf, u64>,
    lsp_change_debounce_tasks: HashMap<PathBuf, Task<()>>,
    dirty_close_blocked_tab: Option<usize>,
    view_id: u64,
    preview_scroll_handle: ScrollHandle,
    preview_parse_generation: u64,
    preview_parse_task: Option<Task<()>>,
    preview_parse_pending: Option<(PathBuf, u64)>,
}

impl EditorView {
    pub fn new_with_font_size(font_size: f32, cx: &mut Context<Self>) -> Self {
        let (lsp_event_tx, lsp_event_rx) = crossbeam_channel::unbounded();
        Self {
            tabs: Vec::new(),
            active_tab: 0,
            open_generation: 0,
            scroll_handle: UniformListScrollHandle::new(),
            cursor_visible: true,
            cursor_blink: None,
            selection_anchor: None,
            content_bounds: None,
            focus_handle: cx.focus_handle(),
            metrics: EditorMetrics::from_terminal_font_size(font_size),
            lsp_clients: HashMap::new(),
            lsp_diagnostics: HashMap::new(),
            lsp_event_tx,
            lsp_event_rx,
            lsp_event_pump: None,
            lsp_change_generations: HashMap::new(),
            lsp_change_debounce_tasks: HashMap::new(),
            dirty_close_blocked_tab: None,
            view_id: NEXT_EDITOR_VIEW_ID.fetch_add(1, Ordering::Relaxed),
            preview_scroll_handle: ScrollHandle::new(),
            preview_parse_generation: 0,
            preview_parse_task: None,
            preview_parse_pending: None,
        }
    }

    pub fn set_font_size(&mut self, font_size: f32, cx: &mut Context<Self>) {
        let metrics = EditorMetrics::from_terminal_font_size(font_size);
        if self.metrics == metrics {
            return;
        }
        self.metrics = metrics;
        cx.notify();
    }

    /// Load a file from disk into the editor pane. If the file is already open,
    /// it becomes the active editor tab; otherwise a new editor tab is appended.
    pub fn open_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.open_generation = self.open_generation.wrapping_add(1);
        let generation = self.open_generation;
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) {
            self.active_tab = index;
            self.dirty_close_blocked_tab = None;
            self.scroll_handle = UniformListScrollHandle::new();
            cx.emit(ActiveFileChanged);
            cx.notify();
            return;
        }

        cx.spawn(async move |this, cx| {
            let path_for_read = path.clone();
            let content = cx
                .background_executor()
                .spawn(async move { std::fs::read_to_string(&path_for_read) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let activate = this.open_generation == generation;
                match content {
                    Ok(content) => {
                        let apply = this.open_file_from_content_with_activation(
                            path.clone(),
                            content,
                            activate,
                        );
                        this.ensure_lsp_for_path(&path);
                        if apply.active_file_changed {
                            cx.emit(ActiveFileChanged);
                        }
                    }
                    Err(error) => {
                        let apply = this.open_file_read_error_with_activation(
                            path.clone(),
                            error,
                            activate,
                        );
                        if apply.active_file_changed {
                            cx.emit(ActiveFileChanged);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open_file_from_content_with_activation(
        &mut self,
        path: PathBuf,
        content: String,
        activate: bool,
    ) -> LoadedFileApply {
        let apply = Self::apply_loaded_file_tab(
            &mut self.tabs,
            &mut self.active_tab,
            path,
            activate,
            false,
            |path| EditorTab::new(path, EditorBuffer::from_text(content)),
        );
        if apply.activated {
            self.dirty_close_blocked_tab = None;
            self.scroll_handle = UniformListScrollHandle::new();
        }
        apply
    }

    fn open_file_read_error_with_activation(
        &mut self,
        path: PathBuf,
        error: std::io::Error,
        activate: bool,
    ) -> LoadedFileApply {
        let apply = Self::apply_loaded_file_tab(
            &mut self.tabs,
            &mut self.active_tab,
            path,
            activate,
            activate,
            |path| EditorTab::read_error(path, error),
        );
        if apply.activated {
            self.dirty_close_blocked_tab = None;
            self.scroll_handle = UniformListScrollHandle::new();
        }
        apply
    }

    fn apply_loaded_file_tab(
        tabs: &mut Vec<EditorTab>,
        active_tab: &mut usize,
        path: PathBuf,
        activate: bool,
        replace_existing: bool,
        make_tab: impl FnOnce(PathBuf) -> EditorTab,
    ) -> LoadedFileApply {
        let old_active_path = tabs.get(*active_tab).map(|tab| tab.path.clone());
        let mut activated = false;

        if let Some(index) = tabs.iter().position(|tab| tab.path == path) {
            if replace_existing {
                tabs[index] = make_tab(path);
            }
            if activate {
                *active_tab = index;
                activated = true;
            }
        } else {
            let index = tabs.len();
            tabs.push(make_tab(path));
            if activate || old_active_path.is_none() {
                *active_tab = index;
                activated = true;
            }
        }

        LoadedFileApply {
            activated,
            active_file_changed: old_active_path
                != tabs.get(*active_tab).map(|tab| tab.path.clone()),
        }
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn active_path(&self) -> Option<&Path> {
        self.active_tab_ref().map(|tab| tab.path.as_path())
    }

    pub fn activate_tab(&mut self, index: usize) {
        if index < self.tabs.len() && self.active_tab != index {
            self.open_generation = self.open_generation.wrapping_add(1);
            self.active_tab = index;
            self.dirty_close_blocked_tab = None;
            self.scroll_handle = UniformListScrollHandle::new();
            self.preview_scroll_handle = ScrollHandle::new();
            if let Some(path) = self.active_path().map(Path::to_path_buf) {
                self.ensure_lsp_for_path(&path);
            }
        }
    }

    fn preview_active(&self) -> bool {
        self.tabs.get(self.active_tab).is_some_and(|tab| tab.preview)
    }

    fn preview_toggle_target(path: &Path, preview: bool) -> Option<bool> {
        (editor_syntax::language_for_path(path) == Some("markdown")).then_some(!preview)
    }

    /// Toggle the rendered markdown preview for the active tab. Only markdown
    /// files can be previewed; other languages ignore the toggle.
    pub fn toggle_preview(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        let Some(preview) = Self::preview_toggle_target(&tab.path, tab.preview) else {
            return;
        };
        let tab = &mut self.tabs[self.active_tab];
        tab.preview = preview;
        let preview = tab.preview;
        self.preview_scroll_handle = ScrollHandle::new();
        if preview {
            self.schedule_preview_parse(self.active_tab, cx);
        }
        cx.notify();
    }

    /// Reparse the tab's markdown after a debounce, unless the cached parse
    /// already matches the buffer revision. Results are keyed by tab path (not
    /// index, which shifts on close), and an in-flight parse for the same
    /// tab+revision is not restarted by unrelated renders.
    fn schedule_preview_parse(&mut self, tab_index: usize, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(tab_index) else {
            return;
        };
        let revision = tab.buffer.revision();
        let cache_current = tab
            .preview_cache
            .as_ref()
            .is_some_and(|(cached, _)| *cached == revision);
        if cache_current {
            return;
        }
        let path = tab.path.clone();
        if self
            .preview_parse_pending
            .as_ref()
            .is_some_and(|(pending_path, pending_revision)| {
                *pending_path == path && *pending_revision == revision
            })
        {
            return;
        }
        let text = tab.buffer.text();
        self.preview_parse_generation = self.preview_parse_generation.wrapping_add(1);
        let generation = self.preview_parse_generation;
        self.preview_parse_pending = Some((path.clone(), revision));
        self.preview_parse_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(PREVIEW_PARSE_DEBOUNCE_MS))
                .await;
            // ParsedChatMarkdown is Send but not Sync (RefCell render caches),
            // so parse on the background executor and wrap in Arc back on the
            // main thread.
            let parsed = cx
                .background_executor()
                .spawn(async move { ParsedChatMarkdown::parse(&text) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.preview_parse_generation != generation {
                    return;
                }
                this.preview_parse_pending = None;
                if let Some(tab) = this.tabs.iter_mut().find(|tab| tab.path == path)
                    && tab.buffer.revision() == revision
                {
                    tab.preview_cache = Some((revision, Arc::new(parsed)));
                }
                cx.notify();
            });
        }));
    }

    fn activate_tab_and_emit(&mut self, index: usize, cx: &mut Context<Self>) {
        self.activate_tab(index);
        if self.active_path().is_some() {
            cx.emit(ActiveFileChanged);
        }
        cx.notify();
    }

    /// Close the active file tab. Returns true when the editor pane should be
    /// closed because there are no file tabs left.
    pub fn close_active_tab(&mut self) -> bool {
        if self.tabs.is_empty() {
            return true;
        }
        if self.tabs[self.active_tab].buffer.is_dirty() {
            self.dirty_close_blocked_tab = Some(self.active_tab);
            return false;
        }
        self.dirty_close_blocked_tab = None;
        self.open_generation = self.open_generation.wrapping_add(1);
        let closed_path = self.tabs[self.active_tab].path.clone();
        self.tabs.remove(self.active_tab);
        self.lsp_clients.remove(&closed_path);
        self.lsp_diagnostics.remove(&closed_path);
        self.lsp_change_generations.remove(&closed_path);
        self.lsp_change_debounce_tasks.remove(&closed_path);
        if self.tabs.is_empty() {
            self.active_tab = 0;
            self.scroll_handle = UniformListScrollHandle::new();
            return true;
        }
        self.active_tab = self.active_tab.min(self.tabs.len() - 1);
        self.scroll_handle = UniformListScrollHandle::new();
        false
    }

    fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        self.active_tab = index;
        let emptied = self.close_active_tab();
        if self.active_path().is_some() {
            cx.emit(ActiveFileChanged);
        } else if emptied {
            cx.emit(EditorEmptied);
        }
        cx.notify();
    }

    #[cfg(test)]
    fn position_for_content_point_for_test(point: gpui::Point<gpui::Pixels>) -> CursorPosition {
        Self::position_for_content_point_with_metrics(
            point,
            gpui::point(px(0.0), px(0.0)),
            usize::MAX,
            EditorMetrics::from_terminal_font_size(EDITOR_FONT_SIZE),
        )
    }

    #[cfg(test)]
    fn position_for_content_point_in_line_for_test(
        point: gpui::Point<gpui::Pixels>,
        line: &str,
    ) -> CursorPosition {
        let metrics = EditorMetrics::from_terminal_font_size(EDITOR_FONT_SIZE);
        let scroll_offset = gpui::point(px(0.0), px(0.0));
        let row = Self::row_for_content_point_with_metrics(point, scroll_offset, metrics);
        let visual_col = Self::visual_column_for_content_point_with_metrics(
            point,
            scroll_offset,
            line.chars().count(),
            metrics,
        );
        CursorPosition::new(row, Self::byte_offset_for_visual_column(line, visual_col))
    }

    fn position_for_window_point(&self, point: Point<Pixels>) -> CursorPosition {
        let local = if let Some(bounds) = self.content_bounds {
            point - bounds.origin
        } else {
            point
        };
        self.position_for_content_point(local)
    }

    fn position_for_content_point(&self, point: Point<Pixels>) -> CursorPosition {
        let scroll_offset = self.scroll_handle.offset();
        let row = Self::row_for_content_point_with_metrics(point, scroll_offset, self.metrics);
        let line_text = self
            .active_tab_ref()
            .and_then(|tab| tab.buffer.lines().get(row).map(String::as_str))
            .unwrap_or("");
        let max_visual_col = line_text.chars().count();
        let visual_col = Self::visual_column_for_content_point_with_metrics(
            point,
            scroll_offset,
            max_visual_col,
            self.metrics,
        );
        CursorPosition::new(
            row,
            Self::byte_offset_for_visual_column(line_text, visual_col),
        )
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn position_for_content_point_with_scroll(
        point: Point<Pixels>,
        scroll_offset: Point<Pixels>,
        max_col: usize,
    ) -> CursorPosition {
        Self::position_for_content_point_with_metrics(
            point,
            scroll_offset,
            max_col,
            EditorMetrics::from_terminal_font_size(EDITOR_FONT_SIZE),
        )
    }

    fn position_for_content_point_with_metrics(
        point: Point<Pixels>,
        scroll_offset: Point<Pixels>,
        max_col: usize,
        metrics: EditorMetrics,
    ) -> CursorPosition {
        let row = Self::row_for_content_point_with_metrics(point, scroll_offset, metrics);
        let column = Self::visual_column_for_content_point_with_metrics(
            point,
            scroll_offset,
            max_col,
            metrics,
        );
        CursorPosition::new(row, column)
    }

    fn visual_column_for_content_point_with_metrics(
        point: Point<Pixels>,
        scroll_offset: Point<Pixels>,
        max_col: usize,
        metrics: EditorMetrics,
    ) -> usize {
        let clicked_x = f32::from(point.x).max(0.0);
        let scroll_x = f32::from(scroll_offset.x);
        let text_x = (clicked_x - scroll_x - ROW_TEXT_LEFT).max(0.0);
        let column = ((text_x / metrics.char_width) + 0.0001).floor() as usize;
        column.min(max_col)
    }

    fn row_for_content_point_with_metrics(
        point: Point<Pixels>,
        scroll_offset: Point<Pixels>,
        metrics: EditorMetrics,
    ) -> usize {
        ((f32::from(point.y) - f32::from(scroll_offset.y)).max(0.0) / metrics.line_height).floor()
            as usize
    }

    #[cfg(test)]
    fn selection_rect_for_line(
        line_index: usize,
        line_len: usize,
        selection: Option<(CursorPosition, CursorPosition)>,
    ) -> Option<(f32, f32)> {
        let (start, end) = selection?;
        if line_index < start.row || line_index > end.row {
            return None;
        }
        let start_col = if line_index == start.row {
            start.column.min(line_len)
        } else {
            0
        };
        let end_col = if line_index == end.row {
            end.column.min(line_len)
        } else {
            line_len
        };
        (end_col > start_col).then_some((
            ROW_TEXT_LEFT + start_col as f32 * CHAR_WIDTH,
            (end_col - start_col) as f32 * CHAR_WIDTH,
        ))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn row_min_width_for_visual_columns(columns: usize) -> f32 {
        ROW_TEXT_LEFT + columns as f32 * CHAR_WIDTH + CHAR_WIDTH
    }

    fn row_min_width_for_visual_columns_with_metrics(
        columns: usize,
        metrics: EditorMetrics,
    ) -> f32 {
        ROW_TEXT_LEFT + columns as f32 * metrics.char_width + metrics.char_width
    }

    fn visual_columns_for_line(line: &str) -> usize {
        line.chars().count()
    }

    fn utf16_column_to_visual_column(line: &str, utf16_column: usize) -> usize {
        let mut consumed_utf16 = 0;
        for (visual_column, ch) in line.chars().enumerate() {
            let next_utf16 = consumed_utf16 + ch.len_utf16();
            if utf16_column < next_utf16 {
                return visual_column;
            }
            consumed_utf16 = next_utf16;
        }
        line.chars().count()
    }

    fn byte_offset_for_visual_column(line: &str, visual_column: usize) -> usize {
        line.char_indices()
            .nth(visual_column)
            .map(|(offset, _)| offset)
            .unwrap_or(line.len())
    }

    fn visual_column_for_byte_offset(line: &str, byte_offset: usize) -> usize {
        let mut byte_offset = byte_offset.min(line.len());
        while byte_offset > 0 && !line.is_char_boundary(byte_offset) {
            byte_offset -= 1;
        }
        line[..byte_offset].chars().count()
    }

    pub fn mouse_down(&mut self, event: &MouseDownEvent) {
        if event.button != MouseButton::Left {
            return;
        }
        let position = self.position_for_window_point(event.position);
        if Self::should_select_word_on_mouse_down(event) {
            self.select_word_at(position.row, position.column);
            self.selection_anchor = None;
            return;
        }
        if event.modifiers.shift {
            let anchor = self.selection_anchor.unwrap_or_else(|| {
                self.active_tab_ref()
                    .map(|tab| tab.buffer.cursor())
                    .unwrap_or(position)
            });
            self.set_selection(anchor, position);
            self.selection_anchor = Some(anchor);
        } else {
            self.set_cursor(position.row, position.column);
            self.selection_anchor = Some(position);
        }
    }

    pub fn mouse_drag(&mut self, event: &MouseMoveEvent) {
        if !Self::should_extend_mouse_selection(event) {
            self.selection_anchor = None;
            return;
        }
        let Some(anchor) = self.selection_anchor else {
            return;
        };
        let position = self.position_for_window_point(event.position);
        self.set_selection(anchor, position);
    }

    pub fn mouse_up(&mut self, _event: &MouseUpEvent) {
        self.selection_anchor = None;
    }

    fn should_extend_mouse_selection(event: &MouseMoveEvent) -> bool {
        event.dragging()
    }

    fn should_select_word_on_mouse_down(event: &MouseDownEvent) -> bool {
        event.button == MouseButton::Left && event.click_count == 2 && !event.modifiers.shift
    }

    fn update_content_bounds(&mut self, bounds: Bounds<Pixels>) {
        self.content_bounds = Some(bounds);
    }

    fn point_hits_scrollbar(&self, point: Point<Pixels>) -> bool {
        let Some(bounds) = self.content_bounds else {
            return false;
        };
        Self::point_hits_scrollbar_in_bounds(bounds, point)
    }

    fn point_in_content_bounds(&self, point: Point<Pixels>) -> bool {
        Self::point_in_optional_bounds(self.content_bounds, point)
    }

    fn point_in_optional_bounds(bounds: Option<Bounds<Pixels>>, point: Point<Pixels>) -> bool {
        bounds.is_some_and(|bounds| bounds.contains(&point))
    }

    fn point_hits_scrollbar_in_bounds(bounds: Bounds<Pixels>, point: Point<Pixels>) -> bool {
        if !bounds.contains(&point) {
            return false;
        }
        let local_x = f32::from(point.x - bounds.origin.x);
        let local_y = f32::from(point.y - bounds.origin.y);
        local_x >= f32::from(bounds.size.width) - SCROLLBAR_HITBOX_SIZE
            || local_y >= f32::from(bounds.size.height) - SCROLLBAR_HITBOX_SIZE
    }

    fn scroll_x_for_cursor(
        cursor_x: f32,
        viewport_width: f32,
        content_width: f32,
        current_offset_x: f32,
    ) -> f32 {
        let max_scroll_left = (content_width - viewport_width).max(0.0);
        let current_left = (-current_offset_x).clamp(0.0, max_scroll_left);
        let current_right = current_left + viewport_width;
        let target_left = if cursor_x < current_left + CURSOR_SCROLL_PADDING {
            (cursor_x - CURSOR_SCROLL_PADDING).max(0.0)
        } else if cursor_x > current_right - CURSOR_SCROLL_PADDING {
            (cursor_x - viewport_width + CURSOR_SCROLL_PADDING).max(0.0)
        } else {
            current_left
        }
        .clamp(0.0, max_scroll_left);

        -target_left
    }

    fn widest_row_min_width(&self) -> f32 {
        let widest_columns = self
            .active_tab_ref()
            .and_then(|tab| {
                tab.buffer
                    .lines()
                    .iter()
                    .map(|line| Self::visual_columns_for_line(line))
                    .max()
            })
            .unwrap_or(0);
        Self::row_min_width_for_visual_columns_with_metrics(widest_columns, self.metrics)
    }

    fn cursor_visual_x(&self) -> Option<f32> {
        let tab = self.active_tab_ref()?;
        let cursor = tab.buffer.cursor();
        let line = tab.buffer.lines().get(cursor.row)?;
        let columns = Self::visual_column_for_byte_offset(line, cursor.column);
        Some(ROW_TEXT_LEFT + columns as f32 * self.metrics.char_width)
    }

    fn scroll_cursor_into_view(&mut self) {
        let Some(bounds) = self.content_bounds else {
            return;
        };
        let Some(cursor_x) = self.cursor_visual_x() else {
            return;
        };
        let cursor_row = self
            .active_tab_ref()
            .map(|tab| tab.buffer.cursor().row)
            .unwrap_or(0);
        self.scroll_handle
            .scroll_to_item(cursor_row, ScrollStrategy::Nearest);

        let viewport_width = f32::from(bounds.size.width).max(1.0);
        let content_width = self.widest_row_min_width().max(viewport_width);
        let current = self.scroll_handle.offset();
        let next_x = Self::scroll_x_for_cursor(
            cursor_x,
            viewport_width,
            content_width,
            f32::from(current.x),
        );
        self.scroll_handle
            .set_offset(gpui::point(px(next_x), current.y));
    }

    fn active_tab_mut(&mut self) -> Option<&mut EditorTab> {
        self.tabs.get_mut(self.active_tab)
    }

    fn ensure_lsp_for_path(&mut self, path: &Path) {
        let path = path.to_path_buf();
        if self.lsp_clients.contains_key(&path) {
            return;
        }
        let Some(text) = self
            .tabs
            .iter()
            .find(|tab| tab.path == path)
            .map(|tab| tab.buffer.text())
        else {
            return;
        };

        match LspClient::start(path.clone(), text, self.lsp_event_tx.clone()) {
            Ok(Some(client)) => {
                log::info!("[editor-lsp] started for {}", path.display());
                self.lsp_clients.insert(path, client);
            }
            Ok(None) => {}
            Err(error) => {
                log::warn!("[editor-lsp] unavailable for {}: {error}", path.display());
            }
        }
    }

    fn notify_lsp_active_did_change(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.active_tab_ref().map(|tab| tab.path.clone()) else {
            return;
        };
        if !self.lsp_clients.contains_key(&path) {
            return;
        }

        let generation = self
            .lsp_change_generations
            .entry(path.clone())
            .and_modify(|generation| *generation = generation.wrapping_add(1))
            .or_insert(1);
        let generation = *generation;

        self.lsp_change_debounce_tasks.insert(
            path.clone(),
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(LSP_DID_CHANGE_DEBOUNCE_MS))
                    .await;
                let _ = this.update(cx, |this, _cx| {
                    if this.lsp_change_generations.get(&path).copied() != Some(generation) {
                        return;
                    }
                    if let Some(text) = this
                        .tabs
                        .iter()
                        .find(|tab| tab.path == path)
                        .map(|tab| tab.buffer.text())
                    {
                        if let Some(client) = this.lsp_clients.get(&path) {
                            client.did_change(text);
                        }
                    }
                });
            }),
        );
    }

    fn drain_lsp_events(&mut self) -> bool {
        let mut changed = false;
        while let Ok(event) = self.lsp_event_rx.try_recv() {
            match event {
                LspClientEvent::Diagnostics { path, diagnostics } => {
                    self.lsp_diagnostics.insert(path, diagnostics);
                    changed = true;
                }
                LspClientEvent::Log(message) => {
                    log::warn!("[editor-lsp] {message}");
                }
            }
        }
        changed
    }

    pub fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.preview_active() {
            return;
        }
        if let Some(tab) = self.active_tab_mut() {
            tab.buffer.insert_text(text);
        }
        self.notify_lsp_active_did_change(cx);
        self.scroll_cursor_into_view();
    }

    pub fn insert_newline(&mut self, cx: &mut Context<Self>) {
        if self.preview_active() {
            return;
        }
        if let Some(tab) = self.active_tab_mut() {
            tab.buffer.insert_newline();
        }
        self.notify_lsp_active_did_change(cx);
        self.scroll_cursor_into_view();
    }

    pub fn delete_backward(&mut self, cx: &mut Context<Self>) {
        if self.preview_active() {
            return;
        }
        if let Some(tab) = self.active_tab_mut() {
            tab.buffer.delete_backward();
        }
        self.notify_lsp_active_did_change(cx);
        self.scroll_cursor_into_view();
    }

    pub fn delete_forward(&mut self, cx: &mut Context<Self>) {
        if self.preview_active() {
            return;
        }
        if let Some(tab) = self.active_tab_mut() {
            tab.buffer.delete_forward();
        }
        self.notify_lsp_active_did_change(cx);
        self.scroll_cursor_into_view();
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) -> bool {
        if self.preview_active() {
            return false;
        }
        let undid = self.active_tab_mut().is_some_and(|tab| tab.buffer.undo());
        if undid {
            self.notify_lsp_active_did_change(cx);
            self.scroll_cursor_into_view();
        }
        undid
    }

    pub fn move_left(&mut self) {
        if let Some(tab) = self.active_tab_mut() {
            tab.buffer.move_left();
        }
        self.scroll_cursor_into_view();
    }

    pub fn move_right(&mut self) {
        if let Some(tab) = self.active_tab_mut() {
            tab.buffer.move_right();
        }
        self.scroll_cursor_into_view();
    }

    pub fn move_up(&mut self) {
        if let Some(tab) = self.active_tab_mut() {
            tab.buffer.move_up();
        }
        self.scroll_cursor_into_view();
    }

    pub fn move_down(&mut self) {
        if let Some(tab) = self.active_tab_mut() {
            tab.buffer.move_down();
        }
        self.scroll_cursor_into_view();
    }

    pub fn move_home(&mut self) {
        if let Some(tab) = self.active_tab_mut() {
            tab.buffer.move_home();
        }
        self.scroll_cursor_into_view();
    }

    pub fn move_end(&mut self) {
        if let Some(tab) = self.active_tab_mut() {
            tab.buffer.move_end();
        }
        self.scroll_cursor_into_view();
    }

    pub fn move_line_start(&mut self) {
        if let Some(tab) = self.active_tab_mut() {
            tab.buffer.move_home();
        }
        self.scroll_cursor_into_view();
    }

    pub fn move_line_end(&mut self) {
        if let Some(tab) = self.active_tab_mut() {
            tab.buffer.move_end();
        }
        self.scroll_cursor_into_view();
    }

    pub fn select_left(&mut self) {
        if let Some(tab) = self.active_tab_mut() {
            tab.buffer.move_left_selecting();
        }
        self.scroll_cursor_into_view();
    }

    pub fn select_right(&mut self) {
        if let Some(tab) = self.active_tab_mut() {
            tab.buffer.move_right_selecting();
        }
        self.scroll_cursor_into_view();
    }

    pub fn select_up(&mut self) {
        if let Some(tab) = self.active_tab_mut() {
            tab.buffer.move_up_selecting();
        }
        self.scroll_cursor_into_view();
    }

    pub fn select_down(&mut self) {
        if let Some(tab) = self.active_tab_mut() {
            tab.buffer.move_down_selecting();
        }
        self.scroll_cursor_into_view();
    }

    pub fn select_home(&mut self) {
        if let Some(tab) = self.active_tab_mut() {
            tab.buffer.move_home_selecting();
        }
        self.scroll_cursor_into_view();
    }

    pub fn select_end(&mut self) {
        if let Some(tab) = self.active_tab_mut() {
            tab.buffer.move_end_selecting();
        }
        self.scroll_cursor_into_view();
    }

    pub fn select_all(&mut self) {
        if let Some(tab) = self.active_tab_mut() {
            tab.buffer.select_all();
        }
        self.scroll_cursor_into_view();
    }

    pub fn save_active(&mut self) -> std::io::Result<Option<PathBuf>> {
        let saved_path = {
            let Some(tab) = self.active_tab_mut() else {
                return Ok(None);
            };
            if !tab.save_enabled {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("cannot save {}; file failed to load", tab.path.display()),
                ));
            }
            tab.buffer.save_to(&tab.path)?;
            tab.path.clone()
        };
        self.dirty_close_blocked_tab = None;
        Ok(Some(saved_path))
    }

    pub fn cut_selection(&mut self, cx: &mut Context<Self>) -> Option<String> {
        if self.preview_active() {
            return None;
        }
        let text = self
            .active_tab_mut()
            .and_then(|tab| tab.buffer.cut_selection());
        if text.is_some() {
            self.notify_lsp_active_did_change(cx);
            self.scroll_cursor_into_view();
        }
        text
    }

    pub fn active_tab_ref(&self) -> Option<&EditorTab> {
        self.tabs.get(self.active_tab)
    }

    pub fn selected_text(&self) -> Option<String> {
        self.active_tab_ref()
            .and_then(|tab| tab.buffer.selected_text())
    }

    fn set_cursor(&mut self, row: usize, column: usize) {
        if let Some(tab) = self.active_tab_mut() {
            tab.buffer.set_cursor(row, column);
        }
        self.scroll_cursor_into_view();
    }

    fn set_selection(&mut self, anchor: CursorPosition, cursor: CursorPosition) {
        if let Some(tab) = self.active_tab_mut() {
            tab.buffer.set_selection(anchor, cursor);
        }
        self.scroll_cursor_into_view();
    }

    fn select_word_at(&mut self, row: usize, column: usize) {
        if let Some(tab) = self.active_tab_mut() {
            tab.buffer.select_word_at(row, column);
        }
        self.scroll_cursor_into_view();
    }

    fn diagnostic_color(
        theme: &gpui_component::Theme,
        severity: editor_lsp::DiagnosticSeverity,
    ) -> Hsla {
        match severity {
            editor_lsp::DiagnosticSeverity::Error => theme.danger_foreground,
            editor_lsp::DiagnosticSeverity::Warning => theme.warning_foreground,
            editor_lsp::DiagnosticSeverity::Info => theme.info_foreground,
            editor_lsp::DiagnosticSeverity::Hint => theme.muted_foreground,
        }
    }

    fn row_background_color(background: Hsla, foreground: Hsla, is_current_line: bool) -> Hsla {
        if is_current_line {
            foreground.opacity(0.055)
        } else {
            background
        }
    }
}

impl Focusable for EditorView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for EditorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.cursor_blink.is_none() {
            self.cursor_blink = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(550))
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        this.cursor_visible = !this.cursor_visible;
                        cx.notify();
                    });
                }
            }));
        }
        if self.lsp_event_pump.is_none() {
            self.lsp_event_pump = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(120))
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        if this.drain_lsp_events() {
                            cx.notify();
                        }
                    });
                }
            }));
        }
        if self.drain_lsp_events() {
            cx.notify();
        }

        let theme = cx.theme().clone();
        let fg = theme.foreground;
        let bg = theme.background;
        let mono_font = cx.theme().mono_font_family.clone();
        let ui_font = cx.theme().font_family.clone();

        if self.tabs.is_empty() {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(theme.background)
                .text_color(theme.muted_foreground)
                .font_family(ui_font)
                .child("No file open")
                .into_any_element();
        }

        let active_index = self.active_tab;
        let metrics = self.metrics;
        let active = self.tabs[active_index].render_snapshot(&theme, mono_font.clone(), metrics);
        let diagnostics = self
            .lsp_diagnostics
            .get(&active.path)
            .cloned()
            .unwrap_or_default();
        let lines = active.lines.clone();
        let syntax_runs = active.syntax_runs.clone();
        let line_count = active.line_count;
        let cursor = active.cursor;
        let selection = active.selection;
        let cursor_visual_column = active
            .lines
            .get(cursor.row)
            .map(|line| Self::visual_column_for_byte_offset(line, cursor.column))
            .unwrap_or(0);
        let cursor_visible = self.cursor_visible;
        let widest_line_index = active.widest_line_index;
        let line_height = px(metrics.line_height);
        let gutter_bg = theme.muted.opacity(0.04);
        let gutter_color = theme.muted_foreground.opacity(0.42);
        let preview_active = self.preview_active();
        if preview_active {
            let cache_stale = match &self.tabs[active_index].preview_cache {
                Some((revision, _)) => *revision != self.tabs[active_index].buffer.revision(),
                None => true,
            };
            if cache_stale {
                self.schedule_preview_parse(active_index, cx);
            }
        }
        let preview_document = if preview_active {
            self.tabs[active_index]
                .preview_cache
                .as_ref()
                .map(|(_, document)| document.clone())
        } else {
            None
        };
        let tabs = self
            .tabs
            .iter()
            .map(|tab| (tab.path.clone(), tab.buffer.is_dirty()))
            .collect::<Vec<_>>();

        let mut tab_bar = div()
            .h(px(TAB_BAR_HEIGHT))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(2.0))
            .px(px(6.0))
            .bg(theme.tab_bar_segmented.opacity(0.72))
            .overflow_x_scrollbar();

        for (index, (path, dirty)) in tabs.iter().enumerate() {
            let is_active = index == active_index;
            let title = path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string());
            let label = if *dirty {
                format!("● {title}")
            } else {
                title
            };
            let activate_index = index;
            let close_index = index;
            let tab_active_bg = theme.tab_active;
            let tab_transparent_bg = theme.transparent;
            let hover_fg = fg;
            let label_color = if is_active {
                fg.opacity(0.92)
            } else {
                theme.muted_foreground
            };
            let mut tab_el = div()
                .id(("editor-file-tab", index))
                .h(px(22.0))
                .max_w(px(180.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(5.0))
                .pl(px(8.0))
                .pr(px(3.0))
                .rounded(px(6.0))
                .cursor_pointer()
                .bg(if is_active {
                    tab_active_bg
                } else {
                    tab_transparent_bg
                })
                .hover(move |s| {
                    s.bg(if is_active {
                        tab_active_bg
                    } else {
                        hover_fg.opacity(0.08)
                    })
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _event, window, cx| {
                        this.focus_handle.focus(window, cx);
                        this.activate_tab_and_emit(activate_index, cx);
                    }),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(12.0))
                        .line_height(px(14.0))
                        .font_family(ui_font.clone())
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(label_color)
                        .child(SharedString::from(label)),
                );

            tab_el = tab_el.child(
                div()
                    .size(px(14.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .text_size(px(11.0))
                    .text_color(fg.opacity(if is_active { 0.55 } else { 0.42 }))
                    .hover(|s| s.bg(gpui::black().opacity(0.08)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, window, cx| {
                            cx.stop_propagation();
                            window.prevent_default();
                            this.focus_handle.focus(window, cx);
                            this.close_tab(close_index, cx);
                        }),
                    )
                    .child(
                        svg()
                            .path("phosphor/x.svg")
                            .size(px(8.0))
                            .text_color(fg.opacity(if is_active { 0.55 } else { 0.42 })),
                    ),
            );

            tab_bar = tab_bar.child(tab_el);
        }

        let preview_available = editor_syntax::language_for_path(&active.path) == Some("markdown");
        let preview_toggle = preview_available.then(|| {
            let icon = if preview_active {
                "phosphor/code.svg"
            } else {
                "phosphor/eye.svg"
            };
            div()
                .h(px(TAB_BAR_HEIGHT))
                .flex_shrink_0()
                .flex()
                .items_center()
                .px(px(6.0))
                .bg(theme.tab_bar_segmented.opacity(0.72))
                .child(
                    Button::new(("editor-preview-toggle", self.view_id))
                        .icon(Icon::default().path(icon))
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _event, window, cx| {
                            this.focus_handle.focus(window, cx);
                            this.toggle_preview(cx);
                        })),
                )
        });

        let header = div()
            .h(px(TAB_BAR_HEIGHT))
            .flex_shrink_0()
            .flex()
            .child(tab_bar.flex_1().min_w_0())
            .children(preview_toggle);

        let diagnostic_count = diagnostics.len();
        let diagnostics_label = if diagnostic_count == 0 {
            String::new()
        } else if diagnostic_count == 1 {
            " — 1 issue".to_string()
        } else {
            format!(" — {diagnostic_count} issues")
        };
        let status_text: SharedString = format!(
            "{} lines — Ln {}, Col {}{} — {}",
            line_count,
            cursor.row + 1,
            cursor_visual_column + 1,
            diagnostics_label,
            active.path.display()
        )
        .into();
        let dirty_close_blocked = self.dirty_close_blocked_tab == Some(active_index);
        let mut status_bar = div()
            .h(px(22.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.0))
            .px(px(12.0))
            .bg(theme.muted.opacity(0.05))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(10.5))
                    .text_color(theme.muted_foreground.opacity(0.55))
                    .font_family(mono_font.clone())
                    .child(status_text),
            );
        if dirty_close_blocked {
            status_bar = status_bar.child(
                div()
                    .flex_shrink_0()
                    .text_size(px(10.5))
                    .text_color(theme.warning_foreground.opacity(0.86))
                    .font_family(ui_font.clone())
                    .child("Unsaved changes - save before closing"),
            );
        }

        let mono_font_list = mono_font.clone();
        let list_theme = theme.clone();
        let list = uniform_list("editor-lines", line_count, move |range, _window, _cx| {
            range
                .map(|i| {
                    let line_num: SharedString = format!("{}", i + 1).into();
                    let line_text = lines[i].to_string();
                    let diagnostic =
                        editor_lsp::strongest_diagnostic_for_line(&diagnostics, i).cloned();
                    let diagnostic_color = diagnostic
                        .as_ref()
                        .map(|diagnostic| Self::diagnostic_color(&list_theme, diagnostic.severity));
                    let line_runs = syntax_runs.get(i).cloned().unwrap_or_default();
                    let line_visual_columns = Self::visual_columns_for_line(&line_text);
                    let row_min_width = px(Self::row_min_width_for_visual_columns_with_metrics(
                        line_visual_columns,
                        metrics,
                    ));
                    let selection_for_line = selection.and_then(|(start, end)| {
                        if i < start.row || i > end.row {
                            return None;
                        }
                        let start_col = if i == start.row {
                            Self::visual_column_for_byte_offset(&line_text, start.column)
                        } else {
                            0
                        };
                        let end_col = if i == end.row {
                            Self::visual_column_for_byte_offset(&line_text, end.column)
                        } else {
                            line_visual_columns
                        };
                        (end_col > start_col).then_some((start_col, end_col))
                    });
                    let cursor_visual_col =
                        Self::visual_column_for_byte_offset(&line_text, cursor.column);
                    let cursor_left =
                        TEXT_IN_CONTENT_LEFT + cursor_visual_col as f32 * metrics.char_width;
                    let text = SharedString::from(line_text.clone());
                    let highlighted_text = StyledText::new(text.clone()).with_runs(line_runs);
                    let mut content = div()
                        .flex_1()
                        .min_w_0()
                        .h(px(metrics.line_height))
                        .relative()
                        .child(
                            div()
                                .absolute()
                                .left(px(TEXT_IN_CONTENT_LEFT))
                                .top(px(0.0))
                                .h(px(metrics.line_height))
                                .flex()
                                .items_center()
                                .text_size(px(metrics.font_size))
                                .text_color(fg.opacity(0.90))
                                .font_family(mono_font_list.clone())
                                .whitespace_nowrap()
                                .child(highlighted_text),
                        );
                    if let Some((start_col, end_col)) = selection_for_line {
                        content = content.child(
                            div()
                                .absolute()
                                .left(px(
                                    TEXT_IN_CONTENT_LEFT + start_col as f32 * metrics.char_width
                                ))
                                .top(px(2.0))
                                .h(px((metrics.line_height - 4.0).max(1.0)))
                                .w(px((end_col - start_col).max(1) as f32 * metrics.char_width))
                                .bg(fg.opacity(0.16)),
                        );
                    }
                    if i == cursor.row {
                        content = content.child(
                            div()
                                .absolute()
                                .left(px(cursor_left))
                                .top(px(3.0))
                                .w(px(1.5))
                                .h(px((metrics.line_height - 6.0).max(1.0)))
                                .bg(if cursor_visible {
                                    fg.opacity(0.90)
                                } else {
                                    fg.opacity(0.0)
                                }),
                        );
                    }
                    if let (Some(diagnostic), Some(color)) = (diagnostic.as_ref(), diagnostic_color)
                    {
                        let diagnostic_start_col = Self::utf16_column_to_visual_column(
                            &line_text,
                            diagnostic.start_character,
                        );
                        let diagnostic_end_col = Self::utf16_column_to_visual_column(
                            &line_text,
                            diagnostic.end_character,
                        );
                        let start_col = diagnostic_start_col.min(line_visual_columns);
                        let end_col = diagnostic_end_col
                            .max(diagnostic_start_col.saturating_add(1))
                            .min(line_visual_columns.max(start_col + 1));
                        content = content.child(
                            div()
                                .absolute()
                                .left(px(
                                    TEXT_IN_CONTENT_LEFT + start_col as f32 * metrics.char_width
                                ))
                                .bottom(px(2.0))
                                .h(px(1.5))
                                .w(px((end_col - start_col).max(1) as f32 * metrics.char_width))
                                .bg(color.opacity(0.85)),
                        );
                    }
                    let mut gutter = div()
                        .w(px(GUTTER_WIDTH))
                        .flex_shrink_0()
                        .h(px(metrics.line_height))
                        .flex()
                        .items_center()
                        .justify_end()
                        .pr(px(10.0))
                        .relative()
                        .bg(gutter_bg)
                        .child(
                            div()
                                .text_size(px((metrics.font_size - 1.0).max(1.0)))
                                .text_color(diagnostic_color.unwrap_or(gutter_color))
                                .font_family(mono_font_list.clone())
                                .child(line_num),
                        );
                    if let Some(color) = diagnostic_color {
                        gutter = gutter.child(
                            div()
                                .absolute()
                                .left(px(6.0))
                                .top(px(7.0))
                                .size(px(6.0))
                                .rounded_full()
                                .bg(color.opacity(0.78)),
                        );
                    }
                    div()
                        .h(line_height)
                        .w_full()
                        .min_w(row_min_width)
                        .flex()
                        .flex_row()
                        .items_start()
                        .relative()
                        .bg(Self::row_background_color(bg, fg, i == cursor.row))
                        .child(gutter)
                        .child(content)
                })
                .collect()
        })
        .flex_1()
        .min_h_0()
        .with_width_from_item(Some(widest_line_index))
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .track_scroll(&self.scroll_handle);

        let list_frame = div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(list)
            .child(Scrollbar::new(&self.scroll_handle).scrollbar_show(ScrollbarShow::Always));

        let body = if preview_active {
            match preview_document {
                Some(document) => {
                    let base_dir = active
                        .path
                        .parent()
                        .map(Path::to_path_buf)
                        .unwrap_or_else(|| PathBuf::from("."));
                    let namespace = format!("editor-preview-{}-{active_index}", self.view_id);
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(editor_preview::render_markdown_preview(
                            &document,
                            &base_dir,
                            &theme,
                            &self.preview_scroll_handle,
                            &namespace,
                        ))
                        .into_any_element()
                }
                None => div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.muted_foreground.opacity(0.6))
                    .child("Rendering preview…")
                    .into_any_element(),
            }
        } else {
            list_frame.into_any_element()
        };

        let view_handle = cx.weak_entity();

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .cursor(CursorStyle::IBeam)
            .track_focus(&self.focus_handle)
            .key_context("EditorView")
            .on_children_prepainted(move |bounds_list, _window, cx| {
                let Some(bounds) = bounds_list.get(1).copied() else {
                    return;
                };
                if let Some(view) = view_handle.upgrade() {
                    view.update(cx, |this, _cx| this.update_content_bounds(bounds));
                }
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    if this.point_hits_scrollbar(event.position) {
                        return;
                    }
                    if !this.point_in_content_bounds(event.position) {
                        return;
                    }
                    this.focus_handle.focus(window, cx);
                    if this.preview_active() {
                        window.prevent_default();
                        cx.notify();
                        return;
                    }
                    this.mouse_down(event);
                    window.prevent_default();
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.preview_active()
                    || this.point_hits_scrollbar(event.position)
                    || !this.point_in_content_bounds(event.position)
                {
                    return;
                }
                this.mouse_drag(event);
                cx.stop_propagation();
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                    if this.preview_active() {
                        return;
                    }
                    if this.point_hits_scrollbar(event.position) {
                        this.mouse_up(event);
                        return;
                    }
                    let in_content = this.point_in_content_bounds(event.position);
                    this.mouse_up(event);
                    if !in_content {
                        return;
                    }
                    window.prevent_default();
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(header)
            .child(body)
            .child(status_bar)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn preview_toggle_only_applies_to_markdown() {
        assert_eq!(
            EditorView::preview_toggle_target(Path::new("notes.md"), false),
            Some(true)
        );
        assert_eq!(
            EditorView::preview_toggle_target(Path::new("README.markdown"), false),
            Some(true)
        );
        assert_eq!(
            EditorView::preview_toggle_target(Path::new("main.rs"), false),
            None
        );
        assert_eq!(
            EditorView::preview_toggle_target(Path::new("notes.md"), true),
            Some(false)
        );
    }

    #[test]
    fn content_point_accounts_for_text_gutter() {
        assert_eq!(
            EditorView::position_for_content_point_for_test(gpui::point(
                px(ROW_TEXT_LEFT),
                px(0.0)
            )),
            CursorPosition::new(0, 0)
        );
        assert_eq!(
            EditorView::position_for_content_point_for_test(gpui::point(
                px(ROW_TEXT_LEFT + CHAR_WIDTH * 3.1),
                px(LINE_HEIGHT * 2.2)
            )),
            CursorPosition::new(2, 3)
        );
    }

    #[test]
    fn mouse_position_and_selection_highlight_share_text_origin() {
        let text_x = px(ROW_TEXT_LEFT + CHAR_WIDTH * 2.0);
        let text_y = px(LINE_HEIGHT * 1.25);
        let position = EditorView::position_for_content_point_for_test(gpui::point(text_x, text_y));
        assert_eq!(position, CursorPosition::new(1, 2));

        let rect = EditorView::selection_rect_for_line(
            1,
            10,
            Some((CursorPosition::new(1, 2), CursorPosition::new(1, 5))),
        );
        assert_eq!(
            rect,
            Some((ROW_TEXT_LEFT + CHAR_WIDTH * 2.0, CHAR_WIDTH * 3.0))
        );
    }

    #[test]
    fn hit_testing_uses_inner_content_coordinates_not_row_coordinates() {
        // Mouse events are converted through `content_bounds`, which is the
        // uniform-list bounds. Each row still contains the line-number gutter,
        // so column 0 begins at ROW_TEXT_LEFT in list-local coordinates.
        assert_eq!(
            EditorView::position_for_content_point_for_test(gpui::point(
                px(ROW_TEXT_LEFT + CHAR_WIDTH * 6.0),
                px(0.0),
            )),
            CursorPosition::new(0, 6)
        );
    }

    #[test]
    fn mouse_column_uses_same_origin_as_rendered_text() {
        // The editor list-local x-coordinate includes the line-number gutter
        // because each row is laid out as `gutter + content`. Column 0 starts
        // where the rendered text/cursor starts: gutter + text inset.
        assert_eq!(
            EditorView::position_for_content_point_for_test(gpui::point(
                px(ROW_TEXT_LEFT + CHAR_WIDTH * 5.4),
                px(0.0),
            )),
            CursorPosition::new(0, 5)
        );
        assert_eq!(
            EditorView::position_for_content_point_for_test(gpui::point(
                px(ROW_TEXT_LEFT + CHAR_WIDTH * 5.6),
                px(0.0),
            )),
            CursorPosition::new(0, 5)
        );
    }

    #[test]
    fn mouse_column_for_utf8_line_returns_byte_offset() {
        let position = EditorView::position_for_content_point_in_line_for_test(
            gpui::point(px(ROW_TEXT_LEFT + CHAR_WIDTH * 1.1), px(0.0)),
            "éx",
        );

        assert_eq!(position, CursorPosition::new(0, "é".len()));
    }

    #[test]
    fn visual_column_and_byte_offset_conversion_handle_utf8() {
        assert_eq!(
            EditorView::visual_column_for_byte_offset("éx", "é".len()),
            1
        );
        assert_eq!(
            EditorView::byte_offset_for_visual_column("éx", 1),
            "é".len()
        );
    }

    #[test]
    fn utf16_diagnostic_columns_convert_to_visual_columns() {
        assert_eq!(EditorView::utf16_column_to_visual_column("a😀b", 0), 0);
        assert_eq!(EditorView::utf16_column_to_visual_column("a😀b", 1), 1);
        assert_eq!(EditorView::utf16_column_to_visual_column("a😀b", 3), 2);
        assert_eq!(EditorView::utf16_column_to_visual_column("a😀b", 4), 3);
    }

    #[test]
    fn scrolled_content_point_uses_negative_gpui_scroll_offset() {
        assert_eq!(
            EditorView::position_for_content_point_with_scroll(
                gpui::point(px(ROW_TEXT_LEFT + CHAR_WIDTH * 4.0), px(LINE_HEIGHT * 5.0)),
                gpui::point(px(0.0), px(-(LINE_HEIGHT * 295.0))),
                20,
            ),
            CursorPosition::new(300, 4)
        );
    }

    #[test]
    fn horizontal_scroll_follows_cursor_when_cursor_leaves_viewport() {
        assert_eq!(
            EditorView::scroll_x_for_cursor(600.0, 200.0, 1000.0, 0.0),
            -432.0
        );
        assert_eq!(
            EditorView::scroll_x_for_cursor(50.0, 200.0, 1000.0, -432.0),
            -18.0
        );
        assert_eq!(
            EditorView::scroll_x_for_cursor(100.0, 200.0, 1000.0, 0.0),
            0.0
        );
    }

    #[test]
    fn ctrl_a_and_ctrl_e_use_non_selecting_boundary_moves() {
        let source = include_str!("editor_view.rs");
        let line_start = source
            .split("pub fn move_line_start")
            .nth(1)
            .and_then(|chunk| chunk.split("pub fn move_line_end").next())
            .expect("move_line_start method exists");
        assert!(
            line_start.contains("tab.buffer.move_home()"),
            "ctrl-a must clear any active selection"
        );

        let line_end = source
            .split("pub fn move_line_end")
            .nth(1)
            .and_then(|chunk| chunk.split("pub fn select_left").next())
            .expect("move_line_end method exists");
        assert!(
            line_end.contains("tab.buffer.move_end()"),
            "ctrl-e must clear any active selection"
        );
    }

    #[test]
    fn editor_root_declares_editor_key_context() {
        let source = include_str!("editor_view.rs");

        assert!(source.contains(".key_context(\"EditorView\")"));
    }

    #[test]
    fn mouse_move_without_left_button_does_not_continue_selection() {
        let released_move = MouseMoveEvent {
            pressed_button: None,
            ..Default::default()
        };
        assert!(!EditorView::should_extend_mouse_selection(&released_move));

        let dragging_move = MouseMoveEvent {
            pressed_button: Some(MouseButton::Left),
            ..Default::default()
        };
        assert!(EditorView::should_extend_mouse_selection(&dragging_move));
    }

    #[test]
    fn double_left_click_selects_word_without_shift() {
        let double_click = MouseDownEvent {
            button: MouseButton::Left,
            click_count: 2,
            ..Default::default()
        };
        assert!(EditorView::should_select_word_on_mouse_down(&double_click));

        let single_click = MouseDownEvent {
            button: MouseButton::Left,
            click_count: 1,
            ..Default::default()
        };
        assert!(!EditorView::should_select_word_on_mouse_down(&single_click));

        let shifted_double_click = MouseDownEvent {
            button: MouseButton::Left,
            click_count: 2,
            modifiers: gpui::Modifiers {
                shift: true,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!EditorView::should_select_word_on_mouse_down(
            &shifted_double_click
        ));
    }

    #[test]
    fn scrollbar_hitbox_covers_bottom_and_right_edges() {
        let bounds = Bounds::new(
            gpui::point(px(10.0), px(20.0)),
            gpui::size(px(200.0), px(100.0)),
        );

        assert!(EditorView::point_hits_scrollbar_in_bounds(
            bounds,
            gpui::point(px(205.0), px(60.0))
        ));
        assert!(EditorView::point_hits_scrollbar_in_bounds(
            bounds,
            gpui::point(px(80.0), px(115.0))
        ));
        assert!(!EditorView::point_hits_scrollbar_in_bounds(
            bounds,
            gpui::point(px(80.0), px(60.0))
        ));
    }

    #[test]
    fn editor_mouse_hit_testing_rejects_non_content_chrome() {
        let bounds = Bounds::new(
            gpui::point(px(10.0), px(20.0)),
            gpui::size(px(200.0), px(100.0)),
        );

        assert!(EditorView::point_in_optional_bounds(
            Some(bounds),
            gpui::point(px(80.0), px(60.0))
        ));
        assert!(!EditorView::point_in_optional_bounds(
            Some(bounds),
            gpui::point(px(80.0), px(19.0))
        ));
        assert!(!EditorView::point_in_optional_bounds(
            Some(bounds),
            gpui::point(px(80.0), px(121.0))
        ));
        assert!(!EditorView::point_in_optional_bounds(
            None,
            gpui::point(px(80.0), px(60.0))
        ));
    }

    #[test]
    fn multiline_selection_rects_continue_after_line_end() {
        let selection = Some((CursorPosition::new(192, 0), CursorPosition::new(194, 4)));

        assert_eq!(
            EditorView::selection_rect_for_line(192, 77, selection),
            Some((ROW_TEXT_LEFT, 77.0 * CHAR_WIDTH))
        );
        assert_eq!(
            EditorView::selection_rect_for_line(193, 78, selection),
            Some((ROW_TEXT_LEFT, 78.0 * CHAR_WIDTH))
        );
        assert_eq!(
            EditorView::selection_rect_for_line(194, 80, selection),
            Some((ROW_TEXT_LEFT, 4.0 * CHAR_WIDTH))
        );
    }

    #[test]
    fn char_width_matches_editor_mono_grid() {
        assert_eq!(editor_char_width(10.0), 6.0);
        assert_eq!(CHAR_WIDTH, EDITOR_FONT_SIZE * 0.6);
    }

    #[test]
    fn editor_metrics_follow_terminal_font_size() {
        let metrics = EditorMetrics::from_terminal_font_size(16.0);

        assert_eq!(metrics.font_size, 16.0);
        assert_eq!(metrics.line_height, 24.0);
        assert_eq!(metrics.char_width, editor_char_width(16.0));
    }

    #[test]
    fn row_min_width_covers_full_line_overlay() {
        let columns = 120;
        let min_width = EditorView::row_min_width_for_visual_columns(columns);

        assert!(min_width >= ROW_TEXT_LEFT + columns as f32 * CHAR_WIDTH);
    }

    #[test]
    fn current_line_background_uses_subtle_foreground_tint() {
        let background = gpui::white();
        let foreground = gpui::black();

        assert_eq!(
            EditorView::row_background_color(background, foreground, false),
            background
        );
        assert_eq!(
            EditorView::row_background_color(background, foreground, true),
            foreground.opacity(0.055)
        );
    }

    #[test]
    fn editor_tab_reuses_render_snapshot_until_buffer_revision_changes() {
        let mut tab = EditorTab::new(
            PathBuf::from("src/main.rs"),
            EditorBuffer::from_text("fn main() {}\n"),
        );
        let theme = gpui_component::Theme::default();
        let mono_font = SharedString::from("Test Mono");
        let metrics = EditorMetrics::from_terminal_font_size(EDITOR_FONT_SIZE);

        let first = tab.render_snapshot(&theme, mono_font.clone(), metrics);
        let second = tab.render_snapshot(&theme, mono_font.clone(), metrics);

        assert!(std::sync::Arc::ptr_eq(&first.lines, &second.lines));
        assert!(std::sync::Arc::ptr_eq(
            &first.syntax_runs,
            &second.syntax_runs
        ));

        tab.buffer.insert_text("// changed");
        let third = tab.render_snapshot(&theme, mono_font, metrics);

        assert!(!std::sync::Arc::ptr_eq(&first.lines, &third.lines));
        assert!(!std::sync::Arc::ptr_eq(
            &first.syntax_runs,
            &third.syntax_runs
        ));
    }

    #[test]
    fn read_error_tabs_are_not_save_enabled() {
        let tab = EditorTab::read_error(
            PathBuf::from("not-utf8.bin"),
            std::io::Error::new(std::io::ErrorKind::InvalidData, "bad data"),
        );

        assert!(!tab.save_enabled);
        assert!(tab.buffer.text().contains("Error reading file:"));
    }

    #[test]
    fn stale_file_load_adds_background_tab_without_stealing_focus() {
        let mut tabs = vec![EditorTab::new(
            PathBuf::from("src/existing.rs"),
            EditorBuffer::from_text("existing"),
        )];
        let mut active_tab = 0;

        let apply = EditorView::apply_loaded_file_tab(
            &mut tabs,
            &mut active_tab,
            PathBuf::from("src/pending.rs"),
            false,
            false,
            |path| EditorTab::new(path, EditorBuffer::from_text("pending")),
        );

        assert_eq!(tabs.len(), 2);
        assert_eq!(active_tab, 0);
        assert_eq!(tabs[0].path, PathBuf::from("src/existing.rs"));
        assert_eq!(tabs[1].path, PathBuf::from("src/pending.rs"));
        assert_eq!(
            apply,
            LoadedFileApply {
                activated: false,
                active_file_changed: false,
            }
        );
    }

    #[test]
    fn fresh_file_load_activates_new_tab() {
        let mut tabs = vec![EditorTab::new(
            PathBuf::from("src/existing.rs"),
            EditorBuffer::from_text("existing"),
        )];
        let mut active_tab = 0;

        let apply = EditorView::apply_loaded_file_tab(
            &mut tabs,
            &mut active_tab,
            PathBuf::from("src/new.rs"),
            true,
            false,
            |path| EditorTab::new(path, EditorBuffer::from_text("new")),
        );

        assert_eq!(tabs.len(), 2);
        assert_eq!(active_tab, 1);
        assert_eq!(tabs[active_tab].path, PathBuf::from("src/new.rs"));
        assert_eq!(
            apply,
            LoadedFileApply {
                activated: true,
                active_file_changed: true,
            }
        );
    }

    #[test]
    fn buffer_tab_behaviors_are_covered_by_editor_buffer_tests() {
        let _ = Path::new("/tmp/example.txt");
    }
}

//! File tree panel — shows the directory tree rooted at the active tab's cwd.
//!
//! Phase 1: read-only directory listing. Clicking a file emits `OpenFile`.
//! The tree root follows the active tab's CWD. Native filesystem events
//! refresh expanded directories while preserving their expansion state.
//!
//! Visual rules
//! ---
//! - Row height: 24 px.
//! - Indent: 12 px per depth level.
//! - Icons come from `file_icons::icon_for_path`: either a Phosphor SVG or,
//!   for languages that have a Seti glyph, a Nerd Font glyph drawn in the
//!   bundled icon font.
//! - Active (open) file row gets a subtle accent bg.
//! - No borders — surface separation via bg opacity.

use futures::{StreamExt, channel::mpsc};
use gpui::{
    ClipboardItem, Context, Entity, EventEmitter, Focusable, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, Pixels, Point, Render, SharedString, Styled, Subscription,
    Window, anchored, deferred, div, prelude::*, px, svg, uniform_list,
};
use gpui_component::menu::PopupMenu;
use gpui_component::{ActiveTheme, tooltip::Tooltip};
use notify::{EventKind, RecursiveMode, Watcher, event::ModifyKind};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::file_icons::{FILE_ICON_FONT_FAMILY, FileIcon, icon_for_path};
use crate::ui_scale::ui_icon_px;

pub(crate) const ROW_HEIGHT: f32 = 24.0;
const INDENT_PER_LEVEL: f32 = 12.0;
const ICON_SIZE: f32 = 13.0;

struct FileContextMenu {
    menu: Entity<PopupMenu>,
    position: Point<Pixels>,
    previous_focus: Option<gpui::FocusHandle>,
    _dismiss: Subscription,
    _observe: Subscription,
}

/// A single entry in the flat file tree list.
#[derive(Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub is_expanded: bool,
}

/// Single-click previews; double-click keeps the file in the shared editor pane.
pub struct OpenFile {
    pub path: PathBuf,
    pub keep_open: bool,
}

impl EventEmitter<OpenFile> for FileTreeView {}

/// Emitted when the user clicks the "open in editor tab" button on a file row.
pub struct OpenFileInEditorTab {
    pub path: PathBuf,
}

impl EventEmitter<OpenFileInEditorTab> for FileTreeView {}

pub struct FileTreeView {
    root: Option<PathBuf>,
    entries: Arc<Vec<FileEntry>>,
    /// Path of the currently open file (highlighted row).
    active_path: Option<PathBuf>,
    load_generation: u64,
    root_generation: u64,
    expanded_paths: HashSet<PathBuf>,
    watcher: Option<Arc<Mutex<TreeWatcher>>>,
    watch_task: Option<gpui::Task<()>>,
    context_menu: Option<FileContextMenu>,
    scroll: gpui::UniformListScrollHandle,
}

impl FileTreeView {
    pub fn new() -> Self {
        Self {
            root: None,
            entries: Arc::new(Vec::new()),
            active_path: None,
            load_generation: 0,
            root_generation: 0,
            expanded_paths: HashSet::new(),
            watcher: None,
            watch_task: None,
            context_menu: None,
            scroll: gpui::UniformListScrollHandle::new(),
        }
    }

    /// Set the root directory and rebuild the entry list.
    pub fn set_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if self.root.as_deref() == Some(root.as_path()) {
            return;
        }
        self.root_generation = self.root_generation.wrapping_add(1);
        self.load_generation = self.load_generation.wrapping_add(1);
        self.watch_task = None;
        self.context_menu = None;
        self.watcher = None;
        self.root = Some(root.clone());
        self.expanded_paths = HashSet::from([root.clone()]);
        self.entries = Arc::new(root_placeholder_entry(&root));
        cx.notify();
        self.start_watching(root, cx);
    }

    pub fn set_active_path(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.active_path != path {
            self.active_path = path;
            cx.notify();
        }
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    fn open_context_menu(
        &mut self,
        path: PathBuf,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previous_focus = window.focused(cx);
        let action_context = previous_focus.clone();
        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            let menu = if let Some(handle) = action_context {
                menu.action_context(handle)
            } else {
                menu
            };
            build_file_context_menu(menu, path)
        });
        let dismiss = cx.subscribe(&menu, |this, _, _: &gpui::DismissEvent, cx| {
            this.context_menu = None;
            cx.notify();
        });
        let observe = cx.observe(&menu, |_, _, cx| cx.notify());
        menu.focus_handle(cx).focus(window, cx);
        self.context_menu = Some(FileContextMenu {
            menu,
            position,
            previous_focus,
            _dismiss: dismiss,
            _observe: observe,
        });
        cx.notify();
    }

    /// A retained tree must not bring an old popup back when its tool reopens.
    pub(crate) fn dismiss_context_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.context_menu.take() else {
            return;
        };
        if state.menu.focus_handle(cx).is_focused(window)
            && let Some(previous_focus) = state.previous_focus
        {
            previous_focus.focus(window, cx);
        }
        cx.notify();
    }

    pub fn active_path(&self) -> Option<&Path> {
        self.active_path.as_deref()
    }

    /// Toggle expand/collapse for a directory entry.
    fn toggle_dir(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(idx) = self.entries.iter().position(|e| e.path == path) else {
            return;
        };
        let entries = Arc::make_mut(&mut self.entries);
        let entry = &mut entries[idx];
        if !entry.is_dir {
            return;
        }
        entry.is_expanded = !entry.is_expanded;
        let expanded = entry.is_expanded;
        if expanded {
            self.expanded_paths.insert(path.to_path_buf());
        } else {
            remove_descendants(entries, idx);
            self.expanded_paths
                .retain(|expanded| !expanded.starts_with(path));
        }
        self.refresh(cx);
        cx.notify();
    }

    fn start_watching(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        let generation = self.root_generation;
        let (mut tx, mut rx) = mpsc::channel(1);
        self.watch_task = Some(cx.spawn(async move |this, cx| {
            let watcher = cx
                .background_executor()
                .spawn(async move {
                    let event_root = root.canonicalize().unwrap_or(root.clone());
                    let requested_root = root.clone();
                    TreeWatcher::new(&root, move |event| {
                        // macOS reports canonical paths; Windows can retain the
                        // requested spelling rather than the verbatim UNC form.
                        if tree_event_needs_refresh(&event, &event_root)
                            || tree_event_needs_refresh(&event, &requested_root)
                        {
                            // A bounded wake queue coalesces bursts without blocking
                            // the native filesystem callback.
                            let _ = tx.try_send(());
                        }
                    })
                })
                .await;
            if this
                .update(cx, |this, cx| {
                    if this.root_generation != generation {
                        return;
                    }
                    match watcher {
                        Ok(watcher) => this.watcher = Some(Arc::new(Mutex::new(watcher))),
                        Err(error) => log::warn!("File tree watcher unavailable: {error}"),
                    }
                    this.refresh(cx);
                })
                .is_err()
            {
                return;
            }
            while rx.next().await.is_some() {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                while rx.try_recv().is_ok() {}
                if this
                    .update(cx, |this, cx| {
                        if this.root_generation == generation {
                            this.refresh(cx);
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }
        }));
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        self.load_generation = self.load_generation.wrapping_add(1);
        let generation = self.load_generation;
        let expanded = self.expanded_paths.clone();
        let watcher = self.watcher.clone();
        cx.spawn(async move |this, cx| {
            let root_for_load = root.clone();
            let entries = cx
                .background_executor()
                .spawn(async move {
                    if let Some(watcher) = watcher {
                        watcher.lock().sync(&expanded, generation);
                    }
                    build_visible_entries(&root_for_load, &expanded)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.load_generation == generation
                    && this.root.as_deref() == Some(root.as_path())
                    && *this.entries != entries
                {
                    this.entries = Arc::new(entries);
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

/// Watches only expanded directories, avoiding recursive watches over large
/// ignored trees. All subscription changes and directory reads run off the UI.
struct TreeWatcher {
    watcher: notify::RecommendedWatcher,
    watched: HashSet<PathBuf>,
    parent: Option<PathBuf>,
    generation: u64,
    subscriptions: Arc<Mutex<WatchSubscriptions>>,
}

#[derive(Default)]
struct WatchSubscriptions {
    // Retain both spellings because removed paths cannot be canonicalized.
    paths: HashMap<PathBuf, PathBuf>,
    invalidated: HashSet<PathBuf>,
}

impl WatchSubscriptions {
    fn invalidate(&mut self, event: &notify::Result<notify::Event>) {
        let reset_all = match event {
            Err(_) => true,
            Ok(event) => {
                event.need_rescan() || matches!(event.kind, EventKind::Any | EventKind::Other)
            }
        };
        let removed_or_moved = event.as_ref().is_ok_and(|event| {
            matches!(
                event.kind,
                EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_))
            )
        });
        if !reset_all && !removed_or_moved {
            return;
        }
        for (requested, canonical) in &self.paths {
            if reset_all
                || event.as_ref().is_ok_and(|event| {
                    event.paths.is_empty()
                        || event
                            .paths
                            .iter()
                            .any(|path| requested.starts_with(path) || canonical.starts_with(path))
                })
            {
                self.invalidated.insert(requested.clone());
            }
        }
    }
}

impl TreeWatcher {
    fn new(root: &Path, mut handler: impl notify::EventHandler) -> notify::Result<Self> {
        let subscriptions = Arc::new(Mutex::new(WatchSubscriptions::default()));
        let event_subscriptions = subscriptions.clone();
        let mut watcher = notify::recommended_watcher(move |event| {
            // Keep invalidation separate from the bounded wake queue: coalescing
            // wakeups must not discard a directory's removal/move notification.
            event_subscriptions.lock().invalidate(&event);
            handler.handle_event(event);
        })?;
        let parent = root.parent().map(Path::to_path_buf);
        if let Some(parent) = &parent
            && let Err(error) = watcher.watch(parent, RecursiveMode::NonRecursive)
        {
            log::warn!(
                "Cannot watch file tree parent {}: {error}",
                parent.display()
            );
        }
        let mut this = Self {
            watcher,
            watched: HashSet::new(),
            parent,
            generation: 0,
            subscriptions,
        };
        this.sync(&HashSet::from([root.to_path_buf()]), 0);
        Ok(this)
    }

    fn sync(&mut self, expanded: &HashSet<PathBuf>, generation: u64) {
        // A superseded background scan must not remove newer subscriptions.
        if generation < self.generation {
            return;
        }
        self.generation = generation;
        let wanted: HashSet<_> = expanded
            .iter()
            .filter(|path| path.is_dir())
            .cloned()
            .collect();
        let invalidated = std::mem::take(&mut self.subscriptions.lock().invalidated);
        let stale: Vec<_> = self
            .watched
            .iter()
            .filter(|path| !wanted.contains(*path) || invalidated.contains(*path))
            .cloned()
            .collect();
        for path in stale {
            self.subscriptions.lock().paths.remove(&path);
            // Do not hold the callback's mutex while calling notify: some
            // backends stop/join their event thread while changing watches.
            let _ = self.watcher.unwatch(&path);
            self.watched.remove(&path);
        }
        for path in wanted
            .difference(&self.watched)
            .cloned()
            .collect::<Vec<_>>()
        {
            if self.parent.as_ref() == Some(&path) {
                continue;
            }
            self.subscriptions
                .lock()
                .paths
                .insert(path.clone(), path.canonicalize().unwrap_or(path.clone()));
            match self.watcher.watch(&path, RecursiveMode::NonRecursive) {
                Ok(()) => {
                    self.watched.insert(path);
                }
                Err(error) => {
                    self.subscriptions.lock().paths.remove(&path);
                    log::warn!(
                        "Cannot watch file tree directory {}: {error}",
                        path.display()
                    );
                }
            }
        }
    }
}

fn tree_event_needs_refresh(event: &notify::Result<notify::Event>, root: &Path) -> bool {
    tree_event_needs_refresh_on_platform(event, root, cfg!(target_os = "windows"))
}

fn tree_event_needs_refresh_on_platform(
    event: &notify::Result<notify::Event>,
    root: &Path,
    windows: bool,
) -> bool {
    let Ok(event) = event else {
        return true;
    };
    if event.need_rescan() {
        return true;
    }
    // notify 7 maps Windows FILE_ACTION_MODIFIED (including log writes) to Any.
    // Add/remove/rename have distinct events, so this need not rebuild the tree.
    if windows && event.kind == EventKind::Modify(ModifyKind::Any) {
        return false;
    }
    let structural = matches!(
        event.kind,
        EventKind::Any
            | EventKind::Other
            | EventKind::Create(_)
            | EventKind::Remove(_)
            | EventKind::Modify(ModifyKind::Name(_) | ModifyKind::Any | ModifyKind::Other)
    );
    structural && (event.paths.is_empty() || event.paths.iter().any(|path| path.starts_with(root)))
}

fn build_visible_entries(root: &Path, expanded: &HashSet<PathBuf>) -> Vec<FileEntry> {
    let mut entries = root_placeholder_entry(root);
    entries[0].is_expanded = expanded.contains(root);
    if !entries[0].is_expanded {
        return entries;
    }
    // Iterative preorder traversal visits only expanded branches and cannot
    // overflow the stack on a deeply nested tree.
    let mut pending: Vec<_> = build_entries(root, 1, false).into_iter().rev().collect();
    while let Some(mut entry) = pending.pop() {
        entry.is_expanded = entry.is_dir && expanded.contains(&entry.path);
        if entry.is_expanded {
            pending.extend(
                build_entries(&entry.path, entry.depth + 1, false)
                    .into_iter()
                    .rev(),
            );
        }
        entries.push(entry);
    }
    entries
}

/// Build the visible tree starting at `root` itself. The root row is shown as
/// an expanded directory so the sidebar has a clear parent label and can be
/// collapsed/expanded like any other folder.
#[cfg(test)]
fn build_root_entries(root: &Path) -> Vec<FileEntry> {
    let mut entries = root_placeholder_entry(root);
    entries.extend(build_entries(root, 1, false));
    entries
}

fn root_placeholder_entry(root: &Path) -> Vec<FileEntry> {
    let name = root
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| root.display().to_string());
    vec![FileEntry {
        path: root.to_path_buf(),
        name,
        depth: 0,
        is_dir: true,
        is_expanded: true,
    }]
}

fn remove_descendants(entries: &mut Vec<FileEntry>, parent_index: usize) {
    let depth = entries[parent_index].depth;
    let remove_start = parent_index + 1;
    let remove_end = entries[remove_start..]
        .iter()
        .position(|entry| entry.depth <= depth)
        .map(|rel| remove_start + rel)
        .unwrap_or(entries.len());
    entries.drain(remove_start..remove_end);
}

fn row_has_open_button(entry: &FileEntry) -> bool {
    !entry.is_dir
}

fn render_file_icon(icon: FileIcon, size: gpui::Pixels, color: gpui::Hsla) -> gpui::AnyElement {
    match icon {
        FileIcon::Svg(svg_path) => svg()
            .path(svg_path)
            .size(size)
            .flex_shrink_0()
            .text_color(color)
            .into_any_element(),
        FileIcon::Glyph(glyph) => div()
            .flex()
            .items_center()
            .justify_center()
            .size(size)
            .flex_shrink_0()
            .font_family(FILE_ICON_FONT_FAMILY)
            .text_size(size)
            .line_height(size)
            .text_color(color)
            .child(SharedString::from(glyph.to_string()))
            .into_any_element(),
    }
}

/// Build a flat entry list for `dir` at `depth`. Only one level deep
/// (children of expanded dirs are inserted lazily by `toggle_dir`).
fn build_entries(dir: &Path, depth: usize, _expand_root: bool) -> Vec<FileEntry> {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut dirs: Vec<FileEntry> = Vec::new();
    let mut files: Vec<FileEntry> = Vec::new();

    for entry in read_dir.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        // Skip hidden files/dirs (dot-prefixed).
        if name.starts_with('.') {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let is_dir = file_type.is_dir();
        let fe = FileEntry {
            path,
            name,
            depth,
            is_dir,
            is_expanded: false,
        };
        if is_dir {
            dirs.push(fe);
        } else {
            files.push(fe);
        }
    }

    dirs.sort_by_key(|a| a.name.to_lowercase());
    files.sort_by_key(|a| a.name.to_lowercase());

    let mut result = Vec::new();
    result.extend(dirs);
    result.extend(files);

    result
}

impl Render for FileTreeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rendered_menu = self.context_menu.as_ref().map(|state| {
            let element = state.menu.update(cx, |menu, cx| {
                let mut element = menu.render(window, cx).into_any_element();
                // The pinned component exposes a stateful Div as its render root.
                // Refine that surface only; retain its menu items, focus path,
                // keyboard handlers and dismissal behavior intact.
                let root = element
                    .downcast_mut::<gpui::Stateful<gpui::Div>>()
                    .expect("gpui-component 0.7 PopupMenu renders a stateful Div");
                root.style().box_shadow = Some(Vec::new());
                let theme = cx.theme();
                let fill = theme
                    .background
                    .blend(
                        theme
                            .foreground
                            .opacity(if theme.is_dark() { 0.04 } else { 0.025 }),
                    );
                root.style().background = Some(gpui::Hsla { a: 1.0, ..fill }.into());
                element
            });
            (state.position, element)
        });
        let theme = cx.theme();

        if self.root.is_none() {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(theme.muted_foreground)
                        .font_family(theme.font_family.clone())
                        .child("No folder open"),
                )
                .into_any_element();
        }

        let active_path = self.active_path().map(Path::to_path_buf);
        let accent_bg = theme
            .primary
            .opacity(if theme.is_dark() { 0.12 } else { 0.075 });
        let hover_bg = theme
            .foreground
            .opacity(if theme.is_dark() { 0.046 } else { 0.026 });

        let entries = self.entries.clone();
        let entry_count = entries.len();
        let weak = cx.weak_entity();
        let list_theme = theme.clone();
        let list = uniform_list("file-tree-rows", entry_count, move |range, _window, _cx| {
            range
                .map(|idx| {
                    let entry = &entries[idx];
                    let path = entry.path.clone();
                    let name: SharedString = entry.name.clone().into();
                    let depth = entry.depth;
                    let is_dir = entry.is_dir;
                    let is_expanded = entry.is_expanded;
                    let is_active = active_path.as_deref() == Some(entry.path.as_path());

                    let indent = INDENT_PER_LEVEL * depth as f32 + 8.0;

                    let disclosure_icon = if is_dir {
                        Some(if is_expanded {
                            "phosphor/caret-down.svg"
                        } else {
                            "phosphor/caret-right.svg"
                        })
                    } else {
                        None
                    };

                    let icon = icon_for_path(&path, is_dir, is_expanded);

                    let icon_color = if is_dir {
                        list_theme.primary
                    } else {
                        list_theme.muted_foreground
                    };

                    let text_color = list_theme.foreground;

                    let row_bg = if is_active {
                        accent_bg
                    } else {
                        list_theme.transparent
                    };

                    let weak = weak.clone();
                    let weak_for_menu = weak.clone();
                    let path_for_menu = path.clone();
                    div()
                        .id(("file-row", idx))
                        .debug_selector(move || format!("file-tree-row-{idx}"))
                        .h(px(ROW_HEIGHT))
                        .w_full()
                        .flex()
                        .items_center()
                        .pl(px(indent))
                        .pr(px(8.0))
                        .gap(px(5.0))
                        .bg(row_bg)
                        .cursor_pointer()
                        .hover(move |s| {
                            if is_active {
                                s.bg(accent_bg)
                            } else {
                                s.bg(hover_bg)
                            }
                        })
                        .child(if let Some(disclosure_icon) = disclosure_icon {
                            svg()
                                .path(disclosure_icon)
                                .size(ui_icon_px(&list_theme, 10.0))
                                .flex_shrink_0()
                                .text_color(list_theme.muted_foreground)
                                .into_any_element()
                        } else {
                            div().w(px(10.0)).flex_shrink_0().into_any_element()
                        })
                        .child(render_file_icon(
                            icon,
                            ui_icon_px(&list_theme, ICON_SIZE),
                            icon_color,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(12.0))
                                .text_color(text_color)
                                .font_family(list_theme.font_family.clone())
                                .child(name),
                        )
                        .child({
                            if !row_has_open_button(entry) {
                                div().size(px(ICON_SIZE)).flex_shrink_0().into_any_element()
                            } else {
                                let path = path.clone();
                                let weak = weak.clone();
                                div()
                                    .id(("file-open-in-editor", idx))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(ui_icon_px(&list_theme, ICON_SIZE))
                                    .flex_shrink_0()
                                    .cursor_pointer()
                                    .opacity(if is_active { 1.0 } else { 0.0 })
                                    .hover(move |s| s.opacity(1.0))
                                    .tooltip(move |window, cx| {
                                        Tooltip::new("Open in Editor Tab").build(window, cx)
                                    })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        move |_: &MouseDownEvent, _window, cx| {
                                            cx.stop_propagation();
                                            if let Some(view) = weak.upgrade() {
                                                view.update(cx, |_this, cx| {
                                                    cx.emit(OpenFileInEditorTab {
                                                        path: path.clone(),
                                                    });
                                                });
                                            }
                                        },
                                    )
                                    .child(
                                        svg()
                                            .path("phosphor/arrow-square-out.svg")
                                            .size(ui_icon_px(&list_theme, ICON_SIZE))
                                            .text_color(list_theme.muted_foreground),
                                    )
                                    .into_any_element()
                            }
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            move |event: &MouseDownEvent, _window, cx| {
                                if let Some(view) = weak.upgrade() {
                                    view.update(cx, |this, cx| {
                                        if is_dir {
                                            this.toggle_dir(&path, cx);
                                        } else {
                                            cx.emit(OpenFile {
                                                path: path.clone(),
                                                keep_open: event.click_count == 2,
                                            });
                                        }
                                    });
                                }
                            },
                        )
                        .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                            cx.stop_propagation();
                            if let Some(tree) = weak_for_menu.upgrade() {
                                tree.update(cx, |tree, cx| {
                                    tree.open_context_menu(
                                        path_for_menu.clone(),
                                        event.position,
                                        window,
                                        cx,
                                    );
                                });
                            }
                        })
                        .into_any_element()
                })
                .collect()
        })
        .track_scroll(&self.scroll)
        .flex_1();

        div()
            .id("file-tree")
            // Deferred context menus inherit the trigger container's font.
            .font_family(theme.font_family.clone())
            .size_full()
            .flex()
            .flex_col()
            .occlude()
            .child(list)
            .when_some(rendered_menu, |root, (position, menu)| {
                root.child(
                    deferred(
                        anchored().child(
                            div()
                                .w(window.viewport_size().width)
                                .h(window.viewport_size().height)
                                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                                .child(
                                    anchored()
                                        .position(position)
                                        .snap_to_window_with_margin(px(8.0))
                                        .child(
                                            div()
                                                .debug_selector(|| "file-copy-menu-surface".into())
                                                .occlude()
                                                .child(menu),
                                        ),
                                ),
                        ),
                    )
                    // The paired gpui-base 0.7 popup layer uses priority 100.
                    .with_priority(100),
                )
            })
            .into_any_element()
    }
}

/// Text copied by the "Copy Path" row action: the full path as displayed.
fn copy_path_text(path: &Path) -> String {
    path.display().to_string()
}

/// Text copied by the "Copy Name" row action: the final path component.
///
/// Filesystem roots such as `/` or `C:\` have no final component, so the
/// displayed root name is copied instead, matching the tree row.
fn copy_name_text(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

/// Build the context menu for a file/directory row in the file tree.
fn build_file_context_menu(menu: PopupMenu, path: PathBuf) -> PopupMenu {
    use gpui_component::menu::PopupMenuItem;

    let path_for_copy = path.clone();
    let copy_path_item = PopupMenuItem::new("Copy Path").on_click(move |_event, _window, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string(copy_path_text(&path_for_copy)));
    });

    let path_for_copy_name = path;
    let copy_name_item = PopupMenuItem::new("Copy Name").on_click(move |_event, _window, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string(copy_name_text(
            &path_for_copy_name,
        )));
    });

    menu.min_w(px(160.0))
        .item(copy_path_item)
        .item(copy_name_item)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct FileTreeTestView(gpui::Entity<FileTreeView>);

    impl Render for FileTreeTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.0.clone())
        }
    }

    #[gpui::test]
    fn tree_scroll_survives_switching_to_another_tool(cx: &mut gpui::TestAppContext) {
        struct Fixture {
            tree: Entity<FileTreeView>,
            visible: bool,
        }
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .size_full()
                    .when(self.visible, |root| root.child(self.tree.clone()))
            }
        }
        cx.update(gpui_component::init);
        let dir = tempfile::tempdir().unwrap();
        let tree = cx.new(|_| FileTreeView::new());
        tree.update(cx, |tree, _| {
            tree.root = Some(dir.path().to_path_buf());
            tree.entries = Arc::new(
                (0..80)
                    .map(|i| FileEntry {
                        path: dir.path().join(format!("item-{i}.txt")),
                        name: format!("item-{i}.txt"),
                        depth: 1,
                        is_dir: false,
                        is_expanded: false,
                    })
                    .collect(),
            );
        });
        let (view, cx) = cx.add_window_view(|_, _| Fixture {
            tree: tree.clone(),
            visible: true,
        });
        cx.simulate_resize(gpui::size(px(300.0), px(200.0)));
        tree.update(cx, |tree, cx| {
            tree.scroll.scroll_to_item(50, gpui::ScrollStrategy::Top);
            cx.notify();
        });
        cx.run_until_parked();
        let before = cx.debug_bounds("file-tree-row-50").unwrap();
        view.update(cx, |view, cx| {
            view.visible = false;
            cx.notify();
        });
        assert!(cx.debug_bounds("file-tree-row-50").is_none());
        view.update(cx, |view, cx| {
            view.visible = true;
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(cx.debug_bounds("file-tree-row-50").unwrap(), before);
    }

    #[gpui::test]
    fn row_copy_menu_targets_the_clicked_entry_without_opening_or_collapsing(
        cx: &mut gpui::TestAppContext,
    ) {
        use std::{cell::Cell, rc::Rc};
        cx.update(gpui_component::init);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes with spaces.txt");
        fs::write(&file, "notes").unwrap();
        let tree = cx.new(|_| FileTreeView::new());
        tree.update(cx, |tree, _| {
            tree.root = Some(dir.path().to_path_buf());
            tree.entries = Arc::new(build_root_entries(dir.path()));
            tree.active_path = Some(file.clone());
        });
        let (_, cx) = cx.add_window_view(|_, _| FileTreeTestView(tree.clone()));
        let previous_focus = cx.update(|window, cx| {
            let handle = cx.focus_handle();
            handle.focus(window, cx);
            handle
        });
        let opened = Rc::new(Cell::new(0));
        cx.update(|_, cx| {
            let opened = opened.clone();
            cx.subscribe(&tree, move |_, _: &OpenFile, _| {
                opened.set(opened.get() + 1);
            })
            .detach();
        });
        for (row, keys, expected) in [
            ("file-tree-row-1", "down enter", file.display().to_string()),
            (
                "file-tree-row-1",
                "down down enter",
                "notes with spaces.txt".to_string(),
            ),
            (
                "file-tree-row-0",
                "down down enter",
                copy_name_text(dir.path()),
            ),
        ] {
            let bounds = cx.debug_bounds(row).unwrap();
            cx.simulate_mouse_down(bounds.center(), MouseButton::Right, Default::default());
            cx.simulate_mouse_up(bounds.center(), MouseButton::Right, Default::default());
            assert!(cx.debug_bounds("file-copy-menu-surface").is_some());
            cx.simulate_keystrokes(keys);
            cx.run_until_parked();
            let copied = cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()));
            assert_eq!(copied.as_deref(), Some(expected.as_str()));
            assert_eq!(opened.get(), 0, "copy must not preview a file");
            assert!(cx.debug_bounds("file-copy-menu-surface").is_none());
            assert!(cx.update(|window, _| previous_focus.is_focused(window)));
            tree.read_with(cx, |tree, _| {
                assert!(
                    tree.entries[0].is_expanded,
                    "copy must not collapse folders"
                );
                assert_eq!(tree.active_path(), Some(file.as_path()));
            });
        }
        let bounds = cx.debug_bounds("file-tree-row-0").unwrap();
        cx.simulate_mouse_down(bounds.center(), MouseButton::Right, Default::default());
        cx.simulate_mouse_up(bounds.center(), MouseButton::Right, Default::default());
        cx.simulate_keystrokes("escape");
        assert!(cx.debug_bounds("file-copy-menu-surface").is_none());
        assert!(cx.update(|window, _| previous_focus.is_focused(window)));
    }

    #[gpui::test]
    fn hiding_tree_dismisses_menu_without_stealing_new_tool_focus(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let tree = cx.new(|_| FileTreeView::new());
        let (_, cx) = cx.add_window_view(|_, _| FileTreeTestView(tree.clone()));
        let original = cx.update(|window, cx| {
            let handle = cx.focus_handle();
            handle.focus(window, cx);
            handle
        });
        for focus_already_moved in [false, true] {
            cx.update(|window, cx| {
                original.focus(window, cx);
                tree.update(cx, |tree, cx| {
                    tree.open_context_menu(
                        PathBuf::from("notes.txt"),
                        Point::default(),
                        window,
                        cx,
                    );
                });
                let next_tool = cx.focus_handle();
                if focus_already_moved {
                    next_tool.focus(window, cx);
                }
                tree.update(cx, |tree, cx| tree.dismiss_context_menu(window, cx));
                assert!(tree.read(cx).context_menu.is_none());
                assert!(if focus_already_moved {
                    next_tool.is_focused(window)
                } else {
                    original.is_focused(window)
                });
            });
        }
    }

    #[gpui::test]
    fn double_clicking_tree_file_keeps_preview_before_browsing_another(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_component::init);
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.txt");
        let second = dir.path().join("second.txt");
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();
        let editor = cx.new(|cx| crate::editor_view::EditorView::new_with_font_size(14.0, cx));
        let tree = cx.new(|_| FileTreeView::new());
        tree.update(cx, |tree, _| {
            tree.root = Some(dir.path().to_path_buf());
            tree.entries = Arc::new(build_root_entries(dir.path()))
        });
        let editor_for_events = editor.clone();
        let _observer = cx.new(|cx: &mut Context<Vec<bool>>| {
            cx.subscribe(&tree, move |events, _, event: &OpenFile, cx| {
                events.push(event.keep_open);
                editor_for_events.update(cx, |editor, cx| {
                    if event.keep_open {
                        editor.open_file(event.path.clone(), cx);
                    } else {
                        editor.open_temporary_file(event.path.clone(), cx);
                    }
                });
            })
            .detach();
            Vec::new()
        });
        let (_, cx) = cx.add_window_view(|_, _| FileTreeTestView(tree));
        cx.run_until_parked();
        let row = cx.debug_bounds("file-tree-row-1").unwrap();
        cx.simulate_click(row.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        editor.read_with(cx, |editor, _| assert!(editor.is_temporary_tab(0)));
        cx.simulate_event(MouseDownEvent {
            position: row.center(),
            button: MouseButton::Left,
            click_count: 2,
            modifiers: gpui::Modifiers::default(),
            first_mouse: false,
        });
        cx.simulate_mouse_up(row.center(), MouseButton::Left, gpui::Modifiers::default());
        cx.run_until_parked();
        _observer.read_with(cx, |events, _| assert_eq!(events, &[false, true]));
        editor.read_with(cx, |editor, _| assert!(!editor.is_temporary_tab(0)));
        let second_row = cx.debug_bounds("file-tree-row-2").unwrap();
        cx.simulate_click(second_row.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        editor.read_with(cx, |editor, _| {
            assert_eq!(editor.tab_count(), 2);
            assert!(!editor.is_temporary_tab(0));
            assert!(editor.is_temporary_tab(1));
        });
    }

    #[test]
    fn windows_content_notifications_do_not_refresh_but_structure_changes_do() {
        use notify::event::{CreateKind, Flag, RemoveKind, RenameMode};
        let root = Path::new("/project");
        let modified =
            Ok(notify::Event::new(EventKind::Modify(ModifyKind::Any))
                .add_path(root.join("app.log")));
        assert!(!tree_event_needs_refresh_on_platform(&modified, root, true));
        assert!(tree_event_needs_refresh_on_platform(&modified, root, false));
        for kind in [
            EventKind::Create(CreateKind::File),
            EventKind::Remove(RemoveKind::File),
            EventKind::Modify(ModifyKind::Name(RenameMode::From)),
            EventKind::Modify(ModifyKind::Name(RenameMode::To)),
        ] {
            let event = Ok(notify::Event::new(kind).add_path(root.join("file.txt")));
            assert!(tree_event_needs_refresh_on_platform(&event, root, true));
        }
        let rescan =
            Ok(notify::Event::new(EventKind::Modify(ModifyKind::Any)).set_flag(Flag::Rescan));
        assert!(tree_event_needs_refresh_on_platform(&rescan, root, true));
    }

    #[test]
    fn removal_and_move_invalidate_only_affected_directory_subscriptions() {
        use notify::event::{RemoveKind, RenameMode};
        let root = PathBuf::from("/alias/project");
        let src = root.join("src");
        let nested = src.join("nested");
        let sibling = root.join("other");
        let paths = HashMap::from([
            (root.clone(), PathBuf::from("/real/project")),
            (src.clone(), PathBuf::from("/real/project/src")),
            (nested.clone(), PathBuf::from("/real/project/src/nested")),
            (sibling.clone(), PathBuf::from("/real/project/other")),
        ]);
        for kind in [
            EventKind::Remove(RemoveKind::Folder),
            EventKind::Modify(ModifyKind::Name(RenameMode::From)),
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
        ] {
            let mut subscriptions = WatchSubscriptions {
                paths: paths.clone(),
                ..Default::default()
            };
            subscriptions.invalidate(&Ok(
                notify::Event::new(kind).add_path(PathBuf::from("/real/project/src"))
            ));
            // The removal survives later coalesced create/content notifications.
            subscriptions.invalidate(&Ok(notify::Event::new(EventKind::Create(
                notify::event::CreateKind::Folder,
            ))
            .add_path(src.clone())));
            assert_eq!(
                subscriptions.invalidated,
                HashSet::from([src.clone(), nested.clone()])
            );
        }
        let mut subscriptions = WatchSubscriptions {
            paths,
            ..Default::default()
        };
        subscriptions
            .invalidate(&Ok(notify::Event::new(EventKind::Remove(RemoveKind::File))
                .add_path(src.join("file.txt"))));
        assert!(subscriptions.invalidated.is_empty());
        subscriptions.invalidate(&Err(notify::Error::generic("lost events")));
        assert_eq!(
            subscriptions.invalidated,
            HashSet::from([root, src, nested, sibling])
        );
    }

    #[test]
    fn replaced_directory_is_rewatched_even_when_its_path_still_exists() {
        use notify::event::RemoveKind;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let src = root.join("src");
        fs::create_dir(&src).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let target = src.join("after-replacement.txt");
        let event_target = target.clone();
        let mut watcher = TreeWatcher::new(&root, move |event: notify::Result<notify::Event>| {
            if event.is_ok_and(|event| event.paths.contains(&event_target)) {
                let _ = tx.send(());
            }
        })
        .unwrap();
        let expanded = HashSet::from([root.clone(), src.clone()]);
        watcher.sync(&expanded, 2);
        // Simulate inotify dropping the original subscription. Keep the old
        // inode alive so recreating src cannot accidentally reuse its identity.
        watcher.watcher.unwatch(&src).unwrap();
        fs::rename(&src, root.join("old-src")).unwrap();
        fs::create_dir(&src).unwrap();
        watcher
            .subscriptions
            .lock()
            .invalidate(&Ok(notify::Event::new(EventKind::Remove(
                RemoveKind::Folder,
            ))
            .add_path(src.clone())));
        watcher.sync(&expanded, 1);
        assert!(watcher.subscriptions.lock().invalidated.contains(&src));
        watcher.sync(&expanded, 3);
        assert!(!watcher.subscriptions.lock().invalidated.contains(&src));
        fs::write(&target, "new directory subscription").unwrap();
        rx.recv_timeout(Duration::from_secs(10))
            .expect("replacement directory must report subsequent file creation");
    }

    #[test]
    fn structural_events_refresh_but_content_and_unrelated_events_do_not() {
        use notify::event::{CreateKind, DataChange, Flag, RenameMode};
        let root = Path::new("/project");
        let event = |kind, path| Ok(notify::Event::new(kind).add_path(PathBuf::from(path)));
        assert!(tree_event_needs_refresh(
            &event(EventKind::Create(CreateKind::File), "/project/new.txt"),
            root
        ));
        assert!(tree_event_needs_refresh(
            &event(
                EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
                "/project/renamed.txt"
            ),
            root
        ));
        assert!(!tree_event_needs_refresh(
            &event(
                EventKind::Modify(ModifyKind::Data(DataChange::Content)),
                "/project/file.txt"
            ),
            root
        ));
        assert!(!tree_event_needs_refresh(
            &event(EventKind::Create(CreateKind::File), "/sibling/file.txt"),
            root
        ));
        let rescan = Ok(notify::Event::new(EventKind::Other).set_flag(Flag::Rescan));
        assert!(tree_event_needs_refresh(&rescan, root));
        assert!(tree_event_needs_refresh(
            &Err(notify::Error::generic("watcher error")),
            root
        ));
    }

    #[gpui::test]
    fn refresh_tracks_create_rename_delete_and_keeps_expansion_and_active_file(
        cx: &mut gpui::TestAppContext,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let src = root.join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("keep.txt"), "keep").unwrap();
        fs::write(root.join("rename.txt"), "rename").unwrap();
        fs::write(root.join("delete.txt"), "delete").unwrap();
        fs::write(root.join(".gitignore"), "hidden").unwrap();
        let view = cx.new(|_| FileTreeView::new());
        view.update(cx, |view, cx| {
            view.root = Some(root.clone());
            view.expanded_paths = HashSet::from([root.clone(), src.clone()]);
            view.active_path = Some(src.join("keep.txt"));
            view.refresh(cx);
        });
        cx.run_until_parked();
        fs::write(src.join("new.txt"), "new").unwrap();
        fs::rename(root.join("rename.txt"), root.join("renamed.txt")).unwrap();
        fs::remove_file(root.join("delete.txt")).unwrap();
        view.update(cx, |view, cx| view.refresh(cx));
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            let paths: Vec<_> = view
                .entries
                .iter()
                .map(|entry| entry.path.clone())
                .collect();
            assert!(paths.contains(&src.join("new.txt")));
            assert!(paths.contains(&root.join("renamed.txt")));
            assert!(!paths.contains(&root.join("rename.txt")));
            assert!(!paths.contains(&root.join("delete.txt")));
            assert!(!paths.contains(&root.join(".gitignore")));
            assert!(
                view.entries
                    .iter()
                    .find(|entry| entry.path == src)
                    .unwrap()
                    .is_expanded
            );
            assert_eq!(view.active_path.as_ref(), Some(&src.join("keep.txt")));
        });
        view.update(cx, |view, cx| {
            view.refresh(cx);
            view.toggle_dir(&src, cx);
        });
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(
                !view
                    .entries
                    .iter()
                    .any(|entry| entry.path == src.join("keep.txt"))
            );
            assert!(!view.expanded_paths.contains(&src));
        });
    }

    #[gpui::test]
    fn stale_scan_cannot_replace_a_new_root(cx: &mut gpui::TestAppContext) {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        fs::write(first.path().join("old.txt"), "old").unwrap();
        fs::write(second.path().join("new.txt"), "new").unwrap();
        let view = cx.new(|_| FileTreeView::new());
        view.update(cx, |view, cx| {
            view.root = Some(first.path().to_path_buf());
            view.expanded_paths.insert(first.path().to_path_buf());
            view.refresh(cx);
            view.root = Some(second.path().to_path_buf());
            view.expanded_paths = HashSet::from([second.path().to_path_buf()]);
            view.refresh(cx);
        });
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(view.entries.iter().any(|entry| entry.name == "new.txt"));
            assert!(!view.entries.iter().any(|entry| entry.name == "old.txt"));
        });
    }

    #[gpui::test]
    fn refresh_keeps_unchanged_cache_and_does_not_reopen_collapsed_root(
        cx: &mut gpui::TestAppContext,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        fs::write(root.join("keep.txt"), "keep").unwrap();
        let view = cx.new(|_| FileTreeView::new());
        view.update(cx, |view, cx| {
            view.root = Some(root.clone());
            view.expanded_paths.insert(root.clone());
            view.refresh(cx);
        });
        cx.run_until_parked();
        let before = view.read_with(cx, |view, _| view.entries.clone());
        view.update(cx, |view, cx| view.refresh(cx));
        cx.run_until_parked();
        view.read_with(cx, |view, _| assert!(Arc::ptr_eq(&before, &view.entries)));
        view.update(cx, |view, cx| view.toggle_dir(&root, cx));
        fs::write(root.join("new.txt"), "new").unwrap();
        view.update(cx, |view, cx| view.refresh(cx));
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert_eq!(view.entries.len(), 1);
            assert!(!view.entries[0].is_expanded);
        });
    }

    #[gpui::test]
    fn switching_roots_and_dropping_view_release_watchers(cx: &mut gpui::TestAppContext) {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let view = cx.new(|_| FileTreeView::new());
        view.update(cx, |view, cx| view.set_root(first.path().to_path_buf(), cx));
        cx.run_until_parked();
        let first_watcher = view.read_with(cx, |view, _| {
            Arc::downgrade(view.watcher.as_ref().expect("watcher installed"))
        });
        view.update(cx, |view, cx| {
            view.set_root(second.path().to_path_buf(), cx)
        });
        cx.run_until_parked();
        assert!(first_watcher.upgrade().is_none());
        let second_watcher = view.read_with(cx, |view, _| {
            Arc::downgrade(view.watcher.as_ref().expect("new watcher installed"))
        });
        drop(view);
        // Flush GPUI's deferred entity release before checking watcher teardown.
        cx.update(|_| {});
        cx.run_until_parked();
        assert!(second_watcher.upgrade().is_none());
    }

    #[test]
    fn native_watcher_observes_changes_and_keeps_newer_subscriptions() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let src = root.join("src");
        fs::create_dir(&src).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let event_root = root.clone();
        let mut watcher = TreeWatcher::new(&root, move |event| {
            if tree_event_needs_refresh(&event, &event_root) {
                let _ = tx.send(());
            }
        })
        .unwrap();
        watcher.sync(&HashSet::from([root.clone(), src.clone()]), 2);
        watcher.sync(&HashSet::from([root.clone()]), 1);
        assert!(watcher.watched.contains(&src));
        fs::write(src.join("new.txt"), "new").unwrap();
        rx.recv_timeout(Duration::from_secs(10))
            .expect("native watcher must report structure changes");
        watcher.sync(&HashSet::from([root.clone()]), 3);
        assert!(!watcher.watched.contains(&src));
    }

    fn temp_tree() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "con-file-tree-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("README.md"), "readme").unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        root
    }

    fn temp_ordered_tree() -> PathBuf {
        let root = temp_tree();
        fs::create_dir_all(root.join("Alpha")).unwrap();
        fs::create_dir_all(root.join("beta")).unwrap();
        fs::write(root.join("zeta.txt"), "z").unwrap();
        fs::write(root.join("apple.txt"), "a").unwrap();
        fs::write(root.join(".hidden"), "hidden").unwrap();
        root
    }

    #[gpui::test]
    fn file_type_glyphs_use_the_bundled_font(_cx: &mut gpui::TestAppContext) {
        let mut element =
            render_file_icon(FileIcon::Glyph('\u{e68b}'), px(ICON_SIZE), gpui::black());
        let text = element.downcast_mut::<gpui::Div>().unwrap().text_style();
        assert_eq!(
            text.font_family.as_ref().map(|font| font.as_ref()),
            Some(FILE_ICON_FONT_FAMILY)
        );
    }

    #[test]
    fn root_directory_is_rendered_as_first_expanded_entry() {
        let root = temp_tree();
        let entries = build_root_entries(&root);

        assert_eq!(entries.first().unwrap().path, root);
        assert_eq!(entries.first().unwrap().depth, 0);
        assert!(entries.first().unwrap().is_dir);
        assert!(entries.first().unwrap().is_expanded);
    }

    #[test]
    fn root_children_start_at_depth_one() {
        let root = temp_tree();
        let entries = build_root_entries(&root);

        assert!(entries.iter().skip(1).all(|entry| entry.depth == 1));
        assert!(entries.iter().any(|entry| entry.name == "src"));
        assert!(entries.iter().any(|entry| entry.name == "README.md"));
    }

    #[test]
    fn build_entries_filters_hidden_entries() {
        let root = temp_ordered_tree();
        let entries = build_entries(&root, 1, false);

        assert!(!entries.iter().any(|entry| entry.name.starts_with('.')));
    }

    #[test]
    fn build_entries_sorts_dirs_first_then_files_case_insensitively() {
        let root = temp_ordered_tree();
        let entries = build_entries(&root, 1, false);
        let names = entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec!["Alpha", "beta", "src", "apple.txt", "README.md", "zeta.txt"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn build_entries_does_not_expand_directory_symlinks() {
        let root = temp_tree();
        std::os::unix::fs::symlink(root.join("src"), root.join("linked-src")).unwrap();

        let entries = build_entries(&root, 1, false);
        let linked = entries
            .iter()
            .find(|entry| entry.name == "linked-src")
            .expect("symlink should still be listed");

        assert!(!linked.is_dir);
    }

    #[test]
    fn remove_descendants_drops_nested_rows_until_next_sibling() {
        let root = PathBuf::from("/tmp/project");
        let src = root.join("src");
        let sibling = root.join("README.md");
        let mut entries = vec![
            FileEntry {
                path: root.clone(),
                name: "project".to_string(),
                depth: 0,
                is_dir: true,
                is_expanded: true,
            },
            FileEntry {
                path: src.clone(),
                name: "src".to_string(),
                depth: 1,
                is_dir: true,
                is_expanded: true,
            },
            FileEntry {
                path: src.join("main.rs"),
                name: "main.rs".to_string(),
                depth: 2,
                is_dir: false,
                is_expanded: false,
            },
            FileEntry {
                path: sibling.clone(),
                name: "README.md".to_string(),
                depth: 1,
                is_dir: false,
                is_expanded: false,
            },
        ];

        remove_descendants(&mut entries, 1);

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[1].path, src);
        assert_eq!(entries[2].path, sibling);
    }

    #[test]
    fn row_has_open_button_returns_true_only_for_files() {
        let root = temp_tree();
        let entries = build_root_entries(&root);

        for entry in entries {
            if entry.is_dir {
                assert!(
                    !row_has_open_button(&entry),
                    "Directory '{}' should not have open button",
                    entry.name
                );
            } else {
                assert!(
                    row_has_open_button(&entry),
                    "File '{}' should have open button",
                    entry.name
                );
            }
        }
    }

    #[test]
    fn copy_path_text_keeps_the_whole_path() {
        let path = Path::new("/project/src/main.rs");
        assert_eq!(copy_path_text(path), "/project/src/main.rs");
    }

    #[test]
    fn copy_name_text_keeps_the_final_component() {
        assert_eq!(copy_name_text(Path::new("/project/src/main.rs")), "main.rs");
        // A directory row copies the directory name, not its contents.
        assert_eq!(copy_name_text(Path::new("/project/src")), "src");
        // Filesystem roots have no final component; copy the displayed root
        // name (e.g. `/`) instead of an empty string.
        assert_eq!(copy_name_text(Path::new("/")), "/");
    }
}

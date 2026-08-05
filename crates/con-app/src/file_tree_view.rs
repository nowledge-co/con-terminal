//! File tree panel — shows the directory tree rooted at the active tab's cwd.
//!
//! Phase 1: read-only directory listing. Clicking a file emits `OpenFile`.
//! The tree root updates when `set_root` is called (driven by GhosttyCwdChanged
//! on the active tab).
//!
//! Visual rules
//! ---
//! - Row height: 24 px.
//! - Indent: 12 px per depth level.
//! - Icons: phosphor/folder.svg, phosphor/folder-open.svg, phosphor/file-text.svg.
//! - Active (open) file row gets a subtle accent bg.
//! - No borders — surface separation via bg opacity.

use gpui::{
    Context, EventEmitter, IntoElement, MouseButton, MouseDownEvent, ParentElement, Render,
    SharedString, Styled, Window, div, prelude::*, px, svg, uniform_list,
};
use gpui_component::{tooltip::Tooltip, ActiveTheme};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const ROW_HEIGHT: f32 = 24.0;
const INDENT_PER_LEVEL: f32 = 12.0;
const ICON_SIZE: f32 = 13.0;

/// A single entry in the flat file tree list.
#[derive(Clone)]
pub struct FileEntry {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub is_expanded: bool,
}

/// Emitted when the user clicks a file row.
pub struct OpenFile {
    pub path: PathBuf,
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
}

impl FileTreeView {
    pub fn new() -> Self {
        Self {
            root: None,
            entries: Arc::new(Vec::new()),
            active_path: None,
            load_generation: 0,
        }
    }

    /// Set the root directory and rebuild the entry list.
    pub fn set_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if self.root.as_deref() == Some(root.as_path()) {
            return;
        }
        self.load_generation = self.load_generation.wrapping_add(1);
        let generation = self.load_generation;
        self.root = Some(root.clone());
        self.entries = Arc::new(root_placeholder_entry(&root));
        cx.notify();
        Self::spawn_root_load(root, generation, cx);
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
        let depth = entry.depth;

        if expanded {
            let path = path.to_path_buf();
            let generation = self.load_generation;
            Self::spawn_children_load(path, depth, generation, cx);
        } else {
            remove_descendants(entries, idx);
        }
        cx.notify();
    }

    fn spawn_root_load(root: PathBuf, generation: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let root_for_load = root.clone();
            let entries = cx
                .background_executor()
                .spawn(async move { build_root_entries(&root_for_load) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.load_generation == generation
                    && this.root.as_deref() == Some(root.as_path())
                {
                    this.entries = Arc::new(entries);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn spawn_children_load(path: PathBuf, depth: usize, generation: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let path_for_load = path.clone();
            let children = cx
                .background_executor()
                .spawn(async move { build_entries(&path_for_load, depth + 1, false) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.load_generation != generation {
                    return;
                }
                let Some(idx) = this.entries.iter().position(|entry| entry.path == path) else {
                    return;
                };
                if !this.entries[idx].is_expanded {
                    return;
                }
                let entries = Arc::make_mut(&mut this.entries);
                remove_descendants(entries, idx);
                let insert_at = idx + 1;
                entries.splice(insert_at..insert_at, children);
                cx.notify();
            });
        })
        .detach();
    }
}

/// Build the visible tree starting at `root` itself. The root row is shown as
/// an expanded directory so the sidebar has a clear parent label and can be
/// collapsed/expanded like any other folder.
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

    dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    let mut result = Vec::new();
    result.extend(dirs);
    result.extend(files);

    result
}

impl Render for FileTreeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                        .text_color(theme.muted_foreground.opacity(0.5))
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

                    let icon = if is_dir {
                        if is_expanded {
                            "phosphor/folder-open.svg"
                        } else {
                            "phosphor/folder.svg"
                        }
                    } else {
                        "phosphor/file-text.svg"
                    };

                    let icon_color = if is_dir {
                        list_theme.primary.opacity(0.75)
                    } else {
                        list_theme.muted_foreground.opacity(0.80)
                    };

                    let text_color = if is_active {
                        list_theme.foreground
                    } else {
                        list_theme.foreground.opacity(0.85)
                    };

                    let row_bg = if is_active {
                        accent_bg
                    } else {
                        list_theme.transparent
                    };

                    let weak = weak.clone();
                    div()
                        .id(("file-row", idx))
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
                                .size(px(10.0))
                                .flex_shrink_0()
                                .text_color(list_theme.muted_foreground.opacity(0.62))
                                .into_any_element()
                        } else {
                            div().w(px(10.0)).flex_shrink_0().into_any_element()
                        })
                        .child(
                            svg()
                                .path(icon)
                                .size(px(ICON_SIZE))
                                .flex_shrink_0()
                                .text_color(icon_color),
                        )
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
                            if !row_has_open_button(&entry) {
                                div().size(px(ICON_SIZE)).flex_shrink_0().into_any_element()
                            } else {
                                let path = path.clone();
                                let weak = weak.clone();
                                div()
                                    .id(("file-open-in-editor", idx))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(ICON_SIZE))
                                    .flex_shrink_0()
                                    .cursor_pointer()
                                    .opacity(if is_active { 0.4 } else { 0.0 })
                                    .hover(move |s| s.opacity(1.0))
                                    .tooltip(move |window, cx| {
                                        Tooltip::new("Open in Editor Tab").build(window, cx)
                                    })
                                    .on_mouse_down(MouseButton::Left, move |_: &MouseDownEvent, _window, cx| {
                                        cx.stop_propagation();
                                        if let Some(view) = weak.upgrade() {
                                            view.update(cx, |_this, cx| {
                                                cx.emit(OpenFileInEditorTab { path: path.clone() });
                                            });
                                        }
                                    })
                                    .child(
                                        svg()
                                            .path("phosphor/arrow-square-out.svg")
                                            .size(px(ICON_SIZE))
                                            .text_color(list_theme.muted_foreground),
                                    )
                                    .into_any_element()
                            }
                        })
                        .on_mouse_down(MouseButton::Left, move |_: &MouseDownEvent, _window, cx| {
                            if let Some(view) = weak.upgrade() {
                                view.update(cx, |this, cx| {
                                    if is_dir {
                                        this.toggle_dir(&path, cx);
                                    } else {
                                        cx.emit(OpenFile { path: path.clone() });
                                    }
                                });
                            }
                        })
                        .into_any_element()
                })
                .collect()
        })
        .flex_1();

        div()
            .id("file-tree")
            .size_full()
            .flex()
            .flex_col()
            .occlude()
            .child(list)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

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
}

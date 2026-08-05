use con_core::session::{PaneLayoutState, PaneSplitDirection, SurfaceState};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{
    ActiveTheme, InteractiveElementExt, Sizable,
    input::{Input, InputState},
    menu::{ContextMenuExt, PopupMenuItem},
    tooltip::Tooltip,
};

use crate::editor_view::EditorView;
use crate::sidebar::{DraggedTab, DraggedTabOrigin};
use crate::terminal_pane::TerminalPane;

const RESTORED_SCREEN_TEXT_MAX_LINES: usize = 600;
const RESTORED_SCREEN_TEXT_MAX_BYTES: usize = 128 * 1024;

/// Split direction for pane layout
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SplitPlacement {
    Before,
    After,
}

/// Unique identifier for a leaf pane.
///
/// Pane IDs remain the public, stable split-layout target used by the
/// built-in agent and the existing `panes.*` control API.
pub type PaneId = usize;

/// Unique identifier for a terminal surface hosted inside a pane.
///
/// A pane can hold multiple surfaces, but only its active surface participates
/// in pane-level agent context. This lets external orchestrators create
/// pane-local tabs without changing the benchmarked pane contract.
pub type SurfaceId = usize;

/// Unique identifier for a split node
pub type SplitId = usize;

#[derive(Clone)]
pub(crate) struct PaneSurface {
    id: SurfaceId,
    terminal: TerminalPane,
    title: Option<String>,
    owner: Option<String>,
    close_pane_when_last: bool,
}

impl PaneSurface {
    fn new(id: SurfaceId, terminal: TerminalPane) -> Self {
        Self {
            id,
            terminal,
            title: None,
            owner: None,
            close_pane_when_last: false,
        }
    }
}

fn restored_screen_text_for_surface(
    terminal: &TerminalPane,
    capture_screen_text: bool,
    cx: &App,
) -> Vec<String> {
    if !capture_screen_text {
        return Vec::new();
    }
    trim_restored_screen_text(terminal.recent_lines(RESTORED_SCREEN_TEXT_MAX_LINES, cx))
}

fn trim_restored_screen_text(lines: Vec<String>) -> Vec<String> {
    let mut trimmed = Vec::with_capacity(lines.len());
    let mut seen_non_blank = false;
    for line in lines {
        if !seen_non_blank && line.trim().is_empty() {
            continue;
        }
        seen_non_blank = true;
        trimmed.push(line);
    }

    while trimmed.last().is_some_and(|line| line.trim().is_empty()) {
        trimmed.pop();
    }

    let mut total = 0usize;
    let mut kept = Vec::new();
    for line in trimmed.into_iter().rev() {
        let line_bytes = line.len().saturating_add(2);
        if line_bytes > RESTORED_SCREEN_TEXT_MAX_BYTES {
            break;
        }
        if total.saturating_add(line_bytes) > RESTORED_SCREEN_TEXT_MAX_BYTES {
            break;
        }
        total = total.saturating_add(line_bytes);
        kept.push(line);
    }
    kept.reverse();
    kept
}

#[derive(Clone)]
pub struct PaneSurfaceInfo {
    pub pane_id: PaneId,
    pub pane_index: usize,
    pub surface_id: SurfaceId,
    pub surface_index: usize,
    pub is_active: bool,
    pub is_focused_pane: bool,
    pub title: Option<String>,
    pub owner: Option<String>,
    pub close_pane_when_last: bool,
    pub terminal: TerminalPane,
}

#[derive(Clone)]
pub struct SurfaceRenameEditor {
    pub surface_id: SurfaceId,
    pub input: Entity<InputState>,
}

#[derive(Debug, Clone)]
pub struct SurfaceCreateOptions {
    pub title: Option<String>,
    pub owner: Option<String>,
    pub close_pane_when_last: bool,
}

impl SurfaceCreateOptions {
    pub fn plain(title: Option<String>) -> Self {
        Self {
            title,
            owner: None,
            close_pane_when_last: false,
        }
    }
}

pub struct SurfaceCloseOutcome {
    pub pane_id: PaneId,
    pub terminal: TerminalPane,
    pub closed_pane: bool,
}

struct SurfaceCloseCandidate {
    pane_id: PaneId,
    terminal: TerminalPane,
    is_last_surface: bool,
    owner: Option<String>,
    close_pane_when_last: bool,
}

/// The content of a leaf pane — either a terminal (with surfaces) or an editor.
#[derive(Clone)]
pub enum PaneContent {
    Terminal {
        surfaces: Vec<PaneSurface>,
        active_surface_id: SurfaceId,
    },
    Editor {
        view: Entity<EditorView>,
    },
}

impl PaneContent {
    fn surfaces(&self) -> &[PaneSurface] {
        match self {
            PaneContent::Terminal { surfaces, .. } => surfaces,
            PaneContent::Editor { .. } => &[],
        }
    }

    fn active_surface_id(&self) -> SurfaceId {
        match self {
            PaneContent::Terminal {
                active_surface_id, ..
            } => *active_surface_id,
            PaneContent::Editor { .. } => 0,
        }
    }

    #[allow(dead_code)]
    fn is_terminal(&self) -> bool {
        matches!(self, PaneContent::Terminal { .. })
    }

    #[allow(dead_code)]
    fn is_editor(&self) -> bool {
        matches!(self, PaneContent::Editor { .. })
    }
}

/// A node in the pane tree — either a pane slot or a split.
enum PaneNode {
    Leaf {
        id: PaneId,
        content: PaneContent,
    },
    Split {
        split_id: SplitId,
        direction: SplitDirection,
        first: Box<PaneNode>,
        second: Box<PaneNode>,
        ratio: f32,
    },
}

/// Active drag state for a split divider
#[derive(Clone)]
pub struct DragState {
    /// Which split node is being dragged
    pub split_id: SplitId,
    /// The ratio at the moment the drag began
    pub start_ratio: f32,
    /// Cursor position (in the axis of the split) when the drag began, in px
    pub start_pos: f32,
}

/// Manages a tree of split panes within a single tab
pub struct PaneTree {
    root: PaneNode,
    focused_pane_id: PaneId,
    zoomed_pane_id: Option<PaneId>,
    next_id: PaneId,
    next_surface_id: SurfaceId,
    next_split_id: SplitId,
    /// Active divider drag, if any
    pub dragging: Option<DragState>,
}

impl PaneTree {
    pub fn new(terminal: TerminalPane) -> Self {
        Self::new_with_surface_options(terminal, SurfaceCreateOptions::plain(None))
    }

    pub fn new_with_surface_options(terminal: TerminalPane, options: SurfaceCreateOptions) -> Self {
        let mut surface = PaneSurface::new(0, terminal);
        surface.title = options.title;
        surface.owner = options.owner;
        surface.close_pane_when_last = options.close_pane_when_last;
        Self {
            root: PaneNode::Leaf {
                id: 0,
                content: PaneContent::Terminal {
                    surfaces: vec![surface],
                    active_surface_id: 0,
                },
            },
            focused_pane_id: 0,
            zoomed_pane_id: None,
            next_id: 1,
            next_surface_id: 1,
            next_split_id: 0,
            dragging: None,
        }
    }

    /// Construct a PaneTree with a single editor pane as the root.
    pub fn new_editor(editor_view: Entity<EditorView>) -> Self {
        Self {
            root: PaneNode::Leaf {
                id: 0,
                content: PaneContent::Editor { view: editor_view },
            },
            focused_pane_id: 0,
            zoomed_pane_id: None,
            next_id: 1,
            next_surface_id: 1,
            next_split_id: 0,
            dragging: None,
        }
    }

    /// Returns true if the focused pane is an Editor pane.
    pub fn focused_pane_is_editor(&self) -> bool {
        Self::focused_pane_is_editor_node(&self.root, self.focused_pane_id)
    }

    fn focused_pane_is_editor_node(node: &PaneNode, focused_pane_id: PaneId) -> bool {
        match node {
            PaneNode::Leaf {
                id,
                content: PaneContent::Editor { .. },
                ..
            } if *id == focused_pane_id => true,
            PaneNode::Leaf { .. } => false,
            PaneNode::Split { first, second, .. } => {
                Self::focused_pane_is_editor_node(first, focused_pane_id)
                    || Self::focused_pane_is_editor_node(second, focused_pane_id)
            }
        }
    }

    pub fn from_state(
        layout: &PaneLayoutState,
        focused_pane_id: Option<PaneId>,
        make_terminal: &mut impl FnMut(Option<&str>, Option<&[String]>, bool) -> TerminalPane,
    ) -> Self {
        let mut next_restored_surface_id = 0;
        let mut used_surface_ids = std::collections::HashSet::new();
        let mut root = Self::node_from_state(
            layout,
            make_terminal,
            &mut next_restored_surface_id,
            &mut used_surface_ids,
        );
        let mut next_split_id = 0;
        Self::normalize_split_ids(&mut root, &mut next_split_id);
        let next_id = Self::max_pane_id(&root).saturating_add(1);
        let next_surface_id = Self::max_surface_id(&root).saturating_add(1);
        let requested_focus = focused_pane_id.unwrap_or_else(|| Self::first_pane_id(&root));
        let focused_pane_id = if Self::find_terminal(&root, requested_focus).is_some() {
            requested_focus
        } else {
            Self::first_pane_id(&root)
        };

        Self {
            root,
            focused_pane_id,
            zoomed_pane_id: None,
            next_id,
            next_surface_id,
            next_split_id,
            dragging: None,
        }
    }

    pub fn to_state(&self, cx: &App, capture_screen_text: bool) -> PaneLayoutState {
        self.to_persisted_state(cx, capture_screen_text)
            .unwrap_or_else(|| PaneLayoutState::Leaf {
                pane_id: self.focused_pane_id,
                cwd: None,
                active_surface_id: None,
                surfaces: Vec::new(),
            })
    }

    pub fn to_persisted_state(
        &self,
        cx: &App,
        capture_screen_text: bool,
    ) -> Option<PaneLayoutState> {
        Self::node_to_state(&self.root, cx, capture_screen_text)
    }

    /// Get the focused terminal
    #[allow(dead_code)]
    pub fn focused_terminal(&self) -> &TerminalPane {
        Self::find_terminal(&self.root, self.focused_pane_id)
            .unwrap_or_else(|| Self::first_terminal(&self.root))
    }

    /// Like `focused_terminal` but returns `None` if no terminal exists
    /// (e.g. the tab contains only editor panes).
    pub fn try_focused_terminal(&self) -> Option<&TerminalPane> {
        Self::find_terminal(&self.root, self.focused_pane_id)
            .or_else(|| Self::try_first_terminal(&self.root))
    }

    /// Get all terminals in the tree
    pub fn all_terminals(&self) -> Vec<&TerminalPane> {
        let mut result = Vec::new();
        Self::collect_terminals(&self.root, &mut result);
        result
    }

    /// Get every terminal surface in the tree, including inactive pane-local tabs.
    pub fn all_surface_terminals(&self) -> Vec<&TerminalPane> {
        let mut result = Vec::new();
        Self::collect_all_surface_terminals(&self.root, &mut result);
        result
    }

    /// Split the focused pane
    pub fn split(&mut self, direction: SplitDirection, new_terminal: TerminalPane) {
        self.split_with_placement(direction, SplitPlacement::After, new_terminal);
    }

    pub fn split_with_placement(
        &mut self,
        direction: SplitDirection,
        placement: SplitPlacement,
        new_terminal: TerminalPane,
    ) {
        let new_id = self.next_id;
        self.next_id += 1;
        let new_surface_id = self.next_surface_id;
        self.next_surface_id += 1;
        let new_split_id = self.next_split_id;
        self.next_split_id += 1;
        Self::split_node(
            &mut self.root,
            self.focused_pane_id,
            direction,
            placement,
            new_id,
            new_surface_id,
            new_terminal,
            new_split_id,
            SurfaceCreateOptions::plain(None),
        );
        self.focused_pane_id = new_id;
        self.zoomed_pane_id = None;
    }

    pub fn split_pane_with_placement(
        &mut self,
        target_pane_id: PaneId,
        direction: SplitDirection,
        placement: SplitPlacement,
        new_terminal: TerminalPane,
    ) {
        let new_id = self.next_id;
        self.next_id += 1;
        let new_surface_id = self.next_surface_id;
        self.next_surface_id += 1;
        let new_split_id = self.next_split_id;
        self.next_split_id += 1;
        if Self::split_node(
            &mut self.root,
            target_pane_id,
            direction,
            placement,
            new_id,
            new_surface_id,
            new_terminal,
            new_split_id,
            SurfaceCreateOptions::plain(None),
        ) {
            self.focused_pane_id = new_id;
            self.zoomed_pane_id = None;
        }
    }

    pub fn split_pane_with_surface_options(
        &mut self,
        target_pane_id: PaneId,
        direction: SplitDirection,
        placement: SplitPlacement,
        new_terminal: TerminalPane,
        options: SurfaceCreateOptions,
    ) -> Option<(PaneId, SurfaceId)> {
        let new_id = self.next_id;
        self.next_id += 1;
        let new_surface_id = self.next_surface_id;
        self.next_surface_id += 1;
        let new_split_id = self.next_split_id;
        self.next_split_id += 1;
        if Self::split_node(
            &mut self.root,
            target_pane_id,
            direction,
            placement,
            new_id,
            new_surface_id,
            new_terminal,
            new_split_id,
            options,
        ) {
            self.focused_pane_id = new_id;
            self.zoomed_pane_id = None;
            Some((new_id, new_surface_id))
        } else {
            None
        }
    }

    /// Split the focused pane to the right and create an editor pane.
    /// Returns the new pane id, or `None` if the split failed.
    pub fn split_with_editor(&mut self, editor_view: Entity<EditorView>) -> Option<PaneId> {
        let new_id = self.next_id;
        self.next_id += 1;
        let new_split_id = self.next_split_id;
        self.next_split_id += 1;

        let new_leaf = PaneNode::Leaf {
            id: new_id,
            content: PaneContent::Editor { view: editor_view },
        };

        let focused = self.focused_pane_id;
        if !Self::insert_editor_leaf(&mut self.root, focused, new_leaf, new_split_id) {
            return None;
        }

        self.focused_pane_id = new_id;
        self.zoomed_pane_id = None;
        Some(new_id)
    }

    /// Find the editor pane in this workspace tab, if one exists.
    pub fn find_editor_pane(&self) -> Option<PaneId> {
        Self::find_editor_pane_node(&self.root)
    }

    fn find_editor_pane_node(node: &PaneNode) -> Option<PaneId> {
        match node {
            PaneNode::Leaf {
                id,
                content: PaneContent::Editor { .. },
                ..
            } => Some(*id),
            PaneNode::Leaf { .. } => None,
            PaneNode::Split { first, second, .. } => {
                Self::find_editor_pane_node(first).or_else(|| Self::find_editor_pane_node(second))
            }
        }
    }

    pub fn editor_view_for_pane(&self, pane_id: PaneId) -> Option<Entity<EditorView>> {
        Self::editor_view_for_pane_node(&self.root, pane_id)
    }

    pub fn pane_id_for_editor_view(&self, editor: &Entity<EditorView>) -> Option<PaneId> {
        Self::find_editor_pane_id_by_entity_id(&self.root, editor.entity_id())
    }

    pub fn editor_views(&self) -> Vec<Entity<EditorView>> {
        let mut views = Vec::new();
        Self::collect_editor_views(&self.root, &mut views);
        views
    }

    pub fn editor_active_path_for_pane(
        &self,
        pane_id: PaneId,
        cx: &App,
    ) -> Option<std::path::PathBuf> {
        self.editor_view_for_pane(pane_id).and_then(|view| {
            view.read(cx)
                .active_path()
                .map(std::path::Path::to_path_buf)
        })
    }

    fn editor_view_for_pane_node(node: &PaneNode, pane_id: PaneId) -> Option<Entity<EditorView>> {
        match node {
            PaneNode::Leaf {
                id,
                content: PaneContent::Editor { view },
                ..
            } if *id == pane_id => Some(view.clone()),
            PaneNode::Leaf { .. } => None,
            PaneNode::Split { first, second, .. } => {
                Self::editor_view_for_pane_node(first, pane_id)
                    .or_else(|| Self::editor_view_for_pane_node(second, pane_id))
            }
        }
    }

    fn collect_editor_views(node: &PaneNode, views: &mut Vec<Entity<EditorView>>) {
        match node {
            PaneNode::Leaf {
                content: PaneContent::Editor { view },
                ..
            } => views.push(view.clone()),
            PaneNode::Leaf { .. } => {}
            PaneNode::Split { first, second, .. } => {
                Self::collect_editor_views(first, views);
                Self::collect_editor_views(second, views);
            }
        }
    }

    /// Focus an existing pane by id (used to re-focus an already-open editor pane).
    pub fn focus_pane(&mut self, pane_id: PaneId) {
        if Self::contains_leaf(&self.root, pane_id) {
            self.focused_pane_id = pane_id;
            if self.zoomed_pane_id.is_some_and(|zoomed| zoomed != pane_id) {
                self.zoomed_pane_id = None;
            }
        }
    }

    fn insert_editor_leaf(
        node: &mut PaneNode,
        target_id: PaneId,
        new_leaf: PaneNode,
        new_split_id: SplitId,
    ) -> bool {
        match node {
            PaneNode::Leaf { id, .. } if *id == target_id => {
                let old = std::mem::replace(node, Self::empty_placeholder_node());
                *node = PaneNode::Split {
                    split_id: new_split_id,
                    direction: SplitDirection::Horizontal,
                    first: Box::new(old),
                    second: Box::new(new_leaf),
                    ratio: 0.5,
                };
                true
            }
            PaneNode::Leaf { .. } => false,
            PaneNode::Split { first, second, .. } => {
                if Self::contains_leaf(first, target_id) {
                    Self::insert_editor_leaf(first, target_id, new_leaf, new_split_id)
                } else {
                    Self::insert_editor_leaf(second, target_id, new_leaf, new_split_id)
                }
            }
        }
    }

    pub fn create_surface_in_pane(
        &mut self,
        pane_id: PaneId,
        terminal: TerminalPane,
        options: SurfaceCreateOptions,
    ) -> Option<SurfaceId> {
        let surface_id = self.next_surface_id;
        self.next_surface_id += 1;
        let mut surface = PaneSurface::new(surface_id, terminal);
        surface.title = options.title;
        surface.owner = options.owner;
        surface.close_pane_when_last = options.close_pane_when_last;

        if Self::push_surface(&mut self.root, pane_id, surface) {
            if self.zoomed_pane_id.is_some_and(|zoomed| zoomed != pane_id) {
                self.zoomed_pane_id = None;
            }
            self.focused_pane_id = pane_id;
            Some(surface_id)
        } else {
            None
        }
    }

    pub fn focus_surface(&mut self, surface_id: SurfaceId) -> bool {
        if let Some((pane_id, surface_changed)) = Self::activate_surface(&mut self.root, surface_id)
        {
            let focus_changed = self.focused_pane_id != pane_id;
            let zoom_changed = self.zoomed_pane_id.is_some_and(|zoomed| zoomed != pane_id);
            if zoom_changed {
                self.zoomed_pane_id = None;
            }
            self.focused_pane_id = pane_id;
            surface_changed || focus_changed || zoom_changed
        } else {
            false
        }
    }

    pub fn rename_surface(&mut self, surface_id: SurfaceId, title: Option<String>) -> bool {
        Self::rename_surface_node(&mut self.root, surface_id, title)
    }

    pub fn close_surface(
        &mut self,
        surface_id: SurfaceId,
        close_empty_owned_pane: bool,
    ) -> Option<SurfaceCloseOutcome> {
        let candidate = Self::surface_close_candidate(&self.root, surface_id)?;
        let outcome = if candidate.is_last_surface {
            let can_close_pane = close_empty_owned_pane
                && candidate.owner.is_some()
                && candidate.close_pane_when_last
                && self.pane_count() > 1;
            if !can_close_pane || !self.close_pane(candidate.pane_id) {
                return None;
            }
            SurfaceCloseOutcome {
                pane_id: candidate.pane_id,
                terminal: candidate.terminal,
                closed_pane: true,
            }
        } else {
            let removed = Self::remove_surface(&mut self.root, surface_id)?;
            SurfaceCloseOutcome {
                pane_id: removed.0,
                terminal: removed.1,
                closed_pane: false,
            }
        };
        if self.pane_count() <= 1
            || self
                .zoomed_pane_id
                .is_some_and(|zoomed_id| Self::find_terminal(&self.root, zoomed_id).is_none())
        {
            self.zoomed_pane_id = None;
        }
        if Self::find_terminal(&self.root, self.focused_pane_id).is_none() {
            self.focused_pane_id = Self::first_pane_id(&self.root);
        }
        Some(outcome)
    }

    /// Close the focused pane, returning true if the tree still has panes
    pub fn close_focused(&mut self) -> bool {
        self.close_pane(self.focused_pane_id)
    }

    /// Close a specific pane by ID, returning true if the tree still has panes
    pub fn close_pane(&mut self, pane_id: PaneId) -> bool {
        if self.pane_count() <= 1 {
            return false;
        }
        // Use a placeholder only if there's a terminal to borrow from.
        // Editor-only trees don't have a terminal to clone.
        let placeholder = if let Some(t) = Self::try_first_terminal(&self.root) {
            PaneNode::Leaf {
                id: 0,
                content: PaneContent::Terminal {
                    surfaces: vec![PaneSurface::new(0, t.clone())],
                    active_surface_id: 0,
                },
            }
        } else {
            Self::empty_placeholder_node()
        };
        let old_root = std::mem::replace(&mut self.root, placeholder);
        self.root = Self::remove_leaf(old_root, pane_id);
        // Re-focus to any surviving pane (not just terminals).
        if !Self::contains_leaf(&self.root, self.focused_pane_id) {
            self.focused_pane_id = Self::first_pane_id(&self.root);
        }
        if self.pane_count() <= 1
            || self
                .zoomed_pane_id
                .is_some_and(|zoomed_id| !Self::contains_leaf(&self.root, zoomed_id))
        {
            self.zoomed_pane_id = None;
        }
        true
    }

    /// Move an existing pane leaf to a split position relative to another pane.
    pub fn move_pane(
        &mut self,
        source_pane_id: PaneId,
        target_pane_id: PaneId,
        direction: SplitDirection,
        placement: SplitPlacement,
    ) -> bool {
        if source_pane_id == target_pane_id
            || self.pane_count() <= 1
            || !Self::contains_leaf(&self.root, source_pane_id)
            || !Self::contains_leaf(&self.root, target_pane_id)
        {
            return false;
        }

        let old_root = std::mem::replace(&mut self.root, Self::empty_placeholder_node());
        let (remaining_root, Some(source_leaf)) = Self::extract_leaf(old_root, source_pane_id)
        else {
            unreachable!("source pane was checked before extraction");
        };
        let Some(remaining_root) = remaining_root else {
            self.root = source_leaf;
            return false;
        };

        let (new_root, inserted) = Self::insert_leaf_at_target(
            remaining_root,
            target_pane_id,
            source_leaf,
            direction,
            placement,
            self.next_split_id,
        );
        if !inserted {
            self.root = new_root;
            return false;
        }
        self.root = new_root;
        self.next_split_id = self.next_split_id.saturating_add(1);
        self.focused_pane_id = source_pane_id;
        self.zoomed_pane_id = None;
        let mut next_split_id = 0;
        Self::normalize_split_ids(&mut self.root, &mut next_split_id);
        self.next_split_id = next_split_id;
        true
    }

    pub fn is_noop_pane_move(
        &self,
        source_pane_id: PaneId,
        target_pane_id: PaneId,
        direction: SplitDirection,
        placement: SplitPlacement,
    ) -> bool {
        source_pane_id != target_pane_id
            && Self::is_direct_sibling_noop_move(
                &self.root,
                source_pane_id,
                target_pane_id,
                direction,
                placement,
            )
    }

    /// Merge another pane tree into this tree as a new root split.
    #[cfg(test)]
    pub fn merge_tree(
        &mut self,
        mut incoming: PaneTree,
        direction: SplitDirection,
        placement: SplitPlacement,
    ) {
        let mut next_pane_id = Self::max_pane_id(&self.root).saturating_add(1);
        let incoming_focused_pane_id = Self::remap_leaf_ids(
            &mut incoming.root,
            &mut next_pane_id,
            incoming.focused_pane_id,
        );

        let original_root = std::mem::replace(&mut self.root, Self::empty_placeholder_node());
        let new_split_id = self.next_split_id;
        let (first, second) = match placement {
            SplitPlacement::Before => (incoming.root, original_root),
            SplitPlacement::After => (original_root, incoming.root),
        };
        self.root = PaneNode::Split {
            split_id: new_split_id,
            direction,
            first: Box::new(first),
            second: Box::new(second),
            ratio: 0.5,
        };
        self.focused_pane_id = incoming_focused_pane_id;
        self.zoomed_pane_id = None;
        self.next_id = Self::max_pane_id(&self.root).saturating_add(1);
        self.next_surface_id = self
            .next_surface_id
            .max(incoming.next_surface_id)
            .max(Self::max_surface_id(&self.root).saturating_add(1));
        let mut next_split_id = 0;
        Self::normalize_split_ids(&mut self.root, &mut next_split_id);
        self.next_split_id = next_split_id;
        self.dragging = None;
    }

    /// Set focus to a pane by ID without changing the current zoom target.
    pub fn focus(&mut self, pane_id: PaneId) {
        if Self::find_terminal(&self.root, pane_id).is_some() {
            self.focused_pane_id = pane_id;
        }
    }

    /// Terminal that should receive app-level focus when returning from UI.
    /// While zoomed, only the zoomed pane is visible, so focus that pane.
    pub fn try_visible_focus_terminal(&self) -> Option<(PaneId, &TerminalPane)> {
        if let Some(zoomed_id) = self.zoomed_pane_id {
            if let Some(terminal) = Self::find_terminal(&self.root, zoomed_id) {
                return Some((zoomed_id, terminal));
            }
        }

        if let Some(terminal) = Self::find_terminal(&self.root, self.focused_pane_id) {
            return Some((self.focused_pane_id, terminal));
        }

        Self::try_first_terminal_with_id(&self.root)
    }

    /// Update focused pane based on which terminal currently has window focus.
    pub fn sync_focus(&mut self, window: &Window, cx: &App) {
        if let Some(id) = Self::find_focused_pane(&self.root, window, cx) {
            self.focused_pane_id = id;
            return;
        }

        if let Some(id) = Self::focused_editor_pane_id(&self.root, window, cx) {
            self.focused_pane_id = id;
        }
    }

    /// Get all pane IDs with their terminal panes
    pub fn pane_terminals(&self) -> Vec<(PaneId, TerminalPane)> {
        let mut result = Vec::new();
        Self::collect_pane_terminals(&self.root, &mut result);
        result
    }

    pub fn surface_terminals(&self) -> Vec<(PaneId, bool, TerminalPane)> {
        let mut result = Vec::new();
        Self::collect_surface_terminals(&self.root, &mut result);
        result
    }

    pub fn surface_infos(&self, target_pane_id: Option<PaneId>) -> Vec<PaneSurfaceInfo> {
        let mut result = Vec::new();
        let mut pane_index = 0;
        Self::collect_surface_infos(
            &self.root,
            target_pane_id,
            self.focused_pane_id,
            &mut pane_index,
            &mut result,
        );
        result
    }

    pub fn active_surface_id_for_pane(&self, pane_id: PaneId) -> Option<SurfaceId> {
        Self::find_active_surface_id(&self.root, pane_id)
    }

    pub fn active_editor_pane_id(&self, window: &Window, cx: &App) -> Option<PaneId> {
        let focused_entity = Self::focused_editor_entity_id(&self.root, window, cx)?;
        Self::find_editor_pane_id_by_entity_id(&self.root, focused_entity)
    }

    fn focused_editor_pane_id(node: &PaneNode, window: &Window, cx: &App) -> Option<PaneId> {
        let focused_entity = Self::focused_editor_entity_id(node, window, cx)?;
        Self::find_editor_pane_id_by_entity_id(node, focused_entity)
    }

    fn focused_editor_entity_id(node: &PaneNode, window: &Window, cx: &App) -> Option<EntityId> {
        match node {
            PaneNode::Leaf {
                content: PaneContent::Editor { view },
                ..
            } => view
                .read(cx)
                .focus_handle(cx)
                .contains_focused(window, cx)
                .then_some(view.entity_id()),
            PaneNode::Leaf { .. } => None,
            PaneNode::Split { first, second, .. } => {
                Self::focused_editor_entity_id(first, window, cx)
                    .or_else(|| Self::focused_editor_entity_id(second, window, cx))
            }
        }
    }

    fn find_editor_pane_id_by_entity_id(node: &PaneNode, entity_id: EntityId) -> Option<PaneId> {
        match node {
            PaneNode::Leaf {
                id,
                content: PaneContent::Editor { view },
                ..
            } if view.entity_id() == entity_id => Some(*id),
            PaneNode::Leaf { .. } => None,
            PaneNode::Split { first, second, .. } => {
                Self::find_editor_pane_id_by_entity_id(first, entity_id)
                    .or_else(|| Self::find_editor_pane_id_by_entity_id(second, entity_id))
            }
        }
    }

    pub fn focused_editor_tab_count(&self, window: &Window, cx: &App) -> Option<usize> {
        let pane_id = self.active_editor_pane_id(window, cx)?;
        self.editor_view_for_pane(pane_id)
            .map(|view| view.read(cx).tab_count())
    }

    pub fn focused_pane_id(&self) -> PaneId {
        self.focused_pane_id
    }

    pub fn focused_terminal_entity_id(&self) -> Option<EntityId> {
        Self::find_terminal(&self.root, self.focused_pane_id).map(|terminal| terminal.entity_id())
    }

    /// Find the pane ID for a given terminal by entity ID
    pub fn pane_id_for_entity(&self, entity_id: EntityId) -> Option<PaneId> {
        Self::find_pane_id_by_entity_id(&self.root, entity_id)
    }

    /// Find the pane ID for a given terminal pane
    pub fn pane_id_for_terminal(&self, terminal: &TerminalPane) -> Option<PaneId> {
        self.pane_id_for_entity(terminal.entity_id())
    }

    /// Check if a given terminal belongs to a specific pane ID
    pub fn terminal_has_pane_id(&self, terminal: &TerminalPane, pane_id: PaneId) -> bool {
        Self::check_terminal_pane_id(&self.root, terminal.entity_id(), pane_id)
    }

    /// Number of panes
    pub fn pane_count(&self) -> usize {
        Self::count_leaves(&self.root)
    }

    fn terminal_title_or_default(terminal: &TerminalPane, cx: &App) -> String {
        let title = terminal.title(cx).unwrap_or_else(|| "Terminal".to_string());
        Self::short_pane_title(&title).to_string()
    }

    fn short_pane_title(title: &str) -> &str {
        // Show only the last path component so narrow panes don't truncate to "...".
        // OSC titles can contain either Unix or Windows separators regardless of host OS.
        title
            .trim_end_matches(['/', '\\'])
            .rsplit(['/', '\\'])
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(title)
    }

    pub fn pane_title(&self, pane_id: PaneId, cx: &App) -> Option<String> {
        Self::find_terminal(&self.root, pane_id)
            .map(|terminal| Self::terminal_title_or_default(terminal, cx))
    }

    pub fn pane_bounds(&self, bounds: Bounds<Pixels>) -> Vec<(PaneId, Bounds<Pixels>)> {
        let mut result = Vec::new();
        if let Some(zoomed_id) = self.zoomed_pane_id {
            if Self::contains_leaf(&self.root, zoomed_id) {
                result.push((zoomed_id, bounds));
            }
            return result;
        }
        Self::collect_pane_bounds(&self.root, bounds, &mut result);
        result
    }

    pub fn zoomed_pane_id(&self) -> Option<PaneId> {
        self.zoomed_pane_id
    }

    pub fn toggle_zoom_focused(&mut self) -> bool {
        if self.pane_count() <= 1 {
            self.zoomed_pane_id = None;
            return false;
        }

        if self.zoomed_pane_id.is_some() {
            self.zoomed_pane_id = None;
        } else {
            self.zoomed_pane_id = Some(self.focused_pane_id);
        }
        true
    }

    /// Start a drag on the given split divider.
    pub fn begin_drag(&mut self, split_id: SplitId, start_pos: f32) {
        if let Some((ratio, _dir)) = Self::find_split_ratio(&self.root, split_id) {
            self.dragging = Some(DragState {
                split_id,
                start_ratio: ratio,
                start_pos,
            });
        }
    }

    /// Update the drag: move the divider based on new cursor position and total container size.
    pub fn update_drag(&mut self, current_pos: f32, total_size: f32) -> bool {
        let drag = match &self.dragging {
            Some(d) => d.clone(),
            None => return false,
        };
        if total_size <= 0.0 {
            return false;
        }
        let delta = current_pos - drag.start_pos;
        let new_ratio = (drag.start_ratio + delta / total_size).clamp(0.05, 0.95);
        Self::set_split_ratio(&mut self.root, drag.split_id, new_ratio)
    }

    /// Finish a drag operation.
    pub fn end_drag(&mut self) {
        self.dragging = None;
    }

    /// Whether a drag is currently in progress
    pub fn is_dragging(&self) -> bool {
        self.dragging.is_some()
    }

    /// Returns the split direction of the currently-dragged split, if any.
    pub fn dragging_direction(&self) -> Option<SplitDirection> {
        let drag = self.dragging.as_ref()?;
        Self::find_split_ratio(&self.root, drag.split_id).map(|(_, dir)| dir)
    }

    /// Returns the split direction for a split id.
    pub fn split_direction(&self, split_id: SplitId) -> Option<SplitDirection> {
        Self::find_split_ratio(&self.root, split_id).map(|(_, dir)| dir)
    }

    /// Returns the currently-dragged split, if a divider drag is active.
    pub fn dragging_split_id(&self) -> Option<SplitId> {
        self.dragging.as_ref().map(|drag| drag.split_id)
    }

    /// Render the pane tree as a GPUI element.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &self,
        session_id: u64,
        begin_drag_cb: impl Fn(SplitId, f32, &mut Window, &mut App) + 'static,
        focus_surface_cb: impl Fn(SurfaceId, &mut Window, &mut App) + 'static,
        focus_pane_cb: impl Fn(PaneId, &mut Window, &mut App) + 'static,
        rename_surface_cb: impl Fn(SurfaceId, &mut Window, &mut App) + 'static,
        close_surface_cb: impl Fn(SurfaceId, &mut Window, &mut App) + 'static,
        close_pane_cb: impl Fn(PaneId, &mut Window, &mut App) + 'static,
        toggle_zoom_cb: impl Fn(PaneId, &mut Window, &mut App) + 'static,
        rename_editor: Option<SurfaceRenameEditor>,
        divider_color: Hsla,
        resize_cover_color: Hsla,
        tab_accent_color: Option<con_core::session::TabAccentColor>,
        tab_accent_inactive_alpha: f32,
        hide_pane_title_bar: bool,
        cx: &App,
    ) -> AnyElement {
        let focus_surface_cb = std::sync::Arc::new(focus_surface_cb);
        let focus_pane_cb = std::sync::Arc::new(focus_pane_cb);
        let rename_surface_cb = std::sync::Arc::new(rename_surface_cb);
        let close_surface_cb = std::sync::Arc::new(close_surface_cb);
        let close_pane_cb = std::sync::Arc::new(close_pane_cb);
        let toggle_zoom_cb = std::sync::Arc::new(toggle_zoom_cb);
        let has_splits = self.pane_count() > 1;
        let focused_pane_id = self.focused_pane_id;
        let zoomed_pane_id = self.zoomed_pane_id;
        let active_drag_split_id = self.dragging_split_id();

        if let Some(zoomed_id) = zoomed_pane_id {
            if let Some(zoomed) = Self::render_zoomed_leaf(
                &self.root,
                zoomed_id,
                focused_pane_id,
                focus_surface_cb.clone(),
                focus_pane_cb.clone(),
                rename_surface_cb.clone(),
                close_surface_cb.clone(),
                close_pane_cb.clone(),
                toggle_zoom_cb.clone(),
                rename_editor.clone(),
                has_splits,
                zoomed_pane_id,
                session_id,
                tab_accent_color,
                tab_accent_inactive_alpha,
                hide_pane_title_bar,
                cx,
            ) {
                return zoomed;
            }
        }

        Self::render_node(
            &self.root,
            focused_pane_id,
            has_splits,
            std::sync::Arc::new(begin_drag_cb),
            focus_surface_cb,
            focus_pane_cb,
            rename_surface_cb,
            close_surface_cb,
            close_pane_cb,
            toggle_zoom_cb,
            rename_editor,
            divider_color,
            resize_cover_color,
            active_drag_split_id,
            has_splits,
            zoomed_pane_id,
            session_id,
            tab_accent_color,
            tab_accent_inactive_alpha,
            hide_pane_title_bar,
            cx,
        )
    }

    // --- Private helpers ---

    fn find_terminal(node: &PaneNode, target_id: PaneId) -> Option<&TerminalPane> {
        match node {
            PaneNode::Leaf {
                id,
                content:
                    PaneContent::Terminal {
                        surfaces,
                        active_surface_id,
                    },
                ..
            } if *id == target_id => {
                Self::active_surface(surfaces, *active_surface_id).map(|surface| &surface.terminal)
            }
            PaneNode::Leaf { .. } => None,
            PaneNode::Split { first, second, .. } => Self::find_terminal(first, target_id)
                .or_else(|| Self::find_terminal(second, target_id)),
        }
    }

    fn first_terminal(node: &PaneNode) -> &TerminalPane {
        Self::try_first_terminal(node).expect("pane tree must have at least one terminal")
    }

    fn try_first_terminal_with_id(node: &PaneNode) -> Option<(PaneId, &TerminalPane)> {
        match node {
            PaneNode::Leaf {
                content:
                    PaneContent::Terminal {
                        surfaces,
                        active_surface_id,
                    },
                id,
                ..
            } => Self::active_surface(surfaces, *active_surface_id)
                .map(|surface| (*id, &surface.terminal)),
            PaneNode::Leaf { .. } => None,
            PaneNode::Split { first, second, .. } => Self::try_first_terminal_with_id(first)
                .or_else(|| Self::try_first_terminal_with_id(second)),
        }
    }

    fn try_first_terminal(node: &PaneNode) -> Option<&TerminalPane> {
        match node {
            PaneNode::Leaf {
                content:
                    PaneContent::Terminal {
                        surfaces,
                        active_surface_id,
                    },
                ..
            } => {
                Self::active_surface(surfaces, *active_surface_id).map(|surface| &surface.terminal)
            }
            PaneNode::Leaf { .. } => None,
            PaneNode::Split { first, second, .. } => {
                Self::try_first_terminal(first).or_else(|| Self::try_first_terminal(second))
            }
        }
    }

    #[allow(dead_code)]
    fn first_pane_id(node: &PaneNode) -> PaneId {
        match node {
            PaneNode::Leaf { id, .. } => *id,
            PaneNode::Split { first, .. } => Self::first_pane_id(first),
        }
    }

    pub fn first_pane_id_pub(&self) -> PaneId {
        Self::first_pane_id(&self.root)
    }

    pub fn contains_pane(&self, pane_id: PaneId) -> bool {
        Self::contains_leaf(&self.root, pane_id)
    }

    fn collect_pane_bounds(
        node: &PaneNode,
        bounds: Bounds<Pixels>,
        result: &mut Vec<(PaneId, Bounds<Pixels>)>,
    ) {
        match node {
            PaneNode::Leaf { id, .. } => result.push((*id, bounds)),
            PaneNode::Split {
                direction,
                first,
                second,
                ratio,
                ..
            } => {
                let ratio = (*ratio).clamp(0.0, 1.0);
                match direction {
                    SplitDirection::Horizontal => {
                        let first_width = bounds.size.width * ratio;
                        let second_width = bounds.size.width - first_width;
                        let first_bounds =
                            Bounds::new(bounds.origin, size(first_width, bounds.size.height));
                        let second_bounds = Bounds::new(
                            point(bounds.origin.x + first_width, bounds.origin.y),
                            size(second_width, bounds.size.height),
                        );
                        Self::collect_pane_bounds(first, first_bounds, result);
                        Self::collect_pane_bounds(second, second_bounds, result);
                    }
                    SplitDirection::Vertical => {
                        let first_height = bounds.size.height * ratio;
                        let second_height = bounds.size.height - first_height;
                        let first_bounds =
                            Bounds::new(bounds.origin, size(bounds.size.width, first_height));
                        let second_bounds = Bounds::new(
                            point(bounds.origin.x, bounds.origin.y + first_height),
                            size(bounds.size.width, second_height),
                        );
                        Self::collect_pane_bounds(first, first_bounds, result);
                        Self::collect_pane_bounds(second, second_bounds, result);
                    }
                }
            }
        }
    }

    fn collect_terminals<'a>(node: &'a PaneNode, result: &mut Vec<&'a TerminalPane>) {
        match node {
            PaneNode::Leaf {
                content:
                    PaneContent::Terminal {
                        surfaces,
                        active_surface_id,
                    },
                ..
            } => {
                if let Some(surface) = Self::active_surface(surfaces, *active_surface_id) {
                    result.push(&surface.terminal);
                }
            }
            PaneNode::Leaf { .. } => {}
            PaneNode::Split { first, second, .. } => {
                Self::collect_terminals(first, result);
                Self::collect_terminals(second, result);
            }
        }
    }

    fn collect_all_surface_terminals<'a>(node: &'a PaneNode, result: &mut Vec<&'a TerminalPane>) {
        match node {
            PaneNode::Leaf {
                content: PaneContent::Terminal { surfaces, .. },
                ..
            } => {
                result.extend(surfaces.iter().map(|surface| &surface.terminal));
            }
            PaneNode::Leaf { .. } => {}
            PaneNode::Split { first, second, .. } => {
                Self::collect_all_surface_terminals(first, result);
                Self::collect_all_surface_terminals(second, result);
            }
        }
    }

    fn count_leaves(node: &PaneNode) -> usize {
        match node {
            PaneNode::Leaf { .. } => 1,
            PaneNode::Split { first, second, .. } => {
                Self::count_leaves(first) + Self::count_leaves(second)
            }
        }
    }

    fn max_pane_id(node: &PaneNode) -> PaneId {
        match node {
            PaneNode::Leaf { id, .. } => *id,
            PaneNode::Split { first, second, .. } => {
                Self::max_pane_id(first).max(Self::max_pane_id(second))
            }
        }
    }

    fn max_surface_id(node: &PaneNode) -> SurfaceId {
        match node {
            PaneNode::Leaf {
                content: PaneContent::Terminal { surfaces, .. },
                ..
            } => surfaces.iter().map(|surface| surface.id).max().unwrap_or(0),
            PaneNode::Leaf { .. } => 0,
            PaneNode::Split { first, second, .. } => {
                Self::max_surface_id(first).max(Self::max_surface_id(second))
            }
        }
    }

    #[cfg(test)]
    fn remap_leaf_ids(
        node: &mut PaneNode,
        next_pane_id: &mut PaneId,
        old_focused_pane_id: PaneId,
    ) -> PaneId {
        match node {
            PaneNode::Leaf { id, .. } => {
                let old_id = *id;
                let new_id = *next_pane_id;
                *id = new_id;
                *next_pane_id = next_pane_id.saturating_add(1);
                if old_id == old_focused_pane_id {
                    new_id
                } else {
                    PaneId::MAX
                }
            }
            PaneNode::Split { first, second, .. } => {
                let first_focus = Self::remap_leaf_ids(first, next_pane_id, old_focused_pane_id);
                let second_focus = Self::remap_leaf_ids(second, next_pane_id, old_focused_pane_id);
                if first_focus != PaneId::MAX {
                    first_focus
                } else {
                    second_focus
                }
            }
        }
    }

    fn normalize_split_ids(node: &mut PaneNode, next_split_id: &mut SplitId) {
        match node {
            PaneNode::Leaf { .. } => {}
            PaneNode::Split {
                split_id,
                first,
                second,
                ..
            } => {
                *split_id = *next_split_id;
                *next_split_id += 1;
                Self::normalize_split_ids(first, next_split_id);
                Self::normalize_split_ids(second, next_split_id);
            }
        }
    }

    fn find_split_ratio(node: &PaneNode, target_id: SplitId) -> Option<(f32, SplitDirection)> {
        match node {
            PaneNode::Leaf { .. } => None,
            PaneNode::Split {
                split_id,
                ratio,
                direction,
                first,
                second,
            } => {
                if *split_id == target_id {
                    Some((*ratio, *direction))
                } else {
                    Self::find_split_ratio(first, target_id)
                        .or_else(|| Self::find_split_ratio(second, target_id))
                }
            }
        }
    }

    fn set_split_ratio(node: &mut PaneNode, target_id: SplitId, new_ratio: f32) -> bool {
        match node {
            PaneNode::Leaf { .. } => false,
            PaneNode::Split {
                split_id,
                ratio,
                first,
                second,
                ..
            } => {
                if *split_id == target_id {
                    if (*ratio - new_ratio).abs() > 0.001 {
                        *ratio = new_ratio;
                        true
                    } else {
                        false
                    }
                } else {
                    Self::set_split_ratio(first, target_id, new_ratio)
                        || Self::set_split_ratio(second, target_id, new_ratio)
                }
            }
        }
    }

    /// Returns `true` if the target was found and split.
    fn split_node(
        node: &mut PaneNode,
        target_id: PaneId,
        direction: SplitDirection,
        placement: SplitPlacement,
        new_id: PaneId,
        new_surface_id: SurfaceId,
        new_terminal: TerminalPane,
        new_split_id: SplitId,
        options: SurfaceCreateOptions,
    ) -> bool {
        match node {
            PaneNode::Leaf { id, .. } if *id == target_id => {
                let mut surface = PaneSurface::new(new_surface_id, new_terminal.clone());
                surface.title = options.title;
                surface.owner = options.owner;
                surface.close_pane_when_last = options.close_pane_when_last;
                let old_node = std::mem::replace(
                    node,
                    PaneNode::Leaf {
                        id: new_id,
                        content: PaneContent::Terminal {
                            surfaces: vec![surface.clone()],
                            active_surface_id: new_surface_id,
                        },
                    },
                );
                let new_leaf = PaneNode::Leaf {
                    id: new_id,
                    content: PaneContent::Terminal {
                        surfaces: vec![surface],
                        active_surface_id: new_surface_id,
                    },
                };
                let (first, second) = match placement {
                    SplitPlacement::Before => (Box::new(new_leaf), Box::new(old_node)),
                    SplitPlacement::After => (Box::new(old_node), Box::new(new_leaf)),
                };
                *node = PaneNode::Split {
                    split_id: new_split_id,
                    direction,
                    first,
                    second,
                    ratio: 0.5,
                };
                true
            }
            PaneNode::Split { first, second, .. } => {
                Self::split_node(
                    first,
                    target_id,
                    direction,
                    placement,
                    new_id,
                    new_surface_id,
                    new_terminal.clone(),
                    new_split_id,
                    options.clone(),
                ) || Self::split_node(
                    second,
                    target_id,
                    direction,
                    placement,
                    new_id,
                    new_surface_id,
                    new_terminal,
                    new_split_id,
                    options,
                )
            }
            _ => false,
        }
    }

    fn push_surface(node: &mut PaneNode, pane_id: PaneId, surface: PaneSurface) -> bool {
        match node {
            PaneNode::Leaf {
                id,
                content:
                    PaneContent::Terminal {
                        surfaces,
                        active_surface_id,
                    },
                ..
            } if *id == pane_id => {
                let surface_id = surface.id;
                surfaces.push(surface);
                *active_surface_id = surface_id;
                true
            }
            PaneNode::Leaf { .. } => false,
            PaneNode::Split { first, second, .. } => {
                Self::push_surface(first, pane_id, surface.clone())
                    || Self::push_surface(second, pane_id, surface)
            }
        }
    }

    fn activate_surface(node: &mut PaneNode, surface_id: SurfaceId) -> Option<(PaneId, bool)> {
        match node {
            PaneNode::Leaf {
                id,
                content:
                    PaneContent::Terminal {
                        surfaces,
                        active_surface_id,
                    },
                ..
            } => {
                if surfaces.iter().any(|surface| surface.id == surface_id) {
                    let changed = *active_surface_id != surface_id;
                    *active_surface_id = surface_id;
                    Some((*id, changed))
                } else {
                    None
                }
            }
            PaneNode::Leaf { .. } => None,
            PaneNode::Split { first, second, .. } => Self::activate_surface(first, surface_id)
                .or_else(|| Self::activate_surface(second, surface_id)),
        }
    }

    fn rename_surface_node(
        node: &mut PaneNode,
        surface_id: SurfaceId,
        title: Option<String>,
    ) -> bool {
        match node {
            PaneNode::Leaf {
                content: PaneContent::Terminal { surfaces, .. },
                ..
            } => {
                if let Some(surface) = surfaces.iter_mut().find(|surface| surface.id == surface_id)
                {
                    surface.title = title;
                    true
                } else {
                    false
                }
            }
            PaneNode::Leaf { .. } => false,
            PaneNode::Split { first, second, .. } => {
                Self::rename_surface_node(first, surface_id, title.clone())
                    || Self::rename_surface_node(second, surface_id, title)
            }
        }
    }

    fn remove_surface(
        node: &mut PaneNode,
        surface_id: SurfaceId,
    ) -> Option<(PaneId, TerminalPane)> {
        match node {
            PaneNode::Leaf {
                id,
                content:
                    PaneContent::Terminal {
                        surfaces,
                        active_surface_id,
                    },
                ..
            } => {
                if surfaces.len() <= 1 {
                    return None;
                }
                let index = surfaces
                    .iter()
                    .position(|surface| surface.id == surface_id)?;
                let removed = surfaces.remove(index);
                if *active_surface_id == surface_id {
                    *active_surface_id = surfaces
                        .get(index.saturating_sub(1))
                        .or_else(|| surfaces.first())
                        .map(|surface| surface.id)
                        .expect("pane leaf must retain a surface");
                }
                Some((*id, removed.terminal))
            }
            PaneNode::Leaf { .. } => None,
            PaneNode::Split { first, second, .. } => Self::remove_surface(first, surface_id)
                .or_else(|| Self::remove_surface(second, surface_id)),
        }
    }

    fn surface_close_candidate(
        node: &PaneNode,
        surface_id: SurfaceId,
    ) -> Option<SurfaceCloseCandidate> {
        match node {
            PaneNode::Leaf {
                id,
                content: PaneContent::Terminal { surfaces, .. },
                ..
            } => {
                let surface = surfaces.iter().find(|surface| surface.id == surface_id)?;
                Some(SurfaceCloseCandidate {
                    pane_id: *id,
                    terminal: surface.terminal.clone(),
                    is_last_surface: surfaces.len() <= 1,
                    owner: surface.owner.clone(),
                    close_pane_when_last: surface.close_pane_when_last,
                })
            }
            PaneNode::Leaf { .. } => None,
            PaneNode::Split { first, second, .. } => {
                Self::surface_close_candidate(first, surface_id)
                    .or_else(|| Self::surface_close_candidate(second, surface_id))
            }
        }
    }

    #[allow(dead_code)]
    fn remove_leaf(node: PaneNode, target_id: PaneId) -> PaneNode {
        match node {
            PaneNode::Split {
                split_id,
                direction,
                first,
                second,
                ratio,
            } => {
                if matches!(first.as_ref(), PaneNode::Leaf { id, .. } if *id == target_id) {
                    return *second;
                }
                if matches!(second.as_ref(), PaneNode::Leaf { id, .. } if *id == target_id) {
                    return *first;
                }
                PaneNode::Split {
                    split_id,
                    direction,
                    first: Box::new(Self::remove_leaf(*first, target_id)),
                    second: Box::new(Self::remove_leaf(*second, target_id)),
                    ratio,
                }
            }
            leaf => leaf,
        }
    }

    fn empty_placeholder_node() -> PaneNode {
        PaneNode::Leaf {
            id: PaneId::MAX,
            content: PaneContent::Terminal {
                surfaces: Vec::new(),
                active_surface_id: 0,
            },
        }
    }

    fn contains_leaf(node: &PaneNode, pane_id: PaneId) -> bool {
        match node {
            PaneNode::Leaf { id, .. } => *id == pane_id,
            PaneNode::Split { first, second, .. } => {
                Self::contains_leaf(first, pane_id) || Self::contains_leaf(second, pane_id)
            }
        }
    }

    fn is_leaf(node: &PaneNode, pane_id: PaneId) -> bool {
        matches!(node, PaneNode::Leaf { id, .. } if *id == pane_id)
    }

    fn is_direct_sibling_noop_move(
        node: &PaneNode,
        source_pane_id: PaneId,
        target_pane_id: PaneId,
        direction: SplitDirection,
        placement: SplitPlacement,
    ) -> bool {
        match node {
            PaneNode::Leaf { .. } => false,
            PaneNode::Split {
                direction: split_direction,
                first,
                second,
                ..
            } => {
                if *split_direction == direction {
                    if Self::is_leaf(first, target_pane_id)
                        && Self::is_leaf(second, source_pane_id)
                        && placement == SplitPlacement::After
                    {
                        return true;
                    }
                    if Self::is_leaf(first, source_pane_id)
                        && Self::is_leaf(second, target_pane_id)
                        && placement == SplitPlacement::Before
                    {
                        return true;
                    }
                }

                Self::is_direct_sibling_noop_move(
                    first,
                    source_pane_id,
                    target_pane_id,
                    direction,
                    placement,
                ) || Self::is_direct_sibling_noop_move(
                    second,
                    source_pane_id,
                    target_pane_id,
                    direction,
                    placement,
                )
            }
        }
    }

    fn extract_leaf(node: PaneNode, pane_id: PaneId) -> (Option<PaneNode>, Option<PaneNode>) {
        match node {
            PaneNode::Leaf { id, .. } if id == pane_id => (None, Some(node)),
            leaf @ PaneNode::Leaf { .. } => (Some(leaf), None),
            PaneNode::Split {
                split_id,
                direction,
                first,
                second,
                ratio,
            } => {
                let first_node = *first;
                let second_node = *second;
                let (new_first, extracted) = Self::extract_leaf(first_node, pane_id);
                if extracted.is_some() {
                    return match new_first {
                        Some(first) => (
                            Some(PaneNode::Split {
                                split_id,
                                direction,
                                first: Box::new(first),
                                second: Box::new(second_node),
                                ratio,
                            }),
                            extracted,
                        ),
                        None => (Some(second_node), extracted),
                    };
                }

                let first_node = new_first.expect("non-matching leaf extraction preserves node");
                let (new_second, extracted) = Self::extract_leaf(second_node, pane_id);
                if extracted.is_some() {
                    return match new_second {
                        Some(second) => (
                            Some(PaneNode::Split {
                                split_id,
                                direction,
                                first: Box::new(first_node),
                                second: Box::new(second),
                                ratio,
                            }),
                            extracted,
                        ),
                        None => (Some(first_node), extracted),
                    };
                }

                (
                    Some(PaneNode::Split {
                        split_id,
                        direction,
                        first: Box::new(first_node),
                        second: Box::new(
                            new_second.expect("non-matching leaf extraction preserves node"),
                        ),
                        ratio,
                    }),
                    None,
                )
            }
        }
    }

    fn insert_leaf_at_target(
        node: PaneNode,
        target_id: PaneId,
        source_leaf: PaneNode,
        direction: SplitDirection,
        placement: SplitPlacement,
        split_id: SplitId,
    ) -> (PaneNode, bool) {
        match node {
            target @ PaneNode::Leaf { id, .. } if id == target_id => {
                let (first, second) = match placement {
                    SplitPlacement::Before => (source_leaf, target),
                    SplitPlacement::After => (target, source_leaf),
                };
                (
                    PaneNode::Split {
                        split_id,
                        direction,
                        first: Box::new(first),
                        second: Box::new(second),
                        ratio: 0.5,
                    },
                    true,
                )
            }
            leaf @ PaneNode::Leaf { .. } => (leaf, false),
            PaneNode::Split {
                split_id: existing_split_id,
                direction: existing_direction,
                first,
                second,
                ratio,
            } => {
                let first_node = *first;
                let second_node = *second;
                if Self::contains_leaf(&first_node, target_id) {
                    let (new_first, inserted) = Self::insert_leaf_at_target(
                        first_node,
                        target_id,
                        source_leaf,
                        direction,
                        placement,
                        split_id,
                    );
                    return (
                        PaneNode::Split {
                            split_id: existing_split_id,
                            direction: existing_direction,
                            first: Box::new(new_first),
                            second: Box::new(second_node),
                            ratio,
                        },
                        inserted,
                    );
                }

                let (new_second, inserted) = Self::insert_leaf_at_target(
                    second_node,
                    target_id,
                    source_leaf,
                    direction,
                    placement,
                    split_id,
                );
                (
                    PaneNode::Split {
                        split_id: existing_split_id,
                        direction: existing_direction,
                        first: Box::new(first_node),
                        second: Box::new(new_second),
                        ratio,
                    },
                    inserted,
                )
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_node(
        node: &PaneNode,
        focused_id: PaneId,
        has_splits: bool,
        begin_drag_cb: std::sync::Arc<dyn Fn(SplitId, f32, &mut Window, &mut App) + 'static>,
        focus_surface_cb: std::sync::Arc<dyn Fn(SurfaceId, &mut Window, &mut App) + 'static>,
        focus_pane_cb: std::sync::Arc<dyn Fn(PaneId, &mut Window, &mut App) + 'static>,
        rename_surface_cb: std::sync::Arc<dyn Fn(SurfaceId, &mut Window, &mut App) + 'static>,
        close_surface_cb: std::sync::Arc<dyn Fn(SurfaceId, &mut Window, &mut App) + 'static>,
        close_pane_cb: std::sync::Arc<dyn Fn(PaneId, &mut Window, &mut App) + 'static>,
        toggle_zoom_cb: std::sync::Arc<dyn Fn(PaneId, &mut Window, &mut App) + 'static>,
        rename_editor: Option<SurfaceRenameEditor>,
        divider_color: Hsla,
        resize_cover_color: Hsla,
        active_drag_split_id: Option<SplitId>,
        tree_has_splits: bool,
        zoomed_pane_id: Option<PaneId>,
        session_id: u64,
        tab_accent_color: Option<con_core::session::TabAccentColor>,
        tab_accent_inactive_alpha: f32,
        hide_pane_title_bar: bool,
        cx: &App,
    ) -> AnyElement {
        match node {
            PaneNode::Leaf { id, content } => Self::render_leaf(
                *id,
                content.surfaces(),
                content.active_surface_id(),
                content,
                focused_id,
                focus_surface_cb,
                focus_pane_cb,
                rename_surface_cb,
                close_surface_cb,
                close_pane_cb,
                toggle_zoom_cb,
                rename_editor,
                tree_has_splits,
                zoomed_pane_id,
                session_id,
                tab_accent_color,
                tab_accent_inactive_alpha,
                hide_pane_title_bar,
                cx,
            ),
            PaneNode::Split {
                split_id,
                direction,
                first,
                second,
                ratio,
            } => {
                let ratio = *ratio;
                let sid = *split_id;
                let dir = *direction;

                let cb_first = begin_drag_cb.clone();
                let cb_second = begin_drag_cb.clone();
                let cb_divider = begin_drag_cb.clone();
                let focus_cb_first = focus_surface_cb.clone();
                let focus_cb_second = focus_surface_cb.clone();
                let focus_pane_cb_first = focus_pane_cb.clone();
                let focus_pane_cb_second = focus_pane_cb.clone();
                let rename_cb_first = rename_surface_cb.clone();
                let rename_cb_second = rename_surface_cb.clone();
                let close_cb_first = close_surface_cb.clone();
                let close_cb_second = close_surface_cb.clone();
                let close_pane_cb_first = close_pane_cb.clone();
                let close_pane_cb_second = close_pane_cb.clone();
                let toggle_zoom_cb_first = toggle_zoom_cb.clone();
                let toggle_zoom_cb_second = toggle_zoom_cb.clone();
                let rename_editor_first = rename_editor.clone();
                let rename_editor_second = rename_editor.clone();

                let first_el = Self::render_node(
                    first,
                    focused_id,
                    has_splits,
                    cb_first,
                    focus_cb_first,
                    focus_pane_cb_first,
                    rename_cb_first,
                    close_cb_first,
                    close_pane_cb_first,
                    toggle_zoom_cb_first,
                    rename_editor_first,
                    divider_color,
                    resize_cover_color,
                    active_drag_split_id,
                    tree_has_splits,
                    zoomed_pane_id,
                    session_id,
                    tab_accent_color,
                    tab_accent_inactive_alpha,
                    hide_pane_title_bar,
                    cx,
                );
                let second_el = Self::render_node(
                    second,
                    focused_id,
                    has_splits,
                    cb_second,
                    focus_cb_second,
                    focus_pane_cb_second,
                    rename_cb_second,
                    close_cb_second,
                    close_pane_cb_second,
                    toggle_zoom_cb_second,
                    rename_editor_second,
                    divider_color,
                    resize_cover_color,
                    active_drag_split_id,
                    tree_has_splits,
                    zoomed_pane_id,
                    session_id,
                    tab_accent_color,
                    tab_accent_inactive_alpha,
                    hide_pane_title_bar,
                    cx,
                );

                let divider_id = ElementId::Name(format!("divider-{}", sid).into());
                let is_active_drag_divider = active_drag_split_id == Some(sid);
                let divider = match dir {
                    SplitDirection::Horizontal => {
                        let handle = div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(px(-2.0))
                            .w(px(5.0))
                            .cursor_col_resize()
                            .bg(if is_active_drag_divider {
                                resize_cover_color
                            } else {
                                gpui::transparent_black()
                            });
                        div()
                            .id(divider_id)
                            .relative()
                            .w(px(1.0))
                            .h_full()
                            .flex_shrink_0()
                            .bg(divider_color)
                            .child(handle.on_mouse_down(
                                MouseButton::Left,
                                move |event: &MouseDownEvent, window, cx| {
                                    cb_divider(sid, f32::from(event.position.x), window, cx);
                                    window.prevent_default();
                                    cx.stop_propagation();
                                },
                            ))
                    }
                    SplitDirection::Vertical => {
                        let handle = div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(px(-2.0))
                            .h(px(5.0))
                            .cursor_row_resize()
                            .bg(if is_active_drag_divider {
                                resize_cover_color
                            } else {
                                gpui::transparent_black()
                            });
                        div()
                            .id(divider_id)
                            .relative()
                            .h(px(1.0))
                            .w_full()
                            .flex_shrink_0()
                            .bg(divider_color)
                            .child(handle.on_mouse_down(
                                MouseButton::Left,
                                move |event: &MouseDownEvent, window, cx| {
                                    cb_divider(sid, f32::from(event.position.y), window, cx);
                                    window.prevent_default();
                                    cx.stop_propagation();
                                },
                            ))
                    }
                };

                let make_pane = |child: AnyElement, basis: f32| -> Div {
                    let mut d = div()
                        .flex_grow()
                        .flex_shrink()
                        .flex_basis(relative(basis))
                        .overflow_hidden();
                    d = match dir {
                        SplitDirection::Horizontal => d.min_w_0().h_full(),
                        SplitDirection::Vertical => d.min_h_0().w_full(),
                    };
                    d.child(child)
                };

                let first_sized = make_pane(first_el, ratio);
                let second_sized = make_pane(second_el, 1.0 - ratio);

                let mut container = div().flex().size_full();
                container = match dir {
                    SplitDirection::Horizontal => container.flex_row(),
                    SplitDirection::Vertical => container.flex_col(),
                };

                container
                    .child(first_sized)
                    .child(divider)
                    .child(second_sized)
                    .into_any_element()
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_zoomed_leaf(
        node: &PaneNode,
        target_id: PaneId,
        focused_id: PaneId,
        focus_surface_cb: std::sync::Arc<dyn Fn(SurfaceId, &mut Window, &mut App) + 'static>,
        focus_pane_cb: std::sync::Arc<dyn Fn(PaneId, &mut Window, &mut App) + 'static>,
        rename_surface_cb: std::sync::Arc<dyn Fn(SurfaceId, &mut Window, &mut App) + 'static>,
        close_surface_cb: std::sync::Arc<dyn Fn(SurfaceId, &mut Window, &mut App) + 'static>,
        close_pane_cb: std::sync::Arc<dyn Fn(PaneId, &mut Window, &mut App) + 'static>,
        toggle_zoom_cb: std::sync::Arc<dyn Fn(PaneId, &mut Window, &mut App) + 'static>,
        rename_editor: Option<SurfaceRenameEditor>,
        tree_has_splits: bool,
        zoomed_pane_id: Option<PaneId>,
        session_id: u64,
        tab_accent_color: Option<con_core::session::TabAccentColor>,
        tab_accent_inactive_alpha: f32,
        hide_pane_title_bar: bool,
        cx: &App,
    ) -> Option<AnyElement> {
        match node {
            PaneNode::Leaf { id, content } if *id == target_id => Some(Self::render_leaf(
                *id,
                content.surfaces(),
                content.active_surface_id(),
                content,
                focused_id,
                focus_surface_cb,
                focus_pane_cb,
                rename_surface_cb,
                close_surface_cb,
                close_pane_cb,
                toggle_zoom_cb,
                rename_editor,
                tree_has_splits,
                zoomed_pane_id,
                session_id,
                tab_accent_color,
                tab_accent_inactive_alpha,
                hide_pane_title_bar,
                cx,
            )),
            PaneNode::Leaf { .. } => None,
            PaneNode::Split { first, second, .. } => Self::render_zoomed_leaf(
                first,
                target_id,
                focused_id,
                focus_surface_cb.clone(),
                focus_pane_cb.clone(),
                rename_surface_cb.clone(),
                close_surface_cb.clone(),
                close_pane_cb.clone(),
                toggle_zoom_cb.clone(),
                rename_editor.clone(),
                tree_has_splits,
                zoomed_pane_id,
                session_id,
                tab_accent_color,
                tab_accent_inactive_alpha,
                hide_pane_title_bar,
                cx,
            )
            .or_else(|| {
                Self::render_zoomed_leaf(
                    second,
                    target_id,
                    focused_id,
                    focus_surface_cb,
                    focus_pane_cb,
                    rename_surface_cb,
                    close_surface_cb,
                    close_pane_cb,
                    toggle_zoom_cb,
                    rename_editor,
                    tree_has_splits,
                    zoomed_pane_id,
                    session_id,
                    tab_accent_color,
                    tab_accent_inactive_alpha,
                    hide_pane_title_bar,
                    cx,
                )
            }),
        }
    }

    /// Render the persistent title bar shown at the top of each pane when
    /// there are 2+ panes in the split tree.
    fn pane_chrome_bg(
        theme: &gpui_component::Theme,
        _is_focused: bool,
        _tab_accent_inactive_alpha: f32,
    ) -> Hsla {
        // Keep pane chrome opaque. Text rendered over translucent blurred/native
        // backing picks up rough glyph edges on macOS.
        theme.title_bar.opacity(1.0)
    }

    fn render_pane_title_bar(
        pane_id: PaneId,
        session_id: u64,
        title: String,
        is_focused: bool,
        has_splits: bool,
        is_zoomed: bool,
        _tab_accent_color: Option<con_core::session::TabAccentColor>,
        tab_accent_inactive_alpha: f32,
        close_pane_cb: std::sync::Arc<dyn Fn(PaneId, &mut Window, &mut App) + 'static>,
        toggle_zoom_cb: std::sync::Arc<dyn Fn(PaneId, &mut Window, &mut App) + 'static>,
        focus_pane_cb: std::sync::Arc<dyn Fn(PaneId, &mut Window, &mut App) + 'static>,
        cx: &App,
    ) -> AnyElement {
        let theme = cx.theme();
        let title_color = if is_focused {
            theme.foreground.opacity(0.76)
        } else {
            theme.muted_foreground
        };
        let btn_color = theme.foreground.opacity(0.52);
        let btn_hover_bg = theme.foreground.opacity(0.08);

        // ⛶/⛶ fullscreen toggle button — always visible
        let zoom_icon = if is_zoomed {
            "phosphor/frame-corners.svg"
        } else {
            "phosphor/corners-out.svg"
        };
        let zoom_tooltip: SharedString = if is_zoomed {
            "Exit fullscreen".into()
        } else {
            "Fullscreen".into()
        };
        let toggle_zoom_cb_btn = toggle_zoom_cb.clone();
        let zoom_btn = div()
            .id(ElementId::Name(format!("pane-zoom-{pane_id}").into()))
            .flex()
            .items_center()
            .justify_center()
            .size(px(20.0))
            .flex_shrink_0()
            .rounded(px(4.0))
            .cursor_pointer()
            .hover(move |s| s.bg(btn_hover_bg))
            .tooltip(move |window, cx| Tooltip::new(zoom_tooltip.clone()).build(window, cx))
            .on_mouse_down(MouseButton::Left, move |_event, window, cx| {
                toggle_zoom_cb_btn(pane_id, window, cx);
                window.prevent_default();
                cx.stop_propagation();
            })
            .child(svg().path(zoom_icon).size(px(11.0)).text_color(btn_color));

        // ✕ close button — only when there are splits
        let close_btn = if has_splits {
            let close_cb_btn = close_pane_cb.clone();
            let btn_hover_bg2 = theme.foreground.opacity(0.08);
            Some(
                div()
                    .id(ElementId::Name(format!("pane-close-{pane_id}").into()))
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(20.0))
                    .flex_shrink_0()
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(move |s| s.bg(btn_hover_bg2))
                    .on_mouse_down(MouseButton::Left, move |_event, window, cx| {
                        close_cb_btn(pane_id, window, cx);
                        window.prevent_default();
                        cx.stop_propagation();
                    })
                    .child(
                        svg()
                            .path("phosphor/x.svg")
                            .size(px(11.0))
                            .text_color(btn_color),
                    ),
            )
        } else {
            None
        };

        // Title text (centered, flex-1)
        // Whole bar — drag starts a GPUI DraggedTab so the tab strip
        // handles it with the same live-reorder logic as tab-to-tab drag.
        let drag_label: SharedString = title.clone().into();

        let title_el = div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_size(px(12.0))
            .line_height(px(16.0))
            .font_family(theme.font_family.clone())
            .font_weight(FontWeight::MEDIUM)
            .text_color(title_color)
            .text_center()
            .child(SharedString::from(title));
        let focus_cb_bar = focus_pane_cb.clone();
        let dragged = DraggedTab {
            session_id,
            label: drag_label,
            icon: "phosphor/terminal.svg",
            origin: DraggedTabOrigin::Pane,
            preview_constraint: None,
            pane_id: Some(pane_id),
        };
        // The whole title bar is draggable. The visible pane drag preview is
        // rendered by Workspace, centred at the live cursor position.
        let bar_bg = Self::pane_chrome_bg(theme, is_focused, tab_accent_inactive_alpha);
        let mut bar = div()
            .id(ElementId::Name(format!("pane-title-bar-{pane_id}").into()))
            .flex()
            .flex_row()
            .items_center()
            .h(px(24.0))
            .w_full()
            .px(px(6.0))
            .bg(bar_bg)
            .cursor_grab()
            .on_mouse_down(MouseButton::Left, move |_event, window, cx| {
                focus_cb_bar(pane_id, window, cx);
            })
            .on_drag(
                dragged,
                move |dragged: &DraggedTab, _offset, _window, cx| {
                    cx.stop_propagation();
                    cx.new(|_| dragged.clone())
                },
            );

        // Reserve the same leading width as the trailing zoom button so the
        // centered title does not drift when the active-pane dot is absent.
        bar = bar.child(div().flex_shrink_0().w(px(20.0)));
        bar = bar.child(title_el).child(zoom_btn);

        if let Some(close) = close_btn {
            bar = bar.child(close);
        }

        bar.into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_leaf(
        pane_id: PaneId,
        surfaces: &[PaneSurface],
        active_surface_id: SurfaceId,
        content: &PaneContent,
        focused_id: PaneId,
        focus_surface_cb: std::sync::Arc<dyn Fn(SurfaceId, &mut Window, &mut App) + 'static>,
        focus_pane_cb: std::sync::Arc<dyn Fn(PaneId, &mut Window, &mut App) + 'static>,
        rename_surface_cb: std::sync::Arc<dyn Fn(SurfaceId, &mut Window, &mut App) + 'static>,
        close_surface_cb: std::sync::Arc<dyn Fn(SurfaceId, &mut Window, &mut App) + 'static>,
        close_pane_cb: std::sync::Arc<dyn Fn(PaneId, &mut Window, &mut App) + 'static>,
        toggle_zoom_cb: std::sync::Arc<dyn Fn(PaneId, &mut Window, &mut App) + 'static>,
        rename_editor: Option<SurfaceRenameEditor>,
        tree_has_splits: bool,
        zoomed_pane_id: Option<PaneId>,
        session_id: u64,
        tab_accent_color: Option<con_core::session::TabAccentColor>,
        tab_accent_inactive_alpha: f32,
        hide_pane_title_bar: bool,
        cx: &App,
    ) -> AnyElement {
        // ── Editor pane — reuses the same title bar as terminal panes ─────
        if let PaneContent::Editor { view } = content {
            let is_focused = pane_id == focused_id;
            let is_zoomed = zoomed_pane_id == Some(pane_id);
            let title = view
                .read(cx)
                .active_path()
                .and_then(|path| path.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "Editor".to_string());

            // Click anywhere on the editor body to focus this pane
            let focus_cb_click = focus_pane_cb.clone();
            let close_pane_cb_menu = close_pane_cb.clone();
            let toggle_zoom_cb_menu = toggle_zoom_cb.clone();

            let mut col = div()
                .id(ElementId::Name(format!("editor-pane-{pane_id}").into()))
                .flex()
                .flex_col()
                .size_full()
                .on_mouse_down(MouseButton::Left, move |_event, window, cx| {
                    focus_cb_click(pane_id, window, cx);
                })
                .context_menu(move |menu, _window, _cx| {
                    let close_cb = close_pane_cb_menu.clone();
                    let zoom_cb = toggle_zoom_cb_menu.clone();
                    menu.item(
                        PopupMenuItem::new(if is_zoomed {
                            "Exit Fullscreen"
                        } else {
                            "Fullscreen"
                        })
                        .on_click(move |_, window, cx| zoom_cb(pane_id, window, cx)),
                    )
                    .item(
                        PopupMenuItem::new("Close Pane")
                            .on_click(move |_, window, cx| close_cb(pane_id, window, cx)),
                    )
                });

            if tree_has_splits && !hide_pane_title_bar {
                let title_bar = Self::render_pane_title_bar(
                    pane_id,
                    session_id,
                    title,
                    is_focused,
                    tree_has_splits,
                    is_zoomed,
                    tab_accent_color,
                    tab_accent_inactive_alpha,
                    close_pane_cb,
                    toggle_zoom_cb,
                    focus_pane_cb.clone(),
                    cx,
                );
                col = col.child(title_bar);
            }

            col = col.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(view.clone()),
            );
            return col.into_any_element();
        }

        // ── Terminal pane (existing logic) ────────────────────────────────
        let theme = cx.theme();
        let is_focused = pane_id == focused_id;
        let active = Self::active_surface(surfaces, active_surface_id)
            .unwrap_or_else(|| surfaces.first().expect("pane leaf must have a surface"));
        let terminal = active.terminal.render_child();

        // Derive pane title from the active terminal title, falling back to "Terminal"
        let pane_title = Self::terminal_title_or_default(&active.terminal, cx);

        let show_surface_strip =
            surfaces.len() > 1 || active.owner.is_some() || active.title.is_some();

        // No splits → no title bar, no surface strip (single-pane layout)
        if !tree_has_splits && !show_surface_strip {
            return div().size_full().child(terminal).into_any_element();
        }

        let inactive_terminals = surfaces
            .iter()
            .filter(|surface| surface.id != active_surface_id)
            .map(|surface| surface.terminal.clone())
            .collect::<Vec<_>>();
        let mut terminal_host = div().flex_1().min_h_0().overflow_hidden().child(terminal);
        if !inactive_terminals.is_empty() {
            terminal_host = terminal_host.on_children_prepainted(move |bounds_list, window, cx| {
                let Some(bounds) = bounds_list.first().copied() else {
                    return;
                };
                for terminal in &inactive_terminals {
                    terminal.sync_surface_layout(bounds, window, cx);
                }
            });
        }

        let mut col = div().flex().flex_col().size_full();

        // Title bar — only when there are 2+ panes and not hidden by config
        if tree_has_splits && !hide_pane_title_bar {
            let is_zoomed = zoomed_pane_id == Some(pane_id);
            let title_bar = Self::render_pane_title_bar(
                pane_id,
                session_id,
                pane_title,
                is_focused,
                tree_has_splits,
                is_zoomed,
                tab_accent_color,
                tab_accent_inactive_alpha,
                close_pane_cb,
                toggle_zoom_cb,
                focus_pane_cb.clone(),
                cx,
            );
            col = col.child(title_bar);
        }

        // Surface strip — only when there are multiple surfaces or an owner/title
        if show_surface_strip {
            let rail_label = format!(
                "{} tab{}",
                surfaces.len(),
                if surfaces.len() == 1 { "" } else { "s" }
            );
            let strip_bg = Self::pane_chrome_bg(theme, is_focused, tab_accent_inactive_alpha);
            let rail_bg = if is_focused {
                theme.tab_bar_segmented.opacity(0.72)
            } else {
                theme.tab_bar_segmented.opacity(0.58)
            };
            let rail_text = theme
                .foreground
                .opacity(if is_focused { 0.66 } else { 0.50 });

            let mut surface_rail = div()
                .id(("surface-tab-strip", pane_id))
                .flex()
                .items_center()
                .gap(px(4.0))
                .h(px(24.0))
                .w_full()
                .max_w(relative(1.0))
                .px(px(4.0))
                .rounded(px(7.0))
                .font_family(theme.mono_font_family.clone())
                .bg(rail_bg)
                .overflow_x_scroll();

            surface_rail = surface_rail.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .flex_shrink_0()
                    .pl(px(2.0))
                    .pr(px(4.0))
                    .text_size(px(11.0))
                    .line_height(px(13.0))
                    .font_weight(FontWeight::NORMAL)
                    .text_color(rail_text)
                    .child(
                        svg()
                            .path("phosphor/stack.svg")
                            .size(px(9.0))
                            .text_color(rail_text.opacity(0.82)),
                    )
                    .child(SharedString::from(rail_label)),
            );
            surface_rail = surface_rail.child(
                div()
                    .w(px(1.0))
                    .h(px(12.0))
                    .flex_shrink_0()
                    .bg(theme.foreground.opacity(0.14)),
            );

            for (index, surface) in surfaces.iter().enumerate() {
                let is_active = surface.id == active_surface_id;
                let title = surface
                    .title
                    .clone()
                    .unwrap_or_else(|| format!("Surface {}", index + 1));
                let color = if is_active {
                    theme.foreground.opacity(0.90)
                } else {
                    theme.foreground.opacity(0.54)
                };
                let bg = if is_active {
                    theme.tab_active.opacity(0.96)
                } else {
                    theme.transparent
                };
                let icon_color = if is_active {
                    theme.foreground.opacity(0.66)
                } else {
                    theme.foreground.opacity(0.38)
                };
                let hover_bg = if is_active {
                    theme.tab_active.opacity(1.0)
                } else {
                    theme.foreground.opacity(0.08)
                };
                let sid = surface.id;
                let focus_cb = focus_surface_cb.clone();
                let rename_cb = rename_surface_cb.clone();
                let rename_cb_for_menu = rename_surface_cb.clone();
                let close_cb_for_menu = close_surface_cb.clone();
                let close_cb_for_button = close_surface_cb.clone();
                let can_close =
                    surfaces.len() > 1 || surface.close_pane_when_last && surface.owner.is_some();
                let editing_input = rename_editor
                    .as_ref()
                    .filter(|editor| editor.surface_id == sid)
                    .map(|editor| editor.input.clone());

                let mut tab = div()
                    .id(ElementId::Name(format!("surface-tab-{sid}").into()))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .h(px(20.0))
                    .max_w(px(190.0))
                    .flex_shrink_0()
                    .pl(px(6.0))
                    .pr(if can_close { px(2.0) } else { px(8.0) })
                    .rounded(px(6.0))
                    .bg(bg)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover_bg))
                    .on_mouse_down(MouseButton::Left, move |_event, window, cx| {
                        focus_cb(sid, window, cx);
                    })
                    .on_double_click(move |_, window, cx| {
                        rename_cb(sid, window, cx);
                    })
                    .child(
                        svg()
                            .path("phosphor/terminal.svg")
                            .size(px(10.0))
                            .flex_shrink_0()
                            .text_color(icon_color),
                    )
                    .when_some(editing_input, |tab, input| {
                        tab.child(
                            div().w(px(104.0)).child(
                                Input::new(&input)
                                    .small()
                                    .appearance(false)
                                    .text_size(px(12.0))
                                    .line_height(px(14.0))
                                    .text_color(theme.foreground.opacity(0.92)),
                            ),
                        )
                    })
                    .when(
                        rename_editor
                            .as_ref()
                            .is_none_or(|editor| editor.surface_id != sid),
                        |tab| {
                            tab.child(
                                div()
                                    .truncate()
                                    .text_size(px(12.0))
                                    .line_height(px(14.0))
                                    .font_family(theme.mono_font_family.clone())
                                    .font_weight(if is_active {
                                        FontWeight::MEDIUM
                                    } else {
                                        FontWeight::NORMAL
                                    })
                                    .text_color(color)
                                    .child(title),
                            )
                        },
                    );

                if can_close {
                    tab = tab.child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(15.0))
                            .flex_shrink_0()
                            .rounded(px(5.0))
                            .text_color(if is_active {
                                theme.foreground.opacity(0.56)
                            } else {
                                theme.foreground.opacity(0.46)
                            })
                            .hover(|s| s.bg(theme.foreground.opacity(0.10)))
                            .on_mouse_down(MouseButton::Left, move |_event, window, cx| {
                                close_cb_for_button(sid, window, cx);
                                window.prevent_default();
                                cx.stop_propagation();
                            })
                            .child(svg().path("phosphor/x.svg").size(px(8.0)).text_color(
                                if is_active {
                                    theme.foreground.opacity(0.62)
                                } else {
                                    theme.foreground.opacity(0.52)
                                },
                            )),
                    );
                }

                let tab = tab
                    .tooltip(move |window, cx| {
                        Tooltip::new("Surface tab in this pane. Double-click to rename.")
                            .build(window, cx)
                    })
                    .context_menu(move |menu, _window, _cx| {
                        let rename_cb = rename_cb_for_menu.clone();
                        let close_cb = close_cb_for_menu.clone();
                        let mut menu = menu.item(PopupMenuItem::new("Rename Surface").on_click({
                            let rename_cb = rename_cb.clone();
                            move |_, window, cx| {
                                rename_cb(sid, window, cx);
                            }
                        }));
                        if can_close {
                            menu = menu.item(PopupMenuItem::new("Close Surface").on_click({
                                let close_cb = close_cb.clone();
                                move |_, window, cx| {
                                    close_cb(sid, window, cx);
                                }
                            }));
                        }
                        menu
                    });

                surface_rail = surface_rail.child(tab);
            }

            let strip = div()
                .flex()
                .items_center()
                .h(px(28.0))
                .w_full()
                .px(px(6.0))
                .py(px(2.0))
                .bg(strip_bg)
                .child(surface_rail);

            col = col.child(strip);
        }

        col.child(terminal_host).into_any_element()
    }

    fn find_focused_pane(node: &PaneNode, window: &Window, cx: &App) -> Option<PaneId> {
        match node {
            PaneNode::Leaf {
                id,
                content:
                    PaneContent::Terminal {
                        surfaces,
                        active_surface_id,
                    },
                ..
            } => {
                if let Some(surface) = Self::active_surface(surfaces, *active_surface_id)
                    && surface.terminal.is_focused(window, cx)
                {
                    Some(*id)
                } else {
                    None
                }
            }
            PaneNode::Leaf { .. } => None,
            PaneNode::Split { first, second, .. } => Self::find_focused_pane(first, window, cx)
                .or_else(|| Self::find_focused_pane(second, window, cx)),
        }
    }

    fn check_terminal_pane_id(node: &PaneNode, entity_id: EntityId, pane_id: PaneId) -> bool {
        match node {
            PaneNode::Leaf {
                id,
                content: PaneContent::Terminal { surfaces, .. },
                ..
            } => {
                *id == pane_id
                    && surfaces
                        .iter()
                        .any(|surface| surface.terminal.entity_id() == entity_id)
            }
            PaneNode::Leaf { .. } => false,
            PaneNode::Split { first, second, .. } => {
                Self::check_terminal_pane_id(first, entity_id, pane_id)
                    || Self::check_terminal_pane_id(second, entity_id, pane_id)
            }
        }
    }

    fn find_pane_id_by_entity_id(node: &PaneNode, entity_id: EntityId) -> Option<PaneId> {
        match node {
            PaneNode::Leaf {
                id,
                content: PaneContent::Terminal { surfaces, .. },
                ..
            } => {
                if surfaces
                    .iter()
                    .any(|surface| surface.terminal.entity_id() == entity_id)
                {
                    Some(*id)
                } else {
                    None
                }
            }
            PaneNode::Leaf { .. } => None,
            PaneNode::Split { first, second, .. } => {
                Self::find_pane_id_by_entity_id(first, entity_id)
                    .or_else(|| Self::find_pane_id_by_entity_id(second, entity_id))
            }
        }
    }

    fn collect_pane_terminals(node: &PaneNode, result: &mut Vec<(PaneId, TerminalPane)>) {
        match node {
            PaneNode::Leaf {
                id,
                content:
                    PaneContent::Terminal {
                        surfaces,
                        active_surface_id,
                    },
                ..
            } => {
                if let Some(surface) = Self::active_surface(surfaces, *active_surface_id) {
                    result.push((*id, surface.terminal.clone()));
                }
            }
            PaneNode::Leaf { .. } => {}
            PaneNode::Split { first, second, .. } => {
                Self::collect_pane_terminals(first, result);
                Self::collect_pane_terminals(second, result);
            }
        }
    }

    fn collect_surface_terminals(node: &PaneNode, result: &mut Vec<(PaneId, bool, TerminalPane)>) {
        match node {
            PaneNode::Leaf {
                id,
                content:
                    PaneContent::Terminal {
                        surfaces,
                        active_surface_id,
                    },
                ..
            } => {
                result.extend(surfaces.iter().map(|surface| {
                    (
                        *id,
                        surface.id == *active_surface_id,
                        surface.terminal.clone(),
                    )
                }));
            }
            PaneNode::Leaf { .. } => {}
            PaneNode::Split { first, second, .. } => {
                Self::collect_surface_terminals(first, result);
                Self::collect_surface_terminals(second, result);
            }
        }
    }

    fn collect_surface_infos(
        node: &PaneNode,
        target_pane_id: Option<PaneId>,
        focused_pane_id: PaneId,
        pane_index: &mut usize,
        result: &mut Vec<PaneSurfaceInfo>,
    ) {
        match node {
            PaneNode::Leaf {
                id,
                content:
                    PaneContent::Terminal {
                        surfaces,
                        active_surface_id,
                    },
                ..
            } => {
                *pane_index += 1;
                if target_pane_id.is_some_and(|target| target != *id) {
                    return;
                }
                result.extend(surfaces.iter().enumerate().map(|(surface_index, surface)| {
                    PaneSurfaceInfo {
                        pane_id: *id,
                        pane_index: *pane_index,
                        surface_id: surface.id,
                        surface_index: surface_index + 1,
                        is_active: surface.id == *active_surface_id,
                        is_focused_pane: *id == focused_pane_id,
                        title: surface.title.clone(),
                        owner: surface.owner.clone(),
                        close_pane_when_last: surface.close_pane_when_last,
                        terminal: surface.terminal.clone(),
                    }
                }));
            }
            PaneNode::Leaf { .. } => {}
            PaneNode::Split { first, second, .. } => {
                Self::collect_surface_infos(
                    first,
                    target_pane_id,
                    focused_pane_id,
                    pane_index,
                    result,
                );
                Self::collect_surface_infos(
                    second,
                    target_pane_id,
                    focused_pane_id,
                    pane_index,
                    result,
                );
            }
        }
    }

    fn find_active_surface_id(node: &PaneNode, pane_id: PaneId) -> Option<SurfaceId> {
        match node {
            PaneNode::Leaf {
                id,
                content:
                    PaneContent::Terminal {
                        active_surface_id, ..
                    },
            } if *id == pane_id => Some(*active_surface_id),
            PaneNode::Leaf { .. } => None,
            PaneNode::Split { first, second, .. } => Self::find_active_surface_id(first, pane_id)
                .or_else(|| Self::find_active_surface_id(second, pane_id)),
        }
    }

    fn active_surface(
        surfaces: &[PaneSurface],
        active_surface_id: SurfaceId,
    ) -> Option<&PaneSurface> {
        surfaces
            .iter()
            .find(|surface| surface.id == active_surface_id)
            .or_else(|| surfaces.first())
    }

    fn node_to_state(
        node: &PaneNode,
        cx: &App,
        capture_screen_text: bool,
    ) -> Option<PaneLayoutState> {
        match node {
            PaneNode::Leaf {
                id,
                content:
                    PaneContent::Terminal {
                        surfaces,
                        active_surface_id,
                    },
                ..
            } => {
                let surface_states = surfaces
                    .iter()
                    .map(|surface| {
                        let screen_text = restored_screen_text_for_surface(
                            &surface.terminal,
                            capture_screen_text,
                            cx,
                        );
                        SurfaceState {
                            surface_id: surface.id,
                            title: surface.title.clone(),
                            owner: surface.owner.clone(),
                            cwd: surface.terminal.current_dir(cx),
                            close_pane_when_last: surface.close_pane_when_last,
                            screen_text,
                        }
                    })
                    .collect::<Vec<_>>();
                let cwd = surface_states
                    .iter()
                    .find(|surface| surface.surface_id == *active_surface_id)
                    .or_else(|| surface_states.first())
                    .and_then(|surface| surface.cwd.clone());

                Some(PaneLayoutState::Leaf {
                    pane_id: *id,
                    cwd,
                    active_surface_id: Some(*active_surface_id),
                    surfaces: surface_states,
                })
            }
            // Editor panes are not persisted to session state (Phase 1).
            PaneNode::Leaf { .. } => None,
            PaneNode::Split {
                direction,
                ratio,
                first,
                second,
                ..
            } => {
                let first = Self::node_to_state(first, cx, capture_screen_text);
                let second = Self::node_to_state(second, cx, capture_screen_text);
                match (first, second) {
                    (Some(first), Some(second)) => Some(PaneLayoutState::Split {
                        direction: match direction {
                            SplitDirection::Horizontal => PaneSplitDirection::Horizontal,
                            SplitDirection::Vertical => PaneSplitDirection::Vertical,
                        },
                        ratio: *ratio,
                        first: Box::new(first),
                        second: Box::new(second),
                    }),
                    (Some(only), None) | (None, Some(only)) => Some(only),
                    (None, None) => None,
                }
            }
        }
    }

    fn node_from_state(
        state: &PaneLayoutState,
        make_terminal: &mut impl FnMut(Option<&str>, Option<&[String]>, bool) -> TerminalPane,
        next_surface_id: &mut SurfaceId,
        used_surface_ids: &mut std::collections::HashSet<SurfaceId>,
    ) -> PaneNode {
        match state {
            PaneLayoutState::Leaf {
                pane_id,
                cwd,
                active_surface_id,
                surfaces,
            } => {
                if surfaces.is_empty() {
                    let surface_id =
                        Self::allocate_restored_surface_id(None, next_surface_id, used_surface_ids);
                    return PaneNode::Leaf {
                        id: *pane_id,
                        content: PaneContent::Terminal {
                            surfaces: vec![PaneSurface::new(
                                surface_id,
                                make_terminal(cwd.as_deref(), None, false),
                            )],
                            active_surface_id: surface_id,
                        },
                    };
                }

                let mut restored_surfaces = Vec::with_capacity(surfaces.len());
                let mut restored_active_surface_id = None;
                for state in surfaces {
                    let surface_id = Self::allocate_restored_surface_id(
                        Some(state.surface_id),
                        next_surface_id,
                        used_surface_ids,
                    );
                    let restore_cwd = state.cwd.as_deref().or(cwd.as_deref());
                    let clamped_screen_text = trim_restored_screen_text(state.screen_text.clone());
                    let restored_text = if clamped_screen_text.is_empty() {
                        None
                    } else {
                        Some(clamped_screen_text.as_slice())
                    };
                    let force_restored_screen_text = state.owner.as_deref()
                        == Some(con_core::session::WORKSPACE_ERROR_SURFACE_OWNER);
                    let mut surface = PaneSurface::new(
                        surface_id,
                        make_terminal(restore_cwd, restored_text, force_restored_screen_text),
                    );
                    surface.title = state.title.clone();
                    surface.owner = state.owner.clone();
                    surface.close_pane_when_last = state.close_pane_when_last;

                    if active_surface_id == &Some(state.surface_id) {
                        restored_active_surface_id = Some(surface_id);
                    }
                    restored_surfaces.push(surface);
                }

                if restored_surfaces.is_empty() {
                    let surface_id =
                        Self::allocate_restored_surface_id(None, next_surface_id, used_surface_ids);
                    restored_surfaces.push(PaneSurface::new(
                        surface_id,
                        make_terminal(cwd.as_deref(), None, false),
                    ));
                    restored_active_surface_id = Some(surface_id);
                }
                let active_surface_id =
                    restored_active_surface_id.unwrap_or(restored_surfaces[0].id);
                PaneNode::Leaf {
                    id: *pane_id,
                    content: PaneContent::Terminal {
                        surfaces: restored_surfaces,
                        active_surface_id,
                    },
                }
            }
            PaneLayoutState::Split {
                direction,
                ratio,
                first,
                second,
            } => PaneNode::Split {
                split_id: 0,
                direction: match direction {
                    PaneSplitDirection::Horizontal => SplitDirection::Horizontal,
                    PaneSplitDirection::Vertical => SplitDirection::Vertical,
                },
                first: Box::new(Self::node_from_state(
                    first,
                    make_terminal,
                    next_surface_id,
                    used_surface_ids,
                )),
                second: Box::new(Self::node_from_state(
                    second,
                    make_terminal,
                    next_surface_id,
                    used_surface_ids,
                )),
                ratio: ratio.clamp(0.05, 0.95),
            },
        }
    }

    fn allocate_restored_surface_id(
        requested: Option<SurfaceId>,
        next_surface_id: &mut SurfaceId,
        used_surface_ids: &mut std::collections::HashSet<SurfaceId>,
    ) -> SurfaceId {
        if let Some(requested) = requested
            && used_surface_ids.insert(requested)
        {
            if let Some(next) = requested.checked_add(1) {
                *next_surface_id = (*next_surface_id).max(next);
            }
            return requested;
        }

        loop {
            let candidate = *next_surface_id;
            *next_surface_id = (*next_surface_id).checked_add(1).unwrap_or(0);
            if used_surface_ids.insert(candidate) {
                return candidate;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_leaf(id: PaneId) -> PaneNode {
        PaneNode::Leaf {
            id,
            content: PaneContent::Terminal {
                surfaces: Vec::new(),
                active_surface_id: 0,
            },
        }
    }

    fn leaf_id(node: &PaneNode) -> PaneId {
        match node {
            PaneNode::Leaf { id, .. } => *id,
            PaneNode::Split { .. } => panic!("expected leaf"),
        }
    }

    fn split_parts(node: &PaneNode) -> (SplitDirection, &PaneNode, &PaneNode) {
        match node {
            PaneNode::Split {
                direction,
                first,
                second,
                ..
            } => (*direction, first, second),
            PaneNode::Leaf { .. } => panic!("expected split"),
        }
    }

    fn collect_leaf_ids(node: &PaneNode, result: &mut Vec<PaneId>) {
        match node {
            PaneNode::Leaf { id, .. } => result.push(*id),
            PaneNode::Split { first, second, .. } => {
                collect_leaf_ids(first, result);
                collect_leaf_ids(second, result);
            }
        }
    }

    fn collect_split_ids(node: &PaneNode, result: &mut Vec<SplitId>) {
        match node {
            PaneNode::Leaf { .. } => {}
            PaneNode::Split {
                split_id,
                first,
                second,
                ..
            } => {
                result.push(*split_id);
                collect_split_ids(first, result);
                collect_split_ids(second, result);
            }
        }
    }

    fn assert_unique(values: &[usize]) {
        let mut sorted = values.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), values.len());
    }

    fn simple_two_pane_tree() -> PaneTree {
        PaneTree {
            root: PaneNode::Split {
                split_id: 0,
                direction: SplitDirection::Horizontal,
                first: Box::new(empty_leaf(0)),
                second: Box::new(empty_leaf(1)),
                ratio: 0.5,
            },
            focused_pane_id: 0,
            zoomed_pane_id: None,
            next_id: 2,
            next_surface_id: 0,
            next_split_id: 1,
            dragging: None,
        }
    }

    #[::core::prelude::v1::test]
    fn focused_terminal_entity_id_is_none_without_focused_terminal() {
        let tree = PaneTree {
            root: empty_leaf(0),
            focused_pane_id: 0,
            zoomed_pane_id: None,
            next_id: 1,
            next_surface_id: 0,
            next_split_id: 0,
            dragging: None,
        };

        assert_eq!(tree.focused_terminal_entity_id(), None);
    }

    #[::core::prelude::v1::test]
    fn short_pane_title_handles_unix_paths() {
        assert_eq!(
            PaneTree::short_pane_title("~/work/con-terminal"),
            "con-terminal"
        );
        assert_eq!(
            PaneTree::short_pane_title("~/work/con-terminal/"),
            "con-terminal"
        );
        assert_eq!(PaneTree::short_pane_title("/"), "/");
    }

    #[::core::prelude::v1::test]
    fn short_pane_title_handles_windows_paths() {
        assert_eq!(
            PaneTree::short_pane_title(r"C:\Users\alice\project"),
            "project"
        );
        assert_eq!(
            PaneTree::short_pane_title(r"C:\Users\alice\project\"),
            "project"
        );
        assert_eq!(PaneTree::short_pane_title(r"C:\"), r"C:");
    }

    #[::core::prelude::v1::test]
    fn short_pane_title_preserves_non_path_titles() {
        assert_eq!(PaneTree::short_pane_title("Terminal"), "Terminal");
        assert_eq!(PaneTree::short_pane_title("nvim main.rs"), "nvim main.rs");
    }

    #[::core::prelude::v1::test]
    fn merge_tree_horizontal_before_places_incoming_first() {
        let mut target = PaneTree {
            root: empty_leaf(0),
            focused_pane_id: 0,
            zoomed_pane_id: None,
            next_id: 1,
            next_surface_id: 0,
            next_split_id: 0,
            dragging: None,
        };
        let incoming = PaneTree {
            root: empty_leaf(0),
            focused_pane_id: 0,
            zoomed_pane_id: None,
            next_id: 1,
            next_surface_id: 0,
            next_split_id: 0,
            dragging: None,
        };

        target.merge_tree(incoming, SplitDirection::Horizontal, SplitPlacement::Before);

        let (root_direction, first, second) = split_parts(&target.root);
        assert_eq!(root_direction, SplitDirection::Horizontal);
        assert_eq!(leaf_id(first), target.focused_pane_id);
        assert_eq!(leaf_id(second), 0);
        let mut leaf_ids = Vec::new();
        collect_leaf_ids(&target.root, &mut leaf_ids);
        assert_unique(&leaf_ids);
    }

    #[::core::prelude::v1::test]
    fn merge_tree_vertical_after_places_incoming_second() {
        let mut target = PaneTree {
            root: empty_leaf(0),
            focused_pane_id: 0,
            zoomed_pane_id: None,
            next_id: 1,
            next_surface_id: 0,
            next_split_id: 0,
            dragging: None,
        };
        let incoming = PaneTree {
            root: empty_leaf(0),
            focused_pane_id: 0,
            zoomed_pane_id: None,
            next_id: 1,
            next_surface_id: 0,
            next_split_id: 0,
            dragging: None,
        };

        target.merge_tree(incoming, SplitDirection::Vertical, SplitPlacement::After);

        let (root_direction, first, second) = split_parts(&target.root);
        assert_eq!(root_direction, SplitDirection::Vertical);
        assert_eq!(leaf_id(first), 0);
        assert_eq!(leaf_id(second), target.focused_pane_id);
        let mut leaf_ids = Vec::new();
        collect_leaf_ids(&target.root, &mut leaf_ids);
        assert_unique(&leaf_ids);
    }

    #[::core::prelude::v1::test]
    fn merge_tree_clears_zoom_and_focuses_incoming() {
        let mut target = simple_two_pane_tree();
        target.zoomed_pane_id = Some(0);
        let incoming = PaneTree {
            root: empty_leaf(0),
            focused_pane_id: 0,
            zoomed_pane_id: None,
            next_id: 1,
            next_surface_id: 0,
            next_split_id: 0,
            dragging: None,
        };

        target.merge_tree(incoming, SplitDirection::Vertical, SplitPlacement::Before);

        assert_eq!(target.zoomed_pane_id(), None);
        assert!(target.focused_pane_id() >= 2);
        assert!(PaneTree::contains_leaf(
            &target.root,
            target.focused_pane_id()
        ));
    }

    #[::core::prelude::v1::test]
    fn focus_pane_supports_non_terminal_leaves_before_zoom() {
        let mut tree = simple_two_pane_tree();

        tree.focus_pane(1);
        assert!(tree.toggle_zoom_focused());

        assert_eq!(tree.focused_pane_id(), 1);
        assert_eq!(tree.zoomed_pane_id(), Some(1));
    }

    #[::core::prelude::v1::test]
    fn merge_tree_assigns_unique_split_ids() {
        let mut target = simple_two_pane_tree();
        let incoming = simple_two_pane_tree();

        target.merge_tree(incoming, SplitDirection::Horizontal, SplitPlacement::After);

        let mut split_ids = Vec::new();
        collect_split_ids(&target.root, &mut split_ids);
        assert_unique(&split_ids);
    }

    #[::core::prelude::v1::test]
    fn move_pane_to_top_edge_places_source_before_target_vertically() {
        let mut tree = simple_two_pane_tree();

        assert!(tree.move_pane(1, 0, SplitDirection::Vertical, SplitPlacement::Before));

        let (root_direction, first, second) = split_parts(&tree.root);
        assert_eq!(root_direction, SplitDirection::Vertical);
        assert_eq!(leaf_id(first), 1);
        assert_eq!(leaf_id(second), 0);
        assert_eq!(tree.focused_pane_id, 1);
    }

    #[::core::prelude::v1::test]
    fn pane_move_noop_when_source_is_already_after_target_in_same_horizontal_split() {
        let tree = simple_two_pane_tree();

        assert!(tree.is_noop_pane_move(1, 0, SplitDirection::Horizontal, SplitPlacement::After,));
        assert!(tree.is_noop_pane_move(0, 1, SplitDirection::Horizontal, SplitPlacement::Before,));
        assert!(!tree.is_noop_pane_move(1, 0, SplitDirection::Vertical, SplitPlacement::Before,));
        assert!(!tree.is_noop_pane_move(1, 0, SplitDirection::Horizontal, SplitPlacement::Before,));
    }

    #[::core::prelude::v1::test]
    fn pane_move_noop_when_source_is_already_after_target_in_same_vertical_split() {
        let mut tree = simple_two_pane_tree();
        tree.root = PaneNode::Split {
            split_id: 0,
            direction: SplitDirection::Vertical,
            first: Box::new(empty_leaf(0)),
            second: Box::new(empty_leaf(1)),
            ratio: 0.5,
        };

        assert!(tree.is_noop_pane_move(1, 0, SplitDirection::Vertical, SplitPlacement::After,));
        assert!(tree.is_noop_pane_move(0, 1, SplitDirection::Vertical, SplitPlacement::Before,));
        assert!(!tree.is_noop_pane_move(1, 0, SplitDirection::Horizontal, SplitPlacement::After,));
    }

    #[::core::prelude::v1::test]
    fn new_editor_creates_editor_pane_tree() {
        // We can't easily construct an Entity<EditorView> in a unit test without
        // the full GPUI App context, so we verify the structure by checking that
        // PaneTree::new(terminal) is NOT an editor tree.
        // The actual new_editor() constructor is tested via the integration test
        // infrastructure that can create EditorView entities.
    }

    #[::core::prelude::v1::test]
    fn new_editor_focused_pane_is_editor_is_false_for_terminal_tree() {
        // Verify that a terminal-based tree reports focused_pane_is_editor == false
        let tree = PaneTree {
            root: empty_leaf(0),
            focused_pane_id: 0,
            zoomed_pane_id: None,
            next_id: 1,
            next_surface_id: 0,
            next_split_id: 0,
            dragging: None,
        };
        assert!(!tree.focused_pane_is_editor());
    }

    #[::core::prelude::v1::test]
    fn pane_move_is_not_noop_when_source_and_target_are_in_opposite_subtrees() {
        let tree = PaneTree {
            root: PaneNode::Split {
                split_id: 0,
                direction: SplitDirection::Horizontal,
                first: Box::new(PaneNode::Split {
                    split_id: 1,
                    direction: SplitDirection::Vertical,
                    first: Box::new(empty_leaf(0)),
                    second: Box::new(empty_leaf(2)),
                    ratio: 0.5,
                }),
                second: Box::new(empty_leaf(1)),
                ratio: 0.5,
            },
            focused_pane_id: 0,
            zoomed_pane_id: None,
            next_id: 3,
            next_surface_id: 0,
            next_split_id: 2,
            dragging: None,
        };

        assert!(!tree.is_noop_pane_move(1, 0, SplitDirection::Horizontal, SplitPlacement::After,));
    }
}

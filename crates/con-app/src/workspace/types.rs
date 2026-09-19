use super::*;

/// Drag state for pane title bar drag-to-tab promotion.
#[derive(Clone)]
pub(super) struct PaneTitleDragState {
    pub(super) title: SharedString,
    pub(super) current_pos: Point<Pixels>,
    /// True while a pane title drag is active.
    pub(super) active: bool,
    pub(super) target: Option<PaneDropTarget>,
}

pub(super) fn pane_title_drag_to_tab_active(drag: Option<&PaneTitleDragState>) -> bool {
    drag.is_some_and(|drag| drag.active)
}

/// Workspace-owned drag preview state for horizontal tab-strip drags.
/// GPUI's built-in active-drag preview does not reliably honor relative
/// `.top/.left` constraints, so horizontal tab drags hide that preview and
/// render this overlay from the workspace root instead.
#[derive(Clone)]
pub(super) struct TabDragPreviewState {
    pub(super) title: SharedString,
    pub(super) icon: &'static str,
    pub(super) source_top: Pixels,
    pub(super) cursor_offset_x: Pixels,
}

pub(super) fn tab_drag_overlay_origin(
    mouse: Point<Pixels>,
    preview: &TabDragPreviewState,
    min_left: Pixels,
    max_left: Pixels,
) -> Point<Pixels> {
    point(
        (mouse.x - preview.cursor_offset_x).clamp(min_left, max_left),
        preview.source_top,
    )
}

pub(super) fn tab_drag_overlay_probe_position(
    mouse: Point<Pixels>,
    preview: &TabDragPreviewState,
    preview_size: Size<Pixels>,
    min_left: Pixels,
    max_left: Pixels,
) -> Point<Pixels> {
    let origin = tab_drag_overlay_origin(mouse, preview, min_left, max_left);
    point(
        origin.x + preview_size.width / 2.0,
        origin.y + preview_size.height / 2.0,
    )
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum PaneDropTarget {
    NewTab { slot: usize },
    Split(PaneSplitDropTarget),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum TabDragTarget {
    Reorder { slot: usize },
}

pub(super) fn centered_drag_preview_origin(
    cursor: gpui::Point<gpui::Pixels>,
    preview_size: gpui::Size<gpui::Pixels>,
) -> gpui::Point<gpui::Pixels> {
    point(
        cursor.x - preview_size.width / 2.0,
        cursor.y - preview_size.height / 2.0,
    )
}

pub(super) fn clamp_preview_origin_to_tab_bar(
    origin: gpui::Point<gpui::Pixels>,
    preview_size: gpui::Size<gpui::Pixels>,
    tab_bar_top: gpui::Pixels,
    tab_bar_height: gpui::Pixels,
) -> gpui::Point<gpui::Pixels> {
    let max_top = tab_bar_top + (tab_bar_height - preview_size.height).max(px(0.0));
    point(origin.x, origin.y.clamp(tab_bar_top, max_top))
}

pub(super) fn tab_drag_preview_origin(
    cursor: gpui::Point<gpui::Pixels>,
    preview_size: gpui::Size<gpui::Pixels>,
    tab_bar_top: gpui::Pixels,
    tab_bar_height: gpui::Pixels,
) -> gpui::Point<gpui::Pixels> {
    clamp_preview_origin_to_tab_bar(
        centered_drag_preview_origin(cursor, preview_size),
        preview_size,
        tab_bar_top,
        tab_bar_height,
    )
}

pub(super) fn pane_drag_floating_preview_origin(
    cursor: gpui::Point<gpui::Pixels>,
    preview_size: gpui::Size<gpui::Pixels>,
) -> gpui::Point<gpui::Pixels> {
    point(
        cursor.x - preview_size.width / 2.0,
        cursor.y - preview_size.height / 2.0,
    )
}

pub(super) fn tab_like_drag_preview_size() -> gpui::Size<gpui::Pixels> {
    Size {
        width: px(TAB_DRAG_PREVIEW_WIDTH),
        height: px(TAB_DRAG_PREVIEW_HEIGHT),
    }
}

pub(super) fn is_tab_strip_preview_active(
    gpui_tab_drag_active: bool,
    pane_title_drag_to_tab_active: bool,
) -> bool {
    gpui_tab_drag_active || pane_title_drag_to_tab_active
}

pub(super) fn pane_title_drag_tab_slot(
    cursor: gpui::Point<gpui::Pixels>,
    tab_bounds: &[gpui::Bounds<gpui::Pixels>],
    tab_count: usize,
) -> usize {
    if tab_bounds.is_empty() {
        return 0;
    }
    for (i, bounds) in tab_bounds.iter().enumerate() {
        let mid_x = bounds.origin.x + bounds.size.width / 2.0;
        if cursor.x < mid_x {
            return i;
        }
    }
    tab_count
}

pub(super) fn remap_drop_slot_for_current_order(
    source_index: usize,
    hovered_index: usize,
) -> usize {
    if hovered_index > source_index {
        hovered_index + 1
    } else {
        hovered_index
    }
}

pub(super) fn is_dragged_tab_source(
    active_dragged_tab_session_id: Option<u64>,
    tab_session_id: u64,
) -> bool {
    active_dragged_tab_session_id == Some(tab_session_id)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SplitPreviewRegions {
    pub(super) incoming: Bounds<Pixels>,
    pub(super) existing: Bounds<Pixels>,
    pub(super) seam: Bounds<Pixels>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PaneSplitDropTarget {
    pub(super) target_pane_id: usize,
    pub(super) direction: SplitDirection,
    pub(super) placement: SplitPlacement,
    pub(super) bounds: Bounds<Pixels>,
}

/// Inline rename editor for the horizontal tab strip.
pub(super) struct TabRenameEditor {
    pub(super) tab_id: u64,
    pub(super) tab_index: usize,
    pub(super) generation: u64,
    pub(super) input: Entity<InputState>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TabRenameStateSnapshot {
    pub(super) tab_id: u64,
    pub(super) tab_index: usize,
}

pub(super) struct Tab {
    pub(super) pane_tree: PaneTree,
    pub(super) title: String,
    /// User-supplied label that overrides every auto-derived name in
    /// the panel/strip. Set via inline rename (double-click in
    /// vertical panel) or context menu. `None` means "use smart
    /// auto-derived name".
    pub(super) user_label: Option<String>,
    /// Optional accent color set via the tab context menu.
    pub(super) color: Option<con_core::session::TabAccentColor>,
    /// AI-suggested label, when the suggestion model is enabled and
    /// has produced one. Sits between `user_label` and the regex
    /// heuristic in the naming priority — never overrides an
    /// explicit user choice, but does override the heuristic when
    /// available.
    pub(super) ai_label: Option<String>,
    /// AI-suggested icon, paired with `ai_label`. When `None`, the
    /// row falls back to the heuristic icon.
    pub(super) ai_icon: Option<TabIconKind>,
    /// Cached classification of the focused terminal's visible screen
    /// as an interactive agent CLI (`"codex"` / `"claude"` /
    /// `"opencode"`). `None` until detection has run or when the
    /// screen no longer matches.
    pub(super) agent_cli: Option<&'static str>,
    /// Stable identifier for this tab across the lifetime of the
    /// window — used as the cache key in the `TabSummaryEngine` so
    /// reorders, closes, and re-opens don't collide. Allocated from
    /// `next_tab_summary_id` at tab construction time.
    pub(super) summary_id: u64,
    /// Bumped when the tab's terminal context changes in a way that
    /// invalidates in-flight AI summaries for the previous context.
    pub(super) summary_epoch: u64,
    pub(super) needs_attention: bool,
    pub(super) session: AgentSession,
    pub(super) agent_routing: AgentRoutingState,
    pub(super) panel_state: PanelState,
    pub(super) runtime_trackers: RefCell<HashMap<usize, con_agent::context::PaneRuntimeTracker>>,
    pub(super) runtime_cache: RefCell<HashMap<usize, con_agent::context::PaneRuntimeState>>,
    pub(super) shell_history: HashMap<usize, VecDeque<CommandSuggestionEntry>>,
}

#[derive(Clone)]
pub(super) struct CommandSuggestionEntry {
    pub(super) command: String,
    pub(super) cwd: Option<String>,
}

pub(super) struct SettingsWindowView {
    pub(super) panel: Entity<SettingsPanel>,
}

impl Render for SettingsWindowView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.panel.clone())
    }
}

#[cfg(target_os = "macos")]
#[repr(C)]
pub(super) struct NSOperatingSystemVersion {
    pub(super) major_version: isize,
    pub(super) minor_version: isize,
    pub(super) patch_version: isize,
}

#[derive(Clone)]
pub(super) struct ShellSuggestionResult {
    pub(super) tab_idx: usize,
    pub(super) pane_id: usize,
    pub(super) prefix: String,
    pub(super) completion: String,
}

pub(super) enum LocalPathCompletion {
    Inline(String),
    Candidates(Vec<String>),
}

#[derive(Clone)]
pub(super) struct ResolvedPaneTarget {
    pub(super) pane: TerminalPane,
    pub(super) pane_index: usize,
    pub(super) pane_id: usize,
}

#[derive(Clone)]
pub(super) struct ResolvedSurfaceTarget {
    pub(super) terminal: TerminalPane,
    pub(super) pane_index: usize,
    pub(super) pane_id: usize,
    pub(super) surface_index: usize,
    pub(super) surface_id: usize,
}

/// A deferred create-pane request waiting for a window-aware context.
pub(super) struct PendingCreatePane {
    pub(super) command: Option<String>,
    pub(super) cwd: Option<String>,
    pub(super) tab_idx: usize,
    pub(super) location: con_agent::tools::PaneCreateLocation,
    pub(super) response_tx: crossbeam_channel::Sender<con_agent::PaneResponse>,
}

pub(super) enum PendingWindowControlRequest {
    TabsNew {
        response_tx: oneshot::Sender<ControlResult>,
    },
    TabsClose {
        tab_idx: usize,
        response_tx: oneshot::Sender<ControlResult>,
    },
}

pub(super) enum PendingSurfaceControlRequest {
    Create {
        tab_idx: usize,
        pane: con_core::PaneTarget,
        title: Option<String>,
        command: Option<String>,
        owner: Option<String>,
        close_pane_when_last: bool,
        response_tx: oneshot::Sender<ControlResult>,
    },
    Split {
        tab_idx: usize,
        source: con_core::SurfaceTarget,
        location: con_agent::tools::PaneCreateLocation,
        title: Option<String>,
        command: Option<String>,
        owner: Option<String>,
        close_pane_when_last: bool,
        response_tx: oneshot::Sender<ControlResult>,
    },
    Close {
        tab_idx: usize,
        target: con_core::SurfaceTarget,
        close_empty_owned_pane: bool,
        response_tx: oneshot::Sender<ControlResult>,
    },
}

pub(super) struct PendingControlAgentRequest {
    pub(super) request_id: u64,
    pub(super) prompt: String,
    pub(super) auto_approve_tools: bool,
    pub(super) response_tx: tokio::sync::oneshot::Sender<ControlResult>,
}

pub(super) enum SessionSaveRequest {
    Save(Session, GlobalHistoryState),
    Flush(Session, GlobalHistoryState, crossbeam_channel::Sender<()>),
}
